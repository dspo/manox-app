//! The sub-agent drill-down: one child session's transcript, rendered live
//! from its own AHP chat fold. The panel owns a conversation pipeline
//! instance driven by the same keyed projection the main column applies
//! (`transcript::project` over the child's folded `ChatState`); the child
//! session is attached on open and the panel inherits the child's whole
//! fold the first time it lands — a child that streamed long before the
//! tab opened (or one that already settled) shows its full transcript, not
//! just the deltas that follow the open. A worker
//! chat is read-only (the host stamps the interactivity; the footer states
//! it).

use gpui::prelude::*;
use gpui::{AnyElement, App, Context, Entity, ScrollHandle, Subscription, Window, px};
use gpui_component::{ActiveTheme as _, ElementExt as _, h_flex, v_flex};
use manox_agent::ToolCallStatus;
use steer_agent_chat_ui::ahp_store::AhpStore;
use steer_agent_chat_ui::transcript::ProjectEnv;

use crate::conversation::ConversationState;
use crate::i18n;
use crate::views::subagents::status_indicator;

pub struct SubagentPanel {
    /// The task topic — the panel banner and the tab label.
    topic: String,
    /// The child's agent type (`Explore`, …) — the inherited user rows'
    /// recipient header.
    label: String,
    /// The status the opener knew; the live read below overrides it while
    /// the child chat has an active turn.
    opened_status: ToolCallStatus,
    /// The child run rendered as a miniature conversation through the shared
    /// message pipeline.
    conversation: Entity<ConversationState>,
    /// The model label the child's turn headers carry (the parent's stand-in
    /// until the child reports its own).
    role: String,
    host: steer_agent_chat_ui::host::ChatHostHandle,
    /// The child session this panel renders: the store plus the run id (the
    /// run id names the child session — the swarm invariant). `None` when
    /// no store is bound; the panel then stays in its settling phase.
    source: Option<(Entity<AhpStore>, String)>,
    _subscription: Option<Subscription>,
    scroll_handle: ScrollHandle,
    stick_to_bottom: bool,
}

impl SubagentPanel {
    pub(crate) fn new(
        topic: String,
        label: String,
        role: String,
        status: ToolCallStatus,
        host: steer_agent_chat_ui::host::ChatHostHandle,
        source: Option<(Entity<AhpStore>, String)>,
        cx: &mut App,
    ) -> Entity<Self> {
        let recipient = manox_agent::MessageAuthor::Agent(label.clone());
        let conversation = cx.new(|_| ConversationState::new(recipient));
        cx.new(|cx| {
            // The pump: every store notification re-projects the child chat's
            // fold into this panel's conversation. The main column's sync
            // projects only the foreground session's fold — the child chat is
            // this panel's alone, and the keyed diff makes the first landed
            // snapshot (which carries everything that streamed before the
            // open) and a single later delta the same cheap pass.
            let subscription = source.as_ref().map(|(store, sid)| {
                let sid = sid.clone();
                cx.observe(store, move |this: &mut Self, store, cx| {
                    this.pump(&sid, &store, cx)
                })
            });
            Self {
                topic,
                label,
                opened_status: status,
                conversation,
                role,
                host,
                source,
                _subscription: subscription,
                scroll_handle: ScrollHandle::new(),
                stick_to_bottom: true,
            }
        })
    }

    /// Re-project the child chat's fold into the panel's conversation.
    /// A child that streamed before the tab opened (the usual case — the
    /// card is clicked after the run started, or again after it settled)
    /// shows its whole transcript at the first landed snapshot; a delta that
    /// raced ahead of the snapshot lands on the same keys and is absorbed.
    fn pump(&mut self, sid: &str, store: &Entity<AhpStore>, cx: &mut Context<Self>) {
        let role = self.role.clone();
        let recipient = manox_agent::MessageAuthor::Agent(self.label.clone());
        let host = self.host.clone();
        let env = ProjectEnv {
            role: &role,
            recipient: &recipient,
            host: &host,
            cwd: None,
            // The panel's rows are a child session's display — no row
            // here is a forkable anchor.
            fork_source: false,
            promoted_confirmation: None,
        };
        let change = self.conversation.update(cx, |conv, cx| {
            let nodes = {
                let view = store.read(cx);
                let leaf = crate::ahp_store::leaf(&view.book, sid);
                conv.stage(leaf.chat, &env)
            };
            conv.reconcile(nodes, &env, cx)
        });
        // A notification the projection answered with no row change has
        // nothing to repaint.
        if !change.any() {
            return;
        }
        // Re-arm tail-follow only while the viewport is still near the
        // bottom; a reader scrolled into history must not be yanked back.
        let off = self.scroll_handle.offset().y;
        let max = self.scroll_handle.max_offset().y;
        if (max + off).abs() < px(20.) {
            self.stick_to_bottom = true;
        }
        cx.notify();
    }

