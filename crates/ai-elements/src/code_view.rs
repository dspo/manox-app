//! A text-editor mock for file-shaped tool bodies: a line-number gutter, a
//! diff marker column, and naive syntax coloring — all rendered **in-band**
//! through the rich-text core, exactly the way the app's terminal panel
//! numbers its lines.
//!
//! In-band is the load-bearing choice: the gutter numbers, the `+`/`-`
//! markers, and the code are one text document, so drag-select and copy are
//! the window-level selection every other markdown surface uses — what you
//! copy is literally what you see, gutter numbers included. The alternative
//! (custom row divs beside a text column) would need per-row alignment against
//! the text engine and would still have no selection.
//!
//! This is a *mock editor* — presentational, no editing. The tokenizer is a
//! line scanner good enough for Rust and C-like syntax; anything it does not
//! recognize renders plain, which is the correct degradation for a preview.
//!
//! Three shapes cover the file tools:
//!
//! - `Read`: [`CodeView::source`] numbers the whole window from 1;
//!   a ranged read builds its [`CodeLine`]s with the file's own numbers and
//!   a [`CodeLine::gap`] between disjoint windows.
//! - `Write`: the same sequential shape — the content is new, but the file
//!   numbers read from 1.
//! - `Edit`: mixed [`CodeLine::removed`] / [`CodeLine::added`] lines. When
//!   any line carries an old number the gutter widens to the old/new pair a
//!   unified diff shows; removed/added lines carry their tint as a run
//!   background — the one surface tint the editor allows itself.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    ElementId, HighlightStyle, Hsla, IntoElement, RenderOnce, SharedString, StyleRefinement, px,
    rems,
};
use gpui_base::{TextView, TextViewStyle};
use gpui_component::{ActiveTheme as _, Theme};

/// A line's role in the editor surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeLineKind {
    Context,
    Added,
    Removed,
    /// The skipped span between disjoint read windows. Carries no numbers.
    Gap,
}

/// One rendered line: its gutter number(s), its role, and its text.
///
/// `old` is the pre-image number a removed/context diff line carries; `new`
/// is the number the file shows after the edit. Plain file reads use `new`
/// alone.
#[derive(Clone, Debug)]
pub struct CodeLine {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub kind: CodeLineKind,
    pub text: SharedString,
}

impl CodeLine {
    pub fn context(number: u32, text: impl Into<SharedString>) -> Self {
        Self {
            old: Some(number),
            new: Some(number),
            kind: CodeLineKind::Context,
            text: text.into(),
        }
    }

    /// A context line without diff numbers — the plain-read shape.
    pub fn plain(number: u32, text: impl Into<SharedString>) -> Self {
        Self {
            old: None,
            new: Some(number),
            kind: CodeLineKind::Context,
            text: text.into(),
        }
    }

    pub fn added(number: u32, text: impl Into<SharedString>) -> Self {
        Self {
            old: None,
            new: Some(number),
            kind: CodeLineKind::Added,
            text: text.into(),
        }
    }

    pub fn removed(number: u32, text: impl Into<SharedString>) -> Self {
        Self {
            old: Some(number),
            new: None,
            kind: CodeLineKind::Removed,
            text: text.into(),
        }
    }

    pub fn gap() -> Self {
        Self {
            old: None,
            new: None,
            kind: CodeLineKind::Gap,
            text: SharedString::default(),
        }
    }
}

/// Default soft-wrap budget in mono columns. The rich-text core wraps a code
/// block at its own left edge — under the gutter, which no editor does — so
/// the view wraps at compose time and hangs continuations under the code
/// column. A host that knows its real column width overrides with
/// [`CodeView::wrap_at`].
pub const DEFAULT_WRAP_COLUMNS: usize = 80;

/// The mock editor surface: gutters, markers, colored code, diff tints.
#[derive(IntoElement)]
pub struct CodeView {
    id: ElementId,
    lines: Vec<CodeLine>,
    wrap: usize,
}

impl CodeView {
    /// A window of lines, gutter numbers and roles exactly as passed.
    pub fn new(id: impl Into<ElementId>, lines: Vec<CodeLine>) -> Self {
        Self {
            id: id.into(),
            lines,
            wrap: DEFAULT_WRAP_COLUMNS,
        }
    }

    /// A whole-file read: text split into lines, numbered from 1.
    pub fn source(id: impl Into<ElementId>, text: &str) -> Self {
        Self {
            id: id.into(),
            lines: text
                .lines()
                .enumerate()
                .map(|(ix, line)| CodeLine::plain(ix as u32 + 1, line))
                .collect(),
            wrap: DEFAULT_WRAP_COLUMNS,
        }
    }

    /// The soft-wrap budget in mono columns.
    pub fn wrap_at(mut self, columns: usize) -> Self {
        self.wrap = columns;
        self
    }
}

