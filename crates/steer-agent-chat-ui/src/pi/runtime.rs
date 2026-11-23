//! The pi transport: one ACP child process per open session, its two channels
//! translated into [`StoreEvent`]s on one queue.
//!
//! The runtime owns spawning, the handshake, the bind (`new`/`load`/`resume`),
//! and the event pumps. It does **not** own state: the consumer folds events
//! into a [`super::book::PiBook`] (and, in a window, into a gpui entity). Every
//! method takes the caller's event sender, so the bind's own facts
//! (`Ready`/`Replay`) reach the consumer before any live frame — the replay /
//! live ordering the ACP contract promises is established structurally.
//!
//! Async-only: production callers run it on the manox tokio runtime, tests on
//! `#[tokio::test]`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_channel::Sender;
use serde_json::Value;
use steer_acp::{
    AcpEnv, AcpError, AcpExit, AcpOptions, AcpProcess, HostRequest, RequestId, SessionInfo,
    SessionProxy, SessionUpdate,
};

/// What one session's pump reports to the host.
#[derive(Clone, Debug)]
pub enum StoreEvent {
    /// The bind answered: selectors, modes, and the banner's precondition.
    Ready {
        session_id: String,
        info: SessionInfo,
    },
    /// The restore's history (already ordered by the harness).
    Replay {
        session_id: String,
        updates: Vec<SessionUpdate>,
    },
    /// One live `session/update` payload.
    Update { session_id: String, update: Value },
    /// One server→client request waiting for the user.
    Request {
        session_id: String,
        request: HostRequest,
    },
    /// The process ended; the reason carries its stderr tail when it left one.
    Exited { session_id: String, reason: String },
}

/// How to bind the session on a fresh process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    /// `session/new` — a fresh conversation.
    New,
    /// `session/load` — reattach with the transcript replay.
    History,
    /// `session/resume` — reattach without replay.
    Resume,
}

/// Where harness processes come from.
#[derive(Clone, Debug)]
pub struct PiRuntime {
    bin: PathBuf,
    /// `ZU_STATE_DIR`: the Steer platform root (`~/.steer`).
    state_dir: PathBuf,
    /// The child's own working directory; each session names its `cwd`.
    cwd: PathBuf,
    /// Extra environment entries (test isolation, a reviewed override).
    env: Vec<(String, String)>,
}

