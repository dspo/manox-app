//! The canvas as a projection of its layout: where each group's tab bar and
//! content area sit, and where each sheet's body sits inside its group.
//!
//! Pure data — no views, no measuring, no gpui. The renderer draws its chrome
//! from these boxes and its content from the frames; a layout edit changes
//! where a sheet is *placed*, never whether it exists.
//!
//! The rules follow the nsh-pi experiment's `projection.ts`, which the sheet
//! host consumed verbatim:
//!
//! - a split's slots are its recorded sizes (the last one takes whatever is
//!   left; unsized slots share what the sized ones leave);
//! - hidden split handles take `SPLIT_HANDLE` points between slots;
//! - a zoomed group — or, below `COMPACT_WIDTH`, the only group on screen —
//!   fills the whole area and is the only one drawn;
//! - a parked canvas's sheets keep the place they had, so restoring a canvas
//!   finds every sheet where it was (no travel to place).

use crate::layout::{self, DockLayout, DockNode};
use crate::state::HeldLayouts;

/// The width of a hidden split handle between two slots.
pub const SPLIT_HANDLE: f32 = 1.0;

/// Below this width the canvas shows one group at a time (nsh's compact
/// threshold).
pub const COMPACT_WIDTH: f32 = 600.0;

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

fn has_panels(node: &DockNode) -> bool {
    match node {
        DockNode::Tabs { panels, .. } => !panels.is_empty(),
        DockNode::Split { children, .. } => children.iter().any(has_panels),
    }
}

