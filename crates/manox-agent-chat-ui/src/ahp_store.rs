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
            // action is tolerated rather than folded.
            Channel::Terminal(_) | Channel::Extension(_) => false,
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
        let Some(client) = self.client.clone() else {
            self.pending_writes.push(PendingWrite::Subscribe(uri));
            return;
        };
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
        let Some(client) = self.client.clone() else {
            self.pending_writes
                .push(PendingWrite::Unsubscribe(uri.into()));
            return;
        };
        let uri = uri.into();
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
        let Some(client) = self.client.clone() else {
            self.pending_writes
                .push(PendingWrite::Dispatch(channel.into(), Box::new(action)));
            return;
        };
        let channel = channel.into();
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
            seat.values.insert(k.clone(), v.clone());
        }
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

    /// Settle a tool-call confirmation (the approval gate). The host keys
    /// the settle on the pending call's auth id, carried in `tool_call_id`.
    pub fn confirm_tool_call(
        &mut self,
        chat_id: &str,
        turn_id: &str,
        tool_call_id: &str,
        approved: bool,
    ) {
        let action = StateAction::ChatToolCallConfirmed(ChatToolCallConfirmedAction {
            turn_id: turn_id.to_string(),
            tool_call_id: tool_call_id.to_string(),
            meta: None,
            approved,
            confirmed: None,
            reason: None,
            edited_tool_input: None,
            user_suggestion: None,
            reason_message: None,
            selected_option_id: None,
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
        let action = StateAction::ChatInputCompleted(ChatInputCompletedAction {
            request_id: request_id.to_string(),
            response: ChatInputResponseKind::Accept,
            answers: Some(answers),
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

    /// Replace the session's (single) working directory.
    pub fn set_cwd(&mut self, session_id: &str, path: &str) {
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
        ext: book.ext.get(&session_uri(session_id)),
        metrics: book.metrics.get(&chat_uri(&chat_id)),
    }
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

impl LeafView<'_> {
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

    /// The working directory (first granted, file:// form stripped).
    pub fn cwd(&self) -> Option<String> {
        self.session
            .and_then(|s| s.working_directories.as_ref())
            .and_then(|dirs| dirs.first())
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
        self.requests().iter().find_map(|r| match r {
            ahp_types::state::SessionInputRequest::ChatInput(c) if c.id == id => {
                Some((crate::ahp_store::id_of(&c.chat).to_string(), &c.request))
            }
            _ => None,
        })
    }

    /// The session goal (verbatim payload).
    pub fn goal(&self) -> Option<&Value> {
        self.ext.and_then(|x| x.goal.as_ref())
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
}
