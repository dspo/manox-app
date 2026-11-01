//! The container lifecycle gestures (plan D7): the ONE path per verb.
//!
//! - **stop** (`stop_container`, the composer's 停止 chip): the foreground
//!   container's `x-manox/teardown` stop-all — the user's explicit stop
//!   kills every live run and background task in the container, DELIBERATELY
//!   overriding the runtime's "background runs survive turn abort" default
//!   (that default governs turn-level interrupts; the stop control is the
//!   coarser, container-level gesture). There is no parallel client-side
//!   kill sweep: the command IS the path, the receipt and the fold echoes
//!   drive the UI.
//! - **archive** (`confirm_archive_containers` → `archive_containers`): an
//!   archive gesture on containers with LIVE dependents first shows a
//!   confirmation listing them; confirming — or a dependent-less archive,
//!   no dialog — lands the same ONE teardown command in archive mode, whose
//!   optimistic half drops the whole tree and retires the object rows. The
//!   retired per-site drops (`AhpStore::archive_sessions`' archive arm, the
//!   shell menu's own reconcile, `/exit`'s explicit row drop) all funnel
//!   here.
//! - **unarchive** (`revive_container`): the whole-tree revive — the
//!   captain's children come back with it.
//!
//! External containers keep the D8 face: close = kill
//! (`teardown_container`, wired by A2), no archive gesture.

use super::*;

/// What follows a confirmed container archive — the fresh container a
/// leaving gesture (`cmd-;`, `/new`) lands on; `None` for gestures that only
/// archive (the row menu, the batch bar, `/exit`'s own landing beat). Shared
/// (`Arc`) because the dialog's builder face may re-run; the dialog closes on
/// the landing, so in practice it fires once.
pub(crate) type ArchiveLanding =
    std::sync::Arc<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

impl Workspace {
    /// The composer's stop control: stop-all on the foreground container
    /// (D7). The observation panels show runs the stop kills — they close
    /// now, so the pane does not carry dead panels; a parked ask card
    /// dismisses with the turn it belonged to (the host's cancel waterfall
    /// settles it). Everything else — the runs, the roster marks, the
    /// composer's running face — follows the command's echoes.
    pub(crate) fn stop_container(&mut self, cx: &mut Context<Self>) {
        let Some((_, sid)) = self.chat_store(cx) else {
            tracing::warn!("stop dropped: no foreground session is bound");
            return;
        };
        // B2-PR-3: a parked question card is dismissed on the interrupt —
        // the stop kills the turn it belongs to, and a card left open would
        // block the composer on a session that is no longer running.
        if self.chat.read(cx).pending_ask.is_some() {
            self.dismiss_ask(cx);
        }
        crate::chrome_assembly::close_tool_tabs_of_kind("subagent", cx);
        self.multiplexer.update(cx, |mux, cx| {
            mux.stop_container(&sid, cx);
        });
        cx.notify();
    }

    /// The row menu's archive toggle, container-semantics edition: a dead
    /// container revives (whole-tree); a live one goes through the
    /// confirm-or-direct archive entry. The id names a native container —
    /// external rows carry the close gesture instead (D8: no archive). The
    /// archived read is the same face the row rendered with
    /// (`effective_archived`), so the menu can never flip against the row
    /// the user clicked.
    pub(crate) fn toggle_container_archived(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.multiplexer.read(cx).store();
        let book = &store.read(cx).book;
        // Neither source knowing the id refuses the gesture: callers pass
        // live rows (the menu only renders on projected rows), and a
        // dispatch into the void would only land in the diagnostics deque.
        let Some(summary) = book.summaries.get(id) else {
            return;
        };
        let archived = crate::multiplexer::effective_archived(book, id, summary);
        if archived {
            self.multiplexer.update(cx, |mux, cx| {
                mux.revive_container(id, cx);
            });
            return;
        }
        self.confirm_archive_containers(vec![id.to_string()], None, window, cx);
    }

