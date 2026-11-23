//! The canvas layout: a pure, serializable tree of tab groups and splits.
//!
//! The shape and the semantics follow the nsh-pi experiment's `dock.ts`
//! (`DockAreaLayout`): a split's slots are its recorded sizes (the last one
//! takes what is left), and a tab group shows one of its panels. The JSON is
//! `{"center":{"type":"tabs","panels":["…"],"activeIndex":0}}` — the file is
//! ours, but the structure matches so the sheet host and the experiment's
//! reasoning carry over field for field.
//!
//! Two properties the whole model rests on:
//!
//! - **Layout is data, a sheet is a resource.** Dropping a node from the tree
//!   never ends a sheet; closing a sheet is a separate edit (`remove_sheet`).
//! - **Every transform is a pure function.** Nothing here stores anything —
//!   `state.rs` is the only writer of a stored layout, and it always takes a
//!   fresh tree from here.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// A split's stacking direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    /// Children side by side (the "split right" gesture).
    Horizontal,
    /// Children stacked (the "split down" gesture).
    Vertical,
}

/// One node of a canvas layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DockNode {
    /// Two or more child nodes laid out along `axis`.
    Split {
        axis: Axis,
        children: Vec<DockNode>,
        /// Each child's slot in points; `None` leaves the slots unconstrained
        /// (the last child takes whatever is left).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sizes: Option<Vec<f32>>,
    },
    /// A tab group: sheets in tab order, one of them displayed.
    Tabs {
        panels: Vec<String>,
        /// The displayed panel's index; absent means the first.
        #[serde(
            default,
            rename = "activeIndex",
            skip_serializing_if = "Option::is_none"
        )]
        active_index: Option<usize>,
    },
}

impl DockNode {
    /// An empty tab group — the center's empty state, and the root a canvas
    /// never loses (a layout always has somewhere to append).
    pub fn empty_tabs() -> Self {
        Self::Tabs {
            panels: Vec::new(),
            active_index: None,
        }
    }

    /// A tab group over `panels`, the first displayed.
    pub fn tabs(panels: impl IntoIterator<Item = String>) -> Self {
        let panels: Vec<String> = panels.into_iter().collect();
        let active_index = (!panels.is_empty()).then_some(0);
        Self::Tabs {
            panels,
            active_index,
        }
    }

    /// A split over `children` with unconstrained slots.
    pub fn split(axis: Axis, children: Vec<DockNode>) -> Self {
        Self::Split {
            axis,
            children,
            sizes: None,
        }
    }
}

/// One container's canvas: the layout tree of its center area.
///
/// v1 has no left/right/bottom docks — the chrome keeps its fixed right pane
/// and bottom dock as they are — so the file is exactly the center.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DockLayout {
    pub center: DockNode,
}

impl Default for DockLayout {
    fn default() -> Self {
        Self {
            center: DockNode::empty_tabs(),
        }
    }
}

impl DockLayout {
    /// A one-group layout over `sheets`, the first displayed.
    pub fn for_sheets(sheets: impl IntoIterator<Item = String>) -> Self {
        Self {
            center: DockNode::tabs(sheets),
        }
    }
}

/// Every sheet id in the layout, in tree order.
pub fn panel_ids(layout: &DockLayout) -> Vec<&str> {
    let mut ids = Vec::new();
    collect_panels(&layout.center, &mut ids);
    ids
}

fn collect_panels<'a>(node: &'a DockNode, out: &mut Vec<&'a str>) {
    match node {
        DockNode::Tabs { panels, .. } => out.extend(panels.iter().map(String::as_str)),
        DockNode::Split { children, .. } => {
            for child in children {
                collect_panels(child, out);
            }
        }
    }
}

/// How many sheets the layout holds.
pub fn sheet_count(layout: &DockLayout) -> usize {
    panel_ids(layout).len()
}

/// Whether `id` is in the layout at all.
pub fn has_sheet(layout: &DockLayout, id: &str) -> bool {
    panel_ids(layout).contains(&id)
}

/// The sheets a user can see now: each group's displayed tab, or only the
/// zoomed one when a zoom is in effect.
pub fn active_panel_ids<'a>(layout: &'a DockLayout, zoomed: Option<&'a str>) -> Vec<&'a str> {
    if let Some(zoom) = zoomed {
        return if has_sheet(layout, zoom) {
            vec![zoom]
        } else {
            Vec::new()
        };
    }
    let mut active = Vec::new();
    collect_active(&layout.center, &mut active);
    active
}

