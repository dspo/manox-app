//! The workspace attach lifecycle (U9b cluster 1): attach/park/reclaim,
//! the create-intent launch paths, and the archive gestures. Split from
//! `workspace.rs` — `super` is the workspace module, so the parent's
//! imports and `Workspace`'s private fields resolve unchanged; the methods
//! are `pub(super)` so the parent module (and its `tests` child) keep
//! calling them.

use super::*;

impl Workspace {
    /// Minimal subscription for a thread parked in `background_threads`. Unlike
    /// `subscribe_thread`, this only coordinates running state and the parked
    /// follow-up stash. It never touches `conversation` or `self.chat.thread`, so a
    /// background thread's streaming deltas and tool events cannot be
    /// misrouted into the foreground view.
    /// The parked entry is left in `background_threads` (reclaimed on a later
    /// `open_thread`); self-removal from within the callback would drop the very
    /// subscription running it.
    pub(super) fn subscribe_background_thread(
        &self,
        _store: &(
            gpui::Entity<manox_agent_chat_ui::ahp_store::AhpStore>,
            String,
        ),
        _id: String,
        cx: &mut Context<Self>,
    ) -> Subscription {
        // Retired with the per-thread event stream: the AHP store folds every
        // channel for every attached session regardless of focus, so parked
        // state does not need a private subscription. Parked-session badge
        // rises re-attach to the store's notify through the multiplexer's
        // attention pump.
        cx.observe(&self.multiplexer, |_, _, _| {})
    }

    /// New thread for the pi harness: no provider reload (registration is
    /// one-shot; actors await readiness themselves). A project binds at
    /// construction time — a single session creation, no orphaned session
    /// file.
    ///
    /// T6: the double path is deleted. The workspace no longer pre-builds a
    /// local `Thread` and then attaches the AgentServer session to the same
    /// id (two `Thread::new_*` calls on one id, both racing to materialize the
    /// session file). It now sends the §D.2 `CreateSession` intent (the full
    /// `{cwd, project?, initial_model?}` so the server owns creation) and,
    /// when the server's `{session_id}` receipt lands, binds a detached
    /// rendering mirror to that server-minted id and opens the follow stream.
    /// Creation happens exactly once — on the server.
    pub fn start_new_thread(
        &mut self,
        project: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The park/draft bookkeeping is identical to `attach_thread`'s switch
        // arm, but there is no outgoing-thread double-bind to preserve: the
        // outgoing thread is detached on the park, the incoming id is unknown
        // until the receipt.
        //
        // Inheritance (L8/L9): the foreground session's wire identity is the
        // projection store — `model` key carries `{provider, modelId}` and
        // `project` the bound folder. The legacy thread mirror is only a
        // pre-snapshot fallback (the #765 round-2 repro: reading the mirror
        // alone lost model AND project on every new thread).
        let inherited = self.chat.read(cx).store.clone().map(|(store, sid)| {
            let view = store.read(cx);
            let leaf = crate::ahp_store::leaf(&view.book, &sid);
            (
                leaf.cwd().filter(|p| !p.is_empty()),
                leaf.model_id().map(str::to_string),
                leaf.approval_mode().map(str::to_string),
                leaf.reasoning_effort().map(str::to_string),
            )
        });
        let inherited_project = project.or_else(|| {
            inherited
                .as_ref()
                .and_then(|(p, _, _, _)| p.clone())
                .map(PathBuf::from)
        });
        let project = inherited_project;
        let model = inherited.as_ref().and_then(|(_, m, _, _)| m.clone());
        let approval = inherited.as_ref().and_then(|(_, _, a, _)| a.clone());
        let effort_str = inherited.as_ref().and_then(|(_, _, _, e)| e.clone());
        let cwd = project
            .as_ref()
            .unwrap_or(&self.cwd)
            .to_string_lossy()
            .to_string();
        let ws = cx.weak_entity();
        let dir_for_store = project.clone();
        // v3: the id is client-minted; the create carries the inherited
        // config, and the attach lands once the host answered.
        let sid = uuid::Uuid::new_v4().to_string();
        let mut config = serde_json::Map::new();
        if let Some(model) = &model {
            config.insert("model".into(), serde_json::json!(model));
        }
        if let Some(approval) = &approval {
            config.insert("approvalMode".into(), serde_json::json!(approval));
        }
        if let Some(effort) = &effort_str {
            config.insert("reasoningEffort".into(), serde_json::json!(effort));
        }
        let cwds = (project.is_none())
            .then(|| format!("file://{cwd}"))
            .into_iter()
            .collect();
        let reply = self.with_foreground_store(cx, |store, _| {
            store.create_session(&sid, cwds, Some(config))
        });
        let sid2 = sid.clone();
        cx.spawn(async move |_, cx| {
            let failed = match reply {
                Some(reply) => matches!(reply.recv().await, Ok(Err(_))),
                None => true,
            };
            if failed {
                tracing::warn!("createSession failed");
                return;
            }
            if let Some(dir) = &dir_for_store {
                let _ = ws.update(cx, |_ws, cx| {
                    Workspace::register_project_in_store(dir, cx);
                });
            }
            match ws.update_in(cx, |this, window, cx| {
                this.attach_created_session(&sid2, window, cx);
            }) {
                Ok(()) => tracing::info!(session_id = %sid2, "new-thread attach landed"),
                Err(err) => {
                    tracing::warn!(session_id = %sid2, error = %err, "new-thread attach FAILED")
                }
            }
        })
        .detach();
        let _ = window;
    }

