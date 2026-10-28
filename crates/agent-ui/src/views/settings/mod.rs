//! Settings overlay — a single-window alternative to opening a separate
//! preferences window. Mounts inline over the Workspace via
//! `Workspace::view_mode` as the upstream `gpui_component::setting::Settings`
//! shell: it owns the sidebar (search input + grouped, collapsible menu), the
//! resizable split and the page stack. Each page stacks its groups; simple
//! rows bind onto this view's state through `SettingField` value/setter
//! closures, while the larger panels ride along as opaque element items.

use std::collections::HashSet;

use gpui::{AnyElement, Context, Entity, EventEmitter, SharedString, Window, div, prelude::*, px};
use gpui_component::{
    ActiveTheme as _,
    group_box::GroupBoxVariant,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
};
use gpui_kit_assets::IconName;

use crate::i18n;
use manox_agent::settings as user_settings;

use crate::views::plugin_manager::PluginManagerView;

mod agents_page;
mod chatgpt;
mod models;
mod panels;
mod vscode;

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
    /// Read-only Agents roster (lead + preset + discovered defs), collected
    /// once at view creation — the registry scan must not run per frame.
    agents: Vec<AgentRow>,
    /// Read-only Skills roster, collected once at view creation.
    skills: Vec<SkillRow>,
    /// The Captain's live system prompt, rendered once at view creation
    /// through the runtime's own builder (template + skill fold). Read-only
    /// preview; the per-session assembly (project instructions, user notes,
    /// active-tool docs) is what a real session adds on top.
    captain_prompt: String,
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
            agents: collect_agents(),
            skills: collect_skills(),
            captain_prompt: captain_prompt_preview(),
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

    /// One page of the settings detail (the main column's content): the
    /// upstream `Settings` shell with a zero-width sidebar and a HIDDEN page
    /// header — the nav tree already says where you are, the detail never
    /// repeats it. Only the page's group stack renders.
    pub(crate) fn render_page(
        &mut self,
        ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = cx.entity();
        // build_page runs while this view holds its update lease, so the
        // state it needs is passed by reference — reaching back through the
        // entity handle here would panic (no reads on an updated entity).
        let page = build_page(
            &view,
            ix,
            &self.mcp_servers,
            &self.mcp_disabled,
            &self.agents,
            &self.skills,
            &self.captain_prompt,
        );
        use gpui::Styled as _;
        let hidden_header = div().hidden().style().clone();
        Settings::new(SharedString::from(format!("settings-detail-{ix}")))
            .sidebar_width(px(0.))
            .sidebar_size_range(px(0.)..px(0.))
            .page(page.header_style(&hidden_header))
            .into_any_element()
    }
}

/// One selectable page of the settings nav tree.
pub(crate) struct NavLeaf {
    /// Fluent key of the leaf's title.
    pub key: &'static str,
    /// Lucide icon rendered before the title.
    pub icon: IconName,
}

