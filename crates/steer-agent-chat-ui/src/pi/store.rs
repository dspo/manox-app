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
use super::session::{parse_plan, parse_subagent_runs, parse_subagent_transcript};

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
            self.book.apply(&StoreEvent::Exited {
                session_id: session_id.to_owned(),
                reason: self
                    .error
                    .clone()
                    .unwrap_or_else(|| "the pi harness is not available".to_owned()),
            });
            cx.notify();
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
                        store.book.apply(&StoreEvent::Exited {
                            session_id: session_id.clone(),
                            reason,
                        });
                    }
                }
                cx.notify();
            });
            if alive.is_err() {
                return;
            }
            while let Ok(event) = receiver.recv().await {
                let updated = this.update(cx, |store, cx| {
                    if store.book.apply(&event) {
                        cx.notify();
                    }
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
                .note_error(session_id, "the pi harness is not connected");
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
                .note_error(session_id, "the pi harness is not connected");
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
                .note_error(session_id, "the pi harness is not connected");
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

    /// Refreshes the two request/response panels (the plan and the subagent
    /// runs). Neither pushes: the harness's tools write the same store the
    /// panel reads, so a pull after every bind and settled turn keeps them
    /// current (the write paths answer with the fresh plan themselves).
    pub fn refresh_panels(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let Some(connection) = self.connections.get(session_id).cloned() else {
            return;
        };
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
                    Ok::<_, String>((plan, runs))
                }
            };
            let Ok((plan, runs)) = tokio_wait(call).await else {
                return;
            };
            this.update(cx, |store, cx| {
                let mut changed = false;
                if let Ok(plan) = plan {
                    changed |= store.book.set_plan(&session_id, parse_plan(&plan));
                }
                // A session without the subagent surface answers
                // method-not-found; that is an empty panel, not an error.
                if let Ok(runs) = runs {
                    changed |= store
                        .book
                        .set_subagents(&session_id, parse_subagent_runs(&runs));
                }
                if changed {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
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
                .note_error(session_id, "the pi harness is not connected");
            cx.notify();
            return;
        };
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
                        // successful write means the surface is on.
                        let mut plan = parse_plan(&value);
                        plan.enabled = true;
                        store.book.set_plan(&session_id, plan);
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

    /// Reattaches a dead session: `session/resume` (with replay, so the sheet
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
    runtime.is_none().then(|| {
        "the pi harness binary was not found (build pi-harness or set STEER_PI_ACP)".to_owned()
    })
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
    rx.recv()
        .await
        .map_err(|_| "the tokio runtime is gone".to_owned())?
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
