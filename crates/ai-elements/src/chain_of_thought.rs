//! A collapsible list of labeled steps — Vercel AI Elements' `ChainOfThought`.
//!
//! Upstream's shape, which this mirrors: `ChainOfThought` is the collapsible
//! container, `ChainOfThoughtHeader` its trigger row, and `ChainOfThoughtStep`
//! one labeled step drawn with a connector rail.
//!
//! Unlike `Reasoning`, upstream's chain of thought holds no behaviour of its own
//! — it has no streaming awareness, no duration, and neither opens nor folds
//! itself — so neither does this one. It is a controlled view: `open` in,
//! `on_toggle` out, and every policy (when to open, when to fold, what wins over
//! what) stays with the host.
//!
//! Divergences from upstream, each deliberate:
//!
//! - No `status` enum. A step takes an icon slot, so a host whose vocabulary is
//!   richer than upstream's `complete | active | pending` keeps it instead of
//!   collapsing it into three states.
//! - No `SearchResults` / `Image` elements, and a `meta` slot on the header
//!   instead: a host's aggregate counts and other trailing detail belong on the
//!   row it already draws them on, not inside a step.
//! - The last step draws no connector rail. Upstream paints one under every
//!   step, so its list ends on a stub hanging below the final marker.
//! - The layout-neutral animations — the chevron turning, the content fading and
//!   sliding, each step fading in as it mounts — are always on, and leave layout
//!   height alone (a `relative().top()` offset moves nothing else). The height
//!   reveal is separate and switchable via [`ChainOfThought::animated`]: on by
//!   default, as upstream's is, and off for a host whose layout cannot follow a
//!   height that changes frame by frame.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, ElementId, FocusHandle, Hsla,
    IntoElement, RenderOnce, SharedString, Window, div, ease_in_out, ease_out_quint, prelude::*,
    px, rems,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, ThemeStyled as _, collapsible::Collapsible,
    h_flex, v_flex,
};

use crate::animation::toggle_key;

/// Chevron rotation and content fade/slide — upstream's `transition-transform`.
const TOGGLE_MOTION: Duration = Duration::from_millis(160);

/// A step's entrance, upstream's `animate-in` on mount.
const STEP_APPEAR: Duration = Duration::from_millis(180);

/// Upstream's `slide-in-from-top-2` (0.5rem), as a fraction of the root size.
const SLIDE_REMS: f32 = 0.5;

/// Upstream's `onOpenChange`, as a gpui click handler.
pub type ToggleHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The step's marker column width, and the box whatever marker sits in.
const MARKER: f32 = 16.;

/// The default step marker: upstream's `DotIcon`.
const MARKER_DOT: f32 = 6.;

/// The collapsible container.
///
/// Layout is upstream's: the header row, then the steps under it with a `gap_3`.
/// The steps are typed rather than slot elements so the container can tell the
/// last one not to draw its connector rail.
#[derive(IntoElement)]
pub struct ChainOfThought {
    id: ElementId,
    open: bool,
    on_toggle: Option<ToggleHandler>,
    header: Option<ChainOfThoughtHeader>,
    steps: Vec<ChainOfThoughtStep>,
    animated: bool,
}

impl ChainOfThought {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            open: false,
            on_toggle: None,
            header: None,
            steps: Vec::new(),
            animated: true,
        }
    }

    /// Upstream's controlled `open`. Nothing here opens or folds the block on
    /// its own.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Upstream's `onOpenChange`: fired when the header row is clicked.
    pub fn on_toggle(
        mut self,
        on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }

    /// The trigger row. Without one the steps render bare — upstream has no such
    /// mode; a host that shows a label line instead owns that line itself.
    pub fn header(mut self, header: ChainOfThoughtHeader) -> Self {
        self.header = Some(header);
        self
    }

    pub fn step(mut self, step: ChainOfThoughtStep) -> Self {
        self.steps.push(step);
        self
    }

    /// Whether opening and closing reveal the steps with a height animation.
    ///
    /// On by default, as upstream's is. It also keeps the steps mounted while
    /// closed, since that is what makes the reveal reversible — so a host that
    /// unmounts its collapsed rows, or that caches row heights (a virtualized
    /// transcript cannot follow a height that changes frame by frame), turns it
    /// off and gets upstream's other half: a closed panel is not rendered.
    pub fn animated(mut self, animated: bool) -> Self {
        self.animated = animated;
        self
    }
}