    /// The archive gestures' shared entry: confirm with the containers'
    /// live dependents when there are any, otherwise archive straight away.
    /// `landing` is what follows the confirmed archive — the fresh container
    /// a leaving gesture (`cmd-;`, `/new`) lands on.
    pub(crate) fn confirm_archive_containers(
        &mut self,
        ids: Vec<String>,
        landing: Option<ArchiveLanding>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lines: Vec<SharedString> = self.live_dependent_lines(&ids, cx);
        if lines.is_empty() {
            self.archive_containers(&ids, cx);
            if let Some(landing) = landing {
                landing(self, window, cx);
            }
            return;
        }
        let ws = cx.weak_entity();
        let ids = std::sync::Arc::new(ids);
        window.open_alert_dialog(cx, move |alert, _, _| {
            // The builder may re-run: every face clones its own handles, the
            // on_ok closure moves them out of THIS builder run.
            let ids = ids.clone();
            let ws = ws.clone();
            let landing = landing.clone();
            let description = v_flex()
                .gap_2()
                .child(i18n::t("archive-confirm-subtitle"))
                .child(
                    v_flex().gap_1().children(
                        lines
                            .iter()
                            .map(|line| gpui::div().child(line.clone()))
                            .collect::<Vec<_>>(),
                    ),
                );
            alert
                .title(i18n::t("archive-confirm-title"))
                .description(description)
                .show_cancel(true)
                .ok_text(i18n::t("archive-confirm-ok"))
                .cancel_text(i18n::t("archive-confirm-cancel"))
                .on_ok(move |_, window, cx| {
                    ws.update(cx, |ws, cx| {
                        ws.archive_containers(&ids, cx);
                        if let Some(landing) = &landing {
                            landing(ws, window, cx);
                        }
                    })
                    .is_ok()
                })
        });
    }

    /// THE container archive path (plan A6): every archive gesture's effects
    /// ride this one sequence, replacing the three retired optimistic-drop
    /// sites. 1. the `x-manox/teardown` archive command — the path itself —
    /// with the store's optimistic whole-tree row drop and the object rows'
    /// archived bits (the draft refill must never resurrect a retired
    /// container); 2. the thread-store mirror's bit (the badge/mirror
    /// faces); 3. the sidebar's shell-side row drop; 4. the predicate-flip
    /// event and the resource reclaim under each container's key (the A2
    /// teardown).
    pub(crate) fn archive_containers(&mut self, ids: &[String], cx: &mut Context<Self>) {
        self.multiplexer.update(cx, |mux, cx| {
            mux.archive_containers(ids, cx);
        });
        for id in ids {
            manox_agent::thread_store_global().with_mut(|s| s.archive_thread(id, true));
        }
        for id in ids {
            crate::chrome_assembly::drop_sidebar_row(id, cx);
        }
        for id in ids {
            // Archive IS the native container's death (D7): raise the
            // predicate-flip event and run the ONE resource teardown. The
            // workspace's own subscription ignores native ids (the external
            // home's process exit is its only event-driven death), so the
            // explicit call here is the reclaim, not a duplicate.
            cx.emit(container::ContainerEvent::Died(id.clone()));
            self.teardown_container(id, cx);
        }
        cx.notify();
    }

    /// The confirm dialog's content: one line per live dependent across the
    /// named containers. Empty = nothing live: the archive needs no
    /// confirmation.
    fn live_dependent_lines(&self, ids: &[String], cx: &App) -> Vec<SharedString> {
        let store = self.multiplexer.read(cx).store();
        dependent_lines(&store.read(cx).book, ids)
    }
}

