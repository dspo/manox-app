//! Settings overlay — a single-window alternative to opening a separate
//! preferences window. Mounts inline over the Workspace via
//! `Workspace::view_mode`; clicks on sidebar items update a local
//! `selected` highlight and the right pane dispatches to one of the shipped
//! panels (General / Models / Plugins / MCP / External Tools → ChatGPT.app,
//! VS Code).

use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, Entity, EventEmitter, Pixels, SharedString,
    Window, ease_out_quint, prelude::*,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, TITLE_BAR_HEIGHT, TitleBar, h_flex,
    input::{Input, InputState},
    v_flex,
};

use crate::i18n;
use manox_agent::settings as user_settings;

use crate::views::management_shell::back_control;
use crate::views::plugin_manager::PluginManagerView;

mod chatgpt;
mod models;
mod panels;
mod vscode;

const CLICK_FLASH_MS: u64 = 280;

/// A single static settings item (icon + label key, optional trailing icon).
/// `label` is a fluent message id resolved via `i18n::t` at render time, so the
/// displayed text tracks the UI locale while the id itself stays a stable key
/// for selection / element-id purposes.
#[derive(Clone)]
struct SettingsItem {
    icon: IconName,
    /// Custom SVG asset path (e.g. `"icons/blocks.svg"`). When set, rendered
    /// via `Icon::default().path(...)` in preference to `icon` (same mechanism
    /// as the sidebar's external-session rows).
    custom_icon: Option<&'static str>,
    label: &'static str,
    trailing: Option<IconName>,
}

struct SettingsGroup {
    title: &'static str,
    items: &'static [SettingsItem],
}

const GROUPS: &[SettingsGroup] = &[
    SettingsGroup {
        title: "settings-group-general",
        items: &[
            SettingsItem::new(IconName::Settings, "settings-item-general", None),
            SettingsItem::new(IconName::MemoryStick, "settings-item-models", None)
                .with_custom_icon("icons/blocks.svg"),
        ],
    },
    SettingsGroup {
        title: "settings-group-integrations",
        items: &[
            SettingsItem::new(IconName::Frame, "settings-item-plugins", None),
            SettingsItem::new(IconName::ChartPie, "settings-item-mcp", None),
        ],
    },
    SettingsGroup {
        title: "settings-group-external-tools",
        items: &[
            SettingsItem::new(IconName::Bot, "settings-item-chatgpt-app", None)
                .with_custom_icon("icons/chatgpt.svg"),
            SettingsItem::new(IconName::Bot, "settings-item-vscode-app", None)
                .with_custom_icon("icons/vscode.svg"),
        ],
    },
];

impl SettingsItem {
    const fn new(icon: IconName, label: &'static str, trailing: Option<IconName>) -> Self {
        Self {
            icon,
            custom_icon: None,
            label,
            trailing,
        }
    }

    /// Set a custom SVG asset path, rendered in preference to `icon`.
    const fn with_custom_icon(mut self, path: &'static str) -> Self {
        self.custom_icon = Some(path);
        self
    }
}

pub struct SettingsView {
    search: Entity<InputState>,

    /// Sidebar width, driven by the shared shell's divider. The Workspace
    /// owns the canonical width and syncs it here (and on re-entry) so the
    /// settings nav resizes exactly like the app sidebar.
    width: Pixels,
    /// Plugin/marketplace management view, rendered in the right pane when
    /// the "Plugins" item is selected. Owned here rather than as a
    /// top-level Workspace mode so plugins live under Settings →
    /// Integrations.
    plugins: Entity<PluginManagerView>,
    /// Sidebar item currently highlighted. Stable fluent message id (e.g
    /// `"settings-item-general"`) so it survives locale switches.
    selected: Option<SharedString>,

    /// Bumped on every click so the click-flash animation re-fires.
    click_gen: u64,

    // --- General panel state ---
    ui_language: SharedString,

    // --- Models panel state ---
    /// Form state for editing `cx.providers.config.yaml`, seeded from the
    /// file when the Settings view is created.
    models_panel: models::ModelsPanelState,

