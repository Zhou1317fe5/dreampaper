//! Implement model 客户端，移植自 `backend/app/adapters.py::ImplementClient`。
//!
//! 返回值统一为图片的 base64。image2 走 OpenAI images 接口（无参考图用
//! generations，有参考图用 multipart edits），banana2 走 Gemini 风格的
//! interactions 接口。两者都强制至少 120s 读超时——同步出图常远超 design 超时。

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::core::config::ModelProfile;
use crate::core::model::design::ImageInput;
use crate::core::net::{
    build_client, decode_b64, effective_timeout, encode_b64, format_http_error, is_retryable,
    merged_headers, model_error, normalize_base_url, post_json_with_retries, require_api_key,
    retry_backoff, MIN_IMAGE_TIMEOUT_SECONDS,
};
use crate::error::AppResult;

pub struct ImplementClient;

impl ImplementClient {
    pub async fn generate(
        profile: &ModelProfile,
        prompt: &str,
        reference_images: &[ImageInput],
        output_overrides: &Map<String, Value>,
        proxy_url: Option<&str>,
    ) -> AppResult<String> {
        let protocol = if profile.protocol == "banna2" {
            "banana2"
        } else {
            profile.protocol.as_str()
        };
        match protocol {
            "image2" => Self::image2(profile, prompt, reference_images, output_overrides, proxy_url).await,
            "banana2" => Self::banana2(profile, prompt, reference_images, output_overrides, proxy_url).await,
            other => Err(model_error(format!("Unsupported implement protocol: {other}"))),
        }
    }

    /// 合并 profile.output_defaults 与本次覆盖项，丢掉空值。
    fn merged_defaults(
        profile: &ModelProfile,
        overrides: &Map<String, Value>,
    ) -> BTreeMap<String, Value> {
        let mut merged: BTreeMap<String, Value> = BTreeMap::new();
        for (key, value) in profile.output_defaults.iter().chain(overrides.iter()) {
            let empty = value.is_null() || value.as_str().is_some_and(|text| text.is_empty());
            if empty {
                merged.remove(key);
            } else {
                merged.insert(key.clone(), value.clone());
            }
        }
        merged
    }

    /// image2 请求字段白名单；默认 response_format=url，避免大体积 b64 被网关截断。
    fn image2_fields(defaults: &BTreeMap<String, Value>) -> Map<String, Value> {
        let mut fields = Map::new();
        let pick = |key: &str, fallback: &str| -> Value {
            defaults
                .get(key)
                .and_then(|value| value.as_str())
                .filter(|text| !text.is_empty())
                .map(|text| Value::String(text.to_string()))
                .unwrap_or_else(|| Value::String(fallback.to_string()))
        };
        fields.insert("size".to_string(), pick("size", "1200x675"));
        fields.insert("quality".to_string(), pick("quality", "auto"));
        fields.insert("response_format".to_string(), pick("response_format", "url"));
        let n = defaults
            .get("n")
            .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
            .unwrap_or(1);
        fields.insert("n".to_string(), json!(n));
        for key in ["background", "moderation", "output_format", "output_compression", "user"] {
            if let Some(value) = defaults.get(key) {
                if !value.is_null() && value.as_str() != Some("") {
                    fields.insert(key.to_string(), value.clone());
                }
            }
        }
        fields
    }

