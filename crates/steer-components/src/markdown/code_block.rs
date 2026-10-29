//! Editor-style fenced code blocks: every fence in a prose document renders
//! as a code editor panel — a header bar (language + copy control), a
//! line-number gutter, and a non-wrapping, syntax-highlighted code area that
//! scrolls horizontally, with a height cap and vertical scrollbar for long
//! blocks. This is the transcript's code surface; the fences a document
//! carries are intercepted at parse time, after the diagram and math fences
//! have taken theirs.
//!
//! The block rides the upstream `MarkdownExtensions` plugin point — the
//! same registration path the svg/mermaid/math fences use — so streaming,
//! incremental appends, and whole-document copy keep working unchanged: the
//! block's text form is its full Markdown source (delimiters and info string
//! included), which is what a drag selection crossing the block copies and
//! what `ChatDocument`'s round trip preserves.
//!
//! What is deliberately given up: per-glyph selection inside the code area.
//! A custom block's text is not a TextView selection leaf; the drag that
//! crosses the block selects it whole (its fence source), and the copy
//! control copies the raw code — the same trade the svg raster face makes for
//! an image, with the copy button as the primary path either way.
//!
//! The copy control is self-contained: its "copied" beat lives in the
//! window's keyed state, so any surface — chat items, static chrome, gallery
//! — gets a working copy button without threading a feedback host through
//! the markdown assembly. The visuals match [`crate::copy_feedback`]'s
//! controls (ghost icon button, check flip, same beat length).
//!
//! Two fence shapes get a second face beyond the flat code panel, both
//! decided at parse time and toggleable from the header (the state rides the
//! window's keyed store, the svg block's face-flip pattern):
//!
//! - a `diff` fence that parses as a unified diff renders side by side by
//!   default — old column left, new column right, rows paired per hunk with
//!   the shorter run padded — with the toggle returning to the flat panel;
//! - a fence carrying complete merge-conflict markers renders as the merge
//!   view — Ours | Base | Theirs columns per conflict (Base only when the
//!   `|||||||` section is present), context lines full width between
//!   conflicts.
//!
//! A fence that fails its shape's parse — a malformed hunk header, a
//! conflict missing its closing marker (the mid-stream shape) — stays the
//! flat panel with the line-color dispatch, so nothing degrades noisily.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Axis, ClipboardItem, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px, relative,
};
use gpui_base::text::{MarkdownNode, MarkdownParseContext, markdown_ast};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::{ScrollableMask, Scrollbar};
use gpui_component::{Sizable as _, Theme, h_flex};
use gpui_kit_assets::IconName;

use crate::copy_feedback::COPY_FEEDBACK;
use crate::markdown::diff_view;
use crate::markdown::svg_block::block_key;
use crate::markdown::text_view::{DispatchPalette, dispatch_runs, is_conflict};

/// Custom block name for one editor-style fence. Renderers are registered by
/// name, so this string is the whole contract between the parser and the
/// renderer halves.
pub(crate) const BLOCK_NAME: &str = "code";

/// How tall a block's body may paint before it scrolls internally. The
/// terminal panel paginates at a similar line budget; an editor panel scrolls
/// instead, because a code block reads as one artifact.
const MAX_BODY_HEIGHT: Pixels = px(420.);

/// The body's vertical padding, shared by the gutter and the code area so
/// their baselines align line for line.
pub(crate) const BODY_PAD_Y: f32 = 12.;

/// One intercepted fence's payload: the raw code (what copy takes), the info
/// string's language token (verbatim — a protocol value, never localized),
/// and the shape parsed out of the text (what the second face renders from).
#[derive(Clone)]
pub(crate) struct EditorPayload {
    pub(crate) code: SharedString,
    lang: Option<SharedString>,
    pub(crate) shape: FenceShape,
}

/// What the fence's text parses into. Computed once at parse time — the
/// render reads the shape and never re-parses.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum FenceShape {
    /// No second face: the flat highlighted panel is the whole story.
    Code,
    /// A unified diff: hunks of paired rows for the side-by-side view.
    Diff(Vec<diff_view::DiffPart>),
    /// Complete merge-conflict markers: column groups for the merge view.
    Merge(Vec<diff_view::MergeSegment>),
}

