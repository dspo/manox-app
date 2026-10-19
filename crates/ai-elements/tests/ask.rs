//! The ask card's pager rule (`ai_elements::AskState`): the forward edge turns
//! pages and, on the last one, IS the submit. The card's check, a host's own
//! submit hook (`AskUserQuestion::on_submit`) and a host's keyboard binding
//! ([`AskState::forward`]) all ride this one rule, so it is pinned here rather
//! than at each caller.

use ai_elements::AskState;
use gpui::{AppContext as _, Entity, TestAppContext};

fn state(cx: &mut TestAppContext, questions: usize) -> Entity<AskState> {
    cx.update(gpui_component::init);
    cx.update(|cx| cx.new(|cx| AskState::new(vec![false; questions], "note", cx)))
}

/// One question: its forward edge is already the submit.
#[gpui::test]
fn a_single_question_opens_on_the_submit_edge(cx: &mut TestAppContext) {
    let ask = state(cx, 1);
    assert!(
        cx.update(|cx| ask.update(cx, |state, cx| state.forward(cx))),
        "the only page's forward edge is the submit"
    );
    assert!(!ask.read_with(cx, |state, _| state.submitted()));
}

/// Three questions: two forward turns, then the submit edge — and the turn
/// alone never settles the card (a host that owns the answer still holds it).
#[gpui::test]
fn forward_turns_pages_and_reports_the_submit_edge(cx: &mut TestAppContext) {
    let ask = state(cx, 3);
    for expected in [1, 2] {
        assert!(
            !cx.update(|cx| ask.update(cx, |state, cx| state.forward(cx))),
            "mid-form the forward edge only turns the page"
        );
        assert_eq!(ask.read_with(cx, |state, _| state.page()), expected);
    }
    assert!(
        cx.update(|cx| ask.update(cx, |state, cx| state.forward(cx))),
        "the last page's forward edge reports the submit"
    );
    assert_eq!(ask.read_with(cx, |state, _| state.page()), 2, "still there");
    assert!(
        !ask.read_with(cx, |state, _| state.submitted()),
        "reporting the edge leaves the card a working form"
    );
}

/// `advance` is the same edge plus the card's own settle, and `go_back` clamps
/// at the first page.
#[gpui::test]
fn advance_settles_and_back_clamps(cx: &mut TestAppContext) {
    let ask = state(cx, 2);
    cx.update(|cx| ask.update(cx, |state, cx| state.go_back(cx)));
    assert_eq!(ask.read_with(cx, |state, _| state.page()), 0);
    cx.update(|cx| ask.update(cx, |state, cx| state.advance(cx)));
    assert_eq!(ask.read_with(cx, |state, _| state.page()), 1);
    assert!(!ask.read_with(cx, |state, _| state.submitted()));
    cx.update(|cx| ask.update(cx, |state, cx| state.advance(cx)));
    assert!(
        ask.read_with(cx, |state, _| state.submitted()),
        "advance past the last page settles the card"
    );
}
