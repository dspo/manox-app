## manox-app UI chrome strings.
##
## This is the ONLY place manox-app user-facing chrome text lives. The primary
## locale is Simplified Chinese (zh-CN.ftl); English (en.ftl) is the fallback.
## Keys are grouped by source file for navigability; ids use `-` (fluent
## forbids `.`).
##
## Scope is strictly app chrome. Anything the manox runtime hands this app —
## tool titles/summaries, slash-command descriptions, plan prose, model output —
## is rendered verbatim and NEVER resolved through these bundles.

sidebar-new-chat = 新对话

sidebar-section-projects = 项目

sidebar-section-conversations = 对话

sidebar-new-session-label = 新建会话

sidebar-new-session-steer = Steer

sidebar-new-terminal = 新建终端

sidebar-remove-project = 移除项目

sidebar-project-menu = 项目操作

session-kind-terminal = 终端

sidebar-close-external = 关闭会话

sidebar-rename-external = 重命名

sidebar-resume-external = 恢复会话

external-session-resume-failed = 恢复外部 agent 失败

sidebar-archive = 归档

sidebar-unarchive = 取消归档
sidebar-multi-select = 多选会话…
sidebar-selection-count = 已选 { $count } 项
sidebar-selection-select-all = 全选
sidebar-selection-archive = 归档
sidebar-selection-cancel = 取消

sidebar-thread-menu = 会话操作

sidebar-thread-tag = 标签

sidebar-thread-tag-placeholder = 标签…

sidebar-thread-tag-add = 添加标签

sidebar-thread-tag-clear = 移除标签

sidebar-thread-tag-rename = 重命名标签

external-wizard-no-model = 尚无模型支持该 agent

external-session-start-failed = 启动外部 agent 失败

plain-session-start-failed = 启动终端会话失败

sidebar-empty-summary = (新对话)

sidebar-copy-thread-id = 复制 thread id

sidebar-pending-auth = 等待审批

sidebar-view-options = 视图选项

sidebar-order-manual = 手动

sidebar-order-updated = 最近更新

sidebar-show-more = 展开其余 { $count } 条

sidebar-time-just-now = 刚刚

sidebar-time-minutes = {$count} 分钟前

sidebar-time-hours = {$count} 小时前

sidebar-time-days = {$count} 天前

sidebar-time-weeks = {$count} 周前

### session_list.rs（侧栏状态槽位的行 tooltip：一行主状态文案——运行中／
### 等待批准／等待回答／未读；等待批准与等待回答共用琥珀像素问号字形，
### 只有文案区分。空闲行不挂 tooltip，故无空闲文案）

sidebar-status-running = 运行中

sidebar-status-pending-auth = 等待批准

sidebar-status-pending-ask = 等待回答

sidebar-status-unread = 未读

### message.rs

message-reasoning = 思考

message-error = 错误

message-notice = 通知

member-editor-tab = 编辑消息

browser-tab = { $title }

browser-address-placeholder = 输入网址

browser-yield-hint = 已让出控制权（例如用于登录）。完成后请点此。

browser-yield-complete = 完成

browser-read-hint = Agent 正在读取本页 —— 页面中已登录的内容将暴露给 agent。

right-pane-toggle = 切换右侧面板

sidebar-toggle = 切换侧边栏

right-tab-new = 新建标签页

right-tab-launcher = 新标签页

launcher-open-browser = 打开集成浏览器

launcher-open-terminal = 打开集成终端

launcher-open-claude = 打开 Claude Code

launcher-open-codex = 打开 Codex

launcher-open-copilot = 打开 GitHub Copilot

message-user-role = 你

message-harness-role = Harness

message-fork-here = 从此处分叉新会话

message-fork-unavailable-not-landed = 回复写入会话记录后即可从此处分叉

message-fork-unavailable-mid-turn = 仅可从已完成轮次的最后一条消息分叉

message-fork-unavailable-not-replayed = 该行不属于会话记录，无法分叉




message-omitted-prefix = …（已省略前面部分）

status-pending = 待审批

status-running = 运行中

status-success = 完成
tool-output-truncated = 输出已截断，拷贝 out 可得完整内容

tool-diff-open-in-pane = 在右栏打开 diff

status-continued = 继续讨论

status-error = 出错

status-denied = 已拒绝

status-cancelled = 已取消

