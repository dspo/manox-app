//! The chat column's state types (PLAN-CHROME-CHAT-SPLIT Phase 2 tail):
//! the `ChatColumn` entity's fields, the ask/queue/recall state families it
//! carries, and the `AskUserQuestion` payload parser. The struct lives here;
//! the workspace (agent-ui) embeds it as an `Entity<ChatColumn>` and
//! orchestrates the wire-facing halves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{Entity, FocusHandle, ListState, Subscription};
use gpui_component::input::{InputState, TextareaState};
use gpui_component::menu::PopupMenu;

use crate::client_store_handle::ClientStoreHandle;
use crate::conversation::{ConversationState, UserImage, UserTurnMeta};
use crate::host::ChatHostHandle;
use crate::views::completion::CompletionState;
use crate::views::message::MessageItem;
use crate::views::turn_navigator::TurnNavigator;

/// Parse an `AskUserQuestion` tool input into a `PendingAsk`. The per-question
/// `InputState` entities are allocated lazily on first render (they need a
/// `Window`, which the event handler lacks). Returns `None` when the input is
/// malformed (the generic question overlay then takes over as a fallback).
pub fn parse_pending_ask(id: String, input: serde_json::Value) -> Option<PendingAsk> {
    let questions = input.get("questions")?.as_array()?;
    // B2-PR-1 removed the 1..=3 question cap (and the 2..=3 option cap) from
    // the tool contract; the card steps through any count. Empty stays
    // malformed.
    if questions.is_empty() {
        return None;
    }
    let mut parsed: Vec<AskQuestion> = Vec::with_capacity(questions.len());
    let mut selections: Vec<Vec<bool>> = Vec::with_capacity(questions.len());
    for (i, q) in questions.iter().enumerate() {
        let question = q.get("question")?.as_str()?.to_string();
        // The server mints a stable id onto each parked question; answers are
        // id-routed and unknown ids are dropped at the settle boundary.
        // Inputs predating the mint (fixtures, older servers) fall back to a
        // positional id, mirroring how the card keys its per-step state. The
        // positional fallback is index-derived (`q{i}`) — a fixed small set of
        // names collided past 3 questions once the count cap was lifted, which
        // made two answers share an id and mis-route at the settle boundary.
        let id = match q
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            Some(explicit) => explicit.to_string(),
            None => format!("q{i}"),
        };
        // B2-PR-1 L1 vocabulary: `detail` is optional markdown support text
        // rendered beneath the question; `intent` names a specialised surface
        // (`kind`, e.g. "plan-review") with the option label that carries the
        // affirmative verdict (`approve`). Both ride the snapshot so the card
        // renders them; later PRs branch on `intent`.
        let detail = q
            .get("detail")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let intent = q.get("intent").and_then(|v| v.as_object()).map(|obj| {
            let read = |k: &str| {
                obj.get(k)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            };
            AskIntent {
                kind: read("kind"),
                approve: read("approve"),
            }
        });
        let header = q
            .get("header")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let multi_select = q
            .get("multiSelect")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut opts: Vec<AskOption> = Vec::new();
        if let Some(arr) = q.get("options").and_then(|v| v.as_array()) {
            for o in arr {
                let raw_label = o
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let description = o
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let explicit_recommended = o
                    .get("recommended")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let (label, suffix_recommended) = strip_recommended_suffix(raw_label);
                opts.push(AskOption {
                    label,
                    description,
                    recommended: explicit_recommended || suffix_recommended,
                });
            }
        }
        // B2-PR-1: options are optional and unbounded (a detail/intent-only
        // question is legal) — the old 2..=3 cap is gone server-side, so the
        // card must not degrade on counts it now receives.
        selections.push(vec![false; opts.len()]);
        parsed.push(AskQuestion {
            id,
            question,
            header,
            detail,
            intent,
            multi_select,
            options: opts,
        });
    }
    Some(PendingAsk {
        id,
        questions: parsed,
        selections,
    })
}

