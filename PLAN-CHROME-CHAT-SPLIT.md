# 拆分计划：manox-agent-chrome-ui + manox-agent-chat-ui

> 状态：**Phase 1/2/3 全部落地；Phase 4 进行中（前哨 + 批1-3 已开）**（截至 2026-09-22：
> Phase 3 = #58；1 = #59；2 = #60+#61+#63+#64；4 前哨 = #62；批 1 = #65 五态 props+投影；
> 批 2 = #66 双壳落地（--features chrome-shell 挂真装配）；**批 3 = PR #67
> `feat/chat-column-view`（叠 #66）——chrome 构建能真聊天**：Workspace 增嵌入渲染模式
> （new_embedded：只渲染会话列——hero/虚拟化列表/composer footer/overlay/rail/turn
> navigator，动作面抽成两壳共享的 apply_chat_actions 根装饰器；旧壳路径零改动）；
> chrome 装配改为单嵌入 Workspace——其 multiplexer 同时喂侧栏投影与会话列，侧栏选中走
> 生产 open_thread 全切换路径，⌘N 走 start_new_thread，标题栏跟随前台线程标题；
> #66 的占位主面退役；抽出的 hero 引用带出一个此前未扫到的 i18n 键
> workspace-hero-heading（双语补齐，key-scan 门禁继续执法）。剩余：批 4（浏览器/CLI
> 页签随宿主解耦 + per-session 页签集）→ 收敛；**修复批 = PR #68**：tranche-3 真机报告两项——侧栏选中死（装配 on_select/on_new_session 钩子
> 因替换失误以 None 提交，现接生产 open_thread/start_new_thread）+ 底部终端随线程走
> （Shell take/set_panel_view 无拆分离槽 + 装配按线程 id stash/restore——旧右栏同语义，
> 仅显式收起拆进程；新拉起 cwd=前台线程 cwd；**修复批 2 = PR #69**：①chrome 构建补注册 codicon（豆腐块根因——example 注册了、
> bin 的 chrome 分支没注册）；②message.rs 四处 conv.update 嵌套双重租约（点 thinking/工具
> 折叠即 abort，日志实证）全部展平、grep 清零；③右栏 per-thread 会话集
> （RightPaneSession stash/restore——打开集/内容仓/激活页签/可见性整体迁移，on_active
> 生命周期驱动：浏览器子视图隐藏、终端保活，仅显式关页签才拆；装配切线程泵 stash 旧
> 恢复新，无 stash 落新标签页空态；内容渲染归页签实例、壳只管 chrome 与生命周期——
> 即用户裁定的右栏分工；**批 4 = PR #70**：右栏注册表补全——AgentTool（claude/codex/copilot 全 cx 启动：mux wire
> 模型行解析 provider/model + AgentBuilder PTY relay + CxSessionSource TUI，cwd=前台线程）+
> EditorTool（markdown 写作面）；工厂 mux 携带、开时实时解析；新 kind 免费继承 #69 的
> per-thread 会话集。浏览器页签为唯一未迁项（宿主绑 Workspace；**修复批 3 = PR #71**：①页签品牌图标（gpui-component Icon 自定义 path——旧壳侧栏同款，
> 裸 gpui::svg 样式错）；②agent 模型菜单回归——页签体=模型选择器（共享级联投影
> cascade_provider_groups：agents 可见性过滤/去重/显示名分组/wire 键；点选即以该端点启动，
> picker 实体自渲染 TUI——页签生命周期单实体，错误留守选择器）；③cwd 继承——前台 cwd 改由
> 活动线程 wire 行 project 列驱动（store.cwd 记录的是 workspace cwd=home，此前全部落在 ~）；
> 终端/CLI/底部 dock 全部以线程项目目录为根。

> Phase 1 落地差异记录（相对 §3 原案）：以单次机械化提交交付（原 1a-1e 切片
> 是为手工编辑去险；实际为脚本改写 + 编译错误驱动补链 + diff 复核），ChatHost
> 推迟至 Phase 2（嵌入结构体阶段无跨实体调用面，先立必为死代码）；顺带发现
> main 预存 test-support 套件失败（ask_card_synthesized…，非确定性断言，基线
> 同样失败，默认门禁看不见——#52/#56 同类坑）。

> Phase 3 落地差异记录（相对 §5 原案）：chrome lib 依赖面比原案更紧——
> **不含** terminal-ui / manox-webview（ToolTab 实现归装配层，经 dev-deps 供
> example/test 使用），仅 gpui + gpui-component + manox-i18n；i18n key-scan
> 门禁（manox-i18n 单测）已扩展扫描 chrome 源码（needle 双轨：`i18n::t` /
> `manox_i18n::t`）；ToolTab 契约已按 D3 升级为实例级 id + ToolTabFactory
> （kind 携带在 tab 上，"+ 按活跃 kind 开新实例"已可用）；per-session 页签集
> （挂起/恢复）仍留装配阶段。离屏截屏偶发混入真实屏幕内容（合成竞态），复跑
> 即净，验收以多跑两遍为准。

---

## 0. 目标与一句话结论

把 manox-app 的 UI 层拆成三块职责：

```
窗口 = chrome 壳子( manox-agent-chrome-ui )
       ├── 主栏槽位 MainSurface ← manox-agent-chat-ui（聊天主栏）
       ├── 右侧栏 ToolTab 注册表 ← 装配层供给（浏览器/终端族/外部会话/编辑器/子代理面板）
       └── 底部面板 / 标题栏 / 侧栏 / 布局
```

