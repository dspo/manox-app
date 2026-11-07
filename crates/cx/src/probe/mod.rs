use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use std::time::Instant;

use anyhow::{Context, Result};
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use serde_json::json;
use tokio::runtime::Runtime;

use crate::{
    CopilotAuth, CxConfig, ProviderConfig, WireApi, build_all_models, cx_state_dir, resolve_apikey,
};

use crate::probe::types::{ProbeCellResult, ProbeRow, ProbeStatus};

pub mod db;
pub mod types;

pub fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().unwrap())
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("创建 HTTP client 失败")
    })
}

/// 运行自动探测（不启动 TUI）
pub fn run_probe_auto(config: &CxConfig, provider_filter: Option<String>) -> Result<()> {
    let db_path = cx_state_dir()?.join("cx.db");
    let conn = rusqlite::Connection::open(&db_path)
        .with_context(|| format!("打开数据库失败: {}", db_path.display()))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    db::init_probe_schema(&conn)?;

    let rows = build_probe_rows(config, &conn, provider_filter)?;

    println!("开始自动探测...");
    let mut total = 0;
    let mut success = 0;
    let mut failed = 0;

    for row in &rows {
        for wire_api in [WireApi::Anthropic, WireApi::Responses, WireApi::Completions] {
            if let Some(result) = row.results.get(&wire_api) {
                if result.status == ProbeStatus::NotApplicable {
                    continue;
                }

                if let Some(provider) = config
                    .providers
                    .iter()
                    .find(|p| p.name == row.provider_name)
                {
                    let endpoint = provider
                        .normalized_endpoints()
                        .into_iter()
                        .find(|e| WireApi::from_str(&e.wire_api) == wire_api);

                    if let Some(endpoint) = endpoint {
                        total += 1;
                        let auth = CopilotAuth::from_endpoint(&endpoint);
                        let configured = row
                            .results
                            .get(&wire_api)
                            .map(|r| r.configured)
                            .unwrap_or(true);
                        let result =
                            do_probe(provider, &endpoint.url, wire_api, &row.model_id, auth);

                        match result {
                            Ok(mut result) => {
                                result.configured = configured;
                                db::save_probe_result(
                                    &conn,
                                    &row.provider_name,
                                    &row.model_id,
                                    wire_api,
                                    &result,
                                )?;
                                if result.status == ProbeStatus::Available {
                                    success += 1;
                                    println!(
                                        "✓ {} {} {} - {}ms",
                                        row.provider_name,
                                        row.model_id,
                                        wire_api.display(),
                                        result.latency_ms.unwrap_or(0)
                                    );
                                } else {
                                    failed += 1;
                                    println!(
                                        "✗ {} {} {} - {:?}",
                                        row.provider_name,
                                        row.model_id,
                                        wire_api.display(),
                                        result.status
                                    );
                                }
                            }
                            Err(e) => {
                                failed += 1;
                                println!(
                                    "✗ {} {} {} - 错误: {}",
                                    row.provider_name,
                                    row.model_id,
                                    wire_api.display(),
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    println!(
        "\n探测完成: 总计={}, 成功={}, 失败={}",
        total, success, failed
    );
    Ok(())
}

pub fn build_probe_rows(
    config: &CxConfig,
    conn: &rusqlite::Connection,
    provider_filter: Option<String>,
) -> Result<Vec<ProbeRow>> {
    let all_models = build_all_models(config);
    let probe_results = db::load_probe_results(conn)?;

    // 解析 provider 筛选条件
    let filter_providers: Option<Vec<String>> = provider_filter.as_ref().map(|s| {
        s.split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    });

    let mut model_set = std::collections::BTreeSet::new();
    for model in &all_models {
        model_set.insert((model.provider_name.clone(), model.id.clone()));
    }

    let mut rows = Vec::new();
    for (provider_name, model_id) in model_set {
        // 如果指定了 provider 筛选，则只显示匹配的 provider
        if let Some(ref filters) = filter_providers
            && !filters.iter().any(|f| f == &provider_name)
        {
            continue;
        }

        let mut results = HashMap::new();

        for wire_api in [WireApi::Anthropic, WireApi::Responses, WireApi::Completions] {
            let key = probe_result_key(&provider_name, &model_id, wire_api);

            // 检查用户是否配置了该 wire_api
            let configured = all_models.iter().any(|m| {
                m.provider_name == provider_name && m.id == model_id && m.wire_api == wire_api
            });

            if let Some(result) = probe_results.get(&key) {
                let mut result = result.clone();
                result.configured = configured;
                results.insert(wire_api, result);
            } else {
                // 未探测过，但始终显示（不隐藏未配置的）
                results.insert(
                    wire_api,
                    ProbeCellResult {
                        status: ProbeStatus::Unknown,
                        latency_ms: None,
                        http_status: None,
                        error_message: None,
                        configured,
                    },
                );
            }
        }

        rows.push(ProbeRow {
            provider_name,
            model_id,
            results,
        });
    }

    Ok(rows)
}

pub fn probe_result_key(provider_name: &str, model_id: &str, wire_api: WireApi) -> String {
    format!("{}\0{}\0{}", provider_name, wire_api.display(), model_id)
}

/// Session id stand-in for one probe process: config headers templated with
/// `{{ session_id }}` (e.g. `x-opencode-session`) render against it, so a
/// session-aware endpoint receives the header shape it expects. Production
/// pins the persisted session id per conversation; a probe run is its own
/// one-session cohort.
fn probe_session_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// Replace every whitespace-tolerant `{{ session_id }}` interpolation with
/// `session_id`. Other template constructs pass through untouched (the
/// caller drops values that still look templated).
fn render_session_id_template(value: &str, session_id: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        let Some(end) = rest[start..].find("}}").map(|offset| start + offset + 2) else {
            // Unclosed braces: keep the tail verbatim; the caller drops it.
            out.push_str(rest);
            return out;
        };
        let inner = rest[start + 2..end - 2].trim();
        if inner == "session_id" {
            out.push_str(&rest[..start]);
            out.push_str(session_id);
        } else {
            out.push_str(&rest[..end]);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Render the merged provider/endpoint header map for the wire. Values
/// carrying `{{ session_id }}` render against [`probe_session_id`]; any
/// other Tera construct (filters, unknown variables, block syntax) drops
/// the header — the same drop-on-unrenderable rule the runtime applies,
/// and the probe cannot mint values it does not have.
fn render_probe_headers(headers: BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .into_iter()
        .filter_map(|(name, value)| {
            let rendered = render_session_id_template(&value, probe_session_id());
            if rendered.contains("{{") || rendered.contains("{%") {
                None
            } else {
                Some((name, rendered))
            }
        })
        .collect()
}

/// 解析 model_id，处理 [1m], [3m] 等后缀。复用 lib 的 `parse_model_context_suffix`，
/// 仅取剥除后缀的 base id（上下文 token 数由 launch 路径使用）。
fn resolve_api_model_id(model_id: &str) -> Cow<'_, str> {
    let (base, _hint) = crate::parse_model_context_suffix(model_id);
    // base 始终是 model_id 的切片（有/无后缀均借用原字符串）。
    Cow::Borrowed(base)
}

pub fn do_probe(
    provider: &ProviderConfig,
    endpoint_url: &str,
    wire_api: WireApi,
    model_id: &str,
    auth: CopilotAuth,
) -> Result<ProbeCellResult> {
    let api_key = match &provider.apikey_source {
        Some(source) => match resolve_apikey(source) {
            Ok(key) => key,
            Err(e) => {
                return Ok(ProbeCellResult {
                    status: ProbeStatus::ClientError,
                    latency_ms: None,
                    http_status: None,
                    error_message: Some(format!("获取 API Key 失败: {e}")),
                    configured: true,
                });
            }
        },
        None => {
            return Ok(ProbeCellResult {
                status: ProbeStatus::ClientError,
                latency_ms: None,
                http_status: None,
                error_message: Some("未配置 API Key".to_string()),
                configured: true,
            });
        }
    };

    // 解析 model_id，处理 [1m] 等后缀
    let resolved_model_id = resolve_api_model_id(model_id);

    // 根据 wire_api 确定完整 URL（兼容基础 URL 和完整 URL）
    let url = match wire_api {
        WireApi::Anthropic => {
            if endpoint_url.ends_with("/v1/messages") {
                endpoint_url.to_string()
            } else {
                format!("{}/v1/messages", endpoint_url.trim_end_matches('/'))
            }
        }
        WireApi::Completions => {
            if endpoint_url.ends_with("/chat/completions") {
                endpoint_url.to_string()
            } else {
                format!("{}/chat/completions", endpoint_url.trim_end_matches('/'))
            }
        }
        WireApi::Responses => {
            if endpoint_url.ends_with("/responses") {
                endpoint_url.to_string()
            } else {
                format!("{}/responses", endpoint_url.trim_end_matches('/'))
            }
        }
        WireApi::Unavailable => endpoint_url.to_string(),
    };

    match wire_api {
        WireApi::Anthropic | WireApi::Completions | WireApi::Responses => {
            let body = match wire_api {
                WireApi::Anthropic => json!({
                    "model": resolved_model_id,
                    "max_tokens": 5,
                    "messages": [{"role": "user", "content": "hi"}]
                }),
                WireApi::Completions => json!({
                    "model": resolved_model_id,
                    "max_tokens": 5,
                    "messages": [{"role": "user", "content": "hi"}]
                }),
                WireApi::Responses => json!({
                    "model": resolved_model_id,
                    "max_output_tokens": 5,
                    "input": [{"role": "user", "content": "hi"}]
                }),
                _ => unreachable!(),
            };
            // Same merged map the runtime uses for real traffic.
            let config_headers = render_probe_headers(provider.request_headers(wire_api));
            probe_endpoint(&url, &api_key, wire_api, auth, body, config_headers)
        }
        WireApi::Unavailable => Ok(ProbeCellResult {
            status: ProbeStatus::NotApplicable,
            latency_ms: None,
            http_status: None,
            error_message: None,
            configured: true,
        }),
    }
}

fn probe_endpoint(
    url: &str,
    api_key: &str,
    wire_api: WireApi,
    auth: CopilotAuth,
    body: serde_json::Value,
    config_headers: BTreeMap<String, String>,
) -> Result<ProbeCellResult> {
    let start = Instant::now();

    runtime().block_on(async {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        // Config headers first; the probe's own headers are inserted after
        // and therefore win same-name collisions — insert semantics,
        // mirroring the runtime (its auth header must replace a same-name
        // config entry, never be duplicated by it).
        for (name, value) in config_headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(&value),
            ) {
                headers.insert(name, value);
            }
        }

        // 认证头由协议决定：
        // - OpenAI 协议（responses/completions）统一使用 Authorization: Bearer
        // - Anthropic 协议默认使用 x-api-key，并补充 anthropic-version 头；
        //   若端点显式配置 copilot_auth: bearer_token，则改用 Bearer
        match wire_api {
            WireApi::Responses | WireApi::Completions => {
                if let Ok(value) = HeaderValue::from_str(&format!("Bearer {api_key}")) {
                    headers.insert(header::AUTHORIZATION, value);
                }
            }
            WireApi::Anthropic => {
                headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
                let (name, value) = match auth {
                    CopilotAuth::BearerToken => (
                        header::AUTHORIZATION,
                        HeaderValue::from_str(&format!("Bearer {api_key}")),
                    ),
                    CopilotAuth::ApiKey => (
                        HeaderName::from_static("x-api-key"),
                        HeaderValue::from_str(api_key),
                    ),
                };
                if let Ok(value) = value {
                    headers.insert(name, value);
                }
            }
            // Never reached (do_probe routes Unavailable to NotApplicable
            // before building a request); auth headers make no sense here.
            WireApi::Unavailable => {}
        }

        let response = http_client()
            .post(url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .context("调用 API 失败")?;

        let status = response.status();
        let latency_ms = start.elapsed().as_millis() as u64;

        if !status.is_success() {
            let error_body = response.text().await.ok();
            return Ok(ProbeCellResult {
                status: if status.is_server_error() {
                    ProbeStatus::ServerError
                } else {
                    ProbeStatus::ClientError
                },
                latency_ms: None,
                http_status: Some(status.as_u16()),
                error_message: error_body,
                configured: true,
            });
        }

        Ok(ProbeCellResult {
            status: ProbeStatus::Available,
            latency_ms: Some(latency_ms),
            http_status: Some(status.as_u16()),
            error_message: None,
            configured: true,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_api_model_id_with_suffix() {
        assert_eq!(resolve_api_model_id("gpt-4o[1m]"), "gpt-4o");
        assert_eq!(resolve_api_model_id("claude-3-opus[999m]"), "claude-3-opus");
        assert_eq!(resolve_api_model_id("model[123m]"), "model");
    }

    #[test]
    fn test_resolve_api_model_id_without_suffix() {
        assert_eq!(resolve_api_model_id("gpt-4o"), "gpt-4o");
        assert_eq!(resolve_api_model_id("claude-3-opus"), "claude-3-opus");
        assert_eq!(resolve_api_model_id("model"), "model");
    }

    #[test]
    fn test_resolve_api_model_id_edge_cases() {
        // 空字符串
        assert_eq!(resolve_api_model_id(""), "");
        // 只有后缀
        assert_eq!(resolve_api_model_id("[1m]"), "");
        // 统一解析器现在处理更多格式：[1] → 1 token, [1mm] → 无效, [m] → 无效
        assert_eq!(resolve_api_model_id("model[1]"), "model");
        assert_eq!(resolve_api_model_id("model[1mm]"), "model[1mm]");
        assert_eq!(resolve_api_model_id("model[m]"), "model[m]");
    }

    #[test]
    fn test_probe_result_key() {
        use crate::WireApi;
        let key = probe_result_key("openai", "gpt-4o", WireApi::Completions);
        assert_eq!(key, "openai\0completions\0gpt-4o");

        let key = probe_result_key("anthropic", "claude-3", WireApi::Anthropic);
        assert_eq!(key, "anthropic\0anthropic\0claude-3");
    }

    #[test]
    fn session_id_interpolation_renders_whitespace_insensitively() {
        assert_eq!(render_session_id_template("{{ session_id }}", "S"), "S");
        assert_eq!(render_session_id_template("{{session_id}}", "S"), "S");
        assert_eq!(
            render_session_id_template("pre {{  session_id  }} post", "S"),
            "pre S post"
        );
        assert_eq!(
            render_session_id_template("no template", "S"),
            "no template"
        );
        // Unrelated constructs pass through untouched (the caller drops them).
        assert_eq!(
            render_session_id_template("{{ model }}", "S"),
            "{{ model }}"
        );
        assert_eq!(
            render_session_id_template("{{ session_id ", "S"),
            "{{ session_id "
        );
    }

    #[test]
    fn probe_renders_session_headers_and_drops_unrenderable_templates() {
        let id = probe_session_id();
        let headers = BTreeMap::from([
            ("x-static".to_string(), "gateway".to_string()),
            ("x-session".to_string(), "{{ session_id }}".to_string()),
            (
                "x-filter".to_string(),
                "sess-{{ session_id | truncate(length=8) }}".to_string(),
            ),
            ("x-unknown".to_string(), "{{ model }}".to_string()),
            ("x-unclosed".to_string(), "{{ session_id ".to_string()),
            ("x-block".to_string(), "{% if x %}y{% endif %}".to_string()),
        ]);
        let rendered = render_probe_headers(headers);
        assert_eq!(
            rendered.get("x-static").map(String::as_str),
            Some("gateway")
        );
        assert_eq!(rendered.get("x-session").map(String::as_str), Some(id));
        assert!(!rendered.contains_key("x-filter"));
        assert!(!rendered.contains_key("x-unknown"));
        assert!(!rendered.contains_key("x-unclosed"));
        assert!(!rendered.contains_key("x-block"));
    }
}
