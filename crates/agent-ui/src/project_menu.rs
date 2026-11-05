//! The project group's action menu — the sidebar feature the legacy shell
//! carried (retired with it in the 2026-09-28 single-shell switch): open the
//! launcher in the project directory, launch a terminal or the editor IN it,
//! and remove the project from the sidebar.
//!
//! The chrome (steer-agent-chrome-ui) supplies only the menu SURFACE — the
//! header's ellipsis button / right-click and the anchored mount; this module
//! is the host half that builds the content. Every action targets the group's
//! project directory:
//!
//! - 新建对话: the LAUNCHER with this project preselected (design §3.3 — the
//!   per-agent cascades retired from the menu; the launcher's action row is
//!   the one agent launch face, D8');
//! - 新建终端: a plain shell PTY in the project (the launcher's terminal pill
//!   is the second entry into the same path — a shortcut that needs no
//!   composer context, so it stays direct);
//! - VS Code: the settings-injected launch (no launch-time choice);
//! - 移除项目: the client-side overlay in [`crate::project_registry`] — the
//!   group dissolves and its threads fall back to the loose "Chats" bucket.
//!
//! A no-project group (the "Chats" bucket) keeps every launch row, scoped to
//! the host's fallback cwd rules, and hides the remove-project row.

use std::path::PathBuf;

use gpui::{App, AppContext as _, Entity, Window, px};
use gpui_component::Icon;
use gpui_component::WindowExt as _;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::notification::Notification;

use crate::Workspace;
use crate::workspace::external_sessions::ExternalSessionLaunch;

/// Build the project group's menu. `project` is the group's directory
/// (`None` on the no-project bucket — the menu then scopes to the fallback
/// cwd and carries no remove-project row). The menu items route their
/// actions through the workspace entity (the data face); right-pane opens go
/// through the shell handle in `chrome_assembly`.
pub fn group_menu(
    project: Option<&str>,
    ws: &gpui::WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<PopupMenu> {
    let project_dir: Option<PathBuf> = project
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty());
    // Every menu closure is 'static: it captures OWNED clones of the
    // workspace handle and the project directory, never the caller's
    // borrows.
    let ws = ws.clone();
    let ws_terminal = ws.clone();
    let dir_launcher = project_dir.clone();
    let dir_terminal = project_dir.clone();
    let dir_remove = project_dir.clone();
    // VS Code: the settings-injected launch, opening the project directory.
    // For the no-project bucket the foreground project backs the launch —
    // "open in editor" never silently opens $HOME.
    let vscode_target = project_dir.clone().or_else(|| {
        crate::chrome_assembly::foreground_project().filter(|p| !p.as_os_str().is_empty())
    });
    PopupMenu::build(window, cx, |menu, _window, _cx| {
        let mut menu = menu.max_w(px(280.));
        // The Chats bucket is where projects are born: the create dialog
        // rides this menu only (the sidebar footer is the other entry; the
        // launcher chip menu joins them).
        if project_dir.is_none() {
            menu = menu.item(
                PopupMenuItem::new(steer_i18n::t("project-new-menu"))
                    .icon(Icon::default().path("icons/plus.svg"))
                    .on_click(move |_, window, cx| {
                        crate::chrome_assembly::open_project_dialog(None, window, cx);
                    }),
            );
        }
        // 新建对话: the launcher with this group's project preselected (the
        // bare ⌘N launcher resolves its own default). No container is minted
        // here — the composer's send is the mint.
        menu = menu.item(
            PopupMenuItem::new(steer_i18n::t("project-new-conversation"))
                .icon(Icon::default().path("icons/steer.svg"))
                .on_click({
                    let ws = ws.clone();
                    move |_, window, cx| {
                        let _ = ws.update(cx, |ws, cx| {
                            ws.enter_launcher(dir_launcher.clone(), window, cx);
                            cx.notify();
                        });
                    }
                }),
        );
        // 新建终端: a plain shell PTY rooted at the project.
        menu = menu.item(
            PopupMenuItem::new(steer_i18n::t("sidebar-new-terminal"))
                .icon(Icon::default().path("icons/terminal.svg"))
                .on_click(move |_, window, cx| {
                    let cwd = dir_terminal.clone().unwrap_or_else(|| fallback_cwd(cx));
                    // The shell PTY IS the new container's 0 号 — no
                    // container env to inherit; its own attrs snapshot is
                    // captured at the registration that follows.
                    let no_attrs = crate::workspace::container::ContainerAttrs {
                        cwd: cwd.clone(),
                        env: Default::default(),
                    };
                    match crate::tool_tabs::spawn_standalone_terminal(&cwd, &no_attrs, cx) {
                        Ok(view) => launch_external(
                            &ws_terminal,
                            ExternalSessionLaunch {
                                agent_id: "terminal",
                                cx_session_id: String::new(),
                                label: steer_i18n::t("chrome-tab-ghostty").to_string(),
                                svg: "icons/terminal.svg",
                                project: dir_terminal.clone(),
                                // A plain shell owns no cx session handle —
                                // its PTY dies with the view drop.
                                cwd: cwd.clone(),
                                handle: None,
                            },
                            view,
                            window,
                            cx,
                        ),
                        Err(e) => spawn_failed_notification(
                            &steer_i18n::t("chrome-tab-ghostty"),
                            &e,
                            window,
                            cx,
                        ),
                    }
                }),
        );
        menu = menu.item(
            PopupMenuItem::new("VS Code")
                .icon(Icon::default().path("icons/vscode.svg"))
                .disabled(!steer_ext_agents::vscode_app::is_installed() || vscode_target.is_none())
                .on_click(move |_, _window, cx| launch_vscode(vscode_target.clone(), cx)),
        );
        // 移除项目: only a real project group can be removed. The overlay
        // write dissolves the group on the next projection (the store's
        // folder registry has no AHP-side grouping voice, but it is written
        // on every bind, so it leaves too); notifying the workspace runs the
        // projection now. (Step-4 reworks this row onto the project
        // entity's delete semantics.)
        if let Some(dir_remove) = dir_remove {
            let ws_remove = ws.clone();
            menu = menu.separator().item(
                PopupMenuItem::new(steer_i18n::t("sidebar-remove-project"))
                    .icon(Icon::default().path("icons/trash-2.svg"))
                    .on_click(move |_, _window, cx| {
                        let path = dir_remove.to_string_lossy();
                        crate::project_registry::forget_project(&path);
                        manox_agent::thread_store_global().with_mut(|s| s.remove_project(&path));
                        let _ = ws_remove.update(cx, |_, cx| cx.notify());
                    }),
            );
        }
        menu
    })
}

/// Register a spawned terminal as an external session and bring it up —
/// FOCUSED — in the MAIN column (the shell wraps the main column —
/// terminal/TUI sessions live there, not in the right pane). The sidebar
/// row rides the workspace's own projection.
fn launch_external(
    ws: &gpui::WeakEntity<Workspace>,
    launch: ExternalSessionLaunch<'_>,
    view: Entity<steer_ghostty::GhosttyView>,
    window: &mut Window,
    cx: &mut App,
) {
    let _ = ws.update(cx, |ws, cx| {
        ws.spawn_external_session(launch, view, window, cx);
    });
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

/// The no-project bucket's launch target: the foreground container's cwd
/// (the same fallback the dock terminal uses), then home.
fn fallback_cwd(cx: &App) -> PathBuf {
    crate::tool_tabs::thread_cwd_or_home(cx)
}
