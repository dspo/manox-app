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
| markdown 渲染栈 | `crates/steer-components/src/markdown/` | **已迁移上游**（#145 删自托管栈，改直驱 gpui-base TextView）；会话代码栅栏落为 app 侧 editor block（feat/editor-code-block）：上游 `MarkdownExtensions` block 插件点整栅栏拦截（svg/mermaid/math 之后注册），header+行号 gutter+nowrap 高亮+限高滚动，copy 反馈走窗口 keyed state；diff/conflict 栅栏再加 side-by-side / 3-way merge 第二面（自写解析——上游 diff 模块数据层 pub(crate) 不可复用、Diff 组件自带虚拟滚动内嵌仍被 #220 判断否决，语义对齐其 Split/conflicts 模型） | `stream_fade`、`selected_source_range`、range highlights、block 插件点（0.7.0） |
| composer context ring | `crates/agent-ui/src/workspace/composer_render.rs` | **明确不跟进**（0.7.0 评估有证据）：plot 的 `Arc` 是 `PathBuilder::fill()` 饼图扇形，画不了 2px 圆头描边弧；且 gpui 只 re-export `StrokeOptions` 不含 `LineCap`——lyon 直依赖摘不掉。ring 保持 canvas 手搓 | plot `shape/arc.rs` 为填充式；后续版本若出描边弧或 re-export `LineCap` 再评 |
| 粘贴图片 | `crates/agent-ui/src/workspace/composer.rs`（stage clipboard image） | **已迁移（#222，0.8.0 轮 E1）**：capture_action 手搓退役，挂 `Textarea::on_paste` 拦截钩子——图片消费并暂存 attachment（alt-text 不进 rope）；无文本条目的剪贴板是复制文件，路径走 pending-attachment 管线而非贴进输入框（顺带修掉旧回退把路径串贴进框的 bug）；纯文本 fallthrough 引擎原子插入，`Change` 契约保 completion 重同步（gpui-base 源码证据链见 #222） | `on_paste`（0.8.0 官方拦截钩子：true 消费 / false 落入 clipboard.text()；图片/文件同钩） |
| 标题栏 session picker / 模型选择器弹出 | `steer-agent-chrome-ui/src/titlebar.rs`；`agent-ui/src/workspace/chips.rs` | **评估完成，不迁移（0.8.0 轮 E2）**：模型选择器是 PopupMenu 子菜单树（provider 分组 + reasoning effort 尾块），`Combobox`（SearchableListDelegate 搜索下拉）形态不匹配；titlebar session picker 能力上 Combobox 可覆盖，但已是成熟自绘面板（搜索过滤 + 最近 10 条）+ 像素校准 chrome，迁移需包 delegate 适配层，收益为负——留作背景缓存 | `Combobox`（0.6.0/0.8.0：IndexPath 选择、multiple/searchable） |
| 「No chats」空态 | `steer-agent-chrome-ui/src/session_list.rs` | **评估完成，不迁移（0.8.0 轮 E3）**：`Empty` 组件族是整页空态（media/title/description），`no_chats_hint` 是分组行内的轻提示，形态不匹配；出现整页空态需求时再评 | 0.6.2 `Empty` 组件族 |
| 设置页表单 | `agent-ui/src/views/settings/` | **已迁移**（feat/settings-adoption + feat/settings-mcp-rows）：sidebar/搜索/过滤/分页由 `Settings` 组件承担；语言行与 MCP 每 server 开关均为 `SettingField::{dropdown,switch}` 行级绑定，mock_switch/mock_dropdown 全部退役；Models/ChatGPT/VSCode/Plugins 仍为 `SettingItem::render` Element 整块，其内部原语（section_card/row_with_control 等）保留 | `setting` 模块（0.7.0 `Settings`/`SettingPage`/`SettingGroup`/`SettingItem`/`SettingField`，比 Form/Field 更贴合的映射）；Form / Field（0.7.0 `Field::visible(false)`）、GroupBox/SettingGroup footer（0.7.0）备查 |
| 工具卡 Edit diff 渲染 | `steer-agent-chat-ui/src/views/tool_cards.rs`（手解 unified diff）+ `ai-elements/src/code_view.rs`（646 行） | **已落地（#220）**：Edit 卡「在右栏打开 diff」chip → ChatHost 链 → workspace 单 DiffTab（内容替换、随线程切换退役）接 `gpui_component::diff` 全默认渲染；解析失败开页签显错不静默。聊天列内嵌不适用（虚拟化自滚动 + 嵌套滚动独占）维持判断；`CodeView` 保留（内嵌摘要 + Read 窗口/`Gap` 语义），瘦身评估后未做——右栏形态已吸收主要价值 | `diff` 模块（0.8.0）：`DiffFile::parse` → `DiffState::new` → `Diff`；行号/高亮/折叠/复制/指示条 |
| 聊天列滚动 | 聊天列装配（自绘 `list()` + stick-to-bottom） | **已落地（#216）**：`ScrollBounce` 包裹（与上游 MessageScroller 同范式：bounce 驱动 ListState、`on_scroll` 桥接 chat notify 供 turn rail 读取）；`FollowMode::Tail` 原生语义不受影响；探针钉 wheel 路由与两端 clamp（拉伸只动绘制） | `gpui_base::overscroll` / `ScrollBounce`（0.8.0 #3500） |
| @-chip 悬停（新能力，无手搓前身） | composer `Textarea` 的 `InputContent` token chip | **已落地（#217）**：`on_token_hover`（弱实体桥接 notify）+ deferred/anchored 手搓小卡片（`Tooltip` 组件形态与 anchored 定位不匹配）；registry key ≠ chip 短名才弹；悬停中删除/清空 chip 由 Change 路径清理（无 exit 事件也不悬挂） | `InlineTokenHoverEvent`（0.8.0：token/range/bounds/进出） |
| 分隔条 | `steer-agent-chrome-ui/src/divider.rs` | **已迁移（#218）**：手搓 handle 换 `gpui_base::resize_handle` band + 空外观（component 默认外观带常驻 hairline，破坏 2026-Light 隐形把手）；shell 根坐标数学与 per-side payload 原样；命中带 6→9px straddle（上游常量后置覆盖，6px 不可保持，取增宽不取变窄），光标等价核实；四张壳图 0 像素差；双击重置走 `.on_double_click` | 独立 `resize_handle`（0.8.0 #3302）；`ResizablePanel::fixed` 评估结论：现壳宽度是纯壳状态、天然跨窗口 resize 保持，无覆盖缺口 |
| 聊天列 transcript 架构 | 聊天列渲染管线（自绘 stick-to-bottom） | **不跟进（0.8.0 轮，作者确认）**：与聊天管线重建计划（PLAN-SESSION-MODEL-REDESIGN 等）冲突，重建落地前是双倍工作；落地后优先重评 → 下期确认清单 | `MessageScroller`（0.8.0 加 `logical_scroll_top`、append/prepend/splice、follow mode） |
| 语法高亮后端 | `gpui-component` feature `tree-sitter-languages`（全语言） | **0.8.0 本期不跟进（作者决定）**：syntect 三比（编译时间/二进制体积/高亮质量）→ 下期确认清单 | `syntect` feature（0.8.0 #3512：bat 语法集；同开时 tree-sitter 优先） |
| 语音输入（新能力） | 无 | **已落地（#221，macOS 先行）**：`speech` feature；`SpeechState::has_recognizer()` 运行时探测（Linux placeholder 恒隐藏、无 plist 键也隐藏，不留坏按钮）；点击开关式（上游 toggle 模型，含 connecting 态）；识别文本 `insert` 落光标 + 焦点回收；steer-app 内嵌 `__TEXT,__info_plist`（dev 态可弹权限窗）+ bundle-mac 双权限键；CI Linux 补 ALSA 头（cpal） | `speech` 模块（0.8.0：objc2-speech/WinRT；电平/波形组件 v1 未用） |
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
