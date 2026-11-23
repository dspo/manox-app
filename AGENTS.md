# AGENTS.md

Guidance for coding agents working in this repo.（本文件是 manox-app 仓库指引的唯一权威；CLAUDE.md / CLAUDE.local.md 仅为指向本文件的入口，不承载内容。）

## 项目概述

manox-app（应用品牌 **Steer**，应用侧 crate 一律 steer-* 前缀；仓库名保持 dspo/manox-app）是 **GPUI 桌面应用仓**：完整的应用（窗口、UI、终端、webview、系统托盘）与 cx 启动器（外部 agent CLI）。agent runtime（harness/agent/session-core/providers/supervisor，外加回流本仓消费的 hyperlinks）来自 **dspo/manox**，经 git 依赖（tag 锁发布，见「与 manox 仓的联动开发」）+ 提交的 Cargo.lock 消费。单二进制、单进程，外加一个特许 sidecar（`pi-harness` 编译出的 `pi-acp`，见「pi harness」一节）。逐文件架构靠读代码获得——本文件只承载不可从代码推导的约束。

### 代码结构

```
crates/                    # 全部 workspace 成员平铺于此（cargo 交付物：桌面 app + 独立 bin 的 cx CLI）
  steer-app/              # 主二进制入口（应用品牌 Steer；crate steer-app，bin steer；窗口 + 主题 + 托盘 + 接线）
  agent-ui/                # 状态 + 装配层（multiplexer/client_store/browser_host/
                           #   dispatch/侧栏投影）+ 会话列（Workspace 的状态机与渲染）
  steer-agent-chrome-ui/   # 唯一壳 crate（2026-Light 令牌/SessionList/右栏 ToolTab/
                           #   底部 dock/工具栏/MainSurface 槽；无数据源，example shell
                           #   为目视验收面，装配在 agent-ui::chrome_assembly）
  steer-agent-chat-ui/     # 会话列的状态机与消息管线 + pi 数据面（pi/：转录 reducer、
                           #   会话状态、进程运行时与事件折叠；chat crate 依赖面见
                           #   script/check-chat-crate-deps.sh）
  steer-canvas/            # 画布模型（Container → canvas → sheet）：sheet id 与布局树、
                           #   每容器画布状态机（单写者 set_layout + ticket、park LRU）、
                           #   布局持久化、SheetKindSpec registry；除 registry 的视图工厂外
                           #   一律 gpui-free（见「画布模型」一节）
  steer-ghostty/           # 终端面（libghostty-vt 仿真核静态链接在 GPUI element 后；
                           #   唯一终端实现，见「Ghostty 终端」一节）
  steer-term-process/      # 终端进程缝（TerminalProcess/ProcessOutput；gpui-free，
                           #   cx/cx-cli 经 steer-ext-agents 消费而不拉 GUI 栈）
  steer-acp/               # pi harness 的 ACP 客户端缝（spawn、ndjson JSON-RPC、
                           #   session 生命周期、updates/requests 双通道；gpui-free）
  ai-elements/             # agent 显示语义组件（Reasoning/…），对齐 Vercel AI Elements 的语义
  steer-components/        # app chrome 与基础渲染件（markdown、TerminalPanel、TurnFrame）
  steer-i18n/              # app chrome 本地化（Fluent 栈 + locales/{zh-CN,en}.ftl，无 gpui 依赖）
  steer-paths/             # 状态根权威（`~/.steer` 与 `~/.steer/.manox` 的分层、
                           #   `MANOX_HOME` 注入与一次性迁移；无 gpui 依赖）
  steer-updater/           # 应用内自更新（GitHub Releases 检查 + dmg 校验下载 + macOS 原地
                           #   覆盖安装 + UpdateStore 状态机；仅 steer-app 消费，seam 外编译为拒绝桩）
  steer-webview/           # wry 原生 webview + Tauri 式 IPC
  steer-webview-macros/    # webview IPC 过程宏
  cx/                      # cx headless 库（配置核心、launch-home、
                           #   chatgpt/vscode launch、probe db）
  cx-cli/                  # cx CLI bin（clap + ratatui TUI + relay + stats + `cx web`）
  steer-ext-agents/        # ext-agent 启动 API、会话管理、IPC relay、
                           #   cx_process 桥（SessionHandle → steer-term-process 的 TerminalProcess）
```

`pi-harness/` 与 `crates/` 平级但**不在 cargo workspace 里**：它是 pi 引擎的
bun/TypeScript 工程（含自带的测试、构建脚本与 ACP 烟测），构建产物是自包含的
`dist/pi-acp`。为什么它不在 `crates/`：版本、工具链与依赖解析（bun.lock）全部独立，
随包分发的形态也不同（见「pi harness（sidecar）」一节）。

组件层边界：`ai-elements` 承载 agent 对话语义（Reasoning、工具、消息流），渲染机械
（markdown、终端输出）归 `steer-components`。`ai-elements` 不依赖
`manox-agent`/`agent-ui` —— 正文与本地化文案由调用方注入，所以组件级验证走
`cargo run -p ai-elements --example gallery`。

