//! The chrome-shell assembly for the real app (PLAN-CHROME-CHAT-SPLIT
//! Phase 4; the sole shell since the legacy workspace shell retired). This
//! module owns everything the app mounts:
//!
//! - the session list fed by the REAL multiplexer (the server wire rows
//!   projected through `sidebar_projection` — five states, team forest,
//!   tags, project grouping);
//! - pin/archive hooks routed through the thread store (the next list
//!   snapshot reconciles);
//! - the right-pane tool registry and bottom dock (the integrated-terminal
//!   adapters in `tool_tabs`);
//! - the main surface: the workspace's conversation column (the workspace
//!   keeps the data face — multiplexer, chat, browser views, tool-tab
//!   adapters);
//! - the shell handle itself, published process-wide so host-driven pane
//!   opens (the agent's browser tab, a sub-agent observation panel) can reach
//!   the live pane with no view-tree path to it.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Styled as _,
    WeakEntity, Window,
};
use gpui_component::{Root, WindowExt as _, notification::Notification};
use steer_agent_chrome_ui::right_pane::ToolTab;
use steer_agent_chrome_ui::{
    CustomizationRow, FixedRow, HostHooks, MainSurface, Shell, ShellConfig, icons,
};

use crate::Workspace;
use crate::tool_tabs::{ThreadTerminalPanelSurface, registry};
use crate::views::subagent_panel::SubagentPanel;

/// The live shell, for pane opens that originate OUTSIDE the view tree (an
/// agent-opened browser tab, a sub-agent observation panel requested from the
/// conversation). Replaced on every window open; a stale handle simply fails
/// its `update`, the same tolerance `dispatch::WINDOW` has.
static SHELL: RwLock<Option<WeakEntity<Shell>>> = RwLock::new(None);

fn shell_handle() -> Option<Entity<Shell>> {
    match SHELL.read() {
        Ok(slot) => slot.as_ref().and_then(WeakEntity::upgrade),
        Err(poisoned) => poisoned.into_inner().as_ref().and_then(WeakEntity::upgrade),
    }
}

/// Open (or focus) a host-built tool tab in the live right pane. `false` when
/// no shell or window is live — the caller's gesture has no surface then.
pub fn open_tool_tab(tab: Arc<dyn ToolTab>, cx: &mut App) -> bool {
    let Some(shell) = shell_handle() else {
        return false;
    };
    let Some(handle) = crate::dispatch::window_global() else {
        return false;
    };
    handle
        .update(cx, |_, window, cx| {
            shell.update(cx, |shell, cx| {
                shell
                    .right
                    .update(cx, |pane, cx| pane.open_tab(tab, window, cx));
            });
        })
        .is_ok()
}

/// Close every open tab of `kind` in the live pane; `false` when no shell or
/// window is live. The workspace retires the ephemeral observation panels
/// through this when it leaves a thread: a panel's content is the child
/// transcript ACCUMULATED SO FAR, so one carried across a thread switch would
/// come back stale (the transcript catches up, the panel does not). Dropping
/// them keeps the invariant "a restored panel == the thread's transcript at
/// open time".
pub fn close_tool_tabs_of_kind(kind: &str, cx: &mut App) -> bool {
    let Some(shell) = shell_handle() else {
        return false;
    };
    let Some(handle) = crate::dispatch::window_global() else {
        return false;
    };
    handle
        .update(cx, |_, window, cx| {
            shell.update(cx, |shell, cx| {
                shell
                    .right
                    .update(cx, |pane, cx| pane.close_kind(kind, window, cx));
            });
        })
        .is_ok()
}

/// The open observation panel for a sub-agent address, when one is mounted in
/// the live pane — how the workspace streams a child event into a panel the
/// shell owns.
pub fn subagent_panel(address: &str, cx: &App) -> Option<Entity<SubagentPanel>> {
    let shell = shell_handle()?;
    shell
        .read(cx)
        .right
        .read(cx)
        .tab_entity::<SubagentPanel>(&crate::tool_tabs::subagent_tab_id(address))
}

