//! One session on a live connection: prompts, cancellation, selectors, and
//! the pi extension requests that name it.

use std::fmt;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::AcpError;
use crate::transport::Connection;
use crate::types::SessionInfo;

/// A session handle. Clones share the connection; the session ends when the
/// harness is told to close it or the process goes away.
#[derive(Clone)]
pub struct SessionProxy {
    connection: Arc<Connection>,
    info: SessionInfo,
}

impl fmt::Debug for SessionProxy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionProxy")
            .field("session_id", &self.info.session_id)
            .finish_non_exhaustive()
    }
}

impl SessionProxy {
    pub(crate) fn new(connection: Arc<Connection>, info: SessionInfo) -> Self {
        Self { connection, info }
    }

    /// The ACP session id (pi's session-file key).
    pub fn id(&self) -> &str {
        &self.info.session_id
    }

    /// What `session/new|load|resume` announced: the raw answer plus the
    /// config selectors and approval modes.
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    /// Runs one turn and settles with the stop reason (`end_turn`,
    /// `cancelled`, …). Streamed updates arrive on the process's update
    /// channel while this call is pending.
    pub async fn prompt(&self, text: &str) -> Result<String, AcpError> {
        let result = self
            .connection
            .request(
                "session/prompt",
                json!({
                    "sessionId": self.info.session_id,
                    "prompt": [{ "type": "text", "text": text }],
                }),
                None,
            )
            .await?;
        Ok(result
            .get("stopReason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    /// Stops the running turn; the pending [`SessionProxy::prompt`] settles
    /// `cancelled`.
    pub fn cancel(&self) -> Result<(), AcpError> {
        self.connection.notify(
            "session/cancel",
            json!({ "sessionId": self.info.session_id }),
        )
    }

    /// `session/set_config_option` (model, thought level): the fresh selector
    /// list.
    pub async fn set_config_option(&self, config_id: &str, value: &str) -> Result<Value, AcpError> {
        let result = self
            .connection
            .request(
                "session/set_config_option",
                json!({
                    "sessionId": self.info.session_id,
                    "configId": config_id,
                    "value": value,
                }),
                None,
            )
            .await?;
        Ok(result.get("configOptions").cloned().unwrap_or(Value::Null))
    }

    /// `session/set_mode` (the approval policy).
    pub async fn set_mode(&self, mode_id: &str) -> Result<(), AcpError> {
        self.connection
            .request(
                "session/set_mode",
                json!({ "sessionId": self.info.session_id, "modeId": mode_id }),
                None,
            )
            .await
            .map(|_| ())
    }

    /// A pi extension request (`_zu/subagents/dispatch`, `_zu/todos/write`,
    /// …). `params` must carry `sessionId` when the method is session-scoped;
    /// [`SessionProxy::request_scoped`] adds it.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        self.connection.request(method, params, None).await
    }

    /// The same call with this session's id merged into the params.
    pub async fn request_scoped(&self, method: &str, mut params: Value) -> Result<Value, AcpError> {
        if let Some(object) = params.as_object_mut() {
            object.insert(
                "sessionId".to_owned(),
                Value::from(self.info.session_id.clone()),
            );
        }
        self.connection.request(method, params, None).await
    }

    /// `session/close`: free the session, keep the process. A failure is the
    /// caller's to log; the process stays usable either way.
    pub async fn close(&self) -> Result<(), AcpError> {
        self.connection
            .request(
                "session/close",
                json!({ "sessionId": self.info.session_id }),
                None,
            )
            .await
            .map(|_| ())
    }
}
