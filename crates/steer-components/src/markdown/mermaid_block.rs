//! Inline ```mermaid fenced blocks: the fence's source becomes a diagram the
//! same way an ```svg fence does — an SVG document produced at markdown-parse
//! time, rasterized through the app's own [`SvgRenderer`] — so the block
//! paints the finished picture with the fence one click away (the identical
//! face flip, the shared [`svg_block`] element).
//!
//! The renderer is [`mermaid_rs_renderer`], pure Rust and fast enough to run
//! during source conversion: the document is re-parsed only for the fragment
//! that changed, conversion rides the background parser for streamed appends,
//! and the raster is owned by the parsed document exactly as long as the
//! message holding it. A fence that does not render — an unsupported diagram
//! kind, a syntax error — falls through to the ordinary code block, highlight
//! and copy control included: there is no failure face, a fence that renders
//! nothing never stopped being code.
//!
//! A fence still being written stays code until its closing delimiter lands.
//! mdast hands an unterminated fence through as a code block too, and mermaid
//! source parses at almost every line boundary, so converting early would
//! swap the block under the reader for a diagram that keeps changing (and
//! rasterize again for every token of the stream).
//!
//! The renderer's default palette is the light chrome the app pins. A dark
//! mode must select the renderer's dark theme here, and the rasters cached in
//! the parsed documents have to be rebuilt with it.

use std::sync::Arc;

use gpui::{RenderImage, SvgRenderer};
use gpui_base::text::{MarkdownNode, MarkdownParseContext, markdown_ast};

use crate::markdown::svg_block::{MAX_SOURCE_BYTES, rasterize};

/// Custom block name for one rendered fence — the whole contract between this
/// parse half and the shared element registered in [`svg_block::extensions`].
pub const BLOCK_NAME: &str = "mermaid";

