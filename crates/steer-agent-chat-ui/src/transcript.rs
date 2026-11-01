//! The single conversation projection.
//!
//! `transcript ≡ project(&ChatState, &Overlays)` — the live stream and the
//! rebuild are two trigger timings of ONE pure function, not two fold paths.
//! The AHP chat state is the only content authority: every row the message
//! list renders is keyed by a stable protocol identity (the part id the manox
//! projection mints, `p-{journal entry id}`, is byte-stable across replay),
//! so re-projecting a grown state diffs by key, updates rows in place, and
//! never reorders or duplicates anything.
//!
//! Client-local state (what the wire cannot express) rides in
//! [`Overlays`]: optimistic send bubbles awaiting turn adoption, workspace
//! info notices, host refusal notices, and per-turn wall-clock anchors. The
//! function is pure over (state, overlays): same inputs, same rows.

use std::collections::HashMap;
use std::time::Instant;

use ahp_types::state::{
    ActiveTurn, ChatInputQuestion, ChatState, ReasoningResponsePart, ResponsePart, ToolCallState,
    ToolInput, ToolResultContent, ToolResultTextContent, Turn, TurnState, UsageInfo,
};
use manox_agent::thread::ToolCallStatus;

use crate::conversation::ActivityEntry;
use crate::conversation::{ConvItem, ThinkingContainer, ToolCallItem, UserTurnMeta};
use crate::model_tag::ModelTag;

/// Stable identity of one projected row. The protocol half is derived from
/// the ids the state already carries (`user:{turn}`, `md:{part}`, …); the
/// `opt:`/`ui:` halves are client-local. A row's key is fixed for its
/// lifetime — the list reconciles by key, so an append inside the fold
/// updates the row that is already on screen instead of minting a twin.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemKey {
    /// A turn's user row (`user:{turn id}`).
    User(String),
    /// A client-local optimistic send bubble (`opt:{n}`), adopted by the
    /// turn that lands with its text.
    Optimistic(usize),
    /// One activity segment (`seg:{first entry id}`) — a maximal run of
    /// consecutive reasoning/tool-call parts.
    Segment(String),
    /// A settled turn's whole-turn process head (`tp:{turn id}`).
    TurnProcess(String),
    /// An assistant markdown bubble (`md:{part id}`).
    Answer(String),
    /// A top-level tool card (`tc:{tool call id}`) — the ask-shaped call, a
    /// Preview call, or a promoted pending-confirmation copy.
    ToolCard(String),
    /// An input-request part (`ir:{request id}`) — the clarify card.
    Ask(String),
    /// A system-notification part (`sys:{turn}:{index}`), a host refusal
    /// (`rej:{server seq}`), or a workspace info notice (`ui:{n}`).
    Notice(String),
    /// A turn error part (`err:{turn}:{index}`).
    Error(String),
}

impl std::fmt::Display for ItemKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User(id) => write!(f, "user:{id}"),
            Self::Optimistic(n) => write!(f, "opt:{n}"),
            Self::Segment(id) => write!(f, "seg:{id}"),
            Self::TurnProcess(id) => write!(f, "tp:{id}"),
            Self::Answer(id) => write!(f, "md:{id}"),
            Self::ToolCard(id) => write!(f, "tc:{id}"),
            Self::Ask(id) => write!(f, "ir:{id}"),
            Self::Notice(id) => write!(f, "notice:{id}"),
            Self::Error(id) => write!(f, "err:{id}"),
        }
    }
}

/// The entry-level identity of a reasoning part — the key an activity entry
/// is addressed by across re-projections. (The retired event fold minted
/// `reasoning:{uuid}` per contiguous delta run; the part id is the
/// protocol's own round identity and is replay-stable. The `rs:` namespace
/// keeps it disjoint from wire tool-call ids by construction.)
pub fn reasoning_entry_id(part: &str) -> String {
    format!("rs:{part}")
}

/// One projected row: its stable identity plus the exact [`ConvItem`] the
/// list should render for it. `role`/`model`/`model_repeats` are the turn
/// identities a `MessageItem` carries beside its kind.
#[derive(Debug, Clone)]
pub struct Node {
    pub key: ItemKey,
    pub kind: ConvItem,
    /// The model identity this row renders (the turn's own `message.model`,
    /// else the session's current label).
    pub role: String,
    pub model_repeats: bool,
}

/// What the fold knows about one turn, uniform across the archived
/// `turns` page and the in-flight `active_turn`.
pub struct TurnView<'a> {
    pub id: &'a str,
    /// The turn's own RFC 3339 start; owned so the in-flight form (a bare
    /// string, not an Option) and the archived form share one shape.
    pub started_at: Option<String>,
    /// Milliseconds; `None` while the turn is still running.
    pub duration_ms: Option<i64>,
    /// `None` while the turn is in flight; the archived turn's outcome.
    pub state: Option<TurnState>,
    pub message: &'a ahp_types::state::Message,
    pub parts: &'a [ResponsePart],
    pub usage: Option<&'a UsageInfo>,
    pub active: bool,
}

impl<'a> TurnView<'a> {
    pub fn archived(turn: &'a Turn) -> Self {
        Self {
            id: &turn.id,
            started_at: turn.started_at.clone(),
            duration_ms: turn.duration,
            state: Some(turn.state),
            message: &turn.message,
            parts: &turn.response_parts,
            usage: turn.usage.as_ref(),
            active: false,
        }
    }

    pub fn active(active: &'a ActiveTurn) -> Self {
        Self {
            id: &active.id,
            started_at: Some(active.started_at.clone()),
            duration_ms: None,
            state: None,
            message: &active.message,
            parts: &active.response_parts,
            usage: active.usage.as_ref(),
            active: true,
        }
    }

