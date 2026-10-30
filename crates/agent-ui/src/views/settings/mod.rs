//! Settings — the split layout's state and page factory. The chrome sidebar
//! carries the nav (`views::settings_nav`); this view renders the selected
//! page's detail as the upstream `gpui_component::setting::Settings` shell
//! with a zero-width sidebar and a hidden header, so only the page's group
//! stack paints. Each page stacks its groups; simple rows bind onto this
//! view's state through `SettingField` value/setter closures, while the
//! larger panels ride along as opaque element items.

use std::collections::HashSet;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, Hsla, SharedString, Window, div, prelude::*, px, rgb,
};
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
// The external-tool panels drive macOS app launches (ChatGPT.app / VS Code
// app bundles via osascript); on other platforms the actions can never
// succeed, so the panels, their nav group and their pages stay mac-only
// instead of dead-ending in error notifications.
#[cfg(target_os = "macos")]
mod chatgpt;
mod models;
mod panels;
#[cfg(target_os = "macos")]
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
    #[cfg(target_os = "macos")]
    chatgpt_panel: chatgpt::ChatGptPanelState,
    /// Form state for the `vscode_app:` section of `cx.providers.config.yaml`,
    /// seeded from the file when the Settings view is created.
    #[cfg(target_os = "macos")]
    vscode_panel: vscode::VsCodePanelState,
    /// Server names switched off by the user; persisted to `settings.toml`
    /// on toggle (`manox_agent::settings::set_mcp_disabled`). Seeded from settings
    /// at view creation.
    pub(crate) mcp_disabled: HashSet<String>,
    /// Read-only Agents roster (lead + preset + discovered defs). Read at view
    /// creation and re-read when the foreground project changes or a plugin
    /// lands — the registry scan must not run per frame.
    agents: Vec<AgentRow>,
    /// Read-only Skills roster, refreshed with the Agents roster.
    skills: Vec<SkillRow>,
    /// The foreground thread's cwd the rosters above were read against
    /// (`None` before any chrome frame populated it). Drives the refresh: the
    /// project layer only changes when this does.
    rosters_cwd: Option<std::path::PathBuf>,
    /// The Captain's live system prompt, rendered through the runtime's own
    /// builder (template + skill fold) against the foreground thread's cwd.
    /// Empty until a foreground thread exists — the preview never invents a
    /// working directory. Read-only; the per-session assembly (project
    /// instructions, user notes, active-tool docs) is what a real session
    /// adds on top.
    captain_prompt: String,
    /// The drilled-down agent of the Agents page (`None` = the list).
    pub(crate) selected_agent: Option<String>,
    /// The read-only source editor behind the detail's 编辑 mode. Owned here
    /// so scroll position and selection survive re-renders; its text follows
    /// the drilled-down agent.
    pub(crate) prompt_editor: Entity<gpui_component::input::EditorState>,
    /// How the detail's system prompt renders. A view choice, not a write:
    /// every drill-down starts from the rendered preview.
    pub(crate) prompt_view: PromptView,
}

/// The system prompt's display mode on the Agents detail.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PromptView {
    /// Rendered markdown.
    #[default]
    Preview,
    /// The read-only source editor (line numbers, soft wrap, scrolling).
    Edit,
}

/// The Claude color vocabulary, in cycle order. The TINTS are not redefined
/// here: [`accent_by_name`] reads the app's one mapping
/// (`steer_agent_chat_ui::views::subagents::agent_color`), so a
/// definition's color is the same on its roster tile as on the transcript's
/// sub-agent row.
const ACCENT_NAMES: [&str; 8] = [
    "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan",
];

/// A stable accent per list position: the thin border marks the item, the
/// avatar carries the same tint, and nothing paints a background. A
/// definition that declares a color takes precedence (see
/// [`accent_by_name`]).
pub(super) fn accent_color(ix: usize) -> Hsla {
    accent_by_name(ACCENT_NAMES[ix % ACCENT_NAMES.len()]).unwrap_or_else(fallback_accent)
}

/// The neutral tint for anything outside the color vocabulary — a render
/// path never panics on a name.
pub(super) fn fallback_accent() -> Hsla {
    Hsla::from(rgb(0x888888))
}