**代码表面 A/B 契约**（`steer-components::code_surface` 共享行模型/度量/解析器，组合策略刻意二分，勿合并）。**契约只规定呈现差异**：
对话正文的代码内容走**效果化**——markdown 代码栅栏渲染为编辑器面板（**行号 gutter 是编辑器形态的硬需求**，也是走自定义 block 的唯一理由；带 `@@` 的完整 patch 默认 side-by-side 分栏、含冲突标记的栅栏默认 3-way，header 可切回平铺；hunk-less 的裸 `-/+` 片段不解析、保持平铺），svg/mermaid 默认渲染为图（翻面到编辑器面板源码）；工具输出的正文走**忠实字面**——Tool 卡的 CodeView/ShellView 与终端面板保持 in-band 单文档（unified diff、不分割不合并），高亮保留（naive 行级分词器）。
**已知限制（实现约束，非产品选择）**：面板行进不了窗口级选区——面板内逐字拖选不可用，跨面板拖选以整块栅栏源码参与选区，块内复制走 header 的 copy 控件；in-band 面的拖选所见即所得。
**维护提示**：编辑器面板复刻了上游 CodeBlock 的 padding/border/语言角标抑制与滚动约定——gpui-kit 每轮升级须与 in-band 三面（CodeView/ShellView/终端面板）双处核对。两侧性质各有测试钉住（steer-components `code_block_panel`、ai-elements `code_view_selection`）。

上游 manox 仓的拆分点：tag `pre-manox-app-split`；涉及 runtime/协议/journal 的改动在 dspo/manox 提 PR，本仓经 `cargo update -p <crate>` 拾取。终端仿真已随 Ghostty 迁移退役（上游 manox-terminal 一并退役，见 dspo/manox#996），`CxProcess` 桥留在本仓 steer-ext-agents。

### 交互协议 v3 = AHP（2026-09-28 切换）

manox ↔ app 的交互协议是 **AHP（Agent Host Protocol）channel 化**（dspo/manox#818 删除 manox-protocol）：journal 仍是唯一 durable 权威，宿主把 journal 折叠成 AHP 通道状态，本仓退化为纯 AHP 客户端。规范唯一事实源在 dspo/manox 的 `docs/ahp-architecture.md`（分层/通道/不变式见 §A–§C；journal 单格式、无版本轴，读面见 §C.3）。

- **数据面**：`crates/steer-agent-chat-ui/src/ahp_store.rs`（`AhpStore`：一个 `ahp::Client` 挂宿主进程单例的 in-proc 腿，折叠 root/session/chat/extension 通道，类型化写面）+ `transcript.rs`（`ChatState` → 转录节点的纯投影）与 `conversation.rs`（投影节点的 keyed 行状态）。视图只许读 AhpStore；v2 wire 词汇（`manox_protocol`/`FromClient`/`ClientCall`/`StreamFrame` 一类命名）不得在本仓重现——上游已删除 manox-protocol，这条从此靠评审守，不再有脚本门禁。
- **生命周期**：`agent-ui/src/multiplexer.rs` 只管 attach（订阅）/focus（GW5）/create-fork 命令缝；unread 是协议状态（`session/isReadChanged`，AhpStore 折叠），不再有本地镜像；不再有 per-session 线程泵。
- **客户端 SDK**：crates.io `ahp`/`ahp-types` `=1.0.0` 精确 pin（与上游 workspace 一致）；x-manox 扩展通道的 fold 与声明常量经上游 `manox-ahp::ext`。
- 已知降级（接 x-manox 通道未建模的部分）：UI 本地注释卡不持久、终端走内核 PTY 直连不走 AHP terminal 通道。
- unarchive（整树复活，上游 #987 的 parent 边）：数据面就绪且经测（`revive_container`）；UI 入口待归档分区面（挂账——侧栏今日不渲染归档行，分区面落地即销账）。

### 画布模型（crates/steer-canvas，PLAN-ABSORB-03）

容器持有画布、画布是布局树、叶子是 sheet。本 crate 是**状态层**（渲染面在 PLAN-ABSORB-04）。不可从代码推导的约束：

