//! The second faces a shaped fence can show: the side-by-side diff view and
//! the merge view. Both render from the shape the fence parser produced
//! ([`crate::code_surface`]'s — the same parser the tool cards' Edit bodies
//! read) with the same window-level metrics the flat panel paints with —
//! mono face, one line height, no wrapping — so a block stays one artifact
//! across its faces.
//!
//! Layout follows the reader's model of a split view: the parser hands back
//! sequential numbered rows, and this view pairs them — a run of deletions
//! followed by a run of additions zips old-left/new-right with the shorter
//! side padded, context lands on both sides, metadata and hunk headers run
//! full width between the column groups. The whole body scrolls
//! horizontally as one region: every row is the same fixed height
//! ([`crate::code_surface::row_height`]) and a column's width is its own
//! longest line. The merge view is the same structure generalized to the
//! conflict's sides: Ours | Base | Theirs columns per conflict (Base only
//! when the fence carries a `|||||||` section), context lines full width
//! between conflicts.

use crate::code_surface::{
    self, CodeRow, CodeRowKind, DiffPart, MergeSegment, gutter_width, row_accent, row_height,
    scroll_root, text_row,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, Styled as _, div, relative,
};
use gpui_component::{Theme, h_flex, v_flex};

/// One paired split row: each side carries its line number and text, or
/// `None` where the counterpart run ran shorter.
struct SplitRow {
    old: Option<(u32, SharedString)>,
    new: Option<(u32, SharedString)>,
    changed: bool,
}

/// Pair a hunk's sequential rows into split rows: deletions and additions
/// accumulate in feed order until context (or the hunk's end) settles them,
/// then zip — the order Git writes is the order a reader pairs.
fn split_rows(rows: &[CodeRow]) -> Vec<SplitRow> {
    let mut paired: Vec<SplitRow> = Vec::new();
    let mut old_run: Vec<(u32, SharedString)> = Vec::new();
    let mut new_run: Vec<(u32, SharedString)> = Vec::new();

    fn settle(
        paired: &mut Vec<SplitRow>,
        old_run: &mut Vec<(u32, SharedString)>,
        new_run: &mut Vec<(u32, SharedString)>,
    ) {
        let pairs = old_run.len().max(new_run.len());
        for index in 0..pairs {
            paired.push(SplitRow {
                old: old_run.get(index).cloned(),
                new: new_run.get(index).cloned(),
                changed: true,
            });
        }
        old_run.clear();
        new_run.clear();
    }

    for row in rows {
        match row.kind {
            CodeRowKind::Removed => {
                old_run.push((
                    row.old.expect("removed rows carry their number"),
                    row.text.clone(),
                ));
            }
            CodeRowKind::Added => {
                new_run.push((
                    row.new.expect("added rows carry their number"),
                    row.text.clone(),
                ));
            }
            CodeRowKind::Context => {
                settle(&mut paired, &mut old_run, &mut new_run);
                // Context rows carry both numbers — they drift apart across
                // the hunk's changes, so each side reads its own.
                paired.push(SplitRow {
                    old: row.old.map(|n| (n, row.text.clone())),
                    new: row.new.map(|n| (n, row.text.clone())),
                    changed: false,
                });
            }
            // A read-gap has no place in a diff; it never arrives.
            CodeRowKind::Gap => settle(&mut paired, &mut old_run, &mut new_run),
        }
    }
    settle(&mut paired, &mut old_run, &mut new_run);
    paired
}

/// The side-by-side face of a parsed diff.
pub(crate) fn split_body(parts: &[DiffPart], key: u64, theme: &Theme) -> AnyElement {
    let row_h = row_height(theme);
    let digits = parts
        .iter()
        .flat_map(|part| match part {
            DiffPart::Meta(_) => None,
            DiffPart::Hunk { rows, .. } => Some(rows),
        })
        .flatten()
        .map(|row| crate::code_surface::gutter_digits([row]))
        .max()
        .unwrap_or(1);

    // At least the viewport wide, wider when the columns demand it — the
    // scroll region must LAYOUT wider than the viewport for the wheel to
    // act; paint overflow alone never scrolls.
    let mut body = v_flex().min_w(relative(1.));
    for part in parts {
        match part {
            DiffPart::Meta(meta) => {
                body = body.child(text_row(meta, theme.muted_foreground, None, row_h));
            }
            DiffPart::Hunk { header, rows } => {
                body = body.child(text_row(
                    header,
                    theme.muted_foreground,
                    Some(theme.secondary.opacity(0.6)),
                    row_h,
                ));
                body = body.child(row_pair_columns(&split_rows(rows), digits, row_h, theme));
            }
        }
    }

    scroll_root(key, theme)
        .debug_selector(|| "markdown-editor-split".into())
        .child(body)
        .into_any_element()
}

