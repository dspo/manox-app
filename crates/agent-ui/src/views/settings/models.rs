//! Settings → Models panel.
//!
//! A provider list with a drill-down detail, over the cx providers config
//! (`~/.manox/cx.providers.config.yaml`, schema: `manox_providers::CxConfig`).
//! The list stacks one thin-bordered tile per provider; clicking a tile opens
//! its detail, which stacks all four sections — 基本信息 / 环境变量 / 端点配置
//! / 模型列表 — in one scrolling column (no nested split). Every edit
//! schedules a debounced autosave that validates the form, atomically writes
//! the whole config back — preserving the top-level `agents:` section
//! verbatim — and reloads the process-wide provider registry on a background
//! thread so new threads pick up the change.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Anchor, AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity,
    InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::theme::Theme;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    v_flex,
};
use gpui_kit_assets::IconName as AssetIcon;

use crate::i18n;
use manox_providers::{
    AgentConfig, ApiKeySourceKind, CxConfig, ProviderConfig, ProviderEndpointDetail,
    ProviderEndpointSpec, ProviderModelConfig, ProviderModels, active_provider_config_path,
    agent_display_name, canonical_agent_id, known_agent_ids, normalize_agent_configs,
    normalize_agent_ids, read_config_file, split_apikey_source, write_config_file,
};

use super::SettingsView;
use super::panels::{
    blur_on_click_out, defer_blur_flush, muted_text, section_card, section_header,
};

const LABEL_W: f32 = 168.;
const KEY_W: f32 = 360.;
const PROVIDER_ROW_MIN_H: f32 = 76.;
const AUTOSAVE_DEBOUNCE_MS: u64 = 600;
const WIRE_APIS: [&str; 3] = ["anthropic", "responses", "completions"];

/// User-facing wire API names, shared by the endpoint dropdown and the model
/// Wire API checkboxes.
fn wire_display(wire: &str) -> &str {
    match wire {
        "anthropic" => "Anthropic Messages",
        "responses" => "OpenAI Responses",
        "completions" => "OpenAI Completions",
        other => other,
    }
}

/// The copilot auth row applies only when the endpoint's agents allow-list
/// covers GitHub Copilot (an empty list means all agents).
fn agents_include_copilot(agents: &[String]) -> bool {
    agents.is_empty() || agents.iter().any(|a| canonical_agent_id(a) == "copilot")
}

fn kind_token(kind: ApiKeySourceKind) -> &'static str {
    match kind {
        ApiKeySourceKind::Env => "env",
        ApiKeySourceKind::Keychain => "keychain",
        ApiKeySourceKind::Shell => "shell",
        ApiKeySourceKind::Literal | ApiKeySourceKind::None => "literal",
    }
}

fn kind_from_token(token: &str) -> ApiKeySourceKind {
    match token {
        "env" => ApiKeySourceKind::Env,
        "keychain" => ApiKeySourceKind::Keychain,
        "shell" => ApiKeySourceKind::Shell,
        _ => ApiKeySourceKind::Literal,
    }
}

fn kind_label(kind: ApiKeySourceKind) -> SharedString {
    match kind {
        ApiKeySourceKind::Env => i18n::t("settings-models-apikey-env"),
        ApiKeySourceKind::Keychain => i18n::t("settings-models-apikey-keychain"),
        ApiKeySourceKind::Shell => i18n::t("settings-models-apikey-shell"),
        ApiKeySourceKind::Literal | ApiKeySourceKind::None => {
            i18n::t("settings-models-apikey-literal")
        }
    }
}

fn kind_placeholder(kind: ApiKeySourceKind) -> &'static str {
    match kind {
        ApiKeySourceKind::Env => "ENV_VAR_NAME",
        ApiKeySourceKind::Keychain => "SERVICE_NAME",
        ApiKeySourceKind::Shell => "command, e.g. op read ...",
        ApiKeySourceKind::Literal | ApiKeySourceKind::None => "sk-...",
    }
}

// --- Panel state ----------------------------------------------------------

/// Everything the Models panel knows about the config file. Built once when
/// the Settings view is created; autosave serializes it back on edit.
pub struct ModelsPanelState {
    /// Resolved config file path (`None` only when `$HOME` is unresolvable).
    path: Option<PathBuf>,
    /// Read/parse failure message. Autosave stays disabled while set so an
    /// unrecognized file is never clobbered.
    load_error: Option<String>,
    /// Top-level `agents:` section — out of scope for this form, preserved
    /// verbatim across saves.
    agents: Vec<AgentConfig>,
    /// Candidate agent ids for the tag pickers (built-ins + configured).
    agent_options: Vec<String>,
    providers: Vec<ProviderForm>,
    /// The provider drilled into (`None` = the provider list).
    selected: Option<usize>,
    /// Delete button armed for two-step confirmation (element id key).
    pending_delete: Option<String>,
    /// Monotonic id source so GPUI element ids stay unique across
    /// add/remove churn.
    next_form_id: usize,
    /// Debounce token for autosave: each `touch` bumps it; a pending save
    /// task only writes when its captured generation is still current.
    save_generation: usize,
    /// Handle to the settings window, kept so the debounced autosave task
    /// (which only sees an `AsyncApp`) can still surface toasts.
    window_handle: AnyWindowHandle,
    /// Unsaved edits exist (set by `touch`, cleared by a successful `save`).
    dirty: bool,
    /// A save completed since the last blur; the next blur surfaces a "Saved"
    /// toast.
    pending_saved_toast: bool,
}

struct ProviderForm {
    id: usize,
    name: Entity<InputState>,
    apikey_kind: ApiKeySourceKind,
    apikey_value: Entity<InputState>,
    endpoints: Vec<EndpointForm>,
    env: Vec<KvForm>,
    /// `models:` is a remote URL string instead of inline definitions.
    remote_models: bool,
    remote_url: Entity<InputState>,
    models: Vec<ModelForm>,
}

struct EndpointForm {
    id: usize,
    /// Map key in `endpoints:` — one of `anthropic` / `responses` /
    /// `completions`.
    wire_api: SharedString,
    url: Entity<InputState>,
    /// Selected `agents:` override filter; empty = all agents.
    agents: Vec<String>,
    /// Empty = unset (null in YAML), else `api_key` / `bearer_token`.
    copilot_auth: SharedString,
}

struct KvForm {
    id: usize,
    key: Entity<InputState>,
    value: Entity<InputState>,
}

struct ModelForm {
    id: usize,
    model_id: Entity<InputState>,
    desc: Entity<InputState>,
    context: Entity<InputState>,
    max_tokens: Entity<InputState>,
    wire_anthropic: bool,
    wire_responses: bool,
    wire_completions: bool,
    /// Selected `agents:` override filter; empty = all agents.
    agents: Vec<String>,
    env: Vec<KvForm>,
    /// Effective value: config null resolves to the documented default
    /// (tools = true, images = false) at load time.
    supports_tools: bool,
    supports_images: bool,
}

type MutApply = Arc<dyn Fn(&mut SettingsView, &mut Context<SettingsView>) + Send + Sync + 'static>;
type WindowApply =
    Arc<dyn Fn(&mut SettingsView, &mut Window, &mut Context<SettingsView>) + Send + Sync + 'static>;
type TokenApply = Arc<
    dyn Fn(&mut SettingsView, SharedString, &mut Window, &mut Context<SettingsView>)
        + Send
        + Sync
        + 'static,
>;
type TagsApply =
    Arc<dyn Fn(&mut SettingsView, Vec<String>, &mut Context<SettingsView>) + Send + Sync + 'static>;

impl ModelsPanelState {
    /// Read the active config file and build the form state. A missing file
    /// is not an error — saving creates it.
    pub fn load(window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        let mut state = Self {
            path: None,
            load_error: None,
            agents: Vec::new(),
            agent_options: Vec::new(),
            providers: Vec::new(),
            selected: None,
            pending_delete: None,
            next_form_id: 1,
            save_generation: 0,
            window_handle: window.window_handle(),
            dirty: false,
            pending_saved_toast: false,
        };
        state.reload_from_disk(window, cx);
        state
    }

    /// Re-read the config file, discarding any unsaved edits.
    pub fn reload_from_disk(&mut self, window: &mut Window, cx: &mut Context<SettingsView>) {
        self.agents.clear();
        self.providers.clear();
        self.pending_delete = None;
        self.selected = None;
        self.load_error = None;
        // A reload discards unsaved edits; the flush-on-blur flags must not
        // outlive them.
        self.dirty = false;
        self.pending_saved_toast = false;

        let path = match active_provider_config_path() {
            Ok(path) => path,
            Err(e) => {
                self.path = None;
                self.load_error = Some(format!("{e:#}"));
                return;
            }
        };
        self.path = Some(path.clone());
        if !path.exists() {
            return;
        }
        match read_config_file(&path) {
            Ok(config) => self.apply_config(window, cx, config),
            Err(e) => self.load_error = Some(format!("{e:#}")),
        }
    }

    fn apply_config(
        &mut self,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
        config: CxConfig,
    ) {
        self.agent_options = known_agent_ids(&config);
        // Canonicalize + dedupe the preserved `agents:` section so removed
        // legacy ids (codex+) never round-trip back into the config on save.
        self.agents = normalize_agent_configs(config.agents);
        self.providers = config
            .providers
            .into_iter()
            .map(|p| ProviderForm::from_config(p, window, cx, &mut self.next_form_id))
            .collect();
        // The list is the entry face; drilling in is always a click.
        self.selected = None;
        self.load_error = None;
    }

