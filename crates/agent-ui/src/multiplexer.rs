//! Single shared client multiplexer for the gpui desktop app (v3).
//!
//! One [`AhpStore`] carries every session over the host's in-proc leg; the
//! store's own pump folds all channels (see `ahp_store`). What remains here
//! is the *lifecycle* the views drive: which sessions are attached
//! (subscribed), which one is focused (GW5 unread suppression — the official
//! bit now), and the create/fork command seams. The sidebar and model
//! surfaces read the store's book through the accessors below — the protocol
//! state has exactly one home.

use std::path::PathBuf;

use crate::sidebar_projection::ThreadRow;
use ahp_types::state::AgentInfo;
use gpui::{App, Context, Entity};
use steer_agent_chat_ui::ahp_store::{
    AhpStore, CLIENT_ID, chat_uri, plan_uri, session_uri, thread_uri, work_uri,
};

/// The per-app multiplexer: store handle plus attach/focus bookkeeping.
pub struct SessionMultiplexer {
    pub(crate) store: Entity<AhpStore>,
    /// The cwd new sessions are granted (the window's workspace root).
    cwd: PathBuf,
    /// Attached (subscribed) session ids, most recent attach last.
    attached: Vec<String>,
    /// The client-owned focus (GW5): the attached session's unread rises are
    /// suppressed while it is the foreground leaf.
    focused: Option<String>,
}

/// The ONE sidebar row pin read (write faces included — `on_pin` flips
/// against this, never against the fold alone): the subscribed thread
/// channel's fold wins once it speaks (live, backed by the thread-scoped
/// baseline); a row whose channel has not spoken yet falls back to the list
/// snapshot's `_meta.x-manox.pinned` (upstream #863) — the store row's pin
/// authority reached the client with the list itself.
pub(crate) fn ext_and_meta_pinned(
    book: &steer_agent_chat_ui::ahp_store::ChannelBook,
    summary: Option<&ahp_types::state::SessionSummary>,
    thread_channel: &str,
) -> bool {
    book.ext
        .get(thread_channel)
        .and_then(|x| x.pinned)
        .or_else(|| {
            summary.and_then(|summary| {
                summary
                    .meta
                    .as_ref()
                    .and_then(|m| m.get("x-manox"))
                    .and_then(|x| x.get("pinned"))
                    .and_then(serde_json::Value::as_bool)
            })
        })
        .unwrap_or(false)
}

/// The ONE sidebar row archived read, live-fold-first: the
/// `session/isArchivedChanged` echo on the session channel is the instant
/// fact, the list snapshot's status bit the fallback. A zero-status fold
/// entry is the pre-snapshot placeholder and carries no archive fact — it
/// defers to the summary (the same guard `toggle_archived` flips against).
pub(crate) fn effective_archived(
    book: &steer_agent_chat_ui::ahp_store::ChannelBook,
    session_id: &str,
    summary: &ahp_types::state::SessionSummary,
) -> bool {
    match book.sessions.get(session_id) {
        Some(state) if state.status != 0 => {
            state.status & ahp_types::state::SessionStatus::IsArchived.bits() != 0
        }
        _ => summary.status & ahp_types::state::SessionStatus::IsArchived.bits() != 0,
    }
}

impl SessionMultiplexer {
    /// Bind the multiplexer to the connected store.
    pub fn new(store: Entity<AhpStore>, cwd: PathBuf) -> Self {
        Self {
            store,
            cwd,
            attached: Vec::new(),
            focused: None,
        }
    }

    /// The store every view reads.
    pub fn store(&self) -> Entity<AhpStore> {
        self.store.clone()
    }