- **chrome** 来自 agents-window-gpui 的搬运与抽象（~2.5k 行 + 适配）；
- **chat** 不是从 agents-window-gpui 搬（那里主栏是占位卡 "Not Implement yet"），
  而是把本仓 agent-ui 现有 MessageColumn 系从 `Workspace` 单体里外科切出（~15k 行动土，大头）；
- agent-ui 瘦身为**状态 + 装配层**（multiplexer / client_store / external_session /
  browser_host / dispatch / sidebar 投影），manox bin 负责最终装配。

## 1. 已锁定的决策（2026-09-21，用户拍板）

| # | 决策点 | 结论 |
| --- | --- | --- |
| D1 | chrome 视觉体系 | **保留 VS Code 2026-Light 令牌**：agents-theme 色板/字体常量/codicon 并入 chrome crate，像素校准成果（±1px 地标）直接继承；暗色模式留作后续第二套令牌，不阻塞本次。chrome 与 chat 各用各的视觉体系。 |
| D2 | 侧栏语义正统 | **SessionList 为底盘，props 模型扩展**：复刻版 SessionList 视觉 + manox 语义（五态/team 嵌套/tag/外部行/排序 reconcile/Show-more）作为投影层喂 props。sidebar.rs 的渲染层死亡、投影逻辑留状态层。 |
| D3 | 右栏契约 | **升级为实例级页签 + 按会话分组**：ToolTab id 从 `'static` 类别 id 改为实例 id（多浏览器/多终端页签），支持 per-session 页签集（保 manox 线程绑定：per-thread stash、SubagentPanel 随线程、threads.db 持久化）。 |
| D4 | 落地顺序 | **先 chat 后 chrome**：agent-ui 内先立 ChatColumn 分界（纯重构零视觉变化）→ 拆出 chat crate → 再立 chrome crate 换壳装配。视觉变化集中在最后一个 PR 系列。 |

推论与边界规则：

- **依赖不变量（CI 可查）**：`manox-agent-chat-ui` 不得依赖 `terminal-ui` /
  `manox-webview` / `manox-ext-agents`（终端/浏览器/外部会话归 chrome 与装配层）；
  `manox-agent-chrome-ui` 不依赖 `manox-agent` / `manox-harness` / `manox-session-core`
  （chrome 无数据语义，数据一律经 props/适配器注入）。agent-ui 可依赖两者。
- chrome crate 的文案是 app chrome → 全部走 `manox-i18n`（本仓 i18n 规则恰好覆盖：
  侧栏/标题栏/页签/菜单均属 app chrome）。chrome 依赖 manox-i18n 合法。
- 单二进制、单进程交付不变；不维护旧壳升级路径（激进纪律：Phase 4 删净，不留开关）。

## 2. 目标架构

### 2.1 crate 拓扑（Phase 4 终态）

```
crates/
  manox-agent-chrome-ui   壳子：设计令牌 / primitives / SessionList(扩展) /
                           titlebar / RightPane + ToolTab(升级) / panel /
                           divider / Shell 布局 + MainSurface 注入 + 主区模式切换
                           + 离屏视觉验收 harness
  manox-agent-chat-ui     聊天主栏：消息管线 / conversation / composer /
                           AskDrawer / PlanReview / FollowStop / Completion /
                           TurnNavigator / ContextRail
  agent-ui                状态 + 装配层：multiplexer / client_store(+handle) /
                           external_session / browser_host / dispatch /
                           sidebar 投影 / git_status / ChatHost 实现
  manox (bin)             窗口 / 托盘 / 菜单栏 / dock badge / 最终装配
```

依赖方向：

```
manox ─→ agent-ui ─→ { manox-agent-chrome-ui, manox-agent-chat-ui }
chrome-ui ─→ { gpui, gpui-component, terminal-ui, manox-webview, manox-i18n,
               manox-terminal(PTY 类型), manox-components(基础渲染件,按需) }
chat-ui   ─→ { gpui, gpui-component, manox-components, ai-elements, manox-i18n,
               manox-protocol, manox-session-core(事件/投影类型) }
```

（terminal-ui / manox-webview 从 agents-window-gpui 视角的 git 依赖变为本仓
workspace 成员，搬运反而变简单。）

**agent-ui 保留，不拆光**（2026-09-21 裁决）：拆掉的是它的 UI 渲染职责
（chat 列、消息管线、右栏、侧栏渲染、壳子布局），状态与装配职责全留下——
multiplexer / client_store 存储本体 / 外部会话生命周期 / browser_host /
dispatch / 侧栏投影（喂 chrome SessionList 的 props）/ Settings 全套视图
（第二个 MainSurface 实现）/ slash_command 等共享件 / ChatHost 实现 /
ToolTab 注册与装配。重构期间**保留现名**（历史连续、diff 噪声最小）；
Phase 4 结束后按残留职责复审是否改名（届时改名是纯机械操作）。

### 2.2 装配公式

```rust
// manox bin（示意）
let chat   = ChatColumn::new(state_face, host, window, cx);   // chat-ui
let shell  = chrome::Shell::new(ChromeConfig {
    main: MainSurface::chat(chat),                             // 主栏槽
    tabs: app_registry(),                                      // ToolTab 注册表
    ..default()
}, window, cx);
```

- 主区模式切换由 chrome 的 MainSurface 槽承担：chat 是第一个实现，SettingsView
  是第二个（**仅此两种**；终端/外部会话的全窗诉求不经主槽，走 dock 最大化）。
