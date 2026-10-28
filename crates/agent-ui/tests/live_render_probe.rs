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
    cx.update(|cx| {
        let mut fold = Fold::new();
        fold.start_turn();
        fold.reasoning("p-r1", "用户想改路由。首先需要看现有路由表的结构。");
        fold.tool_running("tu_1", "Read", "Read src/lib.rs", "1  fn main() {}\n");
        project_fold(cx, &conversation, &mut fold);
    });
    // The projection renders the turn's user row first; the probe paints the
    // activity segment.
    let rows: Vec<gpui::Entity<MessageItem>> = cx.update(|cx| {
        conversation
            .read(cx)
            .items()
            .iter()
            .filter(|item| {
                matches!(
                    item.read(cx).kind(),
                    agent_ui::conversation::ConvItem::Thinking(_)
                )
            })
            .cloned()
            .collect()
    });
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
        .debug_bounds("message-overflow-activity-entry-1-1")
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
        .debug_bounds("message-overflow-activity-entry-1-1")
        .expect("the tool survives a long reasoning round");
    assert!(
        tool_row.size.height > px(8.),
        "the tool row still has height under a long round: {tool_row:?}"
    );

    // An open round keeps its head row and body on separate lines.
    let head = visual
        .debug_bounds("message-overflow-activity-entry-1-0-head")
        .expect("the open reasoning head paints");
    let body = visual
        .debug_bounds("message-overflow-activity-entry-body-1-0")
        .expect("the open reasoning body paints");
    assert!(
        head.bottom() <= body.top() + px(1.),
        "the head sits above the body: head={head:?}, body={body:?}"
    );
}

/// The live fold, driven exactly as the store drives it: StateActions folded
/// through the published reducers, then projected. The synthesized sequence
/// here is a fixture — no real session journal rides this file.
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

    fn apply(&mut self, action: ahp_types::actions::StateAction) {
        ahp::reducers::apply_action_to_chat(&mut self.chat, &action);
    }

    fn start_turn(&mut self) {
        let message: ahp_types::state::Message = serde_json::from_value(serde_json::json!({
            "text": "probe", "origin": { "kind": "user" },
        }))
        .expect("the opening message parses");
        self.apply(ahp_types::actions::StateAction::ChatTurnStarted(
            ahp_types::actions::ChatTurnStartedAction {
                turn_id: TURN.into(),
                started_at: "2026-01-01T00:00:00Z".into(),
                message,
                queued_message_id: None,
                meta: None,
            },
        ));
    }

    fn reasoning(&mut self, part: &str, text: &str) {
        let action: ahp_types::actions::StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/responsePart",
            "turnId": TURN,
            "part": { "kind": "reasoning", "id": part, "content": "" },
        }))
        .expect("the reasoning part action parses");
        self.apply(action);
        self.apply(ahp_types::actions::StateAction::ChatReasoning(
            ahp_types::actions::ChatReasoningAction {
                turn_id: TURN.into(),
                part_id: part.into(),
                content: text.into(),
                meta: None,
            },
        ));
    }

    /// Open a tool call and leave it Running with streamed-in content.
    fn tool_running(&mut self, call: &str, name: &str, intention: &str, output: &str) {
        self.apply(ahp_types::actions::StateAction::ChatToolCallStart(
            ahp_types::actions::ChatToolCallStartAction {
                turn_id: TURN.into(),
                tool_call_id: call.into(),
                meta: None,
                tool_name: name.into(),
                display_name: name.into(),
                intention: Some(intention.into()),
                contributor: None,
            },
        ));
        self.apply(ahp_types::actions::StateAction::ChatToolCallReady(
            ahp_types::actions::ChatToolCallReadyAction {
                turn_id: TURN.into(),
                tool_call_id: call.into(),
                meta: None,
                contributor: None,
                intention: None,
                invocation_message: intention.into(),
                tool_input: Some(ahp_types::state::ToolInput::Inline(
                    serde_json::json!({ "path": "src/lib.rs" }).to_string(),
                )),
                confirmation_title: None,
                risk_assessment: None,
                edits: None,
                editable: None,
                options: None,
                confirmed: Some(ahp_types::state::ToolCallConfirmationReason::UserAction),
            },
        ));
        self.apply(ahp_types::actions::StateAction::ChatToolCallContentChanged(
            ahp_types::actions::ChatToolCallContentChangedAction {
                turn_id: TURN.into(),
                tool_call_id: call.into(),
                meta: None,
                content: vec![ahp_types::state::ToolResultContent::Text(
                    ahp_types::state::ToolResultTextContent {
                        text: output.into(),
                    },
                )],
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
        role: "bigmodel-anthropic/glm-5.3",
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
