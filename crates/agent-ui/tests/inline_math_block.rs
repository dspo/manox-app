//! A `$$…$$` display equation in an assistant message paints as its formula
//! through the production row path, and a formula that does not typeset stays
//! the literal text the unclaimed math path renders. `markdown-inline-math`
//! is the selector the rendered block carries
//! (`steer_components::markdown::math_block`).

use agent_ui::conversation::{ApplyCtx, ConversationState};
use agent_ui::views::message::MessageItem;
use gpui::{
    AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use manox_agent::{MessageAuthor, ThreadEvent};

const MODEL: &str = "bigmodel-anthropic/glm-5.3";

struct RowProbe {
    row: Entity<MessageItem>,
}

impl Render for RowProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().child(self.row.clone())
    }
}

fn paint_message(cx: &mut TestAppContext, body: &str) -> VisualTestContext {
    cx.update(gpui_component::init);
    let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
    let ctx = ApplyCtx {
        host: steer_agent_chat_ui::host::noop_host(),
        cwd: None,
        fork_source: None,
    };
    cx.update(|cx| {
        conversation.update(cx, |c, cx| {
            let _ = c.apply(
                &ThreadEvent::AgentText(body.to_string()),
                MODEL,
                None,
                ctx,
                cx,
            );
        });
    });
    let rows = cx.update(|cx| conversation.read(cx).items().to_vec());
    assert_eq!(rows.len(), 1, "one assistant bubble");

    let window = cx.open_window(size(px(760.), px(600.)), move |_, _| RowProbe {
        row: rows[0].clone(),
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    for _ in 0..2 {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
    visual
}

#[gpui::test]
async fn a_display_equation_paints_as_its_formula(cx: &mut TestAppContext) {
    let mut visual = paint_message(
        cx,
        "The identity:\n\n$$\nx^2 + y^2 = z^2\n$$\n\nThat is the whole proof.",
    );

    let block = visual
        .debug_bounds("markdown-inline-math")
        .expect("the equation paints as a math block");
    assert!(
        block.size.height > px(8.) && block.size.width > px(40.),
        "the formula paints at formula size: {block:?}"
    );
}

#[gpui::test]
async fn an_untypeset_equation_stays_literal(cx: &mut TestAppContext) {
    let mut visual = paint_message(
        cx,
        "Broken:\n\n$$\n\\frac{not a formula\n$$\n\nNothing to draw there.",
    );

    assert!(
        visual.debug_bounds("markdown-inline-math").is_none(),
        "an untypeset equation never becomes an image"
    );
    let body = visual
        .debug_bounds("message-item-body-0")
        .expect("the body still paints as text");
    assert!(
        body.size.height > px(8.),
        "the body has real height: {body:?}"
    );
}
