//! The composer turn surface (U9b cluster 3): submit and its turn
//! builders, the steer/queued follow-up lifecycle (parked stash flush,
//! pending bubbles, stranded-steer healing, drop/undo), and the recall
//! walk over sent turns. Split from `workspace.rs` — `super` is the
//! workspace module; private methods are lifted `pub(super)` so the
//! parent (render/keyboard handlers) and its `tests` child keep
//! calling them. The generic wire helpers they ride (`send_note`,
//! `send_submit_v2`) stay in the parent.

use super::*;

/// Decode one wire message content block into a protocol image attachment
/// (round 4 §5.1): the base64 `Image { data, mime_type }` shape four call
/// sites used to repeat verbatim. Non-image blocks are `None`; a failed
/// decode is dropped (an undecodable attachment cannot ride the wire).
fn wire_image_attachment(
    c: &manox_agent::language_model::MessageContent,
) -> Option<manox_protocol::ImageAttachment> {
    use base64::Engine as _;
    match c {
        manox_agent::language_model::MessageContent::Image { data, mime_type } => {
            base64::engine::general_purpose::STANDARD
                .decode(data.as_bytes())
                .ok()
                .map(|bytes| manox_protocol::ImageAttachment {
                    data: bytes,
                    mime_type: mime_type.clone(),
                })
        }
        _ => None,
    }
}

impl Workspace {
    /// Flip EVERY parked thread's `SteerPending` card to `Failed` when its turn
    /// Drain the stashed `Queued` follow-ups of a parked thread whose turn
    /// just settled: the follow-up turn runs on the parked thread itself,
    /// mirroring the foreground flush without a conversation bubble (a
    /// switch-back rebuild shows it through the thread mirror). U1-flush:
    /// the flush rides the gateway — all but the last drain as
    /// `AppendUserMessage` notes (accept without a run) and the last as the
    /// v2 `Submit` that starts the next turn, matching the foreground
    /// flush's batching. The pre-migration direct facade write
    /// (`insert_user_message` + `run_turn`) bypassed the journal's
    /// accept-time persistence (K5) and the server's queue/drain merge; the
    /// parked thread has no optimistic bubble, so no echo is pushed — the
    /// origin_rpc still rides the Submit for the entry correlation.
    /// `SteerPending`/`Failed` cards stay parked for the user.
    pub(super) fn flush_parked_follow_ups(&mut self, thread_id: &str, _cx: &mut Context<Self>) {
        let Some(queue) = self.chat.queued_follow_ups_by_thread.get_mut(thread_id) else {
            return;
        };
        let mut retain: std::collections::VecDeque<QueuedFollowUp> =
            std::collections::VecDeque::new();
        let mut drained: Vec<DeferredUserTurn> = Vec::new();
        while let Some(item) = queue.pop_front() {
            if matches!(&item.state, FollowUpState::Queued) {
                drained.push(item.turn);
            } else {
                retain.push_back(item);
            }
        }
        if retain.is_empty() {
            self.chat.queued_follow_ups_by_thread.remove(thread_id);
        } else {
            *queue = retain;
        }
        if drained.is_empty() {
            return;
        }
        let n = drained.len();
        for (i, turn) in drained.into_iter().enumerate() {
            let attachments: Vec<manox_protocol::ImageAttachment> = turn
                .images
                .iter()
                .filter_map(wire_image_attachment)
                .collect();
            if i + 1 < n {
                // All but the last: accept without starting a run.
                self.client
                    .send_note(manox_protocol::ClientNote::AppendUserMessage {
                        session_id: thread_id.to_string(),
                        text: turn.text.clone(),
                        images: attachments,
                    });
            } else {
                // Last: accept + start the turn (K5 persists at acceptance;
                // the server's queue/drain merges anything racing in).
                self.client.send_call(manox_protocol::ClientCall::Submit {
                    session_id: thread_id.to_string(),
                    text: turn.text,
                    images: attachments,
                    origin_rpc: Some(uuid::Uuid::new_v4().to_string()),
                });
            }
        }
    }