- **sheet 的存在由 registry 决定**，布局只决定放在哪：`SheetKindSpec` 在装配点注册（kind 二次认领 panic 即 fail fast），`body` 是 `AnyView` 工厂——kind 的状态必须住自己的 store（切 tab 会卸载 body），不得驻留在视图里。两种形态：`body: Some(factory)` 由 kind 自己造视图；`body: None` 由画布按 kind 分派（`session:` 会话列与 `pi-session:` pi 会话走这条）。
- **布局只有一个写者**：`CanvasState::set_layout(container_id, next, ticket)`。手势先 `take_ticket()` 再提交；过期票（含「该画布已切走」）静默丢弃。`sheet_count == 0` 返回 `AutoArchive(cid)`，归档动作由调用方走 workspace 的 teardown 面——状态层不做跨实体写。
- **park 有界**：切走的容器布局进 held（LRU 至多 4 份），被显示的容器永不留在 held；scratch 画布（`SCRATCH_CONTAINER`）不 park、不持久化，布局留在内存，铸成正式容器时 `migrate_scratch` 平移。
- **容器标题是投影**：`container_title` 取创建序中最早存活 sheet 的标题（跳 `home:`；`names_container = false` 的 kind 由调用方返回 None 跳过），无 → None，调用方落 cwd 尾段。
- **持久化**：`~/.steer/layouts/{percent-encoded cid}.json`（`{containerId, layout, zoom, updatedAt}`，temp 文件 + 换名原子写）；损坏文件 warn + 丢弃 → 默认画布（激进纪律：不修复不兼容读）；400ms 写防抖在调用方（保存泵）。
- **gpui 边界**：`id`/`layout`/`state`/`persist` 不依赖 gpui，只有 `registry`（默认开、可关的 feature）拉 gpui；`script/check-chat-crate-deps.sh` 同时守「`--no-default-features` 树里无 gpui」与「canvas 不依赖 chat crate」。

### pi harness（sidecar）

pi 引擎以**特许 sidecar 第二二进制**形态分发：`pi-harness/`（bun/TS，仓根，不在
cargo workspace）用 `bun build --compile` 产出 `dist/pi-acp`，与 manox 的 in-proc
腿平级，经 ACP（ndjson JSON-RPC 2.0 over stdio）与宿主通信。协议权威是
`pi-harness/src/acp/app.ts`；宿主侧只读缝在 `crates/steer-acp`（不拉 GUI 栈）。

- **构建**：`cd pi-harness && bun install --frozen-lockfile && bun run build`。
  build 先跑 `check-version`（`package.json` 的 pi pin == `src/version.ts`），再编译
  host-target 二进制，最后把三件伴生物放进 `dist/`：`photon_rs_bg.wasm`、
  `quickjs.wasm`、`skills/`。**四件不可分**——编译后的 pi-acp 从自身目录读 wasm 与
  skill 树，少一件就是运行时缺件（打包脚本对缺失直接失败）。
- **分发布局**：macOS `Steer.app/Contents/Helpers/`（pi-acp 与 cx 一样先单独
  codesign，再签外层 bundle；wasm 与 `skills/` 实体在 `Contents/Resources/pi-harness/`，
  Helpers 下只留指向它们的 symlink——codesign 视嵌套代码位置里的普通文件/目录为
  未签名的代码对象），deb/rpm `/usr/lib/steer/`；`steer_acp::resolve_pi_acp()`
  是解析唯一入口（`STEER_PI_ACP` 覆盖 → Helpers → `/usr/lib/steer` → 仓内
  `pi-harness/dist`）。
- **env**：宿主注入 `ZU_STATE_DIR=<steer_home>`（settings/subagents 的读根）；
  不注入 `MANOX_HOME`（两个 harness 的状态根不混用）；`PI_CODING_AGENT_DIR` 保持
  缺省（pi 的 `~/.pi/agent`，与官方 CLI 共存），仅测试隔离时覆盖；
  `ACP_NO_EXTENSIONS`/`ACP_EXTRA_EXTENSION_PATHS` 由父环境透传（测试缝）。
- **互操纪律**：stdout 只走协议（日志一律 stderr）；client 关闭 stdin = 优雅退出
  （5s 宽限后才杀进程组）；`session/load` 的 replay 帧先于响应到达（steer-acp 在
  调用期内捕获并随响应返回，不竞速）；崩溃时所有 pending 请求以 stderr tail 失败。
- **门禁**：CI 的 `pi_harness` 腿跑 `typecheck → bun test → build → smoke`（离线
  smoke 不调模型）；真模型验收用 `bun run acceptance:live` 或
  `ACP_SMOKE_LIVE=1 bun run smoke`；`cargo test -p steer-acp -- --ignored`（
  `MANOX_RUN_LIVE=1`）对真实二进制跑握手/new/close 烟雾。

### 与 manox 仓的联动开发

上游走双轨发布（当前线：修复进 `release/0.5` 分支打 `v0.5.x` tag，新特性进 main 打下一轨 tag；以 dspo/manox 的分支与 tag 实存为准，勿按旧轨名 bump）；本仓的 8 个 runtime git 依赖（manox-agent/harness/session-core/providers/supervisor/hyperlinks/manox-ahp/manox-ahp-runtime）共用同一个 rev/tag 声明。

```bash
script/local-manox.sh on            # git 依赖 → ../manox 本地路径（.cargo/config.toml，已 gitignore）
script/local-manox.sh off           # 还原纯 git 依赖（CI 形态）
# 拾取上游新发布（patch off 时）：改 Cargo.toml [workspace.dependencies] 的 tag= 值，
# 然后 cargo update -p manox-agent（全部 8 依赖随同一 tag 整体移动）
```

