//! LaTeX math in a message, in both shapes a model writes it: a `$$…$$` line
//! (a display equation) and a `$…$` inside a sentence (an inline run). Both
//! typeset to a standalone SVG document by [`mathra`] — an embedded pure-Rust
//! engine drawn with the STIX Two Math font's MATH table (fractions, radicals,
//! scripts, matrices, delimiters), glyphs as path outlines — and both ride the
//! same [`svg_block::rasterize`] path the diagram fences paint through.
//!
//! The markdown stack does not hand these over as one node. A `$$` fence alone
//! on its line is `mdast::Node::Math`; a `$$…$$` pair written on one line — the
//! form models actually emit — is `Node::InlineMath` inside a paragraph, and
//! so is `$…$`. The block therefore claims three places:
//!
//! - `Node::Math` → the display block ([`math_fence`]).
//! - A paragraph that is nothing but a `$$…$$` pair → the same display block
//!   ([`math_paragraph`]); the pair was a display equation to every author who
//!   wrote it on a line of its own, whatever the parser called it.
//! - Any remaining `InlineMath` → an inline run on the surrounding line's own
//!   typography, baseline included ([`InlineMath`]).
//!
//! Unclaimed math already renders as its literal text (upstream keeps
//! unclaimed math literal), so there is no failure face to design: a formula
//! that does not typeset — strict mode refuses invalid LaTeX — is the text it
//! always was. Both forms keep the original dollars source as their text form,
//! so whole-document copy and the markdown round-trip keep it.
//!
//! The engine's bundled font carries no CJK glyphs, so a `\text{…}` holding
//! Chinese refuses in strict mode and that formula stays its source; drawing
//! it wants a CJK-capable math font inside the engine, not another raster
//! here.

use std::sync::Arc;

use gpui::{
    AnyElement, App, ImageSource, InteractiveElement as _, IntoElement as _, ObjectFit,
    ParentElement as _, Pixels, RenderImage, Styled as _, StyledImage as _, SvgRenderer, Window,
    div, img, px,
};
use gpui_base::text::{
    InlineElement, InlineRenderContext, MarkdownExtensions, MarkdownNode, MarkdownParseContext,
    MarkdownPlugin, markdown_ast,
};

use crate::markdown::svg_block::{self, MAX_SOURCE_BYTES, rasterize_at};

/// Custom block name for one typeset equation — the whole contract between
/// the parse and render halves registered in [`svg_block::extensions`].
const BLOCK_NAME: &str = "math";

/// The inline run's node name; a plugin's name keys its renderer too.
const RUN_NAME: &str = "math-run";

/// The engine's ink. mathra writes `currentColor` and falls back to a fixed
/// dark ink for standalone documents; the raster IS a standalone document, so
/// the same fixed dark ink is applied as an explicit root `color` (the chat
/// surface is dark ink on light chrome; a theme-aware re-ink is a re-raster
/// and lands with one).
const INK: &str = "#111";

/// The nominal em box, in CSS pixels, the formula's `…em` dimensions are
/// rasterized against: the raster's crispness target (two pixels per em px).
/// The element's natural size comes from the line it sits in, not from this.
const EM_PX: f32 = 16.;

/// The selector the block's element carries, for the rendered-block regressions.
const BLOCK_SELECTOR: &str = "markdown-math-block";

/// The selector the inline run's element carries, for the same regressions.
const RUN_SELECTOR: &str = "markdown-math-run";

/// Which dollars pair wrapped a formula.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Delimiter {
    /// `$…$` — one equation inside the line's own box.
    Inline,
    /// `$$…$$` — display layout (limits above and below, display fractions).
    Display,
}

impl Delimiter {
    /// The delimiter a math node's source carries, or `None` for source this
    /// module does not claim.
    ///
    /// The single-dollar form is claimed only when it sits tight against its
    /// dollars (`$x$`, not `$5 and $10`): the parser cannot tell currency from
    /// math, and claiming the loose form would typeset prose as a formula. The
    /// double-dollar form is an explicit display signal and is always claimed.
    fn of(source: &str) -> Option<Self> {
        if let Some(body) = source
            .strip_prefix("$$")
            .and_then(|rest| rest.strip_suffix("$$"))
        {
            return (!body.is_empty()).then_some(Self::Display);
        }
        let body = source.strip_prefix('$')?.strip_suffix('$')?;
        if body.is_empty()
            || body.starts_with(char::is_whitespace)
            || body.ends_with(char::is_whitespace)
        {
            return None;
        }
        Some(Self::Inline)
    }

    fn display(self) -> bool {
        matches!(self, Self::Display)
    }
}

/// One typeset formula: the raster plus the em dimensions the SVG declared
/// (`width="…em" height="…em"`), which set the element's natural display size
/// at the document's own text size.
#[derive(Clone)]
struct MathRaster {
    raster: Arc<RenderImage>,
    width_em: f32,
    height_em: f32,
    /// How far the formula hangs below the alphabetic baseline, in em (the
    /// root's `vertical-align`). The inline run places the picture by it; the
    /// block centers without it.
    depth_em: f32,
}

