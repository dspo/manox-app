//! Ghostty-backed terminal surface: the libghostty-vt emulation core (Zig,
//! statically linked) behind a self-contained GPUI element.
//!
//! Public surface:
//! - [`spawn_ghostty_view`] — spawn a `$SHELL` PTY rooted at a directory and
//!   return a ready-to-embed [`GhosttyView`]; [`Theme`] carries the embedder's
//!   foreground/background (this crate owns no palette)
//! - [`GhosttyView`] — the component: rendering, keyboard/IME input,
//!   render-driven resize, paste; [`GhosttyView::live_title`] mirrors the
//!   terminal's OSC title for embedders that show one, and
//!   [`GhosttyView::terminate`] ends the shell (dropping the view does not:
//!   the pty is shared with its watcher threads)
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
//! Known gaps (deliberate, tracked for the replacement series — the integrated
//! `terminal-ui` surface still has them):
//! - no text selection / copy (and therefore no `cmd+c`), no scrollback
//!   indicator, no search overlay, no vi mode;
//! - no mouse reporting: TUIs that ask for it (`vim`, `tmux`, `htop`) see the
//!   wheel as scrollback only and never receive clicks;
//! - a paste libghostty refuses (`GHOSTTY_REJECTED`: the text could inject
//!   commands outside bracketed-paste mode) is dropped; the
//!   confirm-then-retry-with-`allow_unsafe` flow has no surface yet;
//! - no `[terminal]` settings: shell comes from `$SHELL` (no override, no extra
//!   env), and the font is the platform mono face at a fixed size;
//! - in-flight IME composition is not painted (committed text is).

mod input;
mod pty;
pub mod terminal;
mod view;

#[doc(hidden)]
pub mod sys;

pub use terminal::{Cell, Frame, KeyEncoder, RenderState, Rgb, Terminal};
pub use view::{GhosttyView, Theme, spawn_ghostty_view};
