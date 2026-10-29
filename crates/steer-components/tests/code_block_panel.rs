//! The conversation-prose side of the code-surface contract, pinned: a fence
//! in a message document renders the editor panel (the `markdown-editor-code`
//! selector) — effect rendering first for the code the model writes. The
//! literal side of the contract (tool bodies stay in-band, never panels) is
//! pinned in ai-elements' `code_view_selection` test.

use gpui::{
    AppContext as _, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext, div, px,
};
use gpui_component::Root;
use steer_components::markdown::{TextViewState, message_document};

struct Host {
    doc: gpui::Entity<TextViewState>,
}

impl Render for Host {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let theme = gpui_component::Theme::global(cx).clone();
        div()
            .w(gpui::px(640.))
            .child(message_document(&self.doc, &theme, px(14.), cx))
    }
}

#[gpui::test]
async fn a_diff_fence_renders_the_editor_panel(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.add_window(|window, cx| {
        let host = cx.new(|cx| {
            let mut doc = steer_components::markdown::ChatDocument::default();
            doc.sync("```diff\n-let gone = 1;\n+let here = 2;\n```", cx);
            Host {
                doc: doc.state().expect("synced").clone(),
            }
        });
        Root::new(host, window, cx)
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    assert!(
        visual.debug_bounds("markdown-editor-code").is_some(),
        "a conversation fence renders as the editor panel"
    );
}

/// The svg block's source face is the same editor panel any fence renders.
/// The flip is a real click on the block's corner toggle; the panel's
/// selector appearing is the acceptance.
#[gpui::test]
async fn an_svg_fence_s_source_face_is_the_editor_panel(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.add_window(|window, cx| {
        let host = cx.new(|cx| {
            let mut doc = steer_components::markdown::ChatDocument::default();
            doc.sync(
                concat!(
                    "```svg\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" ",
                    "height=\"60\"><rect width=\"120\" height=\"60\" fill=\"#3b82f6\"/></svg>\n```"
                ),
                cx,
            );
            Host {
                doc: doc.state().expect("synced").clone(),
            }
        });
        Root::new(host, window, cx)
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(gpui::size(px(640.), px(360.)));
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(
        visual.debug_bounds("markdown-editor-code").is_none(),
        "the image face is the default — no panel yet"
    );

    let toggle = visual
        .debug_bounds("markdown-inline-svg-toggle")
        .expect("the flip toggle renders");
    let center = gpui::point(toggle.center().x, toggle.center().y);
    visual.simulate_mouse_down(center, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.simulate_mouse_up(center, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });

    assert!(
        visual.debug_bounds("markdown-editor-code").is_some(),
        "the flip reveals the editor panel"
    );
}