    async fn image2(
        profile: &ModelProfile,
        prompt: &str,
        reference_images: &[ImageInput],
        output_overrides: &Map<String, Value>,
        proxy_url: Option<&str>,
    ) -> AppResult<String> {
        let base = normalize_base_url(&profile.base_url, "image2");
        let defaults = Self::merged_defaults(profile, output_overrides);
        let fields = Self::image2_fields(&defaults);
        let api_key = require_api_key(profile)?;
        let read_timeout = effective_timeout(profile, None, Some(MIN_IMAGE_TIMEOUT_SECONDS));

        let (status, body) = if reference_images.is_empty() {
            let mut payload = json!({"model": profile.model, "prompt": prompt});
            if let Some(object) = payload.as_object_mut() {
                for (key, value) in &fields {
                    object.insert(key.clone(), value.clone());
                }
            }
            let outcome = post_json_with_retries(
                profile,
                &format!("{base}/images/generations"),
                &payload,
                vec![("Authorization".to_string(), format!("Bearer {api_key}"))],
                None,
                proxy_url,
                Some(MIN_IMAGE_TIMEOUT_SECONDS),
            )
            .await?;
            (outcome.status, outcome.body)
        } else {
            Self::post_multipart(
                profile,
                &format!("{base}/images/edits"),
                prompt,
                reference_images,
                &fields,
                &api_key,
                read_timeout,
                proxy_url,
            )
            .await?
        };

        if status >= 400 {
            return Err(model_error(format_http_error("Image request", status, &body)));
        }
        let data: Value = serde_json::from_str(&body).map_err(|_| {
            model_error(format!(
                "Image response is not JSON: {}",
                body.chars().take(200).collect::<String>()
            ))
        })?;
        let image = data["data"]
            .as_array()
            .and_then(|items| items.first())
            .cloned()
            .unwrap_or(Value::Null);
        if let Some(b64) = image["b64_json"].as_str() {
            return Ok(b64.to_string());
        }
        if let Some(url) = image["url"].as_str() {
            let client = build_client(read_timeout, proxy_url)?;
            let response = client
                .get(url)
                .send()
                .await
                .map_err(|error| model_error(format!("Image download failed: {error}")))?;
            let status = response.status().as_u16();
            if status >= 400 {
                let text = response.text().await.unwrap_or_default();
                return Err(model_error(format_http_error("Image download", status, &text)));
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|error| model_error(format!("Image download failed: {error}")))?;
            return Ok(encode_b64(&bytes));
        }
        Err(model_error("Image response did not contain b64_json or url"))
    }

