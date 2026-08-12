use std::time::Duration;

use base64::Engine;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::config::ModelProfile;
use crate::error::{AppError, AppResult};

pub const DEFAULT_CONNECT_TIMEOUT_SECONDS: u64 = 30;
pub const MIN_IMAGE_TIMEOUT_SECONDS: u64 = 120;
pub const MAX_BACKOFF_SECONDS: u64 = 30;
pub const IMAGE_RETRY_INTERVAL_SECONDS: u64 = 180;
const RETRY_STATUS_CODES: [u16; 5] = [429, 500, 502, 503, 504];

pub fn is_retryable(status: u16) -> bool {
    RETRY_STATUS_CODES.contains(&status)
}

pub fn data_url(mime_type: &str, b64: &str) -> String {
    format!("data:{mime_type};base64,{b64}")
}

pub fn encode_b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_b64(value: &str) -> AppResult<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .map_err(|error| AppError::new("b64_error", error.to_string()))
}

pub fn normalize_base_url(base_url: &str, protocol: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if matches!(protocol, "banana2" | "banna2") {
        return base.to_string();
    }
    let version_pattern = Regex::new(r"/v\d+(?:beta)?(?:/|$)").expect("valid regex");
    let path = base.split_once("://").map(|(_, rest)| rest).unwrap_or(base);
    let path = path
        .split_once('/')
        .map(|(_, rest)| format!("/{rest}"))
        .unwrap_or_default();
    if version_pattern.is_match(&path) {
        return base.to_string();
    }
    format!("{base}/v1")
}

pub fn require_api_key(profile: &ModelProfile) -> AppResult<String> {
    match profile.api_key.as_deref().map(str::trim) {
        Some(key) if !key.is_empty() => Ok(key.to_string()),
        _ => Err(AppError::new(
            "missing_api_key",
            format!("{} 缺少 API key", profile.name),
        )),
    }
}

pub fn safe_error_message(message: &str) -> String {
    let bearer = Regex::new(r"Bearer\s+[A-Za-z0-9._~+/=-]+").expect("valid regex");
    let keyed = Regex::new(r#"(?i)(api[_-]?key|x-api-key|x-goog-api-key)(['"\s:=]+)([^'"\s,&}]+)"#)
        .expect("valid regex");
    let cleaned = bearer.replace_all(message, "Bearer <redacted>");
    let cleaned = keyed.replace_all(&cleaned, "$1$2<redacted>");
    cleaned.chars().take(800).collect()
}

pub fn model_error(message: impl AsRef<str>) -> AppError {
    AppError::new("model_error", safe_error_message(message.as_ref()))
}

pub fn model_profile_error(
    profile: &ModelProfile,
    endpoint: &str,
    code: &str,
    message: impl AsRef<str>,
    http_status: Option<u16>,
    suggestion: impl AsRef<str>,
) -> AppError {
    let summary = safe_error_message(message.as_ref());
    AppError::with_detail(
        code,
        &summary,
        json!({
            "summary": summary.clone(),
            "code": code,
            "role": &profile.role,
            "profile_id": &profile.id,
            "profile_name": &profile.name,
            "protocol": &profile.protocol,
            "model": &profile.model,
            "base_url": &profile.base_url,
            "endpoint": endpoint,
            "http_status": http_status,
            "suggestion": suggestion.as_ref(),
        }),
    )
}

pub fn retry_backoff(attempt: u32, status: Option<u16>) -> Duration {
    let seconds = match status {
        Some(502 | 503 | 504) => (10 * (attempt as u64 + 1)).min(MAX_BACKOFF_SECONDS),
        Some(429) => (15 * (attempt as u64 + 1)).min(MAX_BACKOFF_SECONDS),
        _ => 2u64.saturating_pow(attempt).min(MAX_BACKOFF_SECONDS),
    };
    Duration::from_secs(seconds)
}

pub fn retry_delay(attempt: u32, status: Option<u16>, fixed_interval: Option<u64>) -> Duration {
    match fixed_interval {
        Some(seconds) => Duration::from_secs(seconds.max(1)),
        None => retry_backoff(attempt, status),
    }
}

pub fn describe_transport_error(error: &reqwest::Error) -> String {
    let mut chain = vec![error.to_string()];
    let mut cursor: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(cause) = cursor {
        let text = cause.to_string();
        if !text.trim().is_empty() && !chain.iter().any(|item| item == &text) {
            chain.push(text);
        }
        cursor = cause.source();
    }
    let mut kinds = Vec::new();
    for (flag, label) in [
        (error.is_timeout(), "timeout"),
        (error.is_connect(), "connect"),
        (error.is_request(), "request"),
        (error.is_body(), "body"),
        (error.is_decode(), "decode"),
    ] {
        if flag {
            kinds.push(label);
        }
    }
    let detail = chain.join(" <- ");
    if kinds.is_empty() {
        detail
    } else {
        format!("{detail} [{}]", kinds.join(","))
    }
}

pub fn format_http_error(kind: &str, status: u16, body: &str) -> String {
    let detail = response_error_summary(body);
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(" 服务返回：{detail}")
    };
    match status {
        502 => format!("{kind} 请求失败：HTTP 502，上游网关无法完成请求。{suffix}"),
        503 | 504 => format!(
            "{kind} 请求失败：HTTP {status}，上游网关维护、超时或无法连接模型服务。{suffix}"
        ),
        _ => format!("{kind} 请求失败：HTTP {status}。{suffix}"),
    }
}

