//! The model display identity every turn surface renders.
//!
//! The wire carries a canonical model id (`{registration}/{model}`, the
//! registration itself being `{name}-{wire_api}`, e.g.
//! `zhipu-anthropic/glm-5.3`). Rendering that string verbatim puts the wire
//! api on screen as a `-anthropic` suffix; the app's convention instead is
//! the provider's display name, a slash, the model's display name — with
//! the wire api expressed as a COLOR on the model segment (the same
//! vocabulary the context rail's bubble rows and the composer chip use).
//! [`ModelTag`] is the resolved form: built once at item construction
//! against the provider registry, never per frame.

use gpui::{Hsla, IntoElement as _, ParentElement as _, Styled as _};
use gpui_component::Theme;

use crate::views::context_rail::pi_wire_text_color;

/// A resolved model identity for display: provider display name, model
/// display name, and the wire api driving the model segment's tint.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelTag {
    /// The provider's human display name; `None` when the id did not
    /// resolve (the tag then renders the raw string, untinted).
    pub provider: Option<String>,
    /// The model's display name (metadata `name`, else the id).
    pub model: String,
    /// The wire api (`anthropic` / `openai_responses` /
    /// `openai_completions`); `None` leaves the model segment muted.
    pub wire_api: Option<String>,
}

impl ModelTag {
    /// Resolve a canonical wire identity against the provider registry. An
    /// id without a registration match (a removed provider, a foreign id,
    /// or a host-side fallback display string / localized "no model" label —
    /// the caller's id is not guaranteed to be wire-shaped) falls back to
    /// the raw string with no provider segment and no tint — display-only
    /// degradation, never an error path.
    pub fn resolve(wire_id: &str) -> Self {
        if wire_id.is_empty() {
            return Self::default();
        }
        let Some((provider, id)) = wire_id.split_once('/') else {
            return Self {
                provider: None,
                model: wire_id.to_string(),
                wire_api: None,
            };
        };
        // `global()` panics before `manox_agent::init` has run; a display
        // path must never panic — degrade to the raw string until the
        // registry exists (the app initializes it at startup, so this is
        // the test-before-init window only).
        let hit = std::panic::catch_unwind(manox_agent::provider_glue::global)
            .ok()
            .and_then(|registry| registry.resolve_model(provider, id));
        match hit {
            Some(m) => {
                let mut display = manox_agent::provider_glue::display_provider_name(&m);
                let model = {
                    let name = manox_agent::provider_glue::display_name(&m);
                    if !name.is_empty() { name } else { m.id.clone() }
                };
                if display == m.provider {
                    // The fallback is the registration key `{name}-{wire}`;
                    // the wire half reads as a color elsewhere — strip it so
                    // it never shows as a "-anthropic" text suffix.
                    let suffix = format!("-{}", m.api);
                    if let Some(stripped) = display.strip_suffix(&suffix) {
                        display = stripped.to_string();
                    }
                }
                Self {
                    provider: Some(display),
                    model,
                    wire_api: Some(m.api),
                }
            }
            None => Self {
                provider: None,
                model: wire_id.to_string(),
                wire_api: None,
            },
        }
    }

    /// The one-line text form: `{provider}/{model}` when resolved, the raw
    /// model string otherwise (the user-turn header is plain text and cannot
    /// carry the tint; the colored surfaces render `spans` instead).
    pub fn plain(&self) -> String {
        match self.provider.as_deref() {
            Some(p) if !p.is_empty() && !self.model.is_empty() => format!("{p}/{}", self.model),
            _ => self.model.clone(),
        }
    }

    /// The colored form: the provider in muted foreground, the model in the
    /// wire-api tint. Returns `(text, color)` runs; an unresolved tag is a
    /// single muted run.
    pub fn spans(&self, theme: &Theme) -> Vec<(String, Hsla)> {
        let muted = theme.muted_foreground;
        match self.provider.as_deref() {
            Some(p) if !p.is_empty() && !self.model.is_empty() => {
                let tint = pi_wire_text_color(self.wire_api.as_deref().unwrap_or(""), theme);
                vec![(format!("{p}/"), muted), (self.model.clone(), tint)]
            }
            _ => vec![(self.model.clone(), muted)],
        }
    }

