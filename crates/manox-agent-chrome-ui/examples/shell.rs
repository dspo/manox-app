//! A full chrome-shell assembly on real data — the visual-acceptance target
//! for `manox-agent-chrome-ui` (the `ai-elements` gallery pattern).
//!
//! What the host supplies here:
//! - session rows from the real `~/.manox` thread store (project grouping,
//!   pinned/unread/errored state, recency order, live refresh pump);
//! - tool-tab kinds for the right pane (an integrated browser, the user's
//!   shell, and three CLI-agent terminals);
//! - a PTY terminal as the bottom-dock surface;
//! - a placeholder main surface (the chat column lands with
//!   manox-agent-chat-ui in a later stage).
//!
//! Run: `cargo run -p manox-agent-chrome-ui --example shell`

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{
    AnyView, AppContext as _, ClickEvent, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px, svg,
};
use gpui_component::Root;
use manox_agent_chrome_ui::right_pane::TabStore;
use manox_agent_chrome_ui::session_list::SessionStatus;
use manox_agent_chrome_ui::shell::SessionRow;
use manox_agent_chrome_ui::theme::{self, Icon, icon};
use manox_agent_chrome_ui::{
    CustomizationRow, FixedRow, HostHooks, MainSurface, PanelSurface, Shell, ShellConfig, ToolTab,
    ToolTabFactory, icons, register_fonts, window_options,
};

fn main() {
    // Real data source init (tokio runtime / providers / ThreadStore) — the
    // same embedding path as the freya original.
    manox_agent::init();

    gpui_platform::application().with_assets(Assets).run(|cx| {
        configure(cx);

        cx.open_window(window_options(cx), |window, cx| {
            window.activate_window();
            let chat: AnyView = cx.new(|_| PlaceholderChat).into();
            let shell = cx.new(|cx| Shell::new(shell_config(chat), window, cx));
            start_pump(shell.clone(), cx);
            cx.new(|cx| Root::new(shell, window, cx))
        })
        .expect("failed to open window");
    });
}

/// Component-layer / runtime / font / theme init. Order matters: the
/// terminal's PTY pump needs the shared runtime handle before
/// `manox_terminal::init` builds its store.
fn configure(cx: &mut gpui::App) {
    gpui_component::init(cx);
    manox_i18n::init();
    manox_terminal::runtime::set_runtime(manox_agent::runtime::handle().clone());
    manox_terminal::init();
    terminal_ui::init(cx);
    register_fonts(cx);

    // The global component-theme choice belongs to the HOST (the shipped app
    // decides its own); this example pins the full 2026-Light sheet so the
    // component pieces (menus, popovers, inputs) match the chrome — painting
    // only background/foreground leaves menu text on washed-out default
    // slots.
    install_light_theme(cx);

    cx.bind_keys([gpui::KeyBinding::new(
        "cmd-n",
        manox_agent_chrome_ui::shell::NewSession,
        None,
    )]);
}

/// Everything the shell needs from this host. `main_view` is the
/// placeholder chat column's pre-created view.
fn shell_config(main_view: AnyView) -> ShellConfig {
    ShellConfig {
        main: Arc::new(PlaceholderMain { view: main_view }),
        tool_kinds: vec![
            Arc::new(BrowserKind),
            Arc::new(TerminalKind::shell()),
            Arc::new(TerminalKind::agent(
                "claude-code",
                "Claude Code",
                "icons/claude.svg",
                "claude",
            )),
            Arc::new(TerminalKind::agent(
                "codex",
                "Codex",
                "icons/codex.svg",
                "codex",
            )),
            Arc::new(TerminalKind::agent(
                "copilot",
                "GitHub Copilot",
                "icons/githubcopilot.svg",
                "copilot",
            )),
        ],
        panel_surface: Some(Arc::new(TerminalPanel)),
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
        // Count badges are visual fixtures of the replica, not live data.
        customizations: vec![
            CustomizationRow {
                icon: icons::HOME,
                label: manox_i18n::t("chrome-sidebar-overview"),
                count: None,
            },
            CustomizationRow {
                icon: icons::EXTENSIONS,
                label: manox_i18n::t("chrome-sidebar-plugins"),
                count: None,
            },
            CustomizationRow {
                icon: icons::SETTINGS_GEAR,
                label: manox_i18n::t("chrome-sidebar-mcp"),
                count: Some(1),
            },
            CustomizationRow {
                icon: icons::WAND,
                label: manox_i18n::t("chrome-sidebar-skills"),
                count: Some(13),
            },
        ],
        hooks: HostHooks {
            on_pin: Some(Box::new(|id, _w, _cx| store_toggle_pin(id))),
            on_archive: Some(Box::new(|id, _w, _cx| store_set_archived(id))),
            on_new_session: None,
            on_select: None,
        },
    }
}

