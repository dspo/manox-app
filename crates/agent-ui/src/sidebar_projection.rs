//! The multiplexer → chrome-sidebar projection: pure functions turning the
//! authoritative wire rows (the AHP summary rows, with the multiplexer's
//! status deltas merged) into the chrome `SessionList` props. This module
//! owns no entity and no subscription — the assembly feeds it snapshots on
//! the multiplexer's notify, exactly like the agent-ui sidebar's
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

use std::collections::{HashMap, HashSet};

use ahp_types::state::{SessionStatus as WireStatus, SessionSummary};
use steer_agent_chrome_ui::session_list::{
    SessionGroup, SessionRowData, SessionRowKind, SessionStatus,
};

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
    /// The protocol's archived bit (`SessionStatus::IsArchived` on the
    /// summary): the host persists it (`session/isArchivedChanged`), the
    /// chrome menu's archive toggle dispatches it back.
    pub archived: bool,
    /// The row's position in the HOST's sidebar order (0 = top). The
    /// chrome re-sorts rows by `(pinned, sort_stamp)` locally, so the
    /// projection encodes the rank as a descending stamp — the chrome's
    /// own sort then reproduces the host order exactly, and no local
    /// timestamp can reshuffle it.
    pub rank: i64,
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
            archived: bits & WireStatus::IsArchived.bits() != 0,
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
            // Unranked until the multiplexer assigns the host order's
            // position; the default sorts last.
            rank: i64::MAX,
        }
    }
}

/// Project one wire row: the row's own wire flag is the single unread
/// source (the official IsRead bit folded from the protocol). The sort
/// stamp defaults to the row's own last-active time; a team's members are
/// re-stamped to their leader's by [`project_forest`].
pub fn project_row(item: &ThreadRow) -> SessionRowData {
    SessionRowData {
        id: item.id.clone(),
        short_id: item.id.chars().take(8).collect(),
        title: item.title.clone(),
        updated_at: item.updated_at,
        // The descending-encoded host rank: the chrome's local
        // `(pinned, sort_stamp desc)` sort reproduces the host's
        // interaction-ranked order instead of a reshuffling timestamp.
        sort_stamp: i64::MAX - item.rank,
        status: five_state(item),
        kind: SessionRowKind::Thread,
        pinned: item.pinned,
        archived: item.archived,
        tag: item.tag.clone(),
        team_leader: false,
    }
}

/// The five-state machine over a wire row.
pub fn five_state(item: &ThreadRow) -> SessionStatus {
    if item.errored {
        SessionStatus::Errored
    } else if item.pending_auth {
        SessionStatus::PendingAuth
    } else if item.pending_plan {
        SessionStatus::PendingPlan
    } else if item.running {
        SessionStatus::Running
    } else if item.unread {
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
pub fn project_forest(rows: &[ThreadRow]) -> Vec<SessionRowData> {
    let by_id: HashMap<&str, &ThreadRow> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.depth == 0 {
            let mut leader = project_row(row);
            let members: Vec<&ThreadRow> = rows
                .iter()
                .filter(|r| r.depth > 0 && r.parent_id.as_deref() == Some(row.id.as_str()))
                .collect();
            leader.team_leader = !members.is_empty();
            let unit_stamp = leader.sort_stamp;
            out.push(leader);
            for m in members {
                let mut member = project_row(m);
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
                out.push(project_row(row));
            }
        }
    }
    out
}

/// Group a full wire list into chrome `SessionGroup`s keyed by project
/// display name (the path's last segment; empty → "Chats"), applying
/// `project_forest` per partition. `removed` is the client-side removal
/// overlay ([`crate::project_registry`]): a row whose effective project sits
/// in the set groups as loose, the v2 store's remove-project semantics — the
/// folder dissolves, the history stays.
pub fn project_groups(
    rows: &[ThreadRow],
    folders: &[(String, Option<String>)],
    removed: &HashSet<String>,
) -> Vec<SessionGroup> {
    // The effective grouping project: the row's own, minus removed paths (a
    // removed row keeps its row data — it re-buckets, it does not vanish).
    let effective = |row: &ThreadRow| -> Option<String> {
        row.project
            .clone()
            .filter(|p| !p.is_empty() && !removed.contains(p))
    };
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<ThreadRow>> = HashMap::new();
    let mut bucket_project: HashMap<String, Option<String>> = HashMap::new();
    // The durable project skeleton first: a folder renders even with zero
    // rows — archiving every conversation never dissolves the project. The
    // alias is the folder's own name; the directory segment is the fallback.
    for (path, alias) in folders {
        if removed.contains(path) {
            continue;
        }
        let name = alias
            .clone()
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| project_label(path));
        if !buckets.contains_key(&name) {
            order.push(name.clone());
            bucket_project.insert(name.clone(), Some(path.clone()));
        }
        buckets.entry(name).or_default();
    }
    for row in rows {
        let project = effective(row);
        let key = project_label(project.as_deref().unwrap_or(""));
        if !buckets.contains_key(&key) {
            order.push(key.clone());
            bucket_project.insert(key.clone(), project);
        }
        buckets.entry(key).or_default().push(row.clone());
    }
    order
        .into_iter()
        .map(|name| SessionGroup {
            rows: project_forest(buckets.get(&name).map(Vec::as_slice).unwrap_or(&[])),
            key: name.clone(),
            project: bucket_project.get(&name).cloned().flatten(),
            name,
            collapsed: false,
            hidden: 0,
            expanded: false,
        })
        .collect()
}

