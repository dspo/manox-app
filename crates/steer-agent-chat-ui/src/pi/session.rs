//! One pi session's host-side state: the transcript plus the protocol facts
//! ACP carries outside it (selectors, commands, usage, banner, title) and the
//! server→client requests waiting for the user.
//!
//! The state is gpui-free and folded synchronously: the transport (see
//! [`super::runtime`]) hands updates and requests in, a view reads the state
//! out. Nothing here spawns, awaits, or renders — which is what makes the
//! whole projection testable without a window.

use std::collections::HashMap;
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

/// One plan item's state (`_zu/todos/*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiTodoStatus {
    Pending,
    InProgress,
    Done,
}

impl PiTodoStatus {
    /// The wire word, as `_zu/todos/write` takes it.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        }
    }
}

/// One plan item. `note` is the agent's working detail; `summary`,
/// `verification` and the commit subject are completion facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiTodo {
    pub id: String,
    pub title: String,
    pub status: PiTodoStatus,
    pub note: Option<String>,
    pub summary: Option<String>,
    pub verification: Option<String>,
}

/// One group of plan items (one user request's outcome).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiTodoGroup {
    pub id: String,
    pub title: String,
    pub todos: Vec<PiTodo>,
}

/// The session plan: the loose lane plus the groups.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PiPlan {
    pub enabled: bool,
    pub todos: Vec<PiTodo>,
    pub groups: Vec<PiTodoGroup>,
}

impl PiPlan {
    /// Nothing to draw (also the shape a session without the surface reads as).
    pub fn is_empty(&self) -> bool {
        self.todos.is_empty() && self.groups.iter().all(|group| group.todos.is_empty())
    }

    /// `(done, total)` across the loose lane and every group.
    pub fn progress(&self) -> (usize, usize) {
        let items = self
            .todos
            .iter()
            .chain(self.groups.iter().flat_map(|group| &group.todos));
        let mut done = 0;
        let mut total = 0;
        for item in items {
            total += 1;
            if item.status == PiTodoStatus::Done {
                done += 1;
            }
        }
        (done, total)
    }
}

/// A subagent run's state (`_zu/subagents/list`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiSubagentStatus {
    Queued,
    Running,
    Settled,
    Failed,
    Aborted,
    Interrupted,
}

impl PiSubagentStatus {
    /// The wire word, rendered verbatim (pi's vocabulary, not chrome).
    pub fn wire(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Settled => "settled",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
            Self::Interrupted => "interrupted",
        }
    }

    /// The run is still going (the panel's live marker).
    pub fn is_live(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

/// One `_zu/subagents/list` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiSubagentRun {
    pub run_id: String,
    pub agent: String,
    pub task: String,
    pub status: PiSubagentStatus,
    pub verdict: Option<String>,
    pub output: Option<String>,
}

/// One transcript line of a subagent run (`role` + verbatim text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiSubagentMessage {
    pub role: String,
    pub text: String,
}

