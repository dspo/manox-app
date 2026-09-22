//! ToolTab assembly adapters (PLAN-CHROME-CHAT-SPLIT Phase 4): wrap the
//! app's real surfaces as chrome-shell right-pane tabs.
//!
//! Kinds shipped here:
//! - integrated terminal ($SHELL, standalone PTY);
//! - CLI agent terminals (claude / codex / copilot via the cx
//!   `AgentBuilder`): the tab opens on a MODEL PICKER (the legacy `+`
//!   menu's provider→model cascade reborn) and hands over to the TUI;
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

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, SharedString, Window, px,
};
use manox_agent_chrome_ui::right_pane::{TabStore, ToolTab, ToolTabFactory};
use manox_agent_chrome_ui::theme::{Icon, icon, icons};
use manox_ext_agents::cx_session::CxSessionSource;

static INSTANCE: AtomicU64 = AtomicU64::new(0);

fn next_instance_id(kind: &str) -> String {
    format!("{kind}-{}", INSTANCE.fetch_add(1, Ordering::Relaxed))
}

/// The foreground thread's working directory (its project path — the wire
/// row's `project` column, the same source the sidebar groups by), falling
/// back to the store's cwd, then home.
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

/// The factory carries the multiplexer (the wire model rows — the picker's
/// source) so created tabs can list launch configs at open time.
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
        brand_icon(self.svg)
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
        brand_icon(self.svg)
    }

    fn open(&self, _window: &mut Window, cx: &mut App, store: &mut TabStore) {
        // The tab opens on the MODEL PICKER (the legacy `+` menu's cascade
        // reborn); picking one spawns the agent under that endpoint and the
        // picker renders the TUI from then on — one entity for the tab's
        // whole lifetime, so close-tab teardown stays a single drop.
        let picker = cx.new(|_| AgentPicker {
            agent_id: self.agent_id,
            display: self.display,
            mux: self.mux.clone(),
            launched: None,
            error: None,
        });
        store.put(&self.id, picker);
    }

    fn render(&self, _window: &mut Window, _cx: &App, store: &TabStore) -> AnyElement {
        match store.get::<AgentPicker>(&self.id) {
            Some(picker) => picker.into_any_element(),
            None => div_missing().into_any_element(),
        }
    }
}

fn div_missing() -> gpui::Div {
    use gpui::Styled as _;
    gpui::div().w_full().h_full()
}

/// The full cx launch path: registry-pinned agent + the picked endpoint →
/// `AgentBuilder` (PTY relay) → `Terminal` → `TerminalView`. Dropping the
/// view tears the child tree down.
fn spawn_agent_terminal(
    agent_id: &str,
    cwd: &std::path::Path,
    provider: &str,
    model: &str,
    wire: Option<String>,
    cx: &mut App,
) -> Result<Entity<terminal_ui::TerminalView>, String> {
    let agent = match agent_id {
        "claude" => manox_ext_agents::Agent::Claude,
        "codex" => manox_ext_agents::Agent::Codex,
        "copilot" => manox_ext_agents::Agent::Copilot,
        other => return Err(format!("unknown agent: {other}")),
    };
    let mut builder = manox_ext_agents::AgentBuilder::new()
        .agent(agent)
        .pty(true)
        .provider(provider.to_string())
        .model(model.to_string())
        .cwd(cwd.to_path_buf());
    if let Some(w) = wire {
        builder = builder.wire_api(w);
    }
    let handle = Arc::new(builder.spawn().map_err(|e| e.to_string())?);
    let id = format!("chrome-assembly:{agent_id}:{}", next_instance_id("agent"));
    let source = CxSessionSource::new(Arc::clone(&handle));
    let terminal = manox_terminal::Terminal::spawn(id, cwd.to_path_buf(), 80, 24, Box::new(source))
        .map_err(|e| e.to_string())?;
    let proxy = cx.new(|cx| terminal_ui::terminal_proxy::TerminalProxy::new(terminal, cx));
    Ok(terminal_ui::TerminalView::new(proxy, cx))
}

// ── the agent model picker ────────────────────────────────────────────────

/// One pickable endpoint, straight from the shared cascade projection
/// (`cascade_provider_groups` — the sidebar `+` menu's own rule).
struct PickRow {
    provider: String,
    config_id: String,
    display: String,
    wire: Option<String>,
}

use crate::views::model_cascade::{CascadeEntry, cascade_provider_groups};

impl From<(String, CascadeEntry)> for PickRow {
    fn from((provider, entry): (String, CascadeEntry)) -> Self {
        Self {
            provider,
            config_id: entry.config_id,
            display: entry.display,
            wire: entry.wire,
        }
    }
}

