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

use std::collections::HashMap;

use ahp_types::state::{SessionStatus as WireStatus, SessionSummary};
use steer_agent_chrome_ui::session_list::{
    LOOSE_BUCKET_KEY, PendingKind, SessionGroup, SessionRowData, SessionRowKind, SessionStatus,
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
    /// The host's recency stamp (`_meta.x-manox.lastMessageAt`, unix
    /// seconds): present iff the transcript holds at least one message —
    /// absence is "not rankable", never "very old". The session picker
    /// ranks by it; the sidebar's order ignores it (the host rank owns
    /// that).
    pub last_message_at: Option<i64>,
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
    /// does (`ext_and_meta_pinned`); a summary without the slot reads a
    /// top-level captain with no parent.
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
            last_message_at: summary
                .meta
                .as_ref()
                .and_then(manox_ahp::summary_meta::last_message_at),
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
        last_message_at: item.last_message_at,
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
/// captain's so the unit sorts (and survives a pin re-order) as one; the
/// recency stamp rides the same restamp, so a child that ever reaches a
/// recency surface carries its unit's key, not its own.
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
            let unit_recency = leader.last_message_at;
            out.push(leader);
            for m in members {
                let mut member = project_row(m);
                member.sort_stamp = unit_stamp;
                member.rank = unit_rank;
                member.last_message_at = unit_recency;
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

/// Group a full wire list into chrome `SessionGroup`s keyed by project id
/// (the loose bucket keys by [`steer_agent_chrome_ui::session_list::
/// LOOSE_BUCKET_KEY`]), applying `project_forest` per partition. The data
/// source is the project registry — `projects` in sidebar order and the
/// membership resolver — never the wire rows' own project paths: grouping is
/// an explicit fact (design §2.3), so a container without a membership (an
/// ad-hoc folder, an external container, another client's session) falls to
/// the loose bucket, and a membership naming a deleted project reads loose
/// too. Empty project groups still render (the skeleton); the loose bucket
/// renders only when something lands in it.
pub fn project_groups(
    rows: &[ContainerRow],
    projects: &[crate::projects::Project],
    membership: &dyn Fn(&str) -> Option<String>,
) -> Vec<SessionGroup> {
    let loose_name = loose_bucket_name();
    let mut groups: Vec<(SessionGroup, Vec<ContainerRow>)> = projects
        .iter()
        .map(|project| {
            (
                SessionGroup {
                    key: project.id.clone(),
                    name: crate::projects::display_name(project),
                    project: Some(project.id.clone()),
                    roots: project
                        .roots
                        .iter()
                        .map(|root| root.path.to_string_lossy().into_owned())
                        .collect(),
                    rows: Vec::new(),
                    collapsed: false,
                    hidden: 0,
                    expanded: false,
                },
                Vec::new(),
            )
        })
        .collect();
    let mut loose: Vec<ContainerRow> = Vec::new();
    for row in rows {
        let target = membership(&row.id)
            .and_then(|project_id| groups.iter_mut().find(|(g, _)| g.key == project_id));
        match target {
            Some((_, bucket)) => bucket.push(row.clone()),
            None => loose.push(row.clone()),
        }
    }
    if !loose.is_empty() {
        groups.push((
            SessionGroup {
                key: LOOSE_BUCKET_KEY.to_string(),
                name: loose_name,
                project: None,
                roots: Vec::new(),
                rows: Vec::new(),
                collapsed: false,
                hidden: 0,
                expanded: false,
            },
            loose,
        ));
    }
    groups
        .into_iter()
        .map(|(mut group, bucket)| {
            group.rows = project_forest(bucket.as_slice());
            group
        })
        .collect()
}

/// The loose bucket's display name (the localized "Chats" word); the state
/// key stays the literal [`LOOSE_BUCKET_KEY`].
pub(crate) fn loose_bucket_name() -> String {
    crate::i18n::t(LOOSE_BUCKET_KEY_LOCALIZED).to_string()
}

const LOOSE_BUCKET_KEY_LOCALIZED: &str = "chrome-sidebar-chats";

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
            last_message_at: None,
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
    /// reads a top-level captain — the fold must never hide what it does
    /// not recognize.
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

    /// The recency stamp rides the whole projection hop: a summary whose
    /// `_meta` carries the key reads it through [`ContainerRow::from_summary`],
    /// [`project_row`] hands it to the chrome row, and the forest restamp
    /// gives a child its captain's unit stamp — a dropped line anywhere in
    /// that chain would silently empty the picker with every other test
    /// still green.
    #[test]
    fn the_recency_stamp_rides_the_projection_hop() {
        let summary_with = |meta: serde_json::Value| -> SessionSummary {
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
        use manox_ahp::summary_meta::set_last_message_at;
        let mut meta = serde_json::Map::new();
        set_last_message_at(&mut meta, 1_700_000_000);
        let stamped = ContainerRow::from_summary(
            &summary_with(serde_json::Value::Object(meta)),
            false,
            false,
        );
        assert_eq!(stamped.last_message_at, Some(1_700_000_000));
        assert_eq!(project_row(&stamped).last_message_at, Some(1_700_000_000));

        // A summary without the key reads absent — absence is "not
        // rankable", never "very old".
        let bare = ContainerRow::from_summary(&summary_with(serde_json::json!({})), false, false);
        assert_eq!(bare.last_message_at, None);
        assert_eq!(project_row(&bare).last_message_at, None);

        // The forest gives a child its captain's unit stamp.
        let mut captain = row("cap", MemberKind::Captain, None);
        captain.last_message_at = Some(42);
        let mut kid = row("kid", MemberKind::Subagent, Some("cap"));
        kid.last_message_at = Some(7);
        let out = project_forest(vec![captain, kid].as_slice());
        assert_eq!(
            out[1].last_message_at,
            Some(42),
            "the child rides the unit's stamp"
        );
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
    fn groups_key_by_membership_and_the_loose_bucket_trails() {
        let projects = vec![project("p1", "Steer.app", &["/p/steer"])];
        let mut rows = vec![
            row("a", MemberKind::Captain, None),
            row("b", MemberKind::Captain, None),
        ];
        rows[0].project = Some("/p/steer".into());
        rows[1].tag = Some("mytag".into());
        rows[1].pinned = true;
        let member = |id: &str| (id == "a").then(|| "p1".to_string());
        let groups = project_groups(&rows, &projects, &member);
        let keys: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["p1", "Chats"],
            "membership group first, loose last"
        );
        assert_eq!(groups[0].name, "Steer.app");
        assert_eq!(groups[0].project.as_deref(), Some("p1"));
        let chats = &groups[1];
        assert_eq!(chats.project, None);
        assert_eq!(chats.rows.len(), 1);
        assert_eq!(chats.rows[0].tag.as_deref(), Some("mytag"));
        assert!(chats.rows[0].pinned);
    }

    /// The project skeleton renders empty projects in the given (position)
    /// order, the display name follows the registry's rule (override or the
    /// primary root's basename), and a multi-root project carries its roots
    /// for the header's count badge + tooltip.
    #[test]
    fn skeleton_renders_empty_projects_with_names_and_roots() {
        let multi = project("p1", "", &["/p/manox", "/p/manox-app"]);
        let named = project("p2", "自定义名", &["/p/lib"]);
        let projects = vec![multi, named];
        let groups = project_groups(&[], &projects, &|_| None);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[0].name, "manox",
            "uncustomized follows the primary basename"
        );
        assert_eq!(groups[0].roots, vec!["/p/manox", "/p/manox-app"]);
        assert_eq!(groups[1].name, "自定义名");
        assert_eq!(groups[1].roots.len(), 1);
        assert!(
            groups.iter().all(|g| g.rows.is_empty()),
            "an emptied project still renders"
        );
    }

    /// A membership naming a project that is not in the list (deleted
    /// between the write and this projection) reads loose; with nothing
    /// loose the bucket does not render at all.
    #[test]
    fn a_dangling_membership_reads_loose_and_an_empty_bucket_vanishes() {
        let projects = vec![project("p1", "P", &["/p/one"])];
        let rows = vec![row("a", MemberKind::Captain, None)];
        let groups = project_groups(&rows, &projects, &|_| Some("gone".to_string()));
        assert_eq!(groups.len(), 2, "the p1 skeleton stays, the row goes loose");
        assert_eq!(groups[0].key, "p1");
        assert!(groups[0].rows.is_empty());
        assert_eq!(groups[1].key, LOOSE_BUCKET_KEY);
        assert_eq!(groups[1].rows.len(), 1, "the row re-buckets, not vanishes");

        let groups = project_groups(&rows, &projects, &|id: &str| {
            (id == "a").then(|| "p1".to_string())
        });
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].key, "p1", "no loose bucket with nothing loose");
    }

    /// Two projects whose roots overlap stay two groups: membership decides
    /// the bucket, never the wire path (the same directory can sit in
    /// several projects).
    #[test]
    fn overlapping_roots_stay_two_groups_by_membership() {
        let projects = vec![
            project("p1", "A", &["/p/shared"]),
            project("p2", "B", &["/p/shared"]),
        ];
        let rows = vec![
            row("a", MemberKind::Captain, None),
            row("b", MemberKind::Captain, None),
        ];
        let groups = project_groups(&rows, &projects, &|id: &str| match id {
            "a" => Some("p1".to_string()),
            "b" => Some("p2".to_string()),
            _ => None,
        });
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].rows.len(), 1);
        assert_eq!(groups[1].rows.len(), 1);
        assert_eq!(groups[0].name, "A");
        assert_eq!(groups[1].name, "B");
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

    fn project(id: &str, name: &str, roots: &[&str]) -> crate::projects::Project {
        crate::projects::Project {
            id: id.to_string(),
            name: name.to_string(),
            name_overridden: !name.is_empty(),
            roots: roots
                .iter()
                .enumerate()
                .map(|(i, path)| crate::projects::ProjectRoot {
                    path: std::path::PathBuf::from(path),
                    primary: i == 0,
                })
                .collect(),
            position: 0,
            created_at: 0,
        }
    }
}