### agent-ui workspace — Captain 状态行（composer 上方）
### 「Captain」是产品词汇，不翻译（两种语言都保持字面量）；只翻译它后面的动词。

steering-verb = 正在驾驶
# 子代理行：`{ $name }` 是「类型(任务)」（例如「Explore(修复登录跳转)」，
# 类型由宿主给出，原样渲染）。
subagent-steering-prefix = { $name }正在驾驶
turn-process-completed = 已完成，用时
captain-activity-thinking = 思考中
captain-activity-replying = 正在回复

### views/subagent_stream.rs（主列中的子代理页）

subagent-stream-empty = 暂无活动 — 该子代理还没有产出第一轮内容。

### views/subagent_panel.rs

subagent-panel-settling = 正在接入子会话…
subagent-panel-empty = 子会话已接入，暂无内容。
subagent-panel-readonly = 子代理工作会话 · 只读

subagent-panel-final-note = 实时转录不跨重载保留；以下为子代理的最终回答。

thinking-tool-result = 工具结果

# 共用的 h/m/s 时长格式（`steer_i18n::t_duration`）：Captain 状态行、活动段头、
# 整轮过程头。前导零单位省略，秒始终显示。
duration-hms = { $h }小时{ $m }分{ $s }秒
duration-ms = { $m }分{ $s }秒
duration-s = { $s }秒

activity-failed = { $count } 失败

activity-awaiting-approval = { $count } 待审批

### views/settings.rs

settings-group-general = 通用

settings-item-general = 常规

settings-item-models = 模型

settings-group-integrations = 集成

settings-item-plugins = 插件

settings-group-external-tools = 外部工具

settings-item-chatgpt-app = ChatGPT.app

settings-item-vscode-app = Visual Studio Code.app

settings-back = 返回应用

settings-item-agents = Agents
settings-item-skills = Skills

settings-origin-builtin = 内置
settings-origin-user = 用户
settings-origin-plugin = 插件

agents-captain-desc = 主对话 agent。系统提示词由运行时模板与项目指令、技能清单、用户笔记按会话组装。
agents-field-name = 名称
agents-field-color = 颜色标记
agents-field-model = 模型
agents-field-desc = 描述
agents-field-tools = 可用工具
agents-field-prompt = 系统提示词
agents-tools-custom = 自定义可用工具
agents-tools-all-label = 全部可用工具
agents-tools-hint = 控制该子智能体可以调用的工具范围。
agents-detail-scope = 作用域
agents-detail-scope-user = 用户
agents-save = 保存
agents-cancel = 取消
agents-delete = 删除
agents-captain-meta = 内置 · 模型随会话

agents-all-tools = 全部工具

agents-model-inherit = 继承会话模型
agents-tools-count = 个工具

agents-readonly-note = Agents 目前为只读展示；自定义 agent 放在 ~/.claude/agents/ 下的 .md 文件即可被识别。

skills-readonly-note = Skills 目前为只读展示；用户技能位于 ~/.claude/skills/，插件技能随插件安装。


### views/settings.rs — 常规面板

settings-row-ui-language = 用户界面语言

settings-desc-ui-language = 界面显示语言，保存后立即生效。

settings-save-failed-title = 设置保存失败

settings-saved = 已保存

# 外部工具 → ChatGPT.app 面板

settings-panel-chatgpt-app = ChatGPT.app

settings-desc-chatgpt-top = 配置 cx 启动 ChatGPT.app 时注入的内容。

settings-btn-copy = 复制

settings-btn-copied = 已复制

settings-btn-reveal = 在 Finder 中显示

settings-section-chatgpt-home = Codex Home

settings-desc-chatgpt-home = ChatGPT.app 对话管理主路径，用于与官方路径隔离。

settings-section-chatgpt-injection = 模型注入

settings-desc-chatgpt-injection = 决定自定义模型在 ChatGPT.app 中的可用方式。

settings-row-chatgpt-nickname = 昵称

settings-desc-chatgpt-nickname = 配置后，启动 ChatGPT.app 时以该昵称替换注入的 provider 名称（无论选用哪个 provider）。

settings-chatgpt-nickname-ph = 选填

settings-row-chatgpt-injection = 模型注入方式

settings-value-injection-list = 模型列表

settings-value-injection-single = 单个模型

settings-desc-chatgpt-injection-risk = 注入模型列表须使用 CDP 机制将模型列表注入 ChatGPT.app 进程运行时，选择即表示接受风险。

