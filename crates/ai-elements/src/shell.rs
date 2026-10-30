//! A fake terminal for command-shaped tool bodies: the host's prompt context
//! lines, a `❯` prompt line echoing the command, colored output, and an exit
//! footer.
//!
//! The prompt area is composed from the host's own shell configuration —
//! path, git branch and dirt, toolchain versions, whatever the user's zshrc
//! prints — passed in as classified span lines. The component has no opinion
//! about prompt formats; it renders what the host's environment says.
//!
//! The body is **in-band** through the rich-text core (the same machinery the
//! app's terminal panel renders through): context, prompt, and output are one
//! text document, so drag-select and copy are the window-level selection —
//! what you copy is exactly the session transcript you see. The footer is a
//! plain row below the document; the running spinner and the exit summary
//! are chrome, not transcript.
//!
//! `Bash`, `BashOutput`, and — as a deliberate rendering choice, not a claim
//! about what those tools are — `Grep` and `Glob` speak this surface: the
//! card header keeps the tool's true identity, the body shows the shell
//! equivalent of what it did. (The gallery demos an `Ls` card; the runtime
//! ships no such tool.)
//!
//! The palette rides the app theme — the shell reads as a transcript of the
//! tool's output, not as a dark terminal prop: same surface as every other
//! tool body, spans tinted by the shared accent hues (paths the info color,
//! git segments and the prompt glyph the success hue, errors the danger
//! hue).

use std::time::Duration;

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    ElementId, HighlightStyle, Hsla, IntoElement, RenderOnce, SharedString, div, prelude::*,
};
use gpui_base::TextView;
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use crate::code_view::{doc_style, fenced};
use crate::spinner::BrailleSpinner;
use crate::tool::format_tool_duration;

/// How a span of the transcript is tinted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellSpanKind {
    Plain,
    Dim,
    /// A file path (grep hits, `ls` names, error locations) or the prompt's
    /// working directory.
    Path,
    /// A line number within a located hit.
    LineNo,
    /// The matched substring of a hit, or the prompt's working-tree dirt
    /// markers.
    Hit,
    /// The prompt's git branch segment.
    Git,
    Error,
}

impl ShellSpanKind {
    /// The theme hue a span reads as, the same accents CodeView's tokenizer
    /// maps to: paths the info color, git segments the success hue, hits and
    /// dirt markers the warning hue, errors the danger color, everything
    /// else plain or muted.
    fn color(self, theme: &gpui_component::Theme) -> Hsla {
        match self {
            Self::Plain => theme.foreground,
            Self::Dim => theme.muted_foreground,
            Self::Path => theme.info,
            Self::LineNo => theme.muted_foreground,
            Self::Hit => theme.warning,
            Self::Git => theme.success,
            Self::Error => theme.danger,
        }
    }
}

/// One transcript line as classified spans; an empty span list renders a
/// blank line.
#[derive(Clone, Debug, Default)]
pub struct ShellLine {
    spans: Vec<(ShellSpanKind, SharedString)>,
}

impl ShellLine {
    pub fn spans(spans: Vec<(ShellSpanKind, SharedString)>) -> Self {
        Self { spans }
    }

    pub fn plain(text: impl Into<SharedString>) -> Self {
        Self::spans(vec![(ShellSpanKind::Plain, text.into())])
    }

    pub fn dim(text: impl Into<SharedString>) -> Self {
        Self::spans(vec![(ShellSpanKind::Dim, text.into())])
    }

    pub fn error(text: impl Into<SharedString>) -> Self {
        Self::spans(vec![(ShellSpanKind::Error, text.into())])
    }

    pub fn blank() -> Self {
        Self::default()
    }

    /// A located hit — `path:line:rest` — with the path and line number
    /// tinted, the separators dim.
    pub fn location(path: &str, line: u32, rest: &str) -> Self {
        Self::spans(vec![
            (ShellSpanKind::Path, path.into()),
            (ShellSpanKind::Dim, ":".into()),
            (ShellSpanKind::LineNo, line.to_string().into()),
            (ShellSpanKind::Dim, ":".into()),
            (ShellSpanKind::Plain, rest.into()),
        ])
    }

    /// [`ShellLine::location`] with every occurrence of `needle` inside
    /// `rest` raised to [`ShellSpanKind::Hit`] — the grep view. An empty
    /// needle matches everything, which would never advance the scan — it
    /// degrades to the located line instead.
    pub fn hit(path: &str, line: u32, rest: &str, needle: &str) -> Self {
        if needle.is_empty() {
            return Self::location(path, line, rest);
        }
        let mut spans = vec![
            (ShellSpanKind::Path, SharedString::from(path)),
            (ShellSpanKind::Dim, ":".into()),
            (ShellSpanKind::LineNo, SharedString::from(line.to_string())),
            (ShellSpanKind::Dim, ":".into()),
        ];
        let mut rest = rest;
        while let Some(ix) = rest.find(needle) {
            if ix > 0 {
                spans.push((ShellSpanKind::Plain, rest[..ix].into()));
            }
            spans.push((ShellSpanKind::Hit, needle.into()));
            rest = &rest[ix + needle.len()..];
        }
        if !rest.is_empty() {
            spans.push((ShellSpanKind::Plain, rest.into()));
        }
        Self::spans(spans)
    }
}

/// The fake terminal: prompt context, prompt + command, transcript, exit
/// footer.
#[derive(IntoElement)]
pub struct ShellView {
    id: ElementId,
    context: Vec<ShellLine>,
    command: SharedString,
    lines: Vec<ShellLine>,
    exit: Option<i32>,
    duration: Option<Duration>,
    running: bool,
}

