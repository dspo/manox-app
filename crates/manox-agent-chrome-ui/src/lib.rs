//! manox-agent-chrome-ui — the app-window shell, ported from the
//! agents-window-gpui pixel replica of VS Code 1.139's Agents Window.
//!
//! The crate owns **layout and interaction affordances only**:
//! - the 2026-Light design tokens ([`theme`]) and base controls
//!   ([`primitives`]);
//! - the sidebar session tree ([`session_list`]) — props-driven, host-fed;
//! - the right tool pane shell ([`right_pane`]) — content injected through
//!   the [`right_pane::ToolTab`] trait, kinds registered through
//!   [`right_pane::ToolTabFactory`] (instance-level ids: one kind may be
//!   open many times);
//! - the bottom dock ([`panel`]) — content injected through
//!   [`panel::PanelSurface`] (mount-equals-launch, unmount-equals-teardown);
//! - the main-column slot ([`main_surface`]) — chat is the first intended
//!   implementation, settings the second;
//! - the root [`shell::Shell`] assembling all of the above plus the toolbar
//!   ([`titlebar`]) and resize handles ([`divider`]).
//!
//! It deliberately depends on no data source: session rows are pushed in as
//! snapshots, state-changing actions mirror to [`shell::HostHooks`], and tab
//! content is injected by the host. The `shell` example wires a full
//! assembly (real `~/.manox` threads, a PTY terminal kind, an embedded
//! browser kind) — run it with:
//!
//! ```sh
//! cargo run -p manox-agent-chrome-ui --example shell
//! ```
//!
//! Known gaps at this stage (tracked in PLAN-CHROME-CHAT-SPLIT.md): the
//! session-row props model still carries the replica's three-state status
//! (the manox five-state semantics extend it at the assembly stage);
//! per-session right-pane tab sets are not yet modeled; the toolbar's
//! back/forward/run/split glyphs stay visual-only.
//!
//! All chrome text resolves through `manox-i18n` (`chrome-*` keys, zh-CN
//! primary / en fallback) — chrome copy is app chrome by definition.

pub mod divider;
pub mod main_surface;
pub mod panel;
pub mod primitives;
pub mod right_pane;
pub mod session_list;
pub mod shell;
pub mod theme;
pub mod titlebar;

pub use main_surface::{MainSurface, MainSurfaceHandle};
pub use panel::PanelSurface;
pub use right_pane::{RightPane, TabStore, ToolTab, ToolTabFactory};
pub use session_list::{
    CustomizationRow, FixedRow, SessionGroup, SessionList, SessionRowData, SessionStatus,
};
pub use shell::{HostHooks, SessionRow, Shell, ShellConfig};
pub use theme::{icons, register_fonts};

/// Window geometry for the shell host: the calibrated native traffic-light
/// slot (lights at (12,13), the 38px toolbar's midline ≈ y19 − the light
/// radius), app-owned titlebar dragging (the toolbar's empty space drags),
/// and the opaque background the light chrome expects.
pub fn window_options(cx: &gpui::App) -> gpui::WindowOptions {
    use gpui::px;
    gpui::WindowOptions {
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("Manox".into()),
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(12.), px(13.))),
        }),
        app_owns_titlebar_drag: true,
        window_bounds: Some(gpui::WindowBounds::centered(
            gpui::size(px(1280.), px(820.)),
            cx,
        )),
        window_min_size: Some(gpui::size(px(980.), px(640.))),
        window_background: gpui::WindowBackgroundAppearance::Opaque,
        ..Default::default()
    }
}
