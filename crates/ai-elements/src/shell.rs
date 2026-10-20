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
//! The palette is the component's own and stays dark regardless of the app
//! theme: a terminal surface is read as a terminal by being dark. Colors are
//! GitHub-dark tones — muted enough to sit under a light theme without
//! shouting.

use std::time::Duration;

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    ElementId, HighlightStyle, Hsla, IntoElement, RenderOnce, Rgba, SharedString, div, prelude::*,
    px,
};
use gpui_base::TextView;
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use crate::code_view::{doc_style, fenced};
use crate::spinner::BrailleSpinner;
use crate::tool::format_tool_duration;

/// `gpui::rgb` is not `const`, so the palette spells its literals out.
const fn shell_rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

const BG: Rgba = shell_rgb(0x0f, 0x12, 0x16);
const FG: Rgba = shell_rgb(0xc9, 0xd1, 0xd9);
const DIM: Rgba = shell_rgb(0x8b, 0x94, 0x9e);
const PROMPT: Rgba = shell_rgb(0x3f, 0xb9, 0x50);
const PATH: Rgba = shell_rgb(0x79, 0xc0, 0xff);
const LINE_NO: Rgba = shell_rgb(0xd2, 0xa8, 0xff);
const HIT: Rgba = shell_rgb(0xe3, 0xb3, 0x41);
const GIT: Rgba = shell_rgb(0x7e, 0xe7, 0x87);
const ERR: Rgba = shell_rgb(0xf8, 0x51, 0x49);
const RULE: Rgba = Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.07,
};

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
    fn color(self) -> Rgba {
        match self {
            Self::Plain => FG,
            Self::Dim => DIM,
            Self::Path => PATH,
            Self::LineNo => LINE_NO,
            Self::Hit => HIT,
            Self::Git => GIT,
            Self::Error => ERR,
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
    fn compose(&self) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
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
                        color: Some(Hsla::from(kind.color())),
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
                color: Some(Hsla::from(PROMPT)),
                ..Default::default()
            },
        ));
        let cmd_at = text.len();
        text.push_str(&self.command);
        runs.push((
            cmd_at..cmd_at + self.command.len(),
            HighlightStyle {
                color: Some(Hsla::from(FG)),
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
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        let (doc, runs) = self.compose();
        let runs = Arc::new(runs);

        let duration = self.duration.map(format_tool_duration).unwrap_or_default();

        let footer = if self.running {
            h_flex()
                .gap_2()
                .child(BrailleSpinner::new().xsmall().color(Hsla::from(DIM)))
                .child(div().text_color(Hsla::from(DIM)).child("running"))
        } else {
            let (glyph, glyph_color, label_color) = match self.exit {
                Some(0) => ("✓", PROMPT, DIM),
                _ => ("✗", ERR, ERR),
            };
            let code = self
                .exit
                .map(|c| SharedString::from(format!("exit {c}")))
                .unwrap_or_default();
            h_flex()
                .gap_2()
                .child(div().text_color(Hsla::from(glyph_color)).child(glyph))
                .child(div().text_color(Hsla::from(label_color)).child(code))
                .when(!duration.is_empty(), |row| {
                    row.child(div().text_color(Hsla::from(DIM)).child(duration))
                })
        };

        // The shell's take on the shared skeleton: the dark surface is the
        // wrapper's paint, so the code background stays transparent and the
        // border matches it (no hairline inside the terminal).
        let style = doc_style(
            Hsla::from(FG),
            Hsla::from(DIM),
            gpui::transparent_black(),
            Hsla::from(BG),
            theme.selection,
            true,
        );

        // The dark surface is the wrapper's paint; the document itself is
        // transparent so the wrapper's rounding carries through.
        v_flex()
            .w_full()
            .min_w_0()
            .bg(Hsla::from(BG))
            .rounded(px(6.))
            .py_2()
            .child(
                TextView::markdown((self.id.clone(), "doc"), fenced(&doc))
                    .style(style)
                    .code_block_highlighter(move |_block| runs.as_ref().clone()),
            )
            .child(
                h_flex()
                    .w_full()
                    .mt_1()
                    .mx_3()
                    .pt_1p5()
                    .border_t_1()
                    .border_color(Hsla::from(RULE))
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
