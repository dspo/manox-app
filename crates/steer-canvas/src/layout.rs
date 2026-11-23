//! The canvas layout: a pure, serializable tree of tab groups and splits,
//! plus the three edge docks (`left` / `right` / `bottom`) around the center.
//!
//! The shape and the semantics follow the nsh-pi experiment's `dock.ts`
//! (`DockAreaLayout`): a split's slots are its recorded sizes (the last one
//! takes what is left), a tab group shows one of its panels, and a dock is an
//! optional region with its own tree, a cross-edge size and an open flag. The
//! JSON stays exactly the center when no dock exists, so a v1 layout file
//! reads back unchanged.
//!
//! Three properties the whole model rests on:
//!
//! - **Layout is data, a sheet is a resource.** Dropping a node from the tree
//!   never ends a sheet; closing a sheet is a separate edit (`remove_sheet`).
//! - **Every transform is a pure function.** Nothing here stores anything —
//!   `state.rs` is the only writer of a stored layout, and it always takes a
//!   fresh tree from here.
//! - **Placement has one primitive.** Every drop — a tab slot, a group, a
//!   split at a group edge, a dock region — goes through [`move_sheet`]; the
//!   older named transforms (`split_right`) are thin wrappers over it.
//!
//! Every walk covers the center **and** the dock regions: a sheet parked in a
//! closed dock is still a resource the union recycles against, so a walk that
//! skips a region leaks PTYs, mis-judges the container-death check, or prunes
//! a restore away.

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

/// Which edge of the canvas a dock occupies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSide {
    Left,
    Right,
    Bottom,
}

impl DockSide {
    /// All three docks in their geometry order (the order region walks use).
    pub const ALL: [DockSide; 3] = [DockSide::Left, DockSide::Right, DockSide::Bottom];

    /// The stable wire name (hooks carry this, not the enum — the chrome
    /// stays free of the canvas crate).
    pub fn name(self) -> &'static str {
        match self {
            DockSide::Left => "left",
            DockSide::Right => "right",
            DockSide::Bottom => "bottom",
        }
    }

    /// The side a wire name names; unknown names answer `None`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "left" => Some(DockSide::Left),
            "right" => Some(DockSide::Right),
            "bottom" => Some(DockSide::Bottom),
            _ => None,
        }
    }
}

/// Which edge of a group a split lands on. The edge names the axis: left and
/// right split side by side, top and bottom stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl SplitEdge {
    pub fn axis(self) -> Axis {
        match self {
            SplitEdge::Left | SplitEdge::Right => Axis::Horizontal,
            SplitEdge::Top | SplitEdge::Bottom => Axis::Vertical,
        }
    }

    /// Whether the new group goes before the anchor group along the axis
    /// (left/top) or after it (right/bottom).
    pub fn leads(self) -> bool {
        matches!(self, SplitEdge::Left | SplitEdge::Top)
    }
}

/// Where a drag would land. `anchor` is a member of the target group — the
/// same key the projection's tab boxes carry — so a destination survives one
/// layout edit between the hit test and the commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// A position in the anchor group's tab strip: between `index - 1` and
    /// `index` (an `index` of `len` means "after the last tab").
    TabSlot { anchor: String, index: usize },
    /// Somewhere on the anchor group's content area: appended and shown.
    Group { anchor: String },
    /// A new group split off the anchor group's edge.
    Split { anchor: String, side: SplitEdge },
    /// The dock region on `side` (only ever an open one — the drop hit test
    /// does not produce closed docks).
    Dock { side: DockSide },
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

/// A dock region: one edge's tree, its cross-edge size and its open flag.
///
/// v2 keeps one group per dock (the gestures never build a tree inside a
/// region); the field stays a node so a future tree needs no format change.
/// A `size` of `None` reads as [`DOCK_DEFAULT_SIZE`]; a stored size is
/// clamped at [`DOCK_MIN_SIZE`] here and at the available space by the
/// projection (the upper bound depends on the window, not the layout).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DockRegion {
    pub layout: DockNode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    #[serde(default = "default_open")]
    pub open: bool,
}

fn default_open() -> bool {
    true
}

impl Default for DockRegion {
    fn default() -> Self {
        Self {
            layout: DockNode::empty_tabs(),
            size: None,
            open: true,
        }
    }
}

