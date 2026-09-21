//! Per-session gpui leaf over a [`ClientStore`] + the v2 journal fold.
//!
//! T-D: the handle is a pure leaf (store + emitter) fed by the app-level
//! [`crate::multiplexer::SessionMultiplexer`]. T6 adds the v2 stream side:
//! `StreamItem` frames (Snapshot / Entry / Projections) feed the per-session
//! [`crate::journal_fold::JournalFold`] engine; the committed window changes
//! fold into the store, live deltas re-emit as `ThreadEvent`s, and the
//! optimistic echo retires on the durable row. `StreamEnd{Resync}` (and any
//! engine violation) requests a seamless re-open through the multiplexer
//! [`Self::outbound`] channel; gap-repair `PageHistory` requests ride the same
//! channel and their `Response`s are correlated by MsgId through
//! [`Self::apply_page_response`].

use gpui::{Context, EventEmitter, Task};
use manox_agent::ThreadEvent;
use manox_protocol::{FromServer, MsgId, RpcError, StreamFrame, StreamId};

use crate::client_store::ClientStore;
use crate::journal_fold::{FoldOut, JournalFold, WindowChange};
use crate::journal_translate;
use crate::server_note_translate::{server_call_to_thread_event, server_note_to_thread_event};

/// §E.3: Q-face fetches are debounced — a dense committed burst (an
/// assistant message plus its tool rows settling within milliseconds)
/// fires one trailing fetch instead of one `GetConversationInfo` per row.
/// Test-visible: the info-row debounce the handle applies.
pub const INFO_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);

/// A signal the leaf asks the multiplexer to carry on the shared connection.
#[derive(Debug, Clone)]
pub enum LeafRequest {
    /// Re-open the follow stream for this session. `reattach` (the user's
    /// banner/chip retry only) also re-sends `OpenSession` ahead of the
    /// `StreamOpen`, so the retry recovers the attach's OpenSession having
    /// failed once; automatic reopens stay pure `StreamOpen` — a bare
    /// OpenSession on an already-superseded id would insert a ghost
    /// `ServerSession` under the old id server-side (a leaked session, an
    /// idle pump, and a second engine on the successor's journal).
    Reopen {
        session_id: String,
        stream_id: StreamId,
        reattach: bool,
    },
    /// Fetch a journal page ending at `through_seq` and deliver it back via
    /// [`ClientStoreHandle::apply_page_response`] correlated by `id`.
    PageHistory {
        id: MsgId,
        session_id: String,
        through_seq: u64,
    },
    /// §E.3 Q-face fetch: send `GetConversationInfo` and route the Response
    /// back by MsgId.
    ConversationInfo { id: MsgId, session_id: String },
    /// Cross-domain #5: ask the multiplexer for a `ListThreads` refetch —
    /// the leaf's materialization edge made a deferred session list-visible
    /// (its file landed). The kernel rescan trigger retired; the server
    /// self-holds the scan inside its answer.
    RefreshList,
}

/// Why the leaf stopped reopening the follow stream (§二.3 budget
/// exhausted). The cause is a typed value, not log prose, so the notice can
/// name a specific cause once one is observable: a variant is added when
/// the server starts distinguishing them (e.g. another instance holding
/// this session's write lease, dspo/manox#811) and the notice's only change
/// is a new copy key. The leaf never infers a cause from wire error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowStopReason {
    /// Every reopen since the last good snapshot ended in `Failure` /
    /// `Resync` — repeated stream failure, cause unnamed.
    StreamFailing,
}

impl FollowStopReason {
    /// The Fluent key of this reason's broadcast copy (the dismissible
    /// banner). The strings live in the locale resources, keyed per reason;
    /// copy must not hardcode a cause the client cannot observe.
    pub fn notice_key(self) -> &'static str {
        match self {
            Self::StreamFailing => "follow-stop-stream-failing",
        }
    }

    /// The Fluent key of this reason's persistent-projection copy (the
    /// always-visible footer chip). Split from the broadcast key because
    /// the chip is a compact status, not a sentence; a new reason variant
    /// adds exactly one new key here too.
    pub fn indicator_key(self) -> &'static str {
        match self {
            Self::StreamFailing => "follow-stop-indicator-stream-failing",
        }
    }
}

