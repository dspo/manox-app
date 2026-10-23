//! The chat translation layer (v3): AHP `ChatState`/`SessionState` → the
//! display vocabulary this crate renders.
//!
//! Two halves:
//! - [`synth_display`]: the rebuild path. Folds a snapshot's turns into the
//!   `HistoryEntry` sequence `ConversationState::rebuild_from_display`
//!   already consumes, so the persisted-replay and AHP-snapshot rebuilds stay
//!   one code path (L6: the view never parses provider vocabulary; here the
//!   protocol vocabulary is lowered once).
//! - [`ChatEvent`]: the live path. The store's pump derives these from AHP
//!   action envelopes and hands them to `ConversationState::apply`, replacing
//!   the retired `ThreadEvent` input. Variants mirror what the item list can
//!   actually render — protocol actions with no display fall out at the fold
//!   site, not in the view.

use std::collections::HashMap;
use std::sync::Arc;

use ahp_types::state::{
    ChatState, InputRequestResponsePart, Message as AhpMessage, MessageKind, ResponsePart,
    ToolCallState, ToolInput, ToolResultContent, ToolResultTextContent, Turn, UsageInfo,
};
use manox_agent::Message;
use manox_agent::TokenUsage;
use manox_agent::db::{HistoryEntry, UiNoteKind, UiNoteRecord};
use manox_agent::language_model::{
    LanguageModelToolResult, LanguageModelToolUse, MessageContent, Role,
};
use manox_agent::thread::ToolCallStatus;

/// A display-facing delta for the live conversation list.
#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// Assistant markdown streamed (`chat/delta`).
    AgentText(String),
    /// Reasoning streamed (`chat/reasoning`).
    AgentThinking(String),
    /// A tool call's state advanced (`chat/toolCallStart` /
    /// `toolCallReady` / `toolCallConfirmed` / `toolCallComplete` /
    /// `toolCallContentChanged`).
    ToolCall {
        id: String,
        name: String,
        title: String,
        status: ToolCallStatus,
        input: Option<serde_json::Value>,
    },
    /// Streaming tool output (`content` on a running tool call).
    ToolOutput { id: String, chunk: String },
    /// A tool call settled with a result (`chat/toolCallComplete`).
    ToolResult {
        id: String,
        output: String,
        is_error: bool,
    },
    /// The turn started (`chat/turnStarted` echo). `steer_id` is set when
    /// the opening row IS a parked steer the runtime woke for — that
    /// turn start doubles as the steer's consumption receipt, and the
    /// composer card correlating by the id retires. Emitted for EVERY
    /// turn start (ours, another client's, an auto-flushed queue); the
    /// transcript lifecycle must never lose the event to the retire.
    TurnStarted {
        steer_id: Option<String>,
        /// The opening row's text (`None` for an empty one). The wake-retire
        /// FALLBACK only, never a steer marker: every turn start carries its
        /// opening row's text, so a consumer may match it against a parked
        /// card only when `steer_id` is also set (a wake is always stamped)
        /// and the store has ruled out a carried queued submission (see
        /// `AhpStore::route`). Another client's message with identical
        /// wording stays ambiguous — the host threading the pending id
        /// through the woken row is the exit.
        opening_text: Option<String>,
    },
    /// The turn reached a terminal state (`chat/turnComplete` /
    /// `turnCancelled` / `chat/error`). `cancelled` distinguishes a user
    /// interrupt from a normal completion — the follow-up queue settles
    /// differently across the two.
    TurnFinished { cancelled: bool },
    /// A steer pending message was consumed by the runtime:
    /// `chat/pendingMessageRemoved{steering}` (injected into the running
    /// turn) or a `chat/turnStarted` carrying it as `queuedMessageId` (an
    /// idle steer waking its own run). Carries the client-minted id the
    /// composer's steer card correlates by.
    SteerConsumed { message_id: String },
    /// A compaction summary landed (a `Compaction` user row).
    Compaction { summary: String },
    /// A system notification part (`chat/responsePart`).
    Notice { text: String },
    /// A terminal error part.
    Error { text: String },
    /// The turn's usage report (`chat/usage`).
    Usage { usage: TokenUsage },
}

