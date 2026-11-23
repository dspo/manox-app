//! The wire: one child, newline-framed JSON-RPC 2.0 in both directions.
//!
//! Three tasks own the process's stdio: the reader frames stdout into
//! responses, server→client requests and `session/update` notifications; the
//! writer serialises outbound lines onto stdin; the waiter turns the exit into
//! a settled channel and a failure for every pending call. The process starts
//! in its own process group so teardown reaches pi's own tool subprocesses.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, watch};

use crate::error::{AcpError, RpcFailure};
use crate::types::{AcpExit, HostRequest, RequestId, SessionUpdate};

/// The largest single protocol line accepted. A longer one is a desync, not a
/// message: tool outputs reach MiB scale, so the budget is generous but finite.
const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;
/// Stderr lines kept for failure reports (the tail only).
const STDERR_TAIL_LINES: usize = 64;
/// How long a graceful close waits for the harness to flush and exit.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// The frames a `session/load` diverts from the live channel. The capture
/// ends at its request's settlement: replay frames precede the response on
/// the wire, so a same-session frame settled after it is live traffic.
struct ReplayCapture {
    request_id: i64,
    session_id: String,
    sealed: bool,
    frames: Vec<SessionUpdate>,
}

/// The capture's lifetime guard: taking the frames ends the capture on
/// success, and dropping it unreleased (a cancelled or failed load) does the
/// same — otherwise the slot would swallow every later update.
pub(crate) struct ReplayTicket<'a> {
    connection: &'a Connection,
}

impl ReplayTicket<'_> {
    pub(crate) fn take(self) -> Vec<SessionUpdate> {
        self.connection.take_replay()
    }
}

impl Drop for ReplayTicket<'_> {
    fn drop(&mut self) {
        let _ = self.connection.take_replay();
    }
}

/// One live harness connection and everything the tasks share.
pub(crate) struct Connection {
    pid: u32,
    writer: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    pending: Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcFailure>>>>,
    next_id: AtomicI64,
    /// When `Some`, `session/update` frames *for this session* are captured
    /// here instead of the update channel — `session/load`'s replay, which must
    /// be delivered with the response, not raced against it. Frames for other
    /// sessions keep flowing live.
    replay: Mutex<Option<ReplayCapture>>,
    load_lock: tokio::sync::Mutex<()>,
    updates_tx: Mutex<Option<mpsc::UnboundedSender<SessionUpdate>>>,
    updates_rx: Mutex<Option<mpsc::UnboundedReceiver<SessionUpdate>>>,
    requests_tx: Mutex<Option<mpsc::UnboundedSender<HostRequest>>>,
    requests_rx: Mutex<Option<mpsc::UnboundedReceiver<HostRequest>>>,
    stderr_tail: Mutex<VecDeque<String>>,
    exit: watch::Sender<Option<AcpExit>>,
    killed: AtomicBool,
}

impl Connection {
    /// Starts the child and wires its stdio. The host-facing receivers live
    /// on the returned connection; the host takes them before it pumps.
    pub(crate) fn start(
        bin: &std::path::Path,
        cwd: &std::path::Path,
        env: &[(String, String)],
    ) -> Result<Arc<Self>, AcpError> {
        let mut command = Command::new(bin);
        command
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(env.iter().map(|(name, value)| (name, value)));
        // A harness is a session's whole subtree: pi's tool subprocesses are
        // its children, so teardown must signal the group.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| AcpError::Spawn(format!("cannot start {}: {error}", bin.display())))?;
        let pid = child.id();
        let Some(stdin) = child.stdin.take() else {
            kill_child(&mut child);
            return Err(AcpError::Spawn("the harness has no stdin".to_owned()));
        };
        let Some(stdout) = child.stdout.take() else {
            kill_child(&mut child);
            return Err(AcpError::Spawn("the harness has no stdout".to_owned()));
        };
        let Some(stderr) = child.stderr.take() else {
            kill_child(&mut child);
            return Err(AcpError::Spawn("the harness has no stderr".to_owned()));
        };

        let (updates_tx, updates_rx) = mpsc::unbounded_channel();
        let (requests_tx, requests_rx) = mpsc::unbounded_channel();
        let (writer_tx, writer_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (exit_tx, _) = watch::channel::<Option<AcpExit>>(None);

        let connection = Arc::new(Connection {
            pid: pid.unwrap_or(0),
            writer: Mutex::new(Some(writer_tx)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(0),
            replay: Mutex::new(None),
            load_lock: tokio::sync::Mutex::new(()),
            updates_tx: Mutex::new(Some(updates_tx)),
            updates_rx: Mutex::new(Some(updates_rx)),
            requests_tx: Mutex::new(Some(requests_tx)),
            requests_rx: Mutex::new(Some(requests_rx)),
            stderr_tail: Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)),
            exit: exit_tx,
            killed: AtomicBool::new(false),
        });

        tokio::spawn(read_loop(connection.clone(), BufReader::new(stdout)));
        tokio::spawn(stderr_loop(connection.clone(), stderr));
        tokio::spawn(write_loop(writer_rx, stdin));
        tokio::spawn(wait_loop(connection.clone(), child));

        Ok(connection)
    }

