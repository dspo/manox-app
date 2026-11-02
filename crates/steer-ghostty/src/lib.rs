//! Ghostty-backed terminal surface: the libghostty-vt emulation core (Zig,
//! statically linked) behind a self-contained GPUI element.
//!
//! Public surface:
//! - [`spawn_ghostty_view`] — spawn a `$SHELL` PTY rooted at a directory and
//!   return a ready-to-embed [`GhosttyView`]
//! - [`GhosttyView`] — the component: rendering, keyboard/mouse input,
//!   render-driven resize, paste; [`GhosttyView::live_title`] mirrors the
//!   terminal's OSC title for embedders that show one
//! - [`terminal`] — the safe wrapper over the C API (terminal state, render
//!   snapshots, key encoding), for callers that need to go below the view
//! - [`sys`] — raw FFI bindings (doc-hidden; the transcription reference is
//!   the Ghostty rev pinned in `build.rs`)

mod input;
mod pty;
pub mod terminal;
mod view;

#[doc(hidden)]
pub mod sys;

pub use terminal::{Cell, Frame, KeyEncoder, RenderState, Rgb, Terminal};
pub use view::{GhosttyView, spawn_ghostty_view};
