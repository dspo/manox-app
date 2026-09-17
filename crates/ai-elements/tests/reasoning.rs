//! Behaviour of `Reasoning`, ported case for case from upstream's
//! `packages/elements/__tests__/reasoning.test.tsx` where the case has a gpui
//! equivalent, plus the two behaviours this repo adds on purpose: the
//! user-toggle pin and the programmatic-close pin.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use ai_elements::{AUTO_CLOSE_DELAY, Reasoning, ReasoningEvent, ReasoningState};
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, Modifiers,
    ParentElement as _, Render, TestAppContext, VisualTestContext, Window, div, px,
};
use gpui_component::Theme;
use manox_components::markdown::Markdown;

/// One block on screen. The body carries no selector of its own — the block's
/// own `{id}-content` hook is what the mounting tests read.
struct Harness {
    state: Entity<ReasoningState>,
    animated: bool,
}

impl Render for Harness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Reasoning::new(("reasoning", 0usize), &self.state)
            .animated(self.animated)
            .content(div().child("the thinking itself"))
    }
}

fn init(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
}

/// A block that mounts idle and closed.
fn closed_state(cx: &mut TestAppContext) -> Entity<ReasoningState> {
    init(cx);
    cx.update(|cx| cx.new(ReasoningState::new))
}

fn update<R>(
    cx: &mut TestAppContext,
    state: &Entity<ReasoningState>,
    f: impl FnOnce(&mut ReasoningState, &mut Context<ReasoningState>) -> R,
) -> R {
    cx.update(|cx| state.update(cx, f))
}

fn is_open(cx: &mut TestAppContext, state: &Entity<ReasoningState>) -> bool {
    cx.update(|cx| state.read(cx).is_open())
}

fn duration(cx: &mut TestAppContext, state: &Entity<ReasoningState>) -> Option<u64> {
    cx.update(|cx| state.read(cx).duration())
}

/// Mount one block and paint a frame.
fn mount(
    cx: &mut TestAppContext,
    state: Entity<ReasoningState>,
    animated: bool,
) -> &mut VisualTestContext {
    let (_, visual) = cx.add_window_view(|_, _| Harness { state, animated });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual
}

// —— Mount-time state ————————————————————————————————————————————————————————

/// Upstream: "starts closed by default when not streaming (old messages)".
#[gpui::test]
fn a_block_that_never_streamed_mounts_closed(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    assert!(!is_open(cx, &state));
}

/// Upstream: "starts open when streaming".
#[gpui::test]
fn a_block_born_mid_stream_mounts_open(cx: &mut TestAppContext) {
    init(cx);
    let state = cx.update(|cx| cx.new(|cx| ReasoningState::new(cx).streaming(true, cx)));
    assert!(is_open(cx, &state));
}

/// Upstream: "can be forced open with defaultOpen".
#[gpui::test]
fn default_open_opens_the_block(cx: &mut TestAppContext) {
    init(cx);
    let state = cx.update(|cx| cx.new(|cx| ReasoningState::new(cx).default_open(true)));
    assert!(is_open(cx, &state));
}

/// Upstream: "can be forced closed with defaultOpen={false}" — a stream that
/// arrives afterwards does not open it.
#[gpui::test]
fn default_open_false_survives_a_stream(cx: &mut TestAppContext) {
    init(cx);
    let state = cx.update(|cx| cx.new(|cx| ReasoningState::new(cx).default_open(false)));
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    assert!(
        !is_open(cx, &state),
        "an explicitly closed block must not open itself"
    );
}

/// The two mount-time inputs are order-insensitive, so a host cannot get a
/// different block by swapping the builder calls.
#[gpui::test]
fn mount_time_inputs_are_order_insensitive(cx: &mut TestAppContext) {
    init(cx);
    let pin_last = cx.update(|cx| {
        cx.new(|cx| {
            ReasoningState::new(cx)
                .streaming(true, cx)
                .default_open(false)
        })
    });
    let pin_first = cx.update(|cx| {
        cx.new(|cx| {
            ReasoningState::new(cx)
                .default_open(false)
                .streaming(true, cx)
        })
    });
    assert!(!is_open(cx, &pin_last));
    assert!(!is_open(cx, &pin_first));
}

// —— Streaming transitions ——————————————————————————————————————————————————

/// Upstream: "calls onOpenChange" — plus the auto-open half, which upstream
/// performs on the same rising edge.
#[gpui::test]
fn a_stream_opens_the_block_and_reports_it(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    let seen: Rc<RefCell<Vec<ReasoningEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = seen.clone();
    let subscription = cx.update(|cx| {
        cx.subscribe(&state, move |_, event: &ReasoningEvent, _| {
            recorder.borrow_mut().push(*event)
        })
    });

    update(cx, &state, |state, cx| state.set_streaming(true, cx));

    assert!(is_open(cx, &state));
    assert_eq!(
        seen.borrow().as_slice(),
        &[ReasoningEvent::OpenChanged { open: true }]
    );
    drop(subscription);
}

