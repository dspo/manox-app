//! The chip + interaction-card families (U9b cluster 6): the ask/auth
//! card surface (projection reconcile, snapshots, option toggle/prev/next,
//! resolve-with-response, the auth resolve leg), the model-selector pi
//! face (wire tag/color, the canonical identity/display resolvers, the
//! selector render + popup builder), and the goal chip (popover open,
//! the edit/budget/rounds/replace/new begins, the chip render). Split
//! from `workspace.rs` — `super` is the workspace module; the
//! bare-private methods lift `pub(super)` for the parent's render
//! handlers and the `tests` child.

use super::*;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::tag::{Tag, TagVariant};
use gpui_component::{ColorName, ThemeStyled as _};

/// The protocol id of an open input request.
fn request_id(r: &ahp_types::state::SessionInputRequest) -> &str {
    match r {
        ahp_types::state::SessionInputRequest::ChatInput(c) => &c.id,
        ahp_types::state::SessionInputRequest::ToolConfirmation(c) => &c.id,
        ahp_types::state::SessionInputRequest::ToolClientExecution(c) => &c.id,
        ahp_types::state::SessionInputRequest::ToolAuthentication(c) => &c.id,
        ahp_types::state::SessionInputRequest::Unknown(_) => "",
    }
}

/// The canonical effort string → the display enum.
fn parse_effort(raw: &str) -> Option<manox_agent::language_model::ReasoningEffort> {
    match raw {
        "high" => Some(manox_agent::language_model::ReasoningEffort::High),
        "max" => Some(manox_agent::language_model::ReasoningEffort::Max),
        _ => None,
    }
}

use gpui::Window;