// ── session source (real ~/.manox threads) ────────────────────────────────

/// Push thread snapshots into the shell. The one active scan happens here
/// (off the first frame); afterwards the store's change events drive
/// snapshot-only reads — calling `refresh_thread_list` from an event
/// callback would loop (refresh is an async scan that re-emits the event).
fn start_pump(shell: Entity<Shell>, cx: &mut gpui::App) {
    manox_agent::thread_store::refresh_thread_list();
    let rx = manox_agent::thread_store::global().subscribe();
    cx.spawn(async move |cx| {
        let mut boot_selected = false;
        async fn push(
            shell: &Entity<Shell>,
            boot_selected: &mut bool,
            cx: &mut gpui::AsyncApp,
        ) -> anyhow::Result<()> {
            let rows = cx.background_spawn(async { load_rows() }).await;
            shell.update(cx, |shell, cx| {
                // Auto-select the first thread once the initial scan lands.
                if !*boot_selected && !rows.is_empty() {
                    *boot_selected = true;
                    shell.active = Some(rows[0].id.clone());
                }
                shell.set_sessions(rows);
                cx.notify();
            });
            anyhow::Ok(())
        }
        push(&shell, &mut boot_selected, cx).await?;
        while rx.recv().await.is_ok() {
            if push(&shell, &mut boot_selected, cx).await.is_err() {
                break;
            }
        }
        anyhow::Ok(())
    })
    .detach();
}

