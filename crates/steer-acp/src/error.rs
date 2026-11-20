//! Failures a caller can tell apart: spawn, transport death, protocol break,
//! local closure, and the harness's own coded JSON-RPC errors.

use std::fmt;

use serde_json::Value;

/// One ACP call's failure.
#[derive(Debug, Clone)]
pub enum AcpError {
    /// The child could not be started.
    Spawn(String),
    /// Reading or writing the child's stdio failed.
    Io(String),
    /// The peer broke the framing or the JSON-RPC envelope.
    Protocol(String),
    /// A bounded operation (initialize, session bind) did not answer in time.
    Timeout {
        operation: &'static str,
        millis: u64,
    },
    /// The process is gone; `tail` is its stderr as of the failure.
    Transport { message: String, tail: String },
    /// The harness answered with a JSON-RPC error.
    Rpc {
        code: i64,
        message: String,
        data: Value,
    },
    /// The connection was closed locally before the call settled.
    Closed,
    /// A one-shot channel (`updates`/`requests`) was already taken.
    Taken(&'static str),
}

impl AcpError {
    /// The harness's string code (`data.code`), the vocabulary callers branch
    /// on — e.g. `session_not_found`.
    pub fn data_code(&self) -> Option<&str> {
        match self {
            AcpError::Rpc { data, .. } => data.get("code").and_then(Value::as_str),
            _ => None,
        }
    }

    /// Whether a `session/load` or `session/resume` found no session file.
    /// The caller rebuilds under the same id, as `session_not_found` means
    /// "this connection has no such session", not "the id is invalid".
    pub fn is_session_not_found(&self) -> bool {
        matches!(self, AcpError::Rpc { code: -32002, .. })
            || self.data_code() == Some("session_not_found")
    }

    /// Whether the failure means the process can no longer serve calls.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            AcpError::Spawn(_) | AcpError::Transport { .. } | AcpError::Closed | AcpError::Io(_)
        )
    }

    /// Text for an error line: the harness's code first when it gave one.
    pub fn describe(&self) -> String {
        match self {
            AcpError::Rpc { message, .. } => match self.data_code() {
                Some(code) => format!("{code}: {message}"),
                None => message.clone(),
            },
            other => other.to_string(),
        }
    }
}

impl fmt::Display for AcpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcpError::Spawn(message) => write!(formatter, "cannot start the harness: {message}"),
            AcpError::Io(message) => write!(formatter, "harness I/O failed: {message}"),
            AcpError::Protocol(message) => {
                write!(formatter, "the harness broke the protocol: {message}")
            }
            AcpError::Timeout { operation, millis } => {
                write!(formatter, "{operation} did not answer within {millis} ms")
            }
            AcpError::Transport { message, tail } => {
                write!(formatter, "{message}")?;
                let tail = tail.trim();
                if tail.is_empty() {
                    Ok(())
                } else {
                    write!(formatter, " — harness stderr tail: {tail}")
                }
            }
            AcpError::Rpc { code, message, .. } => {
                write!(formatter, "{message} (JSON-RPC error {code})")
            }
            AcpError::Closed => write!(formatter, "the ACP connection is closed"),
            AcpError::Taken(channel) => {
                write!(formatter, "the {channel} channel was already taken")
            }
        }
    }
}

impl std::error::Error for AcpError {}

/// What a pending request settles with inside the transport. The public
/// error is built at the call site, where the stderr tail is current.
pub(crate) enum RpcFailure {
    Rpc {
        code: i64,
        message: String,
        data: Value,
    },
    Transport {
        message: String,
        tail: String,
    },
}
