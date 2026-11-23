//! The canvas as a projection of its layout: where each dock region, group
//! bar and sheet body sits, and where a drag would land.
//!
//! Pure data — no views, no measuring, no gpui. The renderer draws its chrome
//! from these boxes and its content from the frames; a layout edit changes
//! where a sheet is *placed*, never whether it exists.
//!
//! The rules follow the nsh-pi experiment's `projection.ts`, which the sheet
//! host consumed verbatim:
//!
//! - the left and right docks take full height and their own width; the
//!   bottom dock takes its height **between** them (an IDE's bottom panel,
//!   not a footer under the side panels);
//! - a split's slots are its recorded sizes (the last one takes whatever is
//!   left; unsized slots share what the sized ones leave);
//! - hidden split handles take `SPLIT_HANDLE` points between slots;
//! - a zoomed center group — or, below `COMPACT_WIDTH`, the only group on
//!   screen — fills the **center area**; a dock group never zooms;
//! - a parked canvas's sheets keep the place they had, so restoring a canvas
//!   finds every sheet where it was (no travel to place).

use crate::layout::{
    self, Axis, DockLayout, DockNode, DockRegion, DockSide, DropTarget, SplitEdge,
};
use crate::state::HeldLayouts;

/// The width of a hidden split handle between two slots.
pub const SPLIT_HANDLE: f32 = 1.0;

/// Below this width the canvas shows one group at a time (nsh's compact
/// threshold).
pub const COMPACT_WIDTH: f32 = 600.0;

/// The tab strip's height (the sheet frames start below it). Lives here so
/// the drop hit test and the renderer share one number.
pub const TAB_STRIP_H: f32 = 34.0;

/// The drag band a dock edge carries (the renderer paints it invisibly; the
/// hit strip is what matters).
pub const DOCK_HANDLE: f32 = 6.0;

/// A group's outer edge band that reads as "split here": a quarter of the
/// group, capped.
const EDGE_BAND_MAX: f32 = 60.0;
const EDGE_BAND_RATIO: f32 = 0.25;

/// A point in the canvas's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A rectangle in the canvas's points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    /// A rect at the origin.
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            x: 0.,
            y: 0.,
            width,
            height,
        }
    }

    /// Whether the point falls inside (edges inclusive).
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.x <= self.x + self.width
            && point.y >= self.y
            && point.y <= self.y + self.height
    }

    /// The same rect with its top moved down by `top`.
    pub fn inset_top(self, top: f32) -> Self {
        Self {
            y: self.y + top,
            height: (self.height - top).max(0.),
            ..self
        }
    }

    /// Whether the rect has any area to draw into.
    pub fn is_visible(&self) -> bool {
        self.width > 0. && self.height > 0.
    }
}

/// The canvas's regions at one size: the center's area, each dock's frame,
/// and every drag band.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CanvasFrame {
    /// The area the center tree lays out in (docks already subtracted).
    pub center: Rect,
    /// One frame per dock the layout has. An open one draws content; a
    /// closed one is an empty edge marker (its drag band lives in
    /// `dock_handles`).
    pub docks: Vec<DockFrame>,
    /// The center tree's split handles (a dock is single-group, so it never
    /// grows one).
    pub handles: Vec<SplitHandle>,
    /// A drag band on each present region's inner edge — open or closed (a
    /// closed dock's band is how a drag pulls it back open).
    pub dock_handles: Vec<DockHandle>,
    /// Below `COMPACT_WIDTH`: only the center's active group shows and the
    /// docks do not render at all.
    pub compact: bool,
}

/// One dock's frame: its side, its rect and whether it shows content. A
/// closed dock's rect is a zero-area marker at its edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockFrame {
    pub side: DockSide,
    pub rect: Rect,
    pub open: bool,
}

/// A split's drag band inside the center tree, with the split's own axis,
/// the path from the center root (what `set_split_sizes` edits), and the
/// split's full span (the drag math reads the slot sizes against it).
#[derive(Clone, Debug, PartialEq)]
pub struct SplitHandle {
    pub path: Vec<usize>,
    pub axis: Axis,
    pub rect: Rect,
    pub span: Rect,
}

/// A dock edge's drag band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockHandle {
    pub side: DockSide,
    pub rect: Rect,
}

/// A visible tab group's bar: where the chrome draws, and whose tabs it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct TabBarBox {
    pub rect: Rect,
    /// A panel of the group — the start page opens beside it (any member
    /// works).
    pub anchor: String,
    /// The group's first panel: the box keeps its identity across switches.
    pub key: String,
    /// The group's tabs, in order.
    pub panels: Vec<String>,
    /// Which of `panels` the group displays.
    pub active: usize,
    /// The dock the group lives in; `None` is the center (only center groups
    /// zoom).
    pub region: Option<DockSide>,
}

/// A group's content area: the box under its bar, where its sheets show.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupFrame {
    pub key: String,
    pub panels: Vec<String>,
    pub active: usize,
    pub rect: Rect,
}

/// One sheet's place: a clipped frame at its group's content area, and the
/// offset of its body inside it (`0` at rest, one frame width per place away).
#[derive(Clone, Debug, PartialEq)]
pub struct SheetFrame {
    pub id: String,
    /// The group it belongs to (the frame's identity), or its own id when it
    /// sits on a canvas that is not on screen.
    pub group: String,
    /// Its place among the group's sheets.
    pub index: usize,
    /// How far it is from the displayed one, in places.
    pub place: i32,
    pub rect: Rect,
    /// Where its body sits inside the frame (the tab that moved).
    pub offset: f32,
    /// It is its group's displayed sheet on the canvas on screen.
    pub shown: bool,
    /// It is at most one place from the displayed sheet: the sheets a turn of
    /// the tab strip draws while it slides.
    pub near: bool,
}

/// A dock's cross-edge extent, clamped to its own floor and the space the
/// center keeps.
fn region_extent(region: &DockRegion, budget: f32) -> f32 {
    let wanted = region.size.unwrap_or(layout::DOCK_DEFAULT_SIZE);
    wanted
        .clamp(layout::DOCK_MIN_SIZE, layout::DOCK_MIN_SIZE.max(budget))
        .min(budget.max(0.))
}

