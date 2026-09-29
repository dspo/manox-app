//! The workspace render face: the `Render` impl and its builders — the
//! conversation column mounted as the chrome shell's main surface
//! (`render_column`), the in-card settings swap (`render_settings_card`),
//! the shared action decoration (`apply_chat_actions`), and the follow-stop
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
            self.open_thread(next, window, cx);
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
        // The render pass is the only place a `Window` is in hand, so the
        // composer's blank-project input is maintained here.
        self.ensure_blank_project_input(window, cx);
        // Settings is a main-column swap: the card hosts the settings nav +
        // panel until the back control exits. The nav lives inside the card
        // because the chrome sidebar slot is the session list.
        if matches!(self.view_mode, ViewMode::Settings) && !self.exiting_settings {
            return self.render_settings_card(window, cx);
        }
        // A blocking overlay owns the page: the turn navigator cannot stay
        // mounted under it.
        if self.blocking_overlay_active(cx) && self.chat.read(cx).turn_navigator.is_some() {
            self.close_turn_navigator(window, cx);
        }
        // Per-frame conversation maintenance, all of it needing `&mut Window`
        // (the ask card's per-question inputs) or draining a one-shot marker:
        // reconcile the local cards against the server projections, allocate
        // the missing ask inputs, sync the workspace-derived snapshot onto the
        // owning tool row, and announce cards retired on another client.
        self.reconcile_pending_with_projections(cx);
        self.ensure_ask_custom_inputs(window, cx);
        self.sync_ask_card_snapshots(cx);
        self.notice_settled_elsewhere(window, cx);
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
        .on_action(
            cx.listener(|this, _: &crate::ArchiveCurrentThread, window, cx| {
                this.archive_current_thread(window, cx);
            }),
        )
        .children(self.render_turn_navigator_overlay_placeholder(cx))
        .into_any_element()
    }

    /// Settings inside the card: the settings nav column + the selected
    /// panel, side by side (the shell's sidebar slot belongs to the session
    /// list, so the nav lives in here).
    fn render_settings_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use gpui::{ParentElement as _, Styled as _, div};
        use gpui_component::{h_flex, v_flex};
        let settings = self
            .settings_view
            .as_ref()
            .expect("enter_settings must have created the SettingsView")
            .clone();
        let nav = settings.update(cx, |s, cx| s.render_nav(window, cx));
        let main = settings.update(cx, |s, cx| s.render_main(window, cx));
        let root = h_flex()
            .id("embedded-settings")
            .size_full()
            .min_w_0()
            .child(
                v_flex()
                    .w(px(SETTINGS_NAV_WIDTH))
                    .h_full()
                    .flex_shrink_0()
                    .child(nav),
            )
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .flex_shrink_0()
                    .bg(cx.theme().border),
            )
            .child(v_flex().flex_1().min_w_0().h_full().child(main));
        self.apply_chat_actions(root, window, cx)
    }

    /// The conversation column — the chrome card's main surface, bare of the
    /// shell furniture (gutter/sidebar/card/title bar all belong to the
    /// chrome shell around it). Hero-or-list, footer composer, the floating
    /// context rail, the blank-project / pending-auth overlays, and the
    /// turn-navigator overlay.
    fn render_column(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        use gpui_component::{h_flex, v_flex};
        let theme = cx.theme().clone();

        let running = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).running()
            })
            .unwrap_or(false);
        let first_screen = self.chat_conversation(cx).read(cx).is_empty(cx) && !running;
        let composer_placement = composer_placement(first_screen);
        // The card's interior width — what this view was actually laid out
        // in. The window width is NOT a stand-in for it: the shell's sidebar
        // and right pane are siblings of the card, so it overstates the card
        // by however much they claim (an 1100px window with both open leaves
        // ~360px of card). The measurement lands at prepaint, so the first
        // frame of a fresh window falls back to the window width and the
        // prepaint below schedules the frame that corrects it.
        let card_width = self.chat.read(cx).card_width.clone();
        let main_body_w = card_width
            .get()
            .unwrap_or_else(|| window.bounds().size.width);
        let show_rail = !first_screen
            && self
                .chat
                .read(cx)
                .store
                .as_ref()
                .map(|(store, sid)| {
                    let view = store.read(cx);
                    crate::ahp_store::leaf(&view.book, sid).running()
                })
                .unwrap_or(false)
            && crate::views::context_rail::ContextRail::rail_width_for(main_body_w).is_some();
        // Left-edge turn rail: the user-turn anchors are re-derived every
        // frame (the same projection reads the ⌘M navigator makes on open),
        // and the rail mounts from two turns up — provided the card is wide
        // enough to spend the gutter on. Independent of the context rail's
        // own gate: either side can float alone.
        let rail_turns = crate::views::turn_rail::collect_rail_turns(
            self.chat
                .read(cx)
                .conversation
                .read(cx)
                .items()
                .iter()
                .enumerate()
                .map(|(ix, item)| (ix, item.read(cx).kind())),
        );
        let show_turn_rail =
            rail_turns.len() >= 2 && main_body_w >= px(crate::views::turn_rail::MIN_CARD_WIDTH);
        let turn_rail = if show_turn_rail {
            let workspace = cx.entity();
            let on_jump: crate::views::turn_rail::JumpFn =
                std::rc::Rc::new(move |item_ix, _window, cx| {
                    workspace.update(cx, |workspace, cx| workspace.reveal_message(item_ix, cx));
                });
            crate::views::turn_rail::render_turn_rail(&theme, &self.chat, rail_turns, on_jump, cx)
        } else {
            None
        };
        let overlay = self
            .render_blank_project_overlay(window, &theme, cx)
            .or_else(|| self.render_pending_auth_overlay(&theme, cx));
        let turn_navigator_overlay =
            self.render_turn_navigator_overlay(&theme, show_rail, main_body_w, cx);

        let footer = (composer_placement == ComposerPlacement::Footer).then(|| {
            v_flex()
                .w_full()
                .flex_shrink_0()
                .bg(theme.background)
                .py_2()
                .gap_2()
                .child(centered(gpui::div().w_full().h(px(1.)).bg(theme.border)))
                .children(self.render_attachments(&theme, cx))
                .child(centered(self.render_composer(running, window, &theme, cx)))
        });
        let hero = if composer_placement != ComposerPlacement::Hero {
            None
        } else {
            Some(
                v_flex()
                    .flex_1()
                    .w_full()
                    .justify_center()
                    .items_center()
                    .child(centered(
                        v_flex()
                            .w_full()
                            .gap_5()
                            .items_center()
                            .child(
                                gpui::div()
                                    .text_base()
                                    .font_weight(gpui::FontWeight::BLACK)
                                    .child(i18n::t("workspace-hero-heading")),
                            )
                            .children(self.render_attachments(&theme, cx))
                            .child(centered(self.render_composer(running, window, &theme, cx))),
                    )),
            )
        };

        let conversation_column = v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .relative()
            .overflow_hidden()
            .child(
                // The chrome card starts the content at its top edge, so
                // the body carries no title-bar inset.
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .w_full()
                    .overflow_hidden()
                    .pb_2()
                    .when(show_rail, |this| {
                        this.pr(px(crate::views::context_rail::ENV_CONTENT_INSET))
                    })
                    .when(show_turn_rail, |this| {
                        this.pl(px(crate::views::turn_rail::GUTTER))
                    })
                    .children(self.render_follow_stop_banner(&theme, cx))
                    .children(hero)
                    .children({
                        // The row factory is a pure read-only projection
                        // over the conversation.
                        let conversation = self.chat.read(cx).conversation.clone();
                        let diag_enabled = crate::overlap_diag::enabled();
                        let processor = move |ix: usize, _window: &mut Window, cx: &mut App| {
                            let item = conversation.read(cx).items().get(ix).cloned();
                            match item {
                                Some(item) => {
                                    if diag_enabled {
                                        crate::overlap_diag::record_mapping(
                                            ix,
                                            item.read(cx).diagnostic_id(),
                                        );
                                    }
                                    v_flex()
                                        .w_full()
                                        .pt_1()
                                        .pb_4()
                                        .flex_shrink_0()
                                        .min_w_0()
                                        .debug_selector(move || {
                                            format!("workspace-message-row-{ix}")
                                        })
                                        .when(diag_enabled, |this| {
                                            this.on_prepaint(move |bounds, _window, _cx| {
                                                crate::overlap_diag::record_row(ix, bounds);
                                            })
                                        })
                                        .child(item)
                                        .into_any_element()
                                }
                                None => gpui::div().into_any_element(),
                            }
                        };
                        let list_state = self.chat.read(cx).list_state.clone();
                        let width_state = self.chat.read(cx).list_state.clone();
                        let message_list_width = self.chat.read(cx).message_list_width.clone();
                        let diag_state = self.chat.read(cx).list_state.clone();
                        let mono_family = theme.mono_font_family.clone();
                        (!first_screen).then(move || {
                            let list_el = gpui::list(list_state, processor)
                                .w_full()
                                .h_full()
                                .min_h_0()
                                .min_w_0();
                            let list_wrap = v_flex()
                                .flex_1()
                                .h_full()
                                .min_h_0()
                                .min_w_0()
                                .font_family(mono_family.clone())
                                .font_weight(gpui::FontWeight::LIGHT)
                                .child(list_el)
                                .on_prepaint(move |bounds, window, _app| {
                                    if message_list_width.update(bounds.size.width, &width_state) {
                                        window.refresh();
                                    }
                                    if crate::overlap_diag::enabled() {
                                        crate::overlap_diag::check_completed_frame(
                                            bounds,
                                            diag_state.item_count(),
                                        );
                                    }
                                });
                            h_flex()
                                .flex_1()
                                .w_full()
                                .min_h_0()
                                .min_w_0()
                                .overflow_hidden()
                                .relative()
                                .child(list_wrap)
                                // The rail paints after the list so its marks
                                // and preview float over the transcript band.
                                .children(turn_rail)
                        })
                    })
                    .children(footer)
                    .children(overlay),
            )
            .when(show_rail, |this| {
                this.child(self.chat.read(cx).context_rail.clone())
            });
        let root = gpui::div()
            .id("embedded-column")
            .size_full()
            .flex()
            .child(conversation_column)
            .children(turn_navigator_overlay)
            .on_prepaint(move |bounds, window, _app| {
                if card_width.set(bounds.size.width) {
                    window.refresh();
                }
            });
        self.apply_chat_actions(root, window, cx)
    }
    /// §二.3 stop notice — the dismissible BROADCAST arm. Shown above the
    /// message area while the foreground leaf's reopen budget is exhausted
    /// AND not dismissed. It names what stopped (the reason copy is keyed
    /// by the leaf's observable cause) and carries the retry action, but
    /// it is not the only signal: the persistent projection (`Workspace::
    /// render_follow_stop_chip`, in the footer's composer chip group)
    /// stays visible regardless of dismissal. Dismissing this banner hides
    /// only the broadcast; the state and its retry entry survive in the
    /// footer, so a stuck lease — which never self-heals — can never leave
    /// the frozen transcript signal-less. Dismissal still belongs to the
    /// leaf (per session): the banner stays out until the stream resumes
    /// or a retry dies again.
    fn render_follow_stop_banner(
        &self,
        _theme: &Theme,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // The retry notice retired with the v2 follow stream: a failed turn
        // surfaces as the chat's error part now.
        None
    }
}
