//! 管道共用的解析与重试策略，移植自 `jobs.py` 的
//! `_parse_or_repair` 与 `_parse_validate_or_fill_missing`。
//!
//! 两级重试的分工很重要，不能合并：
//! - 解析失败 → 带原上下文重发，让模型重新生成（而不是修字符串）
//! - 结构缺失 → 只补缺失字段，明确禁止重新设计，避免把已经对的内容改坏

use serde_json::Value;

use crate::core::config::ModelProfile;
use crate::core::model::design::{DesignClient, ImageInput};
use crate::core::net::parse_json_response;
use crate::core::pipeline::validate::ValidationError;
use crate::error::{AppError, AppResult};

const JSON_CONTEXT_RETRY_ATTEMPTS: usize = 1;
const JSON_RETRY_EXCERPT_LIMIT: usize = 4000;

fn excerpt(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

pub struct DesignCall<'a> {
    pub profile: &'a ModelProfile,
    pub system_prompt: &'a str,
    pub user_prompt: &'a str,
    pub images: &'a [ImageInput],
    pub timeout_seconds: Option<u64>,
    pub proxy_url: Option<&'a str>,
}

impl DesignCall<'_> {
    async fn generate(&self, prompt: &str) -> AppResult<String> {
        DesignClient::generate(
            self.profile,
            self.system_prompt,
            prompt,
            self.images,
            self.timeout_seconds,
            self.proxy_url,
        )
        .await
    }
}

/// 解析模型输出；失败时先带原上下文重发，仍失败再要求把上次输出转成 JSON。
pub async fn parse_or_repair(call: &DesignCall<'_>, text: &str) -> AppResult<Value> {
    let first_error = match parse_json_response(text) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };

    let mut last_error = first_error.message;
    let mut retry_text = String::new();

    for _ in 0..JSON_CONTEXT_RETRY_ATTEMPTS {
        let retry_prompt = format!(
            "{}\n\n\
             The previous response for this exact task was not parseable JSON. \
             Regenerate the answer using the same task context and return strict JSON only. \
             Do not wrap in Markdown. Do not explain. Do not omit required fields. \
             Fix JSON syntax issues such as missing commas, dangling quotes, trailing prose, and unescaped newlines.\n\n\
             Parse error: {}\n\
             Previous invalid output excerpt:\n{}",
            call.user_prompt,
            last_error,
            excerpt(text, JSON_RETRY_EXCERPT_LIMIT)
        );
        retry_text = call.generate(&retry_prompt).await?;
        match parse_json_response(&retry_text) {
            Ok(value) => return Ok(value),
            Err(error) => last_error = error.message,
        }
    }

    let source = if retry_text.is_empty() { text } else { &retry_text };
    let repair_prompt = format!(
        "The previous model output was not valid JSON. Convert it into strict JSON only, preserving all useful content.\n\
         Return JSON only. Do not wrap in Markdown. Do not explain.\n\n\
         Original task:\n{}\n\nInvalid output:\n{}",
        call.user_prompt,
        excerpt(source, JSON_RETRY_EXCERPT_LIMIT)
    );
    let repaired = call.generate(&repair_prompt).await?;
    parse_json_response(&repaired)
}

pub struct Validated<T> {
    pub parsed: Value,
    pub value: T,
    pub retried: bool,
}

/// 解析 + 校验；仅当校验报「结构缺失」时才发一次定向补全重试。
/// 语义越界（页码顺序、非法比例）直接失败，重试也修不好。
pub async fn parse_validate_or_fill<T, F>(
    call: &DesignCall<'_>,
    text: &str,
    validator: F,
) -> AppResult<Validated<T>>
where
    F: Fn(&Value) -> Result<T, ValidationError>,
{
    let parsed = parse_or_repair(call, text).await?;
    let error = match validator(&parsed) {
        Ok(value) => {
            return Ok(Validated {
                parsed,
                value,
                retried: false,
            })
        }
        Err(error) => error,
    };
    if !error.is_schema() {
        return Err(AppError::from(error));
    }

    let fill_prompt = format!(
        "The previous model output was valid JSON but failed the required structured output contract.\n\
         Return strict JSON only. Preserve all valid content and do not redesign the figure, slide master, or page plan.\n\
         Only fill, normalize, or add the missing required fields and constraints named by the validation error.\n\n\
         Validation error:\n{}\n\nOriginal task:\n{}\n\nCurrent JSON:\n{}",
        error.message(),
        call.user_prompt,
        serde_json::to_string_pretty(&parsed).unwrap_or_default()
    );
    let filled_text = call.generate(&fill_prompt).await?;
    let filled = parse_json_response(&filled_text)?;
    let value = validator(&filled).map_err(AppError::from)?;
    Ok(Validated {
        parsed: filled,
        value,
        retried: true,
    })
}