/// Turn an `mdast::Node::Math` — a `$$` fence alone on its line — into the
/// block, or `None` to leave it the literal text the unclaimed path renders.
/// The original `$$…$$` span is the text form, exactly as fences keep their
/// delimiters.
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
    let raster = typeset(&math.value, Delimiter::Display, renderer)?;
    Some(
        MarkdownNode::new(BLOCK_NAME, raster)
            .text(source.clone())
            .markdown(source),
    )
}

/// Turn a paragraph that is nothing but a `$$…$$` pair into the display block.
/// The parser calls a one-line pair inside a paragraph inline math; a line
/// holding only that pair is a display equation to every author who wrote it.
/// The paragraph's own source is the text form.
fn math_paragraph(
    node: &markdown_ast::Node,
    cx: &MarkdownParseContext<'_>,
    renderer: &SvgRenderer,
) -> Option<MarkdownNode> {
    let markdown_ast::Node::Paragraph(paragraph) = node else {
        return None;
    };
    let [markdown_ast::Node::InlineMath(math)] = paragraph.children.as_slice() else {
        return None;
    };
    if math.value.len() > MAX_SOURCE_BYTES {
        return None;
    }
    // A paragraph on a list item's own line keeps its marker beside the
    // content: promoting there would strand the marker alone on the line, so
    // the pair falls through to the inline run instead.
    if !opens_its_line(cx, node) {
        return None;
    }
    let source = cx.node_source(node)?.trim();
    if Delimiter::of(source)? != Delimiter::Display {
        return None;
    }
    let raster = typeset(&math.value, Delimiter::Display, renderer)?;
    Some(
        MarkdownNode::new(BLOCK_NAME, raster)
            .text(source.to_string())
            .markdown(source.to_string()),
    )
}