/// A dock's cross-edge size when none is stored.
pub const DOCK_DEFAULT_SIZE: f32 = 200.;
/// The smallest dock width/height a layout may record (below this the drag
/// collapses the dock instead of shrinking it).
pub const DOCK_MIN_SIZE: f32 = 100.;
/// The center's guaranteed width/height the projection reserves against the
/// docks' clamps.
pub const CENTER_MIN: f32 = 200.;

/// One container's canvas: the center tree plus up to three edge docks.
///
/// Absent or `open = false` regions take no space; their sheets stay in the
/// layout (a closed dock's terminal is still a resource — only its frame is
/// not drawn).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DockLayout {
    pub center: DockNode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<DockRegion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<DockRegion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<DockRegion>,
}

impl Default for DockLayout {
    fn default() -> Self {
        Self {
            center: DockNode::empty_tabs(),
            left: None,
            right: None,
            bottom: None,
        }
    }
}

impl DockLayout {
    /// A one-group layout over `sheets`, the first displayed.
    pub fn for_sheets(sheets: impl IntoIterator<Item = String>) -> Self {
        Self {
            center: DockNode::tabs(sheets),
            ..Self::default()
        }
    }

    /// The dock on `side`, when the layout has one.
    pub fn region(&self, side: DockSide) -> Option<&DockRegion> {
        match side {
            DockSide::Left => self.left.as_ref(),
            DockSide::Right => self.right.as_ref(),
            DockSide::Bottom => self.bottom.as_ref(),
        }
    }

    fn region_mut(&mut self, side: DockSide) -> Option<&mut DockRegion> {
        match side {
            DockSide::Left => self.left.as_mut(),
            DockSide::Right => self.right.as_mut(),
            DockSide::Bottom => self.bottom.as_mut(),
        }
    }

    /// The dock on `side`, created at its defaults when absent.
    fn region_or_create(&mut self, side: DockSide) -> &mut DockRegion {
        match side {
            DockSide::Left => self.left.get_or_insert_with(DockRegion::default),
            DockSide::Right => self.right.get_or_insert_with(DockRegion::default),
            DockSide::Bottom => self.bottom.get_or_insert_with(DockRegion::default),
        }
    }

    /// The three docks in walk order, each with its region when present.
    pub fn docks(&self) -> [(DockSide, Option<&DockRegion>); 3] {
        [
            (DockSide::Left, self.left.as_ref()),
            (DockSide::Right, self.right.as_ref()),
            (DockSide::Bottom, self.bottom.as_ref()),
        ]
    }
}

/// Visits every tree a sheet may live in: the center, then each dock's own.
fn for_each_tree<'a>(layout: &'a DockLayout, mut visit: impl FnMut(&'a DockNode)) {
    visit(&layout.center);
    for (_, region) in layout.docks() {
        if let Some(region) = region {
            visit(&region.layout);
        }
    }
}

/// Mutable counterpart of [`for_each_tree`].
fn for_each_tree_mut(layout: &mut DockLayout, mut visit: impl FnMut(&mut DockNode)) {
    visit(&mut layout.center);
    for side in DockSide::ALL {
        if let Some(region) = layout.region_mut(side) {
            visit(&mut region.layout);
        }
    }
}

/// Every sheet id in the layout, center first, then the docks in side order.
pub fn panel_ids(layout: &DockLayout) -> Vec<&str> {
    let mut ids = Vec::new();
    for_each_tree(layout, |node| collect_panels(node, &mut ids));
    ids
}

/// Every tab group's sheet ids, in tree order (empty groups excluded).
pub fn group_panels(layout: &DockLayout) -> Vec<Vec<&str>> {
    let mut groups = Vec::new();
    for_each_tree(layout, |node| collect_groups(node, &mut groups));
    groups
}

fn collect_groups<'a>(node: &'a DockNode, out: &mut Vec<Vec<&'a str>>) {
    match node {
        DockNode::Tabs { panels, .. } => {
            if !panels.is_empty() {
                out.push(panels.iter().map(String::as_str).collect());
            }
        }
        DockNode::Split { children, .. } => {
            for child in children {
                collect_groups(child, out);
            }
        }
    }
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

/// How many sheets the layout holds (a closed dock's sheets count — they are
/// resources, not visible state).
pub fn sheet_count(layout: &DockLayout) -> usize {
    panel_ids(layout).len()
}

/// Whether `id` is in the layout at all.
pub fn has_sheet(layout: &DockLayout, id: &str) -> bool {
    panel_ids(layout).contains(&id)
}

