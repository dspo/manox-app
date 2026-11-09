//! The project group's action menu — the sidebar feature the legacy shell
//! carried (retired with it in the 2026-09-28 single-shell switch): open the
//! launcher in the project, launch a terminal or the editor IN it, edit the
//! project, remove it.
//!
//! The chrome (steer-agent-chrome-ui) supplies only the menu SURFACE — the
//! header's ellipsis button / right-click and the anchored mount; this module
//! is the host half that builds the content. `project` is the group's
//! PROJECT ID (`None` on the loose bucket); the launch targets resolve
//! through the registry AT CLICK TIME, so a project deleted while the menu
//! is open simply does nothing:
//!
//! - 新建对话: the LAUNCHER with this project preselected (design §3.3 — the
//!   per-agent cascades retired from the menu; the launcher's action row is
//!   the one agent launch face, D8');
//! - 新建终端: a plain shell PTY at the project's primary root (the
//!   launcher's terminal pill is the second entry into the same target — a
//!   shortcut that needs no composer context, so it stays direct); the minted
//!   container joins the project (membership, design §2.1's write timing);
//! - VS Code: the settings-injected launch at the primary root;
//! - 编辑项目: the create/edit dialog in its edit variant;
//! - 移除项目: `projects::delete_project` behind the app's alert-dialog
//!   confirmation — the entity and its memberships go, the containers fall
//!   back to the loose bucket, sessions and journals are untouched.
//!
//! The loose bucket keeps every launch row scoped to the host's fallback
//! cwd rules, adds the create-project row (projects are born here), and
//! carries no edit/remove.

use std::path::PathBuf;

use gpui::{App, AppContext as _, Entity, Window, px};
use gpui_component::Icon;
use gpui_component::WindowExt as _;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::notification::Notification;

use crate::Workspace;
use crate::workspace::external_sessions::ExternalSessionLaunch;