    /// A turn the fold still holds open is mid-flight by definition.
    pub fn terminal(&self) -> bool {
        !self.active
    }
}

/// Client-local state the projection threads through. Owned by the
/// conversation entity; every field is keyed so re-projection is idempotent.
#[derive(Debug, Default)]
pub struct Overlays {
    /// Optimistic send bubbles: submitted locally, rendered immediately,
    /// adopted (dropped in favor of the state row) when the turn carrying
    /// their text lands in the fold. FIFO: the k-th unadopted bubble is
    /// adopted by the k-th matching turn, so identical texts pair in order.
    pub bubbles: Vec<OverlayBubble>,
    /// Host refusal notices, drained from the store's server-seq-keyed
    /// table. Emitted on every pass: the `rej:{seq}` key makes the row
    /// unique, so re-projection keeps it in place instead of forgetting it.
    pub refusals: Vec<RefusalNotice>,
    /// The first instant each turn was observed live — the elapsed anchor
    /// when the turn's `started_at` is missing or unparseable. Purely
    /// presentational (the segment header's clock).
    pub turn_seen: HashMap<String, Instant>,
    /// The next optimistic-bubble / ui-notice serial.
    pub next_serial: usize,
}

/// A locally submitted user turn not yet confirmed by the fold.
#[derive(Debug, Clone)]
pub struct OverlayBubble {
    pub serial: usize,
    pub text: String,
    pub images: Vec<crate::conversation::UserImage>,
    pub meta: UserTurnMeta,
    /// The turn id this client minted for the submit, carried verbatim by
    /// `chat/turnStarted` — the adoption identity. `None` for a steer or an
    /// unsent draft, which no turn can claim by identity.
    pub turn_id: Option<String>,
    /// The client steer id a WINDOW bubble bridges: minted by the removal
    /// echo (which names no turn), then upgraded to the receipt turn's
    /// identity by [`crate::conversation::ConversationState::attach_steer_turn`]
    /// when the partition's turn-start lands. `None` for every ordinary
    /// submit bubble.
    pub steer_id: Option<String>,
    /// How many turn rows the fold held when the bubble was minted
    /// (archived turns plus an active one). The text fallback only pairs
    /// with turns that appeared AFTER this point, so a re-sent "继续" can
    /// never be swallowed by the earlier turn that already carried it.
    pub minted_turns: usize,
}

/// The caller's half of an [`OverlayBubble`]: what the send knows, before the
/// conversation stamps its recipient and mint-time turn count.
#[derive(Debug, Clone)]
pub struct OverlayBubbleSeed {
    pub text: String,
    pub images: Vec<crate::conversation::UserImage>,
    pub meta: UserTurnMeta,
    pub turn_id: Option<String>,
    /// The steer id of a window bubble (see [`OverlayBubble::steer_id`]);
    /// `None` for an ordinary submit.
    pub steer_id: Option<String>,
}

/// A host refusal of one of this client's writes.
#[derive(Debug, Clone)]
pub struct RefusalNotice {
    pub seq: u64,
    pub text: String,
    /// The refused action's turn id, when it targeted one (`chat/turnStarted`)
    /// — the identity whose optimistic bubble the refusal must retire.
    pub turn_id: Option<String>,
}

/// The per-call environment: display facts the fold does not carry.
pub struct ProjectEnv<'a> {
    /// The session's current model label — the fallback identity for turns
    /// whose message names no model.
    pub role: &'a str,
    /// The conversation's recipient (every user bubble's header `to`).
    pub recipient: &'a manox_agent::MessageAuthor,
    /// The host port the rows' interactive controls act through.
    pub host: &'a crate::host::ChatHostHandle,
    /// The thread cwd snapshot, for the tool panels' prompt line.
    pub cwd: Option<&'a str>,
    /// The session whose journal these rows replay — set only for the main
    /// conversation, where a part id `p-{entry}` is a forkable anchor.
    pub fork_source: bool,
    /// The tool call the workspace is promoting to a top-level confirmation
    /// card (the park's decision surface). While the park is open that call
    /// renders as the card INSTEAD of its activity-segment entry, so the
    /// call never appears twice.
    pub promoted_confirmation: Option<&'a str>,
    /// Turn ids whose user row mounts with the entrance rise this pass —
    /// the steers whose consumption receipt the sync drained alongside this
    /// projection. The row is fold-synthesized (the mount gate admits only
    /// `Optimistic` keys otherwise), so the steer's landing ride would be
    /// invisible without it; a turn already mounted ignores the flag.
    pub entering_turns: &'a [String],
    /// Turn ids whose opening row IS a steer — the projection facts behind
    /// the row's `steered` badge. Stamped at projection time (not on the
    /// card's retire), so the badge survives re-attach replays and the
    /// removal-before-partition wire order alike.
    pub steered_turns: &'a std::collections::HashSet<String>,
}

/// The empty steered set, for projections that cannot carry steers (a
/// child chat's own display, synthetic fixtures).
pub fn no_steered_turns() -> &'static std::collections::HashSet<String> {
    static EMPTY: std::sync::OnceLock<std::collections::HashSet<String>> =
        std::sync::OnceLock::new();
    EMPTY.get_or_init(std::collections::HashSet::new)
}

/// Project one chat state (or its absence) plus the overlays into the row
/// list. Pure over content; the only writes are the overlays' own
/// bookkeeping (adoption, seen-refusal marks, first-seen turn anchors).
pub fn project(chat: Option<&ChatState>, overlays: &mut Overlays, env: &ProjectEnv) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut prev_segment_model: Option<ModelTag> = None;
    if let Some(chat) = chat {
        for turn in &chat.turns {
            push_turn(
                &TurnView::archived(turn),
                overlays,
                env,
                &mut nodes,
                &mut prev_segment_model,
            );
        }
        if let Some(active) = &chat.active_turn {
            push_turn(
                &TurnView::active(active),
                overlays,
                env,
                &mut nodes,
                &mut prev_segment_model,
            );
        }
        adopt_bubbles(chat, overlays, &mut nodes);
    }
    push_overlay_rows(overlays, &mut nodes);
    nodes
}