/// The center's first group's displayed sheet — the `⌘⇧↵` zoom's target.
pub fn first_center_active(layout: &DockLayout) -> Option<String> {
    fn walk(node: &DockNode) -> Option<String> {
        match node {
            DockNode::Tabs {
                panels,
                active_index,
            } => panels.get(active_index.unwrap_or(0)).cloned(),
            DockNode::Split { children, .. } => children.iter().find_map(walk),
        }
    }
    walk(&layout.center)
}

/// Whether `id` lives in the center tree — the zoom's only jurisdiction.
pub fn has_center_sheet(layout: &DockLayout, id: &str) -> bool {
    let mut ids = Vec::new();
    collect_panels(&layout.center, &mut ids);
    ids.contains(&id)
}

/// The sheets a user can see now: each open group's displayed tab, or only
/// the zoomed one when a zoom is in effect. A closed dock shows nothing; a
/// zoom is a center fact and never names a dock sheet.
pub fn active_panel_ids<'a>(layout: &'a DockLayout, zoomed: Option<&'a str>) -> Vec<&'a str> {
    if let Some(zoom) = zoomed {
        return if has_center_sheet(layout, zoom) {
            vec![zoom]
        } else {
            Vec::new()
        };
    }
    let mut active = Vec::new();
    collect_active(&layout.center, &mut active);
    for (_, region) in layout.docks() {
        let Some(region) = region else { continue };
        if region.open {
            collect_active(&region.layout, &mut active);
        }
    }
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

/// Every sheet id a dock region holds, in tree order (`None` for an absent
/// region).
pub fn panels_in_region(layout: &DockLayout, side: DockSide) -> Vec<&str> {
    let mut ids = Vec::new();
    if let Some(region) = layout.region(side) {
        collect_panels(&region.layout, &mut ids);
    }
    ids
}

/// Displays an open sheet: its group's `activeIndex` moves to it.
pub fn activate_sheet(layout: &DockLayout, id: &str) -> DockLayout {
    let mut next = layout.clone();
    let mut changed = false;
    for_each_tree_mut(&mut next, |node| changed |= activate_in(node, id));
    if changed { next } else { layout.clone() }
}

fn activate_in(root: &mut DockNode, id: &str) -> bool {
    let mut changed = false;
    if let DockNode::Tabs {
        panels,
        active_index,
    } = root
        && let Some(at) = panels.iter().position(|panel| panel == id)
        && *active_index != Some(at)
    {
        *active_index = Some(at);
        changed = true;
    } else if let DockNode::Split { children, .. } = root {
        for child in children {
            changed |= activate_in(child, id);
        }
    }
    changed
}

/// A group the `pred` matched, anywhere on the canvas: which tree it lives
/// in (`None` = center), its path from that tree's root, and its tabs.
#[derive(Clone, Debug)]
struct GroupHit {
    region: Option<DockSide>,
    path: Vec<usize>,
    panels: Vec<String>,
}

/// The first group whose tabs match `pred`: the center's groups first, then
/// the docks in side order (open or not — a placement may target a closed
/// dock's surviving group).
fn find_group<F>(layout: &DockLayout, pred: &mut F) -> Option<GroupHit>
where
    F: FnMut(&[String]) -> bool,
{
    fn walk<F>(
        node: &DockNode,
        pred: &mut F,
        path: &mut Vec<usize>,
    ) -> Option<(Vec<usize>, Vec<String>)>
    where
        F: FnMut(&[String]) -> bool,
    {
        match node {
            DockNode::Tabs { panels, .. } => pred(panels).then(|| (path.clone(), panels.clone())),
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

    let mut path = Vec::new();
    if let Some((found, panels)) = walk(&layout.center, pred, &mut path) {
        return Some(GroupHit {
            region: None,
            path: found,
            panels,
        });
    }
    for (side, region) in layout.docks() {
        let Some(region) = region else { continue };
        let mut path = Vec::new();
        if let Some((found, panels)) = walk(&region.layout, pred, &mut path) {
            return Some(GroupHit {
                region: Some(side),
                path: found,
                panels,
            });
        }
    }
    None
}

/// Applies `edit` to the node at `path` inside `region`'s tree (`None` =
/// center). A path that no longer resolves (the tree changed under it) is a
/// silent no-op.
fn with_group(
    layout: &mut DockLayout,
    region: Option<DockSide>,
    path: &[usize],
    edit: &mut impl FnMut(&mut DockNode),
) {
    let root = match region {
        None => &mut layout.center,
        Some(side) => match layout.region_mut(side) {
            Some(region) => &mut region.layout,
            None => return,
        },
    };
    with_node(root, path, edit);
}

/// Appends a sheet to the group of `prefer` (the shown sheet), else the first
/// group on the canvas, and displays it.
///
/// A sheet is one instance: appending an id the layout already holds only
/// activates it. This is the "open" primitive every kind goes through, so a
/// restored layout can never grow a duplicate tab.
pub fn append_sheet(layout: &DockLayout, id: &str, prefer: Option<&str>) -> DockLayout {
    if has_sheet(layout, id) {
        return activate_sheet(layout, id);
    }
    let preferred = prefer
        .and_then(|prefer| find_group(layout, &mut |panels| panels.iter().any(|p| p == prefer)));
    let target = preferred.or_else(|| find_group(layout, &mut |_| true));
    let mut next = layout.clone();
    match target {
        Some(hit) => {
            with_group(&mut next, hit.region, &hit.path, &mut |node| {
                if let DockNode::Tabs {
                    panels,
                    active_index,
                } = node
                {
                    panels.push(id.to_string());
                    *active_index = Some(panels.len() - 1);
                }
            });
            next
        }
        None => {
            next.center = DockNode::tabs([id.to_string()]);
            next
        }
    }
}

/// Removes a sheet: its tab goes, an emptied group goes with it, and an
/// emptied tree stays as an empty tab group (a dock keeps its region — an
/// empty dock still renders its handle and may take the next drop).
pub fn remove_sheet(layout: &DockLayout, id: &str) -> DockLayout {
    let mut next = layout.clone();
    next.center = remove_from(&layout.center, id).unwrap_or_else(DockNode::empty_tabs);
    for side in DockSide::ALL {
        if let Some(region) = next.region_mut(side) {
            region.layout = remove_from(&region.layout, id).unwrap_or_else(DockNode::empty_tabs);
        }
    }
    next
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
    let mut next = base.clone();
    let hit = find_group(&base, &mut |panels| {
        panels.iter().any(|panel| panel == from)
    });
    match hit {
        Some(hit) => {
            with_group(&mut next, hit.region, &hit.path, &mut |node| {
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
            next
        }
        None => append_sheet(&base, to, None),
    }
}

/// Puts `id` in a new group split to the right of the group showing
/// `beside`. The neighbor keeps its tabs; the new group holds only `id`.
///
/// A thin wrapper over [`move_sheet`] — every placement is one, so a split
/// obeys the same one-instance rule a drag does. A `beside` nobody shows
/// falls back to appending.
pub fn split_right(layout: &DockLayout, beside: &str, id: &str) -> DockLayout {
    if beside == id {
        return activate_sheet(layout, id);
    }
    move_sheet(
        layout,
        id,
        DropTarget::Split {
            anchor: beside.to_string(),
            side: SplitEdge::Right,
        },
    )
}

/// The one placement primitive: every drop — a tab slot, a group, a split at
/// a group edge, a dock region — resolves through it.
///
/// The target resolves against the **old** layout, then the sheet is removed,
/// then inserted (the two-phase move): the dragged sheet's own tab may be the
/// anchor, and after the removal the anchor would be unfindable. The target
/// group is re-found by its first surviving tab — sheet ids are single-
/// instance, so the panel list identifies the group as surely as a path
/// would, and survives the collapses a removal may cause along the way.
///
/// An `id` the layout does not hold is simply placed (that is how "open in a
/// new group beside this tab" works without a prior append).
pub fn move_sheet(layout: &DockLayout, id: &str, target: DropTarget) -> DockLayout {
    match target {
        DropTarget::Dock { side } => {
            let mut next = remove_sheet(layout, id);
            let region = next.region_or_create(side);
            region.open = true;
            // The region's tree is a bare node; the append primitive speaks
            // `DockLayout`, so the tree rides in as a center and back.
            let wrapped = DockLayout {
                center: region.layout.clone(),
                ..DockLayout::default()
            };
            region.layout = append_sheet(&wrapped, id, None).center;
            next
        }
        DropTarget::TabSlot { ref anchor, .. }
        | DropTarget::Group { ref anchor }
        | DropTarget::Split { ref anchor, .. } => {
            let Some(hit) = find_group(layout, &mut |panels| panels.iter().any(|p| p == anchor))
            else {
                // A target nobody shows: the plain append rule decides.
                return append_sheet(layout, id, None);
            };
            let same_group = hit.panels.iter().any(|panel| panel == id);
            let from = hit.panels.iter().position(|panel| panel == id);
            let next = remove_sheet(layout, id);
            // The target group's first surviving tab is its new anchor. A
            // group that held only the dragged sheet has none: the drop had
            // nowhere else to go, and the layout stands.
            let Some(re_anchor) = hit.panels.iter().find(|panel| panel.as_str() != id) else {
                // The target group was exactly the dragged sheet: the drop
                // lands where the sheet already is.
                return layout.clone();
            };
            let Some(spot) = find_group(&next, &mut |panels| panels.iter().any(|p| p == re_anchor))
            else {
                return layout.clone();
            };
            let mut next = next;
            match &target {
                DropTarget::TabSlot { index, .. } => {
                    let index = *index;
                    with_group(&mut next, spot.region, &spot.path, &mut |node| {
                        if let DockNode::Tabs {
                            panels,
                            active_index,
                        } = node
                        {
                            let mut at = index.min(panels.len());
                            // Within the same group the removal shifts every
                            // later slot down one: a boundary that was past
                            // the dragged tab moves up with it.
                            if same_group
                                && let Some(from) = from
                                && from < at
                            {
                                at -= 1;
                            }
                            panels.insert(at, id.to_string());
                            *active_index = Some(at);
                        }
                    });
                }
                DropTarget::Group { .. } => {
                    with_group(&mut next, spot.region, &spot.path, &mut |node| {
                        if let DockNode::Tabs {
                            panels,
                            active_index,
                        } = node
                        {
                            panels.push(id.to_string());
                            *active_index = Some(panels.len() - 1);
                        }
                    });
                }
                DropTarget::Split { side, .. } => {
                    with_group(&mut next, spot.region, &spot.path, &mut |node| {
                        let group = std::mem::replace(node, DockNode::empty_tabs());
                        let newcomer = DockNode::tabs([id.to_string()]);
                        *node = if side.leads() {
                            DockNode::split(side.axis(), vec![newcomer, group])
                        } else {
                            DockNode::split(side.axis(), vec![group, newcomer])
                        };
                    });
                }
                DropTarget::Dock { .. } => unreachable!("handled above"),
            }
            next
        }
    }
}

/// Drops sheets outside `keep` (their resource is gone), de-duplicates
/// repeated ids across the whole tree, and collapses groups that end up empty.
///
/// Restore reads a stored layout through this before activating it: a layout
/// is a claim about the past, and only surviving resources may be shown. The
/// seen-set is shared across groups — a stored file may repeat an id in two
/// tab groups, and two copies of one sheet would break "one sheet, one
/// instance" the moment both are mounted.
pub fn prune_layout(layout: &DockLayout, keep: &HashSet<String>) -> DockLayout {
    let mut next = layout.clone();
    // The dedup set is shared across the WHOLE canvas — center and every
    // dock region: a stored file may repeat an id in two tab groups, and
    // two copies of one sheet would break "one sheet, one instance" the
    // moment both are mounted.
    let mut seen = HashSet::new();
    next.center = prune_node(&layout.center, keep, &mut seen).unwrap_or_else(DockNode::empty_tabs);
    for side in DockSide::ALL {
        if let Some(region) = next.region_mut(side) {
            region.layout = prune_node(&region.layout, keep, &mut seen)
                .unwrap_or_else(DockNode::empty_tabs);
        }
    }
    next
}

fn prune_node<'a>(
    node: &'a DockNode,
    keep: &HashSet<String>,
    seen: &mut HashSet<&'a str>,
) -> Option<DockNode> {
    match node {
        DockNode::Tabs {
            panels,
            active_index,
        } => {
            let panels: Vec<String> = panels
                .iter()
                .filter(|panel| keep.contains(panel.as_str()) && seen.insert(panel.as_str()))
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
                .filter_map(|child| prune_node(child, keep, seen))
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

/// Flips a dock's open flag (creating the region at its defaults when the
/// layout never had one). Reopening restores the size it was left with.
pub fn toggle_dock(layout: &DockLayout, side: DockSide) -> DockLayout {
    let mut next = layout.clone();
    if next.region(side).is_some() {
        let region = next.region_or_create(side);
        region.open = !region.open;
    } else {
        // A dock the layout never had: the first toggle is the open (the
        // region is born open at its defaults), not an instant close.
        next.region_or_create(side);
    }
    next
}

/// Sets a dock's open flag (creating the region at its defaults when absent).
pub fn set_dock_open(layout: &DockLayout, side: DockSide, open: bool) -> DockLayout {
    let mut next = layout.clone();
    next.region_or_create(side).open = open;
    next
}

/// Records a dock's cross-edge size, clamped to the layout's own floor (the
/// ceiling is the window's — the projection clamps against it).
pub fn set_dock_size(layout: &DockLayout, side: DockSide, size: f32) -> DockLayout {
    let mut next = layout.clone();
    let region = next.region_or_create(side);
    region.size = Some(size.max(DOCK_MIN_SIZE));
    next
}

/// Records a split's slot sizes; `None` resets the split to equal shares
/// (the double-click). `region` names the tree the path walks (`None` =
/// center; a dock is single-group, so its splits only ever come from a
/// hand-seeded layout).
pub fn set_split_sizes(
    layout: &DockLayout,
    region: Option<DockSide>,
    path: &[usize],
    sizes: Option<Vec<f32>>,
) -> DockLayout {
    let mut next = layout.clone();
    let root = match region {
        None => &mut next.center,
        Some(side) => match next.region_mut(side) {
            Some(region) => &mut region.layout,
            None => return next,
        },
    };
    with_node(root, path, &mut |node| {
        if let DockNode::Split { sizes: slot, .. } = node {
            *slot = sizes.clone();
        }
    });
    next
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

    fn sheets(ids: &[&str]) -> DockLayout {
        DockLayout::for_sheets(ids.iter().map(|id| (*id).to_string()))
    }

    fn docked(layout: &DockLayout, side: DockSide, panels: &[&str], open: bool) -> DockLayout {
        let mut next = layout.clone();
        let region = next.region_or_create(side);
        region.layout = DockNode::tabs(panels.iter().map(|id| id.to_string()));
        region.open = open;
        next
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
    fn append_prefers_a_dock_group_when_the_prefer_lives_there() {
        let layout = docked(&DockLayout::default(), DockSide::Right, &["r1"], true);
        let layout = append_sheet(&layout, "r2", Some("r1"));
        assert_eq!(panels_in_region(&layout, DockSide::Right), ["r1", "r2"]);
        assert_eq!(ids(&layout), ["r1", "r2"]);
    }

    #[test]
    fn append_falls_back_to_the_empty_center_group_first() {
        // The default layout's center holds an empty group: the first group
        // in walk order is the center's own, even with a dock open.
        let layout = docked(&DockLayout::default(), DockSide::Right, &["r1"], true);
        let layout = append_sheet(&layout, "r2", None);
        assert_eq!(ids(&layout), ["r2", "r1"]);
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
    fn prune_deduplicates_an_id_repeated_across_groups() {
        // A hand-edited or foreign file may carry one sheet in two tab
        // groups; one sheet is one instance, wherever the repeat sits.
        let split = DockNode::split(
            Axis::Horizontal,
            vec![
                DockNode::tabs(["a".to_string(), "b".to_string()]),
                DockNode::tabs(["a".to_string(), "c".to_string()]),
            ],
        );
        let layout = DockLayout { center: split };
        let pruned = prune_layout(&layout, &keep(&["a", "b", "c"]));
        assert_eq!(ids(&pruned), ["a", "b", "c"]);
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

    #[test]
    fn a_dock_region_round_trips_through_json() {
        let layout = docked(
            &DockLayout::for_sheets(["a".to_string()]),
            DockSide::Bottom,
            &["t1"],
            false,
        );
        let mut bottom = layout.bottom.clone().unwrap();
        bottom.size = Some(240.);
        let with_size = DockLayout {
            center: layout.center.clone(),
            left: None,
            right: None,
            bottom: Some(bottom),
        };
        let json = serde_json::to_string(&with_size).unwrap();
        let back: DockLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(back, with_size);
        let value = serde_json::to_value(&with_size).unwrap();
        assert_eq!(value["bottom"]["size"], 240.0);
        assert_eq!(value["bottom"]["open"], false);
        assert_eq!(value["bottom"]["layout"]["panels"][0], "t1");
        assert!(value.get("left").is_none() && value.get("right").is_none());
    }

    #[test]
    fn every_walk_covers_the_docks() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = docked(&layout, DockSide::Right, &["r1"], true);
        let layout = docked(&layout, DockSide::Bottom, &["t1", "t2"], true);
        assert_eq!(ids(&layout), ["a", "r1", "t1", "t2"]);
        assert_eq!(sheet_count(&layout), 4);
        assert!(has_sheet(&layout, "t2"));
        assert_eq!(
            group_panels(&layout),
            [vec!["a"], vec!["r1"], vec!["t1", "t2"]]
        );
    }

    #[test]
    fn a_closed_docks_sheets_stay_resources() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = docked(&layout, DockSide::Bottom, &["t1"], false);
        // The layout is the fact: a closed dock's terminal is still held,
        // even though nothing shows it.
        assert_eq!(ids(&layout), ["a", "t1"]);
        assert_eq!(sheet_count(&layout), 2);
        assert_eq!(active_panel_ids(&layout, None), ["a"]);
    }

    #[test]
    fn a_zoom_never_names_a_dock_sheet() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = docked(&layout, DockSide::Bottom, &["t1"], true);
        assert!(active_panel_ids(&layout, Some("t1")).is_empty());
        assert_eq!(active_panel_ids(&layout, Some("a")), ["a"]);
    }

    #[test]
    fn move_sheet_reorders_within_a_group_in_both_directions() {
        let layout = DockLayout::for_sheets(["a".to_string(), "b".to_string(), "c".to_string()]);
        // Drag `c` onto the boundary between `a` and `b` (index 1).
        let moved = move_sheet(
            &layout,
            "c",
            DropTarget::TabSlot {
                anchor: "a".into(),
                index: 1,
            },
        );
        assert_eq!(ids(&moved), ["a", "c", "b"]);
        assert_eq!(active_panel_ids(&moved, None), ["c"]);

        // Drag `a` after `b` (boundary index 2): the removal shifts the
        // boundary back one, landing `a` between `b` and `c`.
        let back = move_sheet(
            &moved,
            "a",
            DropTarget::TabSlot {
                anchor: "c".into(),
                index: 2,
            },
        );
        assert_eq!(ids(&back), ["c", "a", "b"]);
    }

    #[test]
    fn move_sheet_into_another_groups_slot_moves_it() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let moved = move_sheet(
            &layout,
            "b",
            DropTarget::TabSlot {
                anchor: "a".into(),
                index: 0,
            },
        );
        // The source group collapses; one group remains, b first.
        assert_eq!(ids(&moved), ["b", "a"]);
        assert!(matches!(moved.center, DockNode::Tabs { .. }));
        assert_eq!(active_panel_ids(&moved, None), ["b"]);
    }

    #[test]
    fn move_sheet_onto_a_group_appends_and_shows() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let moved = move_sheet(&layout, "b", DropTarget::Group { anchor: "a".into() });
        assert_eq!(ids(&moved), ["a", "b"]);
        // One group remains, and it shows the sheet that moved onto it.
        assert_eq!(active_panel_ids(&moved, None), ["b"]);
        // The source group collapses with its sheet gone; one group remains.
        assert!(matches!(&moved.center, DockNode::Tabs { panels, .. } if panels == &["a", "b"]));
    }

    #[test]
    fn move_sheet_splits_off_each_edge() {
        let layout = DockLayout::for_sheets(["a".to_string(), "b".to_string()]);
        let right = move_sheet(
            &layout,
            "b",
            DropTarget::Split {
                anchor: "a".into(),
                side: SplitEdge::Right,
            },
        );
        let DockNode::Split { axis, children, .. } = &right.center else {
            panic!("a right split is horizontal");
        };
        assert_eq!(*axis, Axis::Horizontal);
        assert!(matches!(&children[0], DockNode::Tabs { panels, .. } if panels == &["a"]));
        assert!(matches!(&children[1], DockNode::Tabs { panels, .. } if panels == &["b"]));

        let left = move_sheet(
            &layout,
            "b",
            DropTarget::Split {
                anchor: "a".into(),
                side: SplitEdge::Left,
            },
        );
        let DockNode::Split { children, .. } = &left.center else {
            panic!("a left split is horizontal");
        };
        assert!(matches!(&children[0], DockNode::Tabs { panels, .. } if panels == &["b"]));
        assert!(matches!(&children[1], DockNode::Tabs { panels, .. } if panels == &["a"]));

        let bottom = move_sheet(
            &layout,
            "b",
            DropTarget::Split {
                anchor: "a".into(),
                side: SplitEdge::Bottom,
            },
        );
        let DockNode::Split { axis, .. } = &bottom.center else {
            panic!("a bottom split is vertical");
        };
        assert_eq!(*axis, Axis::Vertical);
    }

    #[test]
    fn move_sheet_into_a_dock_opens_and_fills_it() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let moved = move_sheet(
            &layout,
            "a",
            DropTarget::Dock {
                side: DockSide::Bottom,
            },
        );
        assert_eq!(panels_in_region(&moved, DockSide::Bottom), ["a"]);
        let region = moved.region(DockSide::Bottom).unwrap();
        assert!(region.open);
        assert_eq!(ids(&moved), ["a"]);
        assert_eq!(active_panel_ids(&moved, None), ["a"]);
    }

    #[test]
    fn moving_the_last_sheet_out_of_a_dock_keeps_the_region() {
        let layout = move_sheet(
            &sheets(&["c"]),
            "t1",
            DropTarget::Dock {
                side: DockSide::Bottom,
            },
        );
        let moved = move_sheet(&layout, "t1", DropTarget::Group { anchor: "c".into() });
        // The dock keeps its (now empty) region; the sheet lives in center.
        assert!(moved.region(DockSide::Bottom).is_some());
        assert_eq!(
            panels_in_region(&moved, DockSide::Bottom),
            Vec::<&str>::new()
        );
        assert_eq!(ids(&moved), ["c", "t1"]);
    }

    #[test]
    fn dragging_a_groups_only_tab_onto_itself_is_a_no_op() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let same = move_sheet(
            &layout,
            "b",
            DropTarget::TabSlot {
                anchor: "b".into(),
                index: 1,
            },
        );
        assert_eq!(same, layout);
    }

    #[test]
    fn toggle_dock_creates_at_defaults_and_keeps_the_size() {
        let layout = DockLayout::default();
        let opened = toggle_dock(&layout, DockSide::Left);
        let region = opened.region(DockSide::Left).unwrap();
        assert!(region.open);
        assert_eq!(region.size, None);

        let sized = set_dock_size(&opened, DockSide::Left, 320.);
        assert_eq!(sized.region(DockSide::Left).unwrap().size, Some(320.));
        let closed = set_dock_open(&sized, DockSide::Left, false);
        assert!(!closed.region(DockSide::Left).unwrap().open);
        let reopened = set_dock_open(&closed, DockSide::Left, true);
        assert_eq!(reopened.region(DockSide::Left).unwrap().size, Some(320.));
    }

    #[test]
    fn set_dock_size_clamps_to_the_floor() {
        let layout = toggle_dock(&DockLayout::default(), DockSide::Bottom);
        let clamped = set_dock_size(&layout, DockSide::Bottom, 40.);
        assert_eq!(
            clamped.region(DockSide::Bottom).unwrap().size,
            Some(DOCK_MIN_SIZE)
        );
    }

    #[test]
    fn set_split_sizes_none_resets_to_equal_shares() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = split_right(&layout, "a", "b");
        let sized = set_split_sizes(&layout, None, &[], Some(vec![120.]));
        let DockNode::Split { sizes, .. } = &sized.center else {
            panic!("the split is still there");
        };
        assert_eq!(sizes, &Some(vec![120.]));
        let equal = set_split_sizes(&sized, None, &[], None);
        let DockNode::Split { sizes, .. } = &equal.center else {
            panic!("the split is still there");
        };
        assert_eq!(sizes, &None);
    }

    #[test]
    fn activate_sheet_reaches_into_a_dock() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = docked(&layout, DockSide::Bottom, &["t1", "t2"], true);
        assert_eq!(active_panel_ids(&layout, None), ["a", "t1"]);
        let activated = activate_sheet(&layout, "t2");
        assert_eq!(active_panel_ids(&activated, None), ["a", "t2"]);
    }

    #[test]
    fn prune_reaches_into_a_dock() {
        let layout = DockLayout::for_sheets(["a".to_string()]);
        let layout = docked(&layout, DockSide::Right, &["r1"], true);
        let pruned = prune_layout(&layout, &keep(&["a"]));
        assert_eq!(ids(&pruned), ["a"]);
        assert!(pruned.region(DockSide::Right).is_some());
    }
}