/// ThreadStore summaries → chrome session rows.
fn load_rows() -> Vec<SessionRow> {
    let store = manox_agent::thread_store::global();
    let mut rows: Vec<SessionRow> = store
        .read(|st| {
            st.summaries()
                .iter()
                .filter(|t| !t.archived && t.depth == 0 && t.superseded_by.is_none())
                .map(|t| (t.clone(), st.is_running(&t.id)))
                .collect::<Vec<_>>()
        })
        .into_iter()
        .map(|(t, running)| SessionRow {
            id: t.id.clone(),
            title: t
                .title_override
                .clone()
                .or_else(|| t.title.clone())
                .unwrap_or_else(|| t.summary.clone()),
            workspace: project_label(&t.project),
            time: relative_time(t.updated_at),
            status: if t.errored {
                SessionStatus::NeedsInput
            } else if running {
                SessionStatus::Running
            } else {
                SessionStatus::Completed
            },
            updated_at: t.updated_at,
            pinned: t.pinned,
            unread: t.has_unread,
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    rows
}

/// Project display name: the path's last segment (/x/y/manox → manox); an
/// empty project (quick chats) groups under "Chats".
fn project_label(path: &str) -> String {
    if path.is_empty() || path == "." {
        return manox_i18n::t("chrome-sidebar-chats");
    }
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// unix seconds → relative time (the sidebar's time column).
fn relative_time(unix_secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let delta = (now - unix_secs).max(0);
    match delta {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", delta / 60),
        3600..=86_399 => format!("{}h", delta / 3600),
        86_400..=1_209_599 => format!("{}d", delta / 86_400),
        _ => format!("{}w", delta / 604_800),
    }
}

fn store_toggle_pin(id: &str) {
    let loaded = manox_agent::thread_store::global().with_mut(|st| st.load_thread(id));
    if let Ok(Some(handle)) = loaded {
        let was = handle.read(|t| t.is_pinned());
        handle.with_mut(|t| t.set_pinned(!was));
    }
}

fn store_set_archived(id: &str) {
    let loaded = manox_agent::thread_store::global().with_mut(|st| st.load_thread(id));
    if let Ok(Some(handle)) = loaded {
        handle.with_mut(|t| t.set_archived(true));
    }
}

// ── right-pane tool kinds ─────────────────────────────────────────────────

static INSTANCE: AtomicU64 = AtomicU64::new(0);

fn next_instance_id(kind: &str) -> String {
    format!("{kind}-{}", INSTANCE.fetch_add(1, Ordering::Relaxed))
}

/// Glyph: brand SVG asset first, codicon fallback.
fn icon_el(svg_path: Option<&'static str>, codicon: Icon, size: f32) -> gpui::AnyElement {
    match svg_path {
        Some(path) => svg()
            .path(path)
            .size(px(size))
            .flex_shrink_0()
            .into_any_element(),
        None => icon(codicon, size).into_any_element(),
    }
}

struct TerminalKind {
    kind: &'static str,
    /// Agent display name; None = the user's shell.
    display: Option<&'static str>,
    icon_svg: Option<&'static str>,
    binary: Option<&'static str>,
}

impl TerminalKind {
    fn shell() -> Self {
        Self {
            kind: "terminal",
            display: None,
            icon_svg: None,
            binary: None,
        }
    }

    const fn agent(
        kind: &'static str,
        display: &'static str,
        svg: &'static str,
        binary: &'static str,
    ) -> Self {
        Self {
            kind,
            display: Some(display),
            icon_svg: Some(svg),
            binary: Some(binary),
        }
    }
}

impl ToolTabFactory for TerminalKind {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn create(&self) -> Arc<dyn ToolTab> {
        Arc::new(TerminalTab {
            id: next_instance_id(self.kind),
            kind: self.kind,
            title: self
                .display
                .map(Into::into)
                .unwrap_or_else(|| manox_i18n::t("chrome-tab-terminal").into()),
            icon_svg: self.icon_svg,
            binary: self.binary,
        })
    }

    fn quick_action(&self) -> Option<SharedString> {
        match self.display {
            Some(name) => Some(manox_i18n::t_str("chrome-quick-agent", &[("agent", name)]).into()),
            None => Some(manox_i18n::t("chrome-quick-terminal").into()),
        }
    }

    fn icon(&self, _cx: &gpui::App) -> gpui::AnyElement {
        icon_el(self.icon_svg, icons::TERMINAL, 15.)
    }
}

struct TerminalTab {
    id: String,
    kind: &'static str,
    title: SharedString,
    icon_svg: Option<&'static str>,
    binary: Option<&'static str>,
}

impl ToolTab for TerminalTab {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn icon(&self, _cx: &gpui::App) -> gpui::AnyElement {
        icon_el(self.icon_svg, icons::TERMINAL, 15.)
    }

    fn open(&self, _window: &mut Window, cx: &mut gpui::App, store: &mut TabStore) {
        let cwd = home_cwd();
        match spawn_terminal(self.binary, &cwd, cx) {
            Ok(view) => store.put(&self.id, view),
            Err(e) => store.set_error(
                &self.id,
                manox_i18n::t_str(
                    "chrome-spawn-failed",
                    &[("prog", prog_name(self.binary)), ("err", &e)],
                ),
            ),
        }
    }

    fn render(&self, _window: &mut Window, _cx: &gpui::App, store: &TabStore) -> gpui::AnyElement {
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

fn prog_name(binary: Option<&str>) -> &str {
    binary.unwrap_or("$SHELL")
}

/// Agent/terminal working directory.
fn home_cwd() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| ".".into())
}

/// Launch a PTY + TerminalView: `None` = the user's shell; `Some(prog)` =
/// an explicit program. Shared by the right-pane tabs and the bottom dock.
fn spawn_terminal(
    binary: Option<&str>,
    cwd: &std::path::Path,
    cx: &mut gpui::App,
) -> Result<Entity<terminal_ui::TerminalView>, String> {
    let prog = binary.unwrap_or("$SHELL");
    let source: Box<dyn manox_terminal::pty_source::PtySource> = match binary {
        None => manox_terminal::pty::default_source(cwd, 80, 24),
        Some(prog) => manox_terminal::pty::open(cwd, 80, 24, Some(prog), &[])
            .map(|h| Box::new(h) as Box<dyn manox_terminal::pty_source::PtySource>),
    }
    .map_err(|e| e.to_string())?;
    let id = format!("chrome-shell:{prog}:{}", next_instance_id("pty"));
    let handle = manox_terminal::Terminal::spawn(id, cwd.to_path_buf(), 80, 24, source)
        .map_err(|e| e.to_string())?;
    let proxy = cx.new(|cx| terminal_ui::terminal_proxy::TerminalProxy::new(handle, cx));
    Ok(terminal_ui::TerminalView::new(proxy, cx))
}

struct BrowserKind;

impl ToolTabFactory for BrowserKind {
    fn kind(&self) -> &'static str {
        "browser"
    }

    fn create(&self) -> Arc<dyn ToolTab> {
        Arc::new(BrowserTab {
            id: next_instance_id("browser"),
        })
    }

    fn quick_action(&self) -> Option<SharedString> {
        Some(manox_i18n::t("chrome-quick-browser").into())
    }

    fn icon(&self, _cx: &gpui::App) -> gpui::AnyElement {
        icon_el(None, icons::GLOBE, 15.)
    }
}

/// One browser tab instance: a wry webview + an address bar (enter / the Go
/// button navigates); the view entity holds its own subscriptions and
/// `load_url` is the single navigation entry.
struct BrowserTab {
    id: String,
}

impl ToolTab for BrowserTab {
    fn kind(&self) -> &'static str {
        "browser"
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn title(&self) -> SharedString {
        manox_i18n::t("chrome-tab-browser").into()
    }

    fn icon(&self, _cx: &gpui::App) -> gpui::AnyElement {
        icon_el(None, icons::GLOBE, 15.)
    }

    fn open(&self, window: &mut Window, cx: &mut gpui::App, store: &mut TabStore) {
        let view = cx.new(|cx| BrowserTabView::new("https://example.com", window, cx));
        store.put(&self.id, view);
    }

    fn render(&self, _window: &mut Window, _cx: &gpui::App, store: &TabStore) -> gpui::AnyElement {
        match store.get::<BrowserTabView>(&self.id) {
            Some(view) => view.into_any_element(),
            None => div().w_full().h_full().into_any_element(),
        }
    }

    fn on_active(&self, visible: bool, cx: &mut gpui::App, store: &TabStore) {
        // The wry subview is an OS-level child window: without hide it
        // floats above every other tab.
        if let Some(view) = store.get::<BrowserTabView>(&self.id) {
            let webview = view.read(cx).webview.clone();
            webview.update(cx, |w, _| {
                if visible {
                    w.show();
                } else {
                    w.hide();
                }
            });
        }
    }
}

struct BrowserTabView {
    webview: Entity<manox_webview::webview::WebView>,
    address: Entity<gpui_component::input::InputState>,
    #[allow(dead_code)] // kept for future page-title mirroring
    url: String,
}

impl BrowserTabView {
    fn new(url: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let builder = manox_webview::Builder::new()
            .with_webview_id("chrome-shell-browser")
            .trust_mode(manox_webview::TrustMode::Untrusted);
        let wry = builder
            .apply(|b| b.with_url(url))
            .build_as_child(window)
            .expect("manox-webview: build_as_child failed");
        let webview = cx.new(|cx| manox_webview::webview::WebView::new(wry, window, cx));
        let address =
            cx.new(|cx| gpui_component::input::InputState::new(window, cx).placeholder("https://"));
        address.update(cx, |s, cx| s.set_value(url, window, cx));
        // Address-bar enter → navigate (completing the scheme).
        cx.subscribe_in(
            &address,
            window,
            |this, _, ev: &gpui_component::input::InputEvent, window, cx| {
                if let gpui_component::input::InputEvent::PressEnter { shift: false, .. } = ev {
                    let url = this.address.read(cx).value().to_string();
                    if !url.is_empty() {
                        this.load_url(&normalize_url(&url), window, cx);
                    }
                }
            },
        )
        .detach();
        Self {
            webview,
            address,
            url: url.to_string(),
        }
    }

    fn load_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.url = url.to_string();
        self.webview.update(cx, |w, _| w.load_url(url));
        self.address
            .update(cx, |s, cx| s.set_value(url, window, cx));
        cx.notify();
    }
}

impl Render for BrowserTabView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let go = cx.listener(|this, _: &ClickEvent, window, cx| {
            let url = this.address.read(cx).value().to_string();
            if !url.is_empty() {
                let url = normalize_url(&url);
                this.load_url(&url, window, cx);
            }
        });
        let address = self.address.clone();
        let webview = self.webview.clone();
        div()
            .id("browser-tab")
            .w_full()
            .h_full()
            .p(px(8.))
            .gap(px(6.))
            .flex()
            .flex_col()
            // Address bar: Input + Go.
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(30.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(theme::BORDER)
                            .bg(theme::CARD_BG)
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .child(
                                gpui_component::input::Input::new(&address)
                                    .appearance(false)
                                    .h_full()
                                    .w_full()
                                    .text_size(px(13.)),
                            ),
                    )
                    .child(
                        div()
                            .id("browser-go")
                            .on_click(move |e, w, cx| go(e, w, cx))
                            .py(px(5.))
                            .px(px(10.))
                            .rounded(px(5.))
                            .bg(theme::ACCENT)
                            .text_color(theme::BADGE_BLUE_FG)
                            .text_size(px(12.))
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .hover(|style| style.bg(theme::ACCENT_HOVER))
                            .child(manox_i18n::t("chrome-quick-browser")),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .rounded(px(6.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme::BORDER)
                    .child(webview),
            )
    }
}

