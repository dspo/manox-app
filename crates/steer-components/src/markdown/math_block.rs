//! Block LaTeX math: a `$$…$$` pair in an assistant message is a display
//! equation, and the markdown stack hands it over as an `mdast::Node::Math`.
//! The formula typesets to a standalone SVG document by [`mathra`] — an
//! embedded pure-Rust engine drawn with the STIX Two Math font's MATH table
//! (fractions, radicals, scripts, matrices, delimiters), glyphs as path
//! outlines — and that document rides the same [`svg_block::rasterize`] path
//! the diagram fences paint through.
//!
//! Unclaimed `$$…$$` already renders as its literal text (upstream keeps
//! unclaimed math literal), so there is no failure face to design: a formula
//! that does not typeset — strict mode refuses invalid LaTeX — is the text
//! it always was. The block's text form is the original `$$…$$` source, so
//! whole-document copy and the markdown round-trip keep it.

use std::sync::Arc;

use gpui::{
    AnyElement, App, ImageSource, InteractiveElement as _, IntoElement as _, ObjectFit,
    ParentElement as _, Pixels, RenderImage, Styled as _, StyledImage as _, SvgRenderer, div, img,
    px,
};
use gpui_base::text::{MarkdownExtensions, MarkdownNode, MarkdownParseContext, markdown_ast};

use crate::markdown::svg_block::{self, MAX_SOURCE_BYTES, rasterize_at};

/// Custom block name for one typeset equation — the whole contract between
/// the parse and render halves registered in [`svg_block::extensions`].
const BLOCK_NAME: &str = "math";

/// The engine's ink. mathra writes `currentColor` and falls back to a fixed
/// dark ink for standalone documents; the raster IS a standalone document, so
/// the same fixed dark ink is applied as an explicit root `color` (the chat
/// surface is dark ink on light chrome; a theme-aware re-ink is a re-raster
/// and lands with one).
const INK: &str = "#111";

/// The nominal em box, in CSS pixels, the formula's `…em` dimensions are
/// rasterized against: the raster's crispness target (two pixels per em px)
/// and the element's natural-size display both derive from it.
const EM_PX: f32 = 16.;

/// One typeset formula: the raster plus the em dimensions the SVG declared
/// (`width="…em" height="…em"`), which set the element's natural display size
/// at the document's own text size.
#[derive(Clone)]
struct MathRaster {
    raster: Arc<RenderImage>,
    width_em: f32,
    height_em: f32,
}

/// Turn an `mdast::Node::Math` into the block, or `None` to leave it the
/// literal text the unclaimed path renders. The original `$$…$$` span is the
/// text form, exactly as the fences keep their delimiters.
fn math_fence(
    node: &markdown_ast::Node,
    cx: &MarkdownParseContext<'_>,
    renderer: &SvgRenderer,
) -> Option<MarkdownNode> {
    let markdown_ast::Node::Math(math) = node else {
        return None;
    };
    if math.value.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let source = cx
        .node_source(node)
        .map(str::to_string)
        .unwrap_or_else(|| format!("$$\n{}\n$$", math.value));
    let raster = typeset(&math.value, renderer)?;
    Some(
        MarkdownNode::new(BLOCK_NAME, raster)
            .text(source.clone())
            .markdown(source),
    )
}

/// The block's element: the formula centered at its natural size — the em box
/// the SVG declared, at the document's own text size — capped at the column
/// like every other picture.
fn math_element(node: &MarkdownNode, text_size: Option<Pixels>, _cx: &mut App) -> AnyElement {
    let Some(math) = node.data::<MathRaster>() else {
        return div().child(node.as_text().to_string()).into_any_element();
    };
    let em = text_size.map_or(EM_PX, |size| size.as_f32());
    let natural_w = px((math.width_em * em).max(1.));
    let natural_h = px((math.height_em * em).max(1.));
    div()
        .w_full()
        .min_w_0()
        .flex()
        .justify_center()
        .py(px(4.))
        .debug_selector(|| "markdown-inline-math".to_string())
        .child(
            div().max_w_full().child(
                img(ImageSource::Render(math.raster.clone()))
                    .w(natural_w)
                    .h(natural_h)
                    .max_w_full()
                    .object_fit(ObjectFit::Contain),
            ),
        )
        .into_any_element()
}

/// Typeset one formula to its raster, or `None` when it is not math this
/// engine accepts (strict mode: invalid LaTeX is the text it always was).
/// The ink is applied before rasterizing — see [`INK`].
fn typeset(value: &str, renderer: &SvgRenderer) -> Option<MathRaster> {
    let output = mathra::convert(
        value,
        &mathra::Options {
            display: true,
            strict: true,
            ..mathra::Options::default()
        },
    )
    .map_err(|err| log::warn!("math does not typeset, leaving it as text: {err}"))
    .ok()?;
    let svg = with_ink(&output.svg);
    let (width_em, height_em) = em_dimensions(&svg)?;
    let box_px = svg_block::RASTER_BOX;
    // The raster's crispness target is one em at retina scale; a long
    // equation caps at the diagram box like every other picture.
    let width = (width_em * EM_PX * 2.).min(box_px);
    let raster = rasterize_at(renderer, svg.as_bytes(), width)?;
    Some(MathRaster {
        raster,
        width_em,
        height_em,
    })
}

