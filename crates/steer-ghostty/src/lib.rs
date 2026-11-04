//! Ghostty-backed terminal surface: the libghostty-vt emulation core (Zig,
//! statically linked) behind a self-contained GPUI element.
//!
//! Public surface:
//! - [`spawn_ghostty_view`] — spawn a `$SHELL` PTY rooted at a directory and
//!   return a ready-to-embed [`GhosttyView`]; [`Theme`] carries the embedder's
//!   foreground/background (this crate owns no palette)
//! - [`GhosttyView`] — the component: rendering, keyboard/IME input,
//!   render-driven resize, paste; [`GhosttyView::live_title`] mirrors the
//!   terminal's OSC title, and [`GhosttyView::kill`] ends the process
//!   (dropping the view kills it too)
//! - [`TerminalProcess`] / [`ProcessOutput`] — the process abstraction for
//!   hosts whose process is owned elsewhere (cx sessions implement it; the
//!   view kills it on drop). The trait itself is gpui-free: it lives in
//!   `steer-term-process`, re-exported here
//! - [`terminal`] — the safe wrapper over the C API (terminal state, render
//!   snapshots, key encoding), for callers that need to go below the view
//! - [`sys`] — raw FFI bindings (doc-hidden; the transcription reference is
//!   the Ghostty rev pinned in `build.rs`)
//!
//! Build prerequisites and the artifact cache are documented in `build.rs`:
//! `GHOSTTY_SRC` / `GHOSTTY_ZIG` override the sources and toolchain, and
//! `STEER_GHOSTTY_CACHE` moves the build cache out of `OUT_DIR` (CI restores
//! one so the zig build is not paid per job).
//!
//! Known gaps (deliberate, accepted with the migration — these retire with
//! the removed manox-terminal stack rather than being re-implemented here):
//! - no text selection / copy (and therefore no `cmd+c`), no scrollback
//!   indicator, no search overlay, no vi mode;
//! - no mouse reporting: TUIs that ask for it (`vim`, `tmux`, `htop`) see the
//!   wheel as scrollback only and never receive clicks;
//! - a paste libghostty refuses (`GHOSTTY_REJECTED`: the text could inject
//!   commands outside bracketed-paste mode) is dropped; the
//!   confirm-then-retry-with-`allow_unsafe` flow has no surface yet;
//! - the `[terminal]` settings face is partial: `shell` and `env` are
//!   consumed (see [`settings`]), while font / cursor / bell / scrollback /
//!   osc52 / commands_to_skip_shell stay fixed (the font is the platform
//!   mono face at a fixed size);
//! - in-flight IME composition is not painted (committed text is).

mod input;
pub use steer_term_process::{ProcessOutput, TerminalProcess};
pub mod pty;
pub mod settings;
pub mod terminal;
mod view;

#[doc(hidden)]
pub mod sys;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use terminal::{Cell, Frame, KeyEncoder, RenderState, Rgb, Terminal};
pub use view::{GhosttyEvent, GhosttyView, Theme, spawn_ghostty_view};