/// What the tokenizer colors a span as. The palette mapping lives in
/// [`tok_style`]: keywords take the accent, types the info hue, strings
/// danger, numbers warning, comments the muted foreground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    Plain,
    Keyword,
    Type,
    Str,
    Num,
    Comment,
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Split one line into colored spans. Line-local by design: block comments
/// and multi-line strings degrade to plain text, which keeps the scanner
/// honest about being a mock. The spans cover the line contiguously, in
/// order — the offset walk in the render relies on exactly that.
fn tokenize(line: &str) -> Vec<(Tok, String)> {
    let mut spans: Vec<(Tok, String)> = Vec::new();
    let mut rest = line;

    let push = |spans: &mut Vec<(Tok, String)>, tok: Tok, text: String| {
        if text.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some((last, buffer)) if *last == tok => buffer.push_str(&text),
            _ => spans.push((tok, text)),
        }
    };

    while !rest.is_empty() {
        let first = rest.chars().next().expect("non-empty");
        // Line comment: to end of line.
        if rest.starts_with("//") {
            push(&mut spans, Tok::Comment, rest.to_string());
            break;
        }
        // Attribute: `#[...]` through the matching bracket (no nesting —
        // a derive list never nests).
        if rest.starts_with("#[") {
            let end = rest.find(']').map(|ix| ix + 1).unwrap_or(rest.len());
            push(&mut spans, Tok::Type, rest[..end].to_string());
            rest = &rest[end..];
            continue;
        }
        if first == '"' {
            let (span, next) = scan_quoted(rest, '"');
            push(&mut spans, Tok::Str, span);
            rest = next;
            continue;
        }
        if first == '\'' {
            // A char literal closes on the same line; a lifetime is an ident
            // that starts with the quote.
            let chars: Vec<char> = rest.chars().collect();
            let closes = chars.len() >= 3 && chars[2] == '\'' && chars[1] != '\\';
            let escaped = chars.len() >= 4 && chars[3] == '\'' && chars[1] == '\\';
            if closes || escaped {
                let len = if escaped { 4 } else { 3 };
                let span: String = chars[..len].iter().collect();
                push(&mut spans, Tok::Str, span.clone());
                rest = &rest[span.len()..];
            } else {
                let ident: String = rest
                    .chars()
                    .take(
                        1 + rest[1..]
                            .chars()
                            .take_while(|c| is_ident_continue(*c))
                            .count(),
                    )
                    .collect();
                push(&mut spans, Tok::Type, ident.clone());
                rest = &rest[ident.len()..];
            }
            continue;
        }
        if first.is_ascii_digit() {
            let number: String = rest
                .chars()
                .take_while(|c| c.is_ascii_hexdigit() || matches!(c, 'x' | 'X' | '_' | '.'))
                .collect();
            // Numeric suffixes (u32, f64, usize) belong to the literal.
            let suffix: String = rest[number.len()..]
                .chars()
                .take_while(|c| is_ident_continue(*c))
                .collect();
            push(&mut spans, Tok::Num, format!("{number}{suffix}"));
            rest = &rest[number.len() + suffix.len()..];
            continue;
        }
        if is_ident_start(first) {
            let ident: String = rest.chars().take_while(|c| is_ident_continue(*c)).collect();
            rest = &rest[ident.len()..];
            // A macro invocation reads as a keyword-grade highlight.
            let macro_call = rest.starts_with('!');
            let tok = if KEYWORDS.contains(&ident.as_str()) || macro_call {
                Tok::Keyword
            } else if first.is_uppercase() {
                Tok::Type
            } else {
                Tok::Plain
            };
            if macro_call {
                push(&mut spans, tok, format!("{ident}!"));
                rest = &rest[1..];
            } else {
                push(&mut spans, tok, ident);
            }
            continue;
        }
        push(&mut spans, Tok::Plain, first.to_string());
        rest = &rest[first.len_utf8()..];
    }
    spans
}

/// Scan a quoted span from `rest`, honoring backslash escapes; returns the
/// span including both quotes (or through end of line if unterminated) and
/// the remainder.
fn scan_quoted(rest: &str, quote: char) -> (String, &str) {
    let mut escaped = false;
    for (ix, c) in rest.char_indices().skip(1) {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            return (
                rest[..ix + c.len_utf8()].to_string(),
                &rest[ix + c.len_utf8()..],
            );
        }
    }
    (rest.to_string(), "")
}

