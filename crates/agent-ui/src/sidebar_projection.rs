//! The multiplexer → chrome-sidebar projection: pure functions turning the
//! authoritative wire rows (the AHP summary rows, with the multiplexer's
//! status deltas merged) into the chrome `SessionList` props. This module
//! owns no entity and no subscription — the assembly feeds it snapshots on
//! the multiplexer's notify, exactly like the agent-ui sidebar's
//! own `SidebarThreadItem::from_wire`, whose semantics this mirrors:
//!
//! - a row stores three facts: `running` (the chat fold's active turn,
//!   the summary's InProgress bit as the never-subscribed fallback),
//!   `pending` (a turn parked on a human — the chat's open ask or the
//!   session's input-needed queue, Ask winning when both light), `unread`
//!   (the client-owned mirror wins over the deprecated wire flag, GW5);
//! - the display status is never stored — [`derive_status`] folds the
//!   ladder Pending > Running > Unread > Idle. Errors are not a list
//!   state: the wire `Error` bit stays on the fold but no row consumes it
//!   (a turn's failure renders in the transcript);
//! - the container tree (plan D3, two levels only): a `ContainerRow` knows
//!   its membership — `member_kind` (upstream #987's
//!   `_meta.x-manox.memberKind`) and `parent_id` (`parentSessionId`, child
//!   rows only). The catalogue folds level-1 children OUT before projection
//!   ([`ContainerRow::is_child`] — the roster surface joins them, never the
//!   sidebar), so the sidebar is one row per container; [`project_forest`]
//!   keeps the captain/child walk for any feed that does carry children
//!   (captains in list order, children trailing, orphans flatten to
//!   top-level); rows never indent — the hierarchy shows via the group
//!   header and the captain chevron only;
//! - `updated_at` / `archived` / the tag chip / the pinned flag ride the row
//!   verbatim.

use std::collections::{HashMap, HashSet};

use ahp_types::state::{SessionStatus as WireStatus, SessionSummary};
use steer_agent_chrome_ui::session_list::{
    PendingKind, SessionGroup, SessionRowData, SessionRowKind, SessionStatus,
};

/// One sidebar row: one SESSION CONTAINER (plan D1) — the fields the chrome
/// list renders, derived from a session summary plus its extension fold.
/// Replaces the retired v2 `ThreadListItem`; the sidebar's row vocabulary
/// speaks containers, the wire's session vocabulary underneath is untouched.
#[derive(Debug, Clone)]
pub struct ContainerRow {
    pub id: String,
    pub title: String,
    /// Whether this row's own turn is running.
    pub running: bool,
    /// A turn parked on a human: an open ask or a queued tool
    /// confirmation.
    pub pending: Option<PendingKind>,
    /// The unread mirror — the completion reminder.
    pub unread: bool,
    pub pinned: bool,
    /// The parent container's id — upstream #987's
    /// `_meta.x-manox.parentSessionId` (child rows only): the tree edge to
    /// the owning captain.
    pub parent_id: Option<String>,
    /// The row's membership in the container tree (upstream #987's
    /// `_meta.x-manox.memberKind`): a [`MemberKind::Subagent`] row is its
    /// captain's level-1 child and folds out of the sidebar; unannotated
    /// rows (today's runtime sends no member slot) and captains are
    /// top-level.
    pub member_kind: MemberKind,
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
    /// chrome re-sorts rows by `(pinned, rank)` locally, so carrying the
    /// rank lets that sort reproduce the host order exactly — no local
    /// timestamp can reshuffle it. `i64::MAX` (unranked) sorts last.
    pub rank: i64,
}

/// The container tree's two memberships (plan D3 — captain 0-level, produced
/// subagents 1-level, nothing deeper). An unannotated summary reads
/// [`MemberKind::Captain`]: the sidebar renders top-level rows only, so the
/// unknown must render rather than vanish.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MemberKind {
    #[default]
    Captain,
    Subagent,
}

impl MemberKind {
    fn from_meta(value: Option<&str>) -> Self {
        match value {
            Some("subagent") => Self::Subagent,
            // `captain`, unknown words, and the absent slot all read
            // top-level — the fold only ever hides what it recognizes.
            _ => Self::Captain,
        }
    }
}

