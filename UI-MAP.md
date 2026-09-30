# UI Map

Shared vocabulary for every named UI component in manox. When discussing UI, reference
component names from this file so both parties refer to the same thing.

组件名以 crate 归属区分：壳与壳内组件在 `manox-agent-chrome-ui`（`Chrome` 前缀或
crate 路径），会话列与其状态机在 `manox-agent-chat-ui` / `agent-ui`。

Component names use PascalCase. The hierarchy mirrors the visual containment tree.

---

## 0. 架构（2026-09-28 起：唯一壳）

窗口只有一个壳，无构建开关：

```sh
cargo run                       # 桌面应用（chrome 壳，唯一）
```

| 层 | 组件 |
| --- | --- |
| 根视图 | `manox_agent_chrome_ui::Shell`（壳：38px 工具栏 + 侧栏 + 主区卡 + 右栏 + 底部 dock） |
| 侧栏 | `chrome::SessionList`（props 组件）+ `agent_ui::sidebar_projection`（wire 行 → props 的纯投影） |
| 主区卡内容 | `Workspace`（`agent-ui`）的会话列 —— 挂进 chrome 的 `MainSurface` 槽（日志/契约见 §3） |
| 右栏 | `chrome::RightPane` + `ToolTab` 实例页签；`agent_ui::tool_tabs::registry` 提供 kind（终端 / CLI agents / 编辑器 / 浏览器）+ 装配层宿主页签（子代理面板、宿主打开的浏览器页签）；per-thread 会话由 `RightPaneSession` stash/restore，快照落 threads.db |
| 底部 dock | `chrome::panel` + `PanelSurface` 注入（manox 装终端；随前台线程 cwd） |
| Settings | 主区卡内的 nav｜panel 换位（`Workspace::render_settings_card`） |

共享状态层不变：`agent-ui`（multiplexer / client_store / browser_host / dispatch / 侧栏投影 /
slash_command / Settings 视图）、`manox-agent-chat-ui`（聊天状态机与消息管线）、`terminal-ui`、
`manox-webview`。

壳的契约（均可在不改 agent-ui 的前提下扩展）：`MainSurface`（主栏槽）、`ToolTab` /
`ToolTabFactory`（右栏 kind）、`PanelSurface`（dock 内容）、`HostHooks`（pin/archive/new/select
回调）。装配层另有两条进程级通路：`chrome_assembly::{open_tool_tab, subagent_panel}` ——
宿主（agent 的浏览器打开、会话列/rail 上的子代理点击）经它把页签落到活着的右上栏。
依赖不变量：chat crate 不得依赖 terminal-ui/manox-webview/manox-ext-agents
（`script/check-chat-crate-deps.sh` 门禁）；chrome crate 不依赖 manox-agent。
拆分与旧壳退役的历史见 git log（计划文档已随退役删除）。

---

## Harness 现状（manox-harness）

老 manox harness（`harness-manox` crate 与 agent-ui 的 manox 变体）已完全
删除；`crates/manox-harness/src/core`（TS Pi 内核移植）+ `crates/manox-harness/src/ext` 是唯一 harness
核心，`manox-agent` / `agent-ui` 是宿主接线层。本文件只描述当前 manox-harness 路径的 UI；
引用老 manox 实现请查 git 历史或 `origin/Manox` 备份分支。

| 能力 | 状态 | 说明 |
| --- | --- | --- |
| 会话主路径（流式文本 / thinking / 工具卡片 / cancel / steer） | ✅ | `pi_backend::session` + `adapt`；空闲 `prompt()`，运行中 `steer()` |
| 会话持久化与重启恢复 | ✅ | pi jsonl + `session_meta` sidecar |
| 转录渲染（MessageList / ToolCallCard / RetryBadge / ErrorMessage） | ✅ | 共享渲染管线 |
| 权限门控（PermissionMode / ToolCallAuthorization / AskUserQuestion） | ✅ | 文件效果策略：bash 三模式都运行（seatbelt 按模式渲染 read-only/workspace-write profile），fs 写经 `writable_roots` containment + `[sandbox: …]` marker；`sandbox_permissions`+`justification` 升级往返经 `ToolCallAuthorization`；AccessChip 切 ReadOnly/WorkspaceAccess/FullAccess，切换热生效（下一次工具调用即按新模式裁决），chip 乐观即时刷新 |
| Slash commands | ✅ 部分 | `/compact`、`/exit`(`/quit`)、`/new`(`/clear` `/archive`)、`/plan`、`/goal`、`/mode`；markdown/skill 适配器经共享 registry |
| 模型选择器 | ✅ | pi `ProviderRegistry`，按 provider 显示名分组 |
| 项目（composer chip / 侧栏文件夹 / 绑定新会话） | ✅ | 宿主 workspace registry（`ClientCall::Workspace` + `HostEvent::WorkspaceUpdate` 状态流），客户端不再写 threads.db `projects` 表 |
| Sidebar / 会话列表 / 新建切换归档 / LLM 标题 | ✅ | pi `SessionRepository` + sidecar；标题双模式语义移植自 manox |
| ConversationInfoBubble（composer 圆圈上方：Captain/subagents/branch 对/plan 文件/todos/per-model 用量/sources） | ✅ | 用量与圆圈同口径（最近请求输入侧）；per-model 成本行与 ±change 计数随卡片退役 |
| `/compact` + Recap 卡片 | ✅ | 内核 `HarnessEvent` compaction 事件（manual/threshold/overflow） |
| 后台线程（ctrl-b 置底 / 切换自动 park） | ✅ | `background_threads` + `attach_thread` parking |
| CLI agent 终端（Claude Code / Codex / GitHub Copilot） | ✅ | 右栏 ToolTab：先落 provider→model 级联（共享投影 `cascade_provider_groups`），选中即以该端点 `AgentBuilder` 拉起 CLI（PTY relay → `TerminalView`；cwd = 前台线程项目目录） |
| 集成终端 / VS Code 注入 / ChatGPT.app 注入 | ✅ | 右栏终端页签（$SHELL，独立 PTY）+ 工具菜单的 VS Code / ChatGPT.app 注入启动（后台线程 + 通知，应用独立存活） |
| Browser 标签 / Terminal 标签 | ✅ | webview host notify/inbound 桥；平台 terminal surface |
| TurnNavigator | ✅ | cmd-m 打开；↑/↓ 选条、enter 定位、⌘↵ 回填 composer、⌘C 复制；历史回溯另有 ⌥↑/⌥↓ |
| TurnRail（左缘轮次导航） | ✅ | 常驻左缘刻度 rail（dsh TurnNavigator 移植，≥2 轮且卡宽 ≥640px 显示）；悬停预览、点击定位、active 随滚动追踪 |
| 图片附件 | ✅ | 剪贴板粘贴 / plus 选择 → chip → 气泡渲染 → 内核 `ContentBlock::Image` 投递（TS `prompt(text, {images})` parity，#438）；steer 带图同路 |
| MCP | ✅ | 连接核心共享化 + pi AgentTool 桥（#442）；Settings → MCP servers 面板（列表/连接状态/持久开关） |
| Plus 菜单（文件 / 目标 / 插件） | ✅ 部分 | 文件 → native picker → pending attachments；目标 → seed `/goal`；Plugins 组为静态装饰（待与插件面板 #474 整合） |
| skill / subagent @mentions | ✅ | 共享层 registry（#440）：markdown 斜杆命令 + skill mentions（submit_command/submit_skill）；subagent 定义经 agent_defs 注册（#471） |
| Sub-agent 观察 | ✅ 部分 | rail 观察行（生命周期/活动）+ Agent 工具卡片实时流式子转录（text/thinking delta、工具 ▸/✓/✗ 行）；独立钻取面板随 manox 移除，卡片体即钻取面 |
| Plan 模式 / PlanReview | ✅ 部分 | 重实现（#441，参照 oh-my-pi）：ProposePlan 结构化工具 + plan 落盘 + 调研指令注入 + 写硬门控；B2-PR-5 后 plan-review 经单个 `AskUserQuestion`（`intent.kind="plan-review"` + `detail`=plan 正文 + `approve` 选项）呈现为 [PlanReviewDecisionCard](#planreviewdecisioncard)（底部一键裁决卡），三选项 Approve / Approve & compact / Request changes（无 Fresh，裁决映射归服务端）；rail 的 plan 节（`UpdatePlan`）消费执行进度，快照经 sidecar 持久化、compaction 后可恢复；PlanPreview 独立 tab 按设计不恢复 |
| Goal | ✅ 部分 | facade+GoalBridge 共享快照、GetGoal/CreateGoal/UpdateGoal 工具、`/goal` 命令、composer chip+状态 popover（attach/reclaim 时按 projection 重新武装 elapsed ticker）；per-turn 记账/自动续跑/BudgetLimited 强制为后续项 |
| Team | ✅ 部分 | 成员经 `Steer(spawn="TeamMember")` 创建为真实 thread（sidebar 可见、可恢复）；同伴消息经 Steer Inject 路由。旧 roster 容器 `Entity<Team>`、MemberPanel 空壳、composer team chip、sidebar role badge、`TeamDismiss/TeamStatus` 等 roster 工具与授权冒泡已完全退役删除（见 #625）；member→parent 自主汇报未接线（Abort 仅 cancel 当前轮，dismiss/archive 无替代品） |
分层纪律：crates/manox-harness/src/core 只做 TS Pi 对齐与扩展点；harness 能力扩展一律走
crates/manox-harness/src/ext；宿主（manox-agent / agent-ui）只做装配与 UI。

---

## 索引

### 壳（Shell）

