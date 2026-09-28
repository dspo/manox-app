//! Single shared client multiplexer for the gpui desktop app (v3).
//!
//! One [`AhpStore`] carries every session over the host's in-proc leg; the
//! store's own pump folds all channels (see `ahp_store`). What remains here
//! is the *lifecycle* the views drive: which sessions are attached
//! (subscribed), which one is focused (GW5 unread suppression), the local
//! unread map, and the create/fork command seams. The sidebar and model
//! surfaces read the store's book through the accessors below — the protocol
//! state has exactly one home.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::sidebar_projection::ThreadRow;
use ahp_types::state::AgentInfo;
use gpui::{App, Context, Entity};
use manox_agent_chat_ui::ahp_store::{AhpStore, CLIENT_ID, chat_uri, session_uri};

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
    /// Local unread flags keyed by session id (GW5).
    unread: HashMap<String, bool>,
    /// Unsubscribes deferred for a Context-bearing call site.
    unsubscribe_queue: Vec<String>,
}

impl SessionMultiplexer {
    /// Bind the multiplexer to the connected store.
    pub fn new(store: Entity<AhpStore>, cwd: PathBuf) -> Self {
        Self {
            store,
            cwd,
            attached: Vec::new(),
            focused: None,
            unsubscribe_queue: Vec::new(),
            unread: HashMap::new(),
        }
    }

    /// The store every view reads.
    pub fn store(&self) -> Entity<AhpStore> {
        self.store.clone()
    }

    /// Attach (subscribe) a session. Idempotent: a second call for a live
    /// session only refreshes its catalogue row. The session and its default
    /// chat channel are subscribed; the chat's turns stream from there.
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
    ) -> manox_agent_chat_ui::ahp_store::Reply {
        let reply = {
            let store = self.store.read(cx);
            store.create_session(
                session_id,
                vec![format!("file://{}", self.cwd.display())],
                None,
            )
        };
        self.open_or_create(session_id, false, cx);
        reply
    }

    /// Drain deferred unsubscribes (call from a Context-bearing site).
    pub fn flush_unsubscribes(&mut self, cx: &mut Context<Self>) {
        let uris = std::mem::take(&mut self.unsubscribe_queue);
        for uri in uris {
            self.store.update(cx, |store, _| store.unsubscribe(uri));
        }
    }

    /// Detach (unsubscribe) a session — the park leg of a thread switch.
    pub fn forget(&mut self, session_id: &str) {
        self.attached.retain(|id| id != session_id);
        self.unread.remove(session_id);
        if self.focused.as_deref() == Some(session_id) {
            self.focused = None;
        }
        let uri = session_uri(session_id);
        self.unsubscribe_queue.push(uri);
    }

    /// The client-owned focus (GW5).
    pub fn set_focused(&mut self, session_id: Option<&str>, cx: &mut Context<Self>) {
        let changed = self.focused.as_deref() != session_id;
        self.focused = session_id.map(str::to_string);
        if let Some(id) = session_id
            && self.unread.remove(id).is_some()
        {
            cx.notify();
        }
        if changed {
            cx.notify();
        }
    }

    /// Record a local unread rise (GW5: suppressed for the focused session).
    pub fn note_unread(&mut self, session_id: &str, cx: &mut Context<Self>) {
        if self.focused.as_deref() == Some(session_id) {
            return;
        }
        if !self.unread.get(session_id).copied().unwrap_or(false) {
            self.unread.insert(session_id.to_string(), true);
            cx.notify();
        }
    }

    /// The unread map the sidebar dots read.
    pub fn unread_map(&self) -> HashMap<String, bool> {
        self.unread.clone()
    }

    /// The sidebar attention count (GW5, the dock badge source).
    pub fn attention_count(&self) -> usize {
        self.unread.values().filter(|v| **v).count()
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
    /// first (the host's list order).
    pub fn thread_list(&self, cx: &App) -> Vec<ThreadRow> {
        let view = self.store.read(cx);
        let book = &view.book;
        let mut rows: Vec<ThreadRow> = book
            .summaries
            .values()
            .map(|summary| {
                let sid = manox_agent_chat_ui::ahp_store::id_of(&summary.resource);
                let ext = book.ext.get(&session_uri(sid));
                let pinned = ext.and_then(|x| x.pinned).unwrap_or(false);
                let pending_plan = ext
                    .and_then(|x| x.plan_review.as_ref())
                    .and_then(|r| r.get("state"))
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|s| s == "proposed");
                ThreadRow::from_summary(summary, pinned, pending_plan)
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

    /// The workspace row accounting a session: the catalogue channel no
    /// longer carries per-session ownership, so there is none — callers
    /// fall back to the row's own project path.
    pub fn workspace_of_session(&self, _session_id: &str, cx: &App) -> Option<()> {
        let _ = cx;
        None
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
