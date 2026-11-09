//! The chrome-shell assembly for the real app (the sole shell since the
//! legacy workspace shell retired). This module owns everything the app
//! mounts:
//!
//! - the session list fed by the REAL multiplexer (the server wire rows
//!   projected through `sidebar_projection` — the status ladder, team
//!   forest, tags, project grouping);
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

/// The chrome command channel: the ONLY way host code mutates the shell
/// outside the projection pump (`project`). The command runs in a deferred
/// effect flushed at the outermost `App::update`, where no entity lease is
/// held and no window update is in flight.
///
/// Both facts are load-bearing. A chrome listener / hook runs inside its own
/// entity's update, so an inline `shell.update` there is a double lease (the
/// abort class this channel exists to end); and a `WindowHandle::update`
/// issued mid-event has no window to take, so the old round-trip helper
/// silently dropped the command. Deferring removes both hazards instead of
/// classifying call sites by which lease they happen to stand in.
///
/// ORDERING: commands and the notifications that drive [`project`] share the
/// effect queue and run in enqueue order. A host update must therefore issue
/// its chrome commands BEFORE the `cx.notify()` that makes the pump re-read
/// the workspace (the switch paths do: `leave_*`/pane commands first, the
/// notify at the tail). A command enqueued after that notify would run after
/// the projection has already stashed and restored the incoming container's
/// pane — and could then close a tab the projection legitimately restored.
pub fn defer_shell(
    cx: &mut App,
    command: impl FnOnce(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
) {
    cx.defer(move |cx| {
        let Some(shell) = shell_handle() else {
            tracing::debug!("chrome command dropped: no live shell");
            return;
        };
        let Some(handle) = crate::dispatch::window_global() else {
            tracing::debug!("chrome command dropped: no live window");
            return;
        };
        let _ = handle.update(cx, |_, window, cx| {
            shell.update(cx, |shell, cx| command(shell, window, cx));
        });
    });
}

/// Open (or focus) a host-built tool tab in the live right pane.
pub fn open_tool_tab(tab: Arc<dyn ToolTab>, cx: &mut App) {
    defer_shell(cx, move |shell, window, cx| {
        shell
            .right
            .update(cx, |pane, cx| pane.open_tab(tab, window, cx));
    });
}

/// Close every open tab of `kind` in the live pane. The workspace retires the
/// ephemeral observation panels through this when it leaves a thread: a
/// panel's content is the child transcript ACCUMULATED SO FAR, so one carried
/// across a thread switch would come back stale (the transcript catches up,
/// the panel does not). Dropping them keeps the invariant "a restored panel
/// == the thread's transcript at open time".
pub fn close_tool_tabs_of_kind(kind: &str, cx: &mut App) {
    let kind = kind.to_string();
    defer_shell(cx, move |shell, window, cx| {
        shell
            .right
            .update(cx, |pane, cx| pane.close_kind(&kind, window, cx));
    });
}

/// Open (or focus) a right-pane tool kind from outside the view tree (the
/// edit cards' open-diff path).
pub fn open_right_kind(kind: &str, cx: &mut App) {
    let kind = kind.to_string();
    defer_shell(cx, move |shell, window, cx| {
        shell.open_right(&kind, window, cx);
    });
}

/// The open tool-tab instance ids in the live pane, in open order — the
/// diagnostics face for single-instance kinds (the diff tab must never
/// duplicate). Empty when no shell is live.
pub fn pane_open_tab_ids(cx: &App) -> Vec<String> {
    let Some(shell) = shell_handle() else {
        return Vec::new();
    };
    shell.read(cx).right.read(cx).tab_ids()
}

/// Take the live dock's panel view and drop it — dropping the terminal view
/// tears its PTY down (process-tree SIGTERM → SIGKILL), which is how `/exit`
/// stops the dock's subprocesses. A collapsed dock holds no view: nothing to
/// take, nothing to kill.
pub fn stop_dock_terminal(cx: &mut App) {
    defer_shell(cx, |shell, _window, cx| {
        drop(shell.take_panel_view(cx));
    });
}

/// The distinct tool-tab kinds open in the live pane — `/exit`'s read-only
/// probe of what owns live processes. Empty when no shell is live.
pub fn pane_open_kinds(cx: &App) -> Vec<String> {
    let Some(shell) = shell_handle() else {
        return Vec::new();
    };
    shell
        .read(cx)
        .right
        .read(cx)
        .open_kinds()
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Whether the bottom dock holds a live view (the `/exit` teardown's probe).
pub fn dock_terminal_live(cx: &App) -> bool {
    shell_handle().is_some_and(|shell| shell.read(cx).panel_live())
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
    // Documents get their code-block highlighter from the app-wide text
    // defaults, never per view (a per-view closure is a new `Arc` every frame,
    // which keeps gpui-base's selection-preserving reflow armed and leaves the
    // document unselectable). Every caller syncs the theme before mounting, so
    // this lands after `Theme::sync_base` reinstalled Component's defaults.
    steer_components::markdown::install_code_highlighter(cx);
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
    // the dispatch slot (dock badge pump + reopen-focus read it), the
    // process-wide capability provider (clipboard everywhere; browser ops
    // only where the integrated browser is compiled in), and the browser
    // host itself.
    crate::dispatch::set_workspace(ws.clone());
    {
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let ws = ws.clone();
        cx.spawn(async move |cx| {
            crate::capability_host::install(cx);
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            {
                crate::browser_host::WorkspaceBrowserHost::install(ws.clone(), cx);
                // The install lands asynchronously, so a tab restored from
                // threads.db before it (startup) missed its route registration:
                // re-register every live view now that the host exists. The
                // owner is the boot foreground container — at this point every
                // live view belongs to the landing container's restored pane.
                if let Some(host) = crate::browser_host::WorkspaceBrowserHost::concrete() {
                    let (ids, owner) = ws.read_with(cx, |ws, cx| {
                        (
                            ws.browser_views.keys().copied().collect::<Vec<_>>(),
                            ws.foreground_container_id(cx).unwrap_or_default(),
                        )
                    });
                    for id in ids {
                        host.register_ui_tab(id, owner.clone());
                    }
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
    // Per-container dock state: each visited container keeps its live panel
    // terminal across switches (the right-pane stash semantic — a
    // stashed view keeps its process running; only explicit collapse of the
    // LIVE dock tears one down). The stash and the live-owner tracking live
    // in the resource registry (keyed by container id — a native thread id
    // verbatim, an external row id while one owns the main column), so the
    // container teardown can reclaim them.
    //
    // LIFETIME (registry-scoped, 2026-10-30): the stash outlives the window —
    // a tray close + reopen keeps the dock's terminal and the right pane's
    // terminal/CLI tabs (browser and editor tabs come back from the pane
    // registry). The former shell-owned stashes died with the window; the
    // registry matches the workspace's process-lifetime instead, which is
    // what the resources' owner (the container) outlives.
    // Per-container right-pane sessions: the open tab set + content store +
    // active tab + visibility move with the foreground container (stash on
    // switch-out, restore on switch-in; a container with no in-session stash
    // falls back to its pane-registry snapshot — the pane's durable account —
    // and starts on the fresh new-tab page only when neither exists).
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
        project(&shell, &ws, cx);
    })
    .detach();
    // Seed the first frame from the same pump: a re-opened window mounts a
    // fresh shell over the surviving workspace.
    project(&shell, &ws, cx);
    shell
}

/// The projection pump — the ONE writer of chrome state.
///
/// `cx.observe` callbacks run in `flush_effects` at the effect boundary, where
/// no entity lease is held; this function (plus deferred commands) is the only
/// place allowed to call `shell.update`. Host code never writes chrome state
/// inline: it mutates workspace state and notifies, and the pump projects.
///
/// The notifications this runs on are queued by `cx.notify()`; chrome commands
/// queued in the SAME update run after it (see [`defer_shell`]'s ordering
/// note). The pane stash/restore below is what that ordering protects.
fn project(shell: &Entity<Shell>, ws: &Entity<Workspace>, cx: &mut App) {
    let mux = ws.read(cx).multiplexer.clone();
    // The process-lifetime registries the app wires at startup:
    // the dispatch slot (dock badge pump + reopen-focus read it) and the
    // process-wide browser host (IPC routing + the agent's web capability).
    crate::dispatch::set_workspace(ws.clone());
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let ws = ws.clone();
        cx.spawn(async move |cx| {
            crate::browser_host::WorkspaceBrowserHost::install(ws, cx);
        })
        .detach();
    }
    let rows = mux.read(cx).container_list(cx);
    // The grouping data source is the project registry (design §2.3):
    // projects in sidebar order + each container's membership. Rows
    // without a membership (ad-hoc folders, external containers,
    // other clients' sessions) all fall to the loose bucket.
    let projects = crate::projects::list_projects();
    let mut sessions: Vec<steer_agent_chrome_ui::shell::SessionRow> =
        crate::sidebar_projection::project_groups(&rows, &projects, &|id| {
            crate::projects::membership_of(id)
        })
        .into_iter()
        .flat_map(steer_agent_chrome_ui::shell::SessionRow::from_group)
        .collect();
    // Launched external sessions (project-menu agents/terminals) merge
    // into the snapshot as sidebar rows, keyed by their own membership.
    sessions.extend(ws.read(cx).external_session_rows());
    // The project skeleton (id, display name, roots — the roots feed
    // the header's count badge + tooltip) and the display order: the
    // registry's position order, the loose bucket trailing.
    let heads: Vec<(String, String, Vec<String>)> = projects
        .iter()
        .map(|project| {
            (
                project.id.clone(),
                crate::projects::display_name(project),
                project
                    .roots
                    .iter()
                    .map(|root| root.path.to_string_lossy().into_owned())
                    .collect(),
            )
        })
        .collect();
    let mut keys: Vec<String> = projects.iter().map(|p| p.id.clone()).collect();
    keys.push(steer_agent_chrome_ui::session_list::LOOSE_BUCKET_KEY.to_string());
    // The chrome's host-owned surfaces: the dialog slot and the Settings
    // sidebar nav are workspace state (a re-opened window remounts them), so
    // the projection mounts whatever the workspace currently holds.
    let dialog = ws.read(cx).dialog.clone().map(Into::into);
    let sidebar = ws.read(cx).settings_nav_view.clone().map(Into::into);
    shell.update(cx, |shell, cx| {
        shell.set_folder_heads(heads);
        shell.set_group_order(keys);
        shell.set_sessions(sessions);
        shell.set_dialog_surface(dialog, cx);
        shell.set_sidebar_surface(sidebar, cx);
        cx.notify();
    });
    refresh_foreground_project(ws, &rows, cx);
    // The foreground CONTAINER id (a native thread id verbatim, an
    // external row id while one owns the main column): the dock and the
    // right pane both follow it. On a switch, detach the outgoing
    // container's views into the workspace's registry stash and
    // restore/spawn the incoming container's own. The dock and the pane
    // switch together, so one live-owner pair moves in lockstep (the
    // container teardown clears them together when its container dies).
    let fg = ws.read(cx).foreground_container_id(cx);
    let old_id = ws.read(cx).dock_owner();
    if fg != old_id && fg.is_some() {
        // The outgoing views leave the shell first, land under the
        // outgoing container's key, then the incoming container's own
        // come back (or the pane falls back to its persisted snapshot /
        // the fresh new-tab page).
        let taken = shell.update(cx, |shell, cx| shell.take_panel_view(cx));
        let outgoing = shell.update(cx, |shell, cx| shell.stash_right_session(cx));
        ws.update(cx, |ws, _| {
            ws.set_dock_owner(fg.clone());
            ws.set_right_owner(fg.clone());
            if let (Some(old_id), Some(view)) = (&old_id, taken) {
                ws.stash_dock(old_id, view);
            }
            if let Some(old_id) = &old_id
                && let Some(session) = outgoing
            {
                ws.stash_right(old_id, session);
            }
        });
        let (dock_in, right_in) = ws.update(cx, |ws, _| {
            (
                fg.as_ref().and_then(|id| ws.take_dock(id)),
                fg.as_ref().and_then(|id| ws.take_right(id)),
            )
        });
        shell.update(cx, |shell, cx| {
            // No stashed terminal: the slot stays empty — the next
            // expand spawns at the NEW container's attrs cwd via the
            // surface. The right pane rides the same switch.
            if let Some(view) = dock_in {
                shell.set_panel_view(view, cx);
            }
            match right_in {
                Some(session) => shell.restore_right_session(session, cx),
                None => {
                    // No in-session stash: rebuild from the container's
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
                            // Fresh container: drop any lingering pane
                            // state to the empty page, COLLAPSED — the
                            // pane's per-container memory (stash/durable
                            // snapshot) restores the container's own
                            // visibility when it has one; a container
                            // without one starts with the transcript
                            // full-width.
                            shell.right.update(cx, |pane, cx| {
                                pane.new_tab_page(cx);
                                pane.collapse(cx);
                            });
                        }
                    }
                }
            }
        });
    }
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
                    store.update(cx, |store, cx| {
                        // The flip reads through the ONE pin read face (fold
                        // first, list `_meta` baseline second) — flipping
                        // against the fold alone would no-op a row whose
                        // baseline has not landed (the menu shows meta).
                        let pinned = crate::multiplexer::ext_and_meta_pinned(
                            &store.book,
                            store.book.summaries.get(id),
                            &crate::ahp_store::thread_uri(id),
                        );
                        // An un-opened row's thread channel is not subscribed
                        // (the boot path stopped mass-subscribing — one
                        // baseline fold per row froze the FIFO connection for
                        // minutes on a large migrated history). Subscribe
                        // first — ONCE per row (every pin click re-issuing
                        // the subscribe would pile host-side subscriptions):
                        // the connection's ordering lands it before the
                        // dispatch, so the router's subscribed-gate passes
                        // and the pin echo folds back.
                        store.subscribe_thread_once(id, cx);
                        store.dispatch(
                            crate::ahp_store::thread_uri(id),
                            ahp_types::actions::StateAction::Unknown(serde_json::json!({
                                "type": manox_ahp::ext::actions::PINNED_CHANGED,
                                "pinned": !pinned,
                            })),
                        );
                    });
                }
            })),
            // The row menu's archive toggle rides the container lifecycle
            // (plan A6, D7): a live container goes through the
            // confirm-or-direct teardown entry (the confirmation lists the
            // live dependents and the archive lands as the ONE teardown
            // command), an archived one revives whole-tree. The shell-side
            // row drop is the path's own optimistic half — the menu does not
            // pre-drop, or a cancelled confirmation would have already
            // vanished the row.
            on_archive: Some(Box::new({
                let ws = ws.clone();
                move |id, w, cx| {
                    ws.update(cx, |ws, cx| {
                        ws.toggle_container_archived(id, w, cx);
                        cx.notify();
                    });
                }
            })),
            // The selection bar's commit: the whole checked batch goes
            // through the same confirm-or-direct container teardown — ONE
            // teardown command carries every captain.
            on_batch_archive: Some(Box::new({
                let ws = ws.clone();
                move |ids, w, cx| {
                    ws.update(cx, |ws, cx| {
                        ws.confirm_archive_containers(ids.to_vec(), None, w, cx);
                        cx.notify();
                    });
                }
            })),
            // Thread-tag write-back (`None` clears) — the same store write
            // the sidebar's SetThreadTag event lands on.
            on_set_tag: Some(Box::new(|id, tag, _w, _cx| {
                manox_agent::thread_store_global().with_mut(|st| st.set_thread_tag(id, tag));
            })),
            // The New button / ⌘N lands the LAUNCHER (design §3.3) — the
            // pre-container launch pad. Nothing is minted here; repeated New
            // is idempotent (focus the composer). Bare: no preset, unlocked.
            on_new_session: Some(Box::new({
                let ws = ws.clone();
                move |w, cx| {
                    ws.update(cx, |ws, cx| {
                        ws.enter_launcher(None, false, w, cx);
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
                            ws.open_container(id.to_string(), w, cx);
                        }
                        cx.notify();
                    });
                }
            })),
            // The sidebar highlight: the workspace's foreground container,
            // read at render time. No push — a push would have to be repeated
            // at every attach/spawn/launcher/settings leg, and the one it
            // misses would advertise the wrong session.
            active_row: Some(Box::new({
                let ws = ws.clone();
                move |cx| ws.read(cx).chrome_selection(cx)
            })),
            // ←/→ session history: the workspace owns the visited-thread
            // stack; the move changes the foreground, and the highlight
            // above follows it.
            on_nav_back: Some(Box::new({
                let ws = ws.clone();
                move |w, cx| {
                    ws.update(cx, |ws, cx| {
                        ws.nav_back(w, cx);
                        cx.notify();
                    })
                }
            })),
            on_nav_forward: Some(Box::new({
                let ws = ws.clone();
                move |w, cx| {
                    ws.update(cx, |ws, cx| {
                        ws.nav_forward(w, cx);
                        cx.notify();
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
            // The project group's action menu (the host builds the menu
            // entity; the chrome mounts and dismisses it). `project` is the
            // group's PROJECT ID — `None` on the loose bucket — the menu
            // resolves launch targets and the edit/delete actions from it.
            on_group_menu: Some(Box::new({
                let ws = ws.downgrade();
                move |_key, project, _anchor, window, cx| {
                    Some(crate::project_menu::group_menu(project, &ws, window, cx))
                }
            })),
            // The drag-reorder write-back: the key list the shell recorded,
            // verbatim — project ids take positions 0..n, the loose bucket's
            // key names no project and is ignored by the reorder.
            on_group_order: Some(Box::new(|keys, _cx| {
                crate::projects::reorder(keys);
            })),
            // The group header's hover ＋: the new-conversation entry
            // preselecting that group's project — LOCKED (the target is the
            // entry's intent; the chip renders display-only). The loose
            // bucket is not a project: its ＋ lands the bare unlocked
            // no-project default.
            on_group_new: Some(Box::new({
                let ws = ws.downgrade();
                move |key, window, cx| {
                    // The group key IS the project id — no path find (root
                    // overlap is allowed to be ambiguous).
                    let preset = (key != steer_agent_chrome_ui::session_list::LOOSE_BUCKET_KEY)
                        .then(|| crate::launcher::LauncherTarget::Project(key.to_string()));
                    let locked = preset.is_some();
                    let _ = ws.update(cx, |ws, cx| {
                        ws.enter_launcher(preset, locked, window, cx);
                        cx.notify();
                    });
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
            // The external row menu's 重命名: retitle the record — the
            // sidebar row and the main-column titlebar both read it. The
            // terminal's own OSC titles may claim the label back; last
            // writer wins.
            on_set_external_title: Some(Box::new({
                let ws = ws.downgrade();
                move |id, title, _w, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.set_external_title(id, title, cx));
                }
            })),
            // The sidebar footer's ＋ 新建项目 row: open the project dialog.
            // (The Chats bucket's group menu is the other entry; the
            // Launcher chip menu joins in Step-3.)
            on_new_project: Some(Box::new({
                let ws = ws.downgrade();
                move |window, cx| {
                    let _ = ws.update(cx, |ws, cx| ws.open_project_dialog(None, window, cx));
                }
            })),
            // The row-drag membership drop: a project group key attaches the
            // container to that project, the loose bucket's key detaches.
            // The notify runs the projection, the next snapshot re-files the
            // row.
            on_drop_row_to_group: Some(Box::new({
                let ws = ws.clone();
                move |group, row, _w, cx| {
                    if group == steer_agent_chrome_ui::session_list::LOOSE_BUCKET_KEY {
                        crate::projects::remove_membership(row);
                    } else {
                        crate::projects::set_membership(row, group);
                    }
                    ws.update(cx, |_, cx| cx.notify());
                }
            })),
            // The row menu's 移动到项目… submenu: the registry in display
            // order.
            project_list: Some(Box::new(|_cx| {
                crate::projects::list_projects()
                    .into_iter()
                    .map(|project| {
                        let name = crate::projects::display_name(&project);
                        (project.id, name)
                    })
                    .collect()
            })),
            on_move_row_to_project: Some(Box::new({
                let ws = ws.clone();
                move |row, project, _w, cx| {
                    crate::projects::set_membership(row, project);
                    ws.update(cx, |_, cx| cx.notify());
                }
            })),
            on_remove_row_from_project: Some(Box::new({
                let ws = ws.clone();
                move |row, _w, cx| {
                    crate::projects::remove_membership(row);
                    ws.update(cx, |_, cx| cx.notify());
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

/// The foreground thread's PROJECT path, or `None` when there is no
/// foreground row / no project — "open in editor" must not silently open
/// `$HOME`, so it reads this rather than a cwd fallback.
static FOREGROUND_PROJECT: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// The foreground thread's project path (read face for the project menu's
/// no-project fallback — a launch without a directory never silently opens
/// $HOME).
pub fn foreground_project() -> Option<std::path::PathBuf> {
    FOREGROUND_PROJECT
        .lock()
        .expect("foreground project lock")
        .clone()
}

/// The right-pane registry: client-owned presentation state (the home for
/// pane snapshots after the db retired). Session-lifetime — the
/// pane layout a process remembers is the pane layout it opened with.
/// Keys are CONTAINER ids (native thread ids verbatim, external row ids for
/// the external containers' own panes).
static PANE_REGISTRY: std::sync::Mutex<Option<HashMap<String, String>>> =
    std::sync::Mutex::new(None);

/// Upsert the current foreground container's right-pane snapshot into the
/// registry (the pane's own kind/spec encoding, one row per container).
fn persist_right_pane(shell: &Entity<Shell>, ws: &Entity<Workspace>, cx: &App) {
    let Some(container) = ws.read(cx).foreground_container_id(cx) else {
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
    PANE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(container, payload.to_string());
}

/// Load a container's persisted right-pane snapshot (the chrome encoding).
fn load_right_pane(container_id: &str) -> Option<String> {
    PANE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .and_then(|map| map.get(container_id))
        .cloned()
}

/// Remove a container's persisted right-pane snapshot (the teardown's
/// presentation leg): the snapshot is a restore source for a LIVE
/// container's return, so a dead container's key must not outlive it.
pub fn remove_right_pane(container_id: &str) {
    if let Some(map) = PANE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_mut()
    {
        map.remove(container_id);
    }
}

/// Plant a pane snapshot verbatim (test-support only): the teardown
/// regression plants under a container key directly — the production write
/// (`persist_right_pane`) needs a live shell pane.
#[cfg(feature = "test-support")]
pub fn diagnostic_persist_right_pane(container_id: &str, payload: String) {
    PANE_REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(container_id.to_string(), payload);
}

/// The test-support read over [`load_right_pane`] — the teardown regression
/// proves a dead container's snapshot is gone.
#[cfg(feature = "test-support")]
pub fn diagnostic_right_pane_snapshot(container_id: &str) -> Option<String> {
    load_right_pane(container_id)
}

/// The foreground thread's PROJECT directory, refreshed by the assembly's
/// observer on every projection — "open in editor" and the project menu's
/// no-project fallback read it rather than silently opening `$HOME`. The
/// membership is the primary voice (the project entity's primary root); the
/// wire row's bound path stays the fallback for a session with a directory
/// but no membership. (The dock/right spawn cwd no longer rides a static:
/// it reads the foreground container's attrs through
/// [`crate::Workspace::container_attrs`] at spawn time.)
fn refresh_foreground_project(
    ws: &Entity<Workspace>,
    rows: &[crate::sidebar_projection::ContainerRow],
    cx: &App,
) {
    let fg = ws
        .read(cx)
        .chat
        .read(cx)
        .store
        .as_ref()
        .map(|(_, sid)| sid.clone());
    let project = fg
        .as_ref()
        .and_then(|id| crate::projects::membership_of(id))
        .and_then(|project_id| crate::projects::get_project(&project_id))
        .and_then(|project| {
            project
                .roots
                .iter()
                .find(|root| root.primary)
                .map(|root| root.path.clone())
        })
        .or_else(|| {
            fg.as_ref()
                .and_then(|id| rows.iter().find(|r| &r.id == id))
                .and_then(|r| r.project.clone())
                .filter(|p| !p.is_empty())
                .map(std::path::PathBuf::from)
        });
    *FOREGROUND_PROJECT.lock().expect("foreground project lock") = project;
}

/// The container teardown's shell half: reclaim the LIVE dock view and the
/// LIVE right-pane session when the dead container owns them. Runs on the
/// bare App — outside any workspace/window lease (closing live tabs
/// re-enters the shell, which a leased update must not; the teardown defers
/// here for exactly that). Also sweeps any stash entries a racing switch
/// re-created under the dead container's key.
pub fn reclaim_container_panes(container: &str, cx: &mut App) {
    let Some(ws) = crate::dispatch::workspace_global() else {
        return;
    };
    // Belt and braces: the observer may have re-stashed under the dead key
    // between the workspace-side reclaim and this deferred half.
    ws.update(cx, |ws, _| ws.reclaim_stash(container));
    let dock_mine = ws.read(cx).dock_owner().as_deref() == Some(container);
    let right_mine = ws.read(cx).right_owner().as_deref() == Some(container);
    if dock_mine {
        ws.update(cx, |ws, _| ws.set_dock_owner(None));
        stop_dock_terminal(cx);
    }
    if right_mine {
        ws.update(cx, |ws, _| ws.set_right_owner(None));
        // The PTY kinds' close never re-enters the workspace; the browser
        // kind's does (its workspace half already reclaimed — the close is a
        // no-op there) — all fine on the bare App.
        for kind in crate::tool_tabs::PTY_TAB_KINDS {
            close_tool_tabs_of_kind(kind, cx);
        }
        close_tool_tabs_of_kind("browser", cx);
    }
}

/// Wrap a chrome Shell into the window's Root view (the bin mounts this).
pub fn root(shell: Entity<Shell>, window: &mut Window, cx: &mut App) -> Entity<Root> {
    cx.new(|cx| Root::new(shell, window, cx))
}
