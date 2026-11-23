# 上游未修 bug 台账

只记录**根因尚未在上游收敛**、需要我们来绕的 gpui 栈 bug。每条包含触发面、我们现有缓解、上游修复判据、每次 bump 的复核动作与销账条件。销账即删条。

---

## 1. `StyledText::with_runs` "invalid text run"（富文本 run 与文本不一致）

**状态：上游未定位，未修；属同一族，不是同一个稳定复现。**

### 症状与位置

```
panic: invalid text run
location: gpui-pre-0.3.9/src/elements/text.rs:533:17  (StyledText::with_runs)
  7: <gpui::elements::text::StyledText>::with_runs
  8: <gpui_base::text::inline::Inline as Element>::request_layout
 40: <gpui_base::text::text_view::TextView as Element>::request_layout
 62: <gpui_base::motion::reveal::MotionReveal as Element>::prepaint
 85: <ai_elements::tool::Tool>::prepaint
```

`with_runs` 校验双向 tiling 不变量：runs 必须有序、不重叠、在界内、落在字符边界、长度之和等于文本长度。不满足即 abort。进程 `SIGABRT`（布局阶段、主线程）。

崩溃现场：

- 2026-10-19 .. 2026-10-22：聊天正文 transcript ×5（流式 fade 期间引用定义后到，文本回溯变短）。见
  `crates/steer-agent-chat-ui/src/views/message.rs` 的 `decorate_document` 长注释与
  `crates/agent-ui/tests/cjk_fuzz_deep.rs`（最小 20 字节复现）。
- 2026-10-26：Bash 工具卡（`ai_elements::shell::ShellView` body），卡片 open reveal 动画期间流式输出（session `c8c87005-36ec-4cad-8391-8eb853d0f906`，`2026-10-26T06:11:42.651Z` abort，输出 6.7 KiB 密集 CJK + 转义 JSON + 反引号）。

### 根因状态

未在上游钉死。同族特征：**高亮 runs 与已提交（已解析）的文本不一致**。两个可观测面：

- `StyledText::with_runs` 在累积 run 长度落在多字节字符中间时 panic；
- `highlighted_styles` 对超界 range 是**丢弃**而不是裁剪（`panel_document` 的 `clamp_runs` 正是为此加的防线）。

transcript 侧的确定性根因（引用定义后到导致同一段落重排变短、fade 区间落到新文本上）已在 app 侧绕开；工具卡侧的精确触发条件未能离屏复现，但结构性暴露清晰：卡片 body 每帧重建静态 `TextView`（state 按 element id 复用），文本可在已提交解析之下不连续变化（chunk 合批、`live_tail` 12 KiB 掉头、settle 时 `LINE_CAP` 截断）。

### 我们的缓解（改上游后应逐项删除）

- transcript：`ChatDocument::sync` 在进上游前钉住 link reference definition
  （`crates/steer-components/src/markdown/text_view.rs`）。
- 工具卡：伪流式快照
  （`crates/steer-agent-chat-ui/src/views/tool_stream.rs`，消费点
  `crates/steer-agent-chat-ui/src/views/tool_cards.rs` 的 `card_body` / `ensure_tool_card`）：
  append-only + 100ms 合批 + 开合动画 400ms 内不推进 tick、替换立即发布、generation 进 element id。
- 高亮边界：`crates/ai-elements/src/text_runs.rs` 的 `clamp_runs`（`ShellView` / `CodeView` 的
  `code_block_highlighter` 统一过一遍裁剪 + 排序 + 去重叠）。
- 探针：`crates/agent-ui/tests/cjk_fuzz_deep.rs`（`CJK_FUZZ_RUN=1`）与
  `crates/steer-agent-chat-ui/tests/tool_card_stream.rs`。

### 上游修复判据（任一满足即可评估销账）

1. `gpui-base` 在 `Inline::request_layout` / `StyledText::with_runs` 边界对 runs 做裁剪或消毒（越界/非字符边界不再 panic）；
2. `with_runs` 容忍并安全处理 run 与文本不匹配（不再 abort）；
3. `highlighted_styles` 对超界 range 做裁剪而不是丢弃。

### 每次 bump 的复核动作

1. 读新的 `gpui-base` 源码：grep `with_runs`、`InvalidTextRun`、`text_runs`、`highlighted_styles`，确认是否出现边界消毒/裁剪。
2. 跑 `CJK_FUZZ_RUN=1 cargo test -p agent-ui --test cjk_fuzz_deep`（复现状态见下方轮次记录；转红即上游回归或暴露面变化）。
3. 跑 `cargo test -p steer-agent-chat-ui --test tool_card_stream`。
4. 上游已修（三条判据任一满足）：**删除**伪流式合批（恢复真流式）、`clamp_runs` 与本条。上游未修：保持现状，不额外加 workaround。

### 轮次复核记录

- **gpui-pre 0.3.12 / gpui-base 0.9.1（2026-11-23）**：`with_runs` panic 原样（`elements/text.rs:531-536`）；`highlighted_styles` 对超界 range 仍过滤丢弃不裁剪（`text/node.rs:2558`）；`Inline::request_layout` 边界无新增消毒——三条判据均未满足，缓解全维持。**经验状态变化**：`CJK_FUZZ_RUN=1` battery 10/10 全绿（321s 真 fuzz；0.3.9 上按 battery 头注必崩）。确切上游修复点未定位（候选：0.9.1 #3846 高亮合并重写、#3799 选择高亮裁剪——都在 fade/highlight 机械上）。battery 保持 env-gated 作回归金丝雀：后续 bump 转红即报警。
