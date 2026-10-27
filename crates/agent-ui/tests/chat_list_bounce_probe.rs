//! The chat transcript's ScrollBounce wrapper at the exact nesting production
//! mounts it (bounce around the `gpui::list`, inside the stretch column):
//! wheel input routed through the bounce still scrolls the ListState, the
//! logical position clamps at both ends under continued wheel input (the
//! bounce's stretch displaces paint only), and the `on_scroll` callback the
//! production wrap relies on for chat notifications fires on those scrolls.
//!
//! Wheel packets are trackpad-realistic: the engine's overscroll physics
//! reshapes absurd single-packet deltas, so the ends are reached by many
//! moderate packets, as a real gesture delivers them.
//!
//! Bare-window probe after `scroll_list` (no workspace singletons), so it
//! lives in its own binary for speed.

use std::{cell::RefCell, rc::Rc};

use gpui::{
    Context, FollowMode, InteractiveElement as _, IntoElement, ListAlignment, ListOffset,
    ListState, ParentElement as _, Pixels, Render, ScrollEvent, Styled as _, TestAppContext,
    VisualTestContext, Window, WindowHandle, div, list, point, px,
};

struct BounceProbe {
    state: ListState,
    scroll_events: Rc<RefCell<usize>>,
}

impl Render for BounceProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let bounce = self.state.clone();
        let events = self.scroll_events.clone();
        div().id("viewport").size_full().flex().flex_col().child(
            gpui_base::ScrollBounce::new(
                "probe-bounce",
                &bounce,
                list(state, render_row).size_full().min_h_0(),
            )
            .on_scroll(move |_, _| {
                *events.borrow_mut() += 1;
            }),
        )
    }
}

fn render_row(ix: usize, _window: &mut Window, _cx: &mut gpui::App) -> gpui::AnyElement {
    div()
        .id(("row", ix))
        .w_full()
        .h(px(100.))
        .flex_shrink_0()
        .into_any_element()
}

fn wheel(visual: &mut VisualTestContext, position: gpui::Point<Pixels>, dy: f32) {
    visual.simulate_event(ScrollEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
        source: Default::default(),
    });
}

fn draw(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
}

fn scroll_in_steps(
    visual: &mut VisualTestContext,
    state: &ListState,
    center: gpui::Point<Pixels>,
    dy: f32,
    packets: usize,
) -> ListOffset {
    for i in 0..packets {
        wheel(visual, center, dy);
        if i % 10 == 9 {
            draw(visual);
        }
    }
    draw(visual);
    state.logical_scroll_top()
}

#[gpui::test]
fn bounce_routes_wheel_and_clamps_at_both_ends(cx: &mut TestAppContext) {
    // 100 rows of 100px against a 600px viewport: both ends are far apart,
    // and the tail end stays far from the first item.
    let state = ListState::new(100, ListAlignment::Bottom, px(2048.));
    let scroll_events = Rc::new(RefCell::new(0));
    let probe_state = state.clone();
    let probe_events = scroll_events.clone();
    let window: WindowHandle<BounceProbe> = cx.add_window(move |_, _| BounceProbe {
        state: probe_state,
        scroll_events: probe_events,
    });
    // Chat-log shape: follow the tail from the start.
    state.set_follow_mode(FollowMode::Tail);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    // Pin the viewport to a known size: the test window opens maximized on
    // the test display, whose bounds are not contractual here.
    visual.simulate_resize(gpui::size(px(800.), px(600.)));
    draw(&mut visual);

    let center = point(px(200.), px(300.));
    let tail = state.logical_scroll_top();
    assert!(
        tail.item_ix >= 90,
        "bottom alignment starts at the tail (got {})",
        tail.item_ix
    );

    // A moderate wheel step through the bounce must move the list — the
    // engine owns the wheel by default, so a broken takeover would scroll
    // nothing (or double).
    let after_step = scroll_in_steps(&mut visual, &state, center, 120., 3);
    assert_ne!(
        (tail.item_ix, tail.offset_in_item),
        (after_step.item_ix, after_step.offset_in_item),
        "wheel steps through the bounce scroll the list"
    );

    // Continued upward wheel reaches the first item and clamps there: extra
    // packets past the edge cannot push the logical position out of bounds
    // (the bounce's stretch displaces paint only).
    let top = scroll_in_steps(&mut visual, &state, center, 120., 200);
    assert_eq!(top.item_ix, 0, "continued wheel clamps at the first item");

    // And back down: the tail end clamps too (tail-follow re-arms on landing,
    // so the sentinel end is expected).
    let bottom = scroll_in_steps(&mut visual, &state, center, -120., 200);
    assert!(
        bottom.item_ix >= 90,
        "continued wheel clamps at the tail (got {})",
        bottom.item_ix
    );

    // The production wrap notifies the chat from on_scroll on every one of
    // these; the probe's counter proves the callback path fires.
    assert!(
        *scroll_events.borrow() >= 10,
        "on_scroll fired across the wheel steps ({} events)",
        scroll_events.borrow()
    );
}