runtime 侧 API/行为回归优先在 dspo/manox 修；只有装配/接线问题在本仓修。状态根分层（PLAN-ABSORB-01）：应用层（Steer 平台）在 `~/.steer/`（`settings.toml` 存 `ui_language`、`projects.toml` 存项目注册表，`steer-paths` crate 是路径权威，`STEER_HOME` 可覆盖）；manox harness 层在 `~/.steer/.manox/`——两个 bin（steer/cx）启动早期（任何线程与 manox 路径解析之前）调 `steer_paths::install()` 注入 `MANOX_HOME` 并把旧 `~/.manox` 一次性 rename 进来，runtime 代码零改动（路径清单见 dspo/manox 仓 AGENTS.md）。

单实例：`~/.steer/instance.lock` 以非阻塞 `flock` 做唯一权威（进程退出内核自动释放，不留死锁）；第二实例先经 AppKit 把运行中的实例带到前台再自退（macOS），Linux 只保证不重复启动（`crates/steer-app/src/instance.rs`）。

### 跨仓配对修复的消费义务

上游 PR 的 body 或 commit message 点名 dspo/manox-app 需要伴随改动的（「Companion change required in dspo/manox-app」及同类表述），其消费义务落在把锁推进到该发布的那个本仓 PR：**同一 PR 内完成订阅/换道/接线，或在 Assumptions 里显式申报滞留原因**——静默不消费等于上游白修（桌面端撞不到修复）。台账只列**未结清**义务（销账即删行；历史销账记录走 git 历史与 PR 描述，不驻留本文件，禁止「本 PR」类随时腐烂的指代）：

- pi distro 版本跟进（`pi-harness/package.json` 的 `@earendil-works/pi-coding-agent` pin）：升级 = 独立小 PR，同步 `pi-harness/src/version.ts`，重跑 `typecheck / bun test / build / smoke` 与 `MANOX_RUN_LIVE=1 cargo test -p steer-acp --test live -- --ignored`。

**依赖 pin 纪律**：任何 pin 变更（manox tag、gpui-kit、rust channel、Ghostty rev/zig、pi distro、lock 级 pin）= bump PR 必须先读 `docs/upstream-watch.md`，对每个 **open** 项在**新版本里实测核对**（不能只读文档），在台账「核对记录」补一行（日期、旧→新、逐条 missing/satisfied/changed/unchecked），PR 描述带 `upstream-watch: YYYY-MM-DD checked, <n> open → <n> open`。**没有核对视为 bump 未完成**。

## 构建与开发命令

```bash
cargo build                          # debug 下 gpui 依赖需 opt-level=3，否则渲染极慢
cargo run                            # 桌面应用（chrome 壳，唯一）
cargo run -p cx-cli                  # cx CLI
cargo test                           # live 测试用 MANOX_RUN_LIVE=1 env 门控，默认安全
cargo clippy --all-targets
cargo fmt --all
```

pi harness（`pi-harness/`，bun 工程；工具链走 `.github/actions/bun` 的 pin，本机需同版本 bun）：

```bash
cd pi-harness && bun install --frozen-lockfile   # 依赖（bun.lock 是唯一解析）
cd pi-harness && bun run typecheck               # tsc --noEmit（src + scripts）
cd pi-harness && bun test                        # 263 项单测
cd pi-harness && bun run build                   # dist/pi-acp + 两个 wasm + skills/
cd pi-harness && bun run smoke                   # 离线 ACP 烟测（不调模型）
MANOX_RUN_LIVE=1 cargo test -p steer-acp -- --ignored   # 对真 pi-acp 的握手/new/close 烟雾
```

Rust **1.97.1**（`rust-toolchain.toml`），edition **2024**，需 `clippy`/`rustfmt`/`rust-src`。Linux 系统依赖的**唯一清单**是 `.github/actions/linux-deps/action.yml`（Fedora 侧对应 `.github/actions/linux-deps-fedora/action.yml`，两份声明同一包集合；集成浏览器在 Linux 编译排除，故两份都不含 GTK/WebKitGTK；改一处必须同步另一处）；deb/rpm 另声明 `bubblewrap`——Linux 沙箱的 bwrap 地板，缺失即退化为逐调用审批。

**libghostty-vt 的编译期前置（`crates/steer-ghostty`）**：该 crate 的 build script 静态链接 libghostty-vt，源码与工具链不在 Cargo 依赖图里，由三种输入之一提供——
- **Ghostty 源**：`GHOSTTY_SRC=<本地 checkout>`（开发环，zig 自身 cache 使其增量；每次 build script 都重跑 zig），否则下载 `build.rs` 里 pin 的 rev tarball（sha256 校验）。
- **zig**：`GHOSTTY_ZIG` → PATH 上 ≥ 0.17 的 `zig` → 下载 pin 的 zig 工具链（sha256 校验）。
- **产物缓存**：默认落在 `OUT_DIR`，`STEER_GHOSTTY_CACHE` 可改址（CI 经 `.github/actions/ghostty-vt` 恢复 `~/.cache/steer-ghostty`，**key 里的 rev/zig 版本必须与 `build.rs` 同步**）。缓存靠 stamp 判定命中：rev 或 zig 身份变化即重建，不会链到旧静态库。

