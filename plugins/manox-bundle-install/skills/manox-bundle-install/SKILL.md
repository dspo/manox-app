---
name: manox-bundle-install
description: Bundle and install the latest Manox.app and cx CLI on macOS from manox-app main（用户说"帮我 bundle and install 最新的 manox app 和 cx cli"、"打包安装最新 Manox/cx"等时使用）
---

# Bundle & Install Manox.app + cx CLI (macOS)

把 manox-app main 最新代码构建为 macOS .app 并安装：app 落 `/Applications/Manox.app`，
cx CLI 以内嵌可执行文件随包交付并软链到 `~/.local/bin/cx`。整条链路由
`script/bundle-mac` 承载，本 skill 只负责前置核对、构建、验收与如实报告。

## 前置条件（不满足即停下说明，不要绕）

- macOS（脚本自身有 triple 守卫，非 darwin 直接失败）。
- 仓库工作区干净、当前在 `main`：`git status --short` 必须为空。
- 无需 Apple Developer 证书：默认 ad-hoc 签名（含 GPUI JIT 所需 entitlements），
  本机日常使用即此形态。`MACOS_CERTIFICATE`/`MACOS_SIGNING_IDENTITY` 存在时脚本
  自动走真签名，无需干预。

## 流程

### 1. 取最新 main

```bash
git fetch origin main
git pull --ff-only origin main   # 落后则快进；有分叉/脏树则停下问用户
```

### 2. 上游 manox 依赖新鲜度（app 与 cx 同锁一份 runtime）

```bash
git ls-remote https://github.com/dspo/manox.git main
grep -m1 -o 'manox.git?branch=main#[a-f0-9]*' Cargo.lock
```

- 一致（常见）：直接进第 3 步。
- 不一致：`script/local-manox.sh off`（确认纯 git 形态）后
  `cargo update -p manox-agent`（同一 git source 的全部 manox-* 依赖整体抬升），
  跑门禁 `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings &&
  cargo test`。**Cargo.lock 变更留在工作区、如实报告，不擅自提交**——锁 rev 抬升
  按仓库纪律随下一个 PR 走；`cargo update` 顺带引入的 Windows 依赖浮动 churn 应
  `git checkout Cargo.lock` 剔除（churn 与 manox rev 无关，看 diff 里是否只有
  windows-* 行即可判断）。

### 3. 构建 + 打包 + 安装（一条命令覆盖 app 与 cx）

```bash
./script/bundle-mac -i
```

release 构建增量约 2 分钟、冷启动更久——用后台任务跑，完成后再继续。
`-i` 的完整效果：release 构建两个 bin → 组装 .app → ad-hoc 签名 →
替换安装 `/Applications/Manox.app` → `ln -sfn` 把 `~/.local/bin/cx`
指向 bundle 内嵌的 `Contents/MacOS/cx`（已存在的非软链 cx 会自动挪到
`cx.bak-<timestamp>`）。

变体仅在用户点名时用：`--harness pi`（另一 harness 口味的共存包）、
`-d`（debug 构建，仅调试用）、`--no-cx-link`（不建软链）、
`--cx-link-dir <dir>`（软链到别处）。

### 4. 验收（全部通过才算完成）

```bash
stat -f "%Sm %N" /Applications/Manox.app          # 时间戳应是刚刚
codesign --verify --deep /Applications/Manox.app  # signature ok
ls -la ~/.local/bin/cx                            # 软链指向 /Applications/Manox.app/Contents/MacOS/cx
~/.local/bin/cx --help                            # 正常输出（统一 Agent 入口）
```

注意：若 Manox.app 正在运行，替换后的新构建要退出重开才生效——验收时提醒。

### 5. 报告

如实给出：构建自哪个 commit（`git log --oneline -1`）、上游 rev 是否抬升过
（有未提交的 Cargo.lock 变更要点名）、两个交付物的落点路径、验证结果。

## 验收示例（trial prompt）

> 帮我 bundle and install 最新的 manox app 和 cx cli

预期可观察结果：`/Applications/Manox.app` 时间戳刷新为本次构建、
`codesign --verify` 通过、`~/.local/bin/cx` 软链指向新 bundle 且 `cx --help`
可运行；报告包含构建 commit。