/// The 36px rounded-square avatar every roster tile carries, tinted with the
/// item's accent.
pub(super) fn tile_avatar(
    theme: &gpui_component::Theme,
    accent: Hsla,
    icon: gpui_component::Icon,
) -> gpui::Div {
    div()
        .size(px(36.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.radius_lg)
        .bg(accent.opacity(0.12))
        .child(icon.size_4().text_color(accent))
}

/// The 20px read-only pill every roster tile uses for its meta badges.
pub(super) fn tile_badge(theme: &gpui_component::Theme, text: SharedString) -> gpui::Div {
    div()
        .h(px(20.))
        .px_1p5()
        .flex_none()
        .flex()
        .items_center()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .text_size(px(11.))
        .text_color(theme.muted_foreground)
        .child(text)
}

/// The tint for a declared color name (case-insensitive), or `None` for
/// names outside the vocabulary.
pub(super) fn accent_by_name(name: &str) -> Option<Hsla> {
    steer_agent_chat_ui::views::subagents::agent_color(&name.to_ascii_lowercase())
}

#[derive(Clone)]
pub enum SettingsEvent {
    /// A persistence step failed; the message is ready for a toast.
    SaveFailed(SharedString),
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let plugins = cx.new(|cx| PluginManagerView::new(window, cx));
        // Plugin installs and uninstalls change the declared MCP server set
        // AND the agent/skill rosters those plugins ship; both cached lists
        // follow the change instead of re-reading the config files on every
        // render.
        let _plugins_sub = cx.observe(&plugins, |this, _, cx| {
            this.mcp_servers = mcp_server_names();
            this.reload_rosters();
            cx.notify();
        });
        let prompt_editor = cx.new(|cx| {
            let mut state = gpui_component::input::EditorState::new(window, cx);
            state.set_readonly(true, cx);
            state
        });
        // Read the rosters against the foreground thread's cwd: a bundled
        // app's `current_dir()` is `/`, so the process cwd is never the
        // session's project layer.
        let rosters_cwd = crate::chrome_assembly::foreground_cwd();
        Self {
            plugins,
            _plugins_sub,
            mcp_servers: mcp_server_names(),
            ui_language: steer_i18n::load_ui_language(),
            models_panel: models::ModelsPanelState::load(window, cx),
            #[cfg(target_os = "macos")]
            chatgpt_panel: chatgpt::ChatGptPanelState::load(window, cx),
            #[cfg(target_os = "macos")]
            vscode_panel: vscode::VsCodePanelState::load(window, cx),
            mcp_disabled: user_settings::mcp_disabled().into_iter().collect(),
            agents: collect_agents(rosters_cwd.as_deref()),
            skills: collect_skills(),
            rosters_cwd: rosters_cwd.clone(),
            captain_prompt: rosters_cwd
                .as_deref()
                .map(captain_prompt_preview)
                .unwrap_or_default(),
            selected_agent: None,
            prompt_editor,
            prompt_view: PromptView::default(),
        }
    }

    /// Re-read the rosters and the Captain prompt when the foreground project
    /// changed. The scan is UI-thread work (registry + skills + the runtime's
    /// prompt builder), so an unchanged cwd is a no-op — re-entering Settings
    /// in the same project must not pay for it again.
    pub(crate) fn refresh_rosters(&mut self) {
        let cwd = crate::chrome_assembly::foreground_cwd();
        if cwd == self.rosters_cwd {
            return;
        }
        self.rosters_cwd = cwd;
        self.reload_rosters();
    }

    /// Re-read the rosters against the cwd already recorded — a plugin
    /// install/uninstall changes the agent and skill layers without touching
    /// the foreground project.
    pub(crate) fn reload_rosters(&mut self) {
        let cwd = self.rosters_cwd.clone();
        self.agents = collect_agents(cwd.as_deref());
        self.skills = collect_skills();
        self.captain_prompt = cwd
            .as_deref()
            .map(captain_prompt_preview)
            .unwrap_or_default();
    }

    /// Drill into one agent's detail: load its prompt into the source editor
    /// and reset the display to the rendered preview.
    pub(crate) fn open_agent(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.agent_prompt(&name);
        self.selected_agent = Some(name);
        self.prompt_view = PromptView::Preview;
        self.prompt_editor
            .update(cx, |state, cx| state.set_value(body, window, cx));
    }

    /// Return to the Agents list.
    pub(crate) fn close_agent(&mut self) {
        self.selected_agent = None;
        self.prompt_view = PromptView::Preview;
    }

    /// Drill the Models page into the provider at list position `ix`.
    pub(crate) fn select_model_provider(&mut self, ix: usize) {
        self.models_panel.select_at(ix);
    }

    /// Flip the Agents detail's prompt display (the visual harness captures
    /// both modes).
    pub(crate) fn set_prompt_edit(&mut self, edit: bool) {
        self.prompt_view = if edit {
            PromptView::Edit
        } else {
            PromptView::Preview
        };
    }

    /// The system prompt of one roster entry (the lead's runtime-rendered
    /// base for the Captain).
    fn agent_prompt(&self, name: &str) -> String {
        if name == "Captain" {
            return self.captain_prompt.clone();
        }
        self.agents
            .iter()
            .find(|row| row.name == name)
            .map(|row| row.system_prompt.clone())
            .unwrap_or_default()
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
        // The leaf is resolved to its page so the factory can match on the
        // same vocabulary the nav tree renders. `select_settings_page` clamps
        // its input, so an out-of-range index here means something bypassed
        // the setter: degrade to the first page instead of crashing a render.
        let page_id = nav_leaves()
            .into_iter()
            .find(|(leaf_ix, _)| *leaf_ix == ix)
            .map(|(_, leaf)| leaf.page)
            .unwrap_or_else(|| {
                debug_assert!(false, "settings page index {ix} outside the nav tree");
                nav_leaves()
                    .first()
                    .map(|(_, leaf)| leaf.page)
                    .unwrap_or(Page::General)
            });
        let page = build_page(
            &view,
            page_id,
            &self.mcp_servers,
            &self.mcp_disabled,
            &self.skills,
        );
        use gpui::Styled as _;
        let hidden_header = div().hidden().style().clone();
        // Component state keyed by the PAGE, not the index: a NAV reorder
        // must not carry one page's scroll state onto another.
        Settings::new(SharedString::from(format!("settings-detail-{page_id:?}")))
            .sidebar_width(px(0.))
            .sidebar_size_range(px(0.)..px(0.))
            .page(page.header_style(&hidden_header))
            .into_any_element()
    }
}