- ViewMode 旧语义去向（2026-09-21 裁决）：
  - `ViewMode::Workspace` → 主槽 = ChatColumn；
  - `ViewMode::Settings` → 主槽替换 = SettingsView（壳子管切换动画）；
  - `ViewMode::Terminal` → 底部面板（默认展开）**最大化**；
  - `ViewMode::ExternalSession` → 右栏 SessionTab（实例页签，D3 语义），
    全窗 attach = **最大化该页签**。
  - 机制：chrome 一个统一的「最大化当前 dock 面」动作（底部面板/右栏页签
    同款），被放大的 dock 占满主区、聊天实体保持挂载、收起即回；
    **chrome 不引入 ViewMode 枚举**。

---

## 3. Phase 1 — agent-ui 内立 ChatColumn（纯重构，零视觉变化）

**目的**：在不动 crate 边界的前提下，把 `Workspace`（workspace.rs:622-950，
约 90 个具名字段 + 若干 HashMap 簇）的职责切成两个实体，用现有 5078 行
workspace 测试（`workspace/tests.rs`）做守护网。这一步是全部四个阶段里唯一
的"手术"，后续阶段是机械搬移。

**产出**：
1. `ChatColumn` 实体（建议 `crates/agent-ui/src/chat_column.rs` 起步）；
2. `ChatHost` 端口（chat → 壳子/状态的回调面）；
3. `Workspace` 保留壳子字段 + 作为 ChatHost 实现；
4. 全量测试绿、`--features test-support` 绿（教训：test-support 编译单元两次
   被默认门禁漏掉，改 message/workspace 渲染 API 必跑）、视觉零变化。

### 3.1 Workspace 字段二分表

依据 workspace.rs:622-950 逐字段核对（实施时以当日代码为准，新增字段按
同一规则归类）。**C** = ChatColumn，**S** = 壳子（未来 chrome/装配），
**L** = 状态层（留在 agent-ui），**X** = 跨界（走端口，见 3.2）。

| 字段（workspace.rs 行号） | 归属 | 说明 |
| --- | --- | --- |
| `cwd` (623) | L | 工作目录，状态层 |
| `thread` (624) | C | 活动线程句柄（chat 数据面） |
| `store` (631) | C | ClientStoreHandle 叶子镜像 |
| `multiplexer` (635) | L | 会话集合 demux，chat 经端口取叶子 |
| `client` (638) | L | send_note/Reply 通道 |
| `session_id` (641) | C | 前台会话 id |
| `background_threads` (647) | L | 后台线程管理 |
| `git_status_gen` (652) | C | ContextRail 的 git 刷新代（rail 归 chat） |
| `sidebar` / `sidebar_sub` (653, 858) | S | 侧栏 |
| `_mux_lists` (659) | L | 列表/registry 观察订阅 |
| `conversation` (660) | C | |
| `input_state` / `input_sub` (661, 859) | C | composer 输入 |
| `drafts` (665) | C | per-thread composer 草稿 |
| `recall_index` / `recall_draft` (669, 673) | C | 历史回溯 |
| `editor_drafts` … `editor_preview_scroll` (679-697) | S | 右栏编辑器（per-thread 草稿随 D3 分组走壳子） |
| `editor_sub` (860) | S | |
| `right_tabs` / `active_right_tab` / `right_pane_visible` / `right_pane_by_thread` / `hovered_right_tab` (701-711) | S | 右栏全部 |
| `browser_title_ticker_gen` (714) | S | |
| `launcher_menu` / `launcher_menu_sub` / `launcher_menu_kind` (717-721) | S | |
| `subagent_panels` / `subagent_transcripts` / `subagent_final_text` / `subagent_prompts` (723-735) | X | 面板归壳子；转录数据源是线程事件（chat 侧），经端口推送 |
| `browser_views` (740) | S | |
| `editor_width` (742) | S | |
| `sidebar_width` / `sidebar_visible` (746, 751) | S | |
| `pending_ask` … `ask_skipped` (753-789) | C | AskDrawer 全族 |
| `model_open` / `model_menu(+sub)` (790-793) | C | composer chip |
| `plus_open` / `plus_menu(+sub)` (794-796) | C | |
| `access_open` (798) | C | |
| `project_chip_open` / `project_chip_menu(+sub)` (799-802) | X | chip 归 chat；项目注册表操作是壳子/状态职责 |
| `completion` (807) | C | |
| `turn_navigator` / `turn_navigator_sub` / `turn_navigator_previous_focus` (809-811) | C | |
| `queued_follow_ups` / `queued_follow_ups_by_thread` / `queue_drag` (815-822) | C | |
| `composer_placeholder_mode` (825) | C | |
| `pending_attachments` (827) | C | |
| `active_browser_suites` (831) | C | 会话级工具激活，随 composer |
| `project_picker_pending` / `blank_project_parent` / `blank_project_name_input` (835-839) | X | overlay 在 chat；项目写操作走端口 |
| `thread_sub` (840) | C | 前台线程事件 |
| `store_observe` (848) | C | |
| `pending_successor` (852) | C | 绑定身份交接（前台切换） |
| `fork_in_flight` (857) | C | |
| `conversation_sub` (866) | C | |
| `list_state` / `message_list_width` / `list_count` (878-885) | C | 消息列表虚拟化 |
| `view_mode` / `exiting_settings` / `settings_transition_gen` / `settings_view` / `settings_sub` (888-919) | S | 主区模式 |
| `goal_popover_open` / `goal_ticker_gen` (901-906) | C | composer chip 族 |
| `turn_active` / `thinking_ticker_gen` (911-915) | C | |
| `terminal_view` (922) | S | |
| `context_rail` (927) | C | rail 是 chat 列 overlay |
| `external_sessions` / `resumable_external` / `resuming_external` / `cli_session_claims` / `active_external` (932-949) | L/S | 外部会话管理在状态层，呈现归壳子（D3 页签） |

