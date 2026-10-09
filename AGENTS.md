# AGENTS.md

Guidance for coding agents working in this repo.（本文件是 manox-app 仓库指引的唯一权威；CLAUDE.md / CLAUDE.local.md 仅为指向本文件的入口，不承载内容。）

## 项目概述

manox-app（应用品牌 **Steer**，应用侧 crate 一律 steer-* 前缀；仓库名保持 dspo/manox-app）是 **GPUI 桌面应用仓**：完整的应用（窗口、UI、终端、webview、系统托盘）与 cx 启动器（外部 agent CLI）。agent runtime（harness/agent/session-core/providers/supervisor，外加回流的终端仿真核心 manox-terminal 与 hyperlinks）来自 **dspo/manox**，经 git 依赖（tag 锁发布，见「与 manox 仓的联动开发」）+ 提交的 Cargo.lock 消费。单二进制、单进程。逐文件架构靠读代码获得——本文件只承载不可从代码推导的约束。

### 代码结构

```
crates/                    # 全部 workspace 成员平铺于此（本仓只有一个交付物：桌面 app + 独立 bin 的 cx CLI）
  steer-app/              # 主二进制入口（应用品牌 Steer；crate steer-app，bin steer；窗口 + 主题 + 托盘 + 接线）
  agent-ui/                # 状态 + 装配层（multiplexer/client_store/browser_host/
                           #   dispatch/侧栏投影）+ 会话列（Workspace 的状态机与渲染）
  steer-agent-chrome-ui/   # 唯一壳 crate（2026-Light 令牌/SessionList/右栏 ToolTab/
                           #   底部 dock/工具栏/MainSurface 槽；无数据源，example shell
                           #   为目视验收面，装配在 agent-ui::chrome_assembly）
  steer-agent-chat-ui/     # 会话列的状态机与消息管线（chat crate 依赖面见
                           #   script/check-chat-crate-deps.sh）
  terminal-ui/             # 终端渲染层（TerminalElement/TerminalView；仿真核心在 dspo/manox 的 manox-terminal）
  ai-elements/             # agent 显示语义组件（Reasoning/…），对齐 Vercel AI Elements 的语义
  steer-components/        # app chrome 与基础渲染件（markdown、TerminalPanel、TurnFrame）
  steer-i18n/              # app chrome 本地化（Fluent 栈 + locales/{zh-CN,en}.ftl，无 gpui 依赖）
  steer-webview/           # wry 原生 webview + Tauri 式 IPC
  steer-webview-macros/    # webview IPC 过程宏
  cx/                      # cx headless 库（配置核心、launch-home、
                           #   chatgpt/vscode launch、probe db）
  cx-cli/                  # cx CLI bin（clap + ratatui TUI + relay + stats + `cx web`）
  steer-ext-agents/        # ext-agent 启动 API、会话管理、IPC relay、
                           #   cx_session 桥（SessionHandle → PtySource）
```

组件层边界：`ai-elements` 承载 agent 对话语义（Reasoning、工具、消息流），渲染机械
（markdown、终端输出）归 `steer-components`。`ai-elements` 不依赖
`manox-agent`/`agent-ui` —— 正文与本地化文案由调用方注入，所以组件级验证走
`cargo run -p ai-elements --example gallery`。

上游 manox 仓的拆分点：tag `pre-manox-app-split`；涉及 runtime/协议/journal 的改动在 dspo/manox 提 PR，本仓经 `cargo update -p <crate>` 拾取。终端仿真核心（manox-terminal）与 hyperlinks 已回流 dspo/manox（同经 git 依赖消费），`CxSessionSource` 桥留在本仓 steer-ext-agents。

### 交互协议 v3 = AHP（2026-09-28 切换）

manox ↔ app 的交互协议是 **AHP（Agent Host Protocol）channel 化**（dspo/manox#818 删除 manox-protocol）：journal 仍是唯一 durable 权威，宿主把 journal 折叠成 AHP 通道状态，本仓退化为纯 AHP 客户端。规范与映射表唯一事实源在 dspo/manox 的 `docs/ahp-v3-architecture.md`（§G.W3 = 本仓切换、§F.2 = 本仓删除清单）。

