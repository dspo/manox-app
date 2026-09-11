//! The sidebar's client-local view state: which ordering mode is in effect, the
//! editable display order per partition, the observed interaction stamps that
//! make promotion fire exactly once, and the folded-group state.
//!
//! The split mirrors the durable order account on the server: the server owns
//! the *manual* account (activity never rewrites it, an explicit move does),
//! while this module owns the *view* — the sequence actually rendered. Two
//! consequences follow. Entering `Last updated` re-sorts the view once; from
//! then on only the row whose interaction stamp advanced moves (to the head),
//! and every other row keeps its position. `Manual` renders the server order and
//! never promotes.
//!
//! This file is presentation state, not conversation truth: it lives in the
//! shared state root but is owned by the desktop client, and an unreadable file
//! is simply the empty default (the first snapshot re-seeds it).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// The partition key for rows bound to no registered project. Mirrors
/// `manox_agent::sidebar_order::LOOSE`: the server owns an account for it too,
/// but the client keys its view order the same way.
pub const LOOSE: &str = "__loose__";

/// Session order behavior: fixed after edits, or additionally promoted by real
/// user interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderBy {
    /// The server's manual account, verbatim. Activity never moves a row.
    Manual,
    /// Manual order plus a one-time promotion of any row whose interaction
    /// stamp advanced. The default, because a list that tracks the thread you
    /// are working in is the familiar behavior — it just no longer drifts.
    #[default]
    Updated,
}

/// Everything the sidebar persists about how it displays the list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SidebarView {
    #[serde(default)]
    pub order_by: OrderBy,
    /// One editable display order per partition key (a registered project path
    /// or [`LOOSE`]); head = top row.
    #[serde(default)]
    pub account: HashMap<String, Vec<String>>,
    /// Last observed interaction stamp per partition per row — the delta source
    /// that makes a promotion fire once instead of on every repaint.
    #[serde(default)]
    pub observed: HashMap<String, HashMap<String, i64>>,
    /// Project folders the user collapsed. Persisted so the sidebar reopens the
    /// way it was left; absent = expanded.
    #[serde(default)]
    pub collapsed_folders: Vec<String>,
    /// Team leaders whose member subtree is folded. Absent = expanded.
    #[serde(default)]
    pub collapsed_teams: Vec<String>,
}

/// One partition's input to [`next_account`].
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    /// The row key: a thread id, or an `external:<id>` for a CLI session.
    pub id: &'a str,
    /// Unix seconds of the last human interaction (external rows carry their
    /// spawn time, which never changes).
    pub stamp: i64,
}

