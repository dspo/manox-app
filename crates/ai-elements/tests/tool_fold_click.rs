//! The fold click, pinned at the component level: a settled card auto-folds
//! after a second, and clicking the folded header re-opens it — the fold
//! affordance the transcript's tool bodies live behind.

use std::time::Duration;

use ai_elements::{Tool, ToolState, ToolStatus};
use gpui::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use gpui_component::Root;

struct Host {
    state: gpui::Entity<ToolState>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(640.))
            .child(Tool::new("probe-card", &self.state))
    }
}

#[gpui::test]
async fn clicking_a_folded_card_header_re_opens_it(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let state = cx.new(|cx| ToolState::new(cx).with_status(ToolStatus::Succeeded, cx));
    state.update(cx, |state, cx| {
        state.set_duration(Some(Duration::from_millis(120)), cx);
    });
    let window = cx.add_window(|window, cx| {
        let host = cx.new(|_| Host {
            state: state.clone(),
        });
        Root::new(host, window, cx)
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.simulate_resize(size(px(640.), px(400.)));
    visual.run_until_parked();

    // Past the auto-fold delay the settled card folds itself.
    cx.executor().advance_clock(Duration::from_millis(1200));
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let folded = cx.update(|cx| !state.read(cx).is_open());
    assert!(folded, "the settled card has folded by now");

    let header = visual
        .debug_bounds("probe-card-header")
        .expect("the folded header paints");
    visual.simulate_click(header.center(), gpui::Modifiers::default());
    visual.run_until_parked();

    let reopened = cx.update(|cx| state.read(cx).is_open());
    assert!(reopened, "clicking the folded header re-opens the card");
}
