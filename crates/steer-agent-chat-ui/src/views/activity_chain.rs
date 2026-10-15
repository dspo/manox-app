//! The conversation's activity segment: a collapsible header row over one
//! labeled step per entry, flush with the column's left edge — no marker
//! column, no connector rail (the chain-of-thought look is retired; the
//! entry's title row is its own fold affordance). A view-only fold local to
//! the chat column — the open state and every click's meaning stay with the
//! caller, exactly as the container state in `conversation.rs` owns them.
//!
//! The height of the fold never animates: the message list caches row
//! heights, so a reveal that changes height frame by frame would go stale
//! under it. The layout-neutral motions — the chevron turning, the content
//! fading and sliding, each step fading in as it mounts — stay on and move
//! nothing else (a `relative().top()` offset leaves layout height alone).
//! While collapsed the steps are not rendered at all.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, ElementId, FocusHandle, IntoElement,
    RenderOnce, SharedString, Window, div, ease_in_out, ease_out_quint, linear_color_stop,
    linear_gradient, prelude::*, px, rems,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, collapsible::Collapsible, h_flex, v_flex,
};

use super::message::truncate;

/// Chevron rotation and content fade/slide, one motion budget for both.
const TOGGLE_MOTION: Duration = Duration::from_millis(160);

/// A step's entrance on mount.
const STEP_APPEAR: Duration = Duration::from_millis(180);

/// The content's slide-in distance, in fractions of the root size.
const SLIDE_REMS: f32 = 0.5;

/// A click that folds, as a gpui handler.
pub(crate) type ToggleHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The collapsible segment container: the header row, then the steps.
#[derive(IntoElement)]
pub(crate) struct ActivityChain {
    id: ElementId,
    open: bool,
    on_toggle: Option<ToggleHandler>,
    header: Option<ActivityChainHeader>,
    steps: Vec<ActivityStep>,
}

impl ActivityChain {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            open: false,
            on_toggle: None,
            header: None,
            steps: Vec::new(),
        }
    }

    /// The fold's open state; nothing here opens or folds on its own.
    pub(crate) fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Fired when the header row is clicked.
    pub(crate) fn on_toggle(
        mut self,
        on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }

    pub(crate) fn header(mut self, header: ActivityChainHeader) -> Self {
        self.header = Some(header);
        self
    }

    pub(crate) fn step(mut self, step: ActivityStep) -> Self {
        self.steps.push(step);
        self
    }
}

impl RenderOnce for ActivityChain {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ActivityChain {
            id,
            open,
            on_toggle,
            header,
            steps,
        } = self;
        let generation = toggle_generation(window, cx, &id, open);
        let steps: Vec<AnyElement> = steps
            .into_iter()
            .map(|step| step.into_any_element())
            .collect();

        // No height cap: `overflow_y_scroll` never established its scroll
        // frame inside the animation wrapper, so a cap only clipped — the
        // entries past the limit (tools after a long reasoning) vanished
        // with no way to scroll to them. The steps play at natural height.
        let body = div()
            .w_full()
            .min_w_0()
            .with_animation(
                toggle_key(&id, "content", generation),
                Animation::new(TOGGLE_MOTION).with_easing(ease_out_quint()),
                move |body, t| {
                    let t = if open { t } else { 1.0 - t };
                    body.opacity(t)
                        .relative()
                        .top(rems(-SLIDE_REMS * (1.0 - t)))
                },
            )
            .child(v_flex().w_full().min_w_0().gap_3().children(steps));

        let mut collapsible = Collapsible::new().open(open);
        if let Some(header) = header {
            collapsible =
                collapsible.child(header.into_row(open, on_toggle, generation, window, cx));
        }
        // No `motion_id`: the height reveal is off (see the module doc), so a
        // closed segment's steps are unmounted rather than held for a
        // reversible reveal.
        collapsible.content(body).into_any_element()
    }
}

/// The segment's trigger row: the label, the disclosure chevron, then
/// whatever aggregate detail the caller shows (counts, elapsed, badges).
pub(crate) struct ActivityChainHeader {
    id: ElementId,
    label: Option<AnyElement>,
    meta: Vec<AnyElement>,
}

