//! LaTeX math in an assistant message paints as its formula through the
//! production row path — a `$$` fence, a one-line `$$…$$` pair that is a whole
//! line, and a `$…$` run inside a sentence — and anything that does not
//! typeset (or that is currency, not math) stays the literal text the
//! unclaimed math path renders. `markdown-math-block` / `markdown-math-run`
//! are the selectors the rendered pieces carry
//! (`steer_components::markdown::math_block`).

use agent_ui::conversation::ConversationState;
use agent_ui::views::message::MessageItem;
use gpui::{
    AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use manox_agent::MessageAuthor;

const MODEL: &str = "bigmodel-anthropic/glm-5.3";

const BLOCK_SELECTOR: &str = "markdown-math-block";
const RUN_SELECTOR: &str = "markdown-math-run";

struct RowProbe {
    row: Entity<MessageItem>,
}

impl Render for RowProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().child(self.row.clone())
    }
}

fn paint_message(cx: &mut TestAppContext, body: &str) -> VisualTestContext {
    paint_chunks(cx, &[body])
}

/// The same row, fed the way a streaming response feeds it: one
/// `AgentText` delta per chunk.
fn paint_chunks(cx: &mut TestAppContext, chunks: &[&str]) -> VisualTestContext {
    cx.update(gpui_component::init);
    let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
    cx.update(|cx| {
        let mut fold = Fold::new();
        fold.start_turn();
        for chunk in chunks {
            fold.delta("p-1", chunk);
        }
        project_fold(cx, &conversation, &mut fold);
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
    assert_eq!(rows.len(), 1, "one assistant bubble");

    let window = cx.open_window(size(px(760.), px(600.)), move |_, _| RowProbe {
        row: rows[0].clone(),
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    for _ in 0..4 {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
    visual
}
#[gpui::test]
async fn a_fenced_display_equation_paints_as_its_formula(cx: &mut TestAppContext) {
    let mut visual = paint_message(
        cx,
        "The identity:\n\n$$\nx^2 + y^2 = z^2\n$$\n\nThat is the whole proof.",
    );

    let block = visual
        .debug_bounds(BLOCK_SELECTOR)
        .expect("the equation paints as a math block");
    assert!(
        block.size.height > px(8.) && block.size.width > px(40.),
        "the formula paints at formula size: {block:?}"
    );
}

/// The form models actually write: the pair on one line, the line holding
/// nothing else. The parser calls that inline math; the row must render it as
/// the display block anyway.
#[gpui::test]
async fn a_one_line_display_pair_paints_as_a_block(cx: &mut TestAppContext) {
    let mut visual = paint_message(
        cx,
        "The identity:\n\n$$x^2 + y^2 = z^2$$\n\nThat is the whole proof.",
    );

    let block = visual
        .debug_bounds(BLOCK_SELECTOR)
        .expect("a one-line pair on its own line paints as the block");
    assert!(
        block.size.height > px(8.) && block.size.width > px(40.),
        "the formula paints at formula size: {block:?}"
    );
    assert!(
        visual.debug_bounds(RUN_SELECTOR).is_none(),
        "the promoted block is not also an inline run"
    );
}

/// A list item keeps its marker beside the formula: promoting the paragraph
/// there would strand the marker on a line of its own.
#[gpui::test]
async fn a_list_item_keeps_its_marker_beside_the_formula(cx: &mut TestAppContext) {
    let mut visual = paint_message(cx, "1. $$\\lim_{x \\to a} f(x) = 0$$\n2. prose follows.");

    assert!(
        visual.debug_bounds(RUN_SELECTOR).is_some(),
        "the pair paints as the run beside the marker"
    );
    assert!(
        visual.debug_bounds(BLOCK_SELECTOR).is_none(),
        "a list item's paragraph is never promoted out from under its marker"
    );
}

/// A display pair alone in a quote is still a display block; the quote mark
/// shares its line without owning the content.
#[gpui::test]
async fn a_display_pair_inside_a_quote_is_a_block(cx: &mut TestAppContext) {
    let mut visual = paint_message(cx, "> $$x^2 + y^2 = z^2$$");

    let block = visual
        .debug_bounds(BLOCK_SELECTOR)
        .expect("the pair paints as the block inside the quote");
    assert!(
        block.size.height > px(8.) && block.size.width > px(40.),
        "the formula paints at formula size: {block:?}"
    );
}

/// A formula split across streamed deltas typesets once its closing dollars
/// land: the row re-parses the changed tail, and the run replaces the literal
/// text the earlier chunks painted.
#[gpui::test]
async fn a_formula_split_across_chunks_typesets(cx: &mut TestAppContext) {
    let mut visual = paint_chunks(cx, &["Streaming: $$\\frac{", "a}{b}$$ and $x^", "2$ done."]);

    assert!(
        visual.debug_bounds(RUN_SELECTOR).is_some(),
        "the split formula paints as a run once closed"
    );
    assert!(
        visual.debug_bounds(BLOCK_SELECTOR).is_none(),
        "a mid-sentence pair stays inline across chunks"
    );
}

/// `$…$` inside a sentence is the inline run: it paints at the line's size and
/// must not swallow the surrounding prose.
#[gpui::test]
async fn an_inline_equation_paints_on_the_line(cx: &mut TestAppContext) {
    let mut visual = paint_message(cx, "Euler wrote $e^{i\\pi} + 1 = 0$ in prose here.");

    let run = visual
        .debug_bounds(RUN_SELECTOR)
        .expect("the inline formula paints as a run");
    assert!(
        run.size.height > px(6.) && run.size.width > px(10.),
        "the run paints the formula, not an empty box: {run:?}"
    );
    let body = visual
        .debug_bounds("message-item-body-1")
        .expect("the body still paints as text");
    assert!(
        body.size.height > px(8.),
        "the prose around the run survives: {body:?}"
    );
}

/// Two dollars signs are not math by themselves: currency keeps its dollars
/// (the parser hands `$5 and $` over as inline math).
#[gpui::test]
async fn currency_stays_prose(cx: &mut TestAppContext) {
    let mut visual = paint_message(cx, "It costs $5 and $10 in total.");

    assert!(
        visual.debug_bounds(RUN_SELECTOR).is_none(),
        "currency is never typeset as a formula"
    );
    let body = visual
        .debug_bounds("message-item-body-1")
        .expect("the body still paints as text");
    assert!(body.size.height > px(8.), "the body has real height");
}

#[gpui::test]
async fn an_untypeset_equation_stays_literal(cx: &mut TestAppContext) {
    let mut visual = paint_message(
        cx,
        "Broken:\n\n$$\n\\frac{not a formula\n$$\n\nNothing to draw there.",
    );

    assert!(
        visual.debug_bounds(BLOCK_SELECTOR).is_none(),
        "an untypeset equation never becomes an image"
    );
    let body = visual
        .debug_bounds("message-item-body-1")
        .expect("the body still paints as text");
    assert!(
        body.size.height > px(8.),
        "the body has real height: {body:?}"
    );
}

#[gpui::test]
async fn an_untypeset_inline_stays_literal(cx: &mut TestAppContext) {
    let mut visual = paint_message(cx, "Broken: $\\frac{$ in prose.");

    assert!(
        visual.debug_bounds(RUN_SELECTOR).is_none(),
        "an untypeset run never becomes an image"
    );
    assert!(
        visual.debug_bounds(BLOCK_SELECTOR).is_none(),
        "nor does it become a block"
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
        steered_turns: steer_agent_chat_ui::transcript::no_steered_turns(),
    };
    conversation.update(cx, |c, cx| {
        let nodes = c.stage(Some(&fold.chat), &env);
        c.reconcile(nodes, &env, cx);
    });
}