**实施纪律**：切分以"borrow 冲突最小"为单位逐簇迁移（如 ask 簇、composer
簇、列表虚拟化簇），每簇一个 commit，全量测试随跑；`X` 类字段最后处理，
处理前端口必须先立。

### 3.2 ChatHost 端口草案（初版，Phase 1 手术中按实际调用扩）

chat 对壳子/状态的全部需求面，禁止 chat 持有 `Entity<Workspace>`：

```rust
pub trait ChatHost: 'static {
    // —— 主区切换（壳子职责）——
    fn open_settings(&self, cx: &mut App);
    fn toggle_editor(&self, cx: &mut App);              // 右栏编辑器页签
    fn open_browser_tab(&self, url: Option<String>, cx: &mut App);
    fn focus_terminal(&self, cx: &mut App);
    // —— 会话管理（状态层职责）——
    fn archive_thread(&self, id: &str, cx: &mut App);
    fn background_current_thread(&self, cx: &mut App);
    fn spawn_external_session(&self, spec: ExternalSpec, cx: &mut App);
    fn open_subagent_panel(&self, address: &str, cx: &mut App);  // 转录订阅挂接
    // —— 项目（状态层）——
    fn recent_projects(&self, cx: &App) -> Vec<ProjectRef>;
    fn set_project(&self, path: PathBuf, cx: &mut App);
    // —— 杂项 ——
    fn cwd(&self) -> PathBuf;
}
```

方向：Workspace 实现该 trait；Phase 3 后由装配层的新状态对象实现，chat 不改。

### 3.3 Phase 1 的 PR 切法

| PR | 内容 | 验收 |
| --- | --- | --- |
| 1a | `ChatColumn` 空实体 + 渲染委托（render 先整体转发，字段不动） | 全测绿，零视觉 |
| 1b | ask 簇 + composer 簇迁入（含 chips/completion/queue） | 同上 |
| 1c | 消息列表虚拟化簇 + 转录管线（thread_sub/store_observe/conversation_sub） | 同上 |
| 1d | ContextRail + turn_navigator + goal/thinking ticker | 同上 |
| 1e | `ChatHost` 立面（X 类字段清账）+ UI-MAP 同步 | 同上 + UI-MAP 更新 |

---

## 4. Phase 2 — 拆出 crate `manox-agent-chat-ui`

### 4.1 模块搬移清单（agent-ui → chat-ui）

| 源（crates/agent-ui/src/） | 行数 | 去向 | 备注 |
| --- | --- | --- | --- |
| `chat_column.rs`（Phase 1 产物） | — | `chat-ui/src/column.rs` | 主实体 |
| `client_store_handle.rs` | ~? | chat-ui | 前台叶子 gpui 镜像，ConversationState 直接消费；随迁（2026-09-21 裁决）。multiplexer 经 agent-ui→chat-ui 依赖创建 handle 实体，依赖方向合法 |
| `conversation.rs` | ~? | chat-ui | 会话状态 |
| `views/message.rs` | 5121 | chat-ui `views/message.rs` | 消息管线（最大单件，可同 PR 内再按 MessageItem 变体分文件） |
| `views/completion.rs` | 409 | chat-ui | |
| `views/turn_navigator.rs` | 546 | chat-ui | |
| `views/context_rail.rs` | 1278 | chat-ui | rail = chat 列 overlay |
| `views/composer_menu.rs` | 408 | chat-ui | |
| `views/popup_menu.rs` / `title_menu.rs` | 100/136 | 按消费方拆 | title_menu 偏壳子，逐函数核对 |
| `workspace/composer.rs` / `composer_render.rs` / `chips.rs` | 1021/1611/1423 | chat-ui `composer/` | |
| `workspace/plan_review.rs` | ~? | chat-ui | |
| `journal_translate.rs` / `journal_fold.rs` / `server_note_translate.rs` | ~? | chat-ui | journal→UI 投影 |
| `cockpit.rs` | ~? | chat-ui | rail 相位 |
| `git_status.rs` | ~? | chat-ui | rail 消费 |
| `slash_command.rs` | 794 | 留 agent-ui（共享 registry 面）→ 视依赖再定 | 若只被 composer 消费则随迁 |
| `source_gates.rs` / `overlap_diag.rs` | 432/~? | 随其消费者 | 诊断/门控，核对后归属 |

**留在 agent-ui**：`multiplexer.rs`、`client_store.rs`（journal 存储本体；
其 gpui 镜像 `client_store_handle.rs` **随迁 chat-ui**，见上表）、
`external_session.rs`、`browser_host.rs`、`browser_view.rs`、`dispatch.rs`、
`sidebar_view.rs`、`views/sidebar.rs`（投影部分）、`views/launcher.rs`、
`views/model_cascade.rs`、`views/subagent_panel.rs`、`views/subagents.rs`、
`views/settings/*`、`views/management_shell.rs`、`views/plugin_manager.rs`、
`menu.rs`、`i18n.rs`、`assets.rs`、`workspace/external.rs`、`workspace/attach.rs`
（按消费者核对）、`workspace/right_pane.rs`（Phase 4 死亡）、`workspace/render.rs`
（Phase 4 重写）、`workspace/tests.rs`（按被测对象随迁或留守，预计大头随
ChatColumn 与状态层分两处）。