/// The canvas's regions at `(width, height)`: where the center lays out,
/// where each dock sits, and every drag band. Docks the layout lacks are
/// absent; docks it holds but keeps closed leave only their handle.
pub fn canvas_frame(
    layout: &DockLayout,
    width: f32,
    height: f32,
    zoomed: Option<&str>,
) -> CanvasFrame {
    let full = Rect::new(width.max(0.), height.max(0.));
    let compact = width > 0. && width < COMPACT_WIDTH;
    if width <= 0. || height <= 0. {
        return CanvasFrame::default();
    }
    if compact {
        // Narrow window: the center's active group is the whole canvas; the
        // docks render nothing (their layout is untouched — widening the
        // window brings them back).
        return CanvasFrame {
            center: full,
            docks: Vec::new(),
            handles: Vec::new(),
            dock_handles: Vec::new(),
            compact: true,
        };
    }

    let left = layout.region(DockSide::Left);
    let right = layout.region(DockSide::Right);
    let bottom = layout.region(DockSide::Bottom);
    let left_open = left.is_some_and(|r| r.open);
    let right_open = right.is_some_and(|r| r.open);
    let bottom_open = bottom.is_some_and(|r| r.open);

    // Side docks take width (full height); the bottom dock takes height
    // between them. The center keeps its guaranteed share; sequential
    // budgets keep the clamps from ever over-allocating.
    let width_budget = (width - layout::CENTER_MIN).max(0.);
    let left_w = if left_open {
        left.map(|r| region_extent(r, width_budget)).unwrap_or(0.)
    } else {
        0.
    };
    let right_w = if right_open {
        right
            .map(|r| region_extent(r, (width_budget - left_w).max(0.)))
            .unwrap_or(0.)
    } else {
        0.
    };
    let bottom_h = if bottom_open {
        bottom
            .map(|r| region_extent(r, (height - layout::CENTER_MIN).max(0.)))
            .unwrap_or(0.)
    } else {
        0.
    };

    let side_handles = (left_open as i32 + right_open as i32) as f32 * SPLIT_HANDLE;
    let middle = Rect {
        x: left_w + (left_open as i32 as f32) * SPLIT_HANDLE,
        y: 0.,
        width: (width - left_w - right_w - side_handles).max(0.),
        height,
    };
    let center = Rect {
        x: middle.x,
        y: 0.,
        width: middle.width,
        height: (height - bottom_h - (bottom_open as i32 as f32) * SPLIT_HANDLE).max(0.),
    };
    let bottom_rect = Rect {
        x: middle.x,
        y: height - bottom_h,
        width: middle.width,
        height: bottom_h,
    };

    // A closed dock leaves a zero-area frame at its edge — the renderer draws
    // nothing there, but the frame keeps telling the host which regions exist.
    let mut docks = Vec::new();
    if left.is_some() {
        docks.push(DockFrame {
            side: DockSide::Left,
            rect: if left_open {
                Rect {
                    x: 0.,
                    y: 0.,
                    width: left_w,
                    height,
                }
            } else {
                Rect {
                    x: 0.,
                    y: 0.,
                    width: 0.,
                    height,
                }
            },
            open: left_open,
        });
    }
    if right.is_some() {
        docks.push(DockFrame {
            side: DockSide::Right,
            rect: if right_open {
                Rect {
                    x: width - right_w,
                    y: 0.,
                    width: right_w,
                    height,
                }
            } else {
                Rect {
                    x: width,
                    y: 0.,
                    width: 0.,
                    height,
                }
            },
            open: right_open,
        });
    }
    if bottom.is_some() {
        docks.push(DockFrame {
            side: DockSide::Bottom,
            rect: if bottom_open {
                bottom_rect
            } else {
                Rect {
                    x: middle.x,
                    y: height,
                    width: middle.width,
                    height: 0.,
                }
            },
            open: bottom_open,
        });
    }

    // Each present region's inner edge carries a drag band: open ones ride
    // the seam, closed ones hug the canvas edge (the way back open).
    let mut dock_handles = Vec::new();
    for (side, region) in layout.docks() {
        let Some(region) = region else { continue };
        let open = region.open;
        let rect = match side {
            DockSide::Left => Rect {
                x: if open {
                    left_w + SPLIT_HANDLE / 2. - DOCK_HANDLE / 2.
                } else {
                    0.
                },
                y: 0.,
                width: DOCK_HANDLE,
                height,
            },
            DockSide::Right => Rect {
                x: if open {
                    width - right_w - SPLIT_HANDLE / 2. - DOCK_HANDLE / 2.
                } else {
                    width - DOCK_HANDLE
                },
                y: 0.,
                width: DOCK_HANDLE,
                height,
            },
            DockSide::Bottom => Rect {
                x: middle.x,
                y: if open {
                    bottom_rect.y + SPLIT_HANDLE / 2. - DOCK_HANDLE / 2.
                } else {
                    height - DOCK_HANDLE
                },
                width: middle.width,
                height: DOCK_HANDLE,
            },
        };
        dock_handles.push(DockHandle { side, rect });
    }

    let mut handles = Vec::new();
    let mut path = Vec::new();
    walk_handles(&layout.center, &center, &mut path, &mut handles);

    // A zoom is a center fact; the frame reports it by leaving the docks
    // alone (the zoomed group fills `center` — `tab_bar_boxes` does that).
    let _ = zoomed;
    CanvasFrame {
        center,
        docks,
        handles,
        dock_handles,
        compact: false,
    }
}

