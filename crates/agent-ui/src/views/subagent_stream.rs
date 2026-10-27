//! One sub-agent's own transcript, as a MAIN-COLUMN page.
//!
//! The Captain status block lists the Captain's line plus one line per running
//! sub-agent; clicking a sub-agent's line opens this page, which renders that
//! child's conversation — the AHP design puts a sub-agent's state in its own
//! chat (`ahp-chat:/{run-id}`), which is the only source that carries the
//! child's transcript (the parent's chat carries the run's status, never the
//! child's text).
//!
//! The page is a read-only view of a child chat that rides the child's OWN
//! channel: the host bridges a running child's journal into its chat channel,
//! so the one subscribe on open delivers the folded history AND the live
//! deltas. The page owns a store observer and re-projects from the fold on
//! every notify — the same keyed diff every surface uses, so a landed
//! snapshot and a single streaming delta take one path and a pass that
//! changed nothing notifies nothing. No composer: a child run is driven by
//! the parent's dispatch, not by the person watching it.

use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription,
};
use gpui_component::{
    ActiveTheme as _, ElementExt as _, Icon, IconName, Sizable as _, Theme, h_flex, v_flex,
};

use steer_agent_chat_ui::ahp_store::AhpStore;
use steer_agent_chat_ui::conversation::ConversationState;
use steer_agent_chat_ui::host::ChatHostHandle;
use steer_agent_chat_ui::transcript::ProjectEnv;
use steer_agent_chat_ui::views::subagents::CAPTAIN_LABEL;

use crate::i18n;

/// The sub-agent stream page: one child chat rendered read-only.
pub struct SubagentStreamView {
    /// The child's chat id (== the run id == the child session id).
    chat_id: String,
    /// The agent type (`Explore`, …) — the turn headers' recipient.
    label: String,
    /// The model the child's turn headers name (the parent session's label
    /// stands in when the child chat carries none of its own).
    role: String,
    /// The run's task label — the header's topic (the wire's background-work
    /// label stands in when no separate activity was carried).
    activity: Option<String>,
    /// The chat fold's client (the process-singleton AHP store). Held strong:
    /// the page's observer is its live leg, and the singleton outlives pages.
    store: Entity<AhpStore>,
    host: ChatHostHandle,
    conversation: Entity<ConversationState>,
    /// The live leg: every store notify re-projects the child's fold (a chat
    /// delta, a landed snapshot, the parent's work ticks — all one path).
    _store_watcher: Subscription,
    scroll_handle: gpui::ScrollHandle,
    stick_to_bottom: bool,
}

impl SubagentStreamView {
    pub(crate) fn new(
        chat_id: String,
        label: String,
        activity: Option<String>,
        store: Entity<AhpStore>,
        host: ChatHostHandle,
        role: String,
        cx: &mut gpui::App,
    ) -> Entity<Self> {
        let recipient = manox_agent::MessageAuthor::Agent(label.clone());
        let conversation = cx.new(|_| ConversationState::new(recipient));
        cx.new(|cx| {
            let _store_watcher = cx.observe(&store, |this: &mut Self, _, cx| this.refresh(cx));
            Self {
                chat_id,
                label,
                role,
                activity,
                store,
                host,
                conversation,
                _store_watcher,
                scroll_handle: gpui::ScrollHandle::new(),
                stick_to_bottom: true,
            }
        })
    }

    /// The page's assistant texts — the child chat's fold as it renders.
    /// Diagnostic-only read face for the click-through test.
    #[cfg(feature = "test-support")]
    pub(crate) fn transcript_texts(&self, cx: &gpui::App) -> Vec<String> {
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

    /// The page's row kinds in order — the diagnostic face the projection
    /// tests read: a child fold's reasoning and tool parts must land as
    /// `thinking` / `tool` rows exactly as the Captain's do.
    #[cfg(feature = "test-support")]
    pub(crate) fn item_kinds(&self, cx: &gpui::App) -> Vec<&'static str> {
        self.conversation
            .read(cx)
            .items()
            .iter()
            .map(|item| match item.read(cx).kind() {
                crate::conversation::ConvItem::User { .. } => "user",
                crate::conversation::ConvItem::Assistant { .. } => "assistant",
                crate::conversation::ConvItem::Thinking(_) => "thinking",
                crate::conversation::ConvItem::ToolCall(_) => "tool",
                crate::conversation::ConvItem::AgentTask(_) => "task",
                _ => "other",
            })
            .collect()
    }