### 4.2 actions / keybindings 处置（lib.rs `gpui::actions!`）

| action | 归属 |
| --- | --- |
| `AskPrev` `AskNext` `AskCancel` `CompletionUp/Down/Confirm/Dismiss` `ComposerRecallUp/Down` `UndoLastQueued` `ToggleTurnNavigator` `CopySelectedTurn` `FillComposerTurn` `ToggleCockpitTasks` | chat-ui（crate 内定义，agent-ui re-export 保持 main.rs 绑定面稳定） |
| `ToggleEditor` `ToggleEditorPreview` `CloseEditor` `OpenSettings` `NewTerminalTab` `CloseTerminalTab` `FocusTerminal` `FocusConversation` `OpenBrowserTab` `CloseBrowserTab` `BackgroundCurrentThread` `ArchiveCurrentThread` | agent-ui/壳子（Phase 3 后归 chrome 或装配层定义） |

`composer_recall_key_bindings()` / `turn_navigator_key_bindings()` 随 chat-ui 走。

### 4.3 Cargo.toml 草案（chat-ui）

```toml
[dependencies]
# 与 agent-ui 现有 workspace 依赖对齐，剔除：terminal-ui / manox-webview /
# manox-ext-agents / manox-agent(重) —— 数据面经 protocol/session-core 类型 + 端口
gpui, gpui-component, manox-components, ai-elements, manox-i18n,
manox-protocol, manox-session-core, anyhow, serde, serde_json, tokio,
tracing, chrono, futures, uuid(如消息 id 需要)
```

若编译证明 chat 侧确需 `manox-agent` 类型（ThreadHandle 等），**允许**依赖
manox-agent（数据 crate，非 UI），但不得引入其 UI 依赖面；在 PR 描述中显式记录。

chat-ui 定义自己的 `test-support` feature（diagnostic 钩子随 ChatColumn 迁入），
并承接 `tests/workspace_overlap.rs`——其 walk 对象是消息渲染重叠，随消息管线
迁移（2026-09-21 裁决）。

### 4.4 验收

- `cargo clippy -D warnings --all-targets` + 全量 `cargo test`（含
  `--features test-support`）+ `cargo fmt`；
- 依赖不变量检查（§1 推论）落地为 CI 或 script 检查（可用
  `cargo tree -p manox-agent-chat-ui -i terminal-ui` 断言 not found）；
- UI-MAP 同 PR 更新（组件 Source 路径全部改指）。

---

## 5. Phase 3 — 搬运并升级出 crate `manox-agent-chrome-ui`

### 5.1 逐文件搬运处置表（agents-window-gpui → manox-agent-chrome-ui）

| 源（agents-window-gpui） | 行数 | 处置 | 目标 |
| --- | --- | --- | --- |
| `agents-theme/src/palette.rs` `icons.rs` `lib.rs` | 85+162+21 | **照搬** | `chrome-ui/src/theme/`（保留 2026-Light 令牌，D1；作为 chrome 私有令牌，不喂全局主题） |
| `agents-elements/src/primitives.rs` | 104 | **照搬** | `chrome-ui/src/primitives.rs` |
| `agents-elements/src/session_list.rs` | 685 | **改造**（props 模型扩展，§5.3） | `chrome-ui/src/session_list.rs` |
| `agents-app/src/parts/right_pane.rs` | 492 | **改造**（契约升级，§5.4 + 会话分组） | `chrome-ui/src/right_pane.rs` |
| `agents-app/src/parts/titlebar.rs` | 376 | **改造**（文案 i18n；session picker 数据经注入） | `chrome-ui/src/titlebar.rs` |
| `agents-app/src/parts/panel.rs` | 92 | **改造**：通用底部 dock 容器——页签内容注入（与右栏 ToolTab 同款契约或其子集，Phase 3 设计 PR 定），终端实例归装配层；支持 dock 最大化；文案 i18n | `chrome-ui/src/panel.rs` |
| `agents-app/src/parts/divider.rs` | 107 | **照搬** | `chrome-ui/src/divider.rs` |
| `agents-app/src/parts/content.rs` | 160 | **改造**（chat_card 占位 → MainSurface 槽，§5.5） | `chrome-ui/src/layout.rs` |
| `agents-app/src/state.rs`（Shell） | 585 | **改造**（数据源抽离：sessions/pin/archive 经 props+回调注入；主区模式切换） | `chrome-ui/src/shell.rs` |
| `agents-app/src/parts/tool_tabs.rs` | 401 | **剥离**（默认注册表/终端拉起/浏览器视图是本项目实现 → 装配层；`spawn_terminal` 工具函数随装配走） | 不进 chrome |
| `agents-app/src/parts/editor_part.rs` | 254 | **丢弃**（静态演示） | — |
| `agents-app/src/manox_source.rs` | 122 | **剥离**（thread_store 适配器 → agent-ui 状态层） | — |
| `agents-app/src/data.rs` | 55 | **丢弃**（编辑器卡静态数据） | — |
| `agents-app/src/assets.rs` + `assets/` | 30 | **合流**（品牌 SVG 与本仓 `agent-ui/assets` 同源，合并进装配层资产源；codicon.ttf/sf-mono.ttf 移入 chrome 或 bin 的字体注册，§5.7） | — |
| `agents-app/src/lib.rs` `configure()` | 121 | **重写**（全局主题灌槽与 MANOX_HOME hack 不搬，§5.6；chrome 提供 `chrome::init(cx)` 只做自身所需的注册） | `chrome-ui/src/lib.rs` |
| `agents-app/src/main.rs` | 47 | 参考（窗口几何参数：交通灯 (12,13)、`app_owns_titlebar_drag`、1280×820/min 980×640、Opaque 背景 → manox bin 参照改） | — |
| `agents-app/tests/visual.rs` | 66 | **照搬改造**（离屏视觉验收 harness 成为本仓 chrome 回归测试；`AW_SHOT`/`AW_RIGHT`/`AW_PANEL` 变量保留） | `chrome-ui/tests/visual.rs` |

