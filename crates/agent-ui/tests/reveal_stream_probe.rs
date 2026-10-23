//! Reproduction probe shaped like the production crash site: a streaming
//! markdown TextView inside a Collapsible reveal (MotionReveal) whose body
//! carries the unfold fade AnimationElement — the reasoning/tool-card body
//! shape. Animation frames are pumped explicitly (no platform frame loop in
//! tests).

use gpui::{
    AnimationExt as _, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use gpui_component::collapsible::Collapsible;
use steer_components::markdown::{TextView, TextViewState, stream_fade_motion};

struct Shell {
    state: Entity<TextViewState>,
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let body = div()
            .w_full()
            .child(TextView::new(&self.state).motion(stream_fade_motion()));
        // The reasoning/tool-card shape: animated body inside a reveal.
        let animated = body.with_animation(
            "fade",
            gpui::Animation::new(std::time::Duration::from_millis(150))
                .with_easing(gpui::ease_out_quint()),
            |body, t| body.opacity(t),
        );
        Collapsible::new()
            .open(true)
            .motion_id("reveal")
            .child(div().h(px(12.)).w_full())
            .content(animated)
    }
}

fn pump(visual: &mut VisualTestContext, frames: usize) {
    for _ in 0..frames {
        visual.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
}

#[gpui::test]
async fn reveal_animation_over_streaming_cjk(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let state = cx.update(|cx| cx.new(|cx| TextViewState::markdown("", cx)));
    let window = cx.open_window(size(px(760.), px(600.)), {
        let state = state.clone();
        move |_, _| Shell { state }
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    // Stream CJK markdown while the reveal + fade animations run.
    let full = "这是推理正文：先看**加粗中文**，再看`内联代码`，然后[一个链接](https://example.com)。\
                继续流入更多中文，混排 English words，带全角标点：顿号、句号。\
                还有 emoji 🎉 与删除线~~旧结论~~。再补一段让文档换行流动的内容。";
    let mut pos = 0;
    let mut toggle = 0;
    while pos < full.len() {
        let mut end = (pos + 5).min(full.len());
        while end < full.len() && !full.is_char_boundary(end) {
            end += 1;
        }
        let delta = full[pos..end].to_string();
        state.update(cx, |s, cx| s.push_str(&delta, cx));
        pos = end;
        pump(&mut visual, 3);
        // Re-trigger the unfold animation mid-stream every so often, the way
        // a user toggling a reasoning round during streaming would.
        toggle += 1;
        if toggle % 4 == 0 {
            pump(&mut visual, 6);
        }
    }
    pump(&mut visual, 30);
}
