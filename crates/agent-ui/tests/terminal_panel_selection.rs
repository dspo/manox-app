//! Drag-selection regression probe for the terminal-panel body.
//!
//! The panel composes its output into one `ansi` code block and colors it
//! through a code-block highlighter. gpui-base compares that highlighter by
//! `Arc` identity and treats a new one as a selection-preserving reflow; a
//! highlighter rebuilt per render therefore leaves the body unselectable.
//! `TerminalPanel` caches it on the styled runs, and this probe pins that: the
//! body selects like every other transcript surface.

use gpui::{
    AppContext as _, Context, Entity, FollowMode, InteractiveElement as _, IntoElement,
    ListAlignment, ListState, Modifiers, MouseButton, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use gpui_component::{ActiveTheme as _, Root};
use steer_components::markdown::{PanelKind, TerminalPanel};

const TEXT: &str = "line one of the tool output that is long enough to wrap across several \
words and lines so the panel has real text bounds\na second line of output\nthird line";

struct Row {
    panel: Entity<TerminalPanel>,
}

impl Render for Row {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().w_full().min_w_0().child(self.panel.clone())
    }
}

struct Probe {
    state: ListState,
    rows: Vec<Entity<Row>>,
}

impl Render for Probe {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        gpui::list(self.state.clone(), move |ix, _w, _cx| {
            div()
                .w_full()
                .min_w_0()
                .debug_selector(move || format!("row-{ix}"))
                .child(rows[ix].clone())
                .into_any_element()
        })
        .w_full()
        .h_full()
        .min_h_0()
    }
}

fn draw(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
async fn panel_body_is_drag_selectable(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    cx.update(steer_components::markdown::install_code_highlighter);
    let list_state = ListState::new(6, ListAlignment::Bottom, px(2048.));
    let probe_state = list_state.clone();
    let window = cx.add_window(|window, cx| {
        let rows = (0..6)
            .map(|_| {
                cx.new(|cx| {
                    let theme = cx.theme().clone();
                    let panel = cx.new(|cx| {
                        let mut p = TerminalPanel::new(PanelKind::Plain, None, None, &theme);
                        p.set_output(TEXT.to_string(), cx);
                        p
                    });
                    Row { panel }
                })
            })
            .collect::<Vec<_>>();
        let probe = cx.new(|_| Probe {
            state: probe_state,
            rows,
        });
        Root::new(probe, window, cx)
    });
    list_state.set_follow_mode(FollowMode::Tail);

    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(640.), px(360.)));
    draw(&mut visual);

    let row = visual.debug_bounds("row-5").expect("row");
    let start = gpui::point(row.left() + px(24.), row.top() + row.size.height / 2.);
    let end = gpui::point(row.left() + px(300.), row.top() + px(6.));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    draw(&mut visual);
    for step in 1..=6 {
        let t = step as f32 / 6.;
        let p = gpui::point(
            start.x + (end.x - start.x) * t,
            start.y + (end.y - start.y) * t,
        );
        visual.simulate_mouse_move(p, Some(MouseButton::Left), Modifiers::default());
        draw(&mut visual);
    }
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    draw(&mut visual);

    let sel = visual.update(gpui_base::TextSelection::selected_text);
    eprintln!("PANEL selected len={} text={:?}", sel.len(), sel);
    assert!(!sel.trim().is_empty(), "panel body produced no selection");
}
