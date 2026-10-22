//! The Preview tool's shared vocabulary — the single source of truth for
//! both halves of the app-side `preview` tool (mounted through
//! `manox_agent::embedder_tools`): the executing side in agent-ui
//! (`preview_host`) and the rendering side in the conversation column.
//!
//! Positioning, which drives every choice below: Preview is the app's
//! user-facing presentation tool — the model asks the app to SHOW a local
//! file to the user. File content never flows back to the model; the
//! model-visible payload is one fixed-size metadata line no matter how
//! large the previewed file is (reading file content into the model's own
//! context is `Read`'s job, not this tool's).

use std::path::{Path, PathBuf};

/// Model-facing tool name: the sanitized form of the registration name, with
/// no namespace prefix (the engine tells its own tools, embedder tools and
/// MCP tools apart by shape), so it must stay in lockstep with
/// `manox_agent::embedder_tools::embedder_tool_name`.
pub const TOOL_NAME: &str = "preview";

/// Render-side decode budgets, keyed by preview kind. These bound what the
/// UI is willing to read and rasterize in one preview; they have nothing to
/// do with context size (no content ever reaches the model).
pub const MAX_MARKDOWN_BYTES: u64 = 2 * 1024 * 1024;
/// The rasterizer's own source budget, shared with the inline ```svg block —
/// the file this card previews and a fence in a message feed the same renderer.
pub const MAX_SVG_BYTES: u64 = steer_components::markdown::svg_block::MAX_SOURCE_BYTES as u64;
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// What the renderer does with a previewed file. The set matches what the
/// GPUI stack can actually present: the markdown `ChatDocument` pipeline,
/// the static svg rasterizer, and the image loader's format features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Markdown,
    Svg,
    Image,
}

impl PreviewKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewKind::Markdown => "markdown",
            PreviewKind::Svg => "svg",
            PreviewKind::Image => "image",
        }
    }

    /// The render-side byte budget for this kind.
    pub fn byte_cap(self) -> u64 {
        match self {
            PreviewKind::Markdown => MAX_MARKDOWN_BYTES,
            PreviewKind::Svg => MAX_SVG_BYTES,
            PreviewKind::Image => MAX_IMAGE_BYTES,
        }
    }
}

/// The tool's input parameters. Kept in one place so the executing side's
/// hand-written JSON Schema and the renderer's parse never drift apart.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewInput {
    pub path: String,
}

pub fn parse_input(value: &serde_json::Value) -> Option<PreviewInput> {
    let path = value.get("path")?.as_str()?;
    if path.is_empty() {
        return None;
    }
    Some(PreviewInput {
        path: path.to_string(),
    })
}

/// Classify a path by extension (ASCII-case-insensitive). `None` means the
/// type is not previewable and the caller owes the user an actionable error.
pub fn kind_for_path(path: &Path) -> Option<PreviewKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "md" | "markdown" | "mdx" => PreviewKind::Markdown,
        "svg" => PreviewKind::Svg,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif" | "tga" => {
            PreviewKind::Image
        }
        _ => return None,
    })
}

/// The extension list for error messages, so the model and the user see the
/// same supported set the sniffer actually accepts.
pub fn supported_extensions() -> &'static str {
    "md, markdown, mdx, svg, png, jpg, jpeg, gif, webp, bmp, ico, tiff, tga"
}

/// The tool result — the ONLY model-visible payload: one JSON line of
/// metadata (resolved absolute path, kind, byte size). Never content.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewOutcome {
    pub path: PathBuf,
    pub kind: PreviewKind,
    pub bytes: u64,
}

pub fn encode_outcome(outcome: &PreviewOutcome) -> String {
    serde_json::json!({
        "path": outcome.path.display().to_string(),
        "kind": outcome.kind.as_str(),
        "bytes": outcome.bytes,
    })
    .to_string()
}

