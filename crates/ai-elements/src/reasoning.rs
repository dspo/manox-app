//! A collapsible block that shows one round of a model's thinking stream.
//!
//! Mirrors Vercel AI Elements' `Reasoning` / `ReasoningTrigger` /
//! `ReasoningContent` triple: the block opens itself while the thinking stream
//! is live, folds itself a second after the stream ends, and reports the
//! elapsed thinking time on its trigger. `ReasoningState` is the gpui stand-in
//! for upstream's `Reasoning` context + `useReasoning()` hook — the entity owns
//! the open state, the duration clock, and the delayed fold, so callers drive
//! it with `set_streaming` / `set_open` and read it back with `is_open()`.
//!
//! Two behaviours deliberately extend upstream, both because the host
//! conversation already guarantees them:
//!
//! - A user toggle pins the block: once the trigger has been clicked by hand,
//!   neither auto-open nor auto-fold touches it again. Upstream folds a block
//!   the user just expanded.
//! - `set_open(false)` pins the block shut, so a later stream cannot re-open a
//!   block the caller deliberately closed.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, ElementId, Entity, EventEmitter,
    FocusHandle, IntoElement, RenderOnce, SharedString, Task, Window, div, ease_in_out,
    ease_out_quint, prelude::*, radians, rems,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, ThemeStyled as _, collapsible::Collapsible,
    h_flex, shimmer::ShimmerText, v_flex,
};

/// How long a block stays open after its stream ends before folding itself.
///
/// Matches upstream's `AUTO_CLOSE_DELAY`, so a finished round plays out its
/// last deltas before the body disappears.
pub const AUTO_CLOSE_DELAY: Duration = Duration::from_millis(1000);

/// One full sweep of the streaming shimmer — upstream's `<Shimmer duration={1}>`.
const SHIMMER_SWEEP: Duration = Duration::from_secs(1);

/// Chevron rotation and body fade/slide timing, upstream's Tailwind
/// `transition-transform` / `animate-in` duration.
const TOGGLE_MOTION: Duration = Duration::from_millis(160);

/// Upstream's `slide-in-from-top-2` (0.5rem), as a fraction of the root size.
const BODY_SLIDE_REMS: f32 = 0.5;

/// Emitted when the open state changes — upstream's `onOpenChange`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasoningEvent {
    OpenChanged { open: bool },
}

/// Open state, duration clock, and delayed fold for one reasoning round.
///
/// Construct it inside `cx.new`, then hand it to [`Reasoning`]. The mount-time
/// inputs are upstream's props: [`ReasoningState::default_open`] is
/// `defaultOpen` and [`ReasoningState::streaming`] is the `isStreaming` the
/// block mounts with. Both are order-insensitive — `default_open(false)` wins
/// over `streaming(true)` in either order, as upstream's
/// `defaultOpen ?? isStreaming` does.
pub struct ReasoningState {
    is_open: bool,
    is_streaming: bool,
    /// Set by `default_open(false)` and by any programmatic close: the block
    /// refuses to open itself. Upstream's `isExplicitlyClosed`.
    explicitly_closed: bool,
    /// Whether a stream ever ran. Upstream uses the same flag to keep a round
    /// the user opened by hand from folding itself later.
    has_ever_streamed: bool,
    /// The delayed fold fires at most once per block, as upstream's
    /// `hasAutoClosed` does.
    has_auto_closed: bool,
    /// The trigger was worked by hand — clicked or answered on the keyboard:
    /// the block is user-owned from here on.
    user_toggled: bool,
    started_at: Option<Instant>,
    duration: Option<u64>,
    /// Bumped on every open-state change. Toggle animations are keyed on it, so
    /// re-rendering the host replays neither of them.
    toggle_gen: u64,
    /// The trigger is a tab stop, so a keyboard can open and close the block
    /// the way upstream's `<button>` trigger allows.
    focus_handle: FocusHandle,
    _auto_close_task: Option<Task<()>>,
}

