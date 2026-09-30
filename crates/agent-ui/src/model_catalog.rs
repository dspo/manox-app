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
pub(crate) struct ModelRow {
    /// Registration key: `"{provider}-{wire_api}"`, unique per endpoint.
    pub provider: String,
    /// The provider's human display name (cx config `name`).
    pub provider_display: String,
    /// Bare model id (e.g. `glm-5.2`).
    pub id: String,
    /// Display name (metadata `name`, else the id).
    pub name: String,
    /// Wire api (`anthropic` / `openai_responses` / `openai_completions`).
    pub api: String,
}

/// Every registered model row, sorted by provider then id (registry order).
pub(crate) fn rows() -> Vec<ModelRow> {
    provider_glue::global()
        .models()
        .into_iter()
        .map(|m| ModelRow {
            provider_display: provider_glue::display_provider_name(&m),
            name: provider_glue::display_name(&m),
            provider: m.provider.clone(),
            id: m.id.clone(),
            api: m.api.clone(),
        })
        .collect()
}

/// Exact registration match of a canonical `{provider}/{id}` identity.
pub(crate) fn resolve(provider: &str, id: &str) -> Option<ModelRow> {
    rows()
        .into_iter()
        .find(|r| r.provider == provider && r.id == id)
}

/// The wire api → the picker row's visual vocabulary (tag variant + label +
/// the tint the composer chip reuses). Every surface that renders a wire
/// distinction reads this one mapping.
pub(crate) fn wire_visual(
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
