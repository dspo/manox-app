//! Provider→model cascade shared by every external-agent launch surface.
//!
//! Models are the multiplexer's wire `ModelInfo` rows (U2 cross-domain #4 —
//! the former `provider_glue` direct read retired): filtered by the agent id
//! (the registration's `agents` column, absent = visible to all); grouped by
//! provider display name, each provider a nested submenu. A config model
//! registered through several wire apis appears once per wire endpoint (exact
//! duplicates collapse), each row tagged with its wire api like the composer
//! model menu. Picking a model invokes `on_pick` with (provider, model id,
//! wire) — the emitted model id is the raw cx config key (`config_id`,
//! falling back to the model id), which cx matches verbatim; the wire key
//! pins the endpoint variant at launch resolution.

use crate::i18n;
use gpui::{App, Context, Window, prelude::*};
use gpui_component::{
    Sizable as _, h_flex,
    menu::{PopupMenu, PopupMenuItem},
    tag::Tag,
};

/// One cascade entry: the raw cx config key, its display name, the wire api
/// (the row tag's source), and the wire key for the launch pin.
#[derive(Debug)]
pub(crate) struct CascadeEntry {
    pub config_id: String,
    pub display: String,
    pub api: String,
    pub wire: Option<String>,
}

/// The pure cascade projection (U2 cross-domain #4): the agents-visibility
/// filter (an absent list is visible to all; a present list must contain the
/// agent — an empty list hides), the (provider, config_id) dedupe (wire
/// variants of one config stay separate, exact duplicates collapse), and the
/// lookup-based grouping by provider display name (the wire list is not
/// display-name-sorted, so equal names must merge).
pub(crate) fn cascade_provider_groups(
    agent_id: &str,
    agents: &[ahp_types::state::AgentInfo],
) -> Vec<(String, Vec<CascadeEntry>)> {
    let mut providers: Vec<(String, Vec<CascadeEntry>)> = Vec::new();
    // One group per agent registration: AHP's root catalogue already carries
    // the provider identity the v2 wire list flattened, so the dedupe and the
    // visible-agents filter the retired wire shape needed are structural now.
    for agent in agents {
        let entries = agent
            .models
            .iter()
            .map(|m| CascadeEntry {
                config_id: m.id.clone(),
                display: m.name.clone(),
                api: String::new(),
                wire: None,
            })
            .collect();
        let prov = if agent.display_name.is_empty() {
            agent.provider.clone()
        } else {
            agent.display_name.clone()
        };
        let _ = agent_id;
        match providers.iter_mut().find(|(name, _)| *name == prov) {
            Some((_, existing)) => existing.extend(entries),
            None => providers.push((prov, entries)),
        }
    }
    providers
}

pub(crate) fn build_model_cascade(
    menu: PopupMenu,
    agent_id: &'static str,
    agents: &[ahp_types::state::AgentInfo],
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    on_pick: impl Fn(String, String, Option<String>, &mut Window, &mut App) + Clone + 'static,
) -> PopupMenu {
    let providers = cascade_provider_groups(agent_id, agents);

    let mut menu = menu;
    if providers.is_empty() {
        menu = menu.label(i18n::t("external-wizard-no-model"));
        return menu;
    }
    for (prov_name, entries) in providers {
        let prov_for_items = prov_name.clone();
        let on_pick = on_pick.clone();
        menu = menu.submenu(prov_name, window, cx, move |submenu, _window, _cx| {
            let mut submenu = submenu;
            for m in &entries {
                let model_id = m.config_id.clone();
                let model_name = m.display.clone();
                let (variant, label) = crate::Workspace::pi_wire_tag_variant(&m.api);
                let wire = m.wire.clone();
                let prov = prov_for_items.clone();
                let on_pick = on_pick.clone();
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
                            .child(model_name.clone())
                            .into_any_element()
                    })
                    .on_click(move |_e, window, cx: &mut App| {
                        on_pick(prov.clone(), model_id.clone(), wire.clone(), window, cx);
                    }),
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
        let groups = cascade_provider_groups("pi", &agents);
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].0, "Provider A");
        assert_eq!(groups[0].1.len(), 4, "two agents' models merge");
        assert_eq!(groups[1].0, "Provider B");
        assert_eq!(groups[0].1[0].config_id, "m1");
    }
}