impl ChatEvent {
    /// Derive the display event from one AHP action, against the chat state
    /// the fold maintains. `None` for actions with no display face.
    pub fn from_action(
        action: &ahp_types::actions::StateAction,
        chat: Option<&ChatState>,
    ) -> Option<Self> {
        use ahp_types::actions::StateAction as A;
        let chat = chat?;
        match action {
            A::ChatDelta(delta) => Some(ChatEvent::AgentText(delta.content.clone())),
            A::ChatReasoning(reasoning) => {
                Some(ChatEvent::AgentThinking(reasoning.content.clone()))
            }
            // The three lifecycle starts are SELF-DESCRIBING: the action
            // carries the tool's id, name and intention, so a state miss can
            // only mean the reducer's gate (no active turn, or a turn-id
            // mismatch) declined to materialize the part — dropping the
            // event here would drop the round boundary with it, and every
            // later reasoning delta would append to the round before the
            // tool (the "thinking keeps appending above a stack of tools"
            // shape, with the orphaned results rendering as separate
            // bottom segments). The state lookup enriches; it never gates.
            A::ChatToolCallStart(start) => match find_tool_call(chat, &start.tool_call_id) {
                Some(state) => tool_call_event(&start.tool_call_id, state),
                None => Some(ChatEvent::ToolCall {
                    id: start.tool_call_id.clone(),
                    title: tool_identity(&start.tool_name, &start.intention, &start.display_name).1,
                    name: start.tool_name.clone(),
                    status: ToolCallStatus::Running,
                    input: None,
                }),
            },
            A::ChatToolCallReady(ready) => match find_tool_call(chat, &ready.tool_call_id) {
                Some(state) => tool_call_event(&ready.tool_call_id, state),
                None => Some(ChatEvent::ToolCall {
                    id: ready.tool_call_id.clone(),
                    name: String::new(),
                    title: ready.intention.clone().unwrap_or_default(),
                    status: ToolCallStatus::Running,
                    input: ready.tool_input.as_ref().and_then(|input| match input {
                        ToolInput::Inline(json) => serde_json::from_str(json).ok(),
                        // A content-ref input has no inline JSON to
                        // parse; the row renders input-less until a
                        // state-bearing event enriches it.
                        ToolInput::ContentRef(_) => None,
                    }),
                }),
            },
            // Confirmed carries approval state but no tool identity; with
            // no state part to name it there is nothing safe to say (a
            // nameless advance garbles the row it lands on), and the round
            // boundary already rode the Start event.
            A::ChatToolCallConfirmed(confirmed) => {
                let state = find_tool_call(chat, &confirmed.tool_call_id)?;
                tool_call_event(&confirmed.tool_call_id, state)
            }
            A::ChatToolCallComplete(complete) => Some(ChatEvent::ToolResult {
                id: complete.tool_call_id.clone(),
                output: tool_result_text(complete.result.content.as_deref()),
                is_error: !complete.result.success,
            }),
            A::ChatToolCallContentChanged(changed) => {
                let state = find_tool_call(chat, &changed.tool_call_id)?;
                let output = running_output(state);
                Some(ChatEvent::ToolOutput {
                    id: changed.tool_call_id.clone(),
                    chunk: output,
                })
            }
            A::ChatTurnStarted(start) => Some(ChatEvent::TurnStarted {
                // An idle steer wakes its own run: the projection stamps the
                // opening row with the pending id as `queuedMessageId` (its
                // own mint for rows that never carried one). The retire
                // rides ALONGSIDE the lifecycle event, never instead of it.
                steer_id: start.queued_message_id.clone(),
                // The row's text is the wake-retire FALLBACK for a pending id
                // no card holds: the projection mints `m-{entry}` when the
                // journal row carried no client id. The store clears it for a
                // carried queued submission; consumers gate it on `steer_id`
                // (see the field's doc).
                opening_text: (!start.message.text.is_empty()).then(|| start.message.text.clone()),
            }),
            A::ChatTurnComplete(_) => Some(ChatEvent::TurnFinished { cancelled: false }),
            A::ChatTurnCancelled(_) => Some(ChatEvent::TurnFinished { cancelled: true }),
            A::ChatPendingMessageRemoved(removed) => {
                // A steering removal is CONSUMPTION as far as the cards are
                // concerned: the host publishes it only once the injection
                // is confirmed, and this app's own withdrawals (重新排队 /
                // 编辑 / 删除) echo back through the same action — those
                // retire by card id, find the card already gone, and no-op.
                // A foreign id finds no card either.
                if removed.kind == ahp_types::state::PendingMessageKind::Steering {
                    Some(ChatEvent::SteerConsumed {
                        message_id: removed.id.clone(),
                    })
                } else {
                    None
                }
            }
            A::ChatUsage(usage) => Some(ChatEvent::Usage {
                usage: usage_of(&usage.usage),
            }),
            A::ChatResponsePart(part) => match &part.part {
                ResponsePart::SystemNotification(note) => Some(ChatEvent::Notice {
                    text: note.content.as_text().to_string(),
                }),
                ResponsePart::Error(error) => Some(ChatEvent::Error {
                    text: error.error.message.clone(),
                }),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Locate a tool call by id across the chat's parts.
///
/// The ACTIVE turn is searched first and by itself: a live turn's parts live
/// in `active_turn.response_parts` — `turns` only gains a turn when it is
/// archived, so the previous shape (resolve the active turn's id against
/// `turns`, then fall back to `turns.last()`) could never find a live call
/// and silently dropped every tool event of the running turn (a fresh
/// session's first turn has no `turns` at all). Archived turns are searched
/// newest-first for actions that land after their turn settled.
fn find_tool_call<'a>(chat: &'a ChatState, tool_call_id: &str) -> Option<&'a ToolCallState> {
    fn find_in<'a>(parts: &'a [ResponsePart], tool_call_id: &str) -> Option<&'a ToolCallState> {
        parts.iter().find_map(|part| match part {
            ResponsePart::ToolCall(call) if tool_call_id_of(&call.tool_call) == tool_call_id => {
                Some(&call.tool_call)
            }
            _ => None,
        })
    }
    if let Some(active) = &chat.active_turn
        && let Some(hit) = find_in(&active.response_parts, tool_call_id)
    {
        return Some(hit);
    }
    chat.turns
        .iter()
        .rev()
        .find_map(|turn| find_in(&turn.response_parts, tool_call_id))
}

/// A tool state's display identity: the wire name and the human title
/// (the call's own intention, else its display name).
fn tool_identity(
    tool_name: &str,
    intention: &Option<String>,
    display_name: &str,
) -> (String, String) {
    (
        tool_name.to_string(),
        intention
            .clone()
            .unwrap_or_else(|| display_name.to_string()),
    )
}