    /// Mark the form dirty and schedule a debounced autosave.
    pub fn touch(&mut self, cx: &mut Context<SettingsView>) {
        if self.load_error.is_some() || self.path.is_none() {
            return;
        }
        self.save_generation = self.save_generation.wrapping_add(1);
        self.dirty = true;
        let generation = self.save_generation;
        let entity = cx.entity().clone();
        let handle = self.window_handle;
        cx.spawn(async move |_this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(AUTOSAVE_DEBOUNCE_MS))
                .await;
            let stale = entity.read_with(cx, |this, _| {
                this.models_panel.save_generation != generation
            });
            if stale {
                return;
            }
            let _ = handle.update(cx, |_view, window, cx| {
                entity.update(cx, |this, cx| {
                    if this.models_panel.save_generation == generation {
                        // Debounced safety net stays silent; blur surfaces the
                        // "Saved" toast via `flush_on_blur`.
                        let _ = this.models_panel.save(window, cx);
                    }
                });
            });
        })
        .detach();
    }

    fn new_form_id(&mut self) -> usize {
        let id = self.next_form_id;
        self.next_form_id += 1;
        id
    }

    fn provider_mut(&mut self, id: usize) -> Option<&mut ProviderForm> {
        self.providers.iter_mut().find(|p| p.id == id)
    }

    fn endpoint_mut(&mut self, provider_id: usize, id: usize) -> Option<&mut EndpointForm> {
        self.provider_mut(provider_id)?
            .endpoints
            .iter_mut()
            .find(|e| e.id == id)
    }

    fn model_mut(&mut self, provider_id: usize, id: usize) -> Option<&mut ModelForm> {
        self.provider_mut(provider_id)?
            .models
            .iter_mut()
            .find(|m| m.id == id)
    }

    /// Drill into one provider's detail.
    fn select_provider(&mut self, pid: usize) {
        self.selected = Some(pid);
    }

    /// Drill into the provider at `ix` in list order (the visual harness and
    /// any future deep link use this; user input goes through the tiles).
    pub fn select_at(&mut self, ix: usize) {
        self.selected = self.providers.get(ix).map(|p| p.id);
    }

    /// Back to the provider list.
    pub fn close_provider(&mut self) {
        self.selected = None;
    }

    fn add_provider(&mut self, window: &mut Window, cx: &mut Context<SettingsView>) {
        let form = ProviderForm::from_config(
            ProviderConfig {
                name: String::new(),
                apikey_source: None,
                models: ProviderModels::Inline(BTreeMap::new()),
                endpoints: BTreeMap::new(),
                env: BTreeMap::new(),
            },
            window,
            cx,
            &mut self.next_form_id,
        );
        let id = form.id;
        self.providers.push(form);
        // A new provider lands straight in its detail, name input focused, so
        // it can be named without hunting for it in the list.
        self.selected = Some(id);
        if let Some(name) = self.provider_mut(id).map(|p| p.name.clone()) {
            name.update(cx, |state, cx| state.focus(window, cx));
        }
    }

    fn remove_provider(&mut self, id: usize) {
        self.providers.retain(|p| p.id != id);
        if self.selected == Some(id) {
            self.selected = None;
        }
    }

    fn add_endpoint(
        &mut self,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
        provider_id: usize,
    ) {
        let id = self.new_form_id();
        if let Some(provider) = self.provider_mut(provider_id) {
            let used: HashSet<&str> = provider
                .endpoints
                .iter()
                .map(|e| e.wire_api.as_str())
                .collect();
            let Some(wire) = WIRE_APIS.iter().find(|w| !used.contains(**w)) else {
                return;
            };
            provider.endpoints.push(EndpointForm {
                id,
                wire_api: SharedString::from(*wire),
                url: new_input(window, cx, "", Some(i18n::t("settings-models-ph-url"))),
                agents: Vec::new(),
                copilot_auth: SharedString::default(),
            });
        }
    }

    fn remove_endpoint(&mut self, provider_id: usize, id: usize) {
        if let Some(provider) = self.provider_mut(provider_id) {
            provider.endpoints.retain(|e| e.id != id);
        }
    }

    fn env_rows_mut(&mut self, pid: usize, mid: Option<usize>) -> Option<&mut Vec<KvForm>> {
        let provider = self.provider_mut(pid)?;
        match mid {
            Some(mid) => Some(&mut provider.models.iter_mut().find(|m| m.id == mid)?.env),
            None => Some(&mut provider.env),
        }
    }

    /// Insert an empty env row after `index` (or at the end when `None`) and
    /// focus its key input, ready for typing.
    fn add_env_row(
        &mut self,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
        pid: usize,
        mid: Option<usize>,
        index: Option<usize>,
    ) {
        let id = self.new_form_id();
        if let Some(rows) = self.env_rows_mut(pid, mid) {
            let at = match index {
                Some(ix) => (ix + 1).min(rows.len()),
                None => rows.len(),
            };
            rows.insert(at, KvForm::new(id, window, cx));
            if let Some(key) = rows.get(at).map(|row| row.key.clone()) {
                key.update(cx, |state, cx| state.focus(window, cx));
            }
        }
    }

    fn remove_env_row(&mut self, pid: usize, mid: Option<usize>, id: usize) {
        if let Some(rows) = self.env_rows_mut(pid, mid) {
            rows.retain(|kv| kv.id != id);
        }
    }

    fn add_model(
        &mut self,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
        provider_id: usize,
    ) {
        let id = self.new_form_id();
        if let Some(provider) = self.provider_mut(provider_id) {
            provider.models.push(ModelForm {
                id,
                model_id: new_input(window, cx, "", None),
                desc: new_input(window, cx, "", None),
                context: new_input(window, cx, "", Some(i18n::t("settings-models-ph-context"))),
                max_tokens: new_input(
                    window,
                    cx,
                    "",
                    Some(i18n::t("settings-models-ph-max-tokens")),
                ),
                wire_anthropic: false,
                wire_responses: false,
                wire_completions: false,
                agents: Vec::new(),
                env: Vec::new(),
                supports_tools: true,
                supports_images: false,
            });
        }
    }

    fn remove_model(&mut self, provider_id: usize, id: usize) {
        if let Some(provider) = self.provider_mut(provider_id) {
            provider.models.retain(|m| m.id != id);
        }
    }

    /// Validate the form and rebuild the `CxConfig`. Errors are localized
    /// single-line messages suitable for a toast.
    fn collect(&self, cx: &App) -> Result<CxConfig, SharedString> {
        let mut providers = Vec::with_capacity(self.providers.len());
        for (index, p) in self.providers.iter().enumerate() {
            let name = p.name.read(cx).value().trim().to_string();
            if name.is_empty() {
                return Err(i18n::t_str(
                    "settings-models-err-provider-name",
                    &[("index", &index.to_string())],
                ));
            }
            let apikey_value = p.apikey_value.read(cx).value();
            let apikey_source = if apikey_value.trim().is_empty() {
                None
            } else {
                p.apikey_kind.build(apikey_value.as_str())
            };

            let mut endpoints = BTreeMap::new();
            for e in &p.endpoints {
                let url = e.url.read(cx).value().trim().to_string();
                if url.is_empty() {
                    return Err(provider_err(&name, "settings-models-err-endpoint-url", &[]));
                }
                let wire = e.wire_api.to_string();
                if endpoints.contains_key(&wire) {
                    return Err(provider_err(
                        &name,
                        "settings-models-err-endpoint-dup",
                        &[("wire", &wire)],
                    ));
                }
                let copilot_auth = if agents_include_copilot(&e.agents) {
                    optional_text(&e.copilot_auth)
                } else {
                    None
                };
                let spec = if e.agents.is_empty() && copilot_auth.is_none() {
                    ProviderEndpointSpec::Url(url)
                } else {
                    ProviderEndpointSpec::Detailed(ProviderEndpointDetail {
                        url,
                        agents: e.agents.clone(),
                        copilot_auth,
                    })
                };
                endpoints.insert(wire, spec);
            }

            let env = collect_env(cx, &p.env, &name, None)?;

            let models = if p.remote_models {
                let url = p.remote_url.read(cx).value().trim().to_string();
                if url.is_empty() {
                    return Err(provider_err(&name, "settings-models-err-remote-url", &[]));
                }
                ProviderModels::RemoteUrl(url)
            } else {
                let mut map = BTreeMap::new();
                for m in &p.models {
                    let id = m.model_id.read(cx).value().trim().to_string();
                    if id.is_empty() {
                        return Err(provider_err(&name, "settings-models-err-model-id", &[]));
                    }
                    if map.contains_key(&id) {
                        return Err(provider_err(
                            &name,
                            "settings-models-err-model-dup",
                            &[("id", &id)],
                        ));
                    }
                    let context = parse_opt_u64(&m.context.read(cx).value()).map_err(|()| {
                        model_err(
                            &name,
                            &id,
                            "settings-models-err-number",
                            &[("field", i18n::t("settings-models-row-context").as_ref())],
                        )
                    })?;
                    let max_tokens =
                        parse_opt_u64(&m.max_tokens.read(cx).value()).map_err(|()| {
                            model_err(
                                &name,
                                &id,
                                "settings-models-err-number",
                                &[("field", i18n::t("settings-models-row-max-tokens").as_ref())],
                            )
                        })?;
                    let mut wire_apis = Vec::new();
                    if m.wire_anthropic {
                        wire_apis.push("anthropic".to_string());
                    }
                    if m.wire_responses {
                        wire_apis.push("responses".to_string());
                    }
                    if m.wire_completions {
                        wire_apis.push("completions".to_string());
                    }
                    map.insert(
                        id.clone(),
                        ProviderModelConfig {
                            desc: optional_text(&m.desc.read(cx).value()),
                            wire_apis,
                            agents: m.agents.clone(),
                            env: collect_env(cx, &m.env, &name, Some(&id))?,
                            max_tokens,
                            context,
                            supports_tools: Some(m.supports_tools),
                            supports_images: Some(m.supports_images),
                        },
                    );
                }
                ProviderModels::Inline(map)
            };

            providers.push(ProviderConfig {
                name,
                apikey_source,
                models,
                endpoints,
                env,
            });
        }
        Ok(CxConfig {
            providers,
            agents: self.agents.clone(),
            // Carried over from a fresh disk read in `save()`; this form does
            // not model these sections (External Tools panels + subagent
            // model pinning).
            chatgpt_app: None,
            vscode_app: None,
            subagents: Default::default(),
        })
    }

    /// Validate, write the config file, and refresh the runtime registry.
    /// Failures surface as error toasts and return `false`; a successful
    /// write returns `true` with the runtime reload spawned asynchronously.
    fn save(&mut self, window: &mut Window, cx: &mut Context<SettingsView>) -> bool {
        if self.load_error.is_some() {
            return false;
        }
        let Some(path) = self.path.clone() else {
            window.push_notification(
                Notification::error(i18n::t("settings-models-no-path"))
                    .title(i18n::t("settings-save-failed-title")),
                cx,
            );
            return false;
        };
        let mut config = match self.collect(cx) {
            Ok(config) => config,
            Err(msg) => {
                window.push_notification(
                    Notification::error(msg).title(i18n::t("settings-save-failed-title")),
                    cx,
                );
                return false;
            }
        };
        // The `chatgpt_app:` / `vscode_app:` / `subagents:` sections are not
        // modeled by this form: the first two belong to the External Tools
        // panels, `subagents:` to the host's subagent dispatch — any of them
        // may change on disk after this panel took its open-time snapshot.
        // Carry them over from a fresh disk read so this form never clobbers
        // them. A failed re-read must abort the save (not silently drop the
        // sections): writing without them would erase the panels' settings
        // and the user's subagent model pinning, the exact clobber this
        // carry-over exists to prevent. Mirror the panels' `?` propagation
        // with a toast.
        match read_config_file(&path) {
            Ok(fresh) => carry_over_unmodeled_sections(&mut config, &fresh),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "re-read of provider config failed; aborting Models save to preserve unmodeled sections"
                );
                window.push_notification(
                    Notification::error(e.to_string()).title(i18n::t("settings-save-failed-title")),
                    cx,
                );
                return false;
            }
        }
        if let Err(e) = write_config_file(&path, &config) {
            tracing::warn!(error = %e, "failed to write cx providers config");
            window.push_notification(
                Notification::error(e.to_string()).title(i18n::t("settings-save-failed-title")),
                cx,
            );
            return false;
        }

        self.dirty = false;
        self.pending_saved_toast = true;

        // Rebuild the process-wide registry from the freshly written file so
        // new threads see the change without a restart. Api key resolution
        // may hit the OS keychain or run shell commands, so it runs off the
        // main thread; on failure the previous registry stays live.
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { manox_agent::provider_glue::reload() })
                .await;
            let _ = this.update_in(cx, |_this, window, cx| match result {
                Ok(()) => crate::menu::rebuild_menus(cx),
                Err(e) => {
                    tracing::warn!(error = %e, "provider registry reload failed after settings save");
                    window.push_notification(
                        Notification::error(e.to_string())
                            .title(i18n::t("settings-models-reload-failed-title")),
                        cx,
                    );
                }
            });
        })
        .detach();

        true
    }

    /// A blur with unsaved edits flushes the pending autosave immediately, then
    /// surfaces the "Saved" toast exactly once per edited field.
    fn flush_on_blur(&mut self, window: &mut Window, cx: &mut Context<SettingsView>) {
        if self.load_error.is_some() || self.path.is_none() {
            return;
        }
        if self.dirty {
            // Invalidate the in-flight debounced save; save now instead.
            self.save_generation = self.save_generation.wrapping_add(1);
            if !self.save(window, cx) {
                return; // `save` already surfaced the error toast.
            }
            self.pending_saved_toast = false;
            window.push_notification(Notification::success(i18n::t("settings-saved")), cx);
        } else if self.pending_saved_toast {
            self.pending_saved_toast = false;
            window.push_notification(Notification::success(i18n::t("settings-saved")), cx);
        }
    }
}

