//! Conversation view state.
//!
//! A gpui `Entity` holding one keyed row per projected transcript node. The
//! content authority is the AHP chat state: [`ConversationState::project`]
//! diffs [`crate::transcript::project`]'s nodes against the live rows and
//! updates each `Entity<MessageItem>` in place — a streaming delta re-renders
//! only its own row, a settled turn folds its segment immediately, and a
//! row's identity never shifts (rows are keyed by protocol part ids, so a
//! replayed fold lands on the row that is already on screen). Client-local
//! state lives in [`crate::transcript::Overlays`]: optimistic send bubbles,
//! workspace info notices, host refusal notices, and per-turn clock anchors.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use gpui::{App, AppContext as _, Entity, SharedString};
use manox_agent::thread::ToolCallStatus;

use crate::transcript::{ItemKey, Node, Overlays, ProjectEnv};
use crate::views::message::MessageItem;
use crate::views::tool_stream::ToolStreamSnapshot;

/// A decoded image attached to a user message, kept only for UI preview. The
/// canonical bytes live in the submitted turn's content; this holds a gpui
/// image so the user bubble can render a thumbnail without re-decoding.
#[derive(Debug, Clone)]
pub struct UserImage(pub Arc<gpui::Image>);

#[derive(Debug, Clone)]
pub struct UserTurnMeta {
    pub timestamp: i64,
    pub model_id: String,
    pub access_mode: Option<manox_agent::thread::AccessMode>,
    /// True when this user message entered the message list through a steer
    /// injection (mid-turn) rather than starting a fresh turn as a typed
    /// send. Projected rows inherit the marker from the adopting bubble.
    pub steered: bool,
    /// The agent that authored this user turn; `None` = human input and
    /// the bubble header shows the localized "You".
    pub author: Option<manox_agent::MessageAuthor>,
    /// The agent whose conversation renders this turn — the header's `to`.
    /// A view-side fact stamped by the owning `ConversationState`, never
    /// persisted: the same message shows its own recipient in the main
    /// thread, in a member thread, and in a sub-agent panel.
    pub recipient: Option<manox_agent::MessageAuthor>,
    /// Mirrors `MessageUiMetadata::peer`: the turn arrived as a peer delivery,
    /// so its bubble carries the sender's attribution and a peer accent.
    pub peer: bool,
}

impl UserTurnMeta {
    pub fn new(
        timestamp: i64,
        model_id: String,
        access_mode: Option<manox_agent::thread::AccessMode>,
    ) -> Self {
        Self {
            timestamp,
            model_id,
            access_mode,
            steered: false,
            author: None,
            recipient: None,
            peer: false,
        }
    }
}

/// A single renderable conversation item.
#[derive(Debug, Clone)]
pub enum ConvItem {
    User {
        text: String,
        images: Vec<UserImage>,
        meta: Option<UserTurnMeta>,
    },
    Assistant {
        text: String,
        streaming: bool,
        /// Per-turn token usage (input/output/cache) for the user message that
        /// opened this reply's turn, projected from the state's `turn.usage`.
        token_usage: Option<manox_agent::TokenUsage>,
        /// True when an activity segment immediately precedes this reply:
        /// the segment's header row carries the model name, so this reply
        /// renders no model row of its own (pure-text answers stay bare).
        activity_header: bool,
        /// The reply's durable journal entry id (the fork command's
        /// `through_entry_id`) when it is a forkable anchor. `None` when it is
        /// not — `fork_unavailable` then says why, so the control stays
        /// visible and explains itself instead of vanishing.
        entry_id: Option<String>,
        /// Why this reply cannot be forked, or `None` when it can. Resolved
        /// by the projection, so rendering only decides how to draw it.
        fork_unavailable: Option<crate::views::message::ForkUnavailable>,
    },
    /// One contiguous activity segment within a user turn, rendered as a
    /// segment shell: a header row carrying the model display name plus the
    /// clickable cover (chevron + per-kind counts "Read×7 · 思考×8", elapsed,
    /// failure/approval badges) over the segment's entries. A segment is a
    /// maximal run of consecutive reasoning/tool-call parts — the runtime
    /// projection closes text/reasoning slots at every tool call, so part
    /// boundaries are the segment boundaries and the projection never has to
    /// guess them from arrival rhythm. Collapsed shows only the header;
    /// expanded lists every entry. The assistant reply that follows a
    /// segment renders no model row of its own — the header is the single
    /// place the model shows. Segments with fewer than two entries render
    /// flat under a model-name-only header.
    Thinking(ThinkingContainer),
    /// The whole-turn process head (dsh's TurnProcess row): sits ABOVE the
    /// turn's first segment once the turn settled —「已完成，用时 2分5秒」—
    /// derived from the state's own `turn.duration`, so the live path shows
    /// it the moment the turn archives. Folding it hides the turn's segments
    /// (the reply is exempt); `hidden_by_turn_process` resolves that.
    TurnProcess {
        /// Settled run length; `None`/0 renders the bare「已完成」.
        secs: Option<i64>,
        collapsed: bool,
        user_toggled: bool,
    },
    /// A top-level tool-call card — the `AskUserQuestion` clarify card while
    /// pending and its answered-state fallback, a Preview call's artifact,
    /// or a promoted pending-confirmation copy.
    ToolCall(ToolCallItem),
    AgentTask(AgentTaskItem),
    /// A runtime error from the agent (red danger styling).
    Error(String),
    /// An ephemeral system notice — status changes, slash-command acks, host
    /// refusals. Plain text only (i18n chrome strings / mode-change notices);
    /// the body renders as a paginated `TerminalPanel`, so markdown syntax is
    /// not interpreted. Rendered with neutral tones, not danger colors.
    Notice(String),
}

/// A tool-call item, tracking status/output by id.
#[derive(Debug, Clone)]
pub struct ToolCallItem {
    pub id: String,
    pub name: String,
    pub title: String,
    pub status: ToolCallStatus,
    pub output: String,
    pub is_error: bool,
    /// The structured tool input, used for aggregate counts by target (file
    /// path / command / pattern) without re-parsing the localized title.
    pub input: serde_json::Value,
    /// True while live output is still streaming in (the call is running
    /// inside an active turn); flipped to false when the call settles.
    pub streaming: bool,
    /// True ⇒ body hidden. Projection-derived: a completed ask card folds
    /// once its answer lands, unless `user_toggled` says the user chose.
    pub collapsed: bool,
    /// Becomes true the first time the user clicks the card header. Once
    /// set, the projection's derived fold stops touching `collapsed` so the
    /// user's manual choice survives subsequent state transitions.
    pub user_toggled: bool,
    /// Persistent `Entity<TerminalPanel>` carrying the terminal-styled output
    /// body + document-level selection. `None` until first sync
    /// (`MessageItem::sync_tool_*_panel`).
    pub panel: Option<Entity<steer_components::markdown::TerminalPanel>>,
    /// The ai-elements tool card's mirrored state (status + fold), mounted by
    /// `MessageItem::ensure_tool_card` for the card families. `None` for
    /// tools that still render through the `TerminalPanel` path.
    pub card: Option<Entity<ai_elements::ToolState>>,
    /// The working directory snapshot the card's shell context renders —
    /// captured at sync time, the same moment the panel path takes its own.
    pub cwd: Option<SharedString>,
    /// The batched, append-only text the card body renders while the call
    /// streams. `output` stays the authority (copy, diff tabs, lengths); this
    /// snapshot only smooths what reaches the rich-text core (see
    /// [`crate::views::tool_stream`]). The projection lowers a fresh
    /// `ToolCallItem` per pass, but the reconcile mutates the materialized
    /// entry in place and `sync_tool_fields` deliberately does not copy this
    /// field — so the snapshot survives projection passes. Boxed to keep the
    /// activity-entry enum from carrying it inline in every variant.
    pub stream: Box<ToolStreamSnapshot>,
}

/// An entry within a `ThinkingContainer`'s activity segment. A segment mixes
/// reasoning rounds (model thinking text) and tool calls into one unified tree
/// so the collapsed header can summarize the whole turn's activity. A round
/// IS a reasoning part and a tool node IS a tool-call part — both keyed by
/// their protocol ids, so the segment's shape is the fold's shape.
#[derive(Debug, Clone)]
pub enum ActivityEntry {
    /// One reasoning round: one `ReasoningResponsePart` (the runtime closes
    /// the reasoning slot at every tool call, so part boundaries are the
    /// round boundaries). Collapses when a newer sibling arrives or the turn
    /// settles, unless `user_toggled`.
    Reasoning {
        /// The round's stable identity — the part id in the `rs:` namespace
        /// (see [`crate::transcript::reasoning_entry_id`]). The UI addresses
        /// the entry by it exactly as a tool entry is addressed by its
        /// tool-call id — see [`ThinkingContainer::find_entry_index`].
        id: String,
        text: String,
        streaming: bool,
        collapsed: bool,
        user_toggled: bool,
        /// Persistent `ChatDocument` (upstream `TextViewState` plus the source
        /// cache that keeps streaming deltas on the incremental `push_str`
        /// path), mirroring the top-level reasoning body. `None` until first
        /// sync (`MessageItem::sync_reasoning_entry`).
        markdown: Option<steer_components::markdown::ChatDocument>,
    },
    /// One tool invocation (reuses `ToolCallItem` for status/output/collapse).
    Tool(ToolCallItem),
}

impl ActivityEntry {
    /// The entry's stable id — the reconcile and the click paths address
    /// entries by it.
    pub fn id(&self) -> &str {
        match self {
            Self::Reasoning { id, .. } => id,
            Self::Tool(tool) => &tool.id,
        }
    }
}

/// What the foreground turn is doing right now — the composer's status
/// strip reads this (the message column carries no per-entry status glyphs
/// anymore; the strip is the one live "running" surface).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveActivity {
    /// A reasoning round is streaming.
    Thinking,
    /// The assistant body is streaming.
    Replying,
    /// A tool call is in flight; the payload is its one-line summary
    /// (runtime-provided, rendered verbatim).
    Tool(String),
}

/// One contiguous activity segment within a user turn, rendered as a Claude
/// Code–style Thinking status line. Entries are `ActivityEntry` (reasoning
/// rounds + tool calls); the container owns the segment-level summary,
/// collapse, and elapsed-time state.
///
/// A segment spans a maximal run of reasoning/tool-call parts of one turn:
/// the projection bounds segments by part kind, so a `StopReason::ToolUse`
/// (the model paused to execute a tool) and a mid-turn answer are both just
/// part boundaries — no arrival-rhythm heuristic anywhere.
#[derive(Debug, Clone)]
pub struct ThinkingContainer {
    /// The segment's activity entries in stream order: reasoning rounds and
    /// tool calls interleaved as the fold carries them.
    pub entries: Vec<ActivityEntry>,
    /// True while the turn is still in progress — new parts fold into this
    /// segment rather than opening a fresh one. Independent of whether any
    /// individual entry is currently live.
    pub accepting_entries: bool,
    /// True while the segment is live (turn running) OR any entry is still
    /// streaming / non-terminal. Drives the header's spinner and its live
    /// elapsed (a settled segment renders `frozen_secs` instead).
    pub streaming: bool,
    /// True ⇒ the segment renders its header row alone; false ⇒ every entry
    /// renders under the header. Derived: a settled segment folds unless the
    /// user toggled. Segments with fewer than two entries ignore this and
    /// render flat (no cover).
    pub collapsed: bool,
    pub user_toggled: bool,
    /// When the segment started; the live elapsed is
    /// `started_at.elapsed().as_secs()`, rendered h/m/s and recomputed each
    /// render while the ticker fires. Anchored on the turn's own `started_at`
    /// so the elapsed covers the whole turn, not just since the first tool
    /// call.
    pub started_at: Instant,
    /// The elapsed seconds captured when the segment went terminal. Once set,
    /// the header renders this fixed value instead of re-reading `started_at`
    /// (which would keep growing on every later re-render). Derived from the
    /// turn's own `duration`; `None` while the turn runs or when the fold
    /// carried no duration.
    pub frozen_secs: Option<u64>,
}

