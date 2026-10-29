//! The code-surface primitives every editor-looking surface composes from:
//! one row model, one metrics/color policy, the fence-shape parsers, and the
//! column scaffold.
//!
//! The app renders code in two deliberately different ways, and this module
//! is what they share:
//!
//! - **In-band** (ai-elements' `CodeView`, the terminal panel): gutter
//!   numbers, markers and code compose into one text document, so drag
//!   select and copy are the window-level selection — what you copy is what
//!   you see, numbers included. Composed as text + highlight runs.
//! - **Panel** (the markdown editor block and its diff/merge views): rows
//!   are laid-out divs behind a headered panel — a custom block cannot join
//!   the window selection, so the copy control carries that duty instead.
//!
//! Both read the same [`CodeRow`] shape, paint with the same
//! [metrics](row_height) and [accents](row_accent), and the same two
//! fence-shape parsers feed them: [`parse_unified_diff`] (the tool cards'
//! Edit bodies and the markdown split view) and [`parse_conflict_segments`]
//! (the markdown merge view). A new code surface starts here, not with a
//! second row model.
//!
//! The contract the two sides pin (tests in `steer-components`
//! `code_block_panel` and ai-elements `code_view_selection`): conversation
//! prose renders effect-first — editor panels, split diffs, merge views,
//! diagrams as images — while tool OUTPUT stays literal — in-band, unified,
//! drag-select copyable, highlighting kept. Sharing this module must never
//! drag the tool surfaces into the panel faces or the reverse.

use gpui::{
    Hsla, InteractiveElement as _, ParentElement as _, Pixels, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px, relative,
};
use gpui_component::{Theme, h_flex};

// --- the row model ----------------------------------------------------------

/// A line's role in an editor surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeRowKind {
    Context,
    Added,
    Removed,
    /// The skipped span between disjoint read windows. Carries no numbers.
    Gap,
}

/// One editor line: its gutter number(s), its role, and its text.
///
/// `old` is the pre-image number a removed/context diff line carries; `new`
/// is the number the file shows after the edit. Plain file reads use `new`
/// alone. This is the single row model behind the in-band surfaces
/// (ai-elements' `CodeView` composes it into text) and the panel surfaces
/// (the markdown diff/merge views lay it out as columns).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeRow {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub kind: CodeRowKind,
    pub text: SharedString,
}

impl CodeRow {
    /// A context line with both numbers — the unified-diff context shape.
    pub fn context(number: u32, text: impl Into<SharedString>) -> Self {
        Self {
            old: Some(number),
            new: Some(number),
            kind: CodeRowKind::Context,
            text: text.into(),
        }
    }

    /// A context line without diff numbers — the plain-read shape.
    pub fn plain(number: u32, text: impl Into<SharedString>) -> Self {
        Self {
            old: None,
            new: Some(number),
            kind: CodeRowKind::Context,
            text: text.into(),
        }
    }

    /// A row with no gutter number: runtime notes, foreign shapes, a diff
    /// line the parser could not place.
    pub fn gutterless(text: impl Into<SharedString>) -> Self {
        Self {
            old: None,
            new: None,
            kind: CodeRowKind::Context,
            text: text.into(),
        }
    }

    pub fn added(number: u32, text: impl Into<SharedString>) -> Self {
        Self {
            old: None,
            new: Some(number),
            kind: CodeRowKind::Added,
            text: text.into(),
        }
    }

    pub fn removed(number: u32, text: impl Into<SharedString>) -> Self {
        Self {
            old: Some(number),
            new: None,
            kind: CodeRowKind::Removed,
            text: text.into(),
        }
    }

    pub fn gap() -> Self {
        Self {
            old: None,
            new: None,
            kind: CodeRowKind::Gap,
            text: SharedString::default(),
        }
    }
}

// --- the shared policy ------------------------------------------------------

/// The theme color a row's kind reads as: added lines the success hue,
/// removed the danger, context the foreground. The one kind→color mapping;
/// surfaces pick their own wash strength on top (in-band keeps it subtle,
/// panels wash wider).
pub fn row_accent(kind: CodeRowKind, theme: &Theme) -> Hsla {
    match kind {
        CodeRowKind::Added => theme.success,
        CodeRowKind::Removed => theme.danger,
        CodeRowKind::Context | CodeRowKind::Gap => theme.foreground,
    }
}

