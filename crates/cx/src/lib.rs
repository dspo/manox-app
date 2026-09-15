//! cx headless 库 —— 外部 agent CLI 启动器的非交互半边。
//!
//! 承载：provider/agent 配置核心、launch-home 隔离与 codex 配置合并、
//! ChatGPT.app / VS Code Claude 的非交互 launch 与设置 API、probe 缓存 db。
//! 交互面（clap CLI、ratatui TUI、relay、stats 面板、`cx web`）在 cx-cli bin crate。
#![allow(clippy::empty_line_after_doc_comments)]
use manox_ext_agents::*;

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use dirs::home_dir;
use rand::RngCore;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(windows)]
use std::os::windows::fs::{symlink_dir, symlink_file};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

// 公开再导出：cx-cli 与嵌入方（manox 应用侧）经 cx:: 统一取用 provider 词汇。
pub use manox_providers::{
    AgentConfig, ApiKeySourceKind, ChatGptAppSettings, CopilotAuth, CxConfig,
    PROVIDER_CONFIG_FILE_NAME, ProviderConfig, ProviderEndpointSpec, ProviderModelConfig,
    ProviderModels, ResolvedAgent, ResolvedModel, VsCodeAppSettings, WireApi,
    active_provider_config_path, canonical_agent_id, context_window_from_suffix, cx_state_dir,
    effective_agents_for_model, read_config_file, resolve_apikey, resolved_agents,
};

// mod chatgpt_app; -- moved to manox-ext-agents
// mod session; -- moved to manox-ext-agents
// mod vscode_app; -- moved to manox-ext-agents
// mod warp; -- moved to manox-ext-agents
pub mod probe;

pub const LAUNCH_HOME_DIR_NAME: &str = "cx-launch-homes";
pub const LAUNCH_HOME_TTL_SECS: u64 = 60 * 60 * 24;
pub const DEFAULT_PROVIDER_CONFIG_YAML: &str = include_str!("../config/providers.default.yaml");
// Add-wizard 词汇：providers_for_agent 追加的哨兵 provider 与操作标签（cx-cli 的
// Add 向导与嵌入方按同名识别）。
pub const ADD_PROVIDER_SENTINEL: &str = "+ 添加 Provider";
pub const ADD_NEW_PROVIDER_SENTINEL: &str = "+ 新建 Provider";
pub const ADD_WIRE_API_ACTION: &str = "添加 wire_api";
pub const ADD_MODEL_ACTION: &str = "添加 model";
/// Normalize a CLI-supplied agent name: case-insensitive, with alias collapsing.
///
/// `CoDex.App` / `codexapp` / `codex_app` / `ChatGPT.App` / `chatgptapp` all resolve
/// to the `ChatGPT.app` agent; the remaining ids (`claude`, `codex`, `copilot`)
/// are lowercased verbatim. This only touches user-facing input — internal config/registry
/// ids are already canonical.
pub fn canonicalize_agent_name(input: &str) -> String {
    match input.to_lowercase().as_str() {
        "codex_app" | "codexapp" | "codex.app" | "chatgpt_app" | "chatgptapp" | "chatgpt.app" => {
            "ChatGPT.app".into()
        }
        "vscode" | "vs-code" | "vs_code" | "vs code" | "vscode.app" => "VS Code".into(),
        other => other.into(),
    }
}

pub fn find_agent(config: &CxConfig, agent_id: &str) -> Option<ResolvedAgent> {
    let normalized = canonicalize_agent_name(agent_id);
    let agent_id = canonical_agent_id(&normalized);
    resolved_agents(config)
        .into_iter()
        .find(|agent| agent.id == agent_id)
}

/// Map a `WireApi` to the launch vocabulary used by copilot's `COPILOT_PROVIDER_WIRE_API`.
/// `Unavailable` is an error here (cx probe must refresh) — unlike `WireApi::display`,
/// which is a lossless label.
pub fn wire_api_launch_value(wire_api: WireApi) -> Result<&'static str> {
    match wire_api {
        WireApi::Responses => Ok("responses"),
        WireApi::Completions => Ok("completions"),
        WireApi::Anthropic => Ok("anthropic"),
        WireApi::Unavailable => {
            bail!("该模型当前被标记为 unavailable，请先运行 `cx probe` 更新探测结果")
        }
    }
}

pub fn default_agent_configs() -> Vec<AgentConfig> {
    vec![
        AgentConfig {
            id: "copilot".into(),
            binary: "copilot".into(),
            args: Vec::new(),
            wire_apis: vec!["anthropic".into(), "responses".into(), "completions".into()],
            env: BTreeMap::new(),
        },
        AgentConfig {
            id: "claude".into(),
            binary: "claude".into(),
            args: Vec::new(),
            wire_apis: vec!["anthropic".into()],
            env: BTreeMap::new(),
        },
        AgentConfig {
            id: "codex".into(),
            binary: "codex".into(),
            args: Vec::new(),
            wire_apis: vec!["responses".into()],
            env: BTreeMap::new(),
        },
    ]
}

pub fn create_default_provider_config(path: &Path) -> Result<()> {
    write_string_atomic(path, DEFAULT_PROVIDER_CONFIG_YAML)
        .with_context(|| format!("创建默认 Provider 配置失败: {}", path.display()))?;
    eprintln!("未找到 Provider 配置，已按基线创建: {}", path.display());
    Ok(())
}

