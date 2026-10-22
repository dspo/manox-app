//! The app's contribution to a session's tool set: the Preview tool
//! (`preview`), mounted through `manox_agent::embedder_tools` — the
//! upstream seam a hosting app uses to add its own tools to every session
//! (the engine consults the provider at assembly and before every prompt).
//!
//! Positioning: Preview is a user-facing presentation tool, not a
//! reader. Its result is one fixed-size metadata line (resolved absolute
//! path, kind, byte size) — file content never flows back to the model,
//! so the context cost of a preview is constant regardless of file size.
//! Execution touches only filesystem metadata; the content itself is read
//! by the conversation column's renderer, on the user's side of the wire.
//!
//! Read-only and approval-free: the engine's approval gate passes a tool
//! through when `requires_approval` is false and `is_read_only` is true,
//! and a preview mutates nothing.

use std::path::PathBuf;
use std::sync::Arc;

use manox_harness::tool::{AgentTool, AgentToolResult, ToolContext, ToolError};
use steer_agent_chat_ui::preview::{self, PreviewOutcome};
use tokio_util::sync::CancellationToken;

/// The Preview tool. One instance serves every session; it carries no
/// per-session state (the input names the file, the context names the cwd).
pub struct PreviewTool {
    name: String,
}

impl PreviewTool {
    pub fn new() -> Self {
        Self {
            // Derived through the upstream sanitizer so the wire name stays
            // in lockstep with the engine's mount-time derivation
            // (steer_agent_chat_ui::preview::TOOL_NAME asserts the
            // equality).
            name: manox_agent::embedder_tools::embedder_tool_name("preview"),
        }
    }
}

impl Default for PreviewTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AgentTool for PreviewTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Render a local file as a rich preview card in the conversation for \
         the user. Supports Markdown, SVG, and common image formats (PNG, \
         JPEG, GIF, WebP, BMP, TIFF, ICO). Use this to SHOW a file to the \
         user; it never returns file contents — the result is only a one-line \
         confirmation with the resolved path and size. To read a file's \
         contents into your own context, use Read instead. Read-only."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        // Hand-written rather than schemars-derived: one string field, and a
        // literal schema keeps the crate's dependency surface unchanged.
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description":
                        "Absolute or cwd-relative path of the file to \
                         preview for the user",
                }
            },
            "required": ["path"],
            "additionalProperties": false,
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: CancellationToken,
        ctx: &dyn ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        if signal.is_cancelled() {
            return Err(ToolError::Aborted);
        }
        let input = preview::parse_input(&params).ok_or_else(|| {
            ToolError::InvalidArguments("expected {\"path\": string}".to_string())
        })?;
        let raw = PathBuf::from(&input.path);
        let path = if raw.is_absolute() {
            raw
        } else {
            ctx.cwd().join(raw)
        };
        // Metadata only from here on: existence, regular-file, size. The
        // content read happens in the renderer, never on the model's side.
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(_) => {
                return Ok(AgentToolResult::error(format!(
                    "path not found: {}",
                    path.display()
                )));
            }
        };
        if !metadata.is_file() {
            return Ok(AgentToolResult::error(format!(
                "not a regular file: {}",
                path.display()
            )));
        }
        let Some(kind) = preview::kind_for_path(&path) else {
            return Ok(AgentToolResult::error(format!(
                "unsupported preview type (supported: {}): {}",
                preview::supported_extensions(),
                path.display()
            )));
        };
        let bytes = metadata.len();
        if bytes > kind.byte_cap() {
            return Ok(AgentToolResult::error(format!(
                "file too large to preview ({} bytes > {} cap): {}",
                bytes,
                kind.byte_cap(),
                path.display()
            )));
        }
        Ok(AgentToolResult::text(preview::encode_outcome(
            &PreviewOutcome { path, kind, bytes },
        )))
    }
}

