//! Visual acceptance for the tool-card designs (harness=false, main thread):
//! gpui's official offscreen render — `VisualTestAppContext` + Metal texture
//! readback — of the three gallery pages' static cards, no screen-recording
//! permission needed.
//!
//! Run with `GALLERY_SHOT=/tmp/tools.png GALLERY_PAGE=text cargo test -p
//! ai-elements --test visual` (pages: `text`, `shell`, `ask`, `code`, `code2`, `svg`; default
//! `text`). The samples mirror the gallery example's demo content — leaner,
//! one card per family shape, enough to judge the design's pixels. macOS
//! only: the readback is a Metal texture; the harness's imports exist only
//! on that platform.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("visual: the gallery harness is macOS-only (Metal readback), skipping");
}

#[cfg(target_os = "macos")]
fn tall_rust_block() -> String {
    let mut out = String::from("```rust\nmatch event {\n");
    for ix in 0..32 {
        out.push_str(&format!(
            "    Event::Kind{ix:02} {{ payload }} => fold(payload, Acc::from(ix)),\n"
        ));
    }
    out.push_str("    Event::Flush => acc.into_result(),\n}\n```\n");
    out
}

#[cfg(target_os = "macos")]
fn main() {
    use std::sync::Arc;
    use std::time::Duration;

    use ai_elements::{
        AskBlock, AskOption, AskState, AskUserQuestion, CodeLine, CodeView, ShellLine,
        ShellSpanKind, ShellView, Tool, ToolState, ToolStatus,
    };
    use gpui::{
        AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
        SharedString, Styled as _, VisualTestAppContext, Window, div, px, size,
    };
    use gpui_component::{Icon, Root, Sizable as _, Theme, h_flex, v_flex};
    use gpui_kit_assets::IconName;
    use steer_components::markdown::static_document;

    let Ok(path) = std::env::var("GALLERY_SHOT") else {
        eprintln!("visual: GALLERY_SHOT not set, skipping");
        return;
    };
    let page = match std::env::var("GALLERY_PAGE").as_deref() {
        Ok("shell") => PageKind::Shell,
        Ok("ask") => PageKind::Ask,
        Ok("code") => PageKind::Code,
        Ok("code2") => PageKind::CodeMore,
        Ok("svg") => PageKind::Svg,
        _ => PageKind::Text,
    };

    let platform = gpui_platform::current_platform(false);
    let mut cx =
        VisualTestAppContext::with_asset_source(platform, Arc::new(gpui_kit_assets::AllAssets));
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let view = cx
        .open_offscreen_window(size(px(920.), px(1400.)), |window, cx| {
            let page = cx.new(|cx| Page::new(page, window, cx));
            cx.new(|cx| Root::new(page, window, cx))
        })
        .expect("offscreen window");

    // Let the font/layout pass settle; components that ask for one more
    // frame need the explicit pumps (tests have no platform frame loop).
    for i in 0..5 {
        if i > 0 {
            cx.update_window(view.into(), |_view, window, cx| {
                window.simulate_next_frame(cx);
            })
            .expect("window");
        }
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(100));
    }

    let shot = cx
        .capture_screenshot(view.into())
        .expect("capture screenshot");
    shot.save(&path).expect("save png");
    println!("visual: wrote {path} ({}x{})", shot.width(), shot.height());

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum PageKind {
        Text,
        Shell,
        Ask,
        Code,
        CodeMore,
        Svg,
    }

    struct Page {
        kind: PageKind,
        states: Vec<Entity<ToolState>>,
        ask_settled: Option<Entity<AskState>>,
        ask_fresh: Option<Entity<AskState>>,
    }

    impl Page {
        fn new(kind: PageKind, _window: &mut Window, cx: &mut Context<Self>) -> Self {
            // One settled-and-open state per card, as a replayed transcript
            // mounts them.
            let mut states = Vec::new();
            let card_count = match kind {
                PageKind::Text => 4,
                PageKind::Shell => 3,
                PageKind::Ask | PageKind::Code | PageKind::CodeMore | PageKind::Svg => 0,
            };
            for _ in 0..card_count {
                let state = cx.new(|cx| ToolState::new(cx).with_status(ToolStatus::Succeeded, cx));
                state.update(cx, |state, cx| {
                    state.set_duration(Some(Duration::from_millis(340)), cx);
                    state.set_open(true, cx);
                });
                states.push(state);
            }
            // The settled ask: one choice on the first question, the second
            // skipped — the markdown has to carry both. Beside it, a fresh
            // wizard on its first page — the form's own pixels.
            let (ask_settled, ask_fresh) = if kind == PageKind::Ask {
                let settled = cx.new(|cx| {
                    let mut state = AskState::new(vec![false, false], "补充说明…（可选）", cx);
                    state.toggle_option(0, 0, cx);
                    state.submit(cx);
                    state
                });
                let fresh = cx.new(|cx| AskState::new(vec![false, true], "补充说明…（可选）", cx));
                fresh.update(cx, |state, cx| {
                    state.toggle_option(0, 0, cx);
                    // Park on the last page: the pager's forward edge is the
                    // submit check, and that is the pixel under test.
                    state.set_page(1, cx);
                });
                (Some(settled), Some(fresh))
            } else {
                (None, None)
            };
            Self {
                kind,
                states,
                ask_settled,
                ask_fresh,
            }
        }

        fn rows(&self, cx: &App) -> Vec<AnyElement> {
            let theme = Theme::global(cx);
            let foreground = theme.foreground;
            let mono = theme.mono_font_family.clone();
            let cmd_args = |cmd: &'static str| {
                h_flex()
                    .min_w_0()
                    .overflow_hidden()
                    .font_family(mono.clone())
                    .text_xs()
                    .child(div().text_color(foreground).child(cmd))
            };
            // The prompt this machine prints: path + git branch + dirt, then
            // the toolchain line — the dev box's zshrc, pinned for the shot.
            let prompt = |shell: ShellView| {
                shell
                    .context(ShellLine::spans(vec![
                        (ShellSpanKind::Path, "~/projects/dspo/manox-app".into()),
                        (ShellSpanKind::Plain, " ".into()),
                        (ShellSpanKind::Git, "git:main".into()),
                        (ShellSpanKind::Plain, " ".into()),
                        (ShellSpanKind::Hit, "*2!4?11".into()),
                    ]))
                    .context(ShellLine::dim("node v26.0.0 rs v1.95.0"))
            };
            match self.kind {
                PageKind::Text => vec![
                    Tool::new("read", &self.states[0])
                        .icon(Icon::new(IconName::FileText).xsmall())
                        .name("Read")
                        .args(cmd_args("reasoning.rs · crates/ai-elements/src"))
                        .copy_input(
                            r#"{"file_path":{"relative_path":"crates/ai-elements/src/reasoning.rs"}}"#,
                        )
                        .copy_output("[crates/ai-elements/src/reasoning.rs#T4K2] …")
                        .content(CodeView::source(
                            "read-code",
                            "use std::time::Duration;\n\nA deliberately long doc line to expose how the composer soft-wraps when the code outruns the card width: continuations hang under the code column, never under the gutter, because no editor wraps under its own line numbers — that is the invariant the compose tests pin.\npub const AUTO_CLOSE_DELAY: Duration = Duration::from_millis(1000);\n\n#[derive(Clone, Debug, PartialEq)]\npub enum ThinkingLabel {\n    Shimmer,\n    Seconds(u64),\n}\n",
                        ))
                        .into_any_element(),
                    Tool::new("ranged", &self.states[1])
                        .icon(Icon::new(IconName::FileText).xsmall())
                        .name("Read")
                        .args(cmd_args("chat_fold.rs · crates/steer-agent-chat-ui/src"))
                        .content(CodeView::new(
                            "ranged-code",
                            vec![
                                CodeLine::plain(138, "fn find_tool_call<'a>(chat: &'a ChatState, id: &str) -> Option<&'a ToolCallState> {"),
                                CodeLine::plain(139, "    chat.turns.iter().find_map(|turn| find_in(&turn.parts, id))"),
                                CodeLine::plain(140, "}"),
                                CodeLine::gap(),
                                CodeLine::plain(168, "fn tool_call_id_of(call: &ToolCallState) -> &str {"),
                                CodeLine::plain(169, "    &call.id"),
                                CodeLine::plain(170, "}"),
                            ],
                        ))
                        .into_any_element(),
                    Tool::new("edit", &self.states[2])
                        .icon(Icon::new(IconName::Pencil).xsmall())
                        .name("Edit")
                        .args(cmd_args("reasoning.rs · crates/ai-elements/src"))
                        .copy_diff("--- a/reasoning.rs\n+++ b/reasoning.rs\n@@ -132,6 +132,7 @@\n-        if self.is_streaming == streaming {\n+        if self.is_streaming == streaming && !streaming {\n")
                        .content(CodeView::new(
                            "edit-code",
                            vec![
                                CodeLine::context(132, "    pub fn set_streaming(&mut self, streaming: bool, cx: &mut Context<Self>) {"),
                                CodeLine::removed(133, "        if self.is_streaming == streaming {"),
                                CodeLine::removed(134, "            return;"),
                                CodeLine::added(133, "        if self.is_streaming == streaming && !streaming {"),
                                CodeLine::added(134, "            // a no-op falling edge collapses early; the rising edge opens"),
                                CodeLine::context(135, "        }"),
                            ],
                        ))
                        .into_any_element(),
                    Tool::new("write", &self.states[3])
                        .icon(Icon::new(IconName::FilePlus).xsmall())
                        .name("Write")
                        .args(cmd_args("code_view.rs · crates/ai-elements/src"))
                        .content(CodeView::source(
                            "write-code",
                            "//! A text-editor mock for file-shaped tool bodies.\n\npub struct CodeView {\n    lines: Vec<CodeLine>,\n}\n",
                        ))
                        .into_any_element(),
                ],
                PageKind::Shell => vec![
                    Tool::new("bash", &self.states[0])
                        .icon(Icon::new(IconName::SquareTerminal).xsmall())
                        .name("Bash")
                        .args(cmd_args("cargo test -p ai-elements"))
                        .copy_input(r#"{"command":"cargo test -p ai-elements"}"#)
                        .copy_output("running 40 tests\ntest result: ok. 40 passed; 0 failed")
                        .content(prompt(
                            ShellView::new("bash-shell", "cargo test -p ai-elements")
                                .line(ShellLine::dim("   Compiling ai-elements v0.1.0"))
                                .line(ShellLine::plain("running 40 tests"))
                                .line(ShellLine::plain(
                                    "test result: ok. 40 passed; 0 failed; 0 ignored",
                                ))
                                .exit(0)
                                .duration(Duration::from_millis(4210)),
                        ))
                        .into_any_element(),
                    Tool::new("grep", &self.states[1])
                        .icon(Icon::new(IconName::TextSearch).xsmall())
                        .name("Grep")
                        .args(cmd_args("find_tool_call · crates/steer-agent-chat-ui/src"))
                        .content(prompt(
                            ShellView::new(
                                "grep-shell",
                                "grep -rn \"find_tool_call\" crates/steer-agent-chat-ui/src",
                            )
                            .line(ShellLine::hit(
                                "crates/steer-agent-chat-ui/src/chat_fold.rs",
                                138,
                                "fn find_tool_call<'a>(chat: &'a ChatState, id: &str) {",
                                "find_tool_call",
                            ))
                            .line(ShellLine::hit(
                                "crates/steer-agent-chat-ui/src/message.rs",
                                2317,
                                "    let Some(review) = fold_input_request(&turn, id) else {",
                                "find_tool_call",
                            ))
                            .line(ShellLine::blank())
                            .line(ShellLine::dim("2 matches · 2 files"))
                            .exit(0)
                            .duration(Duration::from_millis(120)),
                        ))
                        .into_any_element(),
                    Tool::new("ls", &self.states[2])
                        .icon(Icon::new(IconName::FolderTree).xsmall())
                        .name("Ls")
                        .args(cmd_args("crates/ai-elements"))
                        .content(prompt(
                            ShellView::new("ls-shell", "ls -la crates/ai-elements")
                                .line(ShellLine::spans(vec![
                                    (
                                        ShellSpanKind::Dim,
                                        "drwxr-xr-x   4 czr  staff   128 Oct 16 09:58 ".into(),
                                    ),
                                    (ShellSpanKind::Path, "src".into()),
                                ]))
                                .line(ShellLine::spans(vec![
                                    (
                                        ShellSpanKind::Dim,
                                        "-rw-r--r--   1 czr  staff   512 Oct 15 18:44 ".into(),
                                    ),
                                    (ShellSpanKind::Plain, "Cargo.toml".into()),
                                ]))
                                .exit(0)
                                .duration(Duration::from_millis(40)),
                        ))
                        .into_any_element(),
                ],
                PageKind::Code => {
                    let doc = |id: &'static str, source: String| {
                        v_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(foreground)
                                    .child(SharedString::from(id)),
                            )
                            .child(static_document(format!("code-doc-{id}"), source, theme, cx))
                            .into_any_element()
                    };
                    vec![
                        doc(
                            "rust",
                            "```rust\n/// One streaming delta, appended to a persistent document.\npub fn push_str(&mut self, delta: &str, cx: &mut Context<TextViewState>) {\n    // Only the appended suffix re-parses; the prefix keeps its layout.\n    let parsed = parse_delta(delta).unwrap_or_else(|| delta.to_string());\n    self.text.push_str(&parsed);\n    cx.notify();\n}\n```\n".to_string(),
                        ),
                        doc(
                            "tall",
                            tall_rust_block(),
                        ),
                    ]
                }
                PageKind::Svg => {
                    let theme = Theme::global(cx);
                    vec![static_document(
                        "svg-page",
                        concat!(
                            "An ```svg fence renders as its diagram — the image is the ",
                            "default face (the corner toggle flips to the source panel, ",
                            "pinned by the flip test in steer-components):\n\n",
                            "```svg\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"240\" height=\"120\">",
                            "<rect width=\"240\" height=\"120\" fill=\"#3b82f6\"/>",
                            "<circle cx=\"60\" cy=\"60\" r=\"40\" fill=\"#fbbf24\"/>",
                            "<rect x=\"140\" y=\"20\" width=\"80\" height=\"80\" fill=\"#10b981\"/>",
                            "</svg>\n```\n"
                        ),
                        theme,
                        cx,
                    )
                    .into_any_element()]
                }
                PageKind::CodeMore => {
                    let doc = |id: &'static str, source: String| {
                        v_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(foreground)
                                    .child(SharedString::from(id)),
                            )
                            .child(static_document(format!("code-doc-{id}"), source, theme, cx))
                            .into_any_element()
                    };
                    vec![
                        doc(
                            "wide",
                            "```rust\nlet rewrite = Rewrite { instruction: \"把这段说明文字改成更正式的语气，保留全部技术细节，不要引入新的事实，输出为 JSON 并保持原有结构不变\", target: \"crates/steer-components/src/markdown/code_block.rs\", max_length: 4096 };\nfinish(rewrite);\n```\n".to_string(),
                        ),
                        doc(
                            "diff — split",
                            "```diff\n--- a/crates/steer-components/src/markdown/mod.rs\n+++ b/crates/steer-components/src/markdown/mod.rs\n@@ -12,6 +12,7 @@ pub mod linkify {\n pub mod svg_block {\n     pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;\n+    pub const FENCE_BLOCKS: usize = 4;\n-    pub const DIAGRAM_BLOCKS: usize = 2;\n-    pub const DIAGRAM_CAP: Pixels = px(512.);\n }\n```\n".to_string(),
                        ),
                        doc(
                            "merge — three columns",
                            "```rust\n<<<<<<< HEAD\nlet mode = Mode::Editor;\n||||||| merged common ancestors\nlet mode = Mode::Auto;\n=======\nlet mode = Mode::Panel;\n>>>>>>> feat/code-block\n```\n".to_string(),
                        ),
                        doc(
                            "cjk + bare",
                            "```python\n# 中文注释：流式增量只重解析追加的后缀。\ndef push_str(self, delta: str) -> str:\n    return f\"已追加 {len(delta)} 个字节\"\n```\n\n```\nno info string, no highlighting — but the panel, gutter and copy stay\n```\n".to_string(),
                        ),
                        doc(
                            "message",
                            "The incremental path only re-parses the appended suffix, so a streaming fence renders as a panel from its first delta:\n\n```json\n{\"kind\": \"delta\", \"text\": \"fn main() {\"}\n}\n```\n\nand settles when the closing fence arrives. See `ChatDocument::sync`.\n\n```rust\nlet appended = self.linkified.as_ref()\n    .filter(|prev| next.starts_with(prev.as_str()));\n```\n".to_string(),
                        ),
                    ]
                }
                PageKind::Ask => {
                    let settled = self.ask_settled.as_ref().expect("settled ask state");
                    let fresh = self.ask_fresh.as_ref().expect("fresh ask state");
                    let markdown = "### Question 1: Which rich-text core should the chat standardize on?\n\ngpui-base TextView\n\n### Question 2: Should settled tool bodies fold by default?\n\n*(skipped)*\n";
                    vec![AskUserQuestion::new("ask-settled", settled)
                        .icon(
                            Icon::new(IconName::MessageCircleQuestionMark)
                                .xsmall()
                                .into_any_element(),
                        )
                        .blocks(vec![
                            AskBlock::new("Which rich-text core should the chat standardize on?")
                                .header("Libraries")
                                .option(
                                    AskOption::new("gpui-base TextView")
                                        .badge("Recommended")
                                        .description(
                                            "The stack markdown already renders through — incremental push_str, selection, highlight channels.",
                                        ),
                                )
                                .option(
                                    AskOption::new("gpui-component Markdown")
                                        .description("Higher-level, but a full re-parse per update."),
                                ),
                            AskBlock::new("Should settled tool bodies fold by default?")
                                .header("Default state")
                                .multi_select()
                                .option(AskOption::new("Fold after a second"))
                                .option(AskOption::new("Stay open until clicked")),
                        ])
                        .output(static_document("ask-output", markdown, theme, cx))
                        .into_any_element(),
                    AskUserQuestion::new("ask-fresh", fresh)
                        .icon(
                            Icon::new(IconName::MessageCircleQuestionMark)
                                .xsmall()
                                .into_any_element(),
                        )
                        .blocks(vec![
                            AskBlock::new("Which rich-text core should the chat standardize on?")
                                .header("Libraries")
                                .option(
                                    AskOption::new("gpui-base TextView")
                                        .badge("Recommended")
                                        .description(
                                            "The stack markdown already renders through — incremental push_str, selection, highlight channels.",
                                        ),
                                )
                                .option(
                                    AskOption::new("gpui-component Markdown")
                                        .description("Higher-level, but a full re-parse per update."),
                                ),
                            AskBlock::new("Which surfaces get the tool cards first?")
                                .header("Scope")
                                .multi_select()
                                .option(AskOption::new("Conversation column"))
                                .option(AskOption::new("Right rail ToolTab")),
                        ])
                        .into_any_element(),
                ]
                }
            }
        }
    }
    impl Render for Page {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::global(cx);
            let rows = self.rows(cx);
            div().size_full().bg(theme.background).child(
                v_flex()
                    .w_full()
                    .p_6()
                    .gap_4()
                    .max_w(px(860.))
                    .children(rows),
            )
        }
    }
}
