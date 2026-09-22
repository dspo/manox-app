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

use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, Window};
use gpui_component::Root;
use manox_agent_chrome_ui::{
    CustomizationRow, FixedRow, HostHooks, MainSurface, Shell, ShellConfig, icons,
};

use crate::Workspace;
use crate::tool_tabs::{TerminalPanelSurface, TerminalTool};

/// Build the whole chrome window root: multiplexer + shell + the projection
/// pump. Called from the manox bin under `--features chrome-shell`.
pub fn mount(window: &mut Window, cx: &mut App) -> Entity<Shell> {
    // ONE workspace (embedded render mode) carries the whole data face —
    // its multiplexer feeds both the sidebar projection and the
    // conversation column mounted as the chrome shell's main surface.
    let ws = cx.new(|cx| Workspace::new_embedded(window, cx));
    let shell = cx.new(|cx| Shell::new(shell_config(ws.clone(), cx), window, cx));
    let shell_weak = shell.downgrade();
    cx.observe(&ws, move |ws, cx| {
        let Some(shell) = shell_weak.upgrade() else {
            return;
        };
        let rows = ws.read(cx).multiplexer.read(cx).thread_list().to_vec();
        let unread = ws.read(cx).multiplexer.read(cx).unread_map(cx);
        let sessions: Vec<manox_agent_chrome_ui::shell::SessionRow> =
            crate::sidebar_projection::project_groups(&rows, &unread)
                .into_iter()
                .flat_map(manox_agent_chrome_ui::shell::SessionRow::from_group)
                .collect();
        shell.update(cx, |shell, cx| {
            shell.set_sessions(sessions);
            cx.notify();
        });
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
        panel_surface: Some(Arc::new(TerminalPanelSurface)),
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
            on_new_session: None,
            on_select: None,
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

/// Wrap a chrome Shell into the window's Root view (the bin mounts this).
pub fn root(shell: Entity<Shell>, window: &mut Window, cx: &mut App) -> Entity<Root> {
    cx.new(|cx| Root::new(shell, window, cx))
}