    // --- External tools → ChatGPT.app panel state ---
    /// Form state for the `chatgpt_app:` section of `cx.providers.config.yaml`,
    /// seeded from the file when the Settings view is created.
    chatgpt_panel: chatgpt::ChatGptPanelState,

    // --- External tools → Visual Studio Code.app panel state ---
    /// Form state for the `vscode_app:` section of `cx.providers.config.yaml`,
    /// seeded from the file when the Settings view is created.
    vscode_panel: vscode::VsCodePanelState,

    // --- MCP panel state ---
    /// Server names switched off by the user; persisted to `settings.toml`
    /// on toggle (`manox_agent::settings::set_mcp_disabled`). Seeded from settings
    /// at view creation.
    pub(crate) mcp_disabled: std::collections::HashSet<String>,
}

#[derive(Clone)]
pub enum SettingsEvent {
    Exit,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(width: Pixels, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(i18n::t("settings-search-placeholder"))
        });
        Self {
            search,
            width,
            plugins: cx.new(|cx| PluginManagerView::new(window, cx)),
            // Every nav item owns a panel now, so the pane starts on the
            // General page instead of an empty placeholder.
            selected: Some("settings-item-general".into()),
            click_gen: 0,
            // The endonym is fixed per language and never re-localized, so the
            // picker always reads `简体中文` / `English` regardless of the
            // current UI locale.
            ui_language: steer_i18n::load_ui_language().endonym().into(),
            models_panel: models::ModelsPanelState::load(window, cx),
            chatgpt_panel: chatgpt::ChatGptPanelState::load(window, cx),
            vscode_panel: vscode::VsCodePanelState::load(window, cx),
            mcp_disabled: user_settings::mcp_disabled().into_iter().collect(),
        }
    }

    /// Persist `ui_language`. The dropdown carries a `(endonym, token)` pair;
    /// only the token is written. Save must succeed before the runtime locale
    /// and the displayed value flip, so a failed write leaves the UI in its
    /// prior state and the error propagates to the caller for surfacing.
    fn persist_ui_language(
        &mut self,
        value: SharedString,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let token = value.to_string();
        if token.is_empty() {
            return Ok(());
        }
        let Some(lang) = i18n::Language::from_token(&token) else {
            tracing::warn!(token = %token, "ignoring non-canonical ui_language token");
            return Ok(());
        };
        if let Err(e) = steer_i18n::persist_ui_language(lang) {
            tracing::warn!(error = %e, "failed to save ui_language");
            return Err(e.to_string());
        }
        self.ui_language = lang.endonym().into();
        i18n::set_ui_language(lang);
        cx.refresh_windows();
        crate::menu::rebuild_menus(cx);
        cx.notify();
        Ok(())
    }
}

impl SettingsView {
    /// The settings nav rendered in the shared shell's sidebar slot: the back
    /// control and search input live in a pinned top slot (they never scroll
    /// away), and only the group list scrolls beneath them.
    pub(crate) fn render_nav(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.selected.clone();
        let search = self.search.clone();

        let on_back = cx.listener(|_this, _ev, _window, cx| {
            cx.emit(SettingsEvent::Exit);
        });

        let mut groups: Vec<AnyElement> = Vec::with_capacity(GROUPS.len());
        for group in GROUPS.iter() {
            let title = i18n::t(group.title);
            let mut column = v_flex().gap_0p5().child(
                gpui::div()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title),
            );
            for it in group.items.iter() {
                let label_str: SharedString = i18n::t(it.label);
                let is_selected = selected.as_ref().map(|s| s.as_str()) == Some(it.label);
                let bg = if is_selected {
                    theme.accent.opacity(0.12)
                } else {
                    theme.transparent
                };
                let icon = it.icon.clone();
                let trailing = it.trailing.clone();
                let label_key = it.label;
                let on_click = cx.listener(move |this, _ev, _window, cx| {
                    this.selected = Some(label_key.into());
                    this.click_gen = this.click_gen.wrapping_add(1);
                    cx.notify();
                });
                let mut row = h_flex()
                    .id(it.label)
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .rounded(theme.radius)
                    .text_sm()
                    .text_color(theme.foreground)
                    .bg(bg)
                    .hover(|s| s.bg(theme.accent.opacity(0.08)))
                    .active(|s| s.bg(theme.accent.opacity(0.24)))
                    .cursor_pointer()
                    .on_click(on_click)
                    .child(match it.custom_icon {
                        Some(path) => Icon::default()
                            .path(path)
                            .small()
                            .text_color(theme.muted_foreground),
                        None => Icon::new(icon).small().text_color(theme.muted_foreground),
                    })
                    .child(gpui::div().flex_1().min_w_0().child(label_str.clone()));
                if let Some(t) = trailing {
                    row = row.child(Icon::new(t).small().text_color(theme.muted_foreground));
                }
                if is_selected {
                    let anim_id = format!("settings-click-pulse-{}", self.click_gen);
                    let pulse_el = gpui::div()
                        .size_full()
                        .absolute()
                        .bg(theme.accent.opacity(0.30))
                        .rounded(theme.radius)
                        .with_animation(
                            anim_id,
                            Animation::new(std::time::Duration::from_millis(CLICK_FLASH_MS))
                                .with_easing(ease_out_quint()),
                            move |el, delta| el.opacity(1.0 - delta),
                        );
                    row = row.child(pulse_el);
                }
                column = column.child(row);
            }
            groups.push(column.into_any_element());
        }