impl KvForm {
    fn new(id: usize, window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        Self {
            id,
            key: new_input(window, cx, "", None),
            value: new_input(window, cx, "", None),
        }
    }
}

impl ProviderForm {
    fn from_config(
        config: ProviderConfig,
        window: &mut Window,
        cx: &mut Context<SettingsView>,
        next_form_id: &mut usize,
    ) -> Self {
        let mut take_id = || {
            let id = *next_form_id;
            *next_form_id += 1;
            id
        };
        let (apikey_kind, apikey_value) = match config.apikey_source.as_deref() {
            Some(source) => split_apikey_source(source),
            None => (ApiKeySourceKind::Literal, String::new()),
        };
        let name = new_input(
            window,
            cx,
            &config.name,
            Some(i18n::t("settings-models-ph-name")),
        );
        let endpoints = config
            .endpoints
            .into_iter()
            .map(|(wire_api, spec)| {
                let (url, agents, copilot_auth) = match spec {
                    ProviderEndpointSpec::Url(url) => (url, Vec::new(), None),
                    // Nested allow-lists get the same legacy cleanup as the
                    // top-level `agents:` section.
                    ProviderEndpointSpec::Detailed(detail) => (
                        detail.url,
                        normalize_agent_ids(&detail.agents),
                        detail.copilot_auth,
                    ),
                };
                EndpointForm {
                    id: take_id(),
                    wire_api: SharedString::from(wire_api),
                    url: new_input(window, cx, &url, Some(i18n::t("settings-models-ph-url"))),
                    agents,
                    copilot_auth: SharedString::from(copilot_auth.unwrap_or_default()),
                }
            })
            .collect();
        let env = config
            .env
            .into_iter()
            .map(|(key, value)| KvForm {
                id: take_id(),
                key: new_input(window, cx, &key, None),
                value: new_input(window, cx, &value, None),
            })
            .collect();
        let remote_models = matches!(config.models, ProviderModels::RemoteUrl(_));
        let remote_url = match &config.models {
            ProviderModels::RemoteUrl(url) => url.clone(),
            ProviderModels::Inline(_) => String::new(),
        };
        let remote_url = new_input(
            window,
            cx,
            &remote_url,
            Some(i18n::t("settings-models-ph-remote-url")),
        );
        let models = match config.models {
            ProviderModels::Inline(map) => map
                .into_iter()
                .map(|(model_id, m)| ModelForm {
                    id: take_id(),
                    model_id: new_input(window, cx, &model_id, None),
                    desc: new_input(window, cx, m.desc.as_deref().unwrap_or_default(), None),
                    context: new_input(
                        window,
                        cx,
                        &m.context.map(|v| v.to_string()).unwrap_or_default(),
                        Some(i18n::t("settings-models-ph-context")),
                    ),
                    max_tokens: new_input(
                        window,
                        cx,
                        &m.max_tokens.map(|v| v.to_string()).unwrap_or_default(),
                        Some(i18n::t("settings-models-ph-max-tokens")),
                    ),
                    wire_anthropic: m.wire_apis.iter().any(|w| w == "anthropic"),
                    wire_responses: m.wire_apis.iter().any(|w| w == "responses"),
                    wire_completions: m.wire_apis.iter().any(|w| w == "completions"),
                    agents: normalize_agent_ids(&m.agents),
                    env: m
                        .env
                        .into_iter()
                        .map(|(key, value)| KvForm {
                            id: take_id(),
                            key: new_input(window, cx, &key, None),
                            value: new_input(window, cx, &value, None),
                        })
                        .collect(),
                    supports_tools: m.supports_tools.unwrap_or(true),
                    supports_images: m.supports_images.unwrap_or(false),
                })
                .collect(),
            ProviderModels::RemoteUrl(_) => Vec::new(),
        };
        let id = take_id();
        ProviderForm {
            id,
            name,
            apikey_kind,
            apikey_value: new_input(
                window,
                cx,
                &apikey_value,
                Some(SharedString::from(kind_placeholder(apikey_kind))),
            ),
            endpoints,
            env,
            remote_models,
            remote_url,
            models,
        }
    }
}

// --- Small helpers --------------------------------------------------------