/// The merge face of a parsed conflict fence: column groups per conflict,
/// context rows between them.
pub(crate) fn merge_body(segments: &[MergeSegment], key: u64, theme: &Theme) -> AnyElement {
    let row_h = row_height(theme);
    let digits = segments
        .iter()
        .filter_map(|segment| match segment {
            MergeSegment::Context { start, lines } => Some(start + lines.len().saturating_sub(1)),
            MergeSegment::Conflict { .. } => None,
        })
        .map(|last| last.to_string().len())
        .max()
        .unwrap_or(1);

    let mut body = v_flex().min_w(relative(1.));
    for segment in segments {
        match segment {
            MergeSegment::Context { start, lines } => {
                for (offset, line) in lines.iter().enumerate() {
                    body = body.child(code_surface::numbered_row(
                        start + offset,
                        line,
                        theme.foreground,
                        None,
                        digits,
                        row_h,
                        theme,
                    ));
                }
            }
            MergeSegment::Conflict { ours, base, theirs } => {
                body = body.child(conflict_columns(ours, base.as_ref(), theirs, row_h, theme))
            }
        }
    }

    scroll_root(key, theme)
        .debug_selector(|| "markdown-editor-merge".into())
        .child(body)
        .into_any_element()
}

// --- layout -----------------------------------------------------------------

/// Which side of a paired row a column carries — the deleted side or the
/// added one. The tint follows the column, not the row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Old,
    New,
}

/// One hunk's paired columns: the old side left, the new side right, a
/// hairline between. A column's width is its own longest line, so the group
/// is only as wide as the diff needs; the body scrolls it as one region.
fn row_pair_columns(rows: &[SplitRow], digits: usize, row_h: Pixels, theme: &Theme) -> gpui::Div {
    let gutter_w = gutter_width(digits, theme);
    h_flex()
        .items_start()
        .child(side_column(Side::Old, rows, gutter_w, row_h, theme))
        .child(div().w_px().self_stretch().flex_shrink_0().bg(theme.border))
        .child(side_column(Side::New, rows, gutter_w, row_h, theme))
}

/// One side of a split hunk: stacked fixed-height rows — changed ones washed
/// and tinted by their side, context plain, a padded row an empty cell.
fn side_column(
    side: Side,
    rows: &[SplitRow],
    gutter_w: Pixels,
    row_h: Pixels,
    theme: &Theme,
) -> gpui::Div {
    let kind = match side {
        Side::Old => CodeRowKind::Removed,
        Side::New => CodeRowKind::Added,
    };
    let tint = row_accent(kind, theme);
    let wash = tint.opacity(0.15);
    let mut column = v_flex().flex_shrink_0();
    for row in rows {
        let cell = match side {
            Side::Old => &row.old,
            Side::New => &row.new,
        };
        match cell {
            Some((number, text)) => {
                let (fg, bg) = if row.changed {
                    (tint, Some(wash))
                } else {
                    (theme.foreground, None)
                };
                column = column.child(
                    h_flex()
                        .h(row_h)
                        .items_center()
                        .when_some(bg, |row, bg| row.bg(bg))
                        .child(
                            div()
                                .w(gutter_w)
                                .flex_shrink_0()
                                .px_2()
                                .text_right()
                                .text_color(theme.muted_foreground.opacity(0.7))
                                .child(SharedString::from(number.to_string())),
                        )
                        .child(
                            div()
                                .px_2()
                                .flex_shrink_0()
                                .text_color(fg)
                                .child(text.clone()),
                        ),
                );
            }
            None => {
                column = column.child(
                    h_flex()
                        .h(row_h)
                        .items_center()
                        .child(div().w(gutter_w).flex_shrink_0())
                        .child(div().px_2()),
                );
            }
        }
    }
    column
}

