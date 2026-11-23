//! The workspace render face: the `Render` impl and its builders — the
//! conversation column mounted as the chrome shell's main surface
//! (`render_column`), the in-card settings swap (`render_settings_card`),
//! notice. Split from `workspace.rs` as one contiguous run of impl blocks —
//! `super` is the workspace module, so the builders reach the parent's
//! private fields and methods unchanged.

use super::*;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Identity hand-off: the successor takes the foreground, and the
        // predecessor's thread parks with its transcript (its id keeps
        // resolving server-side for stale sends).
        if let Some(next) = self.chat.update(cx, |chat, cc| {
            let v = chat.pending_successor.take();
            cc.notify();
            v
        }) {
            // The successor hand-off is an identity swap of the same
            // conversation, not user navigation: the current history entry
            // is REWRITTEN to the successor id (appending would strand the
            // retired predecessor one ← away).
            self.replace_nav_current(next, window, cx);
            cx.notify();
        }
        // The turn navigator belongs to the conversation page; leaving it
        // drops the overlay.
        if !matches!(self.view_mode, ViewMode::Workspace) {
            self.drop_turn_navigator(cx);
        }
        // gpui cancels a drag on any mouse-up that doesn't land inside a
        // payload-matching drop target (`on_drop` never runs) — prune the
        // queue-drag marker here. gpui refreshes on that cancel, so this
        // clears the same frame; without it the source row would stay dimmed
        // and the insertion line pinned.
        if !cx.has_active_drag() && self.chat.read(cx).queue_drag.is_some() {
            self.chat.update(cx, |chat, cx| {
                chat.queue_drag = None;
                cx.notify();
            });
        }
        // Settings is the split layout: the chrome sidebar carries the nav
        // (swapped in by `enter_settings`) and this column carries the
        // selected page's detail.
        if matches!(self.view_mode, ViewMode::Settings) {
            return self.render_settings_detail(window, cx);
        }
        // An external session (a launched CLI agent TUI / plain terminal)
        // owns the main column the same way — the shell wraps the main
        // column, and terminal/TUI sessions live THERE, not in the right
        // pane. A vanished record falls through to the conversation
        // defensively.
        if matches!(self.view_mode, ViewMode::ExternalSession)
            && let Some(card) = self.render_external_session_card(cx)
        {
            return self.apply_chat_actions(card, window, cx);
        }
        if matches!(self.view_mode, ViewMode::SubagentStream)
            && let Some(card) = self.render_subagent_stream_card(cx)
        {
            return self.apply_chat_actions(card, window, cx);
        }
        // The launcher (the pre-container launch pad): the composer's ⏎ and
        // the action row mint the containers from here; the page's render
        // also adopts a project the create dialog just minted (the chip
        // menu's 新建项目… closes into this frame).
        if matches!(self.view_mode, ViewMode::Launcher)
            && let Some(launcher) = self.launcher.clone()
        {
            launcher.update(cx, |state, cx| state.adopt_created_project(cx));
            let root = gpui::div()
                .id("launcher-main")
                .debug_selector(|| "launcher-main".into())
                .size_full()
                .child(launcher);
            return self.apply_chat_actions(root, window, cx);
        }
        // A blocking overlay owns the page: the turn navigator cannot stay
        // mounted under it.
        if self.blocking_overlay_active(cx) && self.chat.read(cx).turn_navigator.is_some() {
            self.close_turn_navigator(window, cx);
        }
        // Per-frame conversation maintenance: reconcile the local cards
        // against the server projections and sync the workspace-derived
        // snapshots onto the owning tool rows (the ask card — including
        // mounting its interactive state — and the unified confirmation
        // card).
        self.reconcile_pending_with_projections(cx);
        self.sync_ask_card_snapshots(cx);
        self.sync_confirmation_snapshot(cx);
        self.render_column(window, cx)
    }
}

