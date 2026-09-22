//! ToolTab assembly adapters (PLAN-CHROME-CHAT-SPLIT Phase 4): wrap the
//! app's real surfaces as chrome-shell right-pane tabs.
//!
//! Kinds shipped here:
//! - integrated terminal ($SHELL, standalone PTY);
//! - CLI agent terminals (claude / codex / copilot via the cx
//!   `AgentBuilder` — the same launch path the legacy shell's `+` menu
//!   uses, provider/model resolved from the agent registry);
//! - the editor (markdown write/preview, a `gpui_component` editor).
//!
//! The browser tab awaits its host's decoupling
//! (`WorkspaceBrowserHost::install` binds to a `Workspace`), tracked as the
//! remaining tranche-4 item.
//!
//! All tabs inherit per-thread session state for free: the chrome
//! `RightPaneSession` stash/restore carries the whole tab set across thread
//! switches.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{AnyElement, App, AppContext as _, Entity, IntoElement, SharedString, Window};
use manox_agent_chrome_ui::right_pane::{TabStore, ToolTab, ToolTabFactory};
use manox_agent_chrome_ui::theme::{Icon, icon, icons};
use manox_ext_agents::cx_session::CxSessionSource;

static INSTANCE: AtomicU64 = AtomicU64::new(0);

fn next_instance_id(kind: &str) -> String {
    format!("{kind}-{}", INSTANCE.fetch_add(1, Ordering::Relaxed))
}

fn thread_cwd_or_home() -> std::path::PathBuf {
    crate::chrome_assembly::foreground_cwd().unwrap_or_else(|| {
        std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| ".".into())
    })
}

// ── integrated terminal ────────────────────────────────────────────────────

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
        match spawn_standalone_terminal(&thread_cwd_or_home(), cx) {
            Ok(view) => store.put(&self.id, view),
            Err(e) => store.set_error(
                &self.id,
                manox_i18n::t_str("chrome-spawn-failed", &[("prog", "$SHELL"), ("err", &e)]),
            ),
        }
    }

    fn render(&self, _window: &mut Window, _cx: &App, store: &TabStore) -> AnyElement {
        use gpui::{ParentElement, Styled, div, px};
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

// ── CLI agent terminals ────────────────────────────────────────────────────

/// The factory carries the multiplexer (the wire model rows — the
/// provider/model resolution source) so created tabs can resolve their
/// launch config at open time.
pub struct AgentTool {
    agent_id: &'static str,
    display: &'static str,
    svg: &'static str,
    mux: Entity<crate::multiplexer::SessionMultiplexer>,
}

impl AgentTool {
    pub fn new(
        agent_id: &'static str,
        display: &'static str,
        svg: &'static str,
        mux: Entity<crate::multiplexer::SessionMultiplexer>,
    ) -> Self {
        Self {
            agent_id,
            display,
            svg,
            mux,
        }
    }
}

impl ToolTabFactory for AgentTool {
    fn kind(&self) -> &'static str {
        self.agent_id
    }

    fn create(&self) -> Arc<dyn ToolTab> {
        Arc::new(AgentTab {
            id: next_instance_id(self.agent_id),
            agent_id: self.agent_id,
            display: self.display,
            svg: self.svg,
            mux: self.mux.clone(),
        })
    }

    fn quick_action(&self) -> Option<SharedString> {
        Some(manox_i18n::t_str("chrome-quick-agent", &[("agent", self.display)]).into())
    }

    fn icon(&self, _cx: &App) -> AnyElement {
        brand_icon(self.svg, icons::TERMINAL)
    }
}

struct AgentTab {
    id: String,
    agent_id: &'static str,
    display: &'static str,
    svg: &'static str,
    mux: Entity<crate::multiplexer::SessionMultiplexer>,
}

impl ToolTab for AgentTab {
    fn kind(&self) -> &'static str {
        self.agent_id
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn title(&self) -> SharedString {
        self.display.into()
    }

    fn icon(&self, _cx: &App) -> AnyElement {
        brand_icon(self.svg, icons::TERMINAL)
    }