/// Build the whole chrome window root: multiplexer + shell + the projection
/// pump. Called from the manox bin on every window open.
pub fn mount(window: &mut Window, cx: &mut App) -> Entity<Shell> {
    // ONE workspace carries the whole data face — its multiplexer feeds both
    // the sidebar projection and the conversation column mounted as the
    // shell's main surface. A re-opened window mounts a fresh shell over the
    // SURVIVING workspace (its entity is the process-lifetime one
    // `dispatch::WORKSPACE` holds, so the foreground thread, parked
    // background threads and drafts outlive the window being closed).
    let ws = match crate::dispatch::workspace_global() {
        Some(ws) => ws,
        None => {
            let ws = cx.new(|cx| Workspace::new(window, cx));
            crate::dispatch::set_workspace(ws.clone());
            ws
        }
    };
    let mux = ws.read(cx).multiplexer.clone();
    // The process-lifetime registries the app wires at startup:
    // the dispatch slot (dock badge pump + reopen-focus read it) and the
    // process-wide browser host (IPC routing + the agent's web capability).
    crate::dispatch::set_workspace(ws.clone());
    {
        let ws = ws.clone();
        cx.spawn(async move |cx| {
            crate::browser_host::WorkspaceBrowserHost::install(ws.clone(), cx);
            // The install lands asynchronously, so a tab restored from
            // threads.db before it (startup) missed its route registration:
            // re-register every live view now that the host exists.
            if let Some(host) = crate::browser_host::WorkspaceBrowserHost::concrete() {
                let ids = ws.read_with(cx, |ws, _| {
                    ws.browser_views.keys().copied().collect::<Vec<_>>()
                });
                for id in ids {
                    host.register_ui_tab(id);
                }
            }
        })
        .detach();
    }
    let shell = cx.new(|cx| Shell::new(shell_config(ws.clone(), &mux, cx), window, cx));
    let shell_weak = shell.downgrade();
    match SHELL.write() {
        Ok(mut slot) => *slot = Some(shell.downgrade()),
        Err(poisoned) => *poisoned.into_inner() = Some(shell.downgrade()),
    }
    // Per-thread dock state: each visited thread keeps its live panel
    // terminal across switches (the right-pane stash semantic — a
    // stashed view keeps its process running; only explicit collapse of the
    // LIVE dock tears one down).
    //
    // LIFETIME (accepted difference, 2026-09-28): these stashes belong to the
    // SHELL, so they die with the window — a tray close + reopen leaves the
    // dock's terminal and the right pane's terminal/CLI tabs gone (browser and
    // editor tabs come back from `threads.db`). The legacy shell kept the same
    // state on the (process-lifetime) workspace; the chrome shell has always
    // scoped it to the window, and this build still keeps strictly more than
    // the chrome build it replaces (that one also lost the workspace itself —
    // foreground thread, drafts, parked threads — on every reopen). Moving the
    // stashes onto the workspace is the contained follow-up that would match
    // the legacy lifetime; it is not in this retirement's scope.
    let mut dock_stash: HashMap<String, gpui::AnyView> = HashMap::new();
    let mut dock_thread: Option<String> = None;
    // Per-thread right-pane sessions: the open tab set + content store +
    // active tab + visibility move with the foreground thread (stash on
    // switch-out, restore on switch-in; a thread with no in-session stash
    // falls back to its `threads.db` snapshot — the pane's durable account —
    // and starts on the fresh new-tab page only when
    // neither exists).
    let mut right_stash: HashMap<String, steer_agent_chrome_ui::right_pane::RightPaneSession> =
        HashMap::new();
    // Persist the pane on every change: the shell's own notify is the
    // change signal (tab open/close/activate, visibility, dock), so an
    // upsert here keeps the durable row in step without the pane knowing
    // about persistence.
    {
        let ws = ws.clone();
        let shell_watch = shell.clone();
        let observed = shell_watch.clone();
        cx.observe(&observed, move |_shell, cx| {
            persist_right_pane(&shell_watch, &ws, cx);
        })
        .detach();
    }
    cx.observe(&ws, move |ws, cx| {
        let Some(shell) = shell_weak.upgrade() else {
            return;
        };
        let mux = ws.read(cx).multiplexer.clone();
        // The process-lifetime registries the app wires at startup:
        // the dispatch slot (dock badge pump + reopen-focus read it) and the
        // process-wide browser host (IPC routing + the agent's web capability).
        crate::dispatch::set_workspace(ws.clone());
        {
            let ws = ws.clone();
            cx.spawn(async move |cx| {
                crate::browser_host::WorkspaceBrowserHost::install(ws, cx);
            })
            .detach();
        }
        let rows = mux.read(cx).thread_list(cx);
        let unread = mux.read(cx).unread_map();
        let removed = crate::project_registry::removed_projects();
        let mut sessions: Vec<steer_agent_chrome_ui::shell::SessionRow> =
            crate::sidebar_projection::project_groups(&rows, &unread, &removed)
                .into_iter()
                .flat_map(steer_agent_chrome_ui::shell::SessionRow::from_group)
                .collect();
        // Launched external sessions (project-menu agents/terminals) merge
        // into the snapshot as sidebar rows; the shell regroups them under
        // their project's header.
        sessions.extend(ws.read(cx).external_session_rows());
        // The sidebar highlight follows the FOREGROUND thread, not the last
        // click: new-thread landings, /exit replacements and successor
        // hand-offs all switch without a sidebar click, and a stale
        // highlight would advertise the wrong session as active. An external
        // session's highlight is not stolen — it is no thread, so the
        // foreground rule has no opinion on it while it holds the highlight.
        let fg = ws
            .read(cx)
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(_, sid)| sid.clone());
        let active_is_external = shell
            .read(cx)
            .active
            .as_deref()
            // The record lookup is inherently live-guarded: a close removes
            // the record, so a dead id left in `active` resolves false and
            // the highlight falls back to the foreground thread.
            .map(|id| ws.read(cx).is_external_session(id))
            .unwrap_or(false);
        shell.update(cx, |shell, cx| {
            shell.set_sessions(sessions);
            if !active_is_external && shell.active != fg {
                shell.active = fg;
            }
            cx.notify();
        });
        refresh_foreground_cwd(&ws, &rows, cx);
        // The foreground thread id: dock follows it. On a switch, detach the
        // outgoing view into the stash and restore/spawn the incoming one.
        let fg = ws
            .read(cx)
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(_, sid)| sid.clone());
        if fg != dock_thread && fg.is_some() {
            let old_id = dock_thread.take();
            shell.update(cx, |shell, cx| {
                let taken = shell.take_panel_view(cx);
                if let (Some(old_id), Some(view)) = (&old_id, taken) {
                    dock_stash.insert(old_id.clone(), view);
                }
                if let Some(view) = fg.as_ref().and_then(|id| dock_stash.remove(id)) {
                    shell.set_panel_view(view, cx);
                }
                // No stashed terminal: the slot stays empty — the next
                // expand spawns at the NEW thread's cwd via the surface.
                // Right pane rides the same switch: stash the outgoing
                // thread's whole tab session, restore the incoming one
                // (None → the fresh new-tab page).
                if let Some(old_id) = &old_id
                    && let Some(session) = shell.stash_right_session(cx)
                {
                    right_stash.insert(old_id.clone(), session);
                }
                match fg.as_ref().and_then(|id| right_stash.remove(id)) {
                    Some(session) => shell.restore_right_session(session, cx),
                    None => {
                        // No in-session stash: rebuild from the thread's
                        // durable snapshot (browser/editor restore; live
                        // session kinds are dropped — a dead process cannot
                        // be resurrected). The window handle comes from the
                        // dispatch slot (the restore builds webviews).
                        let restored = fg.as_ref().and_then(|id| {
                            let json = load_right_pane(id)?;
                            let shape: serde_json::Value = serde_json::from_str(&json).ok()?;
                            let visible = shape.get("visible")?.as_bool()?;
                            let active = shape.get("active")?.as_u64()? as usize;
                            let tabs = shape
                                .get("tabs")?
                                .as_array()?
                                .iter()
                                .filter_map(|t| {
                                    let kind = t.get("kind")?.as_str()?.to_string();
                                    let spec = t.get("spec")?.as_str()?.to_string();
                                    Some((kind, spec))
                                })
                                .collect::<Vec<_>>();
                            Some((tabs, visible, active))
                        });
                        match restored {
                            Some((tabs, visible, active)) => {
                                // We are already inside `shell.update`; the
                                // window handle (the restore builds webviews)
                                // comes from the dispatch slot. Re-entering
                                // the Shell entity here would double-lease.
                                if let Some(handle) = crate::dispatch::window_global() {
                                    let _ = handle.update(cx, |_, w, cx| {
                                        shell.right.update(cx, |pane, cx| {
                                            pane.restore_persisted(tabs, visible, active, w, cx);
                                        });
                                    });
                                }
                            }
                            None => {
                                // Fresh thread: drop any lingering pane state
                                // to the empty page, COLLAPSED — the pane's
                                // per-thread memory (stash/durable snapshot)
                                // restores the thread's own visibility when it
                                // has one; a thread without one starts with the
                                // transcript full-width.
                                shell.right.update(cx, |pane, cx| {
                                    pane.new_tab_page(cx);
                                    pane.collapse(cx);
                                });
                            }
                        }
                    }
                }
            });
            dock_thread = fg;
        }
    })
    .detach();
    shell
}

