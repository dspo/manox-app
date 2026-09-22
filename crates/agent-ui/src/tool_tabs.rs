//! ToolTab assembly adapters (PLAN-CHROME-CHAT-SPLIT Phase 4, tranche 2):
//! wrap the app's real surfaces as chrome-shell right-pane tabs. This
//! tranche ships the integrated terminal ($SHELL, standalone PTY spawn —
//! no Workspace coupling); the browser/agent-CLI/editor tabs follow with
//! their hosts' decoupling (the browser host and the CLI-agent launch path
//! are still Workspace-bound).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{AnyElement, App, Entity, IntoElement, SharedString, Window};
use manox_agent_chrome_ui::right_pane::{TabStore, ToolTab, ToolTabFactory};
use manox_agent_chrome_ui::theme::{Icon, icon, icons};

static INSTANCE: AtomicU64 = AtomicU64::new(0);

fn next_instance_id(kind: &str) -> String {
    format!("{kind}-{}", INSTANCE.fetch_add(1, Ordering::Relaxed))
}

/// The integrated-terminal kind: one $SHELL per tab, mount-equals-launch.
pub struct TerminalTool;

impl ToolTabFactory for TerminalTool {
    fn kind(&self) -> &'static str {
        "terminal"
    }

    fn create(&self) -> Arc<dyn ToolTab> {
        Arc::new(TerminalTab {
            id: next_instance_id("terminal"),
        })
    }

    fn quick_action(&self) -> Option<SharedString> {
        Some(manox_i18n::t("chrome-quick-terminal").into())
    }

    fn icon(&self, _cx: &App) -> AnyElement {
        icon(icons::TERMINAL, 15.).into_any_element()
    }
}

struct TerminalTab {
    id: String,
}

impl ToolTab for TerminalTab {
    fn kind(&self) -> &'static str {
        "terminal"
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn title(&self) -> SharedString {
        manox_i18n::t("chrome-tab-terminal").into()
    }

    fn icon(&self, _cx: &App) -> AnyElement {
        icon(icons::TERMINAL, 15.).into_any_element()
    }

    fn open(&self, _window: &mut Window, cx: &mut App, store: &mut TabStore) {
        let cwd = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| ".".into());
        match spawn_standalone_terminal(&cwd, cx) {
            Ok(view) => store.put(&self.id, view),
            Err(e) => store.set_error(
                &self.id,
                manox_i18n::t_str("chrome-spawn-failed", &[("prog", "$SHELL"), ("err", &e)]),
            ),
        }
    }

    fn render(&self, _window: &mut Window, _cx: &App, store: &TabStore) -> AnyElement {
        use gpui::{IntoElement, ParentElement, Styled, div, px};
        match store.get::<terminal_ui::TerminalView>(&self.id) {
            Some(view) => div()
                .w_full()
                .h_full()
                .flex()
                .p(px(4.))
                .child(view)
                .into_any_element(),
            None => div().w_full().h_full().into_any_element(),
        }
    }
}

/// A standalone PTY spawn (the chrome-example path): the tab owns its
/// terminal fully; closing the tab drops the view and tears the process
/// tree down with it.
fn spawn_standalone_terminal(
    cwd: &std::path::Path,
    cx: &mut App,
) -> Result<Entity<terminal_ui::TerminalView>, String> {
    use gpui::AppContext as _;
    let source = manox_terminal::pty::default_source(cwd, 80, 24).map_err(|e| e.to_string())?;
    let id = format!("chrome-assembly:$SHELL:{}", next_instance_id("pty"));
    let handle = manox_terminal::Terminal::spawn(id, cwd.to_path_buf(), 80, 24, source)
        .map_err(|e| e.to_string())?;
    let proxy = cx.new(|cx| terminal_ui::terminal_proxy::TerminalProxy::new(handle, cx));
    Ok(terminal_ui::TerminalView::new(proxy, cx))
}

/// The bottom dock's terminal surface for the chrome assembly.
pub struct TerminalPanelSurface;

impl manox_agent_chrome_ui::PanelSurface for TerminalPanelSurface {
    fn title(&self) -> SharedString {
        manox_i18n::t("chrome-tab-terminal").into()
    }

    fn icon(&self) -> Icon {
        icons::TERMINAL
    }

    fn open(&self, _window: &mut Window, cx: &mut App) -> Result<gpui::AnyView, String> {
        let view = spawn_standalone_terminal(&std::env::temp_dir(), cx)?;
        Ok(gpui::AnyView::from(view))
    }
}