    /// Whether the tag carries anything to show.
    pub fn is_empty(&self) -> bool {
        self.model.is_empty() && self.provider.is_none()
    }

    /// The colored one-line form every turn surface shares: `{provider}/` in
    /// muted foreground, the model in the wire-api tint. An unresolved tag
    /// renders its raw string muted.
    pub fn render(&self, theme: &Theme) -> gpui::AnyElement {
        gpui::div()
            .flex()
            .items_baseline()
            .min_w_0()
            .text_sm()
            .children(self.spans(theme).into_iter().map(|(text, color)| {
                gpui::div()
                    .min_w_0()
                    .truncate()
                    .text_color(color)
                    .child(text)
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fallback ladder: an empty id is an empty tag; an id with no
    /// registry match renders the raw string untinted with no provider
    /// segment; a resolved id carries provider + model + wire api. The
    /// registry read goes through the same global the app installs, so this
    /// only pins the pure fallbacks — the registry hit needs a live
    /// registration (agent-ui's `model_catalog` tests cover that seam).
    /// The registry hit: provider display name (registration suffix
    /// stripped when it fell back to the key), the model's display name,
    /// and the wire api that drives the tint. Runs behind test-support —
    /// `install_for_test` is the deterministic registry seam.
    #[cfg(feature = "test-support")]
    #[test]
    fn a_resolved_registration_carries_display_names_and_wire_api() {
        let registry = std::sync::Arc::new(manox_harness::ProviderRegistry::new());
        registry
            .register_provider(
                "zhipu-anthropic",
                manox_harness::ProviderConfig {
                    name: Some("智谱".into()),
                    base_url: Some("https://example.invalid".into()),
                    api: Some(manox_harness::provider_registry::Api::AnthropicMessages),
                    models: vec![manox_harness::provider_registry::ProviderModelConfig {
                        id: "glm-5.3".into(),
                        name: "GLM 5.3".into(),
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

        let tag = ModelTag::resolve("zhipu-anthropic/glm-5.3");
        assert_eq!(tag.provider.as_deref(), Some("智谱"));
        assert_eq!(tag.model, "GLM 5.3");
        assert_eq!(tag.wire_api.as_deref(), Some("anthropic"));
        assert_eq!(tag.plain(), "智谱/GLM 5.3");

        // A registration without a display name: the fallback is the key,
        // and the `-anthropic` half must strip — the wire api shows as a
        // color, never as text.
        let registry = std::sync::Arc::new(manox_harness::ProviderRegistry::new());
        registry
            .register_provider(
                "bigmodel-anthropic",
                manox_harness::ProviderConfig {
                    base_url: Some("https://example.invalid".into()),
                    api: Some(manox_harness::provider_registry::Api::AnthropicMessages),
                    models: vec![manox_harness::provider_registry::ProviderModelConfig {
                        id: "glm-5.3".into(),
                        name: String::new(),
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
        let tag = ModelTag::resolve("bigmodel-anthropic/glm-5.3");
        assert_eq!(tag.provider.as_deref(), Some("bigmodel"));
        // An empty model display name falls back to the id.
        assert_eq!(tag.model, "glm-5.3");
        assert_eq!(tag.plain(), "bigmodel/glm-5.3");
    }

    #[test]
    fn unresolved_ids_fall_back_to_the_raw_string() {
        let empty = ModelTag::resolve("");
        assert!(empty.is_empty());
        assert_eq!(empty.plain(), "");

        let bare = ModelTag::resolve("glm-5.3");
        assert_eq!(bare.provider, None);
        assert_eq!(bare.model, "glm-5.3");
        assert_eq!(bare.wire_api, None);
        assert_eq!(bare.plain(), "glm-5.3");

        let unknown = ModelTag::resolve("ghost-anthropic/nope");
        assert_eq!(unknown.provider, None);
        assert_eq!(unknown.model, "ghost-anthropic/nope");
        assert_eq!(unknown.plain(), "ghost-anthropic/nope");
    }
}
