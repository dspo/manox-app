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
//! - No `SearchResults` / `Image` elements. A host's aggregate counts belong on
//!   the header it already draws them on, not as badges inside a step.
//! - The layout-neutral animations — the chevron turning, the content fading and
//!   sliding, each step fading in as it mounts — are always on, and leave layout
//!   height alone (a `relative().top()` offset moves nothing else). The height
//!   reveal is separate and switchable via [`ChainOfThought::animated`]: on by
//!   default, as upstream's is, and off for a host whose layout cannot follow a
//!   height that changes frame by frame.
//!
//! - No `status` enum, no `SearchResults` / `Image`, and a `meta` slot on the
//!   header: each of those keeps a host's own vocabulary and content where the
//!   host already draws them.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, ElementId, Hsla, IntoElement,
    RenderOnce, Window, div, ease_in_out, ease_out_quint, prelude::*, px, rems,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, collapsible::Collapsible, h_flex, v_flex,
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
        _window: &mut Window,
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
            .when_some(on_toggle, |row, on_toggle| {
                row.on_click(move |event, window, cx| on_toggle(event, window, cx))
            })
            .children(self.icon)
            .child(div().flex_1().min_w_0().children(self.label))
            .child(chevron)
            .children(self.meta)
            .into_any_element()
    }
}

/// One labeled step: a marker (dot, icon, or spinner) over a connector rail, and
/// the label / description / content column beside it.
///
/// The label is not interactive — upstream's step is not either. A host that
/// wants the step to fold gives the label its own disclosure affordance.
#[derive(IntoElement)]
pub struct ChainOfThoughtStep {
    id: ElementId,
    icon: Option<AnyElement>,
    label: Option<AnyElement>,
    description: Option<AnyElement>,
    content: Option<AnyElement>,
    rail: bool,
    appear_animated: bool,
}

impl ChainOfThoughtStep {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            icon: None,
            label: None,
            description: None,
            content: None,
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

    pub fn label(mut self, label: impl IntoElement) -> Self {
        self.label = Some(label.into_any_element());
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
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
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
                        .children(self.label)
                        .children(self.description.map(|description| {
                            div().text_xs().text_color(muted).child(description)
                        }))
                        .children(self.content),
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

/// The generation a toggle animation is keyed on, for a block with no state
/// entity to hold one.
///
/// It lives in window-scoped element state instead: the generation advances on
/// the frame the open state differs from the last one seen, and stays put on
/// every other frame. The extra repaint that mutating this state costs lands on
/// the frame that already changed — a toggle is user-driven, never per delta.
#[derive(Default)]
struct ToggleTrack {
    open: bool,
    generation: u64,
}

fn toggle_generation(window: &mut Window, cx: &mut App, id: &ElementId, open: bool) -> u64 {
    let track = window.use_keyed_state((id.clone(), "toggle"), cx, |_, _| ToggleTrack::default());
    track.update(cx, |track, _| {
        if track.open != open {
            track.open = open;
            track.generation = track.generation.wrapping_add(1);
        }
        track.generation
    })
}
