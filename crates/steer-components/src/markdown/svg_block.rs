//! Inline ```svg fenced blocks: a fence whose info string names `svg` renders
//! as the rasterized diagram, with the fence itself one click away. The
//! ```mermaid fence ([`super::mermaid_block`]) produces an SVG document and
//! shares everything from [`rasterize`] on: the element, the face flip, the
//! display cap.
//!
//! The block parser is the gate, and it decides at parse time: a fence whose
//! content rasterizes through the app's own [`SvgRenderer`] becomes a custom
//! block carrying the finished raster, and anything else — another language, a
//! document that is not valid SVG, a source past [`MAX_SOURCE_BYTES`] — falls
//! through to the ordinary code block, highlight and copy control included.
//! There is no failure face to design: a fence that does not rasterize is a
//! code block because it never stopped being one.
//!
//! Rasterizing during source conversion (not during render) is what keeps the
//! block free to read: the document is re-parsed only for the fragment that
//! changed, so a completed diagram costs nothing per frame, and the raster is
//! owned by the parsed document — exactly as long as the message that holds
//! it. Conversion runs on the background parser for streamed appends, so the
//! rasterization stays off the UI thread.
//!
//! The block shows its image by default and flips to its source on the hover
//! control ([`Face`]), which is also how the source stays reachable at all: an
//! image is not selectable text. The code face is the surface's own markdown
//! stack over the fence source (with no svg block registered on it — the fence
//! would rasterize again, forever), so the source reads as the code block it
//! is. The block's text form is its Markdown source either way, so whole-document
//! copy keeps the fence, and a message round-trips through `ChatDocument`'s
//! markdown unchanged.

use std::borrow::Cow;
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;

use gpui::{
    AnyElement, App, DevicePixels, Entity, ImageSource, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Pixels, RenderImage, SharedString, Size, Styled as _,
    StyledImage as _, SvgRenderer, SvgSize, Window, div, img, px,
};
use gpui_base::TextView;
use gpui_base::text::{MarkdownExtensions, MarkdownNode, MarkdownParseContext, markdown_ast};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Sizable as _, Theme};
use gpui_kit_assets::IconName;

/// Custom block name for one rasterized fence. Renderers are registered by
/// name, so this string is the whole contract between the two halves below.
const BLOCK_NAME: &str = "svg";

/// One svg's source budget. The Preview card rasterizes through the same
/// renderer and reads the same budget, so both paths accept the same input.
pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

/// How tall one diagram may paint, shared with the Preview card's svg body: no
/// single picture pushes the surface it sits in apart.
pub const MAX_DISPLAY_HEIGHT: Pixels = px(512.);

/// The box one svg rasterizes into, in device pixels: the message column's
/// width at 2x, and the ceiling on its height too. Both the inline ```svg
/// block and the Preview card's svg body come through [`rasterize`], so one
/// box bounds both.
pub(crate) const RASTER_BOX: f32 = 1400.;

/// Width of the throwaway render that measures a document's aspect. Rasterizing
/// is what costs; a pixmap this wide costs nothing next to the real one.
const PROBE_WIDTH: f32 = 64.;

/// Rasterize one svg document for display, or `None` when it is not something
/// this renderer can draw.
///
/// [`SvgSize::Size`] bounds the width alone, so a document taller than it is
/// wide would come back taller than the box — a tall diagram's pixmap would run
/// to tens of megabytes for a picture the display caps at 512px anyway. The
/// aspect is measured on a probe render and the real render takes the width
/// that fits the box on both axes. A document too wide for even the probe to
/// give a pixel of height is far wider than it is tall, so the box width is
/// already inside the box on both axes.
pub fn rasterize(renderer: &SvgRenderer, source: &[u8]) -> Option<Arc<RenderImage>> {
    let text = String::from_utf8_lossy(source);
    let text = with_svg_namespace(&text);
    let parsed = match renderer.parse_svg(text.as_bytes()) {
        Ok(parsed) => parsed,
        Err(err) => {
            log::warn!("svg is not drawable, leaving it as code: {err}");
            return None;
        }
    };
    let aspect = match renderer.render_parsed(&parsed, width_box(PROBE_WIDTH)) {
        Ok(probe) => {
            let size = probe.size(0);
            size.width.0 as f32 / size.height.0.max(1) as f32
        }
        Err(_) => RASTER_BOX,
    };
    let width = RASTER_BOX.min(RASTER_BOX * aspect);
    match renderer.render_parsed(&parsed, width_box(width)) {
        Ok(raster) => Some(raster),
        Err(err) => {
            log::warn!("svg is not drawable, leaving it as code: {err}");
            None
        }
    }
}

