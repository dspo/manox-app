//! Provider→model cascade shared by every external-agent launch surface.
//!
//! Two projections over two sources, one per consumer generation:
//!
//! - [`cascade_provider_groups`] reads AHP's root catalogue (`AgentInfo`
//!   rows) — the right-pane agent tabs' picker. One group per agent
//!   registration (display name, falling back to the provider id), each
//!   model an entry; the entry id is the canonical `provider/model` string
//!   the pick dispatches verbatim; the wire api rides the model's
//!   `x-manox.api` meta and is mapped to the launch pin's vocabulary.
//!   KNOWN DOWNGRADE: the v2 registry's per-agent visibility column has no
//!   AHP successor — every agent's picker lists every provider's models.
//! - [`build_model_menu`] reads the app-layer registry ([`crate::model_catalog`])
//!   and renders the COMPOSER model picker's shape: one submenu per provider
//!   display name (wire variants of one cx config merge), each row a wire
//!   tag + the model's display name. The composer's own selector and the
//!   project menu's agent submenus share this builder — one look, one
//!   interaction.

use gpui::{App, Context, ParentElement as _, Styled as _, Window};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::tag::Tag;
use gpui_component::{Sizable as _, h_flex};
use serde_json::Value;

use crate::model_catalog::{ModelRow, wire_visual};

/// The host's meta api vocabulary ("anthropic" / "openai_responses" /
/// "openai_completions") mapped onto the launch pin's ("anthropic" /
/// "responses" / "completions"); an unknown api carries no pin.
pub(crate) fn launch_wire_key(meta_api: &str) -> Option<String> {
    match meta_api {
        "anthropic" => Some("anthropic".to_string()),
        "openai_responses" => Some("responses".to_string()),
        "openai_completions" => Some("completions".to_string()),
        _ => None,
    }
}

/// One cascade entry: the canonical `provider/model` pick (dispatched
/// verbatim), its display name, and the wire api mapped onto the launch
/// pin's vocabulary ("anthropic" / "responses" / "completions").
#[derive(Debug)]
pub(crate) struct CascadeEntry {
    pub config_id: String,
    pub display: String,
    pub wire: Option<String>,
}

/// The pure cascade projection over AHP's root catalogue: one group per
/// agent registration (the catalogue carries the provider identity the v2
/// wire list flattened); each agent's models become entries. KNOWN
/// DOWNGRADE: the v2 registry's per-agent visibility column has no AHP
/// successor — every agent's picker lists every provider's models.
pub(crate) fn cascade_provider_groups(
    agents: &[ahp_types::state::AgentInfo],
) -> Vec<(String, Vec<CascadeEntry>)> {
    let mut providers: Vec<(String, Vec<CascadeEntry>)> = Vec::new();
    for agent in agents {
        let entries = agent
            .models
            .iter()
            .map(|m| CascadeEntry {
                config_id: m.id.clone(),
                display: m.name.clone(),
                wire: m
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("x-manox"))
                    .and_then(|x| x.get("api"))
                    .and_then(Value::as_str)
                    .and_then(launch_wire_key),
            })
            .collect();
        let prov = if agent.display_name.is_empty() {
            agent.provider.clone()
        } else {
            agent.display_name.clone()
        };
        match providers.iter_mut().find(|(name, _)| *name == prov) {
            Some((_, existing)) => existing.extend(entries),
            None => providers.push((prov, entries)),
        }
    }
    providers
}

/// Group registry rows by the provider's DISPLAY name via lookup (not
/// adjacency): the snapshot is sorted by registration key, so same-display
/// providers with different registrations (the wire variants of one cx
/// config) must still merge into a single submenu.
pub(crate) fn group_by_provider_display(models: &[ModelRow]) -> Vec<(String, Vec<ModelRow>)> {
    let mut providers: Vec<(String, Vec<ModelRow>)> = Vec::new();
    for m in models {
        match providers
            .iter_mut()
            .find(|(name, _)| *name == m.provider_display)
        {
            Some((_, rows)) => rows.push(m.clone()),
            None => providers.push((m.provider_display.clone(), vec![m.clone()])),
        }
    }
    providers
}

