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

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
};
use gpui_component::Root;
use manox_agent_chrome_ui::theme;
use manox_agent_chrome_ui::{
    CustomizationRow, FixedRow, HostHooks, MainSurface, Shell, ShellConfig, icons,
};
use manox_session_core::agent_client::AgentClient;

use crate::multiplexer::SessionMultiplexer;
use crate::sidebar_projection::{self, UnreadMirrors};
use crate::tool_tabs::{TerminalPanelSurface, TerminalTool};

/// Build the whole chrome window root: multiplexer + shell + the projection
/// pump. Called from the manox bin under `--features chrome-shell`.
pub fn mount(window: &mut Window, cx: &mut App) -> Entity<Shell> {
    // The same embedding path Workspace::new uses: one process-global
    // AgentServer, one "desktop" client, one multiplexer over it.
    let cwd = manox_agent::paths::home_dir().unwrap_or_else(|| ".".into());
    let agent_server = manox_session_core::agent_server::global(cwd.clone());
    let client = Arc::new(AgentClient::connect(
        &agent_server,
        "desktop",
        vec![
            manox_protocol::AnswerKind::Approve,
            manox_protocol::AnswerKind::AskUserQuestion,
        ],
        vec![],
    ));
    let mux = cx.new(|cx| SessionMultiplexer::with_client(client.clone(), cx));

    let shell = cx.new(|cx| Shell::new(shell_config(mux.clone(), cx), window, cx));

    // The list pump: every multiplexer notify (wire list / leaf mirrors
    // changed) re-projects the rows into the shell's sidebar.
    let pump_shell = shell.clone();
    let pump_mux = mux.clone();
    cx.observe(&mux, move |_, cx| {
        let rows = pump_mux.read(cx).thread_list().to_vec();
        let unread: UnreadMirrors = pump_mux.read(cx).unread_map(cx);
        let sessions: Vec<manox_agent_chrome_ui::shell::SessionRow> =
            sidebar_projection::project_groups(&rows, &unread)
                .into_iter()
                .flat_map(manox_agent_chrome_ui::shell::SessionRow::from_group)
                .collect();
        pump_shell.update(cx, |shell, cx| {
            shell.set_sessions(sessions);
            cx.notify();
        });
    })
    .detach();

    shell
}

fn shell_config(mux: Entity<SessionMultiplexer>, cx: &mut Context<Shell>) -> ShellConfig {
    let _ = mux;
    let placeholder: gpui::AnyView = cx.new(|_| ChatPending).into();
    ShellConfig {
        main: Arc::new(PendingMain { view: placeholder }),
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

/// The chat column's seat until its view tranche lands.
struct ChatPending;

impl Render for ChatPending {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::FG_FAINT)
            .text_size(gpui::px(13.))
            .child(manox_i18n::t("chrome-main-pending"))
    }
}

struct PendingMain {
    view: gpui::AnyView,
}

impl MainSurface for PendingMain {
    fn view(&self) -> gpui::AnyView {
        self.view.clone()
    }

    fn title(&self, _cx: &App) -> gpui::SharedString {
        "Manox".into()
    }
}

/// Wrap a chrome Shell into the window's Root view (the bin mounts this).
pub fn root(shell: Entity<Shell>, window: &mut Window, cx: &mut App) -> Entity<Root> {
    cx.new(|cx| Root::new(shell, window, cx))
}
