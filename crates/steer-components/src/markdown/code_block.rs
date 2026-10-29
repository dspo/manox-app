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

use std::sync::Arc;

use gpui::{
    AnyElement, App, Axis, ClipboardItem, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px, relative,
};
use gpui_base::text::{MarkdownNode, MarkdownParseContext, markdown_ast};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::{ScrollableMask, Scrollbar};
use gpui_component::{IconName, Sizable as _, Theme, h_flex};

use crate::copy_feedback::COPY_FEEDBACK;
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

/// The body's vertical padding, shared by the gutter and the code area so
/// their baselines align line for line.
const BODY_PAD_Y: f32 = 12.;

/// One intercepted fence's payload: the raw code (what copy takes) and the
/// info string's language token (verbatim — a protocol value, never localized).
#[derive(Clone)]
pub(crate) struct EditorCode {
    code: SharedString,
    lang: Option<SharedString>,
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
    Some(
        MarkdownNode::new(
            BLOCK_NAME,
            Arc::new(EditorCode {
                code: body.to_owned().into(),
                lang: lang.map(SharedString::from),
            }),
        )
        .text(fence)
        .markdown(fence),
    )
}

/// The editor panel: header bar over a gutter-and-code body. The code always
/// reads at the mono size the rest of the app's code surfaces use, so a block
/// is the same artifact wherever it lands.
pub(crate) fn code_element(node: &MarkdownNode, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(payload) = node.data::<Arc<EditorCode>>() else {
        return div().child(node.as_text().to_string()).into_any_element();
    };
    // Cloned up front: the render borrows `cx` again below (keyed state, the
    // copy control), and the held borrow would collide with them.
    let theme = Theme::global(cx).clone();
    let key = block_key(node);

    // Fenced values can carry trailing blank lines; the gutter counts and the
    // painted code agree on the trimmed form, and copy takes it too.
    let code: SharedString = payload.code.trim_end_matches('\n').to_owned().into();
    let line_count = code.split('\n').count().max(1);
    let num_width = line_count.to_string().len();
    let gutter: SharedString = (1..=line_count)
        .map(|n| format!("{n:>num_width$}"))
        .collect::<Vec<_>>()
        .join("\n")
        .into();
    let runs = dispatch_runs(
        &code,
        payload.lang.as_deref(),
        &DispatchPalette::new(&theme),
    );

    let header = h_flex()
        .items_center()
        .justify_between()
        .w_full()
        .min_w_0()
        .px_3()
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
        // The spacer keeps the control at the trailing edge on blocks with no
        // language to show — `justify_between` parks a lone child at the start.
        .child(div().flex_1())
        .child(copy_control(key, code.to_string(), window, cx));

    // The body scrolls as one region: the gutter holds its column while the
    // code scrolls under it, and a tall block grows a scrollbar instead of
    // pushing the transcript apart.
    let scroll = window
        .use_keyed_state(("code-scroll", key), cx, |_, _| ScrollHandle::default())
        .read(cx)
        .clone();
    let body = h_flex()
        .id(("code-body", key))
        .w_full()
        .min_w_0()
        .items_start()
        .max_h(MAX_BODY_HEIGHT)
        .overflow_y_scroll()
        .track_scroll(&scroll)
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
                    gpui::StyledText::new(code)
                        .with_highlights(runs)
                        .into_any_element(),
                ),
        );

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
                .relative()
                .w_full()
                .min_w_0()
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
            .data::<Arc<EditorCode>>()
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
        let payload = block.data::<Arc<EditorCode>>().expect("payload");
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
        let payload = block.data::<Arc<EditorCode>>().expect("payload");
        assert_eq!(payload.lang.as_deref(), Some("python"));
        assert_eq!(payload.code, "x = 1");
    }

    /// A fence whose body ends with blank lines: the renderer trims them for
    /// gutter count and copy, while the payload keeps the parsed value
    /// verbatim (the text form round-trips the source either way).
    #[test]
    fn trailing_blank_lines_survive_in_the_source_form() {
        let block =
            code_block_node("a\nb\n\n", Some("text"), "```text\na\nb\n\n```").expect("taken");
        let payload = block.data::<Arc<EditorCode>>().expect("payload");
        assert_eq!(payload.code, "a\nb\n\n");
        assert_eq!(block.as_text(), "```text\na\nb\n\n```");
    }
}
