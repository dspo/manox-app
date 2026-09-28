//! Provider→model cascade shared by every external-agent launch surface.
//!
//! Models are the multiplexer's wire `ModelInfo` rows (U2 cross-domain #4 —
//! the former `provider_glue` direct read retired): filtered by the agent id
//! (the registration's `agents` column, absent = visible to all); grouped by
//! provider display name. A config model registered through several wire
//! apis appears once per wire endpoint (exact duplicates collapse). The
//! emitted model id is the raw cx config key (`config_id`, falling back to
//! the model id), which cx matches verbatim; `wire` pins the endpoint variant
//! at launch resolution.

/// One cascade entry: the raw cx config key, its display name, the wire api
/// (the row tag's source), and the wire key for the launch pin.
#[derive(Debug)]
pub(crate) struct CascadeEntry {
    pub config_id: String,
    pub display: String,
    pub wire: Option<String>,
}

/// The pure cascade projection over AHP's root catalogue: one group per
/// agent registration (the catalogue carries the provider identity the v2
/// wire list flattened, so the dedupe and the visible-agents filter are
/// structural now); each agent's models become entries.
pub(crate) fn cascade_provider_groups(
    _agent_id: &str,
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
                wire: None,
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
        let groups = cascade_provider_groups("pi", &agents);
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].0, "Provider A");
        assert_eq!(groups[0].1.len(), 4, "two agents' models merge");
        assert_eq!(groups[1].0, "Provider B");
        assert_eq!(groups[0].1[0].config_id, "m1");
    }
}