impl ActivityChainHeader {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            meta: Vec::new(),
        }
    }

    /// The row's identity — the model display name; the segment is the single
    /// place a turn's model shows.
    pub(crate) fn label(mut self, label: impl IntoElement) -> Self {
        self.label = Some(label.into_any_element());
        self
    }

    /// A trailing detail riding after the chevron.
    pub(crate) fn meta(mut self, meta: impl IntoElement) -> Self {
        self.meta.push(meta.into_any_element());
        self
    }

    fn into_row(
        self,
        open: bool,
        on_toggle: Option<ToggleHandler>,
        generation: u64,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let (muted, foreground, accent) = {
            let theme = cx.theme();
            (theme.muted_foreground, theme.foreground, theme.accent)
        };
        let chevron = Icon::new(IconName::ChevronDown)
            .xsmall()
            .with_animation(
                toggle_key(&self.id, "chevron", generation),
                Animation::new(TOGGLE_MOTION).with_easing(ease_in_out),
                move |icon, t| {
                    // The opening arm turns the chevron a half turn; the
                    // closing arm plays the same sweep backwards.
                    let sweep = if open { t } else { 1.0 - t };
                    icon.rotate(gpui::radians(std::f32::consts::PI * sweep))
                },
            )
            .into_any_element();

        // A row the pointer can toggle is a tab stop the keyboard can answer
        // too. A focused gpui div already turns enter / space into a
        // `ClickEvent::Keyboard` delivered to its own click listeners, so the
        // row needs no key handler of its own — one would toggle twice; it
        // only needs to be focusable.
        let focus = on_toggle.is_some().then(|| row_focus(window, cx, &self.id));
        h_flex()
            .id(self.id.clone())
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}")
            })
            .w_full()
            .min_w_0()
            .py_0p5()
            .gap_2()
            .text_sm()
            .text_color(muted)
            // The caller's meta slots can outgrow a narrow column (one chip
            // per tool kind adds up); wrapping keeps them inside it.
            .flex_wrap()
            .when(on_toggle.is_some(), |row| {
                row.cursor_pointer()
                    .hover(move |row| row.text_color(foreground))
            })
            .when_some(focus, |row, focus| {
                row.track_focus(&focus)
                    .tab_index(0)
                    // Keyboard-only affordance: a mouse click must not
                    // leave a ring parked on the row.
                    .focus_visible(|s| s.border_1().border_color(accent))
            })
            .when_some(on_toggle, |row, on_toggle| {
                row.on_click(move |event, window, cx| on_toggle(event, window, cx))
            })
            // The label takes its own width rather than growing to fill the
            // row: everything on this row reads left to right, in order, with
            // no gap between the segment's identity and what it reports.
            .child(div().min_w_0().children(self.label))
            .child(chevron)
            .children(self.meta)
            .into_any_element()
    }
}

/// One labeled step: a head row over its content, flush left — the whole
/// column width is the entry's (no marker column, no rail). The step folds
/// only when the caller gives it a disclosure: the head row carries the
/// disclosure chevron and is itself the affordance (hover, click, keyboard),
/// never the body (text a user may be selecting).
#[derive(IntoElement)]
pub(crate) struct ActivityStep {
    id: ElementId,
    /// The always-on identity row's leading glyph (the thinking icon) —
    /// sits before the title, muted.
    leading_icon: Option<AnyElement>,
    /// The always-on identity row (a tool's command line, a round's label).
    title: Option<SharedString>,
    /// The identity row's trailing summary — the reasoning round's own
    /// first line while folded (the dsh disclosure shape: the summary rides
    /// the title row, never a face of its own).
    summary: Option<SharedString>,
    disclosure: Option<StepDisclosure>,
    content: Option<AnyElement>,
    framed: bool,
}

/// The step's own fold: which way it is, and what a click reports.
struct StepDisclosure {
    open: bool,
    on_toggle: ToggleHandler,
}

impl ActivityStep {
    pub(crate) fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            leading_icon: None,
            title: None,
            summary: None,
            disclosure: None,
            content: None,
            framed: false,
        }
    }

    /// The identity row's leading glyph.
    pub(crate) fn leading_icon(mut self, icon: impl IntoElement) -> Self {
        self.leading_icon = Some(icon.into_any_element());
        self
    }

    /// The identity row's trailing summary (shown while folded).
    pub(crate) fn summary(mut self, summary: impl Into<SharedString>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// The row's one-line identity, monospace, muted, clipped to the column.
    pub(crate) fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Makes the step fold: the head row grows a disclosure chevron and
    /// becomes the affordance — a hover wash and a click that reports back
    /// (enter / space answer it once the row is focused).
    pub(crate) fn disclosed(
        mut self,
        open: bool,
        on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.disclosure = Some(StepDisclosure {
            open,
            on_toggle: Rc::new(on_toggle),
        });
        self
    }

    /// Wraps the content in the bordered box a terminal-styled body wears.
    pub(crate) fn framed(mut self, framed: bool) -> Self {
        self.framed = framed;
        self
    }

    /// The step's body.
    pub(crate) fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }
}