/// The one line height every code surface paints: the flat panel's
/// relative(1.6) at the mono size. As a fixed row height it is the invariant
/// that keeps paired columns aligned.
pub fn row_height(theme: &Theme) -> Pixels {
    theme.mono_font_size * 1.6
}

/// The width of a line-number gutter carrying `digits`: right-aligned
/// numbers with breathing room on both sides.
pub fn gutter_width(digits: usize, theme: &Theme) -> Pixels {
    px(8.) + theme.mono_font_size * 0.65 * digits as f32 + px(8.)
}

/// The gutter digit count for a set of rows: the widest number either side
/// carries, floored at one.
pub fn gutter_digits<'a>(rows: impl IntoIterator<Item = &'a CodeRow>) -> usize {
    rows.into_iter()
        .filter_map(|row| row.old.or(row.new))
        .map(|n| n.to_string().len())
        .max()
        .unwrap_or(1)
}

/// The body's vertical padding, shared by every gutter-and-code layout so
/// their baselines align line for line.
pub const BODY_PAD_Y: f32 = 12.;

// --- the panel scaffold -------------------------------------------------------

/// The horizontal-scroll root a panel face's body sits in — the face owns
/// its horizontal axis, the block shell owns the vertical one.
pub fn scroll_root(key: u64, theme: &Theme) -> gpui::Stateful<gpui::Div> {
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
/// padding shared across faces.
pub fn text_row(text: &str, fg: Hsla, bg: Option<Hsla>, row_h: Pixels) -> gpui::Div {
    let mut row = h_flex().h(row_h).items_center().px_3().text_color(fg);
    if let Some(bg) = bg {
        row = row.bg(bg);
    }
    row.child(text.to_string())
}

/// A numbered line — the merge view's context rows.
pub fn numbered_row(
    number: usize,
    text: &str,
    fg: Hsla,
    bg: Option<Hsla>,
    digits: usize,
    row_h: Pixels,
    theme: &Theme,
) -> gpui::Div {
    let mut row = h_flex()
        .h(row_h)
        .items_center()
        .px_3()
        .text_color(fg)
        .child(
            div()
                .w(gutter_width(digits, theme))
                .flex_shrink_0()
                .px_2()
                .text_right()
                .text_color(theme.muted_foreground.opacity(0.7))
                .child(SharedString::from(number.to_string())),
        )
        .child(div().px_2().child(text.to_string()));
    if let Some(bg) = bg {
        row = row.bg(bg);
    }
    row
}

// --- the unified-diff parser --------------------------------------------------

/// One part of a parsed unified diff: a full-width metadata row (`diff
/// --git`, `index`, `---`/`+++`), or a hunk with its `@@` header and the
/// rows the header's counters numbered. Rows are sequential — deletions,
/// additions and context in feed order, each carrying its own file number;
/// pairing them into side-by-side cells is a view concern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffPart {
    Meta(String),
    Hunk { header: String, rows: Vec<CodeRow> },
}

/// A unified diff parsed into parts. `well_formed` is false when junk crept
/// in — a line with no legal prefix inside a hunk, a hunk header missing its
/// numbers (the mid-stream shape). Such lines degrade to gutter-less rows or
/// metadata; surfaces that need a strict shape (the markdown split view)
/// decline on the flag, surfaces that render whatever arrived (the tool
/// cards) ignore it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedDiff {
    pub parts: Vec<DiffPart>,
    pub well_formed: bool,
}