/// One line per live dependent across `ids`, from the folded book: running
/// subagent rows (the persistent roster's live face, joined with the
/// live-only backgroundWork entries the pre-#989 runtime carries; deduped by
/// display name so a run the two faces both report lists once) and background
/// shell entries. The pure core of the archive confirmation — testable
/// against a hand-built book, the `container_rows` precedent.
fn dependent_lines(
    book: &steer_agent_chat_ui::ahp_store::ChannelBook,
    ids: &[String],
) -> Vec<SharedString> {
    let mut lines: Vec<SharedString> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for id in ids {
        // The roster's live rows (upstream #989's persistent registry —
        // settled rows are records, not work).
        for run in steer_agent_chat_ui::ahp_store::child_runs_of(book, id) {
            if run.status != steer_agent_chat_ui::ahp_store::ChildRunStatus::Running {
                continue;
            }
            let name = if !run.label.is_empty() {
                run.label
            } else if !run.kind.is_empty() {
                run.kind
            } else {
                run.run_id
            };
            if seen.insert(name.clone()) {
                lines.push(i18n::t_str("archive-confirm-subagent", &[("name", &name)]));
            }
        }
        // The live-only backgroundWork face: subagent entries the roster
        // cannot report yet (the pinned runtime), and the background shell
        // entries (live-only by construction).
        let leaf = crate::ahp_store::leaf(book, id);
        for row in leaf.running_subagents() {
            let name = if !row.label.is_empty() {
                row.label
            } else {
                row.id
            };
            if seen.insert(name.clone()) {
                lines.push(i18n::t_str("archive-confirm-subagent", &[("name", &name)]));
            }
        }
        if let Some(chat) = leaf.chat
            && let Some(work) = chat.background_work.as_ref()
        {
            for entry in work {
                if let ahp_types::state::BackgroundWork::Shell(shell) = entry {
                    let name = if !shell.label.is_empty() {
                        shell.label.clone()
                    } else {
                        shell.command.clone()
                    };
                    if seen.insert(name.clone()) {
                        lines.push(i18n::t_str("archive-confirm-bash", &[("name", &name)]));
                    }
                }
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use steer_agent_chat_ui::ahp_store::{
        ChannelBook, ChildRunRow, ChildRunStatus, chat_uri, work_uri,
    };

    fn roster_row(run_id: &str, label: &str, status: ChildRunStatus) -> ChildRunRow {
        ChildRunRow {
            run_id: run_id.to_string(),
            kind: "Explore".to_string(),
            label: label.to_string(),
            color: None,
            status,
            verdict: None,
            started_at: "2026-10-30T00:00:00.000Z".to_string(),
            chat: format!("ahp-chat:/{run_id}"),
        }
    }

    /// The confirm gate's data face (plan D7): live roster rows and
    /// background shells name themselves in the dialog; settled roster rows
    /// and absent work read as nothing live (no confirmation). The SYNTHETIC
    /// fixtures cover both wire faces — the roster rides the work channel's
    /// `childRunsChanged` registry (upstream #989, post-bump) and the
    /// background shells the live-only chat face.
    #[test]
    fn the_confirm_gate_counts_live_dependents_only() {
        let mut book = ChannelBook::default();
        let mut runs = std::collections::BTreeMap::new();
        runs.insert(
            "run-live".to_string(),
            roster_row("run-live", "map the edges", ChildRunStatus::Running),
        );
        runs.insert(
            "run-settled".to_string(),
            roster_row("run-settled", "already done", ChildRunStatus::Settled),
        );
        book.child_runs.insert(work_uri("cap"), runs);
        let chat: ahp_types::state::ChatState = serde_json::from_value(serde_json::json!({
            "resource": chat_uri("cap"),
            "title": "cap",
            "status": 0,
            "modifiedAt": "2026-10-30T00:00:00.000Z",
            "turns": [],
            "backgroundWork": [
                {
                    "kind": "shell",
                    "id": "sh-1",
                    "label": "tail -f build.log",
                    "command": "tail -f build.log",
                    "startedAt": "2026-10-30T00:00:00.000Z",
                },
                {
                    "kind": "subagent",
                    "id": "sub-dup",
                    "label": "map the edges",
                    "chat": "ahp-chat:/sub-dup",
                    "startedAt": "2026-10-30T00:00:00.000Z",
                },
            ],
        }))
        .expect("the chat with live work parses");
        book.chats.insert("cap".to_string(), chat);

        let lines = dependent_lines(&book, &["cap".to_string()]);
        // The settled roster row is a record, not work; the live subagent is
        // reported by BOTH faces and lists ONCE (deduped by display name);
        // the background shell lists.
        assert_eq!(lines.len(), 2, "live subagent (deduped) + live shell");
        assert!(lines.iter().any(|l| l.contains("map the edges")));
        assert!(lines.iter().any(|l| l.contains("tail -f build.log")));

        // No roster rows, no background work: nothing live, no confirmation.
        let quiet = ChannelBook::default();
        assert!(dependent_lines(&quiet, &["cap".to_string()]).is_empty());
    }
}
