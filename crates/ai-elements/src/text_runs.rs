//! Run sanitization for the card bodies' code-block highlighters.
//!
//! The rich-text core tiles highlight runs over a code block's text: they must
//! be sorted, non-overlapping, in bounds, on character boundaries, and they
//! must sum to the text's length. The core fills the gaps between runs
//! itself, but it assumes the input already satisfies the rest; a highlight
//! range computed against a longer or differently shaped text (a streaming
//! update landing between the parse and the draw) aborts the layout in
//! `StyledText::with_runs` ("invalid text run").
//!
//! [`clamp_runs`] makes the contract structural at the component boundary:
//! clip to the text, snap to character boundaries, drop empty ranges, then
//! sort and truncate overlaps. It sanitizes whatever text the parser hands
//! back; it does not compensate for a particular caller's bug.

use std::ops::Range;

use gpui::HighlightStyle;

/// Clip, snap, sort and de-overlap `runs` against `text`.
///
/// Overlaps resolve in favor of the earlier range (the sort is stable, so
/// equal starts keep the caller's order): a later range's head is truncated
/// to the previous range's end, or dropped when it is fully covered.
pub(crate) fn clamp_runs(
    runs: Vec<(Range<usize>, HighlightStyle)>,
    text: &str,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let len = text.len();
    let mut clamped: Vec<(Range<usize>, HighlightStyle)> = runs
        .into_iter()
        .filter_map(|(mut range, style)| {
            range.start = range.start.min(len);
            range.end = range.end.min(len);
            while range.start < len && !text.is_char_boundary(range.start) {
                range.start += 1;
            }
            while range.end < len && !text.is_char_boundary(range.end) {
                range.end += 1;
            }
            (range.start < range.end).then_some((range, style))
        })
        .collect();
    clamped.sort_by_key(|(range, _)| (range.start, range.end));

    let mut runs: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(clamped.len());
    for (mut range, style) in clamped {
        if let Some((previous, _)) = runs.last()
            && range.start < previous.end
        {
            if range.end <= previous.end {
                continue;
            }
            range.start = previous.end;
        }
        runs.push((range, style));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{HighlightStyle, Hsla};

    fn style() -> HighlightStyle {
        HighlightStyle {
            color: Some(Hsla::default()),
            ..Default::default()
        }
    }

    fn spans(runs: &[(Range<usize>, HighlightStyle)]) -> Vec<Range<usize>> {
        runs.iter().map(|(range, _)| range.clone()).collect()
    }

    #[test]
    fn clips_out_of_bounds_ranges() {
        let text = "hello";
        let runs = clamp_runs(vec![(0..99, style())], text);
        assert_eq!(spans(&runs), vec![0..5]);
    }

    #[test]
    fn snaps_to_character_boundaries() {
        // "中" is 3 bytes; offset 1 and 2 are not boundaries.
        let text = "中x";
        let runs = clamp_runs(vec![(1..4, style())], text);
        assert_eq!(spans(&runs), vec![3..4]);
    }

    #[test]
    fn drops_empty_and_fully_out_of_bounds_ranges() {
        let text = "abc";
        let runs = clamp_runs(
            vec![(1..1, style()), (5..9, style()), (0..2, style())],
            text,
        );
        assert_eq!(spans(&runs), vec![0..2]);
    }

    #[test]
    fn sorts_and_splits_overlaps() {
        let text = "abcdef";
        let runs = clamp_runs(
            vec![(4..6, style()), (0..3, style()), (2..5, style())],
            text,
        );
        // `0..3` wins the 2..3 overlap, so `2..5` contributes only 3..5; the
        // later `4..6` then yields to that split and contributes 5..6.
        assert_eq!(spans(&runs), vec![0..3, 3..5, 5..6]);
    }

    #[test]
    fn drops_ranges_covered_entirely_by_an_earlier_one() {
        let text = "abcdef";
        let runs = clamp_runs(vec![(0..5, style()), (1..3, style())], text);
        assert_eq!(spans(&runs), vec![0..5]);
    }
}