/// Parse a unified diff. `None` when the text is not diff-like at all — no
/// hunk and no metadata. `\ No newline at end of file` notes drop from the
/// parsed rows; copy keeps the source verbatim at the call sites.
pub fn parse_unified_diff(code: &str) -> Option<ParsedDiff> {
    // Drop the trailing empty element a final newline produces, but nothing
    // else: an empty line inside a hunk is a context line.
    let mut lines: Vec<&str> = code.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }

    let mut parts: Vec<DiffPart> = Vec::new();
    let mut hunk: Option<PartialHunk> = None;
    let mut well_formed = true;

    for line in lines {
        let Some(current) = hunk.as_mut() else {
            // Before the first hunk: the file metadata, or the hunk header
            // that opens the numbered walk.
            if let Some(rest) = line.strip_prefix("@@ -") {
                match hunk_starts(rest) {
                    Some((old, new)) => {
                        hunk = Some(PartialHunk::new(line.to_owned(), old, new));
                    }
                    None => {
                        // A header cut off mid-stream: metadata for now; the
                        // fence upgrades when the numbers land.
                        parts.push(DiffPart::Meta(line.to_owned()));
                        well_formed = false;
                    }
                }
                continue;
            }
            let is_meta = line.is_empty()
                || line.starts_with("diff --git ")
                || line.starts_with("index ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line.starts_with('\\');
            if !is_meta {
                // Prose before any hunk: the text is not carrying a diff —
                // flagged, and only a later hunk header can rescue it.
                well_formed = false;
                continue;
            }
            if !line.is_empty() {
                parts.push(DiffPart::Meta(line.to_owned()));
            }
            continue;
        };
        if let Some(rest) = line.strip_prefix("@@ -") {
            if let Some(finished) = hunk.take() {
                parts.push(finished.ship());
            }
            hunk = hunk_starts(rest)
                .map(|(old, new)| PartialHunk::new(line.to_owned(), old, new))
                .or_else(|| {
                    well_formed = false;
                    None
                });
            continue;
        }
        current.line(line, &mut well_formed);
    }
    if let Some(current) = hunk {
        parts.push(current.ship());
    }
    // No hunk and no metadata: not a diff.
    (!parts.is_empty()).then_some(ParsedDiff { parts, well_formed })
}

/// The hunk under construction: its header, the rows settled so far, and the
/// two change runs waiting to be flushed (deletions first — the order Git
/// writes).
struct PartialHunk {
    header: String,
    rows: Vec<CodeRow>,
    old_run: Vec<(u32, SharedString)>,
    new_run: Vec<(u32, SharedString)>,
    old: u32,
    new: u32,
}

impl PartialHunk {
    fn new(header: String, old: u32, new: u32) -> Self {
        Self {
            header,
            rows: Vec::new(),
            old_run: Vec::new(),
            new_run: Vec::new(),
            old,
            new,
        }
    }

    /// Consume one hunk body line, flipping `well_formed` when the line has
    /// no legal prefix — it degrades to a gutter-less row rather than
    /// failing the parse.
    fn line(&mut self, line: &str, well_formed: &mut bool) {
        if let Some(text) = line.strip_prefix('-') {
            self.old_run.push((self.old, SharedString::from(text)));
            self.old += 1;
        } else if let Some(text) = line.strip_prefix('+') {
            self.new_run.push((self.new, SharedString::from(text)));
            self.new += 1;
        } else {
            // Context — with Git's leading space, or stripped by a tool that
            // trims trailing whitespace — settles both runs. Its two numbers
            // have drifted apart across the hunk's changes, so it carries
            // both explicitly. A `\ No newline` note is presentation, not a
            // line: dropped here. A line with no legal prefix at all
            // degrades to a gutter-less row and flags the fence.
            let context = match (line.strip_prefix(' '), line.strip_prefix('\\')) {
                (Some(text), _) => Some(text),
                (None, Some(_)) => None,
                (None, None) => {
                    *well_formed = false;
                    self.settle_runs();
                    self.rows.push(CodeRow::gutterless(line));
                    return;
                }
            };
            self.settle_runs();
            if let Some(text) = context {
                self.rows.push(CodeRow {
                    old: Some(self.old),
                    new: Some(self.new),
                    kind: CodeRowKind::Context,
                    text: SharedString::from(text),
                });
                self.old += 1;
                self.new += 1;
            }
        }
    }

    /// Flush the pending change runs as their own numbered rows.
    fn settle_runs(&mut self) {
        for (number, text) in self.old_run.drain(..) {
            self.rows.push(CodeRow::removed(number, text));
        }
        for (number, text) in self.new_run.drain(..) {
            self.rows.push(CodeRow::added(number, text));
        }
    }