    /// Sends one message and waits for its answer.
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, AcpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.request_with_id(id, method, params, timeout).await
    }

    /// [`Connection::request`] for an id minted elsewhere: the replay capture
    /// must know its own request before the frame goes out.
    pub(crate) async fn request_with_id(
        &self,
        id: i64,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, AcpError> {
        let (sender, receiver) = oneshot::channel();
        {
            if self.writer.lock().is_ok_and(|writer| writer.is_none()) {
                return Err(AcpError::Closed);
            }
            self.pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(id, sender);
        }
        if let Err(error) =
            self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        {
            self.pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&id);
            return Err(error);
        }
        let settled = match timeout {
            Some(limit) => match tokio::time::timeout(limit, receiver).await {
                Ok(settled) => settled,
                Err(_) => {
                    self.pending
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&id);
                    return Err(AcpError::Timeout {
                        operation: match method {
                            "initialize" => "initialize",
                            "session/new" | "session/load" | "session/resume" => "session bind",
                            _ => "ACP request",
                        },
                        millis: limit.as_millis() as u64,
                    });
                }
            },
            None => receiver.await,
        };
        match settled {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(RpcFailure::Rpc {
                code,
                message,
                data,
            })) => Err(AcpError::Rpc {
                code,
                message,
                data,
            }),
            Ok(Err(RpcFailure::Transport { message, tail })) => {
                Err(AcpError::Transport { message, tail })
            }
            // The waiter dropped the sender without a verdict: the process is
            // gone in the same way a transport failure describes.
            Err(_) => Err(AcpError::Transport {
                message: "the harness process is gone".to_owned(),
                tail: self.stderr_tail(),
            }),
        }
    }

    /// Fire-and-forget notification.
    pub(crate) fn notify(&self, method: &str, params: Value) -> Result<(), AcpError> {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    pub(crate) fn respond(&self, id: &RequestId, result: Value) -> Result<(), AcpError> {
        self.send(json!({"jsonrpc": "2.0", "id": id.to_json(), "result": result}))
    }

    pub(crate) fn respond_error(
        &self,
        id: &RequestId,
        code: i64,
        message: &str,
    ) -> Result<(), AcpError> {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id.to_json(),
            "error": { "code": code, "message": message },
        }))
    }

    pub(crate) fn take_updates(&self) -> Result<mpsc::UnboundedReceiver<SessionUpdate>, AcpError> {
        self.updates_rx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or(AcpError::Taken("session-update"))
    }

    pub(crate) fn take_requests(&self) -> Result<mpsc::UnboundedReceiver<HostRequest>, AcpError> {
        self.requests_rx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or(AcpError::Taken("host-request"))
    }

    /// Captures this session's `session/update` frames until the returned
    /// request id's response settles; the call that installs it sends the
    /// request under that id and reads the frames back with
    /// [`ReplayTicket::take`]. The settlement seals the capture even before
    /// the waiter observes the answer — the reader may dispatch a
    /// same-session frame in that gap, and replay frames precede the
    /// response, so anything later is live traffic. Dropping the ticket
    /// without taking (a cancelled load) releases the capture, so live
    /// traffic resumes.
    pub(crate) fn begin_replay(
        &self,
        session_id: &str,
    ) -> Result<(i64, ReplayTicket<'_>), AcpError> {
        let mut replay = self
            .replay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if replay.is_some() {
            return Err(AcpError::Protocol(
                "a session replay is already in flight".to_owned(),
            ));
        }
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        *replay = Some(ReplayCapture {
            request_id,
            session_id: session_id.to_owned(),
            sealed: false,
            frames: Vec::new(),
        });
        Ok((request_id, ReplayTicket { connection: self }))
    }

    fn take_replay(&self) -> Vec<SessionUpdate> {
        self.replay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .map(|capture| capture.frames)
            .unwrap_or_default()
    }

    /// The serialising guard for `session/load`: one replay at a time.
    pub(crate) async fn load_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.load_lock.lock().await
    }

    pub(crate) fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn exit(&self) -> Option<AcpExit> {
        self.exit.borrow().clone()
    }

    pub(crate) fn exit_receiver(&self) -> watch::Receiver<Option<AcpExit>> {
        self.exit.subscribe()
    }

    /// Close stdin: ACP's graceful exit. Idempotent.
    pub(crate) fn finish(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            writer.take();
        }
    }

    /// Hangs up the process group. Idempotent.
    pub(crate) fn kill(&self) {
        self.finish();
        if self.killed.swap(true, Ordering::AcqRel) {
            return;
        }
        // A reaped child's group may already be gone; never signal a pid the
        // exit already retired.
        if self.exit().is_some() {
            return;
        }
        #[cfg(unix)]
        if self.pid > 0 && self.pid <= i32::MAX as u32 {
            // SAFETY: `kill` is the platform's signalling call. A negative pid
            // names the child's process group; an already-reaped group is
            // harmless to signal.
            unsafe {
                libc::kill(-(self.pid as i32), libc::SIGKILL);
                libc::kill(self.pid as i32, libc::SIGKILL);
            }
        }
    }

    /// The graceful close: EOF, then the group is the fallback.
    pub(crate) async fn close(&self) {
        self.finish();
        let mut exit = self.exit_receiver();
        if exit.borrow().is_none() {
            let _ = tokio::time::timeout(EXIT_GRACE, exit.wait_for(|value| value.is_some())).await;
        }
        if exit.borrow().is_none() {
            self.kill();
        }
    }

    fn send(&self, message: Value) -> Result<(), AcpError> {
        let mut frame = serde_json::to_vec(&message)
            .map_err(|error| AcpError::Protocol(format!("cannot encode a frame: {error}")))?;
        frame.push(b'\n');
        let writer = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(writer) = writer.as_ref() else {
            return Err(AcpError::Closed);
        };
        writer.send(frame).map_err(|_| AcpError::Closed)
    }

    fn settle(&self, id: i64, outcome: Result<Value, RpcFailure>) {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id);
        // The response seals its replay capture before the waiter observes
        // the answer: dispatch is sequential, so a same-session frame read
        // after the response would otherwise land in the capture while the
        // load call is still on its way to take the frames.
        {
            let mut replay = self
                .replay
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(capture) = replay.as_mut()
                && capture.request_id == id
            {
                capture.sealed = true;
            }
        }
        if let Some(sender) = pending {
            let _ = sender.send(outcome);
        }
    }

    fn push_stderr(&self, line: String) {
        let mut tail = self
            .stderr_tail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if tail.len() == STDERR_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The reader: one line in, one dispatch out. It never awaits a consumer: the
/// channels are unbounded, so a slow host cannot stall the harness's stdout.
/// A framing failure is fatal for the connection: an overrun line means the
/// stream is desynced, and a child left writing into a pipe nobody drains
/// would never exit — hang up so the waiter settles every pending call with
/// the stderr tail.
async fn read_loop(connection: Arc<Connection>, mut stdout: BufReader<ChildStdout>) {
    let mut line = Vec::new();
    loop {
        match next_line(&mut stdout, &mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&line);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(message) => dispatch(&connection, message),
                    Err(error) => connection.push_stderr(format!(
                        "acp: an unparseable frame was dropped ({error}): {}",
                        truncate(trimmed, 200)
                    )),
                }
            }
            Err(error) => {
                connection.push_stderr(format!("acp: stdout framing failed: {error}"));
                connection.kill();
                break;
            }
        }
    }
}