settings-row-chatgpt-providers = Providers & LLMs

settings-chatgpt-models-loading = 正在加载模型目录…

settings-chatgpt-models-load-failed = 加载失败：{ $error }

settings-chatgpt-models-empty = 未找到可注入的模型（需要支持 Responses 的端点）。

settings-section-chatgpt-env = 变量注入

settings-desc-chatgpt-env = 启动 ChatGPT.app 时的额外变量。

settings-chatgpt-env-key-ph = 变量名

settings-chatgpt-env-value-ph = 值

settings-btn-add-env = 添加变量

settings-section-chatgpt-more = 更多配置

settings-desc-chatgpt-more = 写入注入 config.toml 的高级选项。

settings-desc-chatgpt-websockets = 写入注入的 [model_providers.*] 段，默认 false（自定义端点普遍不支持 WebSocket 流式，走 HTTP 流式）。端点支持 WebSocket 流式时可设为 true。

# 外部工具 → Visual Studio Code.app 面板

settings-panel-vscode-app = Visual Studio Code.app

settings-desc-vscode-top = 从 Steer.app 启动 Visual Studio Code 时，为其 Claude Code 拓展和 Codex 拓展配置 Provider 和 LLM。

settings-section-vscode-claude = Claude Code Extension

settings-section-vscode-codex = Codex Extension

settings-row-vscode-provider = Provider

settings-vscode-no-inject = 不注入

settings-vscode-models-loading = 正在加载模型目录…

settings-vscode-models-load-failed = 加载失败：{ $error }

settings-vscode-models-empty-anthropic = 未找到可注入的模型（需要支持 Anthropic 协议的端点）。

settings-vscode-models-empty-responses = 未找到可注入的模型（需要支持 Responses 的端点）。

### views/settings.rs — 配置面板

### views/settings.rs — 模型面板

settings-models-add-provider = 添加 Provider

settings-models-no-path = 无法解析 Provider 配置路径

settings-models-reload-failed-title = Provider 重载失败

settings-models-empty = 尚未配置任何 Provider

settings-models-unnamed = 未命名 Provider

settings-models-no-selection = 选择左侧 Provider 查看模型

settings-models-load-error-title = 配置读取失败

settings-models-load-error-hint = 为避免覆盖无法识别的文件，自动保存已禁用。请修正文件后重新打开本面板。

settings-models-ph-name = Provider 显示名称

settings-models-section-basic = 基本信息

settings-models-row-apikey = API Key

settings-models-apikey-literal = 字面值

settings-models-apikey-env = 环境变量

settings-models-apikey-keychain = Keychain

settings-models-apikey-shell = shell 命令

settings-models-section-env = 环境变量

settings-models-section-endpoints = 端点配置

settings-models-add-endpoint = 添加端点

settings-models-row-url = URL

settings-models-ph-url = https://api.example.com

settings-models-agents-all-hint = 未选择 = 全部 Agents

settings-models-agents-add = 添加

settings-models-row-copilot = GitHub Copilot 认证方式

settings-models-env-empty = 暂无环境变量

settings-models-empty-models = 暂无模型

settings-models-value-unset = 未设置

settings-models-section-models = 模型列表

settings-models-mode-inline = 手动配置

settings-models-mode-remote = 自动获取

settings-models-ph-remote-url = https://api.example.com/v1/models

settings-models-row-model-id = 模型 ID

settings-models-row-desc = 描述

settings-models-row-context = 上下文窗口

settings-models-ph-context = token 数，如 1000000

settings-models-row-max-tokens = 最大输出 tokens

settings-models-ph-max-tokens = token 数，如 131072

settings-models-row-wire-apis = Wire API

settings-models-row-agents = 启用的 Agents

settings-models-row-supports-tools = 工具调用

settings-models-row-supports-images = 图片输入

settings-models-add-model = 添加模型

settings-models-err-provider-name = 第 {$index} 个 Provider：名称不能为空

settings-models-err-endpoint-url = Provider「{$name}」：端点 URL 不能为空

settings-models-err-endpoint-dup = Provider「{$name}」：端点「{$wire}」重复

settings-models-err-remote-url = Provider「{$name}」：远程模型 URL 不能为空

settings-models-err-model-id = Provider「{$name}」：模型 ID 不能为空

settings-models-err-model-dup = Provider「{$name}」：模型「{$id}」重复