    /// Attach the workspace to a freshly created session (whose id the server
    /// minted): reuse the standard switch bookkeeping (`attach_thread` parks
    /// the outgoing thread, restores drafts/queues) against a thread handle
    /// for the new id, then re-open — `open_or_create(reopen = true)`
    /// idempotently re-runs `OpenSession` and binds the follow stream that
    /// the create intent already opened. U6b②: the handle is ALWAYS the
    /// detached *rendering mirror* (a landing thread, no engine — the
    /// server owns the transcript, so no second actor ever races the
    /// session file); the former live_thread/load_thread store reads were
    /// the U6 dual source and are gone.
    pub(super) fn attach_created_session(
        &mut self,
        session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let thread = Thread::landing_with_id(ThreadId(session_id.to_string()), self.cwd.clone());
        self.attach_thread(thread, true, window, cx);
    }

    /// Switch to a new thread: persist the current one, build/load the new
    /// one, re-subscribe, and rebuild the conversation view. Each thread gets
    /// its own AgentServer session (`reopen` = `OpenSession` on an existing
    /// thread, else `CreateSession` on a fresh one); parking a running thread
    /// keeps its store/connection so the session stays alive and is never
    /// cancelled.
    pub(super) fn attach_thread(
        &mut self,
        new_thread: manox_agent::thread::ThreadHandle,
        reopen: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_turn_navigator(window, cx);
        let old_thread = self.chat.read(cx).thread.clone();
        let old_id = old_thread.read(|t| t.id.0.clone());
        let new_id = new_thread.read(|t| t.id.0.clone());

        // Sub-agent observation is per-thread ephemeral state; drop the
        // outgoing thread's transcripts AND its live panels before rebinding
        // (the shell has not stashed the outgoing pane yet — that happens in
        // the assembly's observer, after this attach — so the panels closed
        // here are exactly the outgoing thread's).
        self.clear_subagent_observation(cx);

        // Save the outgoing thread's unsent composer text before switching, so
        // a draft survives a round-trip through another thread (Bug 1). A
        // thread that just submitted already cleared its input, storing "".
        // While the walk runs the composer holds borrowed history — a recall
        // step's turn or a navigator fill — so the user's draft is the walk's
        // working line, not what is on screen.
        let outgoing = match self.chat.read(cx).recall_draft.as_deref() {
            Some(draft) if self.chat.read(cx).recall_index >= 0 => draft.to_string(),
            _ => self.chat_input(cx).read(cx).value().to_string(),
        };
        self.chat.update(cx, |chat, cx| {
            chat.drafts.insert(old_id.clone(), outgoing);
            cx.notify();
        });
        self.end_recall_walk(cx);

        // Queue state is session-local but belongs to a thread, not to the
        // currently visible workspace. Move it aside before rebinding.
        self.chat.update(cx, |chat, cc| {
            let v = std::mem::take(&mut chat.queued_follow_ups);
            // A live queue drag belongs to the outgoing view: its indices
            // mean nothing against the incoming thread's queue — drop it.
            chat.queue_drag = None;
            match v.is_empty() {
                true => {
                    chat.queued_follow_ups_by_thread.remove(&old_id);
                }
                false => {
                    chat.queued_follow_ups_by_thread.insert(old_id.clone(), v);
                }
            }
            // Restore the incoming thread's stashed follow-up queue (moved
            // aside when it was last switched away); without this the queue
            // silently vanishes on switch-back.
            let restored = chat.queued_follow_ups_by_thread.remove(&new_id);
            if let Some(queue) = restored {
                chat.queued_follow_ups = queue;
            }
            cc.notify();
        });

        // Park a still-running old thread in the background (U6b⑤: the
        // turn runs server-side and survives the switch on its own —
        // parking keeps the session ATTACHED so the reclaim is an in-place
        // re-attach with no reopen, and the parked leaf keeps receiving
        // the §D.5 deltas that drive the sidebar badges). The running
        // truth is the foreground LEAF's wire mirror: the facade
        // `is_running` this replaces is a dead flag on the detached
        // mirrors the attach path builds since U6b②. The store-flag seeds
        // retired with the U3b single-writer discipline — the server's
        // pump marked running/background_work when the turn/task started,
        // and its deltas already fed every mirror. An idle old thread's
        // session has no activity to preserve, so it detaches.
        let old_running = (self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).running()
            })
            .unwrap_or(false)
            || manox_agent::background_task::thread_has_running_tasks(&old_id))
            && old_id != new_id;
        if old_running {
            let old_store = self.chat.update(cx, |chat, cx| {
                let v = chat.store.take();
                cx.notify();
                v
            });
            let old_sid = self.chat.update(cx, |chat, cc| {
                let v = chat.session_id.take();
                cc.notify();
                v
            });
            let sub = self.subscribe_background_thread(
                old_store
                    .as_ref()
                    .expect("a parked running thread always has a store"),
                old_id.clone(),
                cx,
            );
            self.background_threads.push(BackgroundThread {
                id: old_id.clone(),
                store: old_store,
                session_id: old_sid,
                _sub: sub,
            });
        } else if old_id != new_id {
            // Idle switch: the outgoing session has nothing in flight. Detach
            // it from the shared connection so the server releases the owner
            // (the session itself survives for a later `OpenSession`), then
            // drop the local handle. No owner leak — the pre-multiplex path
            // leaked because the in-process transport never signalled drop.
            self.multiplexer.update(cx, |m, cx| {
                m.forget(&old_id);
                m.flush_unsubscribes(cx);
            });
            self.chat.update(cx, |chat, cx| {
                chat.store = None;
                cx.notify();
            });
            self.chat.update(cx, |chat, cx| {
                chat.session_id = None;
                cx.notify();
            });
        }

        // If the new thread was previously parked in the background, reclaim it
        // (entity + store) so it becomes the foreground thread and is no longer
        // double-held. The parked session stayed attached to the shared
        // connection, so the reclaim reuses the same leaf and follow stream —
        // but it must still re-own the session (see the §D.6 send below): the
        // gateway only re-delivers unsettled adjudications to a joining owner.
        if let Some(pos) = self.background_threads.iter().position(|b| b.id == new_id) {
            let bg = self.background_threads.remove(pos);
            self.chat.update(cx, |chat, cc| {
                chat.store = bg.store;
                chat.session_id = bg.session_id;
                cc.notify();
            });
        }

        // A brand-new foreground thread gets its own session on the shared
        // connection: `reopen` binds an existing thread via `OpenSession`
        // (history replays as the follow-stream `Snapshot`), otherwise a
        // fresh one via `CreateSession`. The multiplexer registers the leaf
        // handle before sending so the server's reply routes straight to it.
        if self.chat.read(cx).store.is_none() {
            let new_sid = new_id.clone();
            let _cwd = thread_cwd(&new_thread, &None, cx)
                .unwrap_or_default()
                .to_string();
            let store = self.multiplexer.read(cx).store();
            self.multiplexer
                .update(cx, |m, cx| m.open_or_create(&new_sid, reopen, cx));
            self.chat.update(cx, |chat, cc| {
                chat.store = Some((store, new_sid.clone()));
                chat.session_id = Some(new_sid);
                cc.notify();
            });
        }
        // GW5: focus follows the attach on BOTH legs — the newly attached
        // session's leaf goes active (clearing its unread/errored mirrors) and
        // the outgoing one inert. The reclaimed leg skips the `open_or_create`
        // branch, so keeping this inside it stranded the multiplexer's focus
        // on the thread just left: the thread being viewed kept raising
        // unread while the one just left never lit up.
        self.multiplexer
            .update(cx, |m, cx| m.set_focused(Some(&new_id), cx));

        // Persist the old thread's current state before switching away. The
        // spawned-task save backstop in `run_turn` will persist again when the
        // turn actually finishes, capturing the final assistant messages.

        self.chat.update(cx, |chat, cx| {
            chat.thread = new_thread;
            cx.notify();
        });
        // The chips derive from the bound thread's authoritative mirror; the
        // previous thread's suites never bleed across the switch.
        let suites = self
            .chat_store(cx)
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, &sid)
                    .ext
                    .and_then(|x| x.browser_suites.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|s| {
                        serde_json::from_value::<manox_agent::engine::BrowserSuite>(
                            serde_json::Value::String(s),
                        )
                        .ok()
                    })
                    .collect()
            })
            .unwrap_or_else(|| {
                tracing::debug!("foreground store not bound yet (ahp handshake in flight)");
                Default::default()
            });
        self.chat.update(cx, |chat, cc| {
            chat.active_browser_suites = suites;
            cc.notify();
        });
        let id = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(_, sid)| sid.clone())
            .unwrap_or_else(|| {
                tracing::debug!("foreground store not bound yet (ahp handshake in flight)");
                Default::default()
            });
        // Derive the transcript's plan and the sub-agent rows inside one
        // store read — the display fold's message rows ARE the messages
        // (L6, mechanical transcription), and cloning the whole transcript
        // here would copy it on the main thread on every switch.
        let (plan_from_messages, subagent_rows) = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|_| {
                // The plan snapshot and the sub-agent tree ride the x-manox
                // channels (unmodelled on this pass), so the restore starts
                // from an empty message set.
                let msgs: Vec<manox_agent::Message> = Vec::new();
                (
                    manox_agent::plan::rebuild_from_messages(&msgs),
                    manox_agent::subagent_restore::rebuild_from_messages(&msgs),
                )
            })
            .unwrap_or_else(|| {
                tracing::debug!("foreground store not bound yet (ahp handshake in flight)");
                Default::default()
            });
        let (display, usage) = self
            .chat
            .read(cx)
            .store
            .clone()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                let chat = crate::ahp_store::leaf(&view.book, &sid).chat?;
                let mut usage = crate::chat_fold::UsageTable::new();
                let display = crate::chat_fold::synth_display(chat, &mut usage);
                Some((display, usage))
            })
            .unwrap_or_default();
        let background_tasks: Vec<manox_agent::background_task::TaskSnapshot> = Vec::new();
        let role = self.model_label(cx);
        let recipient = self.recipient_author(cx);
        let _weak = cx.weak_entity();
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
        let cwd = thread_cwd(&self.chat.read(cx).thread, &self.chat.read(cx).store, cx);
        let new_conv = cx.new(|cx| {
            let mut conversation = ConversationState::rebuild_from_display(
                &display,
                &usage,
                &role,
                recipient,
                running,
                crate::conversation::ApplyCtx {
                    host: self.chat.read(cx).host.clone(),
                    cwd,
                    // These rows are this session's journal replayed, so their
                    // entry ids are forkable anchors.
                    fork_source: self.fork_source_session(cx),
                },
                cx,
            );
            let host = self.chat.read(cx).host.clone();
            conversation.restore_background_tasks(&background_tasks, &role, host, cx);
            conversation
        });
        self.chat.update(cx, |chat, cx| {
            chat.conversation = new_conv;
            cx.notify();
        });
        self.observe_conversation(cx);
        // Restore the incoming thread's saved draft, or clear the input if it
        // has none — without this the previous thread's text would bleed into
        // the new one (Bug 1). `set_value` is silent (no Change event), so
        // re-sync the slash menu by hand in case the draft begins with `/`.
        let saved = self.chat.update(cx, |chat, cc| {
            let v = chat.drafts.remove(&new_id).unwrap_or_default();
            cc.notify();
            v
        });
        self.chat_input(cx)
            .update(cx, |s, cx| s.set_value(saved, window, cx));
        self.sync_completion(window, cx);
        // Reveal the latest turn for the new thread: `reset` drops the old
        // thread's measured heights and scroll position, then reveal the latest
        // turn once. Both running and completed threads arm `FollowMode::Tail`:
        // the list pins to the end on each layout while following, and GPUI
        // auto-disengages follow the moment the user scrolls up. Using
        // `Normal` here would leave a one-shot scroll that never re-pins if a
        // late delta or notice lands after the user scrolls.
        let count = self.chat_conversation(cx).read(cx).items().len();
        self.chat.update(cx, |chat, cx| {
            chat.list_state.reset(count);
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.list_count = count;
            cx.notify();
        });
        self.chat
            .read(cx)
            .list_state
            .set_follow_mode(FollowMode::Tail);
        self.chat.update(cx, |chat, cx| {
            chat.pending_ask = None;
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.pending_auth = None;
            cx.notify();
        });
        let (thread_events, store_changes) = self.subscribe_thread(cx);
        // v3 semantics: subscribing IS joining — the fold (seeded snapshot +
        // the always-on bridge) carries every unsettled adjudication, so the
        // switch-back re-own of v2 has no successor here. The open ask is
        // re-seeded from the fold right below (`sync_live_ask`).
        self.chat.update(cx, |chat, cc| {
            chat.thread_sub = Some(thread_events);
            chat.store_observe = Some(store_changes);
            cc.notify();
        });
        // The thinking ticker belongs to the outgoing thread: bump its
        // generation so the old ticker self-terminates, then mirror the incoming
        // thread's running state. A parked thread resumed mid-turn keeps the
        // "for Xs" counter live; a completed history thread is idle.
        self.chat.update(cx, |chat, cx| {
            chat.turn_active = running;
            cx.notify();
        });
        if running {
            self.spawn_thinking_ticker(cx);
        }
        // The goal elapsed ticker is per-thread too, and unlike the thinking
        // ticker it has no `running` mirror to read: re-arm it from the
        // incoming thread's goal projection, because a parked thread's
        // `GoalChanged` never reached the foreground handler.
        let goal_elapsed_live = self.goal_elapsed_is_live(cx);
        self.rearm_goal_ticker(goal_elapsed_live, cx);
        // Cockpit state is per-thread: the outgoing thread's plan,
        // running-tool title, and per-model counter state do not apply to the
        // incoming one. The execution plan, unlike the proposed-plan review,
        // IS recoverable: re-derived from the transcript's `UpdatePlan` tool
        // calls, falling back to the independent sidecar snapshot (the facade
        // mirrors the persisted copy on every `PlanUpdated` / `Ready`).
        let restored_plan = plan_from_messages.or_else(|| {
            self.chat.read(cx).store.clone().and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, &sid)
                    .ext
                    .and_then(|x| x.plan.clone())
                    .and_then(|v| serde_json::from_value::<manox_agent::plan::PlanSnapshot>(v).ok())
            })
        });
        let rail_leaf = self.chat.read(cx).store.clone();
        self.chat_rail(cx).update(cx, |r, cx| {
            // Rail-freeze fix (the visual-acceptance report): the store is
            // the rail's only read face (U7b), and the SessionStatus deltas
            // and info-fetch responses feeding it only reach the ATTACHED
            // session's leaf — re-bind to the incoming leaf (already swapped
            // on `self.chat.store` above) so the status row and the usage
            // sections track the live thread instead of the
            // construction-time one.
            r.bind_store(rail_leaf, cx);
            // U7b: the rail's reads are store-only; the switch reset below
            // clears the per-thread cockpit state.
            r.reset_for_thread_switch(running, cx);
            if let Some(snapshot) = restored_plan {
                r.set_plan(snapshot, cx);
            }
            cx.notify();
        });
        // Recover the settled sub-agent observation rows for the incoming
        // thread after the rail reset above (a restoring thread has no
        // messages yet — its rows land with the `HistoryRestored` rebuild).
        self.apply_subagent_rows(subagent_rows, cx);
        // Replay the window's live-only tail (sub-agent child/progress rows, an
        // in-flight tool's streamed output, a live retry notice): the parked
        // subscription drops all of it, and none of it has a display
        // projection, so the rebuild above cannot reproduce it.
        self.catch_up_live_only_state(cx);
        // The user is now viewing this thread: clear any unread red dot it
        // carried from a prior background completion, and any pending-auth
        // badge. Re-surfacing a parked interaction rides the re-own above: the
        // gateway replays unsettled adjudications to the joining owner
        // (manox §D.6), which re-arms the card through the live
        // `ToolCallAuthorization` handler. An in-place reclaim that skipped the
        // re-own left that card unrenderable — the parked subscription never
        // sees it, and no rebuild path can synthesize one from the store (the
        // leaf keeps reply ids, not the ask payload).
        let store = manox_agent::thread_store_global();
        store.with_mut(|s| {
            s.set_unread(&id, false);
            s.mark_pending_auth(&id, false);
        });
        // The incoming thread's cwd / worktree may differ from the outgoing
        // one; refresh the rail's git stats/branch display for it.
        self.spawn_git_status_refresh(cx);
        self.view_mode = ViewMode::Workspace;
        cx.notify();
    }

    /// Archive the active thread and navigate to a fresh empty one. Shared
    /// by the `/exit` slash command and the `cmd-;` keybinding. No-op while
    /// a turn is running (a parked thread would strand pending verdicts).
    pub(crate) fn archive_current_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.archive_active_thread_if_idle(cx) {
            return;
        }
        self.start_new_thread(None, window, cx);
    }

    /// Archive the active thread if it is idle; `false` when a turn is
    /// running (attaching would park the thread and clear `pending_ask`,
    /// stranding a user verdict). Marks the thread archived and persists via
    /// the store, which refreshes the sidebar list.
    pub(super) fn archive_active_thread_if_idle(&mut self, cx: &mut Context<Self>) -> bool {
        if self
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
            })
        {
            return false;
        }
        let id = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(_, sid)| sid.clone())
            .unwrap_or_else(|| {
                tracing::debug!("foreground store not bound yet (ahp handshake in flight)");
                Default::default()
            });
        self.with_foreground_store(cx, |store, sid| {
            store.set_archived(&sid, true);
        });
        let store = manox_agent::thread_store_global();
        store.with_mut(|s| s.archive_thread(&id, true));
        true
    }

    /// Archive the active thread and open a fresh one that inherits the
    /// outgoing thread's project, model, permission mode, and reasoning effort —
    /// `/new` starts a clean conversation without dropping the working
    /// context. No-op while a turn is running (see
    /// `archive_active_thread_if_idle`).
    pub(crate) fn archive_current_thread_inheriting(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.archive_active_thread_if_idle(cx) {
            return;
        }
        let old = self.chat.read(cx).thread.clone();
        let cwd = self
            .chat
            .read(cx)
            .store
            .clone()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, &sid)
                    .cwd()
                    .map(std::path::PathBuf::from)
            })
            .unwrap_or_else(|| old.read(|t| t.cwd().to_path_buf()));
        let project = self
            .chat
            .read(cx)
            .store
            .clone()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, &sid)
                    .cwd()
                    .map(std::path::PathBuf::from)
            })
            .or_else(|| old.read(|t| t.project().cloned()));
        let model = old.read(|t| t.model().cloned());
        let effort_str = self.chat.read(cx).store.clone().and_then(|(store, sid)| {
            let view = store.read(cx);
            crate::ahp_store::leaf(&view.book, &sid)
                .reasoning_effort()
                .map(str::to_string)
        });
        let approval = self.chat.read(cx).store.clone().and_then(|(store, sid)| {
            let view = store.read(cx);
            crate::ahp_store::leaf(&view.book, &sid)
                .approval_mode()
                .map(str::to_string)
        });
        // U6b③: the inherited state rides the v2 CreateSession intent —
        // the compat-note create this replaces carried only the cwd, so
        // the parked model/project/effort/permission never reached the
        // server (the #765 round-2 inheritance defect's shape); the
        // receipt's minted id attaches through the standard
        // created-session path.
        let cwd_str = cwd.to_string_lossy().to_string();
        let model_str = model.map(|m| format!("{}/{}", m.provider, m.id));
        let ws = cx.weak_entity();
        let dir_for_store = project.clone();
        let sid = uuid::Uuid::new_v4().to_string();
        let mut config = serde_json::Map::new();
        if let Some(model) = &model_str {
            config.insert("model".into(), serde_json::json!(model));
        }
        if let Some(approval) = &approval {
            config.insert("approvalMode".into(), serde_json::json!(approval));
        }
        if let Some(effort) = &effort_str {
            config.insert("reasoningEffort".into(), serde_json::json!(effort));
        }
        let cwds = (project.is_none())
            .then(|| format!("file://{cwd_str}"))
            .into_iter()
            .collect();
        let reply = self.with_foreground_store(cx, |store, _| {
            store.create_session(&sid, cwds, Some(config))
        });
        let sid2 = sid.clone();
        cx.spawn(async move |_, cx| {
            let failed = match reply {
                Some(reply) => matches!(reply.recv().await, Ok(Err(_))),
                None => true,
            };
            if failed {
                tracing::warn!("inheriting create failed");
                return;
            }
            if let Some(dir) = &dir_for_store {
                let _ = ws.update(cx, |_ws, cx| {
                    Workspace::register_project_in_store(dir, cx);
                });
            }
            let _ = ws.update_in(cx, |this, window, cx| {
                this.attach_created_session(&sid2, window, cx);
            });
        })
        .detach();
        let _ = window;
    }

    /// Park the active thread into the background (preserving its run + event
    /// subscriptions) and open a fresh empty thread in the same project — the
    /// explicit "background this task, switch to a new one" gesture, bound to
    /// `ctrl-b`. No-op when the active thread is idle so the shortcut can't
    /// spam empty threads. `attach_thread` does the actual parking: a running
    /// thread is moved into `background_threads` with a terminal-`Stop`/`Error`
    /// subscription that flips its sidebar row to idle + unread when it lands.
    pub(super) fn background_current_thread(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (running, project) = {
            let pair = self.chat.read(cx).store.clone();
            match pair {
                Some((store, sid)) => {
                    let view = store.read(cx);
                    let leaf = crate::ahp_store::leaf(&view.book, &sid);
                    (leaf.running(), leaf.cwd().map(std::path::PathBuf::from))
                }
                None => return,
            }
        };
        if !running {
            return;
        }
        self.start_new_thread(project, window, cx);
    }

    pub fn open_thread(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        // If the thread is already running in the background, reclaim it
        // instead of loading a stale snapshot from the db.
        // U6b⑤: the reclaim re-attaches a fresh landing mirror for the
        // parked id — `attach_thread`'s own reclaim branch finds the park
        // by id and restores its leaf/session. The parked session never
        // detached, so no stream is re-opened; `attach_thread` still re-owns
        // the session (`OpenSession`, §D.6) so a parked adjudication card
        // re-arms on the way back in.
        if self.background_threads.iter().any(|b| b.id == id) {
            let thread = Thread::landing_with_id(ThreadId(id), self.cwd.clone());
            self.attach_thread(thread, true, window, cx);
            return;
        }
        // U6b②: the attach read is the landing mirror — the SERVER owns
        // the session and the restore rides the wire reopen flow
        // (OpenSession + the follow stream's Snapshot + the §D.5 mirrors),
        // exactly like the create path. The kernel-side `load_thread` (the
        // U6 dual source: a db-restored facade racing the live wire state)
        // is gone; the row this click came from is itself a wire item, so
        // the id is server-known by construction.
        let thread = Thread::landing_with_id(ThreadId(id), self.cwd.clone());
        self.attach_thread(thread, true, window, cx);
    }

    /// Fork the current session at a durable entry, then open the child
    /// (the `createChat` fork command, #775).
    ///
    /// The child is a prefix copy of this session's active chain through
    /// `through_entry_id`; it lands as an independent sidebar row. Failure
    /// leaves the current view untouched — a fork that cannot be created must
    /// not disturb the transcript the user is reading.
    pub(crate) fn fork_session_at(&mut self, through_entry_id: &str, cx: &mut Context<Self>) {
        // The source is read exactly where the row's anchor was stamped
        // (`fork_source`): an entry id is only addressable within the session
        // it was replayed from.
        let Some(_source_session_id) = self.fork_source_session(cx) else {
            tracing::warn!("fork: no session bound, ignoring");
            return;
        };
        // One fork in flight at a time: the call is a round trip, and a second
        // click on any reply would otherwise mint another child for the same
        // intent. The guard clears when the verdict lands (either way); a
        // receipt lost to a transport end is failed by the multiplexer at
        // that boundary. A receipt lost to a hung handler on a living
        // channel is upstream always-answer territory — no timer
        // compensates for it here.
        if self.chat.read(cx).fork_in_flight {
            tracing::debug!("fork: already in flight, ignoring");
            return;
        }
        self.chat.update(cx, |chat, cx| {
            chat.fork_in_flight = true;
            cx.notify();
        });
        let ws = cx.weak_entity();
        let entry_id = through_entry_id.to_string();
        let forked = self.with_foreground_store(cx, |store, sid| store.fork_chat(&sid, &entry_id));
        let (reply, chat_id) = match forked {
            Some(pair) => pair,
            None => return,
        };
        cx.spawn(async move |_, cx| {
            let failed = matches!(reply.recv().await, Ok(Err(_)) | Err(_));
            if failed {
                tracing::warn!("createChat (fork) failed");
                let _ = ws.update(cx, |this, cx| {
                    this.chat.update(cx, |chat, cx| {
                        chat.fork_in_flight = false;
                        cx.notify();
                    });
                    cx.notify();
                });
                return;
            }
            tracing::info!(session_id = %chat_id, "fork landed");
            let _ = ws.update_in(cx, |this, window, cx| {
                this.chat.update(cx, |chat, cx| {
                    chat.fork_in_flight = false;
                    cx.notify();
                });
                this.open_thread(chat_id, window, cx);
            });
        })
        .detach();
    }
}
