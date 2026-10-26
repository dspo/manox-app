//! An ```svg fence in an assistant message paints as its diagram in the
//! conversation, through the production row path: `ConversationState::apply`
//! feeds the text, `MessageItem` mounts the body, and the fence's block
//! paints at the size the diagram's own aspect asks for. The control case
//! pins the other half of the rule — a fence the rasterizer rejects stays the
//! code block it already is (the body still paints; nothing claims image
//! size).
//!
//! `markdown-inline-svg` is the selector the rendered block carries
//! (`steer_components::markdown::svg_block`).

use agent_ui::conversation::ConversationState;
use agent_ui::views::message::MessageItem;
use gpui::{
    AppContext as _, Entity, IntoElement, Modifiers, ParentElement as _, Pixels, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use manox_agent::MessageAuthor;

const MODEL: &str = "bigmodel-anthropic/glm-5.3";

/// 400x200 — a diagram whose raster paints far taller than a code line.
const DIAGRAM: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200"><rect width="400" height="200" fill="#3b82f6"/></svg>"##;

/// 100x300 — taller than the display cap, so the block has to stop growing.
const TALL: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="300"><rect width="100" height="300" fill="#f59e0b"/></svg>"##;

struct RowProbe {
    row: Entity<MessageItem>,
}

impl Render for RowProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().child(self.row.clone())
    }
}

/// Paint the single assistant row produced by `body` and hand back the
/// painted window, so the caller can read the selectors the frame carries.
fn paint_message(cx: &mut TestAppContext, body: &str) -> VisualTestContext {
    cx.update(gpui_component::init);
    let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
    cx.update(|cx| {
        let mut fold = Fold::new();
        fold.start_turn();
        fold.delta("p-1", body);
        project_fold(cx, &conversation, &mut fold);
    });
    paint_window(cx, &conversation)
}