impl ReasoningState {
    /// A closed, idle block.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            is_open: false,
            is_streaming: false,
            explicitly_closed: false,
            has_ever_streamed: false,
            has_auto_closed: false,
            user_toggled: false,
            started_at: None,
            duration: None,
            toggle_gen: 0,
            focus_handle: cx.focus_handle(),
            _auto_close_task: None,
        }
    }

    /// Upstream's `defaultOpen`: the block mounts open, or mounts shut with
    /// auto-open pinned off.
    pub fn default_open(mut self, open: bool) -> Self {
        self.explicitly_closed = !open;
        self.is_open = open;
        self
    }

    /// Upstream's `isStreaming` at mount: a block born mid-stream starts its
    /// duration clock and opens itself, unless `default_open(false)` pinned it.
    pub fn streaming(mut self, streaming: bool, cx: &mut Context<Self>) -> Self {
        self.is_streaming = streaming;
        self.has_ever_streamed |= streaming;
        if streaming {
            self.started_at = Some(cx.background_executor().now());
            self.is_open = !self.explicitly_closed;
        }
        self
    }

    /// Upstream's `isStreaming`. The rising edge opens the block and starts the
    /// duration clock; the falling edge stops the clock, pins the elapsed
    /// seconds, and schedules the delayed fold.
    pub fn set_streaming(&mut self, streaming: bool, cx: &mut Context<Self>) {
        if self.is_streaming == streaming {
            return;
        }
        if streaming {
            self.is_streaming = true;
            self.has_ever_streamed = true;
            if self.started_at.is_none() {
                self.started_at = Some(cx.background_executor().now());
            }
            if !self.is_open && !self.explicitly_closed {
                self.set_open_inner(true, cx);
            }
        } else {
            self.is_streaming = false;
            if let Some(started) = self.started_at.take() {
                self.duration = Some(whole_secs(started, cx));
            }
            if self.may_auto_fold() {
                self._auto_close_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(AUTO_CLOSE_DELAY).await;
                    let _ = this.update(cx, |state, cx| state.fold_after_stream(cx));
                }));
            }
        }
        cx.notify();
    }

    /// Upstream's controlled `open` prop. Closing pins the block shut so a
    /// later stream cannot re-open it; opening releases the pin.
    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.explicitly_closed = !open;
        self.set_open_inner(open, cx);
    }

    /// Upstream's `setIsOpen` as driven by a user click: the block becomes
    /// user-owned, and nothing automatic moves it afterwards.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.user_toggled = true;
        let open = !self.is_open;
        self.explicitly_closed = !open;
        self.set_open_inner(open, cx);
    }

    /// Upstream's `duration` prop: an externally supplied elapsed time in
    /// seconds, overriding the one measured from the stream.
    pub fn set_duration(&mut self, duration: Option<u64>, cx: &mut Context<Self>) {
        self.duration = duration;
        cx.notify();
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// The trigger's focus handle. A host that adds its own keyboard affordance
    /// can focus the block through it.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    /// Elapsed thinking time in whole seconds — `None` until a stream ends
    /// (unless the caller supplied one), `Some(0)` for a stream that ended
    /// within the second it began.
    pub fn duration(&self) -> Option<u64> {
        self.duration
    }

    /// The fold is worth scheduling only for a block that actually streamed, is
    /// still open, has not folded before, and has not been claimed by a toggle.
    fn may_auto_fold(&self) -> bool {
        self.has_ever_streamed && self.is_open && !self.has_auto_closed && !self.user_toggled
    }

    /// The delayed fold. Re-checks the guards, because the user may have taken
    /// the block over during the delay; one-shot either way, so a block the
    /// user re-opens is never folded out from under them.
    fn fold_after_stream(&mut self, cx: &mut Context<Self>) {
        self.has_auto_closed = true;
        if !self.is_streaming && self.is_open && !self.user_toggled {
            self.set_open_inner(false, cx);
        }
        cx.notify();
    }

    fn set_open_inner(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.is_open == open {
            return;
        }
        self.is_open = open;
        self.toggle_gen = self.toggle_gen.wrapping_add(1);
        cx.emit(ReasoningEvent::OpenChanged { open });
        cx.notify();
    }
}

