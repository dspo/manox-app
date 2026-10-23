//! List items must stack as line boxes plus one small uniform inter-item gap
//! (0.8.0 "more restrained" lists): a marker wrap or an uneven gap breaks the
//! equality, and the gap must stay far under a line box so lists read denser
//! than paragraphs. The heading ladder is upstream's relative hierarchy
//! (size, line height and section spacing all scale with the inherited font
//! size, H1 largest, H6 smallest) — the app pins no overrides, so the guard
//! pins the ladder's shape. The document tiers are density-equal: the
//! reasoning tier differs from the message body by color alone, so at one
//! body size the two must measure the same.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext, Bounds, Element, Entity, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, ParentElement, Pixels, Render, Style, Styled, Window, div, px,
};
use gpui_base::TextView;
use gpui_component::{Root, Theme};
use steer_components::markdown::{
    TextViewState, message_document, message_style, reasoning_document,
};

struct Measure {
    child: AnyElement,
    out: Rc<Cell<Pixels>>,
}

impl IntoElement for Measure {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Measure {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child_id = self.child.request_layout(window, cx);
        (window.request_layout(Style::default(), [child_id], cx), ())
    }
    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.out.set(bounds.size.height);
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _state: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

struct Host {
    source: String,
    body: Option<Pixels>,
    out: Rc<Cell<Pixels>>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = Theme::default();
        let text_size = self.body.unwrap_or(px(13.));
        let view = TextView::markdown("m", self.source.clone())
            .style(message_style(&theme))
            .text_size(text_size);
        div().w(px(600.)).child(Measure {
            child: view.into_any_element(),
            out: self.out.clone(),
        })
    }
}

fn measure_raw(cx: &mut gpui::TestAppContext, source: &str, body: Option<Pixels>) -> f32 {
    cx.update(gpui_base::init);
    let out = Rc::new(Cell::new(px(0.)));
    let src = source.to_string();
    let o2 = out.clone();
    let (_view, cx) = cx.add_window_view(move |_window, _cx| Host {
        source: src,
        body,
        out: o2,
    });
    cx.run_until_parked();
    f32::from(out.get())
}

fn measure(cx: &mut gpui::TestAppContext, source: &str) -> f32 {
    measure_raw(cx, source, Some(px(13.)))
}

/// A document constructor over a persistent [`TextViewState`] — the mount every
/// conversation tier renders through, so the comparison below is
/// constructor-for-constructor and nothing else.
type TierDocument = fn(&Entity<TextViewState>, &Theme, Pixels, &App) -> TextView;

struct TierHost {
    state: Entity<TextViewState>,
    document: TierDocument,
    body: Pixels,
    out: Rc<Cell<Pixels>>,
}

impl Render for TierHost {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let view = (self.document)(&self.state, &Theme::default(), self.body, cx);
        div().w(px(600.)).child(Measure {
            child: view.into_any_element(),
            out: self.out.clone(),
        })
    }
}

fn measure_tier(cx: &mut gpui::TestAppContext, source: &str, document: TierDocument) -> f32 {
    cx.update(gpui_base::init);
    let out = Rc::new(Cell::new(px(0.)));
    let src = source.to_string();
    let o2 = out.clone();
    let (_view, cx) = cx.add_window_view(move |_window, cx| {
        let state = cx.new(|cx| TextViewState::markdown(&src, cx));
        TierHost {
            state,
            document,
            body: px(13.),
            out: o2,
        }
    });
    cx.run_until_parked();
    f32::from(out.get())
}