/// `None` for a state this client version cannot name (a newer protocol's
/// `Unknown` payload): a nameless advance is worse than no advance — it
/// garbles whatever row its id lands on, while the row keeps its own face
/// until a known shape (the completion) settles it.
fn tool_call_event(id: &str, state: &ToolCallState) -> Option<ChatEvent> {
    // Every state names its tool: an event must never reach the transcript
    // nameless — a nameless `ToolCall` garbles whatever row its id lands
    // on, up to wiping an open interaction card's identity (and with it
    // the card's whole render face).
    let (name, title) = match state {
        ToolCallState::Streaming(s) => tool_identity(&s.tool_name, &s.intention, &s.display_name),
        ToolCallState::PendingConfirmation(s) => {
            tool_identity(&s.tool_name, &s.intention, &s.display_name)
        }
        ToolCallState::Running(s) => tool_identity(&s.tool_name, &s.intention, &s.display_name),
        ToolCallState::AuthRequired(s) => {
            tool_identity(&s.tool_name, &s.intention, &s.display_name)
        }
        ToolCallState::PendingResultConfirmation(s) => {
            tool_identity(&s.tool_name, &s.intention, &s.display_name)
        }
        ToolCallState::Completed(s) => tool_identity(&s.tool_name, &s.intention, &s.display_name),
        ToolCallState::Cancelled(s) => tool_identity(&s.tool_name, &s.intention, &s.display_name),
        ToolCallState::Unknown(_) => return None,
    };
    Some(match state {
        ToolCallState::Completed(c) => ChatEvent::ToolResult {
            id: id.to_string(),
            output: tool_result_text(c.content.as_deref()),
            is_error: !c.success,
        },
        ToolCallState::PendingConfirmation(p) => ChatEvent::ToolCall {
            id: id.to_string(),
            name,
            title,
            status: ToolCallStatus::PendingApproval,
            input: tool_input_json(&p.tool_input),
        },
        ToolCallState::Running(r) => ChatEvent::ToolCall {
            id: id.to_string(),
            name,
            title,
            status: ToolCallStatus::Running,
            input: tool_input_json(&r.tool_input),
        },
        ToolCallState::Cancelled(_) => ChatEvent::ToolCall {
            id: id.to_string(),
            name,
            title,
            status: ToolCallStatus::Cancelled,
            input: None,
        },
        // Streaming / auth-required / pending-result / unknown: an advance
        // that settles nothing.
        _ => ChatEvent::ToolCall {
            id: id.to_string(),
            name,
            title,
            status: ToolCallStatus::Running,
            input: None,
        },
    })
}

