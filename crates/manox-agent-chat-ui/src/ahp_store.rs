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

use std::collections::{BTreeMap, HashMap};
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
use manox_ahp::translate::actions::config_keys;
use serde::Serialize;
use serde_json::Value;

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
    /// Connection-level catalogue channels (`x-manox-workspaces://`,
    /// `x-manox-commands://`): their baseline state is an open payload the
    /// XManoxState fold has no slots for, so it is kept verbatim.
    pub catalogues: HashMap<String, Value>,
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
            catalogues: HashMap::new(),
            server_seq: 0,
        }
    }
}

impl ChannelBook {
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

/// The GPUI-facing store. One per process: the host is a singleton (L11) and
/// this is its only desktop client.
pub struct AhpStore {
    pub book: ChannelBook,
    client: Option<Client>,
    /// True while the pre-connect queue is being replayed; writes issued in
    /// that window queue behind the replay instead of racing it.
    replay_pending: bool,
    /// Display events derived from chat-channel folds since the last drain —
    /// the live-streaming leg the conversation applier consumes.
    chat_events: Vec<crate::chat_fold::ChatEvent>,
    /// Optimistic writes the host has not confirmed yet, oldest first:
    /// (session id, config key, the value the key had before). A rejection
    /// restores the remembered values so a refused pick does not linger.
    optimistic_undo: Vec<(String, String, Option<Value>)>,
    /// Writes issued before the handshake completed, replayed in order on
    /// connect. The landing session's create/subscribe rides this: the
    /// workspace constructs the store and binds the chat column in the same
    /// frame, long before the in-proc handshake answers.
    pending_writes: Vec<PendingWrite>,
    /// Host → client capability requests, answered by the UI layer's hook.
    request_hook: Option<RequestHook>,
    /// Recent dispatch rejections (bounded ring) for UI surfacing.
    rejections: std::collections::VecDeque<String>,
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
        let mut store = Self {
            book: ChannelBook::default(),
            client: None,
            replay_pending: false,
            chat_events: Vec::new(),
            optimistic_undo: Vec::new(),
            pending_writes: Vec::new(),
            request_hook: None,
            rejections: std::collections::VecDeque::new(),
            _tasks: Vec::new(),
        };
        store.spawn_handshake(cwd, event_tx, req_tx, cx);
        store.spawn_pump(event_rx, cx);
        store.spawn_capability_pump(req_rx, cx);
        store
    }