pub fn response_error_summary(body: &str) -> String {
    let raw = body.trim();
    if raw.is_empty() {
        return String::new();
    }
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        let candidate = value["error"]["message"]
            .as_str()
            .or_else(|| value["error"]["detail"].as_str())
            .or_else(|| value["message"].as_str())
            .or_else(|| value["detail"].as_str());
        if let Some(text) = candidate {
            return text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(240)
                .collect();
        }
    }
    let title = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("valid title regex");
    let tags = Regex::new(r"(?is)<[^>]+>").expect("valid tag regex");
    let source = title
        .captures(raw)
        .and_then(|capture| capture.get(1).map(|item| item.as_str()))
        .unwrap_or(raw);
    tags.replace_all(source, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}

pub fn http_error_suggestion(role: &str, status: u16) -> String {
    match status {
        502 | 503 | 504 => format!(
            "{} 上游网关异常，请稍后重试；若持续出现，请检查该服务地址或更换中转服务。",
            capitalize_role(role)
        ),
        401 => format!("请检查 {} 配置的 API key。", capitalize_role(role)),
        403 => format!(
            "请检查 {} 配置的 API key、模型权限和服务商访问策略。",
            capitalize_role(role)
        ),
        404 => format!(
            "请检查 {} 的 Base URL、协议和模型名。",
            capitalize_role(role)
        ),
        429 => "请求频率或额度受限。请稍后重试，并检查账户额度。".to_string(),
        _ => format!(
            "请检查 {} 的 Base URL、协议、模型名和服务商状态。",
            capitalize_role(role)
        ),
    }
}

fn capitalize_role(role: &str) -> String {
    let mut chars = role.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub fn model_http_error(
    profile: &ModelProfile,
    endpoint: &str,
    kind: &str,
    status: u16,
    body: &str,
) -> AppError {
    model_profile_error(
        profile,
        endpoint,
        "model_http_error",
        format_http_error(kind, status, body),
        Some(status),
        http_error_suggestion(&profile.role, status),
    )
}

pub fn build_client(
    read_timeout_seconds: u64,
    proxy_url: Option<&str>,
) -> AppResult<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECONDS))
        .timeout(Duration::from_secs(read_timeout_seconds.max(1)))
        .tcp_keepalive(Duration::from_secs(30))
        .pool_max_idle_per_host(0);
    if let Some(proxy) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
        let proxy = reqwest::Proxy::all(proxy)
            .map_err(|error| model_error(format!("代理配置无效: {error}")))?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|error| model_error(format!("HTTP client 构建失败: {error}")))
}

pub fn effective_timeout(
    profile: &ModelProfile,
    override_seconds: Option<u64>,
    minimum: Option<u64>,
) -> u64 {
    let base = override_seconds.unwrap_or(profile.timeout_seconds.max(1) as u64);
    match minimum {
        Some(floor) => base.max(floor),
        None => base,
    }
}

pub fn merged_headers(
    profile: &ModelProfile,
    extra: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> =
        vec![("Content-Type".to_string(), "application/json".to_string())];
    for (key, value) in &profile.headers {
        if let Some(text) = value.as_str() {
            headers.push((key.clone(), text.to_string()));
        }
    }
    headers.extend(extra);
    headers
}

pub struct HttpOutcome {
    pub status: u16,
    pub body: String,
}

#[derive(Clone, Copy, Default)]
pub struct PostOptions<'a> {
    pub timeout_seconds: Option<u64>,
    pub minimum_timeout: Option<u64>,
    pub retry_interval_seconds: Option<u64>,
    pub proxy_url: Option<&'a str>,
}