fn new_input(
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    value: &str,
    placeholder: Option<SharedString>,
) -> Entity<InputState> {
    let value = SharedString::from(value.to_string());
    let state = cx.new(|cx| {
        let mut state = match placeholder {
            Some(placeholder) => InputState::new(window, cx).placeholder(placeholder),
            None => InputState::new(window, cx),
        };
        if !value.is_empty() {
            state.set_value(value, window, cx);
        }
        state
    });
    // Text edits flow into the same debounced autosave as discrete picks; a
    // blur flushes pending edits and surfaces the "Saved" toast.
    cx.subscribe(&state, |this, _state, event, cx| match event {
        InputEvent::Change => this.models_panel.touch(cx),
        InputEvent::Blur => {
            let handle = this.models_panel.window_handle;
            defer_blur_flush(cx, handle, |this, window, cx| {
                this.models_panel.flush_on_blur(window, cx);
            });
        }
        _ => {}
    })
    .detach();
    state
}

fn optional_text(value: &SharedString) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_opt_u64(value: &SharedString) -> Result<Option<u64>, ()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed.parse::<u64>().map(Some).map_err(|_| ())
}

/// Collect `KEY: value` rows. Blank rows are skipped; a value without a key
/// is an error. `model_id` narrows the error message to a model-level `env`
/// block.
fn collect_env(
    cx: &App,
    rows: &[KvForm],
    provider_name: &str,
    model_id: Option<&str>,
) -> Result<BTreeMap<String, String>, SharedString> {
    let mut env = BTreeMap::new();
    for kv in rows {
        let key = kv.key.read(cx).value().trim().to_string();
        // Trim symmetrically with the key: stray padding around env values is
        // never intentional and would silently defeat exact-match parsing
        // downstream (e.g. `MANOX_PROMPT_CACHING: " full"`).
        let value = kv.value.read(cx).value().trim().to_string();
        if key.is_empty() {
            if value.is_empty() {
                continue;
            }
            return Err(match model_id {
                Some(id) => model_err(provider_name, id, "settings-models-err-env-key", &[]),
                None => provider_err(provider_name, "settings-models-err-env-key", &[]),
            });
        }
        env.insert(key, value);
    }
    Ok(env)
}

/// Localized validation error scoped to a provider.
fn provider_err(provider_name: &str, key: &'static str, extra: &[(&str, &str)]) -> SharedString {
    let mut args: Vec<(&str, &str)> = Vec::with_capacity(extra.len() + 1);
    args.push(("name", provider_name));
    args.extend_from_slice(extra);
    i18n::t_str(key, &args)
}

/// Localized validation error scoped to a model inside a provider.
fn model_err(
    provider_name: &str,
    model_id: &str,
    key: &'static str,
    extra: &[(&str, &str)],
) -> SharedString {
    let mut args: Vec<(&str, &str)> = Vec::with_capacity(extra.len() + 2);
    args.push(("name", provider_name));
    args.push(("id", model_id));
    args.extend_from_slice(extra);
    i18n::t_str(key, &args)
}

// --- Rendering ------------------------------------------------------------

/// Entry face of Settings → Models: the provider list, or one provider's
/// detail when drilled in.
pub fn render_models(view: &mut SettingsView, cx: &mut Context<SettingsView>) -> AnyElement {
    let theme = cx.theme().clone();
    if let Some(err) = view.models_panel.load_error.clone() {
        return v_flex()
            .id("models-load-error")
            .w_full()
            .h_full()
            .overflow_y_scroll()
            .p_4()
            .child(render_load_error(&theme, err))
            .into_any_element();
    }
    match view.models_panel.selected {
        Some(pid) => render_provider_detail(view, &theme, pid, cx),
        None => render_provider_list(view, &theme, cx),
    }
}

/// The provider list: one thin-bordered tile per provider (no card fill, a
/// per-position accent), then the dashed add-provider control.
fn render_provider_list(
    view: &mut SettingsView,
    theme: &Theme,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let entity = cx.entity();
    if view.models_panel.providers.is_empty() {
        return v_flex()
            .id("models-empty")
            .w_full()
            .h_full()
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .child(render_empty(theme))
            .child(dashed_add_button(
                theme,
                "models-add-provider",
                "settings-models-add-provider",
                entity,
                Arc::new(move |this, window, cx| {
                    this.models_panel.add_provider(window, cx);
                    this.models_panel.touch(cx);
                }),
            ))
            .into_any_element();
    }

    let mut column = v_flex().w_full().gap(px(10.));
    for (ix, provider) in view.models_panel.providers.iter().enumerate() {
        column = column.child(render_provider_tile(
            theme,
            provider,
            ix,
            entity.clone(),
            cx,
        ));
    }
    v_flex()
        .id("models-list")
        .w_full()
        .h_full()
        .min_h_0()
        .overflow_y_scroll()
        .p_4()
        .gap_3()
        .child(column)
        .child(dashed_add_button(
            theme,
            "models-add-provider",
            "settings-models-add-provider",
            entity,
            Arc::new(move |this, window, cx| {
                this.models_panel.add_provider(window, cx);
                this.models_panel.touch(cx);
            }),
        ))
        .into_any_element()
}

/// One provider tile: avatar · name + kind/endpoint/model badges / the first
/// endpoint URL (or a muted "no endpoints" line).
fn render_provider_tile(
    theme: &Theme,
    provider: &ProviderForm,
    ix: usize,
    entity: Entity<SettingsView>,
    cx: &Context<SettingsView>,
) -> AnyElement {
    let accent = super::accent_color(ix);
    let pid = provider.id;
    let name = provider.name.read(cx).value();
    let name: SharedString = if name.trim().is_empty() {
        i18n::t("settings-models-unnamed")
    } else {
        name
    };
    let endpoint_count = provider.endpoints.len();
    let model_count = provider.models.len();
    let first_url = provider
        .endpoints
        .first()
        .map(|e| e.url.read(cx).value())
        .filter(|url| !url.trim().is_empty());
    let subtitle: SharedString = first_url.unwrap_or_else(|| {
        if provider.endpoints.is_empty() {
            i18n::t("settings-models-no-endpoints")
        } else {
            SharedString::from("")
        }
    });

    let click_entity = entity.clone();
    div()
        .id(("models-provider-row", pid))
        .min_h(px(PROVIDER_ROW_MIN_H))
        .w_full()
        .overflow_hidden()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(accent.opacity(0.45))
        .px_4()
        .py_3()
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .hover(move |this| this.border_color(accent))
        .on_click(move |_ev, _window, cx| {
            click_entity.update(cx, |this, cx| {
                this.models_panel.select_provider(pid);
                cx.notify();
            });
        })
        .child(
            div()
                .size(px(36.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.radius_lg)
                .bg(accent.opacity(0.12))
                .child(Icon::new(AssetIcon::Server).size_4().text_color(accent)),
        )
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap(px(3.))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .overflow_hidden()
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child(name),
                        )
                        .child(meta_badge(theme, kind_label(provider.apikey_kind)))
                        .child(meta_badge(
                            theme,
                            i18n::t_str(
                                "settings-models-badge-endpoints",
                                &[("count", &endpoint_count.to_string())],
                            ),
                        ))
                        .child(meta_badge(
                            theme,
                            i18n::t_str(
                                "settings-models-badge-models",
                                &[("count", &model_count.to_string())],
                            ),
                        )),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(subtitle),
                ),
        )
        .into_any_element()
}

/// Small read-only pill on a provider tile.
fn meta_badge(theme: &Theme, text: SharedString) -> AnyElement {
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
        .into_any_element()
}

fn render_load_error(theme: &Theme, error: String) -> AnyElement {
    section_card(
        theme,
        vec![
            section_header("settings-models-load-error-title"),
            v_flex()
                .px_3()
                .pb_2()
                .gap_1()
                .child(div().text_sm().text_color(theme.danger).child(error))
                .child(muted_text(
                    i18n::t("settings-models-load-error-hint"),
                    theme.muted_foreground,
                ))
                .into_any_element(),
        ],
    )
}

fn render_empty(theme: &Theme) -> AnyElement {
    section_card(
        theme,
        vec![
            v_flex()
                .w_full()
                .items_center()
                .px_3()
                .py_6()
                .child(muted_text(
                    i18n::t("settings-models-empty"),
                    theme.muted_foreground,
                ))
                .into_any_element(),
        ],
    )
}