/// The call id, uniform across every lifecycle state.
fn tool_call_id_of(call: &ToolCallState) -> &str {
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

fn tool_input_json(input: &Option<ahp_types::state::ToolInput>) -> Option<serde_json::Value> {
    match input {
        Some(ahp_types::state::ToolInput::Inline(raw)) => serde_json::from_str(raw).ok(),
        _ => None,
    }
}

fn tool_result_text(content: Option<&[ToolResultContent]>) -> String {
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

fn running_output(state: &ToolCallState) -> String {
    match state {
        ToolCallState::Running(r) => r
            .content
            .as_deref()
            .map(|content| tool_result_text(Some(content)))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn usage_of(usage: &UsageInfo) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.input_tokens.unwrap_or(0).max(0) as u64,
        output_tokens: usage.output_tokens.unwrap_or(0).max(0) as u64,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: usage.cache_read_tokens.unwrap_or(0).max(0) as u64,
    }
}

// ---------------------------------------------------------------------------
// Rebuild path: a chat snapshot → the HistoryEntry sequence.
// ---------------------------------------------------------------------------

/// The usage table `rebuild_from_display` consumes, keyed by the initiating
/// user message id.
pub type UsageTable = HashMap<String, TokenUsage>;

/// Fold one chat snapshot into the display sequence. `usage` is filled with
/// one `TokenUsage` per turn, keyed by the synthesized user message id.
pub fn synth_display(chat: &ChatState, usage: &mut UsageTable) -> Vec<HistoryEntry> {
    let mut entries = Vec::new();
    for turn in &chat.turns {
        push_turn(turn, &mut entries, usage);
    }
    if let Some(active) = &chat.active_turn {
        let turn = Turn {
            id: active.id.clone(),
            started_at: Some(active.started_at.clone()),
            duration: None,
            message: active.message.clone(),
            response_parts: active.response_parts.clone(),
            usage: active.usage.clone(),
            state: ahp_types::state::TurnState::Complete,
        };
        push_turn(&turn, &mut entries, usage);
    }
    // Steering and queued messages render as pending UI above the composer,
    // not as transcript rows: the workspace reads them from the chat state
    // directly. Nothing to synthesize here.
    entries
}

/// The AHP turn's RFC 3339 `started_at` as the epoch-seconds timestamp the
/// display messages render; `None`/unparseable degrades to epoch 0.
fn epoch_of(started_at: &Option<String>) -> i64 {
    started_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp())
        .unwrap_or(0)
}

fn push_turn(turn: &Turn, entries: &mut Vec<HistoryEntry>, usage: &mut UsageTable) {
    // The turn's wall-clock start is the only time the fold carries; parts
    // have no times of their own, so every message lowered from this turn
    // stamps it (a missing/unparseable `started_at` degrades to epoch 0).
    let timestamp = epoch_of(&turn.started_at);
    let user_id = format!("u-{}", turn.id);
    let mut user_content = Vec::new();
    for attachment in turn.message.attachments.iter().flatten() {
        if let ahp_types::state::MessageAttachment::EmbeddedResource(_) = attachment {
            // Embedded-resource attachments carry their text in the message
            // body already (the host inlines them); no separate block.
        }
    }
    user_content.push(MessageContent::Text(turn.message.text.clone()));
    entries.push(HistoryEntry::Message(Message {
        id: user_id.clone(),
        timestamp,
        parent_id: None,
        provenance: manox_agent::MessageProvenance::User,
        role: Role::User,
        content: user_content,
        ui: None,
    }));
    if let Some(info) = &turn.usage {
        usage.insert(user_id, usage_of(info));
    }

    // Tool calls pair their results back as user-role ToolResult blocks (the
    // Anthropic wire contract `ItemBuilder` consumes), so collect them while
    // walking the response parts in order.
    let mut pending_results: Vec<MessageContent> = Vec::new();
    let mut assistant_content: Vec<MessageContent> = Vec::new();

    for part in &turn.response_parts {
        match part {
            ResponsePart::Markdown(md) => {
                flush_assistant(
                    &mut assistant_content,
                    &mut pending_results,
                    entries,
                    timestamp,
                );
                assistant_content.push(MessageContent::Text(md.content.clone()));
            }
            ResponsePart::Reasoning(reasoning) => {
                flush_assistant(
                    &mut assistant_content,
                    &mut pending_results,
                    entries,
                    timestamp,
                );
                assistant_content.push(MessageContent::Thinking {
                    text: reasoning.content.clone(),
                    signature: None,
                });
            }
            ResponsePart::ToolCall(call) => {
                let (use_block, result_block) = tool_blocks(&call.tool_call);
                assistant_content.push(MessageContent::ToolUse(use_block));
                pending_results.push(MessageContent::ToolResult(result_block));
            }
            ResponsePart::SystemNotification(note) => {
                flush_assistant(
                    &mut assistant_content,
                    &mut pending_results,
                    entries,
                    timestamp,
                );
                entries.push(HistoryEntry::Note(UiNoteRecord {
                    kind: UiNoteKind::Notice,
                    data: serde_json::json!({ "text": note.content.as_text() }),
                }));
            }
            ResponsePart::Error(error) => {
                flush_assistant(
                    &mut assistant_content,
                    &mut pending_results,
                    entries,
                    timestamp,
                );
                entries.push(HistoryEntry::Note(UiNoteRecord {
                    kind: UiNoteKind::Error,
                    data: serde_json::json!({ "text": error.error.message }),
                }));
            }
            ResponsePart::InputRequest(request) => {
                flush_assistant(
                    &mut assistant_content,
                    &mut pending_results,
                    entries,
                    timestamp,
                );
                push_input_request(request, entries, timestamp);
            }
            ResponsePart::ContentRef(_) | ResponsePart::Unknown(_) => {
                // Large content refs and unmodelled parts have no transcript
                // face yet.
            }
        }
    }
    flush_assistant(
        &mut assistant_content,
        &mut pending_results,
        entries,
        timestamp,
    );
}

/// Emit the accumulated assistant message (with its paired tool results as a
/// following user message) before a non-tool part.
fn flush_assistant(
    assistant: &mut Vec<MessageContent>,
    results: &mut Vec<MessageContent>,
    entries: &mut Vec<HistoryEntry>,
    timestamp: i64,
) {
    if assistant.is_empty() {
        results.clear();
        return;
    }
    let content = std::mem::take(assistant);
    entries.push(HistoryEntry::Message(Message {
        id: format!("a-{}", entries.len()),
        timestamp,
        parent_id: None,
        provenance: manox_agent::MessageProvenance::Assistant,
        role: Role::Assistant,
        content,
        ui: None,
    }));
    if !results.is_empty() {
        let results = std::mem::take(results);
        entries.push(HistoryEntry::Message(Message {
            id: format!("r-{}", entries.len()),
            timestamp,
            parent_id: None,
            provenance: manox_agent::MessageProvenance::Tool,
            role: Role::User,
            content: results,
            ui: None,
        }));
    }
}

/// Lower one AHP tool call to the (use, result) pair the builder pairs back.
fn tool_blocks(call: &ToolCallState) -> (LanguageModelToolUse, LanguageModelToolResult) {
    let (name, input_raw, output, is_error): (String, String, String, bool) = match call {
        ToolCallState::Completed(c) => (
            c.tool_name.clone(),
            tool_input_inline(&c.tool_input),
            tool_result_text(c.content.as_deref()),
            !c.success,
        ),
        ToolCallState::Running(r) => (
            r.tool_name.clone(),
            tool_input_inline(&r.tool_input),
            String::new(),
            false,
        ),
        ToolCallState::PendingConfirmation(p) => (
            p.tool_name.clone(),
            tool_input_inline(&p.tool_input),
            String::new(),
            false,
        ),
        _ => (String::new(), String::new(), String::new(), false),
    };
    (
        LanguageModelToolUse {
            id: tool_call_id_of(call).to_string(),
            name: Arc::from(name.as_str()),
            raw_input: input_raw.clone(),
            input: serde_json::from_str(&input_raw).unwrap_or(serde_json::Value::Null),
            is_input_complete: true,
            thought_signature: None,
        },
        LanguageModelToolResult {
            tool_use_id: tool_call_id_of(call).to_string(),
            tool_name: Arc::from(name.as_str()),
            is_error,
            content: output,
        },
    )
}

fn tool_input_inline(input: &Option<ahp_types::state::ToolInput>) -> String {
    match input {
        Some(ahp_types::state::ToolInput::Inline(raw)) => raw.clone(),
        _ => String::new(),
    }
}

/// An elicitation becomes an `AskUserQuestion`-shaped ToolUse/ToolResult pair,
/// the shape the conversation renders as an inline clarify card.
fn push_input_request(
    request: &InputRequestResponsePart,
    entries: &mut Vec<HistoryEntry>,
    timestamp: i64,
) {
    let req = &request.request;
    let mut options: Vec<serde_json::Value> = Vec::new();
    let mut questions: Vec<serde_json::Value> = Vec::new();
    for q in req.questions.iter().flatten() {
        let (id, kind, message, opts): (String, &str, String, Vec<(String, String)>) = match q {
            ahp_types::state::ChatInputQuestion::SingleSelect(s) => (
                s.id.clone(),
                "single-select",
                s.message.clone(),
                s.options
                    .iter()
                    .map(|o| (o.id.clone(), o.label.clone()))
                    .collect(),
            ),
            ahp_types::state::ChatInputQuestion::MultiSelect(m) => (
                m.id.clone(),
                "multi-select",
                m.message.clone(),
                m.options
                    .iter()
                    .map(|o| (o.id.clone(), o.label.clone()))
                    .collect(),
            ),
            ahp_types::state::ChatInputQuestion::Text(t) => {
                (t.id.clone(), "text", t.message.clone(), Vec::new())
            }
            ahp_types::state::ChatInputQuestion::Number(n) => {
                (n.id.clone(), "number", n.message.clone(), Vec::new())
            }
            ahp_types::state::ChatInputQuestion::Integer(n) => {
                (n.id.clone(), "number", n.message.clone(), Vec::new())
            }
            ahp_types::state::ChatInputQuestion::Boolean(b) => {
                (b.id.clone(), "boolean", b.message.clone(), Vec::new())
            }
            ahp_types::state::ChatInputQuestion::Unknown(_) => continue,
        };
        questions.push(serde_json::json!({
            "id": id,
            "kind": kind,
            "message": message,
            "options": opts.iter().map(|(oid, label)| serde_json::json!({
                "id": oid, "label": label,
            })).collect::<Vec<_>>(),
        }));
        for (oid, label) in opts {
            options.push(serde_json::json!({ "id": oid, "label": label }));
        }
    }
    let input = serde_json::json!({
        "requestId": req.id,
        "questions": questions,
    });
    let answered = request
        .response
        .as_ref()
        .map(|_| "answered".to_string())
        .unwrap_or_default();
    entries.push(HistoryEntry::Message(Message {
        id: format!("a-{}", entries.len()),
        timestamp,
        parent_id: None,
        provenance: manox_agent::MessageProvenance::Assistant,
        role: Role::Assistant,
        content: vec![MessageContent::ToolUse(LanguageModelToolUse {
            id: req.id.clone(),
            name: Arc::from(manox_agent::tools::ASK_USER_QUESTION),
            raw_input: input.to_string(),
            input,
            is_input_complete: true,
            thought_signature: None,
        })],
        ui: None,
    }));
    entries.push(HistoryEntry::Message(Message {
        id: format!("r-{}", entries.len()),
        timestamp,
        parent_id: None,
        provenance: manox_agent::MessageProvenance::Tool,
        role: Role::User,
        content: vec![MessageContent::ToolResult(LanguageModelToolResult {
            tool_use_id: req.id.clone(),
            tool_name: Arc::from(manox_agent::tools::ASK_USER_QUESTION),
            is_error: false,
            content: answered,
        })],
        ui: None,
    }));
}

/// Whether a tool call's input has the delegation shape: `description` plus
/// `prompt`, with none of the retired Steer envelope keys. The runtime names
/// its delegation tools after the agent definition, so this is also how a
/// call's own name earns the sub-agent type.
pub fn is_agent_task_call(input: Option<&serde_json::Value>) -> bool {
    input.is_some_and(|value| {
        value.get("description").is_some()
            && value.get("prompt").is_some()
            && value.get("to").is_none()
            && value.get("reason").is_none()
    })
}

/// Lower a display event onto the `ThreadEvent` vocabulary the live
/// conversation applier consumes (the mechanical transcription layer: same
/// shapes the v2 pump emitted, now derived from AHP actions).
pub fn to_thread_event(ev: ChatEvent) -> Option<manox_agent::ThreadEvent> {
    use manox_agent::ThreadEvent as T;
    // Notice renders via `ConversationState::push_notice` (no ThreadEvent
    // shape); Usage feeds the rail's metrics fold, not the transcript.
    let t = match ev {
        ChatEvent::Notice { .. } | ChatEvent::Usage { .. } => return None,
        ChatEvent::AgentText(delta) => T::AgentText(delta),
        ChatEvent::AgentThinking(delta) => T::AgentThinking(delta),
        ChatEvent::ToolCall {
            id,
            name,
            title,
            status,
            input,
        } => T::ToolCall {
            id,
            name,
            title,
            status,
            input,
        },
        ChatEvent::ToolOutput { id, chunk } => T::ToolOutput { id, chunk },
        ChatEvent::ToolResult {
            id,
            output,
            is_error,
        } => T::ToolResult {
            id,
            output,
            is_error,
        },
        ChatEvent::TurnStarted { .. } => T::TurnStarted,
        ChatEvent::TurnFinished { cancelled } => T::TurnFinished {
            cancelled,
            failed: false,
            stranded_steer_ids: Vec::new(),
        },
        // The steer card's retire path lives in the workspace pump; the
        // transcript gets the injected row through its own rebuild/user-row
        // vocabulary, not through this event.
        ChatEvent::SteerConsumed { .. } => return None,
        ChatEvent::Compaction { summary } => T::Compaction {
            summary,
            messages_compacted: 0,
            tokens_before: 0,
            retained_tail: Vec::new(),
        },
        ChatEvent::Error { text } => T::Error(anyhow::anyhow!(text)),
    };
    Some(t)
}

/// Whether an AHP message is user-authored (drives turn-boundary detection).
pub fn is_user(message: &AhpMessage) -> bool {
    message.origin.kind == MessageKind::User
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn chat_with_turn(turn: Turn) -> ChatState {
        serde_json::from_value(serde_json::json!({
            "resource": "ahp-chat:/c-1",
            "title": "t",
            "status": 0,
            "modifiedAt": "2026-01-01T00:00:00Z",
            "turns": [turn],
        }))
        .expect("chat parses")
    }

    #[test]
    fn a_snapshot_turn_synthesizes_user_and_assistant_rows() {
        let turn: Turn = serde_json::from_value(serde_json::json!({
            "id": "t-1",
            "message": { "text": "do a thing", "origin": { "kind": "user" } },
            "responseParts": [
                { "kind": "markdown", "id": "p-1", "content": "doing it" },
            ],
            "state": "complete",
        }))
        .expect("turn parses");
        let chat = chat_with_turn(turn);
        let mut usage = UsageTable::new();
        let entries = synth_display(&chat, &mut usage);
        assert_eq!(entries.len(), 2, "user bubble + assistant reply");
        assert!(matches!(
            &entries[0],
            HistoryEntry::Message(m) if m.role == Role::User
        ));
        assert!(matches!(
            &entries[1],
            HistoryEntry::Message(m) if m.role == Role::Assistant
        ));
    }

    #[test]
    fn the_turns_started_at_stamps_the_synthesized_rows() {
        let turn: Turn = serde_json::from_value(serde_json::json!({
            "id": "t-1",
            "startedAt": "2026-09-29T01:02:03Z",
            "message": { "text": "hello", "origin": { "kind": "user" } },
            "responseParts": [
                { "kind": "markdown", "id": "p-1", "content": "hi" },
            ],
            "state": "complete",
        }))
        .expect("turn parses");
        let chat = chat_with_turn(turn);
        let mut usage = UsageTable::new();
        let entries = synth_display(&chat, &mut usage);
        let expected = chrono::DateTime::parse_from_rfc3339("2026-09-29T01:02:03Z")
            .expect("test instant parses")
            .timestamp();
        assert_eq!(entries.len(), 2, "user bubble + assistant reply");
        for entry in &entries {
            match entry {
                HistoryEntry::Message(m) => assert_eq!(
                    m.timestamp, expected,
                    "rows stamp the turn's start, not epoch 0"
                ),
                other => panic!("unexpected entry {other:?}"),
            }
        }
    }

    #[test]
    fn a_completed_tool_call_pairs_back_its_result() {
        let turn: Turn = serde_json::from_value(serde_json::json!({
            "id": "t-1",
            "message": { "text": "read a file", "origin": { "kind": "user" } },
            "responseParts": [
                { "kind": "toolCall", "toolCall": {
                    "status": "completed",
                    "toolCallId": "tc-1",
                    "toolName": "Read",
                    "displayName": "Read",
                    "invocationMessage": "Reading",
                    "toolInput": "{}",
                    "success": true,
                    "pastTenseMessage": "Read it",
                    "confirmed": "notNeeded",
                }},
            ],
            "state": "complete",
        }))
        .expect("turn parses");
        let chat = chat_with_turn(turn);
        let mut usage = UsageTable::new();
        let entries = synth_display(&chat, &mut usage);
        // user → assistant(ToolUse) → user(ToolResult)
        assert_eq!(entries.len(), 3, "got {entries:?}");
        match &entries[2] {
            HistoryEntry::Message(m) => match &m.content[0] {
                MessageContent::ToolResult(result) => {
                    assert_eq!(result.tool_use_id, "tc-1");
                    assert!(!result.is_error);
                }
                other => panic!("expected a tool result, got {other:?}"),
            },
            other => panic!("expected a message, got {other:?}"),
        }
    }

    #[test]
    fn an_elicitation_becomes_an_ask_card_pair() {
        let turn: Turn = serde_json::from_value(serde_json::json!({
            "id": "t-1",
            "message": { "text": "ask", "origin": { "kind": "user" } },
            "responseParts": [
                { "kind": "inputRequest", "request": {
                    "id": "q-1",
                    "questions": [
                        { "type": "single-select", "id": "q", "message": "pick",
                          "options": [ { "id": "a", "label": "A" } ] }
                    ],
                }},
            ],
            "state": "complete",
        }))
        .expect("turn parses");
        let chat = chat_with_turn(turn);
        let mut usage = UsageTable::new();
        let entries = synth_display(&chat, &mut usage);
        assert!(entries.len() >= 3, "got {entries:?}");
        match &entries[1] {
            HistoryEntry::Message(m) => match &m.content[0] {
                MessageContent::ToolUse(use_block) => {
                    assert_eq!(use_block.name.as_ref(), "AskUserQuestion");
                    assert_eq!(use_block.input["requestId"], "q-1");
                }
                other => panic!("expected a tool use, got {other:?}"),
            },
            other => panic!("expected a message, got {other:?}"),
        }
    }
    /// Every non-terminal tool state names its tool in the derived event:
    /// a nameless `ToolCall` reaching the transcript garbles whatever row
    /// its id lands on — historically it wiped an open ask card's identity
    /// and disarmed its interactive face.
    #[test]
    fn tool_state_events_always_carry_the_tool_name() {
        let streaming: ToolCallState = serde_json::from_value(serde_json::json!({
            "status": "streaming",
            "toolCallId": "call_1",
            "toolName": "AskUserQuestion",
            "displayName": "AskUserQuestion",
            "intention": "Clarify the target file",
            "partialInput": "{\"questions\": [",
        }))
        .expect("streaming state parses");
        match tool_call_event("call_1", &streaming) {
            Some(ChatEvent::ToolCall {
                name,
                title,
                status,
                ..
            }) => {
                assert_eq!(name, "AskUserQuestion");
                assert_eq!(title, "Clarify the target file");
                assert_eq!(status, ToolCallStatus::Running);
            }
            other => panic!("streaming derives a tool-call advance, got {other:?}"),
        }

        let running: ToolCallState = serde_json::from_value(serde_json::json!({
            "status": "running",
            "toolCallId": "call_2",
            "toolName": "Read",
            "displayName": "read",
            "invocationMessage": "Read a.rs",
            "confirmed": "approved",
        }))
        .expect("running state parses");
        match tool_call_event("call_2", &running) {
            Some(ChatEvent::ToolCall { name, title, .. }) => {
                assert_eq!(name, "Read");
                // No intention: the display name is the title fallback.
                assert_eq!(title, "read");
            }
            other => panic!("running derives a tool-call advance, got {other:?}"),
        }

        // A state this client cannot name derives nothing at all: an
        // unnamed advance is worse than none (the row keeps its own face
        // until a known shape settles it).
        let unknown: ToolCallState = serde_json::from_value(serde_json::json!({
            "status": "teleporting",
            "toolCallId": "call_9",
        }))
        .expect("unknown state parses");
        assert!(
            tool_call_event("call_9", &unknown).is_none(),
            "an unnameable state derives no event"
        );

        let completed: ToolCallState = serde_json::from_value(serde_json::json!({
            "status": "completed",
            "toolCallId": "call_3",
            "toolName": "Read",
            "displayName": "read",
            "invocationMessage": "Read a.rs",
            "success": true,
            "pastTenseMessage": "Read a.rs",
            "confirmed": "approved",
            "content": [{ "type": "text", "text": "fn main() {}" }],
        }))
        .expect("completed state parses");
        match tool_call_event("call_3", &completed) {
            Some(ChatEvent::ToolResult { is_error, .. }) => assert!(!is_error),
            other => panic!("completed derives the settle face, got {other:?}"),
        }
    }
    // ── the mid-turn send-mode event vocabulary ─────────────────────────────

    mod send_mode_events {
        use super::chat_with_turn;
        use super::*;
        use ahp_types::actions::{
            ChatPendingMessageRemovedAction, ChatTurnStartedAction, StateAction,
        };
        use ahp_types::state::PendingMessageKind;

        fn minimal_chat() -> ChatState {
            let turn: Turn = serde_json::from_value(serde_json::json!({
                "id": "t-1",
                "message": { "text": "run", "origin": { "kind": "user" } },
                "responseParts": [],
                "state": "complete",
            }))
            .expect("turn parses");
            chat_with_turn(turn)
        }

        #[test]
        fn a_steering_removal_is_the_injection_receipt() {
            let action = StateAction::ChatPendingMessageRemoved(ChatPendingMessageRemovedAction {
                kind: PendingMessageKind::Steering,
                id: "steer-1".to_string(),
            });
            let event = ChatEvent::from_action(&action, Some(&minimal_chat()));
            assert!(matches!(
                event,
                Some(ChatEvent::SteerConsumed { message_id }) if message_id == "steer-1"
            ));
        }

        #[test]
        fn a_queued_removal_derives_nothing() {
            let action = StateAction::ChatPendingMessageRemoved(ChatPendingMessageRemovedAction {
                kind: PendingMessageKind::Queued,
                id: "q-1".to_string(),
            });
            assert!(ChatEvent::from_action(&action, Some(&minimal_chat())).is_none());
        }

        #[test]
        fn a_turn_start_carrying_a_pending_id_is_an_idle_steer_receipt() {
            let message: AhpMessage = serde_json::from_value(serde_json::json!({
                "text": "steered while idle", "origin": { "kind": "user" }
            }))
            .expect("message parses");
            let action = StateAction::ChatTurnStarted(ChatTurnStartedAction {
                turn_id: "t-2".to_string(),
                started_at: "2026-01-01T00:00:00Z".to_string(),
                message,
                queued_message_id: Some("steer-9".to_string()),
                meta: None,
            });
            let event = ChatEvent::from_action(&action, Some(&minimal_chat()));
            assert!(matches!(
                event,
                Some(ChatEvent::TurnStarted {
                    steer_id: Some(message_id),
                    ..
                }) if message_id == "steer-9"
            ));
        }

        /// The wake-retire fallback: the opening row's text rides the event
        /// even when the host threads no pending id (empty → `None`).
        #[test]
        fn a_turn_start_carries_the_opening_text_as_the_wake_retire_fallback() {
            let start = |text: &str| {
                let message: AhpMessage = serde_json::from_value(serde_json::json!({
                    "text": text, "origin": { "kind": "user" }
                }))
                .expect("message parses");
                StateAction::ChatTurnStarted(ChatTurnStartedAction {
                    turn_id: "t-2".to_string(),
                    started_at: "2026-01-01T00:00:00Z".to_string(),
                    message,
                    queued_message_id: None,
                    meta: None,
                })
            };
            assert!(matches!(
                ChatEvent::from_action(&start("引导：先看失败项"), Some(&minimal_chat())),
                Some(ChatEvent::TurnStarted {
                    opening_text: Some(text),
                    ..
                }) if text == "引导：先看失败项"
            ));
            assert!(
                matches!(
                    ChatEvent::from_action(&start(""), Some(&minimal_chat())),
                    Some(ChatEvent::TurnStarted {
                        opening_text: None,
                        ..
                    })
                ),
                "an empty opening row carries no text to match by"
            );
        }

        #[test]
        fn a_plain_turn_start_still_fires_turn_started() {
            let message: AhpMessage = serde_json::from_value(serde_json::json!({
                "text": "hello", "origin": { "kind": "user" }
            }))
            .expect("message parses");
            let action = StateAction::ChatTurnStarted(ChatTurnStartedAction {
                turn_id: "t-2".to_string(),
                started_at: "2026-01-01T00:00:00Z".to_string(),
                message,
                queued_message_id: None,
                meta: None,
            });
            assert!(matches!(
                ChatEvent::from_action(&action, Some(&minimal_chat())),
                Some(ChatEvent::TurnStarted { steer_id: None, .. })
            ));
        }

        #[test]
        fn turn_settles_carry_the_cancelled_bit() {
            let complete: StateAction = serde_json::from_value(serde_json::json!({
                "type": "chat/turnComplete",
                "turnId": "t-1",
                "duration": 5,
            }))
            .expect("complete parses");
            assert!(matches!(
                ChatEvent::from_action(&complete, Some(&minimal_chat())),
                Some(ChatEvent::TurnFinished { cancelled: false })
            ));
            let cancelled: StateAction = serde_json::from_value(serde_json::json!({
                "type": "chat/turnCancelled",
                "turnId": "t-1",
                "duration": 1,
            }))
            .expect("cancelled parses");
            assert!(matches!(
                ChatEvent::from_action(&cancelled, Some(&minimal_chat())),
                Some(ChatEvent::TurnFinished { cancelled: true })
            ));
        }
    }

    #[gpui::test]
    fn rebuild_renders_user_bubbles_from_a_host_fold_chat(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        // The chat state exactly as the host fold leaves it: the same actions
        // the translator emits for a journal, applied through the published
        // reducers.
        use ahp::reducers::apply_action_to_chat;
        let mut chat: ChatState = serde_json::from_value(serde_json::json!({
            "resource": "ahp-chat:/c-1",
            "title": "t",
            "status": 0,
            "modifiedAt": "2026-01-01T00:00:00Z",
            "turns": [],
        }))
        .expect("chat parses");
        let actions: Vec<ahp_types::actions::StateAction> =
            serde_json::from_value(serde_json::json!([
                {"type": "chat/turnStarted", "turnId": "t-1",
                 "startedAt": "2026-01-01T00:00:00Z",
                 "message": {"text": "hello world", "origin": {"kind": "user"}}},
                {"type": "chat/delta", "turnId": "t-1", "partId": "p-1", "content": "hi there"},
                {"type": "chat/turnComplete", "turnId": "t-1", "duration": 5},
            ]))
            .expect("actions parse");
        for action in &actions {
            apply_action_to_chat(&mut chat, action);
        }
        assert_eq!(chat.turns.len(), 1);
        assert_eq!(chat.turns[0].message.text, "hello world");

        let mut usage = UsageTable::new();
        let display = synth_display(&chat, &mut usage);
        let user_row = display.iter().find_map(|e| match e {
            HistoryEntry::Message(m) if m.role == Role::User => Some(m.clone()),
            _ => None,
        });
        assert_eq!(
            user_row.and_then(|m| m.content.first().cloned()),
            Some(MessageContent::Text("hello world".into()))
        );

        cx.update(|cx| {
            let conv = crate::conversation::ConversationState::rebuild_from_display(
                &display,
                &usage,
                "lead",
                manox_agent::MessageAuthor::Lead,
                false,
                crate::conversation::ApplyCtx {
                    host: crate::host::noop_host(),
                    cwd: None,
                    fork_source: None,
                },
                cx,
            );
            let user_bubble = conv.items().iter().any(|item| {
                matches!(
                    item.read(cx).kind(),
                    crate::conversation::ConvItem::User { text, .. } if text == "hello world"
                )
            });
            assert!(user_bubble, "the user bubble must render");
        });
    }
}
