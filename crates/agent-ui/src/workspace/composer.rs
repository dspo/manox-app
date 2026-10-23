//! The composer turn surface (U9b cluster 3): submit and its turn
//! builders, the mid-turn send modes (steer / queue / interrupt) with
//! their follow-up lifecycle (steer dispatch + retirement, settle flush,
//! drop/undo), and the recall walk over sent turns. Split from
//! `workspace.rs` — `super` is the workspace module; private methods are
//! lifted `pub(super)` so the parent (render/keyboard handlers) and its
//! `tests` child keep calling them. The generic wire helpers they ride
//! (`send_note`, `send_submit_v2`, `send_steer_v2`) stay in the parent.

/// The per-send delivery choice (thinkrail's streaming send modes): an
/// ACTION taken at submit time — never sticky state, so a plain Enter never
/// silently obeys a mode the user forgot about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitBehavior {
    /// Idle submit: an ordinary new turn.
    Send,
    /// Deliver at the running turn's next model-call boundary.
    Steer,
    /// Park for the settle flush.
    Queue,
    /// Cancel the running turn NOW; the message goes out at the settle.
    Interrupt,
}

use super::*;

/// Decode one wire message content block into a protocol image attachment
/// (round 4 §5.1): the base64 `Image { data, mime_type }` shape four call
/// sites used to repeat verbatim. Non-image blocks are `None`; a failed
/// decode is dropped (an undecodable attachment cannot ride the wire).
impl Workspace {
    /// The composer's Enter (the send pill's default face rides the same
    /// route): an ordinary turn while idle, a QUEUE while running — the most
    /// passive lane is the default, so the barest gesture never disturbs the
    /// running turn.
    pub(crate) fn submit_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let behavior = if self.foreground_running(cx) {
            SubmitBehavior::Queue
        } else {
            SubmitBehavior::Send
        };
        self.submit_composer(behavior, window, cx);
    }

    /// The composer's submit gate: read the input, route ask-card answers
    /// first, block on the empty/picker guards, intercept slash commands,
    /// then hand the message to [`Self::send_user_turn`]. Attachments decode
    /// off the UI thread before the handoff.
    fn submit_composer(
        &mut self,
        behavior: SubmitBehavior,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::info!("composer submit fired");
        // T10c (§D.6): the v1 `history_phase` loading gate is gone — the
        // follow stream's Snapshot → window Replace is the restore boundary,
        // and HEAD already carried an unwritten (default `Ready`) phase.
        // Successor gate (when §K.5 lands): a "no snapshot yet" flag on the
        // v2 fold.
        let text = self.chat_input(cx).read(cx).value().to_string();
        let attachments = self.chat.update(cx, |chat, cc| {
            let v = std::mem::take(&mut chat.pending_attachments);
            cc.notify();
            v
        });
        // A pending ask card answers FIRST, for every behavior — ⇧⌘↩
        // included. The card is the ask's only answer surface, and its
        // settle unblocks the turn the interrupt would target; "absolute
        // cancel priority" belongs to the STOP PILL, which never routes
        // through here.
        if self.chat.read(cx).pending_ask.is_some() {
            self.chat.update(cx, |chat, cx| {
                chat.pending_attachments = attachments;
                cx.notify();
            });
            // A send/Enter while an ask card is up submits that card, not a
            // message: the card is the ask's only answer surface, and its
            // pager's check is the same submit. An untouched question settles
            // as an explicit skip — the card asks for no completeness gate,
            // and neither does this shortcut. The composer text stays put (it
            // is not part of the answer): whatever the user typed is still
            // theirs to send once the ask settles.
            self.end_recall_walk(cx);
            self.close_completion(cx);
            self.submit_pending_ask(cx);
            return;
        }
        // Block submit on empty input or while the project picker is open.
        // Setting the project after a message lands is a no-op (`set_project`
        // guards on `!messages.is_empty()`), so the project would be silently
        // dropped. A message submitted while a turn is running is *not* dropped
        // here — it is routed through `send_user_turn`, which enqueues it as a
        // follow-up instead of interrupting the running turn.
        if (text.trim().is_empty() && attachments.is_empty())
            || self.chat.read(cx).project_picker_pending
        {
            tracing::info!(
                pending_picker = self.chat.read(cx).project_picker_pending,
                "submit swallowed by composer guard"
            );
            self.chat.update(cx, |chat, cx| {
                chat.pending_attachments = attachments;
                cx.notify();
            });
            return;
        }
        self.chat_input(cx)
            .update(cx, |state, cx| state.set_value("", window, cx));
        // Submitting ends the walk: nothing is left to return to.
        self.end_recall_walk(cx);
        self.close_completion(cx);

        // Slash commands (line-initial `/name [args]`) are intercepted before
        // sending a normal user turn. A recognized command fully handles the
        // input (Handled), asks to inject text as a user turn (InjectUserTurn),
        // or declines (NoOp → fall through to the normal path). Slash parsing
        // only applies to text-only input; attachments force the normal path.
        // While idle every recognized command dispatches; while a turn runs,
        // only the hot ones dispatch immediately: `/mode` (the gate governs
        // the very next tool call) and `/exit`/`/quit` (whose whole point is
        // to force-stop a running thread). Everything else typed mid-turn
        // stays in the follow-up queue as raw text rather than interrupting
        // the run (e.g. `/clear` mid-turn would race the streaming
        // conversation); the queued text flushes at turn end.
        let running = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).running()
            })
            .unwrap_or_else(|| {
                tracing::debug!("foreground store not bound yet (ahp handshake in flight)");
                Default::default()
            });
        if attachments.is_empty()
            && let Some(parsed) = crate::slash_command::parse(&text)
            && (!running || matches!(parsed.name.as_str(), "mode" | "exit" | "quit"))
        {
            let result = crate::slash_command::dispatch(&parsed, self, window, cx);
            match result {
                crate::slash_command::SlashResult::Handled => return,
                crate::slash_command::SlashResult::InjectUserTurn(msg) => {
                    let behavior = if self.foreground_running(cx) {
                        SubmitBehavior::Steer
                    } else {
                        SubmitBehavior::Send
                    };
                    self.send_user_turn(msg, Vec::new(), behavior, cx);
                    return;
                }
                crate::slash_command::SlashResult::NoOp => {}
            }
        }

        if attachments.is_empty() {
            self.send_user_turn(text, Vec::new(), behavior, cx);
            return;
        }

        // Reading attachment bytes is blocking IO; do it off the UI thread,
        // then start the turn. The chosen behavior rides the closure: an
        // interrupt with attachments must still cancel (the queue downgrade
        // below only covers the steer lane's text-only wire shape).
        cx.spawn(async move |this, cx| {
            let (text, extra, failed) = cx
                .background_spawn(async move {
                    let mut text = text;
                    let mut extra = Vec::new();
                    let mut failed = 0usize;
                    for att in &attachments {
                        match att {
                            PendingAttachment::ClipboardImage(img) => {
                                let format = match img.format {
                                    gpui::ImageFormat::Png => {
                                        Some(manox_agent::image::PastedImageFormat::Png)
                                    }
                                    gpui::ImageFormat::Jpeg => {
                                        Some(manox_agent::image::PastedImageFormat::Jpeg)
                                    }
                                    gpui::ImageFormat::Webp => {
                                        Some(manox_agent::image::PastedImageFormat::Webp)
                                    }
                                    gpui::ImageFormat::Gif => {
                                        Some(manox_agent::image::PastedImageFormat::Gif)
                                    }
                                    gpui::ImageFormat::Bmp => {
                                        Some(manox_agent::image::PastedImageFormat::Bmp)
                                    }
                                    gpui::ImageFormat::Tiff => {
                                        Some(manox_agent::image::PastedImageFormat::Tiff)
                                    }
                                    // SVG/ICO/PNM aren't raster-decodable; drop them.
                                    _ => None,
                                };
                                let content = format.and_then(|format| {
                                    manox_agent::image::pasted_image_to_message_content(
                                        &manox_agent::image::PastedImage {
                                            format,
                                            bytes: img.bytes.to_vec(),
                                        },
                                    )
                                });
                                match content {
                                    Some(content) => extra.push(content),
                                    None => failed += 1,
                                }
                            }
                            PendingAttachment::File { .. } => {
                                if let Some(content) = load_attachment(att, &mut text) {
                                    extra.push(content);
                                }
                            }
                        }
                    }
                    (text, extra, failed)
                })
                .await;
            this.update(cx, |this, cx| {
                this.send_user_turn(text, extra, behavior, cx);
                if failed > 0 {
                    this.add_info_message(i18n::t("composer-image-process-failed").to_string(), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Stage a clipboard image as a pending attachment chip. Resize happens
    /// off-thread on submit; here we only record the image and re-render.
    pub(super) fn handle_pasted_image(&mut self, image: gpui::Image, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.pending_attachments
                .push(PendingAttachment::ClipboardImage(image));
            cc.notify();
        });
    }

    /// Run a markdown prompt-macro slash turn (`/gitwork:deliver args`).
    pub(crate) fn run_command_turn(&mut self, name: &str, args: &str, cx: &mut Context<Self>) {
        self.run_registry_turn(RegistryTurnKind::Command, name, args, cx);
    }

    /// Run a skill slash turn (`/plugin:skill args` or bare `/skill`).
    pub(crate) fn run_skill_turn(&mut self, key: &str, args: &str, cx: &mut Context<Self>) {
        self.run_registry_turn(RegistryTurnKind::Skill, key, args, cx);
    }

    /// Shared dispatch for registry-backed slash turns: push the compact
    /// `/key args` display bubble, then hand the expanded body to the thread
    /// (`submit_command` / `submit_skill` render the registry content and run
    /// the turn). A registry miss surfaces as a thread error event. The
    /// transcript stores the expanded body, so a reloaded thread shows the
    /// body rather than the compact bubble (parity with the retired manox
    /// harness).
    pub(super) fn run_registry_turn(
        &mut self,
        kind: RegistryTurnKind,
        key: &str,
        args: &str,
        cx: &mut Context<Self>,
    ) {
        let display_text = if args.is_empty() {
            format!("/{key}")
        } else {
            format!("/{key} {args}")
        };
        let meta = self.user_turn_meta(cx);
        // Persist the send-time chrome with the turn: the expanded macro/skill
        // body is the model-facing text, and `display_text` keeps the bubble
        // showing the compact `/key args` invocation after a reload — the same
        // form the live view shows at send time.
        let _weak = cx.weak_entity();
        self.chat_conversation(cx).update(cx, |c, cx| {
            c.push_user(
                display_text,
                Vec::new(),
                meta,
                self.chat.read(cx).host.clone(),
                cx,
            )
        });
        self.sync_list_count(cx);
        // Re-engage tail-follow so the streaming reply stays in view.
        self.follow_message_tail(cx);
        // U2: the registry hit check reads the gateway's command snapshot —
        // the server projects the same command/skill registries the macro
        // and skill adapters dispatch against, so a remote server's
        // registry decides the hit.
        let commands = self
            .multiplexer
            .read(cx)
            .commands(cx)
            .cloned()
            .unwrap_or(serde_json::json!([]));
        let hit = match kind {
            RegistryTurnKind::Command => wire_commands_has(&commands, key, "command"),
            RegistryTurnKind::Skill => wire_commands_has(&commands, key, "skill"),
        };
        if hit {
            let submit_text = format!("/{key} {args}");
            let _ = self.send_submit_v2(submit_text, Vec::new(), cx);
        } else {
            let notice_key = match kind {
                RegistryTurnKind::Command => "workspace-unknown-command",
                RegistryTurnKind::Skill => "workspace-unknown-skill",
            };
            tracing::warn!("unknown {}", i18n::t_str(notice_key, &[("name", key)]));
        }
        // Persist on submit so the sidebar shows the new entry immediately
        // (cross-domain #5: the wire refetch — the server self-holds the
        // rescan in its answer).
        self.multiplexer.update(cx, |m, cx| m.fetch_thread_list(cx));
        cx.notify();
    }

    /// ⌘↩: a steer while running — one notch more assertive than the
    /// default queue, still restrained (delivers at the model's next step,
    /// never cancels). Idle, it degrades to an ordinary send.
    pub(crate) fn submit_steer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let behavior = if self.foreground_running(cx) {
            SubmitBehavior::Steer
        } else {
            SubmitBehavior::Send
        };
        self.submit_composer(behavior, window, cx);
    }

    pub(crate) fn submit_interrupt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_composer(SubmitBehavior::Interrupt, window, cx);
    }

    /// Route a submitted user turn. While the thread is idle every behavior
    /// sends the same ordinary turn. While a turn runs:
    /// - steer: hand the text to the server's steer queue (wire
    ///   `pendingMessageSet{steering}`) — the running turn injects it at
    ///   the next model-call boundary. Attachments have no wire ride here
    ///   and downgrade to the queue.
    /// - queue: park a `Queued` card; the turn's settle flushes the whole
    ///   queue as one combined turn (see `settle_follow_ups`).
    /// - interrupt: cancel the running turn now; the message leaves the
    ///   composer and rides the settle outbox — no card state.
    pub(super) fn send_user_turn(
        &mut self,
        text: String,
        images: Vec<manox_agent::language_model::MessageContent>,
        behavior: SubmitBehavior,
        cx: &mut Context<Self>,
    ) {
        let meta = self.user_turn_meta(cx);
        let weak = cx.weak_entity();
        let user_images = Self::decode_user_images(&images);
        let turn = DeferredUserTurn {
            text,
            images,
            meta,
            user_images,
        };
        let running = self.foreground_running(cx);
        if !running || behavior == SubmitBehavior::Send {
            // The thread is idle (not running). A free-form message dispatches
            // as a normal turn. (A plan-review verdict now reaches the user as
            // an `AskUserQuestion` card, and a send while that card is up
            // submits it — see `submit_input` — so there is no stale verdict
            // card to dismiss on this path any more.)
            self.append_and_run_user_turn(turn, weak, cx);
            // The conversation exists the moment the message is sent: refetch
            // the sidebar list now (the transcript-side refetch on the user
            // MessageEnd notice then fills in the summary text).
            self.multiplexer.update(cx, |m, cx| m.fetch_thread_list(cx));
            return;
        }
        self.chat.update(cx, |chat, cx| {
            chat.queue_drag = None;
            cx.notify();
        });
        match behavior {
            SubmitBehavior::Send => unreachable!("handled above"),
            SubmitBehavior::Interrupt => {
                // Absolute cancel priority: the interrupt fires before
                // anything else, then the message waits out the settle in the
                // outbox — deliberately NOT a card; the queue strip never
                // grows a 「待打断」 lane.
                self.cancel_turn(cx);
                self.chat.update(cx, |chat, cc| {
                    chat.interrupt_outbox.push(turn);
                    cc.notify();
                });
                // The settle may have landed between the running read and the
                // cancel (a fold still shows the turn running with no active
                // turn to cancel). If nothing is running any more, nothing
                // will settle — flush now instead of parking forever.
                if !self.foreground_running(cx) {
                    self.settle_follow_ups(false, cx);
                }
                cx.notify();
            }
            SubmitBehavior::Steer if turn.images.is_empty() => {
                // (Re-)send the online steer and park the card at the
                // steer-group boundary. A dropped send (no session bound)
                // must NOT fake `SteerPending` — the untouched message joins
                // the queue and flushes normally later.
                if let Some(message_id) = self.enqueue_steer_pending(&turn, cx) {
                    self.chat.update(cx, |chat, cx| {
                        let insert_at = steer_group_insert_index(&chat.queued_follow_ups);
                        chat.queued_follow_ups.insert(
                            insert_at,
                            QueuedFollowUp {
                                turn,
                                state: FollowUpState::SteerPending { message_id },
                            },
                        );
                        cx.notify();
                    });
                } else {
                    self.park_queued_turn(turn, cx);
                }
                cx.notify();
            }
            SubmitBehavior::Steer | SubmitBehavior::Queue => {
                // Steer with attachments downgrades: the pending-message wire
                // shape is text-only. The queue keeps the text; the images
                // ride only via the card's edit action (the flush turn is
                // text-only too — see `flush_batch`).
                self.park_queued_turn(turn, cx);
            }
        }
    }

    /// Park a turn as a plain `Queued` card above the composer.
    fn park_queued_turn(&mut self, turn: DeferredUserTurn, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.queued_follow_ups.push_back(QueuedFollowUp {
                turn,
                state: FollowUpState::Queued,
            });
            cc.notify();
        });
        cx.notify();
    }

    /// Re-read whether the foreground session's leaf still holds an active
    /// turn (the post-action race check the mode routes use).
    pub(super) fn foreground_running(&self, cx: &App) -> bool {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).running()
            })
            .unwrap_or(false)
    }

    /// Promote a follow-up to an ONLINE steer: mint a local message id and
    /// hand the text to the server's steer queue (`send_steer_v2` →
    /// `chat/pendingMessageSet{steering}`). Returns the minted id — the
    /// identity the journal row, the removal echo and the composer card
    /// all correlate by (`retire_steer_card`) — or `None` when no session
    /// is bound (the caller parks the message as a plain queue card).
    pub(super) fn enqueue_steer_pending(
        &mut self,
        turn: &DeferredUserTurn,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let message_id = uuid::Uuid::new_v4().to_string();
        let attachments: Vec<serde_json::Value> = Vec::new();
        self.send_steer_v2(cx, message_id.clone(), turn.text.clone(), attachments)
            .then_some(message_id)
    }

    /// Append the user turn (text plus any image content) to the thread and
    /// start the run: optimistic bubble first, then the protocol submit.
    pub(super) fn append_and_run_user_turn(
        &mut self,
        turn: DeferredUserTurn,
        _weak: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) {
        // UI state tracking (always) — conversation bubble + list housekeeping.
        self.chat_conversation(cx).update(cx, |c, cx| {
            c.push_user(
                turn.text.clone(),
                turn.user_images.clone(),
                turn.meta.clone(),
                self.chat.read(cx).host.clone(),
                cx,
            )
        });
        self.sync_list_count(cx);
        self.follow_message_tail(cx);
        // Dual-path: protocol Submit (kernel inserts + runs) vs direct insert + run.
        let attachments: Vec<serde_json::Value> = Vec::new();
        let _ = self.send_submit_v2(turn.text.clone(), attachments, cx);
    }

    /// Retire ONE `SteerPending` card whose steer the wire just consumed:
    /// the model has it, so the card leaves the queue immediately and its
    /// `steered` bubble enters the message list — the injection instant, not
    /// the turn boundary. Returns whether a card actually retired; `false`
    /// when no card matches the id (a foreign steer from another client, or
    /// a card an earlier settle already settled).
    pub(super) fn retire_steer_card(&mut self, message_id: &str, cx: &mut Context<Self>) -> bool {
        let matched = self.chat.read(cx).queued_follow_ups.iter().position(|item| {
            matches!(&item.state, FollowUpState::SteerPending { message_id: id } if id == message_id)
        });
        self.retire_matched_steer(matched, cx)
    }

    /// Retire the FIRST `SteerPending` card whose text equals `text` — the
    /// wake-retire fallback. FIFO first match: the earliest parked copy of
    /// the text is the one a same-text consumption retires, and a second
    /// same-text card stays parked for its own receipt (its id- or
    /// text-matched echo). Never call this on its own: go through
    /// [`Self::retire_woken_steer`], which owns the id-first rule.
    pub(super) fn retire_steer_card_matching_text(
        &mut self,
        text: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let matched = self
            .chat
            .read(cx)
            .queued_follow_ups
            .iter()
            .position(|item| {
                matches!(&item.state, FollowUpState::SteerPending { .. } if item.turn.text == text)
            });
        self.retire_matched_steer(matched, cx)
    }

    /// Retire the steer a WOKEN turn belongs to: the opening row's pending
    /// id first, then — only when the id names no card this client holds —
    /// the row's text. The id-first rule is what keeps one consumption from
    /// retiring two same-text cards; the text fallback exists because the
    /// projection mints `m-{entry}` for a journal row that carried no client
    /// id, so the woken turn can name a steer the card only recognises by
    /// its text. A turn start with no pending id is never a wake (a wake's
    /// opening row is always stamped), so the fallback never runs there;
    /// the store has already cleared the text for a carried queued
    /// submission. The one remaining ambiguity is another client's message
    /// with identical wording.
    pub(super) fn retire_woken_steer(
        &mut self,
        steer_id: Option<&str>,
        opening_text: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(message_id) = steer_id else {
            return false;
        };
        if self.retire_steer_card(message_id, cx) {
            return true;
        }
        opening_text.is_some_and(|text| self.retire_steer_card_matching_text(text, cx))
    }

    /// The shared retire: drop the card at `matched`, push its `steered`
    /// bubble into the transcript. `false` for a `None` match — a no-op,
    /// never an error.
    fn retire_matched_steer(&mut self, matched: Option<usize>, cx: &mut Context<Self>) -> bool {
        let Some(idx) = matched else {
            return false;
        };
        let Some(item) = self.chat.update(cx, |chat, cc| {
            let v = chat.queued_follow_ups.remove(idx);
            // A retire can land mid-drag (the removal echo is wire-timed):
            // void the stale marker so a release can never move the wrong
            // row — same discipline as the manual queue mutations.
            chat.queue_drag = None;
            cc.notify();
            v
        }) else {
            return false;
        };
        let mut meta = item.turn.meta.clone();
        meta.steered = true;
        self.chat_conversation(cx).update(cx, |c, cx| {
            c.push_user(
                item.turn.text.clone(),
                item.turn.user_images.clone(),
                meta,
                self.chat.read(cx).host.clone(),
                cx,
            )
        });
        self.sync_list_count(cx);
        self.follow_message_tail(cx);
        cx.notify();
        true
    }

    /// Replay the parked session's accumulated LIFECYCLE events before the
    /// stashed follow-up surface settles against them. A thread switched
    /// away keeps its subscription: consumption echoes, wake runs and
    /// settles land in the store's event bucket while no transcript is
    /// bound, and the live pump only drains the foreground session's bucket
    /// — a re-attach's first notify returns early on the rebuild path, so
    /// without this replay a steer consumed while parked keeps its card
    /// forever and a cancelled settle never strands anything.
    ///
    /// Only lifecycle events replay. The transcript content (deltas, tool
    /// rows) the rebuild already synthesized from the snapshot — replaying
    /// it would duplicate rows. After the replay, a remaining SteerPending
    /// card is genuinely still parked on the wire (its echo would have been
    /// in this bucket), so nothing is ever marked Failed on a guess: the
    /// only strand signals are positive ones — a replayed cancelled settle
    /// (below, via settle_follow_ups) or the card's own withdraw actions.
    /// The fold's single steering_message slot is deliberately NOT an
    /// oracle: the manox kernel queue holds many steers while AHP folds one
    /// slot, so the slot can neither prove nor disprove a card.
    pub(super) fn replay_parked_chat_events(&mut self, cx: &mut Context<Self>) {
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            return;
        };
        let events = store.update(cx, |s, _| s.drain_chat_events(&sid));
        for event in events {
            match event {
                crate::chat_fold::ChatEvent::TurnStarted {
                    steer_id,
                    opening_text,
                } => {
                    self.chat.update(cx, |chat, cx| {
                        chat.turn_active = true;
                        cx.notify();
                    });
                    self.spawn_thinking_ticker(cx);
                    // An idle steer wakes its own run: the retire rides the
                    // wake's own lifecycle (see `retire_woken_steer`).
                    self.retire_woken_steer(steer_id.as_deref(), opening_text.as_deref(), cx);
                }
                crate::chat_fold::ChatEvent::TurnFinished { cancelled } => {
                    self.chat.update(cx, |chat, cx| {
                        chat.turn_active = false;
                        cx.notify();
                    });
                    self.settle_follow_ups(cancelled, cx);
                }
                // The removal echo IS the injection receipt (the backend
                // publishes it only once the injection is confirmed).
                crate::chat_fold::ChatEvent::SteerConsumed { message_id } => {
                    self.retire_steer_card(&message_id, cx);
                }
                _ => {}
            }
        }
        cx.notify();
    }

    /// Settle the follow-up queue at a turn boundary. A cancelled settle
    /// strands every `SteerPending` card to `Failed` — the engine retracted
    /// the not-yet-injected steers without publishing removals, so no
    /// consumption echo will ever arrive; the retract is mirrored back to
    /// the wire (`pendingMessageRemoved`) so the fold's steering slot
    /// clears too. Then the settle outbox (the 立即打断 messages) flushes
    /// as the next turn, and the plain queue follows on a normal settle
    /// only — the user's stop must not start a surprise new run behind the
    /// queue. One submit per settle: a settle that flushed the outbox leaves
    /// the queue for the NEXT settle (that turn's own), so two batches never
    /// go back-to-back with the first still running.
    pub(super) fn settle_follow_ups(&mut self, cancelled: bool, cx: &mut Context<Self>) {
        // Settles drain cards by wire timing too — drop any in-flight drag
        // marker (its indices can never address the post-settle queue) and
        // the send menu (the running cluster it floats on just ended).
        self.chat.update(cx, |chat, cx| {
            let drag = chat.queue_drag.take().is_some();
            let menu = chat.send_menu.take();
            if drag || menu.is_some() {
                cx.notify();
            }
        });
        if cancelled {
            let stranded: Vec<String> = self
                .chat
                .read(cx)
                .queued_follow_ups
                .iter()
                .filter_map(|item| match &item.state {
                    FollowUpState::SteerPending { message_id } => Some(message_id.clone()),
                    _ => None,
                })
                .collect();
            if !stranded.is_empty() {
                self.chat.update(cx, |chat, cc| {
                    for item in chat.queued_follow_ups.iter_mut() {
                        if matches!(item.state, FollowUpState::SteerPending { .. }) {
                            item.state = FollowUpState::Failed;
                        }
                    }
                    cc.notify();
                });
                if let Some((store, sid)) = self.chat.read(cx).store.clone() {
                    store.update(cx, |store, _| {
                        for id in &stranded {
                            store.remove_pending_message(
                                &sid,
                                id,
                                ahp_types::state::PendingMessageKind::Steering,
                            );
                        }
                    });
                }
            }
        }
        let outbox = self.chat.update(cx, |chat, cc| {
            let outbox = std::mem::take(&mut chat.interrupt_outbox);
            cc.notify();
            outbox
        });
        if !outbox.is_empty() {
            // The outbox turn goes first — and while it RUNS the queue stays
            // parked: a second back-to-back submit would be the one mid-run
            // chat/submit no other client path ever issues (the queue exists
            // precisely to avoid that). The queue flushes at THIS turn's own
            // settle instead.
            self.flush_batch(outbox, cx);
            return;
        }
        if !cancelled {
            let due = self.chat.update(cx, |chat, cc| {
                let due = drain_queued_follow_ups(&mut chat.queued_follow_ups);
                cc.notify();
                due
            });
            if !due.is_empty() {
                self.flush_batch(due, cx);
            }
        }
    }

    /// Flush one batch of turns as a single combined new turn (blank-line
    /// separated user block, request prefix stable). A dropped submit
    /// re-parks the batch as retryable `Failed` cards. TEXT-ONLY: the
    /// wire's turn message has no image ride today (the front-door submit
    /// drops them just the same), so a card's pasted images deliver only
    /// through its edit action, which re-attaches them to the composer.
    fn flush_batch(&mut self, due: Vec<DeferredUserTurn>, cx: &mut Context<Self>) {
        let text = due
            .iter()
            .map(|turn| turn.text.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        let sent = !text.is_empty() && self.send_submit_v2(text.clone(), Vec::new(), cx);
        if !sent {
            // Nothing rode the wire — re-park everything as retryable
            // Failed cards (front-first, preserving the original order).
            self.chat.update(cx, |chat, cc| {
                for turn in due.into_iter().rev() {
                    chat.queued_follow_ups.push_front(QueuedFollowUp {
                        turn,
                        state: FollowUpState::Failed,
                    });
                }
                cc.notify();
            });
            cx.notify();
            return;
        }
        let meta = self.user_turn_meta(cx);
        self.chat_conversation(cx).update(cx, |c, cx| {
            c.push_user(text, Vec::new(), meta, self.chat.read(cx).host.clone(), cx)
        });
        self.sync_list_count(cx);
        self.follow_message_tail(cx);
        cx.notify();
    }

    /// Promote a parked follow-up to an ONLINE steer: hand it to the
    /// server's steer queue and park the card at the head of the queued
    /// group (the end of the steer group) until the wire reports the
    /// consumption (see [`Self::retire_steer_card`]). While idle, send it
    /// as a fresh ordinary turn instead. Works from every parked state
    /// except an in-flight steer itself.
    pub(super) fn steer_follow_up(&mut self, idx: usize, cx: &mut Context<Self>) {
        let running = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).running()
            })
            .unwrap_or_else(|| {
                tracing::debug!("foreground store not bound yet (ahp handshake in flight)");
                Default::default()
            });
        let Some(mut item) = self.chat.update(cx, |chat, cc| {
            let v = chat.queued_follow_ups.remove(idx);
            cc.notify();
            v
        }) else {
            return;
        };
        match item.state {
            FollowUpState::Queued | FollowUpState::Failed => {
                if running {
                    // (Re-)send the online steer and re-insert at the steer-group
                    // boundary, so the promoted card joins the trailing group of
                    // in-flight / failed steers ahead of the plain queue. A
                    // dropped send (no session bound) must NOT fake
                    // `SteerPending` — the untouched card simply rejoins the
                    // queue and flushes normally later.
                    if let Some(message_id) = self.enqueue_steer_pending(&item.turn, cx) {
                        item.state = FollowUpState::SteerPending { message_id };
                    }
                    // The group just changed: any in-flight drag's indices are
                    // stale (undo is a keyboard action and can land mid-drag).
                    self.chat.update(cx, |chat, cx| {
                        chat.queue_drag = None;
                        cx.notify();
                    });
                    let insert_at = steer_group_insert_index(&self.chat.read(cx).queued_follow_ups);
                    self.chat.update(cx, |chat, cx| {
                        chat.queued_follow_ups.insert(insert_at, item);
                        cx.notify();
                    });
                    cx.notify();
                } else {
                    let weak = cx.weak_entity();
                    self.append_and_run_user_turn(item.turn, weak, cx);
                }
            }
            FollowUpState::SteerPending { .. } => {
                // Already handed to the server steer queue; restore untouched
                // (wherever the last settle left it).
                let insert_at = steer_group_insert_index(&self.chat.read(cx).queued_follow_ups);
                self.chat.update(cx, |chat, cx| {
                    chat.queued_follow_ups.insert(insert_at, item);
                    cx.notify();
                });
            }
        }
    }

    /// Withdraw a wire-parked steer (编辑 / 删除 / 重新排队 all free the
    /// slot first). The backend maps the removal to a kernel-queue drop; a
    /// steer the server already injected no-ops it.
    fn withdraw_steer(&mut self, message_id: &str, cx: &mut Context<Self>) {
        if let Some((store, sid)) = self.chat.read(cx).store.clone() {
            store.update(cx, |store, _| {
                store.remove_pending_message(
                    &sid,
                    message_id,
                    ahp_types::state::PendingMessageKind::Steering,
                );
            });
        }
    }

    /// 重新排队 on a 待引导 card: withdraw the wire steer and demote the card
    /// to the plain queue (it flushes as part of the settle batch instead of
    /// injecting into the running turn).
    pub(super) fn requeue_follow_up(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(mut item) = self.chat.update(cx, |chat, cc| {
            let v = chat.queued_follow_ups.remove(idx);
            cc.notify();
            v
        }) else {
            return;
        };
        let insert_at = match &item.state {
            FollowUpState::SteerPending { message_id } => {
                self.withdraw_steer(message_id, cx);
                item.state = FollowUpState::Queued;
                steer_group_insert_index(&self.chat.read(cx).queued_follow_ups)
            }
            _ => steer_group_insert_index(&self.chat.read(cx).queued_follow_ups),
        };
        self.chat.update(cx, |chat, cx| {
            chat.queue_drag = None;
            chat.queued_follow_ups.insert(insert_at, item);
            cx.notify();
        });
        cx.notify();
    }

    /// 立即打断 on a parked card: the running turn's cancel fires NOW and
    /// the message rides the settle outbox (no card state — the interrupt is
    /// an action, not a lane). While idle it degrades to an ordinary send.
    pub(super) fn interrupt_from_card(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(item) = self.chat.update(cx, |chat, cc| {
            let v = chat.queued_follow_ups.remove(idx);
            cc.notify();
            v
        }) else {
            return;
        };
        if let FollowUpState::SteerPending { message_id } = &item.state {
            self.withdraw_steer(message_id, cx);
        }
        let running = self.foreground_running(cx);
        self.chat.update(cx, |chat, cc| {
            chat.interrupt_outbox.push(item.turn);
            cc.notify();
        });
        if running {
            self.cancel_turn(cx);
        }
        // Same post-action race check the interrupt submit rides: a settle
        // that landed between the read and the cancel means nothing will
        // flush us later.
        if !self.foreground_running(cx) {
            self.settle_follow_ups(false, cx);
        }
        cx.notify();
    }

    /// Commit a queue-row drag: reorder the parked `Queued` tail locally.
    /// This is session UI state only — the flush order the model eventually
    /// sees follows these rows one-for-one.
    pub(super) fn commit_queue_drag(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.commit_queue_drag();
            cc.notify();
        });
        cx.notify();
    }

    /// Return a parked row to the composer for another edit: its text merges
    /// into the input (below anything already typed) and its images re-attach
    /// as pending attachments. Only `Queued` / `Failed` rows render this
    /// affordance; the guard re-checks so a stale gesture can no-op.
    pub(super) fn edit_follow_up(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.chat.update(cx, |chat, cc| {
            let v = chat.queued_follow_ups.remove(idx);
            cc.notify();
            v
        }) else {
            return;
        };
        // A 待引导 card goes back into the composer too: the wire steer is
        // withdrawn first, so the slot frees for the next one and the text
        // can never deliver twice (a steer the server already injected
        // no-ops the withdrawal — that row stays, and the edited draft
        // becomes a fresh send).
        if let FollowUpState::SteerPending { message_id } = &item.state {
            self.withdraw_steer(message_id, cx);
        }
        self.chat.update(cx, |chat, cx| {
            chat.queue_drag = None;
            cx.notify();
        });
        let current = self.chat_input(cx).read(cx).value();
        let merged = if current.trim().is_empty() {
            item.turn.text.clone()
        } else {
            format!("{current}\n{}", item.turn.text)
        };
        self.chat_input(cx).update(cx, |state, cx| {
            state.set_value(merged, window, cx);
            state.focus(window, cx);
        });
        for image in item.turn.user_images {
            self.chat.update(cx, |chat, cc| {
                chat.pending_attachments
                    .push(PendingAttachment::ClipboardImage((*image.0).clone()));
                cc.notify();
            });
        }
        cx.notify();
    }

    pub(super) fn delete_follow_up(&mut self, idx: usize, cx: &mut Context<Self>) {
        // Deleting a 待引导 card withdraws the wire steer first (the kernel
        // queue drops it); an already-injected steer no-ops the withdrawal
        // and its row simply stays in the transcript. `Queued` / `Failed`
        // cards are local-only and drop freely.
        let state = self
            .chat
            .read(cx)
            .queued_follow_ups
            .get(idx)
            .map(|i| i.state.clone());
        match state {
            Some(FollowUpState::SteerPending { message_id }) => {
                self.withdraw_steer(&message_id, cx)
            }
            Some(_) => {}
            None => return,
        }
        self.chat.update(cx, |chat, cx| {
            chat.queued_follow_ups.remove(idx);
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.queue_drag = None;
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn undo_last_queued(&mut self, cx: &mut Context<Self>) {
        // Drop the last plain `Queued` card (the ⌘⌥/ undo affordance). Skip
        // any `SteerPending` cards sitting at the tail — undo never touches
        // the wire (use the row's 重新排队 to withdraw a steer) — and leave
        // `Failed` cards for the explicit retry/remove path.
        let Some(item) = self.chat.read(cx).queued_follow_ups.back() else {
            return;
        };
        if matches!(item.state, FollowUpState::Queued) {
            self.chat.update(cx, |chat, cc| {
                chat.queued_follow_ups.pop_back();
                cc.notify();
            });
            // A keyboard action can land mid-drag: void the stale marker so a
            // release can never move the wrong row.
            self.chat.update(cx, |chat, cx| {
                chat.queue_drag = None;
                cx.notify();
            });
            cx.notify();
        }
    }

    /// Decode provider-bound image contents into UI-preview `UserImage`s. The
    /// canonical `MessageContent::Image` bytes still go to the thread; this
    /// only rebuilds a gpui image for the user bubble.
    pub(super) fn decode_user_images(
        images: &[manox_agent::language_model::MessageContent],
    ) -> Vec<UserImage> {
        let _ = images;
        Vec::new()
    }

    /// Pure history-recall step for the composer. `turns` is newest-first;
    /// `index` < 0 means no walk is running, and `draft` is the walk's working
    /// line — the text a `Down` past the newest turn returns to.
    ///
    /// Entering and continuing both take an explicit key (`alt-up` /
    /// `alt-down`), so no text or caret state has to be watched to leave a
    /// walk and nothing compares the input against the recalled text. Whenever
    /// a step replaces the input, the text being left behind becomes the
    /// working line: the draft from walk entry, or a recalled turn the user
    /// has since changed — readline's history slot at position 0, which is
    /// what keeps an edit alive across a round trip back down.
    pub(super) fn recall_step(
        direction: RecallDirection,
        value: &str,
        index: i64,
        draft: Option<&str>,
        turns: &[String],
    ) -> (i64, Option<String>, RecallStep) {
        let walked = usize::try_from(index)
            .ok()
            .and_then(|ix| turns.get(ix).map(|_| ix));
        let left = match walked {
            Some(ix) if turns[ix] != value => Some(value.to_string()),
            Some(_) => draft.map(str::to_string),
            None => (!value.is_empty()).then(|| value.to_string()),
        };
        match (direction, walked) {
            (RecallDirection::Up, Some(ix)) => match turns.get(ix + 1) {
                Some(older) => (ix as i64 + 1, left, RecallStep::Recall(older.clone())),
                None => (index, left, RecallStep::None),
            },
            (RecallDirection::Up, None) => match turns.first() {
                Some(newest) => (0, left, RecallStep::Recall(newest.clone())),
                None => (-1, draft.map(str::to_string), RecallStep::None),
            },
            (RecallDirection::Down, Some(0)) => (
                -1,
                None,
                match left {
                    Some(working) => RecallStep::Recall(working),
                    None => RecallStep::Clear,
                },
            ),
            // A walked index always has a newer slot: `walked` came from
            // `turns.get(ix)`, and `ix == 0` is the arm above.
            (RecallDirection::Down, Some(ix)) => (
                ix as i64 - 1,
                left,
                RecallStep::Recall(turns[ix - 1].clone()),
            ),
            (RecallDirection::Down, None) => (-1, draft.map(str::to_string), RecallStep::None),
        }
    }

    pub(super) fn composer_recall_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_recall_step(RecallDirection::Up, window, cx);
    }

    pub(super) fn composer_recall_down(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_recall_step(RecallDirection::Down, window, cx);
    }

    pub(super) fn apply_recall_step(
        &mut self,
        direction: RecallDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let turns = self.recall_turns(cx);
        let value = self.chat_input(cx).read(cx).value().to_string();
        let (index, draft, step) = Self::recall_step(
            direction,
            &value,
            self.chat.read(cx).recall_index,
            self.chat.read(cx).recall_draft.as_deref(),
            &turns,
        );
        self.chat.update(cx, |chat, cc| {
            chat.set_recall(index, draft);
            cc.notify();
        });
        match step {
            RecallStep::None => {}
            RecallStep::Recall(text) => {
                self.set_composer_text(text, window, cx);
                cx.stop_propagation();
            }
            RecallStep::Clear => {
                self.set_composer_text(String::new(), window, cx);
                cx.stop_propagation();
            }
        }
    }

    /// Replace the composer's whole content and park the caret at its end.
    /// `set_value` emits no `Change`, so whatever must react to the new text
    /// (completion re-sync, recall bookkeeping) is driven by the caller.
    pub(super) fn set_composer_text(
        &mut self,
        text: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chat_input(cx).update(cx, |state, cx| {
            state.set_value(text.into(), window, cx);
            let end = RopeExt::offset_to_position(state.text(), state.text().len());
            state.set_cursor_position(end, window, cx);
        });
        cx.notify();
    }

    /// End a running recall walk and drop its working line.
    pub(super) fn end_recall_walk(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.end_recall_walk();
            cc.notify();
        });
    }

    /// Refill the composer with a past turn picked in the turn navigator
    /// (cmd/ctrl-Enter). A fill is a recall landing: the walk moves onto the
    /// filled turn, so `alt-down` off the newest end hands back whatever the
    /// fill displaced — `set_value` clears the input's undo history, and that
    /// working line is otherwise the only surviving copy of the user's draft.
    /// A turn with no text to recall is no fill at all, and a walk already
    /// running keeps the draft it started with.
    pub(super) fn fill_composer_from_turn(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let landed = self
            .recall_turns(cx)
            .iter()
            .position(|turn| *turn == text)
            .map(|ix| ix as i64);
        if let Some(index) = landed {
            let displaced = self.chat_input(cx).read(cx).value().to_string();
            self.chat.update(cx, |chat, cc| {
                if chat.recall_index < 0 {
                    chat.recall_draft =
                        (!displaced.is_empty() && displaced != text).then(|| displaced.clone());
                }
                cc.notify();
            });
            self.chat.update(cx, |chat, cx| {
                chat.recall_index = index;
                cx.notify();
            });
        } else {
            self.end_recall_walk(cx);
        }
        self.close_completion(cx);
        self.set_composer_text(text, window, cx);
        self.chat_input(cx)
            .update(cx, |state, cx| state.focus(window, cx));
    }

    /// Newest-first user-turn texts for recall, mirroring the navigator's
    /// `collect_user_turns` ordering minus image-only/empty turns.
    pub(super) fn recall_turns(&self, cx: &App) -> Vec<String> {
        collect_user_turns(
            self.chat
                .read(cx)
                .conversation
                .read(cx)
                .items()
                .iter()
                .enumerate()
                .map(|(ix, item)| (ix, item.read(cx).kind())),
        )
        .into_iter()
        .filter(|t| !t.text.trim().is_empty())
        .map(|t| t.text)
        .collect()
    }
}