/// Turn one ```mermaid fence into the block the svg stack paints, or `None`
/// to leave it the code block it already is.
pub fn mermaid_fence(
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

/// The block for one fenced code node, or `None` when the fence is not a
/// mermaid diagram this renderer can draw. The SVG mermaid produces feeds the
/// shared rasterizer, so both fences share one box, one aspect policy and one
/// display cap. `fence` is the block's text form: the whole fence, delimiters
/// and info string included, so copy and Markdown round-trip reproduce it.
/// A fence whose closing delimiter has not arrived is still being written and
/// never converts: the streaming path reparses it for every delta, and each
/// one would paint a different diagram.
fn raster_fence(
    lang: Option<&str>,
    body: &str,
    fence: &str,
    renderer: &SvgRenderer,
) -> Option<MarkdownNode> {
    if !lang.is_some_and(|lang| lang.eq_ignore_ascii_case(BLOCK_NAME)) {
        return None;
    }
    if !is_closed(fence) {
        return None;
    }
    if body.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let svg = mermaid_rs_renderer::render(body)
        .map_err(|err| log::warn!("mermaid does not render, leaving it as code: {err}"))
        .ok()?;
    let raster: Arc<RenderImage> = rasterize(renderer, svg.as_bytes())?;
    Some(
        MarkdownNode::new(
            BLOCK_NAME,
            Arc::new(super::svg_block::DiagramFence::new(
                raster,
                body,
                Some(BLOCK_NAME),
            )),
        )
        .text(fence)
        .markdown(fence),
    )
}

/// Whether the fence source carries its closing delimiter. Only a completed
/// fence has one: mdast ends an unterminated fence at the last content line,
/// so the trailing delimiter run is the signal that the model is done writing
/// the diagram.
fn is_closed(fence: &str) -> bool {
    let Some(opening) = fence.lines().next() else {
        return false;
    };
    let opening = opening.trim_start();
    let Some(delimiter) = opening.chars().next().filter(|c| *c == '`' || *c == '~') else {
        return false;
    };
    let opening_len = opening.chars().take_while(|c| *c == delimiter).count();
    let Some(closing) = fence.trim_end().lines().next_back() else {
        return false;
    };
    let closing = closing.trim();
    opening_len >= 3 && closing.len() >= opening_len && closing.chars().all(|c| c == delimiter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn fence(lang: &str, body: &str) -> String {
        format!("```{lang}\n{body}\n```")
    }

    const FLOWCHART: &str = "flowchart LR\n    A[Input] --> B{Gate}\n    B -->|yes| C[Done]";

    /// A mermaid fence that renders becomes the shared diagram block carrying
    /// its raster, with the fence itself as the text and Markdown form.
    #[gpui::test]
    fn a_rendering_mermaid_fence_becomes_a_diagram_block(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        let source = fence("mermaid", FLOWCHART);
        let block = raster_fence(Some("Mermaid"), FLOWCHART, &source, &renderer)
            .expect("the flowchart renders and rasterizes");

        assert_eq!(block.name(), BLOCK_NAME);
        let fence = block
            .data::<Arc<crate::markdown::svg_block::DiagramFence>>()
            .expect("the block carries its payload");
        let size = fence.raster.size(0);
        assert!(
            size.width.0 > 0 && size.height.0 > 0,
            "a drawn picture: {size:?}"
        );
        assert_eq!(block.as_text(), source, "the text form is the fence");
        assert_eq!(block.as_markdown(), source);
    }

    /// Every diagram family the model is likely to write renders — a kind
    /// mermaid-rs cannot lay out would silently stay a code block, and that
    /// is the regression this pins against upstream drift.
    #[gpui::test]
    fn the_common_diagram_kinds_render(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        for body in [
            "sequenceDiagram\n    Alice->>Bob: hi\n    Bob-->>Alice: hello",
            "classDiagram\n    Animal <|-- Duck\n    class Animal {\n      +name\n    }",
            "stateDiagram-v2\n    [*] --> Idle\n    Idle --> Running : start",
            "pie title Share\n    \"a\" : 40\n    \"b\" : 60",
            "gantt\n    title Plan\n    dateFormat YYYY-MM-DD\n    section A\n    task :a1, 2026-01-01, 3d",
            "erDiagram\n    USER ||--o{ ORDER : places",
            "mindmap\n  root((core))\n    a\n    b",
        ] {
            assert!(
                raster_fence(Some("mermaid"), body, &fence("mermaid", body), &renderer).is_some(),
                "renders: {body}"
            );
        }
    }

    /// A fence that renders nothing stays a code block: unknown syntax is not
    /// a failure face, it is the fence the model wrote.
    #[gpui::test]
    fn a_fence_that_does_not_render_stays_a_code_block(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        // An unrecognized diagram keyword is mermaid's own hard error.
        let body = "notadiagram\n    A --> B";
        assert!(raster_fence(Some("mermaid"), body, &fence("mermaid", body), &renderer).is_none());
        // Another language, no info string, and past the source budget.
        assert!(
            raster_fence(
                Some("rust"),
                FLOWCHART,
                &fence("rust", FLOWCHART),
                &renderer
            )
            .is_none()
        );
        assert!(raster_fence(None, FLOWCHART, &fence("", FLOWCHART), &renderer).is_none());
        let huge = format!(
            "flowchart LR\n{}",
            "A-->B\n".repeat(MAX_SOURCE_BYTES / 6 + 1)
        );
        assert!(
            raster_fence(Some("mermaid"), &huge, &fence("mermaid", &huge), &renderer).is_none()
        );
    }

    /// An unterminated fence is still streaming in: mermaid source parses at
    /// almost every line boundary, so the block must stay code until the
    /// closing delimiter lands. Both delimiter characters are pinned.
    #[gpui::test]
    fn a_streaming_fence_stays_code_until_it_closes(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        let body = "flowchart LR\n    A[Input] --> B{Gate}";
        let open = format!("```mermaid\n{body}\n");
        assert!(
            raster_fence(Some("mermaid"), body, &open, &renderer).is_none(),
            "an open fence never paints"
        );
        let closed = format!("```mermaid\n{body}\n```");
        assert!(
            raster_fence(Some("mermaid"), body, &closed, &renderer).is_some(),
            "the closing fence completes it"
        );
        let tilde = format!("~~~mermaid\n{body}\n~~~");
        assert!(
            raster_fence(Some("mermaid"), body, &tilde, &renderer).is_some(),
            "tilde fences close the same way"
        );
    }
}