/// Reconcile a stored display order with the current membership: stored ids
/// first (still-present only), then ids the account has never seen prepended —
/// a row surfaces at the top, once. Never re-sorts.
pub fn reconciled(rows: &[Row<'_>], stored: Option<&[String]>) -> Vec<String> {
    let live: HashMap<&str, i64> = rows.iter().map(|r| (r.id, r.stamp)).collect();
    let mut out: Vec<String> = Vec::with_capacity(rows.len());
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    if let Some(stored) = stored {
        for id in stored {
            if live.contains_key(id.as_str()) && seen.insert(id.as_str()) {
                out.push(id.clone());
            }
        }
    }
    let mut fresh: Vec<&Row<'_>> = rows.iter().filter(|r| !seen.contains(r.id)).collect();
    // The one timestamp sort, and only for rows appearing for the first time.
    fresh.sort_by(|a, b| b.stamp.cmp(&a.stamp).then_with(|| a.id.cmp(b.id)));
    // Unseen ids take the head, not the tail: a row surfacing for the first
    // time is a new or re-added thread, and the server's manual account put it
    // there too.
    out.splice(0..0, fresh.into_iter().map(|r| r.id.to_string()));
    out
}

/// The next display order for one partition, or `None` when nothing moved (so
/// the caller neither writes the file nor repaints).
///
/// `sort_by_recency` marks the one complete sort that runs when the account is
/// first built or when the user switches into `Last updated`; afterwards the
/// only movement is the promotion of rows whose stamp advanced past what was
/// last observed.
pub fn next_account(
    rows: &[Row<'_>],
    stored: Option<&[String]>,
    observed: &HashMap<String, i64>,
    order_by: OrderBy,
    sort_by_recency: bool,
) -> Option<Vec<String>> {
    let mut order = reconciled(rows, stored);
    if order.is_empty() {
        return None;
    }
    let stamp: HashMap<&str, i64> = rows.iter().map(|r| (r.id, r.stamp)).collect();
    if sort_by_recency {
        order.sort_by(|a, b| compare_recency(a, b, &stamp));
    } else if order_by == OrderBy::Updated {
        let mut promoted: Vec<&String> = order
            .iter()
            .filter(|id| match observed.get(id.as_str()) {
                Some(prev) => stamp.get(id.as_str()).is_some_and(|at| *at > *prev),
                None => true,
            })
            .collect();
        if !promoted.is_empty() {
            promoted.sort_by(|a, b| compare_recency(a, b, &stamp));
            let moved: std::collections::HashSet<&str> =
                promoted.iter().map(|id| id.as_str()).collect();
            let rest: Vec<String> = order
                .iter()
                .filter(|id| !moved.contains(id.as_str()))
                .cloned()
                .collect();
            order = promoted.into_iter().cloned().chain(rest).collect();
        }
    }
    (stored != Some(order.as_slice())).then_some(order)
}

/// Newest stamp first, id as the deterministic tie-break: two rows sharing a
/// second must never swap places between repaints.
fn compare_recency(a: &str, b: &str, stamp: &HashMap<&str, i64>) -> std::cmp::Ordering {
    let left = stamp.get(a).copied().unwrap_or(i64::MIN);
    let right = stamp.get(b).copied().unwrap_or(i64::MIN);
    right.cmp(&left).then_with(|| a.cmp(b))
}

/// DOM `insertBefore` on the display order: move `id` in front of `before`, or
/// to the tail when `before` is `None`. Returns the new order, or `None` when
/// the move is invalid (unknown source/anchor) or already in place — so a
/// redundant drag neither repaints nor writes.
pub fn move_in_account(account: &[String], id: &str, before: Option<&str>) -> Option<Vec<String>> {
    if !account.iter().any(|x| x == id) {
        return None;
    }
    if let Some(anchor) = before {
        if !account.iter().any(|x| x == anchor) {
            return None;
        }
        if anchor == id {
            return None;
        }
    }
    let mut without: Vec<String> = account
        .iter()
        .filter(|x| x.as_str() != id)
        .cloned()
        .collect();
    let at = match before {
        None => without.len(),
        Some(anchor) => without.iter().position(|x| x == anchor)?,
    };
    without.insert(at, id.to_string());
    (without != *account).then_some(without)
}

/// The view-state file under the shared state root.
pub fn view_path() -> PathBuf {
    manox_agent::paths::manox_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("sidebar-view.json")
}

/// Load the persisted view state. Missing or unreadable = the default (the
/// first snapshot re-seeds every account).
pub fn load() -> SidebarView {
    load_from(&view_path())
}

/// [`load`] against an explicit path — the test seam.
pub fn load_from(path: &Path) -> SidebarView {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(path = %path.display(), %error, "sidebar view state unreadable; starting fresh");
            SidebarView::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SidebarView::default(),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "sidebar view state unreadable; starting fresh");
            SidebarView::default()
        }
    }
}

/// Atomic write (temp file + rename) so a crash cannot truncate the file.
pub fn save(view: &SidebarView) -> Result<()> {
    save_to(&view_path(), view)
}