impl PiRuntime {
    pub fn new(
        bin: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            bin: bin.into(),
            state_dir: state_dir.into(),
            cwd: cwd.into(),
            env: Vec::new(),
        }
    }

    /// One extra environment entry for every child this runtime spawns.
    #[must_use]
    pub fn with_env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    /// Spawns, handshakes, binds, and starts the pumps. `session_id` is the
    /// host-minted UUID for [`OpenMode::New`] (`None` lets the harness mint),
    /// and the session to reattach for the other modes.
    ///
    /// A session the harness no longer has (`session_not_found`) is rebuilt
    /// under the same id — the id is the host's handle, and an empty
    /// conversation is recoverable where a failed open is not.
    pub async fn open(
        &self,
        session_id: Option<&str>,
        cwd: &Path,
        mode: OpenMode,
        events: &Sender<StoreEvent>,
    ) -> Result<PiConnection, AcpError> {
        let mut env = AcpEnv::for_state_dir(&self.state_dir);
        for (name, value) in &self.env {
            env = env.with_var(name.clone(), value.clone());
        }
        let process = AcpProcess::spawn(AcpOptions::new(self.bin.clone(), self.cwd.clone(), env))?;
        let updates = process.updates()?;
        let requests = process.requests()?;
        process
            .initialize("steer", env!("CARGO_PKG_VERSION"))
            .await?;

        let (proxy, replay) = match mode {
            OpenMode::New => (process.new_session(session_id, cwd).await?, Vec::new()),
            OpenMode::History => {
                let id = session_id.unwrap_or_default();
                match process.load_session(id, cwd).await {
                    Ok((proxy, replay)) => (proxy, replay),
                    Err(error) if error.is_session_not_found() => {
                        (process.new_session(Some(id), cwd).await?, Vec::new())
                    }
                    Err(error) => return Err(error),
                }
            }
            OpenMode::Resume => {
                let id = session_id.unwrap_or_default();
                match process.resume_session(id, cwd).await {
                    Ok(proxy) => (proxy, Vec::new()),
                    Err(error) if error.is_session_not_found() => {
                        (process.new_session(Some(id), cwd).await?, Vec::new())
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        let session_id = proxy.id().to_owned();
        if events
            .send(StoreEvent::Ready {
                session_id: session_id.clone(),
                info: proxy.info().clone(),
            })
            .await
            .is_err()
        {
            process.close().await;
            return Err(AcpError::Closed);
        }
        if !replay.is_empty() {
            let _ = events
                .send(StoreEvent::Replay {
                    session_id: session_id.clone(),
                    updates: replay,
                })
                .await;
        }

        let process = Arc::new(process);
        spawn_pumps(
            process.clone(),
            session_id.clone(),
            updates,
            requests,
            events.clone(),
        );
        Ok(PiConnection {
            process,
            session: proxy,
        })
    }
}

/// One open session's process and its proxy.
#[derive(Clone)]
pub struct PiConnection {
    process: Arc<AcpProcess>,
    session: SessionProxy,
}

impl PiConnection {
    pub fn session_id(&self) -> &str {
        self.session.id()
    }

    /// What the bind announced (selectors, modes).
    pub fn info(&self) -> &SessionInfo {
        self.session.info()
    }

    pub async fn prompt(&self, text: &str) -> Result<String, AcpError> {
        self.session.prompt(text).await
    }

    pub fn cancel(&self) -> Result<(), AcpError> {
        self.session.cancel()
    }

    pub async fn set_config_option(&self, config_id: &str, value: &str) -> Result<Value, AcpError> {
        self.session.set_config_option(config_id, value).await
    }

    pub async fn set_mode(&self, mode_id: &str) -> Result<(), AcpError> {
        self.session.set_mode(mode_id).await
    }

    /// A pi extension request scoped to this session (`_zu/todos/list`, …).
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        self.session.request_scoped(method, params).await
    }

    /// Answers a server→client request.
    pub fn respond(&self, id: &RequestId, result: Value) -> Result<(), AcpError> {
        self.process.respond(id, result)
    }

    /// Refuses a server→client request.
    pub fn respond_error(&self, id: &RequestId, code: i64, message: &str) -> Result<(), AcpError> {
        self.process.respond_error(id, code, message)
    }

    pub fn is_alive(&self) -> bool {
        self.process.is_alive()
    }

    /// `session/close`: free the session, keep the process.
    pub async fn close_session(&self) -> Result<(), AcpError> {
        self.session.close().await
    }

    /// The graceful process exit (stdin EOF, then the group).
    pub async fn close(&self) {
        self.process.close().await;
    }
}

/// One updates pump and one requests pump per connection. The updates pump is
/// also the exit reporter: its channel closing is the process ending.
fn spawn_pumps(
    process: Arc<AcpProcess>,
    session_id: String,
    updates: tokio::sync::mpsc::UnboundedReceiver<SessionUpdate>,
    requests: tokio::sync::mpsc::UnboundedReceiver<HostRequest>,
    events: Sender<StoreEvent>,
) {
    let update_events = events.clone();
    let update_id = session_id.clone();
    tokio::spawn(async move {
        let mut updates = updates;
        while let Some(update) = updates.recv().await {
            let event = StoreEvent::Update {
                session_id: update_id.clone(),
                update: update.update,
            };
            if update_events.send(event).await.is_err() {
                return;
            }
        }
        let reason = describe_exit(process.exit(), process.stderr_tail());
        let _ = update_events
            .send(StoreEvent::Exited {
                session_id: update_id,
                reason,
            })
            .await;
    });

    let request_events = events;
    tokio::spawn(async move {
        let mut requests = requests;
        while let Some(request) = requests.recv().await {
            let session_id = request
                .params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or(&session_id)
                .to_owned();
            if request_events
                .send(StoreEvent::Request {
                    session_id,
                    request,
                })
                .await
                .is_err()
            {
                return;
            }
        }
    });
}

/// A human sentence for a dead process, with the stderr tail when it left one.
/// The sentence is app chrome (localized through the shared bundles); the
/// tail is harness output and rides verbatim.
fn describe_exit(exit: Option<AcpExit>, tail: String) -> String {
    use steer_i18n::{t, t_str};
    let reason = match exit {
        Some(AcpExit {
            code: Some(code), ..
        }) => t_str("pi-harness-exited-code", &[("code", &code.to_string())]),
        Some(AcpExit {
            signal: Some(signal),
            ..
        }) => t_str("pi-harness-exited-signal", &[("signal", &signal)]),
        _ => t("pi-harness-exited"),
    };
    let tail = tail.trim();
    if tail.is_empty() {
        return reason;
    }
    t_str(
        "pi-harness-exited-tail",
        &[("reason", &reason), ("tail", tail)],
    )
}
