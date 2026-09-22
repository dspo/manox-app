//! The chrome-shell assembly for the real app (PLAN-CHROME-CHAT-SPLIT
//! Phase 4, dual-shell ruling 2026-09-22: both shells ship, the build
//! chooses). This module owns everything the chrome build mounts:
//!
//! - the session list fed by the REAL multiplexer (the server wire rows
//!   projected through `sidebar_projection` — five states, team forest,
//!   tags, project grouping);
//! - pin/archive hooks routed through the thread store (the same seam the
//!   legacy sidebar's row actions use; the next list snapshot reconciles);
//! - the right-pane tool registry and bottom dock (the integrated-terminal
//!   adapters in `tool_tabs`);
//! - the main surface: a placeholder until the chat-column view tranche
//!   lands (the render half still lives on Workspace).
//!
//! The legacy shell is untouched — `--features chrome-shell` on the manox
//! bin swaps only the mounted root.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, Window};
use gpui_component::Root;
use manox_agent_chrome_ui::{
    CustomizationRow, FixedRow, HostHooks, MainSurface, Shell, ShellConfig, icons,
};

use crate::Workspace;
use crate::tool_tabs::{TerminalTool, ThreadTerminalPanelSurface};

/// Build the whole chrome window root: multiplexer + shell + the projection
/// pump. Called from the manox bin under `--features chrome-shell`.
pub fn mount(window: &mut Window, cx: &mut App) -> Entity<Shell> {
    // ONE workspace (embedded render mode) carries the whole data face —
    // its multiplexer feeds both the sidebar projection and the
    // conversation column mounted as the chrome shell's main surface.
    let ws = cx.new(|cx| Workspace::new_embedded(window, cx));
    let shell = cx.new(|cx| Shell::new(shell_config(ws.clone(), cx), window, cx));
    let shell_weak = shell.downgrade();
    // Per-thread dock state: each visited thread keeps its live panel
    // terminal across switches (the legacy right-pane stash semantic — a
    // stashed view keeps its process running; only explicit collapse of the
    // LIVE dock tears one down).
    let mut dock_stash: HashMap<String, gpui::AnyView> = HashMap::new();
    let mut dock_thread: Option<String> = None;
    cx.observe(&ws, move |ws, cx| {
        let Some(shell) = shell_weak.upgrade() else {
            return;
        };
        let mux = ws.read(cx).multiplexer.clone();
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
        refresh_foreground_cwd(&ws, cx);
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
            });
            dock_thread = fg;
        }
    })
    .detach();
    shell
}

fn shell_config(ws: Entity<Workspace>, _cx: &mut Context<Shell>) -> ShellConfig {
    let placeholder: gpui::AnyView = ws.clone().into();
    ShellConfig {
        main: Arc::new(PendingMain {
            view: placeholder,
            ws: ws.clone(),
        }),
        tool_kinds: vec![Arc::new(TerminalTool)],
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
            // Pin/archive ride the thread store — the same seam the legacy
            // sidebar's row actions use; the next wire snapshot reconciles.
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
            // reconciliation — exactly what the legacy sidebar click runs.
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
        // The active thread's display title from the foreground store (the
        // same face the legacy title bar reads); "manox" before any
        // interaction.
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

fn refresh_foreground_cwd(ws: &Entity<Workspace>, cx: &App) {
    let cwd = ws
        .read(cx)
        .chat
        .read(cx)
        .store
        .as_ref()
        .map(|s| std::path::PathBuf::from(s.read(cx).store.cwd.clone()));
    *FOREGROUND_CWD.lock().expect("foreground cwd lock") = cwd;
}

/// Wrap a chrome Shell into the window's Root view (the bin mounts this).
pub fn root(shell: Entity<Shell>, window: &mut Window, cx: &mut App) -> Entity<Root> {
    cx.new(|cx| Root::new(shell, window, cx))
}
