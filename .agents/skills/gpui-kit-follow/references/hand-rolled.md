# 手搓热点表（评估问二的对照面）

本表列出本仓手搓实现与其已知上游映射，供新版本评估时求交。

**维护规则**：adoption PR 合入时同步更新本表；「已知上游映射」标注所评估的版本，只是线索——每轮评估须对新版本重新核对（重评估原则见 SKILL.md）。

> 0.8.0 轮（2026-10-23）记录：整列 bump 已合入 main（#209，gpui-pre 0.3.9 / gpui-component 0.8.0；唯一强制迁移是 gpui-base 0.8 标题阶梯上游化——`with_heading` 覆盖在 Linux 度量下守不住节奏不变量，`text_view.rs` 退役 pinned ladder 改乘上游相对阶梯）。吸收轮排期见 `PLAN-GPUKIT-0.8-ADOPTION.md`。

| 手搓点 | 位置 | 状态（0.8.0 轮已更新；历史轮次见各行） | 已知上游映射 |
|---|---|---|---|
| composer @-补全插入 | `crates/steer-agent-chat-ui/src/views/completion.rs` | **已迁移**（#122 已合，main 349dca3）：`build_replacement_content` 把插入名包成 `InputContent` token，chip 渲染 + 整块删除；文本字面量与 dispatch 语义不变 | 原子 inline token（`InputContent::with_token`）。composer 是 TextareaState，API 可用 |
| goal 状态下拉 | `agent-ui/src/workspace/chips.rs` | **已迁移**（#123 已合，main c5ecb5d）：组件 `Popover` 受控 open，`deferred+absolute+occlude+mouse_down_out` 手搓退役 | `Popover::open/on_open_change/anchor/track_focus`；`Anchor` 在 gpui 本体 |
| context rail 气泡 | `agent-ui/src/workspace/composer_render.rs` | **已迁移**（#123 已合，main c5ecb5d）：组件 `Popover` + `arrow(true)`；slot/负 apron/`BubbleClearance`/同手势 suppression/手绘尾巴全删（净 −111 行），被列裁切的失败模式随 overlay 层消失 | 同上；外点关闭与 Escape 归库管（BasePopover trigger `stop_propagation` + Dialog 角色） |
| typeahead 补全列表 | `steer-agent-chat-ui/src/views/completion.rs` | **明确保留手搓**（#123 评估结论）：caret 锚定（无 trigger 元素）、绝不能抢焦点、键盘导航在 Workspace 绑定——组件 Popover 的 trigger+内容聚焦模型三条都不表达。0.8.0 轮：本体仍手搓（内置补全菜单 `CompletionProvider` 挂 `editor/lsp` 仅 EditorState，TextareaState 接不上）；**caret 锚定已落地 #213**（`prepaint_caret_geometry()` + `anchored().position()`，纯函数翻转/夹取 + 离屏探针钉坐标空间；`#[doc(hidden)]` 漂移作者已接受） | `PrepaintCaretGeometry`（0.8.0 公开；`CaretAnchoredPopup` 包装器仍 `pub(crate)`，贴边翻转自理） |
| markdown 渲染栈 | `crates/steer-components/src/markdown/` | **重评完成，建议 spike 替换**（issue #124）：当年 fork 三动机全被上游补齐，0.7 新增 `markdown_block_parser/block_renderer` 可承载 terminal_panel；先 spike 量化 diff 再删栈 | `stream_fade`、`selected_source_range`、range highlights、block 插件点（0.7.0） |
| composer context ring | `crates/agent-ui/src/workspace/composer_render.rs` | **明确不跟进**（0.7.0 评估有证据）：plot 的 `Arc` 是 `PathBuilder::fill()` 饼图扇形，画不了 2px 圆头描边弧；且 gpui 只 re-export `StrokeOptions` 不含 `LineCap`——lyon 直依赖摘不掉。ring 保持 canvas 手搓 | plot `shape/arc.rs` 为填充式；后续版本若出描边弧或 re-export `LineCap` 再评 |
| 粘贴图片 | `crates/agent-ui/src/workspace/composer.rs`（stage clipboard image） | **0.8.0 轮已立项评估**（作者确认）——PLAN-GPUKIT-0.8-ADOPTION E1 | 0.6.2 Input/Textarea/Editor `on_paste` 原生事件（可拿剪贴板图片/文件） |
| 标题栏 session picker / 模型选择器弹出 | `steer-agent-chrome-ui/src/titlebar.rs`；`agent-ui/src/workspace/chips.rs` | **0.8.0 轮已立项评估**（作者确认）——PLAN-GPUKIT-0.8-ADOPTION E2 | Combobox（0.6.0）；Popover `offset`（0.7.0）——#123 落地后组件 Popover 已是仓内成熟路径 |
| 「No chats」空态 | `steer-agent-chrome-ui/src/session_list.rs` | **0.8.0 轮已立项评估**（作者确认）——PLAN-GPUKIT-0.8-ADOPTION E3 | 0.6.2 `Empty` 组件族 |
| 设置页表单 | `agent-ui/src/views/settings/` | **已迁移**（feat/settings-adoption + feat/settings-mcp-rows）：sidebar/搜索/过滤/分页由 `Settings` 组件承担；语言行与 MCP 每 server 开关均为 `SettingField::{dropdown,switch}` 行级绑定，mock_switch/mock_dropdown 全部退役；Models/ChatGPT/VSCode/Plugins 仍为 `SettingItem::render` Element 整块，其内部原语（section_card/row_with_control 等）保留 | `setting` 模块（0.7.0 `Settings`/`SettingPage`/`SettingGroup`/`SettingItem`/`SettingField`，比 Form/Field 更贴合的映射）；Form / Field（0.7.0 `Field::visible(false)`）、GroupBox/SettingGroup footer（0.7.0）备查 |
| 工具卡 Edit diff 渲染 | `steer-agent-chat-ui/src/views/tool_cards.rs`（手解 unified diff）+ `ai-elements/src/code_view.rs`（646 行） | **0.8.0 轮已立项（spike 先行）**：右栏 ToolTab 全高形态接 `gpui_component::diff`；聊天列内嵌不适用——`Diff` 是 `list()+flex_1/h_full` 虚拟化自滚动列表，需有界高度，叠加 0.8 嵌套滚动独占。`CodeView` 的 Edit 分支是否瘦身待 spike 结论；Read 窗口 + `Gap` 语义 Diff 不表达，`CodeView` 不整删——PLAN-GPUKIT-0.8-ADOPTION PR 4 | `diff` 模块（0.8.0 新增）：`DiffFile::parse(&str)` → `DiffState::new(files, cx)` → `Diff::new(&state)`；行号/语法高亮/hunk 分隔/变更指示条/soft wrap/折叠展开/选区复制/annotations/冲突解析 |
| 聊天列滚动 | 聊天列装配（自绘 `list()` + stick-to-bottom） | **0.8.0 轮已立项**：`ScrollBounce` 包裹拿 macOS 原生回弹；嵌套滚动独占语义作者已确认符合期望；验证点是与自动置底叠加、竖向 wheel 接管去重——PLAN-GPUKIT-0.8-ADOPTION PR 2 | `gpui_base::overscroll` / `ScrollBounce`（0.8.0 #3500：macOS 默认开启，动参对齐 NSScrollView，默认接管竖向滚轮） |
| @-chip 悬停（新能力，无手搓前身） | composer `Textarea` 的 `InputContent` token chip | **0.8.0 轮已立项**（作者确认）：`Textarea::on_token_hover` 悬停提示，第一版只做完整路径——PLAN-GPUKIT-0.8-ADOPTION PR 3 | `InlineTokenHoverEvent`（0.8.0：token + range + bounds + 进入/离开） |
| 分隔条 | `steer-agent-chrome-ui/src/divider.rs`（111 行；坐标数学在 `shell.rs` 根） | **0.8.0 轮已立项（作者确认，翻代决）**：handle 视觉态/拖拽发起换 `resize_handle`，shell 根坐标数学与 per-side payload 不动；验收 6px/`<|>`/双击重置像素不回归——PLAN-GPUKIT-0.8-ADOPTION PR 5 | 独立 `resize_handle`（0.8.0 #3302：handle 只发起拖拽，应用自持数学；`ResizablePanel::fixed` 备查） |
| 聊天列 transcript 架构 | 聊天列渲染管线（自绘 stick-to-bottom） | **不跟进（0.8.0 轮，作者确认）**：与聊天管线重建计划（PLAN-SESSION-MODEL-REDESIGN 等）冲突，重建落地前是双倍工作；落地后优先重评 → 下期确认清单 | `MessageScroller`（0.8.0 加 `logical_scroll_top`、append/prepend/splice、follow mode） |
| 语法高亮后端 | `gpui-component` feature `tree-sitter-languages`（全语言） | **0.8.0 本期不跟进（作者决定）**：syntect 三比（编译时间/二进制体积/高亮质量）→ 下期确认清单 | `syntect` feature（0.8.0 #3512：bat 语法集；同开时 tree-sitter 优先） |
| 语音输入（新能力） | 无 | **0.8.0 轮已立项（作者确认，翻代决）**：composer 按住说话，macOS 先行；Linux placeholder 须能力探测/`cfg` 隐藏，bundle-mac Info.plist 加麦克风权限——PLAN-GPUKIT-0.8-ADOPTION PR 6 | `speech` 模块（0.8.0 feature：macOS objc2-speech + Windows WinRT；麦克风/电平/波形组件） |
| webview 宿主 | `crates/steer-webview`（wry 直连 + Tauri 式 IPC） | **长期观察**：上游 `gpui-webview` 已三平台（Linux X11/XWayland）+ GPUI Fast 后端 overlay；每逢 minor 轮重新对位一次，评估替代可行性 | `gpui-webview`（0.8.0 #3395 改名自 gpui-wry） |