- **数据面**：`crates/steer-agent-chat-ui/src/ahp_store.rs`（`AhpStore`：一个 `ahp::Client` 挂宿主进程单例的 in-proc 腿，折叠 root/session/chat/extension 通道，类型化写面）+ `chat_fold.rs`（`ChatState` → 显示词汇）。视图只许读 AhpStore（grep 门禁 `script/check-no-v2-wire.sh` 冻结 v2 词汇）。
- **生命周期**：`agent-ui/src/multiplexer.rs` 只管 attach（订阅）/focus（GW5）/unread/create-fork 命令缝；不再有 per-session 线程泵。
- **客户端 SDK**：crates.io `ahp`/`ahp-types` `=1.0.0` 精确 pin（与上游 workspace 一致）；x-manox 扩展通道的 fold 与声明常量经上游 `manox-ahp::ext`。
- 已知降级（接 x-manox 通道未建模的部分）：sub-agent 树、UI 本地注释卡不持久、终端走内核 PTY 直连不走 AHP terminal 通道。

### 与 manox 仓的联动开发

上游自 v0.1.0 起走双轨发布：修复进 `release/0.1` 分支打 `v0.1.x` tag，新特性进 main 打 `v0.2.x` tag；本仓的 9 个 runtime git 依赖（manox-agent/harness/session-core/providers/supervisor/terminal/hyperlinks/manox-ahp/manox-ahp-runtime）共用同一个 rev/tag 声明。

```bash
script/local-manox.sh on            # git 依赖 → ../manox 本地路径（.cargo/config.toml，已 gitignore）
script/local-manox.sh off           # 还原纯 git 依赖（CI 形态）
# 拾取上游新发布（patch off 时）：改 Cargo.toml [workspace.dependencies] 的 tag= 值，
# 然后 cargo update -p manox-agent（全部 9 依赖随同一 tag 整体移动）
```

runtime 侧 API/行为回归优先在 dspo/manox 修；只有装配/接线问题在本仓修。两仓共享运行时状态根 `~/.manox/`（路径清单见 dspo/manox 仓 AGENTS.md）。

### 跨仓配对修复的消费义务

上游 PR 的 body 或 commit message 点名 dspo/manox-app 需要伴随改动的（「Companion change required in dspo/manox-app」及同类表述），其消费义务落在把锁推进到该发布的那个本仓 PR：**同一 PR 内完成订阅/换道/接线，或在 Assumptions 里显式申报滞留原因**——静默不消费等于上游白修（桌面端撞不到修复）。台账只列**未结清**义务（销账即删行；历史销账记录走 git 历史与 PR 描述，不驻留本文件，禁止「本 PR」类随时腐烂的指代）：

- dspo/manox#840（interaction park 跨 restore 结算）：纯宿主侧，app 零动作（挂此备忘至上游确认无客户端义务）

## 构建与开发命令

```bash
cargo build                          # debug 下 gpui 依赖需 opt-level=3，否则渲染极慢
cargo run                            # 桌面应用（chrome 壳，唯一）
cargo run -p cx-cli                  # cx CLI
cargo test                           # live 测试用 MANOX_RUN_LIVE=1 env 门控，默认安全
cargo clippy --all-targets
cargo fmt --all
```

Rust **1.95.0**（`rust-toolchain.toml`），edition **2024**，需 `clippy`/`rustfmt`/`rust-src`。Linux 需要 GTK3/Wayland/WebKit2GTK 系统依赖（CI build.yml 的 apt 清单）。

### 出 UI 图（chrome 壳）

UI 静态图**从真实渲染出**，不要另画一套：`crates/steer-agent-chrome-ui/tests/visual.rs`
用 gpui 官方离屏渲染（`VisualTestAppContext` + Metal 回读）把**真实的 `Shell`** 截成 PNG，
不需要录屏权限，窗口在 (-10000,-10000) 渲染、不会闪屏。

```bash
CHROME_SHOT=/tmp/shell.png cargo test -p steer-agent-chrome-ui --test visual
CHROME_SHOT=/tmp/right.png CHROME_RIGHT=1 cargo test -p steer-agent-chrome-ui --test visual   # 展开右栏 + 一个 dummy 页签
CHROME_SHOT=/tmp/panel.png CHROME_PANEL=1 cargo test -p steer-agent-chrome-ui --test visual   # 展开底部 dock
CHROME_SHOT=/tmp/sw.png CHROME_RIGHT=1 CHROME_SWITCH=1 cargo test -p steer-agent-chrome-ui --test visual  # 两个不同宽度的页签 + 切回第一个
```

- 诊断开关 `CHROME_SWITCH=1`：开第二个（更宽的）页签、等布局稳定后切回第一个——**这是唯一能触达页签指示器滑动动画的路径**，单页签出图看不到「从哪来」。
- **动画类改动注意**：离屏测试**没有平台帧循环**，`request_animation_frame` 排的回调只有经 `Window::simulate_next_frame` 才会送达；只 `run_until_parked` 的话动画永远停在第一帧（`delta=0`）。settle 循环已补一帧，但**要看中间帧必须自己 pump**；出图只反映终态。
- 页签指示器的对齐由 `script/check-tab-indicator.sh` 把关（量**左端**而非宽度——曾经宽度/高度全对却整体右偏 6px，任何宽度或 y 断言都看不见）。它依赖 macOS 离屏 harness，CI 上自动跳过。
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