pub async fn post_json_with_retries(
    profile: &ModelProfile,
    url: &str,
    payload: &Value,
    headers: Vec<(String, String)>,
    options: PostOptions<'_>,
) -> AppResult<HttpOutcome> {
    let read_timeout = effective_timeout(profile, options.timeout_seconds, options.minimum_timeout);
    let client = build_client(read_timeout, options.proxy_url)?;
    let all_headers = merged_headers(profile, headers);
    let attempts = (profile.max_retries.max(0) as u32) + 1;
    let mut last_transport_error: Option<String> = None;

    for attempt in 0..attempts {
        let started = std::time::Instant::now();
        let mut request = client.post(url).json(payload);
        for (key, value) in &all_headers {
            request = request.header(key.as_str(), value.as_str());
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                if !is_retryable(status) || attempt == attempts - 1 {
                    return Ok(HttpOutcome { status, body });
                }
                tokio::time::sleep(retry_delay(
                    attempt,
                    Some(status),
                    options.retry_interval_seconds,
                ))
                .await;
            }
            Err(error) => {
                let elapsed = started.elapsed().as_secs();
                last_transport_error = Some(if error.is_timeout() {
                    format!(
                        "模型请求超时：读超时 {read_timeout} 秒内未收到完整响应（本次等待 {elapsed} 秒）。\
                         同步制图接口可能需要更长时间，请在 Model 配置中提高 implement 超时。"
                    )
                } else {
                    format!(
                        "模型请求网络错误（发起后 {elapsed} 秒断开）：{}",
                        describe_transport_error(&error)
                    )
                });
                if attempt == attempts - 1 {
                    break;
                }
                tokio::time::sleep(retry_delay(attempt, None, options.retry_interval_seconds))
                    .await;
            }
        }
    }

    let message =
        last_transport_error.unwrap_or_else(|| "模型请求失败：未收到有效响应".to_string());
    let suggestion = if options
        .proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
    {
        "请检查代理地址和代理服务，或清空代理后直连。".to_string()
    } else {
        format!(
            "请检查 {} 的 Base URL、DNS 和网络连接。",
            capitalize_role(&profile.role)
        )
    };
    Err(model_profile_error(
        profile,
        url,
        "model_network_error",
        message,
        None,
        suggestion,
    ))
}

pub struct SseOutcome {
    pub status: u16,
    pub body: String,
    pub events: Vec<Value>,
}

pub fn drain_sse_events(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut events = Vec::new();
    while let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = buffer.drain(..=index).collect();
        let decoded = String::from_utf8_lossy(&line);
        let Some(rest) = decoded.trim().strip_prefix("data:") else {
            continue;
        };
        let payload = rest.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        events.push(payload.to_string());
    }
    events
}

fn build_stream_client(
    idle_timeout_seconds: u64,
    proxy_url: Option<&str>,
) -> AppResult<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECONDS))
        .read_timeout(Duration::from_secs(idle_timeout_seconds.max(1)))
        .tcp_keepalive(Duration::from_secs(30))
        .pool_max_idle_per_host(0);
    if let Some(proxy) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
        let proxy = reqwest::Proxy::all(proxy)
            .map_err(|error| model_error(format!("代理配置无效: {error}")))?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|error| model_error(format!("HTTP client 构建失败: {error}")))
}