impl ShellView {
    /// The shell the command ran in. The command is what the tool did, in
    /// shell vocabulary — the header above it keeps the tool's real name.
    pub fn new(id: impl Into<ElementId>, command: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            context: Vec::new(),
            command: command.into(),
            lines: Vec::new(),
            exit: None,
            duration: None,
            running: false,
        }
    }

    /// One prompt context line — the segments the user's shell prints above
    /// the prompt: working directory, git branch and dirt, toolchain
    /// versions. Pushed lines render in order, above the `❯` line.
    pub fn context(mut self, line: ShellLine) -> Self {
        self.context.push(line);
        self
    }

    /// Bulk plain output, split on newlines; blank lines are preserved.
    pub fn output(mut self, text: &str) -> Self {
        self.lines.extend(text.lines().map(ShellLine::plain));
        self
    }

    pub fn line(mut self, line: ShellLine) -> Self {
        self.lines.push(line);
        self
    }

    /// The process's exit code. `Some(0)` renders the success footer.
    pub fn exit(mut self, code: i32) -> Self {
        self.exit = Some(code);
        self
    }

    /// Wall time for the footer's tail.
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// The command has not settled: the footer shows the running spinner
    /// instead of an exit line.
    pub fn running(mut self) -> Self {
        self.running = true;
        self
    }

    /// The transcript document: context lines, the `❯` command line, a
    /// blank, then output — one fenced text with a run per span.
    fn compose(
        &self,
        theme: &gpui_component::Theme,
    ) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
        let mut text = String::new();
        let mut runs = Vec::new();
        let emit = |text: &mut String,
                    runs: &mut Vec<(Range<usize>, HighlightStyle)>,
                    line: &ShellLine| {
            for (kind, span) in &line.spans {
                let at = text.len();
                text.push_str(span);
                runs.push((
                    at..at + span.len(),
                    HighlightStyle {
                        color: Some(kind.color(theme)),
                        ..Default::default()
                    },
                ));
            }
            text.push('\n');
        };
        for line in &self.context {
            emit(&mut text, &mut runs, line);
        }
        // The prompt line: the green prompt glyph, then the command a weight
        // heavier than output.
        let prompt_at = text.len();
        text.push_str("❯ ");
        runs.push((
            prompt_at..prompt_at + "❯".len(),
            HighlightStyle {
                color: Some(theme.success),
                ..Default::default()
            },
        ));
        let cmd_at = text.len();
        text.push_str(&self.command);
        runs.push((
            cmd_at..cmd_at + self.command.len(),
            HighlightStyle {
                color: Some(theme.foreground),
                font_weight: Some(gpui::FontWeight::MEDIUM),
                ..Default::default()
            },
        ));
        text.push('\n');
        if !self.lines.is_empty() {
            text.push('\n');
        }
        for line in &self.lines {
            emit(&mut text, &mut runs, line);
        }
        (text, runs)
    }
}

impl RenderOnce for ShellView {
    fn render(self, _window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let (doc, runs) = self.compose(&theme);
        let runs = Arc::new(runs);
        // The highlighter Arc is cached per (view id, text) exactly like
        // CodeView's — a fresh closure per frame arms upstream's
        // selection-preserving reflow every frame and the body never holds
        // a selection.

        let duration = self.duration.map(format_tool_duration).unwrap_or_default();

        let footer = if self.running {
            h_flex()
                .gap_2()
                .child(BrailleSpinner::new().xsmall().color(theme.muted_foreground))
                .child(div().text_color(theme.muted_foreground).child("running"))
        } else {
            let (glyph, glyph_color, label_color) = match self.exit {
                Some(0) => ("✓", theme.success, theme.muted_foreground),
                _ => ("✗", theme.danger, theme.danger),
            };
            let code = self
                .exit
                .map(|c| SharedString::from(format!("exit {c}")))
                .unwrap_or_default();
            h_flex()
                .gap_2()
                .child(div().text_color(glyph_color).child(glyph))
                .child(div().text_color(label_color).child(code))
                .when(!duration.is_empty(), |row| {
                    row.child(div().text_color(theme.muted_foreground).child(duration))
                })
        };

        // The shell's take on the shared skeleton: the surface is the card's
        // own — transparent body, no dark prop — so the document paints with
        // the theme palette on the same ground as every other tool body.
        let style = doc_style(
            theme.foreground,
            theme.muted_foreground,
            gpui::transparent_black(),
            gpui::transparent_black(),
            theme.selection,
            theme.is_dark(),
        );

        let doc_id = gpui::ElementId::Name(SharedString::from(format!("{:?}-doc", self.id)));
        v_flex()
            .w_full()
            .min_w_0()
            .py_2()
            .child(
                TextView::markdown(doc_id.clone(), fenced(&doc))
                    .style(style)
                    .shared_code_block_highlighter(crate::code_view::highlighter_for(
                        &doc_id,
                        &doc,
                        crate::code_view::theme_stamp(&theme),
                        Arc::clone(&runs),
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .mt_1()
                    .mx_3()
                    .pt_1p5()
                    .border_t_1()
                    .border_color(theme.border)
                    .font_family(mono)
                    .text_xs()
                    .child(footer),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty needle must degrade, not loop: `find("")` is always
    /// `Some(0)`, so the unguarded scan would neither advance nor exit.
    #[test]
    fn hit_with_empty_needle_degrades_to_location() {
        let hit = ShellLine::hit("src/lib.rs", 7, "let x = 1;", "");
        let located = ShellLine::location("src/lib.rs", 7, "let x = 1;");
        assert_eq!(hit.spans, located.spans);
    }

    #[test]
    fn hit_raises_every_occurrence() {
        let line = ShellLine::hit("a.rs", 1, "ab ab", "ab");
        // path + ':' + lineno + ':' then hit, gap, hit.
        assert_eq!(line.spans.len(), 7);
    }
}
