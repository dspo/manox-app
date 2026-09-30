# 手搓热点表（评估问二的对照面）

本表列出本仓手搓实现与其已知上游映射，供新版本评估时求交。

**维护规则**：adoption PR 合入时同步更新本表；「已知上游映射」标注所评估的版本，只是线索——每轮评估须对新版本重新核对（重评估原则见 SKILL.md）。

| 手搓点 | 位置 | 现状 | 已知上游映射（截至 0.7.0） |
|---|---|---|---|
| composer @-补全插入 | `crates/manox-agent-chat-ui/src/views/completion.rs`（`build_replacement`） | `InputContent` 字符串手术（prefix/trigger/suffix/caret 推进、尾随分隔符去重） | 0.7.0 原子 inline token：`replace_with_token` / `InputContent::with_token` / `on_token_click` / `ActivateToken`。不可用于 Editor/NumberInput/password/masked 模式；composer 是 InputState，可用 |
| 浮层 ×3：typeahead 补全列表 / goal status popover / context rail 气泡 | `completion.rs`；`column.rs`（`goal_popover_open`）；`views/context_rail.rs` | 零高 slot + 负 margin + `BubbleClearance` + 外点同手势比对 | 0.7.0 组件 Popover 补 `offset`（解「间隙不可调」痛点）与 `arrow(true)`，Root 接管 overlay 宿主。#94 气泡刚像素校准过，迁移需重走校准 |
| markdown 渲染栈 | `crates/manox-components/src/markdown/`（ast/incremental/rich_text/selection/theme） | 自研替换 `TextView::markdown`（流式、选择、copy 反馈、表格 copy） | 0.6.2 `stream_fade` / inline plugins / shaping 缓存；0.7.0 `set_range_highlights`≈copy 反馈高亮、`selected_source_range`≈表格 copy 的源码切片、`reveal_range`。terminal_panel 集成与代际守卫仍自研 |
| composer context ring | `crates/agent-ui/src/workspace/composer_render.rs` | `canvas` + `PathBuilder` + lyon `LineCap::Round`；像素校准 dsh ContextMeter（14px 盒、r 5.5、2px stroke） | 0.7.0 plot 原语下沉 `gpui_kit::base::plot`（scales/shapes/`PlotElement`）。Cargo.toml 直依赖 lyon 的唯一理由就是 ring 的 `LineCap` 类型，迁移成功可摘除 |
| 粘贴图片 | `crates/agent-ui/src/workspace/composer.rs`（stage clipboard image） | `read_clipboard` 自搓 | 0.6.2 Input/Textarea/Editor `on_paste` 原生事件（可拿剪贴板图片/文件） |
| 标题栏 session picker / 模型选择器弹出 | `manox-agent-chrome-ui/src/titlebar.rs`；`agent-ui/src/workspace/chips.rs` | 手搓下拉与弹出面板 | Combobox（0.6.0）；Popover `offset`（0.7.0） |
| 「No chats」空态 | `manox-agent-chrome-ui/src/session_list.rs` | 手搓 hint 行 | 0.6.2 `Empty` / `EmptyHeader` / `EmptyMedia` / `EmptyTitle` / `EmptyDescription` / `EmptyContent` 组件族 |
| 设置页表单 | `agent-ui/src/views/settings/` | 自绘行控件 + 点击 flash 动画 | Form / Field（0.7.0 `Field::visible(false)`）、GroupBox/SettingGroup footer（0.7.0） |

## 已评估、暂不跟进的结论（仅作背景缓存，不构成免评）

重评时若 API 大改或设计约束变化，仍须重新对照。

- **Toolbar/ToolbarGroup**：titlebar 是校准过的 2026-Light chrome（38px 高、traffic-slot 数学对位）。
- **Dock 组件**：`panel.rs` 是注入式 `PanelSurface` + mount-equals-launch 生命周期契约；gpui Dock 是另一套重系统。
- **Message/Bubble 聊天组件**：ai-elements 是对齐 Vercel AI Elements 语义的自有家族。
- **Questionnaire / TimeField / Carousel**：暂无产品场景。