/// Rasterize one svg document at `width` device pixels (the height follows
/// the document's aspect), bounded by the raster box on both axes — the
/// math block's entry, whose pictures are em-sized rather than column-sized.
pub(crate) fn rasterize_at(
    renderer: &SvgRenderer,
    source: &[u8],
    width: f32,
) -> Option<Arc<RenderImage>> {
    let text = String::from_utf8_lossy(source);
    let text = with_svg_namespace(&text);
    let parsed = renderer.parse_svg(text.as_bytes()).ok()?;
    let aspect = match renderer.render_parsed(&parsed, width_box(PROBE_WIDTH)) {
        Ok(probe) => {
            let size = probe.size(0);
            size.width.0 as f32 / size.height.0.max(1) as f32
        }
        Err(_) => RASTER_BOX,
    };
    let width = width.clamp(1., RASTER_BOX).min(RASTER_BOX * aspect);
    renderer.render_parsed(&parsed, width_box(width)).ok()
}

/// The namespace usvg requires on the root element.
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

/// Point the root element's default namespace at [`SVG_NAMESPACE`].
///
/// A missing namespace is accepted, a non-SVG one is not: usvg rejects the
/// whole document ("does not have a root node"), and models write the wrong
/// URI often enough (`http://w3.org`, an `https` variant) that a diagram would
/// otherwise silently stay a code block. Only the root's default `xmlns` is
/// touched — prefixed declarations (`xmlns:xlink`) are how the document names
/// its own vocabulary and are left alone.
fn with_svg_namespace(source: &str) -> Cow<'_, str> {
    let Some(root) = source.find("<svg") else {
        return Cow::Borrowed(source);
    };
    let Some(tag_end) = tag_end(source, root) else {
        return Cow::Borrowed(source);
    };
    let tag = &source[root..tag_end];
    let Some(attr) = tag
        .match_indices("xmlns=")
        .map(|(offset, _)| offset)
        .find(|offset| {
            // `xmlns=` as an attribute, not the tail of a longer name.
            tag[..*offset]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        })
    else {
        return Cow::Borrowed(source);
    };
    let value_start = root + attr + "xmlns=".len();
    let Some(quote) = source[value_start..]
        .chars()
        .next()
        .filter(|c| *c == '"' || *c == '\'')
    else {
        return Cow::Borrowed(source);
    };
    let value_start = value_start + quote.len_utf8();
    let Some(value_len) = source[value_start..].find(quote) else {
        return Cow::Borrowed(source);
    };
    let value_end = value_start + value_len;
    if &source[value_start..value_end] == SVG_NAMESPACE {
        return Cow::Borrowed(source);
    }
    let mut repaired = String::with_capacity(source.len() + SVG_NAMESPACE.len());
    repaired.push_str(&source[..value_start]);
    repaired.push_str(SVG_NAMESPACE);
    repaired.push_str(&source[value_end..]);
    Cow::Owned(repaired)
}