/// Turn a fenced code node into an editor block. Every fence becomes one —
/// language or not, known or not — so the transcript's code keeps one face.
/// Registered after the diagram and math parsers, which keep the fences they
/// own.
pub(crate) fn code_fence(
    node: &markdown_ast::Node,
    cx: &MarkdownParseContext<'_>,
) -> Option<MarkdownNode> {
    let markdown_ast::Node::Code(code) = node else {
        return None;
    };
    let fence = cx.node_source(node).unwrap_or(&code.value);
    code_block_node(&code.value, code.lang.as_deref(), fence)
}

/// The editor block for one fenced code node. `fence` is the block's text
/// form — the whole fence, delimiters and info string included — so copy and
/// Markdown round-trip reproduce the source.
fn code_block_node(body: &str, lang: Option<&str>, fence: &str) -> Option<MarkdownNode> {
    // The classification order matches the flat dispatch: conflict markers
    // route before the `diff` language token, because a conflicted file
    // pasted into a diff fence is a merge view, not a patch.
    let shape = if is_conflict(body) {
        match parse_conflict_segments(body) {
            Some(segments) => FenceShape::Merge(segments),
            None => FenceShape::Code,
        }
    } else if lang.is_some_and(|l| l.eq_ignore_ascii_case("diff")) {
        match parse_unified_diff(body) {
            Some(parts) => FenceShape::Diff(parts),
            None => FenceShape::Code,
        }
    } else {
        FenceShape::Code
    };
    Some(
        MarkdownNode::new(
            BLOCK_NAME,
            Arc::new(EditorPayload {
                code: body.to_owned().into(),
                lang: lang.map(SharedString::from),
                shape,
            }),
        )
        .text(fence)
        .markdown(fence),
    )
}

/// Which face a shaped block shows. The flat panel is one face of every
/// shape; the second faces live in [`diff_view`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockView {
    Flat,
    Split,
    Merge,
}

/// The editor panel: header bar over a body the shape and view decide. The
/// code always reads at the mono size the rest of the app's code surfaces
/// use, so a block is the same artifact wherever it lands.
pub(crate) fn code_element(node: &MarkdownNode, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(payload) = node.data::<Arc<EditorPayload>>() else {
        return div().child(node.as_text().to_string()).into_any_element();
    };
    // Cloned up front: the render borrows `cx` again below (keyed state, the
    // copy control), and the held borrow would collide with them.
    let theme = Theme::global(cx).clone();
    let key = block_key(node);

    // Fenced values can carry trailing blank lines; the gutter counts and the
    // painted code agree on the trimmed form, and copy takes it too.
    let code: SharedString = payload.code.trim_end_matches('\n').to_owned().into();

    // The view starts at the shape's second face when there is one (that is
    // the face the fence was written for) and never drifts from the reader's
    // toggle: the state lives in the window's keyed store per block.
    let view = window.use_keyed_state(("code-view", key), cx, |_, _| match &payload.shape {
        FenceShape::Diff(_) => BlockView::Split,
        FenceShape::Merge(_) => BlockView::Merge,
        FenceShape::Code => BlockView::Flat,
    });
    let showing = *view.read(cx);

    let header = h_flex()
        .items_center()
        .w_full()
        .min_w_0()
        .px_3()
        .gap_2()
        .h(px(30.))
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.4))
        .children(
            payload
                .lang
                .as_ref()
                .filter(|lang| !lang.is_empty())
                .map(|lang| {
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .min_w_0()
                        .overflow_hidden()
                        .child(lang.clone())
                }),
        )
        // The spacer keeps the controls at the trailing edge on blocks with
        // nothing to show on the left.
        .child(div().flex_1())
        .children(
            (payload.shape != FenceShape::Code)
                .then(|| view_toggle(key, &payload.shape, showing, view.clone())),
        )
        .child(copy_control(key, code.to_string(), window, cx));

    // The body scrolls vertically as one region — whichever face it carries —
    // and a tall block grows a scrollbar instead of pushing the transcript
    // apart. Horizontal scrolling belongs to the face.
    let scroll = window
        .use_keyed_state(("code-scroll", key), cx, |_, _| ScrollHandle::default())
        .read(cx)
        .clone();
    let palette = DispatchPalette::new(&theme);
    let body = match (&payload.shape, showing) {
        (FenceShape::Diff(parts), BlockView::Split) => diff_view::split_body(parts, key, &theme),
        (FenceShape::Merge(segments), BlockView::Merge) => {
            diff_view::merge_body(segments, key, &theme)
        }
        _ => flat_body(&code, payload.lang.as_deref(), &palette, &theme, key),
    };

    div()
        .id(("code-block", key))
        .w_full()
        .min_w_0()
        .relative()
        .overflow_hidden()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .child(header)
        .child(
            div()
                .id(("code-body", key))
                .relative()
                .w_full()
                .min_w_0()
                .max_h(MAX_BODY_HEIGHT)
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .child(body)
                .child(ScrollableMask::new(Axis::Vertical, &scroll).id(("code-mask", key)))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(Scrollbar::vertical(&scroll)),
                ),
        )
        .into_any_element()
}