/// Reads one newline-terminated line, enforcing the frame budget. Returns the
/// bytes read (0 at EOF).
async fn next_line(reader: &mut BufReader<ChildStdout>, line: &mut Vec<u8>) -> io::Result<usize> {
    line.clear();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            // EOF ends the stream: a trailing fragment without a newline is a
            // torn frame, not a message.
            return Ok(0);
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                line.extend_from_slice(&available[..=end]);
                reader.consume(end + 1);
                return Ok(line.len());
            }
            None => {
                line.extend_from_slice(available);
                let consumed = available.len();
                reader.consume(consumed);
                if line.len() > MAX_LINE_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("a protocol line exceeded {MAX_LINE_BYTES} bytes"),
                    ));
                }
            }
        }
    }
}

/// Classifies one inbound message: response, server→client request, or
/// notification. Anything else is a protocol note on the stderr tail.
fn dispatch(connection: &Arc<Connection>, message: Value) {
    let Some(object) = message.as_object() else {
        connection.push_stderr("acp: a frame was not a JSON object".to_owned());
        return;
    };
    let id = object.get("id").filter(|value| !value.is_null());
    let method = object.get("method").and_then(Value::as_str);
    match (id, method) {
        (Some(id), None) => {
            let Some(id) = id.as_i64() else {
                connection.push_stderr("acp: a response carried a non-numeric id".to_owned());
                return;
            };
            if let Some(error) = object.get("error") {
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32603);
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the harness returned an unnamed error")
                    .to_owned();
                let data = error.get("data").cloned().unwrap_or(Value::Null);
                connection.settle(
                    id,
                    Err(RpcFailure::Rpc {
                        code,
                        message,
                        data,
                    }),
                );
            } else {
                let result = object.get("result").cloned().unwrap_or(Value::Null);
                connection.settle(id, Ok(result));
            }
        }
        (Some(id), Some(method)) => {
            let Some(id) = RequestId::from_json(id) else {
                connection.push_stderr(format!(
                    "acp: a request for {method} carried an unusable id"
                ));
                return;
            };
            let request = HostRequest {
                id,
                method: method.to_owned(),
                params: object.get("params").cloned().unwrap_or(Value::Null),
            };
            let sender = connection
                .requests_tx
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let delivered = sender.is_some_and(|sender| sender.send(request.clone()).is_ok());
            if !delivered {
                // No host is listening: refuse rather than leave the harness
                // waiting forever for an answer nobody will write.
                let _ = connection.respond_error(
                    &request.id,
                    -32601,
                    &format!("the host does not serve {}", request.method),
                );
            }
        }
        (None, Some("session/update")) => {
            let params = object.get("params");
            let session_id = params
                .and_then(|params| params.get("sessionId"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let Some(update) = params.and_then(|params| params.get("update")).cloned() else {
                connection.push_stderr("acp: a session/update carried no update".to_owned());
                return;
            };
            let update = SessionUpdate { session_id, update };
            {
                let mut replay = connection
                    .replay
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(capture) = replay.as_mut()
                    && !capture.sealed
                    && capture.session_id == update.session_id
                {
                    capture.frames.push(update);
                    return;
                }
            }
            let sender = connection
                .updates_tx
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(sender) = sender {
                let _ = sender.send(update);
            }
        }
        (None, Some(method)) => {
            connection.push_stderr(format!("acp: the notification {method} has no consumer"));
        }
        (None, None) => {
            connection.push_stderr("acp: a frame carried neither id nor method".to_owned());
        }
    }
}

async fn stderr_loop(connection: Arc<Connection>, stderr: tokio::process::ChildStderr) {
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => connection.push_stderr(line.trim_end().to_owned()),
            Err(error) => {
                connection.push_stderr(format!("acp: stderr read failed: {error}"));
                break;
            }
        }
    }
}