impl ThinkingContainer {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            accepting_entries: true,
            streaming: true,
            collapsed: false,
            user_toggled: false,
            started_at: Instant::now(),
            frozen_secs: None,
        }
    }

    /// Find an activity entry — a reasoning round or a tool call — by its id.
    /// Ids are unique across the two kinds (tool entries carry the protocol's
    /// tool-call id, reasoning rounds the part id in the `rs:` namespace), so
    /// one lookup serves both. Returns the entry's index within `entries`.
    pub fn find_entry_index(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.id() == id)
    }

    /// Get a mutable reference to a tool entry by id.
    pub fn get_tool_entry_mut(&mut self, id: &str) -> Option<&mut ToolCallItem> {
        self.entries.iter_mut().find_map(|e| match e {
            ActivityEntry::Tool(t) if t.id == id => Some(t),
            _ => None,
        })
    }
}

impl Default for ThinkingContainer {
    fn default() -> Self {
        Self::new()
    }
}

/// A sub-agent (`Steer` Dispatch) invocation. Displayed as a single-line
/// clickable item in the parent message flow; the click opens the child's
/// own conversation (right-pane drill-down tab, or the main-column stream
/// page), where the full child transcript renders through the shared
/// pipeline.
#[derive(Debug, Clone)]
pub struct AgentTaskItem {
    pub id: String,
    /// The run id the dispatch names — the child session the card drills
    /// into. Foreground results, promotion receipts, and background start
    /// receipts all carry `run_id`; a continuation carries it in the
    /// envelope address itself (`to.agent_address`), so the rework card is
    /// drillable before any ack lands and lands on the SAME child
    /// conversation as the card that created the run.
    pub run_id: Option<String>,
    pub subagent_type: String,
    pub description: String,
    /// True when the dispatch continues an existing run (`to.agent_address`,
    /// no spawn): the card reads as a follow-up on that run, not a fresh
    /// task — the `run/continued` journal row's transcript face.
    pub continuation: bool,
    pub status: ToolCallStatus,
    pub is_error: bool,
}

/// The dispatch ack's run id: the child session a completed card drills
/// into. Every Steer Dispatch ack — the foreground report, the promotion
/// receipt, the background start receipt — names it.
pub fn envelope_run_id(output: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(output)
        .ok()
        .and_then(|v| v.get("run_id").and_then(|r| r.as_str()).map(str::to_string))
}

