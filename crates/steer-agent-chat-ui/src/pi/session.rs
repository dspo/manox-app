//! One pi session's host-side state: the transcript plus the protocol facts
//! ACP carries outside it (selectors, commands, usage, banner, title) and the
//! server→client requests waiting for the user.
//!
//! The state is gpui-free and folded synchronously: the transport (see
//! [`super::runtime`]) hands updates and requests in, a view reads the state
//! out. Nothing here spawns, awaits, or renders — which is what makes the
//! whole projection testable without a window.

use std::path::PathBuf;

use serde_json::Value;
use steer_acp::{HostRequest, RequestId, SessionInfo};

use super::transcript::{PiModel, Transcript};

/// The session's lifecycle face.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PiSessionState {
    /// The process is spawning / binding.
    #[default]
    Starting,
    Ready,
    Streaming,
    /// A turn (or a restore) is parked on an approval, elicitation or ask.
    WaitingUser,
    Interrupted,
    Dead,
}

impl PiSessionState {
    pub fn is_alive(self) -> bool {
        !matches!(self, Self::Interrupted | Self::Dead)
    }
}

/// pi's live context gauge (`usage_update`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PiUsage {
    pub used: i64,
    pub size: i64,
}

impl PiUsage {
    /// The gauge's fill, 0–100.
    pub fn percent(&self) -> Option<u8> {
        (self.size > 0)
            .then(|| ((self.used.max(0) as f64 / self.size as f64) * 100.0).round() as u8)
    }
}

/// One choice of a config selector (a model, a thinking level).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiConfigChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

/// One `session/configOptions` selector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// The harness's own grouping key (`model`, `thought_level`).
    pub category: Option<String>,
    /// `select` today; kept verbatim for a future boolean option.
    pub kind: String,
    pub current_value: Option<String>,
    pub choices: Vec<PiConfigChoice>,
}

/// One slash command the session can run from a prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiCommand {
    pub name: String,
    pub description: String,
}

/// One approval policy (`modes`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiMode {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

/// One loaded resource in the session banner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiBannerEntry {
    pub name: String,
    pub description: Option<String>,
    /// `bundled` / `user` / `project` — display data, never branched on.
    pub source: Option<String>,
}

/// One load diagnostic in the session banner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiBannerWarning {
    pub severity: String,
    pub source: String,
    pub message: String,
}

/// The `_zu/session_banner` facts: what the transcript's first line shows.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PiBanner {
    pub version: String,
    pub model: Option<PiModel>,
    pub thinking_level: Option<String>,
    pub skills: Vec<PiBannerEntry>,
    pub extensions: Vec<PiBannerEntry>,
    pub warnings: Vec<PiBannerWarning>,
}

/// A server→client request waiting for the user's answer: the JSON-RPC id to
/// answer plus the params to render.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingRequest {
    pub id: RequestId,
    pub params: Value,
}

impl PendingRequest {
    /// The ask's `toolCallId` (`_zu/ask/create`), the card's identity.
    pub fn tool_call_id(&self) -> Option<&str> {
        self.params.get("toolCallId").and_then(Value::as_str)
    }
}

/// One pi session, as the host sees it.
#[derive(Clone, Debug)]
pub struct PiSessionView {
    pub id: String,
    pub cwd: PathBuf,
    pub transcript: Transcript,
    pub state: PiSessionState,
    /// A turn is in flight (the composer's interrupt affordance).
    pub streaming: bool,
    /// The restored session's history is in flight: draw the loading face
    /// rather than an empty conversation.
    pub loading: bool,
    pub model: Option<PiModel>,
    pub thinking_level: Option<String>,
    pub usage: Option<PiUsage>,
    pub banner: Option<PiBanner>,
    pub config_options: Vec<PiConfigOption>,
    pub commands: Vec<PiCommand>,
    pub current_mode: Option<String>,
    pub modes: Vec<PiMode>,
    /// Follow-ups waiting behind the running turn.
    pub queued: usize,
    /// The session's program title (`session_info_update`), when pi set one.
    pub title: Option<String>,
    /// The last error the store recorded (a failed turn, a dead process).
    pub error: Option<String>,
    /// Tool approvals (`session/request_permission`).
    pub pending_permissions: Vec<PendingRequest>,
    /// Structured input forms (`elicitation/create`).
    pub pending_elicitations: Vec<PendingRequest>,
    /// The model's question cards (`_zu/ask/create`).
    pub pending_asks: Vec<PendingRequest>,
    /// Requests outside the three known surfaces; kept so nothing is dropped
    /// silently (the store answers them method-not-found).
    pub pending_other: Vec<PendingRequest>,
}