/// The drag bands between a center split's visible slots, recursively.
fn walk_handles(node: &DockNode, rect: &Rect, path: &mut Vec<usize>, out: &mut Vec<SplitHandle>) {
    let DockNode::Split {
        axis,
        children,
        sizes,
    } = node
    else {
        return;
    };
    let slots: Vec<(usize, &DockNode)> = children
        .iter()
        .enumerate()
        .filter(|(_, child)| has_panels(child))
        .collect();
    if slots.len() < 2 {
        return;
    }
    let horizontal = *axis == Axis::Horizontal;
    let total = if horizontal { rect.width } else { rect.height };
    let space = (total - SPLIT_HANDLE * (slots.len() as f32 - 1.)).max(0.);
    // Mirror `walk`'s slot math exactly, so every handle sits on the seam the
    // boxes actually show.
    let (sized, loose) =
        slots[..slots.len() - 1]
            .iter()
            .fold((0., 0usize), |(sum, loose), (child_index, _)| {
                let size = sizes
                    .as_ref()
                    .and_then(|sizes| sizes.get(*child_index).copied());
                match size {
                    Some(size) => (sum + size, loose),
                    None => (sum, loose + 1),
                }
            });
    let share = if loose > 0 {
        ((space - sized) / (loose as f32 + 1.)).max(0.)
    } else {
        0.
    };
    let mut at = if horizontal { rect.x } else { rect.y };
    let mut used = 0.;
    for (index, (child_index, child)) in slots.iter().enumerate() {
        let last = index == slots.len() - 1;
        let size = if last {
            (space - used).max(0.)
        } else {
            sizes
                .as_ref()
                .and_then(|sizes| sizes.get(*child_index).copied())
                .unwrap_or(share)
        };
        used += size;
        let child_rect = if horizontal {
            Rect {
                x: at,
                y: rect.y,
                width: size,
                height: rect.height,
            }
        } else {
            Rect {
                x: rect.x,
                y: at,
                width: rect.width,
                height: size,
            }
        };
        // The band between this slot and the next (none after the last).
        if !last {
            let gap_center = at + size + SPLIT_HANDLE / 2.;
            let band = if horizontal {
                Rect {
                    x: gap_center - DOCK_HANDLE / 2.,
                    y: rect.y,
                    width: DOCK_HANDLE,
                    height: rect.height,
                }
            } else {
                Rect {
                    x: rect.x,
                    y: gap_center - DOCK_HANDLE / 2.,
                    width: rect.width,
                    height: DOCK_HANDLE,
                }
            };
            // The handle names the SPLIT itself (what `set_split_sizes`
            // edits) — the path this node sits at, not a child's.
            out.push(SplitHandle {
                path: path.clone(),
                axis: *axis,
                rect: band,
                span: *rect,
            });
        }
        path.push(*child_index);
        walk_handles(child, &child_rect, path, out);
        path.pop();
        at += size + SPLIT_HANDLE;
    }
}

fn has_panels(node: &DockNode) -> bool {
    match node {
        DockNode::Tabs { panels, .. } => !panels.is_empty(),
        DockNode::Split { children, .. } => children.iter().any(has_panels),
    }
}

/// A tree's visible groups and the rects their bars occupy, recursively.
fn walk(node: &DockNode, rect: Rect, region: Option<DockSide>, out: &mut Vec<TabBarBox>) {
    if !has_panels(node) {
        return;
    }
    if let DockNode::Tabs {
        panels,
        active_index,
    } = node
    {
        out.push(TabBarBox {
            rect,
            anchor: panels[0].clone(),
            key: panels[0].clone(),
            panels: panels.clone(),
            active: active_index.unwrap_or(0).min(panels.len() - 1),
            region,
        });
        return;
    }
    let DockNode::Split {
        axis,
        children,
        sizes,
    } = node
    else {
        return;
    };
    let slots: Vec<(usize, &DockNode)> = children
        .iter()
        .enumerate()
        .filter(|(_, child)| has_panels(child))
        .collect();
    if slots.is_empty() {
        return;
    }
    let horizontal = *axis == Axis::Horizontal;
    let total = if horizontal { rect.width } else { rect.height };
    let space = (total - SPLIT_HANDLE * (slots.len() as f32 - 1.)).max(0.);
    // Every slot but the last takes its recorded size; the last takes the
    // rest. A slot with no size shares what the sized ones leave.
    let (sized, loose) =
        slots[..slots.len() - 1]
            .iter()
            .fold((0., 0usize), |(sum, loose), (child_index, _)| {
                let size = sizes
                    .as_ref()
                    .and_then(|sizes| sizes.get(*child_index).copied());
                match size {
                    Some(size) => (sum + size, loose),
                    None => (sum, loose + 1),
                }
            });
    let share = if loose > 0 {
        ((space - sized) / (loose as f32 + 1.)).max(0.)
    } else {
        0.
    };
    let mut at = if horizontal { rect.x } else { rect.y };
    let mut used = 0.;
    for (index, (child_index, child)) in slots.iter().enumerate() {
        let last = index == slots.len() - 1;
        let size = if last {
            (space - used).max(0.)
        } else {
            sizes
                .as_ref()
                .and_then(|sizes| sizes.get(*child_index).copied())
                .unwrap_or(share)
        };
        used += size;
        let child_rect = if horizontal {
            Rect {
                x: at,
                y: rect.y,
                width: size,
                height: rect.height,
            }
        } else {
            Rect {
                x: rect.x,
                y: at,
                width: rect.width,
                height: size,
            }
        };
        walk(child, child_rect, region, out);
        at += size + SPLIT_HANDLE;
    }
}

/// The boxes a canvas's tab bars occupy, for the chrome drawn over them: the
/// center's groups in its own area, then each open dock's group in its frame.
///
/// A zoom names a center sheet: that group's box expands over the center
/// area (the docks stay put). A zoom naming a dock sheet — or a sheet nobody
/// has — is no zoom at all. Below `COMPACT_WIDTH` only the center's first
/// group shows, full canvas; the docks render nothing.
pub fn tab_bar_boxes(
    layout: &DockLayout,
    width: f32,
    height: f32,
    zoomed: Option<&str>,
) -> Vec<TabBarBox> {
    if width <= 0. || height <= 0. {
        return Vec::new();
    }
    let frame = canvas_frame(layout, width, height, zoomed);
    let mut boxes = Vec::new();
    walk(&layout.center, frame.center, None, &mut boxes);
    if frame.compact {
        return match boxes.first() {
            Some(first) => vec![TabBarBox {
                rect: frame.center,
                ..first.clone()
            }],
            None => Vec::new(),
        };
    }
    // Dock groups keep their frames in every zoom state: the zoom is a
    // center fact, the docks stay put.
    let mut dock_boxes = Vec::new();
    for dock in &frame.docks {
        if !dock.open {
            continue;
        }
        if let Some(region) = layout.region(dock.side) {
            walk(&region.layout, dock.rect, Some(dock.side), &mut dock_boxes);
        }
    }
    if let Some(only) = zoomed
        && let Some(found) = boxes
            .iter()
            .find(|box_| box_.region.is_none() && box_.panels.iter().any(|panel| panel == only))
    {
        let mut only_boxes = vec![TabBarBox {
            rect: frame.center,
            ..found.clone()
        }];
        only_boxes.extend(dock_boxes);
        return only_boxes;
    }
    boxes.extend(dock_boxes);
    boxes
}

