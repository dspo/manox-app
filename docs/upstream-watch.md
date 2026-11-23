# Upstream watch — 四条外部依赖线的台账

本仓的外部依赖线只有四条；每条线的最新 pin、open 项与核对证据都在这里。**任何 pin
变更（manox tag、gpui-kit、rust channel、Ghostty rev、pi distro、lock 级 pin）都必须
先读本文件、对每个 open 项在新版本里实测核对（不能只读文档），并在下面补一行核对
记录**——没有核对的 bump 视为 bump 未完成（见 AGENTS.md「依赖 pin 纪律」）。

## 台账

| 线 | 当前 pin | 声明位置 | 归属仓 |
|---|---|---|---|
| manox runtime | `v0.13.0`（8 个 crate 共用同一 tag） | `Cargo.toml` `[workspace.dependencies]` + `Cargo.lock` | dspo/manox |
| gpui-kit | `gpui-pre* =0.3.12`、`gpui-component =0.9.1`、`gpui-kit-assets =0.9.1`、`gpui-base =0.9.1` | `Cargo.toml` + `Cargo.lock` | longbridge/gpui-kit |
| rust channel | `1.100.0` | `rust-toolchain.toml` | rust-lang |
| pi distro | `@earendil-works/pi-coding-agent 1.5.0` | `pi-harness/package.json`（+ `src/version.ts` 同值） | earendil-works/pi |

（Ghostty rev / zig 与 lock 级 pin 同属第二条与第四条纪律的管辖面；它们没有独立
的 open 项时不出现在台账行里。）

## Open

### manox runtime

- **O1 — ask mailbox 的 restart repair / late answer 缺 runtime 面。**
  宿主侧已能：把「一 ask 一卡」的请求投影成卡片（#282）、按最新未答请求顶掉旧卡
  （supersede 呈现）。缺的是：进程重启后仍在等答案的 ask 无法补答，晚到答案也没有
  注回通道——runtime 在 turn 结束时就给悬空 ask 记了 `cancelled` 决定，归档 turn 里
  的未答请求按设计不可回答（`crates/steer-agent-chat-ui/src/ahp_store.rs::fold_input_request`
  的注释写明了这条不变式与 2026-10 的锁死事故）。
  - 需求：AHP 面提供「重启后仍可回答的 ask」——例如 `chat/inputRequested` 在
    `session/restore` 后重放仍在等待的请求，并提供一个「写回答案」的通道（live 解开
    或 late 注入）。
  - 立项与核对：2026-11-23 建本台账时立项并核对（manox `v0.13.0`）。
  - 状态：**open**（等待 dspo/manox 侧实现；本仓不硬造）。

### gpui-kit

- 无 open 项。

### rust channel

- 无 open 项。

### pi distro

- 无 open 项。升级纪律见 AGENTS.md「跨仓配对修复的消费义务」：独立小 PR + 同步
  `src/version.ts` + 重跑 `typecheck / bun test / build / smoke` 与 live 烟雾。

## 已核对（无需动作）

| 项 | 结论 | 证据 | 核对日期 / pin |
|---|---|---|---|
| 会话扫描「先截断后排序」 | 不存在。扫描是 reconcile（`read_dir` + `stat` 索引），排序走 durable order account（folder → pinned → interaction），截断不参与排序路径。 | `manox-agent/src/thread_store.rs` 的 `rerank` 与 `SessionIndex` 注释 | 2026-11-23 / `v0.13.0` |
| 会话内换模型 / 思考档位 | 已具备。composer 的模型 chip 走 `build_model_cascade`，落 `AhpStore::optimistic_config` + `set_config`，`session/configChanged` 回显即确认、拒绝即回滚。 | `agent-ui/src/workspace/column_composer.rs`（`build_model_popup_menu_pi`）与 `ahp_store.rs` 的 optimistic config 测试 | 2026-11-23 / `v0.13.0` |
| 转录主列虚拟化 | 已具备。主列走 `gpui::list`（`ListState` + 处理器），流式跟随用 `transcript_press` 的 pause/resume tail-follow。 | `agent-ui/src/workspace/column_view.rs`、`steer-agent-chat-ui/src/column.rs` | 2026-11-23 |
| manox ask 的 supersede | 已具备。fold 取 active turn 里最新的未答 `InputRequest`，新 ask 自然顶掉旧卡。 | `ahp_store.rs::fold_input_request` | 2026-11-23 / `v0.13.0` |

## 核对记录（每次 bump 追加一行）

格式：`YYYY-MM-DD | 线 | 旧 → 新 | 逐条 missing/satisfied/changed/unchecked | PR`。

| 日期 | 线 | 旧 → 新 | 逐条结果 | PR |
|---|---|---|---|---|
| 2026-11-23 | manox runtime | `v0.13.0`（建立台账，无变更） | O1 missing；已核对四项 satisfied | — |
| 2026-11-23 | gpui-kit + rust channel | `0.8.0`/`0.3.9` + `1.97.1` → `0.9.1`/`0.3.12` + `1.100.0` | open 项 0 条；三版 release notes 逐条对照消费面（Theme non_exhaustive / KeyBinding 静态化 / scroll 改名等，迁移清单见 PR）；两处行为性变更（动画 executor 时钟、on_prepaint probe 锚点 zed#3689）实测四张壳图 0 字节差异；0.8.0 轮吸收 API 全部存活（0.9.1 源码核验）；`with_runs` 台账复核未修、缓解维持，CJK fuzz battery 转绿作金丝雀 | #315 |