    #[allow(clippy::too_many_arguments)]
    async fn post_multipart(
        profile: &ModelProfile,
        url: &str,
        prompt: &str,
        reference_images: &[ImageInput],
        fields: &Map<String, Value>,
        api_key: &str,
        read_timeout: u64,
        proxy_url: Option<&str>,
    ) -> AppResult<(u16, String)> {
        let client = build_client(read_timeout, proxy_url)?;
        let attempts = (profile.max_retries.max(0) as u32) + 1;
        let mut last_error: Option<String> = None;

        for attempt in 0..attempts {
            let mut form = reqwest::multipart::Form::new()
                .text("model", profile.model.clone())
                .text("prompt", prompt.to_string());
            for (key, value) in fields {
                let text = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                form = form.text(key.clone(), text);
            }
            for image in reference_images {
                let bytes = decode_b64(&image.b64)?;
                let part = reqwest::multipart::Part::bytes(bytes)
                    .file_name(image.filename.clone())
                    .mime_str(&image.mime_type)
                    .map_err(|error| model_error(format!("参考图 MIME 无效: {error}")))?;
                form = form.part("image", part);
            }

            let mut request = client.post(url).multipart(form);
            for (key, value) in merged_headers(profile, vec![]) {
                // multipart 的 Content-Type 由 reqwest 自行设置，不能覆盖
                if key.eq_ignore_ascii_case("content-type") {
                    continue;
                }
                request = request.header(key, value);
            }
            request = request.header("Authorization", format!("Bearer {api_key}"));

            match request.send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let body = response.text().await.unwrap_or_default();
                    if !is_retryable(status) || attempt == attempts - 1 {
                        return Ok((status, body));
                    }
                    tokio::time::sleep(retry_backoff(attempt, Some(status))).await;
                }
                Err(error) => {
                    last_error = Some(if error.is_timeout() {
                        format!("模型请求超时：读超时 {read_timeout} 秒内未收到完整响应")
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
            last_error.unwrap_or_else(|| "模型请求失败：未收到有效响应".to_string()),
        ))
    }

    async fn banana2(
        profile: &ModelProfile,
        prompt: &str,
        reference_images: &[ImageInput],
        output_overrides: &Map<String, Value>,
        proxy_url: Option<&str>,
    ) -> AppResult<String> {
        let defaults = Self::merged_defaults(profile, output_overrides);
        let version = profile.api_version.clone().unwrap_or_else(|| "v1beta".to_string());
        let url = format!("{}/{version}/interactions", profile.base_url.trim_end_matches('/'));

        let mut input: Vec<Value> = reference_images
            .iter()
            .map(|image| json!({"type": "image", "mime_type": image.mime_type, "data": image.b64}))
            .collect();
        input.push(json!({"type": "text", "text": prompt}));

        let text_default = |key: &str, fallback: &str| -> String {
            defaults
                .get(key)
                .and_then(|value| value.as_str())
                .filter(|text| !text.is_empty())
                .unwrap_or(fallback)
                .to_string()
        };
        let payload = json!({
            "model": profile.model,
            "input": input,
            "response_format": {
                "type": "image",
                "aspect_ratio": text_default("aspect_ratio", "16:9"),
                "image_size": text_default("image_size", "4K")
            },
            "generation_config": {"thinking_level": text_default("thinking_level", "high")}
        });

        let outcome = post_json_with_retries(
            profile,
            &url,
            &payload,
            vec![("x-goog-api-key".to_string(), require_api_key(profile)?)],
            None,
            proxy_url,
            Some(MIN_IMAGE_TIMEOUT_SECONDS),
        )
        .await?;
        if outcome.status >= 400 {
            return Err(model_error(format_http_error(
                "Gemini image request",
                outcome.status,
                &outcome.body,
            )));
        }
        let data: Value = serde_json::from_str(&outcome.body)
            .map_err(|error| model_error(format!("Gemini 响应不是 JSON: {error}")))?;
        Self::extract_gemini_image(&data)
            .ok_or_else(|| model_error("Gemini response did not contain image data"))
    }

    /// 兼容两种响应形态：steps[].content[] 与 candidates[].content.parts[]。
    fn extract_gemini_image(data: &Value) -> Option<String> {
        if let Some(steps) = data["steps"].as_array() {
            for step in steps {
                if step["type"].as_str() != Some("model_output") {
                    continue;
                }
                if let Some(blocks) = step["content"].as_array() {
                    for block in blocks {
                        if block["type"].as_str() == Some("image") {
                            if let Some(value) = block["data"].as_str() {
                                return Some(value.to_string());
                            }
                        }
                    }
                }
            }
        }
        if let Some(candidates) = data["candidates"].as_array() {
            for candidate in candidates {
                if let Some(parts) = candidate["content"]["parts"].as_array() {
                    for part in parts {
                        let inline = if part["inlineData"].is_object() {
                            &part["inlineData"]
                        } else {
                            &part["inline_data"]
                        };
                        if let Some(value) = inline["data"].as_str() {
                            return Some(value.to_string());
                        }
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_image_from_both_gemini_shapes() {
        let steps = json!({"steps": [{"type": "model_output", "content": [{"type": "image", "data": "AAA"}]}]});
        assert_eq!(ImplementClient::extract_gemini_image(&steps).as_deref(), Some("AAA"));
        let candidates = json!({"candidates": [{"content": {"parts": [{"inlineData": {"data": "BBB"}}]}}]});
        assert_eq!(ImplementClient::extract_gemini_image(&candidates).as_deref(), Some("BBB"));
        let snake = json!({"candidates": [{"content": {"parts": [{"inline_data": {"data": "CCC"}}]}}]});
        assert_eq!(ImplementClient::extract_gemini_image(&snake).as_deref(), Some("CCC"));
        assert!(ImplementClient::extract_gemini_image(&json!({"data": []})).is_none());
    }

    #[test]
    fn image2_defaults_prefer_url_response() {
        let defaults = BTreeMap::new();
        let fields = ImplementClient::image2_fields(&defaults);
        assert_eq!(fields["response_format"], json!("url"));
        assert_eq!(fields["size"], json!("1200x675"));
        assert_eq!(fields["n"], json!(1));
    }
}
