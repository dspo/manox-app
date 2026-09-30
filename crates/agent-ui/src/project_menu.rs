//! The project group's action menu — the sidebar feature the legacy shell
//! carried (retired with it in the 2026-09-28 single-shell switch): launch
//! agents, a terminal or the editor IN the project directory, and remove the
//! project from the sidebar.
//!
//! The chrome (manox-agent-chrome-ui) supplies only the menu SURFACE — the
//! header's ellipsis button / right-click and the anchored mount; this module
//! is the host half that builds the content. Every action targets the group's
//! project directory:
//!
//! - 新建会话: a Manox thread bound to the project, or a provider→model
//!   cascade per external CLI agent (the same cx launch path the right
//!   pane's agent tabs use, rooted at the project instead of the foreground
//!   thread's cwd);
//! - 新建终端: a plain shell PTY in the project;
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
use crate::views::model_cascade::{build_model_menu, launch_wire_key};

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
    let ws_menu = ws.clone();
    let dir_manox = project_dir.clone();
    let dir_terminal = project_dir.clone();
    let dir_cascade = project_dir.clone();
    let dir_remove = project_dir.clone();
    // VS Code: the settings-injected launch, opening the project directory.
    // For the no-project bucket the foreground project backs the launch —
    // "open in editor" never silently opens $HOME.
    let vscode_target = project_dir.clone().or_else(|| {
        crate::chrome_assembly::foreground_project().filter(|p| !p.as_os_str().is_empty())
    });
    PopupMenu::build(window, cx, |menu, window, cx| {
        let mut menu = menu.max_w(px(280.));
        // 新建会话: Manox flat row + one provider→model cascade per external
        // agent kind — the legacy menu's shape, scoped to this project. The
        // cascades render through the COMPOSER model picker's shared builder
        // (provider display-name submenus, wire-tag rows), so every model
        // picker in the app looks and picks the same.
        menu = menu.submenu_with_icon(
            Some(Icon::default().path("icons/plus.svg")),
            manox_i18n::t("sidebar-new-session-label"),
            window,
            cx,
            move |submenu, window, cx| {
                let mut submenu = submenu;
                let ws_manox = ws_menu.clone();
                let dir_new = dir_manox.clone();
                submenu = submenu.item(
                    PopupMenuItem::new(manox_i18n::t("sidebar-new-session-manox"))
                        .icon(Icon::default().path("icons/manox.svg"))
                        .on_click(move |_, _window, cx| {
                            new_thread_at(&ws_manox, dir_new.clone(), cx);
                        }),
                );
                for (agent_id, display, svg) in EXTERNAL_AGENTS {
                    let dir_agent = dir_cascade.clone();
                    submenu = submenu.submenu_with_icon(
                        Some(Icon::default().path(svg)),
                        display,
                        window,
                        cx,
                        move |sub, window, cx| {
                            let dir_agent = dir_agent.clone();
                            build_model_menu(
                                sub,
                                crate::model_catalog::rows(),
                                move |row, window, cx| {
                                    spawn_agent_tab(
                                        &(agent_id, display, svg),
                                        &row.provider_display,
                                        &row.id,
                                        launch_wire_key(&row.api),
                                        dir_agent.clone(),
                                        window,
                                        cx,
                                    );
                                },
                                window,
                                cx,
                            )
                        },
                    );
                }
                submenu
            },
        );
        // 新建终端: a plain shell PTY rooted at the project.
        menu = menu.item(
            PopupMenuItem::new(manox_i18n::t("sidebar-new-terminal"))
                .icon(Icon::default().path("icons/terminal.svg"))
                .on_click(move |_, window, cx| {
                    let cwd = dir_terminal.clone().unwrap_or_else(fallback_cwd);
                    match crate::tool_tabs::spawn_standalone_terminal(&cwd, cx) {
                        Ok(view) => {
                            open_terminal_tab(manox_i18n::t("chrome-tab-terminal"), view, cx)
                        }
                        Err(e) => spawn_failed_notification(
                            &manox_i18n::t("chrome-tab-terminal"),
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
                .disabled(!manox_ext_agents::vscode_app::is_installed() || vscode_target.is_none())
                .on_click(move |_, _window, cx| launch_vscode(vscode_target.clone(), cx)),
        );
        // 移除项目: only a real project group can be removed. The overlay
        // write dissolves the group on the next projection (the store's
        // folder registry has no AHP-side grouping voice, but it is written
        // on every bind, so it leaves too); notifying the workspace runs the
        // projection now.
        if let Some(dir_remove) = dir_remove {
            let ws_remove = ws.clone();
            menu = menu.separator().item(
                PopupMenuItem::new(manox_i18n::t("sidebar-remove-project"))
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

/// The external CLI agents the cascade offers, in menu order (the same trio
/// the right pane's agent tabs register).
const EXTERNAL_AGENTS: [(&str, &str, &str); 3] = [
    ("claude", "Claude Code", "icons/claude.svg"),
    ("codex", "Codex", "icons/codex.svg"),
    ("copilot", "GitHub Copilot", "icons/githubcopilot.svg"),
];

/// A Manox thread bound to `dir` (inheritance and the create-session intent
/// live in the workspace). Re-registering a removed path rides the
/// workspace's own bind seam ([`crate::workspace::Workspace`]'s project
/// registration) — launching into a folder is the re-registration.
fn new_thread_at(ws: &gpui::WeakEntity<Workspace>, dir: Option<PathBuf>, cx: &mut App) {
    // The window handle supplies the `&mut Window` the switch path needs
    // (attach, drafts stash); a stale handle means the window is gone and
    // the action has no surface left.
    if let Some(handle) = crate::dispatch::window_global() {
        let ws = ws.clone();
        let _ = handle.update(cx, move |_, window, cx| {
            let _ = ws.update(cx, |ws, cx| {
                ws.start_new_thread(dir.clone(), window, cx);
                cx.notify();
            });
        });
    }
}

/// Spawn the agent CLI under the picked endpoint, rooted at the project
/// (falling back to the host's cwd rules), and open it as a right-pane tab.
/// A spawn failure notifies instead of opening anything. `agent` is the
/// `EXTERNAL_AGENTS` row (id, display, icon).
fn spawn_agent_tab(
    agent: &(&'static str, &'static str, &'static str),
    provider: &str,
    model: &str,
    wire: Option<String>,
    dir: Option<PathBuf>,
    window: &mut Window,
    cx: &mut App,
) {
    let (agent_id, display, svg) = *agent;
    let cwd = dir.unwrap_or_else(fallback_cwd);
    match crate::tool_tabs::spawn_agent_terminal(agent_id, &cwd, provider, model, wire, cx) {
        Ok(view) => {
            let tab = crate::tool_tabs::prebuilt_terminal_tab(display, svg, view);
            crate::chrome_assembly::open_tool_tab(tab, cx);
        }
        Err(e) => spawn_failed_notification(display, &e, window, cx),
    }
}

fn open_terminal_tab(
    title: impl Into<gpui::SharedString>,
    view: Entity<terminal_ui::TerminalView>,
    cx: &mut App,
) {
    let tab = crate::tool_tabs::prebuilt_terminal_tab(title, "icons/terminal.svg", view);
    crate::chrome_assembly::open_tool_tab(tab, cx);
}

/// The failure notice names the program that failed to start — the terminal
/// row passes its localized name, an agent row its display name.
fn spawn_failed_notification(prog: &str, error: &str, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::error(manox_i18n::t_str(
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
                manox_ext_agents::launch_vscode_app_from_settings(folder.as_deref())
            })
            .await;
        if let Some(handle) = handle
            && let Err(update_err) = handle.update(cx, |_, window, cx| {
                if let Err(e) = &launch_err {
                    tracing::error!(error = %e, "project-menu VS Code launch failed");
                    window.push_notification(
                        Notification::error(format!(
                            "{}: {e}",
                            manox_i18n::t("vscode-app-launch-failed")
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

/// The no-project bucket's launch target: the foreground thread's cwd (the
/// same fallback the dock terminal uses), then home.
fn fallback_cwd() -> PathBuf {
    crate::tool_tabs::thread_cwd_or_home()
}
