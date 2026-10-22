//! An ```mermaid fence in an assistant message paints as its diagram in the
//! conversation, through the production row path: `ConversationState::apply`
//! feeds the text, `MessageItem` mounts the body, and the fence renders at
//! parse time and paints at the size the diagram's own aspect asks for. The
//! control case pins the other half — a fence that renders nothing stays the
//! code block it already is.
//!
//! `markdown-inline-mermaid` is the selector the rendered block carries
//! (`steer_components::markdown::svg_block`, shared with the svg fence).

use agent_ui::conversation::{ApplyCtx, ConversationState};
use agent_ui::views::message::MessageItem;
use gpui::{
    AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use manox_agent::{MessageAuthor, ThreadEvent};

const MODEL: &str = "bigmodel-anthropic/glm-5.3";

/// A flowchart two ranks wide and one tall — an image a code block would
/// never paint at.
const FLOWCHART: &str =
    "flowchart LR\n    A[Input] --> B{Gate}\n    B -->|yes| C[Done]\n    B -->|no| D[Retry]";

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
    draw(&mut visual);
    visual
}

fn draw(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    for _ in 0..2 {
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
}

#[gpui::test]
async fn a_mermaid_fence_paints_as_an_image(cx: &mut TestAppContext) {
    let mut visual = paint_message(
        cx,
        &format!("Here is the flow:\n\n```mermaid\n{FLOWCHART}\n```\n\nThat is the whole picture."),
    );

    let block = visual
        .debug_bounds("markdown-inline-mermaid")
        .expect("the fence paints as an image block");
    assert!(
        block.size.height > px(100.) && block.size.width > px(100.),
        "the block paints at image size: {block:?}"
    );
}

/// A mermaid body that renders nothing must not claim image space: it stays
/// the code block the parser built (the body still paints).
#[gpui::test]
async fn an_unrenderable_mermaid_fence_stays_a_code_block(cx: &mut TestAppContext) {
    let mut visual = paint_message(
        cx,
        "An unknown kind:\n\n```mermaid\nnotadiagram\n    A --> B\n```\n\nNothing to draw there.",
    );

    assert!(
        visual.debug_bounds("markdown-inline-mermaid").is_none(),
        "an unrenderable fence never becomes an image"
    );
    let body = visual
        .debug_bounds("message-item-body-0")
        .expect("the body still paints as a code block");
    assert!(
        body.size.height > px(8.),
        "the body has real height: {body:?}"
    );
}

/// The shared face flip: the mermaid block's hover control flips it to the
/// fence's own code and back, under its own selectors.
#[gpui::test]
async fn the_block_flips_between_its_image_and_its_code(cx: &mut TestAppContext) {
    let mut visual = paint_message(cx, &format!("The flow:\n\n```mermaid\n{FLOWCHART}\n```\n"));

    assert!(visual.debug_bounds("markdown-inline-mermaid").is_some());
    let toggle = visual
        .debug_bounds("markdown-inline-mermaid-toggle")
        .expect("the hover control paints over the image");
    visual.simulate_click(toggle.center(), gpui::Modifiers::default());
    draw(&mut visual);

    assert!(
        visual
            .debug_bounds("markdown-inline-mermaid-code")
            .is_some(),
        "the flip shows the fence as code"
    );
    assert!(
        visual.debug_bounds("markdown-inline-mermaid").is_none(),
        "and the image is gone until the next flip"
    );
    let code = visual
        .debug_bounds("markdown-inline-mermaid-code")
        .expect("the code face has bounds");
    assert!(
        code.size.height > px(8.) && code.size.width > px(100.),
        "the code face has real size: {code:?}"
    );

    let toggle = visual
        .debug_bounds("markdown-inline-mermaid-toggle")
        .expect("the toggle stays put");
    visual.simulate_click(toggle.center(), gpui::Modifiers::default());
    draw(&mut visual);

    assert!(
        visual.debug_bounds("markdown-inline-mermaid").is_some(),
        "the diagram comes back"
    );
    assert!(
        visual
            .debug_bounds("markdown-inline-mermaid-code")
            .is_none(),
        "and the source folds away again"
    );
}

/// A model writes the fence delta by delta. Mermaid source parses at almost
/// every line boundary, so the block must stay the code the reader is watching
/// until the closing delimiter lands — then the diagram appears once.
#[gpui::test]
async fn a_streamed_mermaid_fence_paints_when_it_closes(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
    let ctx = ApplyCtx {
        host: steer_agent_chat_ui::host::noop_host(),
        cwd: None,
        fork_source: None,
    };
    let apply = |cx: &mut gpui::App, delta: &str| {
        conversation.update(cx, |c, cx| {
            let _ = c.apply(
                &ThreadEvent::AgentText(delta.to_string()),
                MODEL,
                None,
                ctx.clone(),
                cx,
            );
        });
    };

    cx.update(|cx| {
        apply(
            cx,
            "Here is the flow:\n\n```mermaid\nflowchart LR\n    A[Input] --> B{Gate}\n    B -->|yes| C[Done]",
        );
    });
    let rows = cx.update(|cx| conversation.read(cx).items().to_vec());
    let window = cx.open_window(size(px(760.), px(600.)), move |_, _| RowProbe {
        row: rows[0].clone(),
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    draw(&mut visual);

    assert!(
        visual.debug_bounds("markdown-inline-mermaid").is_none(),
        "an open fence stays code, never a half-drawn diagram"
    );
    assert!(
        visual.debug_bounds("message-item-body-0").is_some(),
        "the body paints while the fence streams"
    );

    visual.update(|_window, cx| apply(cx, "\n```\n\nThat is it."));
    draw(&mut visual);

    let block = visual
        .debug_bounds("markdown-inline-mermaid")
        .expect("the closed fence paints as an image block");
    assert!(
        block.size.height > px(100.) && block.size.width > px(100.),
        "the diagram paints at image size: {block:?}"
    );
}
