//! Design model 客户端，移植自 `backend/app/adapters.py::DesignClient`。
//!
//! 三种协议的差别只在请求体结构与取文本的路径，超时/重试/代理统一由 `net` 承担。

use serde_json::{json, Value};

use crate::core::config::ModelProfile;
use crate::core::net::{
    data_url, model_error, normalize_base_url, post_json_with_retries, require_api_key,
};
use crate::error::AppResult;

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
        let payload = json!({
            "model": profile.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": content}
            ],
            "temperature": 0.2
        });
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
        let payload = json!({
            "model": profile.model,
            "instructions": system_prompt,
            "input": [{"role": "user", "content": content}],
            "text": {"format": {"type": "text"}}
        });
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
        let payload = json!({
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
        let outcome = post_json_with_retries(
            profile, &url, &payload, headers, timeout_seconds, proxy_url, None,
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
        let headers = vec![(
            "Authorization".to_string(),
            format!("Bearer {}", require_api_key(profile)?),
        )];
        let outcome = post_json_with_retries(
            profile, url, payload, headers, timeout_seconds, proxy_url, None,
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
}
