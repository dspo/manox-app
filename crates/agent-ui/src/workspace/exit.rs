//! The `/exit` teardown sequence: stop everything this container owns, then
//! archive it and land on a fresh container — the same end state as the
//! sidebar's archive action, reached through a force path that also works
//! while a turn is running.
//!
//! The sequence plays as progress notes in the conversation (the
//! annotation-card face `add_info_message` writes), one beat per action so
//! the teardown is legible: the subagent observation panels, the right
//! pane's terminals + CLI-agent tabs + the dock's terminal + the external
//! containers (the ONE teardown kills their processes and drops their rows),
//! the container's own browser tabs (other containers' tabs survive — the
//! reclaim is keyed, not a global nuke), and then the landing beat: ONE
//! container teardown (archive mode — the `x-manox/teardown` command, whose
//! host waterfall cancels the turn, interrupts the live runs, settles the
//! background tasks and cascades the archive; the path's optimistic half has
//! already emptied the container's UI) plus the fresh container through the
//! unified creation entry. Notes pushed before the switch land on the
//! archived container's journal; the closing pair lands on the fresh one.

use super::*;

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
            // ── the container's own turn ───────────────────────────────
            // The turn's cancel rides the teardown command (the ONE path —
            // `/exit` does not send a parallel turn-cancel write); the
            // beats only announce what the waterfall does.
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
                this.update(cx, |ws, cx| {
                    ws.note("exit-agent-stopped", &[("name", &title)], cx)
                })
                .ok();
                beat(cx).await;
            }
            // ── terminal subprocesses (pane tabs + the dock + externals) ──
            let has_terminals = this
                .update(cx, |ws, cx| {
                    crate::chrome_assembly::pane_open_kinds(cx)
                        .iter()
                        .any(|k| crate::tool_tabs::PTY_TAB_KINDS.contains(&k.as_str()))
                        || crate::chrome_assembly::dock_terminal_live(cx)
                        || !ws.external_container_ids().is_empty()
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
                // The PTY tab kinds' `close` never re-enters the workspace
                // (store resets + shell-only dock take), so they run inside
                // the lease. The external containers join the sweep here
                // (the ONE teardown: kill the process, drop the row,
                // reclaim the resources — D8's close semantics on /exit's
                // way down).
                this.update(cx, |ws, cx| {
                    for kind in crate::tool_tabs::PTY_TAB_KINDS {
                        crate::chrome_assembly::close_tool_tabs_of_kind(kind, cx);
                    }
                    crate::chrome_assembly::stop_dock_terminal(cx);
                    for id in ws.external_container_ids() {
                        ws.teardown_container(&id, cx);
                    }
                })
                .ok();
                this.update(cx, |ws, cx| ws.note("exit-terminal-killed", &[], cx))
                    .ok();
                beat(cx).await;
            }
            // ── the integrated browser (this container's tabs only) ────
            // Compiled out with the integrated browser on Linux/FreeBSD —
            // there are never any browser tabs to close.
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            {
                let browser_tabs = this
                    .update(cx, |ws, _| ws.container_browser_tabs(&sid).len())
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
                    // bare App; the orphan drain afterwards is lease-local. The
                    // leg is container-scoped: the pane belongs to the outgoing
                    // container (foreground), and the workspace half reclaims
                    // only the tabs registered under it — other containers'
                    // browser tabs survive the /exit.
                    cx.update(|cx| {
                        crate::chrome_assembly::close_tool_tabs_of_kind("browser", cx);
                    });
                    this.update(cx, |ws, cx| ws.close_container_browser_tabs(&sid, cx))
                        .ok();
                    this.update(cx, |ws, cx| ws.note("exit-browser-closed", &[], cx))
                        .ok();
                    beat(cx).await;
                }
            }
            // ── the container teardown + the fresh container ──────────
            // One teardown (archive mode) through THE container archive
            // path — the command's host waterfall cancels the turn,
            // interrupts the live runs, settles the background tasks and
            // cascades the archive down the parent edges, while the path's
            // optimistic half (the book drop, the object rows, the sidebar
            // row, the resource reclaim) has already emptied the container's
            // UI. The force path needs no confirm dialog: the beats above
            // ARE the announcement.
            if !foreground_is(&this, cx, &sid) {
                tracing::warn!("/exit aborted: the foreground thread changed mid-sequence");
                return;
            }
            this.update(cx, |ws, cx| {
                ws.note("exit-archiving", &[("name", &title)], cx);
                ws.archive_containers(std::slice::from_ref(&sid), cx);
            })
            .ok();
            // The switch lands on the fresh container; the closing pair
            // rides it. The attach is asynchronous (`new_container` returns
            // the minted id without switching), so the pair is ADDRESSED to
            // that id — a foreground-targeted note here would journal onto
            // the container just archived.
            let landed = this.update_in(cx, |ws, window, cx| ws.new_container(None, window, cx));
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