因此**离线构建**必须预置缓存（或同时给出 `GHOSTTY_SRC` + `GHOSTTY_ZIG`），且解压用到 `curl`/`tar`/`xz`/`sha256sum`（`xz` 已进两份 linux-deps 清单）。换 Ghostty rev 的姿势是改 `build.rs` 的 `GHOSTTY_REV` + 对照 `include/ghostty/vt/*.h` 更新 `src/sys.rs` 的转写。

**Linux 无集成浏览器（编译期排除）**：wry 的 Linux 子 webview 只能经 Xlib 句柄挂载，Wayland 窗口无法承载嵌入式浏览器 —— 集成浏览器在 Linux/FreeBSD 上整体不编译（`#[cfg(not(any(linux, freebsd)))]` 盖住 `browser_host` / `browser_view` / `BrowserTool` / steer-webview 整个 crate，依赖面随 target gate 收紧，wry/WebKitGTK/GTK 不进 Linux 构建），UI 无入口，agent 的 `web_explore_*` 工具经 capability provider 得到干净的「not available」拒绝（剪贴板能力保留在 `capability_host`，全平台）。**Linux 窗口后端仅支持 Wayland**：`gpui_platform` 的 features 只开 `wayland`（不开 `x11`），X11 栈编译排除，X11/XWayland 会话无法建窗。历史注记：当年 X11 会话存在 Vulkan X11 WSI 死锁雷区（Fedora 44 mutter 50.4 + XWayland 24.1 + ANV 实测，`AcquireNextImageKHR` 冻结活跃重绘的 XWayland/X11 窗口，`VK_LOADER_DRIVERS_DISABLE=*` 可绕），这是收敛到 Wayland-only 的背景之一。托盘受宿主限制：无 StatusNotifierWatcher 的桌面（原生 GNOME）上 `tray::install` 刻意失败，应用保持「关窗即退出」而非隐形驻留。

### Release（tag 触发）

push 语义化版本 tag（`vMAJOR.MINOR.PATCH[-PRERELEASE]`）触发 `.github/workflows/release.yml`：三路构建并发布 GitHub Release——macOS arm64（`script/bundle-mac` 产 ad-hoc 签名的 Steer.app，再由 `script/bundle-dmg --skip-bundle` 打成 `.dmg`，zip 一并保留）、deb amd64（`script/bundle-deb`）、rpm x86_64（`script/bundle-rpm`，在 fedora 容器内构建 + `dnf` 冒烟安装）；tag 版本必须等于 `[workspace.package].version`（bundle 脚本以它为唯一版本源，发版前先 bump 再打 tag）。`workflow_dispatch` 可在任意分支跑同一管线做纯构建验证（不发布）。制品内均含 steer 与 cx 双 bin，以及 `pi-acp` sidecar 四件套（macOS `Contents/Helpers/`，deb/rpm `/usr/lib/steer/`；打包腿先跑 `bun install --frozen-lockfile && bun run build`，缺件即失败）。

### 出 UI 图（chrome 壳）

UI 静态图**从真实渲染出**，不要另画一套：`crates/steer-agent-chrome-ui/tests/visual.rs`
用 gpui 官方离屏渲染（`VisualTestAppContext` + Metal 回读）把**真实的 `Shell`** 截成 PNG，
不需要录屏权限，窗口在 (-10000,-10000) 渲染、不会闪屏。

```bash
CHROME_SHOT=/tmp/shell.png cargo test -p steer-agent-chrome-ui --test visual
CHROME_SHOT=/tmp/right.png CHROME_RIGHT=1 cargo test -p steer-agent-chrome-ui --test visual   # 展开右栏 + 一个 dummy 页签
CHROME_SHOT=/tmp/panel.png CHROME_PANEL=1 cargo test -p steer-agent-chrome-ui --test visual   # 展开底部 dock
CHROME_SHOT=/tmp/sw.png CHROME_RIGHT=1 CHROME_SWITCH=1 cargo test -p steer-agent-chrome-ui --test visual  # 两个不同宽度的页签 + 切回第一个
CHROME_SHOT=/tmp/sidebar-canvas.png CHROME_CANVAS=1 cargo test -p steer-agent-chrome-ui --test visual  # 侧栏画布面：容器标题 + sheet 数 + 选中竖条
CHROME_SHOT=/tmp/canvas.png CHROME_CANVAS=1 cargo test -p agent-ui --features test-support --test visual_canvas  # 真装配的画布（两 tab），CANVAS_SPLIT=1 加分屏
```

- 画布出图在 **agent-ui** 侧（`tests/visual_canvas.rs`）：chrome 的 harness 装不进真 Workspace，所以那里挂的是真 `chrome_assembly::mount`；场景经生产手势落地（`focus_conversation` + `diagnostic_bind_store` + 分屏），标题由 seed 的 catalogue 行解析，不重画任何视图。