settings-models-err-number = Provider「{$name}」模型「{$id}」：「{$field}」需为整数 token 数

settings-models-err-env-key = Provider「{$name}」：环境变量名不能为空

### views/settings.rs — 个性化面板

### views/settings.rs — MCP 面板
### views/plugin_manager.rs
### views/settings.rs — 环境面板

### workspace.rs

workspace-input-placeholder = 输入消息，点击发送以开始使用

workspace-composer-placeholder = 编写 markdown…（Cmd-Enter 发送）

workspace-unknown-command = 未知命令：/{$name}（用 `/` 菜单查看已安装命令）

workspace-unknown-skill = 未知技能：/{$name}（用 `/` 菜单查看已安装技能）
workspace-rename-confirm = 重命名

workspace-no-model = 未配置模型

workspace-reasoning-effort = 推理强度

workspace-reasoning-high = 高

workspace-reasoning-max = 最高

workspace-ask-supplement-placeholder = 添加可选补充说明

workspace-ask-recommended = 推荐

workspace-ask-waiting = 等待回答

workspace-ask-question-label = 问{$n}：

workspace-ask-answer-label = 答{$n}：

workspace-ask-note-label = 补充：

workspace-ask-skipped = 已跳过

workspace-cancel = 取消

access-mode-sandbox-title = 沙箱访问

access-mode-full-access-title = 完全访问

workspace-mode-notice = { $mode ->
    [sandbox] 沙箱访问：文件写入受围栏约束；网络保持开启，沙箱出网受域名允许清单与审批约束（未配置清单时直连）。
   *[fullaccess] 完全访问：无沙箱；bash 不受限运行，文件变更不受约束。
}

workspace-project-choose = 选择项目

workspace-project-new = 新建项目

workspace-project-blank = 新建空白项目
workspace-project-unfiled = 对话

workspace-project-select-folder = 选择文件夹

workspace-project-name-prompt = 项目文件夹名称

workspace-empty-prompt = 我们该做什么？

follow-stop-stream-failing = 实时跟进已停止：多次重连失败，视图停留在最后收到的内容。点「重试」可重新连接。

follow-stop-indicator-stream-failing = 跟进已停止

follow-stop-retry = 重试

follow-stop-dismiss = 关闭

follow-stop-indicator-retry = 点击重新连接实时跟进
### views/composer_menu.rs

composer-delivery-error = 连接未就绪，消息可能没有送达
composer-delivery-retry = 重试

composer-add-internal-browser = 内置浏览器

completion-tag-command = 命令

completion-tag-skill = 技能

### 用户消息导航

turn-navigator-search-placeholder = 搜索用户消息…

turn-navigator-empty = 暂无用户消息

turn-navigator-no-results = 没有匹配的消息

turn-navigator-attachment-only = 仅附件消息

turn-navigator-empty-message = 空消息

turn-navigator-copied = 消息已复制到剪贴板。

### slash_command.rs

slash-mode-unknown = 未知访问模式“{ $mode }”— 应为 sandbox 或 full-access

### /exit 停机序列（workspace/exit.rs）

exit-untitled-thread = 当前会话

exit-stopping-subagents = 正在停止子代理……

exit-subagent-stopped = 子代理“{ $name }”已停止。

exit-stopping-agent = 正在停止 { $name }……

exit-agent-stopped = { $name } 已停止。

exit-killing-terminal = 正在终止终端中的子进程……

exit-terminal-killed = 终端中的子进程已全部终止。

exit-closing-browser = 正在关闭浏览器……

exit-browser-closed = 浏览器已关闭。

exit-archiving = 正在归档 { $name }……

exit-archived = 会话已归档。

exit-bye = 拜拜！

menu-settings = Settings…

menu-quit = 退出

menu-open-steer = 打开 Steer

menu-file = File

menu-about = 关于 Steer

menu-tools = 工具

menu-vscode-open = 打开 VS Code

## ChatGPT.app 启动通知（工具 → ChatGPT.app 菜单级联）

chatgpt-app-launched = 已启动 ChatGPT.app · { $provider } · { $model }

chatgpt-app-launch-failed = 启动 ChatGPT.app 失败

## VS Code 启动通知（工具 → VS Code 单一入口，注入按 vscode_app 设置解析）

vscode-app-launched = 已启动 VS Code

vscode-app-launch-failed = 启动 VS Code 失败