impl EventEmitter<ReasoningEvent> for ReasoningState {}

/// Elapsed whole seconds, rounded up — upstream's `Math.ceil`. A stream shorter
/// than a second reads as `0`, which is the case the trigger renders as a live
/// shimmer.
///
/// The clock is the executor's, not `Instant::now()`, so the elapsed time is
/// the same clock that drives the fold timer (and is virtual under test).
fn whole_secs(started: Instant, cx: &App) -> u64 {
    let elapsed = cx
        .background_executor()
        .now()
        .saturating_duration_since(started);
    elapsed.as_millis().div_ceil(1000) as u64
}

/// Upstream's `getThinkingMessage`: builds the trigger label from the block's
/// live state. The default is [`default_thinking_message`].
pub type ThinkingMessage = Rc<dyn Fn(bool, Option<u64>, &mut Window, &mut App) -> AnyElement>;

/// Which of upstream's three trigger labels a state selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThinkingLabel {
    /// A live stream — or one that ended inside its first second, which is
    /// indistinguishable from live and reads the same way.
    Shimmer,
    /// The stream never ran, or the host never reported its length.
    UnknownDuration,
    Seconds(u64),
}

/// Upstream's `defaultGetThinkingMessage` branches: a live stream (or a
/// zero-second one) shimmers "Thinking...", an unknown duration reads "Thought
/// for a few seconds", and a measured one reads its seconds.
fn thinking_label(is_streaming: bool, duration: Option<u64>) -> ThinkingLabel {
    if is_streaming || duration == Some(0) {
        ThinkingLabel::Shimmer
    } else {
        match duration {
            Some(secs) => ThinkingLabel::Seconds(secs),
            None => ThinkingLabel::UnknownDuration,
        }
    }
}

/// Renders [`thinking_label`] as the default trigger's message.
///
/// These are untranslated defaults. A host that localizes its chrome passes its
/// own [`ThinkingMessage`] rather than reading these.
pub fn default_thinking_message(
    is_streaming: bool,
    duration: Option<u64>,
    _cx: &mut App,
) -> AnyElement {
    match thinking_label(is_streaming, duration) {
        ThinkingLabel::Shimmer => ShimmerText::new("Thinking...")
            .duration(SHIMMER_SWEEP)
            .into_any_element(),
        ThinkingLabel::UnknownDuration => {
            SharedString::from("Thought for a few seconds").into_any_element()
        }
        ThinkingLabel::Seconds(secs) => {
            SharedString::from(format!("Thought for {secs} seconds")).into_any_element()
        }
    }
}

/// The clickable header row: state icon, thinking message, disclosure chevron.
///
/// Underneath [`Reasoning`] this is the default trigger. It is a tab stop and
/// opens on Enter or Space, standing in for the `<button>` upstream's
/// `CollapsibleTrigger` renders. It is public so a host can place it elsewhere
/// in its own layout; the debug selector it exposes is `{id}-trigger`.
///
/// Toggling is always `user_toggled`: a hand on the trigger — mouse or keyboard
/// — claims the block, which is what pins it against the automatic fold.
#[derive(IntoElement)]
pub struct ReasoningTrigger {
    id: ElementId,
    state: Entity<ReasoningState>,
    icon: Option<AnyElement>,
    get_thinking_message: Option<ThinkingMessage>,
}

impl ReasoningTrigger {
    pub fn new(id: impl Into<ElementId>, state: &Entity<ReasoningState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            icon: None,
            get_thinking_message: None,
        }
    }

    /// Replaces the leading state icon. Upstream draws a brain; `Brain` ships
    /// only in `gpui_kit_assets`' full catalog, not in the default component
    /// bundle hosts register, so the default here is the book-open mark the
    /// conversation already uses for reasoning.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// Upstream's `getThinkingMessage`.
    pub fn get_thinking_message(
        mut self,
        get_thinking_message: impl Fn(bool, Option<u64>, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.get_thinking_message = Some(Rc::new(get_thinking_message));
        self
    }
}