/// The shared document skeleton for the in-band bodies: mono code blocks at
/// preview density, a tight paragraph gap, the code surface and palette left
/// to the caller. One home — [`crate::ShellView`](crate::shell) composes
/// through it too.
pub(crate) fn doc_style(
    foreground: Hsla,
    muted: Hsla,
    code_background: Hsla,
    border: Hsla,
    selection: Hsla,
    dark: bool,
) -> TextViewStyle {
    TextViewStyle::default()
        .with_foreground(foreground)
        .with_muted_foreground(muted)
        .with_code_background(code_background)
        .with_border(border)
        .with_selection(selection)
        .with_paragraph_gap(rems(0.25))
        .with_dark(dark)
        .with_code_block({
            let mut r = StyleRefinement::default();
            r.text.font_size = Some(px(12.).into());
            r
        })
}

/// The editor's take on the skeleton: a transparent code surface — the card
/// frame provides the chrome, and the only tint the editor allows itself is
/// the diff run background.
fn code_style(theme: &Theme) -> TextViewStyle {
    doc_style(
        theme.foreground,
        theme.muted_foreground,
        gpui::transparent_black(),
        theme.border,
        theme.selection,
        theme.is_dark(),
    )
}

/// A number right-aligned to `width` mono cells, or that many blanks.
fn gutter_cell(number: Option<u32>, width: usize) -> String {
    match number {
        Some(n) => format!("{n:>width$}"),
        None => " ".repeat(width),
    }
}

/// The palette the composer needs from the theme.
struct Palette {
    foreground: Hsla,
    muted: Hsla,
    accent: Hsla,
    info: Hsla,
    danger: Hsla,
    warning: Hsla,
    success: Hsla,
}

impl Palette {
    fn from_theme(theme: &gpui_component::Theme) -> Self {
        Self {
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            accent: theme.accent,
            info: theme.info,
            danger: theme.danger,
            warning: theme.warning,
            success: theme.success,
        }
    }
}

fn tok_color(tok: Tok, p: &Palette) -> Hsla {
    match tok {
        Tok::Plain => p.foreground,
        Tok::Keyword => p.accent,
        Tok::Type => p.info,
        Tok::Str => p.danger,
        Tok::Num => p.warning,
        Tok::Comment => p.muted,
    }
}

impl CodeView {
    /// Compose the document: one text plus disjoint ascending runs. Pure —
    /// the invariants the rich-text core demands are checkable in a unit
    /// test without a window.
    fn compose(&self, p: &Palette) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
        let dual = self.lines.iter().any(|line| line.old.is_some());
        let width = self
            .lines
            .iter()
            .filter_map(|line| line.old.or(line.new))
            .map(|n| n.to_string().len())
            .max()
            .unwrap_or(1);
        let indent = " ".repeat(width + usize::from(dual) * (width + 1));

        let mut text = String::new();
        let mut runs: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        let muted = HighlightStyle {
            color: Some(p.muted),
            ..Default::default()
        };
        for line in &self.lines {
            if line.kind == CodeLineKind::Gap {
                let start = text.len() + indent.len() + 3;
                text.push_str(&indent);
                text.push_str("   ...\n");
                runs.push((start..start + 3, muted));
                continue;
            }
            let (marker, marker_color) = match line.kind {
                CodeLineKind::Added => ("+", p.success),
                CodeLineKind::Removed => ("-", p.danger),
                _ => (" ", p.muted),
            };
            let tint = match line.kind {
                CodeLineKind::Added => Some(p.success.opacity(0.08)),
                CodeLineKind::Removed => Some(p.danger.opacity(0.08)),
                _ => None,
            };

            let line_start = text.len();
            let first_cell = if dual {
                gutter_cell(line.old, width)
            } else {
                gutter_cell(line.new, width)
            };
            text.push_str(&first_cell);
            let old_end = text.len();
            if dual {
                text.push(' ');
                text.push_str(&gutter_cell(line.new, width));
            }
            let new_end = text.len();
            text.push(' ');
            let marker_at = text.len();
            text.push_str(marker);
            text.push(' ');
            let code_start = text.len();

            let chunks: Vec<&str> = if line.text.chars().count() > self.wrap {
                let bounds: Vec<(usize, usize)> = line
                    .text
                    .char_indices()
                    .map(|(start, ch)| (start, start + ch.len_utf8()))
                    .collect();
                bounds
                    .chunks(self.wrap)
                    .map(|bounds| {
                        let start = bounds[0].0;
                        let end = bounds[bounds.len() - 1].1;
                        &line.text[start..end]
                    })
                    .collect()
            } else {
                vec![line.text.as_ref()]
            };

            runs.push((line_start..old_end, muted));
            if dual {
                runs.push((old_end + 1..new_end, muted));
            }
            runs.push((
                marker_at..marker_at + 1,
                HighlightStyle {
                    color: Some(marker_color),
                    background_color: tint,
                    ..Default::default()
                },
            ));
            let mut cursor = code_start;
            for (cix, chunk) in chunks.iter().enumerate() {
                if cix > 0 {
                    // Blank gutter cell(s), blank marker, blank separator —
                    // the first row's prefix is cell(s) + ' ' + marker + ' ',
                    // so the continuation repeats exactly that width.
                    text.push('\n');
                    text.push_str(&indent);
                    text.push_str("   ");
                    cursor = text.len();
                }
                text.push_str(chunk);
                for (tok, span) in tokenize(chunk) {
                    let style = match line.kind {
                        CodeLineKind::Added => HighlightStyle {
                            color: Some(p.success),
                            background_color: tint,
                            ..Default::default()
                        },
                        CodeLineKind::Removed => HighlightStyle {
                            color: Some(p.danger),
                            background_color: tint,
                            ..Default::default()
                        },
                        _ => HighlightStyle {
                            color: Some(tok_color(tok, p)),
                            font_style: (tok == Tok::Comment).then_some(gpui::FontStyle::Italic),
                            background_color: tint,
                            ..Default::default()
                        },
                    };
                    runs.push((cursor..cursor + span.len(), style));
                    cursor += span.len();
                }
            }
            text.push('\n');
        }
        (text, runs)
    }
}

