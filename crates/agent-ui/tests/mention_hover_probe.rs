//! The @-mention chip's hover tooltip: the textarea's token-hover pipeline
//! raises the chip's full registry key (the chip itself shows only the short
//! name), anchors the tooltip under the chip's laid-out rect, and clears both
//! the state and the painted card when the pointer leaves. The pointer leg
//! drives the real hover dispatch with synthetic mouse moves; the render half
//! is asserted through the deferred tooltip element's painted bounds.
//!
//! Own test binary: shared harness singletons (see `tests/common/mod.rs`).
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{Modifiers, TestAppContext, VisualTestContext, point, px};
use steer_agent_chat_ui::views::completion::build_replacement_content;

#[gpui::test]
async fn the_mention_chip_hints_its_registry_key_on_hover(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    // A mention whose registry key differs from the chip's short name — the
    // only shape the tooltip has anything to add.
    let (content, _) = build_replacement_content('@', "some-registry-key", "shortname", "", 0, 0)
        .expect("mention content builds");
    let input = workspace.update(&mut visual.cx, |ws, cx| ws.diagnostic_chat_input(cx));
    window
        .update(&mut visual.cx, |_, window, cx| {
            input.update(cx, |s, cx| s.set_value(content, window, cx));
        })
        .expect("window");

    // Two frames: the first paints the token (and stashes the prepaint
    // geometry), the second settles hover-eligible hitboxes.
    for _ in 0..2 {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    let (input_bounds, caret_y) = workspace.read_with(&mut visual.cx, |ws, cx| {
        let geo = ws
            .diagnostic_chat_input(cx)
            .read(cx)
            .prepaint_caret_geometry()
            .expect("caret geometry is stashed after a draw");
        (geo.input_bounds(), geo.cursor_bounds().center().y)
    });

    // The mention is the input's first content: its chip starts at the text
    // origin just inside the input's box, on the caret's own line.
    let chip_point = point(input_bounds.origin.x + px(16.), caret_y);
    visual.simulate_mouse_move(chip_point, None, Modifiers::default());
    for _ in 0..3 {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    let (key, chip_bounds) = workspace
        .read_with(&mut visual.cx, |ws, cx| ws.diagnostic_mention_hover(cx))
        .expect("the pointer over the chip raises the hover state");
    assert_eq!(key, "some-registry-key");
    assert!(
        chip_bounds.origin.y >= input_bounds.origin.y
            && chip_bounds.origin.y < input_bounds.origin.y + input_bounds.size.height,
        "the reported chip rect sits on a line inside the input: {chip_bounds:?} input {input_bounds:?}"
    );
    let tip = visual
        .debug_bounds("mention-hover-tip")
        .expect("the tooltip paints under the chip");
    assert!(
        tip.origin.y >= chip_bounds.origin.y + chip_bounds.size.height,
        "the tooltip starts at or below the chip's bottom edge: {tip:?} chip {chip_bounds:?}"
    );

    // Leaving the input retires both the state and the card.
    visual.simulate_mouse_move(point(px(4.), px(4.)), None, Modifiers::default());
    for _ in 0..3 {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
    assert!(
        workspace.read_with(&mut visual.cx, |ws, cx| ws
            .diagnostic_mention_hover(cx)
            .is_none()),
        "leaving the chip clears the hover state"
    );
    assert!(
        visual.debug_bounds("mention-hover-tip").is_none(),
        "the tooltip unmounts with the hover state"
    );

    // Hover again, then delete the chip with a content change: no exit event
    // can come from a token whose element is gone, so the Change path must
    // retire the hover state itself.
    visual.simulate_mouse_move(chip_point, None, Modifiers::default());
    for _ in 0..3 {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
    assert!(
        workspace
            .read_with(&mut visual.cx, |ws, cx| ws.diagnostic_mention_hover(cx))
            .is_some(),
        "the hover state is up again before the deletion leg"
    );
    window
        .update(&mut visual.cx, |_, window, cx| {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        })
        .expect("window");
    for _ in 0..3 {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
    assert!(
        workspace.read_with(&mut visual.cx, |ws, cx| ws
            .diagnostic_mention_hover(cx)
            .is_none()),
        "a content change retires the hover state with the chip"
    );
    assert!(
        visual.debug_bounds("mention-hover-tip").is_none(),
        "the tooltip cannot outlive the chip it described"
    );
}
