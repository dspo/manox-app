//! Markdown rendering: the app's `TextView` assembly plus the terminal panel.
//!
//! Rendering, parsing, selection and copy ride upstream gpui-base `TextView`
//! (the old self-hosted ast/incremental/rich_text/selection stack retired —
//! its three original motivations — streaming, cross-block selection, table
//! source copy — are all first-class upstream). What remains app-side:
//!
//! - [`text_view`] — the single assembly point (theme bridge, heading
//!   ladders, code-highlight dispatch, copy controls);
//! - [`linkify`] — source-level bare-URL / file-path linking (upstream has no
//!   autolink that can split a text node);
//! - [`svg_block`] — ```svg fences as their rasterized image, with a flip to
//!   the highlighted source;
//! - [`mermaid_block`] — ```mermaid fences rendered to an SVG document at
//!   parse time, then painted by the same raster block;
//! - [`math_block`] — `$$…$$` as a display block and `$…$` as an inline run,
//!   typeset to an SVG document by the pure-Rust engine behind `mathra`;
//! - [`terminal_panel`] — the terminal-styled tool-output panel, hosted on a
//!   `TextView` code block.
//!
//! The upstream [`TextViewState`] type is re-exported here so consumers name
//! one path for the entity they own.

pub mod linkify;
pub mod math_block;
pub mod mermaid_block;
pub mod svg_block;
pub mod terminal_panel;
pub mod text_view;

pub use gpui_base::TextView;
pub use gpui_base::text::TextViewState;
pub use linkify::{LinkKind, classify_target, linkify_markdown};
pub use svg_block::with_diagram_blocks;
pub use terminal_panel::{GitSummary, PanelKind, TerminalPanel, hashline_text};
pub use text_view::{
    ChatDocument, CodeHighlighter, chrome_style, code_copy_actions, install_code_highlighter,
    install_selection_drag_guard, message_document, message_style, panel_document,
    panel_highlighter, panel_styled_runs, reasoning_document, static_document,
    steer_code_highlighter, stream_fade_motion, table_copy_actions,
};
