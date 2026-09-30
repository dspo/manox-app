---
name: gpui-kit-follow
description: manox-app 的 gpui-kit 版本跟进方法论——评估新发布该不该吃（breaking 是否命中消费面、新特性能否替代手搓实现）、整列升级的 runbook、升级后的 adoption 跟进与手搓热点表维护。凡涉及 gpui-kit、gpui-component、gpui-pre、gpui-kit-assets、gpui 依赖的版本评估/bump/升级，或想调研组件库新特性能否替代本仓手搓实现，或撞到疑似 gpui 层 bug 想查上游是否已修——即使用户只说「gpui 又更新了，看看」「要不要升到 0.7」，也应加载本 skill。
---

# gpui-kit 版本跟进方法论

适用范围：gpui-kit 家族（`gpui-pre` / `gpui-base` / `gpui-component` / `gpui-component-macros` / `gpui-kit-assets` / `gpui-kit` 门面）任何一员的版本评估、升级决策与升级操作。pin 纪律与栈结构的权威在 AGENTS.md「GPUI 依赖版本锁定」，本 skill 只管流程。

## 三个背景事实（评估与升级的全部前提）

1. **发布是周更火车**：minor（0.6→0.7）承载破坏性变更，patch（0.7.0→0.7.1）只修不破。
2. **整栈一列火车**：每个 gpui-kit 发布精确 pin 一个 gpui-pre 快照（例：0.7.0 ⇒ gpui-pre `=0.3.7`），component/macros/assets/base 同线 semver。混搭不可能，升级只能整列移动。
3. **快照 ≠ semver**：gpui-pre 是 zed main 的每周快照，patch 号（0.3.6→0.3.7）之间可进任意 zed 变更。快照移动必须按 minor 风险对待，视觉门禁是真安全网。

## 何时跟进

### 必须跟进（不跟进即损害）

1. **发生 breaking**：升级落点凡含 breaking，必须全额消费——编译不过是最强信号。不留半吊子迁移，不新加 `#[allow]` 或禁用 feature 绕过。
2. **撞到上游已修复的 bug**：崩溃、渲染回归、交互异常，先在 release notes / changelog 确认修复点再升级（先例：element-arena gpui#60295 的 SIGABRT 修复是本仓上 gpui-pre ≥0.3.4 的原因）。
3. **平台演进使旧快照失效**：macOS 大版本、Metal、输入法行为变化。离屏截图与真机往往先暴露。
4. **生态收敛压力**：工具链升级或其它依赖把同族 crate 拉成多版本时，必须归一到单一版本（`cargo tree -d | grep -i gpui` 验证）。
5. **安全修复**。

### 应当跟进（评估后倾向吃）

1. 原生实现可**整体替代**手搓 → 更少代码、更优雅。
2. 原生能力可**部分替代或裨益**手搓 → 混合采用。
3. 新特性**简化实现、提升性能**（如 shaping 缓存、动效系统）。
4. 上游根因修复让本项目**能删除 workaround**——workaround 是债（例：离屏动画 pump 补帧、布局规避）。
5. **新 UI 需求立项时**，先查最新组件库有无原生件，再决定手搓。
6. 上游宣布 deprecated、下个 minor 将移除的别名/API，在其移除前主动迁移。
7. a11y、IME 等面向最终用户质量的改进与本产品形态相关时。

## 重评估原则

每轮评估从零出发。历史结论——包括「明确不跟进」的组件（见 references/hand-rolled.md 末节）——只作上下文缓存用来加速（跳过已确认的背景调查），**不作为跳过评估的理由**：新版本可能带来新发现，而评估成本低（下一节三问全是机械对照）。消费清单和热点表都会随代码漂移，每轮都要重新生成/重读。

## 评估三问（两清单对照）

**问一：breaking 打到消费面了吗？**
消费清单重新生成（代码会变，别信记忆）：

```bash
grep -rhoE "gpui_component::[a-zA-Z_:+]+" crates --include="*.rs" | sort -u
```

清单外的 breaking 判「不命中」，仍记录在案供下轮作背景。逐条核对 release notes 的 breaking 节，给出命中/不命中结论。

**问二：新特性打到手搓面了吗？**
读 `references/hand-rolled.md`（手搓热点表），与 release notes 新特性求交。有交点才展开对照评估：能力覆盖度？与像素校准/设计语言冲突？迁移量多大？评估产出 = adoption 候选（去做）或冲突结论（记录）。

**问三：快照进了什么？**
读 gpui-pre crate 自带的 `[package.metadata]` zed-rev 映射，对照 zed 仓的 commit 范围，重点扫 rendering / input / IME 相关变更。

成本意识：不设固定跟进节奏（开发者想跟就跟），但长期滞留会让 breaking 面叠加——跨了几个 minor 线，在评估时显式计为成本。

## 升级 runbook（独立 PR、行为中立）

0. **原则：bump PR 不夹带功能改动、不夹带 adoption 重构**——回归时 revert 单 commit 即回滚（本仓 squash-only，一 PR 一 commit）。
1. Preflight：`script/local-manox.sh off` 形态（CI 形态）、干净树、记录旧 pin。
2. Cargo.toml 整列改：`gpui-pre` / `gpui-pre-platform` `=` 新快照，`gpui-component` / `gpui-kit-assets` `=` 新版本（用了 `gpui-kit` 门面则一并）。不混版本。
3. `cargo update -p <目标包族>`；**diff Cargo.lock 只许目标包族及其直接传递闭包的 churn**，无关 hunk 回滚（有 zero-rev 卷入传递依赖的前科）。
4. `cargo tree -d | grep -i gpui` 验单一版本；lock 随 PR 提交（macros 随 `^` 浮动，历史上浮出过编译炸）。
5. `cargo clippy --all-targets -- -D warnings`：报错点即真实 breaking 面，按 release notes 迁移节逐条修。
6. 全门禁：`cargo test` + `cargo test -p agent-ui --features test-support` + `cargo fmt`。
7. 视觉门禁（快照风险的主安全网）：四张 `CHROME_SHOT`（base / right / panel / switch）+ `script/check-tab-indicator.sh`，与升级前基线截图对比。
8. 有视觉差异但断言全绿 → `cargo run` 人工过一眼再合。
9. PR：`chore(deps)` 前缀；贴 release notes 链接与 zed-rev 映射；Test Plan 写门禁与截图结论；**同一 PR 同步 AGENTS.md / Cargo.toml 注释里的版本表述**；不带 Co-Authored-By（CI 拒）。

## 吸收轮

- 问二产出的 adoption 候选开**独立功能 PR**：写明替代哪块手搓、预计删多少行、像素断言如何保持。
- adoption 合入后**必须更新 `references/hand-rolled.md`**——热点表是活文档，腐烂即失效。

## 应急

合入后日常使用撞到渲染/行为回归：revert 单 commit 回旧 pin → 带离屏探针证据报上游 issue → 修复版发布后再吃。不做跨 minor 长期滞留旧版的状态。