/// One-line observation title for a delegated task's prompt: whitespace
/// flattened and capped at 60 chars with an ellipsis. Local presentation
/// rule (the runtime helper of the same shape was removed with the retired
/// harness).
pub fn subagent_topic(prompt: &str) -> String {
    let flat: String = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = flat.chars();
    let head: String = chars.by_ref().take(60).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// The first non-empty line of a prompt, for the background-task card title
/// and the restored sub-agent rail rows (local presentation rule, same as
/// `subagent_topic`).
pub fn first_line(prompt: &str) -> Option<String> {
    prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// The delegation-shape predicate lives in the projection vocabulary (the
/// data plane reads it too, to name a live sub-agent row); re-exported here
/// so the conversation's call sites keep one spelling.
pub use crate::transcript::is_agent_task_call;

/// The card's `(subagent_type, description)`: the TYPE is the Steer
/// Dispatch's spawned definition (`to.spawn` — the `run/started` row's
/// `kind`), falling back to the tool name; the description prefers the
/// call's own `description` and falls back to the shared prompt-topic
/// derivation the rail uses — both surfaces show the same title.
pub fn agent_task_labels(name: &str, input: &serde_json::Value) -> (String, String) {
    let subagent_type = crate::transcript::steer_spawn_of(input)
        .filter(|spawn| *spawn != crate::transcript::MEMBER_SPAWN)
        .map(str::to_string)
        .unwrap_or_else(|| name.to_string());
    let description = input
        .get("description")
        .and_then(serde_json::Value::as_str)
        .filter(|d| !d.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            input
                .get("prompt")
                .and_then(serde_json::Value::as_str)
                .map(subagent_topic)
        })
        .unwrap_or_default();
    (subagent_type, description)
}

/// What one projection pass did to the row list, so the caller can keep the
/// message list's `ListState` in sync (splice on append, remeasure on
/// in-place mutation).
#[derive(Debug, Default)]
pub struct ProjectionChange {
    /// The row count moved — splice the list.
    pub count_changed: bool,
    /// Rows changed in place at these indexes; remeasure each.
    pub changed: Vec<usize>,
    /// The list's shape moved beyond a tail append (a mid-list insert, a
    /// removal, a reorder) — remeasure everything; the cached heights are
    /// untrustworthy.
    pub structural: bool,
}

impl ProjectionChange {
    pub fn any(&self) -> bool {
        self.count_changed || !self.changed.is_empty() || self.structural
    }
}

/// The conversation's rows plus its client-local overlays. Content is never
/// owned here — every [`Self::project`] call re-derives it from the chat
/// state the caller passes. `items` and `keys` are parallel arrays: the
/// entity a row renders and the stable identity it answers to.
#[derive(Debug)]
pub struct ConversationState {
    items: Vec<Entity<MessageItem>>,
    keys: Vec<ItemKey>,
    overlays: Overlays,
    /// The agent whose conversation this list renders — every user bubble's
    /// header `to`. Fixed for the conversation's lifetime: the main thread's
    /// is its own agent, a sub-agent panel's is the sub-agent definition.
    recipient: manox_agent::MessageAuthor,
    /// Monotonic item-id counter. `MessageItem::id` is an opaque element-key,
    /// decoupled from the list index so a mid-list insert never renumbers
    /// existing items (which would reset gpui element state).
    next_item_id: usize,
    /// The start instant of the current (or most recent) turn — the Captain
    /// strip's clock. Re-anchored on the active turn's own `started_at` at
    /// every projection.
    turn_started_at: Instant,
}

impl ConversationState {
    pub fn new(recipient: manox_agent::MessageAuthor) -> Self {
        Self {
            items: Vec::new(),
            keys: Vec::new(),
            overlays: Overlays::default(),
            recipient,
            next_item_id: 0,
            turn_started_at: Instant::now(),
        }
    }

    pub fn items(&self) -> &[Entity<MessageItem>] {
        &self.items
    }

    /// The row keys, in list order (diagnostic read face).
    pub fn keys(&self) -> &[ItemKey] {
        &self.keys
    }

    /// True when the conversation has no substantive items (user, assistant,
    /// reasoning, tool call, or agent task). Notice-only items (error cards
    /// used for slash-command acknowledgements and mode switches) don't count
    /// so a mode-switch notice on the empty first screen doesn't prematurely
    /// leave the hero layout.
    pub fn is_empty(&self, cx: &App) -> bool {
        self.items.iter().all(|item| {
            matches!(
                item.read(cx).kind(),
                ConvItem::Error(_) | ConvItem::Notice(_)
            )
        })
    }

    fn alloc_id(&mut self) -> usize {
        let id = self.next_item_id;
        self.next_item_id += 1;
        id
    }

    /// Project the chat state (or its absence) into the row list: the single
    /// entry point every trigger converges on — the live stream's store
    /// notify, a thread attach, an optimistic submit, a sub-agent panel's
    /// pump. Pure over content; the overlays carry the client-local state.
    ///
    /// A caller whose fold reference borrows the same `cx` this needs
    /// mutably (the workspace reads the store while updating this entity)
    /// splits the call: [`Self::stage`] computes the nodes against the
    /// borrowed fold, then [`Self::reconcile`] applies them.
    pub fn project(
        &mut self,
        chat: Option<&ahp_types::state::ChatState>,
        env: &ProjectEnv,
        cx: &mut App,
    ) -> ProjectionChange {
        let nodes = self.stage(chat, env);
        self.reconcile(nodes, env, cx)
    }

    /// Phase one of the projection: diff the fold against the overlays into
    /// the desired row list. No GPUI state is touched — the fold reference
    /// may borrow whatever it likes.
    pub fn stage(
        &mut self,
        chat: Option<&ahp_types::state::ChatState>,
        env: &ProjectEnv,
    ) -> Vec<Node> {
        if let Some(active) = chat.and_then(|chat| chat.active_turn.as_ref()) {
            self.turn_started_at =
                crate::transcript::turn_anchor(&active.id, &active.started_at, &mut self.overlays);
        }
        crate::transcript::project(chat, &mut self.overlays, env)
    }

    /// Diff the projected nodes against the live rows: matched keys update
    /// in place, new keys build rows at their position, vanished keys drop.
    /// An optimistic bubble whose text a landed turn carries is adopted —
    /// the state row reuses the bubble's entity so the entrance animation
    /// and element ids survive the hand-off.
    pub fn reconcile(
        &mut self,
        nodes: Vec<Node>,
        env: &ProjectEnv,
        cx: &mut App,
    ) -> ProjectionChange {
        let mut change = ProjectionChange::default();
        let old_keys = std::mem::take(&mut self.keys);
        let old_items = std::mem::take(&mut self.items);
        let old_index: HashMap<&ItemKey, usize> =
            old_keys.iter().enumerate().map(|(ix, k)| (k, ix)).collect();
        let mut taken = vec![false; old_items.len()];
        let mut kept_order: Vec<usize> = Vec::new();
        for node in &nodes {
            let matched = old_index.get(&node.key).copied().filter(|&ix| !taken[ix]);
            let adopted = matched.is_none().then(|| {
                // Adoption: a `user:{turn}` node may claim an unclaimed
                // optimistic row carrying its exact text — the entity (with
                // its entrance animation and element ids) survives the
                // hand-off to state ownership.
                let ConvItem::User { text, .. } = &node.kind else {
                    return None;
                };
                (0..old_keys.len()).find(|&ix| {
                    !taken[ix]
                        && matches!(old_keys[ix], ItemKey::Optimistic(_))
                        && old_items[ix]
                            .read(cx)
                            .kind()
                            .user_text()
                            .is_some_and(|t| t == text.as_str())
                })
            });
            match matched.or(adopted.flatten()) {
                Some(ix) => {
                    taken[ix] = true;
                    kept_order.push(ix);
                    if sync_row(&old_items[ix], node, env, cx) {
                        change.changed.push(self.items.len());
                    }
                    self.items.push(old_items[ix].clone());
                    self.keys.push(node.key.clone());
                }
                None => {
                    // A fresh row materializes below an existing kept one
                    // only through a mid-list insert; above the kept prefix
                    // it is a tail append.
                    if self.items.len() < kept_order.len() {
                        change.structural = true;
                    }
                    let id = self.alloc_id();
                    let entering = match &node.key {
                        ItemKey::Optimistic(_) => true,
                        // A steer's fold row rides the entrance rise the
                        // receipt names, so the landing reads as the card
                        // sliding into the transcript.
                        ItemKey::User(turn) => env.entering_turns.contains(turn),
                        _ => false,
                    };
                    let item = new_node_item(node.kind.clone(), &node.role, id, env, entering, cx);
                    self.items.push(item);
                    self.keys.push(node.key.clone());
                }
            }
        }
        // Kept rows whose relative order changed mean a reorder — only the
        // adoption path can produce one (the state row takes the bubble's
        // slot), and the heights around it must be re-measured.
        let mut max_seen = 0usize;
        for ix in &kept_order {
            if *ix < max_seen {
                change.structural = true;
            }
            max_seen = max_seen.max(*ix);
        }
        // Any old row left untaken was dropped by the fold.
        if taken.iter().any(|t| !t) {
            change.structural = true;
        }
        change.count_changed = self.items.len() != old_items.len() || change.structural;
        // No explicit notify here: the caller drives this through
        // `Entity::update`, whose return already marks the conversation
        // changed for the height-observe.
        change
    }

    /// Submit a user turn optimistically: the bubble renders immediately and
    /// is adopted by the turn that lands — by the client-minted `turn_id`
    /// when the caller has one, by text only against turns minted after this
    /// point. Re-projects so the row is on screen this frame.
    pub fn push_optimistic_user(
        &mut self,
        chat: Option<&ahp_types::state::ChatState>,
        bubble: crate::transcript::OverlayBubbleSeed,
        env: &ProjectEnv,
        cx: &mut App,
    ) {
        let crate::transcript::OverlayBubbleSeed {
            text,
            images,
            mut meta,
            turn_id,
            steer_id,
        } = bubble;
        // The header's `to` is whoever owns this conversation, so it is
        // stamped here rather than carried by every caller.
        meta.recipient = Some(self.recipient.clone());
        let serial = self.overlays.next_serial;
        self.overlays.next_serial += 1;
        // Count the rows the fold holds now, the active turn included: the
        // text fallback must not pair with any of them.
        let minted_turns = chat
            .map(|c| c.turns.len() + usize::from(c.active_turn.is_some()))
            .unwrap_or(0);
        self.overlays
            .bubbles
            .push(crate::transcript::OverlayBubble {
                serial,
                text,
                images,
                meta,
                turn_id,
                steer_id,
                minted_turns,
            });
        self.project(chat, env, cx);
    }

    /// Upgrade a window bubble to the identity of the turn its steer opened.
    /// The removal echo mints the bubble before any turn exists (no id); the
    /// partition's `turnStarted` later names the turn, and this upgrade lets
    /// [`crate::transcript::project`] pair the bubble by IDENTITY — so the
    /// window is adopted no matter how late the two receipts drain, and no
    /// mint-time turn count can strand it.
    pub fn attach_steer_turn(&mut self, steer_id: &str, turn_id: &str) -> bool {
        let mut changed = false;
        for bubble in &mut self.overlays.bubbles {
            if bubble.steer_id.as_deref() == Some(steer_id) && bubble.turn_id.is_none() {
                bubble.turn_id = Some(turn_id.to_string());
                changed = true;
            }
        }
        changed
    }

    /// Drop window bubbles whose steer never opened a fold row: a run that
    /// aborted before draining the injection retracts the queued steer, so
    /// no turn-start will ever adopt the bridge. The settle is the last
    /// chance to retire it — a permanent tail row would report a message the
    /// model never saw. Returns whether anything was dropped.
    pub fn drop_unlanded_steer_bubbles(&mut self) -> bool {
        let before = self.overlays.bubbles.len();
        self.overlays
            .bubbles
            .retain(|bubble| bubble.steer_id.is_none() || bubble.turn_id.is_some());
        self.overlays.bubbles.len() != before
    }

    /// Take the host refusals the store drained for this session into the
    /// overlays, so the next projection renders them as keyed notice rows.
    /// A refusal naming a turn retires the optimistic bubble that turn was
    /// minted for — the row never had content behind it, and leaving it
    /// would strand a phantom message beside the notice explaining it.
    ///
    /// A `server_seq` already on the table is dropped here: the store's table
    /// drains, so a re-delivered envelope is recorded again, and the rows are
    /// keyed by that seq — two copies would project two rows under one key.
    pub fn absorb_refusals(&mut self, refusals: Vec<crate::transcript::RefusalNotice>) {
        for refusal in refusals {
            if let Some(turn_id) = refusal.turn_id.as_deref() {
                self.overlays
                    .bubbles
                    .retain(|bubble| bubble.turn_id.as_deref() != Some(turn_id));
            }
            if !self.overlays.refusals.iter().any(|r| r.seq == refusal.seq) {
                self.overlays.refusals.push(refusal);
            }
        }
        let cap = 128;
        let excess = self.overlays.refusals.len().saturating_sub(cap);
        self.overlays.refusals.drain(..excess);
    }

    /// Locate the top-level `ToolCall` card with `id` — the ask card the
    /// interactive surface attaches to.
    pub fn find_tool(&self, id: &str, cx: &App) -> Option<usize> {
        self.items
            .iter()
            .position(|item| matches!(item.read(cx).kind(), ConvItem::ToolCall(t) if t.id == id))
    }

    /// Locate a `Thinking` container's entry — a reasoning round or a tool
    /// call — by id. Returns `(container_index, entry_index)` so the caller
    /// can update the entry in place. Scans every container in arrival order
    /// so an id always resolves to its owning entry.
    pub fn find_thinking_entry(&self, id: &str, cx: &App) -> Option<(usize, usize)> {
        for (cix, item) in self.items.iter().enumerate() {
            if let ConvItem::Thinking(t) = item.read(cx).kind()
                && let Some(eix) = t.find_entry_index(id)
            {
                return Some((cix, eix));
            }
        }
        None
    }

    /// Whether the item at `ix` is hidden by a folded whole-turn process
    /// head. The scope is the item's OWN turn: walking backwards, the first
    /// turn-scope marker decides — the turn's process head (folded hides,
    /// open shows) or its user row (a turn without a head above this item
    /// shows). A head from an EARLIER turn never reaches across the user row
    /// that opened this one.
    ///
    /// The reply is exempt by construction (only `Thinking` hides), and the
    /// turn's LAST segment is exempt too (see below).
    pub fn hidden_by_turn_process(&self, ix: usize, cx: &App) -> bool {
        if !matches!(
            self.items.get(ix).map(|item| item.read(cx).kind()),
            Some(ConvItem::Thinking(_))
        ) {
            return false;
        }
        let folded_head =
            self.items[..ix]
                .iter()
                .rev()
                .find_map(|item| match item.read(cx).kind() {
                    ConvItem::TurnProcess { collapsed, .. } => Some(*collapsed),
                    ConvItem::User { .. } => Some(false),
                    _ => None,
                });
        if folded_head != Some(true) {
            return false;
        }
        // Hidden iff a LATER segment follows within this same turn — the
        // turn's last segment is exempt even under a folded head, being the
        // reasoning that faces the answer (dsh's answer-anchor rule:
        // hiding it reads as "the model answered from nothing"). The scan
        // stops at the next turn's markers — its opening user row or its
        // own process head — so a later turn's segments never decide this
        // one's exemption.
        self.items[ix + 1..]
            .iter()
            .take_while(|item| {
                !matches!(
                    item.read(cx).kind(),
                    ConvItem::User { .. } | ConvItem::TurnProcess { .. }
                )
            })
            .any(|item| matches!(item.read(cx).kind(), ConvItem::Thinking(_)))
    }

    /// Whether the row at `ix` should tighten its bottom gap: an assistant
    /// body immediately followed by an activity segment — the segment's own
    /// header padding breathes, so the generic 16px row gap reads as a hole
    /// (the reverse direction keeps it: a reply needs its air).
    pub fn tight_gap_after(&self, ix: usize, cx: &App) -> bool {
        matches!(
            self.items.get(ix).map(|item| item.read(cx).kind()),
            Some(ConvItem::Assistant { .. })
        ) && matches!(
            self.items.get(ix + 1).map(|item| item.read(cx).kind()),
            Some(ConvItem::Thinking(_))
        )
    }

    /// The running turn's elapsed seconds (the Captain strip's clock).
    pub fn running_secs(&self) -> u64 {
        self.turn_started_at.elapsed().as_secs()
    }

    /// The trailing turn's live activity for the composer's status strip.
    /// A streaming assistant body names `Replying`; the trailing segment is
    /// scanned back to front for a streaming reasoning round or a tool in
    /// flight (a tool's one-line summary rides verbatim); a streaming
    /// segment with no live entry — the gap between a tool settling and the
    /// next round — reads as `Thinking`. `None` when nothing is running.
    pub fn live_activity(&self, cx: &App) -> Option<LiveActivity> {
        let last = self.items.last()?.read(cx).kind();
        match last {
            ConvItem::Assistant {
                streaming: true, ..
            } => Some(LiveActivity::Replying),
            ConvItem::Thinking(t) => {
                for entry in t.entries.iter().rev() {
                    match entry {
                        ActivityEntry::Reasoning {
                            streaming: true, ..
                        } => return Some(LiveActivity::Thinking),
                        ActivityEntry::Tool(tool)
                            if tool.streaming
                                || matches!(
                                    tool.status,
                                    ToolCallStatus::Running | ToolCallStatus::PendingApproval
                                ) =>
                        {
                            let title = if !tool.title.is_empty() {
                                tool.title.clone()
                            } else if !tool.name.is_empty() {
                                tool.name.clone()
                            } else {
                                crate::i18n::t("thinking-tool-result").to_string()
                            };
                            return Some(LiveActivity::Tool(title));
                        }
                        _ => {}
                    }
                }
                t.streaming.then_some(LiveActivity::Thinking)
            }
            _ => None,
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.keys.clear();
        self.overlays = Overlays::default();
    }
}

impl ConvItem {
    /// The item's user-bubble text when it is a `User` row (the adoption
    /// matcher reads this).
    fn user_text(&self) -> Option<&str> {
        match self {
            ConvItem::User { text, .. } => Some(text),
            _ => None,
        }
    }
}

/// Build the `MessageItem` entity for a projected node: full text parse +
/// persistent mounts, mirroring what a history rebuild used to do, so a row
/// created live and a row created by a replay are the same object.
fn new_node_item(
    kind: ConvItem,
    role: &str,
    id: usize,
    env: &ProjectEnv,
    entering: bool,
    cx: &mut App,
) -> Entity<MessageItem> {
    // A user item carries its OWN turn's model (the wire stamp in its meta),
    // not the session's current label — per-turn headers stay historical.
    let own_role = match &kind {
        ConvItem::User {
            meta: Some(meta), ..
        } if !meta.model_id.is_empty() => meta.model_id.clone(),
        _ => role.to_string(),
    };
    let cwd = env.cwd.map(SharedString::from);
    cx.new(|cx| {
        let text = match &kind {
            ConvItem::Assistant { text, .. } => Some(text.clone()),
            _ => None,
        };
        let mut item = MessageItem::new(kind, own_role, id, env.host.clone());
        if entering {
            // Optimistic bubbles, plus the fold rows the sync's receipts
            // name (`entering_turns`) — a steer's landing rides the entrance
            // rise even though the row is fold-synthesized.
            item = item.entering_now();
        }
        // For projected text items, feed the full snapshot so the mounted
        // document matches the content exactly (a later append rides the
        // incremental push_str).
        if let Some(text) = text {
            item.update_text(&text, cx);
        }
        // Mount + finalize persistent markdown for every reasoning round
        // inside a `Thinking` segment, so selection works on reloaded
        // history (not just live-streamed turns).
        item.rebuild_activity_reasoning(cx);
        // Mount the persistent `TerminalPanel` for every tool call (segment
        // entries + top-level cards) so the terminal-styled body works with
        // selection from the first frame.
        item.rebuild_tool_panels(cwd, cx);
        // Mount the persistent paginated `TerminalPanel` for every `Notice`
        // so a notice folds like its live counterpart instead of a
        // per-frame fallback.
        item.ensure_notice_panel(cx);
        item
    })
}

/// Apply one node's content onto an existing row: field-by-field, preserving
/// the row's interaction state (`collapsed` / `user_toggled`) except where a
/// derived fold transition fires. Returns whether anything visible changed.
fn sync_row(item: &Entity<MessageItem>, node: &Node, env: &ProjectEnv, cx: &mut App) -> bool {
    let cwd = env.cwd.map(SharedString::from);
    item.update(cx, |item, cx| {
        match &node.kind {
            ConvItem::User { text, meta, .. } => {
                let ConvItem::User {
                    text: old_text,
                    meta: old_meta,
                    ..
                } = item.kind_mut()
                else {
                    return false;
                };
                let mut changed = old_text != text;
                *old_text = text.clone();
                if let (Some(old_meta), Some(meta)) = (old_meta, meta) {
                    // The header identity follows the state, INCLUDING the
                    // steered badge — a projection fact (the fold names the
                    // steer rows), so a later projection must be able to
                    // stamp a row that mounted before its steer was known
                    // (the removal receipt retires the card a turn ahead of
                    // the partition on the wire).
                    old_meta.timestamp = meta.timestamp;
                    old_meta.model_id = meta.model_id.clone();
                    changed |= set(&mut old_meta.steered, meta.steered);
                }
                changed
            }
            ConvItem::Assistant { .. } => sync_assistant(item, node, cx),
            ConvItem::Thinking(n) => sync_segment(item, n, cwd, cx),
            ConvItem::ToolCall(n) => sync_tool_card(item, n, cwd, cx),
            ConvItem::AgentTask(n) => {
                let ConvItem::AgentTask(t) = item.kind_mut() else {
                    return false;
                };
                let mut changed = false;
                if !n.subagent_type.is_empty() {
                    changed |= set(&mut t.subagent_type, n.subagent_type.clone());
                }
                if !n.description.is_empty() {
                    changed |= set(&mut t.description, n.description.clone());
                }
                changed |= set(&mut t.status, n.status);
                changed |= set(&mut t.is_error, n.is_error);
                changed |= set(&mut t.run_id, n.run_id.clone());
                changed |= set(&mut t.continuation, n.continuation);
                changed
            }
            ConvItem::Notice(text) => {
                let ConvItem::Notice(old) = item.kind_mut() else {
                    return false;
                };
                let changed = old != text;
                *old = text.clone();
                if changed {
                    item.ensure_notice_panel(cx);
                }
                changed
            }
            ConvItem::Error(text) => {
                let ConvItem::Error(old) = item.kind_mut() else {
                    return false;
                };
                set(old, text.clone())
            }
            ConvItem::TurnProcess { .. } => {
                // The head is static once minted (secs from a settled
                // duration); nothing to sync.
                false
            }
        }
    })
}

fn sync_assistant(
    item: &mut MessageItem,
    node: &Node,
    cx: &mut gpui::Context<MessageItem>,
) -> bool {
    let ConvItem::Assistant {
        text: n_text,
        streaming,
        activity_header,
        token_usage,
        entry_id,
        fork_unavailable,
        ..
    } = &node.kind
    else {
        return false;
    };
    let mut changed = false;
    {
        let ConvItem::Assistant {
            text,
            streaming: old_streaming,
            activity_header: old_header,
            token_usage: old_usage,
            entry_id: old_entry,
            fork_unavailable: old_unavail,
            ..
        } = item.kind_mut()
        else {
            return false;
        };
        if text != n_text {
            // Feed the full snapshot; an appended delta rides upstream's
            // incremental `push_str`, so this is proportional to the delta
            // and never drops a byte (whitespace included — the fix that
            // motivated this projection).
            let full = n_text.clone();
            *text = full;
            changed = true;
        }
        changed |= set(old_streaming, *streaming);
        changed |= set(old_header, *activity_header);
        changed |= set(old_usage, *token_usage);
        changed |= set(old_entry, entry_id.clone());
        changed |= set(old_unavail, *fork_unavailable);
    }
    if changed {
        let ConvItem::Assistant { text, .. } = item.kind() else {
            return changed;
        };
        let full = text.clone();
        item.update_text(&full, cx);
    }
    changed
}

/// Sync one activity segment: match entries by id (reasoning rounds by part
/// id, tools by call id), append the new ones, and apply the derived fold
/// transitions that replace the old event-driven heuristics.
fn sync_segment(
    item: &mut MessageItem,
    n: &ThinkingContainer,
    cwd: Option<SharedString>,
    cx: &mut gpui::Context<MessageItem>,
) -> bool {
    let was_streaming = matches!(item.kind(), ConvItem::Thinking(t) if t.streaming);
    let mut changed = false;
    let mut mount_reasoning: Vec<usize> = Vec::new();
    let mut mount_tool: Vec<usize> = Vec::new();
    {
        let ConvItem::Thinking(t) = item.kind_mut() else {
            return false;
        };
        // An entry the node no longer carries leaves the row first: a
        // re-folded turn (bridge restart re-issuing `turnStarted`, a re-run)
        // can drop trailing parts, and the live row must match a fresh
        // projection of the same state — otherwise "live and rebuild are one
        // function" diverges exactly here. The drop precedes the merge so
        // the mount indices taken below address the vector the caller will
        // still see (a stale index would mount the wrong entry's body, or
        // silently skip the one that changed).
        let keep: std::collections::HashSet<&str> =
            n.entries.iter().map(|entry| entry.id()).collect();
        let before = t.entries.len();
        t.entries.retain(|entry| keep.contains(entry.id()));
        changed |= t.entries.len() != before;
        for entry in n.entries.iter() {
            let id = entry.id().to_string();
            match t.find_entry_index(&id) {
                Some(found) => {
                    let changed_entry = match (&mut t.entries[found], entry) {
                        (
                            ActivityEntry::Reasoning {
                                text,
                                streaming,
                                markdown,
                                ..
                            },
                            ActivityEntry::Reasoning {
                                text: n_text,
                                streaming: n_streaming,
                                ..
                            },
                        ) => {
                            let mut dirty = text != n_text;
                            if dirty {
                                *text = n_text.clone();
                                if markdown.is_some() {
                                    mount_reasoning.push(found);
                                }
                            }
                            dirty |= set(streaming, *n_streaming);
                            dirty
                        }
                        (ActivityEntry::Tool(old), ActivityEntry::Tool(new)) => {
                            sync_tool_fields(old, new)
                        }
                        _ => false,
                    };
                    changed |= changed_entry;
                    if changed_entry {
                        if matches!(entry, ActivityEntry::Tool(_)) {
                            mount_tool.push(found);
                        } else if !mount_reasoning.contains(&found) {
                            mount_reasoning.push(found);
                        }
                    }
                }
                None => {
                    // A new entry arrives: the entries before it fold (user
                    // pins win) — the derived replacement for the old
                    // fold_preceding heuristic. (The insert below reports
                    // the change for the whole arm.)
                    for prev in t.entries.iter_mut() {
                        match prev {
                            ActivityEntry::Reasoning {
                                collapsed,
                                user_toggled,
                                ..
                            } if !*user_toggled && !*collapsed => {
                                *collapsed = true;
                            }
                            ActivityEntry::Tool(tool) if !tool.user_toggled && !tool.collapsed => {
                                tool.collapsed = true;
                            }
                            _ => {}
                        }
                    }
                    match entry {
                        ActivityEntry::Reasoning {
                            id,
                            text,
                            streaming,
                            ..
                        } => {
                            let at = t.entries.len();
                            t.entries.push(ActivityEntry::Reasoning {
                                id: id.clone(),
                                text: text.clone(),
                                streaming: *streaming,
                                collapsed: false,
                                user_toggled: false,
                                markdown: None,
                            });
                            mount_reasoning.push(at);
                        }
                        ActivityEntry::Tool(tool) => {
                            let at = t.entries.len();
                            t.entries.push(ActivityEntry::Tool(tool.clone()));
                            mount_tool.push(at);
                        }
                    }
                    changed = true;
                }
            }
        }
        // Derived fold transitions replace the old 1s auto-collapse timer:
        // the settle folds everything unpinned the frame the turn archives.
        let settle = was_streaming && !n.streaming;
        if settle {
            for entry in t.entries.iter_mut() {
                match entry {
                    ActivityEntry::Reasoning {
                        streaming,
                        collapsed,
                        user_toggled,
                        ..
                    } => {
                        *streaming = false;
                        if !*user_toggled && !*collapsed {
                            *collapsed = true;
                        }
                    }
                    ActivityEntry::Tool(tool) => {
                        tool.streaming = false;
                        if matches!(
                            tool.status,
                            ToolCallStatus::Success
                                | ToolCallStatus::Error
                                | ToolCallStatus::Denied
                        ) && !tool.user_toggled
                            && !tool.collapsed
                        {
                            tool.collapsed = true;
                        }
                    }
                }
            }
            if !t.user_toggled && !t.collapsed {
                t.collapsed = true;
            }
        }
        // Container bookkeeping follows the node wholesale.
        t.accepting_entries = n.accepting_entries;
        t.streaming = n.streaming;
        if t.frozen_secs.is_none() {
            t.frozen_secs = n.frozen_secs;
        }
    }
    for eix in mount_reasoning {
        item.sync_reasoning_entry(eix, cx);
    }
    for eix in mount_tool {
        item.sync_tool_entry_panel(eix, cwd.clone(), cx);
    }
    changed
}

/// Field-level sync of one tool entry's content; the caller mounts the body
/// when this reports a change.
fn sync_tool_fields(t: &mut ToolCallItem, n: &ToolCallItem) -> bool {
    let mut changed = set(&mut t.name, n.name.clone());
    changed |= set(&mut t.status, n.status);
    if !n.input.is_null() {
        changed |= set(&mut t.input, n.input.clone());
        changed |= set(&mut t.title, n.title.clone());
    }
    changed |= set(&mut t.output, n.output.clone());
    changed |= set(&mut t.is_error, n.is_error);
    changed |= set(&mut t.streaming, n.streaming);
    changed
}

/// Sync a top-level tool card (ask / preview / promoted confirmation). A
/// completed ask card folds once its answer lands (unless pinned); a
/// Preview's body is the artifact and never folds.
fn sync_tool_card(
    item: &mut MessageItem,
    n: &ToolCallItem,
    cwd: Option<SharedString>,
    cx: &mut gpui::Context<MessageItem>,
) -> bool {
    let mut changed;
    {
        let ConvItem::ToolCall(t) = item.kind_mut() else {
            return false;
        };
        changed = sync_tool_fields(t, n);
        if t.cwd.is_none() {
            t.cwd = cwd.clone();
        }
        let settled = matches!(
            t.status,
            ToolCallStatus::Success | ToolCallStatus::Error | ToolCallStatus::Denied
        );
        if settled && !t.user_toggled && !t.collapsed && t.name != crate::preview::TOOL_NAME {
            // The ask's answer landed: fold this frame (the derived
            // replacement for the 1s timer).
            t.collapsed = true;
            changed = true;
        }
    }
    if changed {
        item.sync_tool_call_panel(cwd, cx);
    }
    changed
}

fn set<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        false
    } else {
        *slot = value;
        true
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::transcript::{Overlays, ProjectEnv, project};
    use ahp_types::actions::StateAction;
    use ahp_types::state::ChatState;

    /// An empty fold to fold actions into, exactly as the store holds one.
    fn empty_chat() -> ChatState {
        serde_json::from_value(serde_json::json!({
            "resource": "ahp-chat:/c-test",
            "title": "test",
            "status": 0,
            "modifiedAt": "2026-01-01T00:00:00Z",
            "turns": [],
        }))
        .expect("the empty chat parses")
    }

    fn fold(chat: &mut ChatState, action: &StateAction) {
        ahp::reducers::apply_action_to_chat(chat, action);
    }

    fn start(turn: &str, text: &str) -> StateAction {
        serde_json::from_value(serde_json::json!({
            "type": "chat/turnStarted", "turnId": turn,
            "startedAt": "2026-01-01T00:00:00Z",
            "message": { "text": text, "origin": { "kind": "user" } },
        }))
        .expect("the start action parses")
    }

    fn open_md(turn: &str, part: &str) -> StateAction {
        serde_json::from_value(serde_json::json!({
            "type": "chat/responsePart", "turnId": turn,
            "part": { "kind": "markdown", "id": part, "content": "" },
        }))
        .expect("the open action parses")
    }

    fn open_rs(turn: &str, part: &str) -> StateAction {
        serde_json::from_value(serde_json::json!({
            "type": "chat/responsePart", "turnId": turn,
            "part": { "kind": "reasoning", "id": part, "content": "" },
        }))
        .expect("the open action parses")
    }

    fn delta(turn: &str, part: &str, content: &str) -> StateAction {
        serde_json::from_value(serde_json::json!({
            "type": "chat/delta", "turnId": turn, "partId": part, "content": content,
        }))
        .expect("the delta action parses")
    }

    fn reasoning_delta(turn: &str, part: &str, content: &str) -> StateAction {
        serde_json::from_value(serde_json::json!({
            "type": "chat/reasoning", "turnId": turn, "partId": part, "content": content,
        }))
        .expect("the reasoning action parses")
    }

    fn complete(turn: &str, duration_ms: i64) -> StateAction {
        serde_json::from_value(serde_json::json!({
            "type": "chat/turnComplete", "turnId": turn, "duration": duration_ms,
        }))
        .expect("the complete action parses")
    }

    fn env() -> (
        ProjectEnv<'static>,
        std::sync::Arc<dyn crate::host::ChatHost>,
    ) {
        env_with_steered(&[])
    }

    /// The env with a `steered` set: the projection facts behind the user
    /// rows' steer badge (the store derives the set from the envelopes; the
    /// fixture hands it straight to the projection).
    fn env_with_steered(
        steered: &[&str],
    ) -> (
        ProjectEnv<'static>,
        std::sync::Arc<dyn crate::host::ChatHost>,
    ) {
        // Leak the test constants: the env borrows for the call's lifetime,
        // and each test builds exactly one.
        let recipient = Box::leak(Box::new(manox_agent::MessageAuthor::Lead));
        let host: &'static crate::host::ChatHostHandle =
            Box::leak(Box::new(crate::host::noop_host()));
        let steered: &'static std::collections::HashSet<String> = Box::leak(Box::new(
            steered
                .iter()
                .map(|turn| turn.to_string())
                .collect::<std::collections::HashSet<String>>(),
        ));
        (
            ProjectEnv {
                role: "test-model",
                recipient,
                host,
                cwd: None,
                fork_source: false,
                promoted_confirmation: None,
                entering_turns: &[],
                steered_turns: steered,
            },
            host.clone(),
        )
    }

    fn keys_of(nodes: &[Node]) -> Vec<String> {
        nodes.iter().map(|n| n.key.to_string()).collect()
    }

    // ── the projection's pure face: ChatState in, rows out ────────────────

    /// The bug that motivated the projection: pure-whitespace deltas ride the
    /// fold's accumulated content byte for byte. Nothing anywhere drops them.
    #[test]
    fn whitespace_deltas_survive_byte_for_byte() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "hi"));
        fold(&mut chat, &open_md("t-1", "p-1"));
        let chunks = ["a", " ", "\n", "  ", "\t\n", "b", "\n\n "];
        for chunk in chunks {
            fold(&mut chat, &delta("t-1", "p-1", chunk));
        }
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        let joined: String = chunks.concat();
        let answer = nodes.iter().find_map(|n| match &n.kind {
            ConvItem::Assistant { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert_eq!(
            answer.as_deref(),
            Some(joined.as_str()),
            "the projected body is the exact delta concatenation"
        );
    }

    /// The display shape of one settled exchange: user row, process head
    /// (from the state's duration), answer — with no tool activity there is
    /// no segment and the head does not render.
    #[test]
    fn a_plain_exchange_has_no_process_head() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "hi"));
        fold(&mut chat, &open_md("t-1", "p-1"));
        fold(&mut chat, &delta("t-1", "p-1", "hello"));
        fold(&mut chat, &complete("t-1", 5_000));
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        assert_eq!(
            keys_of(&nodes),
            vec!["user:t-1", "md:p-1"],
            "a segment-free turn renders no process head"
        );
    }

    /// A tool loop: reasoning + tool calls fold into ONE segment per
    /// maximal run, the settled turn earns its process head above it with
    /// the state's own duration, and the answer carries no model row.
    #[test]
    fn a_tool_loop_groups_into_one_segment_with_a_derived_head() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "read it"));
        fold(&mut chat, &open_rs("t-1", "p-r1"));
        fold(&mut chat, &reasoning_delta("t-1", "p-r1", "thinking"));
        let tool_start: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallStart", "turnId": "t-1", "toolCallId": "tc-1",
            "toolName": "Read", "displayName": "Read",
            "intention": "Read AGENTS.md",
        }))
        .unwrap();
        fold(&mut chat, &tool_start);
        fold(&mut chat, &open_md("t-1", "p-1"));
        fold(&mut chat, &delta("t-1", "p-1", "done"));
        fold(&mut chat, &complete("t-1", 12_345));
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        assert_eq!(
            keys_of(&nodes),
            vec!["user:t-1", "tp:t-1", "seg:rs:p-r1", "md:p-1"],
            "segment = the maximal reasoning/tool run; the head sits above it"
        );
        let ConvItem::TurnProcess { secs, .. } = &nodes[1].kind else {
            panic!("the second row is the process head");
        };
        assert_eq!(
            *secs,
            Some(12),
            "the head's seconds come from turn.duration"
        );
        let ConvItem::Thinking(t) = &nodes[2].kind else {
            panic!("the third row is the segment");
        };
        assert_eq!(t.entries.len(), 2, "the reasoning round and the tool node");
        assert!(!t.streaming, "a settled segment is not streaming");
        assert!(t.collapsed, "a settled segment derives folded");
        assert_eq!(t.frozen_secs, Some(12), "the elapsed pins on the duration");
        assert!(!nodes[3].model_repeats);
        let ConvItem::Assistant {
            activity_header, ..
        } = &nodes[3].kind
        else {
            panic!("the last row is the answer");
        };
        assert!(activity_header, "the segment header carries the model");
    }

    /// Tool state machine: a completed call carries the state's own result
    /// text and status; a denied/error completion is `is_error`.
    #[test]
    fn tool_state_lowering_carries_status_and_output() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "x"));
        let tool_start: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallStart", "turnId": "t-1", "toolCallId": "tc-1",
            "toolName": "Read", "displayName": "Read", "intention": "Read it",
        }))
        .unwrap();
        fold(&mut chat, &tool_start);
        let ready: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallReady", "turnId": "t-1", "toolCallId": "tc-1",
            "invocationMessage": "Read it",
            "toolInput": "{\"path\": \"a.rs\"}",
            "confirmed": "not-needed",
        }))
        .unwrap();
        fold(&mut chat, &ready);
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        let ConvItem::Thinking(t) = &nodes[1].kind else {
            panic!("the tool folds into a segment");
        };
        let Some(ActivityEntry::Tool(entry)) = t.entries.first() else {
            panic!("the tool is an entry");
        };
        assert_eq!(entry.status, ToolCallStatus::Running);
        assert_eq!(entry.input["path"], "a.rs");
        assert!(t.streaming, "the active turn keeps the segment live");
        assert!(!t.collapsed, "a live segment derives expanded");

        let done: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallComplete", "turnId": "t-1", "toolCallId": "tc-1",
            "result": {
                "success": true,
                "pastTenseMessage": "Read it",
                "content": [{ "type": "text", "text": "fn main() {}" }],
            },
        }))
        .unwrap();
        fold(&mut chat, &done);
        let nodes = project(Some(&chat), &mut overlays, &env);
        let ConvItem::Thinking(t) = &nodes[1].kind else {
            panic!("still a segment");
        };
        let Some(ActivityEntry::Tool(entry)) = t.entries.first() else {
            panic!("still the tool entry");
        };
        assert_eq!(entry.status, ToolCallStatus::Success);
        assert_eq!(entry.output, "fn main() {}");
        assert!(!entry.is_error);
    }

    /// An elicitation part renders the ask card: pending while the response
    /// is absent, settled once it lands.
    #[test]
    fn an_input_request_projects_the_ask_card() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "ask"));
        let requested: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/inputRequested", "turnId": "t-1",
            "request": {
                "id": "q-1",
                "questions": [{
                    "kind": "single-select", "id": "q", "title": "Pick",
                    "message": "Which one?",
                    "options": [{ "id": "a", "label": "A" }],
                }],
            },
        }))
        .unwrap();
        fold(&mut chat, &requested);
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        assert_eq!(
            keys_of(&nodes),
            vec!["user:t-1", "ir:q-1"],
            "the ask card is the request part"
        );
        let ConvItem::ToolCall(card) = &nodes[1].kind else {
            panic!("the ask card is a tool card");
        };
        assert_eq!(card.status, ToolCallStatus::PendingApproval);
        assert_eq!(card.input["requestId"], "q-1");

        let completed: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/inputCompleted", "turnId": "t-1", "requestId": "q-1",
            "response": "accept",
        }))
        .unwrap();
        fold(&mut chat, &completed);
        let nodes = project(Some(&chat), &mut overlays, &env);
        let ConvItem::ToolCall(card) = &nodes[1].kind else {
            panic!("still the ask card");
        };
        assert_eq!(card.status, ToolCallStatus::Success);
    }

    /// An ask-SHAPED tool call (the runtime's own clarify call) is a
    /// top-level card, never a segment entry.
    #[test]
    fn an_ask_shaped_tool_call_is_a_top_level_card() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "ask"));
        let tool_start: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallStart", "turnId": "t-1", "toolCallId": "tc-9",
            "toolName": "AskUserQuestion", "displayName": "AskUserQuestion",
            "intention": "Clarify",
        }))
        .unwrap();
        fold(&mut chat, &tool_start);
        let ready: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallReady", "turnId": "t-1", "toolCallId": "tc-9",
            "invocationMessage": "Clarify",
            "toolInput": "{\"questions\": []}",
        }))
        .unwrap();
        fold(&mut chat, &ready);
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        assert_eq!(keys_of(&nodes), vec!["user:t-1", "tc:tc-9"]);
    }

    /// System notifications and error parts render keyed per (turn, index) —
    /// the part id they do not carry. The error part reaches the state at
    /// turn-end (the reducer drops Error parts on the wire action and
    /// attaches them when the turn archives), so the fixture state is
    /// written directly.
    #[test]
    fn notices_and_errors_key_on_turn_and_index() {
        let chat: ChatState = serde_json::from_value(serde_json::json!({
            "resource": "ahp-chat:/c-test",
            "title": "test",
            "status": 0,
            "modifiedAt": "2026-01-01T00:00:00Z",
            "turns": [{
                "id": "t-1",
                "startedAt": "2026-01-01T00:00:00Z",
                "message": { "text": "x", "origin": { "kind": "user" } },
                "responseParts": [
                    { "kind": "systemNotification", "content": "note" },
                    { "kind": "error",
                      "error": { "errorType": "e", "message": "boom" } },
                ],
                "state": "error",
            }],
        }))
        .expect("the fixture parses");
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        assert_eq!(
            keys_of(&nodes),
            vec!["user:t-1", "notice:t-1:1", "err:t-1:2"],
            "the keys are (turn, 1-based part index)"
        );
    }

    /// Adjacent segments of one turn repeat the model only when it changed:
    /// model_repeats is a pure derivation over the emitted order.
    #[test]
    fn model_repeats_is_derived_not_stored() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "x"));
        for (rs, md) in [("p-r1", "p-m1"), ("p-r2", "p-m2")] {
            fold(&mut chat, &open_rs("t-1", rs));
            fold(&mut chat, &reasoning_delta("t-1", rs, "think"));
            fold(&mut chat, &open_md("t-1", md));
            fold(&mut chat, &delta("t-1", md, "partial"));
        }
        fold(&mut chat, &complete("t-1", 1_000));
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        let repeats: Vec<bool> = nodes
            .iter()
            .filter(|n| matches!(n.kind, ConvItem::Thinking(_)))
            .map(|n| n.model_repeats)
            .collect();
        assert_eq!(
            repeats,
            vec![false, true],
            "the second segment of the same turn repeats the model"
        );
    }

    /// Refusal notices render once per server seq, keyed, at the tail.
    #[test]
    fn refusal_notices_survive_re_projection() {
        let (env, _host) = env();
        let mut overlays = Overlays {
            refusals: vec![
                crate::transcript::RefusalNotice {
                    seq: 7,
                    text: "rejected: nope".into(),
                    turn_id: None,
                },
                crate::transcript::RefusalNotice {
                    seq: 9,
                    text: "rejected: again".into(),
                    turn_id: None,
                },
            ],
            ..Overlays::default()
        };
        let nodes = project(None, &mut overlays, &env);
        assert_eq!(keys_of(&nodes), vec!["notice:rej:7", "notice:rej:9"]);
        // The same table handed over again (any later store notify while the
        // turn streams) must still render both rows: the seq-keyed identity
        // keeps them in place rather than letting one pass consume them.
        let nodes = project(None, &mut overlays, &env);
        assert_eq!(
            keys_of(&nodes),
            vec!["notice:rej:7", "notice:rej:9"],
            "a refusal row survives re-projection"
        );
    }

    /// The Captain strip's clock counts from the active turn's own `started_at`
    /// — the host stamps it when the turn (and therefore the user message that
    /// opened it) was submitted.
    #[test]
    fn the_captain_clock_anchors_on_the_active_turns_started_at() {
        let started_at = (chrono::Utc::now() - chrono::Duration::seconds(90))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let start: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/turnStarted", "turnId": "t-1",
            "startedAt": started_at,
            "message": { "text": "hi", "origin": { "kind": "user" } },
        }))
        .expect("the start action parses");
        let mut chat = empty_chat();
        fold(&mut chat, &start);
        let (env, _host) = env();
        let mut conversation = ConversationState::new(manox_agent::MessageAuthor::Lead);
        let _ = conversation.stage(Some(&chat), &env);
        assert!(
            conversation.running_secs().abs_diff(90) <= 1,
            "the clock counts from the turn's start stamp"
        );
    }

    /// The repaired anchor is the segment header's anchor too: the header's
    /// live elapsed counts from the turn's start stamp rather than from the
    /// first time this client saw the turn.
    #[test]
    fn the_segment_header_counts_from_the_same_turn_start() {
        let started_at = (chrono::Utc::now() - chrono::Duration::seconds(90))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let start: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/turnStarted", "turnId": "t-1",
            "startedAt": started_at,
            "message": { "text": "hi", "origin": { "kind": "user" } },
        }))
        .expect("the start action parses");
        let mut chat = empty_chat();
        fold(&mut chat, &start);
        fold(&mut chat, &open_rs("t-1", "p-r1"));
        fold(&mut chat, &reasoning_delta("t-1", "p-r1", "thinking"));
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        let segment = nodes
            .iter()
            .find_map(|node| match &node.kind {
                ConvItem::Thinking(t) => Some(t),
                _ => None,
            })
            .expect("the reasoning round forms a live segment");
        assert!(
            segment.started_at.elapsed().as_secs().abs_diff(90) <= 1,
            "the header counts from the turn's start stamp, not first sight"
        );
    }

    /// An unparseable start stamp must not panic: the clock falls back to the
    /// instant this client first saw the turn.
    #[test]
    fn an_unparseable_start_stamp_falls_back_to_first_sight() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "hi"));
        chat.active_turn
            .as_mut()
            .expect("the turn is in flight")
            .started_at = "not-a-time".to_string();
        let (env, _host) = env();
        let mut conversation = ConversationState::new(manox_agent::MessageAuthor::Lead);
        let _ = conversation.stage(Some(&chat), &env);
        assert!(
            conversation.running_secs() <= 1,
            "first sight seeds the fallback anchor"
        );
    }

    // ── the reconcile face: gpui entities, in-place updates, adoption ─────

    type ConvEntity = gpui::Entity<ConversationState>;

    fn projected(cx: &mut gpui::App, chat: &ChatState) -> ConvEntity {
        let (env, _host) = env();
        let recipient = manox_agent::MessageAuthor::Lead;
        let mut conversation = ConversationState::new(recipient);
        let nodes = conversation.stage(Some(chat), &env);
        conversation.reconcile(nodes, &env, cx);
        cx.new(|_| conversation)
    }

    fn resync(conversation: &ConvEntity, chat: &ChatState, cx: &mut gpui::App) -> ProjectionChange {
        let (env, _host) = env();
        conversation.update(cx, |c, cx| {
            let nodes = c.stage(Some(chat), &env);
            c.reconcile(nodes, &env, cx)
        })
    }

    #[gpui::test]
    fn a_streaming_delta_updates_the_row_in_place(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "hi"));
        fold(&mut chat, &open_md("t-1", "p-1"));
        fold(&mut chat, &delta("t-1", "p-1", "hel"));
        let conversation = cx.update(|cx| projected(cx, &chat));
        let keys = cx.read(|cx| conversation.read(cx).keys().to_vec());
        assert_eq!(
            keys,
            vec![ItemKey::User("t-1".into()), ItemKey::Answer("p-1".into())]
        );

        fold(&mut chat, &delta("t-1", "p-1", "lo "));
        fold(&mut chat, &delta("t-1", "p-1", "world"));
        let change = cx.update(|cx| resync(&conversation, &chat, cx));
        assert!(!change.count_changed, "the answer row updates in place");
        let text = cx.read(|cx| conversation.read(cx).items()[1].read(cx).kind().clone());
        let ConvItem::Assistant { text, .. } = text else {
            panic!("an answer row");
        };
        assert_eq!(text, "hello world");
    }

    #[gpui::test]
    fn a_settling_turn_folds_the_segment_immediately(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "x"));
        fold(&mut chat, &open_rs("t-1", "p-r1"));
        fold(&mut chat, &reasoning_delta("t-1", "p-r1", "think"));
        let conversation = cx.update(|cx| projected(cx, &chat));
        let streaming = cx.read(
            |cx| match conversation.read(cx).items()[1].read(cx).kind() {
                ConvItem::Thinking(t) => t.streaming,
                _ => panic!("a segment"),
            },
        );
        assert!(streaming, "live while the turn runs");

        fold(&mut chat, &complete("t-1", 4_000));
        let change = cx.update(|cx| resync(&conversation, &chat, cx));
        assert!(change.any(), "the settle mutates the rows");
        let (streaming, collapsed) =
            cx.read(
                |cx| match conversation.read(cx).items()[2].read(cx).kind() {
                    ConvItem::Thinking(t) => (t.streaming, t.collapsed),
                    _ => panic!("a segment"),
                },
            );
        // The turn landed in `turns`, so the row is now the segment at a new
        // index (the head inserted above it).
        assert!(!streaming, "settled");
        assert!(collapsed, "the derived fold is immediate — no 1s timer");
    }

    #[gpui::test]
    fn an_optimistic_bubble_is_adopted_by_its_turn(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (env, _host) = env();
        let conversation = cx.new(|_| ConversationState::new(manox_agent::MessageAuthor::Lead));
        conversation.update(&mut *cx, |c, cx| {
            c.push_optimistic_user(
                None,
                crate::transcript::OverlayBubbleSeed {
                    text: "send one".into(),
                    images: Vec::new(),
                    meta: UserTurnMeta::new(0, "test-model".into(), None),
                    turn_id: None,
                    steer_id: None,
                },
                &env,
                cx,
            );
        });
        let keys = cx.read(|cx| conversation.read(cx).keys().to_vec());
        assert_eq!(keys, vec![ItemKey::Optimistic(0)]);

        // The turn lands with the exact text: the state row adopts the
        // bubble's entity (the entrance animation and element ids survive).
        let item_before = cx.read(|cx| conversation.read(cx).items()[0].clone());
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "send one"));
        cx.update(|cx| resync(&conversation, &chat, cx));
        let item_after = cx.read(|cx| conversation.read(cx).items()[0].clone());
        let keys = cx.read(|cx| conversation.read(cx).keys().to_vec());
        assert_eq!(keys, vec![ItemKey::User("t-1".into())]);
        assert_eq!(
            item_before, item_after,
            "adoption reuses the bubble's entity"
        );

        // A second send of the same text parks a fresh bubble. The text
        // fallback pairs it only with a turn minted AFTER the bubble (a
        // steer waking its own run), so the earlier turn cannot swallow it.
        conversation.update(&mut *cx, |c, cx| {
            c.push_optimistic_user(
                Some(&chat),
                crate::transcript::OverlayBubbleSeed {
                    text: "send one".into(),
                    images: Vec::new(),
                    meta: UserTurnMeta::new(1, "test-model".into(), None),
                    turn_id: None,
                    steer_id: None,
                },
                &env,
                cx,
            );
        });
        // The turn that adopted the first bubble completes; the next one
        // starts (a real turn ends before the next begins — an abandoned
        // active turn is replaced in the fold, not archived).
        fold(&mut chat, &complete("t-1", 1_000));
        fold(&mut chat, &start("t-2", "send one"));
        cx.update(|cx| resync(&conversation, &chat, cx));
        let keys = cx.read(|cx| conversation.read(cx).keys().to_vec());
        assert_eq!(
            keys,
            vec![ItemKey::User("t-1".into()), ItemKey::User("t-2".into())],
            "the second send pairs with the turn minted after it, never with t-1"
        );
    }

    /// A parked call the workspace promotes renders as its decision card and
    /// nothing else — never the card plus its own segment entry back to back.
    /// (Review probe `probe_promoted_confirmation_renders_once`.)
    #[test]
    fn a_promoted_confirmation_renders_once() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "run it"));
        let tool: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallStart", "turnId": "t-1", "toolCallId": "tc-1",
            "toolName": "Bash", "displayName": "Bash", "intention": "rm -rf build",
        }))
        .unwrap();
        fold(&mut chat, &tool);
        let ask: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallReady", "turnId": "t-1", "toolCallId": "tc-1",
            "invocationMessage": "rm -rf build",
        }))
        .unwrap();
        fold(&mut chat, &ask);
        fold(&mut chat, &open_md("t-1", "p-1"));
        fold(&mut chat, &delta("t-1", "p-1", "done"));

        let (env_plain, _h1) = env();
        let mut overlays = Overlays::default();
        let plain = keys_of(&project(Some(&chat), &mut overlays, &env_plain));
        assert!(
            plain.iter().any(|k| k == "seg:tc-1"),
            "unpromoted: the call sits in its segment: {plain:?}"
        );

        let (mut env_promoted, _h2) = env();
        env_promoted.promoted_confirmation = Some("tc-1");
        let mut overlays = Overlays::default();
        let promoted = keys_of(&project(Some(&chat), &mut overlays, &env_promoted));
        assert_eq!(
            promoted.iter().filter(|k| k.as_str() == "tc:tc-1").count(),
            1,
            "the card renders once: {promoted:?}"
        );
        assert!(
            !promoted.iter().any(|k| k == "seg:tc-1"),
            "and not also as its own segment entry: {promoted:?}"
        );
    }

    /// A re-sent wording is not swallowed by the archived turn that already
    /// carried it: the fresh bubble stays on screen until its own turn
    /// lands. (Review probe `probe_resend_does_not_adopt_the_older_turn`.)
    #[gpui::test]
    fn a_resend_does_not_adopt_an_older_turn(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (env, _host) = env();
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "继续"));
        fold(&mut chat, &open_md("t-1", "p-1"));
        fold(&mut chat, &delta("t-1", "p-1", "好的"));
        fold(&mut chat, &complete("t-1", 1_000));
        let conversation = cx.update(|cx| projected(cx, &chat));
        assert_eq!(
            cx.read(|cx| conversation.read(cx).keys().to_vec()),
            vec![ItemKey::User("t-1".into()), ItemKey::Answer("p-1".into())]
        );

        cx.update(|cx| {
            conversation.update(cx, |c, cx| {
                c.push_optimistic_user(
                    Some(&chat),
                    crate::transcript::OverlayBubbleSeed {
                        text: "继续".into(),
                        images: Vec::new(),
                        meta: UserTurnMeta::new(2_000, "test-model".into(), None),
                        turn_id: None,
                        steer_id: None,
                    },
                    &env,
                    cx,
                );
            });
        });
        assert_eq!(
            cx.read(|cx| conversation.read(cx).keys().to_vec()),
            vec![
                ItemKey::User("t-1".into()),
                ItemKey::Answer("p-1".into()),
                ItemKey::Optimistic(0),
            ],
            "the fresh send keeps its own row instead of being absorbed by t-1"
        );

        // Its own turn lands and adopts it.
        fold(&mut chat, &start("t-2", "继续"));
        cx.update(|cx| resync(&conversation, &chat, cx));
        assert_eq!(
            cx.read(|cx| conversation.read(cx).keys().to_vec()),
            vec![
                ItemKey::User("t-1".into()),
                ItemKey::Answer("p-1".into()),
                ItemKey::User("t-2".into()),
            ],
            "the bubble pairs with the turn minted after it"
        );
    }

    /// A `/skill`-shaped send: the bubble shows the compact invocation while
    /// the journal folds the expanded body. The minted turn id pairs them.
    #[gpui::test]
    fn an_identity_bubble_adopts_a_turn_with_a_different_body(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (env, _host) = env();
        let conversation = cx.new(|_| ConversationState::new(manox_agent::MessageAuthor::Lead));
        let mut chat = empty_chat();
        cx.update(|cx| {
            conversation.update(cx, |c, cx| {
                c.push_optimistic_user(
                    Some(&chat),
                    crate::transcript::OverlayBubbleSeed {
                        text: "/skill demo".into(),
                        images: Vec::new(),
                        meta: UserTurnMeta::new(0, "test-model".into(), None),
                        turn_id: Some("t-expanded".into()),
                        steer_id: None,
                    },
                    &env,
                    cx,
                );
            });
        });
        // The host journals the macro's expansion, not the compact form.
        fold(
            &mut chat,
            &start("t-expanded", "# Skill: demo\n\nfull body…"),
        );
        cx.update(|cx| resync(&conversation, &chat, cx));
        assert_eq!(
            cx.read(|cx| conversation.read(cx).keys().to_vec()),
            vec![ItemKey::User("t-expanded".into())],
            "identity adoption never looks at the text"
        );
    }

    /// A refused submit retires the bubble that turn was minted for, so the
    /// notice explains something instead of standing beside a phantom row.
    #[gpui::test]
    fn a_refusal_retires_the_bubble_it_refused(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (env, _host) = env();
        let conversation = cx.new(|_| ConversationState::new(manox_agent::MessageAuthor::Lead));
        cx.update(|cx| {
            conversation.update(cx, |c, cx| {
                c.push_optimistic_user(
                    None,
                    crate::transcript::OverlayBubbleSeed {
                        text: "dropped".into(),
                        images: Vec::new(),
                        meta: UserTurnMeta::new(0, "test-model".into(), None),
                        turn_id: Some("t-refused".into()),
                        steer_id: None,
                    },
                    &env,
                    cx,
                );
            });
        });
        cx.update(|cx| {
            conversation.update(cx, |c, cx| {
                c.absorb_refusals(vec![crate::transcript::RefusalNotice {
                    seq: 7,
                    text: "rejected: nope".into(),
                    turn_id: Some("t-refused".into()),
                }]);
                c.project(None, &env, cx);
            });
        });
        assert_eq!(
            cx.read(|cx| conversation.read(cx).keys().to_vec()),
            vec![ItemKey::Notice("rej:7".into())],
            "the refused bubble leaves; only the notice remains"
        );
    }

    /// The host re-keys a turn when it re-folds it (`t-{entry}` instead of the
    /// client's id). Identity alone would strand the bubble, so the text
    /// fallback covers id-bearing bubbles too — still only above `minted_turns`.
    #[gpui::test]
    fn a_rewritten_turn_id_still_adopts_by_text(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let mut chat = empty_chat();
        // An older turn with the very same body: the guard must keep the
        // fallback off it.
        fold(&mut chat, &start("t-0", "hello"));
        fold(&mut chat, &complete("t-0", 1_000));
        let conversation = cx.update(|cx| projected(cx, &chat));
        let (env, _host) = env();
        conversation.update(&mut *cx, |c, cx| {
            c.push_optimistic_user(
                Some(&chat),
                crate::transcript::OverlayBubbleSeed {
                    text: "hello".into(),
                    images: Vec::new(),
                    meta: UserTurnMeta::new(0, "test-model".into(), None),
                    turn_id: Some("uuid-client".into()),
                    steer_id: None,
                },
                &env,
                cx,
            );
        });
        assert_eq!(
            cx.read(|cx| conversation.read(cx).keys().to_vec()),
            vec![ItemKey::User("t-0".into()), ItemKey::Optimistic(0)],
            "the older turn must not claim the re-sent text"
        );

        // The host re-keys the turn it started for our submit.
        fold(&mut chat, &start("t-7", "hello"));
        cx.update(|cx| resync(&conversation, &chat, cx));
        assert_eq!(
            cx.read(|cx| conversation.read(cx).keys().to_vec()),
            vec![ItemKey::User("t-0".into()), ItemKey::User("t-7".into())],
            "the rewritten id still adopts the bubble"
        );
    }

    /// A re-delivered refusal (the store's table drains, so the same seq is
    /// recorded again) must not project a second row under the same key.
    #[gpui::test]
    fn a_redelivered_refusal_renders_one_row(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let conversation = cx.update(|cx| projected(cx, &empty_chat()));
        let (env, _host) = env();
        for _ in 0..2 {
            conversation.update(&mut *cx, |c, cx| {
                c.absorb_refusals(vec![crate::transcript::RefusalNotice {
                    seq: 7,
                    text: "rejected: nope".into(),
                    turn_id: None,
                }]);
                let nodes = c.stage(None, &env);
                c.reconcile(nodes, &env, cx);
            });
        }
        assert_eq!(
            cx.read(|cx| conversation.read(cx).keys().to_vec()),
            vec![ItemKey::Notice("rej:7".into())],
            "one seq, one row"
        );
    }

    #[gpui::test]
    fn reconcile_reports_the_diff_decisions(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "hi"));
        let conversation = cx.update(|cx| projected(cx, &chat));

        // Append: count changes, nothing remeasures in place.
        fold(&mut chat, &open_md("t-1", "p-1"));
        fold(&mut chat, &delta("t-1", "p-1", "hey"));
        let change = cx.update(|cx| resync(&conversation, &chat, cx));
        assert!(change.count_changed);
        assert!(
            change.changed.is_empty(),
            "an append mutates no existing row"
        );
        assert!(!change.structural);

        // Content change on the tail: precise remeasure, no count change.
        fold(&mut chat, &delta("t-1", "p-1", " there"));
        let change = cx.update(|cx| resync(&conversation, &chat, cx));
        assert!(!change.count_changed);
        assert_eq!(change.changed, vec![1], "the answer row remeasures");
        assert!(!change.structural);

        // No-op fold: nothing at all.
        let change = cx.update(|cx| resync(&conversation, &chat, cx));
        assert!(!change.any(), "an idempotent projection is a no-op");
    }

    #[gpui::test]
    fn user_toggles_survive_re_projection(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "x"));
        fold(&mut chat, &open_rs("t-1", "p-r1"));
        fold(&mut chat, &reasoning_delta("t-1", "p-r1", "think"));
        let conversation = cx.update(|cx| projected(cx, &chat));

        // The user pins the round open (the render layer's in-place toggle).
        cx.update(|cx| {
            let item = conversation.read(cx).items()[1].clone();
            item.update(cx, |item, _| {
                if let ConvItem::Thinking(t) = item.kind_mut()
                    && let Some(ActivityEntry::Reasoning {
                        collapsed,
                        user_toggled,
                        ..
                    }) = t.entries.first_mut()
                {
                    *collapsed = false;
                    *user_toggled = true;
                }
            });
        });

        // The settle folds everything unpinned — the pin survives.
        fold(&mut chat, &complete("t-1", 1_000));
        cx.update(|cx| resync(&conversation, &chat, cx));
        let pinned_open = cx.read(
            |cx| match conversation.read(cx).items()[2].read(cx).kind() {
                ConvItem::Thinking(t) => match t.entries.first() {
                    Some(ActivityEntry::Reasoning {
                        collapsed,
                        user_toggled,
                        ..
                    }) => *user_toggled && !*collapsed,
                    _ => false,
                },
                _ => panic!("a segment"),
            },
        );
        assert!(
            pinned_open,
            "user_toggled always wins over the derived fold"
        );
    }

    // ── the one-shot equivalence reference ─────────────────────────────────
    //
    /// The retired dual path (a `HistoryEntry` display lowered by the old
    /// `ItemBuilder`) reimplemented here at fixture scale, so the projection's
    /// output can be compared against what a rebuild used to produce. This
    /// reference dies with the PR that introduced the projection — it exists
    /// to prove the convergence, then it is dead weight.
    mod reference {
        use super::*;

        /// The old rebuild's segment rule: tool uses aggregate into one
        /// segment per turn until a text block closes it.
        pub(super) fn lower(chat: &ChatState) -> Vec<ConvItem> {
            let mut items = Vec::new();
            let turn = chat.turns.first();
            let Some(turn) = turn else {
                return items;
            };
            items.push(ConvItem::User {
                text: turn.message.text.clone(),
                images: Vec::new(),
                meta: None,
            });
            let mut segment: Vec<ActivityEntry> = Vec::new();
            let close = |items: &mut Vec<ConvItem>, segment: &mut Vec<ActivityEntry>| {
                if !segment.is_empty() {
                    let mut container = ThinkingContainer::new();
                    container.entries = std::mem::take(segment);
                    container.accepting_entries = false;
                    container.streaming = false;
                    container.collapsed = false;
                    items.push(ConvItem::Thinking(container));
                }
            };
            for part in &turn.response_parts {
                match part {
                    ahp_types::state::ResponsePart::Markdown(md) if !md.content.is_empty() => {
                        close(&mut items, &mut segment);
                        items.push(ConvItem::Assistant {
                            text: md.content.clone(),
                            streaming: false,
                            token_usage: None,
                            activity_header: false,
                            entry_id: None,
                            fork_unavailable: None,
                        });
                    }
                    ahp_types::state::ResponsePart::Reasoning(reasoning)
                        if !reasoning.content.is_empty() =>
                    {
                        segment.push(ActivityEntry::Reasoning {
                            id: crate::transcript::reasoning_entry_id(&reasoning.id),
                            text: reasoning.content.clone(),
                            streaming: false,
                            collapsed: false,
                            user_toggled: false,
                            markdown: None,
                        });
                    }
                    ahp_types::state::ResponsePart::ToolCall(call) => {
                        let state = &call.tool_call;
                        let (name, output, success) = match state {
                            ahp_types::state::ToolCallState::Completed(c) => (
                                c.tool_name.clone(),
                                crate::transcript::tool_result_text(c.content.as_deref()),
                                c.success,
                            ),
                            ahp_types::state::ToolCallState::Running(r) => {
                                (r.tool_name.clone(), String::new(), true)
                            }
                            _ => (String::new(), String::new(), true),
                        };
                        segment.push(ActivityEntry::Tool(ToolCallItem {
                            id: crate::transcript::tool_call_id_of(state).to_string(),
                            name,
                            title: String::new(),
                            status: if matches!(
                                state,
                                ahp_types::state::ToolCallState::Completed(_)
                            ) {
                                if success {
                                    ToolCallStatus::Success
                                } else {
                                    ToolCallStatus::Error
                                }
                            } else {
                                ToolCallStatus::Running
                            },
                            output,
                            is_error: !success,
                            input: serde_json::Value::Null,
                            streaming: !matches!(
                                state,
                                ahp_types::state::ToolCallState::Completed(_)
                            ),
                            collapsed: true,
                            user_toggled: false,
                            panel: None,
                            card: None,
                            cwd: None,
                            stream: Default::default(),
                        }));
                    }
                    ahp_types::state::ResponsePart::SystemNotification(note) => {
                        close(&mut items, &mut segment);
                        items.push(ConvItem::Notice(note.content.as_text().to_string()));
                    }
                    _ => {}
                }
            }
            close(&mut items, &mut segment);
            items
        }

        /// Structural equality the two vocabularies can share: kind shape,
        /// order, and content text.
        pub(super) fn shape(items: &[ConvItem]) -> Vec<String> {
            items
                .iter()
                .map(|item| match item {
                    ConvItem::User { text, .. } => format!("user:{text}"),
                    ConvItem::Assistant { text, .. } => format!("answer:{text}"),
                    ConvItem::Thinking(t) => format!(
                        "segment:{}",
                        t.entries
                            .iter()
                            .map(|e| match e {
                                ActivityEntry::Reasoning { text, .. } => {
                                    format!("rs:{text}")
                                }
                                ActivityEntry::Tool(tool) =>
                                    format!("tc:{}:{}", tool.name, tool.output),
                            })
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    ConvItem::Notice(text) => format!("notice:{text}"),
                    _ => "other".into(),
                })
                .collect()
        }
    }

    /// The convergence proof: on history-shaped fixtures the projection and
    /// the retired rebuild lower to the same structure.
    #[test]
    fn the_projection_matches_the_retired_rebuild_on_history_fixtures() {
        let fixtures: Vec<ChatState> = [
            // A plain Q&A turn.
            serde_json::json!({
                "id": "t-1",
                "message": { "text": "q1", "origin": { "kind": "user" } },
                "responseParts": [
                    { "kind": "markdown", "id": "p-1", "content": "answer one" },
                ],
                "state": "complete",
            }),
            // A tool loop: reasoning, two tools, then the reply.
            serde_json::json!({
                "id": "t-2",
                "message": { "text": "q2", "origin": { "kind": "user" } },
                "responseParts": [
                    { "kind": "reasoning", "id": "p-r1", "content": "think" },
                    { "kind": "toolCall", "toolCall": {
                        "status": "completed", "toolCallId": "tc-1", "toolName": "Read",
                        "displayName": "Read", "invocationMessage": "Read",
                        "toolInput": "{}", "success": true,
                        "pastTenseMessage": "Read", "confirmed": "not-needed",
                        "content": [{ "type": "text", "text": "file body" }] }},
                    { "kind": "toolCall", "toolCall": {
                        "status": "running", "toolCallId": "tc-2", "toolName": "Bash",
                        "displayName": "Bash", "invocationMessage": "run",
                        "confirmed": "approved" }},
                    { "kind": "markdown", "id": "p-m1", "content": "the reply" },
                ],
                "state": "complete",
            }),
            // A turn with a system notification between the parts.
            serde_json::json!({
                "id": "t-3",
                "message": { "text": "q3", "origin": { "kind": "user" } },
                "responseParts": [
                    { "kind": "markdown", "id": "p-1", "content": "part one" },
                    { "kind": "systemNotification", "content": "a note" },
                    { "kind": "markdown", "id": "p-2", "content": "part two" },
                ],
                "state": "complete",
            }),
        ]
        .into_iter()
        .map(|turn| {
            let mut chat = empty_chat();
            fold(&mut chat, &start("t", "ignored"));
            // Replace the fold with the fixture turn directly.
            let _ = &turn;
            serde_json::from_value::<ChatState>(serde_json::json!({
                "resource": "ahp-chat:/c-test",
                "title": "test",
                "status": 0,
                "modifiedAt": "2026-01-01T00:00:00Z",
                "turns": [turn],
            }))
            .expect("the fixture turn parses")
        })
        .collect();

        for chat in &fixtures {
            let (env, _host) = env();
            let mut overlays = Overlays::default();
            let nodes = project(Some(chat), &mut overlays, &env);
            let projected_shapes: Vec<String> = nodes
                .iter()
                .filter(|n| !matches!(n.key, ItemKey::TurnProcess(_)))
                .map(|n| match &n.kind {
                    ConvItem::User { text, .. } => format!("user:{text}"),
                    ConvItem::Assistant { text, .. } => format!("answer:{text}"),
                    ConvItem::Thinking(t) => format!(
                        "segment:{}",
                        t.entries
                            .iter()
                            .map(|e| match e {
                                ActivityEntry::Reasoning { text, .. } => {
                                    format!("rs:{text}")
                                }
                                ActivityEntry::Tool(tool) => {
                                    format!("tc:{}:{}", tool.name, tool.output)
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    ConvItem::Notice(text) => format!("notice:{text}"),
                    other => format!("other:{other:?}"),
                })
                .collect();
            let reference_shapes = reference::shape(&reference::lower(chat));
            assert_eq!(
                projected_shapes, reference_shapes,
                "the projection converges with the retired rebuild"
            );
        }
    }

    // ── the unified Steer surface: dispatch cards, the guided note, the
    //    child's injected row ──────────────────────────────────────────────

    fn steer_start(turn: &str, call: &str) -> StateAction {
        serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallStart", "turnId": turn, "toolCallId": call,
            "toolName": "Steer", "displayName": "Steer",
            "intention": "Steer",
        }))
        .expect("the tool start parses")
    }

    fn steer_ready(turn: &str, call: &str, input: serde_json::Value) -> StateAction {
        serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallReady", "turnId": turn, "toolCallId": call,
            "invocationMessage": "Steer",
            "toolInput": input.to_string(),
            "confirmed": "not-needed",
        }))
        .expect("the tool ready parses")
    }

    fn steer_done(turn: &str, call: &str, ack: serde_json::Value) -> StateAction {
        serde_json::from_value(serde_json::json!({
            "type": "chat/toolCallComplete", "turnId": turn, "toolCallId": call,
            "result": {
                "success": true,
                "pastTenseMessage": "Steered",
                "content": [{ "type": "text", "text": ack.to_string() }],
            },
        }))
        .expect("the tool complete parses")
    }

    fn task_card(nodes: &[Node]) -> &crate::conversation::AgentTaskItem {
        nodes
            .iter()
            .find_map(|n| match &n.kind {
                ConvItem::AgentTask(task) => Some(task),
                _ => None,
            })
            .expect("the dispatch projects a task card")
    }

    /// A create dispatch projects the agent-task card: the type is the
    /// spawned definition (the envelope's `to.spawn` — the `run/started`
    /// row's kind), the label the call's own `description`, and the
    /// completed card drills into the ack's run id.
    #[test]
    fn a_steer_dispatch_projects_the_agent_task_card() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "sweep"));
        fold(&mut chat, &steer_start("t-1", "tc-1"));
        fold(
            &mut chat,
            &steer_ready(
                "t-1",
                "tc-1",
                serde_json::json!({
                    "to": { "spawn": "Explore" },
                    "reason": "Dispatch",
                    "prompt": "find every caller",
                    "description": "call-site sweep",
                }),
            ),
        );
        fold(
            &mut chat,
            &steer_done(
                "t-1",
                "tc-1",
                serde_json::json!({
                    "kind": "foreground",
                    "run_id": "sub-3",
                    "stop_reason": "completed",
                    "output": "done",
                }),
            ),
        );
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        let task = task_card(&nodes);
        assert_eq!(task.subagent_type, "Explore", "the type is the spawn");
        assert_eq!(task.description, "call-site sweep");
        assert_eq!(
            task.run_id.as_deref(),
            Some("sub-3"),
            "the ack names the run"
        );
        assert!(!task.continuation, "a create is not a follow-up");
        assert_eq!(task.status, ToolCallStatus::Success);
    }

    /// A continuation dispatch reads as the follow-up it is: the envelope's
    /// address names the run BEFORE any ack lands, and the completed ack
    /// carries the same run id — the card joins the creating card onto one
    /// child conversation (the `run/continued` row's transcript face).
    #[test]
    fn a_continuation_dispatch_reads_as_a_follow_up_on_the_same_run() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "rework"));
        fold(&mut chat, &steer_start("t-1", "tc-1"));
        fold(
            &mut chat,
            &steer_ready(
                "t-1",
                "tc-1",
                serde_json::json!({
                    "to": { "agent_address": "sub-3" },
                    "reason": "Dispatch",
                    "prompt": "now also check the tests",
                }),
            ),
        );
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        let task = task_card(&nodes);
        assert!(
            task.continuation,
            "the addressed dispatch is marked a follow-up"
        );
        assert_eq!(
            task.run_id.as_deref(),
            Some("sub-3"),
            "the address names the run before any ack"
        );

        fold(
            &mut chat,
            &steer_done(
                "t-1",
                "tc-1",
                serde_json::json!({
                    "kind": "foreground",
                    "run_id": "sub-3",
                    "stop_reason": "completed",
                    "output": "done",
                }),
            ),
        );
        let nodes = project(Some(&chat), &mut overlays, &env);
        let task = task_card(&nodes);
        assert_eq!(
            task.run_id.as_deref(),
            Some("sub-3"),
            "the settled ack keeps the same run id"
        );
        assert!(task.continuation);
    }

    /// Inject and Abort are not task cards: they fold into the activity
    /// segment like any other tool call — the guidance's durable face is
    /// the `run/guided` note, not the verb's call frame.
    #[test]
    fn steer_inject_and_abort_do_not_project_task_cards() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "watch"));
        fold(&mut chat, &steer_start("t-1", "tc-1"));
        fold(
            &mut chat,
            &steer_ready(
                "t-1",
                "tc-1",
                serde_json::json!({
                    "to": { "agent_address": "sub-3" },
                    "reason": "Inject",
                    "prompt": "focus on the projection layer",
                }),
            ),
        );
        fold(&mut chat, &steer_start("t-1", "tc-2"));
        fold(
            &mut chat,
            &steer_ready(
                "t-1",
                "tc-2",
                serde_json::json!({
                    "to": { "agent_address": "sub-3" },
                    "reason": "Abort",
                    "prompt": "",
                }),
            ),
        );
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        assert!(
            nodes
                .iter()
                .all(|n| !matches!(n.kind, ConvItem::AgentTask(_))),
            "no dispatch card for the guide and stop verbs"
        );
        let segment = nodes.iter().find_map(|n| match &n.kind {
            ConvItem::Thinking(t) => Some(t),
            _ => None,
        });
        let segment = segment.expect("the verbs fold into a segment");
        assert_eq!(segment.entries.len(), 2, "both verbs are segment entries");
    }

    /// The `run/guided` row's captain-transcript face: the upstream fold
    /// rides the guidance onto a systemNotification part naming the run,
    /// and the projection renders it as a notice row, verbatim — runtime
    /// text is never re-localized.
    #[test]
    fn the_run_guided_note_projects_a_captain_notice_row() {
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-1", "watch"));
        let guided: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/responsePart", "turnId": "t-1",
            "part": {
                "kind": "systemNotification",
                "content": "Steered run `sub-3`: focus on the projection layer only",
                "_meta": { "x-manox": { "runGuided": { "runId": "sub-3" } } },
            },
        }))
        .expect("the guided note part parses");
        fold(&mut chat, &guided);
        let (env, _host) = env();
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        let notice = nodes.iter().find_map(|n| match &n.kind {
            ConvItem::Notice(text) => Some(text.clone()),
            _ => None,
        });
        assert_eq!(
            notice.as_deref(),
            Some("Steered run `sub-3`: focus on the projection layer only"),
            "the note renders verbatim, naming the guided run"
        );
    }

    /// The child's own face of the same guidance: the injected message
    /// opens its own turn on the child fold (`chat/turnStarted` threading
    /// the steer's pending id), and that opening row carries the steered
    /// badge — the store derives the set from the envelopes, the projection
    /// stamps it.
    #[test]
    fn a_steered_injection_opens_a_badged_child_row() {
        let mut chat = empty_chat();
        let injected: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/turnStarted", "turnId": "t-steer",
            "startedAt": "2026-01-01T00:00:00Z",
            "queuedMessageId": "steer-1",
            "message": {
                "text": "focus on the projection layer only",
                "origin": { "kind": "user" },
            },
        }))
        .expect("the injected turn start parses");
        fold(&mut chat, &injected);
        let (env, _host) = env_with_steered(&["t-steer"]);
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        let Some(ConvItem::User {
            meta: Some(meta), ..
        }) = nodes.first().map(|n| &n.kind)
        else {
            panic!("the injected message opens the row");
        };
        assert!(meta.steered, "the injection row carries the steer badge");

        // A plain turn on the same child carries no badge.
        let mut chat = empty_chat();
        fold(&mut chat, &start("t-plain", "typed send"));
        let (env, _host) = env_with_steered(&["t-steer"]);
        let mut overlays = Overlays::default();
        let nodes = project(Some(&chat), &mut overlays, &env);
        let Some(ConvItem::User {
            meta: Some(meta), ..
        }) = nodes.first().map(|n| &n.kind)
        else {
            panic!("the plain send opens the row");
        };
        assert!(!meta.steered);
    }
}
