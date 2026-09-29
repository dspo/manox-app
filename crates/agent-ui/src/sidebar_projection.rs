//! The multiplexer → chrome-sidebar projection (PLAN-CHROME-CHAT-SPLIT §5.3,
//! the D2 ruling): pure functions turning the authoritative wire rows
//! (the AHP summary rows, with the multiplexer's status deltas
//! deltas merged) into the chrome `SessionList` props. This module owns no
//! entity and no subscription — the assembly (Phase 4's shell swap) feeds it
//! snapshots on the multiplexer's notify, exactly like the agent-ui sidebar's
//! own `SidebarThreadItem::from_wire`, whose semantics this mirrors:
//!
//! - five-state machine: `errored` (danger triangle) → `pending_auth` /
//!   `pending_plan` (filled blue dot) → `running` (pixel grid) → the
//!   client-owned unread mirror (hollow dot) → idle; the leaf's unread
//!   mirror wins over the deprecated wire flag (GW5);
//! - team forest: `parent_id`/`depth` rows nest under their leader
//!   (recency-ordered leaders, members trailing), rows whose parent is
//!   missing or archived flatten to top-level; rows never indent — the
//!   hierarchy shows via the group header and the leader chevron only;
//! - `updated_at` / `archived` / the tag chip / the pinned flag ride the row
//!   verbatim.

use std::collections::HashMap;

use ahp_types::state::{SessionStatus as WireStatus, SessionSummary};
use manox_agent_chrome_ui::session_list::{SessionGroup, SessionRowData, SessionStatus};

/// One sidebar row: the fields the chrome list renders, derived from a
/// session summary plus its extension fold. Replaces the retired v2
/// `ThreadListItem`.
#[derive(Debug, Clone)]
pub struct ThreadRow {
    pub id: String,
    pub title: String,
    pub running: bool,
    pub unread: bool,
    pub errored: bool,
    pub pending_auth: bool,
    pub pending_plan: bool,
    pub pinned: bool,
    pub parent_id: Option<String>,
    pub depth: i32,
    pub project: Option<String>,
    pub tag: Option<String>,
    /// Last-active unix seconds, from the summary's `modified_at` (the same
    /// clock the list order sorts by); 0 when the stamp fails to parse.
    pub updated_at: i64,
    /// The AHP face carries no archived partition — always false today; the
    /// chrome menu's unarchive half is for the archived surface the wire
    /// will grow.
    pub archived: bool,
}

impl ThreadRow {
    /// Derive a row from the AHP summary and its extension state. The
    /// five-state inputs are bit tests over the summary's status set; plan
    /// pending rides the extension fold's plan review lifecycle. Sub-agent
    /// nesting (depth/parent) has no summary face yet — flat, until the
    /// work channel carries the tree.
    pub fn from_summary(summary: &SessionSummary, pinned: bool, pending_plan: bool) -> Self {
        let bits = summary.status;
        Self {
            id: crate::ahp_store::id_of(&summary.resource).to_string(),
            title: summary.title.clone(),
            running: bits & WireStatus::InProgress.bits() != 0,
            unread: bits & WireStatus::IsRead.bits() == 0,
            errored: bits & WireStatus::Error.bits() != 0,
            pending_auth: bits & WireStatus::InputNeeded.bits() != 0,
            pending_plan,
            pinned,
            parent_id: None,
            depth: 0,
            // The host fills `project` from the thread row for every session;
            // workingDirectories only exists on seeded folds (live engine), so
            // the project is the grouping source of truth.
            project: summary
                .project
                .as_ref()
                .map(|p| p.uri.trim_start_matches("file://").to_string())
                .or_else(|| {
                    summary
                        .working_directories
                        .as_ref()
                        .and_then(|dirs| dirs.first())
                        .map(|uri| uri.trim_start_matches("file://").to_string())
                }),
            tag: None,
            updated_at: chrono::DateTime::parse_from_rfc3339(&summary.modified_at)
                .map(|t| t.timestamp())
                .unwrap_or(0),
            archived: false,
        }
    }
}

/// The client-owned unread mirrors (leaf entity ids → live flags), keyed by
/// thread id; `None` entries fall back to the wire row's flag.
pub type UnreadMirrors = HashMap<String, bool>;

/// Project one wire row (GW5: `unread_override` from the live leaf wins).
/// The sort stamp defaults to the row's own last-active time; a team's
/// members are re-stamped to their leader's by [`project_forest`].
pub fn project_row(item: &ThreadRow, unread_override: Option<bool>) -> SessionRowData {
    SessionRowData {
        id: item.id.clone(),
        title: item.title.clone(),
        updated_at: item.updated_at,
        sort_stamp: item.updated_at,
        status: five_state(item, unread_override),
        pinned: item.pinned,
        archived: item.archived,
        tag: item.tag.clone(),
        team_leader: false,
    }
}

/// The five-state machine over a wire row.
pub fn five_state(item: &ThreadRow, unread_override: Option<bool>) -> SessionStatus {
    if item.errored {
        SessionStatus::Errored
    } else if item.pending_auth {
        SessionStatus::PendingAuth
    } else if item.pending_plan {
        SessionStatus::PendingPlan
    } else if item.running {
        SessionStatus::Running
    } else if unread_override.unwrap_or(item.unread) {
        SessionStatus::Unread
    } else {
        SessionStatus::Idle
    }
}