        // macOS traffic-light buttons float over the sidebar's transparent top.
        let top_inset = crate::workspace::sidebar_top_inset();

        // Seamless slot (mirrors the conversation Sidebar): no own background
        // or border — the shell background shows through and the main card's
        // left border is the only visible boundary.
        v_flex()
            .h_full()
            .w(self.width)
            .flex_shrink_0()
            // Pinned top slot: back control + search input stay put while the
            // group list scrolls underneath.
            .child(
                v_flex()
                    .w_full()
                    .flex_shrink_0()
                    .px_2()
                    .pt(top_inset)
                    .pb_2()
                    .gap_2()
                    .child(back_control(&theme, i18n::t("settings-back"), on_back))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded(theme.radius)
                            .bg(theme.secondary)
                            .child(
                                Icon::new(IconName::Search)
                                    .small()
                                    .text_color(theme.muted_foreground),
                            )
                            .child(
                                Input::new(&search)
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("settings-sidebar-body")
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .gap_2()
                    .children(groups),
            )
            .into_any_element()
    }

    /// The settings panel rendered in the shared shell's main column — the
    /// same overlay scaffold as the conversation column: content sits below
    /// TITLE_BAR_HEIGHT; the TitleBar floats absolute on top as a drag region
    /// only (no text — the page's big heading carries the section identity,
    /// mirroring ChatGPT.app Settings).
    pub(crate) fn render_main(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .relative()
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .pt(TITLE_BAR_HEIGHT)
                    .child(self.render_right_pane(&theme, cx)),
            )
            .child(
                gpui::div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(TITLE_BAR_HEIGHT)
                    .child(TitleBar::new().child(h_flex())),
            )
            .into_any_element()
    }

    /// Update the rendered nav width. Called by the shared shell's divider
    /// drag (through the Workspace) so the settings sidebar resizes exactly
    /// like the app sidebar.
    pub fn set_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        if self.width == width {
            return;
        }
        self.width = width;
        cx.notify();
    }
}

impl SettingsView {
    /// Dispatch the right pane based on the currently-selected sidebar item.
    /// Every item owns a panel; an unknown key falls back to General.
    fn render_right_pane(
        &mut self,
        _theme: &gpui_component::theme::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.selected.as_deref() {
            Some("settings-item-models") => models::render_models(self, cx).into_any_element(),
            Some("settings-item-mcp") => panels::render_mcp(self, cx).into_any_element(),
            Some("settings-item-plugins") => self.plugins.clone().into_any_element(),
            Some("settings-item-chatgpt-app") => {
                chatgpt::render_chatgpt_app(self, cx).into_any_element()
            }
            Some("settings-item-vscode-app") => {
                vscode::render_vscode_app(self, cx).into_any_element()
            }
            _ => panels::render_general(self, cx).into_any_element(),
        }
    }
}