/// One selectable page of the settings nav tree. The factory matches on
/// this enum, so a NAV edit that adds or removes a leaf fails to compile
/// until `build_page` follows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct NavLeaf {
    pub page: Page,
    /// Lucide icon rendered before the title.
    pub icon: IconName,
}

/// The pages themselves; each names its Fluent key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Page {
    General,
    Models,
    Agents,
    Plugins,
    Skills,
    Mcp,
    ChatGptApp,
    VsCodeApp,
}

impl Page {
    pub(crate) fn i18n_key(self) -> &'static str {
        match self {
            Page::General => "settings-item-general",
            Page::Models => "settings-item-models",
            Page::Agents => "settings-item-agents",
            Page::Plugins => "settings-item-plugins",
            Page::Skills => "settings-item-skills",
            Page::Mcp => "settings-item-mcp",
            Page::ChatGptApp => "settings-item-chatgpt-app",
            Page::VsCodeApp => "settings-item-vscode-app",
        }
    }
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
            page: Page::General,
            icon: IconName::SlidersHorizontal,
        },
        indented: true,
    },
    NavEntry::Leaf {
        leaf: NavLeaf {
            page: Page::Models,
            icon: IconName::Boxes,
        },
        indented: false,
    },
    NavEntry::Leaf {
        leaf: NavLeaf {
            page: Page::Agents,
            icon: IconName::Bot,
        },
        indented: false,
    },
    NavEntry::Group("settings-group-integrations"),
    NavEntry::Leaf {
        leaf: NavLeaf {
            page: Page::Plugins,
            icon: IconName::Puzzle,
        },
        indented: true,
    },
    NavEntry::Leaf {
        leaf: NavLeaf {
            page: Page::Skills,
            icon: IconName::Sparkles,
        },
        indented: true,
    },
    NavEntry::Leaf {
        leaf: NavLeaf {
            page: Page::Mcp,
            icon: IconName::PlugZap,
        },
        indented: true,
    },
    // 外部工具 — the two pages drive .app launches, so they only exist on macOS
    // (the pages' panels are cfg'd out elsewhere; see the module gates above).
    #[cfg(target_os = "macos")]
    NavEntry::Group("settings-group-external-tools"),
    #[cfg(target_os = "macos")]
    NavEntry::Leaf {
        leaf: NavLeaf {
            page: Page::ChatGptApp,
            icon: IconName::MessageCircle,
        },
        indented: true,
    },
    #[cfg(target_os = "macos")]
    NavEntry::Leaf {
        leaf: NavLeaf {
            page: Page::VsCodeApp,
            icon: IconName::Code,
        },
        indented: true,
    },
];