impl RenderOnce for ChainOfThought {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ChainOfThought {
            id,
            open,
            on_toggle,
            header,
            steps,
            animated,
        } = self;
        let generation = toggle_generation(window, cx, &id, open);

        let last = steps.len().saturating_sub(1);
        let steps: Vec<AnyElement> = steps
            .into_iter()
            .enumerate()
            .map(|(ix, step)| {
                let step = if ix == last { step.rail(false) } else { step };
                step.into_any_element()
            })
            .collect();

        let body = div()
            .w_full()
            .min_w_0()
            .debug_selector({
                let id = id.clone();
                move || format!("{id}-content")
            })
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
        if animated {
            collapsible = collapsible.motion_id((id.clone(), "reveal"));
        }
        collapsible.content(body).into_any_element()
    }
}

/// The trigger row: an optional leading icon, the label, the disclosure chevron,
/// then whatever meta the host shows.
///
/// It is a slot for [`ChainOfThought`] rather than an element of its own, which
/// is upstream's contract too — its header reads the container's context and
/// throws without one.
pub struct ChainOfThoughtHeader {
    id: ElementId,
    icon: Option<AnyElement>,
    label: Option<AnyElement>,
    meta: Vec<AnyElement>,
}

impl ChainOfThoughtHeader {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            icon: None,
            label: None,
            meta: Vec::new(),
        }
    }

    /// The row's leading icon. Upstream draws a brain; `Brain` ships only in
    /// `gpui_kit_assets`' full catalog rather than the default bundle hosts
    /// register, so there is no default here — a header without one starts at
    /// its label.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// The row's label, upstream's `children` (default "Chain of Thought").
    /// A host that localizes its chrome passes its own text.
    pub fn label(mut self, label: impl IntoElement) -> Self {
        self.label = Some(label.into_any_element());
        self
    }

    /// A trailing detail — a count, an elapsed time, a badge. Upstream's header
    /// has no such slot and no equivalent content; this one carries it after the
    /// chevron, which is where the hosts in this repo already show it.
    pub fn meta(mut self, meta: impl IntoElement) -> Self {
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
        let (muted, foreground) = {
            let theme = cx.theme();
            (theme.muted_foreground, theme.foreground)
        };
        let chevron = Icon::new(IconName::ChevronDown)
            .xsmall()
            .with_animation(
                toggle_key(&self.id, "chevron", generation),
                Animation::new(TOGGLE_MOTION).with_easing(ease_in_out),
                move |icon, t| {
                    // Upstream turns a downward chevron a half turn when open;
                    // the closing arm plays the same sweep backwards.
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
            // A host's meta slots can outgrow a narrow column (one chip per tool
            // kind adds up); wrapping keeps them inside it.
            .flex_wrap()
            .when(on_toggle.is_some(), |row| {
                row.cursor_pointer()
                    .hover(move |row| row.text_color(foreground))
            })
            .when_some(focus, |row, focus| {
                row.track_focus(&focus)
                    .tab_index(0)
                    // The ring is a focus affordance only: a resting row draws
                    // none.
                    .when(focus.is_focused(window), |row| {
                        row.focus_ring_style(window, cx)
                    })
            })
            .when_some(on_toggle, |row, on_toggle| {
                row.on_click(move |event, window, cx| on_toggle(event, window, cx))
            })
            .children(self.icon)
            // The label takes its own width rather than growing to fill the
            // row: everything on this row reads left to right, in order, with
            // no gap between the block's identity and what it is reporting.
            // (Upstream lets the label grow and pins the chevron to the right
            // edge; a row of counts floating away from its label is worse.)
            .child(div().min_w_0().children(self.label))
            .child(chevron)
            .children(self.meta)
            .into_any_element()
    }
}

/// One labeled step: a marker (dot, icon, or spinner) over a connector rail, and
/// the label / description / content column beside it.
///
/// A step folds only when the host gives it a disclosure — upstream's step is
/// not interactive either. [`ChainOfThoughtStep::disclosed`] wraps
/// [`ChainOfThoughtStep::title`] in the affordance a folding row needs (chevron,
/// hover, click); a host with its own row shape passes
/// [`ChainOfThoughtStep::label`] instead.
#[derive(IntoElement)]
pub struct ChainOfThoughtStep {
    id: ElementId,
    icon: Option<AnyElement>,
    title: Option<SharedString>,
    disclosure: Option<StepDisclosure>,
    label: Option<AnyElement>,
    description: Option<AnyElement>,
    content: Option<AnyElement>,
    framed: bool,
    rail: bool,
    appear_animated: bool,
}

/// The step's own fold: which way it is, and what a click reports.
struct StepDisclosure {
    open: bool,
    on_toggle: ToggleHandler,
}

impl ChainOfThoughtStep {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            icon: None,
            title: None,
            disclosure: None,
            label: None,
            description: None,
            content: None,
            framed: false,
            rail: true,
            appear_animated: true,
        }
    }

    /// The marker. Upstream's default is a dot, which is also this one's —
    /// drawn rather than loaded, so it needs no asset.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// A host's own row for the step's identity. It replaces the default title
    /// row outright: mutually exclusive with [`ChainOfThoughtStep::title`] and
    /// [`ChainOfThoughtStep::disclosed`] — a step given both renders only the
    /// label, and the disclosure's affordance vanishes with the row it belongs
    /// to (asserted in debug builds).
    pub fn label(mut self, label: impl IntoElement) -> Self {
        self.label = Some(label.into_any_element());
        self
    }

    /// The row's one-line identity, drawn the way a step's identity reads
    /// everywhere in this crate: one line, monospace, muted, clipped to the
    /// column. A host with its own typography passes
    /// [`ChainOfThoughtStep::label`] instead.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Makes the step fold: a chevron beside the title, a hover wash, and a
    /// click that reports back (enter / space answer it once the row is
    /// focused). The affordance covers the title row only — a step's body is
    /// text a user may be selecting, so it never toggles. It belongs to the
    /// default title row: pairing it with [`ChainOfThoughtStep::label`] is a
    /// contract violation (asserted in debug builds).
    pub fn disclosed(
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
    pub fn framed(mut self, framed: bool) -> Self {
        self.framed = framed;
        self
    }

    /// Upstream's `description`: a muted second line under the label.
    pub fn description(mut self, description: impl IntoElement) -> Self {
        self.description = Some(description.into_any_element());
        self
    }

    /// The step's body — upstream's `children`.
    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }

    /// Whether this step draws the connector rail below its marker.
    /// [`ChainOfThought`] turns it off for the last step.
    pub fn rail(mut self, rail: bool) -> Self {
        self.rail = rail;
        self
    }

    /// Whether the step fades in as it mounts, upstream's `animate-in`. On by
    /// default; the animation is keyed on the step's own identity, so a
    /// re-render does not replay it.
    pub fn appear_animated(mut self, appear_animated: bool) -> Self {
        self.appear_animated = appear_animated;
        self
    }
}