升级姿势：`cargo update -p gpui-pre --precise x.y.z`（与组件版本按 gpui-kit lock 对齐）→ 重跑全部门禁；gpui-pre 的 `[[patch.unused]]`/duplicate 用 `cargo tree -d | grep -i gpui` 验证单一版本，`script/check-no-zed-git.sh` 验证 zed 零引用（CI 门禁）。gpui 相关依赖在 debug 下仍需 opt-level=3（`[profile.dev.package] gpui-pre*`）。历史注：旧栈（gpui@zed-1d217ee + gpui-component@longbridge-git）已退役；gpui-pre ≥0.3.4 含上游 gpui#60295 element-arena 修复。

版本跟进/升级的完整流程（跟进触发面、评估三问、整列 runbook、adoption 跟进与手搓热点表）见 `.agents/skills/gpui-kit-follow/SKILL.md`。

## 提示词与 i18n

本仓是 i18n 的**唯一归属地**：`crates/steer-i18n` 自带 Fluent 栈与全部资源（`crates/steer-i18n/locales/{zh-CN,en}.ftl`），中文为第一语言（primary + 默认），英文为次（fallback）。调用经 `agent_ui::i18n::t("key")`（gpui 层 `SharedString` 包装）或 `steer_i18n::t`（无 gpui 依赖的层）。

**i18n 只覆盖 app chrome**：侧栏、设置面板、菜单栏、托盘、About、终端 overlay 等本仓自产的界面文案。

**manox runtime 传入的值一律原样渲染，禁止二次本地化**——工具 title/summary、slash 命令 description（runtime 给英文）、plan 文本、`AskUserQuestion` 的 header、模型产出内容。新增 UI 文案 = 在 `crates/steer-i18n/locales/` 两个 `.ftl` 各加一个键（缺一不可，parity 由单测守护）+ 调用处换 `t("key")`。

模型面向字符串（提示词模板、工具 description）一律英文且**不在本仓维护**：多段落提示词散文全部在 dspo/manox 仓，本仓不得内嵌。

语言配置 `ui_language` 由本仓拥有（`steer_i18n::{load_ui_language, persist_ui_language}`，读写 `~/.manox/settings.toml` 中该键且只碰该键）；`agent_language` 键已随 manox 侧语言轴退役而废除，不再有意涵。

## 工作流约定

- **唯一壳（2026-09-28 旧壳退役）**：窗口只有 `agent-ui::chrome_assembly` 这一个装配
  （chrome crate 的 Shell + 投影侧栏 + 右栏 ToolTab 注册表 + 会话列作为主区卡内容）。
  双壳构建开关（`--features chrome-shell`）与 `agent-ui::Workspace` 的旧全壳渲染已删净；
  上一个双壳形态留在 tag `dual-shell-final`（计划文档已随退役删除，历史见 git log）。
- 每 PR 门禁：`cargo clippy -D warnings --all-targets` + 全量
  `cargo test` + `cargo test -p agent-ui --features test-support` + `cargo fmt`；
  PR 写清 Test Plan 与 Assumptions。
- **合并前必须把 manox 依赖 bump 到最新兼容发布**：决定合并（含批准后的最后一步）前，在 `script/local-manox.sh off` 形态下确认 Cargo.toml 的 runtime tag 指向 release/0.1（或 main 的 v0.2 轨）最新兼容 tag——9 个依赖共用同一 tag，改一处声明 + `cargo update -p manox-agent` 整体移动——重跑全部门禁，并把变更随合并一并提交。若最新上游与本仓不兼容，先在 dspo/manox 修出兼容发布再 bump，或在 PR 中显式声明滞留原因（旧 tag 停留是债务，不是默认状态）。
- 提交信息不得携带 Co-Authored-By 尾注（CI 强制拒绝）。

## 项目规则

- **技术选型喜新厌旧**：能选最新 stable 就选最新 stable（依赖、工具链、API）。
- **禁止 vendor / submodule**：所有依赖经 Cargo 声明；上游 manox 仓一律 git 依赖，禁止改回 path 依赖提交。
- **crate 依赖只认 crate 索引或 git 地址**：外部 crate 只能是 crates.io 版本或 `git = "..."`，禁止 `path = "..."` 指向本机路径（CI 不可复现）；workspace 内部成员间 `path` 例外；本地联动只经 gitignored 的 `.cargo/config.toml` [patch]。
- **只允许单二进制、单进程交付**（桌面应用；cx CLI 为独立 bin）。
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
