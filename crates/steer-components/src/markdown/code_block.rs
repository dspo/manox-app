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

use gpui::prelude::FluentBuilder as _;
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

use crate::code_surface::{self, BODY_PAD_Y, DiffPart, MergeSegment, is_conflict};
use crate::copy_feedback::COPY_FEEDBACK;
use crate::markdown::diff_view;
use crate::markdown::svg_block::block_key;
use crate::markdown::text_view::{DispatchPalette, dispatch_runs};

/// Custom block name for one editor-style fence. Renderers are registered by
/// name, so this string is the whole contract between the parser and the
/// renderer halves.
pub(crate) const BLOCK_NAME: &str = "code";

/// How tall a block's body may paint before it scrolls internally. The
/// terminal panel paginates at a similar line budget; an editor panel scrolls
/// instead, because a code block reads as one artifact.
const MAX_BODY_HEIGHT: Pixels = px(420.);

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
    /// A unified diff: hunks of numbered rows for the side-by-side view.
    Diff(Vec<DiffPart>),
    /// Complete merge-conflict markers: column groups for the merge view.
    Merge(Vec<MergeSegment>),
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
        match code_surface::parse_conflict_segments(body) {
            Some(segments) => FenceShape::Merge(segments),
            None => FenceShape::Code,
        }
    } else if lang.is_some_and(|l| l.eq_ignore_ascii_case("diff")) {
        // Only a well-formed diff opens the split view; junk in the fence
        // (prose, a header mid-write) keeps the flat panel, whose line
        // coloring tolerates anything.
        match code_surface::parse_unified_diff(body) {
            Some(parsed) if parsed.well_formed => FenceShape::Diff(parsed.parts),
            _ => FenceShape::Code,
        }
    } else {
        FenceShape::Code
    };
    // The text form is the raw code (a drag crossing the block and
    // plain-format copies take the code, not the fence); the markdown form
    // is the whole fence, so a Markdown-format copy round-trips the source.
    Some(
        MarkdownNode::new(
            BLOCK_NAME,
            Arc::new(EditorPayload {
                code: body.to_owned().into(),
                lang: lang.map(SharedString::from),
                shape,
            }),
        )
        .text(body)
        .markdown(fence),
    )
}

/// Which face a shaped block shows. The flat panel is one face of every
/// shape; the second faces live in [`diff_view`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockView {
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
    let key = block_key(node);

    // The view starts at the shape's second face when there is one (that is
    // the face the fence was written for) and never drifts from the reader's
    // toggle: the state lives in the window's keyed store per block.
    let view = window.use_keyed_state(("code-view", key), cx, |_, _| match &payload.shape {
        FenceShape::Diff(_) => BlockView::Split,
        FenceShape::Merge(_) => BlockView::Merge,
        FenceShape::Code => BlockView::Flat,
    });

    // Fenced values can carry trailing blank lines; the gutter counts and the
    // painted code agree on the trimmed form, and copy takes it too.
    let code: SharedString = payload.code.trim_end_matches('\n').to_owned().into();
    panel(
        PanelShape::Shaped {
            shape: &payload.shape,
            view: &view,
        },
        &code,
        payload.lang.as_deref(),
        key,
        window,
        cx,
    )
}

/// Which body a panel paints, and whether the header carries the face
/// toggle. `Shaped` is a fence's own panel — shape decides the default face
/// and the toggle appears; `Flat` is a bare code panel with no second face
/// (the diagram blocks' source faces compose through this).
pub(crate) enum PanelShape<'a> {
    Shaped {
        shape: &'a FenceShape,
        view: &'a gpui::Entity<BlockView>,
    },
    Flat,
}

/// One editor panel for arbitrary code — the same artifact a fence renders,
/// callable from outside the markdown pipeline (the diagram blocks' source
/// faces). `key` namespaces the panel's element ids and keyed state; pass a
/// key distinct from the caller's other uses.
pub(crate) fn panel(
    shaped: PanelShape<'_>,
    code: &str,
    lang: Option<&str>,
    key: u64,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    // Cloned up front: the render borrows `cx` again below (keyed state, the
    // copy control), and the held borrow would collide with them.
    let theme = Theme::global(cx).clone();
    let code: SharedString = code.trim_end_matches('\n').to_owned().into();

    let (shape, view): (Option<&FenceShape>, Option<&gpui::Entity<BlockView>>) = match shaped {
        PanelShape::Shaped { shape, view } => (Some(shape), Some(view)),
        PanelShape::Flat => (None, None),
    };
    let showing = match (shape, view) {
        (Some(_), Some(view)) => *view.read(cx),
        _ => BlockView::Flat,
    };

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
        .children(lang.filter(|lang| !lang.is_empty()).map(|lang| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .min_w_0()
                .overflow_hidden()
                .child(SharedString::from(lang.to_owned()))
        }))
        // The spacer keeps the controls at the trailing edge on blocks with
        // nothing to show on the left.
        .child(div().flex_1())
        .children(shape.and_then(|shape| {
            let view = view?;
            (*shape != FenceShape::Code).then(|| view_toggle(key, shape, showing, view.clone()))
        }))
        .child(copy_control(key, code.to_string(), window, cx));

    // The body scrolls vertically as one region — whichever face it carries —
    // and a tall block grows a scrollbar instead of pushing the transcript
    // apart. Horizontal scrolling belongs to the face.
    let scroll = window
        .use_keyed_state(("code-scroll", key), cx, |_, _| ScrollHandle::default())
        .read(cx)
        .clone();
    let palette = DispatchPalette::new(&theme);
    let body = match (shape, showing) {
        (Some(FenceShape::Diff(parts)), BlockView::Split) => {
            diff_view::split_body(parts, key, &theme)
        }
        (Some(FenceShape::Merge(segments)), BlockView::Merge) => {
            diff_view::merge_body(segments, key, &theme)
        }
        _ => flat_body(&code, lang, &palette, &theme, key),
    };

    div()
        .id(("code-block", key))
        .debug_selector(|| "markdown-editor-code".into())
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
    // An empty fence numbers nothing: a gutter row reading "1" over no code
    // is noise.
    let line_count = if code.is_empty() {
        0
    } else {
        code.split('\n').count().max(1)
    };
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
        .when(!gutter.is_empty(), |row| {
            row.child(
                div()
                    .py(px(BODY_PAD_Y))
                    .px_2()
                    .flex_shrink_0()
                    .text_color(theme.muted_foreground.opacity(0.7))
                    .child(gutter),
            )
        })
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
        .debug_selector(|| "markdown-editor-code-view-toggle".into())
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
        // The text form is the raw code (plain-format copies take the code);
        // the markdown form is the whole fence (markdown-format copies
        // round-trip the source).
        assert_eq!(block.as_text(), "fn main() {}\n");
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
        assert_eq!(block.as_text(), "plain\n");
        assert_eq!(block.as_markdown(), "```\nplain\n```");
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
        // A diff fence carrying prose parses but not well-formed: the flat
        // panel keeps it, because its line coloring tolerates anything.
        assert!(matches!(
            shape("just some thoughts\nabout life", "diff"),
            FenceShape::Code
        ));
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
        assert_eq!(block.as_markdown(), "```text\na\nb\n\n```");
    }
}