/// Mount the conversation's rows in the probe window and settle the paint.
fn paint_window(
    cx: &mut TestAppContext,
    conversation: &gpui::Entity<ConversationState>,
) -> VisualTestContext {
    // The projection renders the turn's user row too; the probe paints the
    // assistant bubble.
    let rows = cx.update(|cx| {
        conversation
            .read(cx)
            .items()
            .iter()
            .filter(|item| {
                matches!(
                    item.read(cx).kind(),
                    agent_ui::conversation::ConvItem::Assistant { .. }
                )
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    let window = cx.open_window(size(px(760.), px(600.)), move |_, _| RowProbe {
        row: rows[0].clone(),
    });
    cx.run_until_parked();
    let visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual
}

/// Two frames of the painted row: the first lays out, the second paints what
/// the first produced.
fn draw(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    for _ in 0..2 {
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
}

/// The painted block's own aspect, which is the raster's: the block must not
/// frame the diagram in space its aspect does not ask for.
fn painted_aspect(block: gpui::Bounds<Pixels>) -> f32 {
    block.size.width.as_f32() / block.size.height.as_f32()
}

#[gpui::test]
async fn an_svg_fence_paints_as_an_image(cx: &mut TestAppContext) {
    let mut visual = paint_message(
        cx,
        &format!("Here is the flow:\n\n```svg\n{DIAGRAM}\n```\n\nThat is the whole picture."),
    );

    let block = visual
        .debug_bounds("markdown-inline-svg")
        .expect("the fence paints as an image block");
    assert!(
        block.size.height > px(100.) && block.size.width > px(100.),
        "the block paints at image size: {block:?}"
    );
    assert!(
        (painted_aspect(block) - 2.0).abs() < 0.01,
        "the block carries the diagram's 2:1 aspect: {block:?}"
    );
}

#[gpui::test]
async fn a_diagram_taller_than_the_cap_stops_growing(cx: &mut TestAppContext) {
    let mut visual = paint_message(cx, &format!("```svg\n{TALL}\n```"));

    let block = visual
        .debug_bounds("markdown-inline-svg")
        .expect("the fence paints as an image block");
    assert!(
        block.size.height == steer_components::markdown::svg_block::MAX_DISPLAY_HEIGHT,
        "the block stops at the display cap: {block:?}"
    );
    assert!(
        (painted_aspect(block) - 1.0 / 3.0).abs() < 0.01,
        "the capped block still carries the 1:3 aspect: {block:?}"
    );
}

#[gpui::test]
async fn a_broken_svg_fence_stays_a_code_block(cx: &mut TestAppContext) {
    let mut visual = paint_message(
        cx,
        "An unfinished document:\n\n```svg\n<svg><rect\n```\n\nNothing to draw there.",
    );

    assert!(
        visual.debug_bounds("markdown-inline-svg").is_none(),
        "a fence the rasterizer rejects never becomes an image block"
    );
    let body = visual
        .debug_bounds("message-item-body-1")
        .expect("the body still paints as a code block");
    assert!(
        body.size.height > px(8.),
        "the body has real height: {body:?}"
    );
}

#[gpui::test]
async fn the_block_flips_between_its_image_and_its_code(cx: &mut TestAppContext) {
    let mut visual = paint_message(cx, &format!("The flow:\n\n```svg\n{DIAGRAM}\n```\n"));

    // The image is the default face.
    assert!(
        visual.debug_bounds("markdown-inline-svg").is_some(),
        "the diagram paints first"
    );
    assert!(
        visual.debug_bounds("markdown-inline-svg-code").is_none(),
        "the source stays hidden until asked for"
    );

    let toggle = visual
        .debug_bounds("markdown-inline-svg-toggle")
        .expect("the toggle paints over the block");
    visual.simulate_click(toggle.center(), Modifiers::none());
    draw(&mut visual);

    assert!(
        visual.debug_bounds("markdown-inline-svg").is_none(),
        "the code face replaces the diagram"
    );
    let code = visual
        .debug_bounds("markdown-inline-svg-code")
        .expect("the fence paints as its own code block");
    assert!(
        code.size.height > px(8.) && code.size.width > px(100.),
        "the code face has real size: {code:?}"
    );

    let toggle = visual
        .debug_bounds("markdown-inline-svg-toggle")
        .expect("the toggle stays put");
    visual.simulate_click(toggle.center(), Modifiers::none());
    draw(&mut visual);

    assert!(
        visual.debug_bounds("markdown-inline-svg").is_some(),
        "the diagram comes back"
    );
    assert!(
        visual.debug_bounds("markdown-inline-svg-code").is_none(),
        "and the source folds away again"
    );
}

/// A model writes the fence delta by delta: while the document is still open
/// the fence is an unfinished code block, and the diagram appears when its
/// closing tag lands. The same row must survive both — streaming is the only
/// way a fence ever arrives.
#[gpui::test]
async fn a_streamed_svg_fence_paints_when_it_closes(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
    let fold = std::rc::Rc::new(std::cell::RefCell::new(Fold::new()));
    fold.borrow_mut().start_turn();
    let apply = |cx: &mut gpui::App, delta: &str| {
        let mut fold = fold.borrow_mut();
        fold.delta("p-1", delta);
        project_fold(cx, &conversation, &mut fold);
    };

    cx.update(|cx| {
        apply(
            cx,
            "Here is the flow:\n\n```svg\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"400\" height=\"200\"><rect",
        );
    });
    // The projection renders the turn's user row too; the probe paints the
    // assistant bubble.
    let rows = cx.update(|cx| {
        conversation
            .read(cx)
            .items()
            .iter()
            .filter(|item| {
                matches!(
                    item.read(cx).kind(),
                    agent_ui::conversation::ConvItem::Assistant { .. }
                )
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    let window = cx.open_window(size(px(760.), px(600.)), move |_, _| RowProbe {
        row: rows[0].clone(),
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    draw(&mut visual);

    assert!(
        visual.debug_bounds("markdown-inline-svg").is_none(),
        "an unfinished fence paints as text"
    );
    assert!(
        visual.debug_bounds("message-item-body-1").is_some(),
        "the body paints while the fence streams"
    );

    visual.update(|_window, cx| {
        apply(
            cx,
            " width=\"400\" height=\"200\" fill=\"#3b82f6\"/></svg>\n```\n\nThat is it.",
        );
    });
    draw(&mut visual);

    let block = visual
        .debug_bounds("markdown-inline-svg")
        .expect("the closed fence paints as an image block");
    assert!(
        (painted_aspect(block) - 2.0).abs() < 0.01,
        "the streamed block carries the 2:1 aspect: {block:?}"
    );
}

/// The live fold, driven exactly as the store drives it: StateActions folded
/// through the published reducers, then projected. The synthesized delta
/// sequences here are fixtures — no real session journal rides this file.
struct Fold {
    chat: ahp_types::state::ChatState,
}

const TURN: &str = "t-1";

impl Fold {
    fn new() -> Self {
        Self {
            chat: serde_json::from_value(serde_json::json!({
                "resource": "ahp-chat:/c-probe",
                "title": "probe",
                "status": 0,
                "modifiedAt": "2026-01-01T00:00:00Z",
                "turns": [],
            }))
            .expect("the empty chat parses"),
        }
    }

    fn apply(&mut self, action: &ahp_types::actions::StateAction) {
        ahp::reducers::apply_action_to_chat(&mut self.chat, action);
    }

    fn start_turn(&mut self) {
        let message: ahp_types::state::Message = serde_json::from_value(serde_json::json!({
            "text": "probe", "origin": { "kind": "user" },
        }))
        .expect("the opening message parses");
        self.apply(&ahp_types::actions::StateAction::ChatTurnStarted(
            ahp_types::actions::ChatTurnStartedAction {
                turn_id: TURN.into(),
                started_at: "2026-01-01T00:00:00Z".into(),
                message,
                queued_message_id: None,
                meta: None,
            },
        ));
    }

    fn open_part(&mut self, part: &str, reasoning: bool) {
        let exists = self.chat.active_turn.as_ref().is_some_and(|turn| {
            turn.response_parts.iter().any(|p| match p {
                ahp_types::state::ResponsePart::Markdown(md) => !reasoning && md.id == part,
                ahp_types::state::ResponsePart::Reasoning(rs) => reasoning && rs.id == part,
                _ => false,
            })
        });
        if exists {
            return;
        }
        let part_value = if reasoning {
            serde_json::json!({ "kind": "reasoning", "id": part, "content": "" })
        } else {
            serde_json::json!({ "kind": "markdown", "id": part, "content": "" })
        };
        let action: ahp_types::actions::StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/responsePart", "turnId": TURN, "part": part_value,
        }))
        .expect("the responsePart action parses");
        self.apply(&action);
    }

    /// Open the markdown part (once) and append a delta — the wire's
    /// responsePart-then-delta shape.
    fn delta(&mut self, part: &str, text: &str) {
        self.open_part(part, false);
        self.apply(&ahp_types::actions::StateAction::ChatDelta(
            ahp_types::actions::ChatDeltaAction {
                turn_id: TURN.into(),
                part_id: part.into(),
                content: text.into(),
                meta: None,
            },
        ));
    }
}

/// Project the fold into the conversation — the same stage/reconcile split
/// the workspace sync runs.
fn project_fold(
    cx: &mut gpui::App,
    conversation: &gpui::Entity<ConversationState>,
    fold: &mut Fold,
) {
    let host = steer_agent_chat_ui::host::noop_host();
    let env = steer_agent_chat_ui::transcript::ProjectEnv {
        role: MODEL,
        recipient: &manox_agent::MessageAuthor::Lead,
        host: &host,
        cwd: None,
        fork_source: false,
        promoted_confirmation: None,
    };
    conversation.update(cx, |c, cx| {
        let nodes = c.stage(Some(&fold.chat), &env);
        c.reconcile(nodes, &env, cx);
    });
}