/// The model-picker body an agent tab opens on: the multiplexer's wire
/// model rows this agent may use, grouped provider→model (the sidebar
/// cascade's rule — a missing `agents` column means non-cx registration and
/// stays visible). Picking one spawns the agent under that endpoint and
/// this entity renders the terminal from then on; a spawn failure stays on
/// the picker with the error surfaced.
struct AgentPicker {
    agent_id: &'static str,
    display: &'static str,
    mux: Entity<crate::multiplexer::SessionMultiplexer>,
    launched: Option<Entity<terminal_ui::TerminalView>>,
    error: Option<String>,
}

impl AgentPicker {
    fn groups(&self, cx: &App) -> Vec<(String, Vec<PickRow>)> {
        let models = self.mux.read(cx).models().to_vec();
        cascade_provider_groups(self.agent_id, &models)
            .into_iter()
            .map(|(provider, entries)| {
                (
                    provider.clone(),
                    entries
                        .into_iter()
                        .map(|e| PickRow::from((provider.clone(), e)))
                        .collect(),
                )
            })
            .collect()
    }
}

impl gpui::Render for AgentPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui::{InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, div};
        use gpui_component::{ActiveTheme as _, h_flex, v_flex};
        let theme = cx.theme().clone();

        // Launched: the terminal IS the tab body from here on.
        if let Some(view) = self.launched.clone() {
            return v_flex()
                .size_full()
                .p(px(4.))
                .child(view)
                .into_any_element();
        }

        let heading = manox_i18n::t_str("chrome-agent-pick-model", &[("agent", self.display)]);
        let groups = self.groups(cx);
        let mut list = v_flex().w_full().flex_1().min_h_0().gap_1();
        if let Some(err) = self.error.clone() {
            list = list.child(
                div()
                    .py_2()
                    .text_color(theme.danger)
                    .child(manox_i18n::t_str(
                        "chrome-spawn-failed",
                        &[("prog", self.display), ("err", &err)],
                    )),
            );
        }
        if groups.is_empty() {
            list = list.child(
                div()
                    .py_4()
                    .text_color(theme.muted_foreground)
                    .child(manox_i18n::t("external-wizard-no-model")),
            );
        }
        for (provider, rows) in groups {
            let mut group = v_flex().gap_1();
            group = group.child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme.muted_foreground)
                    .child(provider.clone()),
            );
            for row in rows {
                let agent_id = self.agent_id;
                let wire = row.wire.clone();
                let display_name = row.display.clone();
                let elem_id =
                    SharedString::from(format!("pick-{}-{}", row.provider, row.config_id));
                let on_pick = cx.listener(
                    move |this: &mut AgentPicker,
                          _: &gpui::ClickEvent,
                          _w: &mut Window,
                          cx: &mut Context<AgentPicker>| {
                        let cwd = thread_cwd_or_home();
                        match spawn_agent_terminal(
                            agent_id,
                            &cwd,
                            &row.provider,
                            &row.config_id,
                            wire.clone(),
                            cx,
                        ) {
                            Ok(view) => this.launched = Some(view),
                            Err(e) => this.error = Some(e),
                        }
                        let _ = display_name;
                        cx.notify();
                    },
                );
                group = group.child(
                    div()
                        .id(elem_id)
                        .on_click(move |e, w, cx| on_pick(e, w, cx))
                        .py(px(4.))
                        .px(px(8.))
                        .rounded(px(4.))
                        .hover(|s| s.bg(theme.list_hover))
                        .child(h_flex().items_center().child(row.display.clone())),
                );
            }
            list = list.child(group);
        }
        v_flex()
            .id("agent-picker")
            .size_full()
            .p(px(8.))
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(heading),
            )
            .child(list)
            .into_any_element()
    }
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
    let source: Box<dyn manox_terminal::pty_source::PtySource> =
        manox_terminal::pty::default_source(cwd, 80, 24).map_err(|e| e.to_string())?;
    let id = format!("chrome-assembly:$SHELL:{}", next_instance_id("pty"));
    let handle = manox_terminal::Terminal::spawn(id, cwd.to_path_buf(), 80, 24, source)
        .map_err(|e| e.to_string())?;
    let proxy = cx.new(|cx| terminal_ui::terminal_proxy::TerminalProxy::new(handle, cx));
    Ok(terminal_ui::TerminalView::new(proxy, cx))
}

/// Brand glyph: the SVG asset rides the app's asset source
/// (`ExtrasAssetSource`), rendered the same way the legacy sidebar renders
/// its brand marks — a gpui-component `Icon` with the custom path, sized
/// `.small()`, colored by the surrounding text color.
fn brand_icon(svg_path: &'static str) -> AnyElement {
    use gpui_component::Sizable as _;
    gpui_component::Icon::default()
        .path(svg_path)
        .small()
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