    pub(crate) fn submit_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("composer submit fired");
        // T10c (§D.6): the v1 `history_phase` loading gate is gone — the
        // follow stream's Snapshot → window Replace is the restore boundary,
        // and HEAD already carried an unwritten (default `Ready`) phase.
        // Successor gate (when §K.5 lands): a "no snapshot yet" flag on the
        // v2 fold.
        let text = self.chat.input_state.read(cx).value().to_string();
        let attachments = std::mem::take(&mut self.chat.pending_attachments);
        if self.chat.pending_ask.is_some() {
            self.chat.pending_attachments = attachments;
            // A send/Enter while an ask card is up submits the card, not a
            // message: the tri-state answers are collected from the card's own
            // per-question selections + custom inputs (there is no card-level
            // composer override any more — B2-PR-1 retired the whole-card
            // `response` free text). The composer text is a supplement the user
            // may type but it no longer rides the answer.
            if !text.trim().is_empty() || self.pending_ask_has_selection() {
                // Completeness gate (dsh `submitDrafts` parity): a question
                // that was never touched must not silently fold to a skip.
                // The walk jumps to the first incomplete question — the blank
                // card IS the feedback — and the composer text survives.
                if let Some(missing) = self.first_incomplete_ask_question() {
                    self.chat.ask_step = missing;
                    cx.notify();
                    return;
                }
                self.chat
                    .input_state
                    .update(cx, |state, cx| state.set_value("", window, cx));
                // Submitting ends the walk: nothing is left to return to.
                self.end_recall_walk();
                self.close_completion(cx);
                self.resolve_ask(cx);
            }
            return;
        }
        // Block submit on empty input or while the project picker is open.
        // Setting the project after a message lands is a no-op (`set_project`
        // guards on `!messages.is_empty()`), so the project would be silently
        // dropped. A message submitted while a turn is running is *not* dropped
        // here — it is routed through `send_user_turn`, which enqueues it as a
        // follow-up instead of interrupting the running turn.
        if (text.trim().is_empty() && attachments.is_empty()) || self.chat.project_picker_pending {
            tracing::info!(
                pending_picker = self.chat.project_picker_pending,
                "submit swallowed by composer guard"
            );
            self.chat.pending_attachments = attachments;
            return;
        }
        self.chat
            .input_state
            .update(cx, |state, cx| state.set_value("", window, cx));
        // Submitting ends the walk: nothing is left to return to.
        self.end_recall_walk();
        self.close_completion(cx);

        // Slash commands (line-initial `/name [args]`) are intercepted before
        // sending a normal user turn. A recognized command fully handles the
        // input (Handled), asks to inject text as a user turn (InjectUserTurn),
        // or declines (NoOp → fall through to the normal path). Slash parsing
        // only applies to text-only input; attachments force the normal path.
        // While idle every recognized command dispatches; while a turn runs,
        // only `/mode` dispatches immediately — mode switches are hot (the
        // gate governs the very next tool call) and the prompt half of
        // `/mode <name> <prompt>` parks as a follow-up like any message.
        // Everything else typed mid-turn stays in the follow-up queue as raw
        // text rather than interrupting the run (e.g. `/clear` mid-turn would
        // race the streaming conversation); the queued text flushes at turn
        // end.
        let running = self
            .chat
            .store
            .as_ref()
            .map(|s| s.read(cx).store.running)
            .expect("foreground store present");
        if attachments.is_empty()
            && let Some(parsed) = crate::slash_command::parse(&text)
            && (!running || parsed.name == "mode")
        {
            let result = crate::slash_command::dispatch(&parsed, self, window, cx);
            match result {
                crate::slash_command::SlashResult::Handled => return,
                crate::slash_command::SlashResult::InjectUserTurn(msg) => {
                    self.send_user_turn(msg, Vec::new(), cx);
                    return;
                }
                crate::slash_command::SlashResult::NoOp => {}
            }
        }

        if attachments.is_empty() {
            self.send_user_turn(text, Vec::new(), cx);
            return;
        }