/// Adopt the bubbles a landed turn takes over, stamp their `steered` marker
/// onto the adopting turn rows, and leave the survivors for the tail.
///
/// Identity first: a submit mints its turn id client-side and
/// `chat/turnStarted` carries it verbatim, so the turn that lands IS the
/// bubble — text plays no part (a `/skill` bubble keeps its compact
/// invocation while the journal folds the expanded body; the ids still
/// match).
///
/// Text is the fallback for a turn that did not exist when the bubble was
/// minted, never for an older one: pairing re-sent text with the turn that
/// already carried it swallowed the fresh bubble and stamped `steered` on a
/// turn that never was one. The `minted_turns` guard is the whole of that
/// rule, so an id-bearing bubble keeps the fallback too — the host re-keys a
/// turn when it re-folds it (`t-{entry}` instead of the client's id), and
/// identity alone would then strand the bubble on screen forever with
/// nothing to retire it.
fn adopt_bubbles(chat: &ChatState, overlays: &mut Overlays, nodes: &mut [Node]) {
    if overlays.bubbles.is_empty() {
        return;
    }
    let turns: Vec<TurnView> = chat
        .turns
        .iter()
        .map(TurnView::archived)
        .chain(chat.active_turn.iter().map(TurnView::active))
        .collect();
    let mut paired: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut adopted: Vec<(String, bool)> = Vec::new();
    for (turn_ix, turn) in turns.iter().enumerate() {
        let hit = overlays
            .bubbles
            .iter()
            .enumerate()
            .position(|(ix, bubble)| {
                !paired.contains(&ix) && bubble.turn_id.as_deref().is_some_and(|id| id == turn.id)
            })
            .or_else(|| {
                overlays
                    .bubbles
                    .iter()
                    .enumerate()
                    .position(|(ix, bubble)| {
                        !paired.contains(&ix)
                            && bubble.minted_turns <= turn_ix
                            && bubble.text == turn.message.text
                    })
            });
        if let Some(ix) = hit {
            paired.insert(ix);
            adopted.push((turn.id.to_string(), overlays.bubbles[ix].meta.steered));
        }
    }
    if paired.is_empty() {
        return;
    }
    overlays.bubbles = std::mem::take(&mut overlays.bubbles)
        .into_iter()
        .enumerate()
        .filter(|(ix, _)| !paired.contains(ix))
        .map(|(_, bubble)| bubble)
        .collect();
    for node in nodes.iter_mut() {
        if let ItemKey::User(turn) = &node.key
            && let Some((_, steered)) = adopted.iter().find(|(id, _)| id == turn)
            && let ConvItem::User {
                meta: Some(meta), ..
            } = &mut node.kind
        {
            meta.steered |= *steered;
        }
    }
}

/// Emit the client-local rows (surviving bubbles, ui notices, refusals).
fn push_overlay_rows(overlays: &mut Overlays, nodes: &mut Vec<Node>) {
    for bubble in &overlays.bubbles {
        nodes.push(Node {
            key: ItemKey::Optimistic(bubble.serial),
            kind: ConvItem::User {
                text: bubble.text.clone(),
                images: bubble.images.clone(),
                meta: Some(bubble.meta.clone()),
            },
            role: bubble.meta.model_id.clone(),
            model_repeats: false,
        });
    }
    for refusal in &overlays.refusals {
        nodes.push(Node {
            key: ItemKey::Notice(format!("rej:{}", refusal.seq)),
            kind: ConvItem::Notice(refusal.text.clone()),
            role: String::new(),
            model_repeats: false,
        });
    }
}

