# AGENTS.md

Guidance for coding agents working in this repo.（本文件是 manox-app 仓库指引的唯一权威；CLAUDE.md / CLAUDE.local.md 仅为指向本文件的入口，不承载内容。）

## 项目概述

manox-app 是 **GPUI 桌面应用仓**：完整的应用（窗口、UI、终端、webview、系统托盘）与 cx 启动器（外部 agent CLI）。agent runtime（harness/agent/session-core/protocol/providers/supervisor/lsp，外加回流的终端仿真核心 manox-terminal 与 hyperlinks）来自 **dspo/manox**，经 git 依赖（branch=main）+ 提交的 Cargo.lock 锁 rev 消费。单二进制、单进程。逐文件架构靠读代码获得——本文件只承载不可从代码推导的约束。

### 代码结构

```
apps/desktop/              # 应用侧 crate
  manox/                   # 主二进制入口（窗口 + 主题 + 托盘 + 接线）
  agent-ui/                # GPUI UI 层（Workspace/ConversationState/views）
  terminal-ui/             # 终端渲染层（TerminalElement/TerminalView；仿真核心在 dspo/manox 的 manox-terminal）
  manox-components/        # 一方 gpui 组件库（markdown 渲染等）
  manox-webview/           # wry 原生 webview + Tauri 式 IPC
  manox-webview-macros/    # webview IPC 过程宏
crates/                    # 应用自有的非 UI-框架 crate
  cx/                      # cx headless 库（配置核心、launch-home、
                           #   chatgpt/vscode launch、probe db）
  cx-cli/                  # cx CLI bin（clap + ratatui TUI + relay + stats + `cx web`）
  manox-ext-agents/        # ext-agent 启动 API、会话管理、IPC relay、
                           #   cx_session 桥（SessionHandle → PtySource）
```

上游 manox 仓的拆分点：tag `pre-manox-app-split`；涉及 runtime/协议/journal 的改动在 dspo/manox 提 PR，本仓经 `cargo update -p <crate>` 拾取。终端仿真核心（manox-terminal）与 hyperlinks 已回流 dspo/manox（同经 git 依赖消费），`CxSessionSource` 桥留在本仓 manox-ext-agents。

### 与 manox 仓的联动开发

```bash
script/local-manox.sh on            # git 依赖 → ../manox 本地路径（.cargo/config.toml，已 gitignore）
script/local-manox.sh off           # 还原纯 git 依赖（CI 形态）
cargo update -p manox-agent         # 显式拾取上游新 rev（patch off 时）
```

runtime 侧 API/行为回归优先在 dspo/manox 修；只有装配/接线问题在本仓修。两仓共享运行时状态根 `~/.manox/`（路径清单见 dspo/manox 仓 AGENTS.md）。

## 构建与开发命令

```bash
cargo build                          # debug 下 gpui 依赖需 opt-level=3，否则渲染极慢
cargo run                            # 桌面应用
cargo run -p cx-cli                  # cx CLI
cargo test                           # live 测试用 MANOX_RUN_LIVE=1 env 门控，默认安全
cargo clippy --all-targets
cargo fmt --all
```

Rust **1.95.0**（`rust-toolchain.toml`），edition **2024**，需 `clippy`/`rustfmt`/`rust-src`。Linux 需要 GTK3/Wayland/WebKit2GTK 系统依赖（CI build.yml 的 apt 清单）。

## 工具链 & Skills

涉及 GPUI/UI 开发时，先通过 Skill 工具加载 `.claude/skills/` 下的 skill：
- `gpui` — GPUI 框架（Entity/Render/actions/keybindings/async/layout）
- `gpui-component` — gpui-component 组件库（Button/Input/List/Sidebar 等）
- `gpui-component-dev` — 为 gpui-component 贡献新组件时额外加载

## GPUI 依赖版本锁定

GPUI 栈走 git 仓库地址（crates.io 无 gpui-component）：`gpui`/`gpui_platform` pin zed rev，`gpui-component`/`gpui-component-assets` pin longbridge rev，**三者必须同一 gpui 版本**。gpui 相关依赖在 debug 下需 opt-level=3（根 manifest `[profile.dev.package]`）。升级 rev 时四条目一起动，`cargo tree -d | grep -i gpui` 验证单一版本。

## 提示词与 i18n

模型面向字符串一律英文、绝不本地化；UI chrome 本地化经 `manox_agent::i18n::t("key")`（Fluent 资源与规则在 dspo/manox 仓的 `crates/manox-agent/locales/`）。提示词模板维护在 manox 仓，本仓不得内嵌多段落提示词散文。

## 工作流约定

- 每 PR 门禁：`cargo clippy -D warnings --all-targets` + 全量 `cargo test` + `cargo fmt`；PR 写清 Test Plan 与 Assumptions。
- **合并前必须把 manox 依赖 bump 到最新兼容 commit**：决定合并（含批准后的最后一步）前，在 `script/local-manox.sh off` 形态下执行 `cargo update -p manox-agent`——同出 dspo/manox 一个 git source 的全部依赖（manox-agent/harness/protocol/session-core/providers/supervisor/manox-terminal/hyperlinks）在 Cargo.lock 中共用同一锁 rev，任一条目即可整体抬升——重跑全部门禁，并把 Cargo.lock 变更随合并一并提交。若最新上游与本仓不兼容，先在 dspo/manox 修出兼容 rev 再 bump，或在 PR 中显式声明滞留原因（旧 rev 停留是债务，不是默认状态）。
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