/// The process-wide embedder provider: the Preview tool for every session,
/// chained over whatever provider owned the slot when [`install`] ran.
///
/// Chaining is not optional politeness: the `AgentServer` constructor
/// claims the slot itself (`agent_server::install_embedder_provider` —
/// last-wins by design), so an install that merely overwrites would either
/// be clobbered by the server's construction or, if it won the race,
/// silently kill the `RegisterSessionTools` path the server's provider
/// serves. The chain keeps both surfaces alive.
struct ChainedPreviewProvider {
    fallback: Option<Arc<dyn manox_agent::embedder_tools::EmbedderToolProvider>>,
}

impl manox_agent::embedder_tools::EmbedderToolProvider for ChainedPreviewProvider {
    fn tools_for(&self, session_id: &str) -> Vec<Arc<dyn AgentTool>> {
        let mut tools = vec![Arc::new(PreviewTool::new()) as Arc<dyn AgentTool>];
        if let Some(fallback) = &self.fallback {
            tools.extend(fallback.tools_for(session_id));
        }
        tools
    }
}

/// Register the Preview tool with the engine's embedder seam.
///
/// MUST run after `agent_server::global(...)` — the server's construction
/// claims the process-wide slot last-wins, and an earlier install would be
/// clobbered (or, once-guarded, would burn the guard against a provider-less
/// world). `Workspace::new` calls this immediately after the server global;
/// the `Once` keeps later windows from stacking duplicate chains. The engine
/// re-consults the provider at session assembly and before every prompt, so
/// the registration may land before or after individual sessions exist.
pub fn install() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let fallback = manox_agent::embedder_tools::provider();
        manox_agent::embedder_tools::set_provider(Arc::new(ChainedPreviewProvider { fallback }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use manox_agent::embedder_tools::EmbedderToolProvider as _;
    use std::path::Path;

    /// A minimal `ToolContext`: the preview tool touches only the cwd, so
    /// the env and tool-state halves are unreachable by construction.
    struct CwdCtx {
        cwd: PathBuf,
    }

    impl ToolContext for CwdCtx {
        fn env(&self) -> &dyn manox_harness::env::ExecutionEnv {
            unreachable!("the preview tool never touches the env")
        }
        fn cwd(&self) -> &std::path::Path {
            &self.cwd
        }
        fn tool_state(&self) -> &manox_harness::tool::ToolState {
            unreachable!("the preview tool never touches the tool state")
        }
    }

    fn run(
        tool: &PreviewTool,
        params: serde_json::Value,
        cwd: &std::path::Path,
    ) -> AgentToolResult {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            tool.execute(
                "tc-test",
                params,
                CancellationToken::new(),
                &CwdCtx {
                    cwd: cwd.to_path_buf(),
                },
            )
            .await
            .expect("execute returns Ok for every tool-level outcome")
        })
    }

    fn text_of(result: &AgentToolResult) -> String {
        match result.content.first() {
            Some(manox_harness::types::ContentBlock::Text { text, .. }) => text.clone(),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    #[test]
    fn tool_identity() {
        let tool = PreviewTool::new();
        assert_eq!(tool.name(), preview::TOOL_NAME);
        assert!(tool.is_read_only());
        assert!(!tool.requires_approval(&serde_json::json!({})));
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"][0], "path");
    }

    #[test]
    fn chained_provider_adds_preview_over_the_fallback() {
        struct TwoStubTools;
        impl manox_agent::embedder_tools::EmbedderToolProvider for TwoStubTools {
            fn tools_for(&self, _session_id: &str) -> Vec<Arc<dyn AgentTool>> {
                vec![
                    Arc::new(PreviewTool::new()) as Arc<dyn AgentTool>,
                    Arc::new(PreviewTool::new()) as Arc<dyn AgentTool>,
                ]
            }
        }
        // No fallback: exactly our tool.
        let bare = ChainedPreviewProvider { fallback: None };
        assert_eq!(bare.tools_for("s").len(), 1);
        assert_eq!(bare.tools_for("s")[0].name(), preview::TOOL_NAME);
        // With a fallback: ours first, then everything the fallback serves
        // (the RegisterSessionTools surface keeps its tools).
        let chained = ChainedPreviewProvider {
            fallback: Some(Arc::new(TwoStubTools)),
        };
        let tools = chained.tools_for("s");
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name(), preview::TOOL_NAME);
    }

    /// The install contract under the process-global slot: it chains
    /// whatever provider is live (the AgentServer's, in production) and the
    /// `Once` guard keeps a second install from stacking duplicates. Runs
    /// both halves in ONE test — the slot is process-global, and the
    /// absent/present halves must not race each other across parallel test
    /// threads (the same shape as upstream's provider-gate test).
    #[test]
    fn install_chains_the_live_provider_and_fires_once() {
        manox_agent::embedder_tools::drop_provider_for_test();

        install();
        let first = manox_agent::embedder_tools::provider().expect("install claims the slot");
        let tools = first.tools_for("any-session");
        assert_eq!(tools.len(), 1, "no prior provider → only the preview tool");
        assert_eq!(tools[0].name(), preview::TOOL_NAME);

        // A second install (a second window) is a no-op — the live provider
        // keeps serving exactly one preview tool.
        install();
        let second = manox_agent::embedder_tools::provider().expect("the slot stays claimed");
        assert_eq!(second.tools_for("any-session").len(), 1);

        manox_agent::embedder_tools::drop_provider_for_test();
    }

    #[test]
    fn invalid_arguments() {
        let tool = PreviewTool::new();
        let err = {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                tool.execute(
                    "tc-test",
                    serde_json::json!({"path": 3}),
                    CancellationToken::new(),
                    &CwdCtx {
                        cwd: PathBuf::from("/"),
                    },
                )
                .await
                .unwrap_err()
            })
        };
        assert!(matches!(err, ToolError::InvalidArguments(_)), "got {err:?}");
    }

    #[test]
    fn missing_path_is_an_error_result() {
        let tool = PreviewTool::new();
        let result = run(
            &tool,
            serde_json::json!({"path": "no-such-file.md"}),
            Path::new("/"),
        );
        assert!(result.is_error);
        let text = text_of(&result);
        assert!(text.contains("path not found"), "got {text}");
    }

    #[test]
    fn unsupported_extension_is_an_error_result() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.pdf");
        std::fs::write(&file, b"%PDF-1.4").unwrap();
        let tool = PreviewTool::new();
        let result = run(&tool, serde_json::json!({"path": file}), dir.path());
        assert!(result.is_error);
        let text = text_of(&result);
        assert!(text.contains("unsupported preview type"), "got {text}");
    }

    #[test]
    fn oversize_markdown_is_an_error_result() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.md");
        std::fs::write(
            &file,
            vec![b'x'; (preview::MAX_MARKDOWN_BYTES + 1) as usize],
        )
        .unwrap();
        let tool = PreviewTool::new();
        let result = run(&tool, serde_json::json!({"path": file}), dir.path());
        assert!(result.is_error);
        let text = text_of(&result);
        assert!(text.contains("file too large"), "got {text}");
    }

    #[test]
    fn success_resolves_relative_paths_and_reports_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("guide.md");
        std::fs::write(&file, b"# Guide\n\nbody text").unwrap();
        let tool = PreviewTool::new();

        // Relative input: resolved against the context cwd.
        let result = run(&tool, serde_json::json!({"path": "guide.md"}), dir.path());
        assert!(!result.is_error);
        let text = text_of(&result);

        // Context discipline: the payload is the metadata line, never the
        // content — bounded length, and the body text is absent.
        assert!(text.len() < 256, "got {text}");
        assert!(!text.contains("body text"), "content leaked: {text}");
        let outcome = preview::parse_outcome(&text).expect("outcome decodes");
        assert_eq!(outcome.path, file);
        assert_eq!(outcome.kind, preview::PreviewKind::Markdown);
        assert_eq!(outcome.bytes, 18);
    }

    #[test]
    fn image_kind_classifies_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shot.png");
        std::fs::write(&file, b"\x89PNG-not-really").unwrap();
        let tool = PreviewTool::new();
        let result = run(&tool, serde_json::json!({"path": file}), dir.path());
        assert!(!result.is_error);
        let outcome = preview::parse_outcome(&text_of(&result)).expect("outcome decodes");
        assert_eq!(outcome.kind, preview::PreviewKind::Image);
    }
}