    fn ship(mut self) -> DiffPart {
        self.settle_runs();
        DiffPart::Hunk {
            header: self.header,
            rows: self.rows,
        }
    }
}

/// The `old`/`new` start numbers of a hunk header's fragment — the text
/// after `@@ -`, which carries or omits the counts: `3,7 +3,7 @@` and the
/// count-less `1 +1 @@` both parse.
fn hunk_starts(fragment: &str) -> Option<(u32, u32)> {
    let (old_part, rest) = fragment.split_once(' ')?;
    let old = old_part.split(',').next()?.parse().ok()?;
    let new_part = rest.strip_prefix('+')?;
    let new_token = new_part.split(' ').next()?;
    let new = new_token.split(',').next()?.parse().ok()?;
    Some((old, new))
}

// --- the conflict parser --------------------------------------------------------

/// One side of a parsed conflict: the marker's label, when the line carried
/// one (`HEAD`, a branch name), and the side's lines.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct MergeSide {
    pub label: Option<String>,
    pub lines: Vec<String>,
}

/// One segment of a conflict fence: a run of context lines (numbered from
/// the fence's first line — the only anchor the shape has) or one complete
/// conflict's sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeSegment {
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

/// A fence is a git merge conflict when it carries both the opening and the
/// closing marker — the pair guards against a stray marker mis-routing a
/// real code block. Shared by the flat dispatch (which colors per line) and
/// the fence-shape classifier (which routes to the merge view).
pub fn is_conflict(code: &str) -> bool {
    let mut open = false;
    let mut close = false;
    for line in code.lines() {
        if line.starts_with("<<<<<<<") {
            open = true;
        } else if line.starts_with(">>>>>>>") {
            close = true;
        }
    }
    open && close
}

/// Parse complete merge-conflict markers into segments, or `None` when a
/// conflict never closes (the mid-stream shape) or none exists — the fence
/// stays whatever its fallback face is.
pub fn parse_conflict_segments(code: &str) -> Option<Vec<MergeSegment>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Stage {
        Outside,
        Ours,
        Base,
        Theirs,
    }

    let mut lines: Vec<&str> = code.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }

    let mut segments: Vec<MergeSegment> = Vec::new();
    let mut context: Vec<String> = Vec::new();
    let mut stage = Stage::Outside;
    let mut ours = MergeSide::default();
    let mut base = MergeSide::default();
    let mut theirs = MergeSide::default();
    let mut conflicts = 0usize;

    for (index, line) in lines.iter().enumerate() {
        let number = index + 1;
        match stage {
            Stage::Outside => {
                if let Some(label) = line.strip_prefix("<<<<<<<") {
                    if !context.is_empty() {
                        segments.push(MergeSegment::Context {
                            start: number - context.len(),
                            lines: std::mem::take(&mut context),
                        });
                    }
                    ours.label = non_empty_label(label);
                    ours.lines.clear();
                    stage = Stage::Ours;
                } else if line.starts_with("=======")
                    || line.starts_with(">>>>>>>")
                    || line.starts_with("|||||||")
                {
                    // A marker with no conflict open: the text is not a
                    // conflict fence this view can group.
                    return None;
                } else {
                    context.push((*line).to_owned());
                }
            }
            Stage::Ours => {
                if let Some(label) = line.strip_prefix("|||||||") {
                    base.label = non_empty_label(label);
                    stage = Stage::Base;
                } else if *line == "=======" {
                    stage = Stage::Theirs;
                } else {
                    ours.lines.push((*line).to_owned());
                }
            }
            Stage::Base => {
                if *line == "=======" {
                    stage = Stage::Theirs;
                } else {
                    base.lines.push((*line).to_owned());
                }
            }
            Stage::Theirs => {
                if let Some(label) = line.strip_prefix(">>>>>>>") {
                    theirs.label = non_empty_label(label);
                    segments.push(MergeSegment::Conflict {
                        ours: std::mem::take(&mut ours),
                        base: (base.label.is_some() || !base.lines.is_empty())
                            .then(|| std::mem::take(&mut base)),
                        theirs: std::mem::take(&mut theirs),
                    });
                    conflicts += 1;
                    stage = Stage::Outside;
                } else {
                    theirs.lines.push((*line).to_owned());
                }
            }
        }
    }
    if stage != Stage::Outside || conflicts == 0 {
        return None;
    }
    if !context.is_empty() {
        segments.push(MergeSegment::Context {
            start: lines.len() + 1 - context.len(),
            lines: context,
        });
    }
    Some(segments)
}