impl Workspace {
    /// The turn-navigator overlay for the shared action root. The full
    /// positioning variant (render_turn_navigator_overlay) needs the shell's
    /// geometry flags; the shared root re-renders the overlay entity's
    /// own absolute positioning via its Render impl.
    fn render_turn_navigator_overlay_placeholder(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.chat
            .read(cx)
            .turn_navigator
            .clone()
            .map(|nav| nav.into_any_element())
    }

    fn apply_chat_actions<E>(
        &self,
        root: E,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement
    where
        E: gpui::InteractiveElement
            + gpui::StatefulInteractiveElement
            + gpui::Styled
            + gpui::ParentElement
            + gpui::IntoElement
            + 'static,
    {
        root.on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
            this.enter_settings(window, cx);
        }))
        .on_action(cx.listener(|this, _: &ToggleTurnNavigator, window, cx| {
            this.toggle_turn_navigator(window, cx);
            cx.stop_propagation();
        }))
        .on_action(
            cx.listener(|this, _: &crate::BackgroundCurrentThread, window, cx| {
                this.background_current_thread(window, cx);
            }),
        )
        .on_action(cx.listener(|this, _: &crate::UndoLastQueued, _window, cx| {
            this.undo_last_queued(cx);
        }))
        // Completion actions only match via the `completion == open > Input`
        // keybindings, so any fire means the popover was open and these
        // keystrokes belong to it. Stop propagation so the Input's own
        // parallel up/down/enter/tab/escape binding (same depth, lower
        // register index) doesn't also fire — otherwise Enter would both
        // confirm and submit, Up/Down would move caret and selection, etc.
        .on_action(cx.listener(|this, _: &crate::CompletionUp, window, cx| {
            this.completion_up(window, cx);
            cx.stop_propagation();
        }))
        .on_action(cx.listener(|this, _: &crate::CompletionDown, window, cx| {
            this.completion_down(window, cx);
            cx.stop_propagation();
        }))
        .on_action(
            cx.listener(|this, _: &crate::CompletionConfirm, window, cx| {
                this.completion_confirm_selected(window, cx);
                cx.stop_propagation();
            }),
        )
        .on_action(
            cx.listener(|this, _: &crate::CompletionDismiss, _window, cx| {
                this.close_completion(cx);
                cx.stop_propagation();
            }),
        )
        // Composer history recall: reachable only through the
        // `composer > Input` bindings on alt-up / alt-down, so these
        // listeners never see the bare arrows the Input uses to move the
        // caret.
        .on_action(
            cx.listener(|this, _: &crate::ComposerRecallUp, window, cx| {
                this.composer_recall_up(window, cx);
            }),
        )
        .on_action(
            cx.listener(|this, _: &crate::ComposerRecallDown, window, cx| {
                this.composer_recall_down(window, cx);
            }),
        )
        // ⇧⌘↩ interrupts: a dedicated action, NOT the input's Enter — the
        // Enter path newlines on shift before emitting, and the submit guard
        // would then leave a stray newline behind. Absolute cancel priority
        // applies to the turn, not to a pending ask card: while a card is
        // up, submit_composer answers the card (its own controls are the
        // only answer surface), matching Enter.
        .on_action(cx.listener(
            |this, _: &steer_agent_chat_ui::ComposerSubmitInterrupt, window, cx| {
                this.submit_interrupt(window, cx);
            },
        ))
        .on_action(
            cx.listener(|this, _: &crate::ArchiveCurrentThread, window, cx| {
                this.archive_current_thread(window, cx);
            }),
        )
        .children(self.render_turn_navigator_overlay_placeholder(cx))
        .into_any_element()
    }

    /// The settings detail (the main column's content while Settings is
    /// open): the selected page's group stack below a TitleBar strip naming
    /// the page. Navigation is the chrome sidebar's injected nav — no back
    /// control here, the nav owns exiting.
    fn render_settings_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use gpui::{ParentElement as _, Styled as _, div, px};
        use gpui_component::v_flex;

        let settings = self
            .settings_view
            .as_ref()
            .expect("enter_settings must have created the SettingsView")
            .clone();
        let page_ix = self.settings_nav;
        let content = settings.update(cx, |s, cx| s.render_page(page_ix, window, cx));
        // The detail has NO title bar of its own (the window's toolbar is
        // the drag face) and NO page header — the nav tree already says
        // where you are. The page rides a centered, measure-limited column
        // inside the main card: gutters of card background on the flanks
        // instead of full-bleed content.
        const DETAIL_MAX_W: f32 = 880.;
        let root = v_flex().id("settings-detail").size_full().min_w_0().child(
            div().flex_1().min_h_0().w_full().overflow_hidden().child(
                div()
                    .max_w(px(DETAIL_MAX_W))
                    .w_full()
                    .h_full()
                    .pt(px(16.))
                    .mx_auto()
                    .child(content),
            ),
        );
        self.apply_chat_actions(root, window, cx)
    }

    /// The external session's main-column card: the live terminal, full
    /// surface (the same wrap the right-pane tabs use — a 4px inset on the
    /// card background).
    fn render_external_session_card(
        &mut self,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        use gpui::{ParentElement as _, Styled as _, div, px};
        let id = self.active_external.clone()?;
        let view = self.externals.iter().find(|s| s.id == id)?.view.clone();
        Some(
            div()
                .id("external-session-main")
                .debug_selector(|| "external-session-main".into())
                .size_full()
                .flex()
                .p(px(4.))
                .child(view),
        )
    }

    /// The sub-agent stream page (one running child's own chat) as the main
    /// column's card. The page owns its transcript and its refresh; the
    /// workspace owns which run is on screen, and the page's back control
    /// returns the column to the conversation.
    fn render_subagent_stream_card(
        &mut self,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        use gpui::{ParentElement as _, Styled as _, div};
        let page = self.subagent_stream.as_ref()?;
        Some(
            div()
                .id("subagent-stream-main")
                .debug_selector(|| "subagent-stream-main".into())
                .size_full()
                .min_w_0()
                .min_h_0()
                .child(page.view.clone()),
        )
    }

    fn render_column(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        // The column entity derives the gate and prunes it; this frame only
        // mounts it.
        super::column_view::prune_history_gate(&self.chat, cx);
        let column_view = self.ensure_column_view(cx);
        // The page is built in the column's own entity (its state has one
        // owner). Nested entity elements measure their contents, so a
        // percentage-sized page would collapse — the frame takes the element
        // itself and lays it out as a flex item.
        let page: gpui::AnyElement =
            column_view.update(cx, |view, cx| view.render_page(window, cx));
        let main_body_w = self
            .chat
            .read(cx)
            .card_width
            .get()
            .unwrap_or_else(|| window.bounds().size.width);
        let turn_navigator_overlay = self.render_turn_navigator_overlay(&theme, main_body_w, cx);
        // The card's interior width — what this view was actually laid out in
        // (the shell's sidebar and right pane are siblings, so the window
        // width overstates it). The measurement lands at prepaint; the first
        // frame of a fresh window falls back to the window width and the
        // prepaint below schedules the frame that corrects it.
        let card_width = self.chat.read(cx).card_width.clone();
        let root = gpui::div()
            .id("embedded-column")
            .size_full()
            .flex()
            // The column is its own entity; a cached view element is laid out
            // at the card's size (an uncached child measures its contents, and
            // the page's percentage sizing resolves to zero that way).
            .child(
                gpui::div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .overflow_hidden()
                    .child(page),
            )
            .children(turn_navigator_overlay)
            // A zero-size canvas is the paint-phase hook this registration
            // needs (window listeners may only be installed while painting).
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    |_, _, window, _| {
                        // A popup menu's click stops propagation, so the
                        // selection engine never sees the mouse-up that would
                        // end the drag its mouse-down armed; without the guard
                        // the pointer's next button-less travel across the
                        // transcript selects whatever it crosses.
                        steer_components::markdown::install_selection_drag_guard(window);
                    },
                )
                .absolute()
                .top_0()
                .left_0(),
            )
            .on_prepaint(move |bounds, window, _app| {
                if card_width.set(bounds.size.width) {
                    window.refresh();
                }
            });
        self.apply_chat_actions(root, window, cx)
    }
}