impl RenderOnce for CodeView {
    fn render(self, _window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let theme = cx.theme();
        let (text, runs) = self.compose(&Palette::from_theme(theme));

        // One Arc for every frame: the view is rebuilt per render, and a
        // code block reuses its highlights only while the highlighter is the
        // same Arc — a fresh closure would re-highlight every frame.
        let runs = Arc::new(runs);
        TextView::markdown(self.id, fenced(&text))
            .style(code_style(theme))
            .code_block_highlighter(move |_block| runs.as_ref().clone())
    }
}

/// Fence `text` as one `ansi` code block — the same shape the app's terminal
/// panel composes. The fence is one longer than the longest backtick run
/// inside so no content line can close it early. The one home for this shape
/// in the crate: [`crate::ShellView`] composes through it too.
pub(crate) fn fenced(text: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for b in text.bytes() {
        if b == b'`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat((longest + 1).max(3));
    let mut source = String::with_capacity(text.len() + fence.len() * 2 + 8);
    source.push_str(&fence);
    source.push_str("ansi\n");
    source.push_str(text);
    if !text.ends_with('\n') {
        source.push('\n');
    }
    source.push_str(&fence);
    source
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        Palette {
            foreground: Hsla::default(),
            muted: Hsla::default(),
            accent: Hsla::default(),
            info: Hsla::default(),
            danger: Hsla::default(),
            warning: Hsla::default(),
            success: Hsla::default(),
        }
    }

    /// The rich-text core demands: runs disjoint, ascending, inside the text,
    /// and never crossing a line boundary.
    #[test]
    fn compose_runs_respect_line_boundaries() {
        let view = CodeView::new(
            "t",
            vec![
                CodeLine::plain(138, "fn a<'b>(c: &'b str) -> u32 { 0 }"),
                CodeLine::gap(),
                CodeLine::plain(168, "let s = \"long\"; // comment tail"),
            ],
        );
        let (text, runs) = view.compose(&palette());
        let mut prev_end = 0usize;
        for (range, _) in &runs {
            assert!(range.start >= prev_end, "runs overlap at {}", range.start);
            assert!(range.end <= text.len(), "run past text end: {range:?}");
            let slice = &text[range.clone()];
            assert!(
                !slice.contains('\n'),
                "run crosses a line boundary: {slice:?}"
            );
            prev_end = range.end;
        }
    }

    /// A wrapped line hangs its continuations under the code column with the
    /// same invariant.
    #[test]
    fn compose_wrapped_lines_keep_runs_within_lines() {
        let long: String = "pub fn long_line(&self) -> Option<u32> { ".repeat(6);
        let view = CodeView::new("t", vec![CodeLine::plain(3, long)]);
        let (text, runs) = view.compose(&palette());
        assert!(text.lines().count() > 1, "the line must wrap");
        let mut prev_end = 0usize;
        for (range, _) in &runs {
            assert!(range.start >= prev_end, "runs overlap at {}", range.start);
            let slice = &text[range.clone()];
            assert!(
                !slice.contains('\n'),
                "run crosses a line boundary: {slice:?}"
            );
            prev_end = range.end;
        }
    }

    /// The gutter keeps its column: every visual row of one source line
    /// starts with the same-width prefix.
    #[test]
    fn compose_continuation_prefix_matches_first_row() {
        let long: String = "x".repeat(200);
        let view = CodeView::new("t", vec![CodeLine::plain(1, long)]);
        let (text, _) = view.compose(&palette());
        let mut rows = text.lines();
        let first = rows.next().unwrap();
        let second = rows.next().unwrap();
        // The code character's column: same on the first row and every
        // continuation — the hanging indent.
        let col = |row: &str| row.find('x').expect("code present");
        assert_eq!(
            col(first),
            col(second),
            "continuation must hang under the code column"
        );
    }
}
