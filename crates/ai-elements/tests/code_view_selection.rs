//! The tool bodies' in-band contract, pinned: what you see is what a
//! drag-select copies — gutter numbers included. This is the property that
//! keeps the tool surfaces literal (a headered panel's custom block cannot
//! join the window selection, so a working select-all here proves the body
//! never became one). Highlighting rides the same document; the assertion
//! only checks the selectable text.

use ai_elements::{CodeLine, CodeView};
use gpui::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use gpui_component::Root;

struct Host {
    lines: Vec<CodeLine>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(CodeView::new("probe", self.lines.clone()))
    }
}

#[gpui::test]
async fn selecting_a_code_view_copies_what_you_see(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let text = "fn main() {\n    println!(\"hi\");\n}\n";
    let window = cx.add_window(|window, cx| {
        let host = cx.new(|_| Host {
            lines: text
                .lines()
                .enumerate()
                .map(|(ix, line)| CodeLine::plain(ix as u32 + 1, line))
                .collect(),
        });
        Root::new(host, window, cx)
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(640.), px(360.)));
    visual.run_until_parked();
    // A painted frame registers the body's text as a selection participant.
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    // A real drag across the body — the gesture a reader makes to copy code.
    let body = visual
        .debug_bounds("code-view-body")
        .expect("the body renders");
    let start = gpui::point(body.left() + px(30.), body.top() + px(10.));
    let end = gpui::point(body.left() + px(120.), body.top() + px(50.));
    visual.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
    for step in 1..=4 {
        let t = step as f32 / 4.;
        visual.simulate_mouse_move(
            gpui::point(
                start.x + (end.x - start.x) * t,
                start.y + (end.y - start.y) * t,
            ),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    visual.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let selected = visual.update(gpui_base::TextSelection::selected_text);

    assert!(
        selected.contains("fn main()"),
        "the code itself is selectable (got {selected:?})"
    );
    assert!(
        selected.contains('\n'),
        "the whole window selects across lines (got {selected:?})"
    );
    assert!(
        selected.chars().any(|c| c.is_ascii_digit()),
        "the gutter numbers are part of the document — what you copy is what \
         you see (got {selected:?})"
    );
}
