//! Provider→model cascade shared by every external-agent launch surface.
//!
//! Models are AHP's root-catalogue `AgentInfo` rows: one group per agent
//! registration (display name, falling back to the provider id), each
//! model an entry. The entry id is the canonical `provider/model` string
//! the pick dispatches verbatim; the wire api rides the model's
//! `x-manox.api` meta and is mapped to the launch pin's vocabulary
//! ("anthropic" / "responses" / "completions"). KNOWN DOWNGRADE: the v2
//! registry's per-agent visibility column has no AHP successor — every
//! agent's picker lists every provider's models.

use serde_json::Value;

/// The host's meta api vocabulary ("anthropic" / "openai_responses" /
/// "openai_completions") mapped onto the launch pin's ("anthropic" /
/// "responses" / "completions"); an unknown api carries no pin.
fn launch_wire_key(meta_api: &str) -> Option<String> {
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
}