/// The selectable leaves in nav order — the page index space.
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
/// only through keywords); the localized group title rides along when there
/// is one, so the panel stays findable by its own displayed name in either
/// locale. A `None` title renders the group headless — for pages whose own
/// chrome (the Agents detail's back row) already says where the user is.
fn panel_group(
    view: &Entity<SettingsView>,
    title: Option<&'static str>,
    keywords: &[&'static str],
    render: PanelRender,
) -> SettingGroup {
    let view = view.clone();
    let keywords = keywords
        .iter()
        .copied()
        .map(SharedString::from)
        .chain(title.map(i18n::t));
    let mut group = SettingGroup::new();
    if let Some(title) = title {
        group = group.title(i18n::t(title));
    }
    group
        .item(SettingItem::render(move |_, _window, cx| view.update(cx, render)).keywords(keywords))
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

/// The page factory for one nav leaf, matched on the leaf's [`Page`] — the
/// match must cover every page the nav can select.
fn build_page(
    view: &Entity<SettingsView>,
    page: Page,
    mcp_servers: &[String],
    mcp_disabled: &HashSet<String>,
    skills: &[SkillRow],
) -> SettingPage {
    let title = |key: &str| SettingPage::new(i18n::t(key)).default_open(true);
    match page {
        // 常规 — language and app-chrome preferences.
        Page::General => {
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
        // 模型 — the provider list with a drill-down detail. No group title:
        // the nav tree names the page and the list/detail own their chrome.
        Page::Models => title("settings-item-models").group(panel_group(
            view,
            None,
            &["models", "providers", "llm", "api key", "endpoint"],
            models::render_models,
        )),
        // Agents — read-only roster (the lead + preset + discovered defs).
        // No group title: the nav tree already names the page, and the
        // detail's own top row carries the back control instead.
        Page::Agents => title("settings-item-agents").group(panel_group(
            view,
            None,
            &["agents", "subagents", "子代理", "captain"],
            agents_page::render,
        )),
        // 插件.
        Page::Plugins => title("settings-item-plugins").group(panel_group(
            view,
            Some("settings-item-plugins"),
            &["plugins", "marketplace", "extensions", "插件"],
            |view, _cx| view.plugins.clone().into_any_element(),
        )),
        // Skills — read-only registry view.
        Page::Skills => title("settings-item-skills").group(skills_group(skills)),
        // MCP.
        Page::Mcp => title("settings-item-mcp").group(mcp_group(view, mcp_servers, mcp_disabled)),
        // 外部工具 — macOS only: the panels launch .app bundles, and the nav
        // does not offer these leaves elsewhere, so the arms only need a
        // compile-time-valid body off macOS.
        #[cfg(target_os = "macos")]
        Page::ChatGptApp => title("settings-item-chatgpt-app").group(panel_group(
            view,
            Some("settings-item-chatgpt-app"),
            &["chatgpt", "codex", "injection", "external tools"],
            chatgpt::render_chatgpt_app,
        )),
        #[cfg(target_os = "macos")]
        Page::VsCodeApp => title("settings-item-vscode-app").group(panel_group(
            view,
            Some("settings-item-vscode-app"),
            &["vscode", "visual studio code", "extension", "inject"],
            vscode::render_vscode_app,
        )),
        #[cfg(not(target_os = "macos"))]
        Page::ChatGptApp | Page::VsCodeApp => title("settings-item-general"),
    }
}

/// One read-only roster row for the Agents page.
#[derive(Clone)]
pub(crate) struct AgentRow {
    pub name: String,
    pub description: String,
    /// Fluent key of the origin group (内置 / 用户 / 插件).
    pub origin: &'static str,
    /// The def's pinned model, when it has one.
    pub model: Option<String>,
    /// The def's allowed tool names (empty = all tools).
    pub tools: Vec<String>,
    pub all_tools: bool,
    /// The def's declared display color (Claude vocabulary), when the
    /// manifest carries one.
    pub color: Option<String>,
    /// The def's system prompt (the manifest body).
    pub system_prompt: String,
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
/// plus the project layer when a foreground thread has a cwd (the same
/// layers a live session loads), with earlier layers overridden by later
/// same-name registrations. `None` (no foreground thread yet) loads no
/// project layer rather than inventing one.
fn collect_agents(cwd: Option<&std::path::Path>) -> Vec<AgentRow> {
    use manox_harness::core::ext_point_agent::AgentRegistry;
    let mut rows = Vec::new();
    let mut registry = AgentRegistry::new();
    manox_harness::ext::subagent::register_defaults(&mut registry);
    manox_agent::agent_defs::register_user_and_plugin(&mut registry, cwd);
    for def in registry.all() {
        // TODO(dspo/manox#985): the registry stamps each definition's layer
        // (`AgentSource`) — group on it instead of this name heuristic once
        // the next tag carries it. Project-layer agents mislabel as 用户
        // until then.
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
            tools: def.tools.clone(),
            all_tools: def.tools.is_empty(),
            color: def.color.clone(),
            system_prompt: def.system_prompt.clone(),
        });
    }
    // Stable list order: built-ins first, then user files, then plugin
    // entries — the list reads the same regardless of discovery order.
    // Ranked on the origin itself, not the Fluent key strings.
    rows.sort_by(|a, b| {
        origin_rank(a.origin)
            .cmp(&origin_rank(b.origin))
            .then(a.name.cmp(&b.name))
    });
    rows
}

/// The display order of the origin groups (内置 → 用户 → 插件).
fn origin_rank(origin: &str) -> u8 {
    match origin {
        "settings-origin-builtin" => 0,
        "settings-origin-user" => 1,
        _ => 2,
    }
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
fn captain_prompt_preview(cwd: &std::path::Path) -> String {
    use manox_harness::core::harness::HarnessResources;
    use manox_harness::ext::prompt::{CaptainConfig, SkillSummary};
    let cwd = cwd.to_path_buf();
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
    use super::{accent_by_name, nav_leaves, origin_rank};

    /// The roster palette must BE the chat's mapping, not a second table:
    /// one definition renders on a roster tile and on the transcript's
    /// sub-agent row, and the two must agree.
    #[test]
    fn accent_palette_reads_the_canonical_agent_color_mapping() {
        use steer_agent_chat_ui::views::subagents::agent_color;
        for name in [
            "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan",
        ] {
            assert_eq!(accent_by_name(name), agent_color(name), "{name} drifted");
        }
        assert_eq!(accent_by_name("chartreuse"), None, "outside the vocabulary");
    }

    #[test]
    fn origin_rank_orders_builtin_user_plugin() {
        let mut origins = [
            "settings-origin-plugin",
            "settings-origin-user",
            "settings-origin-builtin",
        ];
        origins.sort_by_key(|origin| origin_rank(origin));
        assert_eq!(
            origins,
            [
                "settings-origin-builtin",
                "settings-origin-user",
                "settings-origin-plugin",
            ]
        );
    }

    /// The flat leaf order is the page index space (`Workspace::settings_nav`
    /// → `nav_leaves()[ix]`); a drift here would reorder the pages behind
    /// the nav's labels.
    #[test]
    fn nav_leaf_order_is_the_page_index_space() {
        let keys: Vec<&str> = nav_leaves()
            .iter()
            .map(|(_, leaf)| leaf.page.i18n_key())
            .collect();
        #[cfg(target_os = "macos")]
        let expected = [
            "settings-item-general",
            "settings-item-models",
            "settings-item-agents",
            "settings-item-plugins",
            "settings-item-skills",
            "settings-item-mcp",
            "settings-item-chatgpt-app",
            "settings-item-vscode-app",
        ];
        #[cfg(not(target_os = "macos"))]
        let expected = [
            "settings-item-general",
            "settings-item-models",
            "settings-item-agents",
            "settings-item-plugins",
            "settings-item-skills",
            "settings-item-mcp",
        ];
        assert_eq!(keys, expected);
    }
}