/// Complete the URL scheme.
fn normalize_url(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        return "https://example.com".into();
    }
    if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        format!("https://{t}")
    }
}

// ── bottom dock: a PTY terminal surface ───────────────────────────────────

struct TerminalPanel;

impl PanelSurface for TerminalPanel {
    fn title(&self) -> SharedString {
        manox_i18n::t("chrome-tab-terminal").into()
    }

    fn icon(&self) -> Icon {
        icons::TERMINAL
    }

    fn open(&self, _window: &mut Window, cx: &mut gpui::App) -> Result<AnyView, String> {
        let view = spawn_terminal(None, &std::env::temp_dir(), cx)?;
        Ok(AnyView::from(view))
    }
}

// ── main surface placeholder ──────────────────────────────────────────────

/// The chat column's seat until manox-agent-chat-ui lands.
struct PlaceholderChat;

impl Render for PlaceholderChat {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::FG_FAINT)
            .text_size(px(13.))
            .child("manox-agent-chat-ui mounts here (Phase 2)")
    }
}

struct PlaceholderMain {
    view: AnyView,
}

impl MainSurface for PlaceholderMain {
    fn view(&self) -> AnyView {
        self.view.clone()
    }

    fn title(&self, _cx: &gpui::App) -> SharedString {
        "Manox".into()
    }
}