### 5.2 chrome crate 公开面（草案）

```rust
pub mod theme;        // 令牌 + icon()
pub mod primitives;
pub mod session_list; // props 组件
pub mod titlebar;
pub mod right_pane;   // RightPane + ToolTab + TabStore（升级版）
pub mod panel;
pub mod divider;
pub mod layout;       // [侧栏 | 主区[MainSurface | 右栏] / 底部面板]
pub mod shell;        // Shell 根视图 + ChromeConfig
pub mod main_surface; // MainSurface 契约
```

chrome 拿数据的方式一律 props + 回调（SessionList 已是范本）；唯一实体状态是
布局与页签生命周期（open/active/close、宽度、可见性、会话分组切换）。

### 5.3 SessionList props 模型扩展（D2）

在复刻版 props 上新增 manox 语义字段（渲染密度仍是两行 46px 卡片底盘）：

```rust
pub struct SessionRowData {                    // 扩展后
    pub id: String,
    pub title: String,
    pub time: String,
    pub status: SessionStatus,                 // 五态化：Errored/NeedsAuth/NeedsPlan/Running/Unread/Idle
    pub pinned: bool,
    pub unread: bool,
    // —— manox 扩展 ——
    pub tag: Option<String>,                   // 用户标签 chip（内联改名/清除）
    pub short_id: Option<String>,              // 点击复制完整 id；running 时 shimmer
    pub kind: RowKind,                         // Thread{archived} / External{agent, resumable, resuming}
    pub indent: u8,                            // team 嵌套层级（14px/级 + 1px 参考线）
    pub team_leader: bool,                     // 折叠 chevron
    pub team_collapsed: bool,
    pub wash: Option<WashColor>,               // permission-mode 洗色
    pub pending_spinner: bool,                 // pending_auth 旋转 + tooltip
}
pub struct SessionGroupData {                  // 分组扩展
    pub name: String,
    pub collapsed: bool,
    pub rows: Vec<SessionRowData>,
    pub show_more: Option<ShowMore>,           // 折叠配额（COLLAPSED_ROWS=5，leader 单元完整）
    pub order_slot: OrderSlot,                 // 手动排序位（插入线宿主）
}
```

投影层（agent-ui 状态层）负责 `from_wire`/team forest/排序 reconcile/外部行
合并——**逻辑从 sidebar.rs 原样搬**，只把输出类型从渲染调用改为 props 构造。

**侧栏语义对齐验收清单**（Phase 3 验收逐项打勾，缺一即回退）：

- [ ] 五态图标机：errored 三角 / pending_auth info / running success+旋转 /
      unread info / idle foreground（含 background_work 刷新）
- [ ] team 森林：leader chevron 折叠、member 缩进、1px 参考线、配额截断不拆 leader
- [ ] short-id Tag chip（点击复制、running shimmer）
- [ ] 用户 tag chip（✕ 清除、双击改名、10 字 clamp）
- [ ] 外部会话合并行：品牌 SVG、cx id 前缀、hover close、resumable dimmed+Play、
      resume spinner、OSC title 镜像
- [ ] 排序模式切换（Updated/Manual）+ Manual 落地时 reconcile（LCS 批次 MoveThread）
- [ ] 行拖拽排序（插入线、仅 Manual 落服务端、拖拽中断清标记）
- [ ] 分组拖拽排序（复刻版已有，保留）
- [ ] Show-more 折叠与展开（transient per mount）
- [ ] permission-mode 洗色（hover/active/selected 三态）
- [ ] pending_auth spinner + "Waiting for approval" tooltip
- [ ] selection-slide wash 动画
- [ ] 置顶星标领先分区
- [ ] 标题单行 truncate（防任何标题撑高行）

### 5.4 ToolTab 契约升级（D3）

```rust
pub trait ToolTab: 'static {
    fn kind(&self) -> &'static str;          // 类别（注册表键、图标/快捷操作来源）
    fn instance_id(&self) -> &str;           // 实例 id（打开集合键；如 browser-<n>）
    fn title(&self) -> SharedString;         // 实例标签（页签文字，可动态）
    fn icon(&self, cx: &App) -> AnyElement;
    fn quick_action(&self) -> Option<SharedString>;   // 新标签页空态；None=不列
    fn open(&self, window: &mut Window, cx: &mut App, store: &mut TabStore);
    fn render(&self, window: &mut Window, cx: &App, store: &mut TabStore) -> AnyElement;
    fn close(&self, store: &mut TabStore) { store.reset(self.instance_id()); }
    fn on_active(&self, visible: bool, cx: &mut App, store: &mut TabStore) {}
    // —— 会话分组（D3）：整个页签集随会话键切换挂起/恢复 ——
    fn suspend(&self, store: &mut TabStore) -> Option<TabSnapshot> { None }
    fn resume(&self, snap: TabSnapshot, window: &mut Window, cx: &mut App, store: &mut TabStore);
}

pub struct RightPane {
    pub session_key: Option<SessionKey>,          // None = 全局页签集
    stashes: HashMap<SessionKey, PaneStash>,      // per-session 页签集
    ...
}
```

