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
        // The render pass is the only place a `Window` is in hand, so the
        // composer's blank-project input is maintained here.
        self.ensure_blank_project_input(window, cx);
        // Settings is a main-column swap: the card hosts the settings nav +
        // panel until the back control exits. The nav lives inside the card
        // because the chrome sidebar slot is the session list.
        if matches!(self.view_mode, ViewMode::Settings) && !self.exiting_settings {
            return self.render_settings_card(window, cx);
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

    /// Settings inside the card: the upstream `Settings` shell fills the area
    /// below the TitleBar strip; the strip spans the whole card as a drag
    /// region and carries the back control at its left edge, over the sidebar
    /// column (the chrome sidebar slot belongs to the session list, so the
    /// whole settings surface — nav included — lives inside the card).
    fn render_settings_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use gpui::{ParentElement as _, Styled as _, div};
        use gpui_component::{TITLE_BAR_HEIGHT, TitleBar, h_flex, v_flex};

        use crate::views::management_shell::back_control;
        let settings = self
            .settings_view
            .as_ref()
            .expect("enter_settings must have created the SettingsView")
            .clone();
        let back = back_control(&cx.theme().clone(), i18n::t("settings-back"), {
            let settings = settings.clone();
            move |_ev, _window, cx| {
                settings.update(cx, |_, cx| cx.emit(SettingsEvent::Exit));
            }
        });
        let content = settings.update(cx, |s, cx| s.render(window, cx));
        let root = v_flex()
            .id("embedded-settings")
            .size_full()
            .min_w_0()
            .relative()
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .pt(TITLE_BAR_HEIGHT)
                    .child(content),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(TITLE_BAR_HEIGHT)
                    .child(TitleBar::new().child(h_flex().px_2().child(back))),
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

    /// The conversation column — the chrome card's main surface, bare of the
    /// shell furniture (gutter/sidebar/card/title bar all belong to the
    /// chrome shell around it). Hero-or-list, footer composer, the floating
    /// context rail, the blank-project overlay, and the turn-navigator
    /// overlay.
    fn render_column(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        use gpui_component::{h_flex, v_flex};
        let theme = cx.theme().clone();

        // History-loading gate: a reopened thread whose chat snapshot has not
        // landed yet (the fold still holds no chat channel) suppresses the
        // hero / list / footer in favor of the meerkat loading page. Render
        // re-checks the fold so a stale flag can never pin the page after the
        // snapshot, and prunes the flag once the wait outlives the timeout so
        // a failed reopen degrades to the hero screen.
        self.prune_history_gate(cx);
        let history_loading = self.chat.read(cx).awaiting_history.is_some()
            && self
                .chat
                .read(cx)
                .store
                .as_ref()
                .map(|(store, sid)| {
                    crate::ahp_store::leaf(&store.read(cx).book, sid.as_str())
                        .chat
                        .is_none()
                })
                .unwrap_or(false);
        let thread_id = if history_loading {
            self.chat.read(cx).thread.read(|t| t.id.0.clone())
        } else {
            String::new()
        };

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
        // Left-edge turn rail: the user-turn anchors are re-derived every
        // frame (the same projection reads the ⌘M navigator makes on open).
        // The width gate is a constant compare, so it short-circuits BEFORE
        // the projection — a narrow card pays nothing. The turn-count gate
        // lives inside `render_turn_rail` (None below two turns), whose
        // result is the single source for both the rail and the gutter, so
        // the two can never fork. Independent of the context rail's own
        // gate: either side can float alone.
        let turn_rail = if main_body_w >= px(crate::views::turn_rail::MIN_CARD_WIDTH) {
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
            let workspace = cx.entity();
            let on_jump: crate::views::turn_rail::JumpFn =
                std::rc::Rc::new(move |item_ix, _window, cx| {
                    workspace.update(cx, |workspace, cx| workspace.reveal_message(item_ix, cx));
                });
            crate::views::turn_rail::render_turn_rail(&theme, &self.chat, rail_turns, on_jump, cx)
        } else {
            None
        };
        let show_turn_rail = turn_rail.is_some();
        let overlay = self.render_blank_project_overlay(window, &theme, cx);
        let turn_navigator_overlay = self.render_turn_navigator_overlay(&theme, main_body_w, cx);

        let footer =
            (composer_placement == ComposerPlacement::Footer && !history_loading).then(|| {
                v_flex()
                    .w_full()
                    .flex_shrink_0()
                    .bg(theme.background)
                    .py_2()
                    .gap_2()
                    .child(centered(gpui::div().w_full().h(px(1.)).bg(theme.border)))
                    .when_some(
                        self.render_captain_strip(running, &theme, cx),
                        |this, strip| this.child(centered(strip)),
                    )
                    .children(self.render_attachments(&theme, cx))
                    .child(centered(self.render_composer(running, window, &theme, cx)))
            });
        let hero = if composer_placement != ComposerPlacement::Hero || history_loading {
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
                    .children(self.render_follow_stop_banner(&theme, cx))
                    .children(hero)
                    .when(history_loading, |this| {
                        this.child(crate::views::history_loading::render_history_loading(
                            &theme, &thread_id,
                        ))
                    })
                    .children({
                        // The row factory is a pure read-only projection
                        // over the conversation.
                        let conversation = self.chat.read(cx).conversation.clone();
                        let diag_enabled = crate::overlap_diag::enabled();
                        let processor = move |ix: usize, _window: &mut Window, cx: &mut App| {
                            // A folded whole-turn process head hides the
                            // turn's segments (the reply is exempt) — the
                            // row renders empty instead.
                            if conversation.read(cx).hidden_by_turn_process(ix, cx) {
                                return v_flex().w_full().min_w_0().into_any_element();
                            }
                            let item = conversation.read(cx).items().get(ix).cloned();
                            match item {
                                Some(item) => {
                                    if diag_enabled {
                                        crate::overlap_diag::record_mapping(
                                            ix,
                                            item.read(cx).diagnostic_id(),
                                        );
                                    }
                                    // A reply immediately followed by an
                                    // activity segment tightens its bottom
                                    // gap: the segment's own header padding
                                    // breathes, so the generic 16px reads as
                                    // a hole (the reverse direction keeps it
                                    // — a reply needs its air).
                                    let tight_gap = conversation.read(cx).tight_gap_after(ix, cx);
                                    v_flex()
                                        .w_full()
                                        .pt_1()
                                        .when(tight_gap, |this| this.pb_1())
                                        .when(!tight_gap, |this| this.pb_4())
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
                        (!first_screen && !history_loading).then(move || {
                            let list_el = gpui::list(list_state, processor)
                                .w_full()
                                .h_full()
                                .min_h_0()
                                .min_w_0();
                            let list_wrap = v_flex()
                                .debug_selector(|| "workspace-message-list".into())
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
                                // h_flex presets items_center, which centers
                                // the children vertically and collapses the
                                // list's h_full to zero — the transcript band
                                // must stretch them instead.
                                .items_stretch()
                                // The gutter pads the list itself, not this
                                // band: the rail's absolute anchor is the
                                // band, so padding the band would drag the
                                // rail right along with the text it must
                                // clear (the ticks would land ON the
                                // transcript instead of hugging the edge).
                                .child(
                                    h_flex()
                                        .flex_1()
                                        .w_full()
                                        .min_h_0()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .items_stretch()
                                        .when(show_turn_rail, |this| {
                                            this.pl(px(crate::views::turn_rail::GUTTER))
                                        })
                                        .child(list_wrap),
                                )
                                // The rail paints after the list so its marks
                                // and preview float over the transcript band.
                                .children(turn_rail)
                        })
                    })
                    .children(footer)
                    .children(overlay),
            );
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

    /// The Captain status block above the composer — the Captain's own row
    /// (`Captain 正在驾驶{provider}/{model}·{elapsed}·{activity}`) while a turn
    /// runs, followed by one row per RUNNING sub-agent, each led by a
    /// PixelSpinner flush at the left edge. Every segment is tight (no spacer
    /// between the verb, the model, the clock and the activity). The model
    /// segment carries the app's wire-color convention (the registration's
    /// `-wire` suffix never shows as text), and the block retires once neither
    /// a turn nor a run is live — the message column carries no per-entry
    /// status glyphs, this is the one live surface.
    ///
    /// The sub-agent rows are the parent chat's AHP background work (the host
    /// opens a run's entry on its first running tick and removes it when the
    /// run settles), so they stay listed while their run does — including a
    /// run that outlived the turn that spawned it (AHP: "an entry may have been
    /// started by an earlier turn"). A sub-agent row reads
    /// `Explore(task)正在驾驶{model}·{elapsed}`. A row's click opens that
    /// child's own transcript in the main column; the round trip back is the
    /// page's own back control (the block is not on screen while the page owns
    /// the column, so it carries no click of its own).
    fn render_captain_strip(
        &self,
        running: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        use ai_elements::{PixelSpinner, PixelSpinnerVariant};
        use steer_agent_chat_ui::conversation::LiveActivity;
        use steer_agent_chat_ui::model_tag::ModelTag;
        use steer_agent_chat_ui::text::one_line;
        use steer_agent_chat_ui::views::subagents::{CAPTAIN_LABEL, task_display_title};

        let subagents = self.running_subagents(cx);
        if !running && subagents.is_empty() {
            return None;
        }
        let tag = ModelTag::resolve(&self.model_label(cx));

        let muted = theme.muted_foreground;
        let spinner = || {
            // The 16px frame is fixed: shrink 0 keeps a tight row from
            // squeezing the glyph (the status line's own flex base is large).
            gpui::div()
                .flex_shrink_0()
                .child(
                    PixelSpinner::new()
                        .variant(PixelSpinnerVariant::Grid)
                        .color(muted),
                )
                .into_any_element()
        };
        // The lead-in's chrome (prefix, separators, clock) is fixed: shrink 0
        // keeps taffy from spreading a long activity summary's overflow across
        // it. The model name and the activity are runtime text — an unresolved
        // id is the raw wire string, a tool summary an arbitrary title — so
        // both ellipsize rather than pushing the clock (or each other) out of
        // the row. A sub-agent row's `{type}({task})` prefix is variable-length
        // for the same reason and stays the row's shrinkable text.
        let model_spans = |theme: &Theme| {
            tag.spans(theme)
                .into_iter()
                .map(|(text, color)| {
                    gpui::div()
                        .min_w_0()
                        .truncate()
                        .text_color(color)
                        .child(text)
                })
                .collect::<Vec<_>>()
        };
        let separator = || gpui::div().flex_shrink_0().child("·");
        let clock = |secs: i64| {
            gpui::div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .child(crate::i18n::t_count("thinking-duration", secs))
        };
        // One tight segment line: name + verb, then the model, the clock and
        // the activity sit flush against each other (no flex gap). Only the
        // spinner keeps its own gutter.
        let status_line = |name: gpui::Div, theme: &Theme| {
            gpui::div()
                .flex()
                .items_center()
                .min_w_0()
                .truncate()
                .child(name)
                .children(model_spans(theme))
        };

        let mut rows = v_flex().w_full().min_w_0().gap_1();
        // The Captain's own row names a model; a session with none registered
        // still lists its sub-agent rows rather than dropping the block.
        if running && !tag.is_empty() {
            let (activity, elapsed) = {
                let conversation = self.chat.read(cx).conversation.read(cx);
                (conversation.live_activity(cx), conversation.running_secs())
            };
            // "Captain" is product vocabulary, never translated; the verb
            // after it is chrome copy.
            let prefix = format!("{CAPTAIN_LABEL} {}", crate::i18n::t("steering-verb"));
            let mut status = status_line(
                gpui::div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .child(prefix),
                theme,
            );
            if elapsed > 0 {
                status = status.child(separator()).child(
                    clock(elapsed as i64).debug_selector(|| "workspace-captain-clock".into()),
                );
            }
            if let Some(activity) = activity {
                // Every arm yields the same way: a bare div's automatic
                // min-width would clamp it to its one-line text, so overflow
                // would clip it instead of ellipsizing at its own end.
                let summary = match &activity {
                    LiveActivity::Thinking => gpui::div()
                        .min_w_0()
                        .truncate()
                        .child(crate::i18n::t("captain-activity-thinking")),
                    LiveActivity::Replying => gpui::div()
                        .min_w_0()
                        .truncate()
                        .child(crate::i18n::t("captain-activity-replying")),
                    LiveActivity::Tool(title) => gpui::div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme.mono_font_family.clone())
                        .child(one_line(title)),
                };
                status = status
                    .child(separator())
                    .child(summary.debug_selector(|| "workspace-captain-activity".into()));
            }
            let row = gpui::div()
                .id("captain-strip")
                .debug_selector(|| "workspace-captain-strip".into())
                .flex()
                .items_center()
                .gap_2()
                .w_full()
                .min_w_0()
                .text_sm()
                .text_color(muted)
                .child(spinner())
                .child(status);
            rows = rows.child(row);
        }

        for (ix, work) in subagents.into_iter().enumerate() {
            // The run's identity: `Explore(task)`. The background-work entry
            // carries the topic; the agent type is resolved from the spawning
            // call. Until that resolves, the entry's own label keeps its old
            // `label · topic` reading rather than inventing a type.
            let topic = work
                .activity
                .as_deref()
                .filter(|topic| !topic.is_empty())
                .unwrap_or(&work.label);
            let title = match work.kind.as_deref().filter(|kind| !kind.is_empty()) {
                Some(kind) => task_display_title(kind, topic),
                None => {
                    task_display_title(&work.label, work.activity.as_deref().unwrap_or_default())
                }
            }
            .unwrap_or_else(|| work.id.clone());
            let title = one_line(&title);
            let mut status = status_line(
                gpui::div()
                    .min_w_0()
                    .truncate()
                    .whitespace_nowrap()
                    .child(crate::i18n::t_str(
                        "subagent-steering-prefix",
                        &[("name", title.as_str())],
                    )),
                theme,
            );
            if let Some(secs) = elapsed_secs(&work.started_at) {
                status = status.child(separator()).child(clock(secs));
            }
            // The model segment names the session's model: a sub-agent run on
            // this harness inherits the parent's model unless its own
            // definition pins one, and the run's own model has no AHP seat on
            // the parent's channel.
            let row = gpui::div()
                .id(("captain-subagent-row", ix))
                .debug_selector(move || format!("workspace-subagent-strip-{ix}"))
                .flex()
                .items_center()
                .gap_2()
                .w_full()
                .min_w_0()
                .text_sm()
                .text_color(muted)
                .cursor_pointer()
                .hover(|s| s.bg(theme.secondary.opacity(0.4)))
                .child(spinner())
                .child(status)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_subagent_stream(&work, cx);
                }));
            rows = rows.child(row);
        }
        Some(rows.into_any_element())
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

/// Whole seconds since an ISO 8601 start stamp; `None` when the stamp is
/// absent/unparseable or the clock is in the future. The status strip's
/// elapses are wall-clock (the wire carries the start, not a local instant).
fn elapsed_secs(started_at: &str) -> Option<i64> {
    let started = chrono::DateTime::parse_from_rfc3339(started_at).ok()?;
    let secs = (chrono::Utc::now() - started.with_timezone(&chrono::Utc)).num_seconds();
    (secs > 0).then_some(secs)
}