/// The flat face: line-number gutter over the dispatch-highlighted code run,
/// sharing the window-level metrics every other code surface paints with.
fn flat_body(
    code: &str,
    lang: Option<&str>,
    palette: &DispatchPalette,
    theme: &Theme,
    key: u64,
) -> AnyElement {
    let line_count = code.split('\n').count().max(1);
    let num_width = line_count.to_string().len();
    let gutter: SharedString = (1..=line_count)
        .map(|n| format!("{n:>num_width$}"))
        .collect::<Vec<_>>()
        .join("\n")
        .into();
    let runs = dispatch_runs(code, lang, palette);

    h_flex()
        .w_full()
        .min_w_0()
        .items_start()
        .font_family(theme.mono_font_family.clone())
        .text_size(theme.mono_font_size)
        .line_height(relative(1.6))
        .whitespace_nowrap()
        .child(
            div()
                .py(px(BODY_PAD_Y))
                .px_2()
                .flex_shrink_0()
                .text_color(theme.muted_foreground.opacity(0.7))
                .child(gutter),
        )
        .child(
            div()
                .id(("code-x", key))
                .flex_1()
                .min_w_0()
                .py(px(BODY_PAD_Y))
                .px_3()
                .overflow_x_scroll()
                .child(
                    gpui::StyledText::new(code.to_owned())
                        .with_highlights(runs)
                        .into_any_element(),
                ),
        )
        .into_any_element()
}

/// The header's face toggle: the icon names the face a click shows, the same
/// reading the svg block's flip uses. State rides the keyed store, so the
/// reader's choice survives every frame the document re-renders.
fn view_toggle(
    key: u64,
    shape: &FenceShape,
    showing: BlockView,
    view: gpui::Entity<BlockView>,
) -> AnyElement {
    let (icon, target) = match (shape, showing) {
        (FenceShape::Diff(_), BlockView::Split) => (IconName::Rows2, BlockView::Flat),
        (FenceShape::Diff(_), _) => (IconName::Columns2, BlockView::Split),
        (FenceShape::Merge(_), BlockView::Merge) => (IconName::Rows2, BlockView::Flat),
        (FenceShape::Merge(_), _) => (IconName::GitMerge, BlockView::Merge),
        (FenceShape::Code, _) => (IconName::Code, BlockView::Flat),
    };
    Button::new(("code-view-toggle", key))
        .ghost()
        .xsmall()
        .icon(icon)
        .on_click(move |_, _, cx: &mut App| {
            view.update(cx, |current, cx| {
                *current = target;
                cx.notify();
            });
        })
        .into_any_element()
}

/// The header's copy control: writes the raw code to the clipboard and flips
/// to a check for one feedback beat. The state lives in the window's keyed
/// store, so the flip survives every frame the document re-renders without a
/// feedback host.
fn copy_control(key: u64, text: String, window: &mut Window, cx: &mut App) -> AnyElement {
    let id: ElementId = ("code-copy", key).into();
    let copied = window.use_keyed_state(("code-copied", key), cx, |_, _| false);
    let active = *copied.read(cx);
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(if active {
            IconName::Check
        } else {
            IconName::Copy
        })
        .on_click(move |_, _, cx: &mut App| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            copied.update(cx, |flag, cx| {
                *flag = true;
                cx.notify();
                cx.spawn(async move |flag, cx| {
                    cx.background_executor().timer(COPY_FEEDBACK).await;
                    let _ = flag.update(cx, |flag, cx| {
                        *flag = false;
                        cx.notify();
                    });
                })
                .detach();
            });
        })
        .into_any_element()
}

// --- fence shape parsers ---------------------------------------------------------