/// Upstream: "auto-closes after delay when streaming stops" — the block is
/// still open the moment the stream ends, and folded only after the delay.
#[gpui::test]
fn a_finished_stream_folds_after_the_delay(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    cx.executor().advance_clock(Duration::from_secs(2));
    update(cx, &state, |state, cx| state.set_streaming(false, cx));
    assert!(
        is_open(cx, &state),
        "the block plays out its last deltas before folding"
    );

    cx.executor().advance_clock(AUTO_CLOSE_DELAY);
    cx.run_until_parked();
    assert!(!is_open(cx, &state));
}

/// Upstream test #86: a round that never streamed is never folded, however the
/// user opened it.
#[gpui::test]
fn a_block_that_never_streamed_is_not_folded(cx: &mut TestAppContext) {
    init(cx);
    let state = cx.update(|cx| cx.new(|cx| ReasoningState::new(cx).default_open(true)));
    cx.executor().advance_clock(AUTO_CLOSE_DELAY * 2);
    cx.run_until_parked();
    assert!(is_open(cx, &state));
}

/// Upstream's `hasAutoClosed` is one-shot: the fold happens once per block, so
/// a block a later round re-opens is left alone.
#[gpui::test]
fn the_automatic_fold_fires_once(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    update(cx, &state, |state, cx| state.set_streaming(false, cx));
    cx.executor().advance_clock(AUTO_CLOSE_DELAY);
    cx.run_until_parked();
    assert!(!is_open(cx, &state));

    // A second round re-opens the block; its end schedules no second fold.
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    assert!(is_open(cx, &state));
    update(cx, &state, |state, cx| state.set_streaming(false, cx));
    cx.executor().advance_clock(AUTO_CLOSE_DELAY * 2);
    cx.run_until_parked();
    assert!(is_open(cx, &state));
}

/// A stream that resumes inside the fold's delay window still folds when it
/// ends for real. The resume calls the pending fold off (upstream's effect
/// cleanup), and a fold that never happened must not spend the block's single
/// one — otherwise the first end would consume it, the stale timer would fire
/// mid-stream, and the block would stay open for the rest of its life.
#[gpui::test]
fn a_stream_resuming_inside_the_delay_window_still_folds(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    update(cx, &state, |state, cx| state.set_streaming(false, cx));

    // Resume midway through the delay, then let the whole window pass.
    cx.executor().advance_clock(AUTO_CLOSE_DELAY / 2);
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    cx.executor().advance_clock(AUTO_CLOSE_DELAY * 2);
    cx.run_until_parked();
    assert!(
        is_open(cx, &state),
        "a live stream keeps the block open past the first window"
    );

    // Ending for real folds it.
    update(cx, &state, |state, cx| state.set_streaming(false, cx));
    cx.executor().advance_clock(AUTO_CLOSE_DELAY);
    cx.run_until_parked();
    assert!(
        !is_open(cx, &state),
        "the second end must still fold the block"
    );
}

// —— The two deliberate extensions ———————————————————————————————————————————

/// A hand toggle claims the block: a fold already scheduled stands down, and
/// the user's choice survives.
#[gpui::test]
fn a_hand_toggle_pins_the_block_against_the_fold(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    update(cx, &state, |state, cx| state.set_streaming(false, cx));

    // The user folds and unfolds by hand inside the delay window.
    update(cx, &state, |state, cx| state.toggle(cx));
    update(cx, &state, |state, cx| state.toggle(cx));
    assert!(is_open(cx, &state));

    cx.executor().advance_clock(AUTO_CLOSE_DELAY);
    cx.run_until_parked();
    assert!(
        is_open(cx, &state),
        "the scheduled fold must respect the user's toggle"
    );

    // And a later round cannot fold it either.
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    update(cx, &state, |state, cx| state.set_streaming(false, cx));
    cx.executor().advance_clock(AUTO_CLOSE_DELAY * 2);
    cx.run_until_parked();
    assert!(is_open(cx, &state));
}

/// A programmatic close is a pin, not a request: the next stream does not
/// re-open a block the host deliberately shut.
#[gpui::test]
fn a_programmatic_close_pins_the_block_shut(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    update(cx, &state, |state, cx| state.set_open(false, cx));

    update(cx, &state, |state, cx| state.set_streaming(false, cx));
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    assert!(!is_open(cx, &state));

    // Opening it programmatically releases the pin.
    update(cx, &state, |state, cx| state.set_open(true, cx));
    assert!(is_open(cx, &state));
}

// —— Duration ————————————————————————————————————————————————————————————————

/// Upstream test #63: a sub-second stream rounds up to one second rather than
/// reading as live.
#[gpui::test]
fn a_sub_second_stream_rounds_up(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    cx.executor().advance_clock(Duration::from_millis(300));
    update(cx, &state, |state, cx| state.set_streaming(false, cx));
    assert_eq!(duration(cx, &state), Some(1));
}