impl RenderOnce for ActivityStep {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = {
            let theme = cx.theme();
            theme.border
        };
        // Flush left: the identity row and the content each span the
        // column's full width. The identity row (icon + title + folded
        // summary) is the ONE fold face; the content renders only while
        // open. A step with no identity row at all (a nameless orphan tool
        // entry) carries no fold affordance, so its body renders outright
        // rather than being stranded behind a fold nothing can open.
        let open = self.disclosure.as_ref().is_some_and(|d| d.open) || self.title.is_none();
        let head = self.title.clone().map(|title| {
            title_row(
                self.id.clone(),
                self.leading_icon,
                title,
                (!open).then_some(self.summary.clone()).flatten(),
                self.disclosure,
                window,
                cx,
            )
        });
        let content = open.then_some(self.content).flatten();
        let row = v_flex()
            .id(self.id.clone())
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}")
            })
            .w_full()
            .min_w_0()
            .gap_1()
            .children(head);
        let row = match content {
            Some(content) => row.child(if self.framed {
                div()
                    .w_full()
                    .min_w_0()
                    .italic()
                    .border_1()
                    .border_color(border)
                    .rounded(cx.theme().radius)
                    .overflow_hidden()
                    .child(content)
                    .into_any_element()
            } else {
                content
            }),
            None => row,
        };

        // The entrance is keyed on the step's own identity, so a re-render
        // does not replay it.
        row.with_animation(
            toggle_key(&self.id, "appear", 0),
            Animation::new(STEP_APPEAR).with_easing(ease_out_quint()),
            move |row, t| row.opacity(t).relative().top(rems(-SLIDE_REMS * (1.0 - t))),
        )
        .into_any_element()
    }
}

/// How much of the row the hover wash tints.
const HOVER_WASH: f32 = 0.3;

/// A step title's one-line width — the conversation has always shown a tool
/// call's command summary at this length.
const TITLE_CHARS: usize = 80;

/// The step's head row: a disclosure chevron (when the step folds), an
/// optional leading glyph, the text clipped to one line, then the folded
/// summary sharing the row. The title rides the tool command-line voice
/// (mono); a reasoning round's summary rides the ambient proportional font.
/// The row IS the affordance (hover wash, cursor, click; enter / space once
/// focused) — a tab stop, so the fold is keyboard-reachable exactly like the
/// header row.
// One render input per slot (the leaf convention `render_item` documents).
#[allow(clippy::too_many_arguments)]
fn title_row(
    id: ElementId,
    leading_icon: Option<AnyElement>,
    title: SharedString,
    summary: Option<SharedString>,
    disclosure: Option<StepDisclosure>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let focus = disclosure.is_some().then(|| row_focus(window, cx, &id));
    let (muted, radius, hover_wash, mono_font, accent, background) = {
        let theme = cx.theme();
        (
            theme.muted_foreground,
            theme.radius,
            theme.secondary.opacity(HOVER_WASH),
            theme.mono_font_family.clone(),
            theme.accent,
            theme.background,
        )
    };
    let head_selector = format!("{id}-head");
    let mut row = h_flex()
        .id(id)
        .debug_selector(move || head_selector.clone())
        .w_full()
        .min_w_0()
        .py_0p5()
        .items_center()
        .gap_1()
        .italic()
        .rounded(radius);

    if let Some(disclosure) = disclosure {
        let on_toggle = disclosure.on_toggle;
        let open = disclosure.open;
        row = row
            .cursor_pointer()
            .hover(move |row| row.bg(hover_wash))
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(muted),
            )
            .on_click(move |event, window, cx| on_toggle(event, window, cx));
    }

    // The title takes its natural width whenever a summary shares the row
    // (the dsh ReasoningRow shape: title, a separator dot, the summary
    // filling the rest); alone it fills the row and truncates.
    let text = div()
        .min_w_0()
        .overflow_x_hidden()
        .text_sm()
        .font_family(mono_font)
        .text_color(muted);
    let text = if summary.is_some() {
        text.flex_shrink_0()
    } else {
        text.flex_1()
    };
    let row = row
        .when_some(focus, |row, focus| {
            row.track_focus(&focus)
                .tab_index(0)
                // Keyboard-only affordance: a mouse click must not leave a
                // ring parked on the row.
                .focus_visible(|s| s.border_1().border_color(accent))
        })
        .children(leading_icon)
        .child(text.child(truncate(&title, TITLE_CHARS)));
    let row = match summary {
        Some(summary) => row
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(2.))
                    .rounded_full()
                    .bg(muted.opacity(0.6)),
            )
            .child(
                div()
                    .relative()
                    .min_w_0()
                    .flex_1()
                    .overflow_x_hidden()
                    .text_sm()
                    .text_color(muted.opacity(0.85))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .overflow_x_hidden()
                            .child(truncate(&summary, TITLE_CHARS)),
                    )
                    .child(
                        // The clip reads as a soft preview, not a hard cut.
                        div().absolute().top_0().bottom_0().right_0().w(px(24.)).bg(
                            linear_gradient(
                                90.,
                                linear_color_stop(gpui::transparent_black(), 0.),
                                linear_color_stop(background, 1.),
                            ),
                        ),
                    ),
            ),
        None => row,
    };
    row.into_any_element()
}