/// Point the root's `color` at the fixed ink and drop mathra's own
/// `svg:root` style: usvg resolves `currentColor` from the `color`
/// attribute reliably; a `<style>` class rule it may not.
fn with_ink(svg: &str) -> String {
    const STYLE: &str = "<style>svg:root{color:#111}</style>";
    let Some(root) = svg.find("<svg") else {
        return svg.to_string();
    };
    let Some(tag_end) = svg[root..].find('>') else {
        return svg.to_string();
    };
    let tag_end = root + tag_end;
    let rest = &svg[tag_end..];
    let mut out = String::with_capacity(svg.len() + INK.len() + 8);
    out.push_str(&svg[..tag_end]);
    out.push_str(&format!(r#" color="{INK}""#));
    match rest.find(STYLE) {
        Some(at) => {
            out.push_str(&rest[..at]);
            out.push_str(&rest[at + STYLE.len()..]);
        }
        None => out.push_str(rest),
    }
    out
}

/// The `width="…em" height="…em"` the engine wrote on the root.
fn em_dimensions(svg: &str) -> Option<(f32, f32)> {
    let root = svg.find("<svg")?;
    let tag_end = root + svg[root..].find('>')?;
    let tag = &svg[root..tag_end];
    Some((em_attr(tag, "width")?, em_attr(tag, "height")?))
}

fn em_attr(tag: &str, name: &str) -> Option<f32> {
    let needle = format!("{name}=\"");
    let at = tag.find(&needle)?;
    let value = &tag[at + needle.len()..];
    let end = value.find('"')?;
    let (number, unit) = value[..end].split_once("em")?;
    if !unit.is_empty() {
        return None;
    }
    number.trim().parse().ok()
}

/// The math block's registration on a document's extensions: the same
/// closure pattern the diagram blocks use, the face style's text size being
/// what the formula's em box resolves against.
pub(crate) fn math_extensions(
    extensions: MarkdownExtensions,
    renderer: SvgRenderer,
    text_size: Option<Pixels>,
) -> MarkdownExtensions {
    extensions
        .block_parser(move |node, cx| math_fence(node, cx, &renderer))
        .block_renderer(BLOCK_NAME, move |node, _window, cx| {
            math_element(node, text_size, cx)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    /// A display equation typesets to a raster carrying its em dimensions.
    #[gpui::test]
    fn a_display_equation_typesets_to_a_raster(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        let math = typeset(r"\frac{a}{b} = \sqrt{c^2 + d}", &renderer).expect("typesets");
        assert!(
            math.width_em > 0. && math.height_em > 0.,
            "{} x {} em",
            math.width_em,
            math.height_em
        );
        let size = math.raster.size(0);
        assert!(size.width.0 > 0 && size.height.0 > 0);
        // The raster keeps the em proportions.
        let raster_aspect = size.width.0 as f32 / size.height.0 as f32;
        let em_aspect = math.width_em / math.height_em;
        assert!(
            (raster_aspect - em_aspect).abs() < 0.05,
            "{raster_aspect} vs {em_aspect}"
        );
    }

    /// The common constructions a chat's math carries.
    #[gpui::test]
    fn the_common_constructions_typeset(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        for tex in [
            r"x^2 + y^2 = z^2",
            r"\int_0^\infty e^{-x}\,dx = 1",
            r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            r"\sin^2\theta + \cos^2\theta = 1",
            r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
        ] {
            assert!(typeset(tex, &renderer).is_some(), "typesets: {tex}");
        }
    }

    /// Strict mode refuses invalid LaTeX: the block stays the literal text it
    /// always was — there is no failure face.
    #[gpui::test]
    fn invalid_latex_stays_text(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        assert!(typeset(r"\frac{", &renderer).is_none());
        assert!(typeset(r"\notacommand{x}", &renderer).is_none());
    }

    /// The ink rewrite touches the root's color and mathra's own style.
    #[test]
    fn the_ink_lands_on_the_root() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2em" height="1em" style="vertical-align: 0em"><style>svg:root{color:#111}</style><g fill="currentColor"/></svg>"#;
        let inked = with_ink(svg);
        assert!(inked.contains(r##"color="#111""##), "{inked}");
        assert!(
            !inked.contains("<style>"),
            "mathra's style is gone: {inked}"
        );
        assert!(
            inked.contains(r#"<g fill="currentColor"/>"#),
            "the body survives: {inked}"
        );
    }

    /// The em dimensions parse off the root the engine writes.
    #[test]
    fn em_dimensions_read_the_root() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 50" width="2.5em" height="1.25em"><g/></svg>"#;
        let (w, h) = em_dimensions(svg).expect("the root carries em dimensions");
        assert!((w - 2.5).abs() < 1e-4 && (h - 1.25).abs() < 1e-4);
        assert!(em_dimensions(r#"<svg width="10px"><g/></svg>"#).is_none());
    }
}
