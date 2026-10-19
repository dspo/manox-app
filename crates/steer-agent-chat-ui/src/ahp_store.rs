//! The AHP data plane (v3): one [`ahp::Client`] over the process-singleton
//! host's in-proc leg, the folded channel state every view reads, and the
//! typed write surface the UI drives.
//!
//! Layout: the protocol bookkeeping lives in [`ChannelBook`], a plain struct
//! with no GPUI in it, so the fold is testable without an entity. [`AhpStore`]
//! wraps a `ChannelBook` in an entity, runs the client pump, and offers the
//! write surface. The client lives on the manox tokio runtime — GPUI threads
//! bridge to it through [`tokio_wait`], and inbound events cross back over a
//! plain async channel to be folded on the store entity.
//!
//! Optimistic writes: AHP has no receipts — a write is a `dispatchAction`
//! notification whose echo carries `origin{clientId, clientSeq}` on acceptance
//! or `rejectionReason` on refusal. The fold never applies rejected actions
//! (the host did not fold them either), so the state self-corrects; the store
//! logs and records rejections for the UI.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use ahp::{Client, ClientConfig};
use ahp_types::actions::StateAction;
use ahp_types::actions::{
    ChatInputCompletedAction, ChatPendingMessageRemovedAction, ChatPendingMessageSetAction,
    ChatToolCallConfirmedAction, ChatTurnCancelledAction, ChatTurnStartedAction,
    SessionActiveClientSetAction, SessionConfigChangedAction, SessionIsArchivedChangedAction,
    SessionTitleChangedAction, SessionWorkingDirectorySetAction,
};
use ahp_types::commands::{
    ChatSource, CreateChatParams, CreateSessionParams, FetchTurnsParams, ForkChatSource,
    InitializeResult, ListSessionsParams, ListSessionsResult, SubscribeResult,
};
use ahp_types::common::ROOT_RESOURCE_URI;
use ahp_types::state::{ChatInputAnswer, ChatInputResponseKind, PendingMessageKind};
use ahp_types::state::{ChatState, RootState, SessionState, SessionSummary, SnapshotState};
use manox_ahp::ext;
use manox_ahp::ext::reducer::{Outcome as ExtOutcome, XManoxState, apply as apply_ext};
// The config-value keys the projection writes onto `session/configChanged`
// (the naming authority lives beside the projector since #879).
use manox_ahp_runtime::ahp::projection::config_keys;
use serde::Serialize;
use serde_json::Value;

/// One running sub-agent, as the host projects it onto the parent chat's
/// background work (AHP `chat/backgroundWorkSet` / `chat/backgroundWorkRemoved`).
/// The host opens an entry on the run's first running tick and removes it when
/// the run settles, so the set IS the live one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentWorkRow {
    /// The run id — the child session id, and the id of the child's own chat
    /// (`ahp-chat:/{id}`), which is where the child's transcript lives.
    pub id: String,
    /// The agent type the host labels the run with (`Explore`, `Sailor`, …).
    pub label: String,
    /// The run's task label (`_meta.latestActivity`), when the host carried
    /// one.
    pub activity: Option<String>,
}

/// How much of one chat's fold a page has already rendered.
///
/// Cheap to read and compare — an id, two counts and the fold's last-entry
/// stamp, never a copy of the content. It moves on every way a transcript
/// grows: a settled turn was appended or the in-flight turn settled, the
/// in-flight turn gained a part or a new id, or the fold's stamp advanced.
///
/// The stamp carries the fine-grained half BY DESIGN: the projection stamps
/// `modifiedAt` with the journal's last entry, so a change that appends no new
/// part (text streaming inside one part, a tool call's state inside an existing
/// part) still moves it. This is a VERSION, not a content hash — a producer
/// that rewrote a settled turn while leaving `modifiedAt` behind would be
/// invisible here, and that is the contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatFoldVersion {
    /// Settled turns.
    turns: usize,
    /// The in-flight turn: `(id, part count)`. An AHP turn's parts grow IN
    /// PLACE, so the turn count alone never moves while a run streams.
    active: Option<(String, usize)>,
    /// The fold's last-entry stamp.
    modified_at: String,
}

/// Read one chat's current fold version.
pub fn chat_fold_version(chat: &ChatState) -> ChatFoldVersion {
    ChatFoldVersion {
        turns: chat.turns.len(),
        active: chat
            .active_turn
            .as_ref()
            .map(|turn| (turn.id.clone(), turn.response_parts.len())),
        modified_at: chat.modified_at.clone(),
    }
}

/// How the client identifies itself to the host (echo correlation, active
/// client claims).
pub const CLIENT_ID: &str = "desktop";

// ---------------------------------------------------------------------------
// ChannelBook: the folded protocol state (no GPUI).
// ---------------------------------------------------------------------------

/// Every folded channel of one client: sessions/chats by bare id, extension
/// channels by full URI.
#[derive(Debug)]
pub struct ChannelBook {
    pub root: RootState,
    /// Session id → summary (the sidebar catalogue, one row per thread).
    pub summaries: BTreeMap<String, SessionSummary>,
    pub sessions: HashMap<String, SessionState>,
    pub chats: HashMap<String, ChatState>,
    /// Extension channel URI → folded state (a baseline replaces, deltas fold).
    pub ext: HashMap<String, XManoxState>,
    /// Metrics channel URI → aggregated Q face.
    pub metrics: HashMap<String, ConversationMetrics>,
    /// Annotations channel state by session id (`ahp-session:/<id>/
    /// annotations`), folded with the SDK reducer.
    pub annotations: std::collections::HashMap<String, ahp_types::state::AnnotationsState>,
    /// Connection-level catalogue channels (`x-manox-workspaces://`,
    /// `x-manox-commands://`): their baseline state is an open payload the
    /// XManoxState fold has no slots for, so it is kept verbatim.
    pub catalogues: HashMap<String, Value>,
    /// Bare chat ids whose `subscribe` snapshot has landed. A first replay
    /// delta can beat the snapshot (the bridge re-emits the journal while the
    /// subscribe answer is in flight), so "the fold holds a chat" does not
    /// imply "the authoritative snapshot arrived" — rebuild consumers use
    /// this to heal a transcript built from the partial pre-snapshot fold.
    chat_snapshots: HashSet<String>,
    /// Highest `serverSeq` seen — the reconnect resume point.
    pub server_seq: u64,
}

/// What one folded action did, so the store can decide whether views need a
/// repaint pass and whether the client's own write came back refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoldEffect {
    /// Nothing visible (unknown action tolerated, no state touched).
    Ignored,
    /// State changed.
    Changed,
    /// The client's own dispatch was rejected; carries the host's reason.
    Rejected(String),
}

impl Default for ChannelBook {
    fn default() -> Self {
        Self {
            root: empty_root(),
            summaries: BTreeMap::new(),
            sessions: HashMap::new(),
            chats: HashMap::new(),
            ext: HashMap::new(),
            metrics: HashMap::new(),
            annotations: HashMap::new(),
            catalogues: HashMap::new(),
            chat_snapshots: HashSet::new(),
            server_seq: 0,
        }
    }
}

impl ChannelBook {
    /// The session id owning a chat id: the session whose catalogue (or
    /// default-chat pointer) carries the chat. A chat id equals its session
    /// id only in the landing form — the two diverge on session continuation
    /// — so protocol writes keyed by a chat id resolve the owner first.
    pub fn session_of_chat(&self, chat_id: &str) -> Option<&str> {
        let uri = chat_uri(chat_id);
        self.sessions.iter().find_map(|(id, state)| {
            let owned = state.default_chat.as_deref() == Some(uri.as_str())
                || state.chats.iter().any(|chat| chat.resource == uri);
            owned.then_some(id.as_str())
        })
    }

    /// Apply one action envelope's action. Typed actions fold through the AHP
    /// reducers; extension actions (and the baseline) fold through the x-manox
    /// reducer, which is unknown-tolerant by design.
    pub fn apply(&mut self, channel: &str, action: &StateAction) -> FoldEffect {
        if let StateAction::Unknown(value) = action {
            return self.apply_unknown(channel, value);
        }
        let Some(parsed) = manox_ahp::channels::parse(channel) else {
            tracing::debug!(channel = %channel, "action for an unparsable channel");
            return FoldEffect::Ignored;
        };
        use ahp::reducers::ReduceOutcome;
        use manox_ahp::channels::Channel;
        let changed = match parsed {
            Channel::Root => matches!(
                ahp::reducers::apply_action_to_root(&mut self.root, action),
                ReduceOutcome::Applied
            ),
            Channel::Session(id) => {
                let state = self
                    .sessions
                    .entry(id)
                    .or_insert_with(|| empty_session(id_of(channel)));
                matches!(
                    ahp::reducers::apply_action_to_session(state, action),
                    ReduceOutcome::Applied
                )
            }
            Channel::Chat(id) => {
                let state = self
                    .chats
                    .entry(id)
                    .or_insert_with(|| empty_chat(id_of(channel)));
                matches!(
                    ahp::reducers::apply_action_to_chat(state, action),
                    ReduceOutcome::Applied
                )
            }
            // Terminal actions have no fold here: the desktop's terminal UI
            // reads the kernel-side PTY registry directly, so an AHP terminal
            // action is tolerated rather than folded. MCP / changeset channels
            // are host-managed state the desktop reads through its own
            // surfaces, not client folds.
            // Annotations fold with the SDK reducer: the channel's state
            // is the durable user notes (durable, cross-client), and this
            // book is what the notes card reads.
            Channel::Annotations(id) => {
                let state = self.annotations.entry(id).or_insert_with(|| {
                    ahp_types::state::AnnotationsState {
                        annotations: Vec::new(),
                    }
                });
                matches!(
                    ahp::reducers::apply_action_to_annotations(state, action),
                    ReduceOutcome::Applied
                )
            }
            Channel::Terminal(_)
            | Channel::Extension(_)
            | Channel::Mcp(_)
            | Channel::Changeset(_) => false,
        };
        if changed {
            FoldEffect::Changed
        } else {
            FoldEffect::Ignored
        }
    }