impl ContainerRow {
    /// Whether the row is a container's LEVEL-1 child: a `subagent`
    /// memberKind, or a parent edge that arrived without a recognized
    /// annotation (the plan's fold rule keys on the parent too). Children
    /// never render as top-level sidebar rows — the catalogue folds them
    /// out (`SessionMultiplexer::container_rows`); they stay in the book for
    /// the roster surface and whole-tree unarchive.
    pub fn is_child(&self) -> bool {
        self.member_kind == MemberKind::Subagent || self.parent_id.is_some()
    }

    /// Derive a row's FALLBACK facts from the AHP summary and its
    /// extension state — bit tests over the summary's status set plus the
    /// passed open-ask flag; Ask outranks Auth when both faces are open —
    /// a question names what is missing. The wire `Error` bit is
    /// deliberately not read: errors are turn content, never a list
    /// state. The multiplexer's `merged_row` then lets a landed live session
    /// or chat fold outrank the row, so the summary bits speak for sessions
    /// this client never subscribed AND through a fold's pre-snapshot
    /// window. The container-tree facts (`memberKind` / `parentSessionId`)
    /// read the summary's `_meta.x-manox` slot — the same read the row pin
    /// does (`ext_and_meta_pinned`); a summary without the slot (today's
    /// pinned runtime) reads a top-level captain with no parent.
    pub fn from_summary(summary: &SessionSummary, pinned: bool, pending_ask: bool) -> Self {
        let bits = summary.status;
        let needs_auth = bits & WireStatus::InputNeeded.bits() != 0;
        let member_slot = || summary.meta.as_ref().and_then(|m| m.get("x-manox"));
        let parent_id = member_slot()
            .and_then(|x| x.get("parentSessionId"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let member_kind = MemberKind::from_meta(
            member_slot()
                .and_then(|x| x.get("memberKind"))
                .and_then(serde_json::Value::as_str),
        );
        Self {
            id: crate::ahp_store::id_of(&summary.resource).to_string(),
            title: summary.title.clone(),
            running: bits & WireStatus::InProgress.bits() != 0,
            pending: if pending_ask {
                Some(PendingKind::Ask)
            } else if needs_auth {
                Some(PendingKind::Auth)
            } else {
                None
            },
            unread: bits & WireStatus::IsRead.bits() == 0,
            archived: bits & WireStatus::IsArchived.bits() != 0,
            pinned,
            parent_id,
            member_kind,
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

/// Project one wire row through [`derive_status`]. The row's own wire flag
/// is the single unread source (the official IsRead bit folded from the
/// protocol). The time stamp and the host rank ride separately — the stamp
/// is a real unix time the chrome's recency buckets decode, the rank its
/// order key; a container tree's children are re-stamped to their captain's
/// by [`project_forest`].
pub fn project_row(item: &ContainerRow) -> SessionRowData {
    SessionRowData {
        id: item.id.clone(),
        short_id: item.id.chars().take(8).collect(),
        title: item.title.clone(),
        updated_at: item.updated_at,
        sort_stamp: item.updated_at,
        rank: item.rank,
        status: derive_status(item),
        kind: SessionRowKind::Thread,
        pinned: item.pinned,
        archived: item.archived,
        tag: item.tag.clone(),
        team_leader: false,
    }
}

/// The display ladder over the stored facts: Pending > Running > Unread >
/// Idle.
pub fn derive_status(item: &ContainerRow) -> SessionStatus {
    match item.pending {
        Some(kind) => SessionStatus::Pending(kind),
        None if item.running => SessionStatus::Running,
        None if item.unread => SessionStatus::Unread,
        None => SessionStatus::Idle,
    }
}

/// Project a partition's rows into the chrome group shape with the container
/// tree materialized: captains in list order, each followed by its child
/// rows ([`ContainerRow::is_child`] rows pointing at the captain — the wire
/// tree is never deeper than the plan's two levels, so no recursion is
/// needed). The catalogue feed never carries children (they fold out in
/// `container_rows`), so the sidebar walk is a pass-through today; the
/// roster feed joins children later through this same shape. Children no
/// longer indent — the captain's chevron is the only nesting marker, and
/// the projection re-stamps each child's `sort_stamp` and `rank` to its
/// captain's so the unit sorts (and survives a pin re-order) as one.
/// Orphans (a parent that is missing, archived, or outside the partition)
/// flatten to top-level rather than vanishing, keeping their own stamp.
pub fn project_forest(rows: &[ContainerRow]) -> Vec<SessionRowData> {
    let by_id: HashMap<&str, &ContainerRow> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if !row.is_child() {
            let members: Vec<&ContainerRow> = rows
                .iter()
                .filter(|r| r.is_child() && r.parent_id.as_deref() == Some(row.id.as_str()))
                .collect();
            let mut leader = project_row(row);
            leader.team_leader = !members.is_empty();
            let unit_stamp = leader.sort_stamp;
            let unit_rank = leader.rank;
            out.push(leader);
            for m in members {
                let mut member = project_row(m);
                member.sort_stamp = unit_stamp;
                member.rank = unit_rank;
                out.push(member);
            }
        } else {
            // A member whose captain is absent from this partition flattens.
            let leader_present = row
                .parent_id
                .as_deref()
                .and_then(|p| by_id.get(p))
                .is_some_and(|l| !l.is_child());
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
    rows: &[ContainerRow],
    folders: &[(String, Option<String>)],
    removed: &HashSet<String>,
) -> Vec<SessionGroup> {
    // The effective grouping project: the row's own, minus removed paths (a
    // removed row keeps its row data — it re-buckets, it does not vanish).
    let effective = |row: &ContainerRow| -> Option<String> {
        row.project
            .clone()
            .filter(|p| !p.is_empty() && !removed.contains(p))
    };
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<ContainerRow>> = HashMap::new();
    let mut bucket_project: HashMap<String, Option<String>> = HashMap::new();
    // The durable project skeleton first: a folder renders even with zero
    // rows — archiving every conversation never dissolves the project. The
    // alias is the folder's own name; the directory segment is the fallback.
    // The path→name map is also the row-filing index: a row joins its
    // project's folder by PATH, so a non-empty alias cannot split the
    // project into an empty alias head plus a basename-headed row bucket.
    let folder_name: HashMap<&str, String> = folders
        .iter()
        .filter(|(path, _)| !removed.contains(path))
        .map(|(path, alias)| {
            (
                path.as_str(),
                alias
                    .clone()
                    .filter(|a| !a.trim().is_empty())
                    .unwrap_or_else(|| project_label(path)),
            )
        })
        .collect();
    for (path, _) in folders {
        if removed.contains(path) {
            continue;
        }
        let name = folder_name[path.as_str()].clone();
        if !buckets.contains_key(&name) {
            order.push(name.clone());
            bucket_project.insert(name.clone(), Some(path.clone()));
        }
        buckets.entry(name).or_default();
    }
    for row in rows {
        let project = effective(row);
        let key = project
            .as_ref()
            .and_then(|p| folder_name.get(p.as_str()).cloned())
            .unwrap_or_else(|| project_label(project.as_deref().unwrap_or("")));
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

    fn row(id: &str, member: MemberKind, parent: Option<&str>) -> ContainerRow {
        ContainerRow {
            id: id.into(),
            title: format!("row {id}"),
            running: false,
            pending: None,
            unread: false,
            pinned: false,
            archived: false,
            rank: 0,
            parent_id: parent.map(|p| p.to_string()),
            member_kind: member,
            project: Some("/p/wire".into()),
            tag: None,
            updated_at: 0,
        }
    }

    #[test]
    fn derive_status_priority_order() {
        let mut r = row("s", MemberKind::Captain, None);
        assert_eq!(derive_status(&r), SessionStatus::Idle);
        r.unread = true;
        assert_eq!(derive_status(&r), SessionStatus::Unread);
        r.running = true;
        assert_eq!(derive_status(&r), SessionStatus::Running);
        r.pending = Some(PendingKind::Ask);
        assert_eq!(derive_status(&r), SessionStatus::Pending(PendingKind::Ask));
        r.pending = Some(PendingKind::Auth);
        assert_eq!(derive_status(&r), SessionStatus::Pending(PendingKind::Auth));
        r.pending = None;
        r.running = false;
        assert_eq!(derive_status(&r), SessionStatus::Unread);
        r.pending = Some(PendingKind::Auth);
        assert_eq!(derive_status(&r), SessionStatus::Pending(PendingKind::Auth));
    }

    /// The pending merge: the wire `InputNeeded` bit alone is Auth, the live
    /// fold's open ask is Ask, Ask wins when both faces are open, and the
    /// wire `Error` bit is consumed by nothing — a row carrying only Error
    /// (and a read flag) still derives Idle.
    #[test]
    fn pending_merge_prefers_ask_and_error_is_not_a_state() {
        let summary = |status: u32| -> SessionSummary {
            serde_json::from_value(serde_json::json!({
                "resource": "ahp-session:/s-1",
                "provider": "test",
                "title": "row",
                "status": status,
                "createdAt": "2026-01-01T00:00:00Z",
                "modifiedAt": "2026-01-01T00:00:00Z",
            }))
            .expect("a summary parses")
        };
        let both = WireStatus::InputNeeded.bits() | WireStatus::Error.bits();
        let r = ContainerRow::from_summary(&summary(both), false, true);
        assert_eq!(r.pending, Some(PendingKind::Ask));
        let r = ContainerRow::from_summary(&summary(both), false, false);
        assert_eq!(r.pending, Some(PendingKind::Auth));
        let read_error = WireStatus::Error.bits() | WireStatus::IsRead.bits();
        let r = ContainerRow::from_summary(&summary(read_error), false, false);
        assert_eq!(derive_status(&r), SessionStatus::Idle);
    }

    /// The member slot parses through [`ContainerRow::from_summary`]: a
    /// `subagent` memberKind with its parent edge reads a level-1 child, a
    /// `captain` annotation reads top-level, and a summary WITHOUT the slot
    /// (today's pinned runtime never sends it) reads a top-level captain —
    /// the fold must never hide what it does not recognize.
    #[test]
    fn from_summary_parses_the_member_slot() {
        let summary = |meta: serde_json::Value| -> SessionSummary {
            serde_json::from_value(serde_json::json!({
                "resource": "ahp-session:/s-1",
                "provider": "test",
                "title": "row",
                "status": 0,
                "createdAt": "2026-01-01T00:00:00Z",
                "modifiedAt": "2026-01-01T00:00:00Z",
                "_meta": meta,
            }))
            .expect("a summary parses")
        };
        let subagent = summary(serde_json::json!({
            "x-manox": { "memberKind": "subagent", "parentSessionId": "cap-1" },
        }));
        let r = ContainerRow::from_summary(&subagent, false, false);
        assert_eq!(r.member_kind, MemberKind::Subagent);
        assert_eq!(r.parent_id.as_deref(), Some("cap-1"));
        assert!(r.is_child());

        let captain = summary(serde_json::json!({
            "x-manox": { "memberKind": "captain" },
        }));
        let r = ContainerRow::from_summary(&captain, false, false);
        assert_eq!(r.member_kind, MemberKind::Captain);
        assert_eq!(r.parent_id, None);
        assert!(!r.is_child());

        let bare = summary(serde_json::json!({}));
        let r = ContainerRow::from_summary(&bare, false, false);
        assert_eq!(r.member_kind, MemberKind::Captain);
        assert_eq!(r.parent_id, None);
        assert!(!r.is_child(), "an unannotated row renders top-level");

        // A parent edge without a recognized memberKind still folds: the
        // tree edge is the plan's fold fact, not just the annotation.
        let parent_only = summary(serde_json::json!({
            "x-manox": { "parentSessionId": "cap-1" },
        }));
        assert!(ContainerRow::from_summary(&parent_only, false, false).is_child());
    }

    #[test]
    fn forest_nests_members_under_leaders_and_flattens_orphans() {
        let mut rows = vec![
            row("leader", MemberKind::Captain, None),
            row("member", MemberKind::Subagent, Some("leader")),
            row("orphan", MemberKind::Subagent, Some("gone")),
            row("top", MemberKind::Captain, None),
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
        // The unit shares one sort stamp AND one host rank (the captain's);
        // a flattened orphan and an unrelated top row keep their own.
        assert_eq!(out[0].sort_stamp, 300);
        assert_eq!(out[0].rank, 0);
        assert_eq!(out[1].sort_stamp, out[0].sort_stamp);
        assert_eq!(out[1].rank, out[0].rank);
        assert_eq!(out[2].sort_stamp, 0);
        assert_eq!(out[2].rank, 2);
        assert_eq!(out[3].rank, 9);
        // The leader's own facts name the leader's own rung.
        assert_eq!(out[0].status, SessionStatus::Idle);
    }

    #[test]
    fn groups_key_by_project_tail_and_chats_bucket() {
        let mut rows = vec![
            row("a", MemberKind::Captain, None),
            row("b", MemberKind::Captain, None),
        ];
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
        let mut rows = vec![row("a", MemberKind::Captain, None)];
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

    /// A non-empty alias names the folder but never re-keys it: the
    /// project's rows file under the alias head by PATH, not under a second
    /// basename-headed bucket beside an empty alias head.
    #[test]
    fn an_aliased_folder_absorbs_its_rows_by_path() {
        let mut rows = vec![
            row("a", MemberKind::Captain, None),
            row("b", MemberKind::Captain, None),
        ];
        rows[0].project = Some("/p/live".into());
        rows[1].project = Some("/p/live".into());
        let folders = vec![("/p/live".to_string(), Some("别名".to_string()))];
        let groups = project_groups(&rows, &folders, &HashSet::new());
        assert_eq!(groups.len(), 1, "one head, no basename twin");
        assert_eq!(groups[0].name, "别名");
        assert_eq!(groups[0].project.as_deref(), Some("/p/live"));
        assert_eq!(groups[0].rows.len(), 2);
    }

    /// A skeleton folder whose project was client-removed dissolves; its
    /// rows (if any) re-bucket loose.
    #[test]
    fn a_removed_skeleton_folder_dissolves() {
        let mut rows = vec![row("a", MemberKind::Captain, None)];
        rows[0].project = Some("/p/gone".into());
        let folders = vec![("/p/gone".to_string(), None)];
        let removed: HashSet<String> = ["/p/gone".to_string()].into_iter().collect();
        let groups = project_groups(&rows, &folders, &removed);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Chats");
    }

    /// The chrome re-sorts by `(pinned, rank)` locally; the projection
    /// carries the HOST rank verbatim so that sort reproduces the host
    /// order — no local timestamp reshuffles it. The sort stamp stays a
    /// real unix time (the recency buckets decode it), never a rank.
    #[test]
    fn the_rank_carries_the_host_order_and_the_stamp_stays_a_time() {
        let mut rows = [
            row("a", MemberKind::Captain, None),
            row("b", MemberKind::Captain, None),
        ];
        rows[0].rank = 0;
        rows[0].updated_at = 1_700_000_000;
        rows[1].rank = 1;
        rows[1].updated_at = 1_700_000_500;
        let projected: Vec<_> = rows.iter().map(project_row).collect();
        // Order key: the rank ascending reproduces the host order.
        let mut sorted = projected.clone();
        sorted.sort_by_key(|r| r.rank);
        assert_eq!(sorted[0].id, "a");
        assert_eq!(sorted[1].id, "b");
        // The stamp is the row's own last-active time, bucket-decodable.
        assert_eq!(projected[0].sort_stamp, 1_700_000_000);
        assert_eq!(projected[1].sort_stamp, 1_700_000_500);
    }

    /// Removed project paths re-bucket their rows loose (the v2
    /// remove-project semantics): the folder dissolves, the history stays.
    #[test]
    fn groups_carry_project_and_removed_paths_rebucket_to_loose() {
        let mut rows = vec![
            row("a", MemberKind::Captain, None),
            row("b", MemberKind::Captain, None),
            row("c", MemberKind::Captain, None),
        ];
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