impl RenderOnce for ReasoningTrigger {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (is_open, is_streaming, duration, toggle_gen) = {
            let state = self.state.read(cx);
            (
                state.is_open,
                state.is_streaming,
                state.duration,
                state.toggle_gen,
            )
        };
        let (muted, foreground) = {
            let theme = cx.theme();
            (theme.muted_foreground, theme.foreground)
        };

        let message = match &self.get_thinking_message {
            Some(get) => get(is_streaming, duration, window, cx),
            None => default_thinking_message(is_streaming, duration, cx),
        };

        let click_state = self.state.clone();
        let chevron = Icon::new(IconName::ChevronDown)
            .xsmall()
            .with_animation(
                animation_key(&self.id, "chevron", toggle_gen),
                Animation::new(TOGGLE_MOTION).with_easing(ease_in_out),
                move |icon, t| {
                    // Upstream rotates a downward chevron a half turn when open;
                    // the closing arm plays the same sweep backwards.
                    let sweep = if is_open { t } else { 1.0 - t };
                    icon.rotate(radians(std::f32::consts::PI * sweep))
                },
            )
            .into_any_element();

        let focus_handle = self.state.read(cx).focus_handle.clone();
        let keyboard_state = self.state.clone();

        h_flex()
            .id(self.id.clone())
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}-trigger")
            })
            .track_focus(&focus_handle)
            .tab_index(0)
            .w_full()
            .min_w_0()
            .gap_2()
            .text_sm()
            .text_color(muted)
            .cursor_pointer()
            .hover(move |row| row.text_color(foreground))
            // The ring is a focus affordance only: a resting trigger draws none.
            .when(focus_handle.is_focused(window), |row| {
                row.focus_ring_style(window, cx)
            })
            .on_click(move |_, _window, cx| click_state.update(cx, |state, cx| state.toggle(cx)))
            .on_key_down(move |event, _window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    keyboard_state.update(cx, |state, cx| state.toggle(cx));
                }
            })
            .child(
                self.icon
                    .unwrap_or_else(|| Icon::new(IconName::BookOpen).xsmall().into_any_element()),
            )
            .child(div().flex_1().min_w_0().child(message))
            .child(chevron)
    }
}

/// The collapsible body. Applies upstream's
/// `mt-4 text-sm text-muted-foreground` to whatever content the block was
/// handed.
#[derive(IntoElement)]
pub struct ReasoningContent {
    body: AnyElement,
}

impl ReasoningContent {
    pub fn new(body: impl IntoElement) -> Self {
        Self {
            body: body.into_any_element(),
        }
    }
}

impl RenderOnce for ReasoningContent {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .w_full()
            .min_w_0()
            .mt_4()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(self.body)
    }
}

/// One reasoning round: a trigger row over a collapsible body.
///
/// The block is stateless — [`ReasoningState`] holds everything that outlives a
/// frame. Because that state lives in its own entity, a host rendering this
/// element must also re-render when the state changes (observe the entity, or
/// subscribe to [`ReasoningEvent`]) for a fold or a toggle to reach the screen.
///
/// Upstream's `not-prose mb-4` root styling is deliberately omitted: the host
/// list owns the spacing between conversation blocks.
#[derive(IntoElement)]
pub struct Reasoning {
    id: ElementId,
    state: Entity<ReasoningState>,
    trigger: Option<AnyElement>,
    content: Option<AnyElement>,
    icon: Option<AnyElement>,
    get_thinking_message: Option<ThinkingMessage>,
    animated: bool,
}

