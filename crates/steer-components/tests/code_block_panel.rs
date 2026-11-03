//! The conversation-prose side of the code-surface contract, pinned: a fence
//! in a message document renders the editor panel (the `markdown-editor-code`
//! selector) — effect rendering first for the code the model writes. The
//! literal side of the contract (tool bodies stay in-band, never panels) is
//! pinned in ai-elements' `code_view_selection` test.

use gpui::{
    AppContext as _, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext, div, px,
};
use gpui_component::Root;
use steer_components::markdown::{ChatDocument, message_document};

struct Host {
    doc: ChatDocument,
}

impl Host {
    /// A host over one synced document.
    fn synced(text: &str, cx: &mut gpui::Context<Self>) -> Self {
        let mut doc = ChatDocument::default();
        doc.sync(text, cx);
        Self { doc }
    }
}

impl Render for Host {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let theme = gpui_component::Theme::global(cx).clone();
        let state = self.doc.state().expect("synced").clone();
        div()
            .w(px(640.))
            .child(message_document(&state, &theme, px(14.), cx))
    }
}

#[gpui::test]
async fn a_diff_fence_renders_the_split_face(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.add_window(|window, cx| {
        let host = cx.new(|cx| {
            Host::synced(
                "```diff\n@@ -1,2 +1,2 @@\n-let gone = 1;\n+let here = 2;\n```",
                cx,
            )
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
    assert!(
        visual.debug_bounds("markdown-editor-split").is_some(),
        "a well-formed diff fence opens the split face"
    );
    assert!(
        visual.debug_bounds("markdown-editor-merge").is_none(),
        "the merge face belongs to conflict fences, not diffs"
    );
}

/// The header's face toggle flips split to flat and back — the round trip
/// the reader uses when the two columns feel too wide.
#[gpui::test]
async fn the_diff_face_toggle_round_trips(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.add_window(|window, cx| {
        let host = cx.new(|cx| {
            Host::synced(
                "```diff\n@@ -1,2 +1,2 @@\n-let gone = 1;\n+let here = 2;\n```",
                cx,
            )
        });
        Root::new(host, window, cx)
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(visual.debug_bounds("markdown-editor-split").is_some());

    let toggle = visual
        .debug_bounds("markdown-editor-code-view-toggle")
        .expect("the face toggle paints on shaped panels");
    let center = gpui::point(toggle.center().x, toggle.center().y);
    visual.simulate_click(center, gpui::Modifiers::default());
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(
        visual.debug_bounds("markdown-editor-split").is_none(),
        "the toggle returns the panel to the flat face"
    );
}

/// A streaming diff fence: the fence mounts mid-write — no hunk header yet,
/// the shape still `Code` — and completes in place, riding the incremental
/// append path. The panel must open the split face on the frame the parse
/// completes, without a remount: the default face re-derives from the shape
/// while the reader's choice is still `None` (the toggle's explicit choice
/// never drifts — pinned by [`the_diff_face_toggle_round_trips`]).
#[gpui::test]
async fn a_streaming_diff_fence_opens_the_split_face_when_it_completes(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    // The language token lands first; the body streams in later — the shape
    // a live diff fence really mounts with.
    let host = cx.update(|cx| cx.new(|cx| Host::synced("```diff\n", cx)));
    let window = cx.add_window(|window, cx| Root::new(host.clone(), window, cx));
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(
        visual.debug_bounds("markdown-editor-code").is_some(),
        "the mid-write fence renders as the flat editor panel"
    );
    assert!(
        visual.debug_bounds("markdown-editor-split").is_none(),
        "the hunk-less shape has no second face yet"
    );

    // The rest of the fence lands: hunk header, body, closing delimiter.
    let completed = "```diff\n@@ -1,2 +1,2 @@\n-let gone = 1;\n+let here = 2;\n```\n";
    cx.update(|cx| {
        host.update(cx, |host, cx| host.doc.sync(completed, cx));
    });
    for _ in 0..4 {
        cx.executor().run_until_parked();
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }
    assert!(
        visual.debug_bounds("markdown-editor-split").is_some(),
        "the completed patch opens its split face without a remount"
    );
}

/// The svg block's source face is the same editor panel any fence renders.
/// The flip is a real click on the block's header toggle (on the source
/// face it rides the panel's header, clear of the copy control); the
/// panel's selector appearing is the acceptance.
#[gpui::test]
async fn an_svg_fence_s_source_face_is_the_editor_panel(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.add_window(|window, cx| {
        let host = cx.new(|cx| {
            Host::synced(
                concat!(
                    "```svg\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" ",
                    "height=\"60\"><rect width=\"120\" height=\"60\" fill=\"#3b82f6\"/></svg>\n```"
                ),
                cx,
            )
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

    // On the source face the flip-back lives in the panel's header row,
    // clear of the copy control it used to cover.
    let copy = visual
        .debug_bounds("markdown-editor-code-copy")
        .expect("the source face carries a copy control");
    let flip = visual
        .debug_bounds("markdown-inline-svg-toggle")
        .expect("the flip-back renders");
    assert!(
        flip.right() <= copy.left() + px(2.),
        "the flip control must not sit on the copy control \
         (flip right {:?}, copy left {:?})",
        flip.right(),
        copy.left()
    );
}