// ── global component theme (host-owned choice) ───────────────────────────

/// Pin the gpui-component global theme to the full 2026-Light sheet — every
/// slot, not just background/foreground, so component text (menus, popovers,
/// inputs) never lands on washed-out default slots.
fn install_light_theme(cx: &mut gpui::App) {
    use gpui::{Hsla, Rgba, rgb};

    let hsla = |c: Rgba| Hsla::from(c);
    let t = gpui_component::Theme::global_mut(cx);
    t.font_family = theme::FONT_UI.into();
    t.mono_font_family = theme::FONT_MONO.into();
    t.font_size = px(13.);
    let c = &mut t.colors;
    c.background = hsla(theme::SHELL_BG);
    c.foreground = hsla(theme::FG);
    c.title_bar = hsla(theme::SHELL_BG);
    c.title_bar_border = hsla(theme::BORDER);
    c.sidebar = hsla(theme::SHELL_BG);
    c.sidebar_border = hsla(theme::BORDER);
    c.sidebar_foreground = hsla(theme::FG);
    c.popover = hsla(theme::CARD_BG);
    c.popover_foreground = hsla(theme::FG);
    c.muted = hsla(theme::TABBAR_BG);
    c.muted_foreground = hsla(theme::FG_DIM);
    c.accent = hsla(theme::ACCENT);
    c.accent_foreground = gpui::white();
    c.border = hsla(theme::BORDER);
    c.danger = hsla(theme::ERR_RED);
    c.danger_foreground = gpui::white();
    c.warning = hsla(theme::WARN_ORANGE);
    c.warning_foreground = gpui::white();
    c.success = hsla(theme::OK_GREEN);
    c.success_foreground = gpui::white();
    // The freya ColorsSheet surface family (button hover/active fills).
    c.secondary = rgb(0xC8E1F9).into();
    c.secondary_hover = rgb(0xE9E9EC).into();
    c.secondary_active = rgb(0xE0E0E3).into();
    c.secondary_foreground = hsla(theme::FG);
    c.list_hover = rgb(0x00000014).into();
    c.list_active = rgb(0x00000025).into();
}

// ── assets ────────────────────────────────────────────────────────────────

/// SVG asset layer: the brand icons embedded via rust-embed; `gpui::svg()`
/// resolves through this source.
use gpui::{AssetSource, Result as AssetResult};

#[derive(rust_embed::RustEmbed)]
#[folder = "examples/assets"]
#[include = "icons/**/*.svg"]
struct EmbeddedAssets;

struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> AssetResult<Option<Cow<'static, [u8]>>> {
        Ok(EmbeddedAssets::get(path).map(|f| f.data))
    }

    fn list(&self, path: &str) -> AssetResult<Vec<SharedString>> {
        let prefix = format!("{}/", path.trim_matches('/'));
        Ok(EmbeddedAssets::iter()
            .filter(|p| p.starts_with(&prefix))
            .map(|p| SharedString::from(p.to_string()))
            .collect())
    }
}