fn walk(node: &DockNode, rect: Rect, out: &mut Vec<TabBarBox>) {
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
    let slots: Vec<(&DockNode, Option<f32>)> = children
        .iter()
        .enumerate()
        .map(|(index, child)| {
            (
                child,
                sizes.as_ref().and_then(|sizes| sizes.get(index).copied()),
            )
        })
        .filter(|(child, _)| has_panels(child))
        .collect();
    if slots.is_empty() {
        return;
    }
    let horizontal = *axis == crate::layout::Axis::Horizontal;
    let total = if horizontal { rect.width } else { rect.height };
    let space = (total - SPLIT_HANDLE * (slots.len() as f32 - 1.)).max(0.);
    // Every slot but the last takes its recorded size; the last takes the
    // rest. A slot with no size shares what the sized ones leave.
    let (sized, loose) =
        slots[..slots.len() - 1]
            .iter()
            .fold((0., 0usize), |(sum, loose), (_, size)| match size {
                Some(size) => (sum + size, loose),
                None => (sum, loose + 1),
            });
    let share = if loose > 0 {
        ((space - sized) / (loose as f32 + 1.)).max(0.)
    } else {
        0.
    };
    let mut at = if horizontal { rect.x } else { rect.y };
    let mut used = 0.;
    for (index, (child, size)) in slots.iter().enumerate() {
        let last = index == slots.len() - 1;
        let size = if last {
            (space - used).max(0.)
        } else {
            size.unwrap_or(share)
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
        walk(child, child_rect, out);
        at += size + SPLIT_HANDLE;
    }
}

/// The boxes a canvas's tab bars occupy, for the chrome drawn over them.
pub fn tab_bar_boxes(
    layout: &DockLayout,
    width: f32,
    height: f32,
    zoomed: Option<&str>,
) -> Vec<TabBarBox> {
    if width <= 0. || height <= 0. {
        return Vec::new();
    }
    let mut boxes = Vec::new();
    walk(&layout.center, Rect::new(width, height), &mut boxes);
    // A zoomed group takes the whole area; so does the only group a compact
    // canvas presents. Only its own bar is on screen, full width.
    let only = zoomed.or_else(|| {
        (width < COMPACT_WIDTH)
            .then(|| layout::active_panel_ids(layout, None).first().copied())
            .flatten()
    });
    let Some(only) = only else {
        return boxes;
    };
    // A zoom on a sheet no group holds is not a zoom: the canvas keeps its
    // layout instead of going blank.
    match boxes
        .iter()
        .find(|box_| box_.panels.iter().any(|panel| panel == only))
    {
        Some(found) => vec![TabBarBox {
            rect: Rect::new(width, height),
            ..found.clone()
        }],
        None => boxes,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{self, Axis, DockLayout, DockNode};
    use crate::state::CanvasSnapshot;

    const BAR: f32 = 30.;

    fn sheets(ids: &[&str]) -> DockLayout {
        DockLayout::for_sheets(ids.iter().map(|id| (*id).to_string()))
    }

    fn panels(box_: &TabBarBox) -> Vec<&str> {
        box_.panels.iter().map(String::as_str).collect()
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
        let split = DockNode::Split {
            axis: Axis::Horizontal,
            children: vec![
                DockNode::tabs(["a".to_string()]),
                DockNode::tabs(["b".to_string()]),
            ],
            sizes: Some(vec![300.]),
        };
        let boxes = tab_bar_boxes(&DockLayout { center: split }, 800., 600., None);
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].rect.width, 300.);
        // 800 - 300 - one 1pt handle.
        assert_eq!(boxes[1].rect.width, 499.);
        assert_eq!(boxes[1].rect.x, 301.);
    }

    #[test]
    fn unsized_slots_share_what_the_sized_ones_leave() {
        let split = DockNode::Split {
            axis: Axis::Vertical,
            children: vec![
                DockNode::tabs(["a".to_string()]),
                DockNode::tabs(["b".to_string()]),
                DockNode::tabs(["c".to_string()]),
            ],
            sizes: Some(vec![100.]),
        };
        let boxes = tab_bar_boxes(&DockLayout { center: split }, 800., 602., None);
        // 602 - 100 - two handles = 500, shared by the two unsized slots.
        assert_eq!(boxes[0].rect.height, 100.);
        assert_eq!(boxes[1].rect.height, 250.);
        assert_eq!(boxes[2].rect.height, 250.);
        assert_eq!(boxes[1].rect.y, 101.);
        assert_eq!(boxes[2].rect.y, 352.);
    }

    #[test]
    fn empty_groups_are_skipped_and_do_not_take_a_slot() {
        let split = DockNode::Split {
            axis: Axis::Horizontal,
            children: vec![
                DockNode::empty_tabs(),
                DockNode::tabs(["a".to_string()]),
                DockNode::tabs(["b".to_string()]),
            ],
            sizes: None,
        };
        let boxes = tab_bar_boxes(&DockLayout { center: split }, 800., 600., None);
        assert_eq!(boxes.len(), 2);
        assert_eq!(panels(&boxes[0]), ["a"]);
        // The empty child takes no slot: no handle is charged for it.
        assert_eq!(boxes[1].rect.x, 400.5);
    }

    #[test]
    fn a_zoom_takes_the_whole_area_and_only_its_own_bar_shows() {
        let split = DockNode::Split {
            axis: Axis::Horizontal,
            children: vec![
                DockNode::tabs(["a".to_string()]),
                DockNode::tabs(["b".to_string()]),
            ],
            sizes: None,
        };
        let layout = DockLayout { center: split };
        let boxes = tab_bar_boxes(&layout, 800., 600., Some("b"));
        assert_eq!(boxes.len(), 1);
        assert_eq!(panels(&boxes[0]), ["b"]);
        assert_eq!(boxes[0].rect, Rect::new(800., 600.));
        // A zoom on a sheet nobody has is not a zoom.
        assert_eq!(tab_bar_boxes(&layout, 800., 600., Some("ghost")).len(), 2);
    }

    #[test]
    fn a_compact_canvas_presents_one_group() {
        let split = DockNode::Split {
            axis: Axis::Horizontal,
            children: vec![
                DockNode::tabs(["a".to_string()]),
                DockNode::tabs(["b".to_string()]),
            ],
            sizes: None,
        };
        let layout = DockLayout { center: split };
        let boxes = tab_bar_boxes(&layout, COMPACT_WIDTH - 1., 600., None);
        assert_eq!(boxes.len(), 1);
        assert_eq!(panels(&boxes[0]), ["a"]);
        assert_eq!(boxes[0].rect, Rect::new(COMPACT_WIDTH - 1., 600.));
        // At the threshold the split comes back.
        assert_eq!(tab_bar_boxes(&layout, COMPACT_WIDTH, 600., None).len(), 2);
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
                height: 600. - BAR,
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
        // `b` is displayed: `a` sits one place to the left, `c` one to the
        // right, and only those two are near enough to draw while sliding.
        assert!(frames[1].shown);
        assert_eq!(frames[1].offset, 0.);
        assert_eq!(frames[0].place, -1);
        assert_eq!(frames[0].offset, -800.);
        assert!(!frames[0].shown && frames[0].near);
        assert_eq!(frames[2].place, 1);
        assert_eq!(frames[2].offset, 800.);
        assert!(frames[2].near);
        // Every frame sits at the group's content area.
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
        // Parked canvases are produced through the public flow: a container is
        // opened, gains a sheet, and the next switch puts it aside.
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
        // The parked sheets keep their own group's place — and the displayed
        // tab they were left on.
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

        // A closed tab on screen still lives on the parked canvas: the union
        // is what keeps its resource alive.
        let ids = open_sheet_ids(&shown, &held);
        assert!(ids.contains(&"terminal:c1:1".to_string()));
        assert!(ids.contains(&"a".to_string()));
        // No repeats.
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
}
