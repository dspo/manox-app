# UI Map

Shared vocabulary for every named UI component in steer. When discussing UI, reference
component names from this file so both parties refer to the same thing.

组件名以 crate 归属区分：壳与壳内组件在 `steer-agent-chrome-ui`（`Chrome` 前缀或
crate 路径），会话列与其状态机在 `steer-agent-chat-ui` / `agent-ui`。

Component names use PascalCase. The hierarchy mirrors the visual containment tree.

---

## 0. 架构（2026-09-28 起：唯一壳）

窗口只有一个壳，无构建开关：

```sh
cargo run                       # 桌面应用（chrome 壳，唯一）
```

| 层 | 组件 |
| --- | --- |
| 根视图 | `steer_agent_chrome_ui::Shell`（壳：38px 工具栏 + 侧栏 + 主区卡 + 右栏 + 底部 dock） |
| 侧栏 | `chrome::SessionList`（props 组件）+ `agent_ui::sidebar_projection`（wire 行 → props 的纯投影） |
| 主区卡内容 | `Workspace`（`agent-ui`）的会话列 —— 挂进 chrome 的 `MainSurface` 槽（日志/契约见 §3） |
| 右栏 | `chrome::RightPane` + `ToolTab` 实例页签；`agent_ui::tool_tabs::registry` 提供 kind（终端 / CLI agents / 编辑器 / 浏览器 / diff）+ 装配层宿主页签（子代理面板、宿主打开的浏览器页签）；per-thread 会话由 `RightPaneSession` stash/restore，快照落 threads.db |
| 底部 dock | `chrome::panel` + `PanelSurface` 注入（steer 装终端；随前台线程 cwd） |
| Settings | 主区卡内换页：上游 `gpui_component::setting::Settings` 壳（`Workspace::render_settings_card`） |

共享状态层不变：`agent-ui`（multiplexer / client_store / browser_host / dispatch / 侧栏投影 /
slash_command / Settings 视图）、`steer-agent-chat-ui`（聊天状态机与消息管线）、`terminal-ui`、
`steer-webview`。

壳的契约（均可在不改 agent-ui 的前提下扩展）：`MainSurface`（主栏槽）、`ToolTab` /
`ToolTabFactory`（右栏 kind）、`PanelSurface`（dock 内容）、`HostHooks`（pin/archive/new/select
回调）。装配层另有两条进程级通路：`chrome_assembly::{open_tool_tab, subagent_panel}` ——
宿主（agent 的浏览器打开、会话列/rail 上的子代理点击）经它把页签落到活着的右上栏。
依赖不变量：chat crate 不得依赖 terminal-ui/steer-webview/steer-ext-agents
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
| 权限门控（AccessMode / ToolCallAuthorization / AskUserQuestion） | ✅ | 文件效果策略：bash 两档都运行（sandbox 档 ride seatbelt profile），fs 写经 `writable_roots` containment + `[sandbox: …]` marker；`require_escalated`+`justification` 升级往返经 `ToolCallAuthorization`；AccessSwitch（composer 底栏内联 segmented TabBar）切 Sandbox/FullAccess，切换热生效（下一次工具调用即按新模式裁决），开关乐观即时刷新、宿主拒绝时回滚并出纠错 notice（写前对账，`LeafView::access_mode_or_default`：座位值 → 宿主声明 schema 默认 → 宿主默认，旧三档持久串一律落默认 Sandbox）；两档网络恒开——无用户面网络开关，app 在每次 attach 播种 `networkAccess=on`（AHP 会话内存态，恢复即关故须重播种；沙箱出网无 allowlist 直连、有 allowlist 走受管代理 + 域名确认卡） |
| Slash commands | ✅ 部分 | `/compact`、`/exit`(`/quit`)、`/new`(`/clear` `/archive`)、`/goal`、`/mode`（`/plan` 是上游内置 skill：裸文本直通 runtime 的 skill 分发，非本地命令）；markdown/skill 适配器经共享 registry。`/exit` 走全量停机序列（`workspace/exit.rs`）：停子代理 → cancel 当前 turn → 关右栏终端/TUI agent 页签与底 dock 终端（PTY drop 即进程树终止）→ 关集成浏览器 → 强制归档（绕过 idle 门禁）→ 落新会话，每步以 info 卡播报；turn 运行中同样可发 |
| 模型选择器 | ✅ | pi `ProviderRegistry`，按 provider 显示名分组；写前对账：乐观写入 → 宿主 echo 确认（撤销史销账）/ `rejectionReason` 回滚 + 纠错 notice |
| 项目（composer chip / 侧栏文件夹 / 绑定新会话） | ✅ | 宿主 workspace registry（`ClientCall::Workspace` + `HostEvent::WorkspaceUpdate` 状态流），客户端不再写 threads.db `projects` 表 |
| Sidebar / 会话列表 / 新建切换归档 / LLM 标题 | ✅ | pi `SessionRepository` + sidecar；标题双模式语义移植自 manox |
| ConversationInfoBubble（composer 圆圈上方：Captain/subagents/branch 对/todos/per-model 用量/sources） | ✅ | 用量与圆圈同口径（最近请求输入侧）；per-model 成本行与 ±change 计数随卡片退役 |
| `/compact` + Recap 卡片 | ✅ | 内核 `HarnessEvent` compaction 事件（manual/threshold/overflow） |
| 后台线程（ctrl-b 置底 / 切换自动 park） | ✅ | `background_threads` + `attach_thread` parking |
| CLI agent 终端（Claude Code / Codex / GitHub Copilot） | ✅ | 右栏 ToolTab：先落 provider→model 级联（共享投影 `cascade_provider_groups`），选中即以该端点 `AgentBuilder` 拉起 CLI（PTY relay → `TerminalView`；cwd = 前台线程项目目录） |
| 集成终端 / VS Code 注入 / ChatGPT.app 注入 | ✅ | 右栏终端页签（$SHELL，独立 PTY）+ 工具菜单的 VS Code / ChatGPT.app 注入启动（后台线程 + 通知，应用独立存活） |
| Browser 标签 / Terminal 标签 | ✅ | webview host notify/inbound 桥；平台 terminal surface |
| TurnNavigator | ✅ | cmd-m 打开；↑/↓ 选条、enter 定位、⌘↵ 回填 composer、⌘C 复制；历史回溯另有 ⌥↑/⌥↓ |
| TurnRail（左缘轮次导航） | ✅ | 常驻左缘刻度 rail（dsh TurnNavigator 移植，≥2 轮且卡宽 ≥640px 显示）；悬停预览、点击定位、active 随滚动追踪 |
| 图片附件 | ✅ | 剪贴板粘贴 → chip → 气泡渲染 → 内核 `ContentBlock::Image` 投递（TS `prompt(text, {images})` parity，#438）；steer 带图同路 |
| MCP | ✅ | 连接核心共享化 + pi AgentTool 桥（#442）；Settings → MCP servers 面板（列表/连接状态/持久开关） |
| skill / subagent @mentions | ✅ | 共享层 registry（#440）：markdown 斜杆命令 + skill mentions（submit_command/submit_skill）；subagent 定义经 agent_defs 注册（#471） |
| Sub-agent 观察 | ✅ | rail agents 段直读 `background_work`（运行中花名册）；Agent 工具卡片携带 run_id 可点击；右栏 SubagentPanel 钻取子会话（同管线渲染、live、相位三态、只读；头部 access-mode 徽章 = dspo/manox#968 的 app 伴随，读子会话自己 session 通道的声明行）；Captain 条的子代理行在主列打开 SubagentStream 观察页——子会话自己的通道即 live 源，同一 keyed diff 投影（含思考段与工具卡）、只读、无轮询 |
| 退役（plan mode 整体删除，随上游 manox PR #946） | — | `/plan` 为上游内置 skill（规划契约一次性注入，见 §Slash 行）；plan 裁决 = 单问题 ask（`detail`=plan 正文，经 wire 的 `ChatInputRequest.message` 折入），批准/修订即普通答案；rail 的 plan 节（`UpdatePlan`）仍消费执行进度，快照经 sidecar 持久化、compaction 后可恢复 |
| Goal | ✅ 部分 | facade+GoalBridge 共享快照、GetGoal/CreateGoal/UpdateGoal 工具、`/goal` 命令、composer chip+状态 popover（attach/reclaim 时按 projection 重新武装 elapsed ticker）；per-turn 记账/自动续跑/BudgetLimited 强制为后续项 |
| Team | ✅ 部分 | 成员经 `Steer(spawn="TeamMember")` 创建为真实 thread（sidebar 可见、可恢复）；同伴消息经 Steer Inject 路由。旧 roster 容器 `Entity<Team>`、MemberPanel 空壳、composer team chip、sidebar role badge、`TeamDismiss/TeamStatus` 等 roster 工具与授权冒泡已完全退役删除（见 #625）；member→parent 自主汇报未接线（Abort 仅 cancel 当前轮，dismiss/archive 无替代品） |
分层纪律：crates/manox-harness/src/core 只做 TS Pi 对齐与扩展点；harness 能力扩展一律走
crates/manox-harness/src/ext；宿主（manox-agent / agent-ui）只做装配与 UI。

---

## 索引

### 壳（Shell）

