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

use std::collections::HashSet;

/// One cascade entry: the raw cx config key, its display name, the wire api
/// (the row tag's source), and the wire key for the launch pin.
#[derive(Debug)]
pub(crate) struct CascadeEntry {
    pub config_id: String,
    pub display: String,
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
    models: &[manox_protocol::ModelInfo],
) -> Vec<(String, Vec<CascadeEntry>)> {
    let mut providers: Vec<(String, Vec<CascadeEntry>)> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for m in models {
        // Missing agents column = non-cx registration (visible); otherwise
        // the effective agent list must contain the cascade's agent (parity
        // with the retired manox `visible_agents` filter).
        let visible = m
            .agents
            .as_ref()
            .map(|list| list.iter().any(|a| a == agent_id))
            .unwrap_or(true);
        if !visible {
            continue;
        }
        let prov = m
            .provider_name
            .clone()
            .unwrap_or_else(|| m.provider.clone());
        let config_id = m.config_id.clone().unwrap_or_else(|| m.id.clone());
        // Identity is the registration name (unique per wire endpoint), so
        // wire variants of one provider stay separate; only exact
        // duplicates collapse (parity with the composer model menu).
        if !seen.insert((m.provider.clone(), config_id.clone())) {
            continue;
        }
        let entry = CascadeEntry {
            config_id,
            display: m.name.clone(),
            wire: manox_agent::provider_glue::wire_key_from_api(&m.api).map(str::to_string),
        };
        // Lookup-based grouping (not adjacency): equal display names merge.
        match providers.iter_mut().find(|(name, _)| *name == prov) {
            Some((_, entries)) => entries.push(entry),
            None => providers.push((prov, vec![entry])),
        }
    }
    providers
}