/// Project one turn: its user row, then a bounded scan of the response
/// parts — runs of reasoning/tool-call parts fold into one activity segment,
/// markdown parts become answer bubbles, everything else is a card or a
/// notice. The turn's settled process head is spliced above its first
/// segment.
fn push_turn(
    turn: &TurnView,
    overlays: &mut Overlays,
    env: &ProjectEnv,
    nodes: &mut Vec<Node>,
    prev_segment_model: &mut Option<ModelTag>,
) {
    let model_id = turn
        .message
        .model
        .as_ref()
        .map(|m| m.id.clone())
        .unwrap_or_else(|| env.role.to_string());
    let tag = ModelTag::resolve(&model_id);
    let timestamp = epoch_of(&turn.started_at);
    nodes.push(Node {
        key: ItemKey::User(turn.id.to_string()),
        kind: ConvItem::User {
            text: turn.message.text.clone(),
            images: Vec::new(),
            meta: Some(UserTurnMeta {
                timestamp,
                model_id: model_id.clone(),
                access_mode: None,
                // The badge is a projection fact: the fold says which turns
                // opened on a steer row, so every projection of the row —
                // first mount, re-projection, re-attach replay — stamps it
                // identically.
                steered: env.steered_turns.contains(turn.id),
                author: None,
                recipient: Some(env.recipient.clone()),
                peer: false,
            }),
        },
        role: model_id.clone(),
        model_repeats: false,
    });

    // The head splices above the turn's first segment — the index the first
    // segment will occupy if one appears. A segment-free turn earns no head
    // (the old fold minted it on the first segment push, and the answer-only
    // turn keeps that shape).
    let turn_first_segment = nodes.len();
    let mut has_segment = false;
    let usage = turn.usage.map(crate::transcript::usage_of);
    let mut segment_parts: Vec<(String, SegmentEntryNode)> = Vec::new();
    let mut part_index = 0usize;
    for part in turn.parts {
        part_index += 1;
        match part {
            ResponsePart::Reasoning(reasoning) => {
                if reasoning.content.is_empty() {
                    continue;
                }
                segment_parts.push((
                    reasoning_entry_id(&reasoning.id),
                    SegmentEntryNode::Reasoning(reasoning),
                ));
            }
            ResponsePart::ToolCall(call) => {
                let state = &call.tool_call;
                // A state variant this build does not model carries no id;
                // key it by position so two of them cannot collide on `tc:`.
                let call_id = match tool_call_id_of(state) {
                    "" => format!("{}:{}", turn.id, part_index),
                    wire => wire.to_string(),
                };
                let name = tool_name_of(state);
                // A delegation call is the sub-agent card: top-level, never
                // folded into an activity segment.
                if is_agent_task_call(Some(&tool_input_of(state))) {
                    has_segment |= flush_segment(
                        turn,
                        overlays,
                        nodes,
                        &mut segment_parts,
                        &tag,
                        &model_id,
                        prev_segment_model,
                    );
                    let input = tool_input_of(state);
                    let (subagent_type, description) =
                        crate::conversation::agent_task_labels(name, &input);
                    // A continuation names its run in the envelope address
                    // itself; a create only learns it from the ack. The
                    // envelope wins once present — the run id is the
                    // whole-cycle address, so both spellings name the same
                    // child conversation.
                    let addressed = steer_run_address_of(&input).map(str::to_string);
                    nodes.push(Node {
                        key: ItemKey::ToolCard(call_id.clone()),
                        kind: ConvItem::AgentTask(crate::conversation::AgentTaskItem {
                            id: call_id,
                            run_id: matches!(state, ToolCallState::Completed(_))
                                .then(|| {
                                    crate::conversation::envelope_run_id(&tool_output_of(state))
                                })
                                .flatten()
                                .or(addressed.clone()),
                            subagent_type,
                            description,
                            continuation: addressed.is_some(),
                            status: tool_status_of(state),
                            is_error: matches!(state, ToolCallState::Completed(c) if !c.success),
                        }),
                        role: model_id.clone(),
                        model_repeats: false,
                    });
                    continue;
                }
                // The ask-shaped call and the Preview call are user-facing
                // artifacts: they render as top-level cards, never folded
                // into an activity segment.
                if name == manox_agent::tools::ASK_USER_QUESTION
                    || name == crate::preview::TOOL_NAME
                {
                    has_segment |= flush_segment(
                        turn,
                        overlays,
                        nodes,
                        &mut segment_parts,
                        &tag,
                        &model_id,
                        prev_segment_model,
                    );
                    nodes.push(tool_card_node(&call_id, state, env));
                    continue;
                }
                // The parked call the workspace promotes to its decision
                // surface renders as that card INSTEAD of its segment entry —
                // the same shape ask/agent-task take. Emitting both put the
                // call on screen twice, back to back.
                if env.promoted_confirmation == Some(call_id.as_str())
                    && matches!(state, ToolCallState::PendingConfirmation(_))
                {
                    has_segment |= flush_segment(
                        turn,
                        overlays,
                        nodes,
                        &mut segment_parts,
                        &tag,
                        &model_id,
                        prev_segment_model,
                    );
                    nodes.push(tool_card_node(&call_id, state, env));
                    continue;
                }
                segment_parts.push((call_id, SegmentEntryNode::Tool(state)));
            }
            ResponsePart::Markdown(md) => {
                if md.content.is_empty() {
                    continue;
                }
                let after_segment = flush_segment(
                    turn,
                    overlays,
                    nodes,
                    &mut segment_parts,
                    &tag,
                    &model_id,
                    prev_segment_model,
                );
                has_segment |= after_segment;
                let anchor = fork_anchor(&md.id, env);
                let followed_by_tool = turn.parts[part_index..]
                    .iter()
                    .any(|p| matches!(p, ResponsePart::ToolCall(_)));
                nodes.push(Node {
                    key: ItemKey::Answer(md.id.clone()),
                    kind: ConvItem::Assistant {
                        text: md.content.clone(),
                        streaming: turn.active,
                        activity_header: after_segment,
                        token_usage: usage,
                        entry_id: anchor.clone(),
                        fork_unavailable: fork_unavailable(
                            anchor,
                            turn.active,
                            followed_by_tool,
                            env,
                        ),
                    },
                    role: model_id.clone(),
                    model_repeats: false,
                });
            }
            ResponsePart::InputRequest(request) => {
                if flush_segment(
                    turn,
                    overlays,
                    nodes,
                    &mut segment_parts,
                    &tag,
                    &model_id,
                    prev_segment_model,
                ) {
                    has_segment = true;
                }
                nodes.push(Node {
                    key: ItemKey::Ask(request.request.id.clone()),
                    kind: ConvItem::ToolCall(ask_card_item(request)),
                    role: model_id.clone(),
                    model_repeats: false,
                });
            }
            ResponsePart::SystemNotification(note) => {
                if flush_segment(
                    turn,
                    overlays,
                    nodes,
                    &mut segment_parts,
                    &tag,
                    &model_id,
                    prev_segment_model,
                ) {
                    has_segment = true;
                }
                nodes.push(Node {
                    key: ItemKey::Notice(format!("{}:{}", turn.id, part_index)),
                    kind: ConvItem::Notice(note.content.as_text().to_string()),
                    role: String::new(),
                    model_repeats: false,
                });
            }
            ResponsePart::Error(error) => {
                if flush_segment(
                    turn,
                    overlays,
                    nodes,
                    &mut segment_parts,
                    &tag,
                    &model_id,
                    prev_segment_model,
                ) {
                    has_segment = true;
                }
                nodes.push(Node {
                    key: ItemKey::Error(format!("{}:{}", turn.id, part_index)),
                    kind: ConvItem::Error(error.error.message.clone()),
                    role: model_id.clone(),
                    model_repeats: false,
                });
            }
            ResponsePart::ContentRef(_) | ResponsePart::Unknown(_) => {
                // Large content refs and unmodelled parts have no transcript
                // face yet.
            }
        }
    }
    if flush_segment(
        turn,
        overlays,
        nodes,
        &mut segment_parts,
        &tag,
        &model_id,
        prev_segment_model,
    ) {
        has_segment = true;
    }

    // The settled turn earns its whole-turn process head above its first
    // segment — 「已完成，用时 2分5秒」 derived from the state's own duration,
    // so the live path shows it the moment the turn archives (the old event
    // fold minted it from a client clock and lost it whenever the Stop
    // event raced the history reload).
    if has_segment && turn_first_segment < nodes.len() && turn.terminal() {
        let secs = turn.duration_ms.map(|ms| (ms / 1000).max(0));
        nodes.insert(
            turn_first_segment,
            Node {
                key: ItemKey::TurnProcess(turn.id.to_string()),
                kind: ConvItem::TurnProcess {
                    secs: secs.filter(|s| *s > 0),
                    collapsed: false,
                    user_toggled: false,
                },
                role: model_id.clone(),
                model_repeats: false,
            },
        );
    }
}