    fn apply_unknown(&mut self, channel: &str, value: &Value) -> FoldEffect {
        if value.get("type").and_then(Value::as_str) == Some(ext::actions::BASELINE) {
            if ext::channels::ALL
                .iter()
                .filter(|p| p.ends_with("//"))
                .any(|p| channel.starts_with(p))
            {
                // Catalogue channels keep their payload verbatim.
                if let Some(state) = value.get("state") {
                    self.catalogues.insert(channel.to_string(), state.clone());
                    return FoldEffect::Changed;
                }
                return FoldEffect::Ignored;
            }
            return match value.get("state") {
                Some(state) => match serde_json::from_value::<XManoxState>(state.clone()) {
                    Ok(state) => {
                        self.ext.insert(channel.to_string(), state);
                        FoldEffect::Changed
                    }
                    Err(err) => {
                        tracing::warn!(channel = %channel, error = %err, "baseline did not parse");
                        FoldEffect::Ignored
                    }
                },
                None => FoldEffect::Ignored,
            };
        }
        if let Some(reason) = value.get("rejectionReason").and_then(Value::as_str) {
            return FoldEffect::Rejected(reason.to_string());
        }
        if channel.starts_with(ext::channels::METRICS) {
            if value.get("type").and_then(Value::as_str) == Some(ext::actions::METRICS_CHANGED) {
                let kind = value
                    .pointer("/state/kind")
                    .or_else(|| value.get("kind"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let data = value
                    .pointer("/state/data")
                    .or_else(|| value.get("data"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let book = self.metrics.entry(channel.to_string()).or_default();
                if book.apply(kind, &data) {
                    return FoldEffect::Changed;
                }
            }
            return FoldEffect::Ignored;
        }
        let Some(book) = self.ext.get_mut(channel) else {
            // An extension delta for a channel this client never subscribed:
            // tolerated (the host may broadcast catalogue changes).
            return FoldEffect::Ignored;
        };
        match apply_ext(book, value) {
            ExtOutcome::Applied => FoldEffect::Changed,
            ExtOutcome::NoOp | ExtOutcome::Unrecognised => FoldEffect::Ignored,
        }
    }

    /// Adopt a `subscribe`/`reconnect` snapshot wholesale.
    pub fn apply_snapshot(&mut self, uri: &str, state: SnapshotState) -> bool {
        match state {
            SnapshotState::Root(root) => {
                self.root = *root;
                true
            }
            SnapshotState::Session(session) => {
                self.sessions.insert(id_of(uri).to_string(), *session);
                true
            }
            SnapshotState::Chat(chat) => {
                self.chat_snapshots.insert(id_of(uri).to_string());
                self.chats.insert(id_of(uri).to_string(), *chat);
                true
            }
            _ => false,
        }
    }

    /// Seed the catalogue from a `listSessions` page.
    pub fn seed_summaries(&mut self, items: Vec<SessionSummary>) -> bool {
        let mut changed = false;
        for summary in items {
            let id = id_of(&summary.resource).to_string();
            changed |= self
                .summaries
                .insert(id.clone(), summary.clone())
                .is_none_or(|old| old.modified_at != summary.modified_at);
        }
        changed
    }

    /// Drop a catalogue row (the session was disposed).
    pub fn remove_summary(&mut self, session_uri: &str) -> bool {
        self.summaries.remove(id_of(session_uri)).is_some()
    }

    /// The default chat URI of a session, from its folded state.
    pub fn default_chat(&self, session_id: &str) -> Option<String> {
        self.sessions
            .get(session_id)
            .and_then(|s| s.default_chat.clone())
    }

    /// Whether the chat channel's authoritative subscribe snapshot has landed
    /// for `session_id` — checked against both the session id and the default
    /// chat pointer's id, since the host answers whichever URI was subscribed.
    pub fn chat_snapshot_landed(&self, session_id: &str) -> bool {
        if self.chat_snapshots.contains(session_id) {
            return true;
        }
        self.default_chat(session_id)
            .map(|uri| id_of(&uri).to_string())
            .is_some_and(|id| self.chat_snapshots.contains(id.as_str()))
    }
}

/// An empty root state (the root snapshot always arrives via `initialize`/
/// `subscribe`, but `Default` must exist for the book).
fn empty_root() -> RootState {
    serde_json::from_value(serde_json::json!({ "agents": [] }))
        .expect("an empty root state is constructible")
}

/// A minimal session state to fold deltas into before the snapshot lands
/// (the SDK guarantees snapshot-then-delta, but a first delta can race a
/// subscribe reply through the fan-in stream).
fn empty_session(uri: &str) -> SessionState {
    serde_json::from_value(serde_json::json!({
        "resource": uri,
        "provider": "",
        "title": "",
        "status": 0,
        "modifiedAt": "",
        "lifecycle": "creating",
        "activeClients": [],
        "chats": [],
    }))
    .expect("an empty session state is constructible from its resource alone")
}

/// See [`empty_session`].
fn empty_chat(uri: &str) -> ChatState {
    serde_json::from_value(serde_json::json!({
        "resource": uri,
        "title": "",
        "status": 0,
        "modifiedAt": "",
        "turns": [],
    }))
    .expect("an empty chat state is constructible from its resource alone")
}

/// One per-model (or per-request) usage row of the Q face (`metricType:
/// "side_call"` / `"main_call"` rows carry these under `data`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UsageSnapshot {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

impl UsageSnapshot {
    fn from_json(v: &Value) -> Self {
        Self {
            input: v.get("input").and_then(Value::as_u64).unwrap_or(0),
            output: v.get("output").and_then(Value::as_u64).unwrap_or(0),
            cache_creation: v.get("cacheWrite").and_then(Value::as_u64).unwrap_or(0),
            cache_read: v.get("cacheRead").and_then(Value::as_u64).unwrap_or(0),
        }
    }

    /// The runtime `TokenUsage` the rail's renderer consumes.
    pub fn to_tokens(self) -> manox_agent::TokenUsage {
        manox_agent::TokenUsage {
            input_tokens: self.input,
            output_tokens: self.output,
            cache_creation_input_tokens: self.cache_creation,
            cache_read_input_tokens: self.cache_read,
        }
    }
}

/// The aggregated Q face (the retired `GetConversationInfo` payload), folded
/// from `x-manox-metrics/changed` increments on the chat's metrics channel.
#[derive(Debug, Clone, Default)]
pub struct ConversationMetrics {
    pub cumulative_usage: Option<UsageSnapshot>,
    pub cumulative_cost: f64,
    pub per_model_usage: HashMap<String, UsageSnapshot>,
    pub per_model_cost: HashMap<String, f64>,
}

impl ConversationMetrics {
    /// Apply one metrics increment. `kind`/`data` ride the journal row
    /// verbatim (`metricType`/`data` on the wire); unknown kinds are
    /// tolerated.
    pub fn apply(&mut self, kind: &str, data: &Value) -> bool {
        match kind {
            "conversation" => {
                if let Some(cu) = data.get("cumulativeUsage") {
                    self.cumulative_usage = Some(UsageSnapshot::from_json(cu));
                }
                if let Some(cost) = data.get("cumulativeCost").and_then(Value::as_f64) {
                    self.cumulative_cost = cost;
                }
                self.per_model_usage.clear();
                self.per_model_cost.clear();
                if let Some(models) = data.get("models").and_then(Value::as_array) {
                    for row in models {
                        // The host emits `model` as the canonical identity
                        // `{provider}/{model}`; re-prefixing would double it.
                        let Some(key) = row.get("model").and_then(Value::as_str) else {
                            continue;
                        };
                        self.per_model_usage
                            .insert(key.to_string(), UsageSnapshot::from_json(row));
                    }
                }
                if let Some(costs) = data.get("perModelCost").and_then(Value::as_object) {
                    for (key, cost) in costs {
                        if let Some(c) = cost.as_f64() {
                            self.per_model_cost.insert(key.clone(), c);
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }
}

/// The bare id a channel URI carries (`ahp-session:/<id>` → `<id>`).
pub fn id_of(uri: &str) -> &str {
    uri.split_once(":/").map(|(_, id)| id).unwrap_or(uri)
}

/// Build a session channel URI for a client-minted id.
pub fn session_uri(id: &str) -> String {
    format!("ahp-session:/{id}")
}

/// Build a chat channel URI for an id.
pub fn chat_uri(id: &str) -> String {
    format!("ahp-chat:/{id}")
}

/// Build the plan extension channel URI for a chat id — the one home of the
/// plan-review lifecycle (proposal and verdict), whose baseline is the only
/// fold-visible record of a review's settlement.
pub fn plan_uri(id: &str) -> String {
    format!("{}{id}", manox_ahp::ext::channels::PLAN)
}

/// Build the thread extension channel URI for a session id — the post-#842
/// home of the thread rows (pinned / label / session info / leaf cursor).
/// Upstream #818–#842 first emitted them on the session channel, where no
/// fold ever consumed them.
pub fn thread_uri(id: &str) -> String {
    format!("{}{id}", manox_ahp::ext::channels::THREAD)
}

/// Build the work extension channel URI for a session id — goal, background
/// tasks, browser suites, sub-agents, active tools.
pub fn work_uri(id: &str) -> String {
    format!("{}{id}", manox_ahp::ext::channels::WORK)
}

// ---------------------------------------------------------------------------
// AhpStore: the entity the UI binds.
// ---------------------------------------------------------------------------

/// A reply slot for a command that awaits the host's answer.
pub type Reply = async_channel::Receiver<Result<Value, String>>;

/// A write issued before the handshake, replayed **in order** on connect —
/// the landing createSession precedes its subscribes, and the subscribes
/// precede the active-client claim, so the replay awaits each step before
/// issuing the next. `Dispatch` is boxed: the typed actions dwarf the URI
/// strings and the queue is transient.
enum PendingWrite {
    CreateSession {
        session_id: String,
        cwds: Vec<String>,
        config: Option<serde_json::Map<String, Value>>,
        reply: async_channel::Sender<Result<Value, String>>,
    },
    Subscribe(String),
    Unsubscribe(String),
    Dispatch(String, Box<StateAction>),
}

/// The UI-side answerer signature for host → client capability requests.
pub type RequestHook = Arc<dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync>;

/// One inbound host → client capability request plus its reply slot.
pub struct CapabilityRequest {
    pub method: String,
    pub params: Value,
    pub reply: async_channel::Sender<Result<Value, String>>,
}

/// Why the submit pipeline cannot currently deliver. One visible reason —
/// the composer's error strip reads this instead of the user guessing why
/// nothing sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryFailure {
    /// The in-proc handshake never completed: every write queues forever
    /// behind it (the silent black hole this face exists to end).
    Handshake(String),
    /// The last dispatch failed to leave the client (transport gone).
    Dispatch(String),
    /// Pre-connect writes have sat queued past the delivery budget.
    Stalled,
}

/// One dispatch outcome bound for the store's own pump: a failure latches
/// the delivery face, a recovery retires a latched `Dispatch` face (a
/// dispatch that leaves proves the transport answers again).
#[derive(Clone, Debug)]
enum DeliverySignal {
    Failed(DeliveryFailure),
    DispatchRecovered,
}

/// Per-session display-event bucket bound. A bucket is a delta log against
/// the conversation's build state: the live leg applies it, and a rebuild
/// from the book — which already folded every event the bucket holds —
/// consumes it outright. A bucket belonging to a session nobody has bound
/// since accumulates unread; the oldest entries are dropped, because the
/// rebuild (not the bucket) is what makes that session's content whole.
const CHAT_EVENT_BUCKET_CAP: usize = 512;

/// The GPUI-facing store. One per process: the host is a singleton (L11) and
/// this is its only desktop client.
pub struct AhpStore {
    pub book: ChannelBook,
    client: Option<Client>,
    /// True while the pre-connect queue is being replayed; writes issued in
    /// that window queue behind the replay instead of racing it.
    replay_pending: bool,
    /// Display events derived from chat-channel folds since their last
    /// drain, bucketed by the owning session — the live-streaming leg the
    /// conversation applier consumes. AHP stamps every action envelope with
    /// its channel while chat payloads carry no URI of their own, so a
    /// derived display event must re-attach the channel's session here;
    /// consumers drain only the bucket they hold a binding for.
    chat_events: HashMap<String, VecDeque<crate::chat_fold::ChatEvent>>,
    /// The foreground session id (the multiplexer publishes on focus
    /// changes): chat activity in any OTHER session rises the official
    /// unread bit (`session/isReadChanged` — client-owned observation
    /// state, echoed to every subscriber) instead of a local map.
    focused_session: Option<String>,
    /// Optimistic writes the host has not confirmed yet, oldest first:
    /// (session id, config key, the value the key had before). A rejection
    /// restores the remembered values so a refused pick does not linger.
    /// Per-key write-ahead undo chains: `(session, key, base, written)` —
    /// one entry per in-flight optimistic write, in write order. Bases and
    /// written values are matched by value on refusal/confirmation (see
    /// `rollback_refused_config` / `confirm_config`).
    optimistic_undo: Vec<(String, String, Option<Value>, Option<Value>)>,
    /// Writes issued before the handshake completed, replayed in order on
    /// connect. The landing session's create/subscribe rides this: the
    /// workspace constructs the store and binds the chat column in the same
    /// frame, long before the in-proc handshake answers.
    pending_writes: Vec<PendingWrite>,
    /// Host → client capability requests, answered by the UI layer's hook.
    request_hook: Option<RequestHook>,
    /// Recent dispatch rejections (bounded ring) for UI surfacing.
    rejections: std::collections::VecDeque<String>,
    /// Session ids whose thread channel this store already subscribed — the
    /// sidebar subscribes one channel per catalogue row (the pin badge and
    /// the pin dispatch both read `x-manox-thread` state), re-issued only
    /// for rows that appear after their subscription.
    thread_subs: std::collections::HashSet<String>,
    /// Sessions with an unread-rise write in flight (issued, the host echo
    /// not folded yet): the rise fires at most once per read→unread
    /// transition instead of once per background action while a turn
    /// streams.
    pending_unread: std::collections::HashSet<String>,
    /// The submit pipeline's LATCHED failure: a handshake that never landed
    /// or a dispatch that failed to leave. The lazy `Stalled` face (a
    /// pre-connect queue past the budget) is computed on read, never stored.
    /// The composer's error strip reads [`Self::delivery_failure`].
    delivery_failure: Option<DeliveryFailure>,
    /// When the first pre-connect write queued — the stall face's clock.
    /// `None` while nothing queues; cleared when the replay drains.
    queue_head_at: Option<std::time::Instant>,
    /// The handshake's dial parameters, retained so [`Self::retry_handshake`]
    /// can re-run the same spawn after a failure.
    handshake_params: Option<(
        PathBuf,
        async_channel::Sender<ahp::ClientEvent>,
        async_channel::Sender<CapabilityRequest>,
    )>,
    /// A handshake (initial or retried) is currently dialing: `retry` must
    /// not stack a second dial on top of a slow one.
    handshaking: bool,
    /// Dispatch outcomes ride this to the store's own pump — the dispatch
    /// spawn runs on the manox runtime with no gpui handle, and the face
    /// needs the entity.
    delivery_signal_tx: Option<async_channel::Sender<DeliverySignal>>,
    _tasks: Vec<gpui::Task<()>>,
}

impl AhpStore {
    /// Wire the store to the process-singleton host and start the pumps.
    ///
    /// `agent_server::global(cwd)` must have run first — it installs the AHP
    /// runtime builder this dials into.
    pub fn connect(cwd: PathBuf, cx: &mut gpui::Context<Self>) -> Self {
        let (event_tx, event_rx) = async_channel::unbounded::<ahp::ClientEvent>();
        let (req_tx, req_rx) = async_channel::unbounded::<CapabilityRequest>();
        let (fail_tx, fail_rx) = async_channel::unbounded::<DeliverySignal>();
        let mut store = Self {
            book: ChannelBook::default(),
            client: None,
            replay_pending: false,
            chat_events: HashMap::new(),
            focused_session: None,
            optimistic_undo: Vec::new(),
            pending_writes: Vec::new(),
            request_hook: None,
            rejections: std::collections::VecDeque::new(),
            thread_subs: std::collections::HashSet::new(),
            pending_unread: std::collections::HashSet::new(),
            delivery_failure: None,
            queue_head_at: None,
            handshake_params: None,
            handshaking: false,
            delivery_signal_tx: Some(fail_tx),
            _tasks: Vec::new(),
        };
        store.spawn_handshake(cwd, event_tx, req_tx, cx);
        store.spawn_pump(event_rx, cx);
        store.spawn_capability_pump(req_rx, cx);
        store.spawn_delivery_failure_pump(fail_rx, cx);
        store
    }

    /// Whether the handshake completed (writes before this are queued).
    pub fn is_connected(&self) -> bool {
        self.client.is_some()
    }

    /// How long a pre-connect queue may sit before the composer reads it as
    /// a delivery failure. The handshake either lands (the queue drains) or
    /// latches its own face long before a healthy host crosses this.
    const DELIVERY_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

    /// The submit pipeline's failure face, if any: the latched handshake /
    /// dispatch failure, or — computed on read, so the healthy path pays
    /// nothing — a pre-connect queue that has sat past the delivery budget.
    pub fn delivery_failure(&self) -> Option<DeliveryFailure> {
        if let Some(failure) = &self.delivery_failure {
            return Some(failure.clone());
        }
        if let Some(queued_at) = self.queue_head_at
            && self.client.is_none()
            && queued_at.elapsed() >= Self::DELIVERY_BUDGET
        {
            return Some(DeliveryFailure::Stalled);
        }
        None
    }

    /// Re-dial the host after a handshake failure (the composer error
    /// strip's retry). A no-op while connected — retry exists for the
    /// failure face only — and while the first handshake is still in
    /// flight, so a slow host never gets dialled twice.
    pub fn retry_handshake(&mut self, cx: &mut gpui::Context<Self>) {
        if self.client.is_some() || self.handshaking {
            return;
        }
        let Some((cwd, event_tx, req_tx)) = self.handshake_params.clone() else {
            return;
        };
        self.delivery_failure = None;
        self.spawn_handshake(cwd, event_tx, req_tx, cx);
        cx.notify();
    }

    /// Apply one dispatch outcome to the delivery face: a failure latches,
    /// a recovery retires a latched `Dispatch` face (a `Handshake` face is
    /// not a dispatch's to clear). Returns whether the face changed — the
    /// pump notifies only then, so a recovery against no latch stays free.
    fn absorb_delivery_signal(&mut self, signal: DeliverySignal) -> bool {
        match signal {
            DeliverySignal::Failed(failure) => {
                self.delivery_failure = Some(failure);
                true
            }
            DeliverySignal::DispatchRecovered => {
                if matches!(self.delivery_failure, Some(DeliveryFailure::Dispatch(_))) {
                    self.delivery_failure = None;
                    return true;
                }
                false
            }
        }
    }

    /// Fold dispatch outcomes onto the store's delivery face. The dispatch
    /// spawn runs on the manox runtime with no gpui handle, so outcomes
    /// ride this channel to a task that owns the entity.
    fn spawn_delivery_failure_pump(
        &mut self,
        fail_rx: async_channel::Receiver<DeliverySignal>,
        cx: &mut gpui::Context<Self>,
    ) {
        let task = cx.spawn(async move |this, cx| {
            while let Ok(signal) = fail_rx.recv().await {
                this.update(cx, |store, cx| {
                    if store.absorb_delivery_signal(signal) {
                        cx.notify();
                    }
                })
                .ok();
            }
        });
        self._tasks.push(task);
    }

    /// A store that never dials the host: the folded book is a plain struct
    /// tests seed directly, and every write queues as a pending write.
    /// Diagnostic-only — the production constructor is [`Self::connect`].
    #[cfg(feature = "test-support")]
    pub fn detached() -> Self {
        Self {
            book: ChannelBook::default(),
            client: None,
            replay_pending: false,
            chat_events: HashMap::new(),
            focused_session: None,
            optimistic_undo: Vec::new(),
            pending_writes: Vec::new(),
            request_hook: None,
            rejections: std::collections::VecDeque::new(),
            thread_subs: std::collections::HashSet::new(),
            pending_unread: std::collections::HashSet::new(),
            delivery_failure: None,
            queue_head_at: None,
            handshake_params: None,
            handshaking: false,
            delivery_signal_tx: None,
            _tasks: Vec::new(),
        }
    }

    /// Latch a handshake-shaped delivery failure without a real dead
    /// transport. Test-support diagnostic: the composer strip's render and
    /// wiring tests drive exactly what the pump latches.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_latch_delivery_failure(&mut self) {
        self.delivery_failure = Some(DeliveryFailure::Handshake("test: dead transport".into()));
    }

    /// Clear the latched failure. Test-support diagnostic: the strip's
    /// render test drives what a successful redial clears.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_clear_delivery_failure(&mut self) {
        self.delivery_failure = None;
    }

    /// Route one wire-shaped event through the production dispatch point.
    /// Test-support diagnostic: a detached store has no client pump, so
    /// tests feed envelopes through the same `route` the live pump runs —
    /// the bucket routing under test lives there.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_route_event(
        &mut self,
        event: ahp::ClientEvent,
        cx: &mut gpui::Context<Self>,
    ) {
        self.route(event, cx);
    }

    /// Seed a placeholder summary for a session this client just created, so
    /// the sidebar row exists the moment the conversation starts: the host's
    /// store row (what `listSessions` serves) lands with the first
    /// persistence, which can lag a whole turn. The host's summary upserts
    /// over the placeholder (`seed_summaries` keys by id).
    /// The session's unresolved annotation cards as (kind, text) render
    /// pairs — the annotations channel's read face for the transcript
    /// rebuild. Kinds ride `_meta["x-manox"]["uiNoteKind"]`; an annotation
    /// without one reads as a plain notice.
    pub fn annotation_cards(&self, session_id: &str) -> Vec<(String, String)> {
        let Some(state) = self.book.annotations.get(session_id) else {
            return Vec::new();
        };
        state
            .annotations
            .iter()
            .filter(|annotation| !annotation.resolved)
            .map(|annotation| {
                let kind = annotation
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("x-manox"))
                    .and_then(|x| x.get("uiNoteKind"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("notice")
                    .to_string();
                let text = annotation
                    .entries
                    .iter()
                    .map(|entry| match &entry.text {
                        ahp_types::common::StringOrMarkdown::Plain(text) => text.clone(),
                        ahp_types::common::StringOrMarkdown::Markdown { markdown } => {
                            markdown.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (kind, text)
            })
            .collect()
    }

    /// Publish the foreground session (the multiplexer's focus call): chat
    /// activity elsewhere rises the official unread bit, and the newly
    /// focused session's bit clears through the same protocol (the echo
    /// lands for every subscriber, this one included).
    pub fn set_focused_session(&mut self, session_id: Option<&str>) {
        self.focused_session = session_id.map(str::to_string);
        if let Some(session_id) = session_id {
            self.dispatch(
                session_uri(session_id),
                StateAction::SessionIsReadChanged(ahp_types::actions::SessionIsReadChangedAction {
                    is_read: true,
                }),
            );
        }
    }

    /// The sessions whose official unread bit is raised (live fold first,
    /// the summary row as the parked fallback) — the dock badge's source.
    pub fn unread_sessions(&self) -> usize {
        self.book
            .summaries
            .values()
            .filter(|summary| {
                let sid = id_of(&summary.resource);
                // Both bits read live-fold-first: the isReadChanged and
                // isArchivedChanged echoes land on the session state the
                // moment they happen; the summary row is the parked fallback.
                let bit = |mask: u32| match self.book.sessions.get(sid) {
                    Some(state) => state.status & mask != 0,
                    None => summary.status & mask != 0,
                };
                !bit(ahp_types::state::SessionStatus::IsRead.bits())
                    && !bit(ahp_types::state::SessionStatus::IsArchived.bits())
            })
            .count()
    }

    pub fn seed_local_summary(&mut self, session_id: &str, title: &str) -> bool {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.book.seed_summaries(vec![SessionSummary {
            provider: String::new(),
            title: title.to_string(),
            // The host's own rows always carry Idle|IsRead; a zeroed status
            // would render the placeholder as an UNREAD conversation.
            status: ahp_types::state::SessionStatus::Idle.bits()
                | ahp_types::state::SessionStatus::IsRead.bits(),
            activity: None,
            origin: None,
            project: None,
            working_directories: None,
            annotations: None,
            resource: session_uri(session_id),
            created_at: now.clone(),
            modified_at: now,
            changes: None,
            meta: None,
            chats: None,
            default_chat: None,
        }])
    }

    /// Pop the next queued write, if any and if the replay may proceed.
    fn next_pending(&mut self) -> Option<PendingWrite> {
        if !self.replay_pending {
            return None;
        }
        let next = (!self.pending_writes.is_empty()).then(|| self.pending_writes.remove(0));
        if next.is_some() && self.pending_writes.is_empty() {
            // The queue drained: the stall face's clock stops.
            self.queue_head_at = None;
        }
        next
    }

    fn spawn_handshake(
        &mut self,
        cwd: PathBuf,
        event_tx: async_channel::Sender<ahp::ClientEvent>,
        req_tx: async_channel::Sender<CapabilityRequest>,
        cx: &mut gpui::Context<Self>,
    ) {
        self.handshake_params = Some((cwd.clone(), event_tx.clone(), req_tx.clone()));
        self.handshaking = true;
        let take = cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let (client, init) = match tokio_wait(move || async move {
                let runtime = manox_ahp_runtime::ahp::runtime::runtime(cwd);
                let transport = runtime.inproc();
                let client = Client::connect(transport, ClientConfig::default())
                    .await
                    .map_err(|err| format!("connect: {err}"))?;
                // Explicit — the SDK's read pump starts here, not in
                // `Client::connect`; skipping it leaves every request timing
                // out against a perfectly healthy host.
                let init = client
                    .initialize(
                        CLIENT_ID.to_string(),
                        // The full supported list, not just the preferred
                        // entry: the host picks the highest caret-compatible
                        // offer, so a newer build also speaks to a host one
                        // baseline behind.
                        ahp_types::version::SUPPORTED_PROTOCOL_VERSIONS
                            .iter()
                            .map(|version| version.to_string())
                            .collect(),
                        vec![ROOT_RESOURCE_URI.to_string()],
                    )
                    .await
                    .map_err(|err| format!("initialize: {err}"))?;
                // Capability requests ride the same client; the answers come
                // from the UI layer through the request channel.
                let bridge_tx = req_tx.clone();
                client.set_server_request_handler(move |method, params| {
                    let tx = bridge_tx.clone();
                    async move { answer_via_bridge(tx, method, params).await }
                });
                Ok::<(Client, InitializeResult), String>((client, init))
            })
            .await
            {
                Ok((client, init)) => (client, init),
                Err(err) => {
                    tracing::error!(error = %err, "ahp handshake failed");
                    // The failure must not be silent: every write issued now
                    // queues behind a handshake that will never answer, and
                    // `next_pending` gates the drain on a landing that never
                    // comes — the black hole. Latch the face the composer's
                    // error strip reads; `retry_handshake` re-dials.
                    this.update(cx, |store, cx| {
                        store.handshaking = false;
                        store.delivery_failure = Some(DeliveryFailure::Handshake(err.clone()));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            if this
                .update(cx, |store, _| {
                    store.client = Some(client.clone());
                    store.replay_pending = true;
                    store.handshaking = false;
                    // The handshake landed: its own failure face retires. A
                    // Dispatch latch (a pre-existing transport death) is not
                    // this success's to clear.
                    if matches!(store.delivery_failure, Some(DeliveryFailure::Handshake(_))) {
                        store.delivery_failure = None;
                    }
                })
                .is_err()
            {
                return;
            }
            // Replay the pre-connect writes **sequentially**: the landing
            // createSession must complete before its subscribes, and the
            // subscribes before the active-client claim. Concurrent issue
            // lets the host answer `not found` for a chat whose session is
            // still being created (real-device smoke, round 5).
            while let Some(write) = this
                .update(cx, |store, _| store.next_pending())
                .ok()
                .flatten()
            {
                match write {
                    PendingWrite::CreateSession {
                        session_id,
                        cwds,
                        config,
                        reply,
                    } => {
                        let result = tokio_wait({
                            let client = client.clone();
                            let channel = session_uri(&session_id);
                            move || async move {
                                client
                                    .request::<_, Value>(
                                        "createSession",
                                        CreateSessionParams {
                                            channel,
                                            meta: None,
                                            provider: None,
                                            working_directories: Some(cwds),
                                            config,
                                            active_client: None,
                                            progress_token: None,
                                        },
                                    )
                                    .await
                                    .map_err(|err| err.to_string())
                            }
                        })
                        .await;
                        let _ = reply.send(result).await;
                    }
                    PendingWrite::Subscribe(uri) => {
                        let folded = tokio_wait({
                            let client = client.clone();
                            let uri = uri.clone();
                            move || async move {
                                client
                                    .request::<_, Value>(
                                        "subscribe",
                                        serde_json::json!({ "channel": uri }),
                                    )
                                    .await
                                    .map_err(|err| err.to_string())
                                    .and_then(|v| {
                                        serde_json::from_value::<SubscribeResult>(v)
                                            .map_err(|err| err.to_string())
                                    })
                            }
                        })
                        .await;
                        // Fold the snapshot the same way the direct path does:
                        // without it a fresh landing session runs on the
                        // empty-session fallback whose `config` seat is None,
                        // and every configChanged merge no-ops (the chip's
                        // picked model never lands).
                        if let Ok(result) = folded
                            && let Some(snapshot) = result.snapshot
                        {
                            let _ = this.update(cx, |store, cx| {
                                let uri = snapshot.resource.clone();
                                if store.apply_snapshot_confirmed(&uri, snapshot.state) {
                                    cx.notify();
                                }
                            });
                        }
                    }
                    PendingWrite::Unsubscribe(uri) => {
                        let _ = tokio_wait({
                            let client = client.clone();
                            move || async move {
                                client
                                    .request::<_, Value>(
                                        "unsubscribe",
                                        serde_json::json!({ "channel": uri }),
                                    )
                                    .await
                                    .map_err(|err| err.to_string())
                            }
                        })
                        .await;
                    }
                    PendingWrite::Dispatch(channel, action) => {
                        let _ = tokio_wait({
                            let client = client.clone();
                            move || async move {
                                client
                                    .dispatch(channel, *action)
                                    .await
                                    .map_err(|err| err.to_string())
                            }
                        })
                        .await;
                    }
                }
            }
            let _ = this.update(cx, |store, _| store.replay_pending = false);
            // Initial catalogue pull — the v3 successor of the v2 Ready →
            // first-pull leg. The initialize snapshots carry the root state
            // (the agents catalogue); listSessions seeds the sidebar with
            // existing history; the two catalogue channels bring the
            // project/command surfaces up. Without this the sidebar and the
            // model list stay empty until an unrelated event fires.
            let _ = this.update(cx, |store, cx| {
                for snap in init.snapshots {
                    let uri = snap.resource.clone();
                    if store.apply_snapshot_confirmed(&uri, snap.state) {
                        cx.notify();
                    }
                }
                for uri in [ext::channels::WORKSPACES, ext::channels::COMMANDS] {
                    store.subscribe(uri, cx);
                }
                store.refresh_sessions(cx);
            });
            // The events receiver must exist before any subscribe: the
            // extension baselines are pushed while the subscribe is answered.
            let mut events = client.events();
            while let Some(event) = events.recv().await {
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
            tracing::error!("ahp pump exited (transport closed)");
            let _ = this.update(cx, |store, cx| {
                store.client = None;
                cx.notify();
            });
        });
        self._tasks.push(take);
    }

    /// Fold every inbound event; notify once per batch of visible changes.
    fn spawn_pump(
        &mut self,
        event_rx: async_channel::Receiver<ahp::ClientEvent>,
        cx: &mut gpui::Context<Self>,
    ) {
        let pump = cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            while let Ok(event) = event_rx.recv().await {
                let outcome = this.update(cx, |store, cx| store.route(event, cx));
                if outcome.is_err() {
                    break;
                }
            }
        });
        self._tasks.push(pump);
    }

    fn spawn_capability_pump(
        &mut self,
        req_rx: async_channel::Receiver<CapabilityRequest>,
        cx: &mut gpui::Context<Self>,
    ) {
        let pump = cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            while let Ok(req) = req_rx.recv().await {
                let CapabilityRequest {
                    method,
                    params,
                    reply,
                } = req;
                let answered = this.read_with(cx, |store, _| {
                    store
                        .request_hook
                        .as_ref()
                        .map(|hook| hook(&method, &params))
                });
                let answer = match answered {
                    Ok(Some(result)) => result,
                    Ok(None) => Err(format!("no handler for {method}")),
                    Err(_) => break,
                };
                if reply.send(answer).await.is_err() {
                    break;
                }
            }
        });
        self._tasks.push(pump);
    }

    /// Register the UI-side answerer for host → client capability requests
    /// (`x-manox/browserOp|clipboardRead|openExternal|invokeTool`).
    pub fn set_request_handler(&mut self, hook: RequestHook) {
        self.request_hook = Some(hook);
    }

    /// Bucket one derived display event under its owning session, dropping
    /// the oldest entry past [`CHAT_EVENT_BUCKET_CAP`].
    fn push_chat_event(&mut self, session_id: String, event: crate::chat_fold::ChatEvent) {
        let bucket = self.chat_events.entry(session_id).or_default();
        bucket.push_back(event);
        while bucket.len() > CHAT_EVENT_BUCKET_CAP {
            bucket.pop_front();
        }
    }

    /// Route one inbound event into the fold.
    ///
    /// This is the client's single dispatch point — the AHP specification
    /// makes the envelope's `channel` the universal routing key ("dispatch
    /// any incoming message by inspecting `(method, params.channel)`"), and
    /// chat payloads carry no URI of their own. Everything derived from an
    /// envelope here must re-attach that identity before leaving the store:
    /// the display buckets are keyed by the owning session for exactly that
    /// reason.
    fn route(&mut self, event: ahp::ClientEvent, cx: &mut gpui::Context<Self>) {
        match event.event {
            ahp::SubscriptionEvent::Action(envelope) => {
                self.book.server_seq = self.book.server_seq.max(envelope.server_seq);
                let own = envelope
                    .origin
                    .as_ref()
                    .is_some_and(|origin| origin.client_id == CLIENT_ID);
                // Write-ahead reconciliation (AHP): an envelope carrying
                // `rejectionReason` is the host's refusal — its action did
                // not land, so it must not enter the fold, and this
                // client's optimistic twin rolls back. Folding refusals as
                // if accepted left a rejected model or mode pick on the
                // chip forever; the rollback used to be reachable only
                // through reducer-level rejections, which the session
                // reducer never produces for config actions.
                if let Some(reason) = envelope.rejection_reason.clone() {
                    tracing::warn!(channel = %envelope.channel, %reason, "dispatch rejected");
                    self.rejections
                        .push_back(format!("{}: {reason}", envelope.channel));
                    while self.rejections.len() > 32 {
                        self.rejections.pop_front();
                    }
                    // A refused unread-rise write never dropped the bit,
                    // so the gate must re-arm — the next background
                    // action may retry the rise.
                    if envelope.channel.starts_with("ahp-session:/") {
                        self.pending_unread.remove(id_of(&envelope.channel));
                    }
                    if own {
                        // Every own refusal is visible: a config write
                        // rolls its optimistic twin back by VALUE (see
                        // `rollback_refused_config`), a cwd write rolls its
                        // latest twin, and any other refused action — a
                        // submit, an answer card, a plan verdict — at least
                        // says so on the transcript. The composer pushes
                        // its bubble optimistically, and a refusal IS the
                        // echo, so silence here would strand the bubble
                        // with no reply and no explanation.
                        // The refusal speaks on the SESSION's bucket, not the
                        // channel's own id: a chat payload carries no URI of
                        // its own, so an `ahp-chat:` channel resolves through
                        // its owning session exactly as the derived display
                        // events do — a notice bucketed under a chat id lands
                        // in a bucket no consumer drains (the stranded
                        // bubble this arm exists to explain).
                        let session_id = if envelope.channel.starts_with("ahp-chat:/") {
                            self.book
                                .session_of_chat(id_of(&envelope.channel))
                                .map(str::to_string)
                        } else {
                            Some(id_of(&envelope.channel).to_string())
                        };
                        if let Some(session_id) = session_id {
                            match config_values_of(&envelope.action) {
                                Some(config) => {
                                    self.rollback_refused_config(&session_id, config);
                                }
                                None => {
                                    if let Some(keys) = touched_config_keys(&envelope.action) {
                                        self.rollback_latest(&session_id, &keys);
                                    }
                                }
                            }
                            self.push_chat_event(
                                session_id,
                                crate::chat_fold::ChatEvent::Notice {
                                    text: format!(
                                        "{}: {reason}",
                                        steer_i18n::t("workspace-change-rejected")
                                    ),
                                },
                            );
                        }
                    }
                    cx.notify();
                    return;
                }
                // The acceptance half: our own accepted config write
                // spends exactly its own undo entry (matched by value) and
                // everything it superseded — a later refusal of another
                // in-flight write to the same key restores to this
                // confirmed value, not past it.
                if own && let Some(config) = config_values_of(&envelope.action) {
                    self.confirm_config(id_of(&envelope.channel), config);
                } else if own && let Some(keys) = touched_config_keys(&envelope.action) {
                    self.confirm_latest(id_of(&envelope.channel), &keys);
                }
                let chat_id = if envelope.channel.starts_with("ahp-chat:/") {
                    Some(id_of(&envelope.channel).to_string())
                } else {
                    None
                };
                match self.book.apply(&envelope.channel, &envelope.action) {
                    FoldEffect::Changed => {
                        // The official unread rise (#870's app half):
                        // visible chat activity in a session that is not the
                        // foreground clears the IsRead bit through the
                        // protocol — the host echoes the action to every
                        // subscriber, so all clients (and this book's live
                        // fold) see the same dot. The owning session is
                        // resolved from the chat (a chat id equals its
                        // session id only in the landing form), and the rise
                        // fires at most once per read→unread transition: an
                        // in-flight gate blocks re-fires until the fold
                        // shows the bit back down (this write's echo, or
                        // another client's), so one streaming background
                        // turn dispatches once — not once per action.
                        let owning = if let Some(chat_id) = &chat_id {
                            self.book.session_of_chat(chat_id).map(str::to_string)
                        } else if envelope.channel.starts_with("ahp-session:/") {
                            Some(id_of(&envelope.channel).to_string())
                        } else {
                            None
                        };
                        if let Some(session_id) = &owning
                            && self.book.sessions.get(session_id).is_some_and(|state| {
                                state.status & ahp_types::state::SessionStatus::IsRead.bits() == 0
                            })
                        {
                            self.pending_unread.remove(session_id);
                        }
                        if let Some(session_id) = &owning
                            && self.focused_session.as_deref() != Some(session_id.as_str())
                            && let Some(session) = self.book.sessions.get(session_id)
                            && session.status & ahp_types::state::SessionStatus::IsRead.bits() != 0
                            && self.pending_unread.insert(session_id.clone())
                        {
                            self.dispatch(
                                session_uri(session_id),
                                StateAction::SessionIsReadChanged(
                                    ahp_types::actions::SessionIsReadChangedAction {
                                        is_read: false,
                                    },
                                ),
                            );
                        }
                        if let Some(chat_id) = chat_id
                            && let Some(chat) = self.book.chats.get(&chat_id)
                            && let Some(event) = crate::chat_fold::ChatEvent::from_action(
                                &envelope.action,
                                Some(chat),
                            )
                        {
                            // The derived event re-attaches the channel's
                            // owning session — an envelope with no owning
                            // session (a chat no session catalog lists) has
                            // no legitimate display consumer, so the event
                            // is dropped; the fold's rebuild covers content.
                            if let Some(session_id) = owning {
                                self.push_chat_event(session_id, event);
                            }
                        }
                        cx.notify();
                    }
                    FoldEffect::Ignored => {}
                    FoldEffect::Rejected(reason) => {
                        tracing::warn!(channel = %envelope.channel, reason = %reason,
                            "dispatch rejected");
                        self.rejections
                            .push_back(format!("{}: {reason}", envelope.channel));
                        while self.rejections.len() > 32 {
                            self.rejections.pop_front();
                        }
                        // A refused unread-rise write never dropped the bit,
                        // so the gate must re-arm — the next background
                        // action may retry the rise.
                        if envelope.channel.starts_with("ahp-session:/") {
                            self.pending_unread.remove(id_of(&envelope.channel));
                        }
                        cx.notify();
                    }
                }
            }
            ahp::SubscriptionEvent::SessionAdded(params) => {
                let changed = self.book.seed_summaries(vec![params.summary]);
                self.subscribe_thread_channels_for_summaries(cx);
                if changed {
                    cx.notify();
                }
            }
            // (the collapsible-if lint fires on the guard-shaped ifs below;
            // they are match-arm bodies, kept as-is is not allowed, so they
            // are restructured)
            ahp::SubscriptionEvent::SessionSummaryChanged(_) => {
                // Deltas are partial; refetching the page is the fold-correct
                // response (the host answers from store rows, not journals).
                self.refresh_sessions(cx);
            }
            ahp::SubscriptionEvent::SessionRemoved(params) => {
                let removed = self.book.remove_summary(&params.channel);
                if removed {
                    self.book.sessions.remove(id_of(&params.channel));
                    cx.notify();
                }
            }
            ahp::SubscriptionEvent::AuthRequired(_) => {
                tracing::info!("auth/required ignored (no auth surface on the desktop)");
            }
            _ => {
                // The SDK's per-URI subscription events (a snapshot tail, an
                // auth nudge) all reach this store through the global fan-in
                // stream as well; anything unmodelled is tolerated.
            }
        }
    }

    /// Subscribe one channel. The snapshot (state-bearing channels) folds
    /// into the book when the host answers.
    pub fn subscribe(&mut self, uri: impl Into<String>, cx: &mut gpui::Context<Self>) {
        let uri: String = uri.into();
        // A write issued while the pre-connect replay is running queues behind
        // it: racing the landing createSession/subscribe silently loses the
        // first message (the host answers not-found for a session still
        // being created).
        if self.client.is_none() || self.replay_pending {
            self.pending_writes.push(PendingWrite::Subscribe(uri));
            return;
        }
        let client = self.client.clone().expect("guard above");
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let value = tokio_wait(move || async move {
                client
                    .request::<_, Value>("subscribe", serde_json::json!({ "channel": uri }))
                    .await
                    .map_err(|err| err.to_string())
            })
            .await;
            let result: SubscribeResult = match value
                .and_then(|v| serde_json::from_value(v).map_err(|err| err.to_string()))
            {
                Ok(result) => result,
                Err(err) => {
                    tracing::warn!(error = %err, "subscribe failed");
                    return;
                }
            };
            if let Some(snapshot) = result.snapshot {
                let _ = this.update(cx, |store, cx| {
                    let uri = snapshot.resource.clone();
                    if store.apply_snapshot_confirmed(&uri, snapshot.state) {
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// Unsubscribe one channel.
    pub fn unsubscribe(&mut self, uri: impl Into<String>) {
        let uri = uri.into();
        // A write issued while the pre-connect replay is running queues behind
        // it: racing the landing createSession/subscribe silently loses the
        // first message (the host answers not-found for a session still
        // being created).
        if self.client.is_none() || self.replay_pending {
            self.pending_writes.push(PendingWrite::Unsubscribe(uri));
            return;
        }
        let client = self.client.clone().expect("guard above");
        manox_agent::runtime::handle().spawn(async move {
            if let Err(err) = client
                .request::<_, Value>("unsubscribe", serde_json::json!({ "channel": uri }))
                .await
            {
                tracing::warn!(error = %err, "unsubscribe failed");
            }
        });
    }

    /// Dispatch one action (write-ahead; the echo folds it). Fire-and-forget:
    /// rejections come back as envelopes and are logged/recorded by the pump.
    pub fn dispatch(&mut self, channel: impl Into<String>, action: StateAction) {
        let channel = channel.into();
        // A write issued while the pre-connect replay is running queues behind
        // it: racing the landing createSession/subscribe silently loses the
        // first message (the host answers not-found for a session still
        // being created).
        if self.client.is_none() || self.replay_pending {
            if self.queue_head_at.is_none() {
                self.queue_head_at = Some(std::time::Instant::now());
            }
            self.pending_writes
                .push(PendingWrite::Dispatch(channel, Box::new(action)));
            return;
        }
        let client = self.client.clone().expect("guard above");
        // Each outcome rides the delivery channel to the store's own pump —
        // this spawn has no gpui handle: a transport failure latches the
        // composer's error face (not only a tracing line), and a dispatch
        // that leaves retires a latched Dispatch face.
        let fail_tx = self.delivery_signal_tx.clone();
        manox_agent::runtime::handle().spawn(async move {
            match client.dispatch(channel, action).await {
                Ok(_) => {
                    if let Some(fail_tx) = fail_tx {
                        let _ = fail_tx.send_blocking(DeliverySignal::DispatchRecovered);
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "dispatch failed");
                    if let Some(fail_tx) = fail_tx {
                        let _ = fail_tx.send_blocking(DeliverySignal::Failed(
                            DeliveryFailure::Dispatch(err.to_string()),
                        ));
                    }
                }
            }
        });
    }

    /// Issue one command and await the raw result value.
    fn call(&self, method: &'static str, params: impl Serialize + Send + 'static) -> Reply {
        let (tx, rx) = async_channel::bounded(1);
        if let Some(client) = self.client.clone() {
            manox_agent::runtime::handle().spawn(async move {
                let result = client
                    .request::<_, Value>(method, params)
                    .await
                    .map_err(|err| err.to_string());
                let _ = tx.send(result).await;
            });
        } else {
            drop(tx.send(Err("not connected".into())));
        }
        rx
    }

    /// Seed/refresh the sidebar catalogue from `listSessions`.
    pub fn refresh_sessions(&mut self, cx: &mut gpui::Context<Self>) {
        let reply = self.call(
            "listSessions",
            ListSessionsParams {
                channel: ROOT_RESOURCE_URI.to_string(),
                meta: None,
                cursor: None,
                limit: None,
            },
        );
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            if let Ok(Ok(value)) = reply.recv().await {
                let Ok(result) = serde_json::from_value::<ListSessionsResult>(value) else {
                    return;
                };
                let _ = this.update(cx, |store, cx| {
                    if store.book.seed_summaries(result.items) {
                        cx.notify();
                    }
                    store.subscribe_thread_channels_for_summaries(cx);
                });
            }
        })
        .detach();
    }

    /// Subscribe the thread channel of every catalogue row. The sidebar's
    /// pin badge reads `x-manox-thread` state (the thread-scoped baseline
    /// rides the subscription), and the pin dispatch targets the same
    /// channel — both only work for rows whose channel is subscribed, so a
    /// catalogue seed subscribes one channel per row, deduplicated per run.
    pub fn subscribe_thread_channels_for_summaries(&mut self, cx: &mut gpui::Context<Self>) {
        let ids: Vec<String> = self.book.summaries.keys().cloned().collect();
        for sid in ids {
            if self.thread_sub_due(&sid) {
                self.subscribe(thread_uri(&sid), cx);
            }
        }
    }

    /// Whether this row's thread subscription is still due (first sight this
    /// run); marks it issued. Split out for the cx-free test face.
    fn thread_sub_due(&mut self, session_id: &str) -> bool {
        self.thread_subs.insert(session_id.to_string())
    }

    /// Create a session over a client-minted id (the idempotency key).
    /// Returns the raw result; callers subscribe the session channel next.
    /// Before the handshake completes the request is queued and the reply
    /// resolves once the replay reaches it.
    pub fn create_session(
        &mut self,
        session_id: &str,
        cwds: Vec<String>,
        config: Option<serde_json::Map<String, Value>>,
    ) -> Reply {
        let (tx, rx) = async_channel::bounded::<Result<Value, String>>(1);
        if let Some(client) = self.client.clone() {
            // Connected: issue directly (the replay loop only runs during
            // the handshake window).
            let channel = session_uri(session_id);
            manox_agent::runtime::handle().spawn(async move {
                let result = tokio_wait(move || async move {
                    client
                        .request::<_, Value>(
                            "createSession",
                            CreateSessionParams {
                                channel,
                                meta: None,
                                provider: None,
                                working_directories: Some(cwds),
                                config,
                                active_client: None,
                                progress_token: None,
                            },
                        )
                        .await
                        .map_err(|err| err.to_string())
                })
                .await;
                let _ = tx.send(result).await;
            });
        } else {
            // Pre-connect: queue behind the handshake; the replay resolves
            // the reply once the host answered.
            self.pending_writes.push(PendingWrite::CreateSession {
                session_id: session_id.to_string(),
                cwds,
                config,
                reply: tx,
            });
        }
        rx
    }

    /// Page older turns into the chat state; the turns arrive as a
    /// `chat/turnsLoaded` echo folded by the pump.
    pub fn fetch_turns(&self, chat_id: &str, cursor: Option<String>) -> Reply {
        self.call(
            "fetchTurns",
            FetchTurnsParams {
                channel: chat_uri(chat_id),
                meta: None,
                cursor,
            },
        )
    }

    /// Optimistic config merge: the chip reflects the pick immediately; the
    /// host's configChanged echo (or the next snapshot) is the confirmation.
    /// Mirrors the v2 permission-chip's optimistic mirror semantics.
    pub fn optimistic_config(&mut self, session_id: &str, config: &serde_json::Map<String, Value>) {
        use ahp_types::state::SessionConfigState;
        let state = self
            .book
            .sessions
            .entry(session_id.to_string())
            .or_insert_with(|| empty_session(&session_uri(session_id)));
        let seat = state.config.get_or_insert_with(|| SessionConfigState {
            schema: ahp_types::state::SessionConfigSchema {
                r#type: "object".to_string(),
                properties: Default::default(),
                required: None,
            },
            values: Default::default(),
        });
        for (k, v) in config {
            // Remember the (old, new) pair around the optimistic write: a
            // host refusal matches the refused write BY VALUE and undoes
            // exactly it (see `rollback_refused_config`), and an accepted
            // echo spends exactly its own entry (see `confirm_config`).
            self.optimistic_undo.push((
                session_id.to_string(),
                k.clone(),
                seat.values.get(k).cloned(),
                Some(v.clone()),
            ));
            seat.values.insert(k.clone(), v.clone());
        }
    }

    /// Roll back a refused write BY VALUE. The undo log carries one
    /// `(old, new)` entry per in-flight write; a refusal envelope names the
    /// exact action it refuses, so the match is on the written value:
    ///
    /// - the refused entry leaves the log;
    /// - if it was the LATEST in-flight write for its key, the fold
    ///   restores the value the session had before it (removing the key
    ///   when there was none — the host does not re-broadcast authority
    ///   for a write it refused);
    /// - if a LATER write is still in flight, the fold keeps showing that
    ///   write, and the later entry's remembered base becomes the refused
    ///   entry's base — the refused value can never resurface as a
    ///   rollback target.
    ///
    /// (Single-slot rollback — restore the latest entry regardless of
    /// which write was refused — stranding the fold on a REFUSED value
    /// whenever two writes to one key were in flight; this is the
    /// reconciliation guide's per-action pending list, collapsed to
    /// per-key value chains because this client is the only writer of
    /// these keys.)
    fn rollback_refused_config(
        &mut self,
        session_id: &str,
        config: &serde_json::Map<String, Value>,
    ) {
        for (key, refused) in config {
            let Some(position) = self.optimistic_undo.iter().rposition(|(sid, k, _, new)| {
                sid == session_id && k == key && new.as_ref() == Some(refused)
            }) else {
                continue;
            };
            let (_, _, base, _) = self.optimistic_undo.remove(position);
            // EVERY write still in flight for this key was written above the
            // refused value, so the refused entry's base is the rollback
            // target for all of them — a later refusal of any one restores
            // to the last confirmed value, never to a refused one. Rewriting
            // only the newest leaves the ones in between pointing at the
            // refused value.
            let mut later = false;
            for entry in self
                .optimistic_undo
                .iter_mut()
                .filter(|(sid, k, _, _)| sid == session_id && k == key)
            {
                entry.2 = base.clone();
                later = true;
            }
            if !later
                && let Some(state) = self.book.sessions.get_mut(session_id)
                && let Some(seat) = &mut state.config
            {
                match base {
                    Some(v) => {
                        seat.values.insert(key.clone(), v);
                    }
                    None => {
                        seat.values.remove(key);
                    }
                }
            }
        }
    }

    /// Roll back the LATEST in-flight write for each named key — the
    /// value-blind fallback for actions whose payload does not echo the
    /// written value back comparable (the cwd write's directory URI).
    fn rollback_latest(&mut self, session_id: &str, keys: &[String]) {
        for key in keys {
            if let Some(position) = self
                .optimistic_undo
                .iter()
                .rposition(|(sid, k, _, _)| sid == session_id && k == key)
            {
                let (_, _, base, _) = self.optimistic_undo.remove(position);
                if let Some(state) = self.book.sessions.get_mut(session_id)
                    && let Some(seat) = &mut state.config
                {
                    match base {
                        Some(v) => {
                            seat.values.insert(key.clone(), v);
                        }
                        None => {
                            seat.values.remove(key);
                        }
                    }
                }
            }
        }
    }

    /// Spend a confirmed write's undo entry (matched by value) and every
    /// earlier entry it superseded: once the host echoed this exact write,
    /// its pre-write value is no longer a truth to restore — a later
    /// refusal of another in-flight write to the same key restores to
    /// THIS value, not past it.
    fn confirm_config(&mut self, session_id: &str, config: &serde_json::Map<String, Value>) {
        for (key, confirmed) in config {
            let Some(position) = self.optimistic_undo.iter().rposition(|(sid, k, _, new)| {
                sid == session_id && k == key && new.as_ref() == Some(confirmed)
            }) else {
                continue;
            };
            let keep: Vec<(String, String, Option<Value>, Option<Value>)> = self
                .optimistic_undo
                .drain(..)
                .enumerate()
                .filter(|(index, (sid, k, _, _))| {
                    !(sid == session_id && k == key && *index <= position)
                })
                .map(|(_, entry)| entry)
                .collect();
            self.optimistic_undo = keep;
        }
    }

    /// Spend the latest entry per named key — the acceptance half for the
    /// value-blind actions (see `rollback_latest`).
    fn confirm_latest(&mut self, session_id: &str, keys: &[String]) {
        for key in keys {
            if let Some(position) = self
                .optimistic_undo
                .iter()
                .rposition(|(sid, k, _, _)| sid == session_id && k == key)
            {
                self.optimistic_undo.remove(position);
            }
        }
    }

    /// Fold a server snapshot and reset that session's write-ahead
    /// reconciliation: a snapshot is host truth (the reconnect contract
    /// clears pending state), so the optimistic undo entries it covers are
    /// spent — restoring against server truth would resurrect values the
    /// host already replaced.
    fn apply_snapshot_confirmed(&mut self, uri: &str, state: SnapshotState) -> bool {
        if matches!(&state, SnapshotState::Session(_)) {
            self.optimistic_undo.retain(|(sid, ..)| sid != id_of(uri));
        }
        self.book.apply_snapshot(uri, state)
    }

    /// Drain the display events of one session — the bucket the caller
    /// holds a binding for (the foreground pair's id half). The LIVE leg
    /// applies them; a REBUILD caller must call this too and DROP the
    /// result: the conversation it just synthesized from the book already
    /// contains everything the bucket holds, so leaving the bucket in place
    /// would let the next notify replay it onto the rebuilt list (every
    /// parked delta rendered twice on a switch-back). Buckets of other
    /// sessions stay theirs until their own bind drains them.
    pub fn drain_chat_events(&mut self, session_id: &str) -> Vec<crate::chat_fold::ChatEvent> {
        self.chat_events
            .remove(session_id)
            .map(Vec::from)
            .unwrap_or_default()
    }

    /// Recent dispatch rejections, oldest first.
    pub fn rejections(&self) -> impl Iterator<Item = &str> {
        self.rejections.iter().map(String::as_str)
    }

    /// How many subscriptions this store has queued because it has no client
    /// yet (a detached test store queues every write). The sub-agent page's
    /// refresh poll is a re-subscribe loop, so counting queued subscribes is
    /// how a test proves the poll fires (and stops). Test-support diagnostic.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_queued_subscribes(&self) -> usize {
        self.pending_writes
            .iter()
            .filter(|write| matches!(write, PendingWrite::Subscribe(_)))
            .count()
    }

    /// Re-apply the folded chat state as its own subscribe snapshot — the
    /// host's snapshot ≈ the replayed journal — marking the snapshot landed
    /// for the rebuild-heal watermark. Test-support diagnostic.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_apply_folded_chat_snapshot(&mut self, session_id: &str) -> bool {
        let Some(chat) = self.book.chats.get(session_id).cloned() else {
            return false;
        };
        self.book
            .apply_snapshot(&chat_uri(session_id), SnapshotState::Chat(Box::new(chat)))
    }

    // ── typed write surface ─────────────────────────────────────────
    // Each helper is a `dispatch` of one declared action; the echo folds
    // through the same reducer the host ran, so no helper mutates state.

    /// Start a turn (the submit path).
    pub fn submit_turn(
        &mut self,
        chat_id: &str,
        turn_id: &str,
        text: String,
        queued_message_id: Option<String>,
    ) {
        let action = StateAction::ChatTurnStarted(ChatTurnStartedAction {
            turn_id: turn_id.to_string(),
            started_at: now_iso(),
            message: user_message(text),
            queued_message_id,
            meta: None,
        });
        self.dispatch(chat_uri(chat_id), action);
    }

    /// Park a steering or queued follow-up.
    pub fn set_pending_message(
        &mut self,
        chat_id: &str,
        id: &str,
        kind: PendingMessageKind,
        text: String,
    ) {
        let action = StateAction::ChatPendingMessageSet(ChatPendingMessageSetAction {
            kind,
            id: id.to_string(),
            message: user_message(text),
        });
        self.dispatch(chat_uri(chat_id), action);
    }

    /// Retire a steering or queued follow-up.
    pub fn remove_pending_message(&mut self, chat_id: &str, id: &str, kind: PendingMessageKind) {
        let action = StateAction::ChatPendingMessageRemoved(ChatPendingMessageRemovedAction {
            kind,
            id: id.to_string(),
        });
        self.dispatch(chat_uri(chat_id), action);
    }

    /// Cancel the running turn.
    pub fn cancel_turn(&mut self, chat_id: &str, turn_id: &str) {
        let action = StateAction::ChatTurnCancelled(ChatTurnCancelledAction {
            turn_id: turn_id.to_string(),
            duration: 0,
            meta: None,
        });
        self.dispatch(chat_uri(chat_id), action);
    }

    /// Settle a tool-call confirmation (the approval gate).
    ///
    /// The host keys the settle on the confirmation's auth id, and only
    /// reads it from the action's `_meta` stamp: a confirmation without
    /// `meta["x-manox"]["authId"]` is treated as a client-minted tool call
    /// and silently ignored, so the verdict must always carry it. The fold's
    /// `SessionToolConfirmationRequest.id` IS that auth id (the translator
    /// seeds the request under it), which is what callers pass here.
    pub fn confirm_tool_call(
        &mut self,
        chat_id: &str,
        turn_id: &str,
        tool_call_id: &str,
        auth_id: &str,
        selected_option_id: Option<String>,
        approved: bool,
    ) {
        // The `x-manox` seat with the translator's auth stamp. The field
        // names mirror the host's `approval_meta` (which exposes no
        // constants): only `authId` is read back today.
        let mut auth = serde_json::Map::new();
        auth.insert("authId".to_string(), Value::String(auth_id.to_string()));
        let mut meta = serde_json::Map::new();
        meta.insert(ext::META_KEY.to_string(), Value::Object(auth));
        let action = StateAction::ChatToolCallConfirmed(ChatToolCallConfirmedAction {
            turn_id: turn_id.to_string(),
            tool_call_id: tool_call_id.to_string(),
            meta: Some(meta),
            selected_option_id,
            approved,
            confirmed: None,
            reason: None,
            edited_tool_input: None,
            user_suggestion: None,
            reason_message: None,
        });
        self.dispatch(chat_uri(chat_id), action);
    }

    /// Submit an elicitation answer.
    pub fn complete_input(
        &mut self,
        chat_id: &str,
        request_id: &str,
        answers: std::collections::HashMap<String, ChatInputAnswer>,
    ) {
        tracing::info!(
            request_id = %request_id,
            answers = answers.len(),
            "chat input: submitting answers"
        );
        let action = StateAction::ChatInputCompleted(ChatInputCompletedAction {
            request_id: request_id.to_string(),
            response: ChatInputResponseKind::Accept,
            answers: Some(answers),
        });
        self.dispatch(chat_uri(chat_id), action);
    }

    /// Decline an elicitation without answering (the ask card's close: the
    /// user left to speak, never a rejection — the engine journals the
    /// question's `dismissed` verdict).
    pub fn decline_input(&mut self, chat_id: &str, request_id: &str) {
        tracing::info!(request_id = %request_id, "chat input: declining (dismissed)");
        let action = StateAction::ChatInputCompleted(ChatInputCompletedAction {
            request_id: request_id.to_string(),
            response: ChatInputResponseKind::Decline,
            answers: None,
        });
        self.dispatch(chat_uri(chat_id), action);
    }

    /// Merge session config keys (model / reasoningEffort / approvalMode).
    pub fn set_config(&mut self, session_id: &str, config: serde_json::Map<String, Value>) {
        let action = StateAction::SessionConfigChanged(SessionConfigChangedAction {
            config,
            replace: None,
        });
        self.dispatch(session_uri(session_id), action);
    }

    /// Rename the session.
    pub fn set_title(&mut self, session_id: &str, title: String) {
        let action = StateAction::SessionTitleChanged(SessionTitleChangedAction { title });
        self.dispatch(session_uri(session_id), action);
    }

    /// Replace the session's (single) working directory. The chip echoes
    /// optimistically — the host's `configChanged` confirmation only lands
    /// after the engine journals the change (the model picker's trade-off).
    pub fn set_cwd(&mut self, session_id: &str, path: &str) {
        let mut config = serde_json::Map::new();
        config.insert(
            config_keys::WORKING_DIRECTORY.to_string(),
            Value::String(path.to_string()),
        );
        self.optimistic_config(session_id, &config);
        let action = StateAction::SessionWorkingDirectorySet(SessionWorkingDirectorySetAction {
            directory: format!("file://{path}"),
        });
        self.dispatch(session_uri(session_id), action);
    }

    /// Archive or unarchive the session.
    pub fn set_archived(&mut self, session_id: &str, archived: bool) {
        let action = StateAction::SessionIsArchivedChanged(SessionIsArchivedChangedAction {
            is_archived: archived,
        });
        self.dispatch(session_uri(session_id), action);
    }

    /// Claim this client's active-client role for the session (contributing
    /// no tools; the desktop's tools ride the kernel, not the protocol).
    pub fn claim_active_client(&mut self, session_id: &str) {
        let action = StateAction::SessionActiveClientSet(SessionActiveClientSetAction {
            active_client: ahp_types::state::SessionActiveClient {
                client_id: CLIENT_ID.to_string(),
                display_name: None,
                tools: Vec::new(),
                customizations: None,
            },
        });
        self.dispatch(session_uri(session_id), action);
    }

    /// Compact the session's history through the `x-manox/compact` command.
    pub fn send_compact(&self, session_id: &str, instructions: Option<String>) -> Reply {
        self.call(
            "x-manox/compact",
            serde_json::json!({
                "channel": session_uri(session_id),
                "instructions": instructions,
            }),
        )
    }

    /// Apply one goal lifecycle action through the `x-manox/goal` command.
    pub fn send_goal(
        &self,
        session_id: &str,
        action: &str,
        objective: Option<String>,
        budget: Option<u64>,
        max_rounds: Option<u64>,
    ) -> Reply {
        self.call(
            "x-manox/goal",
            serde_json::json!({
                "channel": session_uri(session_id),
                "action": action,
                "objective": objective,
                "budget": budget,
                "maxRounds": max_rounds,
            }),
        )
    }

    /// Fork the session's chat at a completed turn. The new chat's URI is
    /// client-minted (the idempotency key), returned alongside the reply.
    pub fn fork_chat(&self, session_id: &str, turn_id: &str) -> (Reply, String) {
        let chat_id = uuid::Uuid::new_v4().to_string();
        let reply = self.call(
            "createChat",
            CreateChatParams {
                channel: ROOT_RESOURCE_URI.to_string(),
                meta: None,
                chat: chat_uri(&chat_id),
                initial_message: None,
                source: Some(ChatSource::Fork(ForkChatSource {
                    chat: session_uri(session_id),
                    turn_id: turn_id.to_string(),
                })),
                working_directories: None,
            },
        );
        (reply, chat_id)
    }
}

/// The current instant in the ISO-8601 shape the protocol carries.
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Block until a command reply lands. The callers live on the gpui thread,
/// so the wait rides the manox runtime handle (never the GPUI executor).
pub fn await_reply(reply: Reply) {
    manox_agent::runtime::handle()
        .block_on(async move { reply.recv().await })
        .ok();
}

/// A user-authored message (the submit/steer/queue payload).
fn user_message(text: String) -> ahp_types::state::Message {
    serde_json::from_value(serde_json::json!({
        "text": text,
        "origin": { "kind": "user" },
    }))
    .expect("a user message parses")
}

/// Run an async closure on the manox tokio runtime and await its result from
/// a GPUI future.
async fn tokio_wait<T, F>(f: impl FnOnce() -> F + Send + 'static) -> Result<T, String>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, String>> + Send,
{
    let (tx, rx) = async_channel::bounded::<Result<T, String>>(1);
    manox_agent::runtime::handle().spawn(async move {
        let _ = tx.send(f().await).await;
    });
    rx.recv()
        .await
        .map_err(|_| "tokio runtime is gone".to_string())?
}

/// Answer one host → client request through the bridge channel.
async fn answer_via_bridge(
    tx: async_channel::Sender<CapabilityRequest>,
    method: String,
    params: Value,
) -> Result<Value, ahp_types::messages::JsonRpcError> {
    let (reply_tx, reply_rx) = async_channel::bounded::<Result<Value, String>>(1);
    let request = CapabilityRequest {
        method,
        params,
        reply: reply_tx,
    };
    if tx.send(request).await.is_err() {
        return Err(ahp_types::messages::JsonRpcError {
            code: -32000,
            message: "client is shutting down".into(),
            data: None,
        });
    }
    match reply_rx.recv().await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(ahp_types::messages::JsonRpcError {
            code: -32000,
            message,
            data: None,
        }),
        Err(_) => Err(ahp_types::messages::JsonRpcError {
            code: -32000,
            message: "capability handler dropped".into(),
            data: None,
        }),
    }
}

/// A per-session view over the book: the fields views used to read off the
/// retired `ClientStore` mirror, derived from AHP channel state on demand.
/// No state of its own beyond the session id — the book is the only home.
pub struct LeafView<'a> {
    pub session_id: &'a str,
    pub session: Option<&'a SessionState>,
    pub chat: Option<&'a ChatState>,
    pub ext: Option<&'a XManoxState>,
    pub metrics: Option<&'a ConversationMetrics>,
}

/// Derive a [`LeafView`] for `session_id` (`chat_id` when the default chat
/// pointer has not landed yet, `chat_id` falls back to the session id).
pub fn leaf<'a>(book: &'a ChannelBook, session_id: &'a str) -> LeafView<'a> {
    let chat_id = book
        .default_chat(session_id)
        .map(|uri| id_of(&uri).to_string())
        .unwrap_or_else(|| session_id.to_string());
    LeafView {
        session_id,
        session: book.sessions.get(session_id),
        chat: book.chats.get(&chat_id),
        // The thread rows (pinned / label / session info / leaf) ride the
        // `x-manox-thread` extension channel since upstream #842 — the
        // session channel's ext face carries nothing anymore.
        ext: book.ext.get(&thread_uri(session_id)),
        // The fold keys the metrics map by the channel the delta rode
        // (`x-manox-metrics:/<chat-id>`), not by the chat channel — reading
        // the chat URI here would miss every entry the fold ever wrote.
        metrics: book
            .metrics
            .get(&format!("{}{chat_id}", manox_ahp::ext::channels::METRICS)),
    }
}

/// The plan-review payload (`{requestId, title, content, planFile}`) from the
/// session's plan channel, when it belongs to `request_id` — the plan content
/// the review card renders beneath the verdict question.
/// The plan channel URI a session's plan rows ride: the default chat's plan
/// channel once the pointer has landed, else the session-id form (the
/// pre-pointer subscription). The single key authority for the subscription
/// face and every read face — they must never fork.
pub fn plan_channel_for(book: &ChannelBook, session_id: &str) -> String {
    let chat_id = book
        .default_chat(session_id)
        .map(|uri| id_of(&uri).to_string())
        .unwrap_or_else(|| session_id.to_string());
    format!("{}{chat_id}", manox_ahp::ext::channels::PLAN)
}

pub fn plan_review_of<'a>(
    book: &'a ChannelBook,
    session_id: &str,
    request_id: &str,
) -> Option<&'a Value> {
    let channel = plan_channel_for(book, session_id);
    book.ext
        .get(&channel)?
        .plan_review
        .as_ref()
        .filter(|payload| payload.get("requestId").and_then(Value::as_str) == Some(request_id))
}

/// Whether the session's plan mode is engaged — the composer plan chip's read
/// face. Plan rows ride the `x-manox-plan` channel keyed by the default chat
/// id (the session id until the pointer lands); the thread-channel ext state
/// carries nothing for them, so a `LeafView.ext` read is always `None`.
pub fn plan_mode_of(book: &ChannelBook, session_id: &str) -> bool {
    book.ext
        .get(&plan_channel_for(book, session_id))
        .and_then(|x| x.plan_mode)
        .unwrap_or(false)
}

/// The session's current plan document (the kernel snapshot shape the plan
/// restore rehydrates), from the `x-manox-plan` channel.
pub fn plan_snapshot_of<'a>(book: &'a ChannelBook, session_id: &str) -> Option<&'a Value> {
    book.ext
        .get(&plan_channel_for(book, session_id))?
        .plan
        .as_ref()
}

/// The session's active browser suites, from the `x-manox-work` channel
/// (keyed by the session id — work rows have no chat scope).
pub fn browser_suites_of(book: &ChannelBook, session_id: &str) -> Vec<String> {
    book.ext
        .get(&work_uri(session_id))
        .and_then(|x| x.browser_suites.clone())
        .unwrap_or_default()
}

/// The session goal (verbatim payload), from the `x-manox-work` channel —
/// work rows never ride the thread channel, so a `LeafView.ext` read is
/// always `None` for them.
pub fn goal_of<'a>(book: &'a ChannelBook, session_id: &str) -> Option<&'a Value> {
    book.ext
        .get(&work_uri(session_id))
        .and_then(|x| x.goal.as_ref())
}

/// Whether the session's plan channel holds an open (proposed) review — the
/// sidebar row badge's read. Resolves the chat id the same way
/// [`plan_review_of`] does, falling back to the session id when the pointer
/// has not landed.
pub fn plan_review_proposed(book: &ChannelBook, session_id: &str) -> bool {
    book.ext
        .get(&plan_channel_for(book, session_id))
        .and_then(|x| x.plan_review.as_ref())
        .and_then(|r| r.get("state"))
        .and_then(Value::as_str)
        == Some("proposed")
}

/// The call id of a confirmation-state tool call (pending / pending-result).
fn confirmation_tool_call_id(call: &ahp_types::state::ToolCallConfirmationState) -> &str {
    match call {
        ahp_types::state::ToolCallConfirmationState::PendingConfirmation(c) => &c.tool_call_id,
        ahp_types::state::ToolCallConfirmationState::PendingResultConfirmation(c) => {
            &c.tool_call_id
        }
        ahp_types::state::ToolCallConfirmationState::Unknown(_) => "",
    }
}

impl<'a> LeafView<'a> {
    /// The session's display title (AHP keeps it on both channel states).
    pub fn display_title(&self) -> Option<&str> {
        self.session.map(|s| s.title.as_str())
    }

    /// Whether a turn is in flight (the chat carries an active turn).
    pub fn running(&self) -> bool {
        self.chat.is_some_and(|c| c.active_turn.is_some())
    }

    /// The chat's running sub-agents, in host order — one row per
    /// `backgroundWork` subagent entry. The host removes an entry when its run
    /// settles, so this is exactly the live set (the Captain status block
    /// lists them beside the Captain's own line).
    pub fn running_subagents(&self) -> Vec<SubagentWorkRow> {
        self.chat
            .and_then(|c| c.background_work.as_ref())
            .map(|work| {
                work.iter()
                    .filter_map(|entry| match entry {
                        ahp_types::state::BackgroundWork::Subagent(sub) => Some(SubagentWorkRow {
                            id: sub.id.clone(),
                            label: sub.label.clone(),
                            activity: sub
                                .meta
                                .as_ref()
                                .and_then(|meta| meta.get("latestActivity"))
                                .and_then(Value::as_str)
                                .filter(|activity| !activity.is_empty())
                                .map(str::to_string),
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The canonical `provider/model` selection string.
    pub fn model_id(&self) -> Option<&str> {
        self.session
            .and_then(|s| s.config.as_ref())
            .and_then(|c| c.values.get("model"))
            .and_then(Value::as_str)
    }

    /// The selected reasoning effort (canonical string form).
    pub fn reasoning_effort(&self) -> Option<&str> {
        self.session
            .and_then(|s| s.config.as_ref())
            .and_then(|c| c.values.get("reasoningEffort"))
            .and_then(Value::as_str)
    }

    /// The selected approval mode (canonical string form).
    pub fn approval_mode(&self) -> Option<&str> {
        self.session
            .and_then(|s| s.config.as_ref())
            .and_then(|c| c.values.get("approvalMode"))
            .and_then(Value::as_str)
    }

    /// The session's effective approval mode for display and turn
    /// stamping: the folded value, else the host-declared schema default,
    /// else the host's own default (`WorkspaceWrite`). A seat-less session
    /// runs under the host default — the retired local `ReadOnly` fallback
    /// contradicted it, so every reopen of a thread that never journaled a
    /// mode displayed a restriction the runtime did not enforce.
    pub fn approval_mode_or_default(&self) -> manox_agent::thread::PermissionMode {
        let wire = self
            .approval_mode()
            .map(str::to_string)
            .or_else(|| {
                self.session
                    .and_then(|s| s.config.as_ref())
                    .and_then(|c| c.schema.properties.get(config_keys::APPROVAL_MODE))
                    .and_then(|prop| prop.default.as_ref())
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| {
                manox_agent::thread::PermissionMode::default()
                    .wire()
                    .to_string()
            });
        manox_agent::thread::PermissionMode::from_wire(&wire).unwrap_or_default()
    }

    /// The most recent request's token usage: the in-flight turn's report
    /// while streaming, else the last completed turn's (the `chat/usage`
    /// action rides the channel, so this replays on attach). Cache-write
    /// tokens are not modeled on the AHP turn usage. `None` before the first
    /// usage report lands.
    pub fn last_usage(&self) -> Option<ahp_types::state::UsageInfo> {
        let chat = self.chat?;
        chat.active_turn
            .as_ref()
            .and_then(|t| t.usage.clone())
            .or_else(|| chat.turns.last().and_then(|t| t.usage.clone()))
    }

    /// AHP 1.0's change footprint (`ChatState.changes`): the session's
    /// uncommitted footprint as the changeset engine's last scan reported it
    /// (dspo/manox#880's host half) — file count and line totals for the
    /// rail's changes section. `None` when the host carried no scan (a
    /// directory set with no repository).
    pub fn change_footprint(&self) -> Option<&ahp_types::state::ChangesSummary> {
        self.chat?.changes.as_ref()
    }

    /// The effective working directory: the config value the host publishes
    /// on every cwd change. AHP's working-directories set is a grant ledger
    /// in grant order — its first entry is the session's creation directory,
    /// not the current one — so only the config read is the echo; the newest
    /// grant is the fallback for folds without the config seat yet.
    pub fn cwd(&self) -> Option<String> {
        if let Some(configured) = self
            .session
            .and_then(|s| s.config.as_ref())
            .and_then(|c| c.values.get(config_keys::WORKING_DIRECTORY))
            .and_then(Value::as_str)
        {
            return Some(configured.to_string());
        }
        self.session
            .and_then(|s| s.working_directories.as_ref())
            .and_then(|dirs| dirs.last())
            .map(|uri| uri.trim_start_matches("file://").to_string())
    }

    /// Whether the session is archived (`SessionStatus::IsArchived` bit).
    pub fn archived(&self) -> bool {
        self.session
            .is_some_and(|s| s.status & ahp_types::state::SessionStatus::IsArchived.bits() != 0)
    }

    /// Whether the session has an open input request (approval or question).
    pub fn input_needed(&self) -> bool {
        self.session
            .is_some_and(|s| s.input_needed.as_ref().is_some_and(|v| !v.is_empty()))
    }

    /// The open input-request list, when any.
    pub fn requests(&self) -> &[ahp_types::state::SessionInputRequest] {
        self.session
            .and_then(|s| s.input_needed.as_deref())
            .unwrap_or(&[])
    }

    /// The tool confirmation whose request id is `id`:
    /// `(chat id, turn id, tool call id)`.
    pub fn confirmation(&self, id: &str) -> Option<(String, String, String)> {
        self.requests().iter().find_map(|r| match r {
            ahp_types::state::SessionInputRequest::ToolConfirmation(c) if c.id == id => Some((
                crate::ahp_store::id_of(&c.chat).to_string(),
                c.turn_id.clone(),
                confirmation_tool_call_id(&c.tool_call).to_string(),
            )),
            _ => None,
        })
    }

    /// The chat-input (elicitation) request whose id is `id`:
    /// `(chat id, request)`.
    pub fn chat_input(&self, id: &str) -> Option<(String, &ahp_types::state::ChatInputRequest)> {
        self.fold_input_request(Some(id))
    }

    /// The first open chat-input (elicitation) request, if any — the live
    /// ask edge's source of truth.
    pub fn open_chat_input(&self) -> Option<(String, &ahp_types::state::ChatInputRequest)> {
        self.fold_input_request(None)
    }

    /// The first open tool-confirmation request, if any — the generic
    /// authorization card's source of truth (Edit/Write sandbox
    /// escalations park the model here).
    pub fn open_tool_confirmation(
        &self,
    ) -> Option<(String, &ahp_types::state::SessionToolConfirmationRequest)> {
        self.requests().iter().find_map(|r| match r {
            ahp_types::state::SessionInputRequest::ToolConfirmation(c) => {
                Some((crate::ahp_store::id_of(&c.chat).to_string(), c))
            }
            _ => None,
        })
    }

    /// Scan the fold for an unanswered chat-input request. The host folds
    /// `chat/inputRequested` into the active turn's response parts — the
    /// session channel's input-needed list is NOT maintained on this path —
    /// so the fold is the only home for both surfacing and answering an ask.
    ///
    /// The ACTIVE turn only, deliberately: the runtime parks an ask while its
    /// turn is running, and a turn that ends with the card unanswered has had
    /// the question discarded host-side (the gate journals the `cancelled`
    /// decision). An unanswered part inside an ARCHIVED turn is therefore
    /// unanswerable by construction — the ahp reducer settles input requests
    /// on the active turn alone — and scanning archived turns only re-surfaced
    /// such corpses, locking the composer in ask mode forever (the live
    /// cancel raced the decision row in 2026-10's strand).
    fn fold_input_request(
        &self,
        id: Option<&str>,
    ) -> Option<(String, &ahp_types::state::ChatInputRequest)> {
        fn scan<'a>(
            parts: &'a [ahp_types::state::ResponsePart],
            id: Option<&str>,
            latest: &mut Option<&'a ahp_types::state::InputRequestResponsePart>,
        ) {
            for part in parts {
                if let ahp_types::state::ResponsePart::InputRequest(input) = part {
                    if input.response.is_some() {
                        continue;
                    }
                    if let Some(want) = id
                        && input.request.id != want
                    {
                        continue;
                    }
                    // Journal order: the latest unanswered request wins.
                    *latest = Some(input);
                }
            }
        }
        let chat = self.chat?;
        let mut latest: Option<&ahp_types::state::InputRequestResponsePart> = None;
        if let Some(active) = &chat.active_turn {
            scan(&active.response_parts, id, &mut latest);
        }
        latest.map(|input| (id_of(&chat.resource).to_string(), &input.request))
    }
}

/// Lower an AHP chat-input request into the pending ask the interactive
/// card renders. Mirrors the fold's question translation: a select
/// question carries its options; a text/number question renders as the
/// card's custom-input step. A request with no structured questions (an
/// old journal's bare ask) yields `None` — there is nothing to answer
/// with, and the generic authorization card is the wrong surface.
pub fn pending_ask_from_ahp(
    id: String,
    request: &ahp_types::state::ChatInputRequest,
) -> Option<crate::column::PendingAsk> {
    use ahp_types::state::ChatInputQuestion as Q;
    let questions = request.questions.as_ref()?;
    if questions.is_empty() {
        return None;
    }
    let mut parsed = Vec::with_capacity(questions.len());
    for q in questions {
        let (id, question, header, multi, options) = match q {
            Q::SingleSelect(s) => (
                s.id.clone(),
                s.message.clone(),
                s.title.clone().unwrap_or_default(),
                false,
                s.options
                    .iter()
                    .map(|o| crate::column::AskOption {
                        label: o.label.clone(),
                        description: o.description.clone().unwrap_or_default(),
                        recommended: o.recommended.unwrap_or(false),
                    })
                    .collect::<Vec<_>>(),
            ),
            Q::MultiSelect(m) => (
                m.id.clone(),
                m.message.clone(),
                m.title.clone().unwrap_or_default(),
                true,
                m.options
                    .iter()
                    .map(|o| crate::column::AskOption {
                        label: o.label.clone(),
                        description: o.description.clone().unwrap_or_default(),
                        recommended: o.recommended.unwrap_or(false),
                    })
                    .collect::<Vec<_>>(),
            ),
            Q::Text(t) => (
                t.id.clone(),
                t.message.clone(),
                t.title.clone().unwrap_or_default(),
                false,
                Vec::new(),
            ),
            Q::Number(n) => (
                n.id.clone(),
                n.message.clone(),
                n.title.clone().unwrap_or_default(),
                false,
                Vec::new(),
            ),
            Q::Integer(n) => (
                n.id.clone(),
                n.message.clone(),
                n.title.clone().unwrap_or_default(),
                false,
                Vec::new(),
            ),
            Q::Boolean(b) => (
                b.id.clone(),
                b.message.clone(),
                b.title.clone().unwrap_or_default(),
                false,
                Vec::new(),
            ),
            Q::Unknown(_) => continue,
        };
        parsed.push(crate::column::AskQuestion {
            id,
            question,
            header,
            detail: String::new(),
            multi_select: multi,
            options,
        });
    }
    if parsed.is_empty() {
        return None;
    }
    Some(crate::column::PendingAsk {
        id,
        questions: parsed,
    })
}

/// The config keys a write-ahead action touches, for the reconciliation
/// halves: a refusal (`rejectionReason`) rolls them back out of the client
/// fold so a refused pick does not linger on the UI, and our own accepted
/// echo spends their undo history so a later refusal restores to the
/// confirmed value, not past it.
fn touched_config_keys(action: &ahp_types::actions::StateAction) -> Option<Vec<String>> {
    use ahp_types::actions::StateAction as A;
    match action {
        A::SessionConfigChanged(changed) => {
            Some(changed.config.keys().map(|k| k.to_string()).collect())
        }
        // The cwd write is a dedicated action, not a config change.
        A::SessionWorkingDirectorySet(_) => Some(vec![config_keys::WORKING_DIRECTORY.to_string()]),
        _ => None,
    }
}

/// The written values of a config action, when the action echoes them
/// back comparable — the value-matched reconciliation half of
/// `touched_config_keys` (the cwd write's payload is a file URI, not the
/// config-seat string, so it stays on the value-blind path).
fn config_values_of(
    action: &ahp_types::actions::StateAction,
) -> Option<&serde_json::Map<String, Value>> {
    use ahp_types::actions::StateAction as A;
    match action {
        A::SessionConfigChanged(changed) => Some(&changed.config),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ahp_types::actions::{ChatDeltaAction, SessionTitleChangedAction};

    /// The sub-agent block's read face: the host's background-work subagent
    /// entries project into rows (id + agent type + the run's task label),
    /// every other kind (a shell, a future kind) is skipped, and an entry
    /// without the `_meta` activity still answers with its type.
    #[test]
    fn running_subagents_project_the_background_work_subagent_entries() {
        let mut book = ChannelBook::default();
        let chat: ChatState = serde_json::from_value(serde_json::json!({
            "resource": chat_uri("s-1"),
            "title": "parent",
            "status": 0,
            "modifiedAt": "2026-01-01T00:00:00Z",
            "turns": [],
            "backgroundWork": [
                {
                    "kind": "shell",
                    "id": "sh-1",
                    "label": "cargo build",
                    "startedAt": "2026-01-01T00:00:00Z",
                    "command": "cargo build",
                },
                {
                    "kind": "subagent",
                    "id": "sub-ns-3",
                    "label": "Explore",
                    "startedAt": "2026-01-01T00:00:00Z",
                    "_meta": { "agentType": "Explore", "latestActivity": "fix the login redirect" },
                    "chat": "ahp-chat:/sub-ns-3",
                },
                {
                    "kind": "subagent",
                    "id": "sub-ns-4",
                    "label": "Sailor",
                    "startedAt": "2026-01-01T00:00:00Z",
                    "chat": "ahp-chat:/sub-ns-4",
                },
                { "kind": "future-kind", "whatever": true },
            ],
        }))
        .expect("a chat with background work parses");
        book.chats.insert("s-1".to_string(), chat);

        let rows = leaf(&book, "s-1").running_subagents();
        assert_eq!(
            rows,
            vec![
                SubagentWorkRow {
                    id: "sub-ns-3".into(),
                    label: "Explore".into(),
                    activity: Some("fix the login redirect".into()),
                },
                SubagentWorkRow {
                    id: "sub-ns-4".into(),
                    label: "Sailor".into(),
                    activity: None,
                },
            ]
        );
    }

    /// The fold version's contract: counts and the last-entry stamp are what
    /// move it (never the content), so a settled turn rewritten UNDER THE SAME
    /// STAMP is invisible — a version, not a content hash. Production stamps
    /// every changed fold (`modifiedAt` = the journal's last entry), which is
    /// what makes that safe, and the page only needs to know "did anything
    /// land since the build?".
    #[test]
    fn the_fold_version_moves_on_counts_and_the_stamp_only() {
        fn chat(stamp: &str, settled: usize, settled_text: &str, active_parts: usize) -> ChatState {
            let turns: Vec<serde_json::Value> = (0..settled)
                .map(|ix| {
                    serde_json::json!({
                        "id": format!("t{ix}"),
                        "message": { "text": "q", "origin": { "kind": "user" } },
                        "responseParts": [{ "kind": "markdown", "id": format!("t{ix}-p0"), "content": settled_text }],
                        "state": "complete",
                    })
                })
                .collect();
            let mut value = serde_json::json!({
                "resource": chat_uri("s-1"),
                "title": "fold version probe",
                "status": 0,
                "modifiedAt": stamp,
                "turns": turns,
            });
            if active_parts > 0 {
                let parts: Vec<serde_json::Value> = (0..active_parts)
                    .map(|ix| {
                        serde_json::json!({ "kind": "markdown", "id": format!("a{ix}"), "content": "streaming" })
                    })
                    .collect();
                value.as_object_mut().expect("object").insert(
                    "activeTurn".into(),
                    serde_json::json!({
                        "id": "active",
                        "startedAt": stamp,
                        "message": { "text": "q", "origin": { "kind": "user" } },
                        "responseParts": parts,
                    }),
                );
            }
            serde_json::from_value(value).expect("the version probe chat parses")
        }

        let base = chat_fold_version(&chat("2026-01-01T00:00:00Z", 1, "a", 1));
        assert_eq!(
            base,
            chat_fold_version(&chat("2026-01-01T00:00:00Z", 1, "a", 1)),
            "an unchanged fold is the same version"
        );
        assert_ne!(
            base,
            chat_fold_version(&chat("2026-01-01T00:00:00Z", 2, "a", 1)),
            "a settled turn was appended"
        );
        assert_ne!(
            base,
            chat_fold_version(&chat("2026-01-01T00:00:00Z", 1, "a", 2)),
            "the in-flight turn gained a part (in place)"
        );
        assert_ne!(
            base,
            chat_fold_version(&chat("2026-01-01T00:00:01Z", 1, "a", 1)),
            "the fold's stamp advanced"
        );
        assert_eq!(
            base,
            chat_fold_version(&chat("2026-01-01T00:00:00Z", 1, "rewritten", 1)),
            "a settled turn rewritten under the SAME stamp is invisible — a version, not a hash"
        );
    }

    #[test]
    fn typed_session_actions_fold_into_the_session_state() {
        let mut book = ChannelBook::default();
        let effect = book.apply(
            &session_uri("s-1"),
            &StateAction::SessionTitleChanged(SessionTitleChangedAction {
                title: "renamed".into(),
            }),
        );
        assert_eq!(effect, FoldEffect::Changed);
        assert_eq!(book.sessions["s-1"].title, "renamed");
    }

    #[test]
    fn the_configured_working_directory_is_the_echo_not_the_grant_order() {
        let mut book = ChannelBook::default();
        for path in ["/old", "/new"] {
            book.apply(
                &session_uri("s-1"),
                &StateAction::SessionWorkingDirectorySet(SessionWorkingDirectorySetAction {
                    directory: format!("file://{path}"),
                }),
            );
        }
        // The engine journals a revert to the first directory: the grant set
        // stays [/old, /new] (the re-grant dedupes), only the config flips.
        // The host's snapshot seats the config container before any echo can
        // merge (the reducer NoOps into a missing seat).
        let seat = book
            .sessions
            .get_mut("s-1")
            .expect("the grants seeded the session");
        seat.config
            .get_or_insert_with(|| ahp_types::state::SessionConfigState {
                schema: ahp_types::state::SessionConfigSchema {
                    r#type: "object".to_string(),
                    properties: Default::default(),
                    required: None,
                },
                values: Default::default(),
            });
        let mut config = serde_json::Map::new();
        config.insert(
            config_keys::WORKING_DIRECTORY.to_string(),
            Value::String("/old".into()),
        );
        book.apply(
            &session_uri("s-1"),
            &StateAction::SessionConfigChanged(SessionConfigChangedAction {
                config,
                replace: None,
            }),
        );
        assert_eq!(
            leaf(&book, "s-1").cwd().as_deref(),
            Some("/old"),
            "the effective value rides the config, not the grant ledger"
        );
    }

    #[test]
    fn without_a_config_seat_the_newest_grant_is_the_echo() {
        let mut book = ChannelBook::default();
        for path in ["/old", "/new"] {
            book.apply(
                &session_uri("s-1"),
                &StateAction::SessionWorkingDirectorySet(SessionWorkingDirectorySetAction {
                    directory: format!("file://{path}"),
                }),
            );
        }
        assert_eq!(leaf(&book, "s-1").cwd().as_deref(), Some("/new"));
    }

    /// The screenshot session's shape (a Read loop whose fold carries the
    /// tool calls): the LIVE derivation (fold then `from_action`, the
    /// pump's exact order) must yield tool events, and the rebuild
    /// lowering (`synth_display` -> `build_items`) must fold them into the
    /// activity segment as tool entries. This is the pinning test for
    /// "工具调用看不到".
    #[test]
    fn a_tool_loop_surfaces_tools_on_both_paths() {
        use ahp_types::actions::{
            ChatDeltaAction, ChatReasoningAction, ChatResponsePartAction,
            ChatToolCallCompleteAction, ChatToolCallReadyAction, ChatToolCallStartAction,
            ChatTurnCompleteAction, ChatTurnStartedAction,
        };
        use ahp_types::state::{
            MarkdownResponsePart, Message, MessageKind, MessageOrigin, ResponsePart,
            ToolCallResult, ToolInput, ToolResultContent, ToolResultTextContent,
        };

        let mut book = ChannelBook::default();
        let uri = chat_uri("c-1");
        let turn_id = "t-1".to_string();
        let base = Message {
            text: "调用一个 Read tool!".into(),
            origin: MessageOrigin {
                kind: MessageKind::User,
            },
            attachments: None,
            model: None,
            agent: None,
            meta: None,
        };
        let actions = vec![
            StateAction::ChatTurnStarted(ChatTurnStartedAction {
                turn_id: turn_id.clone(),
                started_at: "2026-10-15T07:44:00.000Z".into(),
                message: base,
                queued_message_id: None,
                meta: None,
            }),
            StateAction::ChatReasoning(ChatReasoningAction {
                turn_id: turn_id.clone(),
                part_id: "p-r1".into(),
                content: "The user wants me to call a Read tool.".into(),
                meta: None,
            }),
            StateAction::ChatToolCallStart(ChatToolCallStartAction {
                turn_id: turn_id.clone(),
                tool_call_id: "tc-1".into(),
                meta: None,
                tool_name: "Read".into(),
                display_name: "Read".into(),
                intention: Some("Read AGENTS.md".into()),
                contributor: None,
            }),
            StateAction::ChatToolCallReady(ChatToolCallReadyAction {
                turn_id: turn_id.clone(),
                tool_call_id: "tc-1".into(),
                meta: None,
                contributor: None,
                intention: Some("Read AGENTS.md".into()),
                invocation_message: ahp_types::StringOrMarkdown::Plain("Read AGENTS.md".into()),
                tool_input: Some(ToolInput::Inline(r#"{"path":"AGENTS.md"}"#.into())),
                confirmation_title: None,
                risk_assessment: None,
                edits: None,
                editable: None,
                confirmed: None,
                options: None,
            }),
            StateAction::ChatToolCallComplete(ChatToolCallCompleteAction {
                turn_id: turn_id.clone(),
                tool_call_id: "tc-1".into(),
                meta: None,
                result: ToolCallResult {
                    success: true,
                    past_tense_message: ahp_types::StringOrMarkdown::Plain("Read AGENTS.md".into()),
                    content: Some(vec![ToolResultContent::Text(ToolResultTextContent {
                        text: "# AGENTS.md body".into(),
                    })]),
                    structured_content: None,
                    error: None,
                },
                requires_result_confirmation: None,
            }),
            StateAction::ChatResponsePart(ChatResponsePartAction {
                turn_id: turn_id.clone(),
                part: ResponsePart::Markdown(MarkdownResponsePart {
                    id: "p-m1".into(),
                    content: String::new(),
                }),
                meta: None,
            }),
            StateAction::ChatDelta(ChatDeltaAction {
                turn_id: turn_id.clone(),
                part_id: "p-m1".into(),
                content: "完成。".into(),
                meta: None,
            }),
            StateAction::ChatTurnComplete(ChatTurnCompleteAction {
                turn_id: turn_id.clone(),
                duration: 1_000,
                meta: None,
            }),
        ];

        // The pump's exact order: fold first, then derive from the updated
        // state.
        let mut derived: Vec<crate::chat_fold::ChatEvent> = Vec::new();
        for action in &actions {
            book.apply(&uri, action);
            if let Some(chat) = book.chats.get("c-1")
                && let Some(event) = crate::chat_fold::ChatEvent::from_action(action, Some(chat))
            {
                derived.push(event);
            }
        }
        let tool_events = derived
            .iter()
            .filter(|e| matches!(e, crate::chat_fold::ChatEvent::ToolCall { .. }))
            .count();
        let result_events = derived
            .iter()
            .filter(|e| matches!(e, crate::chat_fold::ChatEvent::ToolResult { .. }))
            .count();
        // Start (streaming) and Ready (running) each derive one advance.
        assert_eq!(
            tool_events, 2,
            "the live derivation yields both tool-call advances"
        );
        assert_eq!(
            result_events, 1,
            "the completion derives a tool-result event"
        );

        // The rebuild lowering over the same folded state.
        let chat = book.chats.get("c-1").expect("the fold holds the chat");
        let mut usage = crate::chat_fold::UsageTable::new();
        let display = crate::chat_fold::synth_display(chat, &mut usage);
        // `rebuild_from_display`'s message half (the display carries no
        // notes for this sequence, so the two are equivalent here).
        let messages: Vec<manox_agent::Message> = display
            .iter()
            .filter_map(|e| match e {
                manox_agent::db::HistoryEntry::Message(m) => Some(m.clone()),
                _ => None,
            })
            .collect();
        let items = crate::views::message::build_items(
            &messages,
            &std::collections::HashMap::new(),
            false,
            None,
        );
        let segment = items
            .iter()
            .find_map(|i| match i {
                crate::conversation::ConvItem::Thinking(t) => Some(t),
                _ => None,
            })
            .expect("one activity segment");
        assert!(
            segment
                .entries
                .iter()
                .any(|e| matches!(e, crate::conversation::ActivityEntry::Tool(_))),
            "the tool entry folds into the segment: entries={:?}",
            segment.entries.len()
        );
    }

    #[test]
    fn chat_deltas_fold_into_the_chat_state() {
        let mut book = ChannelBook::default();
        let effect = book.apply(
            &chat_uri("c-1"),
            &StateAction::ChatDelta(ChatDeltaAction {
                turn_id: "t-1".into(),
                part_id: "p-1".into(),
                content: "hello".into(),
                meta: None,
            }),
        );
        // A delta with no preceding markdown part is a no-op for the reducer,
        // but it must not error.
        assert_eq!(effect, FoldEffect::Ignored);
    }

    #[test]
    fn the_extension_baseline_replaces_the_channel_state() {
        let mut book = ChannelBook::default();
        let plan_channel = format!("{}c-1", ext::channels::PLAN);
        book.ext.insert(
            plan_channel.clone(),
            XManoxState {
                order: Some(serde_json::json!(["b", "a"])),
                ..Default::default()
            },
        );
        let baseline = serde_json::json!({
            "type": ext::actions::BASELINE,
            "state": { "order": ["a", "b"] },
        });
        let effect = book.apply(&plan_channel, &StateAction::Unknown(baseline));
        assert_eq!(effect, FoldEffect::Changed);
        assert_eq!(
            book.ext[&plan_channel].order,
            Some(serde_json::json!(["a", "b"]))
        );
    }

    #[test]
    fn unknown_extension_actions_are_tolerated() {
        let mut book = ChannelBook::default();
        book.ext
            .insert("x-manox-plan:/c-1".into(), XManoxState::default());
        let effect = book.apply(
            "x-manox-plan:/c-1",
            &StateAction::Unknown(serde_json::json!({
                "type": "x-manox-plan/someFutureAction",
                "unheard": "of"
            })),
        );
        assert_eq!(effect, FoldEffect::Ignored);
    }

    #[test]
    fn unknown_baseline_channel_actions_are_a_no_op() {
        // Forward compatibility against a newer host: an action type this
        // build's `StateAction` does not know must deserialize to the
        // `Unknown` fallback — the client drops unparseable envelopes
        // instead — and then fold to nothing rather than panic or corrupt
        // state.
        let action: StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/someFutureAction",
            "unheard": "of"
        }))
        .expect("unknown action types must not break deserialization");
        assert!(matches!(action, StateAction::Unknown(_)));
        let mut book = ChannelBook::default();
        let effect = book.apply(&chat_uri("c-1"), &action);
        assert_eq!(effect, FoldEffect::Ignored);
    }

    #[test]
    fn a_chat_state_with_unknown_future_fields_still_parses() {
        // Forward compatibility against a newer protocol: snapshots may carry
        // fields this build has never heard of; they must not break parsing.
        let chat: ChatState = serde_json::from_value(serde_json::json!({
            "resource": "ahp-chat:/c-1",
            "title": "t",
            "status": 0,
            "modifiedAt": "2026-01-01T00:00:00Z",
            "turns": [],
            "someFutureField": { "nested": true },
        }))
        .expect("unknown fields must not break deserialization");
        assert!(chat.turns.is_empty());
    }

    #[test]
    fn the_thread_channel_baseline_replaces_the_row_state() {
        let mut book = ChannelBook::default();
        let channel = thread_uri("s-1");
        let effect = book.apply(
            &channel,
            &StateAction::Unknown(serde_json::json!({
                "type": ext::actions::BASELINE,
                "state": { "pinned": true, "label": "audit", "leaf": "t-9" },
            })),
        );
        assert_eq!(effect, FoldEffect::Changed);
        assert_eq!(book.ext[&channel].pinned, Some(true));
        assert_eq!(book.ext[&channel].label.as_deref(), Some("audit"));
        assert_eq!(book.ext[&channel].leaf.as_deref(), Some("t-9"));
    }

    #[test]
    fn thread_row_deltas_fold_on_the_thread_channel() {
        let mut book = ChannelBook::default();
        let channel = thread_uri("s-1");
        book.ext.insert(channel.clone(), XManoxState::default());
        // Upstream #818 first emitted these rows on the session channel,
        // where no fold ever consumed them; #842 moved them here.
        let pinned = book.apply(
            &channel,
            &StateAction::Unknown(serde_json::json!({
                "type": "x-manox/pinnedChanged",
                "pinned": true,
            })),
        );
        assert_eq!(pinned, FoldEffect::Changed);
        assert_eq!(book.ext[&channel].pinned, Some(true));
        let label = book.apply(
            &channel,
            &StateAction::Unknown(serde_json::json!({
                "type": "x-manox/labelChanged",
                "label": "audit",
            })),
        );
        assert_eq!(label, FoldEffect::Changed);
        assert_eq!(book.ext[&channel].label.as_deref(), Some("audit"));
    }

    #[test]
    fn the_row_accessors_read_their_verbatim_channels() {
        let mut book = ChannelBook::default();
        // Plan rows are keyed by the chat id, work rows and thread rows by the
        // session id — and none of them ever landed on the session channel.
        book.ext.insert(
            thread_uri("s-1"),
            XManoxState {
                pinned: Some(true),
                ..Default::default()
            },
        );
        book.ext.insert(
            plan_uri("s-1"),
            XManoxState {
                plan_mode: Some(true),
                plan: Some(serde_json::json!({ "v": 1 })),
                plan_review: Some(serde_json::json!({ "state": "proposed" })),
                ..Default::default()
            },
        );
        book.ext.insert(
            work_uri("s-1"),
            XManoxState {
                browser_suites: Some(vec!["chrome".into()]),
                goal: Some(serde_json::json!({ "text": "ship" })),
                ..Default::default()
            },
        );
        assert!(plan_mode_of(&book, "s-1"));
        assert_eq!(
            goal_of(&book, "s-1"),
            Some(&serde_json::json!({ "text": "ship" }))
        );
        assert!(goal_of(&book, "s-2").is_none(), "unsubscribed work channel");
        assert_eq!(
            plan_snapshot_of(&book, "s-1"),
            Some(&serde_json::json!({ "v": 1 }))
        );
        assert_eq!(browser_suites_of(&book, "s-1"), vec!["chrome".to_string()]);
        assert_eq!(
            goal_of(&book, "s-1"),
            Some(&serde_json::json!({ "text": "ship" }))
        );
        // The trap: the goal rides the work channel, so the thread-channel
        // ext face (`LeafView.ext`) never sees it.
        assert!(
            leaf(&book, "s-1")
                .ext
                .and_then(|x| x.goal.as_ref())
                .is_none()
        );
        assert!(plan_review_proposed(&book, "s-1"));
        assert_eq!(
            book.ext.get(&thread_uri("s-1")).and_then(|x| x.pinned),
            Some(true)
        );
        // An unsubscribed channel reads empty rather than panicking.
        assert!(browser_suites_of(&book, "s-2").is_empty());
        assert!(goal_of(&book, "s-2").is_none());
        assert!(!plan_mode_of(&book, "s-2"));
        assert!(!plan_review_proposed(&book, "s-2"));
    }

    /// The official unread count (#870's app half): the live session fold
    /// wins over the summary row (the isReadChanged echoes land there the
    /// moment they happen), the row is the parked fallback, and archived
    /// sessions never count.
    #[test]
    fn unread_sessions_reads_the_live_fold_first_and_skips_archived() {
        let mut store = AhpStore {
            book: ChannelBook::default(),
            client: None,
            replay_pending: false,
            chat_events: HashMap::new(),
            focused_session: None,
            optimistic_undo: Vec::new(),
            pending_writes: Vec::new(),
            request_hook: None,
            rejections: std::collections::VecDeque::new(),
            thread_subs: std::collections::HashSet::new(),
            pending_unread: std::collections::HashSet::new(),
            delivery_failure: None,
            queue_head_at: None,
            handshake_params: None,
            handshaking: false,
            delivery_signal_tx: None,
            _tasks: Vec::new(),
        };
        let session_state = |unread: bool, archived: bool| {
            let mut bits = 0u32;
            if !unread {
                bits |= ahp_types::state::SessionStatus::IsRead.bits();
            }
            if archived {
                bits |= ahp_types::state::SessionStatus::IsArchived.bits();
            }
            serde_json::from_value(serde_json::json!({
                "resource": session_uri("s-x"),
                "provider": "manox",
                "title": "x",
                "status": bits,
                "modifiedAt": "2026-10-09T00:00:00.000Z",
                "lifecycle": "ready",
                "activeClients": [],
                "chats": [],
            }))
            .expect("the session state parses")
        };
        // Row says READ, the live fold says UNREAD: the fold wins.
        store.book.seed_summaries(vec![
            serde_json::from_value(serde_json::json!({
                "resource": session_uri("s-x"),
                "provider": "manox",
                "title": "x",
                "status": ahp_types::state::SessionStatus::IsRead.bits()
                    | ahp_types::state::SessionStatus::Idle.bits(),
                "createdAt": "2026-10-09T00:00:00.000Z",
                "modifiedAt": "2026-10-09T00:00:00.000Z",
            }))
            .expect("the summary parses"),
        ]);
        store
            .book
            .sessions
            .insert("s-x".to_string(), session_state(true, false));
        assert_eq!(store.unread_sessions(), 1, "the live fold wins");

        // The focus clear lands on the fold: the count drops.
        store
            .book
            .sessions
            .insert("s-x".to_string(), session_state(false, false));
        assert_eq!(store.unread_sessions(), 0);

        // Parked (no fold) + an unread ROW: the row is the fallback.
        store.book.sessions.remove("s-x");
        store.book.seed_summaries(vec![
            serde_json::from_value(serde_json::json!({
                "resource": session_uri("s-x"),
                "provider": "manox",
                "title": "x",
                "status": ahp_types::state::SessionStatus::Idle.bits(),
                "createdAt": "2026-10-09T00:00:00.000Z",
                "modifiedAt": "2026-10-09T00:00:00.000Z",
            }))
            .expect("the summary parses"),
        ]);
        assert_eq!(store.unread_sessions(), 1, "the parked row reads unread");

        // Archived never counts, even unread.
        store
            .book
            .sessions
            .insert("s-x".to_string(), session_state(true, true));
        assert_eq!(store.unread_sessions(), 0, "archived is out of the badge");
    }

    #[test]
    fn thread_subscriptions_are_due_once_per_row_per_run() {
        let mut store = AhpStore {
            book: ChannelBook::default(),
            client: None,
            replay_pending: false,
            chat_events: HashMap::new(),
            focused_session: None,
            optimistic_undo: Vec::new(),
            pending_writes: Vec::new(),
            request_hook: None,
            rejections: std::collections::VecDeque::new(),
            thread_subs: std::collections::HashSet::new(),
            pending_unread: std::collections::HashSet::new(),
            delivery_failure: None,
            queue_head_at: None,
            handshake_params: None,
            handshaking: false,
            delivery_signal_tx: None,
            _tasks: Vec::new(),
        };
        store.book.seed_summaries(vec![
            serde_json::from_value(serde_json::json!({
                "resource": session_uri("s-1"),
                "provider": "test",
                "title": "row",
                "status": 0,
                "createdAt": "2026-01-01T00:00:00Z",
                "modifiedAt": "2026-01-01T00:00:00Z",
            }))
            .expect("summary parses"),
        ]);
        // First sight is due; the second sight of the same row is not — the
        // catalogue refetches on every summary change and must not re-issue
        // the subscription (nor, with one, duplicate it server-side).
        assert!(store.thread_sub_due("s-1"));
        assert!(!store.thread_sub_due("s-1"));
        assert!(store.thread_sub_due("s-2"), "an unseen row is still due");
    }

    #[test]
    fn a_rejected_echo_is_surfaced_not_folded() {
        let mut book = ChannelBook::default();
        let effect = book.apply(
            &session_uri("s-1"),
            &StateAction::Unknown(serde_json::json!({
                "type": "session/titleChanged",
                "title": "must not fold",
                "rejectionReason": "blank title"
            })),
        );
        assert_eq!(effect, FoldEffect::Rejected("blank title".into()));
        assert!(!book.sessions.contains_key("s-1"));
    }

    #[test]
    fn catalogue_seeding_is_keyed_by_bare_session_id() {
        let mut book = ChannelBook::default();
        let summary: SessionSummary = serde_json::from_value(serde_json::json!({
            "resource": session_uri("s-9"),
            "provider": "test",
            "title": "row",
            "status": 0,
            "createdAt": "2026-01-01T00:00:00Z",
            "modifiedAt": "2026-01-01T00:00:00Z",
        }))
        .expect("a summary parses");
        assert!(book.seed_summaries(vec![summary]));
        assert!(book.summaries.contains_key("s-9"));
        assert!(book.remove_summary(&session_uri("s-9")));
        assert!(book.summaries.is_empty());
    }

    /// A disconnected store: `dispatch` queues into `pending_writes` instead
    /// of dialing the process-singleton host, so a test reads back exactly
    /// what would ride the wire.
    fn disconnected_store() -> AhpStore {
        AhpStore {
            book: ChannelBook::default(),
            client: None,
            replay_pending: false,
            chat_events: HashMap::new(),
            focused_session: None,
            optimistic_undo: Vec::new(),
            pending_writes: Vec::new(),
            request_hook: None,
            rejections: std::collections::VecDeque::new(),
            thread_subs: std::collections::HashSet::new(),
            pending_unread: std::collections::HashSet::new(),
            delivery_failure: None,
            queue_head_at: None,
            handshake_params: None,
            handshaking: false,
            delivery_signal_tx: None,
            _tasks: Vec::new(),
        }
    }

    #[test]
    fn confirmation_verdicts_carry_the_translator_auth_stamp() {
        for (approved, decision) in [(true, "allow"), (false, "deny")] {
            let mut store = disconnected_store();
            store.confirm_tool_call(
                "c-1",
                "t-1",
                "call-1",
                "auth-1",
                (!approved).then(|| "deny".to_string()),
                approved,
            );
            assert_eq!(store.pending_writes.len(), 1);
            let Some(PendingWrite::Dispatch(channel, action)) = store.pending_writes.first() else {
                panic!("the {decision} verdict queues while disconnected");
            };
            assert_eq!(channel, &chat_uri("c-1"));
            let StateAction::ChatToolCallConfirmed(confirmed) = action.as_ref() else {
                panic!("the queued write is the confirmation");
            };
            assert_eq!(confirmed.approved, approved);
            assert_eq!(
                confirmed.selected_option_id.as_deref(),
                (!approved).then_some("deny"),
                "the clicked option rides the verdict (an \"always\"-style \
                 label must not collapse to a bare bool)"
            );
            let meta = confirmed.meta.as_ref().expect("the auth stamp rides _meta");
            assert_eq!(
                meta.get(ext::META_KEY)
                    .and_then(|seat| seat.get("authId"))
                    .and_then(Value::as_str),
                Some("auth-1"),
                "the host settles by this id and silently ignores a verdict without it"
            );
        }
    }

    /// An unanswered ask inside an ARCHIVED turn is a corpse the runtime can
    /// never settle (input requests settle on the active turn only) and no
    /// client can ever answer: the fold must not surface it, or the composer
    /// locks in ask mode forever. The live cancel archives the turn before
    /// the gate's decision row lands — exactly this shape (2026-10 strand).
    #[test]
    fn an_unanswered_ask_in_an_archived_turn_stops_surfacing() {
        let mut book = ChannelBook::default();
        let chat = chat_uri("s-1");
        for action in [
            StateAction::ChatTurnStarted(ChatTurnStartedAction {
                turn_id: "t-1".into(),
                started_at: "2026-10-14T00:00:00.000Z".into(),
                message: user_message("plan the release".into()),
                queued_message_id: None,
                meta: None,
            }),
            StateAction::ChatInputRequested(ahp_types::actions::ChatInputRequestedAction {
                request: ahp_types::state::ChatInputRequest {
                    id: "call_ask".into(),
                    message: Some("AskUserQuestion".into()),
                    url: None,
                    questions: None,
                    answers: None,
                },
            }),
            // The live-cancel race: the turn archives with the part unanswered.
            StateAction::ChatTurnCancelled(ChatTurnCancelledAction {
                turn_id: "t-1".into(),
                duration: 0,
                meta: None,
            }),
        ] {
            book.apply(&chat, &action);
        }
        let view = leaf(&book, "s-1");
        assert!(
            view.open_chat_input().is_none(),
            "a dead ask in an archived turn must not surface — it would re-seed the card forever and lock the composer in ask mode"
        );
        assert!(view.chat_input("call_ask").is_none());
    }

    /// The control: a live ask on the active turn surfaces as before.
    #[test]
    fn an_open_ask_on_the_active_turn_still_surfaces() {
        let mut book = ChannelBook::default();
        let chat = chat_uri("s-1");
        for action in [
            StateAction::ChatTurnStarted(ChatTurnStartedAction {
                turn_id: "t-1".into(),
                started_at: "2026-10-14T00:00:00.000Z".into(),
                message: user_message("plan the release".into()),
                queued_message_id: None,
                meta: None,
            }),
            StateAction::ChatInputRequested(ahp_types::actions::ChatInputRequestedAction {
                request: ahp_types::state::ChatInputRequest {
                    id: "call_ask".into(),
                    message: Some("AskUserQuestion".into()),
                    url: None,
                    questions: None,
                    answers: None,
                },
            }),
        ] {
            book.apply(&chat, &action);
        }
        let view = leaf(&book, "s-1");
        let (chat_id, request) = view.open_chat_input().expect("the live ask surfaces");
        assert_eq!(chat_id, "s-1");
        assert_eq!(request.id, "call_ask");
        assert!(view.chat_input("call_ask").is_some());
    }

    /// The failure face maps the latch verbatim.
    #[test]
    fn a_latched_failure_reports_verbatim() {
        let mut store = test_store();
        assert_eq!(store.delivery_failure(), None, "healthy reads as healthy");
        store.delivery_failure = Some(DeliveryFailure::Handshake("dead".into()));
        assert_eq!(
            store.delivery_failure(),
            Some(DeliveryFailure::Handshake("dead".into())),
        );
    }

    /// The stall face is LAZY: a pre-connect queue counts only past the
    /// delivery budget, and a fresh queue (handshake still dialing) reads
    /// healthy — no timer, no polling pump on the healthy path.
    #[test]
    fn a_stale_pre_connect_queue_reads_as_stalled_not_a_fresh_one() {
        let mut store = test_store();
        store.queue_head_at = Some(std::time::Instant::now());
        assert_eq!(
            store.delivery_failure(),
            None,
            "a fresh queue is in flight, not stalled"
        );
        store.queue_head_at = Some(
            std::time::Instant::now()
                - AhpStore::DELIVERY_BUDGET
                - std::time::Duration::from_secs(1),
        );
        assert_eq!(
            store.delivery_failure(),
            Some(DeliveryFailure::Stalled),
            "a queue past the budget reads as stalled"
        );
    }

    /// A dispatch that leaves retires a latched Dispatch face — the strip
    /// must not outlive the transport it reports dead.
    #[test]
    fn a_recovered_dispatch_retires_a_latched_dispatch_face() {
        let mut store = test_store();
        assert!(
            store.absorb_delivery_signal(DeliverySignal::Failed(DeliveryFailure::Dispatch(
                "dead".into()
            )))
        );
        assert!(matches!(
            store.delivery_failure(),
            Some(DeliveryFailure::Dispatch(_))
        ));
        assert!(store.absorb_delivery_signal(DeliverySignal::DispatchRecovered));
        assert_eq!(store.delivery_failure(), None);
        assert!(
            !store.absorb_delivery_signal(DeliverySignal::DispatchRecovered),
            "a recovery against no latch changes nothing (no spurious notify)"
        );
    }

    /// A handshake face is not a dispatch's to clear: only its own landing
    /// (or a retry) retires it.
    #[test]
    fn a_recovered_dispatch_does_not_retire_a_handshake_face() {
        let mut store = test_store();
        assert!(
            store.absorb_delivery_signal(DeliverySignal::Failed(DeliveryFailure::Handshake(
                "dead".into()
            )))
        );
        assert!(!store.absorb_delivery_signal(DeliverySignal::DispatchRecovered));
        assert!(matches!(
            store.delivery_failure(),
            Some(DeliveryFailure::Handshake(_))
        ));
    }

    /// Derived display events land in the bucket of the session the
    /// envelope's channel resolves to, and a drain takes exactly one bucket:
    /// the routing identity the wire only puts on the envelope must survive
    /// derivation (the cross-thread stream-bleed regression).
    #[test]
    fn display_events_bucket_per_session_and_a_drain_takes_one() {
        let mut store = test_store();
        store.push_chat_event("s-1".to_string(), crate::chat_fold::ChatEvent::TurnStarted);
        store.push_chat_event("s-2".to_string(), crate::chat_fold::ChatEvent::TurnStarted);
        store.push_chat_event("s-1".to_string(), crate::chat_fold::ChatEvent::TurnFinished);

        let drained = store.drain_chat_events("s-1");
        assert_eq!(drained.len(), 2, "only the bound session's bucket drains");
        assert!(matches!(
            drained[0],
            crate::chat_fold::ChatEvent::TurnStarted
        ));
        assert!(matches!(
            drained[1],
            crate::chat_fold::ChatEvent::TurnFinished
        ));

        // The other session's bucket is untouched and drains on its own
        // binding; events are consumed once.
        assert_eq!(store.drain_chat_events("s-2").len(), 1);
        assert!(store.drain_chat_events("s-2").is_empty());
        // A session with no bucket drains empty rather than panicking.
        assert!(store.drain_chat_events("s-none").is_empty());
    }

    /// A bucket nobody drains (a parked session streaming in the background)
    /// is bounded: the oldest entries drop, the newest survive — correctness
    /// is the book's job, the bucket only covers the bound session's live
    /// deltas.
    #[test]
    fn a_display_event_bucket_is_bounded_dropping_oldest_first() {
        let mut store = test_store();
        let total = CHAT_EVENT_BUCKET_CAP + 2;
        for ix in 0..total {
            store.push_chat_event(
                "s-1".to_string(),
                crate::chat_fold::ChatEvent::AgentText(ix.to_string()),
            );
        }
        let drained = store.drain_chat_events("s-1");
        assert_eq!(drained.len(), CHAT_EVENT_BUCKET_CAP, "the bucket is capped");
        let first = match drained.first() {
            Some(crate::chat_fold::ChatEvent::AgentText(text)) => text.as_str(),
            other => panic!("unexpected first event: {other:?}"),
        };
        assert_eq!(first, "2", "the two oldest entries dropped");
        let last = match drained.last() {
            Some(crate::chat_fold::ChatEvent::AgentText(text)) => text.as_str(),
            other => panic!("unexpected last event: {other:?}"),
        };
        assert_eq!(last, (total - 1).to_string(), "the newest entry survives");
    }

    /// A bare store for accessor tests: no client, no channels.
    fn test_store() -> AhpStore {
        AhpStore {
            book: ChannelBook::default(),
            client: None,
            replay_pending: false,
            chat_events: HashMap::new(),
            focused_session: None,
            optimistic_undo: Vec::new(),
            pending_writes: Vec::new(),
            request_hook: None,
            rejections: std::collections::VecDeque::new(),
            thread_subs: std::collections::HashSet::new(),
            pending_unread: std::collections::HashSet::new(),
            delivery_failure: None,
            queue_head_at: None,
            handshake_params: None,
            handshaking: false,
            delivery_signal_tx: None,
            _tasks: Vec::new(),
        }
    }

    fn single_key(values: &[&str]) -> Vec<serde_json::Map<String, Value>> {
        use serde_json::json;
        values
            .iter()
            .map(|value| {
                let mut pick = serde_json::Map::new();
                pick.insert("model".to_string(), json!(value));
                pick
            })
            .collect()
    }

    #[test]
    fn an_accepted_own_config_echo_spends_the_undo_history() {
        let mut store = disconnected_store();
        for pick in single_key(&["prov/new"]) {
            store.optimistic_config("s-1", &pick);
        }
        assert_eq!(store.optimistic_undo.len(), 1);

        // The acceptance half of the write-ahead reconciliation: the host
        // echoed this exact write without a refusal.
        store.confirm_config("s-1", &single_key(&["prov/new"]).remove(0));
        assert!(
            store.optimistic_undo.is_empty(),
            "a confirmed write has no undo left to spend"
        );

        // A later refusal touching the same key must NOT restore the
        // pre-write value over the confirmed one.
        store.rollback_refused_config("s-1", &single_key(&["prov/new"]).remove(0));
        assert_eq!(leaf(&store.book, "s-1").model_id(), Some("prov/new"));
    }

    /// The single-slot undo bug this reconciliation replaces: two writes
    /// to one key in flight, the FIRST one's refusal arrives — the fold
    /// must keep showing the second write (still pending), and when the
    /// second is refused too, the fold restores the original confirmed
    /// base, never the refused first value.
    #[test]
    fn a_refusal_among_two_in_flight_writes_never_surfaces_the_refused_value() {
        let mut store = disconnected_store();
        for pick in single_key(&["prov/a", "prov/b"]) {
            store.optimistic_config("s-1", &pick);
        }
        assert_eq!(
            leaf(&store.book, "s-1").model_id(),
            Some("prov/b"),
            "the fold shows the latest in-flight write"
        );

        // The FIRST write's refusal: the fold keeps the second write, and
        // the second's rollback base becomes the original base (None —
        // the seat never carried a model).
        store.rollback_refused_config("s-1", &single_key(&["prov/a"]).remove(0));
        assert_eq!(
            leaf(&store.book, "s-1").model_id(),
            Some("prov/b"),
            "a refused EARLIER write does not clobber the later pending one"
        );

        // The SECOND write's refusal: the fold restores the base, not the
        // refused first value.
        store.rollback_refused_config("s-1", &single_key(&["prov/b"]).remove(0));
        assert_eq!(
            leaf(&store.book, "s-1").model_id(),
            None,
            "both writes refused: the fold is back to the pre-write truth"
        );
    }

    /// The confirm half under the same load: the first write confirmed
    /// while the second is in flight, then the second refused — the fold
    /// restores the CONFIRMED value, not the pre-first base.
    #[test]
    fn a_confirmed_first_write_is_the_rollback_base_for_a_refused_second() {
        let mut store = disconnected_store();
        // Seed a confirmed base so the restore target is observable.
        let mut base = serde_json::Map::new();
        base.insert(
            "model".to_string(),
            serde_json::Value::String("prov/m0".into()),
        );
        store.optimistic_config("s-1", &base);
        store.confirm_config("s-1", &base);
        for pick in single_key(&["prov/a", "prov/b"]) {
            store.optimistic_config("s-1", &pick);
        }

        // The FIRST of the two new writes is confirmed while the second
        // is still in flight.
        store.confirm_config("s-1", &single_key(&["prov/a"]).remove(0));
        assert_eq!(
            store.optimistic_undo.len(),
            1,
            "only the second write stays pending"
        );

        // The SECOND write is refused: the fold restores the confirmed
        // `prov/a`, never the older `prov/m0`.
        store.rollback_refused_config("s-1", &single_key(&["prov/b"]).remove(0));
        assert_eq!(
            leaf(&store.book, "s-1").model_id(),
            Some("prov/a"),
            "the rollback target is the last CONFIRMED value"
        );
    }

    #[test]
    fn a_snapshot_spends_the_sessions_undo_history() {
        use serde_json::json;
        let mut store = disconnected_store();
        let mut pick = serde_json::Map::new();
        pick.insert("model".to_string(), json!("prov/new"));
        store.optimistic_config("s-1", &pick);
        pick.insert("model".to_string(), json!("prov/other"));
        store.optimistic_config("s-2", &pick);

        store.apply_snapshot_confirmed(
            &session_uri("s-1"),
            SnapshotState::Session(Box::new(empty_session(&session_uri("s-1")))),
        );
        assert!(
            store.optimistic_undo.iter().all(|(sid, ..)| sid != "s-1"),
            "server truth replaces the session's write-ahead history"
        );
        assert!(
            store.optimistic_undo.iter().any(|(sid, ..)| sid == "s-2"),
            "other sessions keep their pending writes"
        );
    }

    #[test]
    fn approval_mode_defaults_follow_the_seated_value_then_schema_then_host() {
        use ahp_types::state::{SessionConfigPropertySchema, SessionConfigState};
        use manox_agent::thread::PermissionMode;
        use serde_json::json;

        let mut book = ChannelBook::default();
        // A seat-less session runs under the host default — the retired
        // `ReadOnly` fallback contradicted it on every such reopen.
        assert_eq!(
            leaf(&book, "s-1").approval_mode_or_default(),
            PermissionMode::WorkspaceWrite
        );

        {
            let state = book
                .sessions
                .entry("s-1".to_string())
                .or_insert_with(|| empty_session(&session_uri("s-1")));
            let mut schema_properties = std::collections::HashMap::new();
            schema_properties.insert(
                "approvalMode".to_string(),
                SessionConfigPropertySchema {
                    r#type: "string".to_string(),
                    title: "Approval mode".to_string(),
                    description: None,
                    default: Some(json!("read-only")),
                    r#enum: None,
                    enum_labels: None,
                    enum_descriptions: None,
                    read_only: None,
                    items: None,
                    min_items: None,
                    max_items: None,
                    properties: None,
                    required: None,
                    additional_properties: None,
                    enum_dynamic: None,
                    session_mutable: Some(true),
                },
            );
            state.config = Some(SessionConfigState {
                schema: ahp_types::state::SessionConfigSchema {
                    r#type: "object".to_string(),
                    properties: schema_properties,
                    required: None,
                },
                values: serde_json::Map::new(),
            });
        }
        assert_eq!(
            leaf(&book, "s-1").approval_mode_or_default(),
            PermissionMode::ReadOnly,
            "the host-declared schema default is the seat-less truth"
        );

        book.sessions
            .get_mut("s-1")
            .unwrap()
            .config
            .as_mut()
            .unwrap()
            .values
            .insert("approvalMode".to_string(), json!("danger-full-access"));
        assert_eq!(
            leaf(&book, "s-1").approval_mode_or_default(),
            PermissionMode::DangerFullAccess,
            "the seated value outranks the schema default"
        );
    }
}
