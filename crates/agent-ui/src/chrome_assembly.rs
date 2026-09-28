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

use gpui::{App, AppContext as _, Context, Entity, WeakEntity, Window};
use gpui_component::Root;
use manox_agent_chrome_ui::right_pane::ToolTab;
use manox_agent_chrome_ui::{
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
    let mut right_stash: HashMap<String, manox_agent_chrome_ui::right_pane::RightPaneSession> =
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
        let rows = mux.read(cx).thread_list().to_vec();
        let unread = mux.read(cx).unread_map(cx);
        let sessions: Vec<manox_agent_chrome_ui::shell::SessionRow> =
            crate::sidebar_projection::project_groups(&rows, &unread)
                .into_iter()
                .flat_map(manox_agent_chrome_ui::shell::SessionRow::from_group)
                .collect();
        shell.update(cx, |shell, cx| {
            shell.set_sessions(sessions);
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
            .map(|s| s.read(cx).store.id.0.clone());
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
                                // to the empty page without touching the stash.
                                shell.right.update(cx, |pane, cx| pane.new_tab_page(cx));
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
    let placeholder: gpui::AnyView = ws.clone().into();
    ShellConfig {
        tool_kinds: registry(mux, &ws),
        main: Arc::new(PendingMain {
            view: placeholder,
            ws: ws.clone(),
        }),

        panel_surface: Some(Arc::new(ThreadTerminalPanelSurface)),
        fixed_rows: vec![
            FixedRow {
                icon: icons::CALENDAR,
                label: manox_i18n::t("chrome-sidebar-automations"),
                badge: Some("NEW".into()),
            },
            FixedRow {
                icon: icons::COMMENT_DISCUSSION,
                label: manox_i18n::t("chrome-sidebar-chats"),
                badge: None,
            },
        ],
        customizations: vec![
            CustomizationRow {
                icon: icons::HOME,
                label: manox_i18n::t("chrome-sidebar-overview"),
                count: None,
            },
            CustomizationRow {
                icon: icons::SETTINGS_GEAR,
                label: manox_i18n::t("chrome-sidebar-mcp"),
                count: None,
            },
        ],
        hooks: HostHooks {
            // Pin/archive ride the thread store; the next wire snapshot
            // reconciles.
            on_pin: Some(Box::new(|id, _w, _cx| {
                let loaded = manox_agent::thread_store::global().with_mut(|st| st.load_thread(id));
                if let Ok(Some(handle)) = loaded {
                    let was = handle.read(|t| t.is_pinned());
                    handle.with_mut(|t| t.set_pinned(!was));
                }
            })),
            on_archive: Some(Box::new(|id, _w, _cx| {
                manox_agent::thread_store::global().with_mut(|st| st.archive_thread(id, true));
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
            // reconciliation.
            on_select: Some(Box::new({
                let ws = ws.clone();
                move |id, w, cx| {
                    ws.update(cx, |ws, cx| {
                        ws.open_thread(id.to_string(), w, cx);
                        cx.notify();
                    });
                }
            })),
        },
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
        // The active thread's display title from the foreground store;
        // "manox" before any interaction.
        self.ws
            .read(cx)
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|s| s.read(cx).store.with(|st| st.display_title.clone()))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Manox".to_string())
            .into()
    }
}

/// The foreground thread's cwd, refreshed by the assembly's observer on
/// every thread switch — the dock surface reads it at open time (a static
/// because the surface must stay entity-free to ride an `Arc`; the same
/// pattern as the badge pump's LAST_COUNT).
static FOREGROUND_CWD: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// The dock surface's read face of the foreground cwd.
pub fn foreground_cwd() -> Option<std::path::PathBuf> {
    FOREGROUND_CWD.lock().expect("foreground cwd lock").clone()
}

/// Upsert the current foreground thread's right-pane snapshot into
/// `threads.db` (the pane's own kind/spec encoding, one row per thread).
fn persist_right_pane(shell: &Entity<Shell>, ws: &Entity<Workspace>, cx: &App) {
    let Some(thread_id) = ws
        .read(cx)
        .chat
        .read(cx)
        .store
        .as_ref()
        .map(|s| s.read(cx).store.id.0.clone())
    else {
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
    let db = manox_agent::thread_store_global().read(|s| s.db().clone());
    if let Err(e) = db.upsert_right_pane(&thread_id, &payload.to_string()) {
        tracing::warn!(error = %e, thread_id = %thread_id, "persist chrome right pane failed");
    }
}

/// Load a thread's persisted right-pane snapshot (the chrome encoding).
fn load_right_pane(thread_id: &str) -> Option<String> {
    let db = manox_agent::thread_store_global().read(|s| s.db().clone());
    db.load_right_pane(thread_id).ok().flatten()
}

fn refresh_foreground_cwd(
    ws: &Entity<Workspace>,
    rows: &[manox_protocol::ThreadListItem],
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
        .map(|s| s.read(cx).store.id.0.clone());
    let project = fg
        .as_ref()
        .and_then(|id| rows.iter().find(|r| &r.id == id))
        .and_then(|r| r.project.clone())
        .filter(|p| !p.is_empty());
    let cwd = project.map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| ".".into())
    });
    *FOREGROUND_CWD.lock().expect("foreground cwd lock") = Some(cwd);
}

/// Wrap a chrome Shell into the window's Root view (the bin mounts this).
pub fn root(shell: Entity<Shell>, window: &mut Window, cx: &mut App) -> Entity<Root> {
    cx.new(|cx| Root::new(shell, window, cx))
}