pub fn write_string_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(PROVIDER_CONFIG_FILE_NAME);
    let tmp_path = path.with_file_name(format!(".{file_name}.{}.tmp", random_urlsafe(6)));
    fs::write(&tmp_path, content)
        .with_context(|| format!("写入临时配置文件失败: {}", tmp_path.display()))?;

    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err).with_context(|| {
            format!(
                "替换配置文件失败: {} -> {}",
                tmp_path.display(),
                path.display()
            )
        });
    }

    Ok(())
}

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("创建目录失败: {}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("设置目录权限失败: {}", path.display()))?;
    Ok(())
}

pub fn write_private_file(path: &Path, content: &str) -> Result<()> {
    write_string_atomic(path, content)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("设置文件权限失败: {}", path.display()))?;
    Ok(())
}

/// 若 `dst` 已存在，在重建符号链接前先移除，保证 `materialize_passthrough_dir` 幂等。
///
/// 需处理的两种残留：
/// - 上次创建的**符号链接**（持久目录二次启动）；
/// - ChatGPT.app 用 atomic-rename 写状态文件时，把我们的符号链接**替换成的普通文件**
///   （`rename(tmp, target)` 会覆盖符号链接，产生真实文件，如 `.codex-global-state.json.bak`、
///   `logs_2.sqlite-wal`）。此时 `real` 目录里有真实数据源，重新符号链接到 `real` 即可，不丢数据。
///
/// 仅移除符号链接与普通文件；**真实目录予以保留**，避免误删 Codex 写入的状态目录（如 `sessions/`）。
pub fn remove_existing_entry(dst: &Path) {
    let Ok(meta) = fs::symlink_metadata(dst) else {
        return; // 不存在，无需处理
    };
    let ft = meta.file_type();
    if ft.is_dir() && !ft.is_symlink() {
        // 真实目录（非指向目录的符号链接）：保留，交由调用方决定是否覆盖。
        return;
    }
    // 符号链接（含指向目录的）或普通文件：移除后由 symlink_path 重建。
    #[cfg(unix)]
    {
        let _ = fs::remove_file(dst); // remove_file 对符号链接（含指向目录的）与普通文件均有效
    }
    #[cfg(windows)]
    {
        let _ = if ft.is_dir() {
            fs::remove_dir(dst)
        } else {
            fs::remove_file(dst)
        };
    }
}

#[cfg(unix)]
pub fn symlink_path(src: &Path, dst: &Path) -> Result<()> {
    remove_existing_entry(dst);
    // 若 dst 仍存在，说明是一个保留的真实目录（见 remove_existing_entry）：
    // 不覆盖、不报错，跳过本次符号链接（如 Codex 在 CODEX_HOME 自建的 sessions/）。
    if dst.exists() {
        return Ok(());
    }
    std::os::unix::fs::symlink(src, dst)
        .with_context(|| format!("创建符号链接失败: {} -> {}", dst.display(), src.display()))
}

#[cfg(windows)]
pub fn symlink_path(src: &Path, dst: &Path) -> Result<()> {
    remove_existing_entry(dst);
    if dst.exists() {
        return Ok(());
    }
    let result = if src.is_dir() {
        symlink_dir(src, dst)
    } else {
        symlink_file(src, dst)
    };
    result.with_context(|| format!("创建符号链接失败: {} -> {}", dst.display(), src.display()))
}

pub fn launch_homes_root() -> PathBuf {
    env::temp_dir().join(LAUNCH_HOME_DIR_NAME)
}

pub fn sweep_old_launch_homes() -> Result<()> {
    let root = launch_homes_root();
    if !root.exists() {
        return Ok(());
    }

    let now = current_unix_secs()?;
    for entry in fs::read_dir(&root).with_context(|| format!("读取目录失败: {}", root.display()))?
    {
        let entry = entry.with_context(|| format!("读取目录项失败: {}", root.display()))?;
        let path = entry.path();
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs());

        match modified {
            Some(modified) if now.saturating_sub(modified) <= LAUNCH_HOME_TTL_SECS => continue,
            _ => {}
        }

        if path.is_dir() {
            let _ = fs::remove_dir_all(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    }

    Ok(())
}

pub fn create_launch_home(agent_id: &str) -> Result<PathBuf> {
    sweep_old_launch_homes()?;
    let root = launch_homes_root();
    ensure_private_dir(&root)?;
    let dir = root.join(format!(
        "{}-{}-{}",
        agent_id,
        current_unix_secs()?,
        random_urlsafe(6)
    ));
    ensure_private_dir(&dir)?;
    Ok(dir)
}

pub fn mirror_home_entries(real_home: &Path, fake_home: &Path, excluded: &[&str]) -> Result<()> {
    ensure_private_dir(fake_home)?;
    for entry in fs::read_dir(real_home)
        .with_context(|| format!("读取主目录失败: {}", real_home.display()))?
    {
        let entry = entry.with_context(|| format!("读取目录项失败: {}", real_home.display()))?;
        let name = entry.file_name();
        let name_string = name.to_string_lossy();
        if excluded
            .iter()
            .any(|excluded_name| *excluded_name == name_string)
        {
            continue;
        }
        symlink_path(&entry.path(), &fake_home.join(&name))?;
    }
    Ok(())
}

pub fn materialize_passthrough_dir(
    real_dir: &Path,
    fake_dir: &Path,
    overridden: &[&str],
) -> Result<()> {
    ensure_private_dir(fake_dir)?;
    if !real_dir.exists() {
        return Ok(());
    }

    for entry in
        fs::read_dir(real_dir).with_context(|| format!("读取目录失败: {}", real_dir.display()))?
    {
        let entry = entry.with_context(|| format!("读取目录项失败: {}", real_dir.display()))?;
        let name = entry.file_name();
        let name_string = name.to_string_lossy();
        if overridden
            .iter()
            .any(|overridden_name| *overridden_name == name_string)
        {
            continue;
        }
        symlink_path(&entry.path(), &fake_dir.join(&name))?;
    }
    Ok(())
}

pub fn toml_basic_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// 把 provider 名称转成 ASCII-safe 的 config.toml provider key。
/// 非字母数字/`-`/`_` 字符被丢弃（如中文「百炼」→ ""）；结果为空时回落到 "custom"。
pub fn provider_config_key(provider_name: &str) -> String {
    let slug: String = provider_name
        .chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else if c == '-' || c == '_' {
                Some(c)
            } else {
                None
            }
        })
        .collect();
    if slug.is_empty() {
        "custom".to_string()
    } else {
        slug
    }
}