### terminal-ui (overlay status / search)

terminal-starting = 正在启动…

terminal-search-status = 搜索：{ $pattern }（{ $count } 处匹配）

### views/title_menu.rs

titlebar-pin = 置顶会话

titlebar-unpin = 取消置顶

titlebar-archive = 归档对话

titlebar-unarchive = 取消归档

titlebar-sidebar-toggle = 打开侧边聊天

titlebar-copy-label = 复制

titlebar-copy-id = 复制会话 ID

titlebar-copy-markdown = 复制为 Markdown

titlebar-copy-cwd = 复制工作目录

titlebar-copy-deeplink = 复制深度链接

titlebar-branch-label = 分支

titlebar-branch-from-here = 从当前消息分支

titlebar-branch-from-start = 从对话起点分支

titlebar-schedule = 添加计划任务...

titlebar-new-window = 在新窗口中打开
# ── 环境信息面板 ──────────────────────────────────────────────────────

workspace-env-no-sources = 暂无来源
workspace-info-changes = 变更
workspace-info-changes-files = { $count ->
    [one] {$count} 个文件
   *[other] {$count} 个文件
}

workspace-env-git-unavailable = git 不可用

workspace-env-git-not-a-repo = 非 git 仓库

workspace-env-git-detached = 分离头指针

# ── 上下文栏（右侧边栏）────────────────────────────────────────────────

# ── Cockpit（运行状态 / 里程碑 / 上下文预算）──────────────────────────
# 运行状态行的阶段标签（三状态 tag：生成中 / 思考中 / 待输入）。
# "待输入"标签归并 idle / stopped / failed / awaiting approval。
# 计划区段标题。

# 计划进度计数，显示在标题栏右侧。{$done}/{$total} 为已完成/总数。

# 折叠态下当前任务之外的剩余任务数。{$count} 为数字。

# 折叠态下全部任务完成的提示。

composer-pasted-image = 粘贴的图片

composer-context-usage-tooltip = 上下文已用 { $pct }%（约 { $used } / { $cap }）

composer-image-process-failed = 部分粘贴的图片无法发送（格式不支持或过大）

composer-placeholder-followup = 要求后续变更…

# Split send pill (idle send / streaming steer + stop + chevron menu)
send-pill-send = 发送

send-pill-queue = 排队

send-pill-stop = 停止

send-menu-tooltip = 发送动作

send-menu-steer = 引导

send-menu-queue = 排队

send-menu-interrupt = 打断

queued-steer-now-action = 进入引导

queue-requeue-action = 重新排队

queue-row-interrupt-action = 立即打断

queued-edit-action = 编辑

queued-delete-action = 移除

message-steer-pending-badge = 待引导

message-queued-badge = 排队中

message-steered-badge = 已引导
### about.rs (About window)

about-title = 关于 Steer

about-ok = 确定

about-copy = 复制

# Background task status card











goal-popover-title = 目标

goal-popover-objective = 目标内容

goal-popover-status = 状态

goal-popover-elapsed = 已运行

goal-popover-reason = 状态原因

goal-popover-tokens = 已用 token

goal-popover-budget = token 预算

goal-popover-remaining = 剩余 token

goal-popover-rounds = 轮次

goal-popover-pause = 暂停

goal-popover-resume = 恢复

goal-popover-edit = 编辑

goal-popover-edit-budget = 编辑预算

goal-popover-edit-rounds = 编辑轮次

goal-popover-replace = 替换

goal-popover-new = 新建目标

goal-popover-clear = 清除目标

goal-status-active = 目标进行中

goal-status-paused = 目标已暂停

goal-status-blocked = 目标受阻

goal-status-budget-limited = 目标已达预算

goal-status-complete = 目标已完成

settings-item-mcp = MCP 服务器

settings-desc-mcp = 连接外部工具和数据源

settings-mcp-restart-note = 开关将在下次启动时生效。

settings-empty-mcp = 尚未配置任何 MCP 服务器。请在 mcp.toml 中添加，或安装声明了服务器的插件。

settings-mcp-status-disabled = 已禁用

settings-mcp-status-not-connected = 未连接

settings-mcp-tool-count = { $count } 个工具

plugins-search-placeholder = 搜索插件…

plugins-tab-marketplace = 市场

plugins-tab-plugin = 插件

plugins-busy = 正在处理…

plugins-select = 选择