/// One provider's detail: the back row, then all four sections stacked in a
/// single scrolling column (no nested split).
fn render_provider_detail(
    view: &mut SettingsView,
    theme: &Theme,
    pid: usize,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let entity = cx.entity();
    let Some(p) = view.models_panel.providers.iter().find(|p| p.id == pid) else {
        // The provider vanished (deleted); fall back to the list.
        return render_provider_list(view, theme, cx);
    };
    let agent_options = view.models_panel.agent_options.clone();
    let pending = view.models_panel.pending_delete.clone();

    // Top row: back to the list on the left, delete-provider on the right.
    let back_entity = entity.clone();
    let back = div()
        .id("models-back")
        .h(px(30.))
        .px_2()
        .flex()
        .items_center()
        .gap_1p5()
        .rounded(theme.radius)
        .text_size(px(13.))
        .text_color(theme.foreground)
        .cursor_pointer()
        .hover(|this| this.bg(theme.secondary))
        .on_click(move |_ev, _window, cx| {
            back_entity.update(cx, |this, cx| {
                this.models_panel.close_provider();
                cx.notify();
            });
        })
        .child(Icon::new(AssetIcon::ArrowLeft).size_4())
        .child(i18n::t("settings-models-back"));
    let remove_key = format!("models-p{pid}-remove");
    let remove_pending = pending.as_deref() == Some(remove_key.as_str());
    let top = h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .child(back)
        .child(delete_provider_button(
            theme,
            pid,
            remove_pending,
            entity.clone(),
        ));

    let sections = v_flex()
        .w_full()
        .gap_4()
        .pt_4()
        .pb_6()
        .child(detail_section(
            theme,
            AssetIcon::BadgeInfo,
            "settings-models-section-basic",
            vec![
                detail_field_row(
                    theme,
                    i18n::t("settings-models-row-name"),
                    input_field(&p.name),
                ),
                detail_field_row(
                    theme,
                    i18n::t("settings-models-row-apikey"),
                    render_apikey_control(entity.clone(), pid, p),
                ),
            ],
        ))
        .child(detail_section(
            theme,
            AssetIcon::Variable,
            "settings-models-section-env",
            vec![env_rows(theme, entity.clone(), pid, None, &p.env, &pending)],
        ))
        .child(detail_section(
            theme,
            AssetIcon::Globe,
            "settings-models-section-endpoints",
            vec![render_endpoints_module(
                theme,
                entity.clone(),
                pid,
                p,
                &agent_options,
                &pending,
            )],
        ))
        .child(detail_section(
            theme,
            AssetIcon::List,
            "settings-models-section-models",
            vec![render_models_module(
                theme,
                entity.clone(),
                pid,
                p,
                &agent_options,
                &pending,
            )],
        ));

    v_flex()
        .id("models-detail")
        .w_full()
        .h_full()
        .min_h_0()
        .overflow_y_scroll()
        .p_4()
        .child(top)
        .child(sections)
        .into_any_element()
}

/// One bordered section card of the provider detail: icon + title + rule,
/// then the body.
fn detail_section(
    theme: &Theme,
    icon: AssetIcon,
    title_key: &'static str,
    body: Vec<AnyElement>,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_3()
        .p_4()
        .rounded(px(12.))
        .border_1()
        .border_color(theme.border.opacity(0.65))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap_2()
                .child(Icon::new(icon).size_4().text_color(theme.info))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(i18n::t(title_key)),
                )
                .child(div().flex_1().h(px(1.)).bg(theme.border.opacity(0.5))),
        )
        .children(body)
        .into_any_element()
}

/// A flat label/control row used inside a detail section (the section card
/// supplies the padding, so this one adds none of its own).
fn detail_field_row(theme: &Theme, label: SharedString, control: AnyElement) -> AnyElement {
    h_flex()
        .w_full()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(LABEL_W))
                .flex_shrink_0()
                .text_sm()
                .text_color(theme.foreground)
                .child(label),
        )
        .child(h_flex().flex_1().min_w_0().child(control))
        .into_any_element()
}

/// The detail's 删除 Provider control: a labeled two-step button (first click
/// arms it with a danger tint, the second deletes and returns to the list).
fn delete_provider_button(
    theme: &Theme,
    pid: usize,
    pending: bool,
    entity: Entity<SettingsView>,
) -> AnyElement {
    let key = format!("models-p{pid}-remove");
    let label = if pending {
        i18n::t("settings-models-delete-confirm")
    } else {
        i18n::t("settings-models-delete-hint")
    };
    let click_key = key.clone();
    div()
        .id(key)
        .h(px(30.))
        .px_2()
        .flex()
        .items_center()
        .gap_1p5()
        .rounded(theme.radius)
        .text_size(px(13.))
        .cursor_pointer()
        .when(pending, |this| {
            this.bg(theme.danger.opacity(0.12)).text_color(theme.danger)
        })
        .when(!pending, |this| {
            this.text_color(theme.muted_foreground)
                .hover(|this| this.bg(theme.secondary).text_color(theme.danger))
        })
        .on_click(move |_ev, _window, cx| {
            entity.update(cx, |this, cx| {
                if this.models_panel.pending_delete.as_deref() == Some(click_key.as_str()) {
                    this.models_panel.pending_delete = None;
                    this.models_panel.remove_provider(pid);
                    this.models_panel.touch(cx);
                } else {
                    this.models_panel.pending_delete = Some(click_key.clone());
                }
                cx.notify();
            });
        })
        .child(Icon::new(AssetIcon::Trash).size_4())
        .child(label)
        .into_any_element()
}

/// 基本信息 API Key control: kind dropdown + value input.
fn render_apikey_control(entity: Entity<SettingsView>, pid: usize, p: &ProviderForm) -> AnyElement {
    let kind_options: Vec<(SharedString, SharedString)> = [
        ApiKeySourceKind::Literal,
        ApiKeySourceKind::Env,
        ApiKeySourceKind::Keychain,
        ApiKeySourceKind::Shell,
    ]
    .iter()
    .map(|k| (kind_label(*k), SharedString::from(kind_token(*k))))
    .collect();

    h_flex()
        .w_full()
        .gap_2()
        .child(token_dropdown(
            format!("models-p{pid}-apikey-kind"),
            SharedString::from(kind_token(p.apikey_kind)),
            kind_options,
            entity.clone(),
            Arc::new(move |this, v, window, cx| {
                if let Some(p) = this.models_panel.provider_mut(pid) {
                    p.apikey_kind = kind_from_token(&v);
                    let placeholder = kind_placeholder(p.apikey_kind);
                    p.apikey_value.update(cx, |state, cx| {
                        state.set_placeholder(placeholder, window, cx)
                    });
                    this.models_panel.touch(cx);
                }
            }),
        ))
        .child(
            div()
                .id(format!(
                    "settings-input-blur-{:?}",
                    p.apikey_value.entity_id()
                ))
                .flex_1()
                .min_w_0()
                .on_mouse_down_out(blur_on_click_out(p.apikey_value.clone()))
                .child(Input::new(&p.apikey_value)),
        )
        .into_any_element()
}

/// 环境变量 key/value rows: each row trailed by 「-」(delete row) and
/// 「+」(insert a focused empty row below). With no rows a muted hint plus a
/// 「+」 offers the first row. The caller supplies the section heading.
fn env_rows(
    theme: &Theme,
    entity: Entity<SettingsView>,
    pid: usize,
    mid: Option<usize>,
    rows: &[KvForm],
    pending: &Option<String>,
) -> AnyElement {
    let mut children: Vec<AnyElement> = Vec::new();

    if rows.is_empty() {
        children.push(
            h_flex()
                .py_1()
                .items_center()
                .gap_2()
                .child(muted_text(
                    i18n::t("settings-models-env-empty"),
                    theme.muted_foreground,
                ))
                .child(icon_button(
                    format!("models-p{pid}-m{mid:?}-env-add-first"),
                    IconName::Plus,
                    entity.clone(),
                    Arc::new(move |this, window, cx| {
                        this.models_panel.add_env_row(window, cx, pid, mid, None);
                        this.models_panel.touch(cx);
                    }),
                ))
                .into_any_element(),
        );
    }

    for (ix, kv) in rows.iter().enumerate() {
        let kid = kv.id;
        children.push(
            h_flex()
                .py_1()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id(format!("settings-input-blur-{:?}", kv.key.entity_id()))
                        .w(px(KEY_W))
                        .flex_shrink_0()
                        .on_mouse_down_out(blur_on_click_out(kv.key.clone()))
                        .child(Input::new(&kv.key)),
                )
                .child(
                    div()
                        .id(format!("settings-input-blur-{:?}", kv.value.entity_id()))
                        .flex_1()
                        .min_w_0()
                        .on_mouse_down_out(blur_on_click_out(kv.value.clone()))
                        .child(Input::new(&kv.value)),
                )
                .child({
                    let key = format!("models-kv{kid}-remove");
                    let is_pending = pending.as_deref() == Some(key.as_str());
                    remove_button(
                        theme,
                        key,
                        is_pending,
                        entity.clone(),
                        Arc::new(move |this, cx| {
                            this.models_panel.remove_env_row(pid, mid, kid);
                            this.models_panel.touch(cx);
                        }),
                    )
                })
                .child(icon_button(
                    format!("models-kv{kid}-add-below"),
                    IconName::Plus,
                    entity.clone(),
                    Arc::new(move |this, window, cx| {
                        this.models_panel
                            .add_env_row(window, cx, pid, mid, Some(ix));
                        this.models_panel.touch(cx);
                    }),
                ))
                .into_any_element(),
        );
    }
    v_flex()
        .w_full()
        .gap_1()
        .children(children)
        .into_any_element()
}

/// 环境变量 block inside a model card: a small heading over the shared row
/// list.
fn render_env_block(
    theme: &Theme,
    entity: Entity<SettingsView>,
    pid: usize,
    mid: Option<usize>,
    rows: &[KvForm],
    pending: &Option<String>,
) -> AnyElement {
    v_flex()
        .w_full()
        .px_3()
        .py_1p5()
        .gap_1()
        .child(
            div()
                .text_sm()
                .text_color(theme.foreground)
                .child(i18n::t("settings-models-section-env")),
        )
        .child(env_rows(theme, entity, pid, mid, rows, pending))
        .into_any_element()
}