- 诊断开关 `CHROME_SWITCH=1`：开第二个（更宽的）页签、等布局稳定后切回第一个——**这是唯一能触达页签指示器滑动动画的路径**，单页签出图看不到「从哪来」。
- **动画类改动注意**：离屏测试**没有平台帧循环**，`request_animation_frame` 排的回调只有经 `Window::simulate_next_frame` 才会送达；只 `run_until_parked` 的话动画永远停在第一帧（`delta=0`）。settle 循环已补一帧，但**要看中间帧必须自己 pump**；出图只反映终态。
- **页签指示器的不变量**（原 `script/check-tab-indicator.sh` 已删，改成出图时人工核对）：① 对齐量**左端**而非宽度——曾经宽度/高度全对却整体右偏 6px，任何宽度或 y 断言都看不见；正确位置是 rail 左端 + 8 device px（`STRIP_PL` 4 logical @2x）。② 可见厚度必须是 4 device px（`TAB_INDICATOR_H` 2 logical），而指示器必须是 strip 的**兄弟节点**（放在 relative wrapper 里）——gpui 在子节点之后绘制父级 border，做成 strip 的子节点会被盖掉一半。
- 产物是 **1280×820 @2x = 2560×1640** 的 PNG（约 300KB）。
- **macOS only**：Metal 纹理回读，其它平台打印一行跳过；**未设 `CHROME_SHOT` 时同样跳过**，所以在 CI 里安全（门禁不受影响）。
- 侧栏铺的是真实 thread store（`refresh_thread_list` 一次同步扫描），所以图里是**真实会话数据**，不是占位。
- 页签内容用 dummy tab：PTY 会触发 gpui 的泄漏检测，wry subview 在离屏下读不回来——需要终端/webview 真内容时改用 `cargo run` 看。
- 改 UI 后**重跑截图**比描述更可信；图与代码不会漂移，因为图就是代码画的。**不接受**用脚本手绘 SVG 复刻一套 UI（已退役，见 git 历史）：那是同一套 UI 的第二种表达，必然与真实渲染分叉。

## 工具链 & Skills

涉及 GPUI/UI 开发时，先通过 Skill 工具加载 `.claude/skills/` 下的 skill（该目录不托管进 git，只在本地存在）：
- `gpui` — GPUI 框架（Entity/Render/actions/keybindings/async/layout）
- `gpui-component` — gpui-component 组件库（Button/Input/List/Sidebar 等）
- `gpui-component-dev` — 为 gpui-component 贡献新组件时额外加载

另有唯一随仓库走的 skill：`.agents/skills/gpui-kit-follow`（gpui-kit 版本跟进/升级方法论，含手搓热点表）——评估 gpui-kit 新版本、bump gpui 依赖、或调研组件库新特性能否替代手搓实现前加载。

## GPUI 依赖版本锁定

GPUI 栈整体走 **longbridge/gpui-kit 轨**（crates.io 发布），**不再直接依赖 zed-industries/zed**：`gpui`/`gpui_platform` 是 `gpui-pre`/`gpui-pre-platform` 的包名别名——Longbridge 每周从 zed main republish 的快照（`[lib]` 名保留，`use gpui::*` 不变），每个版本自带 zed commit 映射（crate description / `[package.metadata]`）可审计。组件层 `gpui-component` 与资产 `gpui-kit-assets`（原 gpui-component-assets）同线 semver。全部 `=` 精确 pin，防周更快照漂移。

升级姿势：`cargo update -p gpui-pre --precise x.y.z`（与组件版本按 gpui-kit lock 对齐）→ 重跑全部门禁；gpui-pre 的 `[[patch.unused]]`/duplicate 用 `cargo tree -d | grep -i gpui` 验证单一版本，并人工确认没有任何 Cargo.toml / `.cargo/config.toml` 重新引入 `git = "...zed-industries..."`（原 `script/check-no-zed-git.sh` 已从 CI 移除，这条改由评审守）。gpui 相关依赖在 debug 下仍需 opt-level=3（`[profile.dev.package] gpui-pre*`）。历史注：旧栈（gpui@zed-1d217ee + gpui-component@longbridge-git）已退役；gpui-pre ≥0.3.4 含上游 gpui#60295 element-arena 修复。

版本跟进/升级的完整流程（跟进触发面、评估三问、整列 runbook、adoption 跟进与手搓热点表）见 `.agents/skills/gpui-kit-follow/SKILL.md`。

## 提示词与 i18n

本仓是 i18n 的**唯一归属地**：`crates/steer-i18n` 自带 Fluent 栈与全部资源（`crates/steer-i18n/locales/{zh-CN,en}.ftl`），中文为第一语言（primary + 默认），英文为次（fallback）。调用经 `agent_ui::i18n::t("key")`（gpui 层 `SharedString` 包装）或 `steer_i18n::t`（无 gpui 依赖的层）。