/// Parse a unified diff into the side-by-side view's parts, or `None` when the
/// text is not a diff this view can carry — a line with no legal prefix inside
/// a hunk, a hunk header missing its numbers (the mid-stream shape) — leaving
/// the fence the flat panel. `\ No newline at end of file` markers are
/// dropped from the paired view; the flat face and copy keep the text
/// verbatim.
pub(crate) fn parse_unified_diff(code: &str) -> Option<Vec<diff_view::DiffPart>> {
    use diff_view::DiffPart;

    // Drop the trailing empty element a final newline produces, but nothing
    // else: an empty line inside a hunk is a context line.
    let mut lines: Vec<&str> = code.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }

    let mut parts: Vec<DiffPart> = Vec::new();
    let mut hunk: Option<PartialHunk> = None;

    for line in lines {
        let Some(current) = hunk.as_mut() else {
            // Before the first hunk: the file metadata, or the hunk header
            // that opens the paired view.
            if let Some(rest) = line.strip_prefix("@@ -") {
                let (old, new) = hunk_starts(rest)?;
                hunk = Some(PartialHunk::new(line.to_owned(), old, new));
                continue;
            }
            let is_meta = line.is_empty()
                || line.starts_with("diff --git ")
                || line.starts_with("index ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line.starts_with('\\');
            if !is_meta {
                return None;
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
            let (old, new) = hunk_starts(rest)?;
            hunk = Some(PartialHunk::new(line.to_owned(), old, new));
            continue;
        }
        if !current.line(line) {
            return None;
        }
    }
    if let Some(current) = hunk {
        parts.push(current.ship());
    }
    // A diff with no hunk at all is metadata only — nothing to pair.
    (!parts.is_empty()).then_some(parts)
}

/// The hunk under construction: its header, the rows settled so far, and the
/// two change runs waiting to be zipped (deletions first — the order Git
/// writes, and the order the pair view reads).
struct PartialHunk {
    header: String,
    rows: Vec<diff_view::DiffRow>,
    old_run: Vec<(usize, String)>,
    new_run: Vec<(usize, String)>,
    old: usize,
    new: usize,
}

impl PartialHunk {
    fn new(header: String, old: usize, new: usize) -> Self {
        Self {
            header,
            rows: Vec::new(),
            old_run: Vec::new(),
            new_run: Vec::new(),
            old,
            new,
        }
    }

    /// Consume one hunk body line. `false` when the line has no legal prefix
    /// — the text is not a diff after all.
    fn line(&mut self, line: &str) -> bool {
        use diff_view::RowKind;
        if let Some(text) = line.strip_prefix('-') {
            self.old_run.push((self.old, text.to_owned()));
            self.old += 1;
        } else if let Some(text) = line.strip_prefix('+') {
            self.new_run.push((self.new, text.to_owned()));
            self.new += 1;
        } else {
            // Context — with Git's leading space, or stripped by a tool that
            // trims trailing whitespace — settles both runs. A `\ No newline`
            // note is presentation, not a line: dropped from the pair view.
            let text = match (line.strip_prefix(' '), line.strip_prefix('\\')) {
                (Some(text), _) => Some(text),
                (None, Some(_)) => None,
                (None, None) => return false,
            };
            self.settle_runs();
            if let Some(text) = text {
                self.rows.push(diff_view::DiffRow {
                    old: Some((self.old, text.to_owned())),
                    new: Some((self.new, text.to_owned())),
                    kind: RowKind::Context,
                });
                self.old += 1;
                self.new += 1;
            }
        }
        true
    }

    /// Fold the pending change runs into paired rows: the runs zip in feed
    /// order and the shorter side pads with `None` cells.
    fn settle_runs(&mut self) {
        use diff_view::{DiffRow, RowKind};
        let pairs = self.old_run.len().max(self.new_run.len());
        for index in 0..pairs {
            self.rows.push(DiffRow {
                old: self.old_run.get(index).cloned(),
                new: self.new_run.get(index).cloned(),
                kind: RowKind::Changed,
            });
        }
        self.old_run.clear();
        self.new_run.clear();
    }

    fn ship(mut self) -> diff_view::DiffPart {
        self.settle_runs();
        diff_view::DiffPart::Hunk {
            header: self.header,
            rows: self.rows,
        }
    }
}

/// The `old`/`new` start numbers of a hunk header's fragment — the text
/// after `@@ -`, which carries or omits the counts: `3,7 +3,7 @@` and the
/// count-less `1 +1 @@` both parse.
fn hunk_starts(fragment: &str) -> Option<(usize, usize)> {
    let (old_part, rest) = fragment.split_once(' ')?;
    let old = old_part.split(',').next()?.parse().ok()?;
    let new_part = rest.strip_prefix('+')?;
    let new_token = new_part.split(' ').next()?;
    let new = new_token.split(',').next()?.parse().ok()?;
    Some((old, new))
}

/// Parse complete merge-conflict markers into the merge view's segments, or
/// `None` when a conflict never closes (the mid-stream shape) or none exists
/// — leaving the fence the flat panel.
pub(crate) fn parse_conflict_segments(code: &str) -> Option<Vec<diff_view::MergeSegment>> {
    use diff_view::{MergeSegment, MergeSide};

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
                        base: (stage_has_base(&base)).then(|| std::mem::take(&mut base)),
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

/// Whether the base side ever collected a line or a label — an empty,
/// unlabeled base is the two-way shape.
fn stage_has_base(base: &diff_view::MergeSide) -> bool {
    base.label.is_some() || !base.lines.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parser's contract over one fence: the payload carries the raw code
    /// and language, the text form is the whole fence.
    #[test]
    fn a_fence_becomes_an_editor_block_carrying_code_and_lang() {
        let block = code_block_node("fn main() {}\n", Some("rust"), "```rust\nfn main() {}\n```")
            .expect("intercepted");

        assert_eq!(block.name(), BLOCK_NAME);
        let payload = block
            .data::<Arc<EditorPayload>>()
            .expect("carries its payload")
            .clone();
        assert_eq!(payload.code, "fn main() {}\n");
        assert_eq!(payload.lang.as_deref(), Some("rust"));
        assert_eq!(block.as_text(), "```rust\nfn main() {}\n```");
        assert_eq!(block.as_markdown(), "```rust\nfn main() {}\n```");
    }

    /// A fence with no info string still becomes an editor block, with no
    /// language to show.
    #[test]
    fn a_bare_fence_intercepts_with_no_lang() {
        let block = code_block_node("plain\n", None, "```\nplain\n```").expect("intercepted");
        let payload = block.data::<Arc<EditorPayload>>().expect("payload");
        assert_eq!(payload.lang, None);
        assert_eq!(payload.code, "plain\n");
        assert_eq!(block.as_text(), "```\nplain\n```");
    }

    /// An unterminated fence — the mid-stream shape — intercepts with the
    /// content parsed so far, so a streaming block renders as an editor panel
    /// from its first delta.
    #[test]
    fn an_unterminated_fence_intercepts_with_its_partial_content() {
        let block = code_block_node("x = 1", Some("python"), "```python\nx = 1").expect("taken");
        let payload = block.data::<Arc<EditorPayload>>().expect("payload");
        assert_eq!(payload.lang.as_deref(), Some("python"));
        assert_eq!(payload.code, "x = 1");
    }

    /// The shape a fence's text parses into — the flat panel's dispatch order
    /// (conflict before diff language), with each parser's verdict.
    #[test]
    fn fence_shapes_classify_like_the_flat_dispatch() {
        let shape = |body: &str, lang: &str| {
            let block = code_block_node(body, Some(lang), "```x").expect("taken");
            let payload = block.data::<Arc<EditorPayload>>().expect("payload");
            payload.shape.clone()
        };
        assert!(matches!(
            shape("<<<<<<< a\nb\n=======\nc\n>>>>>>> b", "rust"),
            FenceShape::Merge(_)
        ));
        assert!(matches!(
            shape("@@ -1 +1 @@\n-a\n+b\n", "diff"),
            FenceShape::Diff(_)
        ));
        assert!(matches!(shape("fn main() {}", "rust"), FenceShape::Code));
        // A conflict in a diff fence is a merge view, not a patch.
        assert!(matches!(
            shape("<<<<<<< a\nb\n=======\nc\n>>>>>>> b", "diff"),
            FenceShape::Merge(_)
        ));
    }

    /// The side-by-side parser: context pairs both sides, change runs zip in
    /// feed order with the shorter side padded, hunk headers settle as their
    /// own parts.
    #[test]
    fn a_unified_diff_parses_into_paired_rows() {
        use diff_view::RowKind;
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
        let parts = parse_unified_diff(diff).expect("parses");
        assert_eq!(parts.len(), 3, "two metadata rows and one hunk");
        assert!(
            matches!(&parts[0], diff_view::DiffPart::Meta(m) if m == "--- a/f"),
            "metadata first"
        );
        let diff_view::DiffPart::Hunk { header, rows } = &parts[2] else {
            panic!("a hunk");
        };
        assert!(header.starts_with("@@ -1,3"));
        assert_eq!(
            rows.len(),
            4,
            "context + a 2-vs-1 changed run (two paired rows) + context"
        );
        assert_eq!(rows[0].kind, RowKind::Context);
        assert_eq!(rows[0].old.as_ref().unwrap().0, 1);
        assert_eq!(rows[0].new.as_ref().unwrap().0, 1);
        assert_eq!(rows[1].kind, RowKind::Changed);
        assert_eq!(
            rows[1].old.as_ref().unwrap().1,
            "removed one",
            "deletions zip first"
        );
        assert_eq!(rows[1].new.as_ref().unwrap().1, "added");
        assert_eq!(
            rows[2].old.as_ref().unwrap().1,
            "removed two",
            "the deletion run continues onto its own row"
        );
        assert!(
            rows[2].new.is_none(),
            "the shorter side pads with an empty cell"
        );
        assert_eq!(rows[2].kind, RowKind::Changed);
    }

    /// Multiple hunks each pair within themselves; numbering restarts from
    /// each header's numbers.
    #[test]
    fn multiple_hunks_pair_within_themselves() {
        let diff = "@@ -10,2 +10,2 @@\n-a\n+b\n@@ -40,1 +40,2 @@\n+c\n+d\n";
        let parts = parse_unified_diff(diff).expect("parses");
        assert_eq!(parts.len(), 2);
        let diff_view::DiffPart::Hunk { rows, .. } = &parts[0] else {
            panic!("a hunk");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].old.as_ref().unwrap().0, 10);
        assert_eq!(rows[0].new.as_ref().unwrap().0, 10);
        let diff_view::DiffPart::Hunk { rows, .. } = &parts[1] else {
            panic!("a hunk");
        };
        assert_eq!(rows.len(), 2, "an additions-only run pads the old side");
        assert!(rows.iter().all(|row| row.old.is_none()));
        assert_eq!(rows[0].new.as_ref().unwrap().0, 40);
    }

    /// Prose in a diff fence — a line with no legal prefix and no hunk header
    /// — leaves the fence the flat panel.
    #[test]
    fn prose_in_a_diff_fence_falls_back() {
        assert!(parse_unified_diff("just some thoughts\nabout life").is_none());
    }

    /// A hunk header cut off mid-stream has no numbers yet: the parser
    /// declines, and the fence renders flat until the header completes.
    #[test]
    fn an_unterminated_hunk_header_falls_back() {
        assert!(parse_unified_diff("@@ -1,\n-a\n").is_none());
    }

    /// A three-way conflict: the `|||||||` section parses as the middle
    /// column, labels ride their markers, context runs number from the fence.
    #[test]
    fn a_three_way_conflict_parses_with_its_base() {
        use diff_view::MergeSegment;
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
        let MergeSegment::Context { start, lines } = &segments[0] else {
            panic!("context first");
        };
        assert_eq!(*start, 1);
        assert_eq!(lines.len(), 1);
        let MergeSegment::Conflict { ours, base, theirs } = &segments[1] else {
            panic!("a conflict");
        };
        assert_eq!(ours.label.as_deref(), Some("HEAD"));
        assert_eq!(ours.lines, vec!["ours"]);
        assert_eq!(
            base.as_ref()
                .expect("three-way carries a base")
                .label
                .as_deref(),
            Some("merged common ancestors")
        );
        assert_eq!(theirs.label.as_deref(), Some("feature"));
    }

    /// The two-way shape — no `|||||||` — parses with no base column, and an
    /// unterminated conflict declines to the flat panel.
    #[test]
    fn a_two_way_conflict_has_no_base_and_unterminated_declines() {
        let segments =
            parse_conflict_segments("<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\n").expect("parses");
        let diff_view::MergeSegment::Conflict { base, .. } = &segments[0] else {
            panic!("a conflict");
        };
        assert!(base.is_none(), "no section, no column");

        assert!(parse_conflict_segments("<<<<<<< HEAD\na\n").is_none());
    }

    /// A fence whose body ends with blank lines: the renderer trims them for
    /// gutter count and copy, while the payload keeps the parsed value
    /// verbatim (the text form round-trips the source either way).
    #[test]
    fn trailing_blank_lines_survive_in_the_source_form() {
        let block =
            code_block_node("a\nb\n\n", Some("text"), "```text\na\nb\n\n```").expect("taken");
        let payload = block.data::<Arc<EditorPayload>>().expect("payload");
        assert_eq!(payload.code, "a\nb\n\n");
        assert_eq!(block.as_text(), "```text\na\nb\n\n```");
    }
}
