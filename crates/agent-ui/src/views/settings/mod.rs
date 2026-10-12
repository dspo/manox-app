//! Settings overlay — a single-window alternative to opening a separate
//! preferences window. Mounts inline over the Workspace via
//! `Workspace::view_mode` as the upstream `gpui_component::setting::Settings`
//! shell: it owns the sidebar (search input + grouped, collapsible menu), the
//! resizable split and the page stack. Each page stacks its groups; simple
//! rows bind onto this view's state through `SettingField` value/setter
//! closures, while the larger panels ride along as opaque element items.

use std::collections::HashSet;

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, SharedString, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme as _, IconName,
    group_box::GroupBoxVariant,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
};

use crate::i18n;
use manox_agent::settings as user_settings;

use crate::views::plugin_manager::PluginManagerView;

mod chatgpt;
mod models;
mod panels;
mod vscode;

/// Sidebar width the upstream `Settings` shell starts at; the user can drag
/// it within the component's built-in range.
pub(crate) const NAV_WIDTH: gpui::Pixels = px(240.);

pub struct SettingsView {
    /// Plugin/marketplace management view, rendered in the "Plugins" group of
    /// the Integrations page. Owned here rather than as a top-level Workspace
    /// mode so plugins live under Settings.
    plugins: Entity<PluginManagerView>,
    /// Keeps the plugin-change observer alive for the view's lifetime.
    _plugins_sub: gpui::Subscription,
    /// Server names from the merged config (mcp.toml + plugin declarations),
    /// sorted. Cached so the settings render path never re-reads the config
    /// files; refreshed when the plugin view reports a change (installs and
    /// uninstalls alter the declared server set). The per-row registry status
    /// reads live off the process registry instead.
    mcp_servers: Vec<String>,
    /// The persisted UI language; the value source for the language dropdown.
    ui_language: i18n::Language,
    /// Form state for editing `cx.providers.config.yaml`, seeded from the
    /// file when the Settings view is created.
    models_panel: models::ModelsPanelState,
    /// Form state for the `chatgpt_app:` section of `cx.providers.config.yaml`,
    /// seeded from the file when the Settings view is created.
    chatgpt_panel: chatgpt::ChatGptPanelState,
    /// Form state for the `vscode_app:` section of `cx.providers.config.yaml`,
    /// seeded from the file when the Settings view is created.
    vscode_panel: vscode::VsCodePanelState,
    /// Server names switched off by the user; persisted to `settings.toml`
    /// on toggle (`manox_agent::settings::set_mcp_disabled`). Seeded from settings
    /// at view creation.
    pub(crate) mcp_disabled: HashSet<String>,
}