pub async fn post_sse_with_retries(
    profile: &ModelProfile,
    url: &str,
    payload: &Value,
    headers: Vec<(String, String)>,
    options: PostOptions<'_>,
) -> AppResult<SseOutcome> {
    let idle_timeout = effective_timeout(profile, options.timeout_seconds, options.minimum_timeout);
    let client = build_stream_client(idle_timeout, options.proxy_url)?;
    let mut all_headers = merged_headers(profile, headers);
    all_headers.push(("Accept".to_string(), "text/event-stream".to_string()));
    let attempts = (profile.max_retries.max(0) as u32) + 1;
    let mut last_error: Option<String> = None;

    for attempt in 0..attempts {
        let started = std::time::Instant::now();
        let mut request = client.post(url).json(payload);
        for (key, value) in &all_headers {
            request = request.header(key.as_str(), value.as_str());
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                if status >= 400 {
                    let body = response.text().await.unwrap_or_default();
                    if !is_retryable(status) || attempt == attempts - 1 {
                        return Ok(SseOutcome {
                            status,
                            body,
                            events: Vec::new(),
                        });
                    }
                    tokio::time::sleep(retry_delay(
                        attempt,
                        Some(status),
                        options.retry_interval_seconds,
                    ))
                    .await;
                    continue;
                }
                match read_sse_events(response).await {
                    Ok(events) => {
                        return Ok(SseOutcome {
                            status,
                            body: String::new(),
                            events,
                        })
                    }
                    Err(error) => {
                        last_error = Some(error.message);
                        if attempt == attempts - 1 {
                            break;
                        }
                        tokio::time::sleep(retry_delay(
                            attempt,
                            None,
                            options.retry_interval_seconds,
                        ))
                        .await;
                    }
                }
            }
            Err(error) => {
                let elapsed = started.elapsed().as_secs();
                last_error = Some(if error.is_timeout() {
                    format!(
                        "模型流式请求超时：{idle_timeout} 秒内没有收到新的输出（本次等待 {elapsed} 秒）。\
                         请在 Model 配置中提高 design 超时，或确认上游是否支持 stream。"
                    )
                } else {
                    format!(
                        "模型请求网络错误（发起后 {elapsed} 秒断开）：{}",
                        describe_transport_error(&error)
                    )
                });
                if attempt == attempts - 1 {
                    break;
                }
                tokio::time::sleep(retry_delay(attempt, None, options.retry_interval_seconds))
                    .await;
            }
        }
    }

    let message = last_error.unwrap_or_else(|| "模型流式请求失败：未收到有效响应".to_string());
    let suggestion = if options
        .proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
    {
        "请检查代理地址和代理服务，或清空代理后直连。".to_string()
    } else {
        format!(
            "请检查 {} 的 Base URL、DNS 和网络连接。",
            capitalize_role(&profile.role)
        )
    };
    Err(model_profile_error(
        profile,
        url,
        "model_network_error",
        message,
        None,
        suggestion,
    ))
}

async fn read_sse_events(response: reqwest::Response) -> AppResult<Vec<Value>> {
    use futures::StreamExt;

    let mut stream = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    let mut events: Vec<Value> = Vec::new();
    let push = |payload: &str, events: &mut Vec<Value>| {
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            events.push(value);
        }
    };

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            model_error(format!(
                "流式响应中断：{}",
                describe_transport_error(&error)
            ))
        })?;
        buffer.extend_from_slice(&chunk);
        for payload in drain_sse_events(&mut buffer) {
            push(&payload, &mut events);
        }
    }
    if !buffer.is_empty() {
        buffer.push(b'\n');
        for payload in drain_sse_events(&mut buffer) {
            push(&payload, &mut events);
        }
    }
    Ok(events)
}