/// 从 apikey_source 推导 Codex config.toml 的 `env_key`（Codex 运行时读取 API Key 的环境变量名）。
/// `keychain:VAR` / `env:VAR` → `VAR`；其余（`literal:` / `$(shell ...)` / None）回落到 `CX_PROVIDER_KEY`。
pub fn env_key_for_apikey_source(source: Option<&str>) -> String {
    if let Some(s) = source
        && let Some(rest) = s
            .strip_prefix("keychain:")
            .or_else(|| s.strip_prefix("env:"))
    {
        let v = rest.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }
    "CX_PROVIDER_KEY".to_string()
}

/// 从既有 config.toml 文本中提取顶层 `model_reasoning_effort` 的值（去引号）。
/// 用于在重写 config 时保留用户偏好，而非硬编码覆盖。
/// 仅匹配任何 `[section]` 之前的顶层键，避免误取 `[model_providers.*]` 等段内同名字段。
pub fn extract_reasoning_effort(existing: Option<&str>) -> Option<String> {
    let existing = existing?;
    for line in existing.lines() {
        let trimmed = line.trim();
        // 进入任意 section 后，顶层键已结束，停止扫描。
        if trimmed.starts_with('[') {
            break;
        }
        if let Some(after) = trimmed.strip_prefix("model_reasoning_effort") {
            let after = after.trim_start();
            if let Some(rhs) = after.strip_prefix('=') {
                let v = rhs.trim().trim_matches('"');
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// 把 `WireApi` 映射成 codex 家族 config.toml 的 `wire_api` 词汇。
/// codex（ChatGPT.app 引擎）读取的规范值为 `responses` / `chat_completions` / `anthropic_messages`，
/// 与 cx 内部 `WireApi::launch_value()`（供 copilot 的 `COPILOT_PROVIDER_WIRE_API` 使用，
/// 值为 `responses` / `completions` / `anthropic`）不同，故单独提供。
pub fn codex_wire_api_str(wire_api: WireApi) -> Result<&'static str> {
    match wire_api {
        WireApi::Responses => Ok("responses"),
        WireApi::Completions => Ok("chat_completions"),
        WireApi::Anthropic => Ok("anthropic_messages"),
        WireApi::Unavailable => {
            bail!("该模型当前被标记为 unavailable，请先运行 `cx probe` 更新探测结果")
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn merge_codex_config(
    existing: Option<&str>,
    model: &ResolvedModel,
    workspace_root: &Path,
    wire_api: WireApi,
    provider_key: &str,
    provider_name: &str,
    env_key: &str,
    // 发给 provider 的 model id（已剥除 cx 内部上下文后缀）。
    api_model_id: &str,
    // 模型上下文窗口 token 数（来自上下文后缀，如 `[1m]`/`[200k]`），写入 codex config.toml 的
    // `model_context_window`。None 则不写、保留用户既有值。
    context_window: Option<i64>,
    // 本地模型目录（ModelsResponse JSON）路径，写入 config.toml 的 `model_catalog_json`。
    // 让引擎「认识」自定义模型、跳过 fallback 元数据对上下文窗口的钳制。None 则不写。
    model_catalog_json: Option<&str>,
) -> Result<String> {
    let project_section = format!(
        "[projects.{}]",
        toml_basic_string(&workspace_root.to_string_lossy())
    );
    // 保留用户已设置的 reasoning effort（若无则回落到 "high"，与 POC 一致）。
    let reasoning_effort = extract_reasoning_effort(existing).unwrap_or_else(|| "high".to_string());
    let mut retained = Vec::new();
    let mut skipping_section = false;

    if let Some(existing) = existing {
        for line in existing.lines() {
            let trimmed = line.trim();
            if skipping_section {
                if trimmed.starts_with('[') && trimmed.ends_with(']') {
                    skipping_section = false;
                } else {
                    continue;
                }
            }

            // 所有 [model_providers.*] section（含历史硬编码的 dashscope）都由本次重新生成，保留时整体跳过
            if trimmed.starts_with("[model_providers.") || trimmed == project_section {
                skipping_section = true;
                continue;
            }

            if !trimmed.starts_with('[')
                && (trimmed.starts_with("model =")
                    || trimmed.starts_with("model_provider =")
                    || trimmed.starts_with("model_reasoning_effort =")
                    // cx 本次要重写 model_context_window 时，剥离用户旧值以免冲突。
                    || (context_window.is_some()
                        && trimmed.starts_with("model_context_window ="))
                    // cx 本次要重写 model_catalog_json 时，剥离用户旧值以免冲突。
                    || (model_catalog_json.is_some()
                        && trimmed.starts_with("model_catalog_json =")))
            {
                continue;
            }

            retained.push(line.to_string());
        }
    }

    let wire_api_str = codex_wire_api_str(wire_api)?;
    let context_window_line = context_window
        .map(|n| format!("model_context_window = {n}\n"))
        .unwrap_or_default();
    let model_catalog_json_line = model_catalog_json
        .map(|p| format!("model_catalog_json = {}\n", toml_basic_string(p)))
        .unwrap_or_default();
    let mut rendered = format!(
        "model = {}\nmodel_provider = {}\nmodel_reasoning_effort = {}\n{}{}[model_providers.{}]\nname = {}\nbase_url = {}\nenv_key = {}\nwire_api = {}\n\n{}{}\ntrust_level = \"trusted\"\n",
        toml_basic_string(api_model_id),
        toml_basic_string(provider_key),
        toml_basic_string(&reasoning_effort),
        context_window_line,
        model_catalog_json_line,
        toml_basic_string(provider_key),
        toml_basic_string(provider_name),
        toml_basic_string(&model.endpoint_url),
        toml_basic_string(env_key),
        toml_basic_string(wire_api_str),
        project_section,
        "\n"
    );

    let retained = retained.join("\n").trim().to_string();
    if !retained.is_empty() {
        rendered.push('\n');
        rendered.push_str(&retained);
        rendered.push('\n');
    }

    Ok(rendered)
}

// Single implementation lives in manox-ext-agents; re-exported so `cx::` stays
// the one-stop vocabulary for consumers of this crate.
pub use manox_ext_agents::parse_model_context_suffix;

/// 在 merge_codex_config 渲染结果中注入 `supports_websockets = <bool>`。
/// 插入点是首个 `wire_api = ...` 行之后——merge_codex_config 会整体丢弃用户
/// `[model_providers.*]` 段并只重新生成本次注入的一个，因此首个匹配即注入段；
/// 保留的用户内容排在其后，不受影响。找不到插入点时原样返回（防御）。

pub fn prepare_codex_launch_home(
    model: &ResolvedModel,
    provider: &ResolvedProvider,
    apikey: String,
    env: &mut BTreeMap<String, String>,
    wire_api: WireApi,
) -> Result<()> {
    let real_home = home_dir().context("无法解析用户主目录")?;
    let fake_home = create_launch_home("codex")?;
    mirror_home_entries(&real_home, &fake_home, &[".codex"])?;

    let real_codex_dir = real_home.join(".codex");
    let fake_codex_dir = fake_home.join(".codex");
    materialize_passthrough_dir(&real_codex_dir, &fake_codex_dir, &["config.toml"])?;

    let provider_key = provider_config_key(&provider.name);
    let env_key = env_key_for_apikey_source(provider.apikey_source.as_deref());
    let existing_config = fs::read_to_string(real_codex_dir.join("config.toml")).ok();
    // 剥除上下文后缀：provider 接收的是 base id（如 glm-5.2），
    // 1m 上下文信息写入 codex 的 model_context_window。
    let (api_model_id, context_window) = parse_model_context_suffix(&model.id);
    let merged_config = merge_codex_config(
        existing_config.as_deref(),
        model,
        &env::current_dir()?,
        wire_api,
        &provider_key,
        &provider.name,
        &env_key,
        api_model_id,
        context_window,
        None,
    )?;
    write_private_file(&fake_codex_dir.join("config.toml"), &merged_config)?;

    env.insert(env_key.clone(), apikey);
    env.insert("HOME".into(), fake_home.display().to_string());
    env.insert(
        "XDG_CONFIG_HOME".into(),
        fake_home.join(".config").display().to_string(),
    );
    env.insert("CODEX_HOME".into(), fake_codex_dir.display().to_string());
    Ok(())
}

/// 为 Codex Desktop App 准备注入配置。
/// 使用固定目录 ~/.manox/.codex/，Symlink 真实 ~/.codex/ 内容（config.toml 除外），
/// 写入我们注入的 config.toml（动态 provider key / env_key）。Codex Desktop 读 CODEX_HOME 指向此目录。
///
/// 返回 `ChatGptAppPrepared`：codex_home 供调用方在启动子进程时设 `CODEX_HOME` 环境变量，
/// env_key 是 config.toml 里 Codex 运行时读取 API Key 的环境变量名，
/// reasoning_effort 是解析出的（或默认 "high"）推理强度，供注入脚本与下拉默认值保持一致。

/// `prepare_chatgpt_launch_home_for_app` 的产物，供 chatgpt_app 启动编排使用。
// struct ChatGptAppPrepared -- now in manox-ext-agents

pub fn load_config() -> Result<CxConfig> {
    let path = active_provider_config_path()?;
    if !path.exists() {
        create_default_provider_config(&path)?;
    }
    read_config_file(&path)
}

pub fn load_config_for_add() -> Result<(CxConfig, PathBuf)> {
    let path = active_provider_config_path()?;
    let config = if path.exists() {
        read_config_file(&path)?
    } else {
        CxConfig {
            providers: Vec::new(),
            agents: default_agent_configs(),
            chatgpt_app: None,
            vscode_app: None,
            subagents: Default::default(),
        }
    };
    Ok((config, path))
}

pub fn save_config(path: &Path, config: &CxConfig) -> Result<()> {
    let yaml = serde_yaml::to_string(config).context("序列化配置失败")?;
    write_string_atomic(path, &yaml)
}

pub fn current_unix_secs() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 Unix Epoch")?
        .as_secs())
}

pub fn random_urlsafe(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

pub fn build_all_models(config: &CxConfig) -> Vec<ResolvedModel> {
    let mut models = Vec::new();
    for provider in &config.providers {
        match resolved_models_for_provider(config, provider) {
            Ok(mut resolved) => models.append(&mut resolved),
            Err(e) => {
                eprintln!(
                    "警告: 获取 Provider `{}` 的 models 失败: {e}",
                    provider.name
                );
            }
        }
    }
    models
}

/// 解析单个 provider 下的全部模型（在线拉取 + endpoint 归一）。
/// 网络/解析失败以 Err 返回，由调用方决定可见性：启动路径回落跳过，
/// Settings 目录则以「加载失败」条目呈现，避免把网络故障误归因于配置。
pub fn resolved_models_for_provider(
    config: &CxConfig,
    provider: &ProviderConfig,
) -> Result<Vec<ResolvedModel>> {
    let fetched = probe::runtime().block_on(provider.list_models())?;
    let mut models = Vec::new();
    for endpoint in provider.normalized_endpoints_resolved(&fetched) {
        for model in &endpoint.models {
            models.push(ResolvedModel::from_config(
                config, provider, &endpoint, model,
            ));
        }
    }
    Ok(models)
}

pub fn provider_supports_agent(
    config: &CxConfig,
    provider: &ProviderConfig,
    agent_id: &str,
) -> bool {
    let agent_id = canonical_agent_id(agent_id);
    if !provider.has_endpoints() {
        return true;
    }
    // Remote-model providers: assume compatible until models are fetched at
    // selection time; the actual model list determines compatibility then.
    if provider.is_remote_models() {
        return true;
    }

    provider.normalized_endpoints().iter().any(|endpoint| {
        endpoint.models.iter().any(|model| {
            effective_agents_for_model(config, provider, endpoint, model)
                .iter()
                .any(|candidate| canonical_agent_id(candidate) == agent_id)
        })
    })
}

pub fn apply_probe_cache(models: &mut [ResolvedModel]) {
    let db_path = match cx_state_dir().map(|d| d.join("cx.db")) {
        Ok(p) => p,
        Err(_) => return,
    };

    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    for model in models.iter_mut() {
        match probe::db::get_available_wire_api(&conn, &model.provider_name, &model.id) {
            Ok(Some(wire_api)) => {
                model.wire_api = wire_api;
            }
            Ok(None) => {
                // 没有可用的 wire_api，标记为 Unavailable
                model.wire_api = WireApi::Unavailable;
            }
            Err(_) => {
                // 查询失败，保持原有 wire_api
            }
        }
    }
}

pub fn providers_for_agent(config: &CxConfig, agent_id: &str) -> Vec<ResolvedProvider> {
    let agent_id = canonical_agent_id(agent_id);
    let mut providers: Vec<ResolvedProvider> = config
        .providers
        .iter()
        .filter(|provider| provider_supports_agent(config, provider, agent_id))
        .map(ResolvedProvider::from_config)
        .collect();

    // Append the "add provider" sentinel
    providers.push(ResolvedProvider {
        name: ADD_PROVIDER_SENTINEL.to_string(),
        has_endpoints: false,
        apikey_source: None,
        env: BTreeMap::new(),
    });
    providers
}

pub fn model_options_for_provider(
    all_models: &[ResolvedModel],
    agent_id: &str,
    provider_name: &str,
) -> Vec<ModelOption> {
    let mut grouped: Vec<Vec<ResolvedModel>> = Vec::new();
    let mut indexes_by_id: BTreeMap<String, usize> = BTreeMap::new();

    for model in all_models
        .iter()
        .filter(|m| m.provider_name == provider_name && resolved_model_supports_agent(m, agent_id))
    {
        if let Some(index) = indexes_by_id.get(&model.id).copied() {
            grouped[index].push(model.clone());
            continue;
        }

        indexes_by_id.insert(model.id.clone(), grouped.len());
        grouped.push(vec![model.clone()]);
    }

    grouped
        .into_iter()
        .map(ModelOption::from_variants)
        .collect()
}

/// 收集某 provider 下所有支持 ChatGPT.app（即 wire 含 Responses）的模型，作为注入桌面端的完整列表。
///
/// 同一 model id 仅保留一条，按 model id 升序排序。首个元素即默认模型。
pub fn injected_models_for_chatgpt_app(
    all_models: &[ResolvedModel],
    provider_name: &str,
) -> Vec<ResolvedModel> {
    let mut models: Vec<ResolvedModel> = all_models
        .iter()
        .filter(|m| {
            m.provider_name == provider_name
                && resolved_model_supports_agent(m, "ChatGPT.app")
                // 显式确认模型支持 Responses wire api。supports_agent 已隐含这点
                // （ChatGPT.app agent 仅兼容 Responses endpoint），但这里再过滤一次，
                // 防止配置层不变量将来变动时把非 Responses 模型注入、生成错误的 wire_api。
                && m.model_wire_apis.contains(&WireApi::Responses)
        })
        .cloned()
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen = std::collections::HashSet::new();
    models.retain(|m| seen.insert(m.id.clone()));
    models
}

/// 构造 ChatGPT.app 启动所需的 `Selection`：provider 下全部 Responses 模型作为注入目录，
/// 调用方选中的默认模型置顶（`injected_models[0]` = 默认模型的既有约定，
/// 写入 config.toml 的 `model =` 并在注入脚本里标记 `isDefault`）。
pub fn build_chatgpt_selection(
    config: &CxConfig,
    all_models: &[ResolvedModel],
    provider_name: &str,
    default_model_id: &str,
) -> Result<Selection> {
    let agent = find_agent(config, "ChatGPT.app").context("配置中缺少 `ChatGPT.app` agent")?;
    let provider = providers_for_agent(config, "ChatGPT.app")
        .into_iter()
        .find(|p| p.name == provider_name)
        .with_context(|| format!("`ChatGPT.app` 下未找到 provider `{provider_name}`"))?;
    let mut injected = injected_models_for_chatgpt_app(all_models, &provider.name);
    if injected.is_empty() {
        bail!(
            "Provider `{provider_name}` 下没有支持 Responses wire api 的模型，无法注入 ChatGPT.app"
        );
    }
    let index = injected
        .iter()
        .position(|m| m.id == default_model_id)
        .with_context(|| {
            format!("Provider `{provider_name}` 的注入目录中未找到模型 `{default_model_id}`")
        })?;
    let default_model = injected.remove(index);
    injected.insert(0, default_model.clone());
    Ok(Selection {
        agent_id: agent.id,
        agent_binary: agent.binary,
        agent_args: agent.args,
        agent_env: agent.env,
        selected_wire_api: WireApi::Responses,
        provider,
        model: Some(default_model),
        injected_models: injected,
    })
}

/// ChatGPT.app 的 API Key 非交互解析（GUI 嵌入路径）：无 stdin 可补齐，缺失即报错。
pub fn resolve_chatgpt_app_apikey(provider: &ResolvedProvider) -> Result<String> {
    let Some(source) = provider.apikey_source.as_deref() else {
        bail!(
            "Provider `{}` 需要 API Key 但未配置 apikey_source",
            provider.name
        );
    };
    let apikey = resolve_apikey(source)
        .with_context(|| format!("解析 Provider `{}` 的 API Key 失败", provider.name))?;
    if apikey.is_empty() {
        bail!("ChatGPT.app 需要 API Key，但未提供");
    }
    Ok(apikey)
}

/// ChatGPT.app 的 API Key 交互解析（CLI 路径）：Keychain 缺失时提示输入并回写。
pub fn resolve_chatgpt_app_apikey_interactive(provider: &ResolvedProvider) -> Result<String> {
    let Some(source) = provider.apikey_source.as_deref() else {
        bail!(
            "Provider `{}` 需要 API Key 但未配置 apikey_source",
            provider.name
        );
    };
    let apikey = resolve_apikey_interactive(source)?;
    if apikey.is_empty() {
        bail!("ChatGPT.app 需要 API Key，但未提供");
    }
    Ok(apikey)
}

/// 非交互启动 ChatGPT.app：显式指定 provider 与默认模型，无 TUI。供 GUI 嵌入方
/// （manox 系统菜单）调用——选中模型作为默认模型，同时注入该 provider 下全部
/// Responses 模型目录。阻塞调用（配置加载、模型列表构建、CDP 注入最长约 20s），
/// 调用方应在后台线程执行。
pub fn launch_chatgpt_app(provider_name: &str, default_model_id: &str) -> Result<()> {
    let config = load_config()?;
    let chatgpt_settings = config.chatgpt_app.clone().unwrap_or_default();
    let mut all_models = build_all_models(&config);
    apply_probe_cache(&mut all_models);
    let selection = build_chatgpt_selection(&config, &all_models, provider_name, default_model_id)?;
    let apikey = resolve_chatgpt_app_apikey(&selection.provider)?;
    manox_ext_agents::chatgpt_app::launch_with_injection(
        &selection,
        &apikey,
        &[],
        &chatgpt_settings,
    )
}

/// ChatGPT.app 注入使用的 CODEX_HOME 目录（Settings 只读展示）。
pub fn chatgpt_codex_home() -> Result<PathBuf> {
    Ok(cx_state_dir()?.join(".codex"))
}

/// 加载 ChatGPT.app 注入设置（`chatgpt_app:` 段缺失时返回默认空设置）。
pub fn chatgpt_app_settings() -> Result<ChatGptAppSettings> {
    Ok(load_config()?.chatgpt_app.unwrap_or_default())
}

/// `supports_websockets: Some(false)` 归一为 `None`（启动端 `None` 按 false
/// 处理，语义等价），`nickname` 空白归一为 `None`，使净零操作与
/// `default()` 相等、整段可省略。纯函数，便于单测。
pub fn normalize_chatgpt_app_settings(mut settings: ChatGptAppSettings) -> ChatGptAppSettings {
    if settings.supports_websockets == Some(false) {
        settings.supports_websockets = None;
    }
    if let Some(nickname) = settings.nickname.take() {
        let trimmed = nickname.trim();
        if trimmed.is_empty() {
            settings.nickname = None;
        } else {
            settings.nickname = Some(trimmed.to_string());
        }
    }
    settings
}

/// ChatGPT.app 注入时 provider 的展示名：昵称（trim 非空）优先，否则
/// provider 本名。CLI 与 GUI 启动路径共用，保证 config.toml 的
/// `[model_providers.*] name` 与启动摘要一致。
pub fn chatgpt_provider_display_name<'a>(
    provider_name: &'a str,
    chatgpt_settings: &'a ChatGptAppSettings,
) -> &'a str {
    chatgpt_settings
        .nickname
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(provider_name)
}

/// 保存 ChatGPT.app 注入设置（保留配置文件的 providers/agents 等其余段）。
pub fn save_chatgpt_app_settings(settings: &ChatGptAppSettings) -> Result<()> {
    let (mut config, path) = load_config_for_add()?;
    // 归一后全默认时整段省略，避免净零操作残留 `chatgpt_app:` 段。
    let normalized = normalize_chatgpt_app_settings(settings.clone());
    config.chatgpt_app = if normalized == ChatGptAppSettings::default() {
        None
    } else {
        Some(normalized)
    };
    save_config(&path, &config)
}

/// ChatGPT.app 可注入模型目录：provider 名 → 可注入 model id 列表，
/// 或该 provider 的拉取错误（面板以「加载失败」呈现，不静默省略）。
pub type ChatGptInjectableCatalog = Vec<(String, Result<Vec<String>, String>)>;

/// ChatGPT.app 可注入模型目录（Settings 只读展示）：每个支持 ChatGPT.app
/// 注入的 provider → 其可注入模型（Responses 能力 + ChatGPT.app 可见）。
/// per-provider 拉取失败以 Err 条目返回（面板显示「加载失败」），不静默省略。
pub fn chatgpt_injectable_catalog() -> Result<ChatGptInjectableCatalog> {
    let config = load_config()?;
    let mut catalog = Vec::new();
    for provider in &config.providers {
        if !provider_supports_agent(&config, provider, "ChatGPT.app") {
            continue;
        }
        // 与启动候选同一谓词：无 endpoints 的 provider 无注入目标，不列入目录。
        if !provider.has_endpoints() {
            continue;
        }
        let entry = match resolved_models_for_provider(&config, provider) {
            Ok(resolved) => Ok(injected_models_for_chatgpt_app(&resolved, &provider.name)
                .into_iter()
                .map(|m| m.id)
                .collect()),
            Err(e) => Err(format!("{e:#}")),
        };
        catalog.push((provider.name.clone(), entry));
    }
    Ok(catalog)
}

/// VS Code Claude Code Extension 可注入模型目录（Settings 只读展示）：每个
/// 支持 VS Code（Anthropic wire）的 provider → 其可见模型。per-provider
/// 拉取失败以 Err 条目返回（面板显示「加载失败」），不静默省略。
pub type VsCodeClaudeCatalog = Vec<(String, Result<Vec<String>, String>)>;

/// VS Code Claude Code Extension 可注入模型目录。
pub fn vscode_claude_injectable_catalog() -> Result<VsCodeClaudeCatalog> {
    let config = load_config()?;
    let mut catalog = Vec::new();
    for provider in &config.providers {
        if !provider_supports_agent(&config, provider, "VS Code") {
            continue;
        }
        // 与启动候选同一谓词：无 endpoints 的 provider 无注入目标，不列入目录。
        if !provider.has_endpoints() {
            continue;
        }
        let entry = match resolved_models_for_provider(&config, provider) {
            Ok(resolved) => Ok(injected_models_for_vscode_claude(&resolved, &provider.name)
                .into_iter()
                .map(|m| m.id)
                .collect()),
            Err(e) => Err(format!("{e:#}")),
        };
        catalog.push((provider.name.clone(), entry));
    }
    Ok(catalog)
}

/// 收集某 provider 下所有 VS Code 可见（Anthropic wire）的模型，按 model id
/// 升序排序，同一 id 仅保留一条。首个元素即默认模型（与 ChatGPT.app 目录同约定）。
pub fn injected_models_for_vscode_claude(
    all_models: &[ResolvedModel],
    provider_name: &str,
) -> Vec<ResolvedModel> {
    let mut models: Vec<ResolvedModel> = all_models
        .iter()
        .filter(|m| {
            m.provider_name == provider_name
                && resolved_model_supports_agent(m, "VS Code")
                // 显式确认 Anthropic wire（supports_agent 已隐含，防配置层不变量漂移，
                // 与 build_vscode_selection 的过滤保持一致）。
                && m.model_wire_apis.contains(&WireApi::Anthropic)
        })
        .cloned()
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen = std::collections::HashSet::new();
    models.retain(|m| seen.insert(m.id.clone()));
    models
}

/// cx 托管的基础保留环境变量键（与 provider 无关）。
pub fn base_reserved_env_keys() -> Vec<String> {
    vec![
        "CODEX_HOME".to_string(),
        "CX_MODEL".to_string(),
        "CX_WARP_SESSION_ID".to_string(),
    ]
}

/// cx 托管的保留环境变量键：用户自定义 env 不允许覆盖。
/// 启动注入端使用：基础保留键 + 所选 provider 的 env_key。

/// 保存校验端保留键：基础保留键 + 全部 provider 的 env_key。
/// 是启动端过滤的超集（「校验 ⊇ 启动过滤」不变量，见测试）。
pub fn all_reserved_env_keys(config: &CxConfig) -> Vec<String> {
    let mut keys = base_reserved_env_keys();
    for provider in &config.providers {
        keys.push(env_key_for_apikey_source(provider.apikey_source.as_deref()));
    }
    keys
}

/// 校验 ChatGPT.app 自定义环境变量键：与 cx 保留键
/// （CODEX_HOME / CX_MODEL / CX_WARP_SESSION_ID / 各 provider env_key）冲突即报错。
pub fn validate_chatgpt_custom_env(env: &BTreeMap<String, String>) -> Result<()> {
    if env.is_empty() {
        return Ok(());
    }
    let config = load_config()?;
    let reserved = all_reserved_env_keys(&config);
    let offenders: Vec<&str> = env
        .keys()
        .filter(|k| reserved.iter().any(|r| r == *k))
        .map(|s| s.as_str())
        .collect();
    if offenders.is_empty() {
        Ok(())
    } else {
        bail!("自定义环境变量与 cx 保留键冲突: {}", offenders.join(", "))
    }
}

/// 构造 VS Code 启动所需的 `Selection`：选中模型即唯一 BYOK 目标（无模型目录
/// 注入——Claude Code 直接消费 ANTHROPIC_MODEL / ANTHROPIC_BASE_URL env）。

/// VS Code 的 API Key 非交互解析（GUI 嵌入路径）：无 stdin 可补齐，缺失即报错。

/// 加载 VS Code 注入设置（`vscode_app:` 段缺失时返回默认空设置）。
pub fn vscode_app_settings() -> Result<VsCodeAppSettings> {
    Ok(load_config()?.vscode_app.unwrap_or_default())
}

/// 保存 VS Code 注入设置（保留配置文件的 providers/agents 等其余段）。
/// 归一后全默认时整段省略；双「不注入」不等于默认，必须保留。
pub fn save_vscode_app_settings(settings: &VsCodeAppSettings) -> Result<()> {
    let (mut config, path) = load_config_for_add()?;
    config.vscode_app = normalize_vscode_app_settings(settings.clone());
    save_config(&path, &config)
}

pub fn normalize_vscode_app_settings(settings: VsCodeAppSettings) -> Option<VsCodeAppSettings> {
    if settings == VsCodeAppSettings::default() {
        None
    } else {
        Some(settings)
    }
}

/// VS Code 的 Claude Code 扩展注入部分（ANTHROPIC_* BYOK env）。
// VsCode types and functions -- now in manox-ext-agents

// ══════════════════════════════════════════════════
// Secret Prompting — 交互式补齐缺失的 API Key
// ══════════════════════════════════════════════════

pub fn resolve_apikey_interactive(source: &str) -> Result<String> {
    match resolve_apikey(source) {
        Ok(key) => Ok(key),
        Err(e) => {
            if let Some(service) = source.strip_prefix("keychain:") {
                if !cfg!(target_os = "macos") {
                    bail!("`keychain:` 仅支持 macOS Keychain，请改用 `env:` 或在 macOS 上运行。");
                }
                eprintln!("从 Keychain 读取 `{service}` 失败: {e}");
                eprint!("请输入 {service} 的 API Key: ");

                let mut input = String::new();
                io::stdin().read_line(&mut input).context("读取输入失败")?;
                let key = input.trim().to_string();

                if key.is_empty() {
                    bail!("API Key 不能为空");
                }

                let user = env::var("USER").unwrap_or_default();
                let child = Command::new("security")
                    .args([
                        "add-generic-password",
                        "-a",
                        &user,
                        "-s",
                        service,
                        "-w",
                        &key,
                        "-U",
                    ])
                    .spawn()
                    .with_context(|| "写入 Keychain 失败")?;

                let output = child
                    .wait_with_output()
                    .with_context(|| "等待 Keychain 写入完成失败")?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    bail!("写入 Keychain 失败: {}", stderr.trim());
                }

                eprintln!("已将 API Key 保存到 Keychain `{service}`。");
                Ok(key)
            } else if let Some(var) = source.strip_prefix("env:") {
                bail!("环境变量 `{var}` 未设置，请通过 `export {var}=<your-key>` 设置后重试")
            } else {
                Err(e)
            }
        }
    }
}