**i18n 只覆盖 app chrome**：侧栏、设置面板、菜单栏、托盘、About、终端 overlay 等本仓自产的界面文案。

**manox runtime 传入的值一律原样渲染，禁止二次本地化**——工具 title/summary、slash 命令 description（runtime 给英文）、plan 文本、`AskUserQuestion` 的 header、模型产出内容。新增 UI 文案 = 在 `crates/steer-i18n/locales/` 两个 `.ftl` 各加一个键（缺一不可，parity 由单测守护）+ 调用处换 `t("key")`。

模型面向字符串（提示词模板、工具 description）一律英文且**不在本仓维护**：多段落提示词散文全部在 dspo/manox 仓，本仓不得内嵌。

语言配置 `ui_language` 由本仓拥有（`steer_i18n::{load_ui_language, persist_ui_language}`，读写 `~/.steer/settings.toml` 中该键且只碰该键；首次触达时从旧 `<manox_root>/settings.toml` 摘出该键并删旧键）；`agent_language` 键已随 manox 侧语言轴退役而废除，不再有意涵。

## 工作流约定

- **唯一壳（2026-09-28 旧壳退役）**：窗口只有 `agent-ui::chrome_assembly` 这一个装配
  （chrome crate 的 Shell + 投影侧栏 + 右栏 ToolTab 注册表 + 会话列作为主区卡内容）。
  双壳构建开关（`--features chrome-shell`）与 `agent-ui::Workspace` 的旧全壳渲染已删净；
  上一个双壳形态留在 tag `dual-shell-final`（计划文档已随退役删除，历史见 git log）。
