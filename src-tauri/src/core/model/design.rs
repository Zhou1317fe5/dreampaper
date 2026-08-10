//! Design model 客户端，移植自 `backend/app/adapters.py::DesignClient`。
//!
//! 三种协议的差别只在请求体结构与取文本的路径，超时/重试/代理统一由 `net` 承担。

use serde_json::{json, Value};

use crate::core::config::ModelProfile;
use crate::core::net::{
    data_url, model_error, normalize_base_url, post_json_with_retries, post_sse_with_retries,
    require_api_key, PostOptions,
};
use crate::error::AppResult;

/// 流式开关，存在 `output_defaults.stream` 里。
///
/// 那本来就是一张自由的请求参数表（`response_format`、`size` 都在里面），
/// 加一个键不用动配置 schema，老配置读出来就是「关」。
/// 前端写进去的是字符串 `"true"`，所以布尔和字符串两种字面都认。
pub fn stream_enabled(profile: &ModelProfile) -> bool {
    match profile.output_defaults.get("stream") {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes"
        ),
        _ => false,
    }
}

/// 把 SSE 事件流拼回完整文本。
///
/// 三种协议的增量字段各不相同，但拼出来的结果必须和非流式那条路取到的文本
/// 一字不差——下游 `parse_json_response` 对两条路是同一份代码。
fn collect_stream_text(protocol: &str, events: &[Value]) -> AppResult<String> {
    // 流里混着错误事件时，那才是真正的失败原因，比「没有文本」有用得多
    if let Some(message) = events.iter().find_map(stream_error_message) {
        return Err(model_error(format!("模型流式响应报错：{message}")));
    }
    let mut text = String::new();
    for event in events {
        match protocol {
            "openai_chat" => {
                if let Some(delta) = event["choices"][0]["delta"]["content"].as_str() {
                    text.push_str(delta);
                }
            }
            "openai_responses" => match event["type"].as_str() {
                Some("response.output_text.delta") => {
                    if let Some(delta) = event["delta"].as_str() {
                        text.push_str(delta);
                    }
                }
                // 有的实现只在收尾事件里给全文，不发增量
                Some("response.completed") if text.is_empty() => {
                    if let Some(full) = event["response"]["output_text"].as_str() {
                        text.push_str(full);
                    }
                }
                _ => {}
            },
            "anthropic_messages" => {
                if event["type"].as_str() == Some("content_block_delta")
                    && event["delta"]["type"].as_str() == Some("text_delta")
                {
                    if let Some(delta) = event["delta"]["text"].as_str() {
                        text.push_str(delta);
                    }
                }
            }
            other => return Err(model_error(format!("Unsupported design protocol: {other}"))),
        }
    }
    if text.trim().is_empty() {
        return Err(model_error(
            "模型流式响应没有返回文本。若上游不支持 stream，请在 Model 配置里关掉流式。",
        ));
    }
    Ok(text)
}

/// 流中途的错误事件。三家形状不同：OpenAI 兼容网关直接挂一个 `error` 对象，
/// Responses 用 `type: error` / `response.failed`。
fn stream_error_message(event: &Value) -> Option<String> {
    let candidate = match event.get("error") {
        Some(value) if !value.is_null() => value,
        _ => match event["type"].as_str() {
            Some("error") => event,
            Some("response.failed") => &event["response"]["error"],
            _ => return None,
        },
    };
    if candidate.is_null() {
        return None;
    }
    candidate["message"]
        .as_str()
        .map(str::to_string)
        .or_else(|| Some(candidate.to_string()))
}

/// 随 prompt 一同发给模型的参考图（template / 母版）。
#[derive(Clone, Debug)]
pub struct ImageInput {
    pub filename: String,
    pub mime_type: String,
    pub b64: String,
}

pub struct DesignClient;