/// The leaf's stopped-follow surface: why following ended, and whether the
/// user dismissed this session's notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowStop {
    pub reason: FollowStopReason,
    pub dismissed: bool,
}

/// A gpui entity that owns a single session's [`ClientStore`], the v2
/// [`JournalFold`] engine, and re-emits the live fold as `ThreadEvent`s. The
/// multiplexer is the sole writer (retained `ServerNote`s) and the sole
/// stream router (v2) — with one sanctioned exception: the workspace's
/// optimistic permission-mode mirror write (the chip moves on click; the
/// journal echo remains authoritative and overwrites it later).
pub struct ClientStoreHandle {
    pub store: ClientStore,
    fold: JournalFold,
    session_id: String,
    outbound: Option<async_channel::Sender<LeafRequest>>,
    /// The MsgId of the `PageHistory` request currently in flight (so a late
    /// reply after a reset is dropped).
    pending_page: Option<MsgId>,
    /// In-flight `GetConversationInfo` correlation id (§E.3 Q face).
    pending_info: Option<MsgId>,
    /// Message-row count at the last info fetch (the committed edge).
    pub info_committed: usize,
    /// The materialization edge fired once (sidebar refresh, see
    /// [`Self::apply_change`]).
    materialized_notified: bool,
    /// The pending debounced Q-face fetch (§E.3); replacing it cancels the
    /// previous one, coalescing a committed burst into a trailing request.
    info_debounce: Option<Task<()>>,
    /// Client-owned focus (§F.2/GW5): while this leaf's session is the
    /// attached one, an `unread` rise is suppressed — the user is watching
    /// it. Driven by [`Self::set_active`] (the multiplexer owns the
    /// transitions); activation also runs the focus clear.
    active: bool,
    /// §二.3: consecutive Failure/Resync reopens since the last good
    /// snapshot. Budgeted with an exponential backoff; past the cap the
    /// leaf stops reopening (the view keeps its last window) instead of
    /// spinning forever against a stream that keeps failing.
    pub reopen_attempts: u32,
    /// §二.3: `Some` once the reopen budget is exhausted — the stop the
    /// user must see. Per-session by construction: the leaf IS the session
    /// and the notice lives with it, so a switch to another thread (a fresh
    /// leaf) starts with a fresh, un-dismissed notice.
    follow_stop: Option<FollowStop>,
}

impl EventEmitter<ThreadEvent> for ClientStoreHandle {}

impl ClientStoreHandle {
    /// A fresh leaf for `session_id`. The store's `id` is set when the
    /// `SessionCreated` note (routed by the multiplexer) lands.
    pub fn leaf(session_id: &str, _cx: &mut Context<Self>) -> Self {
        Self {
            // The leaf exists for exactly this session (L11: the thread id IS
            // the session id), so the mirror carries it from construction —
            // binding only on the v1 `SessionCreated` note left opened
            // sessions with an empty `store.id`, and every same-frame
            // read-back (attach's sidebar selection) saw "".
            store: ClientStore {
                id: manox_agent::ThreadId(session_id.to_string()),
                ..ClientStore::default()
            },
            fold: JournalFold::new(),
            session_id: session_id.to_string(),
            outbound: None,
            pending_page: None,
            pending_info: None,
            info_committed: 0,
            materialized_notified: false,
            info_debounce: None,
            active: false,
            reopen_attempts: 0,
            follow_stop: None,
        }
    }

    /// Wire the leaf's outbound channel to the multiplexer (the multiplexer
    /// calls this when it creates the leaf so the v2 re-open / page fetch can
    /// ride the shared connection).
    pub fn set_outbound(&mut self, outbound: async_channel::Sender<LeafRequest>) {
        self.outbound = Some(outbound);
    }