/// One conflict's columns: Ours | Base | Theirs, Base only when the fence
/// carried a `|||||||` section. Each column opens with its role chip — the
/// localized role plus the marker's own label — over the side's washed rows.
fn conflict_columns(
    ours: &code_surface::MergeSide,
    base: Option<&code_surface::MergeSide>,
    theirs: &code_surface::MergeSide,
    row_h: Pixels,
    theme: &Theme,
) -> gpui::Div {
    // Only the sides the conflict carries get columns; the dividers run
    // between what is actually shown.
    let (ours_role, base_role, theirs_role) = (
        steer_i18n::t("code-block-merge-ours"),
        steer_i18n::t("code-block-merge-base"),
        steer_i18n::t("code-block-merge-theirs"),
    );
    let mut sides: Vec<(&code_surface::MergeSide, &str, Hsla)> =
        vec![(ours, &ours_role, theme.success)];
    if let Some(base) = base {
        sides.push((base, &base_role, theme.muted_foreground));
    }
    sides.push((theirs, &theirs_role, theme.danger));

    let mut group = h_flex().items_start();
    for (index, (side, role, tint)) in sides.iter().enumerate() {
        if index > 0 {
            group = group.child(div().w_px().self_stretch().flex_shrink_0().bg(theme.border));
        }
        let mut column = v_flex().flex_shrink_0();
        column = column.child(
            h_flex()
                .h(row_h)
                .items_center()
                .gap_2()
                .px_3()
                .bg(tint.opacity(0.22))
                .child(
                    div()
                        .text_color(theme.foreground)
                        .child(SharedString::from((*role).to_string())),
                )
                .children(side.label.clone().map(|label| {
                    div()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(label))
                })),
        );
        if side.lines.is_empty() {
            column = column.child(text_row("", theme.muted_foreground, None, row_h));
        } else {
            for line in &side.lines {
                column = column.child(text_row(
                    line,
                    theme.foreground,
                    Some(tint.opacity(0.15)),
                    row_h,
                ));
            }
        }
        group = group.child(column);
    }
    group
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_surface::CodeRow;

    /// Rows pair within change runs in feed order; the shorter side pads with
    /// empty cells; context lands on both sides carrying its own (drifted)
    /// numbers.
    #[test]
    fn runs_pair_in_feed_order_with_padding() {
        let rows = vec![
            CodeRow::context(1, "keep"),
            CodeRow::removed(2, "gone one"),
            CodeRow::removed(3, "gone two"),
            CodeRow::added(2, "here"),
            // The trailing context: the old side spent two deletions more
            // than the new side, so its numbers have drifted.
            CodeRow {
                old: Some(4),
                new: Some(3),
                kind: CodeRowKind::Context,
                text: "tail".into(),
            },
        ];
        let paired = split_rows(&rows);
        assert_eq!(paired.len(), 4);
        assert!(!paired[0].changed);
        assert_eq!(paired[0].old.as_ref().unwrap().1, "keep");
        assert_eq!(paired[0].new.as_ref().unwrap().1, "keep");
        assert!(paired[1].changed);
        assert_eq!(paired[1].old.as_ref().unwrap().1, "gone one");
        assert_eq!(paired[1].new.as_ref().unwrap().1, "here");
        assert!(
            paired[2].new.is_none(),
            "the deletion run pads the new side"
        );
        assert_eq!(paired[2].old.as_ref().unwrap().1, "gone two");
        assert_eq!(
            paired[3].old.as_ref().unwrap().0,
            4,
            "the trailing context reads the old side's own number"
        );
        assert_eq!(paired[3].new.as_ref().unwrap().0, 3, "and the new side's");
    }

    /// Additions without deletions (a pure insertion run) pad the old side.
    #[test]
    fn an_insertion_run_pads_the_old_side() {
        let rows = vec![CodeRow::added(7, "fresh"), CodeRow::added(8, "more")];
        let paired = split_rows(&rows);
        assert_eq!(paired.len(), 2);
        assert!(paired.iter().all(|row| row.old.is_none()));
        assert_eq!(paired[0].new.as_ref().unwrap().0, 7);
    }
}