/// A stream that ends in the second it began reads as zero — upstream's
/// `duration === 0`, which the trigger renders as a live shimmer.
#[gpui::test]
fn an_instant_stream_reads_as_zero(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    update(cx, &state, |state, cx| state.set_streaming(false, cx));
    assert_eq!(duration(cx, &state), Some(0));
}

/// Upstream's `duration` prop: the host's own timing wins until the block
/// measures a stream of its own.
#[gpui::test]
fn a_supplied_duration_overrides_the_measured_one(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    assert_eq!(duration(cx, &state), None);

    update(cx, &state, |state, cx| state.set_duration(Some(12), cx));
    assert_eq!(duration(cx, &state), Some(12));

    update(cx, &state, |state, cx| state.set_streaming(true, cx));
    cx.executor().advance_clock(Duration::from_secs(7));
    update(cx, &state, |state, cx| state.set_streaming(false, cx));
    assert_eq!(duration(cx, &state), Some(7));
}

// —— What the element tree mounts —————————————————————————————————————————————

/// Upstream's default: a closed panel is not in the tree at all.
#[gpui::test]
fn a_closed_un_animated_block_mounts_no_body(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    let visual = mount(cx, state, false);
    assert!(visual.debug_bounds("reasoning-0-trigger").is_some());
    assert!(visual.debug_bounds("reasoning-0-content").is_none());
}

/// An open block mounts its body.
#[gpui::test]
fn an_open_block_mounts_its_body(cx: &mut TestAppContext) {
    init(cx);
    let state = cx.update(|cx| cx.new(|cx| ReasoningState::new(cx).default_open(true)));
    let visual = mount(cx, state, false);
    assert!(visual.debug_bounds("reasoning-0-content").is_some());
}

/// The documented cost of animating: a reversible reveal measures the body it
/// reveals, so an animated block keeps it mounted while closed.
#[gpui::test]
fn an_animated_block_keeps_its_body_mounted_while_closed(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    let visual = mount(cx, state, true);
    assert!(visual.debug_bounds("reasoning-0-content").is_some());
}

/// The trigger's click path — the one wiring a state-level test cannot cover.
#[gpui::test]
fn clicking_the_trigger_opens_the_block(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    let visual = mount(cx, state.clone(), false);

    let trigger = visual
        .debug_bounds("reasoning-0-trigger")
        .expect("the trigger row must be laid out");
    visual.simulate_click(trigger.center(), Modifiers::default());
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.update(|_, cx| state.read(cx).is_open()));
    assert!(visual.debug_bounds("reasoning-0-content").is_some());
}

/// The trigger is a tab stop: a focused block opens and closes from the
/// keyboard, standing in for the `<button>` upstream's trigger renders.
#[gpui::test]
fn the_trigger_answers_the_keyboard(cx: &mut TestAppContext) {
    let state = closed_state(cx);
    let visual = mount(cx, state.clone(), false);

    // Mouse-down focuses the trigger, so a keystroke reaches its listener.
    let trigger = visual
        .debug_bounds("reasoning-0-trigger")
        .expect("the trigger row must be laid out");
    visual.simulate_click(trigger.center(), Modifiers::default());
    assert!(visual.update(|_, cx| state.read(cx).is_open()));

    visual.simulate_keystrokes("enter");
    assert!(!visual.update(|_, cx| state.read(cx).is_open()));

    visual.simulate_keystrokes("space");
    assert!(visual.update(|_, cx| state.read(cx).is_open()));
}

/// A block whose body is the real markdown component the conversation passes
/// in, so the body slot is exercised end to end.
struct MarkdownHarness {
    state: Entity<ReasoningState>,
    body: Entity<Markdown>,
}

impl Render for MarkdownHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Reasoning::new(("reasoning", 0usize), &self.state).content(
            div()
                .debug_selector(|| "body-probe".into())
                .child(self.body.clone()),
        )
    }
}

/// Both the gallery and the conversation pass a *themed* `Markdown` as the
/// body. An unthemed one still occupies the tree while painting nothing, which
/// reads as "the block opened and stayed blank" — so pin the painted height,
/// not just the element's presence.
#[gpui::test]
fn a_themed_markdown_body_paints_inside_the_block(cx: &mut TestAppContext) {
    init(cx);
    let body = cx.update(|cx| {
        cx.new(|cx| Markdown::new("body", "the thinking itself").theme(Theme::global(cx)))
    });
    let state = cx.update(|cx| cx.new(|cx| ReasoningState::new(cx).default_open(true)));
    let (_, visual) = cx.add_window_view(|_, _| MarkdownHarness { state, body });
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let bounds = visual
        .debug_bounds("body-probe")
        .expect("the body must be laid out");
    assert!(
        bounds.size.height > px(1.),
        "a themed body must paint, not just occupy the tree: {bounds:?}"
    );
}
