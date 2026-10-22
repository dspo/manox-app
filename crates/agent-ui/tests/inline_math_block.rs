//! LaTeX math in an assistant message paints as its formula through the
//! production row path — a `$$` fence, a one-line `$$…$$` pair that is a whole
//! line, and a `$…$` run inside a sentence — and anything that does not
//! typeset (or that is currency, not math) stays the literal text the
//! unclaimed math path renders. `markdown-math-block` / `markdown-math-run`
//! are the selectors the rendered pieces carry
//! (`steer_components::markdown::math_block`).

use agent_ui::conversation::{ApplyCtx, ConversationState};
use agent_ui::views::message::MessageItem;
use gpui::{
    AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use manox_agent::{MessageAuthor, ThreadEvent};

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
        conversation.update(cx, |c, cx| {
            for chunk in chunks {
                let ctx = ApplyCtx {
                    host: steer_agent_chat_ui::host::noop_host(),
                    cwd: None,
                    fork_source: None,
                };
                let _ = c.apply(
                    &ThreadEvent::AgentText(chunk.to_string()),
                    MODEL,
                    None,
                    ctx,
                    cx,
                );
            }
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
        .debug_bounds("message-item-body-0")
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
        .debug_bounds("message-item-body-0")
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
        .debug_bounds("message-item-body-0")
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