/// The composer model-picker's cascade, shared by the composer's own
/// selector and the project menu's agent submenus: one submenu per provider
/// display name, each row the wire tag + the model's display name (the same
/// look and pick interaction everywhere). `on_pick` runs on the click path
/// with the picked registry row.
pub(crate) fn build_model_menu(
    menu: PopupMenu,
    models: Vec<ModelRow>,
    on_pick: impl Fn(ModelRow, &mut Window, &mut App) + Clone + 'static,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let providers = group_by_provider_display(&models);
    let mut menu = menu;
    if providers.is_empty() {
        return menu.item(PopupMenuItem::Label(crate::i18n::t(
            "external-wizard-no-model",
        )));
    }
    for (prov_name, rows) in providers {
        // The submenu builder is an `Fn` — every capture must be per-call
        // owned, never moved out of the loop's state.
        let rows = rows.clone();
        let on_pick = on_pick.clone();
        menu = menu.submenu(prov_name, window, cx, move |submenu, _window, _cx| {
            let mut submenu = submenu;
            for m in &rows {
                // Every closure below is 'static: the row is cloned owned,
                // never borrowed out of this builder's captures.
                let m = m.clone();
                let on_pick = on_pick.clone();
                let (variant, label, _) = wire_visual(&m.api);
                let name = m.name.clone();
                submenu = submenu.item(
                    PopupMenuItem::element(move |_window, _cx| {
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Tag::new()
                                    .with_variant(variant)
                                    .outline()
                                    .small()
                                    .child(label),
                            )
                            .child(name.clone())
                    })
                    .on_click(move |_, window, cx| on_pick(m.clone(), window, cx)),
                );
            }
            submenu
        });
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, display: &str) -> ahp_types::state::AgentInfo {
        serde_json::from_value(serde_json::json!({
            "provider": id,
            "displayName": display,
            "description": "",
            "models": [
                { "id": "m1", "provider": id, "name": "Model m1" },
                { "id": "m2", "provider": id, "name": "Model m2" },
            ],
        }))
        .expect("agent parses")
    }

    /// The cascade groups one submenu per agent registration, with each
    /// agent's models as entries (AHP's root catalogue carries the provider
    /// identity the v2 wire list flattened).
    #[test]
    fn cascade_groups_one_submenu_per_agent() {
        let agents = vec![
            agent("prov-a", "Provider A"),
            agent("prov-b", "Provider B"),
            // Same display name merges (lookup grouping, not adjacency).
            agent("prov-c", "Provider A"),
        ];
        let groups = cascade_provider_groups(&agents);
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].0, "Provider A");
        assert_eq!(groups[0].1.len(), 4, "two agents' models merge");
        assert_eq!(groups[1].0, "Provider B");
        assert_eq!(groups[0].1[0].config_id, "m1");
    }

    fn row(provider: &str, id: &str, display: &str) -> ModelRow {
        ModelRow {
            provider: provider.into(),
            cx_name: provider.into(),
            provider_display: display.into(),
            id: id.into(),
            name: id.into(),
            api: "anthropic".into(),
        }
    }

    /// The registry-row grouping merges wire variants of one provider
    /// config (same display name, different registration keys) into one
    /// submenu regardless of the snapshot's registration order.
    #[test]
    fn registry_rows_group_by_display_name_via_lookup() {
        let rows = vec![
            row("bailian-anthropic", "glm-5.2", "百炼"),
            row("deepseek-responses", "deepseek-pro", "DeepSeek"),
            row("bailian-completions", "qwen-max", "百炼"),
        ];
        let groups = group_by_provider_display(&rows);
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].0, "百炼");
        assert_eq!(groups[0].1.len(), 2, "wire variants merge");
        assert_eq!(groups[1].0, "DeepSeek");
    }
}