/// A marker's trailing label, when the line carried one past the markers.
fn non_empty_label(label: &str) -> Option<String> {
    let label = label.trim();
    (!label.is_empty()).then(|| label.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unified_diff_parses_into_sequential_numbered_rows() {
        let diff = concat!(
            "--- a/f\n",
            "+++ b/f\n",
            "@@ -1,3 +1,3 @@\n",
            " context\n",
            "-removed one\n",
            "-removed two\n",
            "+added\n",
            " more\n"
        );
        let parsed = parse_unified_diff(diff).expect("parses");
        assert!(parsed.well_formed);
        assert_eq!(parsed.parts.len(), 3, "two metadata rows and one hunk");
        let DiffPart::Hunk { header, rows } = &parsed.parts[2] else {
            panic!("a hunk");
        };
        assert!(header.starts_with("@@ -1,3"));
        assert_eq!(
            rows,
            &vec![
                CodeRow::context(1, "context"),
                CodeRow::removed(2, "removed one"),
                CodeRow::removed(3, "removed two"),
                CodeRow::added(2, "added"),
                CodeRow {
                    // The trailing context: the old side consumed two
                    // deletions more than the new side, so the numbers have
                    // drifted — old 4, new 3.
                    old: Some(4),
                    new: Some(3),
                    kind: CodeRowKind::Context,
                    text: SharedString::from("more"),
                },
            ],
            "deletions and additions flush in feed order with file numbers"
        );
    }

    #[test]
    fn junk_degrades_the_flag_not_the_parse() {
        let parsed = parse_unified_diff("@@ -1 +1 @@\n-a\nnot a diff line\n+b\n").expect("parses");
        assert!(!parsed.well_formed);
        let DiffPart::Hunk { rows, .. } = &parsed.parts[0] else {
            panic!("a hunk");
        };
        assert_eq!(rows[1], CodeRow::gutterless("not a diff line"));
    }

    #[test]
    fn count_less_hunk_headers_parse() {
        let parsed = parse_unified_diff("@@ -1 +1 @@\n-a\n+b\n").expect("parses");
        assert!(parsed.well_formed);
    }

    #[test]
    fn prose_is_not_a_diff() {
        assert!(parse_unified_diff("just some thoughts\nabout life").is_none());
    }

    #[test]
    fn a_three_way_conflict_parses_with_its_base() {
        let text = concat!(
            "before\n",
            "<<<<<<< HEAD\n",
            "ours\n",
            "||||||| merged common ancestors\n",
            "base\n",
            "=======\n",
            "theirs\n",
            ">>>>>>> feature\n"
        );
        let segments = parse_conflict_segments(text).expect("parses");
        assert_eq!(segments.len(), 2);
        let MergeSegment::Conflict { ours, base, theirs } = &segments[1] else {
            panic!("a conflict");
        };
        assert_eq!(ours.label.as_deref(), Some("HEAD"));
        assert_eq!(
            base.as_ref()
                .expect("three-way carries a base")
                .label
                .as_deref(),
            Some("merged common ancestors")
        );
        assert_eq!(theirs.label.as_deref(), Some("feature"));
    }

    #[test]
    fn conflict_needs_both_markers() {
        assert!(is_conflict("<<<<<<< ours\nx\n>>>>>>> theirs"));
        assert!(!is_conflict("<<<<<<< ours\nx"));
        assert!(!is_conflict("just a > quote"));
    }

    #[test]
    fn a_two_way_conflict_has_no_base_and_unterminated_declines() {
        let segments =
            parse_conflict_segments("<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\n").expect("parses");
        let MergeSegment::Conflict { base, .. } = &segments[0] else {
            panic!("a conflict");
        };
        assert!(base.is_none(), "no section, no column");
        assert!(parse_conflict_segments("<<<<<<< HEAD\na\n").is_none());
    }
}
