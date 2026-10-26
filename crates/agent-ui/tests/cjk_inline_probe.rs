//! Investigation probe for the `invalid text run` abort in
//! `StyledText::with_runs` via `gpui_base::text::inline::Inline::request_layout`
//! (crash reports 2026-10-19 .. 2026-10-22). Paints CJK-bearing markdown
//! batteries through the production message path, whole and streamed.

use agent_ui::conversation::ConversationState;
use agent_ui::views::message::MessageItem;
use gpui::{
    AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use manox_agent::MessageAuthor;

const MODEL: &str = "bigmodel-anthropic/glm-5.3";

struct RowProbe {
    row: Entity<MessageItem>,
}

impl Render for RowProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().child(self.row.clone())
    }
}

fn paint_chunks(cx: &mut TestAppContext, chunks: &[&str]) -> VisualTestContext {
    cx.update(gpui_component::init);
    let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
    cx.update(|cx| {
        let mut fold = Fold::new();
        fold.start_turn();
        for chunk in chunks {
            fold.delta("p-1", chunk);
        }
        project_fold(cx, &conversation, &mut fold);
    });
    // The projection renders the turn's user row too; the probe paints the
    // assistant bubble.
    let rows = cx.update(|cx| {
        conversation
            .read(cx)
            .items()
            .iter()
            .filter(|item| {
                matches!(
                    item.read(cx).kind(),
                    agent_ui::conversation::ConvItem::Assistant { .. }
                )
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    assert_eq!(rows.len(), 1, "one assistant bubble");

    let window = cx.open_window(size(px(760.), px(600.)), move |_, _| RowProbe {
        row: rows[0].clone(),
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    for _ in 0..4 {
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
    visual
}
fn paint_message(cx: &mut TestAppContext, body: &str) -> VisualTestContext {
    paint_chunks(cx, &[body])
}

// --- whole-message batteries ------------------------------------------------

#[gpui::test]
async fn whole_cjk_bold_italic_code(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "这是**加粗中文**，然后*斜体中文*，然后`inline 代码`混排 English text。",
    );
}

#[gpui::test]
async fn whole_cjk_link(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "先看[链接文字](https://example.com/path)后面直接接中文，再看 https://example.com 裸链接。",
    );
}

#[gpui::test]
async fn whole_cjk_strike_emoji(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "~~删除线的中文~~ 与 emoji 😀🎉 混排，全角标点：句号、逗号、顿号！",
    );
}

#[gpui::test]
async fn whole_cjk_math_inline(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "当 $x^2 + y^2 = z^2$ 成立时，勾股定理描述了直角三角形三边的关系。",
    );
}

#[gpui::test]
async fn whole_cjk_list_heading_quote(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "## 中文标题\n\n- 列表项**一**，含`代码`\n- 列表项二：[链接](https://example.com)\n\n> 引用块里的**中文粗体**混排。\n\n| 列甲 | 列乙 |\n|---|---|\n| 单元格**一** | 单元格二 |\n",
    );
}

#[gpui::test]
async fn whole_cjk_html_inline(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "内联 <u>下划线中文</u> 与 <s>删除线中文</s> 以及 <mark>高亮中文</mark> 混排。",
    );
}

#[gpui::test]
async fn whole_cjk_nested_emphasis(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "嵌套 **_斜体加粗中文_** 与 ***三层中文*** 混排 English。",
    );
}

#[gpui::test]
async fn whole_cjk_autolink_angle(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "尖括号链接 <https://example.com/中文路径> 与邮箱 user@example.com 中文后缀。",
    );
}

// --- streamed batteries -----------------------------------------------------

#[gpui::test]
async fn streamed_cjk_bold_split_marker(cx: &mut TestAppContext) {
    paint_chunks(
        cx,
        &["这是一段**加粗", "中文**混排，然后", "更多内容继续流入。"],
    );
}

#[gpui::test]
async fn streamed_cjk_code_fence_split(cx: &mut TestAppContext) {
    paint_chunks(
        cx,
        &["先来一段`内联代", "码`再继续**加粗中文**，", "收尾。"],
    );
}

#[gpui::test]
async fn streamed_cjk_link_split(cx: &mut TestAppContext) {
    paint_chunks(
        cx,
        &[
            "看这个[链接文",
            "字](https://example.com)后",
            "面接中文继续。",
        ],
    );
}