impl Reasoning {
    pub fn new(id: impl Into<ElementId>, state: &Entity<ReasoningState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            trigger: None,
            content: None,
            icon: None,
            get_thinking_message: None,
            animated: true,
        }
    }

    /// Replaces the whole trigger row — upstream's `children` on
    /// `ReasoningTrigger`.
    pub fn trigger(mut self, trigger: impl IntoElement) -> Self {
        self.trigger = Some(trigger.into_any_element());
        self
    }

    /// The body. Upstream's `ReasoningContent` takes a markdown string; here it
    /// takes the element the host already renders, so a streaming body keeps
    /// its own persistent markdown document (selection included) instead of
    /// being re-parsed per frame.
    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }

    /// Replaces the default trigger's leading icon.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// Upstream's `getThinkingMessage`, applied to the default trigger.
    pub fn get_thinking_message(
        mut self,
        get_thinking_message: impl Fn(bool, Option<u64>, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.get_thinking_message = Some(Rc::new(get_thinking_message));
        self
    }

    /// Whether opening and closing animate. Upstream animates both the reveal
    /// and the body's fade/slide. An animated body stays mounted while closed —
    /// that is what makes the reveal reversible — so a host that virtualizes
    /// long transcripts may prefer `animated(false)`, which is upstream's
    /// behaviour too: a closed panel is not rendered at all.
    pub fn animated(mut self, animated: bool) -> Self {
        self.animated = animated;
        self
    }
}

impl RenderOnce for Reasoning {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Reasoning {
            id,
            state,
            trigger,
            content,
            icon,
            get_thinking_message,
            animated,
        } = self;

        let (is_open, toggle_gen) = {
            let state = state.read(cx);
            (state.is_open, state.toggle_gen)
        };

        let trigger = trigger.unwrap_or_else(|| {
            let mut trigger = ReasoningTrigger::new(id.clone(), &state);
            if let Some(icon) = icon {
                trigger = trigger.icon(icon);
            }
            if let Some(get) = get_thinking_message {
                trigger =
                    trigger.get_thinking_message(move |a, b, window, cx| get(a, b, window, cx));
            }
            trigger.into_any_element()
        });

        let body = content.map(|body| {
            let wrapper = div()
                .w_full()
                .min_w_0()
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-content")
                })
                .child(body);
            let inner = if animated {
                wrapper
                    .with_animation(
                        content_animation_id(&id, toggle_gen),
                        Animation::new(TOGGLE_MOTION).with_easing(ease_out_quint()),
                        move |body, t| {
                            let t = if is_open { t } else { 1.0 - t };
                            body.opacity(t)
                                .relative()
                                .top(rems(-BODY_SLIDE_REMS * (1.0 - t)))
                        },
                    )
                    .into_any_element()
            } else {
                wrapper.into_any_element()
            };
            ReasoningContent::new(inner).into_any_element()
        });

        let mut collapsible = Collapsible::new().open(is_open).child(trigger);
        if animated {
            collapsible = collapsible.motion_id((id.clone(), "reveal"));
        }
        match body {
            Some(body) => collapsible.content(body).into_any_element(),
            None => collapsible.into_any_element(),
        }
    }
}

/// A toggle animation's identity. The generation is part of the key so the
/// animation restarts when the block opens or closes, and only then — a
/// re-render that leaves the state alone reuses the key and replays nothing.
fn content_animation_id(id: &ElementId, toggle_gen: u64) -> ElementId {
    animation_key(id, "fade", toggle_gen)
}

fn animation_key(id: &ElementId, role: &'static str, toggle_gen: u64) -> ElementId {
    let role_id: ElementId = (id.clone(), role).into();
    (role_id, format!("g{toggle_gen}")).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upstream's `defaultGetThinkingMessage` branches, case for case: the
    /// label follows the live flag and the measured seconds, and a zero-second
    /// stream reads as live.
    #[test]
    fn trigger_label_follows_upstream() {
        assert_eq!(thinking_label(true, None), ThinkingLabel::Shimmer);
        assert_eq!(thinking_label(true, Some(4)), ThinkingLabel::Shimmer);
        assert_eq!(thinking_label(false, Some(0)), ThinkingLabel::Shimmer);
        assert_eq!(thinking_label(false, None), ThinkingLabel::UnknownDuration);
        assert_eq!(thinking_label(false, Some(1)), ThinkingLabel::Seconds(1));
        assert_eq!(thinking_label(false, Some(12)), ThinkingLabel::Seconds(12));
    }
}
