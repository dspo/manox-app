//! The edit cards' full-diff entry: the workspace parse lands in the right
//! pane's ONE diff tab (a second open replaces the content instead of
//! stacking a tab), the tab paints the upstream `Diff` body, and the label
//! follows the opened file. Driven through the same workspace entry the
//! card chip routes to, against the production chrome assembly.
//!
//! Own test binary: shared harness singletons (see `tests/common/mod.rs`).
#![cfg(feature = "test-support")]

mod common;

use agent_ui::tool_tabs::DIFF_KIND;
use common::{PARK_ROUNDS, PARK_STEP, full_wiring_boot, full_wiring_teardown};
use gpui::{AppContext as _, TestAppContext, VisualTestContext, px};

/// The app's real edit-output shape: an envelope line over a plain unified
/// diff with one file and two hunks.
const ONE_FILE_PATCH: &str = r#"[crates/agent-ui/src/workspace.rs#T4K2]
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

/// A git-style patch over two files — the replace leg's content.
const TWO_FILE_PATCH: &str = r#"diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@
 fn a() {}
+fn a2() {}

 fn b() {}
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1,1 +1,2 @@
 struct B;
+impl B {}
"#;

fn open_diff(
    cx: &mut TestAppContext,
    ws: &gpui::Entity<agent_ui::Workspace>,
    window: gpui::AnyWindowHandle,
    path: Option<&str>,
    patch: &str,
) {
    let ws = ws.clone();
    let path = path.map(str::to_string);
    let patch = patch.to_string();
    cx.update_window(window, move |_, _, cx| {
        ws.update(cx, |ws, cx| ws.diagnostic_open_right_diff(path, patch, cx));
    })
    .expect("the full-wiring window is live");
}

fn pump(visual: &mut VisualTestContext, frames: usize) {
    for _ in 0..frames {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
}

#[gpui::test]
async fn an_edit_patch_opens_as_one_replacing_right_pane_diff_tab(cx: &mut TestAppContext) {
    let (ws, window) = full_wiring_boot(cx);
    let mut visual = VisualTestContext::from_window(window, cx);

    open_diff(
        cx,
        &ws,
        window,
        Some("crates/agent-ui/src/workspace.rs"),
        ONE_FILE_PATCH,
    );
    // The open rides one deferred effect (it re-enters the shell), then the
    // pane's own notify repaints the tab body.
    cx.run_until_parked();
    pump(&mut visual, 3);

    let (title, files) = cx
        .update(|cx| ws.read(cx).diagnostic_right_diff(cx))
        .expect("the parsed diff state exists after the open");
    assert_eq!(files, 1, "the single-file patch parses to one file");
    assert_eq!(
        title, "workspace.rs",
        "the tab label is the target file name"
    );

    let bounds = visual
        .debug_bounds("right-diff-tab")
        .expect("the right pane paints the diff tab body");
    assert!(
        bounds.size.width > px(100.) && bounds.size.height > px(100.),
        "the diff body fills a real pane region, not a collapsed box: {bounds:?}"
    );

    // The replace leg: a second open from another card swaps the content in
    // the SAME tab — one diff instance, the new file count, the new label.
    open_diff(cx, &ws, window, None, TWO_FILE_PATCH);
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        if cx.update(|cx| {
            ws.read(cx)
                .diagnostic_right_diff(cx)
                .is_some_and(|(_, n)| n == 2)
        }) {
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    pump(&mut visual, 2);

    let (title, files) = cx
        .update(|cx| ws.read(cx).diagnostic_right_diff(cx))
        .expect("the replaced diff state exists");
    assert_eq!(files, 2, "the second patch parses to two files");
    assert_eq!(
        title, "a.rs",
        "the label falls back to the patch's own first file"
    );

    let diff_tabs = cx.update(|cx| {
        agent_ui::chrome_assembly::pane_open_tab_ids(cx)
            .into_iter()
            .filter(|id| id == DIFF_KIND)
            .count()
    });
    assert_eq!(diff_tabs, 1, "the diff kind stays a single tab instance");

    // The pane still paints after the replacement (the swapped DiffState
    // reaches the painted body, not just the workspace state).
    assert!(
        visual.debug_bounds("right-diff-tab").is_some(),
        "the replaced diff body still paints"
    );

    full_wiring_teardown(cx);
}
