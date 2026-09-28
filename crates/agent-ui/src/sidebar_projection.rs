//! The multiplexer → chrome-sidebar projection (PLAN-CHROME-CHAT-SPLIT §5.3,
//! the D2 ruling): pure functions turning the authoritative wire rows
//! (`manox_protocol::ThreadListItem`, with the multiplexer's SessionStatus
//! deltas merged) into the chrome `SessionList` props. This module owns no
//! entity and no subscription — the assembly (Phase 4's shell swap) feeds it
//! snapshots on the multiplexer's notify, exactly like the agent-ui sidebar's
//! own `SidebarThreadItem::from_wire`, whose semantics this mirrors:
//!
//! - five-state machine: `errored` (danger triangle) → `pending_auth` /
//!   `pending_plan` (attention pulse) → `running` (blocks) → the client-owned
//!   unread mirror (static dot) → idle; the leaf's unread mirror wins over
//!   the deprecated wire flag (GW5);
//! - team forest: `parent_id`/`depth` rows nest under their leader
//!   (recency-ordered leaders, members trailing, `indent`/`team_leader`
//!   columns), rows whose parent is missing or archived flatten to
//!   top-level;
//! - the tag chip and pinned flag ride the row verbatim.

use std::collections::HashMap;

use manox_agent_chrome_ui::session_list::{SessionGroup, SessionRowData, SessionStatus};
use manox_protocol::ThreadListItem;

/// The client-owned unread mirrors (leaf entity ids → live flags), keyed by
/// thread id; `None` entries fall back to the wire row's flag.
pub type UnreadMirrors = HashMap<String, bool>;

/// Project one wire row (GW5: `unread_override` from the live leaf wins).
pub fn project_row(item: &ThreadListItem, unread_override: Option<bool>) -> SessionRowData {
    SessionRowData {
        id: item.id.clone(),
        title: item.title.clone(),
        time: String::new(),
        status: five_state(item, unread_override),
        pinned: item.pinned,
        unread: unread_override.unwrap_or(item.unread),
        tag: item.tag.clone(),
        indent: 0,
        team_leader: false,
    }
}

/// The five-state machine over a wire row.
pub fn five_state(item: &ThreadListItem, unread_override: Option<bool>) -> SessionStatus {
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
/// rows (indent 1, guide rail), one level deep — the wire's team nesting is
/// never deeper (pi sub-agents), so no recursion is needed. Orphans (a
/// parent that is missing, archived, or outside the partition) flatten to
/// top-level rather than vanishing.
pub fn project_forest(rows: &[ThreadListItem], unread: &UnreadMirrors) -> Vec<SessionRowData> {
    let by_id: HashMap<&str, &ThreadListItem> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.depth == 0 {
            let mut leader = project_row(row, unread.get(&row.id).copied());
            let members: Vec<&ThreadListItem> = rows
                .iter()
                .filter(|r| r.depth > 0 && r.parent_id.as_deref() == Some(row.id.as_str()))
                .collect();
            leader.team_leader = !members.is_empty();
            out.push(leader);
            for m in members {
                let mut member = project_row(m, unread.get(&m.id).copied());
                member.indent = 1;
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
                let mut flat = project_row(row, unread.get(&row.id).copied());
                flat.indent = 0;
                out.push(flat);
            }
        }
    }
    out
}

/// Group a full wire list into chrome `SessionGroup`s keyed by project
/// display name (the path's last segment; empty → "Chats"), applying
/// `project_forest` per partition.
pub fn project_groups(rows: &[ThreadListItem], unread: &UnreadMirrors) -> Vec<SessionGroup> {
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<ThreadListItem>> = HashMap::new();
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

    fn row(id: &str, depth: i32, parent: Option<&str>) -> ThreadListItem {
        ThreadListItem {
            id: id.into(),
            title: format!("row {id}"),
            updated_at: 0,
            running: false,
            unread: false,
            errored: false,
            pending_auth: false,
            pending_plan: false,
            background_work: false,
            model_id: "m".into(),
            pinned: false,
            archived: false,
            parent_id: parent.map(|p| p.to_string()),
            depth,
            project: Some("/p/wire".into()),
            tag: None,
            approval_mode: None,
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
        let rows = vec![
            row("leader", 0, None),
            row("member", 1, Some("leader")),
            row("orphan", 1, Some("gone")),
            row("top", 0, None),
        ];
        let out = project_forest(&rows, &UnreadMirrors::new());
        let ids: Vec<(&str, u8, bool)> = out
            .iter()
            .map(|r| (r.id.as_str(), r.indent, r.team_leader))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("leader", 0, true),
                ("member", 1, false),
                ("orphan", 0, false),
                ("top", 0, false),
            ]
        );
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