    /// The live phase: the snapshot has not landed yet (settling), the fold
    /// carries rows (active), or a landed fold carries none (empty).
    fn phase(&self, cx: &App) -> Phase {
        let Some((store, sid)) = &self.source else {
            return Phase::Settling;
        };
        let view = store.read(cx);
        if !view.book.chat_snapshot_landed(sid) {
            return Phase::Settling;
        }
        if self.conversation.read(cx).items().is_empty() {
            Phase::Empty
        } else {
            Phase::Active
        }
    }

    /// The child chat still has an in-flight turn. The run's liveness on the
    /// PARENT's background work is not readable from here (the panel holds
    /// the child id only); the child's own active turn is the equivalent live
    /// signal, and the opener's status covers the gaps between turns.
    fn running(&self, cx: &App) -> bool {
        let Some((store, sid)) = &self.source else {
            return false;
        };
        let view = store.read(cx);
        crate::ahp_store::leaf(&view.book, sid).running()
    }
}

enum Phase {
    Settling,
    Active,
    Empty,
}

#[cfg(all(test, feature = "test-support"))]
impl SubagentPanel {
    /// Run one pump tick against the bound store. Test-support diagnostic:
    /// the production pump is the store observer this test replaces.
    pub(crate) fn diagnostic_pump(&mut self, cx: &mut Context<Self>) {
        let Some((store, sid)) = self.source.clone() else {
            return;
        };
        self.pump(&sid, &store, cx);
    }

    /// The transcript's assistant texts, in row order.
    pub(crate) fn diagnostic_transcript_texts(&self, cx: &App) -> Vec<String> {
        self.conversation
            .read(cx)
            .items()
            .iter()
            .filter_map(|item| match item.read(cx).kind() {
                crate::conversation::ConvItem::Assistant { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }
}

impl Render for SubagentPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let status = if self.running(cx) {
            ToolCallStatus::Running
        } else {
            self.opened_status
        };

        let header = h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .gap_1p5()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(status_indicator(status, theme))
            .child(
                gpui::div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.foreground)
                    .child(self.topic.clone()),
            );

        let items: Vec<AnyElement> = self
            .conversation
            .read(cx)
            .items()
            .iter()
            .cloned()
            .map(|item| {
                v_flex()
                    .pt_1()
                    .pb_4()
                    .flex_shrink_0()
                    .min_w_0()
                    .child(item)
                    .into_any_element()
            })
            .collect();

        let scroll = self.scroll_handle.clone();
        let weak = cx.weak_entity();

        let mut body = v_flex()
            .id("subagent-transcript-scroll")
            .w_full()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .overflow_x_hidden()
            .track_scroll(&scroll)
            .px_3()
            .py_2();
        // The phases are distinct states, never one placeholder: settling is
        // "the answer is in flight"; empty is "landed, nothing there yet".
        if items.is_empty() {
            let line = match self.phase(cx) {
                Phase::Settling => i18n::t("subagent-panel-settling"),
                Phase::Empty | Phase::Active => i18n::t("subagent-panel-empty"),
            };
            body = body.child(
                gpui::div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(line),
            );
        }
        body = body.children(items);

        let body = body.on_prepaint(move |_bounds, _window, cx| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.update(cx, |this, _| {
                if this.stick_to_bottom {
                    this.scroll_handle.scroll_to_bottom();
                }
            });
        });

        v_flex()
            .w_full()
            .h_full()
            .min_h_0()
            .child(header)
            .child(body)
            .child(
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .py_1p5()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(i18n::t("subagent-panel-readonly")),
            )
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

