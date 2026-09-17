//! Behaviour of `ChainOfThought`.
//!
//! The component is a controlled view — upstream's chain of thought has no
//! streaming awareness, no duration, and neither opens nor folds itself — so
//! what is worth pinning here is that control, the header's slots, and the
//! rail's shape. The block's policy lives with the host and is tested there.

use std::cell::Cell;
use std::rc::Rc;

use ai_elements::{ChainOfThought, ChainOfThoughtHeader, ChainOfThoughtStep};
use gpui::{
    Context, Entity, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _, Render,
    TestAppContext, VisualTestContext, Window, div,
};

/// One block on screen, plus the count of header clicks it has reported.
struct Harness {
    open: bool,
    /// Upstream animates the reveal and keeps the steps mounted while closed;
    /// a host that unmounts collapsed rows turns it off.
    animated: bool,
    step_appear_animated: bool,
    toggles: Rc<Cell<usize>>,
}

impl Render for Harness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let toggles = self.toggles.clone();
        let step_appear_animated = self.step_appear_animated;
        ChainOfThought::new(("cot", 0usize))
            .open(self.open)
            .animated(self.animated)
            .on_toggle(move |_, _, _| toggles.set(toggles.get() + 1))
            .header(
                ChainOfThoughtHeader::new(("cot", 0usize))
                    .icon(div().debug_selector(|| "cot-icon".into()).child("icon"))
                    .label(
                        div()
                            .debug_selector(|| "cot-label".into())
                            .child("Chain of Thought"),
                    )
                    .meta(div().debug_selector(|| "cot-meta-0".into()).child("Readx7"))
                    .meta(div().debug_selector(|| "cot-meta-1".into()).child("12s")),
            )
            .step(
                ChainOfThoughtStep::new(("cot-step", 0usize))
                    .appear_animated(step_appear_animated)
                    .label(
                        div()
                            .debug_selector(|| "cot-step-0".into())
                            .child("first step"),
                    )
                    .description(
                        div()
                            .debug_selector(|| "cot-desc-0".into())
                            .child("a detail"),
                    )
                    .content(
                        div()
                            .debug_selector(|| "cot-body-0".into())
                            .child("step body"),
                    ),
            )
            .step(
                ChainOfThoughtStep::new(("cot-step", 1usize))
                    .appear_animated(step_appear_animated)
                    .label(
                        div()
                            .debug_selector(|| "cot-step-1".into())
                            .child("second step"),
                    ),
            )
    }
}

fn init(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
}