/// A question card that outlived its turn (`_zu/ask/pending`). It has no
/// JSON-RPC request id — it is answered by tool call id through
/// `_zu/ask/answer`, which the harness either resolves (a live wait) or
/// injects as a late answer.
#[derive(Clone, Debug, PartialEq)]
pub struct PiRestoredAsk {
    pub tool_call_id: String,
    /// The ask tool's arguments (`{questions: [...]}`).
    pub params: Value,
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
    /// The session plan (`_zu/todos/list`), `None` until the first read.
    pub plan: Option<PiPlan>,
    /// The captain's subagent runs (`_zu/subagents/list`).
    pub subagents: Vec<PiSubagentRun>,
    /// The run whose transcript is expanded in the panel.
    pub expanded_subagent: Option<String>,
    /// Fetched subagent transcripts, keyed by run id.
    pub subagent_transcripts: HashMap<String, Vec<PiSubagentMessage>>,
    /// Question cards that survived a restart or a turn end (pi's ask
    /// mailbox).
    pub restored_asks: Vec<PiRestoredAsk>,
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
            plan: None,
            subagents: Vec::new(),
            expanded_subagent: None,
            subagent_transcripts: HashMap::new(),
            restored_asks: Vec::new(),
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
            // Compaction has no surface: the transcript keeps the messages,
            // not the marker.
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
    /// travels through the store. A request already waiting is ignored — a
    /// repeated frame must not double the card.
    pub fn apply_request(&mut self, request: &HostRequest) -> bool {
        if self.holds_request(&request.id) {
            return false;
        }
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

    /// Whether a request with `id` still waits in one of the pending queues.
    fn holds_request(&self, id: &RequestId) -> bool {
        self.pending_permissions
            .iter()
            .chain(&self.pending_elicitations)
            .chain(&self.pending_asks)
            .chain(&self.pending_other)
            .any(|pending| &pending.id == id)
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

    /// Folds one `_zu/todos/list` answer. Returns whether the panel changed.
    pub fn set_plan(&mut self, plan: PiPlan) -> bool {
        if self.plan.as_ref() == Some(&plan) {
            return false;
        }
        self.plan = Some(plan);
        true
    }

    /// Folds one `_zu/subagents/list` answer; a run that disappeared collapses
    /// its transcript panel.
    pub fn set_subagents(&mut self, runs: Vec<PiSubagentRun>) -> bool {
        let mut changed = false;
        if let Some(expanded) = &self.expanded_subagent
            && !runs.iter().any(|run| &run.run_id == expanded)
        {
            self.expanded_subagent = None;
            changed = true;
        }
        if self.subagents != runs {
            self.subagents = runs;
            changed = true;
        }
        changed
    }

    /// Expands the run's transcript panel (returning the run id the store must
    /// fetch) or collapses it when it was already open.
    pub fn toggle_subagent(&mut self, run_id: &str) -> Option<String> {
        if self.expanded_subagent.as_deref() == Some(run_id) {
            self.expanded_subagent = None;
            return None;
        }
        self.expanded_subagent = Some(run_id.to_owned());
        Some(run_id.to_owned())
    }

    /// Folds one `_zu/subagents/transcript` answer.
    pub fn set_subagent_transcript(
        &mut self,
        run_id: &str,
        messages: Vec<PiSubagentMessage>,
    ) -> bool {
        self.subagent_transcripts
            .insert(run_id.to_owned(), messages);
        true
    }

    /// Folds a `_zu/ask/pending` answer: the cards a restart kept alive.
    pub fn set_restored_asks(&mut self, asks: Vec<PiRestoredAsk>) -> bool {
        if self.restored_asks == asks {
            return false;
        }
        self.restored_asks = asks;
        true
    }

    /// How many requests wait for the user.
    pub fn pending_count(&self) -> usize {
        self.pending_permissions.len()
            + self.pending_elicitations.len()
            + self.pending_asks.len()
            + self.pending_other.len()
    }

    /// A turn starts: the optimistic row is already in the transcript. One
    /// policy for the sticky faces: a new turn revives an interrupted session
    /// (its failure was a settled turn, the connection is still alive), while
    /// a dead one refuses — its reconnect flow owns the restart, and
    /// `derive_state` is sticky, so a streaming flag there would never show.
    pub fn begin_turn(&mut self) {
        if self.state == PiSessionState::Dead {
            return;
        }
        self.state = PiSessionState::Ready;
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

/// Parses a `_zu/todos/list` answer (`{enabled, plan}`).
pub fn parse_plan(value: &Value) -> PiPlan {
    let plan = value.get("plan").unwrap_or(&Value::Null);
    PiPlan {
        enabled: value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        todos: plan
            .get("todos")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(parse_todo).collect())
            .unwrap_or_default(),
        groups: plan
            .get("groups")
            .and_then(Value::as_array)
            .map(|groups| groups.iter().filter_map(parse_todo_group).collect())
            .unwrap_or_default(),
    }
}

fn parse_todo(value: &Value) -> Option<PiTodo> {
    Some(PiTodo {
        id: value.get("id").and_then(Value::as_str)?.to_owned(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        status: match value.get("status").and_then(Value::as_str) {
            Some("done") => PiTodoStatus::Done,
            Some("in_progress") => PiTodoStatus::InProgress,
            _ => PiTodoStatus::Pending,
        },
        note: text_field(value, "note"),
        summary: text_field(value, "summary"),
        verification: text_field(value, "verification"),
    })
}

fn parse_todo_group(value: &Value) -> Option<PiTodoGroup> {
    Some(PiTodoGroup {
        id: value.get("id").and_then(Value::as_str)?.to_owned(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        todos: value
            .get("todos")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(parse_todo).collect())
            .unwrap_or_default(),
    })
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Parses a `_zu/subagents/list` answer (`{runs}`).
pub fn parse_subagent_runs(value: &Value) -> Vec<PiSubagentRun> {
    value
        .get("runs")
        .and_then(Value::as_array)
        .map(|runs| runs.iter().filter_map(parse_subagent_run).collect())
        .unwrap_or_default()
}

fn parse_subagent_run(value: &Value) -> Option<PiSubagentRun> {
    Some(PiSubagentRun {
        run_id: value.get("runId").and_then(Value::as_str)?.to_owned(),
        agent: value
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        task: value
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        status: match value.get("status").and_then(Value::as_str) {
            Some("queued") => PiSubagentStatus::Queued,
            Some("running") => PiSubagentStatus::Running,
            Some("settled") => PiSubagentStatus::Settled,
            Some("failed") => PiSubagentStatus::Failed,
            Some("aborted") => PiSubagentStatus::Aborted,
            _ => PiSubagentStatus::Interrupted,
        },
        verdict: text_field(value, "verdict"),
        output: text_field(value, "output"),
    })
}

/// Parses a `_zu/subagents/transcript` answer (`{messages: [{role, text}]}`).
pub fn parse_subagent_transcript(value: &Value) -> Vec<PiSubagentMessage> {
    value
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| {
                    Some(PiSubagentMessage {
                        role: message.get("role").and_then(Value::as_str)?.to_owned(),
                        text: message
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses a `_zu/ask/pending` answer (`{asks: [{toolCallId, args}]}`).
pub fn parse_restored_asks(value: &Value) -> Vec<PiRestoredAsk> {
    value
        .get("asks")
        .and_then(Value::as_array)
        .map(|asks| {
            asks.iter()
                .filter_map(|ask| {
                    Some(PiRestoredAsk {
                        tool_call_id: ask.get("toolCallId").and_then(Value::as_str)?.to_owned(),
                        params: ask.get("args").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
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

    #[test]
    fn a_dead_session_refuses_a_turn_and_an_interrupted_one_revives() {
        let mut view = PiSessionView::new("s1", "/tmp");
        view.mark_ready();
        view.mark_interrupted("rate limited");
        view.begin_turn();
        assert_eq!(view.state, PiSessionState::Streaming);
        assert_eq!(view.error, None, "the new turn clears the failure face");

        let mut view = PiSessionView::new("s1", "/tmp");
        view.mark_ready();
        view.mark_dead("the harness exited");
        view.begin_turn();
        assert_eq!(view.state, PiSessionState::Dead);
        assert!(!view.streaming);
    }

    #[test]
    fn a_repeated_request_frame_does_not_double_the_card() {
        let mut view = PiSessionView::new("s1", "/tmp");
        view.mark_ready();
        let request = HostRequest {
            id: RequestId::Number(9),
            method: "session/request_permission".to_owned(),
            params: json!({"sessionId": "s1"}),
        };
        assert!(view.apply_request(&request));
        assert!(!view.apply_request(&request));
        assert_eq!(view.pending_count(), 1);
    }

    #[test]
    fn a_plan_answer_folds_its_lanes_and_progress() {
        let plan = parse_plan(&json!({
            "enabled": true,
            "plan": {
                "v": 1,
                "todos": [
                    {"id": "t-1", "title": "loose draft", "status": "pending", "origin": "user"}
                ],
                "groups": [{
                    "id": "g-1",
                    "title": "ship it",
                    "todos": [
                        {"id": "t-2", "title": "write", "status": "done", "origin": "agent",
                         "verification": "cargo test"},
                        {"id": "t-3", "title": "review", "status": "in_progress", "origin": "agent",
                         "note": "waiting on CI"}
                    ]
                }]
            }
        }));
        assert!(plan.enabled);
        assert_eq!(plan.todos.len(), 1);
        assert_eq!(plan.groups[0].title, "ship it");
        assert_eq!(
            plan.groups[0].todos[0].verification.as_deref(),
            Some("cargo test")
        );
        assert_eq!(plan.groups[0].todos[1].status, PiTodoStatus::InProgress);
        assert_eq!(plan.progress(), (1, 3));

        // A disabled or empty answer draws nothing.
        assert!(
            !parse_plan(&json!({"enabled": false, "plan": {"todos": [], "groups": []}})).enabled
        );
        assert!(PiPlan::default().is_empty());
    }

    #[test]
    fn subagent_runs_and_transcripts_parse_from_the_wire() {
        let runs = parse_subagent_runs(&json!({
            "runs": [
                {"runId": "r-1", "agent": "Explore", "task": "look around",
                 "status": "running", "startedAt": 1},
                {"runId": "r-2", "agent": "worker", "task": "build",
                 "status": "settled", "verdict": "ok", "output": "done"}
            ]
        }));
        assert_eq!(runs.len(), 2);
        assert!(runs[0].status.is_live());
        assert!(!runs[1].status.is_live());
        assert_eq!(runs[1].verdict.as_deref(), Some("ok"));

        let messages = parse_subagent_transcript(&json!({
            "messages": [
                {"role": "assistant", "text": "first"},
                {"role": "tool", "text": "second"}
            ]
        }));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, "tool");
    }

    #[test]
    fn the_transcript_panel_toggles_and_collapses_with_its_run() {
        let mut view = PiSessionView::new("s1", "/tmp");
        assert_eq!(view.toggle_subagent("r-1").as_deref(), Some("r-1"));
        assert_eq!(view.expanded_subagent.as_deref(), Some("r-1"));
        assert_eq!(view.toggle_subagent("r-1"), None);
        assert_eq!(view.expanded_subagent, None);

        // Re-expanding keeps the cached transcript; a list without the run
        // collapses the panel.
        view.toggle_subagent("r-1");
        assert!(view.set_subagent_transcript(
            "r-1",
            vec![PiSubagentMessage {
                role: "assistant".to_owned(),
                text: "hi".to_owned()
            }]
        ));
        assert!(view.subagent_transcripts.contains_key("r-1"));
        assert!(view.set_subagents(Vec::new()));
        assert_eq!(view.expanded_subagent, None);
    }

    #[test]
    fn the_plan_fold_is_idempotent() {
        let plan = parse_plan(&json!({"enabled": true, "plan": {"todos": [], "groups": []}}));
        let mut view = PiSessionView::new("s1", "/tmp");
        assert!(view.set_plan(plan.clone()));
        assert!(!view.set_plan(plan));
        assert!(view.plan.as_ref().unwrap().enabled);
    }
}
