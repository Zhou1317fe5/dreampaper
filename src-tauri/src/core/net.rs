//! HTTP 传输层，移植自 `backend/app/adapters.py`。
//!
//! 与 Python 版保持一致的三处关键行为：
//! 1. connect / read 超时分离 —— 同步出图接口的读等待远长于连接建立
//! 2. 502/503/504 与 429 使用更长的退避 —— 网关返回它们时上游往往仍在出图
//! 3. 错误信息中的密钥一律脱敏后才回传前端

use std::time::Duration;

use base64::Engine;
use regex::Regex;
use serde_json::Value;

use crate::core::config::ModelProfile;
use crate::error::{AppError, AppResult};

pub const DEFAULT_CONNECT_TIMEOUT_SECONDS: u64 = 30;
pub const MIN_IMAGE_TIMEOUT_SECONDS: u64 = 120;
pub const MAX_BACKOFF_SECONDS: u64 = 30;
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

/// 除 banana2 外的协议统一补 `/v1`；已带版本段的 base_url 原样保留。
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

/// 回传前端前抹掉 Bearer token 与各类 api_key，并截断到 800 字符。
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
        .timeout(Duration::from_secs(read_timeout_seconds.max(1)));
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

/// 合并 profile.headers 与调用方 headers，profile 优先级更低。
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

/// 带退避重试的 POST。语义对齐 Python 的 `post_json_with_retries`：
/// 可重试状态码或传输错误才重试，最后一次尝试的响应/错误直接返回给调用方。
pub async fn post_json_with_retries(
    profile: &ModelProfile,
    url: &str,
    payload: &Value,
    headers: Vec<(String, String)>,
    timeout_seconds: Option<u64>,
    proxy_url: Option<&str>,
    minimum_timeout: Option<u64>,
) -> AppResult<HttpOutcome> {
    let read_timeout = effective_timeout(profile, timeout_seconds, minimum_timeout);
    let client = build_client(read_timeout, proxy_url)?;
    let all_headers = merged_headers(profile, headers);
    let attempts = (profile.max_retries.max(0) as u32) + 1;
    let mut last_transport_error: Option<String> = None;

    for attempt in 0..attempts {
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
                tokio::time::sleep(retry_backoff(attempt, Some(status))).await;
            }
            Err(error) => {
                let timed_out = error.is_timeout();
                last_transport_error = Some(if timed_out {
                    format!(
                        "模型请求超时：读超时 {read_timeout} 秒内未收到完整响应。\
                         同步制图接口可能需要更长时间，请在 Model 配置中提高 implement 超时。"
                    )
                } else {
                    format!("模型请求网络错误：{error}")
                });
                if attempt == attempts - 1 {
                    break;
                }
                tokio::time::sleep(retry_backoff(attempt, None)).await;
            }
        }
    }

    Err(model_error(
        last_transport_error.unwrap_or_else(|| "模型请求失败：未收到有效响应".to_string()),
    ))
}

/// 从模型回复中抽出 JSON：先去 Markdown 围栏，再退回首尾大括号截取。
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
        // banana2 不补版本段
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
    fn parses_fenced_and_trailing_json() {
        let fenced = "```json\n{\"ok\": true}\n```";
        assert_eq!(parse_json_response(fenced).unwrap()["ok"], serde_json::json!(true));
        let noisy = "Sure, here you go:\n{\"a\": 1}\nHope that helps.";
        assert_eq!(parse_json_response(noisy).unwrap()["a"], serde_json::json!(1));
    }
}