    const RUN: &str = "sub-ns-7";

    fn child_chat(text: &str) -> ahp_types::state::ChatState {
        serde_json::from_value(serde_json::json!({
            "resource": chat_uri(RUN),
            "title": "the child's chat",
            "status": 0,
            "modifiedAt": "2026-10-21T00:00:03.000Z",
            "turns": [{
                "id": "turn-1",
                "message": { "text": "fix the login redirect", "origin": { "kind": "user" } },
                "responseParts": [
                    { "kind": "markdown", "id": "p1", "content": text },
                ],
                "state": "complete",
            }],
        }))
        .expect("the child chat snapshot parses")
    }

    fn test_panel(store: Entity<AhpStore>, cx: &mut TestAppContext) -> Entity<SubagentPanel> {
        cx.update(|cx| {
            SubagentPanel::new(
                "fix the login redirect".into(),
                "Explore".into(),
                "glm-5.3".into(),
                manox_agent::ToolCallStatus::Running,
                steer_agent_chat_ui::host::noop_host(),
                Some((store, RUN.to_string())),
                cx,
            )
        })
    }

    /// A child that streamed long before the tab opened — or one that already
    /// settled — shows its whole transcript at the first pump after the
    /// snapshot lands, and the deltas that follow the open stream on top of
    /// it without doubling (the fold holds everything the drained events
    /// carried, so the wholesale rebuild covers them). The inherited user row
    /// carries the child's agent type as its recipient, never an empty name.
    #[gpui::test]
    fn a_panel_inherits_the_childs_fold_when_it_lands(cx: &mut TestAppContext) {
        let store = cx.new(|_| AhpStore::detached());
        let panel = test_panel(store.clone(), cx);

        let empty = panel.read_with(cx, |p, cx| p.diagnostic_transcript_texts(cx));
        assert!(empty.is_empty(), "nothing before the snapshot: {empty:?}");

        store.update(cx, |s, _| {
            s.book
                .chats
                .insert(RUN.into(), child_chat("the child's answer"));
            s.diagnostic_mark_chat_snapshot_landed(RUN);
        });
        panel.update(cx, |p, cx| p.diagnostic_pump(cx));

        let texts = panel.read_with(cx, |p, cx| p.diagnostic_transcript_texts(cx));
        assert!(
            texts.iter().any(|text| text.contains("the child's answer")),
            "the inherited transcript carries the fold: {texts:?}"
        );
        let recipients = panel.read_with(cx, |p, cx| {
            p.conversation
                .read(cx)
                .items()
                .iter()
                .filter_map(|item| match item.read(cx).kind() {
                    crate::conversation::ConvItem::User { meta, .. } => {
                        meta.as_ref().and_then(|meta| meta.recipient.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(
            recipients,
            vec![manox_agent::MessageAuthor::Agent("Explore".into())],
            "the inherited prompt names the child's agent type"
        );

        // A post-open delta streams into the child chat's ACTIVE turn; the
        // pump re-projects.
        store.update(cx, |s, _| {
            let settled = s
                .book
                .chats
                .get(RUN)
                .cloned()
                .expect("the child chat folds");
            // Move the settled turn into the in-flight slot so the delta has
            // a target, exactly as a run that is still going looks.
            let mut active: serde_json::Value =
                serde_json::to_value(&settled).expect("the chat serializes");
            let mut turn = active["turns"]
                .as_array()
                .and_then(|t| t.first().cloned())
                .expect("the child's turn is in flight");
            // The in-flight form stamps its start and drops the settled fields.
            turn["startedAt"] = serde_json::Value::String("2026-10-21T00:00:03.000Z".into());
            turn.as_object_mut().expect("object").remove("duration");
            turn.as_object_mut().expect("object").remove("state");
            active["turns"] = serde_json::Value::Array(Vec::new());
            active["activeTurn"] = turn;
            let mut chat: ahp_types::state::ChatState =
                serde_json::from_value(active).expect("the in-flight child chat parses");
            let open: ahp_types::actions::StateAction = serde_json::from_value(serde_json::json!({
                "type": "chat/responsePart", "turnId": "turn-1",
                "part": { "kind": "markdown", "id": "p2", "content": "" },
            }))
            .expect("the open action parses");
            let delta: ahp_types::actions::StateAction =
                serde_json::from_value(serde_json::json!({
                    "type": "chat/delta", "turnId": "turn-1", "partId": "p2",
                    "content": " streamed",
                }))
                .expect("the delta action parses");
            ahp::reducers::apply_action_to_chat(&mut chat, &open);
            ahp::reducers::apply_action_to_chat(&mut chat, &delta);
            s.book.chats.insert(RUN.into(), chat);
        });
        panel.update(cx, |p, cx| p.diagnostic_pump(cx));

        let texts = panel.read_with(cx, |p, cx| p.diagnostic_transcript_texts(cx));
        let joined = texts.join("\n---\n");
        assert!(
            joined.contains("the child's answer"),
            "the inherited rows survive: {joined}"
        );
        assert_eq!(
            joined.matches("the child's answer").count(),
            1,
            "the rebuild must not double the drained content: {joined}"
        );
        assert!(
            joined.contains("streamed"),
            "the post-open delta streams on top: {joined}"
        );
    }

    /// A delta can win the race against the subscribe snapshot (the bucket and
    /// the snapshot ride the same fan-in). That early delta opens rows, so an
    /// inherit keyed on "the panel is still empty" would never run and the
    /// pre-open transcript would stay missing. The landed flag is the key.
    #[gpui::test]
    fn a_racing_delta_does_not_skip_the_fold_inherit(cx: &mut TestAppContext) {
        let store = cx.new(|_| AhpStore::detached());
        let panel = test_panel(store.clone(), cx);

        // A delta wins the race: the fold holds it before the snapshot lands.
        store.update(cx, |s, _| {
            let mut chat = s.book.chats.get(RUN).cloned().unwrap_or_else(|| {
                serde_json::from_value(serde_json::json!({
                    "resource": steer_agent_chat_ui::ahp_store::chat_uri(RUN),
                    "title": "the child's chat",
                    "status": 0,
                    "modifiedAt": "2026-10-21T00:00:03.000Z",
                    "turns": [],
                    "activeTurn": {
                        "id": "turn-1",
                        "startedAt": "2026-10-21T00:00:03.000Z",
                        "message": { "text": "fix the login redirect", "origin": { "kind": "user" } },
                        "responseParts": [],
                    },
                }))
                .expect("the racing child chat parses")
            });
            let open: ahp_types::actions::StateAction = serde_json::from_value(
                serde_json::json!({
                    "type": "chat/responsePart", "turnId": "turn-1",
                    "part": { "kind": "markdown", "id": "p1", "content": "" },
                }),
            )
            .expect("the open action parses");
            let delta: ahp_types::actions::StateAction = serde_json::from_value(
                serde_json::json!({
                    "type": "chat/delta", "turnId": "turn-1", "partId": "p1",
                    "content": "raced",
                }),
            )
            .expect("the delta action parses");
            ahp::reducers::apply_action_to_chat(&mut chat, &open);
            ahp::reducers::apply_action_to_chat(&mut chat, &delta);
            s.book.chats.insert(RUN.into(), chat);
        });
        panel.update(cx, |p, cx| p.diagnostic_pump(cx));
        let early = panel.read_with(cx, |p, cx| p.diagnostic_transcript_texts(cx));
        assert!(early.iter().any(|text| text.contains("raced")));

        store.update(cx, |s, _| {
            s.book
                .chats
                .insert(RUN.into(), child_chat("the child's answer and raced"));
            s.diagnostic_mark_chat_snapshot_landed(RUN);
        });
        panel.update(cx, |p, cx| p.diagnostic_pump(cx));

        let texts = panel.read_with(cx, |p, cx| p.diagnostic_transcript_texts(cx));
        let joined = texts.join("\n---\n");
        assert!(
            joined.contains("the child's answer and raced"),
            "the landed fold is inherited even though a delta opened a row: {joined}"
        );
        assert_eq!(
            joined.matches("raced").count(),
            1,
            "the rebuild replaces the raced row instead of doubling it: {joined}"
        );
    }
}