        // Reading attachment bytes is blocking IO; do it off the UI thread, then start the turn.
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
                this.send_user_turn(text, extra, cx);
                if failed > 0 {
                    this.add_info_message(
                        i18n::t("composer-image-process-failed").to_string(),
                        NoticeAnchor::TurnEnd,
                        None,
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    /// Stage a clipboard image as a pending attachment chip. Resize happens
    /// off-thread on submit; here we only record the image and re-render.
    pub(super) fn handle_pasted_image(&mut self, image: gpui::Image, cx: &mut Context<Self>) {
        self.chat
            .pending_attachments
            .push(PendingAttachment::ClipboardImage(image));
        cx.notify();
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
        self.chat.conversation.update(cx, |c, cx| {
            c.push_user(display_text, Vec::new(), meta, self.chat.host.clone(), cx)
        });
        self.sync_list_count(cx);
        // Re-engage tail-follow so the streaming reply stays in view.
        self.follow_message_tail();
        // U2: the registry hit check reads the gateway's command snapshot —
        // the server projects the same command/skill registries the macro
        // and skill adapters dispatch against, so a remote server's
        // registry decides the hit.
        let commands = self.multiplexer.read(cx).commands().clone();
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
        self.multiplexer.update(cx, |m, _| m.fetch_thread_list());
        cx.notify();
    }

    /// Append the user turn (text plus any image content) to the thread and start the run.
    pub(super) fn send_user_turn(
        &mut self,
        text: String,
        images: Vec<manox_agent::language_model::MessageContent>,
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
        // A turn is running — every new message first parks as a visible
        // follow-up. Steering is an explicit per-item action.
        if self
            .chat
            .store
            .as_ref()
            .map(|s| s.read(cx).store.running)
            .expect("foreground store present")
        {
            self.chat.queue_drag = None;
            self.chat.queued_follow_ups.push_back(QueuedFollowUp {
                turn,
                state: FollowUpState::Queued,
            });
            cx.notify();
            return;
        }
        // The thread is idle (not running). A free-form message dispatches as
        // a normal turn. (A plan-review verdict now reaches the user as an
        // `AskUserQuestion` card, and a send while that card is up submits it —
        // see `submit_input` — so there is no stale verdict card to dismiss on
        // this path any more.)
        self.append_and_run_user_turn(turn, weak, cx);
        // The conversation exists the moment the message is sent: refetch
        // the sidebar list now (the transcript-side refetch on the user
        // MessageEnd notice then fills in the summary text).
        self.multiplexer.update(cx, |m, _| m.fetch_thread_list());
    }

    /// Promote a parked follow-up to an ONLINE steer: mint a local message id,
    /// hand the message to the server's steer queue for the running turn. The
    /// card stays parked in the queue (the caller re-inserts it at the
    /// steer-group boundary) until the model consumes it — the injected row
    /// retires it ([`Self::retire_injected_steer`]). The old path called
    /// `thread.enqueue_steer` on the engine-less render mirror, which only
    /// inserted a local id and never reached the server — a dead end.
    /// Returns the minted `message_id`: the server threads it through as the
    /// injected row's durable identity, so the card retires by id.
    pub(super) fn enqueue_steer_pending(&mut self, turn: &DeferredUserTurn) -> Option<String> {
        let message_id = uuid::Uuid::new_v4().to_string();
        let attachments: Vec<manox_protocol::ImageAttachment> = turn
            .images
            .iter()
            .filter_map(wire_image_attachment)
            .collect();
        self.send_steer_v2(message_id.clone(), turn.text.clone(), attachments)
            .then_some(message_id)
    }

    /// Index at which a promoted steer card belongs so the queue keeps its
    /// invariant `[SteerPending|Failed …] ++ [Queued …]`: the head of the queued
    /// group — i.e. the end of the steer group — which is also `queue.len()`
    /// when there are no plain `Queued` items.
    pub(super) fn steer_group_insert_index(
        queue: &std::collections::VecDeque<QueuedFollowUp>,
    ) -> usize {
        queue
            .iter()
            .position(|item| matches!(item.state, FollowUpState::Queued))
            .unwrap_or(queue.len())
    }

    /// Foreground settle of a turn that finished normally: move every
    /// `SteerPending` card out of the queue and into the message list as a
    /// `steered` user bubble. The card may promote on THIS settle even when the
    /// server injected it during a continuation run it auto-starts afterwards:
    /// delivery is guaranteed by the server's steer-continuation contract
    /// (dspo/manox — idle steers wake their own run, residue at a normal settle
    /// chains a `continue_`, and an aborted run withdraws every stranded steer
    /// from the kernel queue so a retry can never double-deliver). The remaining
    /// `Queued` cards flush as the next turn afterwards, so the list order ends
    /// up matching the real delivery order (injected steers first, then the
    /// batched queue). `Failed` cards stay parked for a retry.
    pub(super) fn promote_settled_steers(&mut self, cx: &mut Context<Self>) {
        if !self
            .chat
            .queued_follow_ups
            .iter()
            .any(|item| matches!(item.state, FollowUpState::SteerPending { .. }))
        {
            return;
        }
        let _weak = cx.weak_entity();
        let follow_tail = self.chat.list_state.is_following_tail();
        let mut promoted = false;
        let mut retain: Vec<QueuedFollowUp> = Vec::new();
        while let Some(item) = self.chat.queued_follow_ups.pop_front() {
            match item.state {
                FollowUpState::SteerPending { .. } => {
                    let mut meta = item.turn.meta.clone();
                    // The 「已引导」 badge rides `meta.steered` (rendered in
                    // `render_user`), now that the pending bubble is gone.
                    meta.steered = true;
                    self.chat.conversation.update(cx, |c, cx| {
                        c.push_user(
                            item.turn.text.clone(),
                            item.turn.user_images.clone(),
                            meta,
                            self.chat.host.clone(),
                            cx,
                        )
                    });
                    promoted = true;
                }
                _ => retain.push(item),
            }
        }
        self.chat.queued_follow_ups.extend(retain);
        if !promoted {
            return;
        }
        self.chat.queue_drag = None;
        self.sync_list_count(cx);
        if follow_tail {
            self.follow_message_tail();
        }
        self.chat.list_state.remeasure();
        cx.notify();
    }

    /// Retire ONE `SteerPending` card whose injected row just landed
    /// (`ThreadEvent::UserRowLanded`): the model has consumed the steer, so the
    /// card leaves the queue immediately and its `steered` bubble enters the
    /// list — the dsh `claimed` instant, not the turn boundary. A no-op when no
    /// card matches the id (every ordinary prompt row reports the same event;
    /// only steers carry a card id). The settle path stays as the fallback for
    /// a row that raced it.
    pub(super) fn retire_injected_steer(&mut self, message_id: &str, cx: &mut Context<Self>) {
        let Some(pos) = self
            .chat
            .queued_follow_ups
            .iter()
            .position(|item| match &item.state {
                FollowUpState::SteerPending { message_id: card } => card.as_str() == message_id,
                _ => false,
            })
        else {
            return;
        };
        let Some(item) = self.chat.queued_follow_ups.remove(pos) else {
            return;
        };
        self.chat.queue_drag = None;
        let _weak = cx.weak_entity();
        let follow_tail = self.chat.list_state.is_following_tail();
        let mut meta = item.turn.meta.clone();
        // The 「已引导」 badge rides `meta.steered` (rendered in `render_user`).
        meta.steered = true;
        self.chat.conversation.update(cx, |c, cx| {
            c.push_user(
                item.turn.text.clone(),
                item.turn.user_images.clone(),
                meta,
                self.chat.host.clone(),
                cx,
            )
        });
        self.sync_list_count(cx);
        if follow_tail {
            self.follow_message_tail();
        }
        self.chat.list_state.remeasure();
        cx.notify();
    }

    pub(super) fn append_and_run_user_turn(
        &mut self,
        turn: DeferredUserTurn,
        _weak: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) {
        // UI state tracking (always) — conversation bubble + list housekeeping.
        self.chat.conversation.update(cx, |c, cx| {
            c.push_user(
                turn.text.clone(),
                turn.user_images.clone(),
                turn.meta.clone(),
                self.chat.host.clone(),
                cx,
            )
        });
        self.sync_list_count(cx);
        self.follow_message_tail();
        // Dual-path: protocol Submit (kernel inserts + runs) vs direct insert + run.
        let attachments: Vec<manox_protocol::ImageAttachment> = turn
            .images
            .iter()
            .filter_map(wire_image_attachment)
            .collect();
        let _ = self.send_submit_v2(turn.text.clone(), attachments, cx);
    }

    /// Drain every parked `Queued` follow-up into a single new turn. Multiple
    /// messages coalesce into one user block, keeping the request prefix stable
    /// (mirrors the team inbox flush). `Failed` cards stay parked for the user
    /// to retry; every `SteerPending` card has already been routed by the
    /// `TurnFinished` settle boundary — promoted into the message list
    /// ([`Self::promote_settled_steers`]) or stranded to `Failed`
    /// ([`Self::mark_stranded_steers_failed`]) — before this runs, so none
    /// reach here.
    pub(super) fn flush_queued_follow_ups(&mut self, cx: &mut Context<Self>) {
        if self.chat.queued_follow_ups.is_empty() {
            return;
        }
        let _weak = cx.weak_entity();
        let follow_tail = self.chat.list_state.is_following_tail();
        let mut retain: Vec<QueuedFollowUp> = Vec::new();
        let mut drained_turns: Vec<DeferredUserTurn> = Vec::new();
        while let Some(item) = self.chat.queued_follow_ups.pop_front() {
            match item.state {
                FollowUpState::Queued => {
                    self.chat.conversation.update(cx, |c, cx| {
                        c.push_user(
                            item.turn.text.clone(),
                            item.turn.user_images.clone(),
                            item.turn.meta.clone(),
                            self.chat.host.clone(),
                            cx,
                        )
                    });
                    self.sync_list_count(cx);
                    if follow_tail {
                        self.follow_message_tail();
                    }
                    drained_turns.push(item.turn);
                }
                _ => retain.push(item),
            }
        }
        self.chat.queued_follow_ups.extend(retain);
        if drained_turns.is_empty() {
            cx.notify();
            return;
        }
        // The `Queued` group just shifted: any in-flight drag's indices are
        // stale — void the gesture rather than move the wrong row.
        self.chat.queue_drag = None;
        let n = drained_turns.len();
        for (i, turn) in drained_turns.into_iter().enumerate() {
            let attachments: Vec<manox_protocol::ImageAttachment> = turn
                .images
                .iter()
                .filter_map(wire_image_attachment)
                .collect();
            if i + 1 < n {
                // All but the last: insert without running.
                let _ = self.send_note(|sid| manox_protocol::ClientNote::AppendUserMessage {
                    session_id: sid.into(),
                    text: turn.text.clone(),
                    images: attachments,
                });
            } else {
                // Last: insert + start the turn. The v2 Submit carries the
                // origin_rpc so the optimistic bubble retires when the durable
                // user row lands (batched predecessors insert without a turn
                // via the compat note and have no echo to retire).
                let _ = self.send_submit_v2(turn.text.clone(), attachments, cx);
            }
        }
        self.multiplexer.update(cx, |m, _| m.fetch_thread_list());
        cx.notify();
    }

    /// Settle the foreground steer group at a turn boundary with the server's
    /// per-id verdict: `stranded` is `stranded_steer_ids.len()` from the wire —
    /// the server retracts only the not-yet-injected tail (FIFO), so the first
    /// `N - stranded` cards were injected (promote into the list) and the LAST
    /// `stranded` retracted (Failed, retryable). `stranded.min(n)` keeps a
    /// miscount harmless. A normal settle carries zero stranded and promotes
    /// the whole group. The drag marker is dropped: the group just moved.
    pub(super) fn settle_steer_group(&mut self, stranded: usize, cx: &mut Context<Self>) {
        if stranded > 0 {
            self.chat.queue_drag = None;
            let positions: Vec<usize> = self
                .chat
                .queued_follow_ups
                .iter()
                .enumerate()
                .filter(|(_, item)| matches!(item.state, FollowUpState::SteerPending { .. }))
                .map(|(ix, _)| ix)
                .collect();
            let n = positions.len();
            let to_fail = stranded.min(n);
            for &ix in &positions[n - to_fail..] {
                self.chat.queued_follow_ups[ix].state = FollowUpState::Failed;
            }
        }
        self.promote_settled_steers(cx);
    }

    /// Settle a parked thread's steer group by the same per-id rule: the
    /// retracted tail turns `Failed`, every other card drops (a parked thread
    /// has no live list; the injected ones surface through the transcript on
    /// switch-back).
    pub(super) fn settle_parked_steer_group(&mut self, thread_id: &str, stranded: usize) {
        let Some(queue) = self.chat.queued_follow_ups_by_thread.get_mut(thread_id) else {
            return;
        };
        let positions: Vec<usize> = queue
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item.state, FollowUpState::SteerPending { .. }))
            .map(|(ix, _)| ix)
            .collect();
        let n = positions.len();
        let to_fail = stranded.min(n);
        for &ix in &positions[n - to_fail..] {
            queue[ix].state = FollowUpState::Failed;
        }
        let mut drop_ixs: Vec<usize> = positions[..n - to_fail].to_vec();
        drop_ixs.sort_unstable_by(|a, b| b.cmp(a));
        for ix in drop_ixs {
            queue.remove(ix);
        }
        if queue.is_empty() {
            self.chat.queued_follow_ups_by_thread.remove(thread_id);
        }
    }