async fn write_loop(
    mut frames: mpsc::UnboundedReceiver<Vec<u8>>,
    mut stdin: tokio::process::ChildStdin,
) {
    while let Some(frame) = frames.recv().await {
        if stdin.write_all(&frame).await.is_err() {
            break;
        }
        if stdin.flush().await.is_err() {
            break;
        }
    }
    // Dropping stdin is the EOF the harness treats as the graceful exit.
    let _ = stdin.shutdown().await;
}

async fn wait_loop(connection: Arc<Connection>, mut child: Child) {
    let status = child.wait().await;
    let exit = match status {
        Ok(status) => exit_of(&status),
        Err(_) => AcpExit::default(),
    };
    // Close the host-facing channels first: a pump ends on channel closure,
    // and the pending failures below are what a caller awaits.
    connection
        .writer
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    connection
        .updates_tx
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    connection
        .requests_tx
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let tail = connection.stderr_tail();
    let message = match (exit.code, &exit.signal) {
        (Some(code), _) => format!("the harness exited (exit {code})"),
        (None, Some(signal)) => format!("the harness exited ({signal})"),
        (None, None) => "the harness exited".to_owned(),
    };
    let pending: Vec<_> = connection
        .pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .drain()
        .map(|(_, sender)| sender)
        .collect();
    for sender in pending {
        let _ = sender.send(Err(RpcFailure::Transport {
            message: message.clone(),
            tail: tail.clone(),
        }));
    }
    // `send_replace` stores the value even when no receiver is listening:
    // exit is a durable fact read via `borrow`, not an event to race.
    connection.exit.send_replace(Some(exit));
}

fn exit_of(status: &std::process::ExitStatus) -> AcpExit {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return AcpExit {
                code: None,
                signal: Some(signal_name(signal)),
            };
        }
    }
    AcpExit {
        code: status.code(),
        signal: None,
    }
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    let name = match signal {
        libc::SIGINT => "SIGINT",
        libc::SIGKILL => "SIGKILL",
        libc::SIGTERM => "SIGTERM",
        libc::SIGPIPE => "SIGPIPE",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGABRT => "SIGABRT",
        _ => return format!("signal {signal}"),
    };
    name.to_owned()
}

fn kill_child(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id()
        && pid <= i32::MAX as u32
    {
        // SAFETY: see `Connection::kill`; the group was created above.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

fn truncate(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