## 已评估、暂不跟进的结论（仅作背景缓存，不构成免评）

重评时若 API 大改或设计约束变化，仍须重新对照。

- **Toolbar/ToolbarGroup**：titlebar 是校准过的 2026-Light chrome（38px 高、traffic-slot 数学对位）。
- **Dock 组件**：`panel.rs` 是注入式 `PanelSurface` + mount-equals-launch 生命周期契约；gpui Dock 是另一套重系统。
- **Message/Bubble 聊天组件**：ai-elements 是对齐 Vercel AI Elements 语义的自有家族。
- **Questionnaire / TimeField / Carousel**：暂无产品场景。
- **plot 原语画 context ring**：见上表 ring 行——填充扇形 ≠ 圆头描边（0.7.0 实证；0.3.9 快照仍无 `LineCap` re-export，0.8.0 轮复核结论维持）。
- **内置补全菜单（`CompletionProvider`/`CompletionMenuOptions`/`ShowCompletions`）**：挂 `input/editor/lsp`，仅 EditorState；composer 是 TextareaState（0.8.0 实证）。
- **`gpui_base::text::editor_host`**（0.8.0）：外部编辑器渲染边界，本仓无自有文档模型的编辑器，无场景。

## 下期 minor 评估时须向作者确认的清单（0.8.0 轮沉淀，2026-10-23）

这些是上一轮**显式搁置**的项，不是永久否决。下期 gpui-kit minor 版本评估时逐项再次向 dspo/manox-app 作者确认是否跟进，不可静默续期；确认后销账（跟进 → 转上表立项行；再搁置 → 更新轮次留此）。0.8.0 轮第二轮问题经作者 2026-10-23 重新作答：divider、speech 翻案立项，粘贴图片/Combobox/Empty 转立项评估，均已从本清单移出。

| 项 | 搁置原因（0.8.0 轮） | 下期确认时附带的新信息 |
|---|---|---|
| syntect 三比（编译/体积/高亮质量） | 作者明确本期不做 | 上游 syntect 后端是否成熟（主题映射、语言覆盖） |
| `MessageScroller` 替代聊天列 | 作者确认：与聊天管线重建计划冲突，重建落地前是双倍工作 | 重建是否已落地；落地则优先重评 |
| `gpui-webview` 替代 `steer-webview` | 长期观察 | 三平台成熟度、IPC 模型是否覆盖 Tauri 式用法 |
