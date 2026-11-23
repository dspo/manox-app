//! The pi store as a gpui entity: connections, the event pump, and the write
//! surface the sheet's view drives.
//!
//! The pure halves live in [`super::book`] (state fold) and [`super::runtime`]
//! (transport). This entity owns both, routes async calls onto the manox tokio
//! runtime through [`tokio_wait`], and wakes the window exactly when a fold
//! changed something. Every render reads [`AcpStore::book`]; no view keeps a
//! mirror.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use gpui::{Context, SharedString};
use serde_json::{Value, json};
use steer_acp::RequestId;

use super::book::PiBook;
use super::runtime::{OpenMode, PiConnection, PiRuntime, StoreEvent};
use super::session::{
    parse_plan, parse_restored_asks, parse_subagent_runs, parse_subagent_transcript,
};
use crate::i18n::t;

/// One window's pi sessions.
pub struct AcpStore {
    /// `None` when no harness binary could be resolved; the view shows that
    /// as a start-page error instead of failing every open the same way.
    runtime: Option<PiRuntime>,
    book: PiBook,
    connections: HashMap<String, PiConnection>,
    /// Follow-up texts queued behind the running turn, per session.
    queues: HashMap<String, VecDeque<String>>,
    opening: HashSet<String>,
    /// The plan panel's generation per session: every plan write bumps it, so
    /// a slow `_zu/todos/list` answer that raced the write is recognized as
    /// stale and dropped instead of overwriting the write's fresh plan.
    plan_generations: HashMap<String, u64>,
    /// The last store-level failure (a missing harness, a broken bind).
    error: Option<String>,
}

impl AcpStore {
    pub fn new(runtime: Option<PiRuntime>, _cx: &mut Context<Self>) -> Self {
        let error = runtime_notice(&runtime);
        Self {
            runtime,
            book: PiBook::new(),
            connections: HashMap::new(),
            queues: HashMap::new(),
            opening: HashSet::new(),
            plan_generations: HashMap::new(),
            error,
        }
    }

    /// The production constructor: resolve the shipped harness and the
    /// platform state root. A missing binary is not fatal — the start page
    /// reports it and every open fails with that reason.
    pub fn from_environment(cx: &mut Context<Self>) -> Self {
        let runtime = steer_acp::resolve_pi_acp().and_then(|bin| {
            let home = steer_paths::steer_home()?;
            Some(PiRuntime::new(bin, home.clone(), home))
        });
        Self::new(runtime, cx)
    }

    /// The folded state; every render reads this.
    pub fn book(&self) -> &PiBook {
        &self.book
    }

    /// The store-level failure (no harness, a refused bind).
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The session's program title, when pi set one.
    pub fn title(&self, session_id: &str) -> Option<SharedString> {
        self.book.title(session_id).map(SharedString::from)
    }

    /// Opens a fresh conversation. `session_id` must be a UUID (pi's pairing
    /// extension enforces the shape; see [`steer_acp::mint_session_id`]).
    pub fn open_new(&mut self, session_id: &str, cwd: PathBuf, cx: &mut Context<Self>) {
        self.open(session_id, cwd, OpenMode::New, cx);
    }

    /// Reattaches to a session: `history` loads the transcript replay,
    /// otherwise the process resumes without one. A session the harness no
    /// longer has is rebuilt under the same id.
    pub fn open_existing(
        &mut self,
        session_id: &str,
        cwd: PathBuf,
        history: bool,
        cx: &mut Context<Self>,
    ) {
        let mode = if history {
            OpenMode::History
        } else {
            OpenMode::Resume
        };
        self.open(session_id, cwd, mode, cx);
    }