    /// Whether the handshake completed (writes before this are queued).
    pub fn is_connected(&self) -> bool {
        self.client.is_some()
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
            chat_events: Vec::new(),
            optimistic_undo: Vec::new(),
            pending_writes: Vec::new(),
            request_hook: None,
            rejections: std::collections::VecDeque::new(),
            _tasks: Vec::new(),
        }
    }

    /// Seed a placeholder summary for a session this client just created, so
    /// the sidebar row exists the moment the conversation starts: the host's
    /// store row (what `listSessions` serves) lands with the first
    /// persistence, which can lag a whole turn. The host's summary upserts
    /// over the placeholder (`seed_summaries` keys by id).
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
        }])
    }

    /// Pop the next queued write, if any and if the replay may proceed.
    fn next_pending(&mut self) -> Option<PendingWrite> {
        if !self.replay_pending {
            return None;
        }
        (!self.pending_writes.is_empty()).then(|| self.pending_writes.remove(0))
    }

    fn spawn_handshake(
        &mut self,
        cwd: PathBuf,
        event_tx: async_channel::Sender<ahp::ClientEvent>,
        req_tx: async_channel::Sender<CapabilityRequest>,
        cx: &mut gpui::Context<Self>,
    ) {
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
                        vec![ahp_types::version::PROTOCOL_VERSION.to_string()],
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
                    return;
                }
            };
            if this
                .update(cx, |store, _| {
                    store.client = Some(client.clone());
                    store.replay_pending = true;
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
                                if store.book.apply_snapshot(&uri, snapshot.state) {
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
                    if store.book.apply_snapshot(&uri, snap.state) {
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

    /// Route one inbound event into the fold.
    fn route(&mut self, event: ahp::ClientEvent, cx: &mut gpui::Context<Self>) {
        match event.event {
            ahp::SubscriptionEvent::Action(envelope) => {
                self.book.server_seq = self.book.server_seq.max(envelope.server_seq);
                let chat_id = if envelope.channel.starts_with("ahp-chat:/") {
                    Some(id_of(&envelope.channel).to_string())
                } else {
                    None
                };
                match self.book.apply(&envelope.channel, &envelope.action) {
                    FoldEffect::Changed => {
                        if let Some(chat_id) = chat_id
                            && let Some(chat) = self.book.chats.get(&chat_id)
                            && let Some(event) = crate::chat_fold::ChatEvent::from_action(
                                &envelope.action,
                                Some(chat),
                            )
                        {
                            self.chat_events.push(event);
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
                        // An optimistic write (model pick, cwd, …) that the
                        // host refused must not linger on the UI: roll the
                        // keys it touched out of the fold, and surface the
                        // refusal on the transcript.
                        if let Some(keys) = rejected_config_keys(&envelope.action) {
                            let session_id = id_of(&envelope.channel).to_string();
                            self.rollback_optimistic(&session_id, &keys);
                            self.chat_events.push(crate::chat_fold::ChatEvent::Notice {
                                text: format!(
                                    "{}: {reason}",
                                    manox_i18n::t("workspace-change-rejected")
                                ),
                            });
                        }
                        cx.notify();
                    }
                }
            }
            ahp::SubscriptionEvent::SessionAdded(params) => {
                let changed = self.book.seed_summaries(vec![params.summary]);
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
                    if store.book.apply_snapshot(&uri, snapshot.state) {
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
            self.pending_writes
                .push(PendingWrite::Dispatch(channel, Box::new(action)));
            return;
        }
        let client = self.client.clone().expect("guard above");
        manox_agent::runtime::handle().spawn(async move {
            if let Err(err) = client.dispatch(channel, action).await {
                tracing::warn!(error = %err, "dispatch failed");
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
                });
            }
        })
        .detach();
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
            // Remember what the key had before the optimistic write: a host
            // rejection restores it, so a refused pick does not linger on the
            // UI as an adopted value.
            self.optimistic_undo.push((
                session_id.to_string(),
                k.clone(),
                seat.values.get(k).cloned(),
            ));
            seat.values.insert(k.clone(), v.clone());
        }
    }

    /// Roll back optimistic writes: every remembered (session, key) touched by
    /// a REJECTED action restores its pre-optimistic value (removing the key
    /// when there was none — the host does not re-broadcast authority for a
    /// write it refused).
    fn rollback_optimistic(&mut self, session_id: &str, keys: &[String]) {
        for key in keys {
            for (sid, k, old) in self.optimistic_undo.iter().rev() {
                if sid == session_id && k == key {
                    if let Some(state) = self.book.sessions.get_mut(session_id)
                        && let Some(config) = &mut state.config
                    {
                        match old {
                            Some(v) => {
                                config.values.insert(k.clone(), v.clone());
                            }
                            None => {
                                config.values.remove(k);
                            }
                        }
                    }
                    break;
                }
            }
        }
        self.optimistic_undo
            .retain(|(sid, k, _)| !(sid == session_id && keys.contains(k)));
    }

    /// Drain the chat display events derived since the last drain (the live
    /// streaming leg into the conversation applier).
    pub fn drain_chat_events(&mut self) -> Vec<crate::chat_fold::ChatEvent> {
        std::mem::take(&mut self.chat_events)
    }

    /// Recent dispatch rejections, oldest first.
    pub fn rejections(&self) -> impl Iterator<Item = &str> {
        self.rejections.iter().map(String::as_str)
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
pub fn plan_review_of<'a>(
    book: &'a ChannelBook,
    session_id: &str,
    request_id: &str,
) -> Option<&'a Value> {
    let chat_id = book
        .default_chat(session_id)
        .map(|uri| id_of(&uri).to_string())
        .unwrap_or_else(|| session_id.to_string());
    let channel = format!("{}{chat_id}", manox_ahp::ext::channels::PLAN);
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
    let chat_id = book
        .default_chat(session_id)
        .map(|uri| id_of(&uri).to_string())
        .unwrap_or_else(|| session_id.to_string());
    let channel = format!("{}{chat_id}", manox_ahp::ext::channels::PLAN);
    book.ext
        .get(&channel)
        .and_then(|x| x.plan_mode)
        .unwrap_or(false)
}

/// The session's current plan document (the kernel snapshot shape the plan
/// restore rehydrates), from the `x-manox-plan` channel.
pub fn plan_snapshot_of<'a>(book: &'a ChannelBook, session_id: &str) -> Option<&'a Value> {
    let chat_id = book
        .default_chat(session_id)
        .map(|uri| id_of(&uri).to_string())
        .unwrap_or_else(|| session_id.to_string());
    let channel = format!("{}{chat_id}", manox_ahp::ext::channels::PLAN);
    book.ext.get(&channel)?.plan.as_ref()
}

/// The session's active browser suites, from the `x-manox-work` channel
/// (keyed by the session id — work rows have no chat scope).
pub fn browser_suites_of(book: &ChannelBook, session_id: &str) -> Vec<String> {
    book.ext
        .get(&work_uri(session_id))
        .and_then(|x| x.browser_suites.clone())
        .unwrap_or_default()
}

/// Whether the session's plan channel holds an open (proposed) review — the
/// sidebar row badge's read. Resolves the chat id the same way
/// [`plan_review_of`] does, falling back to the session id when the pointer
/// has not landed.
pub fn plan_review_proposed(book: &ChannelBook, session_id: &str) -> bool {
    let chat_id = book
        .default_chat(session_id)
        .map(|uri| id_of(&uri).to_string())
        .unwrap_or_else(|| session_id.to_string());
    let channel = format!("{}{chat_id}", manox_ahp::ext::channels::PLAN);
    book.ext
        .get(&channel)
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
        for turn in chat.turns.iter().rev() {
            scan(&turn.response_parts, id, &mut latest);
        }
        latest.map(|input| (id_of(&chat.resource).to_string(), &input.request))
    }

    /// The session goal (verbatim payload).
    pub fn goal(&self) -> Option<&Value> {
        self.ext.and_then(|x| x.goal.as_ref())
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
    let mut selections = Vec::with_capacity(questions.len());
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
        selections.push(vec![false; options.len()]);
        // A plan-review elicitation (request id `plan-review:<entry>`) renders
        // the verdict card: the affirmative option is highlighted by label
        // (the render matches `intent.approve` against the option labels).
        let intent = id
            .starts_with("plan-review:")
            .then(|| crate::column::AskIntent {
                kind: "plan-review".to_string(),
                approve: "Approve".to_string(),
            });
        parsed.push(crate::column::AskQuestion {
            id,
            question,
            header,
            detail: String::new(),
            intent,
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
        selections,
    })
}

/// The config keys an optimistic write touched, when the action is one the
/// host can refuse per-key (a session config change). A rejection rolls
/// these out of the client fold so a refused pick does not linger on the UI.
fn rejected_config_keys(action: &ahp_types::actions::StateAction) -> Option<Vec<String>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ahp_types::actions::{ChatDeltaAction, SessionTitleChangedAction};

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
                ..Default::default()
            },
        );
        assert!(plan_mode_of(&book, "s-1"));
        assert_eq!(
            plan_snapshot_of(&book, "s-1"),
            Some(&serde_json::json!({ "v": 1 }))
        );
        assert_eq!(browser_suites_of(&book, "s-1"), vec!["chrome".to_string()]);
        assert!(plan_review_proposed(&book, "s-1"));
        assert_eq!(
            book.ext.get(&thread_uri("s-1")).and_then(|x| x.pinned),
            Some(true)
        );
        // An unsubscribed channel reads empty rather than panicking.
        assert!(browser_suites_of(&book, "s-2").is_empty());
        assert!(!plan_mode_of(&book, "s-2"));
        assert!(!plan_review_proposed(&book, "s-2"));
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
            chat_events: Vec::new(),
            optimistic_undo: Vec::new(),
            pending_writes: Vec::new(),
            request_hook: None,
            rejections: std::collections::VecDeque::new(),
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
}
