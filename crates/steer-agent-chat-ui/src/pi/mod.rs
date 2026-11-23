//! The pi harness's host-side data plane.
//!
//! | module | what it is | gpui |
//! |---|---|---|
//! | [`transcript`] | the pure ACP-update reducer | no |
//! | [`session`] | one session's protocol faces: selectors, usage, banner, pending requests | no |
//! | [`book`] | every open session, folded synchronously from transport events | no |
//! | [`runtime`] | spawn/handshake/bind + two channel pumps, over `steer-acp` | no |
//!
//! The layering mirrors [`crate::ahp_store`]: the transport reports events, a
//! pure fold builds the state a view reads, and the gpui entity that owns the
//! runtime lives in the assembly (the session sheet's views consume
//! [`PiBook`]).
//!
//! Two invariants the pi leg must not lose:
//!
//! - **The settle marker is the entry's identity.** Live text drafts under its
//!   wire `messageId`; the marker re-keys the row onto pi's entry id in place,
//!   so a rebuilt transcript and a streamed one agree row for row.
//! - **The optimistic row is the user's.** The entry chunk adopts it in place
//!   (even when pi expanded the text), and everything the turn produces is
//!   inserted ahead of rows queued while it ran.

pub mod book;
pub mod runtime;
pub mod session;
pub mod store;
pub mod transcript;

pub use book::PiBook;
pub use runtime::{OpenMode, PiConnection, PiRuntime, StoreEvent};
pub use session::{
    PendingRequest, PiBanner, PiBannerEntry, PiBannerWarning, PiCommand, PiConfigChoice,
    PiConfigOption, PiMode, PiPlan, PiRestoredAsk, PiSessionState, PiSessionView,
    PiSubagentMessage, PiSubagentRun, PiSubagentStatus, PiTodo, PiTodoGroup, PiTodoStatus, PiUsage,
};
/// The host mints pi's session id (a UUID pi's pairing extension requires).
pub use steer_acp::mint_session_id;
pub use store::{AcpStore, fallback_title};
pub use transcript::{
    AssistantEntry, DiffBlock, Draft, Entry, NoteEntry, PiMeta, PiModel, ToolEntry, ToolMeta,
    ToolStatus, Transcript, UserEntry,
};
