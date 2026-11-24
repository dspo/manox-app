//! The edit family's full-diff face: the right pane's diff tab. The chat
//! column's edit cards render the patch as a capped in-band summary; the
//! card's open-diff chip routes here, where the patch is parsed with the
//! upstream `gpui_component::diff` parser and the pane's ONE diff tab takes
//! the result — a later open replaces the content instead of stacking tabs.
//!
//! Ownership: the workspace holds the parsed [`gpui_component::diff::DiffState`]
//! (entity creation stays on this event path — the pane's `render` only
//! reads), the tab in `tool_tabs` only mirrors it. The payload is
//! session-lived like the sub-agent panels: the workspace-wide state is the
//! LATEST patch anywhere, so the tab is retired on thread switch rather than
//! carried across one.

use std::path::Path;

use gpui::{AppContext as _, Context, Entity, SharedString};
use gpui_component::diff::{DiffFile, DiffState};

use super::Workspace;

/// The right pane's diff tab content: the tab label plus the parsed patch —
/// `Err` carries the upstream parse error, so a malformed patch still opens
/// the tab and says why it cannot render.
impl Workspace {
    /// The edit card's open-diff entry: parse the patch, replace the diff
    /// tab's content (one tab, however many cards open it), and focus the
    /// tab. A parse failure opens the tab onto the error instead of
    /// silently dropping the click.
    pub(crate) fn open_right_diff(
        &mut self,
        path: Option<String>,
        patch: String,
        cx: &mut Context<Self>,
    ) {
        let patch = strip_diff_envelope(&patch);
        let parsed = DiffFile::parse(patch);
        let title: SharedString = diff_tab_title(path.as_deref(), parsed.as_deref().ok());
        let state: Result<Entity<gpui_component::diff::DiffState>, SharedString> = parsed
            .map(|files| cx.new(|cx| DiffState::new(files, cx)))
            .map_err(|e| SharedString::from(e.to_string()));
        // The sheet lands in the container's own right dock: per-container
        // canvases give the per-thread ephemeral scoping by construction
        // (the pane era retired the global close-on-rebind leg with it).
        let canvas = self.ensure_canvas_view(cx);
        let sheet =
            steer_canvas::id::diff_sheet_id(&self.foreground_or_attached(cx), "patch", "current");
        canvas.update(cx, |canvas, cx| {
            canvas.panes.update(cx, |panes, cx| {
                panes.put_diff(&sheet, title, state, cx);
            });
            canvas.open_in_right_dock(&sheet, cx);
        });
    }

    /// The container id the canvas keys its sheets by (the canvas host's
    /// own canvas key: the foreground container, or the scratch surface).
    fn foreground_or_attached(&self, cx: &Context<Self>) -> String {
        self.canvas_view
            .as_ref()
            .map(|canvas| canvas.read(cx).host.canvas_key().to_string())
            .unwrap_or_else(|| "scratch".to_string())
    }

    /// Probe: the diff tab's current content — `(label, parsed file count)`;
    /// `None` while nothing opened a diff (or the patch failed to parse).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_right_diff(&self, cx: &gpui::App) -> Option<(String, usize)> {
        let canvas = self.canvas_view.as_ref()?;
        let container = canvas.read(cx).host.canvas_key().to_string();
        let sheet = steer_canvas::id::diff_sheet_id(&container, "patch", "current");
        canvas.read(cx).panes.read(cx).diagnostic_diff(&sheet, cx)
    }

    /// Probe: drive the same entry the edit card's chip routes to (the
    /// workspace method is crate-private; the probes live outside).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_open_right_diff(
        &mut self,
        path: Option<String>,
        patch: String,
        cx: &mut Context<Self>,
    ) {
        self.open_right_diff(path, patch, cx);
    }
}

/// Drops the harness envelope line (`[path#TAG]`) the runtime prefixes the
/// edit output with — the same first-line rule the card's hand parse runs,
/// so the upstream parser only ever sees the diff itself.
fn strip_diff_envelope(patch: &str) -> &str {
    match patch.split_once('\n') {
        Some((first, rest)) if first.starts_with('[') && first.ends_with(']') => rest,
        _ => patch,
    }
}

/// The tab label: the call's target file name when the input carried a path,
/// else the patch's own first file — runtime values, rendered verbatim (never
/// localized); the fallback is the kind's chrome name.
fn diff_tab_title(path: Option<&str>, files: Option<&[DiffFile]>) -> SharedString {
    let name = path.and_then(file_name_of).or_else(|| {
        files
            .and_then(|f| f.first())
            .and_then(|f| file_name_of(f.path()))
    });
    name.map(SharedString::from)
        .unwrap_or_else(|| crate::i18n::t("chrome-tab-diff"))
}

fn file_name_of(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The data plane the tab stands on: the app's real edit-output shape
    /// (envelope line, `---`/`+++` headers, two hunks with mixed context and
    /// changed rows) parses into ONE file whose changed-line counts match the
    /// patch. This pins the hand-off from the runtime's patch text to the
    /// upstream parser — a regression here blanks the tab, not just a card.
    #[test]
    fn edit_patch_parses_to_one_file_with_both_hunks() {
        let patch = r#"[crates/agent-ui/src/workspace.rs#T4K2]
--- a/crates/agent-ui/src/workspace.rs
+++ b/crates/agent-ui/src/workspace.rs
@@ -3,6 +3,7 @@
 use std::collections::HashMap;
 use std::path::PathBuf;
+
 use crate::i18n;
 use gpui::ClickEvent;
-use gpui::DismissEvent;
+use gpui::DismissEvent as Dismiss;

@@ -41,4 +42,5 @@
 fn main() {
 let x = 1;
+let y = 2;
 println!("{x}");
 }
"#;
        let files = DiffFile::parse(strip_diff_envelope(patch)).expect("the patch parses");
        assert_eq!(files.len(), 1, "one edit produces one file");
        assert_eq!(files[0].path().as_str(), "crates/agent-ui/src/workspace.rs");
        assert_eq!(files[0].additions(), 3, "two adds in hunk 1, one in hunk 2");
        assert_eq!(files[0].deletions(), 1, "one rewrite in hunk 1");
    }

    /// The runtime's bracketed envelope is app chrome, not patch: it must be
    /// gone before the upstream parse (and a patch with no envelope passes
    /// through untouched).
    #[test]
    fn the_envelope_line_strips_and_a_bare_patch_passes_through() {
        let enveloped =
            "[src/lib.rs#T4K2]\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n";
        let stripped = strip_diff_envelope(enveloped);
        assert!(!stripped.starts_with('['));
        assert!(stripped.starts_with("--- a/"));

        let bare = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n";
        assert_eq!(strip_diff_envelope(bare), bare);
    }

    /// The tab label prefers the call's target file name, falls back to the
    /// patch's own first file, and only then to the chrome name. Runtime
    /// values render verbatim.
    #[test]
    fn the_tab_label_prefers_the_call_path_then_the_patch() {
        let files = DiffFile::parse(
            "--- a/src/other.rs\n+++ b/src/other.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n",
        )
        .expect("the patch parses");
        assert_eq!(
            diff_tab_title(Some("deep/dir/callsite.rs"), Some(&files)),
            "callsite.rs"
        );
        assert_eq!(diff_tab_title(None, Some(&files)), "other.rs");
        assert_eq!(diff_tab_title(Some(""), Some(&files)), "other.rs");
        assert_eq!(
            diff_tab_title(None, None),
            crate::i18n::t("chrome-tab-diff")
        );
    }
}