/// Build the project group's menu. `project` is the group's project id
/// (`None` on the loose bucket — the menu then scopes to the fallback cwd
/// and carries no edit/remove rows). The menu items route their actions
/// through the workspace entity (the data face); right-pane opens go
/// through the shell handle in `chrome_assembly`.
pub fn group_menu(
    project: Option<&str>,
    ws: &gpui::WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<PopupMenu> {
    let project_id: Option<String> = project
        .map(str::to_string)
        .filter(|id| crate::projects::get_project(id).is_some());
    // Every menu closure is 'static: it captures OWNED clones of the
    // workspace handle and the project id, never the caller's borrows.
    let ws = ws.clone();
    let ws_terminal = ws.clone();
    let id_launcher = project_id.clone();
    let id_terminal = project_id.clone();
    let id_edit = project_id.clone();
    let id_remove = project_id.clone();
    // VS Code: the settings-injected launch, opening the project's primary
    // root. For the loose bucket the foreground project backs the launch —
    // "open in editor" never silently opens $HOME.
    let vscode_target = project_id.as_deref().and_then(primary_root).or_else(|| {
        crate::chrome_assembly::foreground_project().filter(|p| !p.as_os_str().is_empty())
    });
    PopupMenu::build(window, cx, |menu, _window, _cx| {
        let mut menu = menu.max_w(px(280.));
        // The loose bucket is where projects are born: the create dialog
        // rides this menu only (the sidebar footer is the other entry; the
        // launcher chip menu joins them).
        if project_id.is_none() {
            menu = menu.item(
                PopupMenuItem::new(steer_i18n::t("project-new-menu"))
                    .icon(Icon::default().path("icons/plus.svg"))
                    .on_click({
                        let ws = ws.clone();
                        move |_, window, cx| {
                            let _ =
                                ws.update(cx, |ws, cx| ws.open_project_dialog(None, window, cx));
                        }
                    }),
            );
        }
        // 新建对话: the launcher with this project preselected BY ID (root
        // overlap is allowed — a path find could pick the wrong owner) and
        // LOCKED — the chip renders display-only, the target is the entry's
        // intent. The loose bucket's row lands the bare unlocked no-project
        // default. No container is minted here — the composer's send is the
        // mint.
        menu = menu.item(
            PopupMenuItem::new(steer_i18n::t("project-new-conversation"))
                .icon(Icon::default().path("icons/steer.svg"))
                .on_click({
                    let ws = ws.clone();
                    move |_, window, cx| {
                        let preset = id_launcher
                            .clone()
                            .map(crate::launcher::LauncherTarget::Project);
                        let locked = preset.is_some();
                        let _ = ws.update(cx, |ws, cx| {
                            ws.enter_launcher(preset, locked, window, cx);
                            cx.notify();
                        });
                    }
                }),
        );
        // 新建终端: a plain shell PTY rooted at the project's primary root;
        // the minted container joins the project.
        menu = menu.item(
            PopupMenuItem::new(steer_i18n::t("sidebar-new-terminal"))
                .icon(Icon::default().path("icons/terminal.svg"))
                .on_click({
                    let ws = ws_terminal.clone();
                    move |_, window, cx| {
                        let dir = id_terminal.as_deref().and_then(primary_root);
                        let cwd = dir.clone().unwrap_or_else(|| fallback_cwd(cx));
                        // The shell PTY IS the new container's 0 号 — no
                        // container env to inherit; its own attrs snapshot is
                        // captured at the registration that follows.
                        let no_attrs = crate::workspace::container::ContainerAttrs {
                            cwd: cwd.clone(),
                            env: Default::default(),
                        };
                        match crate::tool_tabs::spawn_standalone_terminal(&cwd, &no_attrs, cx) {
                            Ok(view) => {
                                let _ = ws.update(cx, |ws, cx| {
                                    let id = ws.spawn_external_session(
                                        ExternalSessionLaunch {
                                            agent_id: "terminal",
                                            cx_session_id: String::new(),
                                            label: steer_i18n::t("chrome-tab-ghostty").to_string(),
                                            svg: "icons/terminal.svg",
                                            cwd: cwd.clone(),
                                            handle: None,
                                        },
                                        view,
                                        window,
                                        cx,
                                    );
                                    if let Some(project_id) = &id_terminal {
                                        crate::projects::set_membership(&id, project_id);
                                    }
                                    cx.notify();
                                });
                            }
                            Err(e) => spawn_failed_notification(
                                &steer_i18n::t("chrome-tab-ghostty"),
                                &e,
                                window,
                                cx,
                            ),
                        }
                    }
                }),
        );
        menu = menu.item(
            PopupMenuItem::new("VS Code")
                .icon(Icon::default().path("icons/vscode.svg"))
                .disabled(!steer_ext_agents::vscode_app::is_installed() || vscode_target.is_none())
                .on_click(move |_, _window, cx| launch_vscode(vscode_target.clone(), cx)),
        );
        // 编辑项目 / 移除项目: only a real project group carries them. The
        // removal deletes the ENTITY behind the app's one confirm face — its
        // memberships cascade, the containers fall back to the loose bucket,
        // sessions and journals are untouched; notifying the workspace runs
        // the projection now.
        if let Some(id_edit) = id_edit {
            menu = menu.separator().item(
                PopupMenuItem::new(steer_i18n::t("project-edit-title"))
                    .icon(Icon::default().path("icons/settings-2.svg"))
                    .on_click({
                        let ws = ws.clone();
                        move |_, window, cx| {
                            let _ = ws.update(cx, |ws, cx| {
                                ws.open_project_dialog(Some(&id_edit), window, cx)
                            });
                        }
                    }),
            );
        }
        if let Some(id_remove) = id_remove {
            let ws_remove = ws.clone();
            menu = menu.item(
                PopupMenuItem::new(steer_i18n::t("sidebar-remove-project"))
                    .icon(Icon::default().path("icons/trash-2.svg"))
                    .on_click(move |_, window, cx| {
                        // Both layers may re-run (the item is an Fn, the
                        // alert builder an Fn): every face clones its own
                        // handles, the on_ok closure moves them out of THIS
                        // builder run.
                        let id_remove = id_remove.clone();
                        let ws_remove = ws_remove.clone();
                        window.open_alert_dialog(cx, move |alert, _, _| {
                            let project_id = id_remove.clone();
                            let ws_remove = ws_remove.clone();
                            alert
                                .title(steer_i18n::t("project-remove-confirm-title"))
                                .description(steer_i18n::t("project-remove-confirm-body"))
                                .show_cancel(true)
                                .ok_text(steer_i18n::t("project-remove-confirm-ok"))
                                .cancel_text(steer_i18n::t("project-cancel"))
                                .on_ok(move |_, _, cx| {
                                    crate::projects::delete_project(&project_id);
                                    let _ = ws_remove.update(cx, |_, cx| cx.notify());
                                    true
                                })
                        });
                    }),
            );
        }
        menu
    })
}

/// The project's primary root, resolved at call time (the registry is the
/// launch-target face; a deleted project resolves to nothing).
fn primary_root(project_id: &str) -> Option<PathBuf> {
    crate::projects::get_project(project_id).and_then(|project| {
        project
            .roots
            .iter()
            .find(|root| root.primary)
            .map(|root| root.path.clone())
    })
}

/// The failure notice names the program that failed to start — the terminal
/// row passes its localized name.
fn spawn_failed_notification(prog: &str, error: &str, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::error(steer_i18n::t_str(
            "chrome-spawn-failed",
            &[("prog", prog), ("err", error)],
        )),
        cx,
    );
}

/// The settings-injected VS Code launch on the background executor (the
/// launch blocks on `open`'s exit); only a failure notifies — a success
/// announces itself by VS Code opening. The same shape as the shell's
/// open-in-editor hook.
fn launch_vscode(folder: Option<PathBuf>, cx: &mut App) {
    let handle = crate::dispatch::window_global();
    cx.spawn(async move |cx| {
        let launch_err = cx
            .background_spawn(async move {
                steer_ext_agents::launch_vscode_app_from_settings(folder.as_deref())
            })
            .await;
        if let Some(handle) = handle
            && let Err(update_err) = handle.update(cx, |_, window, cx| {
                if let Err(e) = &launch_err {
                    tracing::error!(error = %e, "project-menu VS Code launch failed");
                    window.push_notification(
                        Notification::error(format!(
                            "{}: {e}",
                            steer_i18n::t("vscode-app-launch-failed")
                        )),
                        cx,
                    );
                }
            })
        {
            tracing::warn!(
                launch = ?launch_err,
                update = ?update_err,
                "VS Code launch result unreported (window gone)"
            );
        }
    })
    .detach();
}

/// The loose bucket's launch target: the foreground container's cwd
/// (the same fallback the dock terminal uses), then home.
fn fallback_cwd(cx: &App) -> PathBuf {
    crate::tool_tabs::thread_cwd_or_home(cx)
}