plugins-delete = 删除

plugins-update = 更新

plugins-install = 安装

plugins-uninstall = 卸载

plugins-installed = 已安装

plugins-not-installed = 未安装

plugins-enabled = 已启用

plugins-disabled = 已禁用

plugins-enable = 启用

plugins-disable = 禁用

plugins-marketplace-url = Git URL，例如 https://github.com/org/marketplace.git

plugins-add-marketplace = 添加市场

plugins-marketplace-count = {$count} 个插件

plugins-marketplace-detail = {$name} 插件

plugins-empty-marketplaces = 尚无市场。

plugins-empty-marketplace-selection = 选择一个市场来管理其中的插件。

plugins-empty-marketplace-plugins = 此市场没有插件。

plugins-empty-installed = 尚未安装插件。

plugins-error-marketplace-url = 请输入市场 Git URL。

plugins-notice-marketplace-added = 市场已添加。

plugins-notice-marketplace-updated = 市场已更新。

plugins-notice-marketplace-removed = 市场已删除。

plugins-notice-plugin-installed = 插件已安装。重启 steer 后会加载新注册的工具、技能、agent、hook 和 MCP 服务器。

plugins-notice-plugin-removed = 插件已移除。重启 steer 后会卸载启动时加载的运行时注册表。

plugins-notice-plugin-enabled = 插件已启用。重启 steer 后会加载其工具、技能、agent、hook 和 MCP 服务器。

plugins-notice-plugin-disabled = 插件已禁用。重启 steer 后会卸载启动时加载的运行时注册表。

## steer-agent-chrome-ui（应用壳：侧栏/标题栏/页签/底部面板）

chrome-sessions-title = 会话
chrome-new = 新建
chrome-no-chats = 暂无会话
chrome-customizations = 自定义
chrome-row-copy-id = 复制 Thread ID
chrome-row-pin = 置顶
chrome-row-unpin = 取消置顶
chrome-picker-search = 搜索 session
chrome-picker-empty = 无匹配 session
chrome-tab-new-tab = 新标签页
chrome-tab-retry = 重试
chrome-titlebar-back = 后退一个会话
chrome-titlebar-forward = 前进一个会话
chrome-titlebar-open-editor = 在 VS Code 中打开
vscode-open-failed = 打开 VS Code 失败
vscode-open-no-project = 没有可打开的项目——当前会话未绑定项目目录
chrome-sidebar-grouping = 切换分组方式
chrome-sidebar-search = 搜索会话
chrome-sidebar-search-placeholder = 搜索会话
chrome-sidebar-no-match = 无匹配会话
chrome-sidebar-show-more = 显示更多（{$count} 条）
chrome-sidebar-show-less = 收起
chrome-unimplemented = 尚未实现
chrome-group-today = 今天
chrome-group-yesterday = 昨天
chrome-group-week = 最近 7 天
chrome-group-earlier = 更早
chrome-panel-empty = 面板内容已关闭
chrome-sidebar-automations = 自动化
chrome-sidebar-chats = 对话
chrome-sidebar-overview = 概览
chrome-sidebar-plugins = 插件
chrome-sidebar-mcp = MCP 服务器
chrome-sidebar-skills = 技能
chrome-tab-terminal = 终端
chrome-tab-browser = 浏览器
chrome-quick-terminal = 打开集成终端
chrome-quick-browser = 打开集成浏览器
chrome-quick-agent = 打开 { $agent }
chrome-spawn-failed = 无法启动 { $prog }：{ $err }

## steer-agent-chrome-ui assembly（chrome-shell 构建的占位主面）

chrome-main-pending = 聊天主栏装配中（下一批接入）

workspace-hero-heading = 开始你的对话
workspace-history-loading-heading = 加载对话…

chrome-tab-editor = 编辑器
chrome-quick-editor = 打开编辑器
chrome-tab-diff = Diff
chrome-editor-placeholder = 在此书写 markdown…

chrome-agent-pick-model = 为 { $agent } 选择模型

workspace-change-rejected = 更改未生效（宿主拒绝）

# Preview tool card (chat column, app-provided preview tool)
preview-unavailable = 无法渲染预览（文件可能已被移动或删除）

# New-thread landing（宿主拒绝创建新会话时的落新失败提示）
new-thread-create-failed = 新建会话失败：{ $err }
new-thread-no-session = 会话尚未就绪