/// The identity a toggle animation is keyed on.
///
/// gpui replays an animation when its key changes and only then, so the key
/// has to carry the block's identity *and* which way it is going.
/// `generation` is that second half, derived here from the frames seen (the
/// fold has no state entity of its own). Passing an unchanging generation
/// gives an animation that plays once per mount, which is what a step's
/// entrance wants.
fn toggle_key(id: &ElementId, role: &'static str, generation: u64) -> ElementId {
    let role_id: ElementId = (id.clone(), role).into();
    (role_id, format!("g{generation}")).into()
}

/// The generation a toggle animation is keyed on, for a block with no state
/// entity to hold one.
///
/// It lives in window-scoped element state: the generation advances on the
/// frame the open state differs from the last one seen, and stays put on
/// every other frame. Tracking it costs no extra repaint — `Entity::update`
/// without `cx.notify` never wakes the `use_keyed_state` observer, and the
/// frame that reads a new value is re-rendering for the host's own reason
/// anyway.
struct ToggleTrack {
    open: bool,
    generation: u64,
}

impl ToggleTrack {
    /// Fold one rendered `open` value in and report the generation to key
    /// the toggle animations on.
    fn advance(&mut self, open: bool) -> u64 {
        if self.open != open {
            self.open = open;
            self.generation = self.generation.wrapping_add(1);
        }
        self.generation
    }
}

fn toggle_generation(window: &mut Window, cx: &mut App, id: &ElementId, open: bool) -> u64 {
    // The first frame seeds the track with the value the caller mounts with,
    // so a block that is already open opens no generation — the toggle
    // animations replay only on a change the caller actually made.
    let track = window.use_keyed_state((id.clone(), "toggle"), cx, |_, _| ToggleTrack {
        open,
        generation: 0,
    });
    track.update(cx, |track, _| track.advance(open))
}

/// A clickable row's focus handle, for a block that carries no state entity
/// of its own. Like [`toggle_generation`], it lives in window-scoped element
/// state, so the tab stop and its focus ring survive re-renders.
struct RowFocus {
    handle: FocusHandle,
}

fn row_focus(window: &mut Window, cx: &mut App, id: &ElementId) -> FocusHandle {
    let track = window.use_keyed_state((id.clone(), "focus"), cx, |_, cx| RowFocus {
        handle: cx.focus_handle(),
    });
    track.read(cx).handle.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generation tracks caller toggles, not renders: it advances only
    /// when `open` flips, and repeated frames of the same value hold it
    /// still.
    #[test]
    fn generation_advances_only_when_the_caller_changes_open() {
        let mut track = ToggleTrack {
            open: false,
            generation: 0,
        };
        assert_eq!(track.advance(false), 0);
        assert_eq!(track.advance(true), 1);
        assert_eq!(track.advance(true), 1);
        assert_eq!(track.advance(true), 1);
        assert_eq!(track.advance(false), 2);
    }

    /// A block mounted open seeds the track with its mount value, so the
    /// first frame does not read as a toggle and replay the expansion
    /// animation.
    #[test]
    fn a_block_mounted_open_does_not_advance_on_its_first_frame() {
        let mut track = ToggleTrack {
            open: true,
            generation: 0,
        };
        assert_eq!(track.advance(true), 0);
        assert_eq!(track.advance(false), 1);
    }
}