    fn open(&mut self, session_id: &str, cwd: PathBuf, mode: OpenMode, cx: &mut Context<Self>) {
        if self.opening.contains(session_id) || self.connections.contains_key(session_id) {
            return;
        }
        let Some(runtime) = self.runtime.clone() else {
            self.book.open(session_id, &cwd);
            self.apply_event(
                &StoreEvent::Exited {
                    session_id: session_id.to_owned(),
                    reason: self
                        .error
                        .clone()
                        .unwrap_or_else(|| t("pi-harness-unavailable").to_string()),
                },
                cx,
            );
            return;
        };
        self.book.open(session_id, &cwd);
        if let Some(view) = self.book.view_mut(session_id) {
            view.loading = mode == OpenMode::History;
        }
        self.opening.insert(session_id.to_owned());
        cx.notify();

        let (events, receiver) = async_channel::unbounded();
        let session_id = session_id.to_owned();
        cx.spawn(async move |this, cx| {
            let open = {
                let runtime = runtime.clone();
                let events = events.clone();
                let session_id = session_id.clone();
                let cwd = cwd.clone();
                move || async move {
                    runtime
                        .open(Some(&session_id), &cwd, mode, &events)
                        .await
                        .map_err(|error| error.describe())
                }
            };
            let result = tokio_wait(open).await;
            let alive = this.update(cx, |store, cx| {
                store.opening.remove(&session_id);
                match result {
                    Ok(connection) => {
                        store.error = None;
                        store.connections.insert(session_id.clone(), connection);
                        store.refresh_panels(&session_id, cx);
                    }
                    Err(reason) => {
                        store.apply_event(
                            &StoreEvent::Exited {
                                session_id: session_id.clone(),
                                reason,
                            },
                            cx,
                        );
                    }
                }
                cx.notify();
            });
            if alive.is_err() {
                return;
            }
            while let Ok(event) = receiver.recv().await {
                let updated = this.update(cx, |store, cx| {
                    store.apply_event(&event, cx);
                });
                if updated.is_err() {
                    return;
                }
            }
            // The channel closed without an exit event only when the entity
            // went away; the connection is retired either way.
            let _ = this.update(cx, |store, _| {
                store.connections.remove(&session_id);
            });
        })
        .detach();
    }