    fn open(&self, _window: &mut Window, cx: &mut App, store: &mut TabStore) {
        match spawn_agent_terminal(self.agent_id, &thread_cwd_or_home(), &self.mux, cx) {
            Ok(view) => store.put(&self.id, view),
            Err(e) => store.set_error(
                &self.id,
                manox_i18n::t_str(
                    "chrome-spawn-failed",
                    &[("prog", self.display), ("err", &e)],
                ),
            ),
        }
    }

    fn render(&self, _window: &mut Window, _cx: &App, store: &TabStore) -> AnyElement {
        use gpui::{ParentElement, Styled, div, px};
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

/// The full cx launch path (the legacy `+` menu's cascade, one agent kind
/// pinned): registry-resolved agent config → `AgentBuilder` (PTY relay) →
/// `Terminal` → `TerminalView`. Dropping the view tears the child tree down.
fn spawn_agent_terminal(
    agent_id: &str,
    cwd: &std::path::Path,
    mux: &Entity<crate::multiplexer::SessionMultiplexer>,
    cx: &mut App,
) -> Result<Entity<terminal_ui::TerminalView>, String> {
    use gpui::AppContext as _;
    let agent = match agent_id {
        "claude" => manox_ext_agents::Agent::Claude,
        "codex" => manox_ext_agents::Agent::Codex,
        "copilot" => manox_ext_agents::Agent::Copilot,
        other => return Err(format!("unknown agent: {other}")),
    };
    let mut builder = manox_ext_agents::AgentBuilder::new()
        .agent(agent)
        .pty(true)
        .cwd(cwd.to_path_buf());
    // Provider/model: the registry's launch configuration for this agent
    // (the same resolution the sidebar cascade performs); absent config
    // falls back to the agent's built-in defaults.
    if let Some((provider, model)) = registry_agent_model(mux, cx, agent_id) {
        builder = builder.provider(provider).model(model);
    }
    let handle = Arc::new(builder.spawn().map_err(|e| e.to_string())?);
    let id = format!("chrome-assembly:{agent_id}:{}", next_instance_id("agent"));
    let source = CxSessionSource::new(Arc::clone(&handle));
    let terminal = manox_terminal::Terminal::spawn(id, cwd.to_path_buf(), 80, 24, Box::new(source))
        .map_err(|e| e.to_string())?;
    let proxy = cx.new(|cx| terminal_ui::terminal_proxy::TerminalProxy::new(terminal, cx));
    Ok(terminal_ui::TerminalView::new(proxy, cx))
}

/// The provider/model pair for one agent id, resolved from the
/// multiplexer's wire model rows (the cascade's source): the first model
/// whose effective agent list contains the id — the sidebar cascade's
/// default pick.
fn registry_agent_model(
    mux: &Entity<crate::multiplexer::SessionMultiplexer>,
    cx: &App,
    agent_id: &str,
) -> Option<(String, String)> {
    let models = mux.read(cx).models().to_vec();
    models
        .iter()
        .find(|m| {
            m.agents
                .as_ref()
                .map(|list| list.iter().any(|a| a == agent_id))
                .unwrap_or(true)
        })
        .map(|m| {
            (
                m.provider_name
                    .clone()
                    .unwrap_or_else(|| m.provider.clone()),
                m.config_id.clone().unwrap_or_else(|| m.id.clone()),
            )
        })
}

// ── editor ────────────────────────────────────────────────────────────────

/// The markdown editor kind (write/preview), the legacy right pane's editor
/// tab reborn as a chrome tool tab. Content is per-tab-instance (a fresh
/// editor per open); the chrome session stash carries it across thread
/// switches.
pub struct EditorTool;

impl ToolTabFactory for EditorTool {
    fn kind(&self) -> &'static str {
        "editor"
    }

    fn create(&self) -> Arc<dyn ToolTab> {
        Arc::new(EditorTab {
            id: next_instance_id("editor"),
        })
    }

    fn quick_action(&self) -> Option<SharedString> {
        Some(manox_i18n::t("chrome-quick-editor").into())
    }

    fn icon(&self, _cx: &App) -> AnyElement {
        icon(icons::SYMBOL_FILE, 15.).into_any_element()
    }
}

struct EditorTab {
    id: String,
}

impl ToolTab for EditorTab {
    fn kind(&self) -> &'static str {
        "editor"
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn title(&self) -> SharedString {
        manox_i18n::t("chrome-tab-editor").into()
    }

