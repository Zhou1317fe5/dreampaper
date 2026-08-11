use std::collections::{BTreeMap, VecDeque};

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::core::config::ModelProfile;
use crate::core::model::design::ImageInput;
use crate::core::net::{
    build_client, decode_b64, describe_transport_error, effective_timeout, encode_b64,
    format_http_error, is_retryable, merged_headers, model_error, normalize_base_url,
    post_json_with_retries, require_api_key, retry_backoff, retry_delay,
    IMAGE_RETRY_INTERVAL_SECONDS, MIN_IMAGE_TIMEOUT_SECONDS, PostOptions,
};
use crate::error::AppResult;

#[derive(Debug, PartialEq)]
enum ImagePayload {
    B64(String),
    Url(String),
}

pub struct ImplementClient;

fn image_post_options(proxy_url: Option<&str>) -> PostOptions<'_> {
    PostOptions {
        timeout_seconds: None,
        minimum_timeout: Some(MIN_IMAGE_TIMEOUT_SECONDS),
        retry_interval_seconds: Some(IMAGE_RETRY_INTERVAL_SECONDS),
        proxy_url,
    }
}

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
                image_post_options(proxy_url),
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

        Self::resolve_response("Image request", status, &body, read_timeout, proxy_url).await
    }

    async fn resolve_response(
        kind: &str,
        status: u16,
        body: &str,
        read_timeout: u64,
        proxy_url: Option<&str>,
    ) -> AppResult<String> {
        let parsed = serde_json::from_str::<Value>(body).ok();
        let found = parsed
            .as_ref()
            .and_then(find_image_payload)
            .or_else(|| parsed.as_ref().and_then(find_image_url_in_text));
        if let Some(payload) = found {
            return match payload {
                ImagePayload::B64(b64) => Ok(b64),
                ImagePayload::Url(url) => {
                    Self::download_image(&url, read_timeout, proxy_url).await
                }
            };
        }
        if status >= 400 {
            return Err(model_error(format_http_error(kind, status, body)));
        }
        let snippet: String = body.trim().chars().take(280).collect();
        if parsed.is_none() {
            return Err(model_error(format!("{kind} 响应不是 JSON: {snippet}")));
        }
        Err(model_error(format!(
            "{kind} 响应里没有图片数据（b64_json / url 都没找到）: {snippet}"
        )))
    }

    async fn download_image(
        url: &str,
        read_timeout: u64,
        proxy_url: Option<&str>,
    ) -> AppResult<String> {
        const ATTEMPTS: u32 = 3;
        let proxy = proxy_url
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let routes: Vec<Option<&str>> = match proxy {
            Some(value) => vec![Some(value), None],
            None => vec![None],
        };
        let mut last = "未知原因".to_string();
        for route in routes {
            for attempt in 0..ATTEMPTS {
                match Self::try_download(url, read_timeout, route).await {
                    Ok(b64) => return Ok(b64),
                    Err(message) => {
                        last = message;
                        if attempt + 1 < ATTEMPTS {
                            tokio::time::sleep(retry_backoff(attempt, None)).await;
                        }
                    }
                }
            }
        }
        Err(model_error(format!("Image download failed: {last}")))
    }

    async fn try_download(
        url: &str,
        read_timeout: u64,
        proxy_url: Option<&str>,
    ) -> Result<String, String> {
        let client = build_client(read_timeout.max(60), proxy_url).map_err(|error| error.message)?;
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|error| describe_transport_error(&error))?;
        let status = response.status().as_u16();
        if status >= 400 {
            let text = response.text().await.unwrap_or_default();
            return Err(format_http_error("Image download", status, &text));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| describe_transport_error(&error))?;
        if bytes.is_empty() {
            return Err("下载到 0 字节".to_string());
        }
        Ok(encode_b64(&bytes))
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
            let started = std::time::Instant::now();
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
                    tokio::time::sleep(retry_delay(
                        attempt,
                        Some(status),
                        Some(IMAGE_RETRY_INTERVAL_SECONDS),
                    ))
                    .await;
                }
                Err(error) => {
                    let elapsed = started.elapsed().as_secs();
                    last_error = Some(if error.is_timeout() {
                        format!(
                            "模型请求超时：读超时 {read_timeout} 秒内未收到完整响应（本次等待 {elapsed} 秒）"
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
                    tokio::time::sleep(retry_delay(
                        attempt,
                        None,
                        Some(IMAGE_RETRY_INTERVAL_SECONDS),
                    ))
                    .await;
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
            image_post_options(proxy_url),
        )
        .await?;
        if let Some(image) = serde_json::from_str::<Value>(&outcome.body)
            .ok()
            .as_ref()
            .and_then(Self::extract_gemini_image)
        {
            return Ok(image);
        }
        let read_timeout = effective_timeout(profile, None, Some(MIN_IMAGE_TIMEOUT_SECONDS));
        Self::resolve_response(
            "Gemini image request",
            outcome.status,
            &outcome.body,
            read_timeout,
            proxy_url,
        )
        .await
    }

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

fn find_image_payload(root: &Value) -> Option<ImagePayload> {
    let mut queue = VecDeque::new();
    queue.push_back(root);
    while let Some(node) = queue.pop_front() {
        match node {
            Value::Object(map) => {
                for key in ["b64_json", "b64", "image_base64", "imageBase64", "data"] {
                    if let Some(text) = map.get(key).and_then(Value::as_str) {
                        if let Some(b64) = as_image_b64(text) {
                            return Some(ImagePayload::B64(b64));
                        }
                    }
                }
                for key in ["url", "image_url", "imageUrl", "image"] {
                    match map.get(key) {
                        Some(Value::Object(inner)) => {
                            if let Some(text) = inner.get("url").and_then(Value::as_str) {
                                if let Some(payload) = as_image_reference(text) {
                                    return Some(payload);
                                }
                            }
                        }
                        Some(Value::String(text)) => {
                            if let Some(payload) = as_image_reference(text) {
                                return Some(payload);
                            }
                        }
                        _ => {}
                    }
                }
                queue.extend(map.values());
            }
            Value::Array(items) => queue.extend(items.iter()),
            _ => {}
        }
    }
    None
}

fn find_image_url_in_text(root: &Value) -> Option<ImagePayload> {
    let markdown = Regex::new(r"!\[[^\]]*\]\((https?://[^\s)]+)\)").expect("valid regex");
    let bare = Regex::new(r"https?://[^\s\]\)\x22']+\.(?:png|jpe?g|webp|gif)(?:\?[^\s\]\)\x22']*)?")
        .expect("valid regex");
    let mut queue = VecDeque::new();
    queue.push_back(root);
    while let Some(node) = queue.pop_front() {
        match node {
            Value::Object(map) => queue.extend(map.values()),
            Value::Array(items) => queue.extend(items.iter()),
            Value::String(text) => {
                if let Some(hit) = markdown
                    .captures(text)
                    .and_then(|caps| caps.get(1))
                    .map(|hit| hit.as_str().to_string())
                    .or_else(|| bare.find(text).map(|hit| hit.as_str().to_string()))
                {
                    return Some(ImagePayload::Url(hit));
                }
            }
            _ => {}
        }
    }
    None
}

fn as_image_reference(text: &str) -> Option<ImagePayload> {
    let text = text.trim();
    if text.starts_with("http://") || text.starts_with("https://") {
        return Some(ImagePayload::Url(text.to_string()));
    }
    as_image_b64(text).map(ImagePayload::B64)
}

fn as_image_b64(text: &str) -> Option<String> {
    let text = text.trim();
    let body = match text.split_once("base64,") {
        Some((prefix, rest)) if prefix.starts_with("data:") => rest.trim(),
        _ if text.starts_with("data:") => return None,
        _ => text,
    };
    if body.len() < 256 {
        return None;
    }
    let valid = body
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '-' | '_' | '\n' | '\r'));
    valid.then(|| body.to_string())
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

    fn long_b64() -> String {
        "iVBORw0KGgoAAAANSUhEUg".repeat(20)
    }

    #[test]
    fn finds_image_across_gateway_shapes() {
        let openai = json!({"data": [{"b64_json": long_b64()}]});
        assert_eq!(
            find_image_payload(&openai),
            Some(ImagePayload::B64(long_b64()))
        );

        let by_url = json!({"data": [{"url": "https://cdn.example.com/a.png"}]});
        assert_eq!(
            find_image_payload(&by_url),
            Some(ImagePayload::Url("https://cdn.example.com/a.png".to_string()))
        );

        let nested = json!({"result": {"outputs": [{"mime_type": "image/png", "data": long_b64()}]}});
        assert_eq!(find_image_payload(&nested), Some(ImagePayload::B64(long_b64())));

        let data_url = json!({"image": format!("data:image/png;base64,{}", long_b64())});
        assert_eq!(find_image_payload(&data_url), Some(ImagePayload::B64(long_b64())));

        let chat = json!({"choices": [{"message": {"images": [{"image_url": {"url": "https://x.io/b.png"}}]}}]});
        assert_eq!(
            find_image_payload(&chat),
            Some(ImagePayload::Url("https://x.io/b.png".to_string()))
        );
    }

    #[test]
    fn short_fields_are_not_mistaken_for_images() {
        let noise = json!({"id": "img_abc123", "model": "gpt-image-2", "data": "ok"});
        assert_eq!(find_image_payload(&noise), None);
        assert_eq!(as_image_b64("gpt-image-2"), None);
    }

    #[test]
    fn falls_back_to_url_inside_text() {
        let markdown = json!({"choices": [{"message": {"content": "画好了 ![img](https://cdn.example.com/x.png) 请查收"}}]});
        assert_eq!(
            find_image_url_in_text(&markdown),
            Some(ImagePayload::Url("https://cdn.example.com/x.png".to_string()))
        );
        let bare = json!({"output_text": "结果：https://cdn.example.com/y.jpeg?sig=1"});
        assert_eq!(
            find_image_url_in_text(&bare),
            Some(ImagePayload::Url("https://cdn.example.com/y.jpeg?sig=1".to_string()))
        );
        assert_eq!(find_image_url_in_text(&json!({"text": "没有图"})), None);
    }
}
