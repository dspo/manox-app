//! Full-render probe over a live `apply` stream: pins what the data-layer
//! tests cannot see — a tool entry that paints nothing, a long reasoning
//! round that swallows the tool below it, and a reasoning head row and body
//! sharing a line. The rows come from a real `ConversationState` fed the
//! same event sequence the AHP live path delivers, mounted through the
//! production message-list row shape.

use agent_ui::conversation::{ActivityEntry, ConvItem, ConversationState};
use agent_ui::views::{MessageListWidthInvalidator, message::MessageItem};
use gpui::{
    AppContext as _, InteractiveElement as _, IntoElement, ListAlignment, ListState,
    ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, Window, div, list,
    px, size,
};
use gpui_component::ElementExt as _;
use manox_agent::{ThreadEvent, ToolCallStatus};

struct LiveProbe {
    state: ListState,
    rows: Vec<gpui::Entity<MessageItem>>,
    width_invalidator: MessageListWidthInvalidator,
}

impl Render for LiveProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        let state = self.state.clone();
        let width_state = state.clone();
        let width_invalidator = self.width_invalidator.clone();
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                list(state, move |ix, _window, _cx| {
                    div()
                        .w_full()
                        .pt_1()
                        .pb_4()
                        .min_w_0()
                        .flex_shrink_0()
                        .debug_selector(move || format!("live-row-{ix}"))
                        .child(rows[ix].clone())
                        .into_any_element()
                })
                .w_full()
                .h_full()
                .min_h_0()
                .min_w_0(),
            )
            .on_prepaint(move |bounds, window, _cx| {
                if width_invalidator.update(bounds.size.width, &width_state) {
                    window.refresh();
                }
            })
    }
}

#[gpui::test]
async fn live_tool_rows_paint_and_survive_a_long_reasoning_round(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let conversation =
        cx.update(|cx| cx.new(|_| ConversationState::new(manox_agent::MessageAuthor::Lead)));
    let ctx = agent_ui::conversation::ApplyCtx {
        host: steer_agent_chat_ui::host::noop_host(),
        cwd: None,
        fork_source: None,
    };
    cx.update(|cx| {
        conversation.update(cx, |c, cx| {
            let _ = c.apply(
                &ThreadEvent::AgentThinking("用户想改路由。首先需要看现有路由表的结构。".into()),
                "bigmodel-anthropic/glm-5.3",
                None,
                ctx.clone(),
                cx,
            );
            let _ = c.apply(
                &ThreadEvent::ToolCall {
                    id: "tu_1".into(),
                    name: "Read".into(),
                    title: "Read src/lib.rs".into(),
                    status: ToolCallStatus::Running,
                    input: Some(serde_json::json!({"path": "src/lib.rs"})),
                },
                "bigmodel-anthropic/glm-5.3",
                None,
                ctx.clone(),
                cx,
            );
            let _ = c.apply(
                &ThreadEvent::ToolOutput {
                    id: "tu_1".into(),
                    chunk: "1  fn main() {}\n".into(),
                },
                "bigmodel-anthropic/glm-5.3",
                None,
                ctx.clone(),
                cx,
            );
        });
    });
    let rows: Vec<gpui::Entity<MessageItem>> =
        cx.update(|cx| conversation.read(cx).items().to_vec());
    assert_eq!(rows.len(), 1, "one activity-segment item");

    let state = ListState::new(rows.len(), ListAlignment::Top, px(2048.));
    let window = cx.open_window(size(px(760.), px(600.)), {
        let state = state.clone();
        let rows = rows.clone();
        move |_, _| LiveProbe {
            state,
            rows,
            width_invalidator: MessageListWidthInvalidator::default(),
        }
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let paint = |visual: &mut VisualTestContext| {
        visual.run_until_parked();
        for _ in 0..2 {
            visual.update(|window, cx| window.draw(cx).clear(cx));
        }
    };
    paint(&mut visual);

    // The tool entry renders — a visible row, not nothing. Card-family
    // tools render the ai-elements card (its own header is the fold), so the
    // step's row selector is the stable paint witness either way.
    let tool_row = visual
        .debug_bounds("message-overflow-activity-entry-0-1")
        .expect("the tool entry row paints");
    assert!(
        tool_row.size.height > px(8.),
        "the tool row has real height: {tool_row:?}"
    );

    // Open the (auto-folded) reasoning round over a body far taller than any
    // cap the region ever had: the steps region plays at natural height, so
    // the tool below it must still paint instead of being clipped away.
    cx.update(|cx| {
        conversation.update(cx, |c, cx| {
            if let Some(item) = c.items().first() {
                item.update(cx, |item, cx| {
                    if let ConvItem::Thinking(t) = item.kind_mut()
                        && let Some(ActivityEntry::Reasoning {
                            text, collapsed, ..
                        }) = t.entries.first_mut()
                    {
                        *text = "很长的推理。".repeat(400);
                        *collapsed = false;
                    }
                    item.rebuild_activity_reasoning(cx);
                    cx.notify();
                });
            }
        });
    });
    paint(&mut visual);
    let tool_row = visual
        .debug_bounds("message-overflow-activity-entry-0-1")
        .expect("the tool survives a long reasoning round");
    assert!(
        tool_row.size.height > px(8.),
        "the tool row still has height under a long round: {tool_row:?}"
    );

    // An open round keeps its head row and body on separate lines.
    let head = visual
        .debug_bounds("message-overflow-activity-entry-0-0-head")
        .expect("the open reasoning head paints");
    let body = visual
        .debug_bounds("message-overflow-activity-entry-body-0-0")
        .expect("the open reasoning body paints");
    assert!(
        head.bottom() <= body.top() + px(1.),
        "the head sits above the body: head={head:?}, body={body:?}"
    );
}
