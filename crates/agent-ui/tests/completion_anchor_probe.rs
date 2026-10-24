//! The completion popover's caret anchor against the live textarea geometry:
//! after typing, `prepaint_caret_geometry` reports the caret the input
//! painted, and the origin derived from it sits one line below that caret,
//! inside the window. The geometry is also cross-checked against the input's
//! own bounds — the decisive assertion that `cursor_bounds` is window-space,
//! not input-local (a local-space caret would sit far above the composer's
//! band near the window bottom). The pure flip/clamp branches are unit-tested
//! in steer-agent-chat-ui's `completion.rs`.
//!
//! Own test binary: shared harness singletons (see `tests/common/mod.rs`).
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{TestAppContext, VisualTestContext, px};
use steer_agent_chat_ui::views::completion::{COMPLETION_WIDTH, caret_anchor_origin};
use steer_agent_chat_ui::views::popup_menu::MAX_LIST_HEIGHT;

#[gpui::test]
async fn the_completion_anchor_rides_the_live_caret_geometry(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let input = workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_chat_input(cx));
    window
        .update(&mut visual.cx, |_, window, cx| {
            input.update(cx, |s, cx| {
                s.set_value("你好，输入一些宽度不同的文本 hello", window, cx)
            });
        })
        .expect("window");

    // Two frames: the first paints the new value and stashes the prepaint
    // caret geometry; a second frame is what the overlay's render reads.
    for _ in 0..2 {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    let geo = workspace.read_with(&mut visual.cx, |ws, cx| {
        ws.diagnostic_chat_input(cx)
            .read(cx)
            .prepaint_caret_geometry()
            .expect("caret geometry is stashed after a draw")
    });
    let viewport = window
        .update(&mut visual.cx, |_, window, _| window.viewport_size())
        .expect("window");

    let caret = geo.cursor_bounds();
    let input_bounds = geo.input_bounds();

    // The caret is window-space: it must sit inside the composer band the
    // input itself reported, not near the window origin an input-local
    // coordinate would produce.
    assert!(
        caret.origin.y + geo.line_height() > input_bounds.origin.y,
        "caret sits in the composer band, not above it: caret {caret:?} input {input_bounds:?}"
    );
    assert!(
        caret.right() <= viewport.width,
        "caret x inside the window: {caret:?} in {viewport:?}"
    );

    let origin = caret_anchor_origin(
        caret,
        geo.line_height(),
        geo.scroll_offset().y,
        viewport,
        MAX_LIST_HEIGHT + px(16.),
        px(8.),
    );
    assert!(
        origin.y >= caret.origin.y + geo.line_height(),
        "the popover starts at least one line below the caret: {origin:?} caret {caret:?}"
    );
    assert!(
        origin.x >= px(8.) && origin.x <= (viewport.width - COMPLETION_WIDTH).max(px(8.)),
        "the popover's x stays clamped into the window: {origin:?} in {viewport:?}"
    );
}