fn tool_card_node(call_id: &str, state: &ToolCallState, env: &ProjectEnv) -> Node {
    Node {
        key: ItemKey::ToolCard(call_id.to_string()),
        kind: ConvItem::ToolCall(tool_card_item(state, env)),
        role: String::new(),
        model_repeats: false,
    }
}

/// One segment entry mid-projection: the raw part reference, lowered into an
/// activity entry when the segment flushes.
enum SegmentEntryNode<'a> {
    Reasoning(&'a ReasoningResponsePart),
    Tool(&'a ToolCallState),
}

/// Materialize the pending segment (a run of reasoning/tool-call parts) as
/// one `Thinking` node. An empty run is a no-op; the return says whether a
/// node was emitted.
fn flush_segment(
    turn: &TurnView,
    overlays: &mut Overlays,
    nodes: &mut Vec<Node>,
    parts: &mut Vec<(String, SegmentEntryNode)>,
    tag: &ModelTag,
    model_id: &str,
    prev_segment_model: &mut Option<ModelTag>,
) -> bool {
    if parts.is_empty() {
        return false;
    }
    let first_id = parts[0].0.clone();
    let entries: Vec<ActivityEntry> = parts
        .drain(..)
        .map(|(id, entry)| match entry {
            SegmentEntryNode::Reasoning(reasoning) => ActivityEntry::Reasoning {
                id,
                text: reasoning.content.clone(),
                streaming: turn.active,
                collapsed: !turn.active,
                user_toggled: false,
                markdown: None,
            },
            SegmentEntryNode::Tool(state) => {
                ActivityEntry::Tool(tool_entry_item(&id, state, turn.active))
            }
        })
        .collect();
    let any_entry_live = entries.iter().any(|e| match e {
        ActivityEntry::Reasoning { streaming, .. } => *streaming,
        ActivityEntry::Tool(t) => {
            t.streaming
                || matches!(
                    t.status,
                    ToolCallStatus::Running | ToolCallStatus::PendingApproval
                )
        }
    });
    let model_repeats = prev_segment_model.as_ref().is_some_and(|prev| prev == tag);
    *prev_segment_model = Some(tag.clone());
    nodes.push(Node {
        key: ItemKey::Segment(first_id),
        kind: ConvItem::Thinking(ThinkingContainer {
            entries,
            accepting_entries: turn.active,
            streaming: turn.active || any_entry_live,
            collapsed: !turn.active,
            user_toggled: false,
            started_at: turn_instant(turn.id, &turn.started_at, overlays),
            frozen_secs: (!turn.active)
                .then(|| (turn.duration_ms.unwrap_or(0).max(0) as u64) / 1000),
        }),
        role: model_id.to_string(),
        model_repeats,
    });
    true
}

/// The wall-clock anchor a segment header counts from: the turn's own
/// `started_at` when it parses, else the instant this client first saw the
/// turn (seeded once per turn in the overlays).
fn turn_instant(turn_id: &str, started_at: &Option<String>, overlays: &mut Overlays) -> Instant {
    let epoch = epoch_of(started_at);
    if epoch > 0
        && let Some(anchor) = instant_epochs_ago(chrono::Utc::now().timestamp() - epoch)
    {
        return anchor;
    }
    *overlays
        .turn_seen
        .entry(turn_id.to_string())
        .or_insert_with(Instant::now)
}

/// The turn's wall-clock anchor (the Captain strip's clock) — the same rule
/// the segment headers use, read directly by the conversation.
pub fn turn_anchor(turn_id: &str, started_at: &str, overlays: &mut Overlays) -> Instant {
    turn_instant(turn_id, &Some(started_at.to_string()), overlays)
}

/// `Instant` standing `ago_secs` in the past; `None` for a nonsensical gap
/// (a stamp ahead of this clock, or a gap beyond the monotonic clock's
/// representable range).
fn instant_epochs_ago(ago_secs: i64) -> Option<Instant> {
    if ago_secs < 0 {
        return None;
    }
    Instant::now().checked_sub(std::time::Duration::from_secs(ago_secs as u64))
}

/// The AHP turn's RFC 3339 `started_at` as epoch seconds; `None`/unparseable
/// degrades to 0 (the fold carried no time).
fn epoch_of(started_at: &Option<String>) -> i64 {
    started_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp())
        .unwrap_or(0)
}