    /// The sanctioned sole-writer exception: the workspace mirrors a mode
    /// switch onto the store immediately so the chip reflects the click
    /// without waiting for the journal echo.
    pub fn set_permission_mode_optimistic(&mut self, mode: manox_agent::thread::PermissionMode) {
        self.store.set_permission_mode_optimistic(mode);
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Apply one routed `FromServer` frame.
    pub fn apply_from_server(&mut self, msg: FromServer, cx: &mut Context<Self>) {
        match msg {
            FromServer::Notification { note } => {
                self.store.apply_server_note(&note);
                // PR-4: a delivery settled on another owner retires THIS leaf's
                // card against the matching auth id (the multiplexer fans this
                // session-less note out to every leaf; only the owning leaf's
                // reverse-lookup hits).
                if let manox_protocol::ServerNote::DeliveryCancelled { delivery_id } = &note {
                    self.store.handle_delivery_cancelled(delivery_id);
                }
                if let Some(ev) = server_note_to_thread_event(&note) {
                    cx.emit(ev);
                }
                cx.notify();
            }
            FromServer::Request { id, call } => {
                self.apply_server_request(id, &call, cx);
            }
            FromServer::Response { .. } => {
                // Responses are correlated at the multiplexer and delivered
                // via `apply_page_response`; a raw Response reaching the leaf
                // means no page was awaited — drop (readiness derives from
                // push delivery, as in v1).
            }
            FromServer::StreamItem { frame, .. } => self.apply_stream_frame(frame, cx),
            FromServer::StreamEnd { reason, .. } => self.apply_stream_end(reason, cx),
            // §D.5 host event routed by session: `SessionStatus` mirrors into
            // the store under the monotonic rules (the multiplexer broadcasts
            // to every leaf, including parked ones). C4a: the control events
            // (`SessionCreated`/`SessionDisposed`/`Error`) normalize into the
            // note path here — the host frames are the authority face, so
            // C4b can retire the wire notes with zero behavior change (the
            // former webui onHostEvent pattern (that host is gone — round 3 §二.10④);
            // GW1 dual-emit makes the double
            // processing idempotent during the window).
            FromServer::Host { host } => {
                use manox_protocol::stream::HostEvent;
                match host {
                    HostEvent::SessionStatus {
                        session_id,
                        running,
                        errored,
                        unread,
                        pending_auth,
                        pending_plan,
                        background_work,
                    } if session_id == self.session_id => {
                        // GW5: a focused session never lights up — the unread
                        // rise is suppressed client-side (the deleted webui host did the same: its
                        // mirror gates `unread === true && !active`).
                        let unread = if self.active { None } else { unread };
                        self.store.apply_session_status(
                            running,
                            errored,
                            unread,
                            pending_auth,
                            pending_plan,
                            background_work,
                        );
                        cx.notify();
                    }
                    HostEvent::SessionCreated { session_id, .. }
                        if session_id == self.session_id =>
                    {
                        self.store
                            .apply_server_note(&manox_protocol::ServerNote::SessionCreated {
                                session_id: session_id.clone(),
                            });
                        cx.notify();
                    }
                    HostEvent::SessionDisposed {
                        session_id,
                        successor,
                    } if session_id == self.session_id => {
                        self.store.apply_server_note(
                            &manox_protocol::ServerNote::SessionDisposed {
                                session_id: session_id.clone(),
                            },
                        );
                        // Identity hand-off: remember the successor so the
                        // workspace can move its foreground onto it.
                        self.store.replaced_by = successor.clone();
                        cx.notify();
                    }
                    HostEvent::Error {
                        session_id: Some(session_id),
                        message,
                    } if session_id == self.session_id => {
                        let note = manox_protocol::ServerNote::Error {
                            session_id: Some(session_id.clone()),
                            message: message.clone(),
                        };
                        self.store.apply_server_note(&note);
                        if let Some(ev) = server_note_to_thread_event(&note) {
                            cx.emit(ev);
                        }
                        cx.notify();
                    }
                    _ => {}
                }
            }
        }
    }

    /// A v2 follow-stream frame: Snapshot opens/replaces the window; Entry
    /// feeds the live journal; Projections merge the P-face. Terminal PTY
    /// bytes are a different stream kind and never route here.
    fn apply_stream_frame(&mut self, frame: StreamFrame, cx: &mut Context<Self>) {
        let outs = match frame {
            StreamFrame::Snapshot(snap) => {
                if snap.session_id != self.session_id {
                    return;
                }
                // A good snapshot lands: the reopen budget resets and the
                // stop notice is withdrawn (§二.3).
                self.reopen_attempts = 0;
                self.follow_stop = None;
                let outs = self.fold.snapshot(snap.cursor, snap.records.clone());
                // The snapshot's full projection baseline (§D.1) seeds the
                // P-face; merge it before folding so materialized fields are
                // current for the rebuild.
                self.store
                    .merge_projection_baseline(&snap.projections, snap.projections_as_of_seq);
                outs
            }
            StreamFrame::Entry {
                seq,
                id,
                parent_id,
                timestamp,
                event,
            } => self.fold.entry(manox_protocol::JournalWireEntry {
                seq,
                id,
                parent_id,
                timestamp,
                event,
            }),
            StreamFrame::Projections(frame) => {
                if frame.session_id != self.session_id {
                    return;
                }
                self.store.merge_projections(&frame);
                cx.notify();
                return;
            }
            StreamFrame::TerminalOutput { .. } => return,
        };
        self.handle_fold_outs(outs, cx);
    }

    fn apply_stream_end(
        &mut self,
        reason: manox_protocol::StreamEndReason,
        cx: &mut Context<Self>,
    ) {
        match reason {
            manox_protocol::StreamEndReason::Resync => self.request_reopen(cx),
            // Cancelled/Closed: the multiplexer is tearing the stream down on
            // our behalf (detach/dispose) — no seamless re-open.
            manox_protocol::StreamEndReason::Cancelled
            | manox_protocol::StreamEndReason::Closed => {}
            manox_protocol::StreamEndReason::Failure { code, message } => {
                tracing::warn!(code = %code, message = %message, "follow stream failed; re-opening");
                self.request_reopen(cx);
            }
        }
    }

    fn handle_fold_outs(&mut self, outs: Vec<FoldOut>, cx: &mut Context<Self>) {
        for out in outs {
            match out {
                FoldOut::Change(change) => self.apply_change(change, cx),
                FoldOut::NeedPage(req) => {
                    let Some(outbound) = self.outbound.clone() else {
                        // No multiplexer wired (unit test path): the window
                        // is already gap-free; drop the request.
                        continue;
                    };
                    let id = MsgId::new(format!("page-{}-{}", self.session_id, req.through_seq));
                    self.pending_page = Some(id.clone());
                    let _ = outbound.try_send(LeafRequest::PageHistory {
                        id,
                        session_id: self.session_id.clone(),
                        through_seq: req.through_seq,
                    });
                }
                FoldOut::Resync => {
                    // Seamless reconnect: re-open the follow stream (the next
                    // frame is a fresh Snapshot at a cursor >= our tail, the
                    // engine keeps the old window until it lands).
                    self.request_reopen(cx);
                }
            }
        }
    }

    fn apply_change(&mut self, change: WindowChange, cx: &mut Context<Self>) {
        // T10c (§K.5 closeout): the v2 fold is the sole render source.
        // Structural window changes (the snapshot `Replace` and gap-repair
        // `Prepend`) re-arm the conversation rebuild — the §C.2-era
        // authoritative-history-boundary role of the deleted `ThreadHistory`
        // note, now triggered off the §D.1 snapshot itself. Appends emit
        // their per-entry live events below (the same rows feed the store's
        // `display` fold first).
        let structural = matches!(
            &change,
            WindowChange::Replace { .. } | WindowChange::Prepend { .. }
        );
        let live_events = match &change {
            WindowChange::Append(entry) => vec![entry.clone()],
            _ => Vec::new(),
        };
        self.store.apply_window_change(change);
        // The session file materializes on the FIRST assistant message (the
        // deferred-first-assistant contract) — before that boundary a fresh
        // thread has no file for the sidebar scan to list, so the submit-time
        // refresh misses it and the running conversation is invisible (the
        // round-11 "this thread never appeared in the list" repro). The first
        // assistant row landing in the window IS the materialization edge:
        // refresh the thread list exactly once per session.
        if !self.materialized_notified
            && live_events.iter().any(|e| {
                matches!(
                    &e.event,
                    manox_protocol::JournalWireEvent::Message { role, .. } if role == "assistant"
                )
            })
        {
            self.materialized_notified = true;
            // Cross-domain #5: the deferred session just became
            // list-visible — ask the multiplexer for a wire refetch (the
            // server self-holds the rescan); the kernel trigger retired.
            if let Some(outbound) = self.outbound.clone() {
                let _ = outbound.try_send(LeafRequest::RefreshList);
            }
        }
        // §E.3 Q face: a message row landing in the window is the committed
        // edge — refresh the usage panel (per-turn frequency; the wire usage
        // rows themselves ride the transcript). The counter is maintained
        // incrementally (U7): an Append contributes exactly its own row, so
        // the streaming hot path is O(1) per frame; only structural window
        // changes (snapshot Replace, gap-repair merge, Prepend) recount the
        // fresh window. The former full-window scan ran on every delta
        // frame — O(window) per frame, O(n²) across a session.
        let committed = if structural {
            self.store
                .window
                .iter()
                .filter(|e| matches!(&e.event, manox_protocol::JournalWireEvent::Message { .. }))
                .count()
        } else {
            self.info_committed
                + live_events
                    .iter()
                    .filter(|e| {
                        matches!(&e.event, manox_protocol::JournalWireEvent::Message { .. })
                    })
                    .count()
        };
        if committed != self.info_committed {
            self.info_committed = committed;
            self.schedule_info_fetch(cx);
        }
        if self.store.stream_drives_render {
            if structural {
                cx.emit(ThreadEvent::HistoryRestored);
            }
            for entry in live_events {
                if let Some(ev) = journal_translate::thread_event_of(&entry) {
                    cx.emit(ev);
                }
            }
        }
        cx.notify();
    }

    /// §E.3 debounce: schedule (or replace) the trailing Q-face fetch.
    /// Dropping the previous `Task` cancels it, so a burst of committed
    /// edges coalesces into one request keyed by the latest count. The
    /// timer rides the gpui background executor — never tokio time on the
    /// gpui thread.
    fn schedule_info_fetch(&mut self, cx: &mut Context<Self>) {
        self.info_debounce = Some(cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            cx.background_executor().timer(INFO_DEBOUNCE).await;
            let _ = this.update(cx, |h, _| h.fire_info_fetch());
        }));
    }

    /// Send the Q-face request for the current committed count (the
    /// debounce trailing edge).
    fn fire_info_fetch(&mut self) {
        let Some(outbound) = self.outbound.clone() else {
            return;
        };
        let id = MsgId::new(format!("info-{}-{}", self.session_id, self.info_committed));
        self.pending_info = Some(id.clone());
        let _ = outbound.try_send(LeafRequest::ConversationInfo {
            id,
            session_id: self.session_id.clone(),
        });
    }

    /// Client-side focus transition (§F.2/GW5). Activating clears the
    /// monotonic unread/errored mirrors (the user is now watching); the
    /// return reports whether anything cleared (badge refresh).
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) -> bool {
        self.active = active;
        if active {
            let cleared = self.store.focus_cleared();
            if cleared {
                cx.notify();
            }
            return cleared;
        }
        false
    }

    /// Raise the client-owned unread mirror from local knowledge (GW5):
    /// parked-thread facts the server deltas do not carry (a parked error,
    /// a background-task update). Suppressed while active — the user is
    /// watching, so nothing is "unread".
    pub fn note_local_unread(&mut self, cx: &mut Context<Self>) {
        if self.active {
            return;
        }
        self.store
            .apply_session_status(None, None, Some(true), None, None, None);
        cx.notify();
    }

    /// The stop-notice surface for the session chrome: `Some` once the
    /// §二.3 reopen budget is exhausted (the view keeps its last window and
    /// no longer updates), `None` while following or reopening under
    /// backoff.
    pub fn follow_stop(&self) -> Option<FollowStop> {
        self.follow_stop
    }

    /// Silence this session's stop notice. Automatic paths never re-show it
    /// (the terminal budget arm keeps the dismissal); the notice returns
    /// only after the stream resumes, or after a manual retry dies again.
    pub fn dismiss_follow_stop(&mut self, cx: &mut Context<Self>) {
        if let Some(stop) = self.follow_stop.as_mut() {
            stop.dismissed = true;
            cx.notify();
        }
    }

    /// The notice's retry entry: re-arm the §二.3 budget and ask the
    /// multiplexer for a full re-attach (`OpenSession` + `StreamOpen`), so a
    /// retry genuinely recovers the attach's OpenSession having failed once —
    /// the failure the stop most often stands for. With a multiplexer wired
    /// the notice drops immediately, and a fresh exhaustion re-raises it
    /// undismissed; with none wired there is nothing to re-open, so this is a
    /// no-op and the notice it cannot act on stays — a retry is a user action
    /// whose outcome must be visible either way.
    pub fn retry_follow(&mut self, cx: &mut Context<Self>) {
        // A retry is a user action, so its outcome has to be visible: with no
        // multiplexer wired there is nothing to re-open, and dropping the
        // notice here would erase the only signal the view ever shows.
        if self.outbound.is_none() {
            return;
        }
        self.reopen_attempts = 0;
        self.follow_stop = None;
        self.request_reopen_inner(cx, true);
    }

    /// Deliver a `PageHistory` response the leaf requested. Correlated by
    /// MsgId; a late/foreign reply is dropped.
    pub fn apply_page_response(
        &mut self,
        id: MsgId,
        outcome: Result<serde_json::Value, RpcError>,
        cx: &mut Context<Self>,
    ) {
        if self.pending_page.as_ref() != Some(&id) {
            return;
        }
        self.pending_page = None;
        let records = match outcome {
            // Round 4 §3.5: the server answered but the records failed to
            // parse — feeding the gap repair an EMPTY page would keep the
            // window short forever. The degraded empty page stays (the
            // repair path reports the shortfall loudly), but never
            // without a trace.
            Ok(v) => v
                .get("records")
                .and_then(|r| {
                    serde_json::from_value::<Vec<manox_protocol::JournalWireEntry>>(r.clone())
                    .map_err(|e| {
                        tracing::warn!(
                            error = %e,
                            "PageHistory records failed to parse; feeding the repair an empty page"
                        );
                    })
                    .ok()
                })
                .unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, "PageHistory failed; re-opening follow stream");
                self.request_reopen(cx);
                return;
            }
        };
        let outs = self.fold.deliver_page(records);
        self.handle_fold_outs(outs, cx);
    }

    /// Deliver a `GetConversationInfo` response (§E.3) the leaf requested.
    /// Correlated by MsgId; a late/foreign reply is dropped.
    pub fn apply_conversation_info_response(
        &mut self,
        id: MsgId,
        outcome: Result<serde_json::Value, manox_protocol::RpcError>,
        cx: &mut Context<Self>,
    ) {
        if self.pending_info.as_ref() != Some(&id) {
            return;
        }
        self.pending_info = None;
        if let Ok(payload) = outcome {
            self.store.apply_conversation_info(&payload);
            cx.notify();
        }
    }

    /// §二.3: the reopen budget policy — exponential backoff (500ms, 1s,
    /// 2s, 4s, 8s), `None` past the cap (terminal: stop reopening, the
    /// view keeps its last window). Pure so the policy is testable
    /// without the gpui timer plumbing.
    pub fn reopen_backoff(attempt: u32) -> Option<std::time::Duration> {
        const CAP: u32 = 5;
        if attempt == 0 || attempt > CAP {
            return None;
        }
        Some(std::time::Duration::from_millis(500 << (attempt - 1)))
    }

    fn request_reopen(&mut self, cx: &mut Context<Self>) {
        self.request_reopen_inner(cx, false);
    }

    /// `reattach` marks the USER-facing retry: the multiplexer re-sends
    /// `OpenSession` ahead of the `StreamOpen` so the retry also recovers the
    /// attach's OpenSession having failed once. Automatic reopens stay pure
    /// `StreamOpen` — a bare OpenSession on an already-superseded id would
    /// insert a ghost `ServerSession` under the old id (a leaked session, an
    /// idle pump, and a second engine on the successor's journal), and only
    /// the banner/chip retry is user-visible on a live foreground id.
    fn request_reopen_inner(&mut self, cx: &mut Context<Self>, reattach: bool) {
        self.reopen_attempts += 1;
        let Some(delay) = Self::reopen_backoff(self.reopen_attempts) else {
            // Terminal: a stream that keeps failing (corrupt journal, an
            // engine that never materializes past the server-side
            // deadline) must not spin reopens forever. The fold keeps the
            // last good window; a manual thread switch builds a fresh
            // leaf and retries from zero. The stop is user-facing, never
            // just a log line: the session chrome renders a notice from
            // this state. A re-exhaustion keeps an existing dismissal so
            // no automatic path can pop the banner back.
            self.follow_stop = Some(FollowStop {
                reason: FollowStopReason::StreamFailing,
                dismissed: self.follow_stop.is_some_and(|stop| stop.dismissed),
            });
            tracing::error!(
                session = %self.session_id,
                attempts = self.reopen_attempts,
                "follow-stream reopen budget exhausted; the view stays on its last window"
            );
            cx.notify();
            return;
        };
        self.fold.generation();
        let Some(outbound) = self.outbound.clone() else {
            return;
        };
        let stream_id = StreamId::new(uuid::Uuid::new_v4().to_string());
        let session_id = self.session_id.clone();
        if delay.is_zero() {
            let _ = outbound.try_send(LeafRequest::Reopen {
                session_id,
                stream_id,
                reattach,
            });
        } else {
            // Backoff on the background executor (the info_debounce
            // pattern); the reopen send needs no entity handle.
            cx.spawn(async move |_this, cx: &mut gpui::AsyncApp| {
                cx.background_executor().timer(delay).await;
                let _ = outbound.try_send(LeafRequest::Reopen {
                    session_id,
                    stream_id,
                    reattach,
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Adjudication `ServerCall`: record the MsgId for the reply path (the
    /// workspace answers against it) and re-emit the card as a `ThreadEvent`.
    /// The waterfall fan-out (spec T6-5): the deterministic MsgId is
    /// `auth_id`/`session_id`, so the SAME auth id can be delivered to this
    /// leaf more than once across re-connections; each delivery is recorded
    /// independently (the last wins for the reply path) and re-emitted so the
    /// card surfaces without depending on the v1 note accumulation.
    fn apply_server_request(
        &mut self,
        id: MsgId,
        call: &manox_protocol::ServerCall,
        cx: &mut Context<Self>,
    ) {
        if let Some(delivery_id) = delivery_id_of(call) {
            // GW3 capture: the withdrawal identity, keyed by auth id like the
            // Reply correlation below (a `CancelDelivery` withdraws it, and a
            // PR-4 `DeliveryCancelled` note retires the card against it).
            if let Some(auth_id) = auth_id_of(call) {
                self.store
                    .pending_auth_delivery
                    .insert(auth_id, delivery_id);
            }
        }
        if let Some(auth_id) = auth_id_of(call) {
            self.store.pending_auth.insert(auth_id, id.clone());
            cx.notify();
        }
        if let Some(ev) = server_call_to_thread_event(call) {
            cx.emit(ev);
        }
    }
}

/// Extract the `auth_id` from an adjudication ServerCall (Approve/AskUser).
fn auth_id_of(call: &manox_protocol::ServerCall) -> Option<String> {
    use manox_protocol::ServerCall;
    match call {
        ServerCall::Approve { auth_id, .. } | ServerCall::AskUserQuestion { auth_id, .. } => {
            Some(auth_id.clone())
        }
        _ => None,
    }
}

/// The GW3 delivery identity of an adjudication ServerCall (the variants
/// carry it at the variant level; the same fan-out id reaches every
/// reviewer, and any holder's `CancelDelivery` converges the waterfall).
fn delivery_id_of(call: &manox_protocol::ServerCall) -> Option<String> {
    use manox_protocol::ServerCall;
    match call {
        ServerCall::Approve { delivery_id, .. }
        | ServerCall::AskUserQuestion { delivery_id, .. }
        | ServerCall::InvokeClientTool { delivery_id, .. } => Some(delivery_id.clone()),
        _ => None,
    }
}