    /// Attach (subscribe) a session. Idempotent: a second call for a live
    /// session only refreshes its catalogue row. The session and its default
    /// chat channel are subscribed; the chat's turns stream from there. The
    /// plan channel rides along: its baseline is the only fold-visible record
    /// of a plan review's settlement (the verdict lands after the turn has
    /// archived, and the chat-level part can then never fold answered), and
    /// without the subscription the live-ask edge cannot tell a settled
    /// review from an open one — the #88 composer lock. The thread channel
    /// rides for the row state (pin/label/leaf — upstream #842 moved the rows
    /// there and nothing on the session channel ever folded them), and the
    /// work channel for the browser-suite surface.
    pub fn open_or_create(&mut self, session_id: &str, _reopen: bool, cx: &mut Context<Self>) {
        if self.attached.iter().any(|id| id == session_id) {
            return;
        }
        self.attached.push(session_id.to_string());
        let store = self.store.clone();
        let sid = session_id.to_string();
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            store.update(cx, |store, cx| {
                store.subscribe(session_uri(&sid), cx);
                // The chat channel rides the session's default-chat pointer;
                // subscribing by session id too is tolerated by the host and
                // covers the window before the pointer lands.
                store.subscribe(chat_uri(&sid), cx);
                store.subscribe(plan_uri(&sid), cx);
                store.subscribe(thread_uri(&sid), cx);
                store.subscribe(work_uri(&sid), cx);
                store.claim_active_client(&sid);
            });
            let _ = this.update(cx, |_, _| {});
        })
        .detach();
    }

    /// Create a session over a client-minted id (the idempotency key), then
    /// attach it. `reply` resolves when the host answered the create.
    pub fn create_session(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) -> steer_agent_chat_ui::ahp_store::Reply {
        // The create is queued when the handshake is still in flight and
        // resolved by the replay in order (before this session's subscribes).
        let reply = self.store.update(cx, |store, _| {
            store.create_session(
                session_id,
                vec![format!("file://{}", self.cwd.display())],
                None,
            )
        });
        self.open_or_create(session_id, false, cx);
        reply
    }

    /// Detach a session — the park leg of a thread switch. Channels STAY
    /// subscribed (only the bookkeeping above parks): the official unread
    /// bit rides `session/isReadChanged` echoes on the session channel, so
    /// a parked thread keeps its live dot and a re-attach re-seeds an open
    /// ask card — the protocol's own answer where the local unread map used
    /// to be.
    pub fn forget(&mut self, session_id: &str) {
        self.attached.retain(|id| id != session_id);
        if self.focused.as_deref() == Some(session_id) {
            self.focused = None;
        }
    }

    /// The client-owned focus (GW5): the store publishes it (the official
    /// unread rise suppresses for the foreground) and clears the focused
    /// session's bit through the protocol — the echo lands for every
    /// subscriber, this one included.
    pub fn set_focused(&mut self, session_id: Option<&str>, cx: &mut Context<Self>) {
        // Publish only on a real transition: the store entity outlives
        // switches (chat.store drops a handle, not the store), so a no-op
        // focus call has nothing to re-publish — and every real change
        // (fresh multiplexer, switch, post-forget re-focus) moves
        // `focused` here.
        if self.focused.as_deref() != session_id {
            self.focused = session_id.map(str::to_string);
            self.store.update(cx, |store, _| {
                store.set_focused_session(session_id);
            });
            cx.notify();
        }
    }

    /// Flip a session's archived bit through the protocol
    /// (`session/isArchivedChanged`): the host journals and persists the
    /// decision (the store row, the sidecar) and echoes the action back —
    /// the summary's IsArchived bit is the single state every client reads.
    pub fn toggle_archived(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let view = self.store.read(cx);
        // The live fold outranks the summary row — except a zero-status fold
        // entry (the pre-snapshot empty_session placeholder), which carries
        // no archive fact and defers to the summary. Neither source knowing
        // the id refuses the flip: callers pass live rows, and a dispatch
        // into the void would only land in the diagnostics deque.
        let archived = match view.book.sessions.get(session_id) {
            Some(state) if state.status != 0 => {
                Some(state.status & ahp_types::state::SessionStatus::IsArchived.bits() != 0)
            }
            _ => None,
        }
        .or_else(|| {
            view.book
                .summaries
                .values()
                .find(|summary| {
                    steer_agent_chat_ui::ahp_store::id_of(&summary.resource) == session_id
                })
                .map(|summary| {
                    summary.status & ahp_types::state::SessionStatus::IsArchived.bits() != 0
                })
        });
        let Some(archived) = archived else {
            return;
        };
        self.store.update(cx, |store, _| {
            store.dispatch(
                steer_agent_chat_ui::ahp_store::session_uri(session_id),
                ahp_types::actions::StateAction::SessionIsArchivedChanged(
                    ahp_types::actions::SessionIsArchivedChangedAction {
                        is_archived: !archived,
                    },
                ),
            );
        });
    }

    /// The sidebar attention count (GW5, the dock badge source): the
    /// OFFICIAL unread bits (live fold first, the summary row as the parked
    /// fallback) — the same protocol state the sidebar dots read, no local
    /// map behind it.
    pub fn attention_count(&self, cx: &App) -> usize {
        self.store.read(cx).unread_sessions()
    }

    /// Whether a session is currently attached.
    pub fn is_attached(&self, session_id: &str) -> bool {
        self.attached.iter().any(|id| id == session_id)
    }

    /// Attached session ids, oldest first.
    pub fn attached_ids(&self) -> &[String] {
        &self.attached
    }

    // ── catalogue accessors (the sidebar/model/command faces) ──────────

    /// The sidebar catalogue as projected rows, most recently modified
    /// first (the host's list order). Archived rows are excluded: the host
    /// partitions archived rows out of its own `summaries()`, but the client's
    /// catalogue seed only upserts — a row that ever entered `book.summaries`
    /// would otherwise resurface on every re-projection. The row's archived
    /// bit is read live-fold-first (the pin-read pattern), so the
    /// `session/isArchivedChanged` echo drops it from the sidebar the moment
    /// it lands, ahead of any list refresh.
    pub fn thread_list(&self, cx: &App) -> Vec<ThreadRow> {
        let focused_sid = self.focused.clone();
        let view = self.store.read(cx);
        let book = &view.book;
        let mut rows: Vec<ThreadRow> = book
            .summaries
            .values()
            .filter_map(|summary| {
                let sid = steer_agent_chat_ui::ahp_store::id_of(&summary.resource);
                // The sidebar is the ACTIVE list: an archived row leaves it
                // (see `effective_archived` for the read and the doc on this
                // method for why the filter lives here).
                if effective_archived(book, sid, summary) {
                    return None;
                }
                // A blank row stays out of the list: an attached session
                // whose landed fold carries no turns has never been used.
                // The focused row always shows (a landing thread the user
                // is typing into must be reachable), and a fold that has
                // not landed yet is never judged.
                if focused_sid.as_deref() != Some(sid)
                    && book.chat_snapshot_landed(sid)
                    && crate::ahp_store::leaf(book, sid)
                        .chat
                        .is_some_and(|c| c.turns.is_empty() && c.active_turn.is_none())
                {
                    return None;
                }
                // The row pin read: see `ext_and_meta_pinned`.
                let pinned = ext_and_meta_pinned(book, Some(summary), &thread_uri(sid));
                let pending_plan = steer_agent_chat_ui::ahp_store::plan_review_proposed(book, sid);
                let mut row = ThreadRow::from_summary(summary, pinned, pending_plan);
                // The live fold wins over the summary row for the unread
                // bit (the pin-read pattern): the official
                // `session/isReadChanged` echoes land on the session state
                // the moment they happen, while the summary row trails the
                // host's next list refresh.
                if let Some(state) = book.sessions.get(sid) {
                    row.unread = state.status & ahp_types::state::SessionStatus::IsRead.bits() == 0;
                }
                // The host's project field trails a brand-new session (its
                // store row lands with the first persistence), but the fold's
                // effective cwd is already live — and it is exactly what the
                // composer's project chip shows. Group by it so the row never
                // falls into "Chats" beside a chip naming a project.
                if row.project.is_none()
                    && let Some(cwd) = crate::ahp_store::leaf(book, sid).cwd()
                {
                    row.project = Some(cwd);
                }
                Some(row)
            })
            .collect();
        rows.sort_by(|a, b| {
            let ta = book
                .summaries
                .get(&a.id)
                .map(|s| s.modified_at.as_str())
                .unwrap_or("");
            let tb = book
                .summaries
                .get(&b.id)
                .map(|s| s.modified_at.as_str())
                .unwrap_or("");
            tb.cmp(ta)
        });
        rows
    }

    /// The root agents catalogue: one row per provider with its models.
    pub fn agents(&self, cx: &App) -> Vec<AgentInfo> {
        self.store.read(cx).book.root.agents.clone()
    }

    /// The command catalogue payload (verbatim `x-manox-commands://` state).
    pub fn commands<'a>(&self, cx: &'a App) -> Option<&'a serde_json::Value> {
        self.store
            .read(cx)
            .book
            .catalogues
            .get(manox_ahp::ext::channels::COMMANDS)
    }

    /// The workspace catalogue payload (verbatim `x-manox-workspaces://`).
    pub fn workspaces<'a>(&self, cx: &'a App) -> Option<&'a serde_json::Value> {
        self.store
            .read(cx)
            .book
            .catalogues
            .get(manox_ahp::ext::channels::WORKSPACES)
    }

    /// Refresh the sidebar catalogue from the host.
    pub fn fetch_thread_list(&mut self, cx: &mut Context<Self>) {
        self.store
            .update(cx, |store, cx| store.refresh_sessions(cx));
    }

    /// Refresh the catalogue channels (baseline pull).
    pub fn fetch_catalogues(&mut self, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| {
            for uri in [
                manox_ahp::ext::channels::WORKSPACES,
                manox_ahp::ext::channels::COMMANDS,
            ] {
                store.subscribe(uri, cx);
            }
        });
    }

    /// The client id this multiplexer claims on the host.
    pub fn client_id(&self) -> &'static str {
        CLIENT_ID
    }
}