impl DesignClient {
    pub async fn generate(
        profile: &ModelProfile,
        system_prompt: &str,
        user_prompt: &str,
        images: &[ImageInput],
        timeout_seconds: Option<u64>,
        proxy_url: Option<&str>,
    ) -> AppResult<String> {
        let protocol = if profile.protocol == "banna2" {
            "banana2"
        } else {
            profile.protocol.as_str()
        };
        match protocol {
            "openai_chat" => {
                Self::openai_chat(profile, system_prompt, user_prompt, images, timeout_seconds, proxy_url).await
            }
            "openai_responses" => {
                Self::openai_responses(profile, system_prompt, user_prompt, images, timeout_seconds, proxy_url).await
            }
            "anthropic_messages" => {
                Self::anthropic_messages(profile, system_prompt, user_prompt, images, timeout_seconds, proxy_url).await
            }
            other => Err(model_error(format!("Unsupported design protocol: {other}"))),
        }
    }

    async fn openai_chat(
        profile: &ModelProfile,
        system_prompt: &str,
        user_prompt: &str,
        images: &[ImageInput],
        timeout_seconds: Option<u64>,
        proxy_url: Option<&str>,
    ) -> AppResult<String> {
        let url = format!(
            "{}/chat/completions",
            normalize_base_url(&profile.base_url, &profile.protocol)
        );
        let mut content = vec![json!({"type": "text", "text": user_prompt})];
        content.extend(images.iter().map(|image| {
            json!({
                "type": "image_url",
                "image_url": {"url": data_url(&image.mime_type, &image.b64)}
            })
        }));
        let mut payload = json!({
            "model": profile.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": content}
            ],
            "temperature": 0.2
        });
        if stream_enabled(profile) {
            payload["stream"] = Value::Bool(true);
            let events = Self::stream(profile, &url, &payload, Self::bearer(profile)?, timeout_seconds, proxy_url).await?;
            return collect_stream_text("openai_chat", &events);
        }
        let data = Self::post(profile, &url, &payload, timeout_seconds, proxy_url).await?;
        data["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| model_error("OpenAI Chat 响应缺少 message.content"))
    }

    async fn openai_responses(
        profile: &ModelProfile,
        system_prompt: &str,
        user_prompt: &str,
        images: &[ImageInput],
        timeout_seconds: Option<u64>,
        proxy_url: Option<&str>,
    ) -> AppResult<String> {
        let url = format!(
            "{}/responses",
            normalize_base_url(&profile.base_url, &profile.protocol)
        );
        let mut content = vec![json!({"type": "input_text", "text": user_prompt})];
        content.extend(images.iter().map(|image| {
            json!({
                "type": "input_image",
                "image_url": data_url(&image.mime_type, &image.b64)
            })
        }));
        let mut payload = json!({
            "model": profile.model,
            "instructions": system_prompt,
            "input": [{"role": "user", "content": content}],
            "text": {"format": {"type": "text"}}
        });
        if stream_enabled(profile) {
            payload["stream"] = Value::Bool(true);
            let events = Self::stream(profile, &url, &payload, Self::bearer(profile)?, timeout_seconds, proxy_url).await?;
            return collect_stream_text("openai_responses", &events);
        }
        let data = Self::post(profile, &url, &payload, timeout_seconds, proxy_url).await?;
        if let Some(text) = data["output_text"].as_str() {
            if !text.is_empty() {
                return Ok(text.to_string());
            }
        }
        // 回退：遍历 output[].content[] 找 output_text 块
        if let Some(items) = data["output"].as_array() {
            for item in items {
                if let Some(parts) = item["content"].as_array() {
                    for part in parts {
                        if part["type"].as_str() == Some("output_text") {
                            if let Some(text) = part["text"].as_str() {
                                return Ok(text.to_string());
                            }
                        }
                    }
                }
            }
        }
        Err(model_error("OpenAI Responses result did not contain output text"))
    }