/// [`save`] against an explicit path — the test seam.
pub fn save_to(path: &Path, view: &SidebarView) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(view)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Refresh the observed stamps for one partition to the current values, so a
/// promotion fires on the transition, not on every repaint that follows it.
pub fn observe(view: &mut SidebarView, partition: &str, rows: &[Row<'_>]) {
    let entry = view.observed.entry(partition.to_string()).or_default();
    entry.clear();
    for row in rows {
        entry.insert(row.id.to_string(), row.stamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(spec: &'static [(&'static str, i64)]) -> Vec<Row<'static>> {
        spec.iter()
            .map(|(id, stamp)| Row { id, stamp: *stamp })
            .collect()
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    fn observed(pairs: &[(&str, i64)]) -> HashMap<String, i64> {
        pairs.iter().map(|(id, at)| (id.to_string(), *at)).collect()
    }

    #[test]
    fn reconciled_keeps_stored_order_and_prepends_newcomers() {
        let stored = ids(&["a", "b"]);
        let order = reconciled(&rows(&[("a", 1), ("b", 2), ("c", 3)]), Some(&stored));
        assert_eq!(order, ids(&["c", "a", "b"]));
    }

    #[test]
    fn reconciled_drops_ids_that_left_the_list() {
        let stored = ids(&["gone", "a"]);
        let order = reconciled(&rows(&[("a", 1)]), Some(&stored));
        assert_eq!(order, ids(&["a"]));
    }

    #[test]
    fn a_first_account_or_mode_switch_sorts_by_recency_once() {
        let rows = rows(&[("a", 30), ("b", 10), ("c", 20)]);
        let order = next_account(&rows, None, &HashMap::new(), OrderBy::Manual, true).unwrap();
        assert_eq!(order, ids(&["a", "c", "b"]));
    }

    #[test]
    fn manual_mode_never_promotes_a_row() {
        let stored = ids(&["a", "b"]);
        let rows = rows(&[("a", 100), ("b", 1)]);
        let order = next_account(
            &rows,
            Some(&stored),
            &observed(&[("a", 1)]),
            OrderBy::Manual,
            false,
        );
        assert_eq!(order, None, "Manual must leave the displayed order alone");
    }

    #[test]
    fn updated_mode_promotes_only_the_row_whose_stamp_advanced() {
        let stored = ids(&["a", "b", "c"]);
        // `b` gained a new interaction; `c`'s stamp is unchanged.
        let rows = rows(&[("a", 1), ("b", 50), ("c", 2)]);
        let order = next_account(
            &rows,
            Some(&stored),
            &observed(&[("a", 1), ("b", 5), ("c", 2)]),
            OrderBy::Updated,
            false,
        )
        .unwrap();
        // The promoted row leads; the rest keep their stored relative order.
        assert_eq!(order, ids(&["b", "a", "c"]));
    }

    #[test]
    fn a_second_repaint_with_the_same_stamp_promotes_nobody() {
        let stored = ids(&["b", "a", "c"]);
        let rows = rows(&[("a", 1), ("b", 50), ("c", 2)]);
        let order = next_account(
            &rows,
            Some(&stored),
            &observed(&[("a", 1), ("b", 50), ("c", 2)]),
            OrderBy::Updated,
            false,
        );
        assert_eq!(order, None, "an already-promoted row must not move again");
    }

    #[test]
    fn an_unobserved_row_promotes_once_then_settles() {
        let rows = rows(&[("new", 7)]);
        let order = next_account(&rows, None, &HashMap::new(), OrderBy::Updated, false).unwrap();
        assert_eq!(order, ids(&["new"]));
        let settled = next_account(
            &rows,
            Some(&order),
            &observed(&[("new", 7)]),
            OrderBy::Updated,
            false,
        );
        assert_eq!(settled, None);
    }

    #[test]
    fn equal_stamps_never_swap() {
        let rows = rows(&[("bbb", 9), ("aaa", 9), ("ccc", 9)]);
        for _ in 0..4 {
            let order = next_account(&rows, None, &HashMap::new(), OrderBy::Updated, true).unwrap();
            assert_eq!(order, ids(&["aaa", "bbb", "ccc"]));
        }
    }

    #[test]
    fn an_empty_partition_produces_no_change() {
        let empty: Vec<Row<'static>> = Vec::new();
        assert_eq!(
            next_account(&empty, None, &HashMap::new(), OrderBy::Updated, true),
            None
        );
    }

    #[test]
    fn move_in_account_follows_insert_before_semantics() {
        let account = ids(&["a", "b", "c"]);
        assert_eq!(
            move_in_account(&account, "c", Some("a")),
            Some(ids(&["c", "a", "b"]))
        );
        assert_eq!(
            move_in_account(&account, "a", None),
            Some(ids(&["b", "c", "a"]))
        );
        assert_eq!(move_in_account(&account, "a", Some("b")), None);
        assert_eq!(move_in_account(&account, "a", Some("a")), None);
        assert_eq!(move_in_account(&account, "ghost", None), None);
        assert_eq!(move_in_account(&account, "a", Some("ghost")), None);
    }

    #[test]
    fn a_round_trip_survives_the_file_and_a_partial_file_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sidebar-view.json");
        let mut view = SidebarView {
            order_by: OrderBy::Manual,
            ..Default::default()
        };
        view.account.insert("/p/a".into(), ids(&["t1", "t2"]));
        view.observed.insert("/p/a".into(), observed(&[("t1", 5)]));
        view.collapsed_folders.push("/p/b".into());
        save_to(&path, &view).unwrap();
        assert_eq!(load_from(&path), view);
        assert!(
            !path.with_extension("json.tmp").exists(),
            "tmp file left behind"
        );

        std::fs::write(
            path.with_file_name("partial.json"),
            r#"{"order_by":"manual"}"#,
        )
        .unwrap();
        let partial = load_from(&dir.path().join("partial.json"));
        assert_eq!(partial.order_by, OrderBy::Manual);
        assert!(partial.account.is_empty() && partial.observed.is_empty());

        assert_eq!(
            load_from(&dir.path().join("absent.json")),
            SidebarView::default()
        );
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), SidebarView::default());
    }

    #[test]
    fn order_by_defaults_to_updated_and_serde_uses_snake_case() {
        assert_eq!(OrderBy::default(), OrderBy::Updated);
        assert_eq!(
            serde_json::to_string(&OrderBy::Manual).unwrap(),
            r#""manual""#
        );
        let view: SidebarView = serde_json::from_str("{}").unwrap();
        assert_eq!(view.order_by, OrderBy::Updated);
    }

    #[test]
    fn observe_replaces_the_snapshot_it_compares_against() {
        let mut view = SidebarView::default();
        view.account.insert("/p/a".into(), ids(&["t1", "t2"]));
        view.observed
            .insert("/p/a".into(), observed(&[("stale", 1)]));
        observe(&mut view, "/p/a", &rows(&[("t1", 3), ("t2", 4)]));
        let seen = &view.observed["/p/a"];
        assert!(
            !seen.contains_key("stale"),
            "dead ids accumulate in observed"
        );
        assert_eq!(seen["t1"], 3);
    }
}