#[cfg(test)]
mod pin_read_tests {
    use super::*;
    use steer_agent_chat_ui::ahp_store::ChannelBook;

    fn summary_with_meta(pinned: bool) -> ahp_types::state::SessionSummary {
        serde_json::from_value(serde_json::json!({
            "resource": "ahp-session:/s-1",
            "provider": "test",
            "title": "row",
            "status": 0,
            "createdAt": "2026-01-01T00:00:00Z",
            "modifiedAt": "2026-01-01T00:00:00Z",
            "_meta": { "x-manox": { "pinned": pinned } },
        }))
        .expect("a summary with _meta parses")
    }

    #[test]
    fn pin_reads_the_fold_first_then_the_list_meta() {
        let mut book = ChannelBook::default();
        let channel = "x-manox-thread:/s-1";
        let summary = summary_with_meta(true);

        // No fold yet (the row's thread baseline has not landed): the list
        // snapshot's _meta speaks.
        assert!(ext_and_meta_pinned(&book, Some(&summary), channel));

        // The subscribed fold wins once it speaks — even when it disagrees
        // with the (possibly stale) snapshot.
        book.ext.insert(
            channel.to_string(),
            manox_ahp::ext::reducer::XManoxState {
                pinned: Some(false),
                ..Default::default()
            },
        );
        assert!(!ext_and_meta_pinned(&book, Some(&summary), channel));

        // The steady state for a never-pinned row: the fold entry exists but
        // its baseline omitted `pinned` (XManoxState skips None), so the
        // fold carries no opinion and the _meta fallback stays in charge —
        // pinning the "entry exists ⇒ authoritative" shortcut.
        book.ext.insert(
            channel.to_string(),
            manox_ahp::ext::reducer::XManoxState {
                pinned: None,
                ..Default::default()
            },
        );
        assert!(
            ext_and_meta_pinned(&book, Some(&summary), channel),
            "a fold entry without a pin opinion defers to the _meta baseline"
        );

        // A row the list shipped without _meta reads unpinned, not panicked.
        let bare: ahp_types::state::SessionSummary = serde_json::from_value(serde_json::json!({
            "resource": "ahp-session:/s-2",
            "provider": "test",
            "title": "bare",
            "status": 0,
            "createdAt": "2026-01-01T00:00:00Z",
            "modifiedAt": "2026-01-01T00:00:00Z",
        }))
        .expect("a bare summary parses");
        assert!(!ext_and_meta_pinned(
            &book,
            Some(&bare),
            "x-manox-thread:/s-2"
        ));
    }