fn collect_active<'a>(node: &'a DockNode, out: &mut Vec<&'a str>) {
    match node {
        DockNode::Tabs {
            panels,
            active_index,
        } => {
            if let Some(panel) = panels.get(active_index.unwrap_or(0)) {
                out.push(panel);
            }
        }
        DockNode::Split { children, .. } => {
            for child in children {
                collect_active(child, out);
            }
        }
    }
}

/// Displays an open sheet: its group's `activeIndex` moves to it.
pub fn activate_sheet(layout: &DockLayout, id: &str) -> DockLayout {
    let mut center = layout.center.clone();
    if activate_in(&mut center, id) {
        DockLayout { center }
    } else {
        layout.clone()
    }
}

fn activate_in(root: &mut DockNode, id: &str) -> bool {
    let mut contains = |panels: &[String]| panels.iter().any(|panel| panel == id);
    let Some(path) = find_tabs(root, &mut contains) else {
        return false;
    };
    let mut changed = false;
    with_node(root, &path, &mut |node| {
        if let DockNode::Tabs {
            panels,
            active_index,
        } = node
            && let Some(at) = panels.iter().position(|panel| panel == id)
            && *active_index != Some(at)
        {
            *active_index = Some(at);
            changed = true;
        }
    });
    changed
}

/// Appends a sheet to the group of `prefer` (the shown sheet), else the first
/// group, and displays it.
///
/// A sheet is one instance: appending an id the layout already holds only
/// activates it. This is the "open" primitive every kind goes through, so a
/// restored layout can never grow a duplicate tab.
pub fn append_sheet(layout: &DockLayout, id: &str, prefer: Option<&str>) -> DockLayout {
    if has_sheet(layout, id) {
        return activate_sheet(layout, id);
    }
    let mut center = layout.center.clone();
    let preferred = prefer.and_then(|prefer| {
        let mut contains = |panels: &[String]| panels.iter().any(|panel| panel == prefer);
        find_tabs(&center, &mut contains)
    });
    let target = preferred.or_else(|| find_tabs(&center, &mut |_| true));
    let Some(path) = target else {
        return DockLayout::for_sheets([id.to_string()]);
    };
    with_node(&mut center, &path, &mut |node| {
        if let DockNode::Tabs {
            panels,
            active_index,
        } = node
        {
            panels.push(id.to_string());
            *active_index = Some(panels.len() - 1);
        }
    });
    DockLayout { center }
}

/// Removes a sheet: its tab goes, an emptied group goes with it, and an
/// emptied center stays as an empty tab group.
pub fn remove_sheet(layout: &DockLayout, id: &str) -> DockLayout {
    DockLayout {
        center: remove_from(&layout.center, id).unwrap_or_else(DockNode::empty_tabs),
    }
}

fn remove_from(node: &DockNode, id: &str) -> Option<DockNode> {
    match node {
        DockNode::Tabs {
            panels,
            active_index,
        } => {
            let panels: Vec<String> = panels
                .iter()
                .filter(|panel| panel.as_str() != id)
                .cloned()
                .collect();
            if panels.is_empty() {
                return None;
            }
            // Keep the displayed tab where it was, or fall back to the last.
            let active = active_index.unwrap_or(0).min(panels.len() - 1);
            Some(DockNode::Tabs {
                panels,
                active_index: Some(active),
            })
        }
        DockNode::Split {
            axis,
            children,
            sizes,
        } => {
            let children: Vec<DockNode> = children
                .iter()
                .filter_map(|child| remove_from(child, id))
                .collect();
            // A split needs two children: an emptied one collapses into what
            // is left, so a one-child split never reaches the renderer.
            match children.len() {
                0 => None,
                1 => Some(children.into_iter().next().expect("one child")),
                // Recorded sizes are advisory: a removed child's slot simply
                // no longer applies, so the remaining ones keep their values.
                _ => Some(DockNode::Split {
                    axis: *axis,
                    children,
                    sizes: sizes.clone(),
                }),
            }
        }
    }
}

