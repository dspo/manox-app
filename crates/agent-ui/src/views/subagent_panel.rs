//! The sub-agent drill-down: one child session's transcript, rendered live
//! from its own AHP chat fold. The panel owns a conversation pipeline
//! instance fed by the same fold vocabulary the main column applies
//! (`ChatEvent` → `to_thread_event` → `apply`); the child session is
//! attached on open, its journal replays through the bucket, and the panel
//! inherits the child's whole fold the first time it lands — a child that
//! streamed long before the tab opened (or one that already settled) shows
//! its full transcript, not just the deltas that follow the open. A worker
//! chat is read-only (the host stamps the interactivity; the footer states
//! it).

use std::collections::HashMap;

use gpui::prelude::*;
use gpui::{AnyElement, App, Context, Entity, ScrollHandle, Subscription, Window, px};
use gpui_component::{ActiveTheme as _, ElementExt as _, h_flex, v_flex};
use manox_agent::ToolCallStatus;
use steer_agent_chat_ui::ahp_store::AhpStore;
use steer_agent_chat_ui::chat_fold;

use crate::conversation::{ApplyCtx, ConversationState};
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
    /// Whether the child's fold has been inherited. The bucket only carries
    /// deltas from this panel's subscribe on, so the first landed snapshot is
    /// the one chance to pick up everything that streamed before the open —
    /// including any delta that raced ahead of the snapshot.
    fold_inherited: bool,
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
        let ctx = ApplyCtx {
            host: host.clone(),
            cwd: None,
            // The panel's rows are a child session's display — no row here
            // is a forkable anchor.
            fork_source: None,
        };
        let role_for_conv = role.clone();
        let label_for_conv = label.clone();
        let empty_usage: HashMap<String, manox_agent::language_model::TokenUsage> = HashMap::new();
        let conversation = cx.new(|cx| {
            ConversationState::rebuild_from_display(
                &[],
                &empty_usage,
                &role_for_conv,
                manox_agent::MessageAuthor::Agent(label_for_conv),
                false,
                ctx.clone(),
                cx,
            )
        });
        cx.new(|cx| {
            // The pump: every store notification drains the child session's
            // own event bucket through the shared fold vocabulary into this
            // panel's conversation. The main column's live leg drains only
            // the foreground session's bucket — the child bucket is this
            // panel's alone.
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
                fold_inherited: false,
                _subscription: subscription,
                scroll_handle: ScrollHandle::new(),
                stick_to_bottom: true,
            }
        })
    }

    /// Drain the child session's event bucket into the panel's conversation,
    /// and inherit the child's chat fold once it lands: the bucket only
    /// carries deltas from this panel's subscribe on, so a child that streamed
    /// before the tab opened (the usual case — the card is clicked after the
    /// run started, or again after it settled) would otherwise sit on an empty
    /// transcript forever. The fold holds everything the drained events
    /// carried, so the wholesale rebuild covers them and nothing doubles; a
    /// delta that raced ahead of the snapshot opens rows early, so the inherit
    /// keys on the LANDED flag, not on "the panel is still empty".
    fn pump(&mut self, sid: &str, store: &Entity<AhpStore>, cx: &mut Context<Self>) {
        let events = store.update(cx, |s, _| s.drain_chat_events(sid));
        let had_events = !events.is_empty();
        let role = self.role.clone();
        let ctx = ApplyCtx {
            host: self.host.clone(),
            cwd: None,
            fork_source: None,
        };
        for event in events {
            let Some(thread_event) = chat_fold::to_thread_event(event) else {
                continue;
            };
            self.conversation.update(cx, |c, cx| {
                c.apply(&thread_event, &role, None, ctx.clone(), cx);
            });
        }
        let mut inherited = false;
        let mut landed_now = false;
        if !self.fold_inherited {
            let (landed, display, usage, trailing) = {
                let view = store.read(cx);
                let landed = view.book.chat_snapshot_landed(sid);
                let leaf = crate::ahp_store::leaf(&view.book, sid);
                let mut usage = crate::chat_fold::UsageTable::new();
                let display = if landed {
                    leaf.chat
                        .map(|chat| crate::chat_fold::synth_display(chat, &mut usage))
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                (landed, display, usage, leaf.running())
            };
            if landed {
                self.fold_inherited = true;
                landed_now = true;
                if !display.is_empty() {
                    let recipient = manox_agent::MessageAuthor::Agent(self.label.clone());
                    self.conversation.update(cx, |c, cx| {
                        *c = ConversationState::rebuild_from_display(
                            &display, &usage, &role, recipient, trailing, ctx, cx,
                        );
                    });
                    inherited = true;
                }
            }
        }
        // A notification that neither carried values nor resolved the
        // settling phase has nothing to repaint.
        if !had_events && !inherited && !landed_now {
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

        store.update(cx, |s, _| {
            s.push_chat_event_for_test(RUN, chat_fold::ChatEvent::AgentText(" streamed".into()));
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

        store.update(cx, |s, _| {
            s.push_chat_event_for_test(RUN, chat_fold::ChatEvent::AgentText("raced".into()));
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
