//! The public process handle: spawn, handshake, session creation, and the two
//! host-facing channels.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::PROTOCOL_VERSION;
use crate::error::AcpError;
use crate::session::SessionProxy;
use crate::transport::Connection;
use crate::types::{AcpExit, HostRequest, InitializeResult, RequestId, SessionInfo, SessionUpdate};

/// The handshake watchdog: a silent harness is a failed start.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
/// `session/new|load|resume` ride the same budget.
const BIND_TIMEOUT: Duration = Duration::from_secs(60);

/// The environment a harness needs to serve this host.
///
/// The parent environment is inherited; these entries are the ones the host
/// must pin so the harness reads the same state root the app writes. A pi
/// session's own `PI_CODING_AGENT_DIR` (session files, user settings) stays at
/// pi's default `~/.pi/agent` unless a caller sets it — the two harnesses keep
/// separate roots, and manox's `MANOX_HOME` is not part of this leg.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcpEnv {
    entries: Vec<(String, String)>,
}

impl AcpEnv {
    /// Only the inherited environment (tests, a reviewed manual setup).
    pub fn inherit() -> Self {
        Self::default()
    }

    /// `ZU_STATE_DIR=<dir>`: the state root the harness reads settings and
    /// subagent records from. This is the Steer platform root (`~/.steer`),
    /// not the manox harness layer inside it.
    pub fn for_state_dir(dir: &Path) -> Self {
        Self::inherit().with_var("ZU_STATE_DIR", dir)
    }

    /// Pin pi's own agent directory (session files and user settings). The
    /// default is pi's `~/.pi/agent`, deliberately shared with the official
    /// CLI; callers only set this to isolate a test or an imported profile.
    pub fn with_agent_dir(self, dir: &Path) -> Self {
        self.with_var("PI_CODING_AGENT_DIR", dir)
    }

    /// Extra extension roots the harness loads on top of its own registry.
    pub fn with_extra_extension_paths<P, I>(self, paths: I) -> Self
    where
        P: AsRef<Path>,
        I: IntoIterator<Item = P>,
    {
        let joined = paths
            .into_iter()
            .map(|path| path.as_ref().display().to_string())
            .collect::<Vec<_>>()
            .join(",");
        if joined.is_empty() {
            return self;
        }
        self.with_var("ACP_EXTRA_EXTENSION_PATHS", joined.as_str())
    }

    /// `ACP_NO_EXTENSIONS=1`: the harness's built-in extension set is off.
    pub fn without_extensions(self) -> Self {
        self.with_var("ACP_NO_EXTENSIONS", "1")
    }

    /// One extra environment entry.
    pub fn with_var(self, name: impl Into<String>, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let mut entries = self.entries;
        let name = name.into();
        let value = value.as_ref().to_string_lossy().into_owned();
        entries.push((name, value));
        Self { entries }
    }

    fn pairs(&self) -> Vec<(String, String)> {
        self.entries.clone()
    }
}

/// How to start one harness process.
#[derive(Debug, Clone)]
pub struct AcpOptions {
    /// The `pi-acp` executable ([`resolve_pi_acp`] finds the shipped one).
    pub bin: PathBuf,
    /// The process's working directory; each session names its own `cwd` in
    /// `session/new`, and this is the fallback the child starts in.
    pub cwd: PathBuf,
    pub env: AcpEnv,
}

impl AcpOptions {
    pub fn new(bin: impl Into<PathBuf>, cwd: impl Into<PathBuf>, env: AcpEnv) -> Self {
        Self {
            bin: bin.into(),
            cwd: cwd.into(),
            env,
        }
    }
}

/// One harness process, from spawn to exit. Dropping the handle hangs up the
/// process group; [`AcpProcess::close`] is the graceful path.
pub struct AcpProcess {
    connection: Arc<Connection>,
}

impl AcpProcess {
    /// Starts the child. A failed exec is the only error: protocol failures
    /// arrive through calls.
    pub fn spawn(options: AcpOptions) -> Result<Self, AcpError> {
        let connection = Connection::start(&options.bin, &options.cwd, &options.env.pairs())?;
        Ok(Self { connection })
    }

    /// The `session/update` stream. Take-once: one host pump consumes it.
    pub fn updates(&self) -> Result<mpsc::UnboundedReceiver<SessionUpdate>, AcpError> {
        self.connection.take_updates()
    }

    /// The server→client request stream (approvals, elicitations, `_zu/*`).
    /// Take-once, like [`AcpProcess::updates`].
    pub fn requests(&self) -> Result<mpsc::UnboundedReceiver<HostRequest>, AcpError> {
        self.connection.take_requests()
    }

