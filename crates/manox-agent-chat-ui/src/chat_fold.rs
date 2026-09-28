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
    ToolCallState, ToolResultContent, ToolResultTextContent, Turn, UsageInfo,
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
    /// The turn started (`chat/turnStarted` echo).
    TurnStarted,
    /// The turn reached a terminal state (`chat/turnComplete` /
    /// `turnCancelled` / `chat/error`).
    TurnFinished,
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
            A::ChatToolCallStart(start) => {
                let state = find_tool_call(chat, &start.tool_call_id)?;
                Some(tool_call_event(&start.tool_call_id, state))
            }
            A::ChatToolCallReady(ready) => {
                let state = find_tool_call(chat, &ready.tool_call_id)?;
                Some(tool_call_event(&ready.tool_call_id, state))
            }
            A::ChatToolCallConfirmed(confirmed) => {
                let state = find_tool_call(chat, &confirmed.tool_call_id)?;
                Some(tool_call_event(&confirmed.tool_call_id, state))
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
            A::ChatTurnStarted(_) => Some(ChatEvent::TurnStarted),
            A::ChatTurnComplete(_) | A::ChatTurnCancelled(_) => Some(ChatEvent::TurnFinished),
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

fn find_tool_call<'a>(chat: &'a ChatState, tool_call_id: &str) -> Option<&'a ToolCallState> {
    let turn = chat
        .active_turn
        .as_ref()
        .map(|t| &t.id)
        .and_then(|id| chat.turns.iter().find(|t| &t.id == id))
        .or_else(|| chat.turns.last())?;
    turn.response_parts.iter().find_map(|part| match part {
        ResponsePart::ToolCall(call) if tool_call_id_of(&call.tool_call) == tool_call_id => {
            Some(&call.tool_call)
        }
        _ => None,
    })
}

fn tool_call_event(id: &str, state: &ToolCallState) -> ChatEvent {
    match state {
        ToolCallState::PendingConfirmation(p) => ChatEvent::ToolCall {
            id: id.to_string(),
            name: p.tool_name.clone(),
            title: p
                .intention
                .clone()
                .unwrap_or_else(|| p.display_name.clone()),
            status: ToolCallStatus::PendingApproval,
            input: tool_input_json(&p.tool_input),
        },
        ToolCallState::Running(r) => ChatEvent::ToolCall {
            id: id.to_string(),
            name: r.tool_name.clone(),
            title: r
                .intention
                .clone()
                .unwrap_or_else(|| r.display_name.clone()),
            status: ToolCallStatus::Running,
            input: tool_input_json(&r.tool_input),
        },
        ToolCallState::Completed(c) => ChatEvent::ToolResult {
            id: id.to_string(),
            output: tool_result_text(c.content.as_deref()),
            is_error: !c.success,
        },
        ToolCallState::Cancelled(_) => ChatEvent::ToolCall {
            id: id.to_string(),
            name: String::new(),
            title: String::new(),
            status: ToolCallStatus::Cancelled,
            input: None,
        },
        _ => ChatEvent::ToolCall {
            id: id.to_string(),
            name: String::new(),
            title: String::new(),
            status: ToolCallStatus::Running,
            input: None,
        },
    }
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

fn push_turn(turn: &Turn, entries: &mut Vec<HistoryEntry>, usage: &mut UsageTable) {
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
        timestamp: 0,
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
                flush_assistant(&mut assistant_content, &mut pending_results, entries);
                assistant_content.push(MessageContent::Text(md.content.clone()));
            }
            ResponsePart::Reasoning(reasoning) => {
                flush_assistant(&mut assistant_content, &mut pending_results, entries);
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
                flush_assistant(&mut assistant_content, &mut pending_results, entries);
                entries.push(HistoryEntry::Note(UiNoteRecord {
                    kind: UiNoteKind::Notice,
                    data: serde_json::json!({ "text": note.content.as_text() }),
                }));
            }
            ResponsePart::Error(error) => {
                flush_assistant(&mut assistant_content, &mut pending_results, entries);
                entries.push(HistoryEntry::Note(UiNoteRecord {
                    kind: UiNoteKind::Error,
                    data: serde_json::json!({ "text": error.error.message }),
                }));
            }
            ResponsePart::InputRequest(request) => {
                flush_assistant(&mut assistant_content, &mut pending_results, entries);
                push_input_request(request, entries);
            }
            ResponsePart::ContentRef(_) | ResponsePart::Unknown(_) => {
                // Large content refs and unmodelled parts have no transcript
                // face yet.
            }
        }
    }
    flush_assistant(&mut assistant_content, &mut pending_results, entries);
}

/// Emit the accumulated assistant message (with its paired tool results as a
/// following user message) before a non-tool part.
fn flush_assistant(
    assistant: &mut Vec<MessageContent>,
    results: &mut Vec<MessageContent>,
    entries: &mut Vec<HistoryEntry>,
) {
    if assistant.is_empty() {
        results.clear();
        return;
    }
    let content = std::mem::take(assistant);
    entries.push(HistoryEntry::Message(Message {
        id: format!("a-{}", entries.len()),
        timestamp: 0,
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
            timestamp: 0,
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
fn push_input_request(request: &InputRequestResponsePart, entries: &mut Vec<HistoryEntry>) {
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
        timestamp: 0,
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
        timestamp: 0,
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
        ChatEvent::TurnStarted => T::TurnStarted,
        ChatEvent::TurnFinished => T::TurnFinished {
            cancelled: false,
            failed: false,
            stranded_steer_ids: Vec::new(),
        },
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

    fn chat_with_turn(turn: Turn) -> ChatState {
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
}