/// Whether the node starts its source line — nothing but indentation before
/// it. A list item's paragraph shares its line with the marker; a blockquote's
/// with `>`.
fn opens_its_line(cx: &MarkdownParseContext<'_>, node: &markdown_ast::Node) -> bool {
    let Some(start) = node.position().map(|position| position.start.offset) else {
        return false;
    };
    let source = cx.source();
    let line_start = source[..start].rfind('\n').map_or(0, |at| at + 1);
    source[line_start..start].trim().is_empty()
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
        .debug_selector(|| BLOCK_SELECTOR.to_string())
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

/// The inline half: any `$…$` or `$$…$$` the block parsers did not take, as a
/// run on the line's own font size and baseline. A plugin rather than a block
/// parser, because the run has to inherit the line's typography and the
/// upstream layout owns the baseline slot ([`InlineElement::with_baseline`]).
struct InlineMath {
    renderer: SvgRenderer,
}

impl MarkdownPlugin for InlineMath {
    fn name(&self) -> &str {
        RUN_NAME
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        cx: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::InlineMath(math) = node else {
            return None;
        };
        if math.value.len() > MAX_SOURCE_BYTES {
            return None;
        }
        let delimiter = Delimiter::of(cx.node_source(node)?.trim())?;
        let raster = typeset(&math.value, delimiter, &self.renderer)?;
        Some(MarkdownNode::new(RUN_NAME, raster))
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<InlineElement> {
        let math = node.data::<MathRaster>()?;
        let em = context.font_size().as_f32();
        let width = px((math.width_em * em).max(1.));
        let height = px((math.height_em * em).max(1.));
        // The SVG's top edge sits `height - depth` above the baseline; the
        // layout wants that distance from the top. Upstream measures the box
        // on the device-pixel grid but takes this baseline at full precision,
        // so a formula with no descent — baseline on the box's bottom edge —
        // fails its `baseline <= height` check and paints its literal dollars
        // instead; keep it a pixel inside.
        let baseline = px((math.height_em - math.depth_em).clamp(0., math.height_em) * em)
            .clamp(px(0.), height - px(1.));
        let element = div().debug_selector(|| RUN_SELECTOR.to_string()).child(
            img(ImageSource::Render(math.raster.clone()))
                .w(width)
                .h(height),
        );
        Some(InlineElement::new(element).with_baseline(baseline))
    }
}

/// Typeset one formula to its raster, or `None` when it is not math this
/// engine accepts (strict mode: invalid LaTeX is the text it always was).
/// The ink is applied before rasterizing — see [`INK`].
fn typeset(value: &str, delimiter: Delimiter, renderer: &SvgRenderer) -> Option<MathRaster> {
    let output = mathra::convert(
        value,
        &mathra::Options {
            display: delimiter.display(),
            strict: true,
            ..mathra::Options::default()
        },
    )
    .map_err(|err| log::warn!("math does not typeset, leaving it as text: {err}"))
    .ok()?;
    let svg = with_ink(&output.svg);
    let (width_em, height_em) = em_dimensions(&svg)?;
    let depth_em = baseline_depth(&svg).unwrap_or(0.);
    // The raster's crispness target is one em at retina scale; a long
    // equation caps at the diagram box like every other picture.
    let width = (width_em * EM_PX * 2.).min(svg_block::RASTER_BOX);
    let raster = rasterize_at(renderer, svg.as_bytes(), width)?;
    Some(MathRaster {
        raster,
        width_em,
        height_em,
        depth_em,
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

/// The root element's tag: the engine writes the formula's em box and its
/// baseline there.
fn root_tag(svg: &str) -> Option<&str> {
    let root = svg.find("<svg")?;
    let end = root + svg[root..].find('>')?;
    Some(&svg[root..end])
}

/// The `width="…em" height="…em"` the engine wrote on the root.
fn em_dimensions(svg: &str) -> Option<(f32, f32)> {
    let tag = root_tag(svg)?;
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

/// The depth the formula hangs below the alphabetic baseline, in em, from the
/// root's `style="vertical-align: -0.887em"` (negative is below the baseline).
fn baseline_depth(svg: &str) -> Option<f32> {
    let tag = root_tag(svg)?;
    let at = tag.find("vertical-align:")?;
    let rest = tag[at + "vertical-align:".len()..].trim_start();
    let end = rest.find("em")?;
    rest[..end].trim().parse::<f32>().ok().map(|value| -value)
}

/// The math registrations on a document's extensions: the display block (both
/// the fence and the line that holds one pair) plus the inline run, the face
/// style's text size being what the block's em box resolves against.
pub(crate) fn math_extensions(
    extensions: MarkdownExtensions,
    renderer: SvgRenderer,
    text_size: Option<Pixels>,
) -> MarkdownExtensions {
    let block_renderer = renderer.clone();
    extensions
        .block_parser(move |node, cx| {
            math_fence(node, cx, &renderer).or_else(|| math_paragraph(node, cx, &renderer))
        })
        .block_renderer(BLOCK_NAME, move |node, _window, cx| {
            math_element(node, text_size, cx)
        })
        .plugin(InlineMath {
            renderer: block_renderer,
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
        let math = typeset(
            r"\frac{a}{b} = \sqrt{c^2 + d}",
            Delimiter::Display,
            &renderer,
        )
        .expect("typesets");
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
        // The inline run places the picture by this; the engine writes it.
        assert!(math.depth_em >= 0., "depth {} em", math.depth_em);
    }

    /// The common constructions a chat's math carries, in both layouts.
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
            for delimiter in [Delimiter::Inline, Delimiter::Display] {
                assert!(
                    typeset(tex, delimiter, &renderer).is_some(),
                    "{delimiter:?} typesets: {tex}"
                );
            }
        }
    }

    /// The two layouts are the engine's own styles, not the same drawing.
    #[gpui::test]
    fn inline_and_display_layout_differ(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        let tex = r"\sum_{i=1}^{n} i";
        let inline = typeset(tex, Delimiter::Inline, &renderer).expect("typesets");
        let display = typeset(tex, Delimiter::Display, &renderer).expect("typesets");
        assert!(
            (inline.height_em - display.height_em).abs() > 1e-3
                || (inline.width_em - display.width_em).abs() > 1e-3,
            "inline {}x{} em vs display {}x{} em",
            inline.width_em,
            inline.height_em,
            display.width_em,
            display.height_em
        );
    }

    /// Strict mode refuses invalid LaTeX: the node stays the literal text it
    /// always was — there is no failure face.
    #[gpui::test]
    fn invalid_latex_stays_text(cx: &mut TestAppContext) {
        let renderer = cx.update(|cx| cx.svg_renderer());
        assert!(typeset(r"\frac{", Delimiter::Display, &renderer).is_none());
        assert!(typeset(r"\notacommand{x}", Delimiter::Display, &renderer).is_none());
        assert!(typeset(r"\frac{", Delimiter::Inline, &renderer).is_none());
    }

    /// The source names its delimiter, and currency is not math.
    #[test]
    fn the_source_names_its_delimiter() {
        assert_eq!(Delimiter::of("$$x$$"), Some(Delimiter::Display));
        assert_eq!(Delimiter::of("$x$"), Some(Delimiter::Inline));
        assert_eq!(Delimiter::of("$a + b$"), Some(Delimiter::Inline));
        // The loose pairs a parser cannot tell from currency stay prose.
        assert_eq!(Delimiter::of("$5 and $"), None);
        assert_eq!(Delimiter::of("$ x $"), None);
        assert_eq!(Delimiter::of("$$$$"), None);
        assert_eq!(Delimiter::of("plain"), None);
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

    /// The baseline the inline run hangs from is the root's vertical-align.
    #[test]
    fn the_baseline_depth_reads_the_root() {
        let svg = r#"<svg width="2em" height="1em" style="vertical-align: -0.25em"><g/></svg>"#;
        assert_eq!(baseline_depth(svg), Some(0.25));
        assert_eq!(
            baseline_depth(r#"<svg width="2em" height="1em"><g/></svg>"#),
            None
        );
    }
}
