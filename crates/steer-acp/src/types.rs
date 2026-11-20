//! The protocol vocabulary the host consumes. Deliberately shallow: the pi
//! harness owns its `sessionUpdate` variants, so an update keeps its JSON
//! payload intact and the consumer's reducer reads it.

use serde_json::Value;

/// One `session/update` notification: the session it belongs to and the
/// `update` object verbatim.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionUpdate {
    pub session_id: String,
    pub update: Value,
}

impl SessionUpdate {
    /// The `sessionUpdate` discriminator (`agent_message_chunk`, …).
    pub fn kind(&self) -> Option<&str> {
        self.update.get("sessionUpdate").and_then(Value::as_str)
    }
}

/// A server→client request the host must answer with [`crate::AcpProcess::respond`]
/// or [`crate::AcpProcess::respond_error`]: tool approval, elicitation, and the
/// pi extension requests (`_zu/ask/create`, …).
#[derive(Debug, Clone, PartialEq)]
pub struct HostRequest {
    pub id: RequestId,
    pub method: String,
    pub params: Value,
}

/// A JSON-RPC id, echoed back with the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl RequestId {
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_i64().map(RequestId::Number),
            Value::String(text) => Some(RequestId::String(text.clone())),
            _ => None,
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            RequestId::Number(number) => Value::from(*number),
            RequestId::String(text) => Value::from(text.clone()),
        }
    }
}

/// How the harness process ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcpExit {
    /// The exit status, `None` when a signal ended it.
    pub code: Option<i32>,
    /// The signal's name (`SIGTERM`), `None` on a normal exit.
    pub signal: Option<String>,
}

/// The `initialize` answer, reduced to what a host shows and checks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InitializeResult {
    pub protocol_version: i64,
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
    pub raw: Value,
}

/// A minted or reattached session: the id plus the two selectors the harness
/// announces with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionInfo {
    pub session_id: String,
    /// `configOptions` verbatim: model, thought level, and extension switches.
    pub config_options: Value,
    /// `modes` verbatim: the approval policies.
    pub modes: Value,
    pub raw: Value,
}
