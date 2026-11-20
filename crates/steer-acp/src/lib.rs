//! The ACP client seam over a `pi-acp` child process: spawn, newline-delimited
//! JSON-RPC 2.0 framing, session lifecycle, and the two host-facing channels
//! (session updates, server→client requests).
//!
//! gpui-free by construction, like `steer-term-process`: a host that owns a
//! process (the pi sheet's store) consumes [`AcpProcess`] without pulling a GUI
//! stack. The protocol authority is the pi-harness in this repository
//! (`pi-harness/src/acp/app.ts`); this crate never re-derives the wire, only
//! speaks it.
//!
//! ```no_run
//! # use std::path::Path;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use steer_acp::{AcpEnv, AcpOptions, AcpProcess};
//!
//! let process = AcpProcess::spawn(AcpOptions::new(
//!     steer_acp::resolve_pi_acp().ok_or("pi-acp not found")?,
//!     Path::new("."),
//!     AcpEnv::for_state_dir(Path::new("/home/me/.steer")),
//! ))?;
//! let updates = process.updates()?;
//! let requests = process.requests()?;
//! process.initialize("steer", env!("CARGO_PKG_VERSION")).await?;
//! let session = process.new_session(Some(&steer_acp::mint_session_id()), Path::new(".")).await?;
//! let _ = (updates, requests, session);
//! process.close().await;
//! # Ok(())
//! # }
//! ```

mod error;
mod process;
mod session;
mod transport;
mod types;

pub use error::AcpError;
pub use process::{AcpEnv, AcpOptions, AcpProcess, resolve_pi_acp};
pub use session::SessionProxy;
pub use types::{AcpExit, HostRequest, InitializeResult, RequestId, SessionInfo, SessionUpdate};

/// The ACP protocol version this client speaks.
pub const PROTOCOL_VERSION: i64 = 1;

/// A fresh client-minted pi session id.
///
/// pi's `session/new` pairing extension requires a UUID (the harness refuses
/// anything else), and pi persists session files under this id, so it must be
/// minted once and kept for the session's whole life.
pub fn mint_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