设计留白（Phase 3 设计 PR 定）：挂起点放 RightPane 整体（tab set swap）还是
逐 tab（suspend/resume），倾向**整体 stash + tab 级钩子**（与 manox
`right_pane_by_thread` + `PersistedRightPane` 的 threads.db 快照语义对齐：
快照序列化仍由装配层持有，chrome 只管实体生命周期）。

### 5.5 MainSurface 契约（主栏槽）

```rust
/// 主栏内容面。chat 是第一个实现；SettingsView 是第二个。终端/外部会话的
/// 全窗诉求走 dock 最大化，不经主槽。
pub trait MainSurface: 'static {
    fn view(&self) -> AnyView;                       // 挂载的 gpui 视图
    fn title(&self, cx: &App) -> SharedString;       // 标题栏中段文案（session 下拉触发器）
    fn activate(&self, window: &mut Window, cx: &mut App) {}
    fn deactivate(&self, cx: &mut App) {}
}
```

titlebar 的 session 下拉选择器保留（复刻版偏离项之一）：候选列表数据经
`ChromeConfig.session_source` 注入（装配层给 multiplexer 投影）。

### 5.6 明确不搬清单

1. **MANOX_HOME 重定向 hack**（`redirect_manox_home_for_terminal_theme`）——
   本仓终端主题走自身 settings 链路，chrome 的终端底色由装配层给
   `terminal-ui` 的主题桥；
2. **全局组件主题逐槽灌 2026-Light**——全局 `gpui_component::Theme` 归属由
   manox bin 统一决定（现有字体令牌 JetBrains Mono NL/Lilex 保留与否是独立
   议题，不绑本计划）；chrome 自身显式着色，不依赖全局主题（复刻版现状即如此）；
3. `AGENTS_WINDOW_RIGHT` 调试开关改为 chrome 测试用例内开关（或 `chrome`
   feature），不进产品路径。

### 5.7 字体与资产

- `codicon.ttf` 必须携带 **m 字形补丁**版本（gpui-pre-macos 静默拒载无 m
  字形字体——历史坑，搬运时用 fontTools 验证一次）；
- `sf-mono.ttf`（`.SF NS Mono`）与现有 Lilex/JetBrains Mono NL 并存注册于
  manox bin（注册顺序无冲突，族名不同）；
- 品牌 SVG（claude/codex/githubcopilot 等）与本仓 `agent-ui/assets` 同源，
  合并为单一 `ExtrasAssetSource` 资产面（chrome 只引路径约定，资产实体在装配层）。

### 5.8 i18n 键初始集（chrome.*，双 ftl 各加，parity 单测守护）

```
chrome.sidebar-section-automations / -chats / -customizations-overview /
  -plugins / -mcp-servers / -skills
chrome.row-copy-id / -pin / -unpin / -archive
chrome.picker-search-placeholder / -picker-empty / -picker-default-title
chrome.tab-new-tab / -retry / -browser / -terminal / -editor
chrome.quick-open-browser / -open-terminal / -open-agent（带 {agent} 占位）
chrome.panel-title
chrome.spawn-failed（带 {prog} 占位）
chrome.no-chats
```

（终集以实现为准，本清单是工作量下界：~15-20 键。）

### 5.9 验收

- 离屏视觉 harness（`chrome-ui/tests/visual.rs`）产出基线 PNG（主窗/右栏开/
  面板开三变体），与 agents-window-gpui 现基线比对（允许 chat 槽差异）；
- clippy/test/fmt/UI-MAP 四件套；
- chrome crate 依赖不变量断言（§1 推论）进 CI。

---

## 6. Phase 4 — 装配与退役

### 6.1 manox bin 装配变更

- 窗口几何：`TitlebarOptions { appears_transparent: true,
  traffic_light_position: (12,13) }` + `app_owns_titlebar_drag: true` +
  Opaque 背景；min 980×640；
- `chrome::Shell` 挂 `MainSurface::chat(ChatColumn)`；ToolTab 注册表由
  agent-ui 装配层供给（浏览器/终端族/外部会话/编辑器/子代理面板，复用
  `spawn_terminal`、`BrowserTabView`、`ExternalSession` 现有实现，包成 ToolTab 实例）；
- 底部面板**默认展开**并装终端页签（2026-09-21 裁决；注意复刻版语义是
  挂载即拉起——默认开 = 每次启动拉起一个 $SHELL，属有意选择）；
  全窗终端 = 面板最大化；全窗外部会话 = 右栏 SessionTab 最大化；
- NativeMenuBar / SystemTray / DockBadge 不动（读的是 multiplexer/状态层）；
- `AGENTS.md` 结构图同 PR 增补两个 crate。

### 6.2 删除清单

- `agent-ui/src/workspace/render.rs` 的 WorkspaceShell/card_title_bar/拖拽热区；
- `agent-ui/src/views/sidebar.rs` 渲染层（投影层保留）；
- `agent-ui/src/workspace/right_pane.rs`（RightTab 枚举/快照逻辑并入装配层的
  ToolTab 包装 + threads.db 快照序列化）；