/// Project a partition's rows into the chrome group shape with the team
/// forest materialized: leaders in list order, each followed by its member
/// rows (the wire's team nesting is never deeper — pi sub-agents — so no
/// recursion is needed). Members no longer indent — the leader's chevron is
/// the only nesting marker, and the projection re-stamps each member's
/// `sort_stamp` to its leader's `updated_at` so the team sorts (and survives
/// a pin re-order) as one unit. Orphans (a parent that is missing,
/// archived, or outside the partition) flatten to top-level rather than
/// vanishing, keeping their own stamp.
pub fn project_forest(rows: &[ThreadRow], unread: &UnreadMirrors) -> Vec<SessionRowData> {
    let by_id: HashMap<&str, &ThreadRow> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.depth == 0 {
            let mut leader = project_row(row, unread.get(&row.id).copied());
            let members: Vec<&ThreadRow> = rows
                .iter()
                .filter(|r| r.depth > 0 && r.parent_id.as_deref() == Some(row.id.as_str()))
                .collect();
            leader.team_leader = !members.is_empty();
            let unit_stamp = leader.sort_stamp;
            out.push(leader);
            for m in members {
                let mut member = project_row(m, unread.get(&m.id).copied());
                member.sort_stamp = unit_stamp;
                out.push(member);
            }
        } else {
            // A member whose leader is absent from this partition flattens.
            let leader_present = row
                .parent_id
                .as_deref()
                .and_then(|p| by_id.get(p))
                .is_some_and(|l| l.depth == 0);
            if !leader_present {
                out.push(project_row(row, unread.get(&row.id).copied()));
            }
        }
    }
    out
}

/// Group a full wire list into chrome `SessionGroup`s keyed by project
/// display name (the path's last segment; empty → "Chats"), applying
/// `project_forest` per partition.
pub fn project_groups(rows: &[ThreadRow], unread: &UnreadMirrors) -> Vec<SessionGroup> {
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<ThreadRow>> = HashMap::new();
    for row in rows {
        let key = project_label(row.project.as_deref().unwrap_or(""));
        if !buckets.contains_key(&key) {
            order.push(key.clone());
        }
        buckets.entry(key).or_default().push(row.clone());
    }
    order
        .into_iter()
        .map(|name| SessionGroup {
            rows: project_forest(buckets.get(&name).expect("bucket just built"), unread),
            name,
            collapsed: false,
        })
        .collect()
}

/// Project display name: the path's last segment; empty (quick chats) →
/// "Chats".
fn project_label(path: &str) -> String {
    if path.is_empty() || path == "." {
        return "Chats".into();
    }
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, depth: i32, parent: Option<&str>) -> ThreadRow {
        ThreadRow {
            id: id.into(),
            title: format!("row {id}"),
            running: false,
            unread: false,
            errored: false,
            pending_auth: false,
            pending_plan: false,
            pinned: false,
            parent_id: parent.map(|p| p.to_string()),
            depth,
            project: Some("/p/wire".into()),
            tag: None,
            updated_at: 0,
            archived: false,
        }
    }

    #[test]
    fn five_state_priority_order() {
        let mut r = row("s", 0, None);
        assert_eq!(five_state(&r, None), SessionStatus::Idle);
        r.unread = true;
        assert_eq!(five_state(&r, None), SessionStatus::Unread);
        r.running = true;
        assert_eq!(five_state(&r, None), SessionStatus::Running);
        r.pending_plan = true;
        assert_eq!(five_state(&r, None), SessionStatus::PendingPlan);
        r.pending_auth = true;
        assert_eq!(five_state(&r, None), SessionStatus::PendingAuth);
        r.errored = true;
        assert_eq!(five_state(&r, None), SessionStatus::Errored);
        // The leaf mirror wins over the wire flag (GW5).
        assert_eq!(five_state(&r, Some(false)), SessionStatus::Errored);
        r.errored = false;
        r.pending_auth = false;
        r.pending_plan = false;
        r.running = false;
        assert_eq!(five_state(&r, Some(false)), SessionStatus::Idle);
        assert_eq!(five_state(&r, Some(true)), SessionStatus::Unread);
    }

    #[test]
    fn forest_nests_members_under_leaders_and_flattens_orphans() {
        let mut rows = vec![
            row("leader", 0, None),
            row("member", 1, Some("leader")),
            row("orphan", 1, Some("gone")),
            row("top", 0, None),
        ];
        // Wire columns ride the projection verbatim.
        rows[0].updated_at = 300;
        rows[1].updated_at = 150;
        rows[2].archived = true;
        let out = project_forest(&rows, &UnreadMirrors::new());
        let ids: Vec<(&str, bool)> = out.iter().map(|r| (r.id.as_str(), r.team_leader)).collect();
        assert_eq!(
            ids,
            vec![
                ("leader", true),
                ("member", false),
                ("orphan", false),
                ("top", false),
            ]
        );
        assert_eq!(out[0].updated_at, 300);
        assert_eq!(out[1].updated_at, 150);
        assert!(out[2].archived);
        assert!(!out[0].archived);
        // The team shares one sort stamp (the leader's); a flattened orphan
        // keeps its own.
        assert_eq!(out[0].sort_stamp, 300);
        assert_eq!(out[1].sort_stamp, 300);
        assert_eq!(out[2].sort_stamp, 0);
        assert_eq!(out[3].sort_stamp, 0);
    }

    #[test]
    fn groups_key_by_project_tail_and_chats_bucket() {
        let mut rows = vec![row("a", 0, None), row("b", 0, None)];
        rows[1].project = None;
        rows[1].tag = Some("mytag".into());
        rows[1].pinned = true;
        let groups = project_groups(&rows, &UnreadMirrors::new());
        let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["wire", "Chats"]);
        let chats = &groups[1].rows[0];
        assert_eq!(chats.tag.as_deref(), Some("mytag"));
        assert!(chats.pinned);
    }
}