#[gpui::test]
fn list_vertical_footprint_matches_body_paragraphs(cx: &mut gpui::TestAppContext) {
    let lines = [
        "alpha bravo charlie",
        "delta echo foxtrot",
        "golf hotel india",
        "juliet kilo lima",
        "mike november oscar",
        "papa quebec romeo",
        "sierra tango uniform",
        "victor whiskey xray",
        "yankee zulu one",
        "two three four",
        "five six seven",
        "eight nine ten",
    ];
    let paras = lines.join("\n\n");
    let tight = lines
        .iter()
        .map(|l| format!("- {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    let ordered = lines
        .iter()
        .map(|l| format!("1. {l}"))
        .collect::<Vec<_>>()
        .join("\n");

    let one_par = measure(cx, lines[0]);
    let one_list = measure(cx, &format!("- {}", lines[0]));
    let twelve_par = measure(cx, &paras);
    let twelve_tight = measure(cx, &tight);
    let twelve_ordered = measure(cx, &ordered);

    assert!(
        (one_list - one_par).abs() <= 3.0,
        "single-item list {one_list}px must match single paragraph {one_par}px"
    );
    // 0.8.0 spaces items with one small uniform gap: N items stack as N line
    // boxes plus N-1 gaps, and a marker wrap would inflate an item by an
    // invisible extra line, breaking the arithmetic.
    let line = one_par;
    let gap = (twelve_tight - 12.0 * line) / 11.0;
    assert!(
        gap > 0.0 && gap <= 6.0,
        "12-item list {twelve_tight}px resolves to an inter-item gap of {gap}px          (line box {line}px) — it must be small and uniform"
    );
    assert!(
        (twelve_ordered - twelve_tight).abs() <= 1.0,
        "12-item ordered list {twelve_ordered}px must match the tight list {twelve_tight}px"
    );
    // The paragraphs equivalence is density-relative: paragraphs pay the
    // paragraph gap between blocks, so 12 paragraphs must be taller than the
    // tight list but not absurdly so (11 gaps at ≤ 1rem each).
    assert!(
        twelve_par > twelve_tight && twelve_par - twelve_tight <= 11.0 * 16.0,
        "12 paragraphs {twelve_par}px must exceed the tight list {twelve_tight}px by gaps only"
    );
}

/// Heading ladder under upstream's relative hierarchy: the footprint
/// descends strictly from H1 to H6, H1 stays within a bounded multiple of the
/// body line (one section header must not dwarf the transcript), and H4 — the
/// body-sized rung — sits close to the body line box.
#[gpui::test]
fn heading_ladder_descends_and_stays_bounded(cx: &mut gpui::TestAppContext) {
    let body = measure(cx, "alpha bravo charlie");
    let h1 = measure(cx, "# alpha");
    let h2 = measure(cx, "## alpha");
    let h3 = measure(cx, "### alpha");
    let h4 = measure(cx, "#### alpha");
    let h5 = measure(cx, "##### alpha");
    let h6 = measure(cx, "###### alpha");

    assert!(
        h1 > h2 && h2 > h3 && h3 > h4 && h4 > h5 && h5 > h6,
        "heading footprints must descend strictly: {h1} {h2} {h3} {h4} {h5} {h6}"
    );
    assert!(
        h1 <= 3.0 * body,
        "H1 must stay a bounded step above the body line: {h1} vs {body}"
    );
    assert!(
        h4 < body && h4 > 0.75 * body,
        "H4 rides the body size at the heading ladder's tighter line height,          so its single-line footprint sits just under the paragraph's: {h4} vs {body}"
    );
}

/// The reasoning tier has no size of its own: the muted foreground is the whole
/// difference from a message body, so at one body size the same source must
/// measure identically — heading ladder included. A reasoning size step, or a
/// second ladder, fails here.
#[gpui::test]
fn reasoning_tier_matches_the_message_body_footprint(cx: &mut gpui::TestAppContext) {
    let source = "alpha bravo charlie\n\n# delta echo foxtrot\n\n## golf hotel india\n\njuliet **kilo** lima `mike` november\n\n- oscar\n- papa\n- quebec";
    let message = measure_tier(cx, source, message_document);
    let reasoning = measure_tier(cx, source, reasoning_document);
    assert!(
        (message - reasoning).abs() <= 0.5,
        "the reasoning tier must measure like the message body at one size: \
         reasoning {reasoning}px vs message {message}px"
    );
}

/// A document entity fed text through `set_text` renders the same footprint as
/// the same source mounted statically — the persistent-body path used by the
/// conversation must not carry extra layout.
#[gpui::test]
fn persistent_entity_footprint_matches_static_mount(cx: &mut gpui::TestAppContext) {
    let static_height = measure(cx, "alpha bravo charlie\n\ndelta echo foxtrot");

    let out = Rc::new(Cell::new(px(0.)));
    let o2 = out.clone();
    let (_view, cx) = cx.add_window_view(move |window, cx| {
        let state = cx.new(|cx| TextViewState::markdown("alpha bravo charlie", cx));
        state.update(cx, |s, cx| {
            s.set_text("alpha bravo charlie\n\ndelta echo foxtrot", cx)
        });
        let view = TextView::new(&state)
            .style(message_style(&Theme::default()))
            .text_size(px(13.));
        let host = cx.new(|_cx| HeightHost {
            view,
            out: o2.clone(),
        });
        Root::new(host, window, cx)
    });
    let _ = cx;
    cx.run_until_parked();
    let entity_height = f32::from(out.get());
    assert!(
        (entity_height - static_height).abs() <= 1.0,
        "persistent entity {entity_height}px must match static mount {static_height}px"
    );
}

struct HeightHost {
    view: TextView,
    out: Rc<Cell<Pixels>>,
}

impl Render for HeightHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div().w(px(600.)).child(Measure {
            child: self.view.clone().into_any_element(),
            out: self.out.clone(),
        })
    }
}
