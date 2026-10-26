//! Drag-selection regression probe for the transcript.
//!
//! The window selection engine derives its drag auto-scroll viewport from the
//! anchor participant's content mask. A row-level `overflow_hidden` wrapper
//! makes that mask the message bubble's own rect, so dragging near a short
//! bubble's edge synthesizes wheel events and pulls the list away instead of
//! extending the selection. The rows here reproduce the production body
//! wrapper (full-width, `min_w_0`, no clipping) under `FollowMode::Tail`:
//! grabbing text mid-list and dragging toward a row edge must select without
//! moving the transcript.
//!
//! The row body is the production `message_document` assembly, highlighter
//! included: the document takes its code-block highlighter from the app-wide
//! text defaults, and a per-view highlighter (a new `Arc` every frame) leaves
//! no selection at all — see `install_code_highlighter`.

use gpui::{
    AppContext as _, Context, FollowMode, InteractiveElement as _, IntoElement, ListAlignment,
    ListState, Modifiers, MouseButton, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use gpui_component::{ActiveTheme as _, Root};
use steer_components::markdown::{ChatDocument, message_document};

const ROW_TEXT: &str = "The transcript is a scroll view, not a document. \
Selecting a reply must extend the selection under the pointer; no wheel \
synthesized by the selection engine may yank the column while the drag is \
in flight. This paragraph is long enough to wrap over several lines so the \
row has real text bounds to anchor and extend a selection across.";

struct MarkdownRow {
    doc: ChatDocument,
}

impl Render for MarkdownRow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(state) = self.doc.state() else {
            return div().into_any_element();
        };
        // The production body exactly as `message.rs` mounts it (persistent
        // document, production assembly): full width, shrinkable, and
        // deliberately free of `overflow_hidden`.
        div()
            .w_full()
            .min_w_0()
            .child(message_document(state, &theme, px(14.), cx))
            .into_any_element()
    }
}

struct DragProbe {
    state: ListState,
    rows: Vec<gpui::Entity<MarkdownRow>>,
}

impl Render for DragProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        let list = gpui::list(self.state.clone(), move |ix, _window, _cx| {
            div()
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .debug_selector(move || format!("drag-row-{ix}"))
                .child(rows[ix].clone())
                .into_any_element()
        })
        .w_full()
        .h_full()
        .min_h_0()
        .min_w_0();
        // Production registers the guard from a zero-size canvas painted every
        // frame (`Workspace::render`); the probe mirrors that wiring so the
        // guard under test is the one that ships.
        div().size_full().relative().child(list).child(
            gpui::canvas(
                |_, _, _| (),
                |_, _, window, _| {
                    steer_components::markdown::install_selection_drag_guard(window);
                },
            )
            .absolute()
            .top_0()
            .left_0(),
        )
    }
}

