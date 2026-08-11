use std::time::Duration;

use base64::Engine;
use regex::Regex;
use serde_json::Value;

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
    let path = path.split_once('/').map(|(_, rest)| format!("/{rest}")).unwrap_or_default();
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
    let snippet: String = body.trim().replace('\n', " ").chars().take(280).collect();
    match status {
        502 => format!(
            "{kind} failed: HTTP 502 Bad Gateway. 上游网关在同步等待出图时断开（图片可能已在服务端生成）。\
             请提高 implement 超时（建议 ≥600s）、增加重试，并优先使用 response_format=url。 body={snippet}"
        ),
        503 | 504 => format!("{kind} failed: HTTP {status}. 上游维护或超时，请稍后重试。 body={snippet}"),
        _ => format!("{kind} failed: HTTP {status} {snippet}"),
    }
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

pub fn effective_timeout(profile: &ModelProfile, override_seconds: Option<u64>, minimum: Option<u64>) -> u64 {
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
    let mut headers: Vec<(String, String)> = vec![(
        "Content-Type".to_string(),
        "application/json".to_string(),
    )];
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
                tokio::time::sleep(retry_delay(attempt, None, options.retry_interval_seconds)).await;
            }
        }
    }

    Err(model_error(
        last_transport_error.unwrap_or_else(|| "模型请求失败：未收到有效响应".to_string()),
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

fn build_stream_client(idle_timeout_seconds: u64, proxy_url: Option<&str>) -> AppResult<reqwest::Client> {
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
                        return Ok(SseOutcome { status, body, events: Vec::new() });
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
                    Ok(events) => return Ok(SseOutcome { status, body: String::new(), events }),
                    Err(error) => {
                        last_error = Some(error.message);
                        if attempt == attempts - 1 {
                            break;
                        }
                        tokio::time::sleep(retry_delay(attempt, None, options.retry_interval_seconds))
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
                tokio::time::sleep(retry_delay(attempt, None, options.retry_interval_seconds)).await;
            }
        }
    }

    Err(model_error(
        last_error.unwrap_or_else(|| "模型流式请求失败：未收到有效响应".to_string()),
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
        assert_eq!(normalize_base_url("https://api.openai.com", "openai_chat"), "https://api.openai.com/v1");
        assert_eq!(normalize_base_url("https://api.openai.com/v1", "openai_chat"), "https://api.openai.com/v1");
        assert_eq!(normalize_base_url("https://x.com/v1beta/", "openai_chat"), "https://x.com/v1beta");
        assert_eq!(normalize_base_url("https://gw.example.com/", "banana2"), "https://gw.example.com");
    }

    #[test]
    fn redacts_secrets_in_errors() {
        let message = safe_error_message("failed with Bearer sk-abc123def456 and api_key=topsecret");
        assert!(message.contains("Bearer <redacted>"));
        assert!(!message.contains("topsecret"));
    }

    #[test]
    fn gateway_statuses_back_off_longer() {
        assert_eq!(retry_backoff(0, Some(502)), Duration::from_secs(10));
        assert_eq!(retry_backoff(0, Some(429)), Duration::from_secs(15));
        assert_eq!(retry_backoff(2, None), Duration::from_secs(4));
        assert_eq!(retry_backoff(9, Some(502)), Duration::from_secs(MAX_BACKOFF_SECONDS));
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
        assert_eq!(parse_json_response(fenced).unwrap()["ok"], serde_json::json!(true));
        let noisy = "Sure, here you go:\n{\"a\": 1}\nHope that helps.";
        assert_eq!(parse_json_response(noisy).unwrap()["a"], serde_json::json!(1));
    }
}