/// 端点信息 module: add button first (hidden once all three wire APIs are
/// taken), then one card per endpoint.
fn render_endpoints_module(
    theme: &Theme,
    entity: Entity<SettingsView>,
    pid: usize,
    p: &ProviderForm,
    agent_options: &[String],
    pending: &Option<String>,
) -> AnyElement {
    let used: HashSet<String> = p.endpoints.iter().map(|e| e.wire_api.to_string()).collect();

    let mut children: Vec<AnyElement> = Vec::new();
    for e in &p.endpoints {
        children.push(render_endpoint(
            theme,
            entity.clone(),
            pid,
            &used,
            agent_options,
            e,
            pending,
        ));
    }
    if used.len() < WIRE_APIS.len() {
        children.push(dashed_add_button(
            theme,
            format!("models-p{pid}-add-endpoint"),
            "settings-models-add-endpoint",
            entity.clone(),
            Arc::new(move |this, window, cx| {
                this.models_panel.add_endpoint(window, cx, pid);
                this.models_panel.touch(cx);
            }),
        ));
    }
    v_flex()
        .w_full()
        .gap_2()
        .children(children)
        .into_any_element()
}

fn render_endpoint(
    theme: &Theme,
    entity: Entity<SettingsView>,
    pid: usize,
    used: &HashSet<String>,
    agent_options: &[String],
    e: &EndpointForm,
    pending: &Option<String>,
) -> AnyElement {
    let eid = e.id;
    let current = e.wire_api.to_string();
    // Each wire API may appear at most once per provider: a row offers its
    // own kind plus the kinds no other row has taken.
    let wire_options: Vec<(SharedString, SharedString)> = WIRE_APIS
        .iter()
        .filter(|w| **w == current || !used.contains(**w))
        .map(|w| (SharedString::from(wire_display(w)), SharedString::from(*w)))
        .collect();
    let copilot_options: Vec<(SharedString, SharedString)> = vec![
        (
            i18n::t("settings-models-value-unset"),
            SharedString::default(),
        ),
        (SharedString::from("api_key"), SharedString::from("api_key")),
        (
            SharedString::from("bearer_token"),
            SharedString::from("bearer_token"),
        ),
    ];

    let wire_control = wire_dropdown(
        theme,
        format!("models-e{eid}-wire"),
        e.wire_api.clone(),
        wire_options,
        entity.clone(),
        Arc::new(move |this, v, _window, cx| {
            if let Some(e) = this.models_panel.endpoint_mut(pid, eid) {
                e.wire_api = v;
                this.models_panel.touch(cx);
            }
        }),
    );

    let mut rows = vec![
        field_row(
            theme,
            i18n::t("settings-models-row-wire-apis"),
            wire_control,
        ),
        field_row(
            theme,
            i18n::t("settings-models-row-url"),
            input_field(&e.url),
        ),
        field_row(
            theme,
            i18n::t("settings-models-row-agents"),
            agent_badges(
                format!("models-e{eid}-agents"),
                agent_options,
                &e.agents,
                entity.clone(),
                Arc::new(move |this, v, cx| {
                    if let Some(e) = this.models_panel.endpoint_mut(pid, eid) {
                        e.agents = v;
                        this.models_panel.touch(cx);
                    }
                }),
            ),
        ),
    ];
    if agents_include_copilot(&e.agents) {
        rows.push(field_row(
            theme,
            i18n::t("settings-models-row-copilot"),
            token_dropdown(
                format!("models-e{eid}-copilot"),
                e.copilot_auth.clone(),
                copilot_options,
                entity.clone(),
                Arc::new(move |this, v, _window, cx| {
                    if let Some(e) = this.models_panel.endpoint_mut(pid, eid) {
                        e.copilot_auth = v;
                        this.models_panel.touch(cx);
                    }
                }),
            ),
        ));
    }
    let card = plain_card(theme, rows);
    let key = format!("models-e{eid}-remove");
    let is_pending = pending.as_deref() == Some(key.as_str());
    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .child(div().flex_1().min_w_0().child(card))
        .child(remove_button(
            theme,
            key,
            is_pending,
            entity.clone(),
            Arc::new(move |this, cx| {
                this.models_panel.remove_endpoint(pid, eid);
                this.models_panel.touch(cx);
            }),
        ))
        .into_any_element()
}

/// 模型列表 module: 手动配置 / 自动获取 tab; manual mode renders one card per
/// model (blocks, not hairline-separated rows).
fn render_models_module(
    theme: &Theme,
    entity: Entity<SettingsView>,
    pid: usize,
    p: &ProviderForm,
    agent_options: &[String],
    pending: &Option<String>,
) -> AnyElement {
    let mut children: Vec<AnyElement> = vec![render_mode_tabs(theme, entity.clone(), pid, p)];
    if p.remote_models {
        children.push(field_row(
            theme,
            i18n::t("settings-models-row-url"),
            input_field(&p.remote_url),
        ));
    } else {
        if p.models.is_empty() {
            children.push(
                h_flex()
                    .px_3()
                    .py_2()
                    .child(muted_text(
                        i18n::t("settings-models-empty-models"),
                        theme.muted_foreground,
                    ))
                    .into_any_element(),
            );
        }
        for m in &p.models {
            children.push(render_model(
                theme,
                entity.clone(),
                pid,
                m,
                agent_options,
                pending,
            ));
        }
        children.push(dashed_add_button(
            theme,
            format!("models-p{pid}-add-model"),
            "settings-models-add-model",
            entity.clone(),
            Arc::new(move |this, window, cx| {
                this.models_panel.add_model(window, cx, pid);
                this.models_panel.touch(cx);
            }),
        ));
    }
    v_flex()
        .w_full()
        .gap_2()
        .children(children)
        .into_any_element()
}

/// Two-choice tab replacing the old 内联定义 / 远程 URL button pair; the
/// active tab is underlined and tinted with `theme.info` so it reads clearly.
fn render_mode_tabs(
    theme: &Theme,
    entity: Entity<SettingsView>,
    pid: usize,
    p: &ProviderForm,
) -> AnyElement {
    let muted = theme.muted_foreground;
    let mut tabs = Vec::new();
    for (ix, (active, label_key)) in [
        (!p.remote_models, "settings-models-mode-inline"),
        (p.remote_models, "settings-models-mode-remote"),
    ]
    .into_iter()
    .enumerate()
    {
        let tab_entity = entity.clone();
        let remote = ix == 1;
        let tab = div()
            .id(format!("models-p{pid}-mode-{ix}"))
            .px_2()
            .py_1()
            .border_b_2()
            .cursor_pointer()
            .text_sm();
        let tab = if active {
            tab.text_color(theme.info)
                .border_color(theme.info)
                .font_weight(gpui::FontWeight::MEDIUM)
        } else {
            tab.text_color(muted)
                .border_color(gpui::transparent_black())
                .hover(|s| s.text_color(theme.foreground))
        };
        tabs.push(
            tab.on_click(move |_ev, _window, cx| {
                tab_entity.update(cx, |this, cx| {
                    if let Some(p) = this
                        .models_panel
                        .provider_mut(pid)
                        .filter(|p| p.remote_models != remote)
                    {
                        p.remote_models = remote;
                        this.models_panel.touch(cx);
                    }
                    cx.notify();
                });
            })
            .child(i18n::t(label_key))
            .into_any_element(),
        );
    }
    h_flex()
        .px_3()
        .py_1()
        .gap_2()
        .children(tabs)
        .into_any_element()
}

fn render_model(
    theme: &Theme,
    entity: Entity<SettingsView>,
    pid: usize,
    m: &ModelForm,
    agent_options: &[String],
    pending: &Option<String>,
) -> AnyElement {
    let mid = m.id;

    let card = plain_card(
        theme,
        vec![
            field_row(
                theme,
                i18n::t("settings-models-row-model-id"),
                input_field(&m.model_id),
            ),
            field_row(
                theme,
                i18n::t("settings-models-row-desc"),
                input_field(&m.desc),
            ),
            field_row(
                theme,
                i18n::t("settings-models-row-context"),
                input_field(&m.context),
            ),
            field_row(
                theme,
                i18n::t("settings-models-row-max-tokens"),
                input_field(&m.max_tokens),
            ),
            field_row(
                theme,
                i18n::t("settings-models-row-wire-apis"),
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(wire_checkbox(
                        entity.clone(),
                        format!("models-m{mid}-wire-anthropic"),
                        "anthropic",
                        m.wire_anthropic,
                        pid,
                        mid,
                    ))
                    .child(wire_checkbox(
                        entity.clone(),
                        format!("models-m{mid}-wire-responses"),
                        "responses",
                        m.wire_responses,
                        pid,
                        mid,
                    ))
                    .child(wire_checkbox(
                        entity.clone(),
                        format!("models-m{mid}-wire-completions"),
                        "completions",
                        m.wire_completions,
                        pid,
                        mid,
                    ))
                    .into_any_element(),
            ),
            field_row(
                theme,
                i18n::t("settings-models-row-agents"),
                agent_badges(
                    format!("models-m{mid}-agents"),
                    agent_options,
                    &m.agents,
                    entity.clone(),
                    Arc::new(move |this, v, cx| {
                        if let Some(m) = this.models_panel.model_mut(pid, mid) {
                            m.agents = v;
                            this.models_panel.touch(cx);
                        }
                    }),
                ),
            ),
            field_row(
                theme,
                i18n::t("settings-models-row-supports-tools"),
                bool_dropdown(
                    format!("models-m{mid}-tools"),
                    m.supports_tools,
                    entity.clone(),
                    Arc::new(move |this, v, _window, cx| {
                        if let Some(m) = this.models_panel.model_mut(pid, mid) {
                            m.supports_tools = v == "true";
                            this.models_panel.touch(cx);
                        }
                    }),
                ),
            ),
            field_row(
                theme,
                i18n::t("settings-models-row-supports-images"),
                bool_dropdown(
                    format!("models-m{mid}-images"),
                    m.supports_images,
                    entity.clone(),
                    Arc::new(move |this, v, _window, cx| {
                        if let Some(m) = this.models_panel.model_mut(pid, mid) {
                            m.supports_images = v == "true";
                            this.models_panel.touch(cx);
                        }
                    }),
                ),
            ),
            render_env_block(theme, entity.clone(), pid, Some(mid), &m.env, pending),
        ],
    );
    let key = format!("models-m{mid}-remove");
    let is_pending = pending.as_deref() == Some(key.as_str());
    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .child(div().flex_1().min_w_0().child(card))
        .child(remove_button(
            theme,
            key,
            is_pending,
            entity.clone(),
            Arc::new(move |this, cx| {
                this.models_panel.remove_model(pid, mid);
                this.models_panel.touch(cx);
            }),
        ))
        .into_any_element()
}