fn shell_config(
    ws: Entity<Workspace>,
    mux: &Entity<crate::multiplexer::SessionMultiplexer>,
    _cx: &mut Context<Shell>,
) -> ShellConfig {
    let mux = mux.clone();
    let placeholder: gpui::AnyView = ws.clone().into();
    ShellConfig {
        tool_kinds: registry(&mux, &ws),
        main: Arc::new(PendingMain {
            view: placeholder,
            ws: ws.clone(),
        }),

        panel_surface: Some(Arc::new(ThreadTerminalPanelSurface)),
        // The fixed sidebar rows / Customizations block are still fake
        // surfaces (Automations scheduling, plugin/MCP management pages
        // don't exist here yet) — no fabricated badge/count, the chrome
        // marks the rows unimplemented on its own.
        fixed_rows: vec![
            FixedRow {
                icon: icons::CALENDAR,
                label: steer_i18n::t("chrome-sidebar-automations"),
                badge: None,
            },
            FixedRow {
                icon: icons::COMMENT_DISCUSSION,
                label: steer_i18n::t("chrome-sidebar-chats"),
                badge: None,
            },
        ],
        customizations: vec![
            CustomizationRow {
                icon: icons::HOME,
                label: steer_i18n::t("chrome-sidebar-overview"),
                count: None,
            },
            CustomizationRow {
                icon: icons::SETTINGS_GEAR,
                label: steer_i18n::t("chrome-sidebar-mcp"),
                count: None,
            },
        ],
        hooks: HostHooks {
            // Pin is a client-dispatchable extension action on the thread
            // channel: dispatching journals it host-side (the store row plus
            // sidebar order) and the echo folds back into the sidebar read.
            // The in-memory thread_store write this hook used before never
            // reached the journal, so the write face (thread_store) and the
            // read face (AHP fold) were two states that never met.
            on_pin: Some(Box::new({
                let mux = mux.clone();
                move |id, _w, cx| {
                    let store = mux.read(cx).store();
                    let channel = crate::ahp_store::thread_uri(id);
                    store.update(cx, |store, _| {
                        // The flip reads through the ONE pin read face (fold
                        // first, list `_meta` baseline second) — flipping
                        // against the fold alone would no-op a row whose
                        // baseline has not landed (the menu shows meta).
                        let pinned = crate::multiplexer::ext_and_meta_pinned(
                            &store.book,
                            store.book.summaries.get(id),
                            &channel,
                        );
                        store.dispatch(
                            channel,
                            ahp_types::actions::StateAction::Unknown(serde_json::json!({
                                "type": manox_ahp::ext::actions::PINNED_CHANGED,
                                "pinned": !pinned,
                            })),
                        );
                    });
                }
            })),
            // The menu's archive toggle flips the CURRENT partition.
            // Premise: the wire snapshot rides the ACTIVE partition only, so
            // every row the shell sees is unarchived and the unarchive half
            // is for the archived-partition rows the wire will grow. The
            // store journals a decision (and fires SessionEnd) even for a
            // missing id, so an unknown id is refused here instead of
            // written — callers pass live rows only.
            on_archive: Some(Box::new(|id, _w, _cx| {
                let store = manox_agent::thread_store_global();
                let archived = store.read(|st| {
                    if st.summaries().iter().any(|s| s.id.as_str() == id) {
                        Some(false)
                    } else {
                        st.archived_summaries()
                            .iter()
                            .any(|s| s.id.as_str() == id)
                            .then_some(true)
                    }
                });
                if let Some(archived) = archived {
                    store.with_mut(|st| st.archive_thread(id, !archived));
                }
            })),
            // Thread-tag write-back (`None` clears) — the same store write
            // the sidebar's SetThreadTag event lands on.
            on_set_tag: Some(Box::new(|id, tag, _w, _cx| {
                manox_agent::thread_store_global().with_mut(|st| st.set_thread_tag(id, tag));
            })),
            // The workspace's own new-thread path (park + fresh landing).
            on_new_session: Some(Box::new({
                let ws = ws.clone();
                move |w, cx| {
                    ws.update(cx, |ws, cx| {
                        ws.start_new_thread(None, w, cx);
                        cx.notify();
                    });
                }
            })),
            // The full production switch path: attach, drafts stash, list
            // reconciliation. External-session rows route to their tab
            // instead — a thread id they are not.
            on_select: Some(Box::new({
                let ws = ws.clone();
                move |id, w, cx| {
                    // An external row brings its session back to the MAIN
                    // column — focused (a synchronous workspace update, no
                    // window-handle round-trip, no dispatch hazard); a
                    // thread id takes the full switch path. The routing is
                    // the workspace's own record lookup (the row id is a
                    // namespaced `external:…` uuid, not a prefix protocol).
                    let is_external = ws.read(cx).is_external_session(id);
                    ws.update(cx, |ws, cx| {
                        if is_external {
                            ws.open_external_session(id, w, cx);
                        } else {
                            ws.open_thread(id.to_string(), w, cx);
                        }
                        cx.notify();
                    });
                }
            })),
            // ←/→ session history: the workspace owns the visited-thread
            // stack; the hook reports the landed id so the shell's selection
            // follows without re-deriving host state.
            on_nav_back: Some(Box::new({
                let ws = ws.clone();
                move |w, cx| {
                    ws.update(cx, |ws, cx| {
                        let landed = ws.nav_back(w, cx);
                        cx.notify();
                        landed
                    })
                }
            })),
            on_nav_forward: Some(Box::new({
                let ws = ws.clone();
                move |w, cx| {
                    ws.update(cx, |ws, cx| {
                        let landed = ws.nav_forward(w, cx);
                        cx.notify();
                        landed
                    })
                }
            })),
            nav_avail: Some(Box::new({
                let ws = ws.clone();
                move |cx| ws.read(cx).nav_avail()
            })),
            // "Open in VS Code": hand the foreground thread's project to the
            // plain VS Code launch (no injection, no restart prompts). The
            // launch blocks on `open`'s exit, so it runs on the BACKGROUND
            // executor (`cx.spawn` alone would stay on the main thread and
            // freeze the run loop); only a failure notifies — a success
            // announces itself by VS Code opening.
            on_open_editor: Some(Box::new(|window, cx| {
                let Some(project) = FOREGROUND_PROJECT
                    .lock()
                    .expect("foreground project lock")
                    .clone()
                else {
                    window.push_notification(
                        Notification::error(steer_i18n::t("vscode-open-no-project")),
                        cx,
                    );
                    return;
                };
                let handle = crate::dispatch::window_global();
                cx.spawn(async move |cx| {
                    let launch_err = cx
                        .background_spawn(async move {
                            steer_ext_agents::vscode_app::launch_plain(Some(&project)).err()
                        })
                        .await;
                    if let Some(handle) = handle
                        && let Err(update_err) = handle.update(cx, |_, window, cx| {
                            if let Some(e) = &launch_err {
                                tracing::error!(error = %e, "open-in-VS-Code failed");
                                window.push_notification(
                                    Notification::error(format!(
                                        "{}: {e}",
                                        steer_i18n::t("vscode-open-failed")
                                    )),
                                    cx,
                                );
                            }
                        })
                    {
                        // The window closed before the launch settled: the
                        // notification had no surface left. Log BOTH errors —
                        // the update failure alone would hide what actually
                        // went wrong with the launch.
                        tracing::warn!(
                            launch = ?launch_err,
                            update = ?update_err,
                            "open-in-VS-Code result unreported (window gone)"
                        );
                    }
                })
                .detach();
            })),
            // The project group's action menu: agent/terminal/editor launches
            // rooted at the group's directory + 移除项目. The host builds the
            // menu entity; the chrome mounts and dismisses it.
            on_group_menu: Some(Box::new({
                let ws = ws.downgrade();
                move |_key, project, _anchor, window, cx| {
                    Some(crate::project_menu::group_menu(project, &ws, window, cx))
                }
            })),
            // The external row menu's 关闭会话: drop the session (its
            // terminal view dies with the record) and fall back to the
            // conversation.
            on_close_external: Some(Box::new({
                let ws = ws.downgrade();
                move |id, _w, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.close_external_session(id, cx));
                }
            })),
        },
        // The titlebar's avatar slot wears the app's own mark.
        brand: Some(Arc::new(|| {
            gpui::div()
                .size(gpui::px(13.))
                .child(
                    gpui::svg()
                        .path("icons/steer.svg")
                        .size_full()
                        .text_color(steer_agent_chrome_ui::theme::BADGE_BLUE_FG),
                )
                .into_any_element()
        })),
    }
}

