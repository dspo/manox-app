//! The `/exit` teardown sequence: stop everything this thread owns, then
//! archive it and land on a fresh thread — the same end state as the thread
//! list's archive action, reached through a force path that also works while
//! a turn is running.
//!
//! The sequence plays as progress notes in the conversation (the
//! annotation-card face `add_info_message` writes), one beat per action so
//! the teardown is legible: subagents, the thread's own turn, the right
//! pane's terminals + CLI-agent tabs + the dock's terminal (their PTY drop
//! is the process-tree kill), the integrated browser, and the archive
//! itself. Notes pushed before the switch land on the archived thread's
//! journal; the closing pair lands on the fresh thread.

use super::*;

/// The pane kinds whose tab content owns a PTY: the integrated terminal and
/// the three CLI-agent TUI tabs (claude / codex / copilot). Closing the tab
/// drops the view, and the view drop tears the process tree down.
const TERMINAL_TAB_KINDS: [&str; 4] = ["terminal", "claude", "codex", "copilot"];

/// The pause between one teardown beat's announcement and its action — long
/// enough for the progress notes to read as a sequence, short enough that
/// `/exit` still feels immediate.
const BEAT: std::time::Duration = std::time::Duration::from_millis(320);

impl Workspace {
    /// Kick off the `/exit` sequence. Everything observable is snapshot here
    /// (running subagents, turn state, the display title); the steps run on a
    /// spawned task so each beat can breathe.
    pub(crate) fn begin_exit_sequence(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((store, sid)) = self.chat_store(cx) else {
            tracing::warn!("/exit dropped: no foreground session is bound");
            return;
        };
        let (title, turn_running) = {
            let view = store.read(cx);
            let leaf = crate::ahp_store::leaf(&view.book, &sid);
            (
                leaf.display_title()
                    .map(str::to_string)
                    .unwrap_or_else(|| i18n::t("exit-untitled-thread").to_string()),
                leaf.running(),
            )
        };
        let subagents = self.running_subagent_names(cx);
        cx.spawn(async move |this, cx| {
            // The teardown must keep targeting the thread `/exit` was invoked
            // on: the beats leave the UI interactive, and every destructive
            // step (cancel, pane/dock teardown, archive) resolves through the
            // LIVE foreground — a sidebar click mid-sequence would otherwise
            // tear down and archive whatever thread is focused now.
            if !foreground_is(&this, cx, &sid) {
                tracing::warn!("/exit aborted: the foreground thread changed mid-sequence");
                return;
            }
            // ── subagents ──────────────────────────────────────────────
            if !subagents.is_empty() {
                this.update(cx, |ws, cx| ws.note("exit-stopping-subagents", &[], cx))
                    .ok();
                beat(cx).await;
                // The child turns are aborted by the host's cancel waterfall
                // below; the observation panels go now so the pane does not
                // carry them into the switch.
                this.update(cx, |_ws, cx| {
                    crate::chrome_assembly::close_tool_tabs_of_kind("subagent", cx);
                })
                .ok();
                for name in &subagents {
                    this.update(cx, |ws, cx| {
                        ws.note("exit-subagent-stopped", &[("name", name)], cx)
                    })
                    .ok();
                }
                beat(cx).await;
            }
            // ── the thread's own turn ──────────────────────────────────
            if turn_running {
                if !foreground_is(&this, cx, &sid) {
                    tracing::warn!("/exit aborted: the foreground thread changed mid-sequence");
                    return;
                }
                this.update(cx, |ws, cx| {
                    ws.note("exit-stopping-agent", &[("name", &title)], cx)
                })
                .ok();
                beat(cx).await;
                this.update(cx, |ws, cx| ws.cancel_turn(cx)).ok();
                this.update(cx, |ws, cx| {
                    ws.note("exit-agent-stopped", &[("name", &title)], cx)
                })
                .ok();
                beat(cx).await;
            }
            // ── terminal subprocesses (pane tabs + the dock) ───────────
            let has_terminals = this
                .update(cx, |_ws, cx| {
                    crate::chrome_assembly::pane_open_kinds(cx)
                        .iter()
                        .any(|k| TERMINAL_TAB_KINDS.contains(&k.as_str()))
                        || crate::chrome_assembly::dock_terminal_live(cx)
                })
                .unwrap_or(false);
            if has_terminals {
                if !foreground_is(&this, cx, &sid) {
                    tracing::warn!("/exit aborted: the foreground thread changed mid-sequence");
                    return;
                }
                this.update(cx, |ws, cx| ws.note("exit-killing-terminal", &[], cx))
                    .ok();
                beat(cx).await;
                // These tab kinds' `close` never re-enters the workspace
                // (store resets + shell-only dock take), so they run inside
                // the lease.
                this.update(cx, |_ws, cx| {
                    for kind in TERMINAL_TAB_KINDS {
                        crate::chrome_assembly::close_tool_tabs_of_kind(kind, cx);
                    }
                    crate::chrome_assembly::stop_dock_terminal(cx);
                })
                .ok();
                this.update(cx, |ws, cx| ws.note("exit-terminal-killed", &[], cx))
                    .ok();
                beat(cx).await;
            }
            // ── the integrated browser ─────────────────────────────────
            // Compiled out with the integrated browser on Linux/FreeBSD —
            // there are never any browser tabs to close.
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            {
                let browser_tabs = this
                    .update(cx, |ws, _cx| ws.browser_views.len())
                    .unwrap_or(0);
                if browser_tabs > 0 {
                    if !foreground_is(&this, cx, &sid) {
                        tracing::warn!("/exit aborted: the foreground thread changed mid-sequence");
                        return;
                    }
                    this.update(cx, |ws, cx| ws.note("exit-closing-browser", &[], cx))
                        .ok();
                    beat(cx).await;
                    // A browser tab's `close` re-enters the workspace (it closes
                    // its webview through `Workspace::close_browser_tab`), so the
                    // pane-side close runs OUTSIDE the workspace lease, on the
                    // bare App; the orphan drain afterwards is lease-local.
                    cx.update(|cx| {
                        crate::chrome_assembly::close_tool_tabs_of_kind("browser", cx);
                    });
                    this.update(cx, |ws, cx| ws.close_all_browser_tabs(cx)).ok();
                    this.update(cx, |ws, cx| ws.note("exit-browser-closed", &[], cx))
                        .ok();
                    beat(cx).await;
                }
            }
            // ── archive + fresh thread ─────────────────────────────────
            if !foreground_is(&this, cx, &sid) {
                tracing::warn!("/exit aborted: the foreground thread changed mid-sequence");
                return;
            }
            let archived_id = this
                .update(cx, |ws, cx| {
                    ws.note("exit-archiving", &[("name", &title)], cx);
                    ws.archive_active_thread_force(cx)
                })
                .unwrap_or(None);
            // The sidebar's optimistic drop (the same half the row menu's
            // archive performs): without it the row lingers until the echo
            // lands — and the fresh-thread landing re-projects the list from
            // the not-yet-echoed summary in between.
            if let Some(id) = archived_id.clone() {
                cx.update(|cx| {
                    crate::chrome_assembly::drop_sidebar_row(&id, cx);
                });
            }
            // The switch lands on the fresh thread; the closing pair rides it.
            // The attach is asynchronous (`start_new_thread` returns the
            // minted id without switching), so the pair is ADDRESSED to that
            // id — a foreground-targeted note here would journal onto the
            // thread just archived.
            let landed = this.update_in(cx, |ws, window, cx| ws.start_new_thread(None, window, cx));
            let fresh_sid = match landed {
                Ok(sid) => sid,
                Err(_) => {
                    tracing::warn!("/exit: the workspace died before the fresh-thread landing");
                    return;
                }
            };
            this.update(cx, |ws, cx| {
                ws.note_for(&fresh_sid, "exit-archived", &[], cx);
                ws.note_for(&fresh_sid, "exit-bye", &[], cx);
            })
            .ok();
        })
        .detach();
    }

