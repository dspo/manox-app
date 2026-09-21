# manox-bundle-install

ZCode 插件：把「bundle and install 最新的 manox app 和 cx cli」固化为一个可复用
skill。流程 = 取最新 main → 核对上游 dspo/manox 锁 rev（必要时 `cargo update -p
manox-agent` 抬升并跑门禁）→ `script/bundle-mac -i` 构建、签名、安装
`/Applications/Manox.app` 并把 `~/.local/bin/cx` 软链进 bundle → 时间戳 / 签名 /
可运行三重验收。仅 macOS。

技能正文见 [skills/manox-bundle-install/SKILL.md](skills/manox-bundle-install/SKILL.md)。