/// Project display name: the path's last segment; empty (quick chats) →
/// "Chats".
pub(crate) fn project_label(path: &str) -> String {
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
            archived: false,
            rank: 0,
            parent_id: parent.map(|p| p.to_string()),
            depth,
            project: Some("/p/wire".into()),
            tag: None,
            updated_at: 0,
        }
    }

    #[test]
    fn five_state_priority_order() {
        let mut r = row("s", 0, None);
        assert_eq!(five_state(&r), SessionStatus::Idle);
        r.unread = true;
        assert_eq!(five_state(&r), SessionStatus::Unread);
        r.running = true;
        assert_eq!(five_state(&r), SessionStatus::Running);
        r.pending_plan = true;
        assert_eq!(five_state(&r), SessionStatus::PendingPlan);
        r.pending_auth = true;
        assert_eq!(five_state(&r), SessionStatus::PendingAuth);
        r.errored = true;
        assert_eq!(five_state(&r), SessionStatus::Errored);
        r.errored = false;
        r.pending_auth = false;
        r.pending_plan = false;
        r.running = false;
        assert_eq!(five_state(&r), SessionStatus::Unread);
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
        rows[0].rank = 0;
        rows[1].rank = 5;
        rows[2].rank = 2;
        rows[3].rank = 9;
        let out = project_forest(&rows);
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
        // The team shares one sort stamp (the leader's rank-encoded one);
        // a flattened orphan and an unrelated top row keep their own ranks.
        assert_eq!(out[0].sort_stamp, i64::MAX);
        assert_eq!(out[1].sort_stamp, out[0].sort_stamp);
        assert_eq!(out[2].sort_stamp, i64::MAX - 2);
        assert_eq!(out[3].sort_stamp, i64::MAX - 9);
    }

    #[test]
    fn groups_key_by_project_tail_and_chats_bucket() {
        let mut rows = vec![row("a", 0, None), row("b", 0, None)];
        rows[1].project = None;
        rows[1].tag = Some("mytag".into());
        rows[1].pinned = true;
        let groups = project_groups(&rows, &[], &HashSet::new());
        let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["wire", "Chats"]);
        let chats = &groups[1].rows[0];
        assert_eq!(chats.tag.as_deref(), Some("mytag"));
        assert!(chats.pinned);
    }

    /// The group carries its real project path (the menu's launch target),
    /// a removed project re-buckets to loose WITHOUT losing its rows, and a
    /// no-project group carries no path.
    #[test]
    fn folder_skeleton_renders_empty_projects_with_aliases() {
        let mut rows = vec![row("a", 0, None)];
        rows[0].project = Some("/p/live".into());
        let folders = vec![
            ("/p/empty".to_string(), Some("空项目".to_string())),
            ("/p/live".to_string(), None),
        ];
        let removed: HashSet<String> = HashSet::new();
        let groups = project_groups(&rows, &folders, &removed);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "空项目", "the alias is the folder's name");
        assert_eq!(groups[0].rows.len(), 0, "an emptied project still renders");
        assert_eq!(groups[0].project.as_deref(), Some("/p/empty"));
        assert_eq!(groups[1].name, "live");
        assert_eq!(groups[1].rows.len(), 1);
    }

    /// A skeleton folder whose project was client-removed dissolves; its
    /// rows (if any) re-bucket loose.
    #[test]
    fn a_removed_skeleton_folder_dissolves() {
        let mut rows = vec![row("a", 0, None)];
        rows[0].project = Some("/p/gone".into());
        let folders = vec![("/p/gone".to_string(), None)];
        let removed: HashSet<String> = ["/p/gone".to_string()].into_iter().collect();
        let groups = project_groups(&rows, &folders, &removed);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Chats");
    }

    /// The chrome re-sorts by `(pinned, sort_stamp)` locally; the projection
    /// encodes the HOST rank as a descending stamp so that sort reproduces
    /// the host order — no local timestamp can reshuffle it.
    #[test]
    fn the_sort_stamp_encodes_the_host_rank() {
        let mut rows = [row("a", 0, None), row("b", 0, None)];
        rows[0].rank = 0;
        rows[1].rank = 1;
        let projected: Vec<_> = rows.iter().map(project_row).collect();
        assert!(projected[0].sort_stamp > projected[1].sort_stamp);
        // A local sort of the projected rows reproduces the host order.
        let mut sorted = projected.clone();
        sorted.sort_by_key(|r| std::cmp::Reverse(r.sort_stamp));
        assert_eq!(sorted[0].id, "a");
        assert_eq!(sorted[1].id, "b");
    }

    /// Removed project paths re-bucket their rows loose (the v2
    /// remove-project semantics): the folder dissolves, the history stays.
    #[test]
    fn groups_carry_project_and_removed_paths_rebucket_to_loose() {
        let mut rows = vec![row("a", 0, None), row("b", 0, None), row("c", 0, None)];
        rows[0].project = None;
        rows[1].project = Some("/p/wire".into());
        rows[2].project = Some("/p/wire".into());
        let removed: HashSet<String> = ["/p/wire".to_string()].into_iter().collect();
        let groups = project_groups(&rows, &[], &removed);
        // Everything loose: one "Chats" bucket, no project path on it.
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Chats");
        assert_eq!(groups[0].project, None);
        assert_eq!(
            groups[0].rows.len(),
            3,
            "removed rows re-bucket, not vanish"
        );

        // Without the removal, the bucket is the project group with the path.
        let groups = project_groups(&rows, &[], &HashSet::new());
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Chats");
        assert_eq!(groups[0].project, None);
        assert_eq!(groups[1].name, "wire");
        assert_eq!(groups[1].project.as_deref(), Some("/p/wire"));
        assert_eq!(groups[1].rows.len(), 2);
    }
}