impl RenderOnce for ChainOfThoughtStep {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        debug_assert!(
            self.label.is_none() || self.disclosure.is_none(),
            "`label` replaces the default title row: a step given both `label` \
             and `disclosed` never shows its disclosure"
        );
        let (border, muted) = {
            let theme = cx.theme();
            (theme.border, theme.muted_foreground)
        };

        let rail_id = self.id.clone();
        let row =
            h_flex()
                .id(self.id.clone())
                .debug_selector({
                    let id = self.id.clone();
                    move || format!("{id}")
                })
                .w_full()
                .min_w_0()
                .gap_2()
                .items_stretch()
                .child(
                    v_flex()
                        .w(px(MARKER))
                        .flex_shrink_0()
                        .items_center()
                        .child(marker_box(self.icon.unwrap_or_else(|| dot(border))))
                        .when(self.rail, |column| {
                            column.child(div().w(px(1.)).flex_1().bg(border).debug_selector({
                                let id = rail_id.clone();
                                move || format!("{id}-rail")
                            }))
                        }),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .children(self.label.or_else(|| {
                            self.title.map(|title| {
                                title_row(
                                    (self.id.clone(), "title").into(),
                                    title,
                                    self.disclosure,
                                    window,
                                    cx,
                                )
                            })
                        }))
                        .children(self.description.map(|description| {
                            div().text_xs().text_color(muted).child(description)
                        }))
                        .children(self.content.map(|content| {
                            if self.framed {
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
                            }
                        })),
                );

        if self.appear_animated {
            row.with_animation(
                toggle_key(&self.id, "appear", 0),
                Animation::new(STEP_APPEAR).with_easing(ease_out_quint()),
                move |row, t| row.opacity(t).relative().top(rems(-SLIDE_REMS * (1.0 - t))),
            )
            .into_any_element()
        } else {
            row.into_any_element()
        }
    }
}

/// Whatever marks a step sits in a fixed box, so the rail below it lines up
/// across steps regardless of what the marker is.
fn marker_box(marker: AnyElement) -> impl IntoElement {
    div()
        .w(px(MARKER))
        .h(px(MARKER))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(marker)
}