- **每 PR 门禁 = `.github/workflows/ci.yml` 的 `checks_pass`**（分支保护只 require 这一条；`no-coauthor.yml` 是第二条、事件集不同不能并入）：`guards`（`cargo fmt --check` + chat crate 依赖不变量 + chrome 边界不变量）→ `pi_harness`（bun：typecheck → bun test → build → 离线 ACP smoke）→ 两条并行 verify（linux amd64 `ubuntu-24.04` / macOS arm64 `macos-26`，各自 `clippy -D warnings --all-targets` → 一次 feature-inclusive 全量 `cargo test --features agent-ui/test-support`；`--all-targets` 已把 bin 编过，所以没有单独的 build 步，shipping 二进制由打包腿的 release 构建产出）。本地等价：`cargo fmt --all -- --check`、`./script/check-chat-crate-deps.sh`、`./script/check-chrome-boundary.sh`、`cd pi-harness && bun run typecheck && bun test && bun run build && bun run smoke`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace --all-targets --features agent-ui/test-support`；PR 描述贴出本地门禁结果。打包腿（`.deb` / `.dmg`）与 verify **并行**，但只随 main 推送与 `workflow_dispatch` 启用：PR 触发的 CI 恒为 verify-only，不跑 package（要打包某个 PR 分支就在该 ref 上 `workflow_dispatch`）——冷 release 构建不该躺在任何 PR 的必经路上；`.rpm` 只在 `release.yml` 出（Fedora 容器 + glibc baseline）。PR 写清 Test Plan 与 Assumptions。
- **合并前必须把 manox 依赖 bump 到最新兼容发布**：决定合并（含批准后的最后一步）前，在 `script/local-manox.sh off` 形态下确认 Cargo.toml 的 runtime tag 指向当前 release 线（release/0.5 的 v0.5.x，或 main 的下一轨）最新兼容 tag——8 个依赖共用同一 tag，改一处声明 + `cargo update -p manox-agent` 整体移动——重跑全部门禁，并把变更随合并一并提交。若最新上游与本仓不兼容，先在 dspo/manox 修出兼容发布再 bump，或在 PR 中显式声明滞留原因（旧 tag 停留是债务，不是默认状态）。
- **chrome/host 边界**（#279 的结论，`script/check-chrome-boundary.sh` 守）：`Shell` 是宿主状态的投影。
  gpui 的「一次一个实体租约」让**跨实体同步调用在回调里必崩**（panic 跨不过 macOS 事件的 `extern "C"`
  边界 → SIGABRT），而经 `WindowHandle::update` 绕行的写法在事件分发内会**静默丢命令**。四条不变量：
  ① chrome 状态只有一个写入者——`chrome_assembly::project`（workspace observer，effect 边界无租约）；
  ② chrome 不镜像可派生事实（侧栏高亮 = 渲染期 `HostHooks::active_row`，壳无本地字段）；
  ③ 宿主对 shell 的**动作**只经 `defer_shell`（`cx.defer` + 窗口 handle，落到 effect 边界）；同一次 update 内
  **先发命令、后 `cx.notify()`**——命令与驱动投影泵的 notify 共用 effect 队列、按入队序执行，先 notify 会让泵
  先完成 pane 的 stash/restore，随后的陈旧命令可能关掉刚恢复的 incoming tab；
  ④ hook / subscription 载荷自足，宿主不得在回调内触碰发起它的实体（launcher 的 ⏎ 另走延迟，构造上安全）。
  新增 chrome 写入先在 `chrome_assembly` 里找通道，**绝不**在调用点直接 `shell.update`；手势回归矩阵在
  `crates/agent-ui/tests/chrome_boundary.rs`（真实装配 + 真实租约）。
- 提交信息不得携带 Co-Authored-By 尾注（CI 强制拒绝）。

## 工作方法

源自 nsh-pi 的工作方法（PLAN-ABSORB-07 吸纳），无脚本门禁，靠评审与自觉守：

- **PR 编辑在独立 worktree**：`git worktree add $PWD.worktrees/pr_topic -b feat/pr_topic <PR目标分支>`（吸纳系列期间基分支为 `origin/integrate/absorb-nsh-pi`，其后回 `origin/main`）；合并后回收 branch 与 worktree。
- **先回应再动手**：收到指示后、第一次写操作前用一句话说清理解与打算，说完就继续，不等确认。
- **尽早交付 PR**：提 PR 后不傻等远端检查，静态 review 与本地/CI 验证并行。
- **review 别重复劳动**：GitHub CI 会验证的，review 时非必要不在本地再跑一遍。
- **review 必须给结论**：三选一 `approve` / `approve with changes`（处理意见后自合并）/ `request changes`。
- **不留 follow-up**：follow-up 当场处理——附进当前 PR，或立即开后续 PR，不等用户跟踪。
- **subagent 任务粒度**：10–60 分钟为宜；太小顺手做掉，太大拆阶段委派。
- **subagent 工作必须验收**：必要时要求返工；小瑕疵自己接手修掉。
- **UI 变更给目视验收指引**：PR 涉及 UI/UX 时描述里给运行命令 + 目视验收要点（与「出 UI 图」互补）。

## 项目规则

- **技术选型喜新厌旧**：能选最新 stable 就选最新 stable（依赖、工具链、API）。
- **禁止 vendor / submodule**：所有依赖经 Cargo 声明；上游 manox 仓一律 git 依赖，禁止改回 path 依赖提交。
- **crate 依赖只认 crate 索引或 git 地址**：外部 crate 只能是 crates.io 版本或 `git = "..."`，禁止 `path = "..."` 指向本机路径（CI 不可复现）；workspace 内部成员间 `path` 例外；本地联动只经 gitignored 的 `.cargo/config.toml` [patch]。
- **只允许单二进制、单进程交付**（桌面应用；cx CLI 为独立 bin）。唯一特许 sidecar：`pi-harness` 的 bun 产物 `pi-acp`（随三产物分发、由 `steer-acp` 按需 spawn，生命周期随会话/容器；不引入第三个二进制，也不为它开第二条交付面）。单实例由 `~/.steer/instance.lock` 的 `flock` 保证（`crates/steer-app/src/instance.rs`），不得引入其他 IPC/socket 形态。
- **禁止抄袭第三方 crate 代码**：可参考架构思想，禁止复制粘贴后修改。`git2` 被禁（plugin marketplace shell out 系统 `git`）。
- **注释一律英文，面向终态**（描述不变量/意图）而非过程流水账，非必要不注释。详见 `~/.claude/rules/code-comments.md`。
- **零构建告警**：CI 以 `-D warnings` 编译。提交前本地 `cargo clippy --all-targets -- -D warnings` 全绿。新增 `#[allow(...)]` 视为逃避而非修复，除非 lint 本身与项目设计冲突（如 GPUI 派生宏假阳性），且必须英文注释说明。`Result` 必须 `let _ =` 或 `?` 处理；test 模块在文件末尾。
- **重构 UI 后及时修订 `UI-MAP.md`**：任何 UI 组件层级、命名、增删重组的变更，必须在同一 PR 更新 `UI-MAP.md`。
- **勿以善小而不为**：对正面有效的 review 意见，即便不构成阻塞也应尽量遵从。

## 激进开发纪律

开发早期，不维护 v0→v1 升级路径，不背历史负债。运行时禁止 schema migration；不保留兼容字段/不写 fallback 兼容读；不写 `v0`/`legacy_`/`backward_compat` 模块（细则与 manox 仓一致）。

## cx / cx-cli 拆分边界（2026-09-10 随仓库拆分确立）

- `crates/cx`（lib）：headless —— provider/agent 配置核心、launch-home 隔离、codex 配置合并、ChatGPT.app / VS Code Claude 非交互 launch 与设置 API、probe 缓存 db。**不得引入 clap/crossterm/ratatui**。
- `crates/cx-cli`（bin）：交互面 —— clap CLI、ratatui TUI（launcher/stats/probe 面板）、relay、`cx web` 网关启动。经 `use cx::*` 消费 lib。
- Add-wizard 哨兵常量（`ADD_*_SENTINEL` 等）是 lib 与 CLI 的共享词汇，定义在 cx lib 并 pub。
