//! List items must stack as pure line boxes — no marker-wrap may inflate an
//! item by an invisible extra line, and no per-item margin may creep in (the
//! upstream renderer spaces list items tighter than paragraph gaps; the guard
//! pins that density). The heading ladder must hold its configured sizes under
//! the message-density style: H2–H6 at body size (plus the renderer's constant
//! heading pad), H1 one step up. The document tiers are density-equal: the
//! reasoning tier differs from the message body by color alone, so at one body
//! size the two must measure the same.

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

/// Which document constructor a measurement mounts. Both tiers render a source
/// through the same machinery, so equal footprints at one body size is the
/// invariant: a tier may not carry a size (or a heading ladder) of its own.
#[derive(Clone, Copy)]
enum Tier {
    Message,
    Reasoning,
}

struct Host {
    state: Entity<TextViewState>,
    body: Option<Pixels>,
    tier: Tier,
    out: Rc<Cell<Pixels>>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = Theme::default();
        let text_size = self.body.unwrap_or(px(13.));
        let view = match self.tier {
            Tier::Message => message_document(&self.state, &theme, text_size),
            Tier::Reasoning => reasoning_document(&self.state, &theme, text_size),
        };
        div().w(px(600.)).child(Measure {
            child: view.into_any_element(),
            out: self.out.clone(),
        })
    }
}

fn measure_raw(
    cx: &mut gpui::TestAppContext,
    source: &str,
    body: Option<Pixels>,
    tier: Tier,
) -> f32 {
    cx.update(gpui_base::init);
    let out = Rc::new(Cell::new(px(0.)));
    let src = source.to_string();
    let o2 = out.clone();
    let (_view, cx) = cx.add_window_view(move |_window, cx| {
        let state = cx.new(|cx| TextViewState::markdown(&src, cx));
        Host {
            state,
            body,
            tier,
            out: o2,
        }
    });
    cx.run_until_parked();
    f32::from(out.get())
}

fn measure(cx: &mut gpui::TestAppContext, source: &str) -> f32 {
    measure_raw(cx, source, Some(px(13.)), Tier::Message)
}

fn measure_reasoning(cx: &mut gpui::TestAppContext, source: &str) -> f32 {
    measure_raw(cx, source, Some(px(13.)), Tier::Reasoning)
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
        (one_list - one_par).abs() <= 1.0,
        "single-item list {one_list}px must match single paragraph {one_par}px"
    );
    // Upstream list items carry no per-item margin: N items stack as N line
    // boxes. A marker wrap or stray item margin would break the equality.
    let line = one_par;
    assert!(
        (twelve_tight - 12.0 * line).abs() <= 1.0,
        "12-item list {twelve_tight}px must be 12 bare line boxes ({line}px each)"
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

/// Heading ladder under the message-density style: H2–H6 hold the body line
/// box; H1 steps up. Every heading is at most one step above the body, so no
/// heading dwarfs dense conversation text.
#[gpui::test]
fn heading_ladder_stays_at_body_size_with_one_h1_step(cx: &mut gpui::TestAppContext) {
    let body = measure(cx, "alpha bravo charlie");
    let h1 = measure(cx, "# alpha");
    let h2 = measure(cx, "## alpha");
    let h4 = measure(cx, "#### alpha");

    // Every heading carries the renderer's constant bottom pad, so H2/H4 read
    // as the body line box plus that one constant — never more.
    assert!(
        (h2 - h4).abs() <= 0.5,
        "H2 and H4 share the body size, so their line boxes must match: {h2} vs {h4}"
    );
    let (body_f, h2_f) = (body, h2);
    assert!(
        h2_f > body_f && h2_f - body_f <= 8.0,
        "H2 must be the body line plus the constant heading pad: {h2} vs {body}"
    );
    let h1_f = h1;
    assert!(
        h1_f > h2_f && h1_f - h2_f <= 10.0,
        "H1 must sit one visible step above H2, got {h1} vs {h2}"
    );
}

/// The reasoning tier has no size of its own: the muted foreground is the whole
/// difference from a message body, so at one body size the same source must
/// measure identically — heading ladder included. A reasoning size step, or a
/// second ladder, fails here.
#[gpui::test]
fn reasoning_tier_matches_the_message_body_footprint(cx: &mut gpui::TestAppContext) {
    let source = "alpha bravo charlie\n\n# delta echo foxtrot\n\n## golf hotel india\n\njuliet **kilo** lima `mike` november\n\n- oscar\n- papa\n- quebec";
    let message = measure(cx, source);
    let reasoning = measure_reasoning(cx, source);
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
            .style(message_style(&Theme::default(), px(13.)))
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
