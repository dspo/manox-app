//! The composer footer's inline access switch (segmented `TabBar`).
//!
//! Two regressions this binary pins:
//! - the switch paints inside the footer row at the window's minimum width
//!   (`MIN_WINDOW_W`) without running into the pinned right cluster. The
//!   footer's 14px tier widened every label, the switch itself is
//!   `flex_shrink_0`, and the left chip group deliberately paints overflow
//!   (`min_w_0`, no `overflow_hidden` so popovers can escape upward);
//! - a click transitions exactly once. The no-op guard reads the fold at
//!   click time, so a second click that lands before the optimistic flip
//!   repaints must not emit a second config write / notice.
//!
//! Own test binary, one `#[gpui::test]`, like its neighbours: the harness's
//! process-global singletons are app-affine.
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace_at};
use gpui::{AppContext as _, Modifiers, TestAppContext, VisualTestContext, px};
use steer_agent_chat_ui::ahp_store::{AhpStore, session_uri};
use steer_i18n::Language;

/// The session the probe binds as the foreground store.
const SESSION: &str = "sess-access-switch";
/// As wide as a real footer model face gets: provider / model / effort.
const MODEL: &str = "anthropic/claude-sonnet-4-5";

fn session_dispatch_count(store: &AhpStore) -> usize {
    store
        .diagnostic_dispatch_log()
        .iter()
        .filter(|(channel, _)| channel == &session_uri(SESSION))
        .count()
}

#[gpui::test]
async fn the_access_switch_renders_inline_and_transitions_once_per_click(cx: &mut TestAppContext) {
    init_harness(cx);
    // English is the widest footer locale ("Sandbox access" / "Full access"
    // outrun 沙箱访问 / 完全访问), so the width assertion is honest only there.
    cx.update(|_cx| steer_i18n::set_ui_language(Language::En));

    // `MIN_WINDOW_W`: the floor the footer row's width comment leans on.
    let (window, workspace) = open_workspace_at(cx, 860.);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let store = visual.update(|_, cx| cx.new(|_| AhpStore::detached()));
    // A loaded footer: the widest neighbouring face plus the access seat the
    // switch mirrors (segment 0 = sandbox).
    store.update(&mut visual.cx, |store, _| {
        let mut config = serde_json::Map::new();
        config.insert("accessMode".to_string(), serde_json::json!("sandbox"));
        config.insert("model".to_string(), serde_json::json!(MODEL));
        config.insert("reasoningEffort".to_string(), serde_json::json!("high"));
        store.optimistic_config(SESSION, &config);
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), SESSION, cx);
    });
    for _ in 0..3 {
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    let switch = visual
        .debug_bounds("access-switch")
        .expect("the switch paints in the composer footer");
    let sandbox = visual
        .debug_bounds("access-switch-sandbox")
        .expect("the sandbox segment paints");
    let full = visual
        .debug_bounds("access-switch-full-access")
        .expect("the full-access segment paints");
    assert!(switch.size.width > px(0.) && switch.size.height > px(0.));
    assert!(
        sandbox.right() <= full.left(),
        "the two segments lay out side by side"
    );
    let right = visual
        .debug_bounds("composer-footer-right")
        .expect("the footer's right cluster paints");
    assert!(
        switch.right() <= right.left(),
        "at MIN_WINDOW_W the 14px footer must not push the switch into the \
         right cluster (switch right {:?}, cluster left {:?})",
        switch.right(),
        right.left()
    );

    let count = |visual: &VisualTestContext| {
        store.read_with(&visual.cx, |store, _| session_dispatch_count(store))
    };

    // One click on the inactive segment = exactly one config write.
    let before = count(&visual);
    visual.simulate_click(full.center(), Modifiers::default());
    let after_first = count(&visual);
    assert_eq!(
        after_first,
        before + 1,
        "a click on the inactive segment routes through the single funnel"
    );

    // The optimistic flip has not repainted yet (this click deliberately does
    // not draw). Re-clicking the now-current segment must stay a no-op: the
    // guard reads the fold, not the render-time index.
    visual.simulate_click(full.center(), Modifiers::default());
    assert_eq!(
        count(&visual),
        after_first,
        "clicking the already-active segment is a no-op even before the repaint"
    );

    // A real transition back writes again.
    visual.simulate_click(sandbox.center(), Modifiers::default());
    assert_eq!(
        count(&visual),
        after_first + 1,
        "the return transition writes once"
    );

    manox_agent::thread_store::drop_global_for_test();
}