/// Puts `to` where `from` was — same group, same tab slot — and displays it.
///
/// This is the start page's action row: an action replaces the start page's
/// tab instead of leaving it behind next to its target. Without `from` (it
/// was closed first), `to` is appended.
pub fn replace_sheet(layout: &DockLayout, from: &str, to: &str) -> DockLayout {
    if from == to {
        return activate_sheet(layout, to);
    }
    let base = if has_sheet(layout, to) {
        remove_sheet(layout, to)
    } else {
        layout.clone()
    };
    let mut center = base.center.clone();
    let mut contains = |panels: &[String]| panels.iter().any(|panel| panel == from);
    let Some(path) = find_tabs(&center, &mut contains) else {
        return append_sheet(&base, to, None);
    };
    with_node(&mut center, &path, &mut |node| {
        if let DockNode::Tabs {
            panels,
            active_index,
        } = node
        {
            let at = panels.iter().position(|panel| panel == from);
            for panel in panels.iter_mut() {
                if panel == from {
                    *panel = to.to_string();
                }
            }
            if let Some(at) = at {
                *active_index = Some(at);
            }
        }
    });
    DockLayout { center }
}

/// Puts `id` in a new group split to the right of the group showing
/// `beside`. The neighbor keeps its tabs; the new group holds only `id`.
///
/// A sheet is one instance: a layout that already carries `id` **moves** it
/// into the new group (its old group collapses when that was its last tab)
/// rather than duplicating the tab — the same rule a tab dragged to an edge
/// obeys. A `beside` nobody shows falls back to appending.
pub fn split_right(layout: &DockLayout, beside: &str, id: &str) -> DockLayout {
    if beside == id {
        return activate_sheet(layout, id);
    }
    let base = if has_sheet(layout, id) {
        remove_sheet(layout, id)
    } else {
        layout.clone()
    };
    let mut center = base.center.clone();
    let mut contains = |panels: &[String]| panels.iter().any(|panel| panel == beside);
    let Some(path) = find_tabs(&center, &mut contains) else {
        return append_sheet(&base, id, None);
    };
    with_node(&mut center, &path, &mut |node| {
        let group = std::mem::replace(node, DockNode::empty_tabs());
        *node = DockNode::split(
            Axis::Horizontal,
            vec![group, DockNode::tabs([id.to_string()])],
        );
    });
    DockLayout { center }
}

/// Drops sheets outside `keep` (their resource is gone), de-duplicates
/// repeated ids within a group, and collapses groups that end up empty.
///
/// Restore reads a stored layout through this before activating it: a layout
/// is a claim about the past, and only surviving resources may be shown.
pub fn prune_layout(layout: &DockLayout, keep: &HashSet<String>) -> DockLayout {
    DockLayout {
        center: prune_node(&layout.center, keep).unwrap_or_else(DockNode::empty_tabs),
    }
}

fn prune_node(node: &DockNode, keep: &HashSet<String>) -> Option<DockNode> {
    match node {
        DockNode::Tabs {
            panels,
            active_index,
        } => {
            let mut seen = HashSet::new();
            let panels: Vec<String> = panels
                .iter()
                .filter(|panel| keep.contains(panel.as_str()) && seen.insert((*panel).clone()))
                .cloned()
                .collect();
            if panels.is_empty() {
                return None;
            }
            let active = active_index.unwrap_or(0).min(panels.len() - 1);
            Some(DockNode::Tabs {
                panels,
                active_index: Some(active),
            })
        }
        DockNode::Split {
            axis,
            children,
            sizes,
        } => {
            let children: Vec<DockNode> = children
                .iter()
                .filter_map(|child| prune_node(child, keep))
                .collect();
            match children.len() {
                0 => None,
                1 => Some(children.into_iter().next().expect("one child")),
                _ => Some(DockNode::Split {
                    axis: *axis,
                    children,
                    sizes: sizes.clone(),
                }),
            }
        }
    }
}