/// Byte offset just past the `<`-to-`>` tag starting at `start`, skipping any
/// `>` inside attribute quotes.
fn tag_end(source: &str, start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (offset, byte) in source.bytes().enumerate().skip(start) {
        match (quote, byte) {
            (None, b'"' | b'\'') => quote = Some(byte),
            (Some(open), byte) if byte == open => quote = None,
            (None, b'>') => return Some(offset + 1),
            _ => {}
        }
    }
    None
}

/// A [`SvgSize::Size`] box of `width`: the height follows the document's aspect.
fn width_box(width: f32) -> SvgSize {
    let width = DevicePixels(width as i32);
    SvgSize::Size(Size::new(width, width))
}

/// Register the app's fence blocks on a markdown document — the ```svg and
/// ```mermaid fences (rasterized diagrams), the math fences, and the editor
/// code block every remaining fence renders through
/// ([`super::code_block`]). Every markdown surface that shows authored prose
/// goes through this, so a fence renders the same way wherever it appears.
/// Terminal panels are the one exception: their body is a single `ansi` fence
/// around machine output, so nothing inside it is parsed as a block of its
/// own.
///
/// `body_size` is the text size where the surface pins one (`None` leaves
/// the size to the inherited text style, as a per-frame chrome document
/// does); the math block's inline run reads it, and the source faces of the
/// diagram blocks render as editor panels, which style themselves.
pub fn with_fence_blocks(view: TextView, body_size: Option<Pixels>, cx: &App) -> TextView {
    let extensions = extensions(cx.svg_renderer(), body_size);
    view.markdown_extensions(extensions)
}

/// Which face one block shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Face {
    /// The rasterized diagram — what the fence drew, and the default.
    Image,
    /// The fence itself, as the code block it also is.
    Code,
}

impl Face {
    fn flipped(self) -> Self {
        match self {
            Face::Image => Face::Code,
            Face::Code => Face::Image,
        }
    }
}

/// The document's Markdown extension registry: the defaults plus the two
/// diagram fences, the editor code block ([`super::code_block`], last so the
/// fences the others own never reach it), and LaTeX math
/// ([`super::math_block`]), which typesets to an SVG document in both its
/// shapes — the display block and the inline run — and rides this block's
/// raster path from there.
pub(crate) fn extensions(renderer: SvgRenderer, body_size: Option<Pixels>) -> MarkdownExtensions {
    let mermaid_renderer = renderer.clone();
    let math_renderer = renderer.clone();
    let math_text_size = body_size;
    let extensions = MarkdownExtensions::default()
        .block_parser(move |node, cx| svg_fence(node, cx, &renderer))
        .block_renderer(BLOCK_NAME, move |node, window, cx| {
            svg_element(node, BLOCK_NAME, window, cx)
        })
        .block_parser(move |node, cx| {
            super::mermaid_block::mermaid_fence(node, cx, &mermaid_renderer)
        })
        .block_renderer(super::mermaid_block::BLOCK_NAME, move |node, window, cx| {
            svg_element(node, super::mermaid_block::BLOCK_NAME, window, cx)
        })
        .block_parser(super::code_block::code_fence)
        .block_renderer(super::code_block::BLOCK_NAME, |node, window, cx| {
            super::code_block::code_element(node, window, cx)
        });
    super::math_block::math_extensions(extensions, math_renderer, math_text_size)
}

/// Turn an ```svg fence into a custom block holding its raster, or `None` to
/// leave it as the code block the Markdown parser built.
fn svg_fence(
    node: &markdown_ast::Node,
    cx: &MarkdownParseContext<'_>,
    renderer: &SvgRenderer,
) -> Option<MarkdownNode> {
    let markdown_ast::Node::Code(code) = node else {
        return None;
    };
    let fence = cx.node_source(node).unwrap_or(&code.value);
    raster_fence(code.lang.as_deref(), &code.value, fence, renderer)
}