/// The settings nav tree: labeled groups (non-selectable) and selectable
/// leaves; a leaf may ride at the top level (no group) or indented under
/// the last group. The flat leaf order IS the page index space
/// (`Workspace::settings_nav`).
pub(crate) enum NavEntry {
    Group(&'static str),
    Leaf { leaf: NavLeaf, indented: bool },
}

pub(crate) const NAV: [NavEntry; 11] = [
    NavEntry::Group("settings-group-general"),
    NavEntry::Leaf {
        leaf: NavLeaf {
            key: "settings-item-general",
            icon: IconName::SlidersHorizontal,
        },
        indented: true,
    },
    NavEntry::Leaf {
        leaf: NavLeaf {
            key: "settings-item-models",
            icon: IconName::Boxes,
        },
        indented: false,
    },
    NavEntry::Leaf {
        leaf: NavLeaf {
            key: "settings-item-agents",
            icon: IconName::Bot,
        },
        indented: false,
    },
    NavEntry::Group("settings-group-integrations"),
    NavEntry::Leaf {
        leaf: NavLeaf {
            key: "settings-item-plugins",
            icon: IconName::Puzzle,
        },
        indented: true,
    },
    NavEntry::Leaf {
        leaf: NavLeaf {
            key: "settings-item-skills",
            icon: IconName::Sparkles,
        },
        indented: true,
    },
    NavEntry::Leaf {
        leaf: NavLeaf {
            key: "settings-item-mcp",
            icon: IconName::PlugZap,
        },
        indented: true,
    },
    NavEntry::Group("settings-group-external-tools"),
    NavEntry::Leaf {
        leaf: NavLeaf {
            key: "settings-item-chatgpt-app",
            icon: IconName::MessageCircle,
        },
        indented: true,
    },
    NavEntry::Leaf {
        leaf: NavLeaf {
            key: "settings-item-vscode-app",
            icon: IconName::Code,
        },
        indented: true,
    },
];

/// The selectable leaves in nav order — the page index space.
#[cfg(test)]
pub(crate) fn nav_leaves() -> Vec<(usize, &'static NavLeaf)> {
    let mut ix = 0;
    NAV.iter()
        .filter_map(|entry| match entry {
            NavEntry::Leaf { leaf, .. } => {
                let out = (ix, leaf);
                ix += 1;
                Some(out)
            }
            NavEntry::Group(_) => None,
        })
        .collect()
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
fn mcp_group(
    view: &Entity<SettingsView>,
    servers: &[String],
    disabled: &HashSet<String>,
) -> SettingGroup {
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
        // Field closures are 'static, so each row takes an owned name even
        // though the list itself is only borrowed.
        let name = name.clone();
        let is_disabled = disabled.contains(&name);
        let status: SharedString = if is_disabled {
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
/// The page factory for one nav leaf. Leaf order is `nav_leaves()`:
/// 0 常规 (language) · 1 模型 · 2 Agents · 3 插件 · 4 Skills · 5 MCP ·
/// 6 ChatGPT.app · 7 VS Code.
fn build_page(
    view: &Entity<SettingsView>,
    leaf: usize,
    mcp_servers: &[String],
    mcp_disabled: &HashSet<String>,
    _agents: &[AgentRow],
    skills: &[SkillRow],
    _captain_prompt: &str,
) -> SettingPage {
    let title = |key: &str| SettingPage::new(i18n::t(key)).default_open(true);
    match leaf {
        // 常规 — language and app-chrome preferences.
        0 => {
            let value_view = view.clone();
            let set_view = view.clone();
            title("settings-item-general").group(
                SettingGroup::new().variant(GroupBoxVariant::Fill).item(
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
                ),
            )
        }
        // 模型 — the provider/model editor.
        1 => title("settings-item-models").group(panel_group(
            view,
            "settings-item-models",
            &["models", "providers", "llm", "api key", "endpoint"],
            models::render_models,
        )),
        // Agents — read-only roster (the lead + preset + discovered defs).
        2 => title("settings-item-agents").group(panel_group(
            view,
            "settings-item-agents",
            &["agents", "subagents", "子代理", "captain"],
            agents_page::render,
        )),
        // 插件.
        3 => title("settings-item-plugins").group(panel_group(
            view,
            "settings-item-plugins",
            &["plugins", "marketplace", "extensions", "插件"],
            |view, _cx| view.plugins.clone().into_any_element(),
        )),
        // Skills — read-only registry view.
        4 => title("settings-item-skills").group(skills_group(skills)),
        // MCP.
        5 => title("settings-item-mcp").group(mcp_group(view, mcp_servers, mcp_disabled)),
        // 外部工具.
        6 => title("settings-item-chatgpt-app").group(panel_group(
            view,
            "settings-item-chatgpt-app",
            &["chatgpt", "codex", "injection", "external tools"],
            chatgpt::render_chatgpt_app,
        )),
        7 => title("settings-item-vscode-app").group(panel_group(
            view,
            "settings-item-vscode-app",
            &["vscode", "visual studio code", "extension", "inject"],
            vscode::render_vscode_app,
        )),
        _ => title("settings-item-general"),
    }
}

/// One read-only roster row for the Agents page.
pub(crate) struct AgentRow {
    pub name: String,
    pub description: String,
    /// Fluent key of the origin group (内置 / 用户 / 插件).
    pub origin: &'static str,
    /// The def's pinned model, when it has one.
    pub model: Option<String>,
    /// How many tools the def allows (empty def = all tools).
    pub tools: usize,
    pub all_tools: bool,
}

/// One read-only row for the Skills page.
struct SkillRow {
    name: String,
    description: String,
    /// Fluent key of the origin badge.
    origin: &'static str,
}

/// The Agents roster, read-only: the lead agent first, then the registry the
/// runtime itself builds — the built-in presets plus user/plugin discovery,
/// with user files able to override built-ins by name.
fn collect_agents() -> Vec<AgentRow> {
    use manox_harness::core::ext_point_agent::AgentRegistry;
    let mut rows = Vec::new();
    let mut registry = AgentRegistry::new();
    manox_harness::ext::subagent::register_defaults(&mut registry);
    manox_agent::agent_defs::register_user_and_plugin(&mut registry, None);
    for def in registry.all() {
        let origin = if def.name.contains('@') {
            "settings-origin-plugin"
        } else if def.name == "Explore" || def.name == "Sailor" {
            "settings-origin-builtin"
        } else {
            "settings-origin-user"
        };
        rows.push(AgentRow {
            name: def.name.clone(),
            description: def.description.clone(),
            origin,
            model: def.model.clone(),
            tools: def.tools.len(),
            all_tools: def.tools.is_empty(),
        });
    }
    rows
}

/// The Skills roster, read-only: a fresh scan of user + plugin skills
/// (`list_skill_records` — the same surface the plugin manager reads).
fn collect_skills() -> Vec<SkillRow> {
    manox_agent::skill::list_skill_records()
        .into_iter()
        .map(|record| SkillRow {
            name: record.name,
            description: record.description,
            origin: match record.origin {
                manox_agent::skill::SkillOrigin::Plugin { .. } => "settings-origin-plugin",
                _ => "settings-origin-user",
            },
        })
        .collect()
}

/// A muted right-aligned meta line (origin / model / tool count) — the
/// read-only pages' "control" column.
fn meta_field(text: SharedString) -> SettingField<SharedString> {
    SettingField::render(move |_, _, cx| {
        gpui_component::h_flex()
            .text_size(px(12.))
            .text_color(cx.theme().muted_foreground)
            .child(text.clone())
            .into_any_element()
    })
}

/// The Captain's system prompt rendered through the runtime's own builder —
/// the REAL preset (tool preferences, concurrency model, subagent roster
/// prose, the skill fold), not a summary of it. The per-session fold
/// (project instructions, user notes, active-tool docs) rides on top in a
/// live session; this preview is the stable base.
fn captain_prompt_preview() -> String {
    use manox_harness::core::harness::HarnessResources;
    use manox_harness::ext::prompt::{CaptainConfig, SkillSummary};
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let skills: Vec<SkillSummary> = manox_agent::skill::list_skill_records()
        .into_iter()
        .map(|record| SkillSummary {
            name: record.name,
            description: record.description,
        })
        .collect();
    let builder = manox_harness::ext::prompt::captain_prompt_builder(CaptainConfig {
        cwd: cwd.clone(),
        today: chrono::Local::now().format("%Y-%m-%d").to_string(),
        skills,
        user_notes: None,
    });
    builder(&[], &HarnessResources::default(), &cwd)
}

fn skills_group(skills: &[SkillRow]) -> SettingGroup {
    let mut group = SettingGroup::new().title(i18n::t("settings-item-skills"));
    for row in skills {
        group = group.item(
            SettingItem::new(
                SharedString::from(row.name.clone()),
                meta_field(i18n::t(row.origin)),
            )
            .description(SharedString::from(row.description.clone())),
        );
    }
    group.footer(|_, _| {
        gpui_component::v_flex().py_2().text_sm().child(
            gpui::div()
                .text_color(gpui::rgb(0x737373))
                .child(i18n::t("skills-readonly-note")),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::nav_leaves;

    /// The flat leaf order IS `build_page`'s match-arm index space; a drift
    /// here would silently swap pages behind the nav.
    #[test]
    fn nav_leaf_order_matches_the_page_factory() {
        let keys: Vec<&str> = nav_leaves().iter().map(|(_, leaf)| leaf.key).collect();
        assert_eq!(
            keys,
            [
                "settings-item-general",
                "settings-item-models",
                "settings-item-agents",
                "settings-item-plugins",
                "settings-item-skills",
                "settings-item-mcp",
                "settings-item-chatgpt-app",
                "settings-item-vscode-app",
            ]
        );
    }
}