/// The content areas of a canvas's groups, from the boxes of their bars.
pub fn group_frames(boxes: &[TabBarBox], bar_height: f32) -> Vec<GroupFrame> {
    boxes
        .iter()
        .map(|box_| GroupFrame {
            key: box_.key.clone(),
            panels: box_.panels.clone(),
            active: box_.active,
            rect: box_.rect.inset_top(bar_height),
        })
        .collect()
}

/// The groups of the canvases put aside, at the canvas's own size: a parked
/// sheet keeps the place it had, so coming back finds it where it was.
pub fn parked_groups(
    held: &HeldLayouts,
    width: f32,
    height: f32,
    bar_height: f32,
) -> Vec<GroupFrame> {
    let mut groups = Vec::new();
    for id in held.ids() {
        let Some(canvas) = held.get(id) else {
            continue;
        };
        groups.extend(group_frames(
            &tab_bar_boxes(&canvas.layout, width, height, None),
            bar_height,
        ));
    }
    groups
}

/// Where every sheet sits: the canvas's own groups first, then — parked, so
/// they stay mounted — the sheets of the canvases put aside, each in the
/// place it had.
pub fn sheet_frames(on_screen: &[GroupFrame], parked: &[GroupFrame]) -> Vec<SheetFrame> {
    let mut frames = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for group in on_screen {
        for (index, id) in group.panels.iter().enumerate() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let place = index as i32 - group.active as i32;
            frames.push(SheetFrame {
                id: id.clone(),
                group: group.key.clone(),
                index,
                place,
                rect: group.rect,
                offset: place as f32 * group.rect.width,
                shown: index == group.active,
                near: place.abs() <= 1,
            });
        }
    }
    for group in parked {
        for (index, id) in group.panels.iter().enumerate() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let place = index as i32 - group.active as i32;
            frames.push(SheetFrame {
                id: id.clone(),
                group: group.key.clone(),
                index,
                place,
                rect: group.rect,
                offset: place as f32 * group.rect.width,
                shown: false,
                near: false,
            });
        }
    }
    frames
}

/// Every sheet the app holds open across the canvases — the shown one's, then
/// the parked ones' — in a stable order, without repeats.
///
/// This is the resource face's only question ("does any canvas still show
/// this id?"): a layout edit that closes a sheet while a parked canvas keeps
/// it must not reclaim the resource underneath.
pub fn open_sheet_ids(layout: &DockLayout, held: &HeldLayouts) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let parked = held
        .ids()
        .iter()
        .filter_map(|id| held.get(id))
        .flat_map(|canvas| layout::panel_ids(&canvas.layout));
    for id in layout::panel_ids(layout).into_iter().chain(parked) {
        if seen.insert(id.to_string()) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// What a drop would do and where the chrome draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct DropPreview {
    pub target: DropTarget,
    pub indicator: Indicator,
}

/// The drop indicator's two shapes (DESIGN §3.7): a 2pt brand line for a slot
/// between tabs, a brand-tinted block with a 1pt border for everything else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Indicator {
    Slot(Rect),
    Block(Rect),
}