pub fn strip_recommended_suffix(label: String) -> (String, bool) {
    let lower = label.to_lowercase();
    for suffix in [" (Recommended)", "（推荐）", " (推荐)", "（Recommended）"] {
        let suffix_lower = suffix.to_lowercase();
        if lower.ends_with(&suffix_lower) {
            let stripped = &label[..label.len() - suffix.len()];
            return (stripped.trim().to_string(), true);
        }
    }
    (label, false)
}

/// A non-question authorization parked on the user's decision — a
/// `sandbox_permissions` escalation from Edit/Write/Bash, or an
/// `AskUserQuestion` whose payload failed to parse. The ask card only
/// renders question payloads, so without this surface the pending call
/// blocks invisibly until the turn is cancelled.
pub struct PendingAuth {
    pub id: String,
    pub tool_name: String,
    pub summary: String,
}

/// A parsed `AskUserQuestion` prompt awaiting the user's selections.
pub struct PendingAsk {
    pub id: String,
    pub questions: Vec<AskQuestion>,
    /// Per-question toggled option flags, aligned with `questions[i].options`.
    pub selections: Vec<Vec<bool>>,
}

pub struct AskQuestion {
    /// Stable question id (server-minted) used to route the canonical
    /// `AskAnswer` back through the settle boundary.
    pub id: String,
    pub question: String,
    pub header: String,
    pub detail: String,
    pub intent: Option<AskIntent>,
    pub multi_select: bool,
    pub options: Vec<AskOption>,
}

/// Parsed form of a question's `intent` object: the specialised-surface kind
/// (e.g. "plan-review") and the option label carrying the affirmative verdict.
pub struct AskIntent {
    pub kind: String,
    pub approve: String,
}

pub struct AskOption {
    pub label: String,
    pub description: String,
    pub recommended: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComposerPlaceholderMode {
    Normal,
    FollowUp,
    Ask,
}

pub struct DeferredUserTurn {
    pub text: String,
    pub images: Vec<manox_agent::language_model::MessageContent>,
    pub meta: UserTurnMeta,
    pub user_images: Vec<UserImage>,
}

/// Lifecycle of a follow-up submitted while a turn is running. A queued item
/// renders above the composer; clicking Steer promotes it to `SteerPending`,
/// which is handed to the server's steer queue for the running turn and STAYS
/// parked in the composer queue (at the head of the steer group) until the
/// model actually consumes it. Consumption is observed at the earliest point
/// the wire offers: the injected `user` journal row landing
/// (`ThreadEvent::UserRowLanded`, id == the client-minted `message_id` thanks
/// to the server's stable-id threading) retires the card immediately; the
/// turn-boundary `TurnFinished` (now journal-delivered) is the fallback for a
/// row that raced the settle, and the strand path for a cancelled turn.
pub enum FollowUpState {
    /// Parked, waiting to flush as the next user turn at the turn boundary (or
    /// to be promoted to a steer via the Steer action).
    Queued,
    /// Promoted to the server steer queue for the running turn. Carries the
    /// client-minted id sent with [`manox_protocol::ClientCall::Steer`]: the
    /// injected row's durable identity (the retire-on-injection key) and the
    /// stranded-verdict key at settle. Not removable (no steer-withdrawal
    /// channel in the protocol). A normal settle the injection row missed
    /// promotes it into the message list; a cancelled/failed turn strands it
    /// into [`FollowUpState::Failed`].
    SteerPending { message_id: String },
    /// The running turn exited abnormally (Abort/Error) before injecting the
    /// steer. Stays parked, marked red, retryable via the Steer action (which
    /// re-sends a fresh online steer under a fresh id). Removable. Carries no
    /// id: the retry never reuses the retracted one.
    Failed,
}

/// A follow-up submitted while a turn is running. Every new item starts queued;
/// only an explicit Steer action promotes it to `SteerPending`.
pub struct QueuedFollowUp {
    pub turn: DeferredUserTurn,
    pub state: FollowUpState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueueDragEdge {
    Top,
    Bottom,
}

/// In-flight queue-row drag: the row being dragged, the row whose edge
/// carries the insertion line, and which edge that is (the sidebar's
/// `RowDrag` shape, index-keyed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueueRowDrag {
    pub dragged: usize,
    pub line_on: usize,
    pub edge: QueueDragEdge,
}

/// An attachment staged in the composer but not yet submitted. Either a file
/// picked from the `+` menu or an image pasted straight from the clipboard
/// (resized off-thread on submit). Browser tool-suite chips are tracked
/// separately by the workspace (they persist across submits).
#[derive(Debug, Clone)]
pub enum PendingAttachment {
    File { path: PathBuf, is_image: bool },
    ClipboardImage(gpui::Image),
}

impl PendingAttachment {
    pub fn new(path: PathBuf) -> Self {
        Self::File {
            is_image: is_image_path(&path),
            path,
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(
            self,
            Self::ClipboardImage(_) | Self::File { is_image: true, .. }
        )
    }

    /// The chip's file label. No filename on the clipboard; a localized
    /// label stands in instead.
    pub fn file_name(&self) -> String {
        match self {
            Self::File { path, .. } => path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file")
                .to_string(),
            Self::ClipboardImage(_) => manox_i18n::t("composer-pasted-image"),
        }
    }
}

fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
    )
}

