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
计划与后续见 `PLAN-CHROME-CHAT-SPLIT.md`。

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
| ContextRail（usage/cost/cockpit 相位/git 状态/plan/changes/branch） | ✅ | cost 来自内核 `session_stats`（rate card 计价）；cockpit 相位随事件流驱动 |
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

- [ChromeShell](#chromeshell) · [ChromeSessionList](#chromesessionlist) · [SidebarProjection](#sidebarprojection) · [ChromeRightPane](#chromerightpane) · [ToolTabRegistry](#tooltabregistry) · [ChromePanel](#chromepanel) · [ConversationColumn](#conversationcolumn) · [SettingsCard](#settingscard)

### 顶层

- [Window](#window) · [NativeMenuBar](#nativemenubar) · [AboutWindow](#aboutwindow) · [Workspace](#workspace) · [ViewMode](#viewmode) · [ViewMode::Workspace](#viewmodeworkspace) · [ViewMode::Settings](#viewmodesettings)

### MainView

- [MainView](#mainview)

### MessageColumn

- [MessageColumn](#messagecolumn) · [Body](#body) · [FollowStoppedNotice](#followstoppednotice) · [FollowStopProjection](#followstapprojection) · [TurnRail](#turnrail)

### ContextRail

- [ContextRail](#contextrail) · [ContextRailPanel](#contextrailpanel) · [ContextRailCollapseBtn](#contextrailcollapsebtn) · [ContextRailChangesRow](#contextrailchangesrow) · [ContextRailBranchRow](#contextrailbranchrow) · [ContextRailBranchMenu](#contextrailbranchmenu)

### Hero / LoadingIndicator

- [Hero](#hero) · [LoadingIndicator](#loadingindicator)

### MessageArea

- [MessageArea](#messagearea) · [MessageList](#messagelist) · [MessageItem](#messageitem)

### MessageItem 变体

- [UserMessage](#usermessage) · [AssistantMessage](#assistantmessage) · [AssistantActions](#assistantactions) · [ReasoningBlock](#reasoningblock) · [ActivitySegment](#activitysegment) · [ToolCallCard](#toolcallcard) · [AgentTaskCard](#agenttaskcard) · [BackgroundTaskCard](#backgroundtaskcard) · [ErrorMessage](#errormessage) · [NoticeMessage](#noticemessage) · [RecapCard](#recapcard) · [CacheMissDivider](#cachemissdivider) · [RetryBadge](#retrybadge)

### Footer / Composer

- [Footer](#footer) · [Composer](#composer) · [QueuedFollowUps](#queuedfollowups) · [ComposerDivider](#composerdivider) · [AttachmentChips](#attachmentchips) · [AttachmentChip](#attachmentchip) · [BrowserSuiteChip](#browsersuitechip) · [ComposerInputRow](#composerinputrow) · [InputField](#inputfield) · [SendBtn](#sendbtn) · [ModelChip](#modelchip) · [AccessChip](#accesschip) · [ProjectChip](#projectchip) · [FollowStopProjection](#followstapprojection)

### AskDrawer

- [AskDrawer](#askdrawer) · [AskDrawerHeader](#askdrawerheader) · [AskDrawerQuestion](#askdrawerquestion) · [AskDrawerOptions](#askdraweroptions) · [AskDrawerCustomInput](#askdrawercustominput) · [AskDrawerFooter](#askdrawerfooter) · [AskDrawerSkipButton](#askdrawerskipbutton) · [AskDrawerNav](#askdrawernav) · [PlanReviewDecisionCard](#planreviewdecisioncard) · [AskSettledElsewhereNotice](#asksettledelsewherenotice)

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
ask / blank-project overlay、浮动 [ContextRail](#contextrail)、TurnNavigator overlay，
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
列宽口径：浮动的 [ContextRail](#contextrail) 不是 flex 兄弟列，而是绝对浮层
（`absolute().top(TITLE_BAR_HEIGHT + 16).right(16).w(ENV_CARD_WIDTH).occlude()`，内容高度），
会话正文预留 `ENV_CONTENT_INSET` 右内边距；窄于 `RAIL_NARROW_BREAK`（消息列 900px）时卡片折叠、
消息列吃满。[TurnRail](#turnrail) 可见时正文另预留 `GUTTER`（40px）左内边距。TurnNavigator 浮层
锚定卡片内边距盒（左右各 `CARD_BORDER / 2`，显示 rail 时右侧再加其内容内边距）。

```
┌ 壳主区卡（圆角 + 边框）──────────────────────┐
│ ╭─────────────────────────────┬──────────╮ │
│ │ 会话列                      │ 右栏     │ │
│ │ 刻度│ 消息列表 / hero       │ ToolTab  │ │
│ │ TurnRail（≥2 轮）│          │ 页签体   │ │
│ │  浮动 ContextRail           │          │ │
│ │  composer footer            │          │ │
│ ╰─────────────────────────────┴──────────╯ │
└────────────────────────────────────────────┘
```

### 3.2 MessageColumn

Central conversation column, flex-1 — the main card's only content (the shell's sidebar and right
pane are siblings of the card, outside this view). The [ContextRail](#contextrail) floats over this
column's top-right as an absolute overlay (not a flex sibling); the conversation body reserves
`ENV_CONTENT_INSET` right padding when the rail is shown so the message list clears it.

#### MessageColumn

Vertical flex container, fills remaining width.

> Source: `crates/agent-ui/src/workspace/render.rs`

#### Body

Vertical flex below TitleBar, `pt:TITLE_BAR_HEIGHT`, houses the [FollowStoppedNotice](#followstoppednotice) (only while the follow stream has stopped) and then [Hero](#hero) (with the [LoadingIndicator](#loadingindicator) while an empty session restores) or [MessageArea](#messagearea) + [Footer](#footer).

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

#### LoadingIndicator

Centered BrailleSpinner + "Loading conversation…" (`workspace-loading-history`), shown inside the [Hero](#hero) while a sidebar-opened session's history is still restoring. The composer mounts immediately below it and accepts draft edits; send remains disabled and keyboard submission is gated on the thread's `HistoryPhase` until `Ready`. Preview batches stream into the [MessageArea](#messagearea) incrementally (`ThreadEvent::HistoryProgress`); once the first preview content lands, the composer moves to the [Footer](#footer) without waiting for the authoritative restore.

> Source: `crates/agent-ui/src/workspace/render.rs`

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

Marks are re-derived from the conversation every frame — but only past the width gate, which short-circuits before the projection, and `render_turn_rail`'s `Some`/`None` is the single gate for both the rail and the gutter (`collect_rail_turns`: prompt = the user bubble's text collapsed and capped at 50 chars, or the ⌘M navigator's attachment-only / empty-message copy for a textless bubble — the same distinction `TurnEntry::new` draws; response = the turn's last non-empty assistant reply capped at 120 — dsh's `findLast` rule; both caps run on a `2×limit` prefix slice so a huge turn costs O(limit), not O(全文)). The active mark is `active_rail_turn`: the last turn whose anchor item is at or above the list's `logical_scroll_top` item (the tail-follow floor reports `count`, resolving to the newest mark). Interactions: hover grows the tick 12→18px with a border→muted 140ms tween and the vacated mark sinks back in the same run — a pointer sweep reads as a wave down the ladder (dsh's CSS-transition semantics; each change keys one tween pair under `turn_rail_hover_gen`, with `turn_rail_hover_prev`/`_painted` snapshotting only on change; fast sweeps within the 140ms window truncate the wave's tail by design — the prev slot is single); hover opens a 300px preview card beside the rail (prompt line + response excerpt), fading in over 120ms with a 4px slide and traveling between marks over 140ms `ease_out_quint` (the from-top snapshots only when the hovered mark changes — the tab-indicator `indicator_from` discipline); click jumps through `Workspace::reveal_message` (the ⌘M navigator's own path). The active tick tweens width+color over 140ms on change (previous mark shrinks, new mark grows, keyed per generation; a tick that loses active while hovered hands the animation slot to the hover wave); active-follow scrolls the ladder (`scroll_to_item(Nearest)`) whenever the pointer is outside the strip (`turn_rail_pointer_inside` pauses it so marks never travel under the hand). An over-420px ladder scrolls inside the strip (`uniform_list` + `ListSizingBehavior::Infer` + `max_h`); the preview's geometry consumes the ladder's `ScrollHandle` offset **sign-corrected to positive-down** (gpui's raw offset runs negative scrolling down — the `-offset.y` convention `uniform_list` itself uses; round-1 C1). Thread re-projection (`attach_thread`, diagnostic replace) resets the interaction state via `ChatColumn::reset_turn_rail_interaction` — a stale hover index must not mount a preview on the new conversation. Tick rows follow the gpui hover-crossing rule: a row's leave retracts only its own mark.

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

Inline transcript card, not a footer swap. Every
`ThreadEvent::ToolCallAuthorization` — `AskUserQuestion` calls and bubbled
team-member questions — sets the pending state AND synthesizes the matching
`ToolCall` row (same-frame guarantee); the payload's options carry the
decision. A parked thread's subscription drops that event by design, so the
card re-surfaces on switch-back by an explicit re-own: the reclaim sends
`OpenSession` (same leaf and follow stream — the parked session never
detached) and the gateway replays the unsettled adjudications to the joining
owner (manox §D.6). A card whose id leaves the leaf's `pending_auth` projection
after having been confirmed in it settled remotely and is reconciled away.

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

#### AskSettledElsewhereNotice

PR-4 first-claim-wins: when an ask/approve delivery is settled on ANOTHER
client (or the approve quorum fails remotely), the server sends a session-less
`ServerNote::DeliveryCancelled { delivery_id }`. The multiplexer broadcasts it to
every leaf (keyed by delivery id, not session); the owning leaf's
`ClientStore::handle_delivery_cancelled` reverse-looks the delivery to its auth
id, drops the reply (`pending_auth`) + withdrawal (`pending_auth_delivery`)
correlations and the projection-set membership (so
`reconcile_pending_with_projections` retires the card — under the
`pending_projection_confirmed` guard), and arms the auth id in
`settled_elsewhere`. The workspace's `notice_settled_elsewhere` then drains the
set and surfaces one transient `Notification::info` ("handled on another
client"). A local settle (`resolve_ask` / `dismiss_ask` / `resolve_auth`) calls
`retire_auth` so a later note for the same delivery cannot mis-fire the notice.

> Source: `crates/manox-agent-chat-ui/src/client_store.rs` (`handle_delivery_cancelled`, `retire_auth`) + `crates/manox-agent-chat-ui/src/client_store_handle.rs` + `crates/agent-ui/src/multiplexer.rs` (broadcast) + `crates/agent-ui/src/workspace/chips.rs` (`notice_settled_elsewhere`)

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

### 3.3 ContextRail

Right-side context panel that floats over the Workspace's conversation column top-right as an absolute overlay — NOT a flex sibling of [MessageColumn](#messagecolumn). The `Render` impl positions it (`absolute().top(TITLE_BAR_HEIGHT + 16).right(16).w(ENV_CARD_WIDTH).occlude()`); the panel body (`render_panel`) carries the card chrome (`border_1` / `rounded(theme.radius)` / drop shadow / `bg:background` + `p_3`/`gap_2`). Content height, never full-height — a compact floating card, not a flush column or a second title bar. The conversation body reserves `ENV_CONTENT_INSET` (card width + 36px gutter) right padding so the message list never hides behind the card. Owned by `Workspace` as `Entity<ContextRail>`; the rail owns the cockpit state (run phase, the model's `PlanSnapshot`, per-cell counter animation) that used to live on `Workspace`.

Visibility is gated on the main-column body width (`ContextRail::rail_width_for`): shown as `Some(ENV_CARD_WIDTH)` (260px) at/above `RAIL_NARROW_BREAK` (900px), folded away (`None`) below it. The card's `top` clears the shared [TitleBar](#titlebar) overlay.

The card is hidden on the empty first screen and before the thread has interacted; the right pane's
width is the shell's business (it sits outside the card), so the only width the rail gate reads is
the card interior. The card floats as an absolute overlay (content height); the conversation column
is `flex_1`/`min_w_0` and reserves `ENV_CONTENT_INSET` right padding when the card is shown.

#### ContextRail

Floating absolute card over the conversation column's top-right (`absolute().top(TITLE_BAR_HEIGHT + 16).right(16).w(ENV_CARD_WIDTH).occlude()`). Owns `Entity<Thread>` and renders the panel body (`render_panel`) which carries the card chrome (border / rounded / shadow / background + `p_3`/`gap_2`) at content height.

> Source: `crates/manox-agent-chat-ui/src/views/context_rail.rs`

#### ContextRailPanel

Panel body (the card's content, content height — no internal scroll surface, though the plan section has its own bounded scroll region). The conversation-info rows (title, status, changes, branch) sit above the usage tree; the plan section renders from cockpit state owned by the rail.

Contents, top to bottom:

- **Header**: bold title (i18n `context-rail-title`) + a [ContextRailCollapseBtn](#contextrailcollapsebtn) ghost button.
- **Agents section** (`render_agents_section`): `Bot` icon + "Agents" / "智能体" header (i18n `context-agents-title`), then a Captain row plus one observe-only row per pi sub-agent fed by `SubagentProgress` events. Each row: status indicator + truncated `{type} · {topic}` title; live rows append a truncated one-line watchdog health verdict (working / tool running / stalled / looping) from the event's `health` field — `stalled` renders warning-colored, `looping` danger-colored, others muted, and the row tooltip reads `{title} — {health}`. Sub-agent rows are left-aligned with the Captain row (shared icon column, no extra indent — the earlier `pl(12px)` that offset them was removed). Rows are flat (pi sub-agents never nest deeper than one level) and open the sub-agent tab on click.
- **Status block** (`cockpit_status_block`): a two-line card — phase label (semibold) on line 1, an xs muted elapsed+tokens meta line (i18n `cockpit-run-status-meta`) on line 2. Elapsed refreshes per-second via the thinking ticker.
- **Usage section** (`render_usage_section`): `zodiac-scorpio` icon + "Usage" / "消费" header with cumulative token total, plus cumulative USD cost via `format_cost` when the session carries priced usage (kernel `session_stats` rate-card pricing); a hover tooltip splits main-call vs side-call usage. Then a per-model tree (sorted by total tokens desc, empty for unused models). Each model node is keyed by the canonical `{provider}/{model}` identity the server fold emits verbatim in the §E.3 payload's `model` field (`ClientStore::apply_conversation_info` keys `per_model_usage` by it directly — it must not re-prefix `provider`, or every `split_once('/')` resolution below silently fails); the node carries a tree prefix (`├─` / `└─`) and renders the resolved display pair `{display_provider_name}/{display_name}` (e.g. `百炼/qwen3.8-max`), with the model segment tinted by its wire api via `pi_wire_text_color` (theme-token `info`/`success`/`warning`, shared with the composer model chip), and the `[1m]` context-window suffix resolved from pi registry metadata (`model_window_tokens`); it falls back to the raw key only when the registry cannot resolve it. Tree children per model (indented `│   ` / `    ` + `├─` / `└─`):
  1. **Context budget row**: `{pct}% {used}/{cap}` from `context_budget_pct(window_tokens, effective_context_tokens(...))`; only when the model's window size resolves. Goes warning-colored at ≥90%.
  2. **Token row**: `↑{input} ↓{output} R{cache_read} CH{cache_hit%}` (`--` when there is no input to measure). `CH` renders via `format_cache_hit`: an imperfect hit rate never rounds up to a full 100% — values in the rounding-up band clamp to the top value at the display precision (99.9% at one decimal), so only an exact 1.0 ratio reads as full.
  3. **Cost row** (only for priced models): `format_cost(cost)` from `Thread::per_model_cost`.
- **Plan section** (`render_plan_section`, collapsible via `ToggleCockpitTasks` / ctrl/cmd-shift-m, `cockpit_hide_tasks`): the model's execution plan, taken verbatim from the `PlanSnapshot` it publishes via the `UpdatePlan` tool.
- **Changes row**: [ContextRailChangesRow](#contextrailchangesrow).
- **Branch row**: [ContextRailBranchRow](#contextrailbranchrow).
- **Hairline divider**.
- **Sources section**: `Sources` label + "No sources yet" placeholder.

Each numeric cell animates scoreboard-style (`counter_animated`): a fresh `gen` is appended to the animation id on every value delta, so gpui fires a 600ms `ease_out_quint` tween from the previous rendered value to the new one. `env_counter_state: HashMap<String, (u64, u64)>` lives on `ContextRail`, rebuilt every render inside `render_usage_section` to auto-prune cells whose model disappeared.

> Source: `crates/manox-agent-chat-ui/src/views/context_rail.rs` (`render_panel`)

#### ContextRailCollapseBtn

Ghost `xsmall` button in the panel header, `IconName::PanelRightClose`, tooltip i18n `context-rail-collapse`. Folds the rail into a drawer when narrow (the drawer's open affordance uses `context-rail-drawer-open` / `context-rail-expand`).

> Source: `crates/manox-agent-chat-ui/src/views/context_rail.rs`

#### ContextRailChangesRow

Working-tree diff stat line in the panel body. `env_row` with `Frame` icon, "Changes" label, and a trailing `+added` (green) / `-deleted` (red) / `?untracked` (muted) cluster from `GitChangeStats`. Before the first git refresh lands (or when no project is bound) the trailing slot shows `--` / "No project" so the row keeps its height instead of flickering.

Stats come from `git diff --numstat HEAD` (binary rows `-`/`-` skipped) plus `git ls-files --others --exclude-standard` for untracked, shelled out via [`crate::git_status`](#git_status) on the global tokio runtime. Refreshed (debounced 400ms) by `Workspace` on thread attach and terminal `Stop`.

> Source: `crates/manox-agent-chat-ui/src/views/context_rail.rs` (`render_changes_row`)

#### ContextRailBranchRow

Resolved git identity block in the panel body (`render_branch_block`). When the session's effective cwd differs from the launch directory (a worktree entered through a per-call `cwd`), a leading directory-name row precedes the branch row; both rows share the same `h_flex` (icon + label) layout, `text_sm` font, and `gap_2` spacing so they read as peer rows.

- **Working-directory row** (rendered only while the effective cwd is reported): lucide `workflow` icon (resolved via [assets](#assets) at `icons/workflow.svg`) + the directory basename as the label. Non-interactive — no trailing, no cursor, no menu.
- **Branch row**: `env_row_clickable` with lucide `git-branch` icon (`icons/git-branch.svg`) — the whole row is a pointer cursor that opens [ContextRailBranchMenu](#contextrailbranchmenu). The label shows:
  - The branch name when on a normal branch.
  - The short sha + "(detached)" hint when in detached HEAD.
  - "Not a git repo" when `git rev-parse --show-toplevel` fails.
  - "git unavailable" when the `git` binary is missing.
  - "--" before the first refresh lands; "No project" when no project is bound.

Both glyphs live in manox's local asset bundle (`ExtrasAssetSource` in `crates/agent-ui/src/assets.rs`), not `gpui-kit-assets` — `IconName` is generated at compile time from the latter's directory and cannot reference them, so the rows construct `Icon::default().path("icons/…")` instead of `Icon::new(IconName::…)`. Branch resolution shells out to `git branch --show-current`, falling back to `git rev-parse --short HEAD` for detached HEAD. All via [`crate::git_status`](#git_status).

> Source: `crates/manox-agent-chat-ui/src/views/context_rail.rs` (`render_branch_block`)

#### ContextRailBranchMenu

`PopupMenu` anchored under the branch row, rendered as a `deferred(...).with_priority(1)` overlay so it paints on top of the entire workspace tree and is never occluded by the rail's later-painted siblings (usage/budget/plan rows) nor clipped by the rail's scroll container. Mirrors the title-menu / model-selector pattern: the menu entity + its `DismissEvent` subscription are created lazily on open, dropped on close. Items:

- **Copy branch name** (i18n `workspace-env-git-copy-branch`) — shown when a branch resolved; writes to the clipboard silently.
- **Copy working-directory path** (i18n `workspace-env-git-copy-path`) — shown when an effective cwd is reported.

> Source: `crates/manox-agent-chat-ui/src/views/context_rail.rs` (`render_branch_row`)

#### git_status

Pure parsing + tokio-bridged IO module backing [ContextRailChangesRow](#contextrailchangesrow) / [ContextRailBranchRow](#contextrailbranchrow). Shells out to the system `git` binary (never `git2` — banned by project rule) on the global tokio runtime via `manox_agent::runtime::handle`, delivering results back through an `async_channel`.

- `parse_numstat` / `parse_branch` / `parse_short_sha` / `count_untracked` — pure value-type parsers (unit-tested without a real repo).
- `gather` — runs `git rev-parse --show-toplevel`, `git branch --show-current` / `git rev-parse --short HEAD`, `git diff --numstat HEAD`, `git ls-files --others --exclude-standard` in one background task; returns `None` when the cwd is not under git.
- `gather_bridged` — spawns `gather` on the tokio runtime and awaits the result from a gpui `cx.spawn`.

> Source: `crates/manox-agent-chat-ui/src/git_status.rs`

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
由 [ContextRail](#contextrail) agents 段的子代理行、或会话里的子代理卡片点击打开
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

应用壳根视图（`crates/manox-agent-chrome-ui/src/shell.rs`）：垂直布局 = 38px 工具栏（原生交通灯槽位 70px、侧栏开关、session 下拉选择器、VS 徽标、Sync 胶囊、面板/右栏开关、头像）+ 内容区（侧栏｜主区卡［主槽｜右栏］／底部 dock）。主区卡圆角 8px、卡缝 6px；两条调宽把手为隐形 absolute 层（挂在根做绝对坐标数学，载荷类型左右各一）。`ShellConfig` 注入主槽（`MainSurface`）、右栏 kind 注册表、dock surface、侧栏固定行/自定义行与 `HostHooks`；会话行由宿主推送快照（`set_sessions`），壳自身只持交互态（选中、分组折叠、分组拖排、下拉/行菜单）。

> Source: `crates/manox-agent-chrome-ui/src/shell.rs`, `crates/manox-agent-chrome-ui/src/titlebar.rs`, `crates/manox-agent-chrome-ui/src/divider.rs`

#### ChromeSessionList

侧栏会话树（props 驱动，`crates/manox-agent-chrome-ui/src/session_list.rs`）：**三行 66px 行卡（2026-09-29 thread-item 设计稿）**——标题行（16px 状态槽 + 6px 间距 + 标题）、tag 行（短 id chip 恒首位 + 用户 tag chip）、info 行（仅最后活跃时间：72h 内相对、之外本地 `MM-DD HH:MM`），三行共用一条左基线、无任何右对齐内容、**不随项目层级缩进**（层级只由分组头与 leader chevron 表达）。行面上**零控件**：pin/archive/标签/复制 ID 全部收进右键菜单（`Shell::open_row_menu` 五项：置顶 toggle／归档 toggle／添加·重命名标签／移除标签／复制 ID；tag 内联编辑挂在 tag 行，Escape 取消、Enter/blur 提交、空值丢弃、10 字上限；双击用户 tag 芯片 = 老壳同款进入重命名编辑，短 id 芯片单击复制完整 id）。五态字形（`Errored` 红三角／`PendingAuth`·`PendingPlan` 实心 8px 蓝点／`Running` 像素积木 2×3 点阵 1820ms 阶梯循环（VS Code pixelSpinner grid 变体移植）／`Unread` 空心 6.5px 蓝点／`Idle` 空槽）。四态表面：未选中无背景、悬浮 `LIST_HOVER` + 标题转 500 字重 + **截断标题跑马灯**（双份标题 + 24px 间隔的无缝循环轨道：24px/s、每循环停 600ms、回绕点像素级相同无闪跳；仅 `is_hovered && title_truncated` 启动，移开复位；截断判定 = 与渲染器省略号同一套 `shape_text` 实测宽 vs `on_prepaint` 逐帧记录的剪裁盒宽）、选中白卡 + 15% 描边、键盘焦点 = `track_focus` + `focus_visible` 1.5px accent 环（↑/↓ 在可见行间移动焦点，行高四态一致不 reflow）；分组头可折叠并作为拖拽源/放置目标（2px accent 插入线）。

> Source: `crates/manox-agent-chrome-ui/src/session_list.rs`, `crates/manox-agent-chrome-ui/src/shell.rs`

#### SidebarProjection

wire 行 → chrome 侧栏 props 的**纯投影**（`crates/agent-ui/src/sidebar_projection.rs`）：`ThreadListItem`（multiplexer 权威行，含 §D.5 增量合并）→ 五态优先级（errored > pending_auth > pending_plan > running > unread，叶子 unread 镜像覆盖 wire 标志）＋ team 森林（leader 保序、member 随后、孤儿拍平；**member 不再缩进**，leader chevron 是唯一嵌套标记）＋ `updated_at`/`archived`/tag/pinned 原样透传 ＋ 按项目路径尾段分组。装配层在 multiplexer notify 时喂给 `ChromeSessionList`。

> Source: `crates/agent-ui/src/sidebar_projection.rs`

#### ChromeRightPane

右栏外壳（`crates/manox-agent-chrome-ui/src/right_pane.rs`）：圆角卡 + 页签条（**下划线式页签**：平面标签压在条带自身的 `border_b_1` 共享轨道上，激活项为 `ACCENT` + 半粗；一条**共享的下划线指示器**按激活 id 播放滑动动画，从旧页签横移到新页签）+ 新标签页空态（快捷操作由注册表生成）+ 打开/激活/关闭生命周期（最后一个页签关闭即收起）。

页签几何由 `on_prepaint` 实测上报（`TabBounds`，键为页签 id），指示器据此定位——标签宽度不一，无法由序号推出。注意 `on_prepaint` 上报的是**内容盒原点**（它挂的是 `canvas().absolute().size_full()` 子元素，padding 已计入），故记录时减去 `TAB_PL` 还原页签左边界；指示器与条带是**兄弟**（同在 relative wrapper 内）而非父子——gpui 的 `Style::paint` 先画子元素、**后画自身 border**，所以子元素永远压不住条带的 `border_b_1`，会只剩半截可见。wrapper 即指示器的包含块，其原点也就是 tab 几何的反基准坐标系。内容经 `ToolTab` 注入、kind 经 `ToolTabFactory` 注册；**实例级 id**（一种 kind 可多开）。**per-thread 会话**：`RightPaneSession{open, store, active_id, visible}` 整体 stash/restore（挂起走 `on_active(false)`——浏览器子视图隐藏、终端保活；仅显式关页签才拆内容）。快照经 `ToolTab::persist` / `ToolTabFactory::restore`（浏览器 `{"url"}`、编辑器空稿可恢复；终端与 CLI 会话不可复活，恢复时丢弃）落 `threads.db` 的 `thread_right_pane`。

> Source: `crates/manox-agent-chrome-ui/src/right_pane.rs`, `crates/agent-ui/src/chrome_assembly.rs`

#### ToolTabRegistry

chrome 壳右栏的 kind 全集（`crates/agent-ui/src/tool_tabs.rs`，快捷操作顺序）：**终端**（$SHELL，独立 PTY，关页签拆进程树）、**Claude Code / Codex / GitHub Copilot**（页签体先落模型选择器——复用共享级联投影 `cascade_provider_groups`；点选即以该端点 `AgentBuilder` 拉起 CLI，picker 实体此后自渲染 TUI；cwd = 前台线程项目目录）、**编辑器**（markdown 软换行 + 行号）、**浏览器**（真 `BrowserView`：地址栏 + 导航；走生产 `restore_browser_tab`/`close_browser_tab`，注册进进程级 `WorkspaceBrowserHost`——IPC notify/inbound、eval oneshot、yield 全通；2s ticker 把页面 `<title>` 镜像到页签标签，`on_active` 隐藏 OS 子视图防漂浮）。

> Source: `crates/agent-ui/src/tool_tabs.rs`

#### ChromePanel

底部 dock（`crates/manox-agent-chrome-ui/src/panel.rs` + `shell.rs` 的渲染）：通用容器，内容经 `PanelSurface` 注入（`open`=展开即拉起、`close`=收起即回收；新建/清理/收起三个动作）。manox 装配装集成终端，cwd 随前台线程；dock 视图按线程 stash/restore（同右栏语义）。

> Source: `crates/manox-agent-chrome-ui/src/panel.rs`, `crates/agent-ui/src/chrome_assembly.rs`

#### ConversationColumn

`Workspace::render_column`：会话列本身（hero 空屏／虚拟化消息列表／composer footer＋附件与 chips／ask 与 blank-project overlay／浮动 ContextRail／TurnNavigator overlay），四周的 gutter / 侧栏槽 / 卡壳 / 内嵌标题栏全部归壳；键盘动作面经根装饰器 `apply_chat_actions`。装配把该视图作为壳的 `MainSurface`，其 multiplexer 同时喂侧栏投影泵。

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