/// Where a pointer would land a drag, and what to draw for it.
///
/// Priority: an open dock's content is the explicit placement surface and
/// outranks everything; then a group's outer edge bands (split), then its tab
/// strip (a slot), then its content (join the group). The canvas's outermost
/// ring has no target of its own — it is the outermost group's edge, so a
/// single-group layout dragged to the right edge splits right. Compact
/// frames have no dock targets (the docks do not render).
pub fn drop_target(
    frame: &CanvasFrame,
    boxes: &[TabBarBox],
    pointer: Point,
) -> Option<DropPreview> {
    for dock in &frame.docks {
        if dock.open && dock.rect.is_visible() && dock.rect.contains(pointer) {
            return Some(DropPreview {
                target: DropTarget::Dock { side: dock.side },
                indicator: Indicator::Block(dock.rect),
            });
        }
    }
    for box_ in boxes {
        let content = box_.rect.inset_top(TAB_STRIP_H);
        let in_strip = box_.rect.contains(pointer) && !content.contains(pointer);
        if in_strip {
            // The switcher lays its tabs out in equal shares, so the slot
            // boundaries are shares of the strip — the same math in reverse.
            let count = box_.panels.len().max(1);
            let share = box_.rect.width / count as f32;
            let index =
                (((pointer.x - box_.rect.x) / share).ceil() as i32).clamp(0, count as i32) as usize;
            let at = box_.rect.x + index as f32 * share;
            let slot = Rect {
                x: (at - 1.).max(box_.rect.x),
                y: box_.rect.y + (TAB_STRIP_H - 22.) / 2.,
                width: 2.,
                height: 22.,
            };
            return Some(DropPreview {
                target: DropTarget::TabSlot {
                    anchor: box_.anchor.clone(),
                    index,
                },
                indicator: Indicator::Slot(slot),
            });
        }
        if !content.contains(pointer) {
            continue;
        }
        let band_w = (content.width * EDGE_BAND_RATIO).min(EDGE_BAND_MAX);
        let band_h = (content.height * EDGE_BAND_RATIO).min(EDGE_BAND_MAX);
        let edge = if pointer.x < content.x + band_w {
            Some(SplitEdge::Left)
        } else if pointer.x > content.x + content.width - band_w {
            Some(SplitEdge::Right)
        } else if pointer.y < content.y + band_h {
            Some(SplitEdge::Top)
        } else if pointer.y > content.y + content.height - band_h {
            Some(SplitEdge::Bottom)
        } else {
            None
        };
        let half = |edge: SplitEdge| -> Rect {
            // The half of the group the new group would take.
            match edge {
                SplitEdge::Left => Rect {
                    width: content.width / 2.,
                    ..content
                },
                SplitEdge::Right => Rect {
                    x: content.x + content.width / 2.,
                    width: content.width / 2.,
                    ..content
                },
                SplitEdge::Top => Rect {
                    height: content.height / 2.,
                    ..content
                },
                SplitEdge::Bottom => Rect {
                    y: content.y + content.height / 2.,
                    height: content.height / 2.,
                    ..content
                },
            }
        };
        return match edge {
            Some(edge) => Some(DropPreview {
                target: DropTarget::Split {
                    anchor: box_.anchor.clone(),
                    side: edge,
                },
                indicator: Indicator::Block(half(edge)),
            }),
            None => Some(DropPreview {
                target: DropTarget::Group {
                    anchor: box_.anchor.clone(),
                },
                indicator: Indicator::Block(content),
            }),
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{self, DropTarget, SplitEdge};
    use crate::state::CanvasSnapshot;

    const BAR: f32 = 30.;

    fn sheets(ids: &[&str]) -> DockLayout {
        DockLayout::for_sheets(ids.iter().map(|id| (*id).to_string()))
    }

    fn panels(box_: &TabBarBox) -> Vec<&str> {
        box_.panels.iter().map(String::as_str).collect()
    }

    /// A layout with one dock region seeded (size, open, panels).
    fn docked(
        layout: &DockLayout,
        side: DockSide,
        size: Option<f32>,
        open: bool,
        panels: &[&str],
    ) -> DockLayout {
        let mut next = layout.clone();
        let region = match side {
            DockSide::Left => next.left.get_or_insert_with(DockRegion::default),
            DockSide::Right => next.right.get_or_insert_with(DockRegion::default),
            DockSide::Bottom => next.bottom.get_or_insert_with(DockRegion::default),
        };
        region.layout = DockNode::tabs(panels.iter().map(|id| id.to_string()));
        region.size = size;
        region.open = open;
        next
    }

    fn split_layout() -> DockLayout {
        let center = DockNode::split(
            Axis::Horizontal,
            vec![
                DockNode::tabs(["a".to_string()]),
                DockNode::tabs(["b".to_string()]),
            ],
        );
        DockLayout {
            center,
            ..DockLayout::default()
        }
    }

    #[test]
    fn a_tabs_only_canvas_fills_the_area() {
        let boxes = tab_bar_boxes(&sheets(&["a", "b"]), 800., 600., None);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].rect, Rect::new(800., 600.));
        assert_eq!(panels(&boxes[0]), ["a", "b"]);
        assert_eq!(boxes[0].active, 0);
        assert_eq!(boxes[0].anchor, "a");
        assert_eq!(boxes[0].key, "a");
        assert_eq!(boxes[0].region, None);
    }

    #[test]
    fn an_empty_canvas_draws_nothing() {
        let boxes = tab_bar_boxes(&DockLayout::default(), 800., 600., None);
        assert!(boxes.is_empty());
        assert!(tab_bar_boxes(&sheets(&["a"]), 0., 600., None).is_empty());
        assert!(tab_bar_boxes(&sheets(&["a"]), 800., 0., None).is_empty());
    }

    #[test]
    fn a_split_gives_each_slot_its_size_and_the_last_the_rest() {
        let mut layout = split_layout();
        layout.center = match layout.center {
            DockNode::Split { axis, children, .. } => DockNode::Split {
                axis,
                children,
                sizes: Some(vec![300.]),
            },
            other => other,
        };
        let boxes = tab_bar_boxes(&layout, 800., 600., None);
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].rect.width, 300.);
        // 800 - 300 - one 1pt handle.
        assert_eq!(boxes[1].rect.width, 499.);
        assert_eq!(boxes[1].rect.x, 301.);
    }

    #[test]
    fn unsized_slots_share_what_the_sized_ones_leave() {
        let center = DockNode::Split {
            axis: Axis::Vertical,
            children: vec![
                DockNode::tabs(["a".to_string()]),
                DockNode::tabs(["b".to_string()]),
                DockNode::tabs(["c".to_string()]),
            ],
            sizes: Some(vec![100.]),
        };
        let layout = DockLayout {
            center,
            ..DockLayout::default()
        };
        let boxes = tab_bar_boxes(&layout, 800., 602., None);
        // 602 - 100 - two handles = 500, shared by the two unsized slots.
        assert_eq!(boxes[0].rect.height, 100.);
        assert_eq!(boxes[1].rect.height, 250.);
        assert_eq!(boxes[2].rect.height, 250.);
        assert_eq!(boxes[1].rect.y, 101.);
        assert_eq!(boxes[2].rect.y, 352.);
    }

    #[test]
    fn empty_groups_are_skipped_and_do_not_take_a_slot() {
        let center = DockNode::Split {
            axis: Axis::Horizontal,
            children: vec![
                DockNode::empty_tabs(),
                DockNode::tabs(["a".to_string()]),
                DockNode::tabs(["b".to_string()]),
            ],
            sizes: None,
        };
        let layout = DockLayout {
            center,
            ..DockLayout::default()
        };
        let boxes = tab_bar_boxes(&layout, 800., 600., None);
        assert_eq!(boxes.len(), 2);
        assert_eq!(panels(&boxes[0]), ["a"]);
        // The empty child takes no slot: no handle is charged for it.
        assert_eq!(boxes[1].rect.x, 400.5);
    }

    #[test]
    fn a_zoom_takes_the_center_area_and_only_its_own_bar_shows() {
        let layout = split_layout();
        let boxes = tab_bar_boxes(&layout, 800., 600., Some("b"));
        assert_eq!(boxes.len(), 1);
        assert_eq!(panels(&boxes[0]), ["b"]);
        assert_eq!(boxes[0].rect, Rect::new(800., 600.));
        // A zoom on a sheet nobody has is not a zoom.
        assert_eq!(tab_bar_boxes(&layout, 800., 600., Some("ghost")).len(), 2);
    }

    #[test]
    fn a_compact_canvas_presents_one_group() {
        let layout = split_layout();
        let boxes = tab_bar_boxes(&layout, COMPACT_WIDTH - 1., 600., None);
        assert_eq!(boxes.len(), 1);
        assert_eq!(panels(&boxes[0]), ["a"]);
        assert_eq!(boxes[0].rect, Rect::new(COMPACT_WIDTH - 1., 600.));
        // At the threshold the split comes back.
        assert_eq!(tab_bar_boxes(&layout, COMPACT_WIDTH, 600., None).len(), 2);
    }

    #[test]
    fn docks_take_their_edges_and_the_center_takes_the_rest() {
        let layout = docked(
            &docked(&sheets(&["a"]), DockSide::Left, None, true, &["l1"]),
            DockSide::Bottom,
            None,
            true,
            &["t1"],
        );
        let frame = canvas_frame(&layout, 1000., 800., None);
        let left = frame
            .docks
            .iter()
            .find(|d| d.side == DockSide::Left)
            .expect("left frame");
        assert_eq!(
            left.rect,
            Rect {
                x: 0.,
                y: 0.,
                width: 200.,
                height: 800.
            }
        );
        let bottom = frame
            .docks
            .iter()
            .find(|d| d.side == DockSide::Bottom)
            .expect("bottom frame");
        // The bottom dock sits between the side strips and eats height; the
        // 1pt handle rides between it and the center.
        assert_eq!(
            bottom.rect,
            Rect {
                x: 201.,
                y: 600.,
                width: 799.,
                height: 200.
            }
        );
        assert_eq!(
            frame.center,
            Rect {
                x: 201.,
                y: 0.,
                width: 799.,
                height: 599.
            }
        );
        // Each present region carries a drag band on its inner edge.
        assert_eq!(frame.dock_handles.len(), 2);
        // The groups render: center, left dock, bottom dock — each tagged.
        let boxes = tab_bar_boxes(&layout, 1000., 800., None);
        assert_eq!(
            boxes.iter().map(|b| b.region).collect::<Vec<_>>(),
            [None, Some(DockSide::Left), Some(DockSide::Bottom)]
        );
    }

    #[test]
    fn side_docks_flank_the_bottom_one() {
        let layout = docked(
            &docked(&sheets(&["a"]), DockSide::Left, None, true, &["l1"]),
            DockSide::Right,
            None,
            true,
            &["r1"],
        );
        let frame = canvas_frame(&layout, 1000., 800., None);
        let left = frame
            .docks
            .iter()
            .find(|d| d.side == DockSide::Left)
            .unwrap();
        let right = frame
            .docks
            .iter()
            .find(|d| d.side == DockSide::Right)
            .unwrap();
        assert_eq!(left.rect.width, 200.);
        // The right strip starts after the middle and its own seam.
        assert_eq!(right.rect.x, 800.);
        assert_eq!(right.rect.width, 200.);
        // Side docks take full height; no bottom dock leaves the center full
        // height too.
        assert_eq!(left.rect.height, 800.);
        assert_eq!(frame.center.height, 800.);
        assert_eq!(frame.center.width, 1000. - 200. - 200. - 2.);
    }

    #[test]
    fn a_closed_dock_leaves_only_its_handle() {
        let layout = docked(
            &sheets(&["a"]),
            DockSide::Bottom,
            Some(220.),
            false,
            &["t1"],
        );
        let frame = canvas_frame(&layout, 1000., 800., None);
        let bottom = frame
            .docks
            .iter()
            .find(|d| d.side == DockSide::Bottom)
            .unwrap();
        assert!(!bottom.open);
        assert!(!bottom.rect.is_visible());
        // The center keeps everything the closed dock would have taken.
        assert_eq!(frame.center.height, 800.);
        // Its drag band still exists, at the canvas edge.
        let handle = frame
            .dock_handles
            .iter()
            .find(|h| h.side == DockSide::Bottom)
            .unwrap();
        assert_eq!(handle.rect.y, 800. - DOCK_HANDLE);
        // A closed dock's group renders no bar.
        assert!(
            tab_bar_boxes(&layout, 1000., 800., None)
                .iter()
                .all(|b| b.region.is_none())
        );
    }

    #[test]
    fn a_docks_size_clamps_against_the_center_share() {
        let layout = docked(&sheets(&["a"]), DockSide::Left, Some(40.), true, &["l1"]);
        let frame = canvas_frame(&layout, 1000., 800., None);
        // The layout's floor is 100.
        assert_eq!(frame.docks[0].rect.width, 100.);
        let big = docked(&sheets(&["a"]), DockSide::Left, Some(2000.), true, &["l1"]);
        let frame = canvas_frame(&big, 1000., 800., None);
        // The center's guaranteed share wins: 1000 - 200 = 800.
        assert_eq!(frame.docks[0].rect.width, 800.);
    }

    #[test]
    fn a_zoomed_center_group_leaves_the_docks_alone() {
        let center = DockNode::split(
            Axis::Horizontal,
            vec![
                DockNode::tabs(["a".to_string()]),
                DockNode::tabs(["b".to_string()]),
            ],
        );
        let layout = docked(
            &DockLayout {
                center,
                ..DockLayout::default()
            },
            DockSide::Bottom,
            None,
            true,
            &["t1"],
        );
        let boxes = tab_bar_boxes(&layout, 1000., 800., Some("a"));
        // The zoomed center box covers the center area (above the dock), and
        // the dock group still renders.
        assert_eq!(boxes.len(), 2);
        let zoomed = boxes.iter().find(|b| b.region.is_none()).unwrap();
        assert_eq!(
            zoomed.rect,
            Rect {
                x: 0.,
                y: 0.,
                width: 1000.,
                height: 599.
            }
        );
        // Zooming a dock sheet is ignored: no center box matches, and the
        // plain layout shows through (two center groups + the dock group).
        let boxes = tab_bar_boxes(&layout, 1000., 800., Some("t1"));
        assert_eq!(boxes.len(), 3);
    }

    #[test]
    fn compact_frames_have_no_docks() {
        let layout = docked(&split_layout(), DockSide::Bottom, None, true, &["t1"]);
        let frame = canvas_frame(&layout, COMPACT_WIDTH - 1., 600., None);
        assert!(frame.compact);
        assert!(frame.docks.is_empty());
        assert!(frame.dock_handles.is_empty());
        assert_eq!(frame.center, Rect::new(COMPACT_WIDTH - 1., 600.));
        // Widening the window brings the dock back untouched.
        let frame = canvas_frame(&layout, COMPACT_WIDTH, 600., None);
        assert!(!frame.compact);
        assert_eq!(frame.docks.len(), 1);
    }

    #[test]
    fn a_center_split_grows_drag_bands_on_its_seams() {
        let mut layout = split_layout();
        layout.center = match layout.center {
            DockNode::Split { axis, children, .. } => DockNode::Split {
                axis,
                children,
                sizes: Some(vec![300.]),
            },
            other => other,
        };
        let frame = canvas_frame(&layout, 800., 600., None);
        assert_eq!(frame.handles.len(), 1);
        let handle = &frame.handles[0];
        assert_eq!(handle.axis, Axis::Horizontal);
        // The handle names the split itself (what `set_split_sizes` edits).
        assert_eq!(handle.path, Vec::<usize>::new());
        // The band straddles the 1pt seam after the first slot.
        assert_eq!(
            handle.rect,
            Rect {
                x: 300. + SPLIT_HANDLE / 2. - DOCK_HANDLE / 2.,
                y: 0.,
                width: DOCK_HANDLE,
                height: 600.
            }
        );
    }

    #[test]
    fn a_group_frame_is_the_bar_box_without_the_bar() {
        let frames = group_frames(&tab_bar_boxes(&sheets(&["a"]), 800., 600., None), BAR);
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].rect,
            Rect {
                x: 0.,
                y: BAR,
                width: 800.,
                height: 600. - BAR
            }
        );
        assert_eq!(frames[0].key, "a");
        assert_eq!(frames[0].active, 0);
    }

    #[test]
    fn sheet_frames_place_every_tab_around_the_displayed_one() {
        let layout = layout::activate_sheet(&sheets(&["a", "b", "c"]), "b");
        let frames = sheet_frames(
            &group_frames(&tab_bar_boxes(&layout, 800., 600., None), BAR),
            &[],
        );
        assert_eq!(frames.len(), 3);
        assert!(frames[1].shown);
        assert_eq!(frames[1].offset, 0.);
        assert_eq!(frames[0].place, -1);
        assert_eq!(frames[0].offset, -800.);
        assert!(!frames[0].shown && frames[0].near);
        assert_eq!(frames[2].place, 1);
        assert_eq!(frames[2].offset, 800.);
        assert!(frames[2].near);
        assert!(frames.iter().all(|frame| frame.rect.y == BAR));
        assert!(frames.iter().all(|frame| frame.group == "a"));
    }

    #[test]
    fn far_tabs_are_mounted_but_not_near() {
        let mut ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        ids.push("d".to_string());
        let layout = layout::activate_sheet(&DockLayout::for_sheets(ids), "c");
        let frames = sheet_frames(
            &group_frames(&tab_bar_boxes(&layout, 800., 600., None), BAR),
            &[],
        );
        assert!(
            !frames[0].near,
            "two places away is not drawn while sliding"
        );
        assert_eq!(frames[0].place, -2);
    }

    #[test]
    fn a_parked_canvas_keeps_its_sheets_in_place() {
        let mut state = crate::state::CanvasState::new();
        state.switch_container("c1", None, |_| true);
        let ticket = state.take_ticket();
        state.set_layout(
            "c1",
            layout::append_sheet(state.layout(), "session:s1", None),
            ticket,
        );
        state.switch_container("c2", None, |_| true);
        let held = state.held().clone();
        assert!(held.contains("c1"));

        let parked = parked_groups(&held, 800., 600., BAR);
        let frames = sheet_frames(&[], &parked);
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|frame| !frame.shown && !frame.near));
        assert_eq!(frames[0].id, "home:c1");
        assert_eq!(frames[1].id, "session:s1");
        assert_eq!(frames[0].place, -1);
        assert_eq!(frames[0].offset, -800.);
        assert_eq!(frames[1].place, 0);
        assert_eq!(frames[1].offset, 0.);
    }

    #[test]
    fn a_sheet_on_both_the_screen_and_a_parked_canvas_is_drawn_once() {
        let layout = sheets(&["session:s1"]);
        let mut state = crate::state::CanvasState::new();
        state.switch_container("c1", None, |_| true);
        state.switch_container("c2", None, |_| true);
        let held = state.held().clone();

        let on_screen = group_frames(&tab_bar_boxes(&layout, 800., 600., None), BAR);
        let frames = sheet_frames(&on_screen, &parked_groups(&held, 800., 600., BAR));
        let shown: Vec<&str> = frames
            .iter()
            .filter(|frame| frame.id == "session:s1")
            .map(|frame| frame.id.as_str())
            .collect();
        assert_eq!(shown, ["session:s1"]);
        assert!(frames[0].shown, "the on-screen copy wins");
    }

    #[test]
    fn open_sheet_ids_is_the_union_across_canvases() {
        let shown = sheets(&["a", "b"]);
        let mut state = crate::state::CanvasState::new();
        state.switch_container("c1", None, |_| true);
        let ticket = state.take_ticket();
        state.set_layout(
            "c1",
            layout::append_sheet(state.layout(), "terminal:c1:1", None),
            ticket,
        );
        state.switch_container("c2", None, |_| true);
        let held = state.held().clone();

        let ids = open_sheet_ids(&shown, &held);
        assert!(ids.contains(&"terminal:c1:1".to_string()));
        assert!(ids.contains(&"a".to_string()));
        let mut deduped = ids.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(deduped.len(), ids.len());
    }

    #[test]
    fn a_parked_canvas_snapshot_is_what_parking_stored() {
        let mut state = crate::state::CanvasState::new();
        state.switch_container("c1", None, |_| true);
        let ticket = state.take_ticket();
        let layout = layout::append_sheet(state.layout(), "session:s1", None);
        state.set_layout("c1", layout.clone(), ticket);
        state.switch_container("c2", None, |_| true);
        assert_eq!(
            state.held().get("c1"),
            Some(&CanvasSnapshot { layout, zoom: None })
        );
    }

    // ── drop_target ───────────────────────────────────────────────────

    fn target_at(
        layout: &DockLayout,
        width: f32,
        height: f32,
        x: f32,
        y: f32,
    ) -> Option<DropTarget> {
        let frame = canvas_frame(layout, width, height, None);
        let boxes = tab_bar_boxes(layout, width, height, None);
        drop_target(&frame, &boxes, Point { x, y }).map(|preview| preview.target)
    }

    #[test]
    fn an_open_dock_outranks_every_other_target() {
        let layout = docked(&sheets(&["a"]), DockSide::Bottom, None, true, &["t1"]);
        let frame = canvas_frame(&layout, 1000., 800., None);
        let boxes = tab_bar_boxes(&layout, 1000., 800., None);
        // Deep inside the dock's content — the center group's content is far away.
        let pointer = Point { x: 500., y: 700. };
        let preview = drop_target(&frame, &boxes, pointer).expect("a dock target");
        assert_eq!(
            preview.target,
            DropTarget::Dock {
                side: DockSide::Bottom
            }
        );
        assert!(matches!(preview.indicator, Indicator::Block(_)));
    }

    #[test]
    fn a_single_groups_right_edge_is_a_split_not_a_dock() {
        let target = target_at(&sheets(&["a"]), 1000., 800., 990., 400.);
        assert_eq!(
            target,
            Some(DropTarget::Split {
                anchor: "a".into(),
                side: SplitEdge::Right
            })
        );
    }

    #[test]
    fn each_edge_band_splits_off_its_own_edge() {
        let layout = sheets(&["a"]);
        // Content area: y 34..800, x 0..1000. Bands: 25% capped at 60.
        assert_eq!(
            target_at(&layout, 1000., 800., 30., 400.),
            Some(DropTarget::Split {
                anchor: "a".into(),
                side: SplitEdge::Left
            })
        );
        assert_eq!(
            target_at(&layout, 1000., 800., 970., 400.),
            Some(DropTarget::Split {
                anchor: "a".into(),
                side: SplitEdge::Right
            })
        );
        assert_eq!(
            target_at(&layout, 1000., 800., 500., 90.),
            Some(DropTarget::Split {
                anchor: "a".into(),
                side: SplitEdge::Top
            })
        );
        assert_eq!(
            target_at(&layout, 1000., 800., 500., 790.),
            Some(DropTarget::Split {
                anchor: "a".into(),
                side: SplitEdge::Bottom
            })
        );
    }

    #[test]
    fn the_strip_yields_a_slot_boundary_and_the_content_yields_the_group() {
        let layout = sheets(&["a", "b", "c"]);
        // Strip share = 1000/3; x=420 falls past boundary 1 (333) → slot 2.
        assert_eq!(
            target_at(&layout, 1000., 800., 420., 17.),
            Some(DropTarget::TabSlot {
                anchor: "a".into(),
                index: 2
            })
        );
        // Past the last tab → "after the last" (index 3).
        assert_eq!(
            target_at(&layout, 1000., 800., 990., 17.),
            Some(DropTarget::TabSlot {
                anchor: "a".into(),
                index: 3
            })
        );
        // The content middle joins the group.
        assert_eq!(
            target_at(&layout, 1000., 800., 500., 400.),
            Some(DropTarget::Group { anchor: "a".into() })
        );
    }

    #[test]
    fn adjacent_groups_own_their_own_edges() {
        let layout = split_layout();
        // Left group: 0..399.5; right group: 400.5..800. The right band of
        // the left group is inside its own content; the left band of the
        // right group starts at 400.5.
        assert_eq!(
            target_at(&layout, 800., 600., 390., 300.),
            Some(DropTarget::Split {
                anchor: "a".into(),
                side: SplitEdge::Right
            })
        );
        assert_eq!(
            target_at(&layout, 800., 600., 410., 300.),
            Some(DropTarget::Split {
                anchor: "b".into(),
                side: SplitEdge::Left
            })
        );
        // Outside every group (the 1pt seam) is no target at all.
        assert_eq!(target_at(&layout, 800., 600., 400., 300.), None);
    }

    #[test]
    fn compact_has_no_dock_targets() {
        let layout = docked(&sheets(&["a"]), DockSide::Bottom, None, true, &["t1"]);
        // The bottom dock's content would be y 400..599 at this height —
        // compact collapses it, so the point lands the full-canvas group's
        // bottom band instead. No target is ever a dock.
        let target = target_at(&layout, COMPACT_WIDTH - 1., 600., 300., 590.);
        assert!(matches!(
            target,
            Some(DropTarget::Split {
                side: SplitEdge::Bottom,
                ..
            })
        ));
        assert!(!matches!(target, Some(DropTarget::Dock { .. })));
    }

    #[test]
    fn the_slot_indicator_is_a_2pt_line_and_the_block_half_the_group() {
        let layout = sheets(&["a", "b"]);
        let frame = canvas_frame(&layout, 1000., 800., None);
        let boxes = tab_bar_boxes(&layout, 1000., 800., None);
        let preview = drop_target(&frame, &boxes, Point { x: 600., y: 17. }).unwrap();
        match preview.indicator {
            Indicator::Slot(rect) => {
                assert_eq!(rect.width, 2.);
                assert_eq!(rect.height, 22.);
            }
            other => panic!("a strip drop draws a slot line, got {other:?}"),
        }
        let preview = drop_target(&frame, &boxes, Point { x: 500., y: 400. }).unwrap();
        match preview.indicator {
            Indicator::Block(rect) => {
                assert_eq!(
                    rect,
                    Rect {
                        x: 0.,
                        y: TAB_STRIP_H,
                        width: 1000.,
                        height: 800. - TAB_STRIP_H
                    }
                );
            }
            other => panic!("a content drop draws a block, got {other:?}"),
        }
    }
}