    fn icon(&self, _cx: &App) -> AnyElement {
        icon(icons::SYMBOL_FILE, 15.).into_any_element()
    }

    fn open(&self, window: &mut Window, cx: &mut App, store: &mut TabStore) {
        let view = cx.new(|cx| {
            gpui_component::input::EditorState::new(window, cx)
                .language("markdown")
                .line_number(true)
                .folding(false)
                .soft_wrap(true)
                .submit_on_enter(false)
                .placeholder(manox_i18n::t("chrome-editor-placeholder"))
        });
        store.put(&self.id, view);
    }

    fn render(&self, _window: &mut Window, _cx: &App, store: &TabStore) -> AnyElement {
        use gpui::{ParentElement, Styled, div, px};
        match store.get::<gpui_component::input::EditorState>(&self.id) {
            Some(view) => div()
                .w_full()
                .h_full()
                .flex()
                .p(px(8.))
                .child(view)
                .into_any_element(),
            None => div().w_full().h_full().into_any_element(),
        }
    }
}

// ── shared helpers ────────────────────────────────────────────────────────

/// A standalone PTY spawn (the chrome-example path): the tab owns its
/// terminal fully; closing the tab drops the view and tears the process
/// tree down with it.
pub(crate) fn spawn_standalone_terminal(
    cwd: &std::path::Path,
    cx: &mut App,
) -> Result<Entity<terminal_ui::TerminalView>, String> {
    use gpui::AppContext as _;
    let source: Box<dyn manox_terminal::pty_source::PtySource> =
        manox_terminal::pty::default_source(cwd, 80, 24).map_err(|e| e.to_string())?;
    let id = format!("chrome-assembly:$SHELL:{}", next_instance_id("pty"));
    let handle = manox_terminal::Terminal::spawn(id, cwd.to_path_buf(), 80, 24, source)
        .map_err(|e| e.to_string())?;
    let proxy = cx.new(|cx| terminal_ui::terminal_proxy::TerminalProxy::new(handle, cx));
    Ok(terminal_ui::TerminalView::new(proxy, cx))
}

/// Brand glyph: the SVG asset rides the app's asset source; the codicon
/// fallback renders when the asset is unavailable.
fn brand_icon(svg_path: &'static str, _fallback: Icon) -> AnyElement {
    use gpui::Styled as _;
    gpui::svg()
        .path(svg_path)
        .size(gpui::px(15.))
        .flex_shrink_0()
        .into_any_element()
}

/// The bottom dock's terminal surface for the chrome assembly: each spawn
/// roots at the FOREGROUND thread's cwd (the store's working directory),
/// so the dock follows the conversation.
pub struct ThreadTerminalPanelSurface;

impl manox_agent_chrome_ui::PanelSurface for ThreadTerminalPanelSurface {
    fn title(&self) -> SharedString {
        manox_i18n::t("chrome-tab-terminal").into()
    }

    fn icon(&self) -> Icon {
        icons::TERMINAL
    }

    fn open(&self, _window: &mut Window, cx: &mut App) -> Result<gpui::AnyView, String> {
        let view = spawn_standalone_terminal(&thread_cwd_or_home(), cx)?;
        Ok(gpui::AnyView::from(view))
    }
}

/// The full registry for the chrome assembly's right pane (quick-action
/// order).
pub fn registry(
    mux: &Entity<crate::multiplexer::SessionMultiplexer>,
) -> Vec<Arc<dyn ToolTabFactory>> {
    vec![
        Arc::new(TerminalTool),
        Arc::new(AgentTool::new(
            "claude",
            "Claude Code",
            "icons/claude.svg",
            mux.clone(),
        )),
        Arc::new(AgentTool::new(
            "codex",
            "Codex",
            "icons/codex.svg",
            mux.clone(),
        )),
        Arc::new(AgentTool::new(
            "copilot",
            "GitHub Copilot",
            "icons/githubcopilot.svg",
            mux.clone(),
        )),
        Arc::new(EditorTool),
    ]
}
