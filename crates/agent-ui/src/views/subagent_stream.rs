//! One sub-agent's own transcript, as a MAIN-COLUMN page.
//!
//! The Captain status block lists the Captain's line plus one line per running
//! sub-agent; clicking a sub-agent's line opens this page, which renders that
//! child's conversation — the AHP design puts a sub-agent's state in its own
//! chat (`ahp-chat:/{run-id}`), which is the only source that carries the
//! child's transcript (the parent's chat carries the run's status, never the
//! child's text).
//!
//! The page is a read-only view of a child chat: it inherits the child's
//! transcript from the CHAT FOLD when it opens, and keeps re-inheriting it —
//! the host serves a child chat SNAPSHOT-ONLY (the child's journal is folded
//! into the child's own channel and is never bridged to the parent), so the
//! workspace re-subscribes that channel on a bounded poll and rebuilds this
//! page whenever the child's fold version moved (see
//! `agent_ui::workspace::subagent_stream` for the polling half). No composer: a
//! child run is driven by the parent's dispatch, not by the person watching it.

use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
};
use gpui_component::{
    ActiveTheme as _, ElementExt as _, Icon, IconName, Sizable as _, Theme, h_flex, v_flex,
};

use steer_agent_chat_ui::ahp_store::AhpStore;
use steer_agent_chat_ui::conversation::{ApplyCtx, ConversationState};
use steer_agent_chat_ui::host::ChatHostHandle;

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
    /// The run's task label, when the host carried one (the header's topic).
    activity: Option<String>,
    /// The chat fold's client (the process-singleton AHP store).
    store: gpui::WeakEntity<AhpStore>,
    host: ChatHostHandle,
    conversation: Entity<ConversationState>,
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
        let conversation = build_conversation(&store, &chat_id, &label, &role, &host, cx);
        cx.new(|_| Self {
            chat_id,
            label,
            role,
            activity,
            store: store.downgrade(),
            host,
            conversation,
            scroll_handle: gpui::ScrollHandle::new(),
            stick_to_bottom: true,
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

    /// Rebuild the transcript from the child chat's fold. The CALLER decides
    /// freshness (the workspace holds the fold version it last built from), so
    /// a rebuild here is unconditional. Tail-follow is the USER's state, not
    /// the rebuild's: a fresh snapshot must not yank a reader who scrolled
    /// back up to the bottom.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.upgrade() else {
            return;
        };
        let role = self.role.clone();
        self.conversation =
            build_conversation(&store, &self.chat_id, &self.label, &role, &self.host, cx);
        cx.notify();
    }
}

fn build_conversation(
    store: &Entity<AhpStore>,
    chat_id: &str,
    label: &str,
    role: &str,
    host: &ChatHostHandle,
    cx: &mut gpui::App,
) -> Entity<ConversationState> {
    let (display, usage) = {
        let view = store.read(cx);
        let mut usage = crate::chat_fold::UsageTable::new();
        let display = view
            .book
            .chats
            .get(chat_id)
            .map(|chat| crate::chat_fold::synth_display(chat, &mut usage))
            .unwrap_or_default();
        (display, usage)
    };
    let role = role.to_string();
    let recipient = manox_agent::MessageAuthor::Agent(label.to_string());
    let ctx = ApplyCtx {
        host: host.clone(),
        cwd: None,
        // A child chat's rows are a sub-agent's own display, and no row here
        // is a forkable anchor of the parent conversation.
        fork_source: None,
    };
    cx.new(|cx| {
        ConversationState::rebuild_from_display(&display, &usage, &role, recipient, false, ctx, cx)
    })
}

impl Render for SubagentStreamView {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme: Theme = cx.theme().clone();
        use steer_agent_chat_ui::views::subagents::{one_line, task_display_title};
        // One line, always: the run's label is runtime text and may carry
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
                            .child(i18n::t("subagent-stream-back")),
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
                    // back; the next re-subscribe re-arms it.
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