- [ChromeShell](#chromeshell) · [ChromeSessionList](#chromesessionlist) · [ProjectGroupMenu](#projectgroupmenu) · [SidebarProjection](#sidebarprojection) · [ChromeRightPane](#chromerightpane) · [ToolTabRegistry](#tooltabregistry) · [ChromePanel](#chromepanel) · [ConversationColumn](#conversationcolumn) · [SettingsCard](#settingscard)

### 顶层

- [Window](#window) · [NativeMenuBar](#nativemenubar) · [AboutWindow](#aboutwindow) · [Workspace](#workspace) · [ViewMode](#viewmode) · [ViewMode::Workspace](#viewmodeworkspace) · [ViewMode::Settings](#viewmodesettings) · [ViewMode::ExternalSession](#viewmodeexternalsession) · [ViewMode::SubagentStream](#viewmodesubagentstream)

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

- [UserMessage](#usermessage) · [AssistantMessage](#assistantmessage) · [AssistantActions](#assistantactions) · [ReasoningBlock](#reasoningblock) · [ActivitySegment](#activitysegment) · [ModelTag](#modeltag) · [ToolCallCard](#toolcallcard) · [PreviewCard](#previewcard) · [AgentTaskCard](#agenttaskcard) · [BackgroundTaskCard](#backgroundtaskcard) · [ErrorMessage](#errormessage) · [NoticeMessage](#noticemessage) · [RecapCard](#recapcard) · [CacheMissDivider](#cachemissdivider) · [RetryBadge](#retrybadge)

### Footer / Composer

- [Footer](#footer) · [CaptainStatusStrip](#captainstatusstrip) · [Composer](#composer) · [DeliveryErrorStrip](#deliveryerrorstrip) · [QueuedFollowUps](#queuedfollowups) · [ComposerDivider](#composerdivider) · [AttachmentChips](#attachmentchips) · [AttachmentChip](#attachmentchip) · [BrowserSuiteChip](#browsersuitechip) · [ComposerInputRow](#composerinputrow) · [InputField](#inputfield) · [SendCluster](#sendcluster) · [ModelChip](#modelchip) · [ContextUsagePill](#contextusagepill) · [DictationButton](#dictationbutton) · [AccessSwitch](#accessswitch) · [ProjectChip](#projectchip) · [FollowStopProjection](#followstapprojection)

### AskDrawer

- [AskDrawer](#askdrawer) · [ConfirmationCard](#confirmationcard)

### Popups & Dropdowns

- [CompletionPopover](#completionpopover) · [ModelMenu](#modelmenu) · [ProjectMenu](#projectmenu)

### Overlays

- [BlankProjectOverlay](#blankprojectoverlay)

### 右栏页签内容

- [SubagentPanel](#subagentpanel) · [SubagentStream](#subagentstream) · [BrowserView](#browserview)

### ManagementShell

- [ManagementBackControl](#managementbackcontrol)

### Settings

- [SettingsView](#settingsview) · [SettingsTitleBar](#settingstitlebar) · [SettingsShell](#settingsshell) · [SettingsModelsPanel](#settingsmodelspanel) · [SettingsChatGptAppPanel](#settingschatgptapppanel) · [SettingsSectionCard](#settingssectioncard) · [SettingsRow](#settingsrow) · [SettingsSectionHeader](#settingssectionheader) · [SettingsHairline](#settingshairline)

### Terminal

- [TerminalView](#terminalview) · [TerminalTabBar](#terminaltabbar) · [TerminalGrid](#terminalgrid)

### Shared Primitives

- [Button](#shared-primitives) · [Input](#shared-primitives) · [PopupMenu](#shared-primitives) · [PopupMenuItem](#shared-primitives) · [TabBar](#shared-primitives) · [Tag](#shared-primitives) · [Markdown](#markdown) · [TerminalPanel](#terminalpanel) · [TurnFrame](#turnframe) · [Icon](#shared-primitives) · [BrailleSpinner](#braillespinner) · [PixelSpinner](#pixelspinner) · [Meerkat](#meerkat) · [Tool](#tool) · [CodeView](#codeview) · [ShellView](#shellview) · [AskUserQuestion](#askuserquestion) · [ScrollHandle](#shared-primitives) · [TitleBar](#shared-primitives) · [ContextMenu](#shared-primitives) · [Tooltip](#shared-primitives)

### 状态

- [AccessMode](#access-modes) · [ToolCallStatus](#tool-call-statuses)

---

## 1. Window

#### Window

Top-level native window, title "Steer", min 900×600.

> Source: `crates/steer-app/src/main.rs`

#### NativeMenuBar

macOS menu bar built by `build_app_menus()`: `Steer` (About/Settings…/Quit), `Terminal` (new/close tab), and `工具` (Tools) with two app cascades. `ChatGPT.app` → provider → model: models mirror the provider registry snapshot filtered by `visible_agents()` containing `ChatGPT.app` (Responses-capable models), grouped by provider; picking a model dispatches `LaunchChatGptApp { provider, model }`, routed through the App-level action handler to `Workspace::launch_chatgpt_app`, which starts ChatGPT.app via cx's injection path on a background thread (selected model = default; the provider's full Responses catalog is injected). `VS Code` → provider → model: models filtered by `visible_agents()` containing `VS Code` (Anthropic-wire models); picking a model dispatches `LaunchVSCode { provider, model }` → `Workspace::launch_vscode_app` → `cx::launch_vscode_app`, which resolves the login-shell env, overlays Claude Code BYOK env (`ANTHROPIC_BASE_URL`/`ANTHROPIC_API_KEY`/`ANTHROPIC_MODEL` + provider/model env) at highest priority, and launches VS Code with `VSCODE_CLI=1` so the extension host, the Claude Code extension's bundled CLI, and integrated terminals inherit the injected env (a running VS Code is restarted after user confirmation; no settings.json writes, API key never persisted). A trailing 「打开」item dispatches `LaunchVSCodePlain` → `cx::launch_vscode_plain` (plain `open -a`). The VS Code submenu is disabled when VS Code is not installed. Text-only — gpui native menu items carry no images. Rebuilt by `i18n::rebuild_menus` on UI-language change, after a provider-registry reload, and once when the initial background provider registration lands.

> Source: `crates/steer-app/src/main.rs`

#### AboutWindow

Centered, non-resizable floating dialog (440×440, `WINDOW_WIDTH` / `WINDOW_HEIGHT` in `about.rs`) opened by the `OpenAbout` action from the native menu bar / tray path, single-instance (an existing About window is activated instead of a second one). Two direct children of the root (`about-window`, `p_4`, `justify_between`): `details` (`about-details`) — the app icon, the headline `Steer.app v<version>+<short commit>` (this repository's commit, abbreviated; a build without git metadata stays on the bare version), and the provenance **axis** (`about-axis`): one continuous vertical rule with every label/value row hung on it — the label column right-aligns onto the rule, the value column left-aligns off it (both columns re-pin their text alignment against the dialog root's centered text). The rows, in display order: `OS` (platform + the chip's marketing name via `sysctlbyname`, e.g. `macOS · Apple M2 Max`; arch fallback where the platform has no brand string), `rustc` (the compiling toolchain's self-report, captured by `build.rs` from the rustc cargo drives), `manox runtime` (the dspo/manox runtime short commit), `gpui-component` and `gpui-pre` (the lockfile pins, the bare version literal or `owner/repo @ rev` when resolved from git); a row whose value the build could not resolve is absent. The other child is `about-buttons` (`about-buttons`) with the OK / Copy pair (`about-ok` closes, `about-copy` writes the clipboard block and closes). Escape closes the window. Rows come from `provenance_rows()`; the clipboard block is the runtime's `version::structured_about()`, then the full app commit (`steer-app commit: …`, the headline only abbreviates it), then one `label: value` line per row.

> Source: `crates/steer-app/src/about.rs`, `crates/steer-app/src/pins.rs`, `crates/steer-app/build.rs`

#### SystemTray

Process-lifetime system tray installed right after the first main window opens (`tray::install` — ordered after window creation because the status item creates its own `NSStatusBarWindow`, which must not become a startup death mode when window-server resources are exhausted), the lifeline for reaching steer while no window exists. Backends: macOS/Windows use `tray-icon` (native status item + menu; both platforms pump the tray's messages on the gpui main thread), Linux uses `ksni` (StatusNotifierItem over D-Bus on its own thread, no GTK involvement). Menu items: 「打开 Steer」(`menu-open-steer`) and 「退出」(`menu-quit`), labels re-resolved through the `i18n::rebuild_menus` path on UI-language change. Windows additionally opens/focuses the window on left icon click (right click pops the menu); macOS pops the menu on icon click. Event bridge: gpui exposes no cross-thread wake, so a foreground task polls every 100ms and drains the backend's event channels into `TrayCmd::Open` / `TrayCmd::Quit`. With a tray, the app runs under `QuitMode::Explicit`: closing the main window parks the process instead of quitting it — the `Workspace` entity stashed in `agent_ui::dispatch` is process-lifetime, so the foreground thread and any parked background threads keep running through the close; 「打开 Steer」(or the macOS dock icon, via `on_reopen`) re-opens the window over that same workspace, restoring conversation, drafts, and thread list. Tray install failure keeps the platform default (quit-on-last-window-close off macOS) so the app never strands invisibly.

> Source: `crates/steer-app/src/tray.rs`, `crates/steer-app/src/main.rs`

#### DockBadge

The count on the macOS Dock icon (`NSDockTile.setBadgeLabel` via `objc2-app-kit`), mirroring the sessions currently owing the user attention: a parked ask / tool-approval card (`pending_auth`/`pending_ask`), an errored mark, or an unseen settle on a non-focused thread (the client-owned unread mirror). One point per non-archived thread row; the aggregate lives on `Workspace::attention_count` → `SessionMultiplexer::attention_count`. Clear paths mirror the sidebar's own semantics: the interaction flags retire when answered, unread retires on focus, and the leaf's live unread mirror wins over the row flag — so on live rows the badge and the sidebar cannot disagree. Two deliberate divergences: archived rows are dismissed by the badge while the sidebar still paints their attention marks, and `errored` follows the §D.5 mirror rule (only the next list snapshot clears it, like the sidebar's danger triangle — no user gesture retires that point). Not fed from state edges — local unread raises only notify the leaf entity — so a foreground task polls every 500ms (the tray pump's pattern) and only crosses into AppKit when the count changed; counts above 99 render as `99+`. Windows (taskbar overlay icon) and Linux have no backend yet: they keep a clean no-op behind the same API. The pump starts with the tray install block but runs regardless of it.

> Source: `crates/steer-app/src/badge.rs`, `crates/steer-app/src/main.rs`, `crates/agent-ui/src/multiplexer.rs`

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

`Workspace` has four modes:

#### ViewMode::Workspace
默认 —— 会话列（hero 或消息列表 + composer）。

#### ViewMode::Settings
主区卡内的设置页（nav｜panel 两栏，替换会话列，直到 back 控件退出），带 200ms 退出延迟。

#### ViewMode::ExternalSession
项目菜单拉起的 CLI agent TUI / 普通终端顶替会话列占主列（PTY 属于 Workspace，切走只停靠、保活）。

#### ViewMode::SubagentStream
一个运行中子代理的自家 transcript 顶替会话列（[CaptainStatusStrip](#captainstatusstrip) 的子代理行点开；
见 [SubagentStream](#subagentstream)）。子代理的 journal 由宿主 bridge 进它自己的 chat 通道，打开即一次
订阅：折叠历史与 live delta 走同一通道，页面用自己的 store observe 驱动 keyed diff 重投影——没有轮询腿；
任何离开它的 mode 翻转（点侧栏行 / 新建会话 / 进设置 / **外部会话抢走主列**——项目菜单拉起或侧栏 external 行点击）
都 drop 该页（页面自带 observer，drop 即断跟随）；不变量是「页面存在 ⟺ `view_mode == SubagentStream`」
（与 `active_external` 同一条规矩）。

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

> Source: `crates/agent-ui/src/workspace/render.rs` (`render_follow_stop_banner`); state: `crates/steer-agent-chat-ui/src/client_store_handle.rs` (`FollowStop` / `FollowStopReason`)

#### FollowStopProjection

Permanent minimal projection of the stopped-follow state: a compact danger chip in the composer's footer chip group (tail of the left cluster, beside the send control — where the user reaches to resend/retry; a global overlay was rejected: the status belongs beside the recovery action). Shown while the foreground leaf's `follow_stop()` is `Some` — it deliberately does NOT read `dismissed`: the state outlives the broadcast's dismissal, so the entry never disappears on its own (only a good snapshot or a live retry retires it). The chip IS the retry entry: clicking fires the same leaf `retry_follow` the banner's Retry button wires (no-op with no multiplexer — the entry survives its own dead click), and its tooltip previews that consequence. Visible copy is a per-reason Fluent key (`FollowStopReason::indicator_key()` — today `follow-stop-indicator-stream-failing`), so a new reason adds a variant + key, never edited prose. Geometry: `flex_shrink_0` at the group tail; arrival/departure can never squeeze or shift the pinned model/send controls.

> Source: `crates/agent-ui/src/workspace/composer_render.rs` (`render_follow_stop_chip`); state: `crates/steer-agent-chat-ui/src/client_store_handle.rs` (`FollowStop` / `FollowStopReason`)


#### 3.2.1 Hero

Shown when the thread has no substantive messages (and is not loading).

#### Hero

Vertically centered welcome area: logo/heading + inline [Composer](#composer).

> Source: `crates/agent-ui/src/workspace/render.rs`

#### HistoryLoading

Full-column pixel loading page that suppresses the hero, message list, and footer while a reopened thread's chat snapshot is still in flight: `ChatColumn.awaiting_history` is set at reopen attach when the caller declares it expects history (`expect_history`, not the wire-side `reopen` flag — a created session re-opens too) and the fold holds no chat channel. Cleared by the snapshot rebuild (history present), by the store observe (genuinely empty session → hero returns), or by the render-time timeout (`HISTORY_TIMEOUT`, 10s) so a failed reopen cannot pin the page. Render re-checks the fold, so a stale flag cannot pin it either. Layout: a 12×14-cell pixel meerkat sprite played as a 6-slot loop at 3 slots/sec (4 distinct frames: idle, bob, blink, tail flick — each cell a flat solid block, no bevel), centered above the heading (`workspace-history-loading-heading`) and the monospace thread id. No composer while it shows. The sprite, its grids and its clock live in the shared [`Meerkat`](#meerkat) component (`ai_elements::Meerkat`); this page is the heading, the thread id, and the timeout constant.

> Source: `crates/agent-ui/src/views/history_loading.rs` (page); sprite: `crates/ai-elements/src/meerkat.rs`. Gate: `crates/agent-ui/src/workspace/attach.rs` (`attach_thread`), `crates/agent-ui/src/workspace/render.rs` (`render_column`), `crates/steer-agent-chat-ui/src/column.rs` (`awaiting_history`)

#### 3.2.2 MessageArea

Shown when the thread has messages. Replaces [Hero](#hero).


#### MessageArea

Wraps [MessageList](#messagelist).

> Source: `crates/agent-ui/src/workspace/render.rs`

#### MessageList

Virtual list backed by native `gpui::list` (`gpui::list(list_state, render_item)`, `ListState` held directly on `Workspace`), wrapped in gpui-base `ScrollBounce` — the wrapper owns the vertical wheel and precise scrolling bounces with NSScrollView-tuned motion on macOS; its `on_scroll` notifies the chat because the bounce scrolls the ListState past the list's own scroll handler. GPUI owns virtualization, scroll, the per-item height cache, and tail-follow; `ListAlignment::Bottom` gives native chat-log semantics — short histories sit at the viewport bottom, long ones scroll — and `FollowMode::Tail` pins to the live end on each layout while following (disengaging on upward scroll, re-arming at the bottom). The row factory captures `Conversation` directly and is strictly read-only during list measurement/prepaint; Workspace-derived ask-card snapshots are synchronized before list construction. `MSG_LIST_OVERDRAW` pre-measures rows below the viewport. Visible rows re-measure every frame, but the pinned official GPUI revision retains off-screen row heights across width changes, so `MessageListWidthInvalidator` observes the final positive list width after layout, invalidates the complete cache with `remeasure_items`, and requests a settling frame while preserving the logical item/offset anchor. Count changes are reconciled via `splice` and in-place mutations via `remeasure_items`, both driven from the projection pass's `ProjectionChange` (count delta, per-index remeasures, a structural flag for mid-list moves). There is exactly ONE transcript path: `transcript ≡ project(&ChatState, &Overlays)` (`crates/steer-agent-chat-ui/src/transcript.rs`). Every trigger — the store observe on any fold change, a thread attach, an optimistic submit, a sub-agent panel's pump — funnels into `Workspace::sync_conversation`, which stages the projected nodes against the foreground session's fold and reconciles them by stable row key (`user:{turn}` / `md:{part}` / `seg:{first entry}` / `tc:{call}` / `ir:{request}` / `notice:{…}`; the part ids the manox projection mints are byte-stable across replay). Live streaming and history are the same keyed diff at two trigger timings — a parked thread's stream lands in its own fold and reaches the bound transcript only when that thread is the one bound (`cross_session_stream_isolation` pins the routing; the switch-back convergence is pinned by `reopen_rebuild_watermark`). Only the visible items render. Markdown text rows render through upstream gpui-base `TextView` (issue #124): the renderer reports content-driven intrinsic sizes per measure and reconciles them with the final allocated width at paint, so row heights track the text engine's own shaping without app-side geometry.

> Source: `crates/agent-ui/src/workspace/render.rs` (`ListState` wiring, `MSG_LIST_OVERDRAW`), `crates/steer-components/src/markdown/text_view.rs` (TextView assembly, code-highlight dispatch)

#### TurnRail

Left-edge turn navigation: the dsh TurnNavigator mirrored onto the conversation column's leading edge. An absolute strip (`absolute().top_0().bottom_0().left(RAIL_LEFT_INSET=4).w(RAIL_WIDTH=28)`), vertically centered inside the message band's `h_flex` (mounted as a sibling painted after [MessageList](#messagelist), so it floats over the transcript but never over the composer — the band excludes the footer). One 2px tick per user turn at a fixed 10px pitch; from two turns up, and only when the card interior is at least `MIN_CARD_WIDTH` (640px) — independent of the [ContextRail](#contextrail)'s own gate, either side can float alone. While visible, the `GUTTER` (40px) left padding goes on the **list wrapper inside the band, never on the band itself**: the rail's absolute anchor is the band, so padding the band would drag the rail right along with the text it must clear (the ticks would sit on the transcript instead of hugging the edge — acceptance-round-1 regression).

Marks are re-derived from the conversation every frame — but only past the width gate, which short-circuits before the projection, and `render_turn_rail`'s `Some`/`None` is the single gate for both the rail and the gutter (`collect_rail_turns`: prompt = the user bubble's text collapsed and capped at 50 chars, or the ⌘M navigator's attachment-only / empty-message copy for a textless bubble — the same distinction `TurnEntry::new` draws; response = the turn's last non-empty assistant reply capped at 120 — dsh's `findLast` rule; both caps word-accumulate up to the budget (`split_whitespace` skips whitespace runs of any length, so indented blocks fill it like prose), so a huge turn costs O(limit), not O(全文)). The active mark is `active_rail_turn`: the last turn whose anchor item is at or above the list's `logical_scroll_top` item (the tail-follow floor reports `count`, resolving to the newest mark). Interactions: hover grows the tick 12→18px with a border→muted 140ms tween and the vacated mark sinks back in the same run — a pointer sweep reads as a wave down the ladder (dsh's CSS-transition semantics; each change keys one tween pair under `turn_rail_hover_gen`, with `turn_rail_hover_prev`/`_painted` snapshotting only on change; fast sweeps within the 140ms window truncate the wave's tail by design — the prev slot is single); hover opens a 300px preview card beside the rail (prompt line + response excerpt), fading in over 120ms with a 4px slide and traveling between marks over 140ms `ease_out_quint` (the from-top snapshots only when the hovered mark changes — the tab-indicator `indicator_from` discipline); click jumps through `Workspace::reveal_message` (the ⌘M navigator's own path). The active tick tweens width+color over 140ms on change (previous mark shrinks, new mark grows, keyed per generation; a tick that loses active while hovered hands the animation slot to the hover wave); active-follow scrolls the ladder (`scroll_to_item(Nearest)`) whenever the pointer is outside the strip (`turn_rail_pointer_inside` pauses it so marks never travel under the hand). An over-420px ladder scrolls inside the strip (`uniform_list` + `ListSizingBehavior::Infer` + `max_h`); the preview's geometry consumes the ladder's `ScrollHandle` offset **sign-corrected to positive-down** (gpui's raw offset runs negative scrolling down — the `-offset.y` convention `uniform_list` itself uses; round-1 C1). Thread re-projection (`attach_thread`, diagnostic replace) resets the interaction state via `ChatColumn::reset_turn_rail_interaction` — a stale hover index must not mount a preview on the new conversation. Tick rows follow the gpui hover-crossing rule: a row's leave retracts only its own mark.

> Source: `crates/steer-agent-chat-ui/src/views/turn_rail.rs` (rail + state contract), state fields on `ChatColumn` (`crates/steer-agent-chat-ui/src/column.rs`), mounted in `crates/agent-ui/src/workspace/render.rs` (`render_column`)

#### MessageItem

Single rendered conversation item, centered, full width (no fixed content cap — the transcript adapts to the window width). Each `MessageItem` renders one of the variant cards below based on `ConvItem` kind. Every kind that carries a text body — user (incl. peer deliveries), assistant, error, notice — mounts a persistent `Entity<TextViewState>` (`MessageItem::markdown`, created lazily by `ensure_markdown`) instead of rebuilding one per frame: a persistent body keeps content, selection and streaming motion across frames and leaves inline links clickable; static sub-bodies mount per-frame documents keyed off their element id.

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`

##### MessageItem variants

#### UserMessage

Full-width user turn block rendered inside [TurnFrame](#turnframe): `{from} > {to}·{provider}/{model}·Time` metadata header (`user_turn_header`, the model fragment is the [`ModelTag`](#modeltag) plain form; empty segments drop, no `>` clause when nothing follows `from`), persistent selectable markdown body, copy btn (hover; flips to a check briefly after copying — [Copy Feedback](#copy-feedback)), and an access-mode-colored frame captured at send time. `from` is the turn's real author — unattributed human input renders the localized "You", otherwise Captain (lead), Harness (host-injected turns, e.g. an answered ask's seed), or the named agent (team peer delivery, shown with a `theme.primary` peer accent); `to` is the agent whose conversation renders the turn (main thread shows Captain, a member thread its own name, a sub-agent panel the sub-agent type) — a view-side fact stamped by the owning `ConversationState`, never persisted. Peer deliveries share this same renderer live and after reload.

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`

#### AssistantMessage

Full-width block: optional model row ([`ModelTag`](#modeltag) colored form — provider muted, model wire-tinted) + markdown body (plain text while streaming) + a hover-revealed action row beneath the body. A reply that immediately follows an [ActivitySegment](#activitysegment) omits its own model row — the segment's header row carries the model name. The action row (`assistant_action_row`) renders **under** the body, never overlaid on prose, and carries the copy button (flips to a check briefly after copying — [Copy Feedback](#copy-feedback)) followed by the fork button; the whole row fades in on hover of the enclosing group.

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`

#### AssistantActions

Fork affordance for an assistant reply (`ClientCall::ForkSession`, dspo/manox#775). The branch button is **always present** (the row keeps a stable shape) and is *disabled with a tooltip naming the reason* whenever the reply is not a forkable anchor — a control that disappears cannot teach the rule it enforces. Three gates, resolved at build time (`ConvItem::Assistant`'s `fork_unavailable`): the rows must be a faithful replay of the session journal (`NotReplayed` — a sub-agent panel's answer backfill or a compaction retained tail has no id of its own); the reply must close its turn (`MidTurn` — an entry carrying a tool call has its result on a later entry, so a prefix ending there would be repaired with a synthetic "No result provided" and the child would start from false history); and the reply must have landed (`Streaming`). When all three hold, clicking forks the session at that reply — the child is a prefix copy of the source's active chain through that entry, lands as an independent sidebar row, and is opened in place; a failed fork leaves the current view untouched and stays retryable. One fork may be outstanding at a time (`Workspace::fork_in_flight`), so a double click cannot mint two children. The fork inherits the source's model / cwd / approval / effort because every intent field is left unset. Fork rides `Workspace::fork_session_at` → `SessionMultiplexer::fork_session_intent`, whose `{session_id}` receipt reuses the `CreateSession` continuation (`CreateSessionDone`).

Note: the child inherits the source's **title** — a fork copies the prefix including the journal `title` entry the auto-titler writes after the first response — so the sidebar shows two same-named rows. The runtime exposes no rename primitive yet (`thread_store::rename_thread` is still only referenced in a comment), so no `increaseTitle` increment is applied.

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`, `crates/agent-ui/src/workspace/attach.rs`, `crates/agent-ui/src/multiplexer.rs`

#### ReasoningBlock

Collapsible: chevron + "Reasoning" label + muted italic body, rendered as the title and content of an [ActivitySegment](#activitysegment) step (flush left; per-entry status glyphs are gone — the live "what is it doing" face is the [Captain Status Strip](#captainstatusstrip) above the composer). The `ai_elements::Reasoning` component itself is not wired here yet; this still uses the entry's own row and body. Each reasoning round (an `ActivityEntry::Reasoning` inside a `Thinking` segment, plus the top-level `ConvItem::Reasoning`) owns a persistent `Entity<TextViewState>` (`markdown` field) mounted on first sync — so selection survives across frames and reloaded history selects like live turns. Italic styling rides the row's ambient `.italic()` text style.

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`

#### ActivitySegment

One contiguous thinking + tool-call segment within a user turn, rendered as an **ActivityChain** (`views::activity_chain::ActivityChain`, driven by `render_thinking`) — the chain-of-thought look is RETIRED: no marker column, no connector rail, no per-entry chevron. Entries render flush with the column's left edge (the title row and the body each span the full width), and an entry's title row IS its fold affordance (hover wash + click; enter / space once focused — a tab stop like the header row, but with no chevron glyph).

**Header row** (the segment's only chrome): the model identity + rotating chevron + live braille spinner + per-kind counts (`Read×7`, `Edit×6`, `思考×8` via `message-reasoning`) + elapsed h/m/s (leading zero units dropped, second always shown — `steer_i18n::t_duration`; the span is the whole turn: live counts from the turn's start, settled reads the turn duration) + red `activity-failed` / orange `activity-awaiting-approval` badges, all riding the header's `meta` slots; clicking toggles the whole segment (`collapsed` + sticky `user_toggled`). The model identity renders through [`ModelTag`](#modeltag) — `{provider display}/{model display}` with the model segment tinted by wire api; the registration's `-anthropic`-style suffix never shows as text. **The model shows once per run**: a segment whose nearest earlier segment already carries the same model is stamped `model_repeats` and renders NO model fragment (chevron + counts only; a flat single-entry segment renders no header row at all) — a turn's repeated segments do not stack the same name.

**Whole-turn process head (dsh's TurnProcess row)**: a settled turn with at least one segment gains「已完成，用时 2分5秒 ▾」above its first emitted row (its first segment, or a leading delegation/ask card) — the duration renders as the same h/m/s form (leading zero units dropped, second always shown), derived from the state's own `turn.duration`, so the live path shows it the moment the fold archives the turn (the old event-fold minted it from a client clock and lost it whenever the Stop event raced a history reload). It mounts EXPANDED — folding is the reader's call (a settled turn's process vanishing on arrival read as "tools never render"). Folding hides the turn's SEGMENTS via a render-layer overlay (`hidden_by_turn_process`: the scope is the row's OWN turn — walking backwards, the first turn-scope marker decides: the turn's process head, or the user row that opened it, so an earlier turn's fold never reaches in; only `Thinking` hides — the reply is exempt by construction) **except the turn's LAST segment** (the answer-facing reasoning, dsh's answer-anchor rule). Live/failed turns never carry one.

**Row rhythm**: a reply immediately followed by an activity segment tightens its bottom row gap (`tight_gap_after` → `pb_1` instead of `pb_4`): the segment's own header padding breathes, so the generic 16px read as a hole (the reverse direction keeps the air — a reply needs it).

**Fold discipline (derived)**: the projection derives every fold from the fold state — no arrival-rhythm timers. A reasoning round plays OPEN while it is the segment's newest entry; the moment a new entry arrives every preceding round the user has not pinned open folds. A TOOL entry folds by default — its title row already names the call — and only a manual expansion opens it. The turn's settle folds the segment and every unpinned entry the frame the fold archives it (the retired 1s `schedule_auto_collapse` timer is gone). A manual expansion is sticky (`user_toggled`) and always wins — the reconcile preserves it and the derived fold skips pinned rows. Replayed history derives the same defaults from the same state.

**Per-entry faces (dsh-aligned)**: a reasoning round is a disclosure row — a brain glyph + the localized「思考」title carrying the round's own first line as the folded summary (separator dot, clipped, right-edge fade); expanded, the body renders through the reasoning tier (`reasoning_document`: the message ladder at the message body size, muted foreground — a round is secondary by color, never by a size of its own). Every foldable row carries a disclosure chevron (▸ folded / ▾ open) and a KEYBOARD-ONLY focus affordance (`focus_visible` border — a mouse click leaves no ring parked on the row, segment header included). The steps region plays at NATURAL height — keep it that way: an `overflow_y_scroll` height cap never established its scroll frame inside the toggle animation wrapper, so it only clipped entries past the limit (the tools after a long reasoning vanished with no way to scroll). A tool entry's head is the call's own one-line identity (`Read path`, `$ cmd` via the runtime's `tool_title`) — never a generic "tool result" label; an orphan result with neither title nor name renders its body with no head row. A tool's EXPANDED body splits by family: the seven card families render [`Tool`](#tool) cards (below); every other tool keeps the step's input rows (one muted mono `key value` row per top-level argument field, `tool_input_rows`, values single-lined and clipped) above a persistent `TerminalPanel` with document-level selection.

**Segment boundaries are part boundaries**: a segment is a maximal run of consecutive reasoning/tool-call parts (the runtime projection closes text/reasoning slots at every tool call, so the fold IS the segmentation — no heuristic anywhere). An EMPTY markdown/reasoning part (a part opened before its first delta) projects no row until it carries content; content accumulates byte-for-byte in the state (`chat/delta` / `chat/reasoning` are push_str), so pure-whitespace deltas survive verbatim — the byte-fidelity bug the single projection fixed.

**Tool cards (the ai-elements families)**: `Read` / `Write` / `Edit` / `Bash` / `BashOutput` / `Grep` / `Glob` render as [`Tool`](#tool) cards — a headless `ActivityStep` carries the card, whose own header (icon + name + mono args + localized status chip) is the fold. The card's `Entity<ToolState>` (`ToolCallItem.card`, mounted by `ensure_tool_card` on the sync paths, `no_auto_fold`) is a pure mirror: the conversation's `collapsed`/`user_toggled` stays the fold authority, the header click routes back through `entry_toggle`, and the only state the card owns is the copy flash (observers held on `MessageItem.card_subs`). Bodies are [`CodeView`](#codeview) / [`ShellView`](#shellview) with the family's parsers (read envelope, unified diff, grep hits); 400-line cap with the `tool-output-truncated` note; streaming bodies read the live tail. Edit cards additionally carry a 「在右栏打开 diff」 chip next to the copy controls (`entry_open_diff` re-reads the settled entry by id on click and hands `(path, output)` to `ChatHost::open_right_diff`) — the full-patch face lives in the right pane's [DiffTab](#difftab), the in-band body stays capped.

The component holds no policy: `open` is `layout.expanded` (which folds in the approval force-open), clicks write the caller's state, and the reveal is layout-neutral (the collapse height never animates — the list caches row heights). Segments with fewer than two entries render flat under a model-only header with no cover to click. An approval-pending entry force-opens the segment so the interactive row is never hidden. The assistant reply that follows a segment renders no model row of its own — the header is the single place the model shows. The elapsed counter ticks every second via a gpui background timer spawned on `TurnStarted` and self-terminating on terminal `Stop`/`Error`; `frozen_secs` pins the final value. Ordinary tool calls fold here instead of producing standalone cards; a reasoning round's body is its persistent `Entity<TextViewState>` markdown (selection survives across frames). Attaching a thread replays the leaf window's live-only tail (`workspace/catch_up.rs`), so an in-flight tool's streamed output survives a switch away and back.

> Source: `crates/steer-agent-chat-ui/src/views/message.rs` — `render_thinking`, `render_activity_entry`, `reasoning_step`, `tool_step`, `segment_layout`, `segment_stats`. Component: `crates/steer-agent-chat-ui/src/views/activity_chain.rs` (`ActivityChain`/`ActivityChainHeader`/`ActivityStep`, local to the chat crate — `ai-elements` no longer ships it). Container state: `ConversationState` (`ConvItem::Thinking` / `ThinkingContainer`); the derived folds and the segment assembly live in `transcript::project` + `ConversationState::reconcile`.

#### ModelTag

The resolved model identity every turn surface renders (`crates/steer-agent-chat-ui/src/model_tag.rs`): the wire's canonical id (`{registration}/{model}`, registration itself `{name}-{wire_api}`, e.g. `zhipu-anthropic/glm-5.3`) resolves once at `MessageItem` construction against the provider registry into `{provider display name}/{model display name}` + the wire api. The wire api is expressed as a COLOR on the model segment (the shared `pi_wire_text_color` vocabulary: anthropic=blue, responses=cyan, completions=amber) — the `-anthropic`-style registration suffix never shows as text. Unresolved ids (removed provider, foreign id, pre-init registry) degrade to the raw string untinted; an empty model display name falls back to the id. Consumers: the user-turn header (`plain()`, text-only), the assistant model row, the activity segment header, and the [Captain Status Strip](#captainstatusstrip) (`render()`, colored spans). A user item carries its OWN turn's model — the history wrap overrides the session label with the turn's `meta.model_id`.

> Source: `crates/steer-agent-chat-ui/src/model_tag.rs`

#### ToolCallCard

A standalone tool-call card (`render_tool_call`) for the special-case tools that don't fold into an [ActivitySegment](#activitysegment) batch — today `agent` sub-agent calls and `AskUserQuestion`. A model response's other tool calls batch into the `Thinking` container; the seven card families render as [Tool](#tool) cards (see [ActivitySegment](#activitysegment)), every other tool's output renders via [TerminalPanel](#terminalpanel).

Statuses: `PendingApproval` | `Running` | `Success` | `Error` | `Denied` — see [ToolCallStatus](#tool-call-statuses).

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`

#### PreviewCard

The app-provided `preview` tool's top-level card (`render_preview_card`) — the user-facing artifact the model shows with the Preview tool. The tool is registered app-side through the engine's embedder seam (`manox_agent::embedder_tools`, `agent-ui::preview_host::install` from `main.rs`); it is read-only and approval-free, and its result is one fixed-size metadata line (resolved absolute path / kind / bytes) — file content never flows back to the model. The card never folds into an [ActivitySegment](#activitysegment) (mirrors the ask card's top-level mint on both the live and rebuild paths) and is exempt from the terminal auto-collapse: the rendered body is the deliverable.

Faces: header (mono path + status + hover copy-path + chevron for manual collapse) over a body that mounts lazily on success — markdown renders through the persistent `ChatDocument` pipeline (read once, latched on `MessageItem::preview`; `PreviewBody::Failed` latches a one-line fallback), an svg rasterizes through the svg renderer's color path into a bounded `RenderImage` (gpui's `svg()` element would flatten it to a tinted alpha-mask silhouette), and raster images ride `img(ImageSource::from(path))` with `ScaleDown` (no mounted state). Unsupported/missing/oversize inputs land as tool-level error results the card shows in its error face.

> Source: `crates/steer-agent-chat-ui/src/preview.rs` (shared vocabulary), `crates/steer-agent-chat-ui/src/views/message.rs` (card), `crates/agent-ui/src/preview_host.rs` (tool + registration)

#### AgentTaskCard

Compact, single-line sub-agent row: `[status] type · short title`. Running and pending rows use a braille-dot spinner (`BrailleSpinner`); terminal rows use check, error, or minus icons. The title is always one line with truncation and a full-title tooltip. It deliberately renders no child text, nested messages, copy control, metrics, or expansion affordance; clicking stays a no-op. Live drill-down lives on the Agent tool-call card instead: the child session's streamed text/thinking deltas and tool lifecycle lines (`▸ Tool hint` / `✓ Tool` / `✗ Tool`) append to the card's output in real time (bridged through the Agent tool's progress channel).

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`

#### BackgroundTaskCard

Bordered card showing a background task's kind (Background Bash / subagent — async `Steer` Dispatch registered as `TaskKind::Subagent`; the Monitor kinds retired with the upstream pull-model migration), description, status badge (Running / Stopping / Completed / Failed / Timed out / Stopped / Session ended), event count, and total bytes. The title row keeps only the description's first line (a background bash description is the full command, heredoc body included) with single-line ellipsis; the complete text is shown in a hover tooltip. The detail row (failure summary or latest event) wraps in full — it is the only UI surface for a task's error text. Running tasks show a braille spinner and a Stop button that calls `background_task::stop` (cancels the child token the run task observes). Terminal tasks show a static status icon. Updated in-place by task ID via `ThreadEvent::BackgroundTaskUpdated` — the card is created when the first event snapshot arrives and never duplicated. A subagent's final text is delivered to the Captain via `BackendNotice::SteerDelivered{reason: Complete}` (facade injects a peer message + fires a turn), not via this card's Stop button; an explicit Abort settles silently (`TaskStatus::Stopped`).

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`

#### ErrorMessage

Rounded card, `bg:danger/0.06`, red text, "Error" label + copy btn. Body is a persistent selectable `Entity<TextViewState>`. A turn that fails while its thread is parked persists the same card: the parked subscription annotates the error against its own session (`append_ui_note_for`), so the card is present on switch-back and on reload. A turn that fails while its thread is parked persists the same card: the parked subscription annotates the error against its own session (`append_ui_note_for`), so the card is present on switch-back and on reload.

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`

#### NoticeMessage

Rounded card, `bg:secondary/0.15`, muted text, "Notice" label + copy btn. Body is a persistent paginated `TerminalPanel` (`PanelKind::Plain`, no command/cwd) — the same folded surface as tool output: default `PAGE_SIZE` (20) lines with a `+N` load-more row; selection + pagination cursor survive across frames. Mounted by `MessageItem::ensure_notice_panel` (live) and `new_history_item` (reload).

> Source: `crates/steer-agent-chat-ui/src/views/message.rs` · panel: `crates/steer-components/src/markdown/terminal_panel.rs`

#### RecapCard

Collapsible compaction summary card: chevron + book icon + "Context compacted" label + copy btn. Body is the model-generated handoff summary (markdown, not localized), mounted as a persistent selectable `Entity<TextViewState>`. Collapsed by default; emitted on `ThreadEvent::Compaction` and rebuilt from `MessageContent::Compaction` on thread reload.

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`

#### CacheMissDivider

Slim left-aligned divider rendered above an assistant turn whose request lost the prompt cache, matching oh-my-pi's `CacheInvalidationMarkerComponent`. Rendered as a 10-character rule + muted label `"cache miss · N tokens"` (tokens formatted by `format_tokens`). Emitted on `ThreadEvent::CacheInvalidation` and inserted as a `ConvItem::CacheMiss` into the conversation list. Live-only (`CacheInvalidation` has no journal row), so it does not survive a reload or a switch away and back. Live-only (`CacheInvalidation` has no journal row), so it does not survive a reload or a switch away and back.

> Source: `crates/steer-agent-chat-ui/src/views/message.rs` — `render_cache_miss`. Event handler: `crates/steer-agent-chat-ui/src/conversation.rs`. Enum: `crates/steer-agent-chat-ui/src/conversation.rs` (`ConvItem::CacheMiss`).

#### RetryBadge

Amber badge, `bg:warning/0.12`, braille spinner + "Retry N/M (in Xs)" text. The retry detail body, when present, is a persistent selectable `Entity<TextViewState>`, re-synced when a coalesced retry rewrites the item's detail in place. The trailing retry row is replayed when a thread is attached while its turn is still running (a parked thread's `Retry` never reached the foreground handler), and only while nothing has moved past it: content, a terminal error, or a turn boundary retires the candidate, mirroring the live pop. The trailing retry row is replayed when a thread is attached while its turn is still running (a parked thread's `Retry` never reached the foreground handler), and only while nothing has moved past it: content, a terminal error, or a turn boundary retires the candidate, mirroring the live pop.

> Source: `crates/steer-agent-chat-ui/src/views/message.rs`

#### AttachCatchUp

Attach-time replay of the live-only rows a parked thread dropped (`crates/agent-ui/src/workspace/catch_up.rs`): streamed `ToolOutput` for an unsettled call, sub-agent child/progress rows, and the trailing retry notice — each replayed onto the freshly attached thread from the leaf window, and each bounded on its own terms (the settle row ends the output, the window moving past the retry ends it).

> Source: `crates/agent-ui/src/workspace/catch_up.rs`

#### 3.2.3 Footer

Bottom area of MessageColumn, below [MessageArea](#messagearea) (or below [Hero](#hero) on first screen). It remains mounted while non-empty history is restoring so drafting never waits for backend readiness; send stays disabled until the restore lands.

#### Footer

Vertical flex, `flex_shrink_0`, `py_2`: top hairline divider, then the
[Captain Status Strip](#captainstatusstrip) (while a turn runs), then the
attachment rows and [Composer](#composer). The ask/auth interaction cards
render inline in the transcript ([AskDrawer](#askdrawer)), never by swapping
the footer — cancel priority keeps the composer live underneath them.

#### CaptainStatusStrip

The live-status block above the composer, shown while the foreground turn runs
OR while any sub-agent run is still live (AHP: "an entry may have been started
by an earlier turn", so a run can outlive the turn that spawned it), and retired
once neither is — the message column
carries no per-entry status glyphs anymore (the chain-of-thought markers are
retired), so this is the single "what is it doing" surface. Its first row is the
Captain's own: a [`PixelSpinner`](#meerkat) (grid variant, `ai_elements::PixelSpinner`)
flush at the left = healthy running, then the literal product name `Captain` +
the localized verb (`steering-verb`：正在驾驶 / is steering), the model via
[`ModelTag`](#modeltag) (`{provider}/` muted, model tinted by wire api) flush
against the verb (no spacer: `Captain 正在驾驶百炼/qwen3.8-flash·3分54秒`), `·`,
and the activity — 思考中/Thinking while a reasoning round streams,
正在回复/Replying while the assistant body streams, or the running tool's
one-line summary (monospace, runtime-provided; newline runs collapse to single
spaces so a multi-line `bash` body cannot stretch the block — the row's
`.truncate()` then ellipsizes it on width). The running clock counts from
`ActiveTurn.started_at` — the instant the host stamped the turn, standing in
for the user message that opened it — read via
`ConversationState::running_secs` and rendered as localized h/m/s with leading
zero units dropped (`2小时3分5秒` / `2h 3m 5s`; seconds always shown). The
activity comes
from `ConversationState::live_activity` (`LiveActivity`), a read-only
projection over the trailing item: a streaming assistant body names Replying,
the trailing segment is scanned back-to-front for a streaming round or an
in-flight tool, and a streaming segment with no live entry (the gap between a
tool settling and the next round) reads as Thinking. No activity yet (turn
started, nothing arrived) renders the prefix + model alone.

The rows BELOW it are the running sub-agents: one row per entry the host
projects as the parent chat's AHP background work
(`leaf.running_subagents()`, `crates/steer-agent-chat-ui/src/ahp_store.rs`) —
the host opens a run's entry on its first running tick and removes it when the
run settles, so the rows are exactly the live set. Each row is the same shape
(`spinner` + `subagent-steering-prefix`「{name}正在驾驶」+ the model tag +
`·{elapsed}`), where `{name}` is the run's agent type and task label as
`{type}({topic})` (`task_display_title`: 「Explore(修登录跳转)」— flattened by
`one_line` and bounded the same way the Captain's own row is: a run's task
label is runtime text and may carry newlines, and the block must never grow a
line per input line). The wire's background-work entry carries the topic only;
the type is resolved from the delegation tool call (its
`ToolResultSubagentContent.agentName`, or the call's own name on a completion
receipt naming the run) and stays absent when that call is not in the retained
fold. The model segment names the
SESSION's model: a sub-agent run on this harness inherits it unless its own
definition pins one, and the run's own model has no seat on the parent's
channel. The row's clock is wall-clock from the entry's `startedAt`, rendered
in the same h/m/s form. A sub-agent row's click opens that child's transcript
in the main column ([SubagentStream](#subagentstream)). The Captain's own row carries NO
click: the block is not on screen while the page owns the column, so the round
trip back is the page's own back control (no dead affordance here).

> Source: `crates/agent-ui/src/workspace/render.rs` (`render_captain_strip`), `crates/steer-agent-chat-ui/src/conversation.rs` (`live_activity`), `crates/agent-ui/src/workspace/subagent_stream.rs`

> Source: `crates/agent-ui/src/workspace/render.rs`

##### SubagentStream

The main-column page behind a sub-agent row's click
(`crates/agent-ui/src/views/subagent_stream.rs`, `ViewMode::SubagentStream`):
the child's own chat (`ahp-chat:/{run-id}`) rendered through the shared message
pipeline — the same `transcript::project` keyed diff the conversation column
uses, so the rows ARE the child's messages (never a second transcript
expression). It is
read-only by design: a child run is driven by the parent's dispatch, not by the
watcher (no composer, no chips). Header = back control (`Captain` → the
conversation) + `{type}({topic})`.

Live model: the host bridges a running child's journal into its own chat
channel, so the ONE subscribe on open delivers the folded history AND the live
deltas. The page owns a store observer and re-projects the child fold through
the keyed diff on every notify — the same path a landed snapshot and a single
streaming delta take; a pass that lands nothing new notifies nothing, and
tail-follow stays the user's state (a re-projection does not yank a reader who
scrolled back up, and a reader sitting at the bottom rides along). No polling
leg: what the page shows is the fold, and the fold only moves on host actions.

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

#### DeliveryErrorStrip

Full-width danger strip painted directly above the input while the submit pipeline cannot deliver — a latched
handshake failure (every write queues behind a dial that never lands), a latched dispatch transport failure
(retired by the next dispatch that leaves — the strip must not outlive the transport it reports dead), or
a pre-connect queue past the 10s delivery budget (the stall face is computed lazily on read; no polling pump on
the healthy path). Row: `triangle-alert` icon + 「连接未就绪，消息可能没有送达」, plus a **Retry** button on the
handshake face only (`retry_handshake` re-runs the retained dial; a no-op while connected or while a dial is in
flight). The states used to be tracing-only, so a user typing into a dead composer had no signal at all —
the black-hole shape where sends vanish without a trace.

> Source: `crates/steer-agent-chat-ui/src/ahp_store.rs` (`DeliveryFailure`, `delivery_failure`,
> `retry_handshake`, `absorb_delivery_signal`, `spawn_delivery_failure_pump`, the handshake failure latch);
> render in `crates/agent-ui/src/workspace/composer_render.rs` (`render_delivery_error_strip`)

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

#### QueuedFollowUps

Flat stack of follow-up items parked above the input while a turn is running — the queue strip has exactly two lanes. The summary line wears the transcript's own body size (`MESSAGE_BODY_SIZE`, 13px) — the row IS the message the model will read next — and stays one adaptive line: ellipsis + `min_w_0` shrink it with the window, never covering the action cluster. **待引导 (SteerPending)** rows are handed to the server's steer queue (`chat/pendingMessageSet{steering}`; the pending id IS the steer id the journal row, removal echo and card correlate by); **排队中 (Queued)** rows park locally for the settle flush. A steer-mode submit carrying attachments downgrades to the queue (the pending-message wire shape is text-only).

Every row carries, right to left: **立即打断** (`zap`, danger — cancels the running turn NOW; the row leaves the strip and its message rides the settle outbox; there is no 「待打断」 state — interrupting is an action, not a lane), a lane move (**重新排队** on a steer card: withdraw the wire steer via `chat/pendingMessageRemoved` and demote to Queued; **进入引导** on a queued card: dispatch the steer and promote), **编辑** (a steer card withdraws from the wire first, then the text/images return to the composer; a consumed steer no-ops the withdrawal and its row simply stays), and **删除** (same withdrawal discipline). Because withdrawal is the protocol's own drop path, a steer card is fully rewritable until the server actually injects it — the only race is a withdrawal that lands after the consumption echo, where the injected row stays and a re-queued card would later flush a duplicate (accepted; the window is one injection poll). The lane badge reads at a glance: accent 「待引导」 / muted 「排队中」. A failed card renders exactly like a queued one — the same four actions (立即打断 / 进入引导 / 编辑 / 删除; 进入引导 re-sends the steer under a fresh id) — with the summary text in red as the only distinction.

Retirement of a steer card happens at the **injection instant**: the store records a consumption receipt from the envelope alone (`SteerReceipt` — the `chat/pendingMessageRemoved{steering}` echo the backend publishes once the injection is confirmed, or a `chat/turnStarted` carrying the id as `queuedMessageId` — an idle steer waking its own run; the receipt rides the envelope, not the fold effect, because the single steering slot can have evicted the id long before) → the sync drains the table and `Workspace::retire_steer_card` drops the card and appends its **steered** bubble (「已引导」 badge, `meta.steered`, adopted by the turn that lands with its text). A **cancelled** settle strands every `SteerPending` card to `Failed` — the engine retracts the not-yet-injected tail without publishing removals — and mirrors `pendingMessageRemoved` back so the fold's steering slot clears.

The **settle** (`Workspace::settle_follow_ups`) runs on every `TurnFinished{cancelled}`: the outbox (the 立即打断 messages) flushes first, always, as one combined turn; the plain queue follows on a normal settle only — the user's stop must not start a surprise new run behind the queue. A dropped submit re-parks the batch as `Failed`. `⌘⌥/` (`UndoLastQueued`) pops the last plain `Queued` card. Queues and outbox are retained in memory per thread across switches, not persisted across restarts; the per-view drag marker dies on thread switch.

The **drag reorder** is the sidebar group gesture whole-cloth: the entire `Queued` row is the drag source (no grip), a bordered ghost pill carrying the row's summary tracks the cursor, and the insertion marker is a 2px absolutely-positioned accent bar on the hovered row's top/bottom edge (`top_0`/`bottom_0`, `rounded_full`) — zero layout shift where an in-flow hairline used to pump rows down 2px on every edge flip. The strip container clears the marker capture-phase on every drag move (parent before child), so the line vanishes when the pointer is between rows or off the strip, and a drop that lands on no row abandons the gesture. Commits land at the last drag-move edge and only inside the contiguous `Queued` tail (`queue_move_index_in`) — the `[SteerPending|Failed …] ++ [Queued …]` invariant is never negotiable.

On a thread (re)attach, the same sync runs: the parked session's accumulated receipts drain from the table and retire their cards; the running edge (a settle while parked) settles the queue exactly as the live edge would. Only positive signals strand a card; a remaining 待引导 card is genuinely still parked on the wire.

> Source: `crates/agent-ui/src/workspace/composer_render.rs` (`render_queued_follow_ups`, `render_queue_row`, `DraggedQueueRow` + ghost pill, `insertion_line`, `QueueRowDrag`/`QueueDragEdge`); `crates/agent-ui/src/workspace/composer.rs` (`send_user_turn` behavior routing, `steer_follow_up`, `requeue_follow_up`, `interrupt_from_card`, `withdraw_steer`, `edit_follow_up`, `delete_follow_up`, `retire_steer_card`, `settle_follow_ups`, `flush_batch`, `sync_conversation`); `crates/steer-agent-chat-ui/src/column.rs` (`FollowUpState`, `drain_queued_follow_ups`, `steer_group_insert_index`, the `interrupt_outbox` pair); steer consumption receipts in `crates/steer-agent-chat-ui/src/ahp_store.rs` (`SteerReceipt`, `steering_receipt_of`, `drain_steer_receipts`); steered badge render in `crates/steer-agent-chat-ui/src/views/message.rs` (`render_user`).

#### ComposerDivider

1px horizontal border above the composer.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

#### AttachmentChips

Vertical stack of up to two chip rows above the composer (conditional,
rendered by `Workspace::render_attachments`): a file/image attachment row
(`render_attachment_chips`, cleared on submit) and an opt-in browser-suite row
(`render_browser_chips`, persists across submits; removing a chip deactivates
the WebExplore tool suite).

> Source: `crates/agent-ui/src/workspace/composer_render.rs` + `crates/agent-ui/src/views/composer_menu.rs`

#### AttachmentChip

Single attachment chip: icon + filename + remove btn.

> Source: `crates/agent-ui/src/views/composer_menu.rs`

#### BrowserSuiteChip

Single browser-tool-suite chip: frame icon + localized suite name +
remove btn. Removing it calls `deactivate_browser_tool_suite`.

> Source: `crates/agent-ui/src/views/composer_menu.rs`

#### ComposerInputRow

Horizontal flex: [InputField](#inputfield) + [SendCluster](#sendcluster) + chips.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

#### InputField

Multi-line auto-grow text input, placeholder text. Paste rides the input's own gpui-component 0.8 `Textarea::on_paste` hook (the composer no longer capture-intercepts the `Paste` action itself): a clipboard image stages as a pending attachment chip ([AttachmentChips](#attachmentchips)) and nothing reaches the rope, copied files (`ExternalPaths`) stage through the same pending-attachment pipeline instead of pasting their path string, and plain text falls through to the engine's own insert — the `InputEvent::Change` it emits drives the [CompletionPopover](#completionpopover) re-sync. An `@`-mention chip inside the value hints its full registry key on hover: a small popover card (`mention-hover-tip`, deferred overlay beside the [CompletionPopover](#completionpopover)'s mount) pinned right under the hovered chip's laid-out rect via gpui-kit 0.8 `Textarea::on_token_hover` — shown only when the key differs from the chip's short name, since a matching pair would repeat the chip verbatim. Slash `/` tokens get no tooltip.

> Source: `crates/agent-ui/src/workspace/composer_render.rs` (via `gpui_component::Input`; hover state on `ChatColumn.mention_hover`, `crates/steer-agent-chat-ui/src/column.rs`)

#### SendCluster

The composer's action cluster (footer right, `render_send_cluster`), following thinkrail's split-send design and dressed exactly like the footer's other chips (`send_cluster_chip`: px_2/py_1, `text_xs` label, xsmall icon — the old 24px pill buttons towered over the chip row). **Idle**: one accent 发送 chip (`send-pill-send`, `arrow-up`) — inert on an empty composer. **Running**: an icon-only danger `pause` **停止** chip (`send-pill-stop` tooltip; click = `cancel_turn`; absolute cancel priority, never gated by a draft or a pending card), then the default face — an accent `list-plus` **排队** chip (`send-pill-queue`; click/Enter parks the draft for the settle flush) — and the `chevron-down` chip opening the **send menu**: three ACTION rows, most-passive first, keys escalating with the lane's assertiveness — `send-menu-queue` (排队, ↩ — 本轮结束后自动发送), `send-menu-steer` (引导, ⌘↩ / Ctrl↩ — 伺机注入当前回合), `send-menu-interrupt` (打断, ⇧⌘↩ / Ctrl⇧↩ — 停止当前回复并立即发送; the hint glyphs follow the platform's secondary modifier, pins to the row's right edge in `secondary_foreground`) — each fires the current draft with that behavior on click; the chevron stays enabled on an empty draft so the shortcuts stay discoverable. The menu is the **project chip's `PopupMenu`, whole-cloth**: built open-time (`open_send_menu`), mounted in the cluster's deferred slot (`bottom_full/right_0/occlude`), dismissed by outside click / `Escape` / row click through its own `DismissEvent` (`send_menu` + `send_menu_sub` on the column). The keyboard mirrors it on the Input (`subscribe_input`): `Enter` sends/queues, `⌘↩` steers, `⇧⌘↩` interrupts (the last is a dedicated `ComposerSubmitInterrupt` action bound at `composer > Input` — overloading `Enter` would newline AND submit). There is no 「待打断」 card state: interrupting is an action — the running turn's cancel fires now and the message rides the settle outbox ([`ChatColumn::interrupt_outbox`], stashed per thread like the queue).

> Source: `crates/agent-ui/src/workspace/composer_render.rs` (`render_send_cluster`, `send_menu_rows`, `send_cluster_chip`); `crates/agent-ui/src/workspace/composer.rs` (`SubmitBehavior`, `submit_input`/`submit_steer`/`submit_interrupt` → `submit_composer`); `crates/agent-ui/src/lib.rs` (`composer_send_key_bindings`)

#### ModelChip

Dropdown chip showing `provider · model · effort` (the reasoning-effort wire value, `high`/`max`) → [ModelMenu](#modelmenu) popup.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

#### ContextUsagePill

Compact context-occupancy pill in the footer's right control cluster, between [ModelChip](#modelchip) and [SendCluster](#sendcluster) (the dsh ContextMeter port): a 14px stroke ring + the integer percent, and the [ConversationInfoBubble](#conversationinfobubble)'s toggle — click opens/closes the bubble above it, hover only tints (never opens). The tooltip carries the absolute figures, `composer-context-usage-tooltip`. Data: the foreground leaf's `last_token_usage` — the latest request's input-side tokens (`input + cache_creation + cache_read`, the same active-token formula as the rail's budget row) — against the foreground model's `context_window` resolved through the shared provider registry. Renders nothing until a usage row has landed AND the window resolves (no capacity → no meter); fill + percent flip to `warning` at ≥90% (the rail's near-full threshold). The ring is a GPUI `canvas`: a `border`-tone full-circle stroke plus a `muted_foreground` arc sweeping clockwise from 12 o'clock, replicating the reference SVG verbatim (r 5.5, 2px round-capped stroke, `r + stroke/2 = 6.5` filling the 7px half-box); a full turn redraws the closed circle, 0% strokes nothing. Live updates ride the leaf's `TokenUsageUpdated` notify.

> Source: `crates/agent-ui/src/workspace/composer_render.rs` (`render_context_usage_ring`, `context_usage_ring`, `occupancy_arc_end`, the `CONTEXT_RING_*` constants)

#### DictationButton

Push-to-talk dictation entry in the footer's right control cluster, between [ContextUsagePill](#contextusagepill) and [SendCluster](#sendcluster): the upstream `SpeechButton` (gpui-component `speech` feature) over the workspace's `Entity<SpeechState>`. A click starts a recognition session (mic glyph; the platform's recognizer — macOS `SFSpeechRecognizer`, on-device only, audio never leaves the machine), the next click stops it and the final transcript lands at the composer's caret via the public `insert` edit path (so the input's `Change` fires and the completion popover re-syncs), and the composer takes focus back; while the final result is pending the button shows a spinner and ignores clicks, and the tooltip follows the app's ui language through `gpui_component::set_locale` (语音输入 / 停止语音输入). Renders as **nothing** wherever dictation cannot work — no system recognizer (Linux placeholder), or a process whose Info.plist lacks `NSSpeechRecognitionUsageDescription` — so unsupported platforms reserve no space and never see a dead button (`render_dictation_button` gates on `has_recognizer()`; the button itself would also self-hide). Session errors surface as `tracing::warn` only in v1. First use triggers the macOS microphone + speech-recognition permission prompts (usage keys: `script/bundle-mac`'s generated Info.plist, and `crates/steer-app/resources/macos-info.plist` embedded into the dev binary's `__TEXT,__info_plist` by `build.rs` so `cargo run` can prompt too).

> Source: `crates/agent-ui/src/workspace/composer_render.rs` (`render_dictation_button`); state + landing: `crates/agent-ui/src/workspace.rs` (`speech`, `land_dictation`, the `SpeechEvent` subscription in `Workspace::new`)

#### AccessSwitch

Inline two-segment [AccessMode](#access-modes) switch in the footer's left control group — the segmented variant of gpui-component `TabBar`: the two tiers (`access-mode-sandbox-title` / `access-mode-full-access-title`, plain-text labels, no icons) sit on a muted track and the sliding indicator pill animates between segments (the component's own animation), so either tier is one click away — no popover, no second step. The selected segment IS the current mode, read from the fold (§J11 `access_mode_or_default`) into `selected_index` — a pure projection with no local open state. Clicks on the inactive segment route through `Workspace::apply_access_mode` (write-ahead config write + object-row mirror + localized notice; optimistic flip, echo confirms / refusal rolls back); clicking the already-active segment is a no-op — the guard reads the fold at click time, so a click landing before the optimistic flip repaints stays silent (the notice announces a transition, not an echo). The left control group keeps its intrinsic width and the switch never shrinks (`flex_shrink_0`); at narrow widths the footer's right cluster yields through the model face (the model name ellipsizes) so the chips never paint under the pinned ring/dictation/send controls.

> Source: `crates/agent-ui/src/workspace.rs` (`build_access_switch`), `crates/agent-ui/src/workspace/composer_render.rs` (`render_access_switch`)

#### ProjectChip

Dropdown chip showing current project → [ProjectMenu](#projectmenu) popup. The
label is the workspace row accounting the foreground session (host registry
order), falling back to the session's project mirror for sessions no row
accounts; the menu's recency list is the same registry, and each row names the
directory by its LAST SEGMENT (`sidebar_projection::project_label`, the same
label the sidebar groups use) with the full path on the row's tooltip — the
list never spends its width on the path.

> Source: `crates/agent-ui/src/workspace/composer_render.rs`

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

The pending `AskUserQuestion`, rendered inside the conversation as the
ai-elements paged form ([AskUserQuestion](#askuserquestion)) — the card the
gallery shows is the card the app shows, in BOTH phases. There is no second,
app-local ask presentation: `render_ask_user_card`
(`crates/steer-agent-chat-ui/src/views/message.rs`) mounts `render_ask_form_card`
while the ask is pending and `render_ask_settled_card` once it is answered, and
nothing routes on the question's shape. The ask's questions become `AskBlock`s
(header, question, `detail` as host-rendered markdown, options with the 「推荐」
badge, multi-select flags); the note placeholder and the 「推荐」 badge label are
localized here, and a block's detail element rides its own page. A plan review is
an ordinary one-question ask whose `detail` (the wire's request-level `message`)
carries the plan body — the card's
pending chip (`等待回答`, the amber dot of the same awaiting-human family the
sidebar's parked-turn slot carries) and close control are
its only chrome.

The interactive half (page, selections, per-question notes) lives in the
`ai_elements::AskState` entity, whose only owner is the workspace
(`ChatColumn::ask_state`, mounted by `Workspace::ensure_ask_state` on the
render path) — the card's row holds a clone for rendering, and the composer's
Enter submits the same entity, so the answers have exactly one reader. The card
submits through `ChatHost::submit_ask` → `Workspace::reply_ask`
(`crates/agent-ui/src/workspace/chips.rs`), which folds the reported indices
and notes into the canonical `AskAnswer` rows (selected labels; a bare note as
explicit free text; an untouched question as an explicit skip). The header's
close control takes the dismissal leg (`ChatHost::dismiss_ask`) — a card close,
never a rejection — and so does Esc, see the keyboard note below.

**Keyboard.** The card takes focus the first time it mounts
(`MessageItem::seed_ask_focus` → `Window::focus` on the state's own
`FocusHandle`; the seed is window-scoped, so later frames never steal focus
back), and the host mounts `track_focus` + `tab_index(0)` on the card root.
That is what makes the `AskDrawer` bindings reachable at all — gpui dispatches
actions along the focus path, and a card nothing can focus is a card whose keys
never fire. With focus in the card: `←`/`→` are the pager's edges (`→` on the
last page is the submit the check is) and `Esc` closes. Clicking the note field
hands focus to that input, whose own bindings keep the arrows for the caret —
the deeper context wins. Pinned by
`the_ask_card_takes_focus_so_its_keys_fire`, which drives real
`simulate_keystrokes`, not a direct action dispatch.

Answered, the row keeps the SAME card in its settled phase: the header (call
name + status chip) owns the fold, the body is the host's answer markdown —
`ask_answer_markdown` folds the server's canonical result payload
(`{"answers":[{"id","selected","custom"?}]}`) into one stacked block per
question: `问{n}：`/`答{n}：` lines (a note with no selection IS the answer;
an untouched question reads as skipped, `补充：` carries a supplement to a
selection), the question text recovered from the call's own input by id. The
row mounts that phase's `AskState` itself (`MessageItem::ensure_ask_settled`,
the same lazily mounted body shape as `ensure_preview_body`). A result that
does not fold into canonical JSON (a prose result, an error) keeps the
ordinary tool card's raw output panel.

The live edge (`sync_live_ask`) arms the card from the fold: an open
elicitation (`chat/inputRequested`) seeds the pending ask AND synthesizes the
matching `ToolCall` row (same-frame guarantee), then
`sync_ask_card_snapshots` budgets the snapshot and the card state onto that row
before the list measures. A parked thread's subscription drops that event by
design, so the card re-surfaces on switch-back by an explicit re-own: the
reclaim sends `OpenSession` (same leaf and follow stream — the parked session
never detached) and the gateway replays the unsettled adjudications to the
joining owner (manox §D.6). A card whose id leaves the fold's input-needed list
after having been confirmed in it settled remotely and is reconciled away
(`reconcile_pending_with_projections`).

> Source: `crates/steer-agent-chat-ui/src/views/message.rs` (`render_ask_form_card`, `render_ask_settled_card`, `ask_answer_markdown`) + `crates/agent-ui/src/workspace/chips.rs` (`ensure_ask_state`, `reply_ask`)

**Plan review** rides that one card: the built-in `/plan` skill's proposal
arrives as a single-question ask whose `detail` (folded from the wire's
request-level `ChatInputRequest.message`) is the plan body and whose options are
the skill contract's verdicts — **Approve** / **Approve with changes** /
**Revise** — composed by the model, answered like any ask. There is no
plan-mode state, no verdict enum, and no server-side review chain: the answer
seeds the next turn (approve → execute, revise → re-present), entirely as prompt
discipline on the runtime side. The client's only plan surface is the ask card
itself.

> Source: `crates/agent-ui/src/workspace.rs` (`sync_live_ask` folds `message` into detail via `pending_ask_from_ahp`) + `crates/steer-agent-chat-ui/src/views/message.rs` (`render_ask_form_card`)

#### 3.2.4 Popups & Dropdowns

`PopupMenu` entries are `PopupMenu` entities created on open and destroyed on close. [CompletionPopover](#completionpopover) is not a `PopupMenu` — it is a pure render overlay that never takes focus.

#### CompletionPopover

Trigger: typing `/` (slash commands) or `@` (skills + subagents) at the caret in [InputField](#inputfield). A typeahead list anchored on the caret (`prepaint_caret_geometry`, one line below, flipping above when the window's bottom runs out, x clamped to the window): filters live on every keystroke, navigated with up/down, confirmed with Tab or Enter, dismissed with Escape. While open the composer wrapper sets a `completion = open` key context so the `completion == open > Input` keybindings shadow the Input's own navigation bindings. A pure render overlay — [InputField](#inputfield) keeps focus throughout, so the query keeps filtering as the user types.

> Source: `crates/steer-agent-chat-ui/src/views/completion.rs` (state + detection + rendering), wired in `crates/agent-ui/src/workspace/composer_render.rs`

#### ModelMenu

Trigger: [ModelChip](#modelchip). Model selector dropdown: provider submenus for the model list, then a Reasoning effort block (High / Max, current effort checked) under a separator.

> Source: `crates/agent-ui/src/workspace/chips.rs`

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

1. **Captain + subagents**：主 Agent 行字面量 `Captain`（不走 i18n；`views/message.rs` 的署名同样
   是字面量 `Captain`，两种语言都不翻译）。subagent 行 `{type}({topic})`
   （`subagents.rs::task_display_title`）与 Captain 同一条左基线（无缩进、无树符——从属关系唯一，
   不需要视觉编码）。**默认只显示未结束的行**（`ToolCallStatus::Running | PendingApproval` 判定，
   不用 watchdog 的 `health`），已结束的折叠进 `+N`；活行 0 也保留 Captain 行 + `+N`。上限 5。
2. **branch / worktree 对**：`┌ {branch}` / `└ {worktree 目录名}`（mono 字体，长名 truncate），
   数据 = `git_branch_display` + store `cwd`（原 `render_branch_block` 的取数方式；±change 计数与
   点击复制随卡片退役，`git_status` 收敛为 branch-only）。
3. **todos**：`UpdatePlan` 的 `PlanSnapshot.steps`，稳定排序 InProgress → Pending → Completed
   （组内保持原时序，`sort_by_key`）。状态符号 = 统一外径圆环（Pending 空心 muted /
   InProgress accent 环 + accent 内圆、文字 semibold / Completed muted 环 + 勾、文字 muted +
   删除线）——替换旧 `◻ / ▶ / ✔` 三源字形。全部状态可见（Completed 是进度感的一部分）。上限 8。
4. **per-model 用量**：模型行无树符，`{provider}/{model}` 三段分别渲染（provider 恒 muted、
   `/` muted、model 段按 wire api `pi_wire_text_color` 着色——h_flex 分段排列，不做宽度算术）。
   `├ Context {pct}% {used} / {cap}` = **该模型最后一次请求**的占用 / 窗口上限（与圆圈同口径：
   `last_token_usage` + `model_window_tokens`；只有前台模型可解析，其余模型只画 `└` 行），
   ≥90% 转 warning。`└ ↑in ↓out R{cache_read} CH{hit%}`（`format_cache_hit`，无输入 `--`）。
   按总 token 降序（稳定），上限 5。无段标题、无 per-model 成本行。
5. **sources**：沿用占位行（`workspace-env-no-sources`），去掉「来源」标题。

`+N` 折叠行：chevron + 计数，与段内行共用左基线；只在隐藏数 > 0 时出现；点击就地展开
（气泡随内容长高），再点收起。排序必须稳定（store notify 频繁，不稳定排序会跳行闪动）。

> Source: `crates/steer-agent-chat-ui/src/views/context_rail.rs`（`render_bubble` 及各段 builder、
> `fold_window` / `subagent_display_order` / `sort_todo_steps` / `todo_visual`）；挂载与开关：
> `crates/agent-ui/src/workspace/composer_render.rs`（`render_context_usage_ring`，
> 相对/绝对普通挂载 + 尾巴 `icons/context-bubble-tail.svg`）；验收图：
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

> Source: `crates/steer-agent-chat-ui/src/views/context_rail.rs`；`git_status` 分支解析：
> `crates/steer-agent-chat-ui/src/git_status.rs`

### 3.4 右栏页签内容

右栏本体（页签条 / 新标签页空态 / per-thread stash / threads.db 快照）归壳，见
[ChromeRightPane](#chromerightpane)；kind 注册表见 [ToolTabRegistry](#tooltabregistry)。
内容实体由 `ToolTab` 实现提供：

#### SubagentPanel

单个子代理运行的只读钻取页签（装配层宿主页签，`ToolTab` kind `subagent`）。页签**身份**是 run id
（`subagent:{run_id}`——`open_tab` 按 id 去重，topic 撞车的两个 run 各得各的转录；身份拿 topic 当，
第二次打开只会聚焦第一个 run 的旧面板）；页签标签是任务的 **topic**（不再是裸 run id）；面板头部是状态指示 + **access-mode 徽章**（dspo/manox#968 的 app 伴随：子 journal 的开局
`access_mode_change` 行折进子会话自己的 session 通道，泵读该叶的 `access_mode` 并缓存——纯观察面，
声明未落地就不显示，绝不猜测运行未见证的档位；文案复用 `access-mode-{sandbox,full-access}-title`
与 composer 开关同名同义，full-access 染 danger；network 半边在该协议代无持久行亦无 wire 面，
故徽章只载 mode）+ mono topic 文本——状态在运行期实时派生自
`background_work`（run 在列即 Running），终态沿用打开时的快照。正文是与主列**同一套会话管线**
渲染的微型会话：子会话（run id 即子会话 id，swarm 不变量）在打开时经 multiplexer attach，
面板持有自己的 `ConversationState`，观察 `AhpStore` 并排水**该子会话自己的事件桶**
（`ChatEvent` → `to_thread_event` → `apply`，与主列 live 腿同一词汇与同一 apply 语义）——
journal 重放与实时 delta 都经此泵进入，运行中钻取是 live 的。泵还有**fold 继承**半边：
事件桶只载本面板订阅之后的 delta，打开前（或已结算后才点开）流过的内容在子会话自己的
chat fold 里——泵在**尚未继承**时按 fold 快照整表重建（`synth_display`，fold 已含已排水事件，
故重建覆盖之、不重不漏），之后 delta 照常流式叠加。继承以快照 landed 为键、不以「面板无行」
为键：领先快照到达的 delta 会先开行，若等「无行」才继承就永远错过打开前的转录。相位三态：
`subagent-panel-settling`（订阅快照未回）→ 活动（有行）/`subagent-panel-empty`（快照已回、
无内容），永不渲染无依据的假占位。底部是 `subagent-panel-readonly` 只读说明条（宿主侧 #910
已把子会话标记为 `ChatInteractivity::ReadOnly`）。入口：会话内子代理卡片点击——卡片的
`AgentTaskItem.run_id` 来自派发信封（前台结果 / promotion 回执 / 后台启动回执都携带 `run_id`）；
rail agents 段（#165 起直读 `background_work`）展示运行中花名册。live 行的 wire `label` 只作
话题（`description`）不作类型；类型由父会话的派发 tool call 解析（`ToolResultSubagentContent.agentName`
或完成回执按 `run_id` 关联到的 call 名），行标题渲染为「Explore(话题)」。主列的
[SubagentStream](#subagentstream) 观察页共享同一 live 源（子会话自己的通道）与同一投影词汇，
但走自己的腿：fold 进、keyed diff 出，无事件桶。

> Source: `crates/agent-ui/src/views/subagent_panel.rs`, `crates/agent-ui/src/workspace/subagent.rs`

#### BrowserView

一个不可信内嵌原生 webview 的页签体（`ToolTab` kind `browser`）。chrome 行是纯 GPUI：
后退/前进 + 单行地址栏（`Enter` 导航）。内容区是 `steer-webview` 的 `WebViewElement`，
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

#### DiffTab

Edit 类工具卡的完整 diff 页签（`ToolTab` kind `diff`）。**单页签、内容替换**：实例 id 固定为
kind 名（`open_kind` 按 id 去重，第二次打开只聚焦），内容实体由 workspace 持有
（`Workspace::right_diff`：页签标题 + 解析结果）——`DiffFile::parse`（gpui-component 0.8 的
`diff` 模块）在 workspace 的事件路径上跑（页签 `render` 只读，不在渲染期建实体），再次打开
替换该实体，已开的页签无需 close/reopen 就换到新 patch。页签体是上游 `Diff`
（unified 模式默认：行号、语法高亮、hunk 折叠/展开、选区复制、变更指示条全走上游默认；
虚拟化自滚动，右栏即有界容器）。数据面：卡片回调 `entry_open_diff` 按工具 id 重读结算后的
entry，把 `(path, output)` 交给 `ChatHost::open_right_diff` → workspace 剥掉运行时信封行
（`[path#TAG]`，与卡片手解的同一首行规则）后交上游解析；解析失败不静默——页签照开、正文显示
解析错误。页签标题 = 调用 path 的文件名，缺失时落 patch 自身首个文件，再落 `chrome-tab-diff`。
会话级生命周期（无持久化形态）：patch 是活工具输出，线程切换时与 sub-agent 面板同规则退役
（workspace 侧 payload 是全局最新 patch，带过线程会串内容）。入口只有 Edit 卡片 chip，无
快捷动作（空 diff 页签没有意义）。

> Source: `crates/agent-ui/src/tool_tabs.rs`（`DiffTool`/`DiffTab`）,
> `crates/agent-ui/src/workspace/right_diff.rs`, `crates/ai-elements/src/tool.rs`（`open_diff` chip）


## 4. ViewMode::Settings

Settings page rendered inside the shell's main card as the upstream `gpui_component::setting::Settings`
shell (its own sidebar + resizable split + page stack, starting at 240px), swapping the conversation
column until the back control exits. The exit waits 200ms before flipping the mode back.

#### ManagementBackControl

Unified "back to app" control — `ArrowLeft` + label row (px_2/py_1p5/gap_2, accent hover wash, `theme.radius`). Mounted at the left edge of the [SettingsTitleBar](#settingstitlebar) strip, over the settings sidebar column. The settings page no longer ships a shared management TitleBar — each management surface reuses the app-page scaffold (overlay TitleBar in the main column), and the back control lives in that strip.

> Source: `crates/agent-ui/src/views/management_shell.rs`

#### SettingsView

Settings page state + page factory. `render` hands `build_pages()` to the upstream `Settings` shell each frame (titles re-resolve through Fluent, so they track the live locale); selection and the search query live in the shell's window-keyed state (stable id `settings`). Holds the panel sub-states — the Models / ChatGPT / VS Code form states, the MCP disabled set, the plugins view — and the persisted `ui_language`, bound to the language dropdown through `SettingField` value/setter closures (persist-then-flip; a failed save emits `SettingsEvent::SaveFailed`, surfaced by the Workspace as a toast).

> Source: `crates/agent-ui/src/views/settings/mod.rs`

#### SettingsTitleBar

Absolute-positioned `TitleBar` overlay (`h(TITLE_BAR_HEIGHT)`, `top_0/left_0`) across the whole settings card — same chrome as the conversation column's TitleBar. A window-drag region that renders no text; the [ManagementBackControl](#managementbackcontrol) sits at its left edge, above the sidebar column.

> Source: `crates/agent-ui/src/views/settings/mod.rs`

#### SettingsShell

The upstream `gpui_component::setting::Settings` component, mounted below the TitleBar strip. It owns the sidebar (a built-in search input on top of a grouped, collapsible `SidebarMenu`), the resizable split (starts at 240px, draggable) and the per-page group stack (virtualized list + scrollbar). Three pages mirror the former nav groups: **General** (the language group — a `Fill` card whose single row is the UI-language dropdown — plus the Models panel group), **Integrations** (Plugins + MCP), **External Tools** (ChatGPT.app + VS Code). Group titles double as the sidebar's sub-items; clicking one selects the page and scrolls to the group. Search filters groups by item title/description/keywords. The larger panels ride as opaque `SettingItem::render` element items inside `GroupBoxVariant::Normal` groups (no card surface), each with English keywords so search reaches them. MCP servers render as per-server `SettingField::switch` rows — the server name as the title, the live registry status (tool count / not connected / disabled) as the description — with the group description and restart-note on the group itself.

> Source: `crates/agent-ui/src/views/settings/mod.rs`

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

Plugin management lives under Settings → Plugins (`PluginManagerView`): a Marketplace tab (add/refresh/remove marketplaces, browse + install their plugins) and a Plugin tab (installed set with update/enable/disable/uninstall), all async with a busy spinner + notice banner; registry changes apply on restart (notice texts say so). Skill authoring and mcp.toml server authoring tabs from the retired harness are not ported yet (need shared-layer write APIs). The backend registry (`manox_agent::plugin::PluginManager`) stays in the agent crate.
---

## 6. 壳（Shell）

#### ChromeShell

应用壳根视图（`crates/steer-agent-chrome-ui/src/shell.rs`）：垂直布局 = 38px 工具栏（原生交通灯槽位 70px、侧栏开关、**←/→ 会话历史导航**（可用性由宿主经 `nav_avail` 查询钩子渲染期提供，无可去边界时置灰 inert）、session 下拉选择器（触发器是两组 `flex_1` 控件翼之外的最后挂载 absolute 层：固定 360px 宽、按 `(视口宽−360)/2` 钉在窗口水平中心线；下拉开面板用同一公式锚在标题栏下方 4px，面板恰好盖住触发器正下方，不再记录点击锚点）、**「在 VS Code 中打开」**（`on_open_editor` → 宿主后台 spawn `launch_plain(前台 project)`，只失败推错误通知（成功由 VS Code 打开自证）；无项目时推「没有可打开的项目」错误通知，绝不静默打开 `$HOME`）、面板/右栏开关、**品牌位**（`ShellConfig.brand` 注入的 app logo 元素工厂，None 回退通用字形；刻意非交互，且被排除在窗口拖拽面外——mouse-down 被吞掉））+ 内容区（侧栏｜主区卡［主槽｜右栏］／底部 dock）。主区卡圆角 8px、卡缝 6px；两条调宽把手为上游 gpui-base `resize_handle` 隐形带（外观渲染为空——卡缝即卡间空隙，不画分隔线；把手只发起拖载与双击重置，`on_drag_move` 挂根做绝对坐标数学，载荷类型左右各一）。`ShellConfig` 注入主槽（`MainSurface`）、右栏 kind 注册表、dock surface、侧栏固定行/自定义行、品牌位与 `HostHooks`；会话行由宿主推送快照（`set_sessions`），壳自身只持交互态（分组折叠/拖排、下拉/行菜单、**侧栏分组模式与过滤词**）；选中高亮由宿主每次快照用前台线程 id 覆写（`shell.active`），不跟随点击。固定行（Automations/Chats）与 Customizations 块（Overview/MCP）**尚未实现**：整行 `FG_FAINT` 化 + hover「尚未实现」tooltip，保持 inert（2026-09-30 假控件清理：Run/split/右栏 split·external 已删——原版 Run 是 split-button、manox 无任务系统；Sync Changes 胶囊已删——git pull/push 集成另立特性）。

会话历史的权威栈在 `Workspace` 的 `NavHistory`（cap 100，去重判据是「当前指向项」而非栈尾）：用户发起的 `open_thread`（侧栏点击/下拉选中）与 fork 落地、新建落地（⌘N、/exit 的替换会话经 `attach_created_session`）都入栈并截断前进尾；←/→ 移动指针不重复记录；successor 换代把**当前条目原地改写**为后继 id（`replace_nav_current`——追加会把已处置的前任留在 ← 一步可达处）。降级语义：栈内 id 若在线程归档/换代后失效，回退仍走 landing attach（空 landing 可接受）——不剪枝是接受的取舍。

> Source: `crates/steer-agent-chrome-ui/src/shell.rs`, `crates/steer-agent-chrome-ui/src/titlebar.rs`, `crates/steer-agent-chrome-ui/src/divider.rs`, `crates/agent-ui/src/workspace/attach.rs`

#### ChromeSessionList

侧栏会话树（props 驱动，`crates/steer-agent-chrome-ui/src/session_list.rs`）：**三行 66px 行卡（2026-09-29 thread-item 设计稿）**——标题行（16px 状态槽 + 6px 间距 + 标题）、tag 行（短 id chip 恒首位 + 用户 tag chip）、info 行（仅最后活跃时间：72h 内相对、之外本地 `MM-DD HH:MM`），三行共用一条左基线、无任何右对齐内容、**不随项目层级缩进**（层级只由分组头与 leader chevron 表达）。行面上**零控件**：pin/archive/标签/复制 ID 全部收进右键菜单（`Shell::open_row_menu` 六项：置顶 toggle／归档 toggle／多选会话…／添加·重命名标签／移除标签／复制 ID；tag 内联编辑挂在 tag 行，Escape 取消、Enter/blur 提交、空值丢弃、10 字上限；双击用户 tag 芯片 = 老壳同款进入重命名编辑，短 id 芯片单击复制完整 id）。四态字形（派生阶梯 Pending > Running > Unread > Idle，行事实活取 live fold：`running` 取 chat fold 的 `active_turn`、`pending` 取未答 ask／input-needed 队列、`unread` 取 fold 位的客户端镜像，summary 位仅作未订阅会话与快照未落地窗口的回退——`multiplexer::merged_row`）：`Pending`（`Auth` 待批准／`Ask` 待回答，两态同字形、只 tooltip 文案区分）琥珀 `WARN_AMBER` 静态问号（2px 圆点点阵拼成，与 `Running` 积木同一点阵语言）／`Running` 复用 [`PixelSpinner`](#pixelspinner) grid 变体（ACCENT 色，`.synced()` 锁 App 共享时钟运行行同拍 + `.max_fps(30.)` 限帧——原 `session_list::pixel_grid` 手搓近似已删除）／`Unread` 实心 6.5px 绿点 `SUCCESS_GREEN`（「跑完待验收」完成提醒语义）／`Idle` 空槽；`Errored` 已离开列表——错误是 turn 内容、在转录正文呈现，行永不显示错误。行 tooltip 仅挂非 Idle 行（空闲行无事可说，也让开截断跑马灯）：一行主状态文案（`sidebar-status-*` 双语键），翻译惰性发生在 tooltip 打开时、不在渲染路径。四态表面：未选中无背景、悬浮 `LIST_HOVER` + 标题转 500 字重 + **截断标题跑马灯**（双份标题 + 24px 间隔的无缝循环轨道：24px/s、每循环停 600ms、回绕点像素级相同无闪跳；仅 `is_hovered && title_truncated` 启动，移开复位；截断判定 = 与渲染器省略号同一套 `shape_text` 实测宽 vs `on_prepaint` 逐帧记录的剪裁盒宽）、选中白卡 + 15% 描边、键盘焦点 = `track_focus` + `focus_visible` 1.5px accent 环（↑/↓ 在可见行间移动焦点，行高四态一致不 reflow）；分组头可折叠（折叠态按**稳定 state key** 存取——`SessionGroup.key`，时间分组用 i18n 键字符串、workspace 分组用项目路径（无项目桶退回其显示名 "Chats"，该显示名同时是它的拖拽与持久化身份），显示名随语言切换不落状态）并作为拖拽源/放置目标（2px accent 插入线；**仅 workspace 模式**——时间模式的分组头不是拖拽源、容器不是放置目标，渲染期直接不挂拖拽机械）；workspace 模式下分组头同时是**项目菜单**面（见 [ProjectGroupMenu](#projectgroupmenu)：头右端常驻 `MORE` 省略号钮（FG_FAINT，hover 转 FG）+ 头右键，二者都 `stop_propagation` 不触发折叠）。**5 行折叠**（`GROUP_ROW_LIMIT`，两种分组模式同律）：超出上限的组只渲染前 5 行 + 组底 footer 行——「显示更多（N 条）」↔「收起」（`Shell::toggle_show_more`）；折叠分组（chevron）即重置折叠，重新展开回到 5 行，重启不持久化；藏区含**前台活跃行**时该组自动展开（一次性：按活跃行 id 记录 `revealed_for`，手动「收起」后不再弹回，直到前台会话再次变化——自动展开与手动展开同落 `expanded_groups`，footer 的「收起」才能一次生效）。

头部右侧控件（2026-09-30 起为真控件）：**sort**（workspace ↔ 时间分组切换，时间模式下点亮；时间分组 = 本地自然日四桶「今天/昨天/最近 7 天/更早」，分桶键 `sort_stamp`（真实时间戳——member 沿用 leader 的戳，team 不拆桶；编码 rank 进时间戳会让 chrono 解码全部越界落「更早」），桶内排序键 host `rank` 升序（member 沿用 leader 的 rank，team 不散序；成员单独置顶仍可上浮——pin 逐行的既有语义），空桶不渲染）与 **search**（展开 header 下过滤行：InputState 过滤输入 + × 清空；title/project/tag 不区分大小写包含，无匹配组隐藏、过滤中强制展开，全滤空时显示「无匹配会话」提示；纯壳内显示态，不持久化）。

**批量选择模式**（`Shell::selection: Option<Vec<String>>`，行菜单「多选会话…」进入并勾选该行）：thread 行的状态槽换成勾选圈（未选 `CIRCLE` 空心 / 已选 `CHECK` accent），行左键**切换勾选而非切换会话**（`on_select` 不触发），已选行白卡 + accent 描边；外部会话行不可选（保留品牌标、点击惰性）。列表底部钉一条操作栏（`selection_bar`）：「已选 N 项」+ 全选（当前可见的 thread 行——过滤后的投影、折叠分组不计）／归档（accent 药丸，空批次置灰且不提交；Escape 等同取消）／取消。归档一次性经 `HostHooks::on_batch_archive(&[id])` 提交——宿主侧 `SessionMultiplexer::archive_sessions` 发**一条** `x-manox/archivedChanged`（ids 入 payload，走常驻订阅的 catalogue root，不逐行订阅 session channel——宿主 subscribed-gate 会拒掉未附着行的原生 `session/isArchivedChanged`，附着又要付整本 journal fold）；行当场离开侧栏（壳 `set_archived` 乐观删 + store `remove_summary`），退出选择模式。右键菜单的单条归档走同一写路径（一条的批次）。store 侧 `commit_summary_walk(exhausted, keep)`：走完到底的 `listSessions` 分页即宿主活跃分区全集，未被点名的行（本端或他端归档）从 catalogue 剪掉（前台会话例外），终结 upsert-only 的「归档后又刷回来」。离屏截图开关 `CHROME_SELECT=1`。

> Source: `crates/steer-agent-chrome-ui/src/session_list.rs`, `crates/steer-agent-chrome-ui/src/shell.rs`

#### ProjectGroupMenu

项目分组头的动作菜单——旧壳侧栏（2026-09-28 随单壳切换退役）的「项目 `…` 菜单」在 chrome 壳上的回归。**职责切分**：chrome 只出**面**——分组头省略号钮/右键捕获打开位置，`HostHooks::on_group_menu(key, project, anchor)` 把 (state key, 项目路径) 交给宿主，宿主经 `agent_ui::project_menu::group_menu` 构建菜单实体，壳用与行菜单同型的 deferred/anchored 浮层挂载（`chrome-group-menu`），DismissEvent 收起；时间模式无此面（时间桶不是启动目标）。**内容**（`project` 为 `None` 的 Chats 桶保留全部启动行、按宿主回退 cwd 定界，仅隐藏移除行）：「新建会话」子菜单（Steer 平行行 = `start_new_thread(Some(project))`；Claude Code／Codex／GitHub Copilot 各一个 provider→model 级联——**复用 Composer 选模型的共享 builder** `model_cascade::build_model_menu`（`model_catalog::rows()` 按 provider 显示名分组、每行 wire Tag 徽标 + 模型显示名，与 composer 选择器同一 look & pick 交互），选中即 `spawn_agent_terminal` 以**该项目目录**为 cwd 拉起 CLI，经 `external_sessions::launch` 注册为**外部会话**（右栏 ToolTab + 侧栏行，见下）；「新建终端」（`spawn_standalone_terminal`，项目目录，同样注册为外部会话）；「VS Code」（`launch_vscode_app_from_settings` 注入式启动、后台执行，未装 VS Code 或无目录可用时禁用）；分隔线 + 「移除项目」（`project_registry::forget_project` overlay 写 + `thread_store::remove_project` 同步 v2 账户，`ws.notify()` 立即重投影——组溶解、会话落回 Chats 桶，历史不动）。

**持久化**（AHP 世界无项目注册表可反注册，分组读会话自身 project 绑定）：`agent_ui::project_registry` 在共享 `~/.manox/settings.toml` 的 app 自有**三键**上做 parse-edit-serialize（与 `ui_language` 同款只碰己键纪律）+ 进程级缓存（投影每次 multiplexer notify 都要查，不逐帧读文件）——`removed_projects`（移除路径集；`project_groups(rows, removed)` 把命中行**改投 Chats 桶而非隐藏**）、`project_added`（路径 → 首次出现在侧栏的时间戳，`note_seen_projects` 首见打戳；默认序 = 最新添加在最上，功能上线前的存量项目视为同时添加、以既有最近活动序为初始序）、`project_order`（手动拖拽序，决定它所列键的最终顺序；未列键按默认序浮在其上，「Chats」无戳垫底）。重新绑定即重新注册：`register_project` 清移除条目 + 撤销手动序条目——组浮回手动序区块之上（若已有更新的未记录项目，仍排在其下）；首次出现的时间戳不动（它记录的是目录何时首次出现，不是何时回归；`Workspace::register_project_in_store` 仍是全部绑定路径的汇聚点）。装配层快照时解析显示序注入壳（`order_groups` → `Shell::set_group_order`）；壳内拖拽经 `HostHooks::on_group_order` 回写全量键序 → `set_manual_order` 持久化。

**外部会话 = 主列会话**（`agent_ui::workspace::external_sessions`，旧壳 external-session 家族的 chrome 回归）：AHP 无外部会话通道，注册表是客户端状态、挂在 Workspace 实体上（`externals: Vec<ExternalSessionRecord>` + `active_external`，各记录持 live `TerminalView` 实体与它的 `Title` 事件订阅）——项目菜单拉起 agent/终端即注册并**顶替会话列占主列且自动聚焦**（`ViewMode::ExternalSession`，与 Settings 同一条主列换页轨道；壳子套主列，终端/TUI 就放主列，不放右栏）。**身份模型（legacy 同款）**：行 id = `external:{agent}:{uuid}`（命名空间隔离 manox thread UUID），记录另持**真实外部会话 id**（`cx_session_id`，从 cx session socket 的 `<id>.sock` 文件名导出；spawn 时 tracing 记录两者关联）；路由按 `ws.is_external_session(id)` 记录查找（非前缀协议）。切走即停靠（点 thread 行/新建会话隐式 `leave_external_session`，终端保活）；侧栏行走 `on_select` 的 `ext-` 前缀分叉 → `open_external_session` 切回主列（同步 ws 更新，无 window handle 往返，无 dispatch 时序坑）；行菜单「关闭会话」→ `close_external_session`（drop 记录 = 拆进程树，正在前台则回落会话列）。行投影 `external_session_rows` 并入装配层快照（`rank = i64::MAX` 恒排已排线程之后）、壳按项目 path key 归入项目组（显示名——尤其 alias——从不当组身份；快照进壳即统一排序：pinned 置顶、host `rank` 升序——`set_sessions` 内 `sort_sessions`，新拉起的会话行置组首，`session_order` 回归钉死）；高亮不被前台线程规则抢走（`active_is_external` 不回写）。主列标题（`PendingMain::title`）在外部会话前台时显示会话名。**进程级寿命**：PTY 属于 Workspace（进程单例），窗口重开仍在（比右栏 stash 的窗口级寿命更强，与 legacy 一致）。行级抽象同款 `SessionRowKind::{Thread, External{icon}}`——品牌位 + 行菜单（关闭会话／**重命名**／复制 ID）；时序律仍有效：open/close_tool_tab（右栏页签通用路径）在 click dispatch 内会被拒（探针测试钉死），外部会话已改为纯 ws 更新不受其辖；**Steer 平行行同律**——click 闭包自带 `&mut Window` 直通 `ws.update`（`project_menu_new_thread` 回归钉死），绝不走 `window_global().update`：click dispatch 已租住窗口，重入 update 一律 `Err(window not found)`，被 `let _ =` 吞掉即「点了没反应」（本菜单 Steer 行的旧形态正是此坑）。**标题双写面**（sidebar 行与主列标题共用 `record.label` 一个字段）：① **TUI 自报**——终端的 OSC 0/2 标题流经 manox-terminal 解析成 `TerminalEvent::Title`，spawn 时 `spawn_external_session` 订阅 **proxy 实体**（gpui 事件面；**绝不订 handle 的 broadcast**——proxy 泵依赖它是 handle 的唯一订阅者，`Arc::try_unwrap` 在多订阅者下丢全部事件）→ `apply_terminal_title` 写记录并去重（TUI 高频重发、逐条 notify 会刷侧栏；`Title(None)` 重置回 `default_label` 启动名），80 字符截断与重命名编辑器同预算；② **用户重命名**——行菜单「重命名」在**标题行**原位挂内联输入框（`title_edit`，与 tag 编辑器同生命周期：Escape 取消、Enter/blur 提交、空值丢弃、80 字上限），提交走 `on_set_external_title` 宿主钩子写同一字段。两面 last-writer-wins：重命名后 TUI 下一次自报仍会夺回标题（TUI 拥有标题所有权）。

> Source: `crates/agent-ui/src/project_menu.rs`, `crates/agent-ui/src/project_registry.rs`, `crates/agent-ui/src/workspace/external_sessions.rs`, `crates/agent-ui/src/chrome_assembly.rs`, `crates/steer-agent-chrome-ui/src/shell.rs`

#### SidebarProjection

wire 行 → chrome 侧栏 props 的**纯投影**（`crates/agent-ui/src/sidebar_projection.rs`）：`ThreadRow`（multiplexer 权威行，含 §D.5 增量合并）三事实 `running`（chat fold 的 active turn；未订阅会话回退 wire `InProgress`）／`pending`（会话 chat 折叠里有未答的 input request = Ask——turn-final 的 ask 会保持 turn 打开直到答案结算；会话 input-needed 队列 = Auth；同开时 Ask 胜。fold 的沉默须待承载它的通道快照落地才生效——chat 通道管 running/Ask，session 队列管 Auth，wire 位是快照落地前的回退）／`unread`（叶子 unread 镜像覆盖 wire 标志，status-0 占位退让 summary）→ `derive_status` 阶梯（Pending > Running > Unread > Idle；wire `Error` 位不再被行消费——错误是 turn 内容，列表永不显示）＋ team 森林（leader 保序、member 随后、孤儿拍平；**member 不再缩进**，leader chevron 是唯一嵌套标记；member 的 unread 不上浮；成员运行数尚未聚合——team tree 无 wire face，成员行恒平铺）＋ `updated_at`/`archived`/tag/pinned 原样透传 ＋ 按项目路径尾段分组。装配层在 multiplexer notify 时喂给 `ChromeSessionList`。

> Source: `crates/agent-ui/src/sidebar_projection.rs`

#### ChromeRightPane

右栏外壳（`crates/steer-agent-chrome-ui/src/right_pane.rs`）：圆角卡 + 页签条（**下划线式页签**：平面标签压在条带自身的 `border_b_1` 共享轨道上，激活项为 `ACCENT` + 半粗；一条**共享的下划线指示器**按激活 id 播放滑动动画，从旧页签横移到新页签；条右端仅「+」一个动作——再开一个激活 kind 的实例；无激活页签（新标签页空态）时按共享扁平按钮的 Disabled 形态置灰且不挂点击，2026-09-30 删除无语义的 split/external 假钮；**条行容器必须显式 `.flex()`**——gpui div 默认 block，缺了动作组会换行压进正文）+ 新标签页空态（快捷操作由注册表生成）+ 打开/激活/关闭生命周期（最后一个页签关闭即收起）。

页签几何由 `on_prepaint` 实测上报（`TabBounds`，键为页签 id），指示器据此定位——标签宽度不一，无法由序号推出。注意 `on_prepaint` 上报的是**内容盒原点**（它挂的是 `canvas().absolute().size_full()` 子元素，padding 已计入），故记录时减去 `TAB_PL` 还原页签左边界；指示器与条带是**兄弟**（同在 relative wrapper 内）而非父子——gpui 的 `Style::paint` 先画子元素、**后画自身 border**，所以子元素永远压不住条带的 `border_b_1`，会只剩半截可见。wrapper 即指示器的包含块，其原点也就是 tab 几何的反基准坐标系。内容经 `ToolTab` 注入、kind 经 `ToolTabFactory` 注册；**实例级 id**（一种 kind 可多开）。**per-thread 会话**：`RightPaneSession{open, store, active_id, visible}` 整体 stash/restore（挂起走 `on_active(false)`——浏览器子视图隐藏、终端保活；仅显式关页签才拆内容）。快照经 `ToolTab::persist` / `ToolTabFactory::restore`（浏览器 `{"url"}`、编辑器空稿可恢复；终端与 CLI 会话不可复活，恢复时丢弃）落 `threads.db` 的 `thread_right_pane`。

> Source: `crates/steer-agent-chrome-ui/src/right_pane.rs`, `crates/agent-ui/src/chrome_assembly.rs`

#### ToolTabRegistry

chrome 壳右栏的 kind 全集（`crates/agent-ui/src/tool_tabs.rs`，快捷操作顺序）：**终端**（$SHELL，独立 PTY，关页签拆进程树）、**Claude Code / Codex / GitHub Copilot**（页签体先落模型选择器——复用共享级联投影 `cascade_provider_groups`；点选即以该端点 `AgentBuilder` 拉起 CLI，picker 实体此后自渲染 TUI；cwd = 前台线程项目目录）、**编辑器**（markdown 软换行 + 行号）、**浏览器**（真 `BrowserView`：地址栏 + 导航；走生产 `restore_browser_tab`/`close_browser_tab`，注册进进程级 `WorkspaceBrowserHost`——IPC notify/inbound、eval oneshot、yield 全通；2s ticker 把页面 `<title>` 镜像到页签标签，`on_active` 隐藏 OS 子视图防漂浮）、**diff**（见 [DiffTab](#difftab)，注册表内但不上快捷操作页——无 patch 的空 diff 页签没有意义，入口只在 Edit 卡片上）。

> Source: `crates/agent-ui/src/tool_tabs.rs`

#### ChromePanel

底部 dock（`crates/steer-agent-chrome-ui/src/panel.rs` + `shell.rs` 的渲染）：通用容器，内容经 `PanelSurface` 注入（`open`=展开即拉起、`close`=收起即回收；新建/清理/收起三个动作）。steer 装配装集成终端，cwd 随前台线程；dock 视图按线程 stash/restore（同右栏语义）。

> Source: `crates/steer-agent-chrome-ui/src/panel.rs`, `crates/agent-ui/src/chrome_assembly.rs`

#### ConversationColumn

`Workspace::render_column`：会话列本身（hero 空屏／虚拟化消息列表／composer footer＋附件与 chips／ask 与 blank-project overlay／TurnNavigator overlay），四周的 gutter / 侧栏槽 / 卡壳 / 内嵌标题栏全部归壳；键盘动作面经根装饰器 `apply_chat_actions`。装配把该视图作为壳的 `MainSurface`，其 multiplexer 同时喂侧栏投影泵。

> Source: `crates/agent-ui/src/workspace/render.rs`

#### SettingsCard

Settings（`Workspace::render_settings_card`）：同一状态机（`ViewMode::Settings` + `SettingsView` 订阅），渲染换位——设置导航列（240px）｜分隔线｜面板 放进主卡（chrome 侧栏槽是会话列表）；⌘, 与原生菜单 `Settings…` 经 `apply_chat_actions` 的 `OpenSettings` 处理进入，返回走 nav 的 back 控件（`SettingsEvent::Exit` → 滑出后回会话）。

> Source: `crates/agent-ui/src/workspace/render.rs`, `crates/agent-ui/src/views/settings/mod.rs`

---

## 7. Shared Primitives

Reusable UI elements from `gpui_component` and `steer-components` used across all views.

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

First-party selectable text panel (`steer-components::markdown::TerminalPanel`, an `Entity` + `Render` in the `markdown` module) that renders tool output as a terminal-styled shell — **not** a real terminal (no PTY, no grid; `crates/manox-terminal`/`TerminalView` are not involved). One persistent `Entity<TerminalPanel>` is owned per `ToolCallItem` (live + reloaded history); selection and copy ride the window-level `TextSelection` shared with every other markdown surface, so a drag crosses prompt, body, and neighboring message text. The panel renders **only the body**: a transparent, untinted vertical flex (no background fill on the content — it blends into the message list; mono font at `text_sm`（代码档，与代码块同号；thinking body 走 markdown 正文档 `text_base`）; `px_3 py_2`) that hosts one `TextView` document composed of a prompt block + the body (the composed text + highlight runs are fenced into an `ansi` code block, with the panel's runs handed to the code-block highlighter). The prompt block appears **only for `bash`** — the one tool that runs a real shell command a human would type in a terminal; internal tools (`grep`/`read_file`/`edit_file`/`glob`/`list_directory`/…) and MCP tools are manox abstractions, not terminal commands, so they render the body only (no cwd / `❯` preamble that would imply "run this in a shell"). The `bash` prompt block: line 1 cwd (home `~`-collapsed) + `git:{branch}` + status markers (`*mod ✘del !conflict ?untracked`, zero counts and the whole git segment omitted when not a repo); line 2 `❯` (green) + the echoed command. **Three-way text styling** separates prompt chrome / input / output by color × slant: guidance (cwd / `git:` / branch / markers / `❯`) — foreground + **upright**; the echoed command — foreground + **italic**; the body (output) — muted + **italic**. The document's base run paints muted + italic (so command output / file content / diff context all read muted + italic); the prompt-block guidance and command runs pin their own slant via `styled(color, italic)` so the body's base does not leak into the prompt. The body is rendered per a `PanelKind` chosen by the agent-ui layer from the tool name (`tool_panel_body`): `File` (`read_file`/`write_file`) — a sequential line-number gutter (the agent-ui layer pre-strips the hashline `[path#TAG]` header + `N:` prefixes for `read_file` and feeds the written `content` for `write_file`, so the panel just numbers the content lines 1..N); `Diff` (`edit_file`) — `+`/`-` lines green/red, `@@` hunk headers cyan, `[path#TAG]`/`---` separators muted; `Plain` (default, `bash` + everything else) — `vte::ansi::Processor` parses SGR foreground/bold/italic into `HighlightStyle` ranges (control bytes stripped from the plain text, truecolor/256/16-color resolved; background SGR tracked but not painted). The whole document is one continuous selection across prompt + output (the panel caches its highlighter `Arc` on the styled runs — a rebuilt one would leave the body unselectable); streaming bodies ride the fade-in motion, settled ones adopt updates at once. The terminal chrome — a **titlebar** showing the command summary (`gh issue create` for `bash`, `read_file path` otherwise) + status + disclosure chevron, click-to-toggle the body — lives in the agent-ui header (`render_tool_entry` / `render_tool_call`): the titlebar and this body share one bordered rounded frame so the pair reads as a single terminal window (titlebar gets a `border_b_1` separator only while the body is shown). Selection supports the upstream window-selection gestures (drag-extend, double-click word, triple-click line) and Cmd/Ctrl+C copy via the `Copy` action. Git state is snapshotted per `bash` panel by the agent-ui layer via a background `git status --porcelain` + `git rev-parse --abbrev-ref HEAD` probe keyed off the thread cwd (internal tools skip the probe — they render no prompt block). `render_tool_output` returns `item.panel.into_any_element()` when mounted, falling back to a fenced code block otherwise. **AskUserQuestion answered-state exception** (B2-PR-2): when the call is an `AskUserQuestion` and its `output` parses as the canonical result JSON (`{"answers":[{"id","selected","custom"?}]}` — what both the live `ToolResult` deposit and the rebuild journal translate carry once the server makes the model-facing result canonical), `render_tool_output` short-circuits the panel/fenced paths and renders compact human Q/A rows via `ask_result_qa_rows` + `render_ask_result_body` — the question text recovered from the call's own `input` by `id` (unknown id → the id itself), a skip (`selected: []`, no `custom`) shown as an em dash. Any non-canonical payload (the transitional prose render, an error, malformed JSON) falls through to the raw output verbatim, so the fold is forward-compatible and never regresses on a parse miss.

**Syntax highlighting (`File`/`Numbered` only).** `set_lang` names the body's language (the agent-ui layer derives it from the input path's extension for the file-shaped tools): the content lines highlight through the same tree-sitter dispatch the markdown stack uses (`text_view::code_runs`, thread-local cached), with each line's runs translated past its gutter; `Numbered` gap markers split the highlight into independent segments (a window gap is not source to carry across). `Plain`/`Diff` ignore `lang` — they carry their own coloring (ANSI / diff). Unknown languages degrade to the plain body.

**Pagination.** A finalized body renders `PAGE_SIZE` (20) lines at a time; a "load more" affordance below the body (a centered `ChevronDown` + `+N` count, top-bordered, hover-tinted) grows the window by another page via `show_more`, clamped to the total. Streaming bodies render the whole live output (no pagination); on the streaming→finalized transition the cursor resets to the first page so the result opens at the top. The panel has **no internal vertical scroll** — the message-list `message-list` div scrolls the whole panel — so `show_more` never touches a scroll handle: growing the window appends lines below the current viewport without jumping to the tail. The pixel-anchored, tail-following message-list arbitration (recomputed each frame in `on_prepaint`) keeps the viewport at the user's reading position across the growth, so successive "load more" clicks stay anchored to the current line rather than snapping to the end.

> Source: `crates/steer-components/src/markdown/terminal_panel.rs` · wired by `crates/steer-agent-chat-ui/src/views/message.rs` (`tool_panel_body` → `ensure_tool_panel` / `sync_tool_*_panel` / `rebuild_tool_panels`, titlebar frame in `render_tool_entry` / `render_tool_call`) + `crates/steer-agent-chat-ui/src/conversation.rs` (`apply` ToolOutput/ToolResult arms, `rebuild_from_messages`)

#### Markdown


Upstream gpui-base `TextView` rendering, assembled at one point (`steer-components::markdown::text_view`) so every markdown surface shares the theme bridge, the heading ladders, and the code-highlight dispatch. The self-hosted ast/incremental/rich_text/selection stack retired with the TextView adoption (issue #124): streaming (incremental parse + fade-in motion), cross-block selection, and table source copy are all first-class upstream — selection is window-level (`TextSelection`), so a drag now crosses message bodies, reasoning rounds, and terminal panels; Cmd/Ctrl+C rides the `Copy` action with `SelectionFormat` (plain text, or markdown source for partial selections). Text documents are persistent `ChatDocument` bodies (`steer-components::markdown`) — an upstream `TextViewState` plus a source cache. Upstream `set_text` re-parses from an empty baseline every call, so the cache routes syncs: an unchanged source is a no-op, an appended linkified delta rides the incremental `push_str` path, and a rewrite falls back to `set_text`; static chrome mounts per-frame documents keyed off their element id. Streaming bodies fade new text in word by word (`stream_fade` motion, claude.ai-measured timing; the old trailing cursor retired). Code-highlight dispatch per fence language: `ansi` parses SGR runs (terminal panels), conflict markers classify as conflicts regardless of language, `diff` colors per line, `svg` takes the markup grammar, everything else highlights through tree-sitter with a per-`(lang, content_hash)` cache. The highlighter is installed ONCE as the app-wide `TextViewDefaults` highlighter (`install_code_highlighter`, called from the chrome mount after every theme sync) and is never passed per view: gpui-base compares the resolved highlighter by `Arc` identity and treats a new one as a selection-preserving reflow, so a per-render closure keeps that path armed every frame and the document ends up UNSELECTABLE. Per-document runs (terminal panels) go through `shared_code_block_highlighter` with the `Arc` cached on the document owner (`TerminalPanel::highlighter`). A window-level guard installed from the workspace paint (`install_selection_drag_guard`) ends a selection drag the moment the pointer moves with no button held: a popup menu's click stops propagation, so the engine never sees the mouse-up that would end the drag its mouse-down armed, and its move handler extends the selection without checking the button. An ```svg fence is not text by default: the block parser rasterizes it into a custom block during Markdown conversion (`steer-components::markdown::svg_block`, the app's `SvgRenderer`, same source budget and raster box as the svg [PreviewCard](#previewcard) body), so it paints as its diagram in a box with the diagram's own aspect, capped at 512px tall, with a corner toggle (`</>` / image icon) that flips the block between that image and the fence's own highlighted code block — the state lives in the window's keyed state per block, so a flip survives re-renders; a fence that is not drawable — another language, invalid SVG, past the budget — falls through to the ordinary code block, and the block's text form stays the fence source, so whole-document copy keeps it. The ```mermaid fence rides the same block from parse on (`steer-components::markdown::mermaid_block`): its source renders to an SVG document at conversion time by the pure-Rust `mermaid-rs-renderer` (23 diagram kinds; an unknown kind or syntax error leaves the fence a code block, and a still-streaming fence stays code until its closing delimiter lands), and that document feeds the shared rasterizer, face flip and selectors (`markdown-inline-mermaid…`); the registration lives beside the svg one in `svg_block::extensions`, behind `with_diagram_blocks`. Math is the other prose-that-is-not-text: it typesets at conversion time by the pure-Rust `mathra` engine (STIX Two Math, MATH-table layout, glyphs as path outlines; strict mode — an invalid formula stays the literal text the unclaimed path renders), the ink is applied as a root `color` before the shared rasterizer, and the text form stays the dollars source so copy and the markdown round-trip keep it (`steer-components::markdown::math_block`). Three shapes are claimed: the mdast `Node::Math` a `$$` fence alone on its line parses to, a paragraph that is nothing but a one-line `$$…$$` pair (the parser calls that inline math — the form models actually write — so the block claims the paragraph and paints the display block, selector `markdown-math-block`, centered at the em box the SVG declares at the document's own text size), and every remaining `$…$` / `$$…$$` as an inline run on the line's own font size and baseline (`InlineElement` baseline from the root's `vertical-align`, selector `markdown-math-run`); a single-dollar pair is claimed only when it sits tight against its dollars, so currency (`$5 and $10`) stays prose. Bare URLs and filesystem paths in plain text become explicit `[text](target)` links at the source level (`linkify::linkify_markdown` — upstream has no autolink that can split a text node); a plain click opens them (browser for URLs, VS Code for paths). GFM tables render upstream-style with a copy control that copies the table's own GFM serialization (`TableData::markdown`); code blocks and tables carry hover copy controls wired to the message item's `CopiedRegistry`. Heading ladders: message density holds H2–H6 at body size (H1 one step up, levels discriminated by weight), chrome density flattens the ladder to base size — the old `HeadingMode` contracts.

#### Copy Feedback

Transient copied state for copy controls (`steer-components::copy_feedback`): the owning entity holds a `CopiedRegistry` keyed by each control's `ElementId` and exposes it via `CopyFeedbackHost`; `copy_button` (`ghost` + `xsmall`) shows `IconName::Check` instead of `Copy` while its id is lit — `COPY_FEEDBACK` (1.2 s) after the click — and routes the click through the owner's weak handle (`fire_copied` marks the generation, notifies, and spawns the revert timer; a re-fire while lit replaces the generation so only the newest timer reverts). Wired by the markdown code/table copy controls (`MessageItem` hosts the registry, threaded as `CopyFeedback` through the item renderers) and the message copy buttons; the ChatGPT settings panel's text control swaps its label instead (「已复制」/ "Copied", `settings-btn-copied`).

> Source: `crates/steer-components/src/copy_feedback.rs`

#### TurnFrame

Shared framed text container (`steer-components::turn_frame::TurnFrame`) used for user turns. It paints one continuous accent-colored stroke path for the door-shaped frame, leaving the bottom center open while preserving rounded `╰─` / `─╯` corners. The lower stroke is lifted slightly into the bottom padding so the open edge visually hugs the final text line without letting markdown content overflow its layout box. The component does not fill the content background, does not rely on masking a complete border, and avoids assembling the frame from independent rail nodes. Callers provide header, trailing controls, and body content.

> Source: `crates/steer-components/src/turn_frame.rs`

#### Icon

Named icon from the icon set (e.g., `IconName::Folder`, `IconName::Search`). 全部图标统一走 SVG 方案：组件层用 gpui-component 的 `Icon`（`IconName` 枚举或 `Icon::default().path("icons/…")`），运行时经 `ExtrasAssetSource`（steer 本地 svg 优先 → `gpui-kit-assets::AllAssets` 全量 Lucide）解析。

chrome 壳自有图标表在 `steer-agent-chrome-ui/src/theme/icons.rs`：`IconAsset(pub &'static str)` 常量即 svg 资产路径（Lucide 名），宏同时生成常量与 `ALL` 列表；`theme::icon(glyph, size)` 返回 `gpui_component::Icon`，且 `IconAsset` 实现了 `IconNamed`（常量可直喂 `PopupMenuItem::icon` / `Button::icon`）；颜色继承祖先 `text_color`。守护测试遍历 `ALL` 断言每条路径在嵌入 bundle 内可解析 + 路径两两不重复（上游改名测试即红）。旧 codicon 字体方案（codicon.ttf + `FONT_ICON`）已退役。

#### BrailleSpinner

Text-based braille-dot spinner cycling through `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` (10 frames, 800 ms cycle). Used wherever an in-progress indicator is needed, replacing the old rotating-circle `Spinner`.

> Source: `crates/ai-elements/src/spinner.rs`

#### PixelSpinner

VS Code session-status 的像素 spinner，gpui 原语复刻（`ai_elements::PixelSpinner`）：16px 框内六颗 2px 圆点。`Grid` 变体是下落级联（−4px 淡入 → 原位停留 → +7px 坠出，逐点延迟 520/650/260/390/0/130ms，周期 1820ms，Cycle/Long/Short 三种停留时长）；`Ring` 变体是等待输入态（六点压 0.25 透明度，高亮按 200ms 一步顺时针扫一圈，周期 1200ms）。全部动效由单个 `Animation` 时钟驱动：周期时间折算过每点延迟后查关键帧表，每个移动分段经 `steps(6/4, jump-none)` 量化——离散跳变即像素感，`smooth()` 换 linear 是对照臂。旋钮：`zoom`（1=原生 16px）、`speed`（速率倍率）、`sharp`（方点）、`synced`（相位锁 App 共享时钟，多实例同拍）、`max_fps`（限帧——多行复用时按原生跳变节奏 ~30fps 封顶）。侧栏 `Running` 状态位复用本组件（ACCENT 色 + `.synced()`，运行行同拍）；早期 `session_list::pixel_grid` 近似（整周期量化 + 12fps 节流）已删除。

> Source: `crates/ai-elements/src/pixel_spinner.rs`。目检面：`cargo run -p ai-elements --example gallery` → Pixel Spinner（1×/8×、ring、¼ 速慢放 + linear 对照）。

#### Meerkat

The conversation-loading sprite (`ai_elements::Meerkat`): a 12×14-cell pixel meerkat played as a 6-slot loop (idle → bob → blink → bob → idle → tail flick) at 3 slots/sec — the face the main column shows while a reopened thread's chat snapshot is in flight (see the history-loading page above). Sprite data is character-row grids parsed to one div per solid cell, all under one gpui `Animation` clock: cycle time picks the slot, the slot picks the frame, and only the active frame renders (the pre-component page stacked six full-frame layers and opacity-toggled between them). Knobs: `cell` ([`NATIVE_CELL`] = 10 px, the page's scale), `speed`, `freeze(frame)` for a static render. Consumed by `agent-ui/src/views/history_loading.rs`; acceptance surface is the gallery's History Loading page.

> Source: `crates/ai-elements/src/meerkat.rs`

#### Tool

The tool-call card frame (`ai_elements::Tool` / `ToolHeader` / `ToolState`): icon + tool name + mono argument summary + status chip over a collapsible body, mirroring Vercel AI Elements' Tool triple. Lifecycle contract (the Reasoning arrangement on tool vocabulary): a `Running` edge opens the body, a settle folds it one second later (`AUTO_FOLD_DELAY`), a hand toggle pins it, `no_auto_fold` hands the clock to a host that owns the fold elsewhere — the conversation's arrangement (see [ActivitySegment](#activitysegment)). Always-visible copy chips at the body's top-right quote the raw call — `in` = input JSON, `out` = raw output, `diff` for the edit family — pressing, landing on the clipboard, and flashing a check for 1.2 s (state in `ToolState`, so a host must observe the entity for the flash to repaint). `open_diff(label, on_open)` rides the same row as one more chip (no flash — nothing lands): a card whose body is a capped view hands the full view to a host callback (the app opens the right pane's diff tab); `label` is host-localized, the crate injects no copy. The header click can be rerouted with `on_toggle` (the conversation routes it back through its own fold). Selection copy is WYSIWYG: the body rides the rich-text core (see [CodeView](#codeview) / [ShellView](#shellview)).

> Source: `crates/ai-elements/src/tool.rs`. Mount point: `crates/steer-agent-chat-ui/src/views/tool_cards.rs`.

#### CodeView

The editor mock for file-shaped bodies (`ai_elements::CodeView`): line-number gutter, diff markers, syntax palette, all rendered **in-band** through the rich-text core — gutter, markers, and code are one text document, so drag-select copies what the card shows. Shapes: a whole-file read numbers from 1 (`CodeView::source`); a ranged read keeps the file's own numbers with a `...` gap row; an edit hunk widens to the old/new gutter pair, diff rows carrying their added/removed color whole with a run-background tint (the one surface tint). Long lines soft-wrap at compose time (`DEFAULT_WRAP_COLUMNS`, overridable `wrap_at`) hanging continuations under the code column — the core has no nowrap and wraps at the block's left edge, under the gutter, which no editor does; the budget must stay under the block's real pixel width or a highlight run crosses the core's internal wrap point and panics. `compose` is a pure function pinned by invariant tests (runs disjoint/ascending/within lines; continuation prefix = the code column).

> Source: `crates/ai-elements/src/code_view.rs` (fence composition shared with [ShellView](#shellview) via `fenced`).

#### ShellView

The fake terminal for command-shaped bodies (`ai_elements::ShellView`): host-supplied prompt context lines (zsh-style — path, git branch, toolchains; the component has no opinion about prompt formats), a `❯` command echo, span-tinted output (`ShellLine::hit` raises grep needles; an empty needle degrades to the located line — the unguarded scan would never advance), and an exit footer (success = exit 0; failures carry no invented code; a streaming call runs). In-band like [CodeView](#codeview), on a fixed dark palette — a terminal surface stays dark under either app theme. `Bash`, `BashOutput`, `Grep` and `Glob` speak it; Grep/Glob render in shell vocabulary (their command equivalent), never as shell tools.

> Source: `crates/ai-elements/src/shell.rs`

#### AskUserQuestion

The ask card as a paged form (`ai_elements::AskUserQuestion` driven by `AskState`): one question per page, radio circles for single-select blocks, checkboxes for multi-select, a note input per question (lazily mounted — an input state needs a window), and a `< 1/3 >` pager whose three controls are identical plain glyphs (the submit check included — the last page's forward edge). `advance` past the last page settles the card (`AskSubmitted` event) and swaps the form for the host's markdown `.output(...)`, which the settled header folds (chevron + click). Strings are the host's (headers, badges, placeholder, footnote); a block's `detail` element is the host's too (the crate renders no markdown of its own); the card's page is clamped against the blocks it received. A host that owns the answer elsewhere overrides the pager's check with `.on_submit(...)` — the state stays a working form until that host drops it — and adds the header's dismissal control with `.on_close(...)`, rendered while the card is unsettled only (a settled card is a transcript row, not a surface to leave). Both hooks are driven by the live conversation card ([AskDrawer](#askdrawer)); the gallery exercises the default path.

> Source: `crates/ai-elements/src/ask.rs`. Acceptance surface: `cargo run -p ai-elements --example gallery` → AskUserQuestion; the live face is [AskDrawer](#askdrawer).

The state also owns the card's `FocusHandle`: a host mounts `track_focus` +
`tab_index(0)` on the card and moves focus into it, which is the only way its
own pager/dismissal bindings (the `AskDrawer` key context) can fire at all —
gpui dispatches actions along the focus path.

#### ScrollHandle / ScrollableElement

Scroll container with custom scrollbar.

#### TitleBar

Standard title bar component from gpui_component.

#### ContextMenu

Right-click context menu.

#### Tooltip

Hover tooltip.

---

## 8. Access Modes

Two file-effect tiers drive the seatbelt profile (bash) and the fs write fence; a denied call carries a `[sandbox: …]` marker and a `require_escalated`+`justification` escalation path through the `ToolCallAuthorization` card. Switching is hot: a mid-turn switch governs the very next tool call, and the [AccessSwitch](#accessswitch) flips optimistically on click (`/mode` also dispatches immediately mid-turn). The tier colors below also tint the user turn frame at send time.

#### Sandbox access

Green — file writes fenced to the workspace, the manox home (~/.manox), and temp areas; bash rides the seatbelt profile (default). The network is always on: there is no user-facing network switch — the app seeds the `networkAccess` config (`on`, AHP session-mutable, memory-only — a restored session comes back closed, so every attach re-seeds it idempotently); sandbox egress runs direct when no allowlist is configured, and through the host's managed proxy + domain confirm cards when `~/.manox/settings.toml [sandbox.network] allow` is configured.

#### Full access

Red — no sandbox; bash unsandboxed, fs mutations unfenced.
---

## 9. Tool Call Statuses

States of a [ToolCallCard](#toolcallcard).

#### PendingApproval

Waiting for user, surfaces as the [AskDrawer](#askdrawer) form card.

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