    /// Re-project the transcript from the child chat's fold — the same keyed
    /// diff every trigger uses, so a landed snapshot updates the rows that
    /// are already mounted instead of rebuilding the page, and a streaming
    /// delta lands through one more pass of the same path. A pass that
    /// changed nothing notifies nothing. Tail-follow is the USER's state,
    /// not the projection's: a delta must not yank a reader who scrolled
    /// back up, and a reader sitting at the bottom rides along.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let role = self.role.clone();
        let recipient = manox_agent::MessageAuthor::Agent(self.label.clone());
        let host = self.host.clone();
        let chat_id = self.chat_id.clone();
        let env = ProjectEnv {
            role: &role,
            recipient: &recipient,
            host: &host,
            cwd: None,
            // A child chat's rows are a sub-agent's own display, and no
            // row here is a forkable anchor of the parent conversation.
            fork_source: false,
            promoted_confirmation: None,
        };
        let change = self.conversation.update(cx, |conv, cx| {
            let nodes = conv.stage(self.store.read(cx).book.chats.get(&chat_id), &env);
            conv.reconcile(nodes, &env, cx)
        });
        if change.any() {
            // Re-arm tail-follow only while the viewport sits near the
            // bottom; a reader scrolled into history stays put.
            let off = self.scroll_handle.offset().y;
            let max = self.scroll_handle.max_offset().y;
            if (max + off).abs() < gpui::px(20.) {
                self.stick_to_bottom = true;
            }
            cx.notify();
        }
    }
}

impl Render for SubagentStreamView {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme: Theme = cx.theme().clone();
        use steer_agent_chat_ui::text::one_line;
        use steer_agent_chat_ui::views::subagents::task_display_title;
        // One line, always: the run's topic is runtime text and may carry
        // newlines — the header must not grow a line per input line.
        let title = one_line(
            &task_display_title(&self.label, self.activity.as_deref().unwrap_or_default())
                .unwrap_or_else(|| self.chat_id.clone()),
        );

        let header = h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .gap_1p5()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .id("subagent-stream-back")
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.secondary))
                    .child(
                        Icon::new(IconName::ChevronLeft)
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        gpui::div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            // Product vocabulary, never translated.
                            .child(CAPTAIN_LABEL),
                    )
                    .on_click(cx.listener(|_this, _, _, cx| {
                        // The main column returns to the parent conversation;
                        // the parent is still the foreground thread, so no
                        // attach is needed.
                        cx.emit(BackToConversation);
                    })),
            )
            .child(
                gpui::div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .whitespace_nowrap()
                    .text_sm()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.foreground)
                    .child(title),
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
        let empty = items.is_empty();
        let mut body = v_flex()
            .id("subagent-stream-scroll")
            .w_full()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .overflow_x_hidden()
            .track_scroll(&scroll)
            .px_3()
            .py_2();
        if empty {
            body = body.child(
                gpui::div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(i18n::t("subagent-stream-empty")),
            );
        }
        body = body.children(items);
        let body = body
            .on_prepaint(move |_bounds, _window, cx| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                if !this.read(cx).stick_to_bottom {
                    return;
                }
                let off = scroll.offset().y;
                let max = scroll.max_offset().y;
                if (max + off).abs() < gpui::px(1.) {
                    return;
                }
                scroll.scroll_to_bottom();
            })
            .on_scroll_wheel(
                cx.listener(|this, ev: &gpui::ScrollWheelEvent, window, cx| {
                    // An upward wheel breaks tail-follow so the user can read
                    // back; a pass that finds the viewport back at the bottom
                    // re-arms it.
                    let dy = ev.delta.pixel_delta(window.line_height()).y;
                    if dy > gpui::Pixels::ZERO {
                        this.stick_to_bottom = false;
                        cx.notify();
                    }
                }),
            );

        v_flex()
            .h_full()
            .w_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(header)
            .child(body)
    }
}

/// Emitted by the page's back control: the workspace returns the main column
/// to the parent conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackToConversation;

impl gpui::EventEmitter<BackToConversation> for SubagentStreamView {}