- [ChromeShell](#chromeshell) · [ChromeSessionList](#chromesessionlist) · [ProjectGroupMenu](#projectgroupmenu) · [SidebarProjection](#sidebarprojection) · [ChromeRightPane](#chromerightpane) · [ToolTabRegistry](#tooltabregistry) · [ChromePanel](#chromepanel) · [ConversationColumn](#conversationcolumn) · [SettingsCard](#settingscard)

### 顶层

- [Window](#window) · [NativeMenuBar](#nativemenubar) · [AboutWindow](#aboutwindow) · [Workspace](#workspace) · [ViewMode](#viewmode) · [ViewMode::Workspace](#viewmodeworkspace) · [ViewMode::Settings](#viewmodesettings)

### MainView

- [MainView](#mainview)

### MessageColumn

- [MessageColumn](#messagecolumn) · [Body](#body) · [FollowStoppedNotice](#followstoppednotice) · [FollowStopProjection](#followstapprojection) · [TurnRail](#turnrail)

### ConversationInfoBubble

- [ConversationInfoBubble](#conversationinfobubble)（composer 圆圈上方气泡；状态舱数据仍在 `ContextRail` 实体上）

### Hero / HistoryLoading

- [Hero](#hero) · [HistoryLoading](#historyloading)

### MessageArea

- [MessageArea](#messagearea) · [MessageList](#messagelist) · [MessageItem](#messageitem)

### MessageItem 变体

- [UserMessage](#usermessage) · [AssistantMessage](#assistantmessage) · [AssistantActions](#assistantactions) · [ReasoningBlock](#reasoningblock) · [ActivitySegment](#activitysegment) · [ToolCallCard](#toolcallcard) · [AgentTaskCard](#agenttaskcard) · [BackgroundTaskCard](#backgroundtaskcard) · [ErrorMessage](#errormessage) · [NoticeMessage](#noticemessage) · [RecapCard](#recapcard) · [CacheMissDivider](#cachemissdivider) · [RetryBadge](#retrybadge)

### Footer / Composer

- [Footer](#footer) · [Composer](#composer) · [QueuedFollowUps](#queuedfollowups) · [ComposerDivider](#composerdivider) · [AttachmentChips](#attachmentchips) · [AttachmentChip](#attachmentchip) · [BrowserSuiteChip](#browsersuitechip) · [ComposerInputRow](#composerinputrow) · [InputField](#inputfield) · [SendBtn](#sendbtn) · [ModelChip](#modelchip) · [ContextUsagePill](#contextusagepill) · [AccessChip](#accesschip) · [ProjectChip](#projectchip) · [FollowStopProjection](#followstapprojection)

### AskDrawer

- [AskDrawer](#askdrawer) · [AskDrawerHeader](#askdrawerheader) · [AskDrawerQuestion](#askdrawerquestion) · [AskDrawerOptions](#askdraweroptions) · [AskDrawerCustomInput](#askdrawercustominput) · [AskDrawerFooter](#askdrawerfooter) · [AskDrawerSkipButton](#askdrawerskipbutton) · [AskDrawerNav](#askdrawernav) · [PlanReviewDecisionCard](#planreviewdecisioncard) · [ConfirmationCard](#confirmationcard)

### Popups & Dropdowns

- [CompletionPopover](#completionpopover) · [ModelMenu](#modelmenu) · [AccessMenu](#accessmenu) · [ProjectMenu](#projectmenu)

### Overlays

- [BlankProjectOverlay](#blankprojectoverlay)

### 右栏页签内容

- [SubagentPanel](#subagentpanel) · [BrowserView](#browserview)

### ManagementShell

- [ManagementBackControl](#managementbackcontrol)

### Settings

- [SettingsView](#settingsview) · [SettingsTitleBar](#settingstitlebar) · [SettingsLeftNav](#settingsleftnav) · [SettingsSearchInput](#settingssearchinput) · [SettingsGroupList](#settingsgrouplist) · [SettingsGroup](#settingsgroup) · [SettingsItem](#settingsitem) · [SettingsRightPane](#settingsrightpane) · [SettingsPanel](#settingspanel) · [SettingsModelsPanel](#settingsmodelspanel) · [SettingsChatGptAppPanel](#settingschatgptapppanel) · [SettingsSectionCard](#settingssectioncard) · [SettingsRow](#settingsrow) · [SettingsSectionHeader](#settingssectionheader) · [SettingsHairline](#settingshairline)

### Terminal

- [TerminalView](#terminalview) · [TerminalTabBar](#terminaltabbar) · [TerminalGrid](#terminalgrid)

### Shared Primitives

- [Button](#shared-primitives) · [Input](#shared-primitives) · [PopupMenu](#shared-primitives) · [PopupMenuItem](#shared-primitives) · [TabBar](#shared-primitives) · [Tag](#shared-primitives) · [Markdown](#markdown) · [TerminalPanel](#terminalpanel) · [TurnFrame](#turnframe) · [Icon](#shared-primitives) · [BrailleSpinner](#braillespinner) · [ScrollHandle](#shared-primitives) · [TitleBar](#shared-primitives) · [ContextMenu](#shared-primitives) · [Tooltip](#shared-primitives)

### 状态

- [PermissionMode](#permission-modes) · [ToolCallStatus](#tool-call-statuses)

---

## 1. Window

#### Window

Top-level native window, title "manox", min 900×600.

> Source: `crates/manox/src/main.rs`

#### NativeMenuBar

macOS menu bar built by `build_app_menus()`: `manox` (About/Settings…/Quit), `Terminal` (new/close tab), and `工具` (Tools) with two app cascades. `ChatGPT.app` → provider → model: models mirror the provider registry snapshot filtered by `visible_agents()` containing `ChatGPT.app` (Responses-capable models), grouped by provider; picking a model dispatches `LaunchChatGptApp { provider, model }`, routed through the App-level action handler to `Workspace::launch_chatgpt_app`, which starts ChatGPT.app via cx's injection path on a background thread (selected model = default; the provider's full Responses catalog is injected). `VS Code` → provider → model: models filtered by `visible_agents()` containing `VS Code` (Anthropic-wire models); picking a model dispatches `LaunchVSCode { provider, model }` → `Workspace::launch_vscode_app` → `cx::launch_vscode_app`, which resolves the login-shell env, overlays Claude Code BYOK env (`ANTHROPIC_BASE_URL`/`ANTHROPIC_API_KEY`/`ANTHROPIC_MODEL` + provider/model env) at highest priority, and launches VS Code with `VSCODE_CLI=1` so the extension host, the Claude Code extension's bundled CLI, and integrated terminals inherit the injected env (a running VS Code is restarted after user confirmation; no settings.json writes, API key never persisted). A trailing 「打开」item dispatches `LaunchVSCodePlain` → `cx::launch_vscode_plain` (plain `open -a`). The VS Code submenu is disabled when VS Code is not installed. Text-only — gpui native menu items carry no images. Rebuilt by `i18n::rebuild_menus` on UI-language change, after a provider-registry reload, and once when the initial background provider registration lands.

> Source: `crates/manox/src/main.rs`

#### AboutWindow

Centered, non-resizable floating dialog (440×440, `WINDOW_WIDTH` / `WINDOW_HEIGHT` in `about.rs`) opened by the `OpenAbout` action from the native menu bar / tray path, single-instance (an existing About window is activated instead of a second one). Two direct children of the root (`about-window`, `p_4`, `justify_between`): `details` (`about-details`) — the app icon, the headline `Manox <app version> (<build type>)`, and then one muted-label provenance row per pinned stack, `manox desktop` (this repository's short commit), `manox harness` (the dspo/manox runtime commit), `gpui-component` and `gpui-pre` (the lockfile pins, rendered as their version literal or `owner/repo @ rev` when resolved from git); a row whose value the build could not resolve is absent — and `about-buttons` (`about-buttons`) with the OK / Copy pair (`about-ok` closes, `about-copy` writes the clipboard block and closes). Escape closes the window. Rows come from `provenance_rows()`; the clipboard block is the runtime's `version::structured_about()` followed by one `label: value` line per row.

> Source: `crates/manox/src/about.rs`

#### SystemTray

Process-lifetime system tray installed right after the first main window opens (`tray::install` — ordered after window creation because the status item creates its own `NSStatusBarWindow`, which must not become a startup death mode when window-server resources are exhausted), the lifeline for reaching manox while no window exists. Backends: macOS/Windows use `tray-icon` (native status item + menu; both platforms pump the tray's messages on the gpui main thread), Linux uses `ksni` (StatusNotifierItem over D-Bus on its own thread, no GTK involvement). Menu items: 「打开 Manox」(`menu-open-manox`) and 「退出」(`menu-quit`), labels re-resolved through the `i18n::rebuild_menus` path on UI-language change. Windows additionally opens/focuses the window on left icon click (right click pops the menu); macOS pops the menu on icon click. Event bridge: gpui exposes no cross-thread wake, so a foreground task polls every 100ms and drains the backend's event channels into `TrayCmd::Open` / `TrayCmd::Quit`. With a tray, the app runs under `QuitMode::Explicit`: closing the main window parks the process instead of quitting it — the `Workspace` entity stashed in `agent_ui::dispatch` is process-lifetime, so the foreground thread and any parked background threads keep running through the close; 「打开 Manox」(or the macOS dock icon, via `on_reopen`) re-opens the window over that same workspace, restoring conversation, drafts, and thread list. Tray install failure keeps the platform default (quit-on-last-window-close off macOS) so the app never strands invisibly.

> Source: `crates/manox/src/tray.rs`, `crates/manox/src/main.rs`

#### DockBadge

The count on the macOS Dock icon (`NSDockTile.setBadgeLabel` via `objc2-app-kit`), mirroring the sessions currently owing the user attention: a parked ask / tool-approval card (`pending_auth`), a plan review awaiting a verdict (`pending_plan`), an errored mark, or an unseen settle on a non-focused thread (the client-owned unread mirror). One point per non-archived thread row; the aggregate lives on `Workspace::attention_count` → `SessionMultiplexer::attention_count`. Clear paths mirror the sidebar's own semantics: the interaction flags retire when answered, unread retires on focus, and the leaf's live unread mirror wins over the row flag — so on live rows the badge and the sidebar cannot disagree. Two deliberate divergences: archived rows are dismissed by the badge while the sidebar still paints their attention marks, and `errored` follows the §D.5 mirror rule (only the next list snapshot clears it, like the sidebar's danger triangle — no user gesture retires that point). Not fed from state edges — local unread raises only notify the leaf entity — so a foreground task polls every 500ms (the tray pump's pattern) and only crosses into AppKit when the count changed; counts above 99 render as `99+`. Windows (taskbar overlay icon) and Linux have no backend yet: they keep a clean no-op behind the same API. The pump starts with the tray install block but runs regardless of it.

> Source: `crates/manox/src/badge.rs`, `crates/manox/src/main.rs`, `crates/agent-ui/src/multiplexer.rs`

## 2. Workspace

`agent-ui` 的会话列（`crates/agent-ui/src/workspace.rs`）：状态 + 装配层。持有
multiplexer（单连接 demux）、`ChatColumn`（线程面/会话状态/composer/ask drawer/rail）、
browser host 槽位、侧栏投影泵所需的全部状态；渲染只产出会话列本身（render 入口见
`workspace/render.rs`），由 `chrome_assembly` 挂进壳的主区卡。

#### Workspace

会话列的根容器：hero 空屏或虚拟化消息列表、composer footer（附件 + chips）、
ask / blank-project overlay、TurnNavigator overlay，
外面套一层键盘动作装饰（`apply_chat_actions`：settings / turn navigator / 队列回退 /
completion / 回忆 / archive / 后台化）。每帧在这里维护的状态：blank-project input、
ask 卡与投影的 reconcile、ask 自定义输入框、投影快照、跨端已结 settle 通知。

> Source: `crates/agent-ui/src/workspace.rs`, `crates/agent-ui/src/workspace/render.rs`

### 2.1 ViewMode

`Workspace` 只有两个模式：

#### ViewMode::Workspace
默认 —— 会话列（hero 或消息列表 + composer）。

#### ViewMode::Settings
主区卡内的设置页（nav｜panel 两栏，替换会话列，直到 back 控件退出），带 200ms 退出延迟。

---

## 3. 会话列布局

会话列是壳主区卡的内部内容，四周的 gutter / 侧栏槽 / 卡壳 / 内嵌标题栏全部由壳负责。
会话信息以 [ConversationInfoBubble](#conversationinfobubble) 气泡按需展开（composer 右侧圆圈
上方），不再占用常驻纵向空间。[TurnRail](#turnrail) 可见时正文预留 `GUTTER`（40px）左内边距；
TurnNavigator 浮层锚定卡片内边距盒（左右各 `CARD_BORDER / 2`）。

```
┌ 壳主区卡（圆角 + 边框）──────────────────────┐
│ ╭─────────────────────────────┬──────────╮ │
│ │ 会话列                      │ 右栏     │ │
│ │ 刻度│ 消息列表 / hero       │ ToolTab  │ │
│ │TurnRail（≥2 轮）│（信息以圆圈  │ 页签体   │ │
│ │                   │ 气泡按需展开）│          │ │
│ │  composer footer            │          │ │
│ ╰─────────────────────────────┴──────────╯ │
└────────────────────────────────────────────┘
```

### 3.2 MessageColumn

Central conversation column, flex-1 — the main card's only content (the shell's sidebar and right
pane are siblings of the card, outside this view). Session info lives in the
[ConversationInfoBubble](#conversationinfobubble) above the composer's context ring — the column
carries no reserved overlay inset.

#### MessageColumn

Vertical flex container, fills remaining width.

> Source: `crates/agent-ui/src/workspace/render.rs`

#### Body

Vertical flex below TitleBar, `pt:TITLE_BAR_HEIGHT`, houses the [FollowStoppedNotice](#followstoppednotice) (only while the follow stream has stopped) and then [Hero](#hero) or [HistoryLoading](#historyloading) (while a reopened thread's snapshot is in flight) or [MessageArea](#messagearea) + [Footer](#footer).

> Source: `crates/agent-ui/src/workspace/render.rs`

#### FollowStoppedNotice

Dismissible BROADCAST banner above the message area, shown while the foreground leaf's §二.3 reopen budget is exhausted AND this session's notice is not dismissed (the transcript silently keeps its last window). Row: `IconName::TriangleAlert` + reason copy (a Fluent key chosen by the leaf's typed `FollowStopReason` — today only `follow-stop-stream-failing`, deliberately cause-agnostic because the client cannot observe more; the server-side lease-holder signal, dspo/manox#811, lands as a new variant + key, not a wire-code guess), a ghost **Retry** button (leaf `retry_follow`: re-arms the budget and requests a full re-attach — `OpenSession` ahead of the `StreamOpen`, so the retry also recovers the attach's `OpenSession` having failed once; automatic reopens stay pure `StreamOpen`) and a ghost `×` dismiss (leaf `dismiss_follow_stop`). Dismissing hides ONLY the broadcast — the permanent [FollowStopProjection](#followstapprojection) in the composer's footer chip group keeps the state visible and the retry entry live forever after (one trigger, the write-lease arm, never self-heals; the frozen view may never be signal-less). Dismissal lives on the leaf — per session: no automatic path re-shows the banner (a re-exhaustion keeps the flag), a good snapshot withdraws the whole state (both surfaces retire together), and a thread switch builds a fresh leaf. A manual retry's own terminal exhaustion re-shows the banner undismissed (an explicit user action's outcome must be visible).

> Source: `crates/agent-ui/src/workspace/render.rs` (`render_follow_stop_banner`); state: `crates/manox-agent-chat-ui/src/client_store_handle.rs` (`FollowStop` / `FollowStopReason`)

#### FollowStopProjection

Permanent minimal projection of the stopped-follow state: a compact danger chip in the composer's footer chip group (tail of the left cluster, beside the send control — where the user reaches to resend/retry; a global overlay was rejected: the status belongs beside the recovery action). Shown while the foreground leaf's `follow_stop()` is `Some` — it deliberately does NOT read `dismissed`: the state outlives the broadcast's dismissal, so the entry never disappears on its own (only a good snapshot or a live retry retires it). The chip IS the retry entry: clicking fires the same leaf `retry_follow` the banner's Retry button wires (no-op with no multiplexer — the entry survives its own dead click), and its tooltip previews that consequence. Visible copy is a per-reason Fluent key (`FollowStopReason::indicator_key()` — today `follow-stop-indicator-stream-failing`), so a new reason adds a variant + key, never edited prose. Geometry: `flex_shrink_0` at the group tail; arrival/departure can never squeeze or shift the pinned model/send controls.

> Source: `crates/agent-ui/src/workspace/composer_render.rs` (`render_follow_stop_chip`); state: `crates/manox-agent-chat-ui/src/client_store_handle.rs` (`FollowStop` / `FollowStopReason`)


#### 3.2.1 Hero

Shown when the thread has no substantive messages (and is not loading).

#### Hero

Vertically centered welcome area: logo/heading + inline [Composer](#composer).

> Source: `crates/agent-ui/src/workspace/render.rs`

#### HistoryLoading

Full-column pixel loading page that suppresses the hero, message list, and footer while a reopened thread's chat snapshot is still in flight: `ChatColumn.awaiting_history` is set at reopen attach when the caller declares it expects history (`expect_history`, not the wire-side `reopen` flag — a created session re-opens too) and the fold holds no chat channel. Cleared by the snapshot rebuild (history present), by the store observe (genuinely empty session → hero returns), or by the render-time timeout (`HISTORY_TIMEOUT`, 10s) so a failed reopen cannot pin the page. Render re-checks the fold, so a stale flag cannot pin it either. Layout: a 12×14-cell pixel meerkat sprite played as a 6-slot loop at 3 slots/sec (4 distinct frames: idle, bob, blink, tail flick — each cell a flat solid block, no bevel), centered above the heading (`workspace-history-loading-heading`) and the monospace thread id. No composer while it shows.

> Source: `crates/agent-ui/src/views/history_loading.rs`; gate: `crates/agent-ui/src/workspace/attach.rs` (`attach_thread`), `crates/agent-ui/src/workspace/render.rs` (`render_column`), `crates/manox-agent-chat-ui/src/column.rs` (`awaiting_history`)

#### 3.2.2 MessageArea

Shown when the thread has messages. Replaces [Hero](#hero).


#### MessageArea

Wraps [MessageList](#messagelist).

> Source: `crates/agent-ui/src/workspace/render.rs`

#### MessageList

Virtual list backed by native `gpui::list` (`gpui::list(list_state, render_item)`, `ListState` held directly on `Workspace`). GPUI owns virtualization, scroll, the per-item height cache, and tail-follow; `ListAlignment::Bottom` gives native chat-log semantics — short histories sit at the viewport bottom, long ones scroll — and `FollowMode::Tail` pins to the live end on each layout while following (disengaging on upward scroll, re-arming at the bottom). The row factory captures `Conversation` directly and is strictly read-only during list measurement/prepaint; Workspace-derived ask-card snapshots are synchronized before list construction. `MSG_LIST_OVERDRAW` pre-measures rows below the viewport. Visible rows re-measure every frame, but the pinned official GPUI revision retains off-screen row heights across width changes, so `MessageListWidthInvalidator` observes the final positive list width after layout, invalidates the complete cache with `remeasure_items`, and requests a settling frame while preserving the logical item/offset anchor. Count changes are reconciled via `splice` and in-place mutations via `remeasure_items`, both driven from the `ThreadEvent` handler's `ApplyOutcome`. Only the visible items render. Markdown text rows use Manox's public-API `RichText` leaf rather than GPUI `StyledText`: every width constraint is shaped independently, widths narrower than one em are treated as intrinsic probes, and prepaint reconciles shaping with the final allocated width. This prevents zero-width explosion from entering the list cache and makes painted glyph height match the row allocation without a Zed fork.

> Source: `crates/agent-ui/src/workspace/render.rs` (`ListState` wiring, `MSG_LIST_OVERDRAW`), `crates/manox-components/src/markdown/rich_text.rs` (constraint-safe shaping and paint geometry)

#### TurnRail

Left-edge turn navigation: the dsh TurnNavigator mirrored onto the conversation column's leading edge. An absolute strip (`absolute().top_0().bottom_0().left(RAIL_LEFT_INSET=4).w(RAIL_WIDTH=28)`), vertically centered inside the message band's `h_flex` (mounted as a sibling painted after [MessageList](#messagelist), so it floats over the transcript but never over the composer — the band excludes the footer). One 2px tick per user turn at a fixed 10px pitch; from two turns up, and only when the card interior is at least `MIN_CARD_WIDTH` (640px) — independent of the [ContextRail](#contextrail)'s own gate, either side can float alone. While visible, the `GUTTER` (40px) left padding goes on the **list wrapper inside the band, never on the band itself**: the rail's absolute anchor is the band, so padding the band would drag the rail right along with the text it must clear (the ticks would sit on the transcript instead of hugging the edge — acceptance-round-1 regression).

Marks are re-derived from the conversation every frame — but only past the width gate, which short-circuits before the projection, and `render_turn_rail`'s `Some`/`None` is the single gate for both the rail and the gutter (`collect_rail_turns`: prompt = the user bubble's text collapsed and capped at 50 chars, or the ⌘M navigator's attachment-only / empty-message copy for a textless bubble — the same distinction `TurnEntry::new` draws; response = the turn's last non-empty assistant reply capped at 120 — dsh's `findLast` rule; both caps word-accumulate up to the budget (`split_whitespace` skips whitespace runs of any length, so indented blocks fill it like prose), so a huge turn costs O(limit), not O(全文)). The active mark is `active_rail_turn`: the last turn whose anchor item is at or above the list's `logical_scroll_top` item (the tail-follow floor reports `count`, resolving to the newest mark). Interactions: hover grows the tick 12→18px with a border→muted 140ms tween and the vacated mark sinks back in the same run — a pointer sweep reads as a wave down the ladder (dsh's CSS-transition semantics; each change keys one tween pair under `turn_rail_hover_gen`, with `turn_rail_hover_prev`/`_painted` snapshotting only on change; fast sweeps within the 140ms window truncate the wave's tail by design — the prev slot is single); hover opens a 300px preview card beside the rail (prompt line + response excerpt), fading in over 120ms with a 4px slide and traveling between marks over 140ms `ease_out_quint` (the from-top snapshots only when the hovered mark changes — the tab-indicator `indicator_from` discipline); click jumps through `Workspace::reveal_message` (the ⌘M navigator's own path). The active tick tweens width+color over 140ms on change (previous mark shrinks, new mark grows, keyed per generation; a tick that loses active while hovered hands the animation slot to the hover wave); active-follow scrolls the ladder (`scroll_to_item(Nearest)`) whenever the pointer is outside the strip (`turn_rail_pointer_inside` pauses it so marks never travel under the hand). An over-420px ladder scrolls inside the strip (`uniform_list` + `ListSizingBehavior::Infer` + `max_h`); the preview's geometry consumes the ladder's `ScrollHandle` offset **sign-corrected to positive-down** (gpui's raw offset runs negative scrolling down — the `-offset.y` convention `uniform_list` itself uses; round-1 C1). Thread re-projection (`attach_thread`, diagnostic replace) resets the interaction state via `ChatColumn::reset_turn_rail_interaction` — a stale hover index must not mount a preview on the new conversation. Tick rows follow the gpui hover-crossing rule: a row's leave retracts only its own mark.

> Source: `crates/manox-agent-chat-ui/src/views/turn_rail.rs` (rail + state contract), state fields on `ChatColumn` (`crates/manox-agent-chat-ui/src/column.rs`), mounted in `crates/agent-ui/src/workspace/render.rs` (`render_column`)

#### MessageItem

Single rendered conversation item, centered, full width (no fixed content cap — the transcript adapts to the window width). Each `MessageItem` renders one of the variant cards below based on `ConvItem` kind. Every kind that carries a text body — user (incl. peer deliveries), assistant, error, notice, recap, retry detail, plan review — mounts a persistent `Entity<Markdown>` (`MessageItem::markdown`, created lazily by `ensure_markdown`) instead of rebuilding one per frame: a per-frame `Entity` resets the document's `DocSelection`/`FocusHandle` on every render and breaks drag-select + Cmd/Ctrl+C (the old `markdown_tv` fallback), while a persistent body keeps its selection state alive across frames and leaves inline links clickable.

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`

##### MessageItem variants

#### UserMessage

Full-width user turn block rendered inside [TurnFrame](#turnframe): `{from} > {to}·ModelID·Time` metadata header (`user_turn_header`; empty segments drop, no `>` clause when nothing follows `from`), persistent selectable markdown body, copy btn (hover; flips to a check briefly after copying — [Copy Feedback](#copy-feedback)), and a permission-mode-colored frame captured at send time. `from` is the turn's real author — unattributed human input renders the localized "You", otherwise Captain (lead), Harness (host-injected turns, e.g. the plan-execution seed), or the named agent (team peer delivery, shown with a `theme.primary` peer accent); `to` is the agent whose conversation renders the turn (main thread shows Captain, a member thread its own name, a sub-agent panel the sub-agent type) — a view-side fact stamped by the owning `ConversationState`, never persisted. Peer deliveries share this same renderer live and after reload.

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`

#### AssistantMessage

Full-width block: optional model row + markdown body (plain text while streaming) + a hover-revealed action row beneath the body. A reply that immediately follows an [ActivitySegment](#activitysegment) omits its own model row — the segment's header row carries the model name. The action row (`assistant_action_row`) renders **under** the body, never overlaid on prose, and carries the copy button (flips to a check briefly after copying — [Copy Feedback](#copy-feedback)) followed by the fork button; the whole row fades in on hover of the enclosing group.

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`

#### AssistantActions

Fork affordance for an assistant reply (`ClientCall::ForkSession`, dspo/manox#775). The branch button is **always present** (the row keeps a stable shape) and is *disabled with a tooltip naming the reason* whenever the reply is not a forkable anchor — a control that disappears cannot teach the rule it enforces. Three gates, resolved at build time (`ConvItem::Assistant`'s `fork_unavailable`): the rows must be a faithful replay of the session journal (`NotReplayed` — a sub-agent panel's answer backfill or a compaction retained tail has no id of its own); the reply must close its turn (`MidTurn` — an entry carrying a tool call has its result on a later entry, so a prefix ending there would be repaired with a synthetic "No result provided" and the child would start from false history); and the reply must have landed (`Streaming`). When all three hold, clicking forks the session at that reply — the child is a prefix copy of the source's active chain through that entry, lands as an independent sidebar row, and is opened in place; a failed fork leaves the current view untouched and stays retryable. One fork may be outstanding at a time (`Workspace::fork_in_flight`), so a double click cannot mint two children. The fork inherits the source's model / cwd / approval / effort because every intent field is left unset. Fork rides `Workspace::fork_session_at` → `SessionMultiplexer::fork_session_intent`, whose `{session_id}` receipt reuses the `CreateSession` continuation (`CreateSessionDone`).

Note: the child inherits the source's **title** — a fork copies the prefix including the journal `title` entry the auto-titler writes after the first response — so the sidebar shows two same-named rows. The runtime exposes no rename primitive yet (`thread_store::rename_thread` is still only referenced in a comment), so no `increaseTitle` increment is applied.

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`, `crates/agent-ui/src/workspace/attach.rs`, `crates/agent-ui/src/multiplexer.rs`

#### ReasoningBlock

Collapsible: chevron + "Reasoning" label + muted italic body, rendered as the label and content of a [Chain of Thought](#activitysegment) step — the round's status marker (spinner while streaming, book-open when settled) sits in the step's marker column rather than in this row. The `ai_elements::Reasoning` component itself is not wired here yet; this still uses the entry's own row and body. Each reasoning round (an `ActivityEntry::Reasoning` inside a `Thinking` segment, plus the top-level `ConvItem::Reasoning`) owns a persistent `Entity<Markdown>` (`markdown` field) mounted on first sync — so drag-select + Cmd/Ctrl+C survive across frames (a per-frame `Entity` would reset the `DocSelection`/`FocusHandle` every render and break selection on reasoning text the same way it did on tool output). Italic styling propagates from the row's `Markdown::italic` toggle.

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`

#### ActivitySegment

One contiguous thinking + tool-call segment within a user turn, rendered as a **Chain of Thought** (`ai_elements::ChainOfThought`, `render_thinking`) — the first `ai-elements` component wired into the conversation. Header row: model display name (the row's label) + chevron + live braille spinner + per-kind counts (`Read×7`, `Edit×6`, `思考×8` via `message-reasoning`) + elapsed (`thinking-duration`) + red `activity-failed` / orange `activity-awaiting-approval` badges — counts, spinner and badges ride the header's `meta` slots; clicking the row fires the component's `on_toggle`, which writes the container's `collapsed` and sets `user_toggled` (manual state is sticky — auto-collapse never fights the user). The header row and a step's title row are tab stops: keyboard enter / space answers the same toggle. Collapsed shows the header alone whether live or settled; expanded lists one `ChainOfThoughtStep` per entry (`render_activity_entry`), each drawing the connector rail below its marker — the last step draws none, so the list does not end on a stub. A step's marker column carries the entry's status (braille spinner while a reasoning round streams or a tool runs; book-open / check / cross / minus once settled), its label is the entry's own clickable row (chevron + title), and its content is the entry's body (the reasoning round's persistent `Entity<Markdown>`, or a tool's terminal-styled output panel). **The component holds no policy**: `open` is `layout.expanded` — which already folds in the approval force-open — and the click handler is the host's; `animated(false)` keeps the reveal layout-neutral so the list's cached row heights stay honest. Segments with fewer than two entries render flat under a model-name-only header with no cover to click. An approval-pending entry force-opens the segment so the interactive row is never hidden. The assistant reply that follows a segment renders no model row of its own — the header is the single place the model shows. The elapsed counter ticks every second via a gpui background timer spawned on `TurnStarted` and self-terminating on terminal `Stop`/`Error`; `frozen_secs` pins the final value so later re-renders don't inflate it. Ordinary tool calls fold here instead of producing standalone cards. Attaching a thread replays the leaf window's live-only tail (`workspace/catch_up.rs`), so an in-flight tool's streamed output survives a switch away and back; a settled call's chunks are skipped (its display row already carries the output). Attaching a thread replays the leaf window's live-only tail, so an in-flight tool's streamed output survives a switch away and back; a settled call's chunks are skipped (its display row already carries the output).

> Source: `crates/manox-agent-chat-ui/src/views/message.rs` — `render_thinking`, `render_activity_entry`, `reasoning_step`, `tool_step`, `segment_layout`, `segment_stats`. Component: `crates/ai-elements/src/chain_of_thought.rs`. Container state: `ConversationState` (`ConvItem::Thinking` / `ThinkingContainer`).

#### ToolCallCard

A standalone tool-call card (`render_tool_call`) for the special-case tools that don't fold into an [ActivitySegment](#activitysegment) batch — today `agent` sub-agent calls and `AskUserQuestion`. A model response's other tool calls batch into the `Thinking` container; their output renders via [TerminalPanel](#terminalpanel).

Statuses: `PendingApproval` | `Running` | `Success` | `Error` | `Denied` — see [ToolCallStatus](#tool-call-statuses).

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`

#### AgentTaskCard

Compact, single-line sub-agent row: `[status] type · short title`. Running and pending rows use a braille-dot spinner (`BrailleSpinner`); terminal rows use check, error, or minus icons. The title is always one line with truncation and a full-title tooltip. It deliberately renders no child text, nested messages, copy control, metrics, or expansion affordance; clicking stays a no-op. Live drill-down lives on the Agent tool-call card instead: the child session's streamed text/thinking deltas and tool lifecycle lines (`▸ Tool hint` / `✓ Tool` / `✗ Tool`) append to the card's output in real time (bridged through the Agent tool's progress channel).

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`

#### BackgroundTaskCard

Bordered card showing a background task's kind (Monitor command / Monitor WebSocket / Background Bash / subagent — async `Steer` Dispatch registered as `TaskKind::Subagent`), description, status badge (Running / Stopping / Completed / Failed / Timed out / Stopped / Session ended), event count, and total bytes. The title row keeps only the description's first line (a background bash description is the full command, heredoc body included) with single-line ellipsis; the complete text is shown in a hover tooltip. The detail row (failure summary or latest event) wraps in full — it is the only UI surface for a task's error text. Running tasks show a braille spinner and a Stop button that calls `background_task::stop` (cancels the child token the run task observes). Terminal tasks show a static status icon. Updated in-place by task ID via `ThreadEvent::BackgroundTaskUpdated` — the card is created when the first event snapshot arrives and never duplicated. A subagent's final text is delivered to the Captain via `BackendNotice::SteerDelivered{reason: Complete}` (facade injects a peer message + fires a turn), not via this card's Stop button; an explicit Abort settles silently (`TaskStatus::Stopped`).

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`

#### ErrorMessage

Rounded card, `bg:danger/0.06`, red text, "Error" label + copy btn. Body is a persistent selectable `Entity<Markdown>`. A turn that fails while its thread is parked persists the same card: the parked subscription annotates the error against its own session (`append_ui_note_for`), so the card is present on switch-back and on reload. A turn that fails while its thread is parked persists the same card: the parked subscription annotates the error against its own session (`append_ui_note_for`), so the card is present on switch-back and on reload.

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`

#### NoticeMessage

Rounded card, `bg:secondary/0.15`, muted text, "Notice" label + copy btn. Body is a persistent paginated `TerminalPanel` (`PanelKind::Plain`, no command/cwd) — the same folded surface as tool output: default `PAGE_SIZE` (20) lines with a `+N` load-more row; selection + pagination cursor survive across frames. Mounted by `MessageItem::ensure_notice_panel` (live) and `new_history_item` (reload).

> Source: `crates/manox-agent-chat-ui/src/views/message.rs` · panel: `crates/manox-components/src/markdown/terminal_panel.rs`

#### RecapCard

Collapsible compaction summary card: chevron + book icon + "Context compacted" label + copy btn. Body is the model-generated handoff summary (markdown, not localized), mounted as a persistent selectable `Entity<Markdown>`. Collapsed by default; emitted on `ThreadEvent::Compaction` and rebuilt from `MessageContent::Compaction` on thread reload.

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`

#### CacheMissDivider

Slim left-aligned divider rendered above an assistant turn whose request lost the prompt cache, matching oh-my-pi's `CacheInvalidationMarkerComponent`. Rendered as a 10-character rule + muted label `"cache miss · N tokens"` (tokens formatted by `format_tokens`). Emitted on `ThreadEvent::CacheInvalidation` and inserted as a `ConvItem::CacheMiss` into the conversation list. Live-only (`CacheInvalidation` has no journal row), so it does not survive a reload or a switch away and back. Live-only (`CacheInvalidation` has no journal row), so it does not survive a reload or a switch away and back.

> Source: `crates/manox-agent-chat-ui/src/views/message.rs` — `render_cache_miss`. Event handler: `crates/manox-agent-chat-ui/src/conversation.rs`. Enum: `crates/manox-agent-chat-ui/src/conversation.rs` (`ConvItem::CacheMiss`).

#### RetryBadge

Amber badge, `bg:warning/0.12`, braille spinner + "Retry N/M (in Xs)" text. The retry detail body, when present, is a persistent selectable `Entity<Markdown>`, re-synced when a coalesced retry rewrites the item's detail in place. The trailing retry row is replayed when a thread is attached while its turn is still running (a parked thread's `Retry` never reached the foreground handler), and only while nothing has moved past it: content, a terminal error, or a turn boundary retires the candidate, mirroring the live pop. The trailing retry row is replayed when a thread is attached while its turn is still running (a parked thread's `Retry` never reached the foreground handler), and only while nothing has moved past it: content, a terminal error, or a turn boundary retires the candidate, mirroring the live pop.

> Source: `crates/manox-agent-chat-ui/src/views/message.rs`

#### AttachCatchUp

Attach-time replay of the live-only rows a parked thread dropped (`crates/agent-ui/src/workspace/catch_up.rs`): streamed `ToolOutput` for an unsettled call, sub-agent child/progress rows, and the trailing retry notice — each replayed onto the freshly attached thread from the leaf window, and each bounded on its own terms (the settle row ends the output, the window moving past the retry ends it).

> Source: `crates/agent-ui/src/workspace/catch_up.rs`

#### 3.2.3 Footer

Bottom area of MessageColumn, below [MessageArea](#messagearea) (or below [Hero](#hero) on first screen). It remains mounted while non-empty history is restoring so drafting never waits for backend readiness; send stays disabled until the restore lands.

#### Footer

Vertical flex, `flex_shrink_0`, `py_2`, contains [Composer](#composer). The
ask/auth interaction cards render inline in the transcript
([AskDrawer](#askdrawer)), never by swapping the footer — cancel priority
keeps the composer live underneath them.

> Source: `crates/agent-ui/src/workspace/render.rs`

##### Composer

#### Composer

Centered wrapper around the input row + chips. Its `Input` keeps the bare arrows for the caret: `up` / `down`
always move the cursor inside the multi-line draft (first line's start and last line's end included, never
history). Composer history recall lives on `⌥↑` / `⌥↓` (`ComposerRecallUp` / `ComposerRecallDown`, bound on the
`composer > Input` key context the wrapper carries while the completion popover is closed). The walk is entered
only by those keys — a running walk keeps stepping after an edit, and the text it leaves behind becomes the
walk's working line, which `⌥↓` past the newest turn restores. Submitting or switching threads ends the walk.
A [TurnNavigator](#capability-matrix) `⌘↵` fill is a walk landing too: the walk moves onto the filled turn, so
the draft the fill displaced is the working line `⌥↓` returns — `InputState::set_value` clears the input's undo
history, so nothing else survives that replacement.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

#### QueuedFollowUps

Flat stack of follow-up items parked above the input while a turn is running. Every submitted follow-up starts as **Queued**: a `grip-vertical` drag handle, an optional `image` badge (when the row carries pasted images), a single-line width-ellipsized summary (`queue_row_line`, whitespace collapsed + `text_ellipsis` — no char cap), then a `corner-right-up`-icon **立即 (Steer-now)** button, a `pencil` **Edit** button, and a `trash` **Delete** button. The queue holds a stable two-group order — `[SteerPending|Failed …] ++ [Queued …]` (the pure `steer_group_insert_index` rule) — so promoting a message to a steer moves it to the end of the steer group, ahead of the untouched queue.

Dragging the grip handle reorders the parked **Queued** tail: the move uses the same half-row insertion-edge cue as the sidebar (`queue_drag_boundary` → a 2px `accent` insertion hairline + 0.4 opacity on the source row), and `queue_move_index` restricts commits to land inside the contiguous `Queued` tail — a committed `SteerPending` row is neither draggable nor a crossing destination, so the group invariant survives even a drop onto the status rows. Reordering is session UI state only (the flush order is the queue order); there is no server round-trip. `Edit` (pencil) returns a `Queued`/`Failed` row's text into the composer (merged below anything already typed) and re-attaches its images as `pending_attachments` — the row disappears from the queue for another edit; it is offered on non-committed rows only.

Clicking Steer does **not** touch the message list: it sends the online `ClientCall::Steer` to the server (via `Workspace::send_steer_v2` — the desktop `thread` is an engine-less render mirror, so the old local `enqueue_steer` was a dead end) and turns the row into a **SteerPending** status line — an invisible grip-width spacer (to stay column-aligned), optional `image` badge, one-line summary, and a 「待引导」 badge on the right, no buttons (a live steer is already committed to the server and the protocol has no steer-withdrawal channel, so it is not removable, editable, or draggable). The message enters the conversation only at the **turn settle** boundary: a normal `TurnFinished{cancelled:false, failed:false}` moves every `SteerPending` card out of the queue and appends it to the message list as a persistent **steered** user bubble (「已引导」 badge, `meta.steered`), and the still-parked `Queued` cards then flush as the next turn — so the list order matches the real delivery order (injected steers first, then the batched queue). A cancelled/failed `TurnFinished` settles the group by the server's per-id verdict (`stranded_steer_ids`, FIFO): only the not-yet-injected tail turns into a red **Failed** row (立即-retry / Edit / Remove) — the injected head promotes with its `steered` bubble, so a retry can never double-deliver. A normal settle carries zero stranded and promotes the whole group. The client never observes the mid-turn injection instant; the settle is its earliest verifiable equivalent signal (dspo/manox's steer-continuation guarantee makes promote-at-settle honest: every accepted steer is injected this run or an auto-chained continuation, and an aborted run withdraws its stranded steers so a retry can't double-deliver). `⌘ + ⌥ + /` (`UndoLastQueued`) pops the last removable `Queued` card and skips any `SteerPending` at the tail (not undoable); `Failed` cards stay for the explicit retry/remove path. The 「待引导」 badge on the queue row is live-only (the steer hasn't reached the transcript yet), whereas 「已引导」 still appears only in the live list and drops on reload (the server builds the persisted steer row's `ui` without `steered` — a parity gap needing an upstream fix, not a regression here). Queues are retained in memory per task across task switches, but are not persisted across app restarts; the queue's per-view drag marker is dropped on thread switch (its indices are view-local).

> Source: `crates/agent-ui/src/workspace/composer_render.rs` (`render_queued_follow_ups`, the SteerPending status row, `DraggedQueueRow`/`QueueRowDrag` drag types); `crates/agent-ui/src/workspace/composer.rs` (`steer_follow_up`, `enqueue_steer_pending`, `steer_group_insert_index`, `queue_move_index`, `commit_queue_drag`, `edit_follow_up`, `retire_injected_steer`, `promote_settled_steers`, `settle_steer_group`, `settle_parked_steer_group`); `crates/agent-ui/src/workspace.rs` (`send_steer_v2` + the `TurnFinished` settle routing, the `queue_drag` field); steered badge render in `crates/manox-agent-chat-ui/src/views/message.rs` (`render_user`). The facade's `BackendNotice::Settled` emits `SteerInjected` per steered id, but `manox-session-core/src/translate.rs` drops it on the v2 wire, so the client never receives it and the settle alone drives the outcome.

#### ComposerDivider

1px horizontal border above the composer.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

#### AttachmentChips

Vertical stack of up to two chip rows above the composer (conditional,
rendered by `Workspace::render_attachments`): a file/image attachment row
(`render_attachment_chips`, cleared on submit) and an opt-in browser-suite row
(`render_browser_chips`, persists across submits; removing a chip deactivates
the ChromeUse / WebExplore tool suite).

> Source: `crates/agent-ui/src/workspace/composer_render.rs` + `crates/agent-ui/src/views/composer_menu.rs`

#### AttachmentChip

Single attachment chip: icon + filename + remove btn.

> Source: `crates/agent-ui/src/views/composer_menu.rs`

#### BrowserSuiteChip

Single browser-tool-suite chip: globe/frame icon + localized suite name +
remove btn. Removing it calls `deactivate_browser_tool_suite`.

> Source: `crates/agent-ui/src/views/composer_menu.rs`

#### ComposerInputRow

Horizontal flex: [InputField](#inputfield) + [SendBtn](#sendbtn) + chips.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

#### InputField

Multi-line auto-grow text input, placeholder text.

> Source: `crates/agent-ui/src/workspace/composer_render.rs` (via `gpui_component::Input`)

#### SendBtn

Circular button driven by the raw running edge alone: `danger` Pause glyph
(acts as stop) while a turn runs — under any pending ask/approve/plan card
(absolute cancel priority: interrupting is never blocked by an unanswered
interaction) — and `accent` ArrowUp (send) when idle, inert on empty input.
While an ask card is up, Enter/send submits the card (per-question selections +
custom inputs), not a composer-fed free-text answer — the card-level supplement
was retired by B2-PR-1 in favour of the per-question custom input.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

#### ModelChip

Dropdown chip showing `provider · model · effort` (the reasoning-effort wire value, `high`/`max`) → [ModelMenu](#modelmenu) popup.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

#### ContextUsagePill

Compact context-occupancy pill in the footer's right control cluster, between [ModelChip](#modelchip) and [SendBtn](#sendbtn) (the dsh ContextMeter port): a 14px stroke ring + the integer percent, and the [ConversationInfoBubble](#conversationinfobubble)'s toggle — click opens/closes the bubble above it, hover only tints (never opens). The tooltip carries the absolute figures, `composer-context-usage-tooltip`. Data: the foreground leaf's `last_token_usage` — the latest request's input-side tokens (`input + cache_creation + cache_read`, the same active-token formula as the rail's budget row) — against the foreground model's `context_window` resolved through the shared provider registry. Renders nothing until a usage row has landed AND the window resolves (no capacity → no meter); fill + percent flip to `warning` at ≥90% (the rail's near-full threshold). The ring is a GPUI `canvas`: a `border`-tone full-circle stroke plus a `muted_foreground` arc sweeping clockwise from 12 o'clock, replicating the reference SVG verbatim (r 5.5, 2px round-capped stroke, `r + stroke/2 = 6.5` filling the 7px half-box); a full turn redraws the closed circle, 0% strokes nothing. Live updates ride the leaf's `TokenUsageUpdated` notify.

> Source: `crates/agent-ui/src/workspace/composer_render.rs` (`render_context_usage_ring`, `context_usage_ring`, `occupancy_arc_end`, the `CONTEXT_RING_*` constants)

#### AccessChip

Dropdown chip showing [PermissionMode](#permission-modes) → [AccessMenu](#accessmenu) popup.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

#### ProjectChip

Dropdown chip showing current project → [ProjectMenu](#projectmenu) popup. The
label is the workspace row accounting the foreground session (host registry
order), falling back to the session's project mirror for sessions no row
accounts; the menu's recency list is the same registry.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

##### AskDrawer

Inline transcript card, not a footer swap. The live edge (`sync_live_ask`,
`crates/agent-ui/src/workspace/chips.rs`) arms the card from the fold: an open
elicitation (`chat/inputRequested`) seeds the pending ask AND synthesizes the
matching `ToolCall` row (same-frame guarantee). A parked thread's subscription
drops that event by design, so the card re-surfaces on switch-back by an
explicit re-own: the reclaim sends `OpenSession` (same leaf and follow stream
— the parked session never detached) and the gateway replays the unsettled
adjudications to the joining owner (manox §D.6). A card whose id leaves the
fold's input-needed list after having been confirmed in it settled remotely
and is reconciled away (`reconcile_pending_with_projections`).

##### ConfirmationCard

The tool confirmation's unified surface: the parked call's OWN conversation
row (`ToolCallStatus::PendingApproval`) carrying a decision footer — the
fold's `ConfirmationOption`s rendered verbatim (approve kind primary, the
rest outline; runtime labels are never re-localized), each click settling the
verdict through `ChatHost::resolve_tool_confirmation` →
`Workspace::resolve_auth` (the auth id rides the verdict's `_meta` stamp).
The workspace budgets the `ConfirmationSnapshot` onto the row per frame
(`sync_confirmation_snapshot`, `crates/agent-ui/src/workspace/chips.rs`):
present only while the fold still carries the park (`session.inputNeeded`),
so a settle anywhere — here, another client, a cancel — retires the buttons
on the next sync. The park state (`Workspace::pending_confirmation`) arms
from `sync_live_ask`'s confirmation edge; the composer stays live while a
card is up (steering is a parallel input, not a competing decision). The
retired full-column `render_pending_auth_overlay` modal is gone.

#### AskDrawer

Multi-step question navigator rendered inside the conversation. The card
carries two presentations routed at the one render entry
(`render_ask_user_card`, `crates/manox-agent-chat-ui/src/views/message.rs`): a
single-question ask with a `plan-review` intent whose `approve` label matches
one of its own options renders the [PlanReviewDecisionCard](#planreviewdecisioncard);
every other ask renders the generic stepper flow (`render_question_card`).
Decision actions live in a fixed footer BELOW the content (pager + skip +
next/submit), never above it; the body caps at 520px and scrolls internally so
the footer stays reachable on a long plan. The ask state
(`Workspace::pending_ask` + snapshot sync) lives in `chips.rs`.

> Source: `crates/agent-ui/src/workspace/chips.rs`

#### AskDrawerHeader

Title + close (X, the dismissal leg). The stepper moved to
[AskDrawerFooter](#askdrawerfooter).

> Source: `crates/manox-agent-chat-ui/src/views/message.rs` (`render_question_card`)

#### AskDrawerQuestion

Header tag + question text, then an optional `detail` block — markdown support
text rendered with the repo `Markdown` component (`markdown_tv`) beneath the
question. On the generic card the plan-review body rides here when the intent
fallback fires (see [PlanReviewDecisionCard](#planreviewdecisioncard)).

> Source: `crates/manox-agent-chat-ui/src/views/message.rs` (`render_question_card`)

#### AskDrawerOptions

Checkbox/radio list with labels + descriptions. Single-select resets siblings on
toggle; multi-select toggles in place. Options are optional and unbounded — a
detail/intent-only question is legal (B2-PR-1 lifted the 1..=3 / 2..=3 caps).

> Source: `crates/agent-ui/src/workspace/chips.rs` (`toggle_ask_option`)

#### AskDrawerCustomInput

Per-question free-text `custom` input, rendered under the options with an
`Input` bound to a lazily-allocated `Entity<InputState>`
(`Workspace::ensure_ask_custom_inputs`, run on the render path because an
`InputState` needs a `Window`). Its live text mirrors into
`Workspace::ask_custom_text[qi]`. At the settle fold a `custom` REPLACES a
single-select's selection and SUPPLEMENTS a multi-select's — free text can only
ever attach to its own question (the removed card-level "response" override is
gone). The skip affordance that used to sit beside it moved to
[AskDrawerFooter](#askdrawerfooter).

> Source: `crates/manox-agent-chat-ui/src/views/message.rs` (`render_question_card`) + `crates/agent-ui/src/workspace/chips.rs`

#### AskDrawerFooter

Fixed decision row BELOW the scrollable body (which caps at 520px): left the
pager — Prev / "N of M" / Next ghost steppers; right the Skip outline button
and the primary action — 「下一步」 until the last question, then 「提交」
(last step submits via the composer's `submit_input` gate) — DISABLED until
the CURRENT question is answered (a pick or typed custom; dsh
`disabled={!answered}` parity), so the composer's silent submit gate can never
be reached as a no-op that reads as a broken button. Decision actions sit
where the reading finishes, never pinned above the content they settle.

> Source: `crates/manox-agent-chat-ui/src/views/message.rs` (`render_question_card`)

#### AskDrawerSkipButton

Explicit per-question skip (footer outline button): clears that question's
selection and custom, MARKS it explicitly skipped (`Workspace.ask_skipped`,
dsh `QuestionDraftAnswer.skipped` parity), and moves the walk — a mid-card
skip advances to the next question; the last question's skip settles the card
through the same completeness gate as the submit. The settled canonical row is
`{id, selected: []}` with no `custom` (the server's `AskAnswer::is_skip`). A
skip is distinct from closing the whole card (that is `dismiss_ask`, the
header X; the AskDrawer Esc binding only lands while focus sits INSIDE the
card — reachable on the generic card after clicking the custom input, never
on the decision card, which takes no focus).

> Source: `crates/manox-agent-chat-ui/src/views/message.rs` (`render_question_card`) + `crates/agent-ui/src/workspace/chips.rs` (`skip_ask_question`)

#### AskDrawerNav

Footer Prev / Next steppers plus the primary 下一步/提交 action (the last step
submits); the header keeps only the Cancel leg (close ⇒ `{"dismissed": true}`).
Submitting is completeness-gated (dsh `submitDrafts`'s `findIndex(!completed)`
parity): a question that was never touched blocks the settle and the walk
jumps back to the first incomplete question — the blank card IS the feedback;
only answered or EXPLICITLY skipped questions settle. The reply frame gathers
every question's tri-state (`selected` + `custom`, skipped ones carry
`selected: []`) into the canonical `{"answers":[{"id","selected","custom"?}]}`
— the legacy positional `[[question, answer]]` pair and the card-level
`response` are removed. Dismiss keeps `{"dismissed": true}`; the generic
approval card's allow/deny leg (`resolve_auth`, `{"allow": …}`) is unchanged.

> Source: `crates/agent-ui/src/workspace/chips.rs` (`resolve_ask`, `dismiss_ask`, `first_incomplete_ask_question`) + `crates/agent-ui/src/workspace/composer.rs` (`submit_input` gate)

#### PlanReviewDecisionCard

B2-PR-5: a proposed plan is no longer a dedicated `PlanVerdict` drawer with a
four-choice verdict (that surface, its `PendingPlanReview` state / stash /
overlay, and the `ExecuteFresh` create-and-reseed path are deleted). It reaches
the client as a single-question [AskDrawer](#askdrawer) whose `input` carries
`detail` = the plan body (markdown) and `intent = {kind: "plan-review",
approve: "Approve"}` over three options — **Approve**, **Approve & compact**,
**Request changes**. When the routing (`plan_review_approve_index`: single
question, single-select, `intent.kind == "plan-review"`, `approve` matching
one of the question's own labels) fires, the ask renders as a decision card in
the deepseek-harness `PlanReviewPanel` shape: a warning-tinted strip (dot +
「Plan 评审」), the plan as the body that owns the internal scroll, and a fixed
BOTTOM decision row — the approve option as the primary button, the remaining
options as outline buttons (description in the tooltip), and a quiet
「继续讨论」 ghost action carrying the dismissal leg (`dismiss_ask`). A click
IS the verdict (`decide_ask_option` folds the clicked option in and settles in
the same activation — no selection-then-confirm two-step). 「继续讨论」 is the
decision card's only keyboard-free exit: the AskDrawer Esc binding is
context-scoped and lands only while focus sits INSIDE the card, and this card
takes no focus (buttons avoid focus on mouse-down) — a deliberate button-only
surface, same as dsh's `PlanReviewPanel`.

The user's answer rides the normal canonical reply; the SERVER owns the verdict
mapping (Approve → keep, Approve & compact → compact, anything else — Request
changes / custom / skip → refine, and a card close → stop and stay in plan mode
awaiting a message). The client sends no verdict enum. A free-form message while
the card is up submits the card (per [AskDrawerNav](#askdrawernav)), not a
separate dismissal. A plan-review ask that fails the routing (extra questions,
a multi-select question, unmatched `approve`) renders the generic stepper flow
instead.

> Source: `crates/agent-ui/src/workspace.rs` (`parse_pending_ask` intent) + `crates/manox-agent-chat-ui/src/views/message.rs` (`plan_review_approve_index`, `render_plan_review_card`) + `crates/agent-ui/src/workspace/chips.rs` (`decide_ask_option`)

#### 3.2.4 Popups & Dropdowns

`PopupMenu` entries are `PopupMenu` entities created on open and destroyed on close. [CompletionPopover](#completionpopover) is not a `PopupMenu` — it is a pure render overlay that never takes focus.

#### CompletionPopover

Trigger: typing `/` (slash commands) or `@` (skills + subagents) at the caret in [InputField](#inputfield). A typeahead list anchored above the composer: filters live on every keystroke, navigated with up/down, confirmed with Tab or Enter, dismissed with Escape. While open the composer wrapper sets a `completion = open` key context so the `completion == open > Input` keybindings shadow the Input's own navigation bindings. A pure render overlay — [InputField](#inputfield) keeps focus throughout, so the query keeps filtering as the user types.

> Source: `crates/manox-agent-chat-ui/src/views/completion.rs` (state + detection + rendering), wired in `crates/agent-ui/src/workspace/composer_render.rs`

#### ModelMenu

Trigger: [ModelChip](#modelchip). Model selector dropdown: provider submenus for the model list, then a Reasoning effort block (High / Max, current effort checked) under a separator.

> Source: `crates/agent-ui/src/workspace/chips.rs`

#### AccessMenu

Trigger: [AccessChip](#accesschip). [PermissionMode](#permission-modes) selector: three title-only rows — Read Only / Workspace Access / Full Access, check on the current one. No header row, no "Learn more", no per-mode descriptions.

> Source: `crates/agent-ui/src/workspace.rs` (`build_permission_content`)

#### ProjectMenu

Trigger: [ProjectChip](#projectchip). Recent projects + create blank / select folder.

> Source: `crates/agent-ui/src/workspace/chips.rs`

> Source: `crates/agent-ui/src/views/title_menu.rs`

#### 3.2.5 Overlays

Absolute-positioned over [Body](#body), with scrim.


#### BlankProjectOverlay

Trigger: "Create blank project" from [ProjectMenu](#projectmenu). Centered modal: project name input + confirm.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

### 3.3 ConversationInfoBubble

Composer 圆圈上方的会话信息气泡（dsh ContextMeter 弹层的对位实现）：点击
[ContextUsagePill](#contextusagepill) 开/关，外点 / `Escape` / 会话切换关闭。无标题、无段标题，
段与段之间只用 1px 细线分隔；宽度 `clamp(内容自然宽, 260, 360)`（260 = 原卡片
`ENV_CARD_WIDTH`，信息密度不退化；360 防超长 branch 名/todo 横撑），高度 = 内容高、封顶于
圆圈上方可用空间，超出内部滚动。气泡是只读信息面（无交互控件）；展开/折叠是气泡本地 UI 状态，
随关闭重置，不持久化。

段序（自上而下，空段连同分割线一起不画，相邻段不产生双线）：

1. **Captain + subagents**：主 Agent 行字面量 `Captain`（不走 i18n；`views/message.rs` 的署名仍走
   `context-agents-captain`「船长」，互不相干）。subagent 行 `{type} · {topic}`
   （`subagents.rs::task_display_title`）与 Captain 同一条左基线（无缩进、无树符——从属关系唯一，
   不需要视觉编码）。**默认只显示未结束的行**（`ToolCallStatus::Running | PendingApproval` 判定，
   不用 watchdog 的 `health`），已结束的折叠进 `+N`；活行 0 也保留 Captain 行 + `+N`。上限 5。
2. **branch / worktree 对**：`┌ {branch}` / `└ {worktree 目录名}`（mono 字体，长名 truncate），
   数据 = `git_branch_display` + store `cwd`（原 `render_branch_block` 的取数方式；±change 计数与
   点击复制随卡片退役，`git_status` 收敛为 branch-only）。
3. **plan 文件**：本对话写出的 plan 文件行（文档图标 + 标题），来源 = 会话内 `ProposePlan` 工具行
   （模型唯一的 plan 审批通道，参数带 `slug`/`title`；`Workspace::collect_plan_files` 零拷贝扫描
   `kind()` 引用，按 slug 去重、末次写入倒序）。标题 =  supplied title，缺省 slug
   （即 `<slug>-plan.md` 的文件干）。点击开右栏 markdown 预览本期不实现。上限 5。
4. **todos**：`UpdatePlan` 的 `PlanSnapshot.steps`，稳定排序 InProgress → Pending → Completed
   （组内保持原时序，`sort_by_key`）。状态符号 = 统一外径圆环（Pending 空心 muted /
   InProgress accent 环 + accent 内圆、文字 semibold / Completed muted 环 + 勾、文字 muted +
   删除线）——替换旧 `◻ / ▶ / ✔` 三源字形。全部状态可见（Completed 是进度感的一部分）。上限 8。
5. **per-model 用量**：模型行无树符，`{provider}/{model}` 三段分别渲染（provider 恒 muted、
   `/` muted、model 段按 wire api `pi_wire_text_color` 着色——h_flex 分段排列，不做宽度算术）。
   `├ Context {pct}% {used} / {cap}` = **该模型最后一次请求**的占用 / 窗口上限（与圆圈同口径：
   `last_token_usage` + `model_window_tokens`；只有前台模型可解析，其余模型只画 `└` 行），
   ≥90% 转 warning。`└ ↑in ↓out R{cache_read} CH{hit%}`（`format_cache_hit`，无输入 `--`）。
   按总 token 降序（稳定），上限 5。无段标题、无 per-model 成本行。
6. **sources**：沿用占位行（`workspace-env-no-sources`），去掉「来源」标题。

`+N` 折叠行：chevron + 计数，与段内行共用左基线；只在隐藏数 > 0 时出现；点击就地展开
（气泡随内容长高），再点收起。排序必须稳定（store notify 频繁，不稳定排序会跳行闪动）。

> Source: `crates/manox-agent-chat-ui/src/views/context_rail.rs`（`render_bubble` 及各段 builder、
> `fold_window` / `subagent_display_order` / `sort_todo_steps` / `todo_visual`）；挂载与开关：
> `crates/agent-ui/src/workspace/composer_render.rs`（`render_context_usage_ring`，
> 相对/绝对普通挂载 + 尾巴 `icons/context-bubble-tail.svg`）；plan 投影：
> `crates/agent-ui/src/workspace.rs`（`collect_plan_files`）；验收图：
> `design/conversation-info-bubble/acceptance/`（`BUBBLE_SHOT=… cargo test -p agent-ui --test
> visual_bubble`，`BUBBLE_STATE=open|closed`）。

#### 状态舱（ContextRail 实体）

气泡的数据舱仍是 `ContextRail` 实体（`Entity` 由 chat state 持有，`Workspace` 经
`chat_rail()` 取用）：cockpit 相位（`update_cockpit_phase`）、subagent 行
（`apply_subagent_progress`，首现序即展示序）、`PlanSnapshot`（`set_plan`）、branch 显示
（`set_git_branch`，`git_status::gather_branch` 后台 400ms 去抖刷新）。浮层卡片的渲染面
（`render_panel` 及分段渲染）、宽度门（`rail_width_for`/`RAIL_NARROW_BREAK`）、
`ENV_CONTENT_INSET` 列内缩、`set_host` 观察面板入口已随卡片删除；会话切换
（`reset_for_thread_switch`）连带关气泡并重置折叠态。

> Source: `crates/manox-agent-chat-ui/src/views/context_rail.rs`；`git_status` 分支解析：
> `crates/manox-agent-chat-ui/src/git_status.rs`

### 3.4 右栏页签内容

右栏本体（页签条 / 新标签页空态 / per-thread stash / threads.db 快照）归壳，见
[ChromeRightPane](#chromerightpane)；kind 注册表见 [ToolTabRegistry](#tooltabregistry)。
内容实体由 `ToolTab` 实现提供：

#### SubagentPanel

单个子代理运行的只读观察页签（装配层宿主页签，`ToolTab` kind `subagent`）。页签标签是子代理
**地址**（`SubagentProgress.id`，如 `Sailor_0`）；面板二级头显示 **topic** —— 状态指示 + mono
topic 文本（共享 `subagent_topic` / 派发提示首行推导，空则回退地址）。正文是一段微型会话，走与
主会话**同一套 `ConversationState` + 消息管线**：以 Captain 的派发提示作为首条用户气泡
（来自 Steer 工具调用，存入 `Workspace::subagent_prompts`，带发送时间），头部读作
`Captain > {recipient}·{model}·{time}`（`recipient` 为子代理类型，`model` 为子会话派发时上报的
模型），随后是 bridged 子事件翻译成共享 `ThreadEvent` 契约后的助手气泡、推理折叠、工具卡片。
实时累积在 `Workspace::subagent_transcripts`，整个会话期保留（终态也不再裁剪），所以事后打开仍
重放全过程；重载后打开则回退到 `subagent_final_text` + `subagent-panel-final-note` 提示。
由会话里的子代理卡片点击打开（原 rail agents 段的点击入口随只读气泡退役）
（`ChatHost::open_subagent_tab` → 装配层 `chrome_assembly::open_tool_tab`）；页签随线程走壳的
per-thread stash，切线程时只清该线程的转写数据（`clear_subagent_observation`）。

> Source: `crates/agent-ui/src/views/subagent_panel.rs`, `crates/agent-ui/src/workspace/subagent.rs`

#### BrowserView

一个不可信内嵌原生 webview 的页签体（`ToolTab` kind `browser`）。chrome 行是纯 GPUI：
后退/前进 + 单行地址栏（`Enter` 导航）。内容区是 `manox-webview` 的 `WebViewElement`，
按 gpui 布局走 `set_bounds`。以 `TrustMode::Untrusted` 构建：只注入封闭枚举的 notify 桥与
inbound 写入请求桥 —— 页面没有 Tauri 命令面。进程级桥在构建时经
`WorkspaceBrowserHost::attach_to_builder` 挂上；宿主在启动时装一次
（`WorkspaceBrowserHost::install`），按 tab 路由通知。打开入口：右栏 "+" 的快捷操作、或 agent
自己的 web 工具（宿主 `open_tab` → `Workspace::open_browser_tab` → 装配层开页签）；关闭走页签
× 或 `ToolTab::close`。页签标签镜像页面 `<title>`（页签自己的 2s ticker 轮询
`document.title`，URL 兜底），`on_active` 负责显示/隐藏 OS 子视图（否则会浮在所有页签之上）。
`tab_id` 进程内唯一并织入 webview label，宿主据此把 inbound 通知路由回页签。

两个瞬时横幅渲染在 chrome 行与内容区之间，均由 host 设在视图上的标志驱动（导航/解决时清除）：

- **Yield banner** —— `web_explore_yield` 挂起期间显示；"Done" 经
  `WorkspaceBrowserHost::resolve_handback` 解决挂起的 Task（页面侧 `user_handback` 通知按设计忽略）。
- **Read hint** —— `read_text` / `read_dom` / `screenshot` / `eval_script` 从 `https://` 源取到内容后
  显示的一行弱化提示，表明登录态页面内容已暴露给 agent。

> Source: `crates/agent-ui/src/views/browser_view.rs`


## 4. ViewMode::Settings

Settings page rendered inside the shell's main card (`SettingsLeftNav | divider | main`, the nav
column fixed at 240px), swapping the conversation column until the back control exits. The exit
waits 200ms before flipping the mode back.

#### ManagementBackControl

Unified "back to app" control — `ArrowLeft` + label row (px_2/py_1p5/gap_2, accent hover wash, `theme.radius`). Mounted as the first row of the [SettingsLeftNav](#settingsleftnav) pinned top slot (above the search input and group list) so the back affordance reads as a peer of the sidebar menu items, not an isolated button. The settings page no longer ships a shared management TitleBar — each management surface reuses the app-page scaffold (sidebar + overlay TitleBar in the main column), and the back control lives in the sidebar.

> Source: `crates/agent-ui/src/views/management_shell.rs`

#### SettingsView

Settings page state + renderers. `render_nav` produces the sidebar slot element and `render_main` the main-column element; the Workspace mounts both into the shared shell. Holds the sidebar `width` (synced from `Workspace::sidebar_width` by the divider drag, and seeded on entry) so the settings sidebar resizes exactly like the app sidebar. The main column is a relative `v_flex` with an absolute [SettingsTitleBar](#settingstitlebar) overlay on top and [SettingsRightPane](#settingsrightpane) content below `pt(TITLE_BAR_HEIGHT)`.

> Source: `crates/agent-ui/src/views/settings/mod.rs`

#### SettingsTitleBar

Absolute-positioned `TitleBar` overlay (`h(TITLE_BAR_HEIGHT)`, `top_0/left_0/right_0`) in the settings main column — same chrome as the conversation column's TitleBar. Pure window-drag region: it renders no text (the selected item's identity is carried by each panel's own big page heading, mirroring ChatGPT.app Settings). Carries macOS traffic-light avoidance. No back button — back lives in [SettingsLeftNav](#settingsleftnav).

> Source: `crates/agent-ui/src/views/settings/mod.rs`

#### SettingsLeftNav

Settings sidebar (`bg:background`, right border) rendered at the shared sidebar width; no standalone TitleBar, the macOS traffic-light buttons float over its transparent top (`pt(top_inset)`, 28px on macOS / 8px elsewhere). The back control + search input live in a pinned top slot that never scrolls; only the [SettingsGroupList](#settingsgrouplist) scrolls (`overflow_y_scroll`) beneath them.

> Source: `crates/agent-ui/src/views/settings/mod.rs`

#### SettingsSearchInput

Search/filter input in left nav.

> Source: `crates/agent-ui/src/views/settings/mod.rs`

#### SettingsGroupList

Scrollable list of settings groups with section headers.

> Source: `crates/agent-ui/src/views/settings/mod.rs`

#### SettingsGroup

A labeled group of settings items. Groups: General, Integrations, Coding, External Tools, Archived.

> Source: `crates/agent-ui/src/views/settings/mod.rs`

#### SettingsItem

Single settings row: icon + label, clickable, highlights when selected. An item may carry an optional `custom_icon` (an embedded SVG asset path, e.g. `icons/blocks.svg`) rendered via `Icon::default().path(...)` in preference to the `IconName` (same mechanism as the sidebar's external-session rows); the General → Models item (`blocks`) and the External Tools → ChatGPT.app / VS Code items use it.

> Source: `crates/agent-ui/src/views/settings/mod.rs`

#### SettingsRightPane

Right content area, dispatches to panel renderers. Each panel/content view owns its own scroll and padding.

> Source: `crates/agent-ui/src/views/settings/mod.rs`

#### SettingsPanel

A specific settings panel rendered in the right pane. Implemented panels: General, Config, Models (the cx provider config editor, see [SettingsModelsPanel](#settingsmodelspanel)), Personalization, Environment, ChatGPT.app (External Tools, see [SettingsChatGptAppPanel](#settingschatgptapppanel)). Every other left-nav item (Appearance, Pets, Keyboard, Snapshots, Plugins, Browser, Computer, Hooks, …) renders the shared "Coming soon" placeholder.

> Source: `crates/agent-ui/src/views/settings/panels.rs`

#### SettingsModelsPanel

Settings → General → Models: two-column form editor for the cx provider config (`~/.manox/cx.providers.config.yaml`). Left column is a tree nav: provider nodes (double-click header renames inline) whose expanded children are the four module names — 基本信息 / 环境变量 / 端点配置 / 模型列表 — and a dashed 「+ 添加 Provider」 button at the list end; clicking a module child selects (provider, module) and the wide right column renders that module's form in a bordered panel: 基本信息 (API Key kind dropdown/value pair), 环境变量 (indented key/value rows with per-row 「-」/「+」), 端点配置 (one card per endpoint) or 模型列表 (one card per model, 手动配置 / 自动获取 tab). Add-item buttons are dashed full-width and sit at the end of their lists. Remove controls are uniform 「-」 buttons with two-step confirmation (first click arms with a danger tint, second deletes); block-level removes (model / endpoint) sit outside the block's right edge, vertically centered; selected tree items use `theme.info` text; form blocks carry no background fill; the 手动配置 / 自动获取 tabs underline the active choice in `theme.info`; double-click provider rename exits on blur, Enter or mouse-down-out and autosaves. Every edit debounces into an autosave: validate, atomically write the whole config back (top-level `agents:` preserved verbatim), then reload the provider registry off the main thread. Autosave is disabled while the file fails to parse. Endpoints are unique per Wire API and displayed as Anthropic Messages / OpenAI Responses / OpenAI Completions; `agents:` filters are badge pickers fed by a dropdown (empty selection = all agents); supports_tools / supports_images echo their effective defaults instead of an unset state.

> Source: `crates/agent-ui/src/views/settings/models.rs`

#### SettingsChatGptAppPanel

Settings → External Tools → ChatGPT.app: visualizes and edits the ChatGPT.app injection settings (`manox_providers::ChatGptAppSettings`, top-level `chatgpt_app:` section of `cx.providers.config.yaml`, shared by the CLI and GUI launch paths). Visual language mirrors ChatGPT.app Settings: a big page heading (`text_xl`, the TitleBar renders no text), then per-block name (14px foreground, non-bold) + muted description left-aligned **above** a border-only rounded card (no fill) whose rows are separated by hairlines; each row is two-line (name foreground + description muted, left) with the value right-aligned. Four blocks: **Codex Home** (read-only CODEX_HOME value with copy / reveal-in-Finder), **Model Injection** (display nickname input — replaces the injected provider name when set, whatever provider is launched — plus the injection mode as a segmented two-choice — model list via CDP vs single model via the official config.toml mechanism, active segment a filled pill / inactive plain muted text, with the CDP risk note as the row's inline description — plus a read-only Providers & LLMs catalog, one two-line row per provider, per-provider fetch failures shown as a "failed to load" row rather than omitted), **Variable Injection** (custom env key/value rows with add/remove; reserved keys rejected on save), **More Settings** (`supports_websockets` switch, default false). Editable items autosave through the same debounced touch/save_generation mechanism as the Models panel; the Models panel carries `chatgpt_app:` over from a fresh disk read on save so the two panels never clobber each other. Launch args and the CDP script injection are internal mechanics and are not surfaced.

> Source: `crates/agent-ui/src/views/settings/chatgpt.rs`

#### SettingsSectionCard

Rounded container, `bg:secondary`, holds rows with hairline dividers.

> Source: `crates/agent-ui/src/views/settings/panels.rs`

#### SettingsRow

Single row: title (left) + control (right), optional description.

> Source: `crates/agent-ui/src/views/settings/panels.rs`

#### SettingsSectionHeader

Small bold label for a subsection.

> Source: `crates/agent-ui/src/views/settings/panels.rs`

#### SettingsHairline

1px divider between rows.

> Source: `crates/agent-ui/src/views/settings/panels.rs`

---

## 5. PluginManager (in Settings)

The plugin/skill management UI (PluginManagerView, tab bar, marketplace/plugin/skill cards) was retired with the manox harness. The Settings → Plugins item remains in the left nav and renders the shared "Coming soon" placeholder; the backend registry (`manox_agent::plugin::PluginManager`) stays in the agent crate. MCP server management has its own panel: Settings → MCP servers lists the merged config (mcp.toml + plugin declarations) with live connection state and persistent per-server switches (`[mcp] disabled` in settings.toml, applied on next launch).
Plugin management lives under Settings → Plugins (`PluginManagerView`): a Marketplace tab (add/refresh/remove marketplaces, browse + install their plugins) and a Plugin tab (installed set with update/enable/disable/uninstall), all async with a busy spinner + notice banner; registry changes apply on restart (notice texts say so). Skill authoring and mcp.toml server authoring tabs from the retired harness are not ported yet (need shared-layer write APIs).
---

## 6. 壳（Shell）

#### ChromeShell

应用壳根视图（`crates/manox-agent-chrome-ui/src/shell.rs`）：垂直布局 = 38px 工具栏（原生交通灯槽位 70px、侧栏开关、**←/→ 会话历史导航**（可用性由宿主经 `nav_avail` 查询钩子渲染期提供，无可去边界时置灰 inert）、session 下拉选择器、**「在 VS Code 中打开」**（`on_open_editor` → 宿主后台 spawn `launch_plain(前台 project)`，只失败推错误通知（成功由 VS Code 打开自证）；无项目时推「没有可打开的项目」错误通知，绝不静默打开 `$HOME`）、面板/右栏开关、**品牌位**（`ShellConfig.brand` 注入的 app logo 元素工厂，None 回退通用字形；刻意非交互，且被排除在窗口拖拽面外——mouse-down 被吞掉））+ 内容区（侧栏｜主区卡［主槽｜右栏］／底部 dock）。主区卡圆角 8px、卡缝 6px；两条调宽把手为隐形 absolute 层（挂在根做绝对坐标数学，载荷类型左右各一）。`ShellConfig` 注入主槽（`MainSurface`）、右栏 kind 注册表、dock surface、侧栏固定行/自定义行、品牌位与 `HostHooks`；会话行由宿主推送快照（`set_sessions`），壳自身只持交互态（分组折叠/拖排、下拉/行菜单、**侧栏分组模式与过滤词**）；选中高亮由宿主每次快照用前台线程 id 覆写（`shell.active`），不跟随点击。固定行（Automations/Chats）与 Customizations 块（Overview/MCP）**尚未实现**：整行 `FG_FAINT` 化 + hover「尚未实现」tooltip，保持 inert（2026-09-30 假控件清理：Run/split/右栏 split·external 已删——原版 Run 是 split-button、manox 无任务系统；Sync Changes 胶囊已删——git pull/push 集成另立特性）。

会话历史的权威栈在 `Workspace` 的 `NavHistory`（cap 100，去重判据是「当前指向项」而非栈尾）：用户发起的 `open_thread`（侧栏点击/下拉选中）与 fork 落地、新建落地（⌘N、/exit 的替换会话经 `attach_created_session`）都入栈并截断前进尾；←/→ 移动指针不重复记录；successor 换代把**当前条目原地改写**为后继 id（`replace_nav_current`——追加会把已处置的前任留在 ← 一步可达处）。降级语义：栈内 id 若在线程归档/换代后失效，回退仍走 landing attach（空 landing 可接受）——不剪枝是接受的取舍。

> Source: `crates/manox-agent-chrome-ui/src/shell.rs`, `crates/manox-agent-chrome-ui/src/titlebar.rs`, `crates/manox-agent-chrome-ui/src/divider.rs`, `crates/agent-ui/src/workspace/attach.rs`

#### ChromeSessionList

侧栏会话树（props 驱动，`crates/manox-agent-chrome-ui/src/session_list.rs`）：**三行 66px 行卡（2026-09-29 thread-item 设计稿）**——标题行（16px 状态槽 + 6px 间距 + 标题）、tag 行（短 id chip 恒首位 + 用户 tag chip）、info 行（仅最后活跃时间：72h 内相对、之外本地 `MM-DD HH:MM`），三行共用一条左基线、无任何右对齐内容、**不随项目层级缩进**（层级只由分组头与 leader chevron 表达）。行面上**零控件**：pin/archive/标签/复制 ID 全部收进右键菜单（`Shell::open_row_menu` 五项：置顶 toggle／归档 toggle／添加·重命名标签／移除标签／复制 ID；tag 内联编辑挂在 tag 行，Escape 取消、Enter/blur 提交、空值丢弃、10 字上限；双击用户 tag 芯片 = 老壳同款进入重命名编辑，短 id 芯片单击复制完整 id）。五态字形（`Errored` 红三角／`PendingAuth`·`PendingPlan` 实心 8px 蓝点／`Running` 像素积木 2×3 点阵 1820ms 阶梯循环（VS Code pixelSpinner grid 变体移植）／`Unread` 空心 6.5px 蓝点／`Idle` 空槽）。四态表面：未选中无背景、悬浮 `LIST_HOVER` + 标题转 500 字重 + **截断标题跑马灯**（双份标题 + 24px 间隔的无缝循环轨道：24px/s、每循环停 600ms、回绕点像素级相同无闪跳；仅 `is_hovered && title_truncated` 启动，移开复位；截断判定 = 与渲染器省略号同一套 `shape_text` 实测宽 vs `on_prepaint` 逐帧记录的剪裁盒宽）、选中白卡 + 15% 描边、键盘焦点 = `track_focus` + `focus_visible` 1.5px accent 环（↑/↓ 在可见行间移动焦点，行高四态一致不 reflow）；分组头可折叠（折叠态按**稳定 state key** 存取——`SessionGroup.key`，时间分组用 i18n 键字符串、workspace 分组用项目名，显示名随语言切换不落状态）并作为拖拽源/放置目标（2px accent 插入线；**仅 workspace 模式**——时间模式的分组头不是拖拽源、容器不是放置目标，渲染期直接不挂拖拽机械）；workspace 模式下分组头同时是**项目菜单**面（见 [ProjectGroupMenu](#projectgroupmenu)：头右端常驻 `MORE` 省略号钮（FG_FAINT，hover 转 FG）+ 头右键，二者都 `stop_propagation` 不触发折叠）。

头部右侧控件（2026-09-30 起为真控件）：**sort**（workspace ↔ 时间分组切换，时间模式下点亮；时间分组 = 本地自然日四桶「今天/昨天/最近 7 天/更早」，分桶与桶内排序同源 `sort_stamp`（member 沿用 leader 的戳，team 不拆桶不散序；成员单独置顶仍可上浮——pin 逐行的既有语义），空桶不渲染）与 **search**（展开 header 下过滤行：InputState 过滤输入 + × 清空；title/project/tag 不区分大小写包含，无匹配组隐藏、过滤中强制展开，全滤空时显示「无匹配会话」提示；纯壳内显示态，不持久化）。

> Source: `crates/manox-agent-chrome-ui/src/session_list.rs`, `crates/manox-agent-chrome-ui/src/shell.rs`

#### ProjectGroupMenu

项目分组头的动作菜单——旧壳侧栏（2026-09-28 随单壳切换退役）的「项目 `…` 菜单」在 chrome 壳上的回归。**职责切分**：chrome 只出**面**——分组头省略号钮/右键捕获打开位置，`HostHooks::on_group_menu(key, project, anchor)` 把 (state key, 项目路径) 交给宿主，宿主经 `agent_ui::project_menu::group_menu` 构建菜单实体，壳用与行菜单同型的 deferred/anchored 浮层挂载（`chrome-group-menu`），DismissEvent 收起；时间模式无此面（时间桶不是启动目标）。**内容**（`project` 为 `None` 的 Chats 桶保留全部启动行、按宿主回退 cwd 定界，仅隐藏移除行）：「新建会话」子菜单（Manox 平行行 = `start_new_thread(Some(project))`；Claude Code／Codex／GitHub Copilot 各一个 provider→model 级联——**复用 Composer 选模型的共享 builder** `model_cascade::build_model_menu`（`model_catalog::rows()` 按 provider 显示名分组、每行 wire Tag 徽标 + 模型显示名，与 composer 选择器同一 look & pick 交互），选中即 `spawn_agent_terminal` 以**该项目目录**为 cwd 拉起 CLI，经 `external_sessions::launch` 注册为**外部会话**（右栏 ToolTab + 侧栏行，见下）；「新建终端」（`spawn_standalone_terminal`，项目目录，同样注册为外部会话）；「VS Code」（`launch_vscode_app_from_settings` 注入式启动、后台执行，未装 VS Code 或无目录可用时禁用）；分隔线 + 「移除项目」（`project_registry::forget_project` overlay 写 + `thread_store::remove_project` 同步 v2 账户，`ws.notify()` 立即重投影——组溶解、会话落回 Chats 桶，历史不动）。

**移除的持久化**（AHP 世界无项目注册表可反注册，分组读会话自身 project 绑定）：`agent_ui::project_registry` 在共享 `~/.manox/settings.toml` 的 app 自有键 `removed_projects` 上做 parse-edit-serialize（与 `ui_language` 同款只碰己键纪律）+ 进程级缓存（投影每次 multiplexer notify 都要查，不逐帧读文件）；`project_groups(rows, unread, removed)` 把命中行**改投 Chats 桶而非隐藏**。重新绑定即重新注册：`Workspace::register_project_in_store` 是全部绑定路径的汇聚点，顺带清 overlay 条目（往目录里拉起会话 = 重新注册）。

**外部会话 = 主列会话**（`agent_ui::workspace::external_sessions`，旧壳 external-session 家族的 chrome 回归）：AHP 无外部会话通道，注册表是客户端状态、挂在 Workspace 实体上（`externals: Vec<ExternalSessionRecord>` + `active_external`，各记录持 live `TerminalView` 实体）——项目菜单拉起 agent/终端即注册并**顶替会话列占主列**（`ViewMode::ExternalSession`，与 Settings 同一条主列换页轨道；壳子套主列，终端/TUI 就放主列，不放右栏）。切走即停靠（点 thread 行/新建会话隐式 `leave_external_session`，终端保活）；侧栏行走 `on_select` 的 `ext-` 前缀分叉 → `open_external_session` 切回主列（同步 ws 更新，无 window handle 往返，无 dispatch 时序坑）；行菜单「关闭会话」→ `close_external_session`（drop 记录 = 拆进程树，正在前台则回落会话列）。行投影 `external_session_rows` 并入装配层快照、壳按显示名归入项目组；高亮不被前台线程规则抢走（`active_is_external` 不回写）。主列标题（`PendingMain::title`）在外部会话前台时显示会话名。**进程级寿命**：PTY 属于 Workspace（进程单例），窗口重开仍在（比右栏 stash 的窗口级寿命更强，与 legacy 一致）。行级抽象同款 `SessionRowKind::{Thread, External{icon}}`——品牌位 + 关闭会话菜单；时序律仍有效：open/close_tool_tab（右栏页签通用路径）在 click dispatch 内会被拒（探针测试钉死），外部会话已改为纯 ws 更新不受其辖。

> Source: `crates/agent-ui/src/project_menu.rs`, `crates/agent-ui/src/project_registry.rs`, `crates/agent-ui/src/external_sessions.rs`, `crates/agent-ui/src/chrome_assembly.rs`, `crates/manox-agent-chrome-ui/src/shell.rs`

#### SidebarProjection

wire 行 → chrome 侧栏 props 的**纯投影**（`crates/agent-ui/src/sidebar_projection.rs`）：`ThreadListItem`（multiplexer 权威行，含 §D.5 增量合并）→ 五态优先级（errored > pending_auth > pending_plan > running > unread，叶子 unread 镜像覆盖 wire 标志）＋ team 森林（leader 保序、member 随后、孤儿拍平；**member 不再缩进**，leader chevron 是唯一嵌套标记）＋ `updated_at`/`archived`/tag/pinned 原样透传 ＋ 按项目路径尾段分组。装配层在 multiplexer notify 时喂给 `ChromeSessionList`。

> Source: `crates/agent-ui/src/sidebar_projection.rs`

#### ChromeRightPane

右栏外壳（`crates/manox-agent-chrome-ui/src/right_pane.rs`）：圆角卡 + 页签条（**下划线式页签**：平面标签压在条带自身的 `border_b_1` 共享轨道上，激活项为 `ACCENT` + 半粗；一条**共享的下划线指示器**按激活 id 播放滑动动画，从旧页签横移到新页签；条右端仅「+」一个动作——再开一个激活 kind 的实例；无激活页签（新标签页空态）时按共享扁平按钮的 Disabled 形态置灰且不挂点击，2026-09-30 删除无语义的 split/external 假钮；**条行容器必须显式 `.flex()`**——gpui div 默认 block，缺了动作组会换行压进正文）+ 新标签页空态（快捷操作由注册表生成）+ 打开/激活/关闭生命周期（最后一个页签关闭即收起）。

页签几何由 `on_prepaint` 实测上报（`TabBounds`，键为页签 id），指示器据此定位——标签宽度不一，无法由序号推出。注意 `on_prepaint` 上报的是**内容盒原点**（它挂的是 `canvas().absolute().size_full()` 子元素，padding 已计入），故记录时减去 `TAB_PL` 还原页签左边界；指示器与条带是**兄弟**（同在 relative wrapper 内）而非父子——gpui 的 `Style::paint` 先画子元素、**后画自身 border**，所以子元素永远压不住条带的 `border_b_1`，会只剩半截可见。wrapper 即指示器的包含块，其原点也就是 tab 几何的反基准坐标系。内容经 `ToolTab` 注入、kind 经 `ToolTabFactory` 注册；**实例级 id**（一种 kind 可多开）。**per-thread 会话**：`RightPaneSession{open, store, active_id, visible}` 整体 stash/restore（挂起走 `on_active(false)`——浏览器子视图隐藏、终端保活；仅显式关页签才拆内容）。快照经 `ToolTab::persist` / `ToolTabFactory::restore`（浏览器 `{"url"}`、编辑器空稿可恢复；终端与 CLI 会话不可复活，恢复时丢弃）落 `threads.db` 的 `thread_right_pane`。

> Source: `crates/manox-agent-chrome-ui/src/right_pane.rs`, `crates/agent-ui/src/chrome_assembly.rs`

#### ToolTabRegistry

chrome 壳右栏的 kind 全集（`crates/agent-ui/src/tool_tabs.rs`，快捷操作顺序）：**终端**（$SHELL，独立 PTY，关页签拆进程树）、**Claude Code / Codex / GitHub Copilot**（页签体先落模型选择器——复用共享级联投影 `cascade_provider_groups`；点选即以该端点 `AgentBuilder` 拉起 CLI，picker 实体此后自渲染 TUI；cwd = 前台线程项目目录）、**编辑器**（markdown 软换行 + 行号）、**浏览器**（真 `BrowserView`：地址栏 + 导航；走生产 `restore_browser_tab`/`close_browser_tab`，注册进进程级 `WorkspaceBrowserHost`——IPC notify/inbound、eval oneshot、yield 全通；2s ticker 把页面 `<title>` 镜像到页签标签，`on_active` 隐藏 OS 子视图防漂浮）。

> Source: `crates/agent-ui/src/tool_tabs.rs`

#### ChromePanel

底部 dock（`crates/manox-agent-chrome-ui/src/panel.rs` + `shell.rs` 的渲染）：通用容器，内容经 `PanelSurface` 注入（`open`=展开即拉起、`close`=收起即回收；新建/清理/收起三个动作）。manox 装配装集成终端，cwd 随前台线程；dock 视图按线程 stash/restore（同右栏语义）。

> Source: `crates/manox-agent-chrome-ui/src/panel.rs`, `crates/agent-ui/src/chrome_assembly.rs`

#### ConversationColumn

`Workspace::render_column`：会话列本身（hero 空屏／虚拟化消息列表／composer footer＋附件与 chips／ask 与 blank-project overlay／TurnNavigator overlay），四周的 gutter / 侧栏槽 / 卡壳 / 内嵌标题栏全部归壳；键盘动作面经根装饰器 `apply_chat_actions`。装配把该视图作为壳的 `MainSurface`，其 multiplexer 同时喂侧栏投影泵。

> Source: `crates/agent-ui/src/workspace/render.rs`

#### SettingsCard

Settings（`Workspace::render_settings_card`）：同一状态机（`ViewMode::Settings` + `SettingsView` 订阅），渲染换位——设置导航列（240px）｜分隔线｜面板 放进主卡（chrome 侧栏槽是会话列表）；⌘, 与原生菜单 `Settings…` 经 `apply_chat_actions` 的 `OpenSettings` 处理进入，返回走 nav 的 back 控件（`SettingsEvent::Exit` → 滑出后回会话）。

> Source: `crates/agent-ui/src/workspace/render.rs`, `crates/agent-ui/src/views/settings/mod.rs`

---

## 7. Shared Primitives

Reusable UI elements from `gpui_component` and `manox-components` used across all views.

#### Button

Clickable button with variants (primary, ghost, outline, danger, etc.).

#### Input / InputState

Text input field (single-line or multi-line auto-grow).

#### PopupMenu

Dropdown menu with items, submenus, separators, labels.

#### PopupMenuItem

Single menu row: icon + label + optional trailing.

#### TabBar

Horizontal tab bar with selectable tabs.

#### Tag

Small colored chip/badge with variant colors.

#### TerminalPanel

First-party selectable text panel (`manox-components::markdown::TerminalPanel`, an `Entity` + `Render` in the `markdown` module) that renders tool output as a terminal-styled shell — **not** a real terminal (no PTY, no grid; `crates/manox-terminal`/`TerminalView` are not involved). One persistent `Entity<TerminalPanel>` is owned per `ToolCallItem` (live + reloaded history) so the document-level `DocSelection` and its `FocusHandle` survive across re-renders: a drag started on frame N keeps its anchor on frame N+1, and Cmd/Ctrl+C reaches a stable focus handle — the same persistence fix that makes assistant/thinking text selectable. The panel renders **only the body**: a transparent, untinted vertical flex (no background fill on the content — it blends into the message list; mono font at `text_sm`（代码档，与代码块同号；thinking body 走 markdown 正文档 `text_base`）; `px_3 py_2`; `cursor_text`) that mounts a zero-size sentinel first, then a single `RichText` document composed of a prompt block + the body. The prompt block appears **only for `bash`** — the one tool that runs a real shell command a human would type in a terminal; internal tools (`grep`/`read_file`/`edit_file`/`glob`/`list_directory`/`monitor`/…) and MCP tools are manox abstractions, not terminal commands, so they render the body only (no cwd / `❯` preamble that would imply "run this in a shell"). The `bash` prompt block: line 1 cwd (home `~`-collapsed) + `git:{branch}` + status markers (`*mod ✘del !conflict ?untracked`, zero counts and the whole git segment omitted when not a repo); line 2 `❯` (green) + the echoed command. **Three-way text styling** separates prompt chrome / input / output by color × slant: guidance (cwd / `git:` / branch / markers / `❯`) — foreground + **upright**; the echoed command — foreground + **italic**; the body (output) — muted + **italic**. The doc div's `.italic()` sets the base the `RichText` unstyled ranges inherit (so command output / file content / diff context all read muted + italic); the prompt-block guidance and command runs pin their own slant via `styled(color, italic)` so the body's inherited italic does not leak into the prompt. The body is rendered per a `PanelKind` chosen by the agent-ui layer from the tool name (`tool_panel_body`): `File` (`read_file`/`write_file`) — a sequential line-number gutter (the agent-ui layer pre-strips the hashline `[path#TAG]` header + `N:` prefixes for `read_file` and feeds the written `content` for `write_file`, so the panel just numbers the content lines 1..N); `Diff` (`edit_file`) — `+`/`-` lines green/red, `@@` hunk headers cyan, `[path#TAG]`/`---` separators muted; `Plain` (default, `bash` + everything else) — `vte::ansi::Processor` parses SGR foreground/bold/italic into `HighlightStyle` ranges (control bytes stripped from the plain text, truecolor/256/16-color resolved; background SGR tracked but not painted). The whole document wraps at panel width (long lines wrap, no horizontal blow-out) and is one continuous selection across prompt + output. The terminal chrome — a **titlebar** showing the command summary (`gh issue create` for `bash`, `read_file path` otherwise) + status + disclosure chevron, click-to-toggle the body — lives in the agent-ui header (`render_tool_entry` / `render_tool_call`): the titlebar and this body share one bordered rounded frame so the pair reads as a single terminal window (titlebar gets a `border_b_1` separator only while the body is shown). Selection supports double-click word (a click inside a registered inline-code span selects the whole span), triple-click line, drag-extend, and Cmd/Ctrl+C copy — shared with `Markdown` via `DocSelection`. Git state is snapshotted per `bash` panel by the agent-ui layer via a background `git status --porcelain` + `git rev-parse --abbrev-ref HEAD` probe keyed off the thread cwd (internal tools skip the probe — they render no prompt block). `render_tool_output` returns `item.panel.into_any_element()` when mounted, falling back to a fenced code block otherwise. **AskUserQuestion answered-state exception** (B2-PR-2): when the call is an `AskUserQuestion` and its `output` parses as the canonical result JSON (`{"answers":[{"id","selected","custom"?}]}` — what both the live `ToolResult` deposit and the rebuild journal translate carry once the server makes the model-facing result canonical), `render_tool_output` short-circuits the panel/fenced paths and renders compact human Q/A rows via `ask_result_qa_rows` + `render_ask_result_body` — the question text recovered from the call's own `input` by `id` (unknown id → the id itself), a skip (`selected: []`, no `custom`) shown as an em dash. Any non-canonical payload (the transitional prose render, an error, malformed JSON) falls through to the raw output verbatim, so the fold is forward-compatible and never regresses on a parse miss.

**Pagination.** A finalized body renders `PAGE_SIZE` (20) lines at a time; a "load more" affordance below the body (a centered `ChevronDown` + `+N` count, top-bordered, hover-tinted) grows the window by another page via `show_more`, clamped to the total. Streaming bodies render the whole live output (no pagination); on the streaming→finalized transition the cursor resets to the first page so the result opens at the top. The panel has **no internal vertical scroll** — the message-list `message-list` div scrolls the whole panel — so `show_more` never touches a scroll handle: growing the window appends lines below the current viewport without jumping to the tail. The pixel-anchored, tail-following message-list arbitration (recomputed each frame in `on_prepaint`) keeps the viewport at the user's reading position across the growth, so successive "load more" clicks stay anchored to the current line rather than snapping to the end.

> Source: `crates/manox-components/src/markdown/terminal_panel.rs` · wired by `crates/manox-agent-chat-ui/src/views/message.rs` (`tool_panel_body` → `ensure_tool_panel` / `sync_tool_*_panel` / `rebuild_tool_panels`, titlebar frame in `render_tool_entry` / `render_tool_call`) + `crates/manox-agent-chat-ui/src/conversation.rs` (`apply` ToolOutput/ToolResult arms, `rebuild_from_messages`)

#### Markdown

Self-built stateful markdown renderer (`manox-components::markdown::Markdown`, an `Entity` + `Render`) replacing `gpui_component::TextView::markdown`. Owns the source + an `IncrementalParser` (parse-once: freezes the completed prefix so a streaming append only re-parses the growing tail) + a document-level `DocSelection`. The `Render` builds a focusable vertical flex root mounting a zero-size sentinel as the first child — the sentinel clears the per-frame block registry at paint start, then each block's `RichText` re-registers its geometry during paint; the root's mouse listeners hit-test that registry to drive one continuous selection across paragraph / code / list boundaries, and the key listener copies it on Cmd/Ctrl+C. Click semantics: single click places the anchor + starts a drag; double-click selects the word at the click (a click landing inside a registered inline-code span selects the whole span verbatim); triple-click selects the line. `RichText` is a Manox-owned public-GPUI leaf using `shape_text` + `WrappedLine` for both measurement and paint; min-content and max-content probe answers live in dedicated cache slots and never replace the exact-width layout used for paint, and min-content width comes from shaped glyph segments partitioned by Unicode line-break opportunities. The same definite-width geometry drives selection/link hit testing; it does not use GPUI `StyledText`'s old constraint cache. Block visuals: paragraphs/headings use highlighted `RichText`; code blocks have a line-number gutter + `overflow_x_scroll` + tree-sitter highlighting (highlight result cached per `(lang, content_hash)`); unified-diff blocks have an accent wash + left bar; GFM tables have column alignment + horizontal scroll + a hover-revealed copy control that copies the table's own source markdown verbatim — `Block::Table` carries the node's absolute source range (`parse_tail` shifts it by the tail base, so it stays valid across freeze states) and the control slices it out of the document, so links, emphasis, and image targets survive exactly as written (re-serializing the parsed cell text would silently drop them); task-list checkboxes; code/table block hover copy control (diff/conflict blocks have none). Every copy control flips to a check briefly after its click (see [Copy Feedback](#copy-feedback)). Streaming bodies paint plain text + cursor; the full layout mounts once the stream ends.

#### Copy Feedback

Transient copied state for copy controls (`manox-components::copy_feedback`): the owning entity holds a `CopiedRegistry` keyed by each control's `ElementId` and exposes it via `CopyFeedbackHost`; `copy_button` (`ghost` + `xsmall`) shows `IconName::Check` instead of `Copy` while its id is lit — `COPY_FEEDBACK` (1.2 s) after the click — and routes the click through the owner's weak handle (`fire_copied` marks the generation, notifies, and spawns the revert timer; a re-fire while lit replaces the generation so only the newest timer reverts). Wired by the markdown renderer's code/table controls (`Markdown` holds the registry) and the message copy buttons (`MessageItem` holds it, threaded as `CopyFeedback` through the item renderers); the ChatGPT settings panel's text control swaps its label instead (「已复制」/ "Copied", `settings-btn-copied`).

> Source: `crates/manox-components/src/copy_feedback.rs`

#### TurnFrame

Shared framed text container (`manox-components::turn_frame::TurnFrame`) used for user turns. It paints one continuous accent-colored stroke path for the door-shaped frame, leaving the bottom center open while preserving rounded `╰─` / `─╯` corners. The lower stroke is lifted slightly into the bottom padding so the open edge visually hugs the final text line without letting markdown content overflow its layout box. The component does not fill the content background, does not rely on masking a complete border, and avoids assembling the frame from independent rail nodes. Callers provide header, trailing controls, and body content.

> Source: `crates/manox-components/src/turn_frame.rs`

#### Icon

Named icon from the icon set (e.g., `IconName::Folder`, `IconName::Search`). 全部图标统一走 SVG 方案：组件层用 gpui-component 的 `Icon`（`IconName` 枚举或 `Icon::default().path("icons/…")`），运行时经 `ExtrasAssetSource`（manox 本地 svg 优先 → `gpui-kit-assets::AllAssets` 全量 Lucide）解析。

chrome 壳自有图标表在 `manox-agent-chrome-ui/src/theme/icons.rs`：`IconAsset(pub &'static str)` 常量即 svg 资产路径（Lucide 名），宏同时生成常量与 `ALL` 列表；`theme::icon(glyph, size)` 返回 `gpui_component::Icon`，且 `IconAsset` 实现了 `IconNamed`（常量可直喂 `PopupMenuItem::icon` / `Button::icon`）；颜色继承祖先 `text_color`。守护测试遍历 `ALL` 断言每条路径在嵌入 bundle 内可解析 + 路径两两不重复（上游改名测试即红）。旧 codicon 字体方案（codicon.ttf + `FONT_ICON`）已退役。

#### BrailleSpinner

Text-based braille-dot spinner cycling through `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` (10 frames, 800 ms cycle). Used wherever an in-progress indicator is needed, replacing the old rotating-circle `Spinner`.

> Source: `crates/ai-elements/src/spinner.rs`

#### ScrollHandle / ScrollableElement

Scroll container with custom scrollbar.

#### TitleBar

Standard title bar component from gpui_component.

#### ContextMenu

Right-click context menu.

#### Tooltip

Hover tooltip.

---

## 8. Permission Modes

Three file-effect modes drive the seatbelt profile (bash) and the fs write fence; a denied call carries a `[sandbox: …]` marker and a `sandbox_permissions`+`justification` escalation path through the `ToolCallAuthorization` card. Switching is hot: a mid-turn switch governs the very next tool call, and the chip flips optimistically on click (`/mode` also dispatches immediately mid-turn).

#### Read Only

Amber — bash runs but writes are denied by the seatbelt; fs mutations refused.

#### Workspace Access

Green — writes under the workspace, the manox home (~/.manox), and temp areas; bash confined to the workspace-write profile (default).

#### Full Access

Red — no sandbox; bash unsandboxed, fs mutations unfenced.
---

## 9. Tool Call Statuses

States of a [ToolCallCard](#toolcallcard).

#### PendingApproval

Waiting for user, surfaces as the [AskDrawer](#askdrawer) question card.

#### Running

Braille-dot spinner or shimmer.

#### Success

Green check, collapsible output.

#### Error

Red X, error output.

#### Denied

Greyed out, "denied" label.

---

## 10. Reasoning Effort Levels

Thread-level reasoning effort, chosen in [ModelMenu](#modelmenu) (High / Max, current effort checked), persisted in the session sidecar so a reopened session restores it. The value is inherited across `/new` (`archive_current_thread_inheriting`) and set via the engine facade; High / Max remain the canonical values.

#### High
#### Max
