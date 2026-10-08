//! The app-layer model catalogue: the provider registry read directly
//! (in-process, the same source the stream resolver matches against).
//!
//! The AHP root catalogue exists for remote clients; the desktop's own
//! picker needs the registry's richer columns (wire api, cx config key,
//! agents visibility, provider display name) that have no native AHP slot.
//! Display-side registry reads are the sanctioned pattern (the context
//! rail's per-model rows resolve the same way).

use manox_agent::provider_glue;

/// One model row across all registered provider endpoints.
#[derive(Debug, Clone)]
pub struct ModelRow {
    /// Registration key: `"{provider}-{wire_api}"`, unique per endpoint.
    pub provider: String,
    /// The cx CONFIG provider name behind the registration (the config
    /// entry's key, e.g. `zhipu` behind the registration `zhipu-anthropic`)
    /// — the launch APIs' provider argument (`AgentBuilder::provider`
    /// matches `providers_for_agent` names against THIS, never the display
    /// name). Falls back to the registration name when the registry entry
    /// carries no config name.
    pub cx_name: String,
    /// The provider's human display name (picker submenu label).
    pub provider_display: String,
    /// The CONFIG-level model id (metadata `config_id` — the raw id as
    /// written in the cx yaml, context suffixes included, e.g.
    /// `glm-5.3[1m]`). The registry's own `id` is the PARSED id (suffix
    /// stripped), which never matches the launch APIs' config-side
    /// vocabulary — every spawn argument rides THIS field.
    pub config_id: String,
    /// The agent ids this model is visible to (registration-time
    /// `effective_agents`, canonicalized). Pickers scoped to one agent
    /// filter on it; `["*"]`-style entries are expanded at registration.
    pub agents: Vec<String>,
    /// Bare model id (e.g. `glm-5.2`).
    pub id: String,
    /// Display name (metadata `name`, else the id).
    pub name: String,
    /// Wire api (`anthropic` / `openai_responses` / `openai_completions`).
    pub api: String,
}

/// Every registered model row, sorted by provider then id (registry order).
pub fn rows() -> Vec<ModelRow> {
    let registry = provider_glue::global();
    registry
        .models()
        .into_iter()
        .map(|m| {
            let cx_name = registry
                .provider_config(&m.provider)
                .and_then(|c| c.name)
                .unwrap_or_else(|| m.provider.clone());
            let agents = m
                .metadata
                .get("agents")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            ModelRow {
                cx_name,
                provider_display: provider_glue::display_provider_name(&m),
                config_id: provider_glue::config_id(&m),
                agents,
                name: provider_glue::display_name(&m),
                provider: m.provider.clone(),
                id: m.id.clone(),
                api: m.api.clone(),
            }
        })
        .collect()
}

/// Exact registration match of a canonical `{provider}/{id}` identity.
pub fn resolve(provider: &str, id: &str) -> Option<ModelRow> {
    rows()
        .into_iter()
        .find(|r| r.provider == provider && r.id == id)
}

/// The wire api → the picker row's visual vocabulary (tag variant + label +
/// the tint the composer chip reuses). Every surface that renders a wire
/// distinction reads this one mapping.
pub fn wire_visual(
    api: &str,
) -> (
    gpui_component::tag::TagVariant,
    &'static str,
    gpui_component::ColorName,
) {
    use gpui_component::ColorName;
    use gpui_component::tag::TagVariant;
    match api {
        "anthropic" => (
            TagVariant::Color(ColorName::Blue),
            "Anthropic",
            ColorName::Blue,
        ),
        "openai_responses" => (
            TagVariant::Color(ColorName::Cyan),
            "Responses",
            ColorName::Cyan,
        ),
        "openai_completions" => (
            TagVariant::Color(ColorName::Amber),
            "Completions",
            ColorName::Amber,
        ),
        _ => (TagVariant::Secondary, "N/A", ColorName::Gray),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The launch bridge's contract: `cx_name` resolves the cx CONFIG name
    /// behind a registration (`zhipu-anthropic` → `zhipu`), and falls back
    /// to the registration name when the entry carries none. The composer's
    /// canonical ref and the AgentBuilder provider argument both ride this —
    /// a regression here breaks every agent launch with "provider not
    /// found".
    #[test]
    fn cx_name_resolves_the_config_name_behind_a_registration() {
        // Deterministic seam: register into a LOCAL registry and install it
        // as the global. `install_for_test` freezes the slot — init()'s
        // background registration build can no longer swap a fresh registry
        // (built from the developer's real cx config) in over the test
        // provider, which is exactly the fd48c07 CI red — and the test never
        // reads the developer's config at all.
        let registry = std::sync::Arc::new(manox_harness::ProviderRegistry::new());
        registry
            .register_provider(
                "launch-bridge-test-anthropic",
                manox_harness::ProviderConfig {
                    name: Some("launch-bridge-test".into()),
                    base_url: Some("https://example.invalid".into()),
                    api: Some(manox_harness::provider_registry::Api::AnthropicMessages),
                    models: vec![manox_harness::provider_registry::ProviderModelConfig {
                        id: "bridge-model".into(),
                        name: "bridge-model".into(),
                        reasoning: false,
                        input: vec![],
                        context_window: 8_192,
                        max_tokens: 1_024,
                        cost: Default::default(),
                        api: None,
                        base_url: None,
                        metadata: Default::default(),
                    }],
                    ..Default::default()
                },
            )
            .expect("register");
        manox_agent::provider_glue::install_for_test(registry);
        let rows = rows();
        let bridged = rows
            .iter()
            .find(|r| r.provider == "launch-bridge-test-anthropic")
            .expect("the registered provider's rows appear");
        assert_eq!(bridged.cx_name, "launch-bridge-test");
    }
}
