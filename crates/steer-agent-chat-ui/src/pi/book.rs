//! The pi session book: every opened session's view state, folded from the
//! transport's events by pure, synchronous calls.
//!
//! Kept apart from the transport ([`super::runtime`]) and from any gpui entity
//! so the whole projection is testable without a process or a window: the
//! pump only has to translate [`StoreEvent`] into these methods.

use std::collections::BTreeMap;
use std::path::Path;

use steer_acp::RequestId;

use super::runtime::StoreEvent;
use super::session::{
    PendingRequest, PiPlan, PiRestoredAsk, PiSessionView, PiSubagentMessage, PiSubagentRun,
};

/// Every session the host has opened, keyed by pi session id.
#[derive(Debug, Default)]
pub struct PiBook {
    sessions: BTreeMap<String, PiSessionView>,
}

impl PiBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates (or resets) the view for a session that is starting.
    pub fn open(&mut self, id: &str, cwd: &Path) -> &mut PiSessionView {
        self.sessions
            .insert(id.to_owned(), PiSessionView::new(id, cwd));
        self.sessions.get_mut(id).expect("just inserted")
    }

    /// The view for `id`, when the session was opened.
    pub fn view(&self, id: &str) -> Option<&PiSessionView> {
        self.sessions.get(id)
    }

    /// The mutable view, for the store's own lifecycle writes.
    pub fn view_mut(&mut self, id: &str) -> Option<&mut PiSessionView> {
        self.sessions.get_mut(id)
    }

    /// Sets the follow-up queue depth the composer strip shows.
    pub fn set_queued(&mut self, id: &str, queued: usize) {
        if let Some(view) = self.sessions.get_mut(id) {
            view.queued = queued;
        }
    }

    /// Folds a `_zu/todos/list` (or a write's) answer.
    pub fn set_plan(&mut self, id: &str, plan: PiPlan) -> bool {
        self.sessions
            .get_mut(id)
            .is_some_and(|view| view.set_plan(plan))
    }

    /// Folds a `_zu/ask/pending` answer.
    pub fn set_restored_asks(&mut self, id: &str, asks: Vec<PiRestoredAsk>) -> bool {
        self.sessions
            .get_mut(id)
            .is_some_and(|view| view.set_restored_asks(asks))
    }

    /// Folds a `_zu/subagents/list` answer.
    pub fn set_subagents(&mut self, id: &str, runs: Vec<PiSubagentRun>) -> bool {
        self.sessions
            .get_mut(id)
            .is_some_and(|view| view.set_subagents(runs))
    }

    /// Expands (and asks the store to fetch) or collapses a run's transcript.
    pub fn toggle_subagent(&mut self, id: &str, run_id: &str) -> Option<String> {
        self.sessions
            .get_mut(id)
            .and_then(|view| view.toggle_subagent(run_id))
    }

    /// Folds a `_zu/subagents/transcript` answer.
    pub fn set_subagent_transcript(
        &mut self,
        id: &str,
        run_id: &str,
        messages: Vec<PiSubagentMessage>,
    ) -> bool {
        self.sessions
            .get_mut(id)
            .is_some_and(|view| view.set_subagent_transcript(run_id, messages))
    }

    /// Records an error without changing the lifecycle face (a config call
    /// that failed leaves the session usable).
    pub fn note_error(&mut self, id: &str, reason: &str) {
        if let Some(view) = self.sessions.get_mut(id) {
            view.error = Some(reason.to_owned());
        }
    }

    /// Every open session, in id order.
    pub fn sessions(&self) -> impl Iterator<Item = &PiSessionView> {
        self.sessions.values()
    }

    /// Whether the session is open.
    pub fn contains(&self, id: &str) -> bool {
        self.sessions.contains_key(id)
    }

    /// Drops the view (the sheet is gone; the process is closed separately).
    pub fn remove(&mut self, id: &str) -> Option<PiSessionView> {
        self.sessions.remove(id)
    }

    /// Applies one transport event. Returns whether a view changed.
    pub fn apply(&mut self, event: &StoreEvent) -> bool {
        match event {
            StoreEvent::Update { session_id, update } => self
                .sessions
                .get_mut(session_id)
                .is_some_and(|view| view.apply_update(update)),
            StoreEvent::Request {
                session_id,
                request,
            } => self
                .sessions
                .get_mut(session_id)
                .is_some_and(|view| view.apply_request(request)),
            StoreEvent::Exited { session_id, reason } => {
                if let Some(view) = self.sessions.get_mut(session_id) {
                    view.mark_dead(reason);
                    true
                } else {
                    false
                }
            }
            StoreEvent::Ready { session_id, info } => {
                if let Some(view) = self.sessions.get_mut(session_id) {
                    view.adopt_info(info);
                    view.mark_ready();
                    true
                } else {
                    false
                }
            }
            StoreEvent::Replay {
                session_id,
                updates,
            } => {
                let Some(view) = self.sessions.get_mut(session_id) else {
                    return false;
                };
                for update in updates {
                    view.transcript.apply(&update.update);
                }
                view.mark_ready();
                true
            }
        }
    }

    /// The user sent a prompt: the optimistic row plus the streaming face.
    pub fn prompt_started(&mut self, id: &str, text: &str) -> Option<String> {
        let view = self.sessions.get_mut(id)?;
        let row = view.transcript.push_optimistic_user(text);
        view.begin_turn();
        Some(row)
    }

    /// A turn settled normally (the harness answered with a stop reason).
    pub fn turn_finished(&mut self, id: &str) {
        if let Some(view) = self.sessions.get_mut(id) {
            view.finish_turn();
        }
    }

    /// A turn failed without killing the process.
    pub fn turn_failed(&mut self, id: &str, reason: &str) {
        if let Some(view) = self.sessions.get_mut(id) {
            view.mark_interrupted(reason);
        }
    }

    /// Removes the answered request and returns it, so the caller can write
    /// the JSON-RPC response the card's answer maps to.
    pub fn take_request(&mut self, id: &str, request_id: &RequestId) -> Option<PendingRequest> {
        self.sessions
            .get_mut(id)
            .and_then(|view| view.take_request(request_id))
    }

    /// The session's current program title, when pi set one.
    pub fn title(&self, id: &str) -> Option<&str> {
        self.sessions.get(id).and_then(|view| view.title.as_deref())
    }

    /// Every pending ask across sessions (`_zu/ask/create`), for restore
    /// reconciliation: `(session id, request)`.
    pub fn pending_asks(&self) -> impl Iterator<Item = (&str, &PendingRequest)> {
        self.sessions.iter().flat_map(|(id, view)| {
            view.pending_asks
                .iter()
                .map(move |request| (id.as_str(), request))
        })
    }

    /// The transcript's display rows for one session.
    pub fn rows(&self, id: &str) -> Vec<super::transcript::Entry> {
        self.sessions
            .get(id)
            .map(|view| view.transcript.rows())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use steer_acp::{HostRequest, RequestId, SessionInfo};

    use super::*;

    fn ready() -> StoreEvent {
        StoreEvent::Ready {
            session_id: "s1".to_owned(),
            info: SessionInfo {
                session_id: "s1".to_owned(),
                config_options: json!([{
                    "id": "model", "name": "Model", "type": "select",
                    "currentValue": "p/m", "options": [{"value": "p/m", "name": "M"}]
                }]),
                modes: json!({"currentModeId": "ask", "availableModes": [{"id": "ask", "name": "Ask"}]}),
                raw: json!({}),
            },
        }
    }

    fn update(value: serde_json::Value) -> StoreEvent {
        StoreEvent::Update {
            session_id: "s1".to_owned(),
            update: value,
        }
    }

    #[test]
    fn events_fold_into_one_view() {
        let mut book = PiBook::new();
        book.open("s1", Path::new("/tmp"));
        assert!(book.apply(&ready()));
        let view = book.view("s1").unwrap();
        assert_eq!(view.state, super::super::session::PiSessionState::Ready);
        assert_eq!(view.model.as_ref().unwrap().id, "m");

        assert!(book.apply(&update(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "hello"},
            "messageId": "m1",
        }))));
        assert_eq!(book.rows("s1").len(), 1);

        let request = HostRequest {
            id: RequestId::Number(1),
            method: "_zu/ask/create".to_owned(),
            params: json!({"sessionId": "s1", "toolCallId": "call-1"}),
        };
        assert!(book.apply(&StoreEvent::Request {
            session_id: "s1".to_owned(),
            request,
        }));
        let pending = book.pending_asks().next().unwrap();
        assert_eq!(pending.0, "s1");
        assert!(book.take_request("s1", &RequestId::Number(1)).is_some());
        assert!(book.take_request("s1", &RequestId::Number(1)).is_none());
    }

    #[test]
    fn a_turn_streams_and_settles_through_the_book() {
        let mut book = PiBook::new();
        book.open("s1", Path::new("/tmp"));
        book.apply(&ready());

        let row = book.prompt_started("s1", "hi").unwrap();
        assert_eq!(row, "pending:1");
        assert_eq!(
            book.view("s1").unwrap().state,
            super::super::session::PiSessionState::Streaming
        );
        book.apply(&update(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "hello"},
            "messageId": "m1",
        })));
        book.apply(&update(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": ""},
            "messageId": "e1",
            "_meta": {"pi": {"settled": true, "entryId": "e1"}},
        })));
        // The settle already closed the streaming face; the prompt's own
        // answer confirms it.
        assert!(!book.view("s1").unwrap().streaming);
        book.turn_finished("s1");
        assert_eq!(
            book.view("s1").unwrap().state,
            super::super::session::PiSessionState::Ready
        );
    }

    #[test]
    fn a_replay_lands_before_live_frames() {
        let mut book = PiBook::new();
        book.open("s1", Path::new("/tmp"));
        book.apply(&StoreEvent::Replay {
            session_id: "s1".to_owned(),
            updates: vec![steer_acp::SessionUpdate {
                session_id: "s1".to_owned(),
                update: json!({
                    "sessionUpdate": "user_message_chunk",
                    "content": {"type": "text", "text": "earlier"},
                    "messageId": "e0",
                    "_meta": {"pi": {"entryId": "e0"}},
                }),
            }],
        });
        book.apply(&update(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "reply"},
            "messageId": "m1",
        })));
        let rows = book.rows("s1");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id(), "e0");
    }

    #[test]
    fn an_exit_keeps_the_transcript() {
        let mut book = PiBook::new();
        book.open("s1", Path::new("/tmp"));
        book.prompt_started("s1", "hi");
        assert!(book.apply(&StoreEvent::Exited {
            session_id: "s1".to_owned(),
            reason: "the harness exited (exit 7)".to_owned(),
        }));
        let view = book.view("s1").unwrap();
        assert_eq!(view.state, super::super::session::PiSessionState::Dead);
        assert_eq!(view.error.as_deref(), Some("the harness exited (exit 7)"));
        assert_eq!(view.transcript.settled().len(), 1);
    }
}