/// Mount one block and paint a frame.
fn mount(cx: &mut TestAppContext, open: bool) -> (Entity<Harness>, &mut VisualTestContext) {
    init(cx);
    let toggles = Rc::new(Cell::new(0));
    let (view, visual) = cx.add_window_view(|_, _| Harness {
        open,
        animated: false,
        step_appear_animated: true,
        toggles,
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    (view, visual)
}

fn set_open(visual: &mut VisualTestContext, view: &Entity<Harness>, open: bool) {
    visual.update(|_, cx| {
        view.update(cx, |harness, cx| {
            harness.open = open;
            cx.notify();
        })
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
}

fn toggles(view: &Entity<Harness>, cx: &mut VisualTestContext) -> usize {
    cx.update(|_, cx| view.read(cx).toggles.get())
}

// —— The controlled contract ————————————————————————————————————————————————

/// Upstream's `defaultOpen = false` shape: with `open` false the steps are not
/// in the tree at all.
#[gpui::test]
fn a_closed_block_mounts_no_steps(cx: &mut TestAppContext) {
    let (_view, visual) = mount(cx, false);
    assert!(visual.debug_bounds("cot-0").is_some(), "the header stays");
    assert!(visual.debug_bounds("cot-0-content").is_none());
    assert!(visual.debug_bounds("cot-step-0").is_none());
}

/// Nothing about the block opens itself — the host's `open` decides.
#[gpui::test]
fn an_open_block_mounts_its_steps(cx: &mut TestAppContext) {
    let (_view, visual) = mount(cx, true);
    assert!(visual.debug_bounds("cot-0-content").is_some());
    assert!(visual.debug_bounds("cot-step-0").is_some());
    assert!(visual.debug_bounds("cot-step-1").is_some());
    assert!(visual.debug_bounds("cot-body-0").is_some());
    assert!(visual.debug_bounds("cot-desc-0").is_some());
}

/// Toggling is pure reporting: the host's state is what moves, and it may ignore
/// the callback entirely.
#[gpui::test]
fn clicking_the_header_reports_exactly_one_toggle(cx: &mut TestAppContext) {
    let (view, visual) = mount(cx, false);
    let header = visual
        .debug_bounds("cot-0")
        .expect("the header row must be laid out");
    visual.simulate_click(header.center(), Modifiers::default());
    assert_eq!(toggles(&view, visual), 1);
}

/// The host owns the state: driving `open` mounts and unmounts the steps without
/// any interaction.
#[gpui::test]
fn the_steps_follow_the_host_s_open_state(cx: &mut TestAppContext) {
    let (view, visual) = mount(cx, false);
    assert!(visual.debug_bounds("cot-0-content").is_none());

    set_open(visual, &view, true);
    assert!(visual.debug_bounds("cot-0-content").is_some());

    set_open(visual, &view, false);
    assert!(visual.debug_bounds("cot-0-content").is_none());

    set_open(visual, &view, true);
    assert!(visual.debug_bounds("cot-0-content").is_some());
}

// —— The header ——————————————————————————————————————————————————————————————

/// Every `meta` slot renders, alongside the icon and label.
#[gpui::test]
fn the_header_renders_every_slot(cx: &mut TestAppContext) {
    let (_view, visual) = mount(cx, false);
    for selector in ["cot-icon", "cot-label", "cot-meta-0", "cot-meta-1"] {
        assert!(
            visual.debug_bounds(selector).is_some(),
            "the header slot {selector} must render"
        );
    }
}

/// The header row spans the block, so a click anywhere on it toggles.
#[gpui::test]
fn the_header_spans_the_block(cx: &mut TestAppContext) {
    let (_view, visual) = mount(cx, false);
    let header = visual.debug_bounds("cot-0").expect("header");
    let label = visual.debug_bounds("cot-label").expect("label");
    assert!(
        header.size.width > label.size.width,
        "the clickable row must be wider than its label: {header:?} vs {label:?}"
    );
}

// —— The steps ——————————————————————————————————————————————————————————————

/// The rail runs between steps: every step but the last draws one, so the list
/// does not end on a stub hanging under the final marker.
#[gpui::test]
fn every_step_but_the_last_draws_a_rail(cx: &mut TestAppContext) {
    let (_view, visual) = mount(cx, true);
    assert!(visual.debug_bounds("cot-step-0-rail").is_some());
    assert!(visual.debug_bounds("cot-step-1-rail").is_none());
}

/// The entrance animation is opt-out and layout-neutral either way.
#[gpui::test]
fn a_step_can_skip_its_entrance_animation(cx: &mut TestAppContext) {
    init(cx);
    let toggles = Rc::new(Cell::new(0));
    let (_view, visual) = cx.add_window_view(|_, _| Harness {
        open: true,
        animated: false,
        step_appear_animated: false,
        toggles,
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(visual.debug_bounds("cot-step-0").is_some());
    assert!(visual.debug_bounds("cot-body-0").is_some());
}

/// The reveal is on by default (upstream's behaviour), and it keeps the steps
/// mounted while closed — that is what makes a height reveal reversible.
#[gpui::test]
fn the_default_reveal_keeps_its_steps_mounted_while_closed(cx: &mut TestAppContext) {
    init(cx);
    let toggles = Rc::new(Cell::new(0));
    struct Animated {
        toggles: Rc<Cell<usize>>,
    }
    impl Render for Animated {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let toggles = self.toggles.clone();
            // No `.animated`: the reveal is upstream's default.
            ChainOfThought::new(("cot", 0usize))
                .open(false)
                .on_toggle(move |_, _, _| toggles.set(toggles.get() + 1))
                .step(
                    ChainOfThoughtStep::new(("cot-step", 0usize))
                        .label(div().debug_selector(|| "cot-step-0".into()).child("first")),
                )
        }
    }
    let (_view, visual) = cx.add_window_view(|_, _| Animated { toggles });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(visual.debug_bounds("cot-step-0").is_some());
}

/// A step used on its own still renders its marker and rail.
#[gpui::test]
fn a_step_renders_standalone(cx: &mut TestAppContext) {
    init(cx);
    struct Single;
    impl Render for Single {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            ChainOfThoughtStep::new(("cot-step", 0usize)).label(
                div()
                    .debug_selector(|| "cot-step-0".into())
                    .child("standalone"),
            )
        }
    }
    let (_view, visual) = cx.add_window_view(|_, _| Single);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(visual.debug_bounds("cot-step-0").is_some());
    assert!(visual.debug_bounds("cot-step-0-rail").is_some());
}