/// The block for one fenced code node, or `None` when the fence is not an svg
/// this renderer can draw — which leaves the fence the code block it already
/// is. `fence` is the block's text form: the whole fence, delimiters and info
/// string included, so copy and Markdown round-trip reproduce the source.
fn raster_fence(
    lang: Option<&str>,
    body: &str,
    fence: &str,
    renderer: &SvgRenderer,
) -> Option<MarkdownNode> {
    if !lang.is_some_and(|lang| lang.eq_ignore_ascii_case(BLOCK_NAME)) {
        return None;
    }
    if body.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let raster = rasterize(renderer, body.as_bytes())?;
    Some(
        MarkdownNode::new(BLOCK_NAME, Arc::new(DiagramFence::new(raster, body, lang)))
            .text(fence)
            .markdown(fence),
    )
}

/// One diagram fence's payload: the raster the image face paints, plus the
/// raw source and language the source face renders as an editor panel.
#[derive(Clone)]
pub(crate) struct DiagramFence {
    pub(crate) raster: Arc<RenderImage>,
    code: SharedString,
    lang: Option<SharedString>,
}

impl DiagramFence {
    pub(crate) fn new(
        raster: Arc<RenderImage>,
        code: impl Into<SharedString>,
        lang: Option<&str>,
    ) -> Self {
        Self {
            raster,
            code: code.into(),
            lang: lang.map(SharedString::from),
        }
    }
}

/// The block's element: the face it shows, over the hover control that flips
/// it. The state lives in the window's keyed state, so the reader's flip
/// survives every frame the document re-renders (and every scroll that remounts
/// the row) without the document or the message having to own it.
fn svg_element(
    node: &MarkdownNode,
    name: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(fence) = node.data::<Arc<DiagramFence>>() else {
        return div().child(node.as_text().to_string()).into_any_element();
    };
    let key = block_key(node);
    let face = window.use_keyed_state(("markdown-svg-face", key), cx, |_, _| Face::Image);
    let showing = *face.read(cx);
    let group: SharedString = format!("markdown-svg-{key}").into();

    div()
        .relative()
        .group(group.clone())
        .w_full()
        .min_w_0()
        .child(match showing {
            Face::Image => image_face(&fence.raster, name),
            Face::Code => {
                // The source face is the same editor panel any fence renders
                // — one implementation, one presentation (Flat: no second
                // face to toggle, the flip back belongs to the block). The
                // wrapper keeps the face's own selector for the flip tests.
                div()
                    .debug_selector(move || format!("markdown-inline-{name}-code"))
                    .w_full()
                    .min_w_0()
                    .child(super::code_block::panel(
                        super::code_block::PanelShape::Flat,
                        &fence.code,
                        fence.lang.as_deref(),
                        key,
                        window,
                        cx,
                    ))
                    .into_any_element()
            }
        })
        .child(
            div()
                .absolute()
                .top_1()
                .right_1()
                .opacity(0.75)
                .group_hover(group, |style| style.opacity(1.))
                .child(face_toggle(key, name, showing, face, cx)),
        )
        .into_any_element()
}

/// One block's identity inside its document: the source range upstream recorded
/// for it. Two documents can hand out the same range, but the element ids these
/// key into are namespaced by the document that renders them.
pub(crate) fn block_key(node: &MarkdownNode) -> u64 {
    // Only the range's START hashes: the end grows with every streamed
    // delta, and these keys name window-keyed state (scroll offset, face
    // flips, copy flashes) that must survive appends instead of resetting
    // the panel each delta. Within one document no two blocks share a start.
    let mut hasher = std::hash::DefaultHasher::new();
    match node.source_range() {
        Some(range) => range.start.hash(&mut hasher),
        // A custom node always carries its span; a block without one still
        // keys apart by its source instead of collapsing onto key zero.
        None => node.as_markdown().len().hash(&mut hasher),
    }
    hasher.finish()
}