struct PendingMain {
    view: gpui::AnyView,
    ws: Entity<Workspace>,
}

impl MainSurface for PendingMain {
    fn view(&self) -> gpui::AnyView {
        self.view.clone()
    }

    fn title(&self, cx: &App) -> gpui::SharedString {
        // An external session owning the main column titles it.
        if let Some(label) = self.ws.read(cx).active_external_label() {
            return label.into();
        }
        // The active thread's display title from the foreground store;
        // "steer" before any interaction.
        self.ws
            .read(cx)
            .chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid)
                    .display_title()
                    .map(str::to_string)
            })
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Steer".to_string())
            .into()
    }
}

/// The foreground thread's cwd, refreshed by the assembly's observer on
/// every thread switch — the dock surface reads it at open time (a static
/// because the surface must stay entity-free to ride an `Arc`; the same
/// pattern as the badge pump's LAST_COUNT). Falls back to $HOME: a terminal
/// has to spawn SOMEWHERE.
static FOREGROUND_CWD: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);
/// The foreground thread's PROJECT path, or `None` when there is no
/// foreground row / no project — "open in editor" must not silently open
/// `$HOME`, so it reads this rather than the cwd fallback above.
static FOREGROUND_PROJECT: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// The dock surface's read face of the foreground cwd.
pub fn foreground_cwd() -> Option<std::path::PathBuf> {
    FOREGROUND_CWD.lock().expect("foreground cwd lock").clone()
}

