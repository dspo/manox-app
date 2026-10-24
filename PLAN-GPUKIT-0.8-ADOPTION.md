# gpui-kit 0.8.0 跟进计划（2026-10-23 制定）

执行状态（2026-10-23 晚更新）：**PR 0 bump 已合入 main（#209）**；PR 1（caret 锚定）已开 **#213**；并行会话的重复 bump PR #204 已去重关闭建议、其 CI 修复 #206 已先行合入（lld 链接，CI 失败根因之一）；E1–E3 与 PR 2–6 按序排期。skill 侧记录（hand-rolled.md 0.8.0 轮 + 下期确认清单）随本文件同 PR 落库。

评估依据：gpui-component/gpui-base 0.7.0→0.8.0 源码 diff、[v0.8.0 release notes](https://github.com/longbridge/gpui-kit/releases/tag/v0.8.0)、zed 快照映射 `1a28cff → 93d7456`（2026-09-27 ~ 10-10，22 个 gpui 提交）。评估三问结论：breaking 不命中（`cargo check --all-targets` 全绿，唯一 warning 为传递依赖 `block v0.1.6` future-incompat）；新特性 × 手搓热点求交结果见下。

## 作者前置决策（已确认）

- 0.8「最内层滚动区独占整段手势，到底不交给外层」：**符合期望**，非回归。
- `PrepaintCaretGeometry` 为 `#[doc(hidden)]` 半公开面，**接受漂移风险**。

## 本期决策清单

| 项 | 决策 | 来源 |
|---|---|---|
| bump 收尾（PR 0） | 跟进（固定项），本会话已开始执行 | — |
| typeahead caret 锚定（PR 1） | 跟进 | 作者确认 |
| 聊天列 ScrollBounce（PR 2） | 跟进 | 作者确认 |
| @-chip 悬停（PR 3） | 跟进 | 作者确认（重新询问） |
| 右栏 Diff ToolTab spike（PR 4） | 跟进（spike 先行） | 作者确认 |
| resize_handle 替代 divider.rs（PR 5） | 跟进 | 作者确认（翻代决） |
| speech 语音输入（PR 6） | 跟进，macOS 先行 | 作者确认（翻代决） |
| 遗留三行评估（E1–E3） | 本期全评：on_paste / Combobox / Empty，产出结论不承诺迁移 | 作者确认 |
| syntect 三比 | 本期不做，下期确认 | 作者确认 |
| MessageScroller | 本期不做，下期确认（聊天管线重建落地后优先重评） | 作者确认 |
| gpui-webview 替代 steer-webview | 长期观察，逢 minor 对位 | — |

第二轮曾因作者未值守由评估者代决；2026-10-23 重新询问后以作者答复为准（chip 悬停维持跟进，divider 与 speech 翻案纳入）。仍搁置的项不构成永久否决：记入 `hand-rolled.md`「下期确认清单」，下期 gpui-kit minor 评估时逐项再次向作者确认。

## PR 序列

所有 adoption PR 依赖 PR 0 先合（0.8 API 才可用）。PR 1–6 相互独立，可并行推进；合计约 5–6.5 天，另有 E1–E3 评估约 1 天。

### PR 0 — bump 收尾（chore(deps)，行为中立）

工作树已就位（`gpui-pre =0.3.9` / `gpui-component =0.8.0` / `gpui-base =0.8.0` + 迁移改动）：

- `cargo clippy --all-targets -- -D warnings`、全量 `cargo test`、`cargo test -p agent-ui --features test-support`、`cargo fmt`。
- 视觉门禁：四张 `CHROME_SHOT`（base / right / panel / switch）+ `script/check-tab-indicator.sh`，与 0.7 基线对比。
- 真机 `cargo run` 手感验收（离屏看不到动效）：composer（`.auto_grow(4,12)`）滚到底回弹、右栏 Editor / 设置页回弹、嵌套滚动独占——语义已确认期望，看手感即可。
- PR 描述贴 release notes 链接 + zed-rev 映射；不带 Co-Authored-By。

### PR 1 — typeahead caret 锚定（小，约半天）

- 位置：`crates/steer-agent-chat-ui/src/views/completion.rs` 锚点计算 + `deferred + anchored` 装配处。
- 做法：`TextareaState::prepaint_caret_geometry()` 的 `cursor_bounds / line_height / scroll_offset` 计算锚点（同帧、无上一帧延迟）；上方空间不足翻到下方。
- 不变式：typeahead 本体保持手搓（不抢焦点 + 键盘导航留在 Workspace 绑定；内置补全菜单 Editor-only 不适用）。
- 测试：现有探针（`tests/cjk_*` 等）+ 新增贴边翻转探针。

### PR 2 — 聊天列 ScrollBounce（小–中，约半天–1 天）

- 位置：聊天列装配处（自绘 `list()` 外包 `ScrollBounce::new(id, &handle, child)`，macOS 默认动参已对齐 NSScrollView）。
- 验证重点：与 stick-to-bottom 自动置底的叠加；`ScrollBounce` 默认接管竖向滚轮，需与聊天列现有 wheel 处理去重。
- 验收：真机手感 + 离屏（动画中间帧需自行 pump，出图只反映终态）。

### PR 3 — @-chip 悬停（小，约半天）

- 位置：composer 的 `Textarea` 挂 `.on_token_hover(...)`；hover @-mention chip 显示完整路径/名称 tooltip（第一版只做路径提示，避免新增文案）。
- 复用现有 Tooltip 路径；若最终需要新 UI 文案，`zh-CN/en` 双 `.ftl` 各加键。

### PR 4 — 右栏 Diff ToolTab spike（中，约 1–2 天）

- 目标：ToolTab 注册表加 DiffTab——Edit 类工具卡加「在右栏打开 diff」入口 → `DiffFile::parse(patch)` → `DiffState::new(files, cx)` → `Diff::new(&state)` 全高渲染。
- spike 验收问题清单：
  1. 视觉：gpui-component Diff 与 2026-Light 令牌一致性；
  2. 性能：大 patch（数千行）折叠/展开与滚动；
  3. 交互：选区复制、行号、语法高亮（tree-sitter 路径）可用；
  4. 隔离：右栏自滚动与聊天列滚动的手势边界（嵌套独占语义）。
- 产出：spike 结论 PR（可含 feature 门控的装配）。**通过后**才开正式 PR：i18n 键（「在右栏打开 diff」等，双语）+ `UI-MAP.md` 更新；`CodeView` 的 Edit 分支是否瘦身以 spike 结论为准（Read 窗口 + `Gap` 语义 Diff 不表达，`CodeView` 不整删）。
- 明确边界：聊天列内嵌不适用（Diff 是虚拟化自滚动列表，需有界高度）。

### PR 5 — resize_handle 替代 divider.rs（小，约半天）

- 位置：`crates/steer-agent-chrome-ui/src/divider.rs`（111 行）。handle 的视觉态与拖拽**发起**部分换上游独立 `resize_handle`（0.8.0 #3302，`on_drag` 现取 `&T`）；`shell.rs` 根部的绝对坐标数学与 per-side payload 类型不动（不在替代范围）。
- 顺带核 `ResizablePanel::fixed` 是否正好是右栏/底部 dock 现在手工维护的「容器变化时保持尺寸」行为。
- 验收：6px 宽、`<|>` 光标、双击重置三项像素/行为不回归；四张 `CHROME_SHOT` 对比 + 手动拖拽。

### PR 6 — speech 语音输入（中，约 1–1.5 天，macOS 先行）

- 依赖：`gpui-component` 开 `speech` feature（macOS `objc2-speech`、Windows WinRT；Linux 为 placeholder）。
- 功能：composer 加「按住说话」入口，识别文本插入 `TextareaState` 光标处；电平/波形组件第一版不接。
- 平台降级：发行含 deb/rpm，不能给 Linux 用户一个坏按钮——按运行时能力探测隐藏/禁用；若上游无探测 API，按 `cfg(target_os)` 编译期隐藏。
- macOS 权限：`script/bundle-mac` 产物的 Info.plist 需加 `NSMicrophoneUsageDescription`（release 管线随同 PR 改）；`cargo run` 开发态需确认系统提示路径可用。
- 文案：新 UI 文案 `zh-CN/en` 双 `.ftl` 加键；`UI-MAP.md` 更新。

### 评估任务 E1–E3（合计约 1 天，产出结论不承诺迁移）

对照手搓与原生件的能力/像素/迁移量，每项产出 adoption 候选（转 PR）或冲突结论（记热点表）；无论向背都更新 `hand-rolled.md` 销账。

- **E1 粘贴图片**：`agent-ui/src/workspace/composer.rs` 的 stage clipboard image vs Input/Textarea `on_paste` 原生事件（可拿剪贴板图片/文件）。
- **E2 标题栏 picker / 模型选择器**：`steer-agent-chrome-ui/src/titlebar.rs`、`agent-ui/src/workspace/chips.rs` 的手搓弹出 vs `Combobox`（+ Popover `offset`）；#123 后组件 Popover 已是仓内成熟路径。
- **E3 「No chats」空态**：`steer-agent-chrome-ui/src/session_list.rs` vs `Empty` 组件族。

## 义务（每 PR）

- 全量门禁 + PR 写清 Test Plan 与 Assumptions；adoption PR 写明替代哪块手搓、预计删多少行、像素断言如何保持。
- 合入后同步 `.agents/skills/gpui-kit-follow/references/hand-rolled.md`（本计划已预记立项行）；UI 变更同 PR 更新 `UI-MAP.md`。
- 不夹带：bump PR 与 adoption PR 严格分离，revert 单 commit 即回滚。

## 本期明确不做（详见 hand-rolled.md「下期确认清单」）

syntect 三比、MessageScroller（待聊天管线重建落地）、gpui-webview 替代 steer-webview（长期观察）。这些不是永久否决——下期 gpui-kit minor 版本评估时逐项向作者再次确认。