impl Workspace {
    /// Remote-settle reconcile for a surfaced interaction card. The leaf's
    /// `pending_auth` projection is the gateway's authoritative pending
    /// view: an id that was confirmed in it and then vanished settled
    /// elsewhere (timeout expiry, another owner's answer, a cancel discard)
    /// and the local card must not latch onto the dead interaction.
    ///
    /// Arming on the FIRST confirmed membership keeps the startup race safe:
    /// the `Request` frame can land before the projection fold reflects the
    /// park, and an id never yet confirmed must never be cleared for being
    /// absent. Re-surfacing after a thread switch needs no local memory
    /// either — the gateway replays unsettled adjudications to every joining
    /// owner (manox §D.6), so the wire itself is the restore path.
    pub(crate) fn reconcile_pending_with_projections(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .chat
            .read(cx)
            .pending_ask
            .as_ref()
            .map(|a| a.id.clone())
            .or_else(|| {
                self.chat
                    .read(cx)
                    .pending_auth
                    .as_ref()
                    .map(|a| a.id.clone())
            })
        else {
            self.chat.update(cx, |chat, cx| {
                chat.pending_projection_confirmed = false;
                cx.notify();
            });
            return;
        };
        // No leaf yet (attach in flight): nothing to reconcile against.
        if self.chat.read(cx).store.is_none() {
            return;
        }
        let live = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .is_some_and(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid)
                    .requests()
                    .iter()
                    .any(|r| request_id(r) == id)
            });
        if live {
            self.chat.update(cx, |chat, cx| {
                chat.pending_projection_confirmed = true;
                cx.notify();
            });
            return;
        }
        if !self.chat.read(cx).pending_projection_confirmed {
            return;
        }
        self.chat.update(cx, |chat, cx| {
            chat.pending_ask = None;
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.pending_auth = None;
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.pending_projection_confirmed = false;
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.ask_step = 0;
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.ask_transition_gen = chat.ask_transition_gen.wrapping_add(1);
            cx.notify();
        });
        self.reset_ask_custom(cx);
        // The settled call's MsgId has no live waiter left; dropping the
        // mapping keeps a stale card click from replying to a dead call.
        cx.notify();
    }

    /// PR-4: surface a transient "answered on another client" notice for any
    /// card the leaf retired via a `DeliveryCancelled` since the last frame,
    /// then drain the marker set (so the notice fires exactly once per remote
    /// settle). Runs on the render path where a `Window` is available for the
    /// notification surface; the card itself is cleared by
    /// [`Self::reconcile_pending_with_projections`] (the leaf dropped the id
    /// from the projection set).
    pub(crate) fn notice_settled_elsewhere(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let drained: Vec<String> = Vec::new();
        if !drained.is_empty() {
            window.push_notification(
                Notification::info(i18n::t("workspace-ask-settled-elsewhere")),
                cx,
            );
        }
    }

    pub(crate) fn resolve_auth(&mut self, decision: PermissionDecision, cx: &mut Context<Self>) {
        // The generic approval card's allow/deny leg. The question card's
        // close is NOT this path — it is `dismiss_ask` (B2-PR-3): a close is
        // "the user left to speak", never a rejection, and the two must not
        // share an exit (they used to both render `WrapperToolDenied`).
        let Some(auth) = self.chat.update(cx, |chat, cc| {
            let v = chat.pending_auth.take();
            cc.notify();
            v
        }) else {
            return;
        };
        let id = auth.id;
        let allow = matches!(decision, PermissionDecision::AllowOnce);
        if let Some((store, sid)) = self.chat.read(cx).store.clone() {
            let view = store.read(cx);
            if let Some((chat_id, turn_id, tool_call_id)) =
                crate::ahp_store::leaf(&view.book, &sid).confirmation(&id)
            {
                store.update(cx, |store, _| {
                    store.confirm_tool_call(&chat_id, &turn_id, &tool_call_id, allow);
                });
                cx.notify();
                return;
            }
        }
        cx.notify();
    }

    /// Close the pending question card without answering: reply with the
    /// Batch-1 dismissal marker (`{"dismissed": true}` on the wire,
    /// `AskUserQuestionDismissed` in-process) so the parked waterfall
    /// converges immediately and the model reads "the user left to speak"
    /// — neither a denial nor an empty answer. The generic approval card's
    /// allow/deny leg stays in `resolve_auth`; the two exits must not merge.
    // ── ask-family thin wrappers (state lives on ChatColumn; wire/orchestration
    // halves stay below) ────────────────────────────────────────────────
    pub(crate) fn ask_card_snapshot(&self, id: &str, cx: &App) -> Option<AskCardSnapshot> {
        self.chat.read(cx).ask_card_snapshot(id)
    }

    pub(super) fn pending_ask_has_selection(&self, cx: &App) -> bool {
        self.chat.read(cx).pending_ask_has_selection()
    }

    pub(crate) fn first_incomplete_ask_question(&self, cx: &App) -> Option<usize> {
        self.chat.read(cx).first_incomplete_ask_question()
    }

    pub(crate) fn toggle_ask_option(&mut self, qi: usize, oi: usize, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.toggle_ask_option(qi, oi);
            cc.notify();
        });
    }

    pub(crate) fn decide_ask_option(&mut self, qi: usize, oi: usize, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.decide_ask_option(qi, oi);
            cc.notify();
        });
        self.resolve_ask(cx);
    }

    pub(crate) fn ask_prev(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.ask_prev();
            cc.notify();
        });
    }

    pub(crate) fn ask_next(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.ask_next();
            cc.notify();
        });
    }

    pub(super) fn reset_ask_custom(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.reset_ask_custom();
            cc.notify();
        });
    }

    pub(crate) fn dismiss_ask(&mut self, cx: &mut Context<Self>) {
        let ask = match self.chat.update(cx, |chat, cc| {
            let v = chat.take_pending_ask();
            cc.notify();
            v
        }) {
            Some(a) => a,
            None => return,
        };
        self.reset_ask_custom(cx);
        if let Some((store, sid)) = self.chat.read(cx).store.clone() {
            let view = store.read(cx);
            if let Some((chat_id, _request)) =
                crate::ahp_store::leaf(&view.book, &sid).chat_input(&ask.id)
            {
                let request_id = _request.id.clone();
                // A close is "the user left to speak", never a rejection —
                // on the wire it is a Decline, which the engine journals as
                // the question's `dismissed` verdict (the fold maps that
                // verdict right back to Decline).
                store.update(cx, |store, _| {
                    store.decline_input(&chat_id, &request_id);
                });
                cx.notify();
                return;
            }
        }
        cx.notify();
    }

    /// Diagnostic-only: the interactive ask card element for the CURRENT
    /// pending-ask snapshot, as the conversation list would render it. The
    /// workspace's message list only paints through the live event path, so
    /// render-level probe tests pull the card out and mount it directly.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_ask_card_element(
        &self,
        host: manox_agent_chat_ui::host::ChatHostHandle,
        ix: usize,
        cx: &mut App,
    ) -> Option<gpui::AnyElement> {
        let id = self.chat.read(cx).pending_ask.as_ref()?.id.clone();
        let snapshot = self.ask_card_snapshot(&id, cx)?;
        let item = crate::conversation::ToolCallItem {
            id,
            name: manox_agent::tools::ASK_USER_QUESTION.to_string(),
            title: String::new(),
            status: manox_agent::ToolCallStatus::PendingApproval,
            output: String::new(),
            is_error: false,
            input: serde_json::Value::Null,
            streaming: false,
            collapsed: false,
            user_toggled: false,
            panel: None,
        };
        // The diagnostic mount shows the card's controls but nothing copies
        // from it, so the copy-feedback context is ownerless.
        let copy_registry = manox_components::copy_feedback::CopiedRegistry::default();
        Some(crate::views::message::render_ask_user_card(
            &item,
            ix,
            &cx.theme().clone(),
            Some(&crate::views::message::ToolCallCtx {
                host,
                ask: Some(snapshot),
            }),
            &crate::views::message::CopyFeedback::inert(&copy_registry),
            cx,
        ))
    }

    /// Synchronize Workspace-derived ask state before the native list starts
    /// measuring. Updating a `MessageItem` from the row callback dirties the
    /// same entity tree whose height GPUI is currently caching, which can make
    /// the cached row and the painted subtree describe different layouts.
    pub(super) fn sync_ask_card_snapshots(&mut self, cx: &mut App) {
        let next = self.chat.read(cx).pending_ask.as_ref().and_then(|ask| {
            let snapshot = self.ask_card_snapshot(&ask.id, cx)?;
            let ix = self.chat_conversation(cx).read(cx).find_tool(&ask.id, cx)?;
            let item = self.chat_conversation(cx).read(cx).items().get(ix)?.clone();
            Some((item, snapshot))
        });

        if let Some(previous) = self.chat.update(cx, |chat, cc| {
            let v = chat.ask_snapshot_item.take();
            cc.notify();
            v
        }) && next
            .as_ref()
            .is_none_or(|(current, _)| current != &previous)
            && previous.read(cx).ask_snapshot.is_some()
        {
            previous.update(cx, |item, cx| {
                item.ask_snapshot = None;
                cx.notify();
            });
        }

        if let Some((item, snapshot)) = next {
            if item.read(cx).ask_snapshot.as_ref() != Some(&snapshot) {
                item.update(cx, |item, cx| {
                    item.ask_snapshot = Some(snapshot);
                    cx.notify();
                });
            }
            self.chat.update(cx, |chat, cx| {
                chat.ask_snapshot_item = Some(item);
                cx.notify();
            });
        }
    }

    pub(super) fn composer_can_submit(&self, running: bool, cx: &App) -> bool {
        // T10c: the v1 `history_phase` loading gate retired with the fold
        // (the restore boundary is now the §D.1 snapshot; a pending-snapshot
        // gate lands with the §K.5 closeout).
        if running {
            return true;
        }
        let input_empty = self.chat_input(cx).read(cx).value().trim().is_empty();
        if self.chat.read(cx).pending_ask.is_some() {
            !input_empty || self.pending_ask_has_selection(cx)
        } else {
            !input_empty || !self.chat.read(cx).pending_attachments.is_empty()
        }
    }

    /// Submit the ask drawer: fold each question's tri-state (its selected
    /// labels plus its own free-text `custom`) into canonical `AskAnswer` rows.
    /// A blank custom is `None`; a question with nothing selected and no custom
    /// settles as an explicit skip (`selected: []`, no `custom`) — the same
    /// tri-state the server's `fold_ask_answers` reads. There is no card-level
    /// free-text override: a card close is `dismiss_ask`, and per-question free
    /// text rides the answer's `custom`.
    pub(crate) fn resolve_ask(&mut self, cx: &mut Context<Self>) {
        let (ask, custom_texts) = match self.chat.update(cx, |chat, cc| {
            let ask = chat.take_pending_ask();
            let texts = chat.ask_custom_text.clone();
            cc.notify();
            (ask, texts)
        }) {
            (Some(a), t) => (a, t),
            (None, _) => return,
        };
        // The wire answer per question: the selected option(s) (a select
        // question), the free-form text (a text question or a select's
        // freeform input). A question with neither carries no answer.
        let mut answers: std::collections::HashMap<String, ahp_types::state::ChatInputAnswer> =
            std::collections::HashMap::new();
        for (i, q) in ask.questions.iter().enumerate() {
            let sel = ask.selections.get(i).map(|s| s.as_slice()).unwrap_or(&[]);
            let selected: Vec<String> = q
                .options
                .iter()
                .zip(sel.iter())
                .filter_map(|(o, &s)| s.then_some(o.label.clone()))
                .collect();
            let custom = custom_texts
                .get(i)
                .filter(|s| !s.trim().is_empty())
                .cloned();
            let value = if q.multi_select {
                ahp_types::state::ChatInputAnswerValue::SelectedMany(
                    ahp_types::state::ChatInputSelectedManyAnswerValue {
                        value: selected,
                        freeform_values: custom.map(|c| vec![c]),
                    },
                )
            } else if let Some(first) = selected.first() {
                ahp_types::state::ChatInputAnswerValue::Selected(
                    ahp_types::state::ChatInputSelectedAnswerValue {
                        value: first.clone(),
                        freeform_values: custom.map(|c| vec![c]),
                    },
                )
            } else if let Some(text) = custom {
                ahp_types::state::ChatInputAnswerValue::Text(
                    ahp_types::state::ChatInputTextAnswerValue { value: text },
                )
            } else {
                continue;
            };
            answers.insert(
                q.id.clone(),
                ahp_types::state::ChatInputAnswer::Submitted(ahp_types::state::ChatInputAnswered {
                    value,
                }),
            );
        }
        let id = ask.id.clone();
        self.chat.update(cx, |chat, cx| {
            chat.ask_transition_gen = chat.ask_transition_gen.wrapping_add(1);
            cx.notify();
        });
        self.reset_ask_custom(cx);
        if let Some((store, sid)) = self.chat.read(cx).store.clone() {
            let view = store.read(cx);
            if let Some((chat_id, request)) =
                crate::ahp_store::leaf(&view.book, &sid).chat_input(&id)
            {
                let request_id = request.id.clone();
                store.update(cx, |store, _| {
                    store.complete_input(&chat_id, &request_id, answers);
                });
                cx.notify();
                return;
            }
        }
        cx.notify();
    }

    /// Align the per-question custom-answer scratch with the current ask:
    /// reset lengths to `questions.len()` (on re-seed / count change) and
    /// create each `custom` `InputState` lazily — allocation needs a `Window`,
    /// which the park event handler lacks, so this runs on the render path.
    /// Each input's `Change` repaints the card (driving the skip affordance and
    /// the submit gate) and mirrors its live text into `ask_custom_text`.
    pub(crate) fn ensure_ask_custom_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self
            .chat
            .read(cx)
            .pending_ask
            .as_ref()
            .map_or(0, |ask| ask.questions.len());
        if self.chat.read(cx).pending_ask.is_none() {
            if !self.chat.read(cx).ask_custom_inputs.is_empty() {
                self.reset_ask_custom(cx);
            }
            return;
        }
        if self.chat.read(cx).ask_custom_text.len() != count
            || self.chat.read(cx).ask_custom_inputs.len() != count
            || self.chat.read(cx).ask_skipped.len() != count
            || self.chat.read(cx).ask_body_scroll.len() != count
        {
            self.reset_ask_custom(cx);
            self.chat.update(cx, |chat, cc| {
                chat.ask_custom_text = vec![String::new(); count];
                chat.ask_skipped = vec![false; count];
                chat.ask_custom_inputs = vec![None; count];
                chat.ask_body_scroll = (0..count).map(|_| ScrollHandle::new()).collect();
                cc.notify();
            });
        }
        for qi in 0..count {
            if self.chat.read(cx).ask_custom_inputs[qi].is_none() {
                let initial = self
                    .chat
                    .read(cx)
                    .ask_custom_text
                    .get(qi)
                    .cloned()
                    .filter(|s| !s.is_empty());
                let state: Entity<InputState> = cx.new(|cx| {
                    let mut st = InputState::new(window, cx)
                        .placeholder(i18n::t("workspace-ask-supplement-placeholder"));
                    if let Some(initial) = initial {
                        st.set_value(initial, window, cx);
                    }
                    st
                });
                let sub = cx.subscribe(&state, move |this, state, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let val = state.read(cx).value().to_string();
                        this.chat.update(cx, |chat, cc| {
                            if let Some(slot) = chat.ask_custom_text.get_mut(qi) {
                                *slot = val;
                            }
                            cc.notify();
                        });
                        cx.notify();
                    }
                });
                self.chat.update(cx, |chat, cc| {
                    chat.ask_custom_inputs[qi] = Some(state);
                    chat.ask_custom_subs.push(sub);
                    cc.notify();
                });
            }
        }
    }

    /// The ask card body's tracked scroll handle for question `qi`. `None`
    /// before the ask scratch is allocated (the render path allocates it); the
    /// card then keeps gpui's untracked element-state scroll and takes no wheel
    /// guard — the pre-fix behaviour, never a broken body. The chat crate's
    /// message views read this through the `ChatHost` port
    /// (`contain_body_scroll`).
    pub(crate) fn ask_body_scroll(&self, cx: &App, qi: usize) -> Option<ScrollHandle> {
        self.chat.read(cx).ask_body_scroll.get(qi).cloned()
    }

    /// Skip question `qi` (deepseek `QuestionFlow.skipQuestion` semantics):
    /// clear its selection and its `custom` text and mark it EXPLICITLY
    /// skipped — the settled answer is the canonical explicit skip
    /// (`selected: []`, no `custom`), never a card dismissal — then either
    /// advance the walk or, on the last question, settle the whole card. The
    /// settle runs the same completeness gate as the submit: questions that
    /// were never touched jump the walk back instead of silently folding to
    /// skips. A skip can never strand the user: the walk always moves.
    pub(crate) fn skip_ask_question(
        &mut self,
        qi: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let walk = self.chat.update(cx, |chat, cc| {
            let v = chat.skip_ask_question_state(qi);
            cc.notify();
            v
        });
        if let Some(state) = self
            .chat
            .read(cx)
            .ask_custom_inputs
            .get(qi)
            .and_then(|slot| slot.clone())
        {
            state.update(cx, |st, cx| st.set_value("", window, cx));
        }
        // `walk` is Some(target) to advance to, None to settle the card.
        match walk {
            Some(target) => {
                self.chat.update(cx, |chat, cc| {
                    chat.ask_step = target;
                    cc.notify();
                });
                cx.notify();
            }
            None => self.resolve_ask(cx),
        }
    }

    /// Wire api string → Tag variant + label for the pi model menu.
    /// Wire api string → text color for the pi composer model label and the
    /// context rail's per-model usage rows. Tinted directly from theme tokens
    /// (matching `mode_chip_visual` and the settings panel) so both surfaces
    /// The foreground session's model identity from the `model` projection:
    /// the canonical `{provider, modelId}` wire identity (L8) — never a full
    /// `Model` blob. Deserializing the projection into `Model` (an earlier
    /// iteration) always failed on the missing display fields, so the chip
    /// rendered "no model" no matter what the journal said.
    pub(crate) fn foreground_model_identity(&self, cx: &App) -> Option<(String, String)> {
        let (store, sid) = self.chat.read(cx).store.clone()?;
        let view = store.read(cx);
        let model = crate::ahp_store::leaf(&view.book, &sid)
            .model_id()?
            .to_string();
        let (provider, id) = model.split_once('/')?;
        (!provider.is_empty() && !id.is_empty()).then(|| (provider.to_string(), id.to_string()))
    }

    /// Exact registration match of a canonical model identity against the
    /// gateway's model-registry snapshot (U2 display resolution): the chip
    /// and the picker menu read the wire `ModelInfo` the server projects, so
    /// a remote server's registry drives the desktop display. A stale id
    /// resolves to `None` and the caller renders the raw identity, never a
    /// fuzzy look-alike.
    pub(crate) fn resolve_model_display(
        &self,
        provider: &str,
        id: &str,
        _cx: &App,
    ) -> Option<(String, crate::model_catalog::ModelRow)> {
        // Display resolution rides the in-process provider registry (the
        // same source the streaming side matches against); the AHP root
        // catalogue serves remote clients.
        crate::model_catalog::resolve(provider, id).map(|row| (row.provider_display.clone(), row))
    }

    /// The pi-harness model selector. Reads the gateway's model-registry
    /// snapshot (U2 — the server projects the pi registry the streaming side
    /// resolves against): closed, a ghost button showing
    /// `provider · model · effort` with the model name tinted by wire api;
    /// open, a PopupMenu of provider submenus.
    pub(super) fn render_model_selector_pi(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.chat.read(cx).model_open;
        let model_identity = self.foreground_model_identity(cx);
        let model = model_identity
            .as_ref()
            .and_then(|(provider, id)| self.resolve_model_display(provider, id, cx));
        let effort = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid)
                    .reasoning_effort()
                    .and_then(parse_effort)
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        let trigger = h_flex()
            .id("model-trigger")
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .hover(|s| s.bg(theme.accent.opacity(0.08)))
            .cursor_pointer()
            .children(if let Some((ref prov_display, ref m)) = model {
                let (_, _, color_name) = Self::wire_visual(&m.api);
                let model_color = color_name.scale(500);
                let dot = || {
                    gpui::div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("·")
                };
                vec![
                    gpui::div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(prov_display.clone())
                        .into_any_element(),
                    dot().into_any_element(),
                    gpui::div()
                        .text_xs()
                        .text_color(model_color)
                        .child(m.name.clone())
                        .into_any_element(),
                    dot().into_any_element(),
                    gpui::div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(effort.wire_value().to_string())
                        .into_any_element(),
                ]
            } else if let Some((ref provider, ref id)) = model_identity {
                // The journal identity no longer resolves against the
                // gateway's model snapshot (provider/model deregistered) —
                // render it raw so the chip still tells the truth instead
                // of "no model".
                vec![
                    gpui::div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(provider.clone())
                        .into_any_element(),
                    gpui::div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("·")
                        .into_any_element(),
                    gpui::div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(id.clone())
                        .into_any_element(),
                    gpui::div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("·")
                        .into_any_element(),
                    gpui::div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(effort.wire_value().to_string())
                        .into_any_element(),
                ]
            } else {
                vec![
                    gpui::div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(i18n::t("workspace-no-model").to_string())
                        .into_any_element(),
                ]
            })
            .child(
                Icon::new(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .xsmall()
                .text_color(theme.muted_foreground),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                if this.chat.read(cx).model_open {
                    this.chat.update(cx, |chat, cx| {
                        chat.model_open = false;
                        cx.notify();
                    });
                    this.chat.update(cx, |chat, cx| {
                        chat.model_menu = None;
                        cx.notify();
                    });
                    this.chat.update(cx, |chat, cx| {
                        chat.model_menu_sub = None;
                        cx.notify();
                    });
                } else {
                    this.chat.update(cx, |chat, cx| {
                        chat.model_open = true;
                        cx.notify();
                    });
                    let current_effort = this
                        .chat
                        .read(cx)
                        .store
                        .as_ref()
                        .and_then(|(store, sid)| {
                            let view = store.read(cx);
                            crate::ahp_store::leaf(&view.book, sid)
                                .reasoning_effort()
                                .map(str::to_string)
                        })
                        .and_then(|e| parse_effort(&e))
                        .unwrap_or_else(|| {
                            tracing::debug!(
                                "foreground store not bound yet (ahp handshake in flight)"
                            );
                            Default::default()
                        });
                    let workspace = cx.entity().downgrade();
                    // U2: the menu lists the gateway's model-registry
                    // snapshot (the server projects the same pi registry the
                    // streaming side resolves against). The open also
                    // re-pulls, so a settings-side provider reload (no
                    // server push yet — §D.5 cross-domain ask) converges by
                    // the next open at the latest.
                    let models = crate::model_catalog::rows();
                    let menu = PopupMenu::build(window, cx, |menu, window, cx| {
                        Self::build_model_popup_menu_pi(
                            menu,
                            workspace,
                            models,
                            current_effort,
                            window,
                            cx,
                        )
                    });
                    let sub = cx.subscribe(
                        &menu,
                        |this: &mut Workspace,
                         _menu: Entity<PopupMenu>,
                         _: &DismissEvent,
                         cx: &mut Context<Workspace>| {
                            this.chat.update(cx, |chat, cx| {
                                chat.model_open = false;
                                cx.notify();
                            });
                            this.chat.update(cx, |chat, cx| {
                                chat.model_menu = None;
                                cx.notify();
                            });
                            this.chat.update(cx, |chat, cx| {
                                chat.model_menu_sub = None;
                                cx.notify();
                            });
                            cx.notify();
                        },
                    );
                    this.chat.update(cx, |chat, cx| {
                        chat.model_menu = Some(menu);
                        cx.notify();
                    });
                    this.chat.update(cx, |chat, cx| {
                        chat.model_menu_sub = Some(sub);
                        cx.notify();
                    });
                }
                cx.notify();
            }));

        if !open {
            return trigger.into_any_element();
        }

        let menu = self
            .chat
            .read(cx)
            .model_menu
            .clone()
            .expect("model_menu exists when open");

        gpui::div()
            .relative()
            .child(trigger)
            .child(
                deferred(
                    gpui::div()
                        .id("model-dropdown")
                        .absolute()
                        .bottom_full()
                        .right_0()
                        .occlude()
                        .child(menu),
                )
                .with_priority(1),
            )
            .into_any_element()
    }

    /// Model menu for the pi harness: grouped by provider display name;
    /// each row shows a wire-api Tag and selects through the gateway. U2:
    /// the rows are the server's wire `ModelInfo` snapshot (already deduped
    /// per registration at the source); a config model registered through
    /// several wire apis appears once per wire endpoint (registration names
    /// differ), so the responses and completions variants stay selectable
    /// alongside the anthropic one.
    /// The one wire-api visual mapping: the menu row's tag and the chip
    /// echo's text color read the same source, so the surfaces cannot
    /// drift apart.
    pub(crate) fn wire_visual(api: &str) -> (TagVariant, &'static str, gpui_component::ColorName) {
        match api {
            "anthropic" => (
                TagVariant::Color(ColorName::Blue),
                "Anthropic",
                ColorName::Blue,
            ),
            "openai_responses" => (
                TagVariant::Color(ColorName::Cyan),
                "Responses",
                ColorName::Cyan,
            ),
            "openai_completions" => (
                TagVariant::Color(ColorName::Amber),
                "Completions",
                ColorName::Amber,
            ),
            _ => (TagVariant::Secondary, "N/A", ColorName::Gray),
        }
    }

    pub(super) fn build_model_popup_menu_pi(
        menu: PopupMenu,
        workspace: WeakEntity<Workspace>,
        models: Vec<crate::model_catalog::ModelRow>,
        current_effort: manox_agent::language_model::ReasoningEffort,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        // Group by DISPLAY name via lookup (not adjacency): the snapshot is
        // sorted by registration name, so same-display-name providers with
        // different registrations must still merge into one submenu.
        // One submenu per agent registration; AHP's root catalogue carries
        // the provider identity the v2 wire list flattened.
        // Group by the provider's display name via lookup: wire variants of
        // one cx config merge into a single submenu, each row carrying its
        // own wire tag.
        let mut providers: Vec<(String, Vec<crate::model_catalog::ModelRow>)> = Vec::new();
        for m in models {
            match providers
                .iter_mut()
                .find(|(name, _)| *name == m.provider_display)
            {
                Some((_, rows)) => rows.push(m),
                None => providers.push((m.provider_display.clone(), vec![m])),
            }
        }
        let mut menu = menu;
        if providers.is_empty() {
            return menu.item(PopupMenuItem::Label("No models configured".into()));
        }
        for (prov_name, models) in providers {
            let ws = workspace.clone();
            menu = menu.submenu(prov_name, window, cx, move |submenu, _window, _cx| {
                let mut submenu = submenu;
                for m in &models {
                    let model = m.clone();
                    let model_name = model.name.clone();
                    let (variant, label, _) = Self::wire_visual(&model.api);
                    let ws = ws.clone();
                    submenu = submenu.item(
                        PopupMenuItem::element(move |_window, _cx| {
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    Tag::new()
                                        .with_variant(variant)
                                        .outline()
                                        .small()
                                        .child(label),
                                )
                                .child(model_name.clone())
                        })
                        .on_click(move |_, _, cx: &mut gpui::App| {
                            let model = model.clone();
                            let _ = ws.update(cx, |this, cx| {
                                // L8 wire identity: the registration-qualified
                                // `{provider}/{model}` ref, so a pick pins the
                                // exact endpoint (wire variants of one model
                                // share the bare id).
                                this.with_foreground_store(cx, |store, sid| {
                                    let mut config = serde_json::Map::new();
                                    config.insert(
                                        "model".into(),
                                        serde_json::json!(format!(
                                            "{}/{}",
                                            model.provider, model.id
                                        )),
                                    );
                                    store.optimistic_config(&sid, &config);
                                    store.set_config(&sid, config);
                                });
                            });
                        }),
                    );
                }
                submenu
            });
        }
        // The reasoning-effort knob lives in the model dropdown, next to the
        // model switch it tunes. The current effort is checked; a click
        // applies to the next request (same mid-run semantics as a model
        // switch; the menu dismisses like a model row).
        let themed = cx.theme().clone();
        menu = menu.separator();
        menu = menu.label(i18n::t("workspace-reasoning-effort"));
        for effort in manox_agent::language_model::ReasoningEffort::ALL {
            let ws = workspace.clone();
            let themed = themed.clone();
            let label = match effort {
                manox_agent::language_model::ReasoningEffort::High => {
                    i18n::t("workspace-reasoning-high")
                }
                manox_agent::language_model::ReasoningEffort::Max => {
                    i18n::t("workspace-reasoning-max")
                }
            };
            let selected = effort == current_effort;
            menu = menu.item(
                PopupMenuItem::element(move |_window, _cx| {
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            gpui::div()
                                .text_sm()
                                .text_color(themed.foreground)
                                .child(label.clone()),
                        )
                        .when(selected, |el| {
                            el.child(Icon::new(IconName::Check).small().text_color(themed.accent))
                        })
                })
                .on_click(move |_, _, cx: &mut gpui::App| {
                    let _ = ws.update(cx, |this, cx| {
                        let effort_str = match effort {
                            manox_agent::language_model::ReasoningEffort::High => "high",
                            manox_agent::language_model::ReasoningEffort::Max => "max",
                        };
                        this.with_foreground_store(cx, |store, sid| {
                            let mut config = serde_json::Map::new();
                            config.insert("reasoningEffort".into(), serde_json::json!(effort_str));
                            store.set_config(&sid, config);
                        });
                    });
                }),
            );
        }
        menu
    }

    /// Open the goal status popover (from the bare `/goal` command).
    pub fn open_goal_popover(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| {
            chat.goal_popover_open = true;
            cx.notify();
        });
        cx.notify();
    }

    /// Whether the foreground thread's goal chip shows a live elapsed value —
    /// the elapsed ticker's condition. The chip's elapsed is wall-clock
    /// anchored while the goal is non-terminal and frozen at created→updated
    /// once it settles (`Thread::goal_elapsed_seconds`), so only a
    /// non-terminal goal needs the once-a-second repaint. Reads the projection
    /// mirror rather than an event, so an attach can re-arm the ticker for a
    /// thread whose `GoalChanged` arrived while it was parked.
    pub(super) fn goal_elapsed_is_live(&self, cx: &App) -> bool {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).goal().cloned()
            })
            .and_then(|v| serde_json::from_value::<manox_agent::goal::ThreadGoal>(v).ok())
            .is_some_and(|goal| !goal.status.is_terminal())
    }

    /// (Re)arm the goal elapsed ticker for the foreground thread: bump the
    /// generation so any prior ticker self-terminates, then start a fresh one
    /// only while the elapsed value is live. The live `GoalChanged` arm passes
    /// the event's own verdict; an attach passes
    /// [`Self::goal_elapsed_is_live`], because the goal chip is
    /// projection-backed and the event that would have armed it was dropped
    /// while the thread was parked.
    pub(super) fn rearm_goal_ticker(&mut self, active: bool, cx: &mut Context<Self>) {
        let ticker_gen = self.chat.update(cx, |chat, cc| {
            chat.goal_ticker_gen = chat.goal_ticker_gen.wrapping_add(1);
            cc.notify();
            chat.goal_ticker_gen
        });
        if !active {
            return;
        }
        let entity = cx.entity().clone();
        cx.spawn(async move |_this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let still = entity.read_with(cx, |this, cx| {
                    this.chat.read(cx).goal_ticker_gen == ticker_gen
                        && this.goal_elapsed_is_live(cx)
                });
                if !still {
                    break;
                }
                entity.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }

    /// Prefill the composer with the durable objective so `/goal edit` is an
    /// explicit, inspectable update rather than an ephemeral popover field.
    pub fn begin_goal_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(objective) = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).goal().cloned()
            })
            .and_then(|v| serde_json::from_value::<manox_agent::goal::ThreadGoal>(v).ok())
            .map(|goal| goal.objective.clone())
        else {
            self.chat.update(cx, |chat, cx| {
                chat.goal_popover_open = true;
                cx.notify();
            });
            cx.notify();
            return;
        };
        self.chat_input(cx).update(cx, |state, cx| {
            state.set_value(format!("/goal edit {objective}"), window, cx);
        });
        cx.notify();
    }

    pub fn begin_goal_budget_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).goal().cloned()
            })
            .and_then(|v| serde_json::from_value::<manox_agent::goal::ThreadGoal>(v).ok())
            .and_then(|goal| goal.token_budget)
            .map(|budget| budget.to_string())
            .unwrap_or_else(|| "none".into());
        self.chat_input(cx).update(cx, |state, cx| {
            state.set_value(format!("/goal budget {value}"), window, cx);
        });
        cx.notify();
    }

    pub fn begin_goal_rounds_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).goal().cloned()
            })
            .and_then(|v| serde_json::from_value::<manox_agent::goal::ThreadGoal>(v).ok())
            .and_then(|goal| goal.max_rounds)
            .map(|max| max.to_string())
            .unwrap_or_else(|| "none".into());
        self.chat_input(cx).update(cx, |state, cx| {
            state.set_value(format!("/goal rounds {value}"), window, cx);
        });
        cx.notify();
    }

    /// Selecting Replace only prepares an explicit confirmation command; the
    /// persisted CAS replacement happens when the user submits it.
    pub fn begin_goal_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_goal_replace_with_objective("", window, cx);
    }

    pub fn begin_goal_replace_with_objective(
        &mut self,
        objective: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chat_input(cx).update(cx, |state, cx| {
            state.set_value(format!("/goal replace {objective}"), window, cx);
        });
        cx.notify();
    }

    pub fn begin_goal_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chat_input(cx).update(cx, |state, cx| {
            state.set_value("/goal ".to_string(), window, cx);
        });
        cx.notify();
    }

    pub(super) fn render_goal_chip(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let g = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).goal().cloned()
            })
            .and_then(|v| serde_json::from_value::<manox_agent::goal::ThreadGoal>(v).ok())?;
        let accent = theme.accent;
        let muted = theme.muted_foreground;
        let fg = theme.foreground;
        let status_key = match g.status {
            manox_agent::goal::GoalStatus::Active => "goal-status-active",
            manox_agent::goal::GoalStatus::Paused => "goal-status-paused",
            manox_agent::goal::GoalStatus::Blocked => "goal-status-blocked",
            manox_agent::goal::GoalStatus::BudgetLimited => "goal-status-budget-limited",
            manox_agent::goal::GoalStatus::Complete => "goal-status-complete",
        };
        let elapsed = format_elapsed(std::time::Duration::from_secs(
            self.chat
                .read(cx)
                .thread
                .read(|t| t.goal_elapsed_seconds())
                .unwrap_or_default(),
        ));
        let label: SharedString = format!("◎ {} · {}", i18n::t(status_key), elapsed).into();
        let open = self.chat.read(cx).goal_popover_open;

        let trigger = h_flex()
            .id("goal-chip")
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .bg(theme.secondary)
            .border_1()
            .border_color(accent)
            .cursor_pointer()
            .child(
                gpui::div()
                    .text_xs()
                    .text_color(theme.accent_foreground)
                    .child(label),
            )
            .child(
                Icon::new(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .xsmall()
                .text_color(muted),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.chat.update(cx, |chat, cx| {
                    chat.goal_popover_open = !chat.goal_popover_open;
                    cx.notify();
                });
                cx.notify();
            }));

        if !open {
            return Some(trigger.into_any_element());
        }

        let objective = g.objective.clone();
        let status = i18n::t(status_key);
        let reason = g
            .blocked_reason
            .as_ref()
            .map(|reason| reason.message.clone())
            .unwrap_or_else(|| "—".into());
        let tokens = g.tokens_used.to_string();
        let budget = g
            .token_budget
            .map(|value| value.to_string())
            .unwrap_or_else(|| "∞".into());
        let remaining = g
            .remaining_tokens()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "∞".into());
        let rounds = format!(
            "{}/{}",
            g.rounds_started,
            g.max_rounds
                .map(|max| max.to_string())
                .unwrap_or_else(|| "∞".into())
        );
        let goal_status = g.status;
        let objective_label = i18n::t("goal-popover-objective");
        let status_label = i18n::t("goal-popover-status");
        let elapsed_label = i18n::t("goal-popover-elapsed");
        let reason_label = i18n::t("goal-popover-reason");
        let tokens_label = i18n::t("goal-popover-tokens");
        let budget_label = i18n::t("goal-popover-budget");
        let remaining_label = i18n::t("goal-popover-remaining");
        let rounds_label = i18n::t("goal-popover-rounds");
        let clear_label = i18n::t("goal-popover-clear");
        let pause_label = i18n::t("goal-popover-pause");
        let resume_label = i18n::t("goal-popover-resume");
        let edit_label = i18n::t("goal-popover-edit");
        let edit_budget_label = i18n::t("goal-popover-edit-budget");
        let edit_rounds_label = i18n::t("goal-popover-edit-rounds");
        let replace_label = i18n::t("goal-popover-replace");
        let new_label = i18n::t("goal-popover-new");
        let title_label = i18n::t("goal-popover-title");
        let popover = v_flex()
            .w_full()
            .gap_1()
            .p_3()
            .child(
                gpui::div()
                    .text_xs()
                    .text_color(theme.accent_foreground)
                    .child(format!("◎ {title_label}")),
            )
            .child(goal_popover_row(&objective_label, &objective, fg, muted))
            .child(goal_popover_row(&status_label, &status, fg, muted))
            .child(goal_popover_row(&elapsed_label, &elapsed, fg, muted))
            .child(goal_popover_row(&reason_label, &reason, fg, muted))
            .child(goal_popover_row(&tokens_label, &tokens, fg, muted))
            .child(goal_popover_row(&budget_label, &budget, fg, muted))
            .child(goal_popover_row(&remaining_label, &remaining, fg, muted))
            .child(goal_popover_row(&rounds_label, &rounds, fg, muted))
            .child(
                h_flex()
                    .justify_end()
                    .gap_1()
                    .when(
                        goal_status == manox_agent::goal::GoalStatus::Active,
                        |row| {
                            row.child(
                                Button::new("goal-pause")
                                    .small()
                                    .label(pause_label)
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.with_foreground_store(cx, |store, sid| {
                                            let reply =
                                                store.send_goal(&sid, "pause", None, None, None);
                                            crate::ahp_store::await_reply(reply);
                                        });
                                    })),
                            )
                        },
                    )
                    .when(
                        matches!(
                            goal_status,
                            manox_agent::goal::GoalStatus::Paused
                                | manox_agent::goal::GoalStatus::Blocked
                        ),
                        |row| {
                            row.child(
                                Button::new("goal-resume")
                                    .small()
                                    .label(resume_label)
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.with_foreground_store(cx, |store, sid| {
                                            let reply =
                                                store.send_goal(&sid, "resume", None, None, None);
                                            crate::ahp_store::await_reply(reply);
                                        });
                                    })),
                            )
                        },
                    )
                    .when(
                        matches!(
                            goal_status,
                            manox_agent::goal::GoalStatus::Active
                                | manox_agent::goal::GoalStatus::Paused
                                | manox_agent::goal::GoalStatus::Blocked
                        ),
                        |row| {
                            row.child(Button::new("goal-edit").small().label(edit_label).on_click(
                                cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.chat.update(cx, |chat, cx| {
                                        chat.goal_popover_open = false;
                                        cx.notify();
                                    });
                                    this.begin_goal_edit(window, cx);
                                }),
                            ))
                        },
                    )
                    .when(
                        matches!(
                            goal_status,
                            manox_agent::goal::GoalStatus::Active
                                | manox_agent::goal::GoalStatus::Paused
                                | manox_agent::goal::GoalStatus::Blocked
                        ),
                        |row| {
                            row.child(
                                Button::new("goal-edit-rounds")
                                    .small()
                                    .label(edit_rounds_label)
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.chat.update(cx, |chat, cx| {
                                                chat.goal_popover_open = false;
                                                cx.notify();
                                            });
                                            this.begin_goal_rounds_edit(window, cx);
                                        },
                                    )),
                            )
                        },
                    )
                    .when(
                        goal_status == manox_agent::goal::GoalStatus::BudgetLimited,
                        |row| {
                            row.child(
                                Button::new("goal-edit-budget")
                                    .small()
                                    .label(edit_budget_label)
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.chat.update(cx, |chat, cx| {
                                                chat.goal_popover_open = false;
                                                cx.notify();
                                            });
                                            this.begin_goal_budget_edit(window, cx);
                                        },
                                    )),
                            )
                        },
                    )
                    .when(
                        matches!(
                            goal_status,
                            manox_agent::goal::GoalStatus::Paused
                                | manox_agent::goal::GoalStatus::Blocked
                                | manox_agent::goal::GoalStatus::BudgetLimited
                        ),
                        |row| {
                            row.child(
                                Button::new("goal-replace")
                                    .small()
                                    .label(replace_label)
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.chat.update(cx, |chat, cx| {
                                                chat.goal_popover_open = false;
                                                cx.notify();
                                            });
                                            this.begin_goal_replace(window, cx);
                                        },
                                    )),
                            )
                        },
                    )
                    .when(
                        goal_status == manox_agent::goal::GoalStatus::Complete,
                        |row| {
                            row.child(Button::new("goal-new").small().label(new_label).on_click(
                                cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.chat.update(cx, |chat, cx| {
                                        chat.goal_popover_open = false;
                                        cx.notify();
                                    });
                                    this.begin_goal_new(window, cx);
                                }),
                            ))
                        },
                    )
                    .child(
                        Button::new("goal-clear")
                            .small()
                            .label(clear_label)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.with_foreground_store(cx, |store, sid| {
                                    let reply = store.send_goal(&sid, "clear", None, None, None);
                                    crate::ahp_store::await_reply(reply);
                                });
                                this.chat.update(cx, |chat, cx| {
                                    chat.goal_popover_open = false;
                                    cx.notify();
                                });
                                cx.notify();
                            })),
                    ),
            );

        Some(
            gpui::div()
                .relative()
                .child(trigger)
                .child(
                    deferred(
                        gpui::div()
                            .id("goal-dropdown")
                            .absolute()
                            .bottom_full()
                            .left_0()
                            .occlude()
                            .w(gpui::px(360.))
                            .popover_style(cx)
                            .child(popover)
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.chat.update(cx, |chat, cx| {
                                    chat.goal_popover_open = false;
                                    cx.notify();
                                });
                                cx.notify();
                            })),
                    )
                    .with_priority(1),
                )
                .into_any_element(),
        )
    }
}