fn draw(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
async fn dragging_transcript_text_does_not_scroll_the_list(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    cx.update(steer_components::markdown::install_code_highlighter);
    let state = ListState::new(6, ListAlignment::Bottom, px(2048.));
    let probe_state = state.clone();
    let window = cx.add_window(|window, cx| {
        let rows = (0..6)
            .map(|_| {
                cx.new(|cx| {
                    let mut doc = ChatDocument::default();
                    doc.sync(ROW_TEXT, cx);
                    MarkdownRow { doc }
                })
            })
            .collect::<Vec<_>>();
        let probe = cx.new(|_| DragProbe {
            state: probe_state,
            rows,
        });
        Root::new(probe, window, cx)
    });
    state.set_follow_mode(FollowMode::Tail);

    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(640.), px(360.)));
    draw(&mut visual);

    let tail = state.logical_scroll_top();
    assert!(
        state.is_following_tail(),
        "the probe starts pinned to the tail"
    );

    // Anchor in the tail row's text, then drag up toward its top edge. That
    // edge sits well inside the list viewport: a row-level content mask would
    // read it as the auto-scroll trigger, the list viewport must not.
    let row = visual
        .debug_bounds("drag-row-5")
        .expect("the tail row is on screen");
    let start = gpui::point(row.left() + px(48.), row.top() + row.size.height / 2.);
    let end = gpui::point(row.left() + px(48.), row.top() + px(4.));
    assert!(
        row.top() > px(16.),
        "the row sits inside the list viewport, clear of its dead zone"
    );

    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    draw(&mut visual);
    for step in 1..=6 {
        let y = start.y + (end.y - start.y) * (step as f32 / 6.);
        visual.simulate_mouse_move(
            gpui::point(start.x, y),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        // A painted frame gives the selection its copyable document state.
        draw(&mut visual);
    }
    // Let any synthesized auto-scroll tick land before releasing.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    visual.run_until_parked();
    let dragged = state.logical_scroll_top();

    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    draw(&mut visual);

    let selected = visual.update(gpui_base::TextSelection::selected_text);
    assert!(
        !selected.is_empty(),
        "the drag produced a text selection (got {selected:?})"
    );
    assert_eq!(
        (dragged.item_ix, dragged.offset_in_item),
        (tail.item_ix, tail.offset_in_item),
        "dragging inside the transcript must not scroll the list"
    );
    let released = state.logical_scroll_top();
    assert_eq!(
        (released.item_ix, released.offset_in_item),
        (tail.item_ix, tail.offset_in_item),
        "the release must not leave the list moved either"
    );
}

/// A popup menu's click stops propagation, so the selection engine never sees
/// the `mouse_up` that would end the drag its `mouse_down` armed — and its move
/// handler extends the selection without checking whether a button is still
/// held. The guard must end the stale gesture on the first button-less move
/// instead of letting the pointer's drift select whatever it crosses, while a
/// real drag (button held) keeps working.
#[gpui::test]
async fn a_swallowed_mouse_up_cannot_select_on_buttonless_moves(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    cx.update(steer_components::markdown::install_code_highlighter);
    let state = ListState::new(6, ListAlignment::Bottom, px(2048.));
    let probe_state = state.clone();
    let window = cx.add_window(|window, cx| {
        let rows = (0..6)
            .map(|_| {
                cx.new(|cx| {
                    let mut doc = ChatDocument::default();
                    doc.sync(ROW_TEXT, cx);
                    MarkdownRow { doc }
                })
            })
            .collect::<Vec<_>>();
        let probe = cx.new(|_| DragProbe {
            state: probe_state,
            rows,
        });
        Root::new(probe, window, cx)
    });
    state.set_follow_mode(FollowMode::Tail);

    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(640.), px(360.)));
    draw(&mut visual);

    let row = visual
        .debug_bounds("drag-row-5")
        .expect("the tail row is on screen");
    let start = gpui::point(row.left() + px(48.), row.top() + row.size.height / 2.);

    // The press that opens a popup arms the gesture; the popup's click handler
    // swallows the matching release.
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    draw(&mut visual);
    for step in 1..=6 {
        let p = gpui::point(
            start.x + px(40. * step as f32),
            start.y - px(8. * step as f32),
        );
        visual.simulate_mouse_move(p, None, Modifiers::default());
        draw(&mut visual);
    }
    let selected = visual.update(gpui_base::TextSelection::selected_text);
    assert!(
        selected.is_empty(),
        "a button-less drift must not select text (got {selected:?})"
    );

    // A real drag holds the button: the guard must not eat it.
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    draw(&mut visual);
    let end = gpui::point(start.x + px(240.), start.y);
    for step in 1..=4 {
        let t = step as f32 / 4.;
        let p = gpui::point(start.x + (end.x - start.x) * t, start.y);
        visual.simulate_mouse_move(p, Some(MouseButton::Left), Modifiers::default());
        draw(&mut visual);
    }
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    draw(&mut visual);
    let selected = visual.update(gpui_base::TextSelection::selected_text);
    assert!(
        !selected.is_empty(),
        "a real drag still selects (got {selected:?})"
    );
}