pub fn parse_json_response(text: &str) -> AppResult<Value> {
    let fence_start = Regex::new(r"^```(?:json)?").expect("valid regex");
    let fence_end = Regex::new(r"```$").expect("valid regex");
    let cleaned = text.trim();
    let cleaned = fence_start.replace(cleaned, "");
    let cleaned = cleaned.trim();
    let cleaned = fence_end.replace(cleaned, "");
    let cleaned = cleaned.trim();

    if let Ok(value) = serde_json::from_str::<Value>(cleaned) {
        return Ok(value);
    }
    let start = cleaned.find('{');
    let end = cleaned.rfind('}');
    if let (Some(start), Some(end)) = (start, end) {
        if end > start {
            return serde_json::from_str::<Value>(&cleaned[start..=end])
                .map_err(|error| model_error(format!("模型返回的不是合法 JSON: {error}")));
        }
    }
    Err(model_error("模型返回的不是合法 JSON"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_base_url() {
        assert_eq!(
            normalize_base_url("https://api.openai.com", "openai_chat"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1", "openai_chat"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base_url("https://x.com/v1beta/", "openai_chat"),
            "https://x.com/v1beta"
        );
        assert_eq!(
            normalize_base_url("https://gw.example.com/", "banana2"),
            "https://gw.example.com"
        );
    }

    #[test]
    fn extracts_the_first_complete_json_value_from_noisy_text() {
        let noisy = "analysis mentions {an invalid example} first\n{\"ok\":true,\"text\":\"brace } inside string\"}\ntrailing {notes}";
        let parsed = parse_json_response(noisy).expect("完整 JSON 对象应被提取");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["text"], "brace } inside string");
    }

    #[test]
    fn parses_json_arrays_wrapped_in_prose() {
        let parsed =
            parse_json_response("Result follows: [1, {\"x\": 2}] done").expect("数组也应支持");
        assert_eq!(parsed[1]["x"], 2);
    }

    #[test]
    fn redacts_secrets_in_errors() {
        let message =
            safe_error_message("failed with Bearer sk-abc123def456 and api_key=topsecret");
        assert!(message.contains("Bearer <redacted>"));
        assert!(!message.contains("topsecret"));
    }

    #[test]
    fn extracts_gateway_html_title_without_leaking_the_page() {
        let body = r#"<html><head><title>无法连接到服务器</title></head><body><style>large page</style></body></html>"#;
        assert_eq!(response_error_summary(body), "无法连接到服务器");
        let message = format_http_error("Design", 504, body);
        assert!(message.contains("HTTP 504"));
        assert!(message.contains("无法连接到服务器"));
        assert!(!message.contains("<html"));
    }

    #[test]
    fn model_http_errors_include_the_profile_and_endpoint() {
        let profile = ModelProfile {
            id: "design-default".to_string(),
            role: "design".to_string(),
            name: "Design model".to_string(),
            protocol: "openai_responses".to_string(),
            base_url: "https://gateway.example".to_string(),
            model: "model-a".to_string(),
            api_key: Some("secret".to_string()),
            api_version: None,
            headers: serde_json::Map::new(),
            timeout_seconds: 120,
            max_retries: 2,
            output_defaults: serde_json::Map::new(),
            has_api_key: None,
            api_key_hint: None,
        };
        let error = model_http_error(
            &profile,
            "https://gateway.example/v1/responses",
            "Design",
            504,
            "<title>upstream unavailable</title>",
        );
        let detail = error.detail.expect("structured detail");
        assert_eq!(detail["role"], "design");
        assert_eq!(detail["profile_id"], "design-default");
        assert_eq!(detail["model"], "model-a");
        assert_eq!(detail["http_status"], 504);
        assert_eq!(detail["endpoint"], "https://gateway.example/v1/responses");
        assert!(!detail.to_string().contains("secret"));
    }

    #[test]
    fn gateway_statuses_back_off_longer() {
        assert_eq!(retry_backoff(0, Some(502)), Duration::from_secs(10));
        assert_eq!(retry_backoff(0, Some(429)), Duration::from_secs(15));
        assert_eq!(retry_backoff(2, None), Duration::from_secs(4));
        assert_eq!(
            retry_backoff(9, Some(502)),
            Duration::from_secs(MAX_BACKOFF_SECONDS)
        );
    }

    #[test]
    fn sse_framing_survives_split_chunks() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(b"data: {\"a\":1}\n\ndata: {\"b\"");
        let first = drain_sse_events(&mut buffer);
        assert_eq!(first, vec!["{\"a\":1}".to_string()], "完整的一行要立刻交出去");

        buffer.extend_from_slice(b":2}\n\n");
        let second = drain_sse_events(&mut buffer);
        assert_eq!(second, vec!["{\"b\":2}".to_string()], "跨 chunk 的后半截要接上");
    }

    #[test]
    fn sse_framing_survives_split_multibyte_characters() {
        let mut buffer = Vec::new();
        let line = "data: {\"t\":\"图\"}\n".as_bytes().to_vec();
        let (head, tail) = line.split_at(line.len() - 3);
        buffer.extend_from_slice(head);
        assert!(drain_sse_events(&mut buffer).is_empty(), "没收到换行就不该交出去");

        buffer.extend_from_slice(tail);
        assert_eq!(drain_sse_events(&mut buffer), vec!["{\"t\":\"图\"}".to_string()]);
    }

    #[test]
    fn sse_framing_skips_terminators_and_comments() {
        let mut buffer =
            b": ping\r\nevent: message\r\ndata: {\"ok\":true}\r\n\r\ndata: [DONE]\r\n\r\n".to_vec();
        assert_eq!(drain_sse_events(&mut buffer), vec!["{\"ok\":true}".to_string()]);
    }

    #[test]
    fn image_retry_interval_is_fixed_at_three_minutes() {
        let fixed = Some(IMAGE_RETRY_INTERVAL_SECONDS);
        assert_eq!(retry_delay(0, None, fixed), Duration::from_secs(180));
        assert_eq!(retry_delay(3, Some(502), fixed), Duration::from_secs(180));
        assert_eq!(retry_delay(2, None, None), retry_backoff(2, None));
        assert_eq!(retry_delay(0, Some(429), None), retry_backoff(0, Some(429)));
    }

    #[test]
    fn parses_fenced_and_trailing_json() {
        let fenced = "```json\n{\"ok\": true}\n```";
        assert_eq!(
            parse_json_response(fenced).unwrap()["ok"],
            serde_json::json!(true)
        );
        let noisy = "Sure, here you go:\n{\"a\": 1}\nHope that helps.";
        assert_eq!(
            parse_json_response(noisy).unwrap()["a"],
            serde_json::json!(1)
        );
    }
}