/// The foreground thread's project path (read face for the project menu's
/// no-project fallback — a launch without a directory never silently opens
/// $HOME).
pub fn foreground_project() -> Option<std::path::PathBuf> {
    FOREGROUND_PROJECT
        .lock()
        .expect("foreground project lock")
        .clone()
}

/// The threads database behind the store global — the pane snapshot's
/// upsert/load face (one acquisition site shared by both directions).
fn pane_db() -> std::sync::Arc<manox_agent::db::ThreadsDatabase> {
    manox_agent::thread_store_global().read(|s| s.db().clone())
}

/// Upsert the current foreground thread's right-pane snapshot into
/// `threads.db` (the pane's own kind/spec encoding, one row per thread).
fn persist_right_pane(shell: &Entity<Shell>, ws: &Entity<Workspace>, cx: &App) {
    let Some(thread_id) = ws.read(cx).chat.read(cx).store.clone().map(|(_, sid)| sid) else {
        return;
    };
    let (visible, active, tabs) = shell.read(cx).right.read(cx).persisted(cx);
    let payload = serde_json::json!({
        "visible": visible,
        "active": active,
        "tabs": tabs
            .iter()
            .map(|(kind, spec)| serde_json::json!({ "kind": kind, "spec": spec }))
            .collect::<Vec<_>>(),
    });
    if let Err(e) = pane_db().upsert_right_pane(&thread_id, &payload.to_string()) {
        tracing::warn!(error = %e, thread_id = %thread_id, "persist chrome right pane failed");
    }
}

