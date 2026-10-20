//! Visual acceptance for the tool-card designs (harness=false, main thread):
//! gpui's official offscreen render — `VisualTestAppContext` + Metal texture
//! readback — of the three gallery pages' static cards, no screen-recording
//! permission needed.
//!
//! Run with `GALLERY_SHOT=/tmp/tools.png GALLERY_PAGE=text cargo test -p
//! ai-elements --test visual` (pages: `text`, `shell`, `ask`; default
//! `text`). The samples mirror the gallery example's demo content — leaner,
//! one card per family shape, enough to judge the design's pixels. macOS
//! only: the readback is a Metal texture; the harness's imports exist only
//! on that platform.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("visual: the gallery harness is macOS-only (Metal readback), skipping");
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
        Styled as _, VisualTestAppContext, Window, div, px, size,
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
                PageKind::Ask => 0,
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
