use serde_json::{json, Value};

use crate::core::config::ModelProfile;
use crate::core::net::{
    data_url, model_error, model_http_error, model_profile_error, normalize_base_url,
    post_json_with_retries, post_sse_with_retries, require_api_key, PostOptions,
};
use crate::error::AppResult;

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

fn design_max_tokens(profile: &ModelProfile) -> u64 {
    profile
        .output_defaults
        .get("max_tokens")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
        .unwrap_or(8192)
        .clamp(256, 32768)
}

fn collect_stream_text(protocol: &str, events: &[Value]) -> AppResult<String> {
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
    if protocol == "anthropic_messages"
        && events.iter().any(|event| {
            event["type"].as_str() == Some("message_delta")
                && event["delta"]["stop_reason"].as_str() == Some("max_tokens")
        })
    {
        return Err(model_error(
            "Anthropic 响应达到 max_tokens 后被截断。请提高 Design 的 max_tokens。",
        ));
    }
    Ok(text)
}

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
                Self::openai_chat(
                    profile,
                    system_prompt,
                    user_prompt,
                    images,
                    timeout_seconds,
                    proxy_url,
                )
                .await
            }
            "openai_responses" => {
                Self::openai_responses(
                    profile,
                    system_prompt,
                    user_prompt,
                    images,
                    timeout_seconds,
                    proxy_url,
                )
                .await
            }
            "anthropic_messages" => {
                Self::anthropic_messages(
                    profile,
                    system_prompt,
                    user_prompt,
                    images,
                    timeout_seconds,
                    proxy_url,
                )
                .await
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
            let events = Self::stream(
                profile,
                &url,
                &payload,
                Self::bearer(profile)?,
                timeout_seconds,
                proxy_url,
            )
            .await?;
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
            let events = Self::stream(
                profile,
                &url,
                &payload,
                Self::bearer(profile)?,
                timeout_seconds,
                proxy_url,
            )
            .await?;
            return collect_stream_text("openai_responses", &events);
        }
        let data = Self::post(profile, &url, &payload, timeout_seconds, proxy_url).await?;
        if let Some(text) = data["output_text"].as_str() {
            if !text.is_empty() {
                return Ok(text.to_string());
            }
        }
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
        Err(model_error(
            "OpenAI Responses result did not contain output text",
        ))
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
            "max_tokens": design_max_tokens(profile),
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
            let events =
                Self::stream(profile, &url, &payload, headers, timeout_seconds, proxy_url).await?;
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
            return Err(model_http_error(
                profile,
                &url,
                "Design",
                outcome.status,
                &outcome.body,
            ));
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
        if data["stop_reason"].as_str() == Some("max_tokens") {
            return Err(model_profile_error(
                profile,
                &url,
                "model_output_truncated",
                format!(
                    "Anthropic 响应达到 max_tokens={} 后被截断，无法保证 JSON 完整。",
                    design_max_tokens(profile)
                ),
                None,
                "请在 Design 高级参数中提高 max_tokens，或减少一次请求的上下文。",
            ));
        }
        if text.trim().is_empty() {
            let content_types = data["content"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| part["type"].as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "missing content array".to_string());
            return Err(model_profile_error(
                profile,
                &url,
                "model_empty_output",
                format!("Anthropic 响应没有可用文本（content types: {content_types}）。"),
                None,
                "请检查该中转服务是否完整兼容 Anthropic Messages 响应格式，或稍后重试。",
            ));
        }
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
            return Err(model_http_error(
                profile,
                url,
                "Design",
                outcome.status,
                &outcome.body,
            ));
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
            return Err(model_http_error(
                profile,
                url,
                "Design",
                outcome.status,
                &outcome.body,
            ));
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

    #[test]
    fn stream_flag_accepts_both_literals_and_defaults_off() {
        assert!(!stream_enabled(&profile_with(None)));
        assert!(stream_enabled(&profile_with(Some(Value::Bool(true)))));
        assert!(stream_enabled(&profile_with(Some(Value::String(
            "true".into()
        )))));
        assert!(stream_enabled(&profile_with(Some(Value::String(
            " ON ".into()
        )))));
        assert!(!stream_enabled(&profile_with(Some(Value::String(
            "false".into()
        )))));
    }

    #[test]
    fn design_max_tokens_defaults_high_and_accepts_an_override() {
        let mut profile = profile_with(None);
        assert_eq!(design_max_tokens(&profile), 8192);
        profile
            .output_defaults
            .insert("max_tokens".to_string(), Value::String("12000".to_string()));
        assert_eq!(design_max_tokens(&profile), 12000);
    }

    #[test]
    fn collects_text_from_each_protocol() {
        let chat = events(&[
            r#"{"choices":[{"delta":{"role":"assistant"}}]}"#,
            r#"{"choices":[{"delta":{"content":"{\"layout\""}}]}"#,
            r#"{"choices":[{"delta":{"content":":1}"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(
            collect_stream_text("openai_chat", &chat).unwrap(),
            "{\"layout\":1}"
        );

        let responses = events(&[
            r#"{"type":"response.created"}"#,
            r#"{"type":"response.output_text.delta","delta":"前半"}"#,
            r#"{"type":"response.output_text.delta","delta":"后半"}"#,
            r#"{"type":"response.completed"}"#,
        ]);
        assert_eq!(
            collect_stream_text("openai_responses", &responses).unwrap(),
            "前半后半"
        );

        let anthropic = events(&[
            r#"{"type":"message_start"}"#,
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"图"}}"#,
            r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"忽略"}}"#,
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"例"}}"#,
        ]);
        assert_eq!(
            collect_stream_text("anthropic_messages", &anthropic).unwrap(),
            "图例"
        );
    }

    #[test]
    fn falls_back_to_the_completed_event_when_no_deltas_arrive() {
        let only_completed = events(&[
            r#"{"type":"response.created"}"#,
            r#"{"type":"response.completed","response":{"output_text":"全文"}}"#,
        ]);
        assert_eq!(
            collect_stream_text("openai_responses", &only_completed).unwrap(),
            "全文"
        );
    }

    #[test]
    fn surfaces_an_error_event_instead_of_an_empty_result() {
        let failed = events(&[
            r#"{"choices":[{"delta":{"content":"半截"}}]}"#,
            r#"{"error":{"message":"insufficient quota"}}"#,
        ]);
        let error = collect_stream_text("openai_chat", &failed).expect_err("错误事件应当报错");
        assert!(
            error.message.contains("insufficient quota"),
            "实际：{}",
            error.message
        );

        let refused = events(&[
            r#"{"type":"response.failed","response":{"error":{"message":"model not found"}}}"#,
        ]);
        let error =
            collect_stream_text("openai_responses", &refused).expect_err("失败事件应当报错");
        assert!(
            error.message.contains("model not found"),
            "实际：{}",
            error.message
        );
    }

    #[test]
    fn empty_stream_points_at_the_toggle() {
        let error = collect_stream_text("openai_chat", &[]).expect_err("空流应当报错");
        assert!(error.message.contains("stream"), "实际：{}", error.message);
    }

    #[test]
    fn truncated_anthropic_stream_is_not_returned_as_complete_json() {
        let truncated = events(&[
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"{\"partial\":"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"}}"#,
        ]);
        let error =
            collect_stream_text("anthropic_messages", &truncated).expect_err("截断输出必须报错");
        assert!(error.message.contains("max_tokens"));
    }
}