#[derive(Clone)]
pub enum SettingsEvent {
    Exit,
    /// A persistence step failed; the message is ready for a toast.
    SaveFailed(SharedString),
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let plugins = cx.new(|cx| PluginManagerView::new(window, cx));
        // Plugin installs and uninstalls change the declared MCP server set;
        // the cached list follows those changes instead of re-reading the
        // config files on every render.
        let _plugins_sub = cx.observe(&plugins, |this, _, cx| {
            this.mcp_servers = mcp_server_names();
            cx.notify();
        });
        Self {
            plugins,
            _plugins_sub,
            mcp_servers: mcp_server_names(),
            ui_language: steer_i18n::load_ui_language(),
            models_panel: models::ModelsPanelState::load(window, cx),
            chatgpt_panel: chatgpt::ChatGptPanelState::load(window, cx),
            vscode_panel: vscode::VsCodePanelState::load(window, cx),
            mcp_disabled: user_settings::mcp_disabled().into_iter().collect(),
        }
    }

    /// Persist `ui_language` and apply it live. The field flips only on a
    /// successful save; on failure the dropdown re-reads the unchanged value
    /// on the next frame, so the picker snaps back while the error surfaces
    /// as a `SettingsEvent::SaveFailed` toast.
    fn apply_ui_language(&mut self, token: SharedString, cx: &mut Context<Self>) {
        let Some(lang) = i18n::Language::from_token(&token) else {
            tracing::warn!(token = %token, "ignoring non-canonical ui_language token");
            return;
        };
        if lang == self.ui_language {
            return;
        }
        if let Err(e) = steer_i18n::persist_ui_language(lang) {
            tracing::warn!(error = %e, "failed to save ui_language");
            cx.emit(SettingsEvent::SaveFailed(e.to_string().into()));
            return;
        }
        self.ui_language = lang;
        i18n::set_ui_language(lang);
        // The settings shell's own strings (search placeholder, …) come from
        // gpui-component's rust_i18n bundle and follow the same locale.
        gpui_component::set_locale(lang.token());
        crate::menu::rebuild_menus(cx);
        cx.refresh_windows();
        cx.notify();
    }

    /// Toggle one MCP server and persist the whole disabled set.
    /// Persist-then-flip: the set commits only on a successful write, so a
    /// failed save leaves the switch at its prior value (it re-reads the
    /// unchanged set on the next render) while the error surfaces as a
    /// `SettingsEvent::SaveFailed` toast. The persisted set is what the
    /// registry consults on the next launch.
    fn apply_mcp_server(&mut self, name: &str, enabled: bool, cx: &mut Context<Self>) {
        let mut disabled = self.mcp_disabled.clone();
        if enabled {
            disabled.remove(name);
        } else {
            disabled.insert(name.to_string());
        }
        let list: Vec<String> = disabled.iter().cloned().collect();
        if let Err(e) = user_settings::set_mcp_disabled(list) {
            tracing::warn!(error = %e, "failed to persist MCP switch");
            cx.emit(SettingsEvent::SaveFailed(e.to_string().into()));
            cx.notify();
            return;
        }
        self.mcp_disabled = disabled;
        cx.notify();
    }

    /// The settings shell — the whole two-column surface below the TitleBar
    /// strip. Pages are rebuilt on every render so titles track the live
    /// locale; selection lives in the component's window-keyed state.
    pub(crate) fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity();
        Settings::new("settings")
            .sidebar_width(NAV_WIDTH)
            .pages(build_pages(&view, cx))
            .into_any_element()
    }
}

/// A panel renderer expressed as a free function of the view — the element
/// item runs it at paint time, after the owning render pass has released
/// the entity lease.
type PanelRender = fn(&mut SettingsView, &mut Context<SettingsView>) -> AnyElement;