/// The reply's durable journal entry id — a forkable anchor — derived from
/// the part id (`p-{entry}`) when these rows replay this session's journal.
fn fork_anchor(part_id: &str, env: &ProjectEnv) -> Option<String> {
    if !env.fork_source {
        return None;
    }
    part_id.strip_prefix("p-").map(str::to_string)
}

/// Why the reply cannot be forked: still streaming (`NotLanded`), a mid-turn
/// step whose tool results ride later entries (`MidTurn`), or a row whose id
/// names no entry of this session (`NotReplayed`).
fn fork_unavailable(
    anchor: Option<String>,
    active: bool,
    followed_by_tool: bool,
    env: &ProjectEnv,
) -> Option<crate::views::message::ForkUnavailable> {
    use crate::views::message::ForkUnavailable;
    if active {
        return Some(ForkUnavailable::NotLanded);
    }
    if anchor.is_none() || !env.fork_source {
        return (!env.fork_source).then_some(ForkUnavailable::NotReplayed);
    }
    followed_by_tool.then_some(ForkUnavailable::MidTurn)
}

/// The call id, uniform across every lifecycle state.
pub fn tool_call_id_of(call: &ToolCallState) -> &str {
    match call {
        ToolCallState::Streaming(s) => &s.tool_call_id,
        ToolCallState::PendingConfirmation(s) => &s.tool_call_id,
        ToolCallState::Running(s) => &s.tool_call_id,
        ToolCallState::AuthRequired(s) => &s.tool_call_id,
        ToolCallState::PendingResultConfirmation(s) => &s.tool_call_id,
        ToolCallState::Completed(s) => &s.tool_call_id,
        ToolCallState::Cancelled(s) => &s.tool_call_id,
        ToolCallState::Unknown(_) => "",
    }
}

fn tool_name_of(call: &ToolCallState) -> &str {
    match call {
        ToolCallState::Streaming(s) => &s.tool_name,
        ToolCallState::PendingConfirmation(s) => &s.tool_name,
        ToolCallState::Running(s) => &s.tool_name,
        ToolCallState::AuthRequired(s) => &s.tool_name,
        ToolCallState::PendingResultConfirmation(s) => &s.tool_name,
        ToolCallState::Completed(s) => &s.tool_name,
        ToolCallState::Cancelled(s) => &s.tool_name,
        ToolCallState::Unknown(_) => "",
    }
}

/// The human title: the call's own intention, else its display name.
fn tool_title_of(call: &ToolCallState) -> String {
    let (intention, display_name) = match call {
        ToolCallState::Streaming(s) => (&s.intention, &s.display_name),
        ToolCallState::PendingConfirmation(s) => (&s.intention, &s.display_name),
        ToolCallState::Running(s) => (&s.intention, &s.display_name),
        ToolCallState::AuthRequired(s) => (&s.intention, &s.display_name),
        ToolCallState::PendingResultConfirmation(s) => (&s.intention, &s.display_name),
        ToolCallState::Completed(s) => (&s.intention, &s.display_name),
        ToolCallState::Cancelled(s) => (&s.intention, &s.display_name),
        ToolCallState::Unknown(_) => (&None, &String::new()),
    };
    intention.clone().unwrap_or_else(|| display_name.clone())
}