/// The default marker, upstream's `DotIcon`.
fn dot(color: Hsla) -> AnyElement {
    div()
        .size(px(MARKER_DOT))
        .rounded_full()
        .bg(color)
        .into_any_element()
}

/// How much of the row the hover wash tints.
const HOVER_WASH: f32 = 0.3;

/// A step title's one-line width — the conversation has always shown a tool
/// call's command summary at this length.
const TITLE_CHARS: usize = 80;

/// The step's title as a row: an optional disclosure chevron, then the title
/// itself clipped to one line. A disclosed row is a tab stop, so the fold is
/// keyboard-reachable exactly like the header row (see
/// [`ChainOfThoughtHeader::into_row`] for why it carries no key handler).
fn title_row(
    id: ElementId,
    title: SharedString,
    disclosure: Option<StepDisclosure>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let focus = disclosure.is_some().then(|| row_focus(window, cx, &id));
    let (muted, radius, hover_wash, mono_font) = {
        let theme = cx.theme();
        (
            theme.muted_foreground,
            theme.radius,
            theme.secondary.opacity(HOVER_WASH),
            theme.mono_font_family.clone(),
        )
    };
    let mut row = h_flex()
        .id(id)
        .w_full()
        .min_w_0()
        .py_0p5()
        .gap_1p5()
        .items_center()
        .italic()
        .rounded(radius);

    if let Some(disclosure) = disclosure {
        let on_toggle = disclosure.on_toggle;
        let chevron = if disclosure.open {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        row = row
            .cursor_pointer()
            .hover(move |row| row.bg(hover_wash))
            .child(Icon::new(chevron).xsmall().text_color(muted))
            .on_click(move |event, window, cx| on_toggle(event, window, cx));
    }

    row.when_some(focus, |row, focus| {
        row.track_focus(&focus)
            .tab_index(0)
            // The ring is a focus affordance only: a resting row draws none.
            .when(focus.is_focused(window), |row| {
                row.focus_ring_style(window, cx)
            })
    })
    .child(
        div()
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .text_sm()
            .font_family(mono_font)
            .text_color(muted)
            .child(one_line(&title, TITLE_CHARS)),
    )
    .into_any_element()
}

/// Collapse `s` to a single line, clipped to `max_chars` with an ellipsis.
///
/// `agent-ui`'s `message.rs` carries a line-for-line twin, `truncate` — the
/// crate boundary keeps the two apart, so change one and change the other.
fn one_line(s: &str, max_chars: usize) -> SharedString {
    let flat = s.replace('\n', " ");
    if flat.chars().count() > max_chars {
        let clipped: String = flat.chars().take(max_chars).collect();
        format!("{clipped}…").into()
    } else {
        flat.into()
    }
}

/// The generation a toggle animation is keyed on, for a block with no state
/// entity to hold one.
///
/// It lives in window-scoped element state: the generation advances on the
/// frame the open state differs from the last one seen, and stays put on every
/// other frame. Tracking it costs no extra repaint — `Entity::update` without
/// `cx.notify` never wakes the `use_keyed_state` observer, and the frame that
/// reads a new value is re-rendering for the host's own reason anyway.
struct ToggleTrack {
    open: bool,
    generation: u64,
}

impl ToggleTrack {
    /// Fold one rendered `open` value in and report the generation to key the
    /// toggle animations on.
    fn advance(&mut self, open: bool) -> u64 {
        if self.open != open {
            self.open = open;
            self.generation = self.generation.wrapping_add(1);
        }
        self.generation
    }
}

fn toggle_generation(window: &mut Window, cx: &mut App, id: &ElementId, open: bool) -> u64 {
    // The first frame seeds the track with the value the host mounts with, so
    // a block that is already open opens no generation — the toggle animations
    // replay only on a change the host actually made.
    let track = window.use_keyed_state((id.clone(), "toggle"), cx, |_, _| ToggleTrack {
        open,
        generation: 0,
    });
    track.update(cx, |track, _| track.advance(open))
}

/// A clickable row's focus handle, for a block that carries no state entity of
/// its own. Like [`toggle_generation`], it lives in window-scoped element
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

    /// The generation tracks host toggles, not renders: it advances only when
    /// `open` flips, and repeated frames of the same value hold it still.
    #[test]
    fn generation_advances_only_when_the_host_changes_open() {
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

    /// A block mounted open seeds the track with its mount value, so the first
    /// frame does not read as a toggle and replay the expansion animation.
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