impl PiSessionView {
    pub fn new(id: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            cwd: cwd.into(),
            transcript: Transcript::new(),
            state: PiSessionState::Starting,
            streaming: false,
            loading: false,
            model: None,
            thinking_level: None,
            usage: None,
            banner: None,
            config_options: Vec::new(),
            commands: Vec::new(),
            current_mode: None,
            modes: Vec::new(),
            queued: 0,
            title: None,
            error: None,
            pending_permissions: Vec::new(),
            pending_elicitations: Vec::new(),
            pending_asks: Vec::new(),
            pending_other: Vec::new(),
        }
    }

    /// Adopts the `session/new|load|resume` answer.
    pub fn adopt_info(&mut self, info: &SessionInfo) {
        self.config_options = parse_config_options(&info.config_options);
        self.model = model_of(&self.config_options);
        self.thinking_level = current_of(&self.config_options, "thought_level");
        if let Some(modes) = info.modes.as_object() {
            self.current_mode = modes
                .get("currentModeId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            self.modes = modes
                .get("availableModes")
                .and_then(Value::as_array)
                .map(|modes| {
                    modes
                        .iter()
                        .filter_map(|mode| {
                            Some(PiMode {
                                id: mode.get("id").and_then(Value::as_str)?.to_owned(),
                                name: mode
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                description: mode
                                    .get("description")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
    }

    /// Folds one `session/update` payload. Returns whether anything a view
    /// renders changed (the pump uses it to skip a pointless repaint).
    pub fn apply_update(&mut self, update: &Value) -> bool {
        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("_zu/session_banner") => {
                self.banner = update
                    .get("banner")
                    .and_then(parse_banner)
                    .or(Some(PiBanner::default()));
                true
            }
            Some("usage_update") => {
                let used = update.get("used").and_then(Value::as_i64);
                let size = update.get("size").and_then(Value::as_i64);
                let usage = match (used, size) {
                    (Some(used), Some(size)) => Some(PiUsage { used, size }),
                    _ => None,
                };
                if self.usage == usage {
                    return false;
                }
                self.usage = usage;
                true
            }
            Some("session_info_update") => {
                let title = update
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|title| !title.trim().is_empty())
                    .map(str::to_owned);
                if self.title == title {
                    return false;
                }
                self.title = title;
                true
            }
            Some("config_option_update") => {
                let options = update
                    .get("configOptions")
                    .map(parse_config_options)
                    .unwrap_or_default();
                self.model = model_of(&options);
                self.thinking_level = current_of(&options, "thought_level");
                self.config_options = options;
                true
            }
            Some("current_mode_update") => {
                let mode = update
                    .get("currentModeId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if self.current_mode == mode {
                    return false;
                }
                self.current_mode = mode;
                true
            }
            Some("available_commands_update") => {
                let commands = update
                    .get("availableCommands")
                    .and_then(Value::as_array)
                    .map(|commands| {
                        commands
                            .iter()
                            .filter_map(|command| {
                                Some(PiCommand {
                                    name: command.get("name").and_then(Value::as_str)?.to_owned(),
                                    description: command
                                        .get("description")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_owned(),
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if self.commands == commands {
                    return false;
                }
                self.commands = commands;
                true
            }
            // Compaction is a protocol event with no v1 surface (the plan's
            // non-goals: the transcript keeps the messages, not the marker).
            Some("compaction_update") => false,
            _ => {
                let settled_before = self.transcript.draft().is_some();
                self.transcript.apply(update);
                // The settle marker closes the reply the stream opened.
                if settled_before && self.transcript.draft().is_none() {
                    self.streaming = false;
                    self.derive_state();
                }
                true
            }
        }
    }

    /// Registers a server→client request. The card is the state; the answer
    /// travels through the store.
    pub fn apply_request(&mut self, request: &HostRequest) -> bool {
        let pending = PendingRequest {
            id: request.id.clone(),
            params: request.params.clone(),
        };
        match request.method.as_str() {
            "session/request_permission" => self.pending_permissions.push(pending),
            "elicitation/create" => self.pending_elicitations.push(pending),
            "_zu/ask/create" => self.pending_asks.push(pending),
            _ => self.pending_other.push(pending),
        }
        self.derive_state();
        true
    }

    /// Removes the request with `id` (the answer was written) and returns it.
    pub fn take_request(&mut self, id: &RequestId) -> Option<PendingRequest> {
        let mut taken = None;
        for queue in [
            &mut self.pending_permissions,
            &mut self.pending_elicitations,
            &mut self.pending_asks,
            &mut self.pending_other,
        ] {
            if let Some(index) = queue.iter().position(|pending| &pending.id == id) {
                taken = Some(queue.remove(index));
                break;
            }
        }
        if taken.is_some() {
            self.derive_state();
        }
        taken
    }

    /// How many requests wait for the user.
    pub fn pending_count(&self) -> usize {
        self.pending_permissions.len()
            + self.pending_elicitations.len()
            + self.pending_asks.len()
            + self.pending_other.len()
    }

    /// A turn starts: the optimistic row is already in the transcript.
    pub fn begin_turn(&mut self) {
        self.streaming = true;
        self.error = None;
        self.derive_state();
    }

    /// A turn settled normally.
    pub fn finish_turn(&mut self) {
        self.streaming = false;
        self.loading = false;
        self.derive_state();
    }

    /// The session is usable after a bind.
    pub fn mark_ready(&mut self) {
        self.loading = false;
        self.derive_state();
    }

    /// The process died: keep the transcript and the reason.
    pub fn mark_dead(&mut self, reason: impl Into<String>) {
        self.streaming = false;
        self.loading = false;
        self.queued = 0;
        self.error = Some(reason.into());
        self.state = PiSessionState::Dead;
    }

    /// A failed call that leaves the session re-connectable.
    pub fn mark_interrupted(&mut self, reason: impl Into<String>) {
        self.streaming = false;
        self.loading = false;
        self.error = Some(reason.into());
        self.state = PiSessionState::Interrupted;
    }

    /// Recomputes the lifecycle face from the facts it is derived from.
    fn derive_state(&mut self) {
        if matches!(
            self.state,
            PiSessionState::Dead | PiSessionState::Interrupted
        ) {
            return;
        }
        self.state = if self.pending_count() > 0 {
            PiSessionState::WaitingUser
        } else if self.streaming {
            PiSessionState::Streaming
        } else if self.loading {
            PiSessionState::Starting
        } else {
            PiSessionState::Ready
        };
    }
}

/// Parses a `session/configOptions` array.
pub fn parse_config_options(value: &Value) -> Vec<PiConfigOption> {
    let Some(options) = value.as_array() else {
        return Vec::new();
    };
    options
        .iter()
        .filter_map(|option| {
            let id = option.get("id").and_then(Value::as_str)?.to_owned();
            Some(PiConfigOption {
                name: option
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_owned(),
                description: option
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                category: option
                    .get("category")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                kind: option
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("select")
                    .to_owned(),
                current_value: config_value(option.get("currentValue")),
                choices: option
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|choices| {
                        choices
                            .iter()
                            .filter_map(|choice| {
                                Some(PiConfigChoice {
                                    value: config_value(choice.get("value"))?,
                                    name: choice
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_owned(),
                                    description: choice
                                        .get("description")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                id,
            })
        })
        .collect()
}

/// A config value collapses to the id string (the harness accepts all three
/// shapes: string, boolean, or an object with `value`).
fn config_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Object(object) => object
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

/// The current value of one selector.
fn current_of(options: &[PiConfigOption], id: &str) -> Option<String> {
    options
        .iter()
        .find(|option| option.id == id)
        .and_then(|option| option.current_value.clone())
}

/// The model identity the `model` selector currently names
/// (`provider/id`).
fn model_of(options: &[PiConfigOption]) -> Option<PiModel> {
    let current = current_of(options, "model")?;
    let (provider, id) = current.split_once('/')?;
    let name = options
        .iter()
        .find(|option| option.id == "model")
        .and_then(|option| {
            option
                .choices
                .iter()
                .find(|choice| choice.value == current)
                .map(|choice| choice.name.clone())
        });
    Some(PiModel {
        provider: provider.to_owned(),
        id: id.to_owned(),
        name,
    })
}

/// Parses the `_zu/session_banner` payload.
pub fn parse_banner(value: &Value) -> Option<PiBanner> {
    let version = value.get("version").and_then(Value::as_str)?.to_owned();
    Some(PiBanner {
        version,
        model: value.get("model").and_then(PiModel::from_json),
        thinking_level: value
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .map(str::to_owned),
        skills: banner_entries(value.get("skills")),
        extensions: banner_entries(value.get("extensions")),
        warnings: value
            .get("warnings")
            .and_then(Value::as_array)
            .map(|warnings| {
                warnings
                    .iter()
                    .filter_map(|warning| {
                        Some(PiBannerWarning {
                            severity: warning.get("severity")?.as_str()?.to_owned(),
                            source: warning.get("source")?.as_str()?.to_owned(),
                            message: warning.get("message")?.as_str()?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn banner_entries(value: Option<&Value>) -> Vec<PiBannerEntry> {
    value
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(PiBannerEntry {
                        name: entry.get("name")?.as_str()?.to_owned(),
                        description: entry
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        source: entry
                            .get("source")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn info() -> SessionInfo {
        SessionInfo {
            session_id: "s1".to_owned(),
            config_options: json!([
                {
                    "id": "model",
                    "name": "Model",
                    "category": "model",
                    "type": "select",
                    "currentValue": "openai/gpt-5",
                    "options": [
                        {"value": "openai/gpt-5", "name": "GPT-5", "description": "openai"},
                        {"value": "anthropic/sonnet", "name": "Sonnet", "description": "anthropic"}
                    ]
                },
                {
                    "id": "thought_level",
                    "name": "Thinking",
                    "category": "thought_level",
                    "type": "select",
                    "currentValue": "medium",
                    "options": [{"value": "medium", "name": "medium"}]
                }
            ]),
            modes: json!({
                "currentModeId": "ask",
                "availableModes": [
                    {"id": "ask", "name": "Ask"},
                    {"id": "auto", "name": "Auto", "description": "no approvals"}
                ]
            }),
            raw: json!({}),
        }
    }

    #[test]
    fn the_bind_answer_become_selectors_and_modes() {
        let mut view = PiSessionView::new("s1", "/tmp");
        view.adopt_info(&info());
        assert_eq!(view.model.as_ref().unwrap().id, "gpt-5");
        assert_eq!(view.model.as_ref().unwrap().provider, "openai");
        assert_eq!(view.model.as_ref().unwrap().name.as_deref(), Some("GPT-5"));
        assert_eq!(view.thinking_level.as_deref(), Some("medium"));
        assert_eq!(view.current_mode.as_deref(), Some("ask"));
        assert_eq!(view.modes.len(), 2);
        assert_eq!(view.config_options[0].choices.len(), 2);
        view.mark_ready();
        assert_eq!(view.state, PiSessionState::Ready);
    }

    #[test]
    fn protocol_updates_reach_their_faces() {
        let mut view = PiSessionView::new("s1", "/tmp");
        assert!(view.apply_update(&json!({
            "sessionUpdate": "_zu/session_banner",
            "banner": {
                "version": "1.5.0",
                "model": {"provider": "p", "id": "m", "name": "M"},
                "thinkingLevel": "high",
                "skills": [{"name": "todos", "source": "bundled"}],
                "extensions": [{"name": "todos", "source": "bundled"}],
                "warnings": [{"severity": "warning", "source": "skill", "message": "bad"}]
            }
        })));
        let banner = view.banner.as_ref().unwrap();
        assert_eq!(banner.version, "1.5.0");
        assert_eq!(banner.skills[0].name, "todos");
        assert_eq!(banner.warnings[0].message, "bad");

        assert!(
            view.apply_update(&json!({"sessionUpdate": "usage_update", "used": 12, "size": 200}))
        );
        assert_eq!(view.usage.unwrap().percent(), Some(6));

        assert!(view.apply_update(
            &json!({"sessionUpdate": "session_info_update", "title": "Fix the bug"})
        ));
        assert_eq!(view.title.as_deref(), Some("Fix the bug"));
        // An unchanged value is not a repaint.
        assert!(!view.apply_update(
            &json!({"sessionUpdate": "session_info_update", "title": "Fix the bug"})
        ));

        assert!(view.apply_update(&json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": "compact", "description": "Compact."}]
        })));
        assert_eq!(view.commands[0].name, "compact");

        assert!(view.apply_update(
            &json!({"sessionUpdate": "current_mode_update", "currentModeId": "auto"})
        ));
        assert_eq!(view.current_mode.as_deref(), Some("auto"));

        assert!(view.apply_update(&json!({
            "sessionUpdate": "config_option_update",
            "configOptions": [{
                "id": "model", "name": "Model", "type": "select",
                "currentValue": "anthropic/sonnet",
                "options": [{"value": "anthropic/sonnet", "name": "Sonnet"}]
            }]
        })));
        assert_eq!(view.model.as_ref().unwrap().id, "sonnet");
        assert_eq!(view.thinking_level, None, "the selector list was replaced");

        // Transcript updates go to the transcript, not the faces.
        assert!(view.apply_update(&json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "hi"},
            "messageId": "m1",
        })));
        assert_eq!(view.transcript.draft().unwrap().text, "hi");
    }

    #[test]
    fn a_settle_closes_the_turn_the_stream_opened() {
        let mut view = PiSessionView::new("s1", "/tmp");
        view.mark_ready();
        view.begin_turn();
        assert_eq!(view.state, PiSessionState::Streaming);
        view.apply_update(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}, "messageId": "m1"}));
        view.apply_update(&json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": ""},
            "messageId": "e1",
            "_meta": {"pi": {"settled": true, "entryId": "e1"}},
        }));
        assert!(!view.streaming);
        assert_eq!(view.state, PiSessionState::Ready);
    }

    #[test]
    fn requests_park_the_session_until_answered() {
        let mut view = PiSessionView::new("s1", "/tmp");
        view.mark_ready();
        view.begin_turn();
        let request = HostRequest {
            id: RequestId::Number(7),
            method: "session/request_permission".to_owned(),
            params: json!({"sessionId": "s1", "toolCall": {"toolCallId": "call-1"}}),
        };
        view.apply_request(&request);
        assert_eq!(view.state, PiSessionState::WaitingUser);
        assert_eq!(view.pending_count(), 1);

        let ask = HostRequest {
            id: RequestId::Number(8),
            method: "_zu/ask/create".to_owned(),
            params: json!({"sessionId": "s1", "toolCallId": "call-2", "questions": []}),
        };
        view.apply_request(&ask);
        assert_eq!(view.pending_asks[0].tool_call_id(), Some("call-2"));

        let taken = view.take_request(&RequestId::Number(7)).unwrap();
        assert_eq!(taken.params["toolCall"]["toolCallId"], "call-1");
        // One ask still waits: the turn stays parked.
        assert_eq!(view.state, PiSessionState::WaitingUser);
        view.take_request(&RequestId::Number(8));
        assert_eq!(view.pending_count(), 0);
        assert_eq!(view.state, PiSessionState::Streaming);
    }

    #[test]
    fn a_dead_process_keeps_the_transcript_and_the_reason() {
        let mut view = PiSessionView::new("s1", "/tmp");
        view.begin_turn();
        view.transcript.push_optimistic_user("hello");
        view.mark_dead("the harness exited (exit 7)");
        assert_eq!(view.state, PiSessionState::Dead);
        assert!(!view.streaming);
        assert_eq!(view.error.as_deref(), Some("the harness exited (exit 7)"));
        assert_eq!(view.transcript.settled().len(), 1);
    }
}