pub struct ChatColumn {
    /// The port the moved chat views call through (see `host.rs` in
    /// manox-agent-chat-ui and agent-ui's `WorkspaceChatHost`).
    pub host: ChatHostHandle,

    pub thread: manox_agent::thread::ThreadHandle,
    /// The `AgentServer`-backed `ClientStoreHandle` — the v2 `SessionStore`
    /// (journal window + projection face + echo map) fed by the multiplexer's
    /// follow stream. `None` until the workspace creates the AgentServer
    /// connection (landing thread); views read the store mirror. Held on
    /// the workspace for the next wiring step (re-handling the store on
    /// thread switch) — written at landing, read there.
    pub store: Option<gpui::Entity<ClientStoreHandle>>,
    /// γ-3: the AgentServer session_id for the landing thread. Used as the
    /// `session_id` field in `FromClient` commands.
    pub session_id: Option<String>,
    /// Generation counter for git-status refreshes: bumping it means any
    /// prior in-flight refresh self-cancels instead of overwriting newer
    /// state. The refresh runs on the global tokio runtime and delivers its
    /// result back via `async_channel`, the same bridge the worktree tool uses.
    pub git_status_gen: u64,
    pub conversation: Entity<ConversationState>,
    pub input_state: Entity<TextareaState>,
    /// Per-thread unsent composer text, keyed by thread id. Saved when
    /// switching away and restored on return, so each thread keeps its own
    /// in-progress draft instead of a single shared input bleeding across.
    pub drafts: HashMap<String, String>,
    /// Composer history-recall position into the newest-first user-turn texts;
    /// -1 means the walk is not running. Only `alt-up` / `alt-down` move along
    /// it, so nothing about the text or the caret has to be watched to leave it.
    pub recall_index: i64,
    /// The walk's working line: what the composer held when the walk was
    /// entered, or the last recalled turn once the user has changed it. `Down`
    /// past the newest turn restores it and ends the walk.
    pub recall_draft: Option<String>,
    /// A pending `AskUserQuestion` card rendered inline in the message list.
    pub pending_ask: Option<PendingAsk>,
    pub pending_auth: Option<PendingAuth>,
    /// Whether the CURRENT pending interaction's id has been observed in the
    /// leaf store's `pending_auth` projection set. Arms the remote-settle
    /// reconcile (chips.rs) so the startup race — Request landing before the
    /// projection frame — can never clear a freshly surfaced card.
    pub pending_projection_confirmed: bool,
    /// Tool row currently carrying the Workspace-derived ask snapshot. This is
    /// synchronized before list construction; the row factory itself remains
    /// a read-only projection during measurement and prepaint.
    pub ask_snapshot_item: Option<Entity<MessageItem>>,
    /// Current question index in the ask drawer (0-based).
    pub ask_step: usize,
    /// Animation generation counter for the ask drawer slide, bumped on every
    /// open/close so a fresh tween fires rather than replaying a cached delta.
    pub ask_transition_gen: u64,
    /// Per-question free-text `custom` inputs for the pending ask card, one
    /// slot per question (index-aligned with `pending_ask.questions`). Created
    /// lazily at render time because an `InputState` needs a `Window`, which
    /// the park event handler lacks; reset whenever the ask is (re)seeded or
    /// retired. A tri-state answer is `{selected, custom}` — `custom` overrides
    /// a single-select and supplements a multi-select at the settle fold, and
    /// an empty selection with no `custom` is an explicit skip.
    pub ask_custom_inputs: Vec<Option<Entity<InputState>>>,
    /// Subscriptions keeping the card repainted as the custom inputs change.
    pub ask_custom_subs: Vec<Subscription>,
    /// Authoritative per-question custom text (index-aligned with
    /// `pending_ask.questions`), the value `resolve_ask` folds
    /// into each canonical `AskAnswer`. The `InputState` entities above mirror
    /// this for live editing; tests drive this directly. Reset with the ask.
    pub ask_custom_text: Vec<String>,
    /// Per-question explicit-skip markers (index-aligned with
    /// `pending_ask.questions`): set by the footer Skip, cleared with the ask.
    /// The submit completeness gate reads them — an untouched question is
    /// never silently folded to a skip, but an explicitly skipped one counts
    /// as completed (dsh `QuestionDraftAnswer.skipped` parity).
    pub ask_skipped: Vec<bool>,
    pub model_open: bool,
    /// PopupMenu entity for the open model selector; created on open, destroyed on close.
    pub model_menu: Option<Entity<PopupMenu>>,
    pub model_menu_sub: Option<Subscription>,
    pub plus_open: bool,
    pub plus_menu: Option<Entity<PopupMenu>>,
    pub plus_menu_sub: Option<Subscription>,
    /// Access-chip dropdown (permission modes). Mirrors the model selector pattern.
    pub access_open: bool,
    /// Project-chip dropdown (recent projects + new project submenu).
    pub project_chip_open: bool,
    pub project_chip_menu: Option<Entity<PopupMenu>>,
    pub project_chip_menu_sub: Option<Subscription>,
    /// Composer typeahead completion popover (`/` commands, `@` skills/agents).
    /// `None` when no trigger token is active at the caret. A pure render
    /// overlay — it never grabs focus, so the `InputState` keeps focus and the
    /// query filters live on every keystroke.
    pub completion: Option<CompletionState>,
    /// Searchable, newest-first snapshot of the active thread's user turns.
    pub turn_navigator: Option<Entity<TurnNavigator>>,
    pub turn_navigator_sub: Option<Subscription>,
    pub turn_navigator_previous_focus: Option<FocusHandle>,
    /// Follow-ups submitted while a turn is running. Steer items are injected
    /// into the running turn at the next safe join point; queue items flush as
    /// the next user turn at `TurnFinished`.
    pub queued_follow_ups: std::collections::VecDeque<QueuedFollowUp>,
    /// Session-only per-thread queue stash. Switching tasks moves the active
    /// deque here and restores it on return; no database persistence is used.
    pub queued_follow_ups_by_thread: HashMap<String, std::collections::VecDeque<QueuedFollowUp>>,
    /// In-flight queue-row drag (the composer queue's grip handle): the
    /// insertion marker cleared on commit / cancel / thread switch. Mirrors
    /// the sidebar's `drag_row` cue for the same gesture.
    pub queue_drag: Option<QueueRowDrag>,
    /// Tracks which composer placeholder is installed, so render only mutates
    /// the input state on mode transitions.
    pub composer_placeholder_mode: ComposerPlaceholderMode,
    /// Files picked via the `+` menu, not yet sent. Cleared on submit.
    pub pending_attachments: Vec<PendingAttachment>,
    /// Opt-in browser tool suites activated via the `+` menu. Unlike file
    /// attachments these persist across submits (they track session-level tool
    /// activation); removing a chip deactivates the suite.
    pub active_browser_suites: Vec<manox_agent::engine::BrowserSuite>,
    /// True while a native directory picker is open from the "Choose project" row.
    /// Guards against the user submitting a message before the picker resolves
    /// (which would make `set_project` a silent no-op once `messages` is non-empty).
    pub project_picker_pending: bool,
    /// Parent directory selected for "Create blank project"; waiting for name input.
    pub blank_project_parent: Option<PathBuf>,
    /// Input state for the blank project folder name overlay.
    pub blank_project_name_input: Option<Entity<InputState>>,
    pub thread_sub: Option<Subscription>,
    /// Observes the foreground leaf store itself (beyond `thread_sub`'s
    /// events): a projection-only frame (mid-session `SetModel` →
    /// `Projections` delta) writes the chip fields and notifies the LEAF, but
    /// the entry event's re-render can land in an earlier tick — without this
    /// observe the workspace never repaints and the chip stays stale (the
    /// #765 "picks a model, nothing happens" repro: the journal had both
    /// changes, the render never showed them).
    pub store_observe: Option<Subscription>,
    /// A successor session the foreground must switch to at the next render
    /// (a bind's identity hand-off arrived while this workspace held the
    /// predecessor). Taken once, so the switch cannot re-trigger.
    pub pending_successor: Option<String>,
    /// A `ForkSession` round trip is outstanding. The fork control is a button
    /// on every forkable reply, and the verdict takes a round trip, so without
    /// this a double click mints two children for one intent. Cleared on both
    /// verdicts so a failed fork stays retryable.
    pub fork_in_flight: bool,
    pub input_sub: Option<Subscription>,
    /// Height-invalidation subscription: any `ConversationState` mutation may
    /// change a row's height (including off-screen rows whose height is cached
    /// in the list sum tree). Remeasure all rows on every conversation notify —
    /// the same cure a window resize applies — so a stale cached height can
    /// never survive to paint an overlapping row.
    pub conversation_sub: Option<Subscription>,
    /// Scroll/virtualization state for the message column, held natively by
    /// `gpui::ListState`. `ListAlignment::Bottom` gives chat-log semantics:
    /// short histories sit at the bottom, long ones scroll. `FollowMode::Tail`
    /// pins to the live end on each layout while following, disengages on an
    /// upward user scroll, and re-arms when a scroll lands back at the bottom.
    /// `MSG_LIST_OVERDRAW` rows below the viewport are pre-measured; a width
    /// change invalidates every cached height, and visible rows re-measure
    /// every frame (so a height change without an explicit signal self-
    /// corrects). Count changes are reconciled via `splice`, in-place
    /// mutations via `remeasure_items`, both driven by `ApplyOutcome`. Only
    /// the visible items render.
    pub list_state: ListState,
    /// Exact width of the list child from the previous prepaint. Official GPUI
    /// at the pinned revision does not invalidate off-screen row heights when
    /// this changes, so the application explicitly remeasures the cache.
    pub message_list_width: crate::views::MessageListWidthInvalidator,
    /// Cached `items().len()`; the event handler reconciles the list count via
    /// `splice` whenever the conversation grows or shrinks.
    pub list_count: usize,
    /// Whether the goal status popover is open (toggled by the `◎ /goal active`
    /// chip or the bare `/goal` command).
    pub goal_popover_open: bool,
    /// Generation counter for the goal elapsed-time ticker. Incremented when a
    /// goal is cleared or the active thread changes so the prior ticker
    /// self-terminates instead of notifying a stale chip. Mirrors
    /// `settings_transition_gen`.
    pub goal_ticker_gen: u64,
    /// True while the active thread has a turn in flight, so the Thinking
    /// status row's "for Xs" counter ticks every second. Set on `TurnStarted`,
    /// cleared on a terminal `Stop`/`Error`. The ticker task polls this and
    /// self-terminates when it goes false.
    pub turn_active: bool,
    /// Generation counter for the thinking elapsed-time ticker. Incremented
    /// on every `TurnStarted` and on thread switch so a prior ticker
    /// self-terminates instead of driving a stale container.
    pub thinking_ticker_gen: u64,
    /// Right-hand context rail. Owns the cockpit state (run phase, the model's
    /// plan snapshot, per-cell counter animation state) that used to live
    /// directly on `Workspace`, plus strong handles to the active thread and conversation
    /// it renders against. Writes flow through `self.context_rail.update`.
    pub context_rail: Entity<crate::views::context_rail::ContextRail>,
}
