//! Regression for the turn rail's preview card on the ladder's internal-scroll
//! path: once the marks exceed `MAX_RAIL_HEIGHT` (60 turns × 10px > 420px),
//! the ladder scrolls and `ScrollHandle::offset().y` runs NEGATIVE — the
//! preview's top must consume that offset sign-corrected, or the card pins to
//! the rail's bottom instead of the hovered mark (review round 1, C1).
//! Re-projecting to a short conversation must reset the rail's interaction
//! state (no stale hover surviving the switch).
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
use gpui::{AppContext as _, Modifiers, TestAppContext, VisualTestContext};

#[gpui::test]
async fn preview_tracks_the_hovered_mark_when_the_ladder_scrolls(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let conversation = cx.new(|cx| {
        ConversationState::rebuild_from_display(
            &tall_history(60),
            &std::collections::HashMap::new(),
            "test-model",
            manox_agent::MessageAuthor::Lead,
            true,
            ApplyCtx {
                host: steer_agent_chat_ui::host::noop_host(),
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

    // Turn 39 anchors at item 78. Tail-follow puts active on the newest
    // turn, so the ladder is scrolled to its tail (offset.y ≈ -180) and
    // mark 39 sits mid-ladder, on screen.
    let mark = visual
        .debug_bounds("turn-rail-mark-78")
        .expect("the mid-ladder mark renders while the ladder is scrolled");
    visual.simulate_event(gpui::MouseMoveEvent {
        position: mark.center(),
        pressed_button: None,
        modifiers: Modifiers::default(),
    });
    visual.cx.run_until_parked();
    visual.refresh().unwrap();
    let preview = visual
        .debug_bounds("turn-rail-preview")
        .expect("the hovered mark's preview renders");
    assert!(
        (f32::from(preview.center().y) - f32::from(mark.center().y)).abs() <= 1.0,
        "the preview must sit at the hovered mark, not the rail's bottom \
         (preview center y={:?} vs mark center y={:?})",
        preview.center().y,
        mark.center().y
    );

    // Re-projecting to a short conversation resets the rail's interaction
    // state: the stale hover must not survive the switch (no preview card
    // for a pointer that is not on the rail, and no stale guard feeding
    // `&turns[ix]`).
    let short = cx.new(|cx| {
        ConversationState::rebuild_from_display(
            &tall_history(1),
            &std::collections::HashMap::new(),
            "test-model",
            manox_agent::MessageAuthor::Lead,
            true,
            ApplyCtx {
                host: steer_agent_chat_ui::host::noop_host(),
                cwd: None,
                fork_source: None,
            },
            cx,
        )
    });
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_replace_conversation(short, cx);
    });
    visual.cx.run_until_parked();
    visual.refresh().unwrap();
    let hover = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_turn_rail_hover(cx));
    assert_eq!(
        hover, None,
        "the rail's hover state resets on re-projection"
    );
    assert!(
        visual.debug_bounds("turn-rail").is_none(),
        "the rail unmounts below two turns"
    );
}