- `ViewMode::Terminal` / `ViewMode::ExternalSession` 全窗路径（按 §2.2 映射）；
- 旧 TitleBar/SidebarDivider/RightPaneToggleBtn 等旧壳组件。

### 6.3 UI-MAP 重写大纲

按新层级重组：`Window(原生交通灯+38px 工具栏) > Shell > [SessionList |
MainArea[ ChatColumn | RightPane ] / Panel]`；ChatColumn 以下沿用现有
MessageColumn 族条目改挂；RightPane 族按 ToolTab 实例页签重写；旧
WorkspaceShell/ViewMode 条目删除。**每个 Phase 的 PR 都同步改 UI-MAP**
（仓库规则：UI 变更同 PR 更新），Phase 4 做最终重组。

---

## 7. 横切关注点

### 7.1 流程门禁（每 PR）

1. `cargo clippy -D warnings --all-targets` + 全量 `cargo test` +
   `cargo fmt --all`；
2. **`cargo test -p agent-ui --features test-support`**，Phase 2 起加
   **`cargo test -p manox-agent-chat-ui --features test-support`**——
   test-support 编译单元两次被默认门禁漏掉（#52、#56 教训），双 crate 显式跑；
3. UI-MAP 同 PR；ftl 双语 parity 键扫描（follow-stop 教训的 key-scan gate）；
4. 提交信息无 Co-Authored-By；合并前 `script/local-manox.sh off` 形态下
   `cargo update -p manox-agent` bump + 全门禁重跑 + Cargo.lock 随合并提交。

### 7.2 风险登记册

| 风险 | 等级 | 缓解 |
| --- | --- | --- |
| Workspace 单体手术 borrow 冲突连环 | 高 | Phase 1 簇级迁移 + 每簇全测；X 类字段最后、端口先立 |
| 侧栏语义静默丢失 | 高 | §5.3 验收清单逐项打勾；以 workspace/tests.rs 的侧栏用例为守护 |
| chat 换壳后视觉回归（宽度/rail 几何/焦点拓扑） | 中 | 离屏 harness 基线比对；AskDrawer/TurnNavigator/composer 的 context 谓词绑定逐一重验 |
| keybinding 拓扑漂移（main.rs 数十个绑定） | 中 | actions 归属表（§4.2）落地时保持 re-export，main.rs 绑定面最后一步才动 |
| ToolTab 会话分组与 threads.db 快照语义错位 | 中 | 挂起/恢复钩子先写设计小节（Phase 3 设计 PR），快照序列化留在装配层 |
| gpui-component 全局主题与 chrome 显式着色打架（PopupMenu/Input 槽位） | 中 | chrome 内 gpui-component 件显式着色或经配置注入；全局主题归 bin 决定 |
| codicon m 字形 / 字体注册顺序 | 低 | fontTools 验证 + 离屏首帧检查 |

### 7.3 估算（供排期参考，非承诺）

| 阶段 | 规模 | PR 数 |
| --- | --- | --- |
| Phase 1 | 动 workspace.rs + tests.rs ~8k 行 | 5 |
| Phase 2 | 搬移 ~12k 行（含 message.rs 5121） | 3-4 |
| Phase 3 | 移植 2.5k + 扩展 ~1.5k | 4-5（含设计 PR） |
| Phase 4 | 装配 + 删除 ~6k | 3 |

---

## 8. 开放问题（2026-09-21 全部裁决）

| # | 问题 | 裁决 |
| --- | --- | --- |
| 1 | 底部面板 | **默认启用**；chrome 侧抽象为通用 dock——页签内容可注入任何东西（与右栏同款契约或其子集），manox-app 集成终端实例。注意挂载即拉起语义：默认开 = 每次启动拉起一个 $SHELL，属有意选择。 |
| 2 | 全窗终端/外部会话 | **dock 最大化**：chrome 一个统一的「最大化当前 dock 面」动作（终端=最大化底部面板；外部会话=最大化右栏 SessionTab），聊天实体保持挂载、收起即回；MainSurface 只有 Chat/Settings，chrome 不引入 ViewMode 枚举。 |
| 3 | ContextRail | **维持 chat 列 overlay**（不转右栏页签）。 |
| 4 | agent-ui 命名/去留 | **保留不拆光**（状态 + 装配层，见 §2.1 残留清单）；重构期间保留现名，Phase 4 后按残留职责复审改名。 |
| 5 | agents-window-gpui 仓 | **不动**（搬运源保持原状）。 |
| 6 | test-support | chat-ui 承接 `test-support` feature 与 `tests/workspace_overlap.rs`（walk 对象=消息渲染重叠，随管线迁移）；门禁双 crate 显式跑（§7.1）。 |
| 7 | client_store_handle | **随迁 chat-ui**；`client_store.rs` 存储本体留 agent-ui；multiplexer 经 agent-ui→chat-ui 依赖创建 handle 实体（§4.1）。 |

---

## 附：事实基线（2026-09-21 核对）

- agents-window-gpui：3 crate / ~4227 行（agents-theme 268、agents-elements
  799、agents-app 3160 含 tests）；gpui-pre `=0.3.4` / gpui-component `=0.6.1`
  （与本仓同轨）；主栏为占位卡，消息 UI 于 freya 版已拆。
- manox-app agent-ui：45 文件 / ~19k 行（workspace.rs 3132 + workspace/* ~10k
  + views/* ~9k）；`Workspace` 字段区间 workspace.rs:622-950。
- 本计划文件：`PLAN-CHROME-CHAT-SPLIT.md`（仓库根，与 AGENTS/UI-MAP 同级）。