    /// Sends the user's message. A prompt while a turn runs is a follow-up:
    /// it queues behind the running turn (and its optimistic row renders
    /// after the answer it interrupted).
    pub fn prompt(&mut self, session_id: &str, text: String, cx: &mut Context<Self>) {
        if self.book.view(session_id).is_none() {
            return;
        }
        if self
            .book
            .view(session_id)
            .is_some_and(|view| view.streaming)
        {
            let queue = self.queues.entry(session_id.to_owned()).or_default();
            queue.push_back(text.clone());
            let depth = queue.len();
            self.book.prompt_started(session_id, &text);
            self.book.set_queued(session_id, depth);
            cx.notify();
            return;
        }
        let Some(connection) = self.connections.get(session_id).cloned() else {
            self.book
                .note_error(session_id, &t("pi-harness-not-connected"));
            cx.notify();
            return;
        };
        self.book.prompt_started(session_id, &text);
        cx.notify();

        let session_id = session_id.to_owned();
        cx.spawn(async move |this, cx| {
            let call = {
                let connection = connection.clone();
                let text = text.clone();
                move || async move {
                    connection
                        .prompt(&text)
                        .await
                        .map_err(|error| error.describe())
                }
            };
            let result = tokio_wait(call).await;
            this.update(cx, |store, cx| {
                store.settle_turn(&session_id, result, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Interrupts the running turn (the pending prompt settles `cancelled`).
    pub fn cancel(&mut self, session_id: &str, cx: &mut Context<Self>) {
        if let Some(connection) = self.connections.get(session_id) {
            let _ = connection.cancel();
        }
        cx.notify();
    }

    /// Answers a server→client request. The pending card leaves the state
    /// before the wire write, so a double click cannot answer twice.
    pub fn answer(
        &mut self,
        session_id: &str,
        request: &RequestId,
        result: Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pending) = self.book.take_request(session_id, request) else {
            return false;
        };
        let written = self
            .connections
            .get(session_id)
            .is_some_and(|connection| connection.respond(&pending.id, result).is_ok());
        if !written {
            self.book
                .note_error(session_id, &t("pi-harness-not-connected"));
        }
        cx.notify();
        written
    }

    /// Refuses a server→client request with a JSON-RPC error.
    pub fn refuse(
        &mut self,
        session_id: &str,
        request: &RequestId,
        code: i64,
        message: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pending) = self.book.take_request(session_id, request) else {
            return false;
        };
        let written = self
            .connections
            .get(session_id)
            .is_some_and(|connection| connection.respond_error(&pending.id, code, message).is_ok());
        if !written {
            self.book
                .note_error(session_id, &t("pi-harness-not-connected"));
        }
        cx.notify();
        written
    }

    /// Switches a selector (model, thinking level); the harness's
    /// `config_option_update` is what lands the new list.
    pub fn set_config_option(
        &mut self,
        session_id: &str,
        config_id: String,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.connections.get(session_id).cloned() else {
            self.book
                .note_error(session_id, &t("pi-harness-not-connected"));
            cx.notify();
            return;
        };
        let session_id = session_id.to_owned();
        cx.spawn(async move |this, cx| {
            let call = {
                let connection = connection.clone();
                let config_id = config_id.clone();
                let value = value.clone();
                move || async move {
                    connection
                        .set_config_option(&config_id, &value)
                        .await
                        .map(|_| ())
                        .map_err(|error| error.describe())
                }
            };
            let result = tokio_wait(call).await;
            if let Err(reason) = result {
                this.update(cx, |store, cx| {
                    store.book.note_error(&session_id, &reason);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Switches the approval policy.
    pub fn set_mode(&mut self, session_id: &str, mode_id: String, cx: &mut Context<Self>) {
        let Some(connection) = self.connections.get(session_id).cloned() else {
            self.book
                .note_error(session_id, &t("pi-harness-not-connected"));
            cx.notify();
            return;
        };
        let session_id = session_id.to_owned();
        cx.spawn(async move |this, cx| {
            let call = {
                let connection = connection.clone();
                let mode_id = mode_id.clone();
                move || async move {
                    connection
                        .set_mode(&mode_id)
                        .await
                        .map_err(|error| error.describe())
                }
            };
            let result = tokio_wait(call).await;
            if let Err(reason) = result {
                this.update(cx, |store, cx| {
                    store.book.note_error(&session_id, &reason);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Refreshes the request/response surfaces the sheet pulls (the plan, the
    /// subagent runs, and the question cards a restart kept alive). None of
    /// them pushes: the harness's tools write the same store the panel reads,
    /// so a pull after every bind and settled turn keeps them current (the
    /// write paths answer with the fresh plan themselves).
    pub fn refresh_panels(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let Some(connection) = self.connections.get(session_id).cloned() else {
            return;
        };
        let stamp = self.plan_generation(session_id);
        let session_id = session_id.to_owned();
        cx.spawn(async move |this, cx| {
            let call = {
                let connection = connection.clone();
                move || async move {
                    let plan = connection
                        .request("_zu/todos/list", json!({}))
                        .await
                        .map_err(|error| error.describe());
                    let runs = connection
                        .request("_zu/subagents/list", json!({}))
                        .await
                        .map_err(|error| error.describe());
                    let asks = connection
                        .request("_zu/ask/pending", json!({}))
                        .await
                        .map_err(|error| error.describe());
                    Ok::<_, String>((plan, runs, asks))
                }
            };
            let Ok((plan, runs, asks)) = tokio_wait(call).await else {
                return;
            };
            this.update(cx, |store, cx| {
                let mut changed = false;
                // The answer folds only while no plan write happened since the
                // request went out; an older response must not overwrite the
                // write's fresher plan.
                if let Ok(plan) = plan
                    && store.plan_answer_is_current(&session_id, stamp)
                {
                    changed |= store.book.set_plan(&session_id, parse_plan(&plan));
                }
                // A session without the subagent surface answers
                // method-not-found; that is an empty panel, not an error.
                if let Ok(runs) = runs {
                    changed |= store
                        .book
                        .set_subagents(&session_id, parse_subagent_runs(&runs));
                }
                if let Ok(asks) = asks {
                    changed |= store
                        .book
                        .set_restored_asks(&session_id, parse_restored_asks(&asks));
                }
                if changed {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Answers a question card that outlived its turn (`_zu/ask/answer`): the
    /// harness resolves a live wait or injects a late answer and starts a turn.
    pub fn answer_restored_ask(
        &mut self,
        session_id: &str,
        tool_call_id: String,
        response: Value,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.connections.get(session_id).cloned() else {
            self.book
                .note_error(session_id, &t("pi-harness-not-connected"));
            cx.notify();
            return;
        };
        // Take the card before the wire write: a second click finds nothing
        // left to take and must not send a second `_zu/ask/answer` (the same
        // take-first shape the live request path gets from `take_request`).
        if !self.take_restored_ask(session_id, &tool_call_id) {
            return;
        }
        cx.notify();
        let session_id = session_id.to_owned();
        cx.spawn(async move |this, cx| {
            let call = {
                let connection = connection.clone();
                let tool_call_id = tool_call_id.clone();
                move || async move {
                    connection
                        .request(
                            "_zu/ask/answer",
                            json!({ "toolCallId": tool_call_id, "response": response }),
                        )
                        .await
                        .map_err(|error| error.describe())
                }
            };
            let result = tokio_wait(call).await;
            this.update(cx, |store, cx| {
                match result {
                    Ok(value) => {
                        if value.get("delivered").and_then(Value::as_str) == Some("none") {
                            store
                                .book
                                .note_error(&session_id, &t("pi-harness-no-answerable-ask"));
                        }
                        store.refresh_panels(&session_id, cx);
                    }
                    Err(reason) => store.book.note_error(&session_id, &reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Removes the restored ask with `tool_call_id` (its answer is being
    /// written); `false` means it was already answered or never pending.
    fn take_restored_ask(&mut self, session_id: &str, tool_call_id: &str) -> bool {
        let Some(view) = self.book.view_mut(session_id) else {
            return false;
        };
        let before = view.restored_asks.len();
        view.restored_asks
            .retain(|ask| ask.tool_call_id != tool_call_id);
        view.restored_asks.len() != before
    }

    /// Writes one plan edit (`_zu/todos/write`); the harness answers with the
    /// new plan, which lands here (the panel never mutates its own copy).
    pub fn set_todo_status(
        &mut self,
        session_id: &str,
        todo_id: String,
        status: &'static str,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.connections.get(session_id).cloned() else {
            self.book
                .note_error(session_id, &t("pi-harness-not-connected"));
            cx.notify();
            return;
        };
        // The write invalidates any panel refresh that raced it: stamp this
        // write with the newest generation before the request goes out.
        let stamp = self.bump_plan_generation(session_id);
        let session_id = session_id.to_owned();
        cx.spawn(async move |this, cx| {
            let call = {
                let connection = connection.clone();
                let todo_id = todo_id.clone();
                move || async move {
                    connection
                        .request(
                            "_zu/todos/write",
                            json!({
                                "op": { "kind": "update", "id": todo_id, "status": status }
                            }),
                        )
                        .await
                        .map_err(|error| error.describe())
                }
            };
            let result = tokio_wait(call).await;
            this.update(cx, |store, cx| {
                match result {
                    Ok(value) => {
                        // The write answer is `{plan}` without `enabled`; a
                        // successful write means the surface is on. A newer
                        // write's answer wins, so only the current
                        // generation's answer folds.
                        if store.plan_answer_is_current(&session_id, stamp) {
                            let mut plan = parse_plan(&value);
                            plan.enabled = true;
                            store.book.set_plan(&session_id, plan);
                        }
                    }
                    Err(reason) => store.book.note_error(&session_id, &reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Expands a subagent run's transcript (fetching it on first open) or
    /// collapses it.
    pub fn toggle_subagent(&mut self, session_id: &str, run_id: String, cx: &mut Context<Self>) {
        let Some(expand) = self.book.toggle_subagent(session_id, &run_id) else {
            cx.notify();
            return;
        };
        let cached = self
            .book
            .view(session_id)
            .is_some_and(|view| view.subagent_transcripts.contains_key(expand.as_str()));
        cx.notify();
        if cached {
            return;
        }
        let Some(connection) = self.connections.get(session_id).cloned() else {
            return;
        };
        let session_id = session_id.to_owned();
        cx.spawn(async move |this, cx| {
            let call = {
                let connection = connection.clone();
                let run_id = run_id.clone();
                move || async move {
                    connection
                        .request("_zu/subagents/transcript", json!({ "runId": run_id }))
                        .await
                        .map_err(|error| error.describe())
                }
            };
            let result = tokio_wait(call).await;
            this.update(cx, |store, cx| {
                match result {
                    Ok(value) => {
                        store.book.set_subagent_transcript(
                            &session_id,
                            &run_id,
                            parse_subagent_transcript(&value),
                        );
                    }
                    Err(reason) => store.book.note_error(&session_id, &reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reattaches a dead session: `session/load` (with replay, so the sheet
    /// keeps its transcript) under the same id. A harness that no longer has
    /// the session re-mints a fresh one under that id — the same repair the
    /// bind path performs.
    pub fn reconnect(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let Some(cwd) = self.book.view(session_id).map(|view| view.cwd.clone()) else {
            return;
        };
        self.open_existing(session_id, cwd, true, cx);
    }

    /// Closes one session: `session/close`, then the process (one process per
    /// session). The view is dropped with it.
    pub fn close_session(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let connection = self.connections.remove(session_id);
        self.queues.remove(session_id);
        self.plan_generations.remove(session_id);
        self.book.remove(session_id);
        cx.notify();
        let Some(connection) = connection else {
            return;
        };
        cx.spawn(async move |_this, _cx| {
            let _ = connection.close_session().await;
            connection.close().await;
        })
        .detach();
    }

    /// Applies one transport event to the book. A dead process also drops the
    /// session's queued follow-ups: they were waiting on that process's turns,
    /// and letting them ride into the next connection would auto-send
    /// messages the user has long stopped waiting on.
    fn apply_event(&mut self, event: &StoreEvent, cx: &mut Context<Self>) -> bool {
        if let StoreEvent::Exited { session_id, .. } = event {
            self.queues.remove(session_id);
        }
        let changed = self.book.apply(event);
        if changed {
            cx.notify();
        }
        changed
    }

    /// The session's current plan generation.
    fn plan_generation(&self, session_id: &str) -> u64 {
        self.plan_generations
            .get(session_id)
            .copied()
            .unwrap_or_default()
    }

    /// Bumps the plan generation (every plan write) and returns the new value.
    fn bump_plan_generation(&mut self, session_id: &str) -> u64 {
        let next = self.plan_generation(session_id) + 1;
        self.plan_generations.insert(session_id.to_owned(), next);
        next
    }

    /// Whether a plan answer stamped `stamp` still reflects the newest plan
    /// write: responses can overtake or trail a racing write, and only the
    /// newest generation's answer may fold.
    fn plan_answer_is_current(&self, session_id: &str, stamp: u64) -> bool {
        self.plan_generation(session_id) == stamp
    }

    /// Test seam: creates the store-side view a bind would create, so folded
    /// events land on a session instead of being dropped.
    #[cfg(feature = "test-support")]
    pub fn open_for_test(&mut self, session_id: &str, cwd: &std::path::Path) {
        self.book.open(session_id, cwd);
    }

    /// Test seam: folds one transport event exactly as a live connection's
    /// pump would, so view tests can seed transcripts without a harness
    /// process.
    #[cfg(feature = "test-support")]
    pub fn fold_event_for_test(&mut self, event: &StoreEvent, cx: &mut Context<Self>) {
        if self.book.apply(event) {
            cx.notify();
        }
    }

    /// A turn settled: finish the face, then run the next queued follow-up.
    fn settle_turn(
        &mut self,
        session_id: &str,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(_) => self.book.turn_finished(session_id),
            Err(reason) => self.book.turn_failed(session_id, &reason),
        }
        let next = self
            .queues
            .get_mut(session_id)
            .and_then(|queue| queue.pop_front());
        let depth = self
            .queues
            .get(session_id)
            .map(VecDeque::len)
            .unwrap_or_default();
        self.book.set_queued(session_id, depth);
        if let Some(next) = next {
            self.prompt(session_id, next, cx);
        }
        // The turn may have moved the plan or settled subagent runs.
        self.refresh_panels(session_id, cx);
        cx.notify();
    }
}

/// The start-page notice for a store with no harness binary.
fn runtime_notice(runtime: &Option<PiRuntime>) -> Option<String> {
    runtime
        .is_none()
        .then(|| t("pi-harness-not-found").to_string())
}

/// Runs an async closure on the manox tokio runtime and awaits its result from
/// a GPUI future (the same bridge `ahp_store` uses).
async fn tokio_wait<T, F>(f: impl FnOnce() -> F + Send + 'static) -> Result<T, String>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, String>> + Send,
{
    let (tx, rx) = async_channel::bounded::<Result<T, String>>(1);
    manox_agent::runtime::handle().spawn(async move {
        let _ = tx.send(f().await).await;
    });
    rx.recv().await.map_err(|_| t("pi-tokio-runtime-gone"))?
}

/// The sheet title's source: the program title, else the session's short id.
pub fn fallback_title(store: &AcpStore, session_id: &str) -> SharedString {
    if let Some(title) = store.title(session_id) {
        return title;
    }
    SharedString::from(short_id(session_id))
}

fn short_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::super::session::PiRestoredAsk;
    use super::*;

    /// An entity-free store: the harness is missing and the book is empty.
    /// These paths never touch gpui, so no app context is needed.
    fn bare_store() -> AcpStore {
        AcpStore {
            runtime: None,
            book: PiBook::new(),
            connections: HashMap::new(),
            queues: HashMap::new(),
            opening: HashSet::new(),
            plan_generations: HashMap::new(),
            error: None,
        }
    }

    fn store_with_restored_ask() -> AcpStore {
        let mut store = bare_store();
        store.book.open("s1", Path::new("/tmp"));
        if let Some(view) = store.book.view_mut("s1") {
            view.restored_asks.push(PiRestoredAsk {
                tool_call_id: "call-1".to_owned(),
                params: json!({}),
            });
        }
        store
    }

    #[test]
    fn a_restored_ask_is_taken_once() {
        let mut store = store_with_restored_ask();
        assert!(store.take_restored_ask("s1", "call-1"));
        assert!(
            !store.take_restored_ask("s1", "call-1"),
            "the second take finds nothing: the repeat is a no-op"
        );
        assert!(!store.take_restored_ask("gone", "call-1"));
    }

    #[test]
    fn only_the_newest_plan_generation_folds() {
        let mut store = bare_store();
        assert!(store.plan_answer_is_current("s1", 0));
        assert_eq!(store.bump_plan_generation("s1"), 1);
        assert!(
            !store.plan_answer_is_current("s1", 0),
            "a response stamped before the write is stale"
        );
        assert!(store.plan_answer_is_current("s1", 1));
    }
}