    /// Shakes hands. A harness that speaks another protocol version is
    /// refused, and a silent start is killed.
    pub async fn initialize(
        &self,
        client_name: &str,
        client_version: &str,
    ) -> Result<InitializeResult, AcpError> {
        let result = self
            .connection
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": {},
                    "clientInfo": { "name": client_name, "version": client_version },
                }),
                Some(INITIALIZE_TIMEOUT),
            )
            .await?;
        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                AcpError::Protocol("initialize returned no protocolVersion".to_owned())
            })?;
        if protocol_version != PROTOCOL_VERSION {
            self.kill();
            return Err(AcpError::Protocol(format!(
                "the harness speaks protocol {protocol_version}, this host speaks {PROTOCOL_VERSION}"
            )));
        }
        let agent = result.get("agentInfo");
        Ok(InitializeResult {
            protocol_version,
            agent_name: agent
                .and_then(|agent| agent.get("name"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            agent_version: agent
                .and_then(|agent| agent.get("version"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            raw: result,
        })
    }

    /// `session/new`. `session_id` is the host-minted UUID that becomes the
    /// harness's session id (and pi's session-file key); `None` lets the
    /// harness mint one.
    pub async fn new_session(
        &self,
        session_id: Option<&str>,
        cwd: &Path,
    ) -> Result<SessionProxy, AcpError> {
        let mut params = json!({
            "cwd": cwd.display().to_string(),
            "mcpServers": [],
        });
        if let Some(session_id) = session_id {
            params["_meta"] = json!({ "sessionId": session_id });
        }
        let result = self
            .connection
            .request("session/new", params, Some(BIND_TIMEOUT))
            .await
            .inspect_err(|_| self.kill())?;
        let info = session_info(result)?;
        Ok(SessionProxy::new(self.connection.clone(), info))
    }

    /// `session/load`: reattach with the transcript replay. The replay frames
    /// precede the response on the wire, so they are captured while the call
    /// is in flight and returned with it — never raced against the response.
    pub async fn load_session(
        &self,
        session_id: &str,
        cwd: &Path,
    ) -> Result<(SessionProxy, Vec<SessionUpdate>), AcpError> {
        let _guard = self.connection.load_guard().await;
        self.connection.begin_replay()?;
        let result = self
            .connection
            .request(
                "session/load",
                bind_params(session_id, cwd),
                Some(BIND_TIMEOUT),
            )
            .await;
        let replay = self.connection.take_replay();
        let result = result.inspect_err(|_| self.kill())?;
        let info = session_info_for(session_id, result)?;
        Ok((SessionProxy::new(self.connection.clone(), info), replay))
    }

    /// `session/resume`: reattach without a replay. A coded refusal (the
    /// harness answered, it did not die) keeps the process usable, so the
    /// caller can rebuild the session under the same id on this connection.
    pub async fn resume_session(
        &self,
        session_id: &str,
        cwd: &Path,
    ) -> Result<SessionProxy, AcpError> {
        let result = self
            .connection
            .request(
                "session/resume",
                bind_params(session_id, cwd),
                Some(BIND_TIMEOUT),
            )
            .await;
        match result {
            Ok(result) => Ok(SessionProxy::new(
                self.connection.clone(),
                session_info_for(session_id, result)?,
            )),
            Err(error @ AcpError::Rpc { .. }) => Err(error),
            Err(error) => {
                self.kill();
                Err(error)
            }
        }
    }

    /// Answers a server→client request.
    pub fn respond(&self, id: &RequestId, result: Value) -> Result<(), AcpError> {
        self.connection.respond(id, result)
    }

    /// Refuses a server→client request with a JSON-RPC error.
    pub fn respond_error(&self, id: &RequestId, code: i64, message: &str) -> Result<(), AcpError> {
        self.connection.respond_error(id, code, message)
    }

    /// The harness's last stderr lines, newest last.
    pub fn stderr_tail(&self) -> String {
        self.connection.stderr_tail()
    }

    /// How the process ended, once it has.
    pub fn exit(&self) -> Option<AcpExit> {
        self.connection.exit()
    }

    /// Whether the process is still running.
    pub fn is_alive(&self) -> bool {
        self.connection.exit().is_none()
    }

    /// The graceful exit: stdin EOF, then the group after a five-second grace.
    pub async fn close(&self) {
        self.connection.close().await;
    }

    /// Hangs up at once (the drop path; callable explicitly).
    pub fn kill(&self) {
        self.connection.kill();
    }
}

fn bind_params(session_id: &str, cwd: &Path) -> Value {
    json!({
        "sessionId": session_id,
        "cwd": cwd.display().to_string(),
        "mcpServers": [],
    })
}

fn session_info(result: Value) -> Result<SessionInfo, AcpError> {
    let session_id = result
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| AcpError::Protocol("session/new returned no sessionId".to_owned()))?
        .to_owned();
    session_info_for(&session_id, result)
}

fn session_info_for(session_id: &str, result: Value) -> Result<SessionInfo, AcpError> {
    Ok(SessionInfo {
        session_id: session_id.to_owned(),
        config_options: result.get("configOptions").cloned().unwrap_or(Value::Null),
        modes: result.get("modes").cloned().unwrap_or(Value::Null),
        raw: result,
    })
}

impl Drop for AcpProcess {
    fn drop(&mut self) {
        self.connection.kill();
    }
}

/// The `pi-acp` executable, in shipped and development layouts.
///
/// Order: `STEER_PI_ACP` (tests and manual runs), the app bundle's
/// `Contents/Helpers/pi-acp`, the Linux install's `/usr/lib/steer/pi-acp`,
/// the repository build (`pi-harness/dist/pi-acp`), and finally a `pi-acp`
/// beside the running executable.
pub fn resolve_pi_acp() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("STEER_PI_ACP").filter(|path| !path.is_empty()) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        // Steer.app/Contents/MacOS/steer → Steer.app/Contents/Helpers/pi-acp
        candidates.push(dir.join("..").join("Helpers").join("pi-acp"));
        candidates.push(dir.join("pi-acp"));
    }
    candidates.push(PathBuf::from("/usr/lib/steer/pi-acp"));
    // The development build: this crate lives two levels under the repo root.
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("pi-harness")
            .join("dist")
            .join("pi-acp"),
    );
    candidates.into_iter().find(|path| path.is_file())
}