#[gpui::test]
async fn streamed_cjk_word_by_word(cx: &mut TestAppContext) {
    let text = "中文流式输出：**加粗段**、`代码段`、[链接段](https://example.com)、*斜体段*，还有 emoji 🎉。";
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut chunks: Vec<&str> = Vec::new();
    while start < bytes.len() {
        let mut end = (start + 7).min(bytes.len());
        while end < bytes.len() && !text.is_char_boundary(end) {
            end += 1;
        }
        chunks.push(&text[start..end]);
        start = end;
    }
    paint_chunks(cx, &chunks);
}

#[gpui::test]
async fn streamed_cjk_unclosed_then_closed(cx: &mut TestAppContext) {
    paint_chunks(cx, &["text 前缀 ", "**中文", "加粗**", " 后缀"]);
}

// --- kept from the harness's own streaming loop -----------------------------

#[gpui::test]
async fn streamed_long_paragraph_cjk(cx: &mut TestAppContext) {
    paint_chunks(
        cx,
        &[
            "GPUI 是 Zed 团队的高性能 UI 框架，",
            "本应用基于 **gpui-pre** 快照构建。",
            "当 `TextView` 渲染包含 CJK 字符的 markdown 时，",
            "需要保证 [高亮区间](https://example.com) 与字节长度一致，",
            "否则 `StyledText::with_runs` 会 panic 导致**应用崩溃**。",
        ],
    );
}

#[gpui::test]
async fn whole_ascii_control_probe(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "Control chars: a\u{00a0}nbsp, zero\u{200b}width, tab\there, combining e\u{301}.",
    );
}

// --- parse-side suspects: decoded text vs source byte offsets ---------------

#[gpui::test]
async fn whole_cjk_html_entities(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "实体：&#x4e2d;&#x6587; 与 &amp;lt; 及 &nbsp;，再**加粗&#x4e2d;文**混排。",
    );
}

#[gpui::test]
async fn whole_cjk_backslash_escapes(cx: &mut TestAppContext) {
    paint_message(
        cx,
        r"转义：\*不是斜体\* 与 \\ 反斜杠，以及 **中文\*嵌套\*粗**。",
    );
}

#[gpui::test]
async fn whole_cjk_hard_breaks(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "第一行中文  \n第二行中文，与<br>HTML 换行，**加粗**继续。",
    );
}

#[gpui::test]
async fn whole_cjk_reference_links(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "引用式：[中文标签][ref] 与 [简洁]，定义在下面。\n\n[ref]: https://example.com\n[简洁]: https://example.org",
    );
}

#[gpui::test]
async fn whole_cjk_footnotes(cx: &mut TestAppContext) {
    paint_message(cx, "脚注：中文正文[^1]继续。\n\n[^1]: 这是脚注内容中文。");
}

#[gpui::test]
async fn whole_cjk_image_inline(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "内联图片：![中文替代文字](https://example.com/图.png)后面接中文。",
    );
}

#[gpui::test]
async fn whole_cjk_punctuation_adjacent_emphasis(cx: &mut TestAppContext) {
    paint_message(cx, "全角紧邻：中文**，加粗，**中文与**。加粗。**测试。");
}

#[gpui::test]
async fn whole_cjk_setext_heading(cx: &mut TestAppContext) {
    paint_message(cx, "中文标题\n====\n\n正文**加粗**中文继续。");
}

#[gpui::test]
async fn whole_cjk_task_list(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "- [ ] 待办**一**中文\n- [x] 已办：`代码`中文\n- 普通**项**",
    );
}

#[gpui::test]
async fn whole_cjk_nested_quote_list(cx: &mut TestAppContext) {
    paint_message(
        cx,
        "> 引用\n> - 列表**内**嵌套\n> - [链接](https://e.com)中文\n\n尾部。",
    );
}

// --- streaming code blocks (tool output shape) -------------------------------

#[gpui::test]
async fn streamed_cjk_code_block(cx: &mut TestAppContext) {
    let full =
        "```ansi\n处理中...\n[OK] 第一步完成：文件已写入\n  PR!101 [merged] ->main 中文说明\n```";
    let bytes = full.as_bytes();
    let mut chunks: Vec<&str> = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let mut end = (start + 6).min(bytes.len());
        while end < bytes.len() && !full.is_char_boundary(end) {
            end += 1;
        }
        chunks.push(&full[start..end]);
        start = end;
    }
    paint_chunks(cx, &chunks);
}