/// The path of the first tab group matching `pred`, as child indices from the
/// root.
fn find_tabs<F>(root: &DockNode, pred: &mut F) -> Option<Vec<usize>>
where
    F: FnMut(&[String]) -> bool,
{
    fn walk<F>(node: &DockNode, pred: &mut F, path: &mut Vec<usize>) -> Option<Vec<usize>>
    where
        F: FnMut(&[String]) -> bool,
    {
        match node {
            DockNode::Tabs { panels, .. } => pred(panels).then(|| path.clone()),
            DockNode::Split { children, .. } => {
                for (index, child) in children.iter().enumerate() {
                    path.push(index);
                    if let Some(found) = walk(child, pred, path) {
                        return Some(found);
                    }
                    path.pop();
                }
                None
            }
        }
    }
    walk(root, pred, &mut Vec::new())
}

/// Applies `edit` to the node at `path`.
fn with_node(root: &mut DockNode, path: &[usize], edit: &mut impl FnMut(&mut DockNode)) {
    let Some((&head, rest)) = path.split_first() else {
        edit(root);
        return;
    };
    if let DockNode::Split { children, .. } = root
        && let Some(child) = children.get_mut(head)
    {
        with_node(child, rest, edit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(layout: &DockLayout) -> Vec<&str> {
        panel_ids(layout)
    }

    fn keep(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| (*id).to_string()).collect()
    }

    #[test]
    fn append_adds_one_tab_and_shows_it() {
        let layout = DockLayout::for_sheets(["home:c1".to_string()]);
        let layout = append_sheet(&layout, "session:s1", None);
        assert_eq!(ids(&layout), ["home:c1", "session:s1"]);
        assert_eq!(active_panel_ids(&layout, None), ["session:s1"]);
    }

    #[test]
    fn append_is_one_instance_per_sheet() {
        let layout = DockLayout::for_sheets(["home:c1".to_string()]);
        let layout = append_sheet(&layout, "session:s1", None);
        let again = append_sheet(&layout, "session:s1", Some("home:c1"));
        assert_eq!(ids(&again), ["home:c1", "session:s1"]);
        // Re-opening an existing sheet only displays it.
        assert_eq!(active_panel_ids(&again, None), ["session:s1"]);
        assert_eq!(sheet_count(&again), 2);
    }

    #[test]
    fn append_prefers_the_group_of_the_given_sheet() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let layout = append_sheet(&layout, "c", Some("b"));
        let center = &layout.center;
        let DockNode::Split { children, .. } = center else {
            panic!("the layout stays a split");
        };
        assert_eq!(ids(&layout), ["a", "b", "c"]);
        assert!(matches!(&children[0], DockNode::Tabs { panels, .. } if panels == &["a"]));
        assert!(matches!(&children[1], DockNode::Tabs { panels, .. } if panels == &["b", "c"]));
    }

    #[test]
    fn append_falls_back_to_the_first_group() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let layout = append_sheet(&layout, "c", Some("nobody-shows-this"));
        assert_eq!(ids(&layout), ["a", "c", "b"]);
    }

    #[test]
    fn append_into_an_empty_canvas_creates_the_group() {
        let layout = DockLayout::default();
        let layout = append_sheet(&layout, "home:c1", None);
        assert_eq!(ids(&layout), ["home:c1"]);
        assert_eq!(active_panel_ids(&layout, None), ["home:c1"]);
    }

    #[test]
    fn split_right_grafts_beside_the_shown_group() {
        let layout = DockLayout::for_sheets(["a".to_string(), "a2".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let DockNode::Split {
            axis: Axis::Horizontal,
            children,
            sizes,
        } = &layout.center
        else {
            panic!("the center becomes a horizontal split");
        };
        assert_eq!(sizes, &None);
        assert!(matches!(&children[0], DockNode::Tabs { panels, .. } if panels == &["a", "a2"]));
        assert!(matches!(&children[1], DockNode::Tabs { panels, .. } if panels == &["b"]));
        // The neighbor group keeps its displayed tab.
        assert_eq!(active_panel_ids(&layout, None), ["a", "b"]);
    }

    #[test]
    fn split_right_of_a_sheet_already_open_moves_it_into_the_new_group() {
        let layout = DockLayout::for_sheets(["a".to_string(), "b".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let DockNode::Split { children, .. } = &layout.center else {
            panic!("splitting an open sheet grafts it instead of doing nothing");
        };
        // One instance: the tab left its old group instead of duplicating.
        assert!(matches!(&children[0], DockNode::Tabs { panels, .. } if panels == &["a"]));
        assert!(matches!(&children[1], DockNode::Tabs { panels, .. } if panels == &["b"]));
        assert_eq!(ids(&layout), ["a", "b"]);
        assert_eq!(active_panel_ids(&layout, None), ["a", "b"]);
    }

    #[test]
    fn split_right_of_a_sheet_nobody_shows_appends() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = split_right(&layout, "ghost", "b");
        assert_eq!(ids(&layout), ["a", "b"]);
        assert!(matches!(layout.center, DockNode::Tabs { .. }));
    }

    #[test]
    fn remove_drops_the_tab_and_its_emptied_group() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let layout = remove_sheet(&layout, "b");
        assert_eq!(ids(&layout), ["a"]);
        assert!(matches!(layout.center, DockNode::Tabs { .. }));
    }

    #[test]
    fn removing_the_last_sheet_leaves_an_empty_group() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = remove_sheet(&layout, "a");
        assert_eq!(sheet_count(&layout), 0);
        assert!(
            matches!(&layout.center, DockNode::Tabs { panels, active_index } if panels.is_empty() && active_index.is_none())
        );
    }

    #[test]
    fn remove_keeps_the_displayed_tab_in_range() {
        let layout = DockLayout::for_sheets(["a".to_string(), "b".to_string(), "c".to_string()]);
        let layout = activate_sheet(&layout, "c");
        let layout = remove_sheet(&layout, "c");
        // The slot stays, clamped to the last remaining tab.
        assert_eq!(active_panel_ids(&layout, None), ["b"]);
    }

    #[test]
    fn replace_puts_the_target_in_the_removed_slot() {
        let layout = DockLayout::for_sheets(["home:c1".to_string()]);
        let layout = append_sheet(&layout, "session:s1", None);
        let layout = replace_sheet(&layout, "home:c1", "terminal:c1:1");
        assert_eq!(ids(&layout), ["terminal:c1:1", "session:s1"]);
        assert_eq!(active_panel_ids(&layout, None), ["terminal:c1:1"]);
    }

    #[test]
    fn replace_moves_an_already_open_target_instead_of_duplicating_it() {
        let layout = DockLayout::for_sheets(["home:c1".to_string()]);
        let layout = append_sheet(&layout, "session:s1", None);
        let layout = replace_sheet(&layout, "home:c1", "session:s1");
        assert_eq!(ids(&layout), ["session:s1"]);
        assert_eq!(active_panel_ids(&layout, None), ["session:s1"]);
    }

    #[test]
    fn replace_without_the_source_appends() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = replace_sheet(&layout, "closed", "b");
        assert_eq!(ids(&layout), ["a", "b"]);
    }

    #[test]
    fn prune_keeps_only_surviving_ids_and_drops_duplicates() {
        let mut panels = vec!["a".to_string(), "dead".to_string(), "a".to_string()];
        panels.push("b".to_string());
        let layout = DockLayout::for_sheets(panels);
        let layout = append_sheet(&layout, "c", None);
        let pruned = prune_layout(&layout, &keep(&["a", "b"]));
        assert_eq!(ids(&pruned), ["a", "b"]);
        assert_eq!(active_panel_ids(&pruned, None).len(), 1);
    }

    #[test]
    fn prune_collapses_the_tree_when_everything_is_dead() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let pruned = prune_layout(&layout, &keep(&[]));
        assert_eq!(sheet_count(&pruned), 0);
        assert!(matches!(pruned.center, DockNode::Tabs { .. }));
    }

    #[test]
    fn a_zoomed_sheet_is_the_only_visible_one() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = append_sheet(&layout, "b", None);
        assert_eq!(active_panel_ids(&layout, Some("a")), ["a"]);
        assert!(active_panel_ids(&layout, Some("ghost")).is_empty());
    }

    #[test]
    fn the_json_shape_matches_the_layout_contract() {
        let layout = DockLayout::for_sheets(["home:c1".to_string()]);
        let value = serde_json::to_value(&layout).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "center": { "type": "tabs", "panels": ["home:c1"], "activeIndex": 0 }
            })
        );
        let back: DockLayout = serde_json::from_value(value).unwrap();
        assert_eq!(back, layout);
    }

    #[test]
    fn splits_round_trip_through_json() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let json = serde_json::to_string(&layout).unwrap();
        let back: DockLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(back, layout);
    }
}