/// One group hosting a full existing panel renderer as an opaque element —
/// the transitional shape for panels whose internals are not yet expressed
/// as `SettingItem` rows. `GroupBoxVariant` defaults to `Normal`, so the
/// panel draws without a group card surface around it. The keywords keep
/// the panel reachable from the search box (element items match the query
/// only through keywords); the localized group title rides along so the
/// panel is findable by its own displayed name in either locale.
fn panel_group(
    view: &Entity<SettingsView>,
    title: &'static str,
    keywords: &[&'static str],
    render: PanelRender,
) -> SettingGroup {
    let view = view.clone();
    SettingGroup::new().title(i18n::t(title)).item(
        SettingItem::render(move |_, _window, cx| view.update(cx, render)).keywords(
            keywords
                .iter()
                .copied()
                .map(SharedString::from)
                .chain(std::iter::once(i18n::t(title))),
        ),
    )
}

/// The merged-config server names, in stable sorted order (BTreeMap keys).
fn mcp_server_names() -> Vec<String> {
    manox_agent::mcp::load_merged_config()
        .mcp_servers
        .keys()
        .cloned()
        .collect()
}

/// The MCP group: one `SettingField::switch` row per server in the merged
/// config, with the live registry status as the row description. Toggles
/// persist immediately and apply from the next launch.
fn mcp_group(view: &Entity<SettingsView>, cx: &App) -> SettingGroup {
    let servers = view.read(cx).mcp_servers.clone();
    let connected: std::collections::HashMap<String, usize> = manox_agent::mcp::try_global()
        .map(|registry| {
            registry
                .servers()
                .iter()
                .map(|s| (s.name.clone(), s.tools.len()))
                .collect()
        })
        .unwrap_or_default();

    let mut group = SettingGroup::new()
        .title(i18n::t("settings-item-mcp"))
        .description(i18n::t("settings-desc-mcp"))
        .footer(|_, _| div().child(i18n::t("settings-mcp-restart-note")));

    if servers.is_empty() {
        return group.item(
            SettingItem::render(|_, _, cx| {
                div()
                    .py_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(i18n::t("settings-empty-mcp"))
            })
            .keywords(mcp_keywords()),
        );
    }

    for name in servers {
        let disabled = view.read(cx).mcp_disabled.contains(&name);
        let status: SharedString = if disabled {
            i18n::t("settings-mcp-status-disabled")
        } else if let Some(count) = connected.get(&name) {
            i18n::t_str("settings-mcp-tool-count", &[("count", &count.to_string())])
        } else {
            i18n::t("settings-mcp-status-not-connected")
        };
        let value_view = view.clone();
        let set_view = view.clone();
        let set_name = name.clone();
        group = group.item(
            SettingItem::new(
                SharedString::from(name.clone()),
                SettingField::switch(
                    move |cx| !value_view.read(cx).mcp_disabled.contains(&name),
                    move |enabled, cx| {
                        set_view
                            .update(cx, |this, cx| this.apply_mcp_server(&set_name, enabled, cx));
                    },
                ),
            )
            .description(status)
            .keywords(mcp_keywords()),
        );
    }
    group
}

/// Search keywords for the MCP rows: the English terms plus the localized
/// group title, so the rows stay reachable from the search box in either
/// locale (element-adjacent matching only sees keywords, titles and
/// descriptions — the localized group title is otherwise unsearched).
fn mcp_keywords() -> Vec<SharedString> {
    ["mcp", "server", "tools"]
        .into_iter()
        .map(SharedString::from)
        .chain(std::iter::once(i18n::t("settings-item-mcp")))
        .collect()
}

/// The three pages of the settings shell, mirroring the previous nav groups:
/// General (language + Models), Integrations (Plugins + MCP), External Tools.
fn build_pages(view: &Entity<SettingsView>, cx: &App) -> Vec<SettingPage> {
    let language_group = {
        let value_view = view.clone();
        let set_view = view.clone();
        SettingGroup::new()
            .title(i18n::t("settings-item-general"))
            .variant(GroupBoxVariant::Fill)
            .item(
                SettingItem::new(
                    i18n::t("settings-row-ui-language"),
                    SettingField::dropdown(
                        // (value, label): the field carries and persists the
                        // token, the picker displays the fixed endonym.
                        [i18n::Language::ZhCn, i18n::Language::En]
                            .into_iter()
                            .map(|lang| {
                                (
                                    SharedString::from(lang.token()),
                                    SharedString::from(lang.endonym()),
                                )
                            })
                            .collect(),
                        move |cx| SharedString::from(value_view.read(cx).ui_language.token()),
                        move |token, cx| {
                            set_view.update(cx, |this, cx| this.apply_ui_language(token, cx));
                        },
                    ),
                )
                .description(i18n::t("settings-desc-ui-language"))
                .keywords(["language", "locale", "语言", "english"]),
            )
    };

    vec![
        SettingPage::new(i18n::t("settings-group-general"))
            .icon(IconName::Settings)
            .default_open(true)
            .group(language_group)
            .group(panel_group(
                view,
                "settings-item-models",
                &["models", "providers", "llm", "api key", "endpoint"],
                models::render_models,
            )),
        SettingPage::new(i18n::t("settings-group-integrations"))
            .icon(IconName::Frame)
            .default_open(true)
            .group(panel_group(
                view,
                "settings-item-plugins",
                &["plugins", "marketplace", "extensions", "插件"],
                |view, _cx| view.plugins.clone().into_any_element(),
            ))
            .group(mcp_group(view, cx)),
        SettingPage::new(i18n::t("settings-group-external-tools"))
            .icon(IconName::Bot)
            .default_open(true)
            .group(panel_group(
                view,
                "settings-item-chatgpt-app",
                &["chatgpt", "codex", "injection", "external tools"],
                chatgpt::render_chatgpt_app,
            ))
            .group(panel_group(
                view,
                "settings-item-vscode-app",
                &["vscode", "visual studio code", "extension", "inject"],
                vscode::render_vscode_app,
            )),
    ]
}