#[gpui::test]
async fn streamed_cjk_code_block_growing_lines(cx: &mut TestAppContext) {
    let lines: Vec<String> = (0..30)
        .map(|i| format!("行{i:02}：**状态** `运行中` 中文输出 {i}\n"))
        .collect();
    let full = format!("```ansi\n{}\n```", lines.concat());
    let bytes = full.as_bytes();
    let mut chunks: Vec<&str> = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let mut end = (start + 11).min(bytes.len());
        while end < bytes.len() && !full.is_char_boundary(end) {
            end += 1;
        }
        chunks.push(&full[start..end]);
        start = end;
    }
    paint_chunks(cx, &chunks);
}

#[gpui::test]
async fn streamed_cjk_table(cx: &mut TestAppContext) {
    paint_chunks(
        cx,
        &[
            "| 工具 | 状态 |\n|---|---|\n",
            "| 读取 | **完成** |\n",
            "| 写入 | 运行中 |\n",
            "| 搜索[链接](https://e.com) | `代码` |\n",
        ],
    );
}

/// The live fold, driven exactly as the store drives it: StateActions folded
/// through the published reducers, then projected. The synthesized delta
/// sequences here are fixtures — no real session journal rides this file.
struct Fold {
    chat: ahp_types::state::ChatState,
}

const TURN: &str = "t-1";

impl Fold {
    fn new() -> Self {
        Self {
            chat: serde_json::from_value(serde_json::json!({
                "resource": "ahp-chat:/c-probe",
                "title": "probe",
                "status": 0,
                "modifiedAt": "2026-01-01T00:00:00Z",
                "turns": [],
            }))
            .expect("the empty chat parses"),
        }
    }

    fn apply(&mut self, action: &ahp_types::actions::StateAction) {
        ahp::reducers::apply_action_to_chat(&mut self.chat, action);
    }

    fn start_turn(&mut self) {
        let message: ahp_types::state::Message = serde_json::from_value(serde_json::json!({
            "text": "probe", "origin": { "kind": "user" },
        }))
        .expect("the opening message parses");
        self.apply(&ahp_types::actions::StateAction::ChatTurnStarted(
            ahp_types::actions::ChatTurnStartedAction {
                turn_id: TURN.into(),
                started_at: "2026-01-01T00:00:00Z".into(),
                message,
                queued_message_id: None,
                meta: None,
            },
        ));
    }

    fn open_part(&mut self, part: &str, reasoning: bool) {
        let exists = self.chat.active_turn.as_ref().is_some_and(|turn| {
            turn.response_parts.iter().any(|p| match p {
                ahp_types::state::ResponsePart::Markdown(md) => !reasoning && md.id == part,
                ahp_types::state::ResponsePart::Reasoning(rs) => reasoning && rs.id == part,
                _ => false,
            })
        });
        if exists {
            return;
        }
        let part_value = if reasoning {
            serde_json::json!({ "kind": "reasoning", "id": part, "content": "" })
        } else {
            serde_json::json!({ "kind": "markdown", "id": part, "content": "" })
        };
        let action: ahp_types::actions::StateAction = serde_json::from_value(serde_json::json!({
            "type": "chat/responsePart", "turnId": TURN, "part": part_value,
        }))
        .expect("the responsePart action parses");
        self.apply(&action);
    }

    /// Open the markdown part (once) and append a delta — the wire's
    /// responsePart-then-delta shape.
    fn delta(&mut self, part: &str, text: &str) {
        self.open_part(part, false);
        self.apply(&ahp_types::actions::StateAction::ChatDelta(
            ahp_types::actions::ChatDeltaAction {
                turn_id: TURN.into(),
                part_id: part.into(),
                content: text.into(),
                meta: None,
            },
        ));
    }
}

/// Project the fold into the conversation — the same stage/reconcile split
/// the workspace sync runs.
fn project_fold(
    cx: &mut gpui::App,
    conversation: &gpui::Entity<ConversationState>,
    fold: &mut Fold,
) {
    let host = steer_agent_chat_ui::host::noop_host();
    let env = steer_agent_chat_ui::transcript::ProjectEnv {
        role: MODEL,
        recipient: &manox_agent::MessageAuthor::Lead,
        host: &host,
        cwd: None,
        fork_source: false,
        promoted_confirmation: None,
    };
    conversation.update(cx, |c, cx| {
        let nodes = c.stage(Some(&fold.chat), &env);
        c.reconcile(nodes, &env, cx);
    });
}
