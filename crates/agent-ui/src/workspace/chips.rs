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
pub(super) fn parse_effort(raw: &str) -> Option<manox_agent::language_model::ReasoningEffort> {
    match raw {
        "high" => Some(manox_agent::language_model::ReasoningEffort::High),
        "max" => Some(manox_agent::language_model::ReasoningEffort::Max),
        _ => None,
    }
}

use gpui::Window;

/// The goal chip as a [`Popover`] trigger. `Selectable` is the library's hook
/// for pushing open-state styling into its own components; the chip paints its
/// open state (the chevron) from workspace state, so the impl is a no-op.
pub(super) struct GoalChipTrigger(pub(super) gpui::AnyElement);

impl gpui::IntoElement for GoalChipTrigger {
    type Element = gpui::AnyElement;
    fn into_element(self) -> Self::Element {
        self.0
    }
}

impl gpui_component::Selectable for GoalChipTrigger {
    fn selected(self, _: bool) -> Self {
        self
    }
    fn is_selected(&self) -> bool {
        false
    }
}

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
    /// Remote-settle reconcile for the surfaced interaction cards. The
    /// leaf's input-needed list is the gateway's authoritative pending view:
    /// an id that was confirmed in it and then vanished settled elsewhere
    /// (timeout expiry, another owner's answer, a cancel discard) and the
    /// local card must not latch onto the dead interaction.
    ///
    /// The ask card and the confirmation card reconcile INDEPENDENTLY —
    /// each with its own confirmed-membership arm, each clearing only its
    /// own state — because they are separate interactions that settle
    /// separately. Arming on the FIRST confirmed membership keeps the
    /// startup race safe: the request frame can land before the fold
    /// reflects the park, and an id never yet confirmed must never be
    /// cleared for being absent. Re-surfacing after a thread switch needs no
    /// local memory either — the gateway replays unsettled adjudications to
    /// every joining owner (manox §D.6), so the wire itself is the restore
    /// path.
    pub(crate) fn reconcile_pending_with_projections(&mut self, cx: &mut Context<Self>) {
        // No leaf yet (attach in flight): nothing to reconcile against.
        if self.chat.read(cx).store.is_none() {
            return;
        }
        // ── ask leg ────────────────────────────────────────────────────
        let ask_id = self
            .chat
            .read(cx)
            .pending_ask
            .as_ref()
            .map(|a| a.id.clone());
        match ask_id {
            None => {
                self.chat.update(cx, |chat, cx| {
                    chat.ask_projection_confirmed = false;
                    cx.notify();
                });
            }
            Some(id) => {
                let live = self.id_pending_in_fold(&id, cx);
                if live {
                    self.chat.update(cx, |chat, cx| {
                        chat.ask_projection_confirmed = true;
                        cx.notify();
                    });
                } else if self.chat.read(cx).ask_projection_confirmed {
                    self.chat.update(cx, |chat, cx| {
                        chat.pending_ask = None;
                        cx.notify();
                    });
                    self.reset_ask_state(cx);
                    cx.notify();
                }
            }
        }
        // ── confirmation leg ───────────────────────────────────────────
        let auth_id = self
            .chat
            .read(cx)
            .pending_confirmation
            .as_ref()
            .map(|a| a.auth_id.clone());
        match auth_id {
            None => {
                self.chat.update(cx, |chat, cx| {
                    chat.confirmation_projection_confirmed = false;
                    cx.notify();
                });
            }
            Some(id) => {
                let live = self.id_pending_in_fold(&id, cx);
                if live {
                    self.chat.update(cx, |chat, cx| {
                        chat.confirmation_projection_confirmed = true;
                        cx.notify();
                    });
                } else if self.chat.read(cx).confirmation_projection_confirmed {
                    self.chat.update(cx, |chat, cx| {
                        chat.pending_confirmation = None;
                        cx.notify();
                    });
                    cx.notify();
                }
            }
        }
    }

    /// Whether `id` is still pending in the leaf's input-needed list.
    fn id_pending_in_fold(&self, id: &str, cx: &App) -> bool {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .is_some_and(|(store, sid)| {
                crate::ahp_store::leaf(&store.read(cx).book, sid)
                    .requests()
                    .iter()
                    .any(|r| request_id(r) == id)
            })
    }

    /// Budget the live confirmation's decision surface onto the parked
    /// call's conversation row, and strip it the moment the fold stops
    /// carrying the park — a settle anywhere retires the row's action
    /// buttons on the next sync.
    ///
    /// The row is PROMOTED: an ordinary tool call nests inside the active
    /// activity segment, and only interaction cards sit at the top level —
    /// so when the fold's call has no top-level row, one is synthesized
    /// here (the ask card's same rule) and flagged as synthesized: the
    /// park's retirement REMOVES a promoted row (it carries no history of
    /// its own), while a row that already existed stays. Either way the
    /// fold re-owns the id while the park lives — later lifecycle rows
    /// absorb into the card in place, renaming it to the actual tool and
    /// flipping its status on the echo.
    pub(crate) fn sync_confirmation_snapshot(&mut self, cx: &mut App) {
        // The fold's live park, lowered to the row's decision surface:
        // `None` when the park is gone — a settle anywhere retires the
        // buttons on this pass.
        let live: Option<(
            String,
            String,
            Option<String>,
            steer_agent_chat_ui::column::ConfirmationSnapshot,
        )> = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                let leaf = crate::ahp_store::leaf(&view.book, sid);
                leaf.open_tool_confirmation()
                    .map(|(chat_id, confirmation)| {
                        let (_, _, tool_call_id) = leaf
                            .confirmation(&confirmation.id)
                            .unwrap_or_else(|| (chat_id, String::new(), confirmation.id.clone()));
                        // Only the pre-run confirmation carries options; a
                        // result confirmation settles with a plain confirm.
                        let options = match &confirmation.tool_call {
                            ahp_types::state::ToolCallConfirmationState::PendingConfirmation(c) => {
                                c.options.as_deref()
                            }
                            _ => None,
                        };
                        let actions = options
                            .unwrap_or(&[])
                            .iter()
                            .map(|option| steer_agent_chat_ui::column::ConfirmationAction {
                                option_id: option.id.clone(),
                                label: option.label.clone(),
                                approve: matches!(
                                    option.kind,
                                    ahp_types::state::ConfirmationOptionKind::Approve
                                ),
                            })
                            .collect();
                        // The promoted row renders the fold's own name and
                        // intention so the decision is readable without the
                        // call's (nested) argument row.
                        let (tool_name, title, edits) = match &confirmation.tool_call {
                            ahp_types::state::ToolCallConfirmationState::PendingConfirmation(c) => {
                                let edits = c
                                    .edits
                                    .as_ref()
                                    .map(|collection| {
                                        collection
                                            .items
                                            .iter()
                                            .map(|edit| {
                                                steer_agent_chat_ui::column::ConfirmationEdit {
                                                    path: edit
                                                        .after
                                                        .as_ref()
                                                        .or(edit.before.as_ref())
                                                        .map(|side| display_path(&side.uri))
                                                        .unwrap_or_default(),
                                                    added: edit.diff.as_ref().and_then(|d| d.added),
                                                    removed: edit
                                                        .diff
                                                        .as_ref()
                                                        .and_then(|d| d.removed),
                                                    creation: edit.before.is_none(),
                                                    deletion: edit.after.is_none(),
                                                }
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                (
                                    c.tool_name.clone(),
                                    c.intention.clone().map(|t| t.to_string()),
                                    edits,
                                )
                            }
                            _ => (String::new(), None, Vec::new()),
                        };
                        (
                            tool_call_id,
                            tool_name,
                            title,
                            steer_agent_chat_ui::column::ConfirmationSnapshot {
                                auth_id: confirmation.id.clone(),
                                actions,
                                edits,
                            },
                        )
                    })
            })
            // No store (a diagnostic seed, or a detached column): the parked
            // state still names the park; its options live in the fold, so
            // the promoted card renders without buttons until a fold lands.
            .or_else(|| {
                self.chat
                    .read(cx)
                    .pending_confirmation
                    .as_ref()
                    .map(|parked| {
                        (
                            parked.tool_call_id.clone(),
                            parked.auth_id.clone(),
                            None,
                            steer_agent_chat_ui::column::ConfirmationSnapshot {
                                auth_id: parked.auth_id.clone(),
                                actions: Vec::new(),
                                // The parked fallback has no fold behind it,
                                // so no preview face either — the buttons-less
                                // card stays honest about what it knows.
                                edits: Vec::new(),
                            },
                        )
                    })
            });
        // The parked call's conversation row: the projection renders that
        // call as the top-level card while the park is open (the sync's
        // `promoted_confirmation`, in place of its segment entry), so this
        // path only ATTACHES the decision snapshot to that row — it never
        // mints or retires rows itself.
        let target = live.and_then(|(tool_call_id, _, _, snapshot)| {
            let conversation = self.chat_conversation(cx);
            let ix = conversation.read(cx).find_tool(&tool_call_id, cx)?;
            let item = conversation.read(cx).items().get(ix)?.clone();
            Some((tool_call_id, item, snapshot))
        });

        // Ask-shaped reconcile: strip the previous owner when it is no
        // longer the current row, then set the current surface (unchanged
        // surfaces skip the write).
        let next = target;
        if let Some(previous) = self.chat.update(cx, |chat, cc| {
            let v = chat.confirmation_snapshot_item.take();
            cc.notify();
            v
        }) {
            let still_current = next.as_ref().is_some_and(|(_, item, _)| item == &previous);
            if !still_current {
                previous.update(cx, |item, cx| {
                    if item.confirmation.take().is_some() {
                        cx.notify();
                    }
                });
            }
        }
        if let Some((_, item, snapshot)) = next {
            if item.read(cx).confirmation.as_ref() != Some(&snapshot) {
                item.update(cx, |item, cx| {
                    item.confirmation = Some(snapshot);
                    cx.notify();
                });
            }
            self.chat.update(cx, |chat, cc| {
                chat.confirmation_snapshot_item = Some(item);
                cc.notify();
            });
        }
    }

    pub(crate) fn resolve_auth(
        &mut self,
        auth_id: &str,
        selected_option_id: Option<String>,
        decision: PermissionDecision,
        cx: &mut Context<Self>,
    ) {
        // The unified confirmation card's allow/deny leg. The question card's
        // close is NOT this path — it is `dismiss_ask` (B2-PR-3): a close is
        // "the user left to speak", never a rejection, and the two must not
        // share an exit (they used to both render `WrapperToolDenied`).
        //
        // The verdict names the park it was minted for: a click landing
        // after the armed park changed (the first park settled elsewhere
        // while a second escalation armed) must no-op, never settle the new
        // park with a decision it was never shown.
        let Some(auth) = self.chat.update(cx, |chat, cc| {
            let matches = chat
                .pending_confirmation
                .as_ref()
                .is_some_and(|parked| parked.auth_id == auth_id);
            let v = matches.then(|| chat.pending_confirmation.take()).flatten();
            cc.notify();
            v
        }) else {
            tracing::warn!(
                request_id = %auth_id,
                "confirmation verdict ignored: a different park is armed"
            );
            return;
        };
        let id = auth.auth_id;
        let allow = matches!(decision, PermissionDecision::AllowOnce);
        if let Some((store, sid)) = self.chat.read(cx).store.clone() {
            let view = store.read(cx);
            // The fold request's id IS the auth id the host settles by, so it
            // rides the verdict's `_meta` stamp (see `AhpStore::
            // confirm_tool_call`): without it the host silently ignores the
            // verdict and the card re-arms on the next fold event. The
            // clicked option rides along — its label may promise semantics
            // ("Always allow") the bare bool alone would break.
            if let Some((chat_id, turn_id, tool_call_id)) =
                crate::ahp_store::leaf(&view.book, &sid).confirmation(&id)
            {
                store.update(cx, |store, _| {
                    store.confirm_tool_call(
                        &chat_id,
                        &turn_id,
                        &tool_call_id,
                        &id,
                        selected_option_id,
                        allow,
                    );
                });
                cx.notify();
                return;
            }
        }
        // The fold no longer carries the id: nothing was sent on the wire and
        // the card only cleared locally (the live edge re-arms it). Answering
        // nothing here is the right wire behavior — but silently it is a dead
        // end to debug.
        tracing::warn!(
            request_id = %id,
            approved = allow,
            "confirmation verdict dropped: no matching open request in the fold"
        );
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

    /// Mount the pending ask's interactive card state — the page, the
    /// selections and the per-question note fields the form drives — replacing
    /// it whenever the ask changes. It lives on the column rather than on the
    /// card's row because the composer's Enter submits the same card, so the
    /// answers have exactly one reader. Runs on the render path, before the
    /// list measures (the state's note inputs need no window of their own
    /// until the card renders them).
    pub(super) fn ensure_ask_state(&mut self, cx: &mut App) {
        let pending = self.chat.read(cx).pending_ask.as_ref().map(|ask| {
            (
                ask.id.clone(),
                ask.questions
                    .iter()
                    .map(|q| q.multi_select)
                    .collect::<Vec<_>>(),
            )
        });
        let Some((id, multi)) = pending else {
            // No ask: retire the state so a re-seed of the same request (the
            // dismiss/restore race) starts from a blank form.
            if self.chat.read(cx).ask_state.is_some() {
                self.chat.update(cx, |chat, cx| {
                    chat.reset_ask_state();
                    cx.notify();
                });
            }
            return;
        };
        if self
            .chat
            .read(cx)
            .ask_state
            .as_ref()
            .is_some_and(|(state_id, _)| state_id == &id)
        {
            return;
        }
        let state = cx.new(|cx| {
            ai_elements::AskState::new(multi, i18n::t("workspace-ask-supplement-placeholder"), cx)
        });
        self.chat.update(cx, |chat, cx| {
            chat.ask_state = Some((id, state));
            cx.notify();
        });
    }

    /// Drop the ask's interactive state (the settle, dismissal and retirement
    /// paths all take it with them).
    pub(super) fn reset_ask_state(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| {
            chat.reset_ask_state();
            cx.notify();
        });
    }

    /// Settle the pending ask from the card's own state — the composer's Enter
    /// leg, which holds no element of its own to read the form back from.
    pub(super) fn submit_pending_ask(&mut self, cx: &mut Context<Self>) {
        let state = self
            .chat
            .read(cx)
            .ask_state
            .as_ref()
            .map(|(_, s)| s.clone());
        let Some(state) = state else {
            return;
        };
        let replies = steer_agent_chat_ui::ask_card::replies_of(&state, cx);
        self.reply_ask(replies, cx);
    }

    pub(crate) fn dismiss_ask(&mut self, cx: &mut Context<Self>) {
        let ask = match self.chat.update(cx, |chat, cc| {
            let v = chat.take_pending_ask();
            // Remember the dismissal: the engine's restore re-parks an
            // unsettled question (upstream #840), so the fold can re-open
            // this exact request after a NoOp'd decline race; the live-ask
            // edge re-issues the decline for it inside the window.
            chat.last_declined_ask = v
                .as_ref()
                .map(|a| (a.id.clone(), std::time::Instant::now()));
            cc.notify();
            v
        }) {
            Some(a) => a,
            None => return,
        };
        self.reset_ask_state(cx);
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

    /// Synchronize Workspace-derived ask state before the native list starts
    /// measuring: the pending ask's interactive state (mounting it on this
    /// path, since it is the ask's own render input) and the snapshot onto the
    /// ToolCall row the card renders on. Updating a `MessageItem` from the row
    /// callback dirties the same entity tree whose height GPUI is currently
    /// caching, which can make the cached row and the painted subtree describe
    /// different layouts.
    pub(super) fn sync_ask_card_snapshots(&mut self, cx: &mut App) {
        self.ensure_ask_state(cx);
        let next = self.chat.read(cx).pending_ask.as_ref().and_then(|ask| {
            let snapshot = self.ask_card_snapshot(&ask.id, cx)?;
            let state = self.chat.read(cx).ask_state.as_ref()?.1.clone();
            let ix = self.chat_conversation(cx).read(cx).find_tool(&ask.id, cx)?;
            let item = self.chat_conversation(cx).read(cx).items().get(ix)?.clone();
            Some((item, snapshot, state))
        });

        if let Some(previous) = self.chat.update(cx, |chat, cc| {
            let v = chat.ask_snapshot_item.take();
            cc.notify();
            v
        }) && next
            .as_ref()
            .is_none_or(|(current, _, _)| current != &previous)
            && previous.read(cx).ask_snapshot.is_some()
        {
            previous.update(cx, |item, cx| {
                item.ask_snapshot = None;
                item.ask_state = None;
                cx.notify();
            });
        }

        if let Some((item, snapshot, state)) = next {
            if item.read(cx).ask_snapshot.as_ref() != Some(&snapshot)
                || item.read(cx).ask_state.as_ref() != Some(&state)
            {
                item.update(cx, |item, cx| {
                    item.ask_snapshot = Some(snapshot);
                    item.ask_state = Some(state);
                    cx.notify();
                });
            }
            self.chat.update(cx, |chat, cx| {
                chat.ask_snapshot_item = Some(item);
                cx.notify();
            });
        }
    }

    /// Settle the pending ask: fold each question's answer (its selected
    /// option labels plus its own note) into canonical `AskAnswer` rows. A
    /// question with nothing selected and no note settles as an explicit skip
    /// (`selected: []`, no `custom`) — the same tri-state the server's
    /// `fold_ask_answers` reads. There is no card-level free-text override: a
    /// card close is `dismiss_ask`, and per-question free text rides the
    /// answer's `custom`.
    pub(crate) fn reply_ask(
        &mut self,
        replies: Vec<steer_agent_chat_ui::ask_card::AskCardReply>,
        cx: &mut Context<Self>,
    ) {
        let ask = match self.chat.update(cx, |chat, cc| {
            let ask = chat.take_pending_ask();
            cc.notify();
            ask
        }) {
            Some(a) => a,
            None => return,
        };
        let answers = ask_answers(&ask, &replies);
        let id = ask.id.clone();
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
        // The fold no longer carries the request (settled elsewhere, or the
        // answer raced its own decision row). Answering nothing here is the
        // right wire behavior — but silently it is a dead end to debug. The
        // card is already taken (`take_pending_ask` above), so the composer
        // leaves ask-supplement mode with this call: say so, or a submit that
        // answered nothing reads as the #88 dead-lock.
        tracing::warn!(
            request_id = %id,
            "ask answer dropped: no matching open request in the fold"
        );
        tracing::info!(
            request_id = %id,
            "ask card retired without an answer; composer back to plain submit"
        );
        cx.notify();
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
                crate::ahp_store::goal_of(&view.book, sid).cloned()
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
                crate::ahp_store::goal_of(&view.book, sid).cloned()
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
                crate::ahp_store::goal_of(&view.book, sid).cloned()
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
                crate::ahp_store::goal_of(&view.book, sid).cloned()
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
}

/// Fold a card's answers into the canonical `AskAnswer` rows. Per question:
/// the selected option LABELS (a select question), the free-form note (a text
/// question, or a select's supplemental freeform), or an explicit skip when
/// the user touched neither — the same tri-state the server's
/// `fold_ask_answers` reads. A reply index with no option, or an option index
/// the question does not have, contributes nothing rather than a phantom
/// label.
pub(super) fn ask_answers(
    ask: &PendingAsk,
    replies: &[steer_agent_chat_ui::ask_card::AskCardReply],
) -> std::collections::HashMap<String, ahp_types::state::ChatInputAnswer> {
    let mut answers = std::collections::HashMap::new();
    for (i, q) in ask.questions.iter().enumerate() {
        let reply = replies.get(i);
        let selected: Vec<String> = reply
            .map(|r| {
                r.selected
                    .iter()
                    .filter_map(|&oi| q.options.get(oi).map(|o| o.label.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let custom = reply
            .map(|r| r.note.clone())
            .filter(|note| !note.trim().is_empty());
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
            // An untouched question is a SKIPPED answer, not a missing key:
            // the host distinguishes "the user skipped this" from "the
            // client never sent it", and the model must see the skip.
            answers.insert(
                q.id.clone(),
                ahp_types::state::ChatInputAnswer::Skipped(ahp_types::state::ChatInputSkipped {
                    freeform_values: None,
                }),
            );
            continue;
        };
        answers.insert(
            q.id.clone(),
            ahp_types::state::ChatInputAnswer::Submitted(ahp_types::state::ChatInputAnswered {
                value,
            }),
        );
    }
    answers
}

/// A file URI lowered to its display path: strips the `file://` scheme and
/// any host segment, so the card reads `src/main.rs` rather than
/// `file:///Users/…/src/main.rs`.
fn display_path(uri: &str) -> String {
    uri.strip_prefix("file://")
        .map(|rest| {
            // `file://host/path` carries an authority; `file:///path` does
            // not. Strip up to the first `/` after any non-slash host.
            match rest.find('/') {
                Some(0) => rest.to_string(),
                Some(at) => rest[at..].to_string(),
                None => rest.to_string(),
            }
        })
        .unwrap_or_else(|| uri.to_string())
}