    /// Promote a parked follow-up to a steer. While running, hand it to the
    /// server's steer queue and park the card at the head of the queued group
    /// (the end of the steer group) — no message-list bubble is pushed; the card
    /// moves into the list only when the turn settles (see
    /// [`Self::promote_settled_steers`]). While idle, send it as a fresh ordinary
    /// turn instead.
    pub(super) fn steer_follow_up(&mut self, idx: usize, cx: &mut Context<Self>) {
        let running = self
            .chat
            .store
            .as_ref()
            .map(|s| s.read(cx).store.running)
            .expect("foreground store present");
        let Some(mut item) = self.chat.queued_follow_ups.remove(idx) else {
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
                    if let Some(message_id) = self.enqueue_steer_pending(&item.turn) {
                        item.state = FollowUpState::SteerPending { message_id };
                    }
                    // The group just changed: any in-flight drag's indices are
                    // stale (undo is a keyboard action and can land mid-drag).
                    self.chat.queue_drag = None;
                    let insert_at = Self::steer_group_insert_index(&self.chat.queued_follow_ups);
                    self.chat.queued_follow_ups.insert(insert_at, item);
                    cx.notify();
                } else {
                    let weak = cx.weak_entity();
                    self.append_and_run_user_turn(item.turn, weak, cx);
                }
            }
            FollowUpState::SteerPending { .. } => {
                // Already handed to the server steer queue; restore untouched
                // (wherever the last settle left it).
                let insert_at = Self::steer_group_insert_index(&self.chat.queued_follow_ups);
                self.chat.queued_follow_ups.insert(insert_at, item);
            }
        }
    }

    /// Resolve a live queue-row drag to the index the dragged item lands at,
    /// or `None` when the gesture must no-op: no marker, the row dropped on
    /// itself, a committed (non-`Queued`) drag source, or a landing spot
    /// outside the contiguous `Queued` tail — the
    /// `[SteerPending|Failed …] ++ [Queued …]` group invariant survives even
    /// if the pointer hovers the status rows mid-drag.
    pub(super) fn queue_move_index(
        queue: &std::collections::VecDeque<QueuedFollowUp>,
        drag: composer_render::QueueRowDrag,
    ) -> Option<usize> {
        let from = drag.dragged;
        if from == drag.line_on {
            return None;
        }
        if !matches!(queue.get(from)?.state, FollowUpState::Queued) {
            return None;
        }
        let target = match drag.edge {
            composer_render::QueueDragEdge::Top => drag.line_on,
            composer_render::QueueDragEdge::Bottom => drag.line_on + 1,
        }
        .min(queue.len());
        let insert = if target > from { target - 1 } else { target };
        // The `Queued` group is the queue's contiguous tail (invariant):
        // anything below the first `Queued` row belongs to the committed
        // group and is not a legal destination.
        let head = queue
            .iter()
            .position(|item| matches!(item.state, FollowUpState::Queued))
            .unwrap_or(queue.len());
        if insert < head || insert > queue.len() - 1 {
            return None;
        }
        (insert != from).then_some(insert)
    }

    /// Commit a queue-row drag: reorder the parked `Queued` tail locally. This
    /// is session UI state only — the flush order the model eventually sees is
    /// the queue's own order, so no server round-trip is involved.
    pub(super) fn commit_queue_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.chat.queue_drag.take() else {
            cx.notify();
            return;
        };
        if let Some(insert) = Self::queue_move_index(&self.chat.queued_follow_ups, drag)
            && let Some(item) = self.chat.queued_follow_ups.remove(drag.dragged)
        {
            self.chat.queued_follow_ups.insert(insert, item);
        }
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
        let Some(item) = self.chat.queued_follow_ups.remove(idx) else {
            return;
        };
        if matches!(item.state, FollowUpState::SteerPending { .. }) {
            self.chat.queued_follow_ups.insert(idx, item);
            return;
        }
        self.chat.queue_drag = None;
        let current = self.chat.input_state.read(cx).value();
        let merged = if current.trim().is_empty() {
            item.turn.text.clone()
        } else {
            format!("{current}\n{}", item.turn.text)
        };
        self.chat.input_state.update(cx, |state, cx| {
            state.set_value(merged, window, cx);
            state.focus(window, cx);
        });
        for image in item.turn.user_images {
            self.chat
                .pending_attachments
                .push(PendingAttachment::ClipboardImage((*image.0).clone()));
        }
        cx.notify();
    }

    pub(super) fn delete_follow_up(&mut self, idx: usize, cx: &mut Context<Self>) {
        // `SteerPending` cards are not removable: the steer is already handed to
        // the server and the protocol has no steer-withdrawal channel, so
        // deleting the card would let it inject invisibly (see the composer
        // render's disabled delete). `Queued` and `Failed` cards drop freely.
        let Some(item) = self.chat.queued_follow_ups.get(idx) else {
            return;
        };
        if matches!(item.state, FollowUpState::SteerPending { .. }) {
            return;
        }
        self.chat.queued_follow_ups.remove(idx);
        self.chat.queue_drag = None;
        cx.notify();
    }

    pub(super) fn undo_last_queued(&mut self, cx: &mut Context<Self>) {
        // Drop the last plain `Queued` card (the ⌘⌥/ undo affordance). Skip any
        // `SteerPending` cards sitting at the tail — an online steer can't be
        // withdrawn, so it isn't undoable — and leave `Failed` cards for the
        // explicit retry/remove path.
        let Some(item) = self.chat.queued_follow_ups.back() else {
            return;
        };
        if matches!(item.state, FollowUpState::Queued) {
            self.chat.queued_follow_ups.pop_back();
            // A keyboard action can land mid-drag: void the stale marker so a
            // release can never move the wrong row.
            self.chat.queue_drag = None;
            cx.notify();
        }
    }

    /// Decode provider-bound image contents into UI-preview `UserImage`s. The
    /// canonical `MessageContent::Image` bytes still go to the thread; this
    /// only rebuilds a gpui image for the user bubble.
    pub(super) fn decode_user_images(
        images: &[manox_agent::language_model::MessageContent],
    ) -> Vec<UserImage> {
        images
            .iter()
            .filter_map(|c| {
                let attachment = wire_image_attachment(c)?;
                let fmt = gpui::ImageFormat::from_mime_type(attachment.mime_type.as_str())?;
                Some(UserImage(std::sync::Arc::new(gpui::Image::from_bytes(
                    fmt,
                    attachment.data,
                ))))
            })
            .collect()
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
        let value = self.chat.input_state.read(cx).value().to_string();
        let (index, draft, step) = Self::recall_step(
            direction,
            &value,
            self.chat.recall_index,
            self.chat.recall_draft.as_deref(),
            &turns,
        );
        self.chat.recall_index = index;
        self.chat.recall_draft = draft;
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
        self.chat.input_state.update(cx, |state, cx| {
            state.set_value(text.into(), window, cx);
            let end = RopeExt::offset_to_position(state.text(), state.text().len());
            state.set_cursor_position(end, window, cx);
        });
        cx.notify();
    }

    /// End a running recall walk and drop its working line.
    pub(super) fn end_recall_walk(&mut self) {
        self.chat.recall_index = -1;
        self.chat.recall_draft = None;
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
            let displaced = self.chat.input_state.read(cx).value().to_string();
            if self.chat.recall_index < 0 {
                self.chat.recall_draft =
                    (!displaced.is_empty() && displaced != text).then(|| displaced.clone());
            }
            self.chat.recall_index = index;
        } else {
            self.end_recall_walk();
        }
        self.close_completion(cx);
        self.set_composer_text(text, window, cx);
        self.chat
            .input_state
            .update(cx, |state, cx| state.focus(window, cx));
    }

    /// Newest-first user-turn texts for recall, mirroring the navigator's
    /// `collect_user_turns` ordering minus image-only/empty turns.
    pub(super) fn recall_turns(&self, cx: &App) -> Vec<String> {
        collect_user_turns(
            self.chat
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
