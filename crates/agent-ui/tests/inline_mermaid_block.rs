//! An ```mermaid fence in an assistant message paints as its diagram in the
//! conversation, through the production row path: `ConversationState::apply`
//! feeds the text, `MessageItem` mounts the body, and the fence renders at
//! parse time and paints at the size the diagram's own aspect asks for. The
//! control case pins the other half — a fence that renders nothing stays the
//! code block it already is.
//!
//! `markdown-inline-mermaid` is the selector the rendered block carries
//! (`steer_components::markdown::svg_block`, shared with the svg fence).

use agent_ui::conversation::ConversationState;
use agent_ui::views::message::MessageItem;
use gpui::{
    AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use manox_agent::MessageAuthor;

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
        .debug_bounds("message-item-body-1")
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
            "Here is the flow:\n\n```mermaid\nflowchart LR\n    A[Input] --> B{Gate}\n    B -->|yes| C[Done]",
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
        visual.debug_bounds("markdown-inline-mermaid").is_none(),
        "an open fence stays code, never a half-drawn diagram"
    );
    assert!(
        visual.debug_bounds("message-item-body-1").is_some(),
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

        entering_turns: &[],
    };
    conversation.update(cx, |c, cx| {
        let nodes = c.stage(Some(&fold.chat), &env);
        c.reconcile(nodes, &env, cx);
    });
}
