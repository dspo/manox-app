# 手搓热点表（评估问二的对照面）

本表列出本仓手搓实现与其已知上游映射，供新版本评估时求交。

**维护规则**：adoption PR 合入时同步更新本表；「已知上游映射」标注所评估的版本，只是线索——每轮评估须对新版本重新核对（重评估原则见 SKILL.md）。

| 手搓点 | 位置 | 状态（2026-10-01，0.7.0 吸收轮） | 已知上游映射（截至 0.7.0） |
|---|---|---|---|
| composer @-补全插入 | `crates/manox-agent-chat-ui/src/views/completion.rs` | **已迁移**（#122，stacked 待合）：`build_replacement_content` 把插入名包成 `InputContent` token，chip 渲染 + 整块删除；文本字面量与 dispatch 语义不变 | 原子 inline token（`InputContent::with_token`）。composer 是 TextareaState，API 可用 |
| goal 状态下拉 | `agent-ui/src/workspace/chips.rs` | **已迁移**（#123，stacked 待合）：组件 `Popover` 受控 open，`deferred+absolute+occlude+mouse_down_out` 手搓退役 | `Popover::open/on_open_change/anchor/track_focus`；`Anchor` 在 gpui 本体 |
| context rail 气泡 | `agent-ui/src/workspace/composer_render.rs` | **已迁移**（#123）：组件 `Popover` + `arrow(true)`；slot/负 apron/`BubbleClearance`/同手势 suppression/手绘尾巴全删（净 −111 行），被列裁切的失败模式随 overlay 层消失 | 同上；外点关闭与 Escape 归库管（BasePopover trigger `stop_propagation` + Dialog 角色） |
| typeahead 补全列表 | `manox-agent-chat-ui/src/views/completion.rs` | **明确保留手搓**（#123 评估结论）：caret 锚定（无 trigger 元素）、绝不能抢焦点、键盘导航在 Workspace 绑定——组件 Popover 的 trigger+内容聚焦模型三条都不表达 | 0.7.0 组件 Popover 不适用；后续版本若出现 caret 锚定 + 非受焦浮层原语再评 |
| markdown 渲染栈 | `crates/manox-components/src/markdown/` | **重评完成，建议 spike 替换**（issue #124）：当年 fork 三动机全被上游补齐，0.7 新增 `markdown_block_parser/block_renderer` 可承载 terminal_panel；先 spike 量化 diff 再删栈 | `stream_fade`、`selected_source_range`、range highlights、block 插件点（0.7.0） |
| composer context ring | `crates/agent-ui/src/workspace/composer_render.rs` | **明确不跟进**（0.7.0 评估有证据）：plot 的 `Arc` 是 `PathBuilder::fill()` 饼图扇形，画不了 2px 圆头描边弧；且 gpui 只 re-export `StrokeOptions` 不含 `LineCap`——lyon 直依赖摘不掉。ring 保持 canvas 手搓 | plot `shape/arc.rs` 为填充式；后续版本若出描边弧或 re-export `LineCap` 再评 |
| 粘贴图片 | `crates/agent-ui/src/workspace/composer.rs`（stage clipboard image） | 待评估 | 0.6.2 Input/Textarea/Editor `on_paste` 原生事件（可拿剪贴板图片/文件） |
| 标题栏 session picker / 模型选择器弹出 | `manox-agent-chrome-ui/src/titlebar.rs`；`agent-ui/src/workspace/chips.rs` | 待评估 | Combobox（0.6.0）；Popover `offset`（0.7.0）——#123 落地后组件 Popover 已是仓内成熟路径 |
| 「No chats」空态 | `manox-agent-chrome-ui/src/session_list.rs` | 待评估 | 0.6.2 `Empty` 组件族 |
| 设置页表单 | `agent-ui/src/views/settings/` | 待评估 | Form / Field（0.7.0 `Field::visible(false)`）、GroupBox/SettingGroup footer（0.7.0） |

## 已评估、暂不跟进的结论（仅作背景缓存，不构成免评）

重评时若 API 大改或设计约束变化，仍须重新对照。

- **Toolbar/ToolbarGroup**：titlebar 是校准过的 2026-Light chrome（38px 高、traffic-slot 数学对位）。
- **Dock 组件**：`panel.rs` 是注入式 `PanelSurface` + mount-equals-launch 生命周期契约；gpui Dock 是另一套重系统。
- **Message/Bubble 聊天组件**：ai-elements 是对齐 Vercel AI Elements 语义的自有家族。
- **Questionnaire / TimeField / Carousel**：暂无产品场景。
- **plot 原语画 context ring**：见上表 ring 行——填充扇形 ≠ 圆头描边（0.7.0 实证）。