/// Decode a successful tool result. Used by the renderer to learn the
/// resolved path (the tool resolved relative inputs against the session
/// cwd; the renderer cannot redo that resolution on its own).
pub fn parse_outcome(text: &str) -> Option<PreviewOutcome> {
    let value: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let path = value.get("path")?.as_str()?;
    let kind = value.get("kind")?.as_str()?;
    let bytes = value.get("bytes")?.as_u64()?;
    let kind = match kind {
        "markdown" => PreviewKind::Markdown,
        "svg" => PreviewKind::Svg,
        "image" => PreviewKind::Image,
        _ => return None,
    };
    Some(PreviewOutcome {
        path: PathBuf::from(path),
        kind,
        bytes,
    })
}

/// The renderer's view of a preview call: the tool's outcome line when it
/// parsed, else the input path re-sniffed by extension (rows whose output
/// never carried the outcome encoding still preview — display only reads
/// the file, so re-deriving the kind from the input is safe).
pub fn resolved_outcome(output: &str, input: &serde_json::Value) -> Option<PreviewOutcome> {
    if let Some(outcome) = parse_outcome(output) {
        return Some(outcome);
    }
    let parsed = parse_input(input)?;
    let path = PathBuf::from(parsed.path);
    let kind = kind_for_path(&path)?;
    Some(PreviewOutcome {
        path,
        kind,
        bytes: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The engine mounts the sanitized registration name and replaces the
    // embedder set wholesale; if upstream ever changes the sanitizing rule
    // this fails before the mismatch can ship.
    #[test]
    fn tool_name_matches_upstream_sanitizing() {
        assert_eq!(
            TOOL_NAME,
            manox_agent::embedder_tools::embedder_tool_name("preview")
        );
    }

    #[test]
    fn kind_table() {
        let k = |p: &str| kind_for_path(Path::new(p));
        assert_eq!(k("a.md"), Some(PreviewKind::Markdown));
        assert_eq!(k("a.MARKDOWN"), Some(PreviewKind::Markdown));
        assert_eq!(k("a.mdx"), Some(PreviewKind::Markdown));
        assert_eq!(k("dir/a.svg"), Some(PreviewKind::Svg));
        assert_eq!(k("a.PNG"), Some(PreviewKind::Image));
        assert_eq!(k("a.jpeg"), Some(PreviewKind::Image));
        assert_eq!(k("a.webp"), Some(PreviewKind::Image));
        assert_eq!(k("a.tiff"), Some(PreviewKind::Image));
        assert_eq!(k("a.pdf"), None);
        assert_eq!(k("noext"), None);
        assert_eq!(k(""), None);
    }

    #[test]
    fn outcome_round_trip() {
        let outcome = PreviewOutcome {
            path: PathBuf::from("/tmp/some dir/a.md"),
            kind: PreviewKind::Markdown,
            bytes: 1234,
        };
        assert_eq!(parse_outcome(&encode_outcome(&outcome)), Some(outcome));
        assert_eq!(parse_outcome("not json"), None);
        assert_eq!(
            parse_outcome(r#"{"path":"x","kind":"pdf","bytes":1}"#),
            None
        );
    }

    // Context discipline, stated as a test: the encoded payload is a few
    // dozen bytes regardless of the previewed file's actual size.
    #[test]
    fn encoded_outcome_is_bounded() {
        let outcome = PreviewOutcome {
            path: PathBuf::from("/very/long/path/with/many/segments/file.png"),
            kind: PreviewKind::Image,
            bytes: u64::MAX,
        };
        assert!(encode_outcome(&outcome).len() < 128);
    }

    #[test]
    fn input_parsing() {
        assert_eq!(
            parse_input(&serde_json::json!({"path": "docs/a.md"})),
            Some(PreviewInput {
                path: "docs/a.md".to_string()
            })
        );
        assert_eq!(parse_input(&serde_json::json!({"path": ""})), None);
        assert_eq!(parse_input(&serde_json::json!({})), None);
        assert_eq!(parse_input(&serde_json::json!({"path": 3})), None);
    }
}