/// The diagram: a box carrying the raster's own aspect at the column's width,
/// capped at [`MAX_DISPLAY_HEIGHT`].
///
/// The box takes the raster's aspect rather than the image's natural size:
/// `img` boxes an image into the size it was asked for and centers it there,
/// so a natural-height box parks a wide diagram in dead space. The cap holds
/// for both dimensions, which keeps the diagram's aspect and centering it
/// where the column is wider than the capped picture.
fn image_face(raster: &Arc<RenderImage>, name: &'static str) -> AnyElement {
    let size = raster.size(0);
    let aspect = size.width.0 as f32 / size.height.0.max(1) as f32;
    let max_w = px(MAX_DISPLAY_HEIGHT.as_f32() * aspect);
    div()
        .w_full()
        .min_w_0()
        .max_w(max_w)
        .max_h(MAX_DISPLAY_HEIGHT)
        .mx_auto()
        .aspect_ratio(aspect)
        .debug_selector(|| format!("markdown-inline-{name}"))
        .child(
            img(ImageSource::Render(raster.clone()))
                .size_full()
                .object_fit(ObjectFit::Contain),
        )
        .into_any_element()
}

/// The hover control that flips the block. Its icon names the face a click
/// shows — the one the reader is not looking at — and it carries a background
/// because it sits on the picture.
fn face_toggle(
    key: u64,
    name: &'static str,
    showing: Face,
    face: Entity<Face>,
    cx: &mut App,
) -> AnyElement {
    let next = showing.flipped();
    let icon = match next {
        Face::Image => IconName::Image,
        Face::Code => IconName::Code,
    };
    let theme = Theme::global(cx);
    div()
        .rounded(theme.radius)
        .bg(theme.background.opacity(0.85))
        .border_1()
        .border_color(theme.border)
        .debug_selector(|| format!("markdown-inline-{name}-toggle"))
        .child(
            Button::new(("markdown-svg-toggle", key))
                .ghost()
                .xsmall()
                .icon(icon)
                .on_click(move |_, _, cx| {
                    face.update(cx, |face, cx| {
                        *face = face.flipped();
                        cx.notify();
                    });
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    /// A fence that draws: one rect, no text (a test App has no bundled fonts
    /// to resolve, and a shapes-only document needs none).
    const DIAGRAM: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"100\"><rect width=\"200\" height=\"100\" fill=\"#3b82f6\"/></svg>";

    fn fence(lang: &str, body: &str) -> String {
        format!("```{lang}\n{body}\n```")
    }

    #[gpui::test]
    fn a_drawable_svg_fence_becomes_an_image_block(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        let source = fence("svg", DIAGRAM);
        let block =
            raster_fence(Some("SVG"), DIAGRAM, &source, &renderer).expect("the fence rasterizes");

        assert_eq!(block.name(), BLOCK_NAME);
        let fence = block
            .data::<Arc<DiagramFence>>()
            .expect("carries its payload");
        assert!(matches!(fence.lang.as_deref(), Some("SVG") | Some("svg")));
        assert_eq!(fence.code, DIAGRAM);
        assert_eq!(block.as_text(), source, "the text form is the fence");
        assert_eq!(block.as_markdown(), source);
    }

    /// A model-written artifact: the namespace is wrong (`http://w3.org`), the
    /// labels are emoji, and the card styling leans on gradients and a drop
    /// shadow. usvg rejects a non-SVG namespace outright, so this must be
    /// repaired rather than silently left as a code block.
    const MODEL_ARTIFACT: &str = r##"<svg xmlns="http://w3.org" viewBox="0 0 1100 200" width="100%" height="100%">
  <defs>
    <linearGradient id="gitflowGrad" x1="0%" y1="0%" x2="0%" y2="100%">
      <stop offset="0%" stop-color="#2c3e50" />
      <stop offset="100%" stop-color="#1a2632" />
    </linearGradient>
    <filter id="cardShadow" x="-10%" y="-10%" width="120%" height="125%">
      <feDropShadow dx="0" dy="5" stdDeviation="8" flood-color="#000000" flood-opacity="0.12"/>
    </filter>
  </defs>
  <g transform="translate(0, 0)" filter="url(#cardShadow)">
    <rect width="340" height="200" rx="16" fill="url(#gitflowGrad)" stroke="#475569" stroke-width="1.5" />
    <text x="170" y="95" font-family="sans-serif" font-size="22" font-weight="black" fill="#f8fafc" text-anchor="middle">规范的 Gitflow</text>
    <text x="170" y="130" font-family="sans-serif" font-size="13" font-weight="bold" fill="#94a3b8" text-anchor="middle">🎯 分支清晰 • 研发井然有序</text>
  </g>
</svg>"##;

    #[gpui::test]
    fn a_wrong_root_namespace_is_repaired(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        let block = raster_fence(
            Some("svg"),
            MODEL_ARTIFACT,
            &fence("svg", MODEL_ARTIFACT),
            &renderer,
        )
        .expect("the artifact draws once its namespace names svg");
        assert!(block.data::<Arc<DiagramFence>>().is_some());
    }

    #[test]
    fn namespace_repair_touches_only_the_roots_default_declaration() {
        let wrong = "<svg xmlns=\"http://w3.org\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"><rect/></svg>";
        assert_eq!(
            with_svg_namespace(wrong),
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"><rect/></svg>"
        );
        // Already canonical, absent, and prefixed-only documents pass through.
        for untouched in [
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>",
            "<svg viewBox=\"0 0 1 1\"><rect/></svg>",
            "<svg xmlns:xlink=\"http://www.w3.org/1999/xlink\"><rect/></svg>",
        ] {
            assert!(matches!(with_svg_namespace(untouched), Cow::Borrowed(_)));
        }
    }

    #[gpui::test]
    fn every_other_fence_stays_a_code_block(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        let undrawable =
            |body: &str| raster_fence(Some("svg"), body, &fence("svg", body), &renderer);

        // Another language, an indented block (no info string at all).
        assert!(
            raster_fence(
                Some("rust"),
                "fn main() {}",
                "```rust\nfn main() {}\n```",
                &renderer
            )
            .is_none()
        );
        assert!(raster_fence(None, DIAGRAM, &fence("", DIAGRAM), &renderer).is_none());
        // An svg the rasterizer rejects: a fence that draws nothing would read
        // as a broken message, so it must not become a block.
        assert!(undrawable("<svg><rect").is_none());
        // Past the source budget.
        let huge = format!("<svg>{}</svg>", " ".repeat(MAX_SOURCE_BYTES));
        assert!(undrawable(&huge).is_none());
    }

    /// The raster box bounds both axes. A document far wider than the probe can
    /// express a height still rasterizes (the probe's zero-height render must
    /// not read as "not drawable"), and a tall one is drawn at the width that
    /// fits the box rather than at its own height.
    #[gpui::test]
    fn a_raster_fits_the_box_on_both_axes(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        let wide = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2000\" height=\"10\"><rect width=\"2000\" height=\"10\"/></svg>";
        let tall = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"2000\"><rect width=\"100\" height=\"2000\"/></svg>";
        let box_px = RASTER_BOX as i32;

        let wide = rasterize(&renderer, wide.as_bytes()).expect("a very wide document draws");
        let wide = wide.size(0);
        assert_eq!(wide.width.0, box_px, "a wide document fills the box width");
        assert!(wide.height.0 <= box_px, "and stays inside it: {wide:?}");

        let tall = rasterize(&renderer, tall.as_bytes()).expect("a very tall document draws");
        let tall = tall.size(0);
        assert!(
            (1..=box_px).contains(&tall.height.0),
            "a tall document is drawn within the box height: {tall:?}"
        );
        assert!(
            (tall.width.0 as f32 / tall.height.0 as f32 - 0.05).abs() < 0.01,
            "and keeps its aspect: {tall:?}"
        );
    }
}