fn tool_input_of(call: &ToolCallState) -> serde_json::Value {
    let input = match call {
        ToolCallState::Streaming(s) => s.partial_input.as_deref(),
        ToolCallState::PendingConfirmation(s) => tool_input_inline(&s.tool_input),
        ToolCallState::Running(s) => tool_input_inline(&s.tool_input),
        ToolCallState::AuthRequired(s) => tool_input_inline(&s.tool_input),
        ToolCallState::PendingResultConfirmation(s) => tool_input_inline(&s.tool_input),
        ToolCallState::Completed(s) => tool_input_inline(&s.tool_input),
        ToolCallState::Cancelled(s) => tool_input_inline(&s.tool_input),
        ToolCallState::Unknown(_) => None,
    };
    input
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn tool_input_inline(input: &Option<ToolInput>) -> Option<&str> {
    match input {
        Some(ToolInput::Inline(raw)) => Some(raw),
        _ => None,
    }
}

/// The accumulated result text of a settled (or streaming) call — read from
/// the state itself, never from a client-side output buffer.
fn tool_output_of(call: &ToolCallState) -> String {
    let content = match call {
        ToolCallState::Running(r) => r.content.as_deref(),
        ToolCallState::AuthRequired(s) => s.content.as_deref(),
        ToolCallState::PendingResultConfirmation(s) => s.content.as_deref(),
        ToolCallState::Completed(s) => s.content.as_deref(),
        _ => None,
    };
    tool_result_text(content)
}

pub(crate) fn tool_result_text(content: Option<&[ToolResultContent]>) -> String {
    content
        .unwrap_or(&[])
        .iter()
        .filter_map(|c| match c {
            ToolResultContent::Text(ToolResultTextContent { text }) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The wire lifecycle state mapped onto the display status vocabulary.
fn tool_status_of(call: &ToolCallState) -> ToolCallStatus {
    match call {
        ToolCallState::Completed(c) => {
            if c.success {
                ToolCallStatus::Success
            } else {
                ToolCallStatus::Error
            }
        }
        ToolCallState::PendingConfirmation(_) => ToolCallStatus::PendingApproval,
        ToolCallState::Cancelled(_) => ToolCallStatus::Cancelled,
        // Streaming / running / auth-required / pending-result: the call is
        // in flight. A state this client cannot name keeps the row's live
        // face until a known shape settles it.
        _ => ToolCallStatus::Running,
    }
}

/// Lower one tool-call state into an activity-segment entry.
pub fn tool_entry_item(id: &str, call: &ToolCallState, active_turn: bool) -> ToolCallItem {
    let status = tool_status_of(call);
    ToolCallItem {
        id: id.to_string(),
        name: tool_name_of(call).to_string(),
        title: tool_title_of(call),
        status,
        output: tool_output_of(call),
        is_error: matches!(call, ToolCallState::Completed(c) if !c.success),
        input: tool_input_of(call),
        streaming: active_turn && matches!(status, ToolCallStatus::Running),
        // A tool entry folds by default — the title row names the call; a
        // manual expansion is sticky (`user_toggled`) and the reconcile
        // never fights it.
        collapsed: true,
        user_toggled: false,
        panel: None,
        card: None,
        cwd: None,
        // The reconcile carries the materialized entry's snapshot forward;
        // a fresh lowering only seeds the initial empty one.
        stream: Default::default(),
    }
}

/// Lower one tool-call state into a top-level card (ask-shaped call, Preview
/// call, or a promoted pending-confirmation copy). Cards open expanded: a
/// pending ask is the interaction surface, a Preview's body is the artifact.
pub fn tool_card_item(call: &ToolCallState, env: &ProjectEnv) -> ToolCallItem {
    let mut item = tool_entry_item(tool_call_id_of(call), call, false);
    item.collapsed = false;
    item.cwd = env.cwd.map(gpui::SharedString::from);
    item
}

/// Lower an input-request part into the ask card: pending while the
/// response is absent, settled once it lands. The input is the same
/// `AskUserQuestion`-shaped JSON the interactive card parses.
pub fn ask_card_item(request: &ahp_types::state::InputRequestResponsePart) -> ToolCallItem {
    let req = &request.request;
    let answered = request.response.is_some();
    ToolCallItem {
        id: req.id.clone(),
        name: manox_agent::tools::ASK_USER_QUESTION.to_string(),
        title: req.message.clone().unwrap_or_default(),
        status: if answered {
            ToolCallStatus::Success
        } else {
            ToolCallStatus::PendingApproval
        },
        output: if answered {
            "answered".to_string()
        } else {
            String::new()
        },
        is_error: false,
        input: ask_input_json(req),
        streaming: false,
        collapsed: false,
        user_toggled: false,
        panel: None,
        card: None,
        cwd: None,
        stream: Default::default(),
    }
}

/// The `AskUserQuestion` input shape the ask card renders from: one entry
/// per question with its id, header, message, and options.
fn ask_input_json(req: &ahp_types::state::ChatInputRequest) -> serde_json::Value {
    let questions: Vec<serde_json::Value> = req
        .questions
        .iter()
        .flatten()
        .filter_map(|q| match q {
            ChatInputQuestion::SingleSelect(s) => Some(serde_json::json!({
                "id": s.id, "kind": "single-select", "question": s.message,
                "header": s.title.clone().unwrap_or_default(),
                "multiSelect": false,
                "options": s.options.iter().map(|o| serde_json::json!({
                    "id": o.id, "label": o.label,
                    "description": o.description.clone().unwrap_or_default(),
                })).collect::<Vec<_>>(),
            })),
            ChatInputQuestion::MultiSelect(m) => Some(serde_json::json!({
                "id": m.id, "kind": "multi-select", "question": m.message,
                "header": m.title.clone().unwrap_or_default(),
                "multiSelect": true,
                "options": m.options.iter().map(|o| serde_json::json!({
                    "id": o.id, "label": o.label,
                    "description": o.description.clone().unwrap_or_default(),
                })).collect::<Vec<_>>(),
            })),
            ChatInputQuestion::Text(t) => Some(serde_json::json!({
                "id": t.id, "kind": "text", "question": t.message,
                "header": t.title.clone().unwrap_or_default(),
            })),
            ChatInputQuestion::Number(n) => Some(serde_json::json!({
                "id": n.id, "kind": "number", "question": n.message,
                "header": n.title.clone().unwrap_or_default(),
            })),
            ChatInputQuestion::Integer(n) => Some(serde_json::json!({
                "id": n.id, "kind": "number", "question": n.message,
                "header": n.title.clone().unwrap_or_default(),
            })),
            ChatInputQuestion::Boolean(b) => Some(serde_json::json!({
                "id": b.id, "kind": "boolean", "question": b.message,
                "header": b.title.clone().unwrap_or_default(),
            })),
            ChatInputQuestion::Unknown(_) => None,
        })
        .collect();
    serde_json::json!({ "requestId": req.id, "questions": questions })
}

/// A usage report's display mapping (the per-turn footer).
pub fn usage_of(usage: &UsageInfo) -> manox_agent::TokenUsage {
    manox_agent::TokenUsage {
        input_tokens: usage.input_tokens.unwrap_or(0).max(0) as u64,
        output_tokens: usage.output_tokens.unwrap_or(0).max(0) as u64,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: usage.cache_read_tokens.unwrap_or(0).max(0) as u64,
    }
}

/// The `to.spawn` literal that spawns a real manox Thread — the frozen
/// member arm of the Steer tool. Every other spawn value names a subagent
/// definition; the literal lives here because the card vocabulary is what
/// must not claim it.
pub const MEMBER_SPAWN: &str = "TeamMember";

/// Whether a tool call's input is the unified `Steer` Dispatch that creates
/// or continues a subagent run — the shared vocabulary the data plane and
/// the projection both read. The envelope is `reason: "Dispatch"` +
/// `prompt`, addressing a definition spawn (`to.spawn`) or a continued run
/// (`to.agent_address`); the run id is the whole-cycle address, so a
/// continuation names its run before any ack lands. Everything else on the
/// Steer tool is not a run card: the member spawn arm rides a plain tool
/// call, and Inject/Abort ride the `run/guided` note and the activity
/// segment. The retired per-definition delegation shape (`description` +
/// `prompt` with no envelope) is not recognized.
pub fn is_agent_task_call(input: Option<&serde_json::Value>) -> bool {
    let Some(value) = input else {
        return false;
    };
    if value.get("reason").and_then(serde_json::Value::as_str) != Some("Dispatch")
        || value.get("prompt").is_none()
    {
        return false;
    }
    match steer_spawn_of(value) {
        Some(spawn) => spawn != MEMBER_SPAWN,
        None => steer_run_address_of(value).is_some(),
    }
}

/// The definition a Steer envelope's Dispatch spawns (`to.spawn`); `None`
/// when the envelope addresses a run instead.
pub fn steer_spawn_of(input: &serde_json::Value) -> Option<&str> {
    input
        .get("to")?
        .get("spawn")?
        .as_str()
        .filter(|spawn| !spawn.is_empty())
}

/// The run id a Steer envelope addresses (`to.agent_address`) — a
/// continuation Dispatch's rework target, an Inject's guidance address, an
/// Abort's stop address.
pub fn steer_run_address_of(input: &serde_json::Value) -> Option<&str> {
    input
        .get("to")?
        .get("agent_address")?
        .as_str()
        .filter(|addr| !addr.is_empty())
}

/// Whether an AHP message is user-authored (drives turn-boundary checks).
pub fn is_user(message: &ahp_types::state::Message) -> bool {
    message.origin.kind == ahp_types::state::MessageKind::User
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A create dispatch: `reason: "Dispatch"` + `to.spawn` + `prompt` —
    /// the unified Steer envelope the card renders.
    #[test]
    fn a_steer_dispatch_with_spawn_is_an_agent_task() {
        let input = serde_json::json!({
            "to": { "spawn": "Explore" },
            "reason": "Dispatch",
            "prompt": "find every caller",
            "description": "call-site sweep",
        });
        assert!(is_agent_task_call(Some(&input)));
    }

    /// A continuation dispatch: the envelope addresses a settled run instead
    /// of naming a spawn — still a card, the follow-up on that run.
    #[test]
    fn a_steer_dispatch_to_a_run_id_is_an_agent_task() {
        let input = serde_json::json!({
            "to": { "agent_address": "sub-3" },
            "reason": "Dispatch",
            "prompt": "now also check the tests",
        });
        assert!(is_agent_task_call(Some(&input)));
        assert_eq!(steer_run_address_of(&input), Some("sub-3"));
    }

    /// Only Dispatch cards: Inject (the guided note rides the `run/guided`
    /// row, not a card) and Abort (the stop verb) fold into the segment.
    #[test]
    fn steer_reason_variants_are_not_task_cards() {
        let inject = serde_json::json!({
            "to": { "agent_address": "sub-3" },
            "reason": "Inject",
            "prompt": "focus on the projection layer",
        });
        let abort = serde_json::json!({
            "to": { "agent_address": "sub-3" },
            "reason": "Abort",
            "prompt": "",
        });
        let unreasoned = serde_json::json!({
            "to": { "spawn": "Explore" },
            "prompt": "find every caller",
        });
        assert!(!is_agent_task_call(Some(&inject)));
        assert!(!is_agent_task_call(Some(&abort)));
        assert!(!is_agent_task_call(Some(&unreasoned)));
        assert!(!is_agent_task_call(None));
    }

    /// The frozen member arm spawns a real manox Thread — a sidebar row, not
    /// a subagent run, so it never claims the card.
    #[test]
    fn the_member_spawn_arm_is_not_a_task_card() {
        let member = serde_json::json!({
            "to": { "spawn": MEMBER_SPAWN, "agent_address": "refactorer" },
            "reason": "Dispatch",
            "prompt": "start the refactor",
        });
        assert!(!is_agent_task_call(Some(&member)));
    }

    /// The retired per-definition delegation shape (`description` + `prompt`
    /// with no envelope) is not recognized — the aggressive discipline, no
    /// dual detection.
    #[test]
    fn the_retired_delegation_shape_is_not_a_card() {
        let retired = serde_json::json!({
            "description": "sweep",
            "prompt": "find every caller",
        });
        assert!(!is_agent_task_call(Some(&retired)));
    }

    /// The run id a completed card drills into: every Steer Dispatch ack —
    /// the foreground report, the background start receipt, the promotion
    /// receipt — names `run_id`.
    #[test]
    fn envelope_run_id_reads_the_steer_acks() {
        let foreground = serde_json::json!({
            "kind": "foreground",
            "run_id": "sub-3",
            "stop_reason": "completed",
            "output": "done",
        })
        .to_string();
        let background = serde_json::json!({
            "kind": "background",
            "run_id": "sub-4",
            "status": "started",
        })
        .to_string();
        let promoted = serde_json::json!({
            "kind": "foreground",
            "run_id": "sub-5",
            "stop_reason": "promoted",
            "output": "promoted to background after 5000ms",
        })
        .to_string();
        assert_eq!(
            crate::conversation::envelope_run_id(&foreground),
            Some("sub-3".into())
        );
        assert_eq!(
            crate::conversation::envelope_run_id(&background),
            Some("sub-4".into())
        );
        assert_eq!(
            crate::conversation::envelope_run_id(&promoted),
            Some("sub-5".into())
        );
        assert_eq!(crate::conversation::envelope_run_id("not json"), None);
    }
}