    /// Push one progress note into the conversation (the annotation-card
    /// face — durable on the current session's journal, displayed live).
    fn note(&mut self, key: &str, args: &[(&str, &str)], cx: &mut Context<Self>) {
        self.add_info_message(i18n::t_str(key, args).to_string(), cx);
    }

    /// The session-addressed note: same card, but journaled on `sid` rather
    /// than the live foreground — for beats that must land on a thread the
    /// workspace has not switched to yet.
    fn note_for(&mut self, sid: &str, key: &str, args: &[(&str, &str)], cx: &mut Context<Self>) {
        self.append_ui_note_for(
            sid,
            manox_agent::db::UiNoteKind::Notice,
            i18n::t_str(key, args).to_string(),
            cx,
        );
        cx.notify();
    }

    /// The running subagents' display names (task description, falling back
    /// to the agent type, then the address) — the `/exit` per-agent lines.
    fn running_subagent_names(&self, cx: &App) -> Vec<String> {
        self.chat
            .read(cx)
            .conversation
            .read(cx)
            .items()
            .iter()
            .filter_map(|item| match item.read(cx).kind() {
                ConvItem::AgentTask(t) if t.status == manox_agent::ToolCallStatus::Running => {
                    let name = if !t.description.is_empty() {
                        t.description.clone()
                    } else if !t.subagent_type.is_empty() {
                        t.subagent_type.clone()
                    } else {
                        t.id.clone()
                    };
                    Some(name)
                }
                _ => None,
            })
            .collect()
    }

    /// Archive the active thread WITHOUT the idle gate — the `/exit` force
    /// path, sharing `archive_thread_now`'s body with the gated command. The
    /// caller has already cancelled the turn (the host's waterfall settles
    /// pending verdicts and ask cards), which is the hazard the idle guard in
    /// `archive_active_thread_if_idle` exists for. Returns the archived
    /// session id (the caller's sidebar-reconciliation key).
    fn archive_active_thread_force(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let pair = self.chat.read(cx).store.clone();
        let Some((_, sid)) = &pair else {
            return None;
        };
        let sid = sid.clone();
        self.archive_thread_now(&sid, cx);
        Some(sid)
    }
}

/// Pause one beat between teardown steps (the visible pacing of `/exit`).
async fn beat(cx: &mut gpui::AsyncApp) {
    cx.background_executor().timer(BEAT).await;
}

/// Whether the foreground is still the thread `/exit` was invoked on — the
/// gate every destructive step passes through (see the task body).
fn foreground_is(this: &WeakEntity<Workspace>, cx: &mut gpui::AsyncApp, sid: &str) -> bool {
    this.update(cx, |ws, cx| {
        ws.chat_store(cx).is_some_and(|(_, live)| live == sid)
    })
    .unwrap_or(false)
}
