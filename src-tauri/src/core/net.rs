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
/// 制图路径的固定重试间隔：3 分钟。
///
/// 出图是同步长请求。连接被中间层掐断时上游通常还在画，
/// 秒级退避会在上游画完之前又提交一遍——上游面板上就是
/// 「同一个请求隔两分钟来了三次」，每次都要计费。
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

/// 给定固定间隔就按固定间隔等（制图用 3 分钟），否则回落到按状态码退避。
pub fn retry_delay(attempt: u32, status: Option<u16>, fixed_interval: Option<u64>) -> Duration {
    match fixed_interval {
        Some(seconds) => Duration::from_secs(seconds.max(1)),
        None => retry_backoff(attempt, status),
    }
}

/// 展开 reqwest 错误的 source 链。
///
/// reqwest 的 Display 只有最外一层——`error sending request for url (…)`，
/// 真正的原因（连接被重置、代理拒绝、TLS 失败、响应不完整）全在 source 里。
/// 不展开就分不清是本机代理没开、网络不通，还是上游把长连接掐了。
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
        // 出图请求可能几百秒一个字节都不回，链路上的空闲回收会静默掐断连接；
        // keepalive 让 NAT / 网关看到这条连接还活着
        .tcp_keepalive(Duration::from_secs(30))
        // 不复用空闲连接：长任务之间池里那条连接常已被上游单方面关掉，
        // 再拿来用就表现为一个没有原因的 "error sending request"
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

/// 一次 POST 的传输选项。
///
/// 原先这些是并列参数，加到第八个（重试间隔）就该收拢了：
/// 调用点全是 `None, proxy_url, None` 这种一眼分不清谁是谁的实参。
#[derive(Clone, Copy, Default)]
pub struct PostOptions<'a> {
    /// 覆盖 `profile.timeout_seconds`
    pub timeout_seconds: Option<u64>,
    /// 读超时下限（制图用 `MIN_IMAGE_TIMEOUT_SECONDS`）
    pub minimum_timeout: Option<u64>,
    /// 固定重试间隔；None 时按状态码退避
    pub retry_interval_seconds: Option<u64>,
    pub proxy_url: Option<&'a str>,
}

/// 带退避重试的 POST。语义对齐 Python 的 `post_json_with_retries`：
/// 可重试状态码或传输错误才重试，最后一次尝试的响应/错误直接返回给调用方。
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

/// 一次流式 POST 的结果。非 2xx 时 `events` 为空、`body` 是原始错误体。
pub struct SseOutcome {
    pub status: u16,
    pub body: String,
    /// 各 SSE 事件的 `data:` 负载，按到达顺序。`[DONE]` 与心跳注释已剔除。
    pub events: Vec<Value>,
}

/// 从 SSE 缓冲里切出完整的 `data:` 负载，尾部不完整的一行留在 buffer 里等下一块。
///
/// 拆成独立函数是为了能脱网测：chunk 边界落在一行中间、甚至落在一个汉字的
/// 三个字节中间，是 SSE 最容易错的地方，而这类错误在真机上只表现为
/// 「偶尔少一段文字」，几乎复现不出来。按字节缓冲、按整行解码可以绕开它：
/// `\n` 一定是 UTF-8 的字符边界，所以整行永远不会把一个字符劈成两半。
pub fn drain_sse_events(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut events = Vec::new();
    while let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = buffer.drain(..=index).collect();
        let decoded = String::from_utf8_lossy(&line);
        // 事件之间的空行、`:` 开头的心跳注释、`event:` 行都直接跳过
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

/// 流式请求用的 client。与非流式那条的差别只在超时语义。
///
/// 非流式给的是「整个请求的总时长上限」；流式给的是「两次读之间的空档上限」
/// （`read_timeout`），并且不设总上限。否则流式刚把网关的 524 绕开，
/// 又会撞上我们自己配的 120 秒总超时——那对用户是同一件事：任务白跑。
/// 流式下真正该管的是「模型是不是还在吐字」，不是「一共花了多久」。
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

/// 带重试的 SSE POST。调用方负责往 payload 里放 `stream: true`。
///
/// 为什么要有这条路径：Cloudflare 一类的中间层只给源站约 100 秒返回响应头，
/// 超了就自己合成一个 `524` 回给我们——于是我们看到的是网关的 524，
/// 而不是模型的任何回复，且此时上游往往还在推理。流式让上游在第一个 token
/// 就把响应头发出来，那 100 秒的表从此不对我们计时。
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
                    // 错误响应不是 SSE，照常整体读
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
                        // 流中途断了。半截文本没有用处（后面还要按 JSON 解析），当传输错误重试
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
        // 网关偶尔混进非 JSON 的 keep-alive 负载，跳过即可，不该让整个任务失败
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
    // 最后一个事件可能没有换行收尾
    if !buffer.is_empty() {
        buffer.push(b'\n');
        for payload in drain_sse_events(&mut buffer) {
            push(&payload, &mut events);
        }
    }
    Ok(events)
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

    /// chunk 边界落在一行中间是 SSE 的常态（TCP 不按行切）。
    /// 切错了只表现为「回复偶尔少一段」，所以这里必须钉住。
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

    /// 汉字是三字节，chunk 边界可能落在字符中间。按整行解码就不会劈开字符：
    /// `\n` 一定是字符边界。
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

    /// `[DONE]`、心跳注释、`event:` 行都不是负载，混进去后面按 JSON 解析会炸。
    #[test]
    fn sse_framing_skips_terminators_and_comments() {
        let mut buffer =
            b": ping\r\nevent: message\r\ndata: {\"ok\":true}\r\n\r\ndata: [DONE]\r\n\r\n".to_vec();
        assert_eq!(drain_sse_events(&mut buffer), vec!["{\"ok\":true}".to_string()]);
    }

    /// 制图路径固定 3 分钟：不管第几次重试、不管什么状态码，
    /// 都不能退回到秒级退避——上游还在画的时候重投就是重复计费。
    #[test]
    fn image_retry_interval_is_fixed_at_three_minutes() {
        let fixed = Some(IMAGE_RETRY_INTERVAL_SECONDS);
        assert_eq!(retry_delay(0, None, fixed), Duration::from_secs(180));
        assert_eq!(retry_delay(3, Some(502), fixed), Duration::from_secs(180));
        // 不给固定间隔时行为与 retry_backoff 完全一致
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