// --- Element builders -----------------------------------------------------

fn plain_card(theme: &Theme, children: Vec<AnyElement>) -> AnyElement {
    v_flex()
        .w_full()
        .p_2()
        .gap_0()
        .rounded(px(10.))
        .border_1()
        .border_color(theme.border.opacity(0.5))
        .children(children)
        .into_any_element()
}

/// Label-left / control-right row for form fields; every module row goes
/// through here so labels and controls share one grid.
fn field_row(theme: &Theme, label: SharedString, control: AnyElement) -> AnyElement {
    h_flex()
        .w_full()
        .items_center()
        .gap_3()
        .px_3()
        .py_1p5()
        .child(
            div()
                .w(px(LABEL_W))
                .flex_shrink_0()
                .text_sm()
                .text_color(theme.foreground)
                .child(label),
        )
        .child(h_flex().flex_1().min_w_0().child(control))
        .into_any_element()
}

fn input_field(state: &Entity<InputState>) -> AnyElement {
    div()
        .id(format!("settings-input-blur-{:?}", state.entity_id()))
        .w_full()
        .on_mouse_down_out(blur_on_click_out(state.clone()))
        .child(Input::new(state))
        .into_any_element()
}

/// Dropdown over `(display, token)` pairs; the selected token is applied via
/// `apply` and only the display value flips on the button label.
fn token_dropdown(
    id: impl Into<gpui::ElementId>,
    current: SharedString,
    options: Vec<(SharedString, SharedString)>,
    entity: Entity<SettingsView>,
    apply: TokenApply,
) -> AnyElement {
    let label = options
        .iter()
        .find(|(_, token)| *token == current)
        .map(|(display, _)| display.clone())
        .unwrap_or(current);
    Button::new(id)
        .label(label)
        .dropdown_caret(true)
        .outline()
        .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _window, _cx| {
            options.iter().fold(menu, |menu, (display, token)| {
                let token = token.clone();
                let entity = entity.clone();
                let apply = apply.clone();
                menu.item(
                    PopupMenuItem::new(display.clone()).on_click(move |_ev, window, cx| {
                        entity.update(cx, |this, cx| {
                            apply(this, token.clone(), window, cx);
                            cx.notify();
                        });
                    }),
                )
            })
        })
        .into_any_element()
}

/// The config endpoint map's wire key (`responses` / `completions`) vs. the
/// canonical model-catalog api key the picker palette is keyed by.
fn wire_color_name(wire: &str) -> gpui_component::ColorName {
    let api = match wire {
        "responses" => "openai_responses",
        "completions" => "openai_completions",
        other => other,
    };
    crate::model_catalog::wire_visual(api).2
}

/// The wire api's dedicated tint at the picker tag's scale (600 light / 300
/// dark); unknown keys fall back to muted.
fn wire_tint(theme: &Theme, wire: &str) -> gpui::Hsla {
    match wire_color_name(wire) {
        gpui_component::ColorName::Gray => theme.muted_foreground,
        color => color.scale(if theme.is_dark() { 300 } else { 600 }),
    }
}

/// The Wire API dropdown: the shared token dropdown with the api's dedicated
/// color on its dot, label and border.
fn wire_dropdown(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    current: SharedString,
    options: Vec<(SharedString, SharedString)>,
    entity: Entity<SettingsView>,
    apply: TokenApply,
) -> AnyElement {
    let label = options
        .iter()
        .find(|(_, token)| *token == current)
        .map(|(display, _)| display.clone())
        .unwrap_or(current.clone());
    let tint = wire_tint(theme, &current);
    Button::new(id)
        .outline()
        .dropdown_caret(true)
        .border_color(tint.opacity(0.45))
        .child(
            h_flex()
                .items_center()
                .gap_1p5()
                .child(div().size(px(8.)).flex_none().rounded_full().bg(tint))
                .child(
                    div()
                        .text_color(tint)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(label),
                ),
        )
        .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _window, _cx| {
            options.iter().fold(menu, |menu, (display, token)| {
                let token = token.clone();
                let display = display.clone();
                let entity = entity.clone();
                let apply = apply.clone();
                menu.item(
                    PopupMenuItem::element({
                        let token = token.clone();
                        move |_window, cx| {
                            // Each row wears its own api color, so the open
                            // menu reads exactly like the selected control.
                            let tint = wire_tint(cx.theme(), &token);
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .child(div().size(px(8.)).flex_none().rounded_full().bg(tint))
                                .child(div().text_color(tint).child(display.clone()))
                        }
                    })
                    .on_click(move |_ev, window, cx| {
                        entity.update(cx, |this, cx| {
                            apply(this, token.clone(), window, cx);
                            cx.notify();
                        });
                    }),
                )
            })
        })
        .into_any_element()
}

/// true/false dropdown for the effective supports_tools / supports_images
/// value (unset config resolves to the documented default at load time).
fn bool_dropdown(
    id: impl Into<gpui::ElementId>,
    current: bool,
    entity: Entity<SettingsView>,
    apply: TokenApply,
) -> AnyElement {
    let options: Vec<(SharedString, SharedString)> = vec![
        (SharedString::from("true"), SharedString::from("true")),
        (SharedString::from("false"), SharedString::from("false")),
    ];
    token_dropdown(
        id,
        SharedString::from(if current { "true" } else { "false" }),
        options,
        entity,
        apply,
    )
}

fn wire_checkbox(
    entity: Entity<SettingsView>,
    id: impl Into<gpui::ElementId>,
    wire: &'static str,
    checked: bool,
    pid: usize,
    mid: usize,
) -> AnyElement {
    Checkbox::new(id)
        .label(wire_display(wire))
        .checked(checked)
        .on_click(move |&new_val, _window, cx| {
            entity.update(cx, |this, cx| {
                if let Some(m) = this.models_panel.model_mut(pid, mid) {
                    match wire {
                        "anthropic" => m.wire_anthropic = new_val,
                        "responses" => m.wire_responses = new_val,
                        "completions" => m.wire_completions = new_val,
                        // Checkboxes are only constructed for `WIRE_APIS`.
                        _ => {}
                    }
                    this.models_panel.touch(cx);
                }
                cx.notify();
            });
        })
        .into_any_element()
}

/// Selected agents as removable badges plus a dropdown that adds the
/// remaining candidates — replaces the old click-to-toggle chip row.
fn agent_badges(
    id_prefix: String,
    options: &[String],
    selected: &[String],
    entity: Entity<SettingsView>,
    apply: TagsApply,
) -> AnyElement {
    let mut all: Vec<String> = options.to_vec();
    for tag in selected {
        if !all.contains(tag) {
            all.push(tag.clone());
        }
    }

    let mut children: Vec<AnyElement> = Vec::new();
    for tag in selected {
        let tag_clone = tag.clone();
        let selected_vec = selected.to_vec();
        let entity = entity.clone();
        let apply = apply.clone();
        children.push(
            h_flex()
                .items_center()
                .gap_1()
                .pl_2()
                .pr_1()
                .py_0p5()
                .rounded(px(6.))
                .bg(gpui::hsla(0., 0., 0.5, 0.08))
                .text_xs()
                .child(SharedString::from(agent_display_name(tag)))
                .child(
                    Button::new(format!("{id_prefix}-rm-{tag}"))
                        .icon(Icon::new(IconName::Close).small())
                        .ghost()
                        .small()
                        .on_click(move |_ev, _window, cx| {
                            let next: Vec<String> = selected_vec
                                .iter()
                                .filter(|t| *t != &tag_clone)
                                .cloned()
                                .collect();
                            entity.update(cx, |this, cx| {
                                apply(this, next.clone(), cx);
                                cx.notify();
                            });
                        }),
                )
                .into_any_element(),
        );
    }

    let remaining: Vec<String> = all
        .iter()
        .filter(|t| !selected.contains(*t))
        .cloned()
        .collect();
    let add_button = Button::new(format!("{id_prefix}-add"))
        .label(i18n::t("settings-models-agents-add"))
        .small()
        .outline()
        .dropdown_caret(true);
    let selected_vec = selected.to_vec();
    children.push(
        add_button
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _window, _cx| {
                remaining.iter().fold(menu, |menu, tag| {
                    let tag = tag.clone();
                    let entity = entity.clone();
                    let apply = apply.clone();
                    let selected_vec = selected_vec.clone();
                    menu.item(
                        PopupMenuItem::new(SharedString::from(agent_display_name(&tag))).on_click(
                            move |_ev, _window, cx| {
                                let mut next = selected_vec.clone();
                                next.push(tag.clone());
                                entity.update(cx, |this, cx| {
                                    apply(this, next.clone(), cx);
                                    cx.notify();
                                });
                            },
                        ),
                    )
                })
            })
            .into_any_element(),
    );
    if selected.is_empty() {
        children.push(muted_text(
            i18n::t("settings-models-agents-all-hint"),
            gpui::transparent_black(),
        ));
    }
    h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap_1()
        .children(children)
        .into_any_element()
}

