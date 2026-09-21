//! VS Code 2026-Light palette, value-for-value from the agents-window
//! calibration (source: VS Code theme-defaults `2026-light.json` and the
//! agents-window tokens in `vscode/src/vs/sessions/common/theme.ts`).
//!
//! Mapping: agents.background(light) = sideBar `#FAFAFD` (shell base),
//! agentsPanel.background(light) = editor `#FFFFFF` (cards), borders are the
//! foreground at 15% alpha, the gradient tint is button `#0069CC`, and cards
//! use the 8px `--vscode-cornerRadius-large`.
//!
//! `gpui::rgb()`/`rgba()` are not const fn, so constants hold `Rgba` literals
//! (channels 0.0–1.0); the `c!`-style helpers below keep the source hex
//! readable.

use gpui::Rgba;

/// `#RRGGBB` → `Rgba` (const-context usable).
const fn c(hex: u32) -> Rgba {
    let [_, r, g, b] = hex.to_be_bytes();
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

/// `#RRGGBBAA` → `Rgba` (const-context usable).
const fn ca(hex: u32) -> Rgba {
    let [r, g, b, a] = hex.to_be_bytes();
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

// ---- base surfaces ----
pub const SHELL_BG: Rgba = c(0xFAFAFD); // agents.background / sideBar / titleBar / panel
pub const CARD_BG: Rgba = c(0xFFFFFF); // agentsPanel.background / editor / activeTab
pub const PANEL_BG: Rgba = c(0xFAFAFD); // terminal panel
pub const TABBAR_BG: Rgba = c(0xEAEAEA); // editorGroupHeader.tabsBackground / inactiveTab
pub const GRADIENT_TINT: Rgba = c(0x0069CC); // agentsGradient.tintColor = button

// ---- lines and strokes ----
pub const BORDER: Rgba = c(0xE3E3E6); // baseline border (light gray)
pub const CARD_BORDER: Rgba = ca(0x20202026); // foreground at 15% ≈ 38/255

// ---- text ----
pub const FG: Rgba = c(0x202020); // foreground / editor.foreground
pub const FG_STRONG: Rgba = c(0x202020); // headings (2026-light keeps foreground)
pub const FG_DIM: Rgba = c(0x606060); // descriptionForeground / titleBar.activeForeground
pub const FG_FAINT: Rgba = c(0x8A8A8E); // de-emphasized text (placeholders, times)

// ---- interaction ----
pub const ACCENT: Rgba = c(0x0069CC); // button.background / focusBorder
pub const ACCENT_HOVER: Rgba = c(0x005AAE); // one step darker
pub const LIST_HOVER: Rgba = ca(0x00000014);
pub const LIST_ACTIVE: Rgba = ca(0x00000025);
pub const TOOLBAR_HOVER: Rgba = ca(0x00000014);
pub const INPUT_BG: Rgba = c(0xFFFFFF); // input.background
/// Icon-button active underlay (accent at 10% ≈ 26/255).
pub const ICON_ON_BG: Rgba = ca(0x0069CC1A);
/// Flat-button whole-box hover (freya ColorsSheet surface_tertiary).
pub const SURFACE_TERTIARY: Rgba = c(0xF0F0F2);
/// Pressed state (freya ColorsSheet active).
pub const SURFACE_ACTIVE: Rgba = c(0xE0E0E3);

// ---- status colors (VS Code semantic colors) ----
pub const OK_GREEN: Rgba = c(0x1A7F37); // additions
pub const WARN_ORANGE: Rgba = c(0x9A6700); // modifications / warnings
pub const ERR_RED: Rgba = c(0xCF222E); // deletions / errors
pub const SPINNER_BLUE: Rgba = c(0x0069CC);

// badges
pub const BADGE_BLUE_BG: Rgba = c(0x0069CC);
pub const BADGE_BLUE_FG: Rgba = c(0xFFFFFF);

/// 8px card radius (`--vscode-cornerRadius-large`).
pub const CARD_RADIUS: f32 = 8.0;

/// Gap between floating cards and the window edge
/// (`--vscode-agents-layout-floatingPanelGap`).
pub const FLOAT_GAP: f32 = 8.0;

/// macOS traffic-light colors (kept for self-drawn Linux/Windows tracks; the
/// macOS build uses the native lights).
pub const TRAFFIC_RED: Rgba = c(0xFF5F57);
pub const TRAFFIC_YELLOW: Rgba = c(0xFEBC2E);
pub const TRAFFIC_GREEN: Rgba = c(0x28C840);
