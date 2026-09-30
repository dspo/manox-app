//! Regression: the left-edge turn rail mounts one mark per user turn from two
//! turns up; the pointer reaches the strip (hover registers), and clicking a
//! mark lands the message list on that turn's anchor (the same
//! `reveal_message` path the ⌘M popup navigator uses), with the rail hugging
//! the card's left edge and clearing the transcript.
//!
//! The transcript is deliberately taller than the viewport: with short
//! content the native `gpui::list` re-anchors every layout at its floor
//! (chat-log semantics) and no scroll position can survive, click or not.
//!
//! Own test binary, exactly one `#[gpui::test]`: the shared harness
//! initializes process-global singletons (`manox_agent::runtime`,
//! `pi_providers`, `thread_store`) whose global store entity is app-affine,
//! so two tests in one process would overwrite each other's store under the
//! parallel test harness (`tests/common/mod.rs` states the constraint).
#![cfg(feature = "test-support")]

mod common;

use agent_ui::conversation::{ApplyCtx, ConversationState};
use common::{init_harness, open_workspace, tall_history};
use gpui::{AppContext as _, Modifiers, TestAppContext, VisualTestContext, px};

#[gpui::test]
async fn turn_rail_marks_render_and_a_click_lands_on_the_turn(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let conversation = cx.new(|cx| {
        ConversationState::rebuild_from_display(
            &tall_history(30),
            &std::collections::HashMap::new(),
            "test-model",
            manox_agent::MessageAuthor::Lead,
            true,
            ApplyCtx {
                host: manox_agent_chat_ui::host::noop_host(),
                cwd: None,
                fork_source: None,
            },
            cx,
        )
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_replace_conversation(conversation, cx);
    });
    visual.refresh().unwrap();

    // The rail strip mounts, one 10px hit row per user turn, left-anchored
    // and ascending with their turns.
    let strip = visual
        .debug_bounds("turn-rail")
        .expect("the rail mounts from two turns up");
    assert!(strip.size.width >= px(28.));
    let first = visual
        .debug_bounds("turn-rail-mark-0")
        .expect("the first turn's mark renders");
    let second = visual
        .debug_bounds("turn-rail-mark-2")
        .expect("the second turn's mark renders");
    assert_eq!(
        first.size.height,
        px(10.),
        "each hit row spans the fixed pitch"
    );
    assert_eq!(
        first.origin.x, second.origin.x,
        "marks share the rail's left edge"
    );
    assert!(
        second.origin.y > first.origin.y,
        "marks ascend with their turns"
    );

    // The pointer reaches the strip: moving over a mark registers its hover.
    visual.simulate_event(gpui::MouseMoveEvent {
        position: first.center(),
        pressed_button: None,
        modifiers: Modifiers::default(),
    });
    let hover = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_turn_rail_hover(cx));
    assert_eq!(hover, Some(0), "the mark row receives pointer events");

    // Clicking the older mark jumps the list to that turn's user bubble.
    visual.simulate_click(first.center(), Modifiers::default());
    let top = workspace.read_with(&visual.cx, |ws, cx| {
        ws.diagnostic_list_state(cx).logical_scroll_top()
    });
    assert_eq!(
        top.item_ix, 0,
        "the click landed the list on the first turn's anchor"
    );

    // After the jump row 0 is on screen: the rail hugs the band's left edge
    // and clears the transcript — the gutter pads the list wrapper, not the
    // band the rail anchors to, so the ticks sit at the card edge while the
    // text starts a full gutter in. Row geometry comes straight from the
    // list state, which is more direct than element bounds.
    visual.cx.run_until_parked();
    let list_state = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_list_state(cx));
    let strip = visual
        .debug_bounds("turn-rail")
        .expect("rail still mounted");
    let row = list_state
        .bounds_for_item(0)
        .expect("the jumped-to row is laid out");
    assert!(
        strip.origin.x <= px(8.),
        "the rail hugs the band's left edge, got x={:?}",
        strip.origin.x
    );
    assert!(
        strip.origin.x + strip.size.width <= row.origin.x,
        "the rail must clear the transcript (rail {:?} vs row {:?})",
        strip.origin.x + strip.size.width,
        row.origin.x
    );
}