/// Load a thread's persisted right-pane snapshot (the chrome encoding).
fn load_right_pane(thread_id: &str) -> Option<String> {
    pane_db().load_right_pane(thread_id).ok().flatten()
}

fn refresh_foreground_cwd(
    ws: &Entity<Workspace>,
    rows: &[crate::sidebar_projection::ThreadRow],
    cx: &App,
) {
    // The thread's working directory is its PROJECT path — the wire row's
    // `project` column, the same source the sidebar groups by. The store's
    // own cwd records the workspace cwd (home for the embedded build), so
    // it is only the fallback; a missing row/project lands on home.
    let fg = ws
        .read(cx)
        .chat
        .read(cx)
        .store
        .as_ref()
        .map(|(_, sid)| sid.clone());
    let project = fg
        .as_ref()
        .and_then(|id| rows.iter().find(|r| &r.id == id))
        .and_then(|r| r.project.clone())
        .filter(|p| !p.is_empty());
    let cwd = project
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| ".".into())
        });
    *FOREGROUND_CWD.lock().expect("foreground cwd lock") = Some(cwd);
    *FOREGROUND_PROJECT.lock().expect("foreground project lock") =
        project.map(std::path::PathBuf::from);
}

/// Wrap a chrome Shell into the window's Root view (the bin mounts this).
pub fn root(shell: Entity<Shell>, window: &mut Window, cx: &mut App) -> Entity<Root> {
    cx.new(|cx| Root::new(shell, window, cx))
}