    #[test]
    fn archived_reads_the_fold_first_and_defers_the_placeholder() {
        use ahp_types::state::SessionStatus;

        fn summary(status: u32) -> ahp_types::state::SessionSummary {
            serde_json::from_value(serde_json::json!({
                "resource": "ahp-session:/s-1",
                "provider": "test",
                "title": "row",
                "status": status,
                "createdAt": "2026-01-01T00:00:00Z",
                "modifiedAt": "2026-01-01T00:00:00Z",
            }))
            .expect("a summary parses")
        }

        fn state(status: u32) -> ahp_types::state::SessionState {
            ahp_types::state::SessionState {
                provider: "test".into(),
                title: "row".into(),
                status,
                activity: None,
                origin: None,
                project: None,
                working_directories: None,
                annotations: None,
                lifecycle: ahp_types::state::SessionLifecycle::Ready,
                creation_error: None,
                server_tools: None,
                active_clients: Vec::new(),
                chats: Vec::new(),
                default_chat: None,
                config: None,
                customizations: None,
                changesets: None,
                input_needed: None,
                meta: None,
            }
        }

        let mut book = steer_agent_chat_ui::ahp_store::ChannelBook::default();
        let idle = summary(0);
        assert!(
            !effective_archived(&book, "s-1", &idle),
            "no fold entry and a clean summary reads unarchived"
        );

        // The echo landed: the fold's IsArchived bit is the instant fact.
        book.sessions
            .insert("s-1".into(), state(SessionStatus::IsArchived.bits()));
        assert!(
            effective_archived(&book, "s-1", &summary(0)),
            "the echo must drop the row ahead of any list refresh"
        );

        // A zero-status fold entry is the pre-snapshot placeholder: it
        // carries no archive fact and defers to the summary bit.
        book.sessions.insert("s-1".into(), state(0));
        assert!(!effective_archived(&book, "s-1", &summary(0)));

        // The summary bit as the fallback (a snapshot seeded before the
        // fold spoke).
        let archived_summary = summary(SessionStatus::IsArchived.bits());
        assert!(effective_archived(&book, "s-1", &archived_summary));

        // Unarchive: the cleared fold bit wins over a stale archived summary.
        book.sessions.insert(
            "s-1".into(),
            state(SessionStatus::Idle.bits() | SessionStatus::IsRead.bits()),
        );
        assert!(
            !effective_archived(&book, "s-1", &archived_summary),
            "the cleared echo bit revives the row without a refetch"
        );
    }
}
