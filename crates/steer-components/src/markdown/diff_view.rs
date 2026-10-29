//! The second faces a shaped fence can show: the side-by-side diff view and
//! the merge view. Both render from the shape the fence parser produced (see
//! [`super::code_block`]) with the same window-level metrics the flat panel
//! paints with — mono face, one line height, no wrapping — so a block stays
//! one artifact across its faces.
//!
//! Layout follows the reader's model of a split view: rows pair within a
//! hunk (old column left, new column right, the shorter change run padded
//! with empty cells), file metadata and hunk headers run full width between
//! the column groups, and the whole body scrolls horizontally as one region
//! — the columns keep their rows aligned because every row is the same fixed
//! height and a column's width is its own longest line. The merge view is
//! the same structure generalized to the conflict's sides: Ours | Base |
//! Theirs columns per conflict (Base only when the fence carries a `|||||||`
//! section), context lines full width between conflicts.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px, relative,
};
use gpui_component::{Theme, h_flex, v_flex};

use crate::markdown::code_block::BODY_PAD_Y;
use steer_i18n::t;

/// One parsed line's contribution to a paired split row. The parser strips
/// the diff prefix — the side a row sits on says `+` or `-` better than a
/// character column would.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DiffRow {
    /// The old side's `(line number, text)`, or `None` on a padded row.
    pub(crate) old: Option<(usize, String)>,
    /// The new side's `(line number, text)`, or `None` on a padded row.
    pub(crate) new: Option<(usize, String)>,
    pub(crate) kind: RowKind,
}

/// Whether a paired row carries a change (wash + tinted text on both its
/// populated cells) or context (plain on both).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowKind {
    Context,
    Changed,
}

/// One full-width run between the column groups: file metadata (`diff --git`,
/// `index`, `---`/`+++`) or a hunk's `@@` header, both muted.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum DiffPart {
    Meta(String),
    Hunk { header: String, rows: Vec<DiffRow> },
}

/// One side of a parsed conflict: the marker's label, when the line carried
/// one (`HEAD`, a branch name), and the side's lines.
#[derive(Default, Clone, PartialEq, Eq)]
pub(crate) struct MergeSide {
    pub(crate) label: Option<String>,
    pub(crate) lines: Vec<String>,
}

/// One segment of a conflict fence: a run of context lines (numbered from the
/// fence's first line — the only anchor the shape has) or one complete
/// conflict's sides.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum MergeSegment {
    Context {
        start: usize,
        lines: Vec<String>,
    },
    Conflict {
        ours: MergeSide,
        base: Option<MergeSide>,
        theirs: MergeSide,
    },
}

/// Which side of a paired row a column carries — the deleted side or the
/// added one. The tint follows the column, not the row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Old,
    New,
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
        .flat_map(|row| [row.old.as_ref(), row.new.as_ref()])
        .flatten()
        .map(|(n, _)| n.to_string().len())
        .max()
        .unwrap_or(1);

    let mut body = v_flex().min_w(relative(1.));
    for part in parts {
        match part {
            DiffPart::Meta(meta) => body = body.child(meta_row(meta, None, row_h, theme)),
            DiffPart::Hunk { header, rows } => {
                body = body.child(meta_row(
                    header,
                    Some(theme.secondary.opacity(0.6)),
                    row_h,
                    theme,
                ));
                body = body.child(row_pair_columns(rows, digits, row_h, theme));
            }
        }
    }

    scroll_root(key, theme).child(body).into_any_element()
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
                    body = body.child(numbered_row(
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

    scroll_root(key, theme).child(body).into_any_element()
}

// --- shared primitives ---------------------------------------------------------

/// The one line height every face paints: the flat panel's relative(1.6) at
/// the mono size, as a fixed row height — the invariant that keeps paired
/// columns aligned.
fn row_height(theme: &Theme) -> Pixels {
    theme.mono_font_size * 1.6
}

/// The horizontal-scroll root a face's body sits in — the face owns its
/// horizontal axis, the block shell owns the vertical one.
fn scroll_root(key: u64, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("code-x", key))
        .w_full()
        .min_w_0()
        .py(px(BODY_PAD_Y))
        .overflow_x_scroll()
        .font_family(theme.mono_font_family.clone())
        .text_size(theme.mono_font_size)
        .line_height(relative(1.6))
        .whitespace_nowrap()
}

/// One fixed-height text row: wash and tint over the full row, the cell
/// padding shared with the flat face's code column.
fn text_row(text: &str, fg: Hsla, bg: Option<Hsla>, row_h: Pixels) -> gpui::Div {
    h_flex()
        .h(row_h)
        .items_center()
        .px_3()
        .text_color(fg)
        .when_some(bg, |row, bg| row.bg(bg))
        .child(text.to_string())
}

/// A full-width muted row between column groups — file metadata or a hunk
/// header (the header carries the hunk's wash so it reads as a section mark).
fn meta_row(text: &str, bg: Option<Hsla>, row_h: Pixels, theme: &Theme) -> gpui::Div {
    text_row(text, theme.muted_foreground, bg, row_h)
}

/// A numbered line in the merge view's context runs.
fn numbered_row(
    number: usize,
    text: &str,
    fg: Hsla,
    bg: Option<Hsla>,
    digits: usize,
    row_h: Pixels,
    theme: &Theme,
) -> gpui::Div {
    h_flex()
        .h(row_h)
        .items_center()
        .when_some(bg, |row, bg| row.bg(bg))
        .child(
            div()
                .w(px(8.) + theme.mono_font_size * 0.65 * digits as f32 + px(8.))
                .flex_shrink_0()
                .px_2()
                .text_right()
                .text_color(theme.muted_foreground.opacity(0.7))
                .child(SharedString::from(number.to_string())),
        )
        .child(div().px_2().text_color(fg).child(text.to_string()))
}

/// One hunk's paired columns: the old side left, the new side right, a
/// hairline between. A column's width is its own longest line, so the group
/// is only as wide as the diff needs; the body scrolls it as one region.
fn row_pair_columns(rows: &[DiffRow], digits: usize, row_h: Pixels, theme: &Theme) -> gpui::Div {
    let gutter_w = px(8.) + theme.mono_font_size * 0.65 * digits as f32 + px(8.);
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
    rows: &[DiffRow],
    gutter_w: Pixels,
    row_h: Pixels,
    theme: &Theme,
) -> gpui::Div {
    let (tint, wash) = match side {
        Side::Old => (theme.danger, theme.danger.opacity(0.15)),
        Side::New => (theme.success, theme.success.opacity(0.15)),
    };
    let mut column = v_flex().flex_shrink_0();
    for row in rows {
        let cell = match side {
            Side::Old => &row.old,
            Side::New => &row.new,
        };
        match cell {
            Some((number, text)) => {
                let (fg, bg) = match row.kind {
                    RowKind::Context => (theme.foreground, None),
                    RowKind::Changed => (tint, Some(wash)),
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
                                .child(text.to_string()),
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
    ours: &MergeSide,
    base: Option<&MergeSide>,
    theirs: &MergeSide,
    row_h: Pixels,
    theme: &Theme,
) -> gpui::Div {
    // Only the sides the conflict carries get columns; the dividers run
    // between what is actually shown.
    let (ours_role, base_role, theirs_role) = (
        t("code-block-merge-ours"),
        t("code-block-merge-base"),
        t("code-block-merge-theirs"),
    );
    let mut sides: Vec<(&MergeSide, &str, Hsla)> = vec![(ours, &ours_role, theme.success)];
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