fn icon_button(
    id: impl Into<gpui::ElementId>,
    icon: IconName,
    entity: Entity<SettingsView>,
    apply: WindowApply,
) -> AnyElement {
    Button::new(id)
        .icon(Icon::new(icon))
        .small()
        .ghost()
        .on_click(move |_ev, window, cx| {
            entity.update(cx, |this, cx| {
                apply(this, window, cx);
                cx.notify();
            });
        })
        .into_any_element()
}

fn dashed_add_button(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    label_key: &'static str,
    entity: Entity<SettingsView>,
    apply: WindowApply,
) -> AnyElement {
    let border = theme.border.opacity(0.9);
    let muted = theme.muted_foreground;
    let accent = theme.accent;
    div()
        .id(id)
        .w_full()
        .py_2()
        .rounded(px(8.))
        .border_1()
        .border_dashed()
        .border_color(border)
        .cursor_pointer()
        .hover(move |s| s.border_color(accent).text_color(accent))
        .on_click(move |_ev, window, cx| {
            entity.update(cx, |this, cx| {
                apply(this, window, cx);
                cx.notify();
            });
        })
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_center()
                .gap_1()
                .text_sm()
                .text_color(muted)
                .child(Icon::new(IconName::Plus).small())
                .child(i18n::t(label_key)),
        )
        .into_any_element()
}

/// Uniform 「-」 remove control with two-step confirmation: the first click
/// arms it (danger tint), the second executes the deletion.
fn remove_button(
    theme: &Theme,
    id: String,
    pending: bool,
    entity: Entity<SettingsView>,
    apply: MutApply,
) -> AnyElement {
    let key = id.clone();
    let icon = Icon::new(IconName::Minus);
    let icon = if pending {
        icon.text_color(theme.danger)
    } else {
        icon.text_color(theme.muted_foreground)
    };
    let button = Button::new(id).icon(icon).ghost();
    let button = if pending {
        button.bg(theme.danger.opacity(0.12))
    } else {
        button
    };
    button
        .on_click(move |_ev, _window, cx| {
            entity.update(cx, |this, cx| {
                if this.models_panel.pending_delete.as_deref() == Some(key.as_str()) {
                    this.models_panel.pending_delete = None;
                    apply(this, cx);
                } else {
                    this.models_panel.pending_delete = Some(key.clone());
                }
                cx.notify();
            });
        })
        .into_any_element()
}

/// Carry over top-level sections this form does not model (owned by the
/// External Tools panels and the host's subagent dispatch) from a fresh disk
/// read, so a Models-panel save can never erase them.
fn carry_over_unmodeled_sections(config: &mut CxConfig, fresh: &CxConfig) {
    config.chatgpt_app = fresh.chatgpt_app.clone();
    config.vscode_app = fresh.vscode_app.clone();
    config.subagents = fresh.subagents.clone();
}

#[cfg(test)]
mod tests {
    use super::{carry_over_unmodeled_sections, wire_color_name};
    use gpui_component::ColorName;
    use manox_providers::{ChatGptAppSettings, CxConfig, VsCodeAppSettings, VsCodeExtensionBlock};

    /// The endpoint map's wire keys must reuse the picker's dedicated palette
    /// (Blue / Cyan / Amber), not fall through to the unknown-API gray.
    #[test]
    fn endpoint_wire_keys_map_to_the_picker_palette() {
        assert_eq!(wire_color_name("anthropic"), ColorName::Blue);
        assert_eq!(wire_color_name("responses"), ColorName::Cyan);
        assert_eq!(wire_color_name("completions"), ColorName::Amber);
        // Canonical catalog keys keep working too.
        assert_eq!(wire_color_name("openai_responses"), ColorName::Cyan);
        assert_eq!(wire_color_name("openai_completions"), ColorName::Amber);
        assert_eq!(wire_color_name("unknown"), ColorName::Gray);
    }

    /// Regression: the CxConfig produced by the Models panel's `collect()`
    /// carries empty values for all three unmodeled sections; the carry-over
    /// must restore `chatgpt_app` / `vscode_app` / `subagents` from the fresh
    /// disk read, or autosave silently erases the External Tools panels'
    /// settings and the host's subagent model pinning.
    #[test]
    fn carry_over_preserves_unmodeled_sections() {
        let mut collected = CxConfig::default();
        let fresh = CxConfig {
            providers: Vec::new(),
            agents: Vec::new(),
            chatgpt_app: Some(ChatGptAppSettings::default()),
            vscode_app: Some(VsCodeAppSettings {
                claude_code: VsCodeExtensionBlock {
                    provider: Some("百炼".into()),
                    disabled: false,
                },
                codex: VsCodeExtensionBlock {
                    provider: None,
                    disabled: true,
                },
            }),
            subagents: [(
                "Explore".to_string(),
                "百炼::qwen3.8-flash::high".to_string(),
            )]
            .into_iter()
            .collect(),
        };
        carry_over_unmodeled_sections(&mut collected, &fresh);
        assert!(collected.chatgpt_app.is_some());
        let vscode = collected
            .vscode_app
            .expect("vscode_app must survive carry-over");
        assert_eq!(vscode.claude_code.provider.as_deref(), Some("百炼"));
        assert!(vscode.codex.disabled);
        assert_eq!(
            collected.subagents.get("Explore").map(String::as_str),
            Some("百炼::qwen3.8-flash::high"),
            "subagents must survive carry-over"
        );
    }
}

#[cfg(test)]
mod blur_flush_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{AppContext as _, Entity, ParentElement as _, Render, TestAppContext, div};
    use gpui_component::Root;
    use gpui_component::input::{Input, InputEvent, InputState};

    struct ProbeView {
        input: Entity<InputState>,
    }

    impl Render for ProbeView {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            div().child(Input::new(&self.input))
        }
    }

    /// The settings panels flush pending edits when an input blurs. The flush
    /// is wired in an `InputEvent::Blur` subscription, whose dispatch leases
    /// the view entity for the whole callback; the flush itself needs
    /// `&mut Window`, which subscriptions do not receive, so
    /// `panels::defer_blur_flush` re-enters the view through the saved window
    /// handle at the end of the effect cycle, after the dispatch lease is
    /// released (a synchronous re-entry would double-lease the entity and
    /// panic). This test drives a real focus→blur cycle through gpui's draw
    /// focus phase and pins that deferred re-entry behavior.
    #[gpui::test]
    fn blur_flush_reentry_runs_after_focus_loss(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let flushed = Rc::new(Cell::new(false));
        let flushed_for_view = flushed.clone();
        let slot: Rc<std::cell::RefCell<Option<Entity<ProbeView>>>> = Default::default();
        let slot_for_window = slot.clone();
        let (_root, cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|cx| {
                let window_handle = window.window_handle();
                let input = cx.new(|cx| InputState::new(window, cx));
                // Same shape as the `new_input`/`panel_input` Blur arm.
                let flushed = flushed_for_view.clone();
                cx.subscribe(&input, move |_this, _state, event, cx| {
                    if matches!(event, InputEvent::Blur) {
                        let flushed = flushed.clone();
                        super::super::panels::defer_blur_flush(
                            cx,
                            window_handle,
                            move |_this, _window, _cx| {
                                flushed.set(true);
                            },
                        );
                    }
                })
                .detach();
                ProbeView { input }
            });
            *slot_for_window.borrow_mut() = Some(view.clone());
            Root::new(view, window, cx)
        });
        let view = slot.borrow().clone().expect("view initialized");
        let input = view.read_with(cx, |view, _| view.input.clone());

        // Focus change events only dispatch while the window is active.
        cx.update(|window, _cx| window.activate_window());
        cx.run_until_parked();

        cx.update(|window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
        });
        cx.run_until_parked();

        cx.update(|window, cx| window.blur(cx));
        cx.run_until_parked();

        assert!(
            flushed.get(),
            "deferred blur flush never ran after focus loss"
        );
    }
}