    async fn anthropic_messages(
        profile: &ModelProfile,
        system_prompt: &str,
        user_prompt: &str,
        images: &[ImageInput],
        timeout_seconds: Option<u64>,
        proxy_url: Option<&str>,
    ) -> AppResult<String> {
        let url = format!(
            "{}/messages",
            normalize_base_url(&profile.base_url, &profile.protocol)
        );
        let mut content = vec![json!({"type": "text", "text": user_prompt})];
        content.extend(images.iter().map(|image| {
            json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": image.mime_type,
                    "data": image.b64
                }
            })
        }));
        let mut payload = json!({
            "model": profile.model,
            "max_tokens": 4096,
            "system": system_prompt,
            "messages": [{"role": "user", "content": content}]
        });
        let api_key = require_api_key(profile)?;
        let version = profile
            .api_version
            .clone()
            .unwrap_or_else(|| "2023-06-01".to_string());
        let headers = vec![
            ("x-api-key".to_string(), api_key),
            ("anthropic-version".to_string(), version),
        ];
        if stream_enabled(profile) {
            payload["stream"] = Value::Bool(true);
            let events = Self::stream(profile, &url, &payload, headers, timeout_seconds, proxy_url).await?;
            return collect_stream_text("anthropic_messages", &events);
        }
        let outcome = post_json_with_retries(
            profile,
            &url,
            &payload,
            headers,
            PostOptions {
                timeout_seconds,
                proxy_url,
                ..Default::default()
            },
        )
        .await?;
        if outcome.status >= 400 {
            return Err(model_error(format!(
                "Model request failed: HTTP {} {}",
                outcome.status,
                outcome.body.chars().take(400).collect::<String>()
            )));
        }
        let data: Value = serde_json::from_str(&outcome.body)
            .map_err(|error| model_error(format!("Anthropic 响应不是 JSON: {error}")))?;
        let text = data["content"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter(|part| part["type"].as_str() == Some("text"))
                    .filter_map(|part| part["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        Ok(text)
    }

    async fn post(
        profile: &ModelProfile,
        url: &str,
        payload: &Value,
        timeout_seconds: Option<u64>,
        proxy_url: Option<&str>,
    ) -> AppResult<Value> {
        let outcome = post_json_with_retries(
            profile,
            url,
            payload,
            Self::bearer(profile)?,
            PostOptions {
                timeout_seconds,
                proxy_url,
                ..Default::default()
            },
        )
        .await?;
        if outcome.status >= 400 {
            return Err(model_error(format!(
                "Model request failed: HTTP {} {}",
                outcome.status,
                outcome.body.chars().take(400).collect::<String>()
            )));
        }
        serde_json::from_str(&outcome.body)
            .map_err(|error| model_error(format!("模型响应不是 JSON: {error}")))
    }

    fn bearer(profile: &ModelProfile) -> AppResult<Vec<(String, String)>> {
        Ok(vec![(
            "Authorization".to_string(),
            format!("Bearer {}", require_api_key(profile)?),
        )])
    }

    /// 流式请求，返回原始 SSE 事件；按协议拼文本交给 `collect_stream_text`。
    ///
    /// 错误分支和非流式那条刻意保持同一句措辞（`Model request failed: HTTP …`），
    /// 这样开关流式之后，同一个上游故障在界面上还是同一条信息。
    async fn stream(
        profile: &ModelProfile,
        url: &str,
        payload: &Value,
        headers: Vec<(String, String)>,
        timeout_seconds: Option<u64>,
        proxy_url: Option<&str>,
    ) -> AppResult<Vec<Value>> {
        let outcome = post_sse_with_retries(
            profile,
            url,
            payload,
            headers,
            PostOptions {
                timeout_seconds,
                proxy_url,
                ..Default::default()
            },
        )
        .await?;
        if outcome.status >= 400 {
            return Err(model_error(format!(
                "Model request failed: HTTP {} {}",
                outcome.status,
                outcome.body.chars().take(400).collect::<String>()
            )));
        }
        Ok(outcome.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(raw: &[&str]) -> Vec<Value> {
        raw.iter()
            .map(|line| serde_json::from_str(line).expect("测试数据是合法 JSON"))
            .collect()
    }

    fn profile_with(stream: Option<Value>) -> ModelProfile {
        let mut output_defaults = serde_json::Map::new();
        if let Some(value) = stream {
            output_defaults.insert("stream".to_string(), value);
        }
        ModelProfile {
            id: "design".to_string(),
            role: "design".to_string(),
            name: "Design model".to_string(),
            protocol: "openai_responses".to_string(),
            base_url: "https://example.com".to_string(),
            model: "m".to_string(),
            api_key: Some("k".to_string()),
            api_version: None,
            headers: serde_json::Map::new(),
            timeout_seconds: 120,
            max_retries: 0,
            output_defaults,
            has_api_key: Some(true),
            api_key_hint: None,
        }
    }

    /// 前端把开关写成字符串 `"true"`，配置里也可能是布尔。两种都得认，
    /// 而没写过这个键的老配置必须是「关」——不然升级一下就悄悄改了请求形状。
    #[test]
    fn stream_flag_accepts_both_literals_and_defaults_off() {
        assert!(!stream_enabled(&profile_with(None)));
        assert!(stream_enabled(&profile_with(Some(Value::Bool(true)))));
        assert!(stream_enabled(&profile_with(Some(Value::String("true".into())))));
        assert!(stream_enabled(&profile_with(Some(Value::String(" ON ".into())))));
        assert!(!stream_enabled(&profile_with(Some(Value::String("false".into())))));
    }

    /// 三种协议的增量字段各不相同，拼出来必须和非流式取到的文本一致。
    #[test]
    fn collects_text_from_each_protocol() {
        let chat = events(&[
            r#"{"choices":[{"delta":{"role":"assistant"}}]}"#,
            r#"{"choices":[{"delta":{"content":"{\"layout\""}}]}"#,
            r#"{"choices":[{"delta":{"content":":1}"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(collect_stream_text("openai_chat", &chat).unwrap(), "{\"layout\":1}");

        let responses = events(&[
            r#"{"type":"response.created"}"#,
            r#"{"type":"response.output_text.delta","delta":"前半"}"#,
            r#"{"type":"response.output_text.delta","delta":"后半"}"#,
            r#"{"type":"response.completed"}"#,
        ]);
        assert_eq!(collect_stream_text("openai_responses", &responses).unwrap(), "前半后半");

        let anthropic = events(&[
            r#"{"type":"message_start"}"#,
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"图"}}"#,
            r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"忽略"}}"#,
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"例"}}"#,
        ]);
        assert_eq!(collect_stream_text("anthropic_messages", &anthropic).unwrap(), "图例");
    }

    /// 有的实现不发增量，只在收尾事件里给全文。这时也得取到，
    /// 不然界面上是「没有返回文本」，而上游其实成功了还计了费。
    #[test]
    fn falls_back_to_the_completed_event_when_no_deltas_arrive() {
        let only_completed = events(&[
            r#"{"type":"response.created"}"#,
            r#"{"type":"response.completed","response":{"output_text":"全文"}}"#,
        ]);
        assert_eq!(collect_stream_text("openai_responses", &only_completed).unwrap(), "全文");
    }

    /// 流中途报错时，报的必须是上游那句话。只说「没有文本」会把
    /// 配额用尽、模型名写错这类一眼能改的问题藏起来。
    #[test]
    fn surfaces_an_error_event_instead_of_an_empty_result() {
        let failed = events(&[
            r#"{"choices":[{"delta":{"content":"半截"}}]}"#,
            r#"{"error":{"message":"insufficient quota"}}"#,
        ]);
        let error = collect_stream_text("openai_chat", &failed).expect_err("错误事件应当报错");
        assert!(error.message.contains("insufficient quota"), "实际：{}", error.message);

        let refused = events(&[r#"{"type":"response.failed","response":{"error":{"message":"model not found"}}}"#]);
        let error = collect_stream_text("openai_responses", &refused).expect_err("失败事件应当报错");
        assert!(error.message.contains("model not found"), "实际：{}", error.message);
    }

    /// 上游不支持 stream 时常见的表现是回一个空流。这时候要指路到开关，
    /// 否则用户只看到「没有返回文本」，不知道去哪关。
    #[test]
    fn empty_stream_points_at_the_toggle() {
        let error = collect_stream_text("openai_chat", &[]).expect_err("空流应当报错");
        assert!(error.message.contains("stream"), "实际：{}", error.message);
    }
}
