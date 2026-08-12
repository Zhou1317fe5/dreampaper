use std::future::Future;

use serde_json::Value;

use crate::core::config::ModelProfile;
use crate::core::model::design::{DesignClient, ImageInput};
use crate::core::net::{model_error, model_profile_error, normalize_base_url, parse_json_response};
use crate::core::pipeline::validate::ValidationError;
use crate::error::{AppError, AppResult};

const JSON_CONTEXT_RETRY_ATTEMPTS: usize = 1;
const JSON_RETRY_EXCERPT_LIMIT: usize = 4000;

fn excerpt(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

pub trait DesignGenerator {
    fn user_prompt(&self) -> &str;
    fn generate(&self, prompt: String) -> impl Future<Output = AppResult<String>> + Send;

    fn invalid_json_error(&self, _parse_error: &str, output: &str) -> AppError {
        model_error(format!(
            "模型多次返回无效 JSON（{}；响应字符数：{}）。",
            response_shape(output),
            output.chars().count()
        ))
    }
}

pub struct DesignCall<'a> {
    pub profile: &'a ModelProfile,
    pub system_prompt: &'a str,
    pub user_prompt: &'a str,
    pub images: &'a [ImageInput],
    pub timeout_seconds: Option<u64>,
    pub proxy_url: Option<&'a str>,
}

impl DesignGenerator for DesignCall<'_> {
    fn user_prompt(&self) -> &str {
        self.user_prompt
    }

    async fn generate(&self, prompt: String) -> AppResult<String> {
        DesignClient::generate(
            self.profile,
            self.system_prompt,
            &prompt,
            self.images,
            self.timeout_seconds,
            self.proxy_url,
        )
        .await
    }

    fn invalid_json_error(&self, parse_error: &str, output: &str) -> AppError {
        let endpoint = design_endpoint(self.profile);
        model_profile_error(
            self.profile,
            &endpoint,
            "model_invalid_json",
            format!(
                "Design 模型在重新生成和修复后仍未返回合法 JSON（{}；响应字符数：{}；解析错误：{}）。",
                response_shape(output),
                output.chars().count(),
                excerpt(parse_error, 240)
            ),
            None,
            "请重试；若持续出现，请确认中转服务完整返回模型文本，或切换同模型支持的协议。",
        )
    }
}

fn design_endpoint(profile: &ModelProfile) -> String {
    let suffix = match profile.protocol.as_str() {
        "openai_chat" => "chat/completions",
        "openai_responses" => "responses",
        "anthropic_messages" => "messages",
        _ => return profile.base_url.clone(),
    };
    format!(
        "{}/{suffix}",
        normalize_base_url(&profile.base_url, &profile.protocol)
    )
}

fn response_shape(output: &str) -> &'static str {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        "空响应"
    } else if trimmed.starts_with('<') || trimmed.to_ascii_lowercase().contains("<html") {
        "HTML 响应"
    } else if !trimmed.contains('{') && !trimmed.contains('[') {
        "纯文本响应，未发现 JSON 起始符"
    } else {
        "JSON 语法不完整或损坏"
    }
}

pub async fn parse_or_repair<G: DesignGenerator>(call: &G, text: &str) -> AppResult<Value> {
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
            call.user_prompt(),
            last_error,
            excerpt(text, JSON_RETRY_EXCERPT_LIMIT)
        );
        retry_text = call.generate(retry_prompt).await?;
        match parse_json_response(&retry_text) {
            Ok(value) => return Ok(value),
            Err(error) => last_error = error.message,
        }
    }

    let source = if retry_text.is_empty() {
        text
    } else {
        &retry_text
    };
    let repair_prompt = format!(
        "The previous model output was not valid JSON. Convert it into strict JSON only, preserving all useful content.\n\
         Return JSON only. Do not wrap in Markdown. Do not explain.\n\n\
         Original task:\n{}\n\nInvalid output:\n{}",
        call.user_prompt(),
        excerpt(source, JSON_RETRY_EXCERPT_LIMIT)
    );
    let repaired = call.generate(repair_prompt).await?;
    parse_json_response(&repaired)
        .map_err(|error| call.invalid_json_error(&error.message, &repaired))
}

#[derive(Debug)]
pub struct Validated<T> {
    pub parsed: Value,
    pub value: T,
    pub retried: bool,
}

pub async fn parse_validate_or_fill<G, T, F>(
    call: &G,
    text: &str,
    validator: F,
) -> AppResult<Validated<T>>
where
    G: DesignGenerator,
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
        call.user_prompt(),
        serde_json::to_string_pretty(&parsed).unwrap_or_default()
    );
    let filled_text = call.generate(fill_prompt).await?;
    let filled = parse_json_response(&filled_text)?;
    let value = validator(&filled).map_err(AppError::from)?;
    Ok(Validated {
        parsed: filled,
        value,
        retried: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pipeline::validate::validate_paper_design;
    use std::sync::Mutex;

    struct StubGenerator {
        user_prompt: String,
        responses: Mutex<Vec<String>>,
        prompts: Mutex<Vec<String>>,
    }

    impl StubGenerator {
        fn new(user_prompt: &str, responses: &[&str]) -> Self {
            Self {
                user_prompt: user_prompt.to_string(),
                responses: Mutex::new(
                    responses
                        .iter()
                        .rev()
                        .map(|item| item.to_string())
                        .collect(),
                ),
                prompts: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.prompts.lock().unwrap().len()
        }

        fn prompt(&self, index: usize) -> String {
            self.prompts.lock().unwrap()[index].clone()
        }
    }

    impl DesignGenerator for StubGenerator {
        fn user_prompt(&self) -> &str {
            &self.user_prompt
        }

        async fn generate(&self, prompt: String) -> AppResult<String> {
            self.prompts.lock().unwrap().push(prompt);
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| AppError::new("stub_exhausted", "no stub response left"))
        }
    }

    fn valid_diagram_design() -> Value {
        crate::core::pipeline::validate::tests::valid_diagram_design()
    }

    #[test]
    fn response_shape_distinguishes_common_gateway_failures() {
        assert_eq!(response_shape("  "), "空响应");
        assert_eq!(response_shape("<html>gateway</html>"), "HTML 响应");
        assert_eq!(
            response_shape("I cannot provide that format"),
            "纯文本响应，未发现 JSON 起始符"
        );
        assert_eq!(response_shape("{\"partial\":"), "JSON 语法不完整或损坏");
    }

    #[tokio::test]
    async fn parse_failure_retries_same_context_before_repair() {
        let stub = StubGenerator::new(
            "original task with complete context",
            [r#"{"ok": true}"#].as_slice(),
        );
        let parsed = parse_or_repair(&stub, r#"{"ok": true "broken": false}"#)
            .await
            .unwrap();

        assert_eq!(parsed, serde_json::json!({"ok": true}));
        assert_eq!(stub.calls(), 1);
        let retry_prompt = stub.prompt(0);
        assert!(retry_prompt.contains("original task with complete context"));
        assert!(retry_prompt.contains("Regenerate the answer using the same task context"));
        assert!(retry_prompt.contains("Previous invalid output excerpt"));
    }

    #[tokio::test]
    async fn valid_json_does_not_call_the_model() {
        let stub = StubGenerator::new("task", &[]);
        let parsed = parse_or_repair(&stub, r#"{"ok": true}"#).await.unwrap();
        assert_eq!(parsed, serde_json::json!({"ok": true}));
        assert_eq!(stub.calls(), 0);
    }

    #[tokio::test]
    async fn structured_fill_retry_only_fills_schema_gaps() {
        let filled = valid_diagram_design();
        let stub = StubGenerator::new("original task", &[&filled.to_string()]);

        let mut initial = valid_diagram_design();
        initial["figure"]
            .as_object_mut()
            .unwrap()
            .remove("diagram_spec");

        let result = parse_validate_or_fill(&stub, &initial.to_string(), |value| {
            validate_paper_design(value)
        })
        .await
        .unwrap();

        assert_eq!(result.parsed, filled);
        assert!(result.retried);
        assert_eq!(stub.calls(), 1);
        let fill_prompt = stub.prompt(0);
        assert!(fill_prompt.contains("Only fill"));
        assert!(fill_prompt.contains("do not redesign"));
        assert!(fill_prompt.contains("Diagram figure missing diagram_spec"));
    }

    #[tokio::test]
    async fn semantic_errors_fail_without_retry() {
        let stub = StubGenerator::new("original task", &[]);
        let error = parse_validate_or_fill(&stub, r#"{"ok": true}"#, |_| {
            Err::<(), _>(ValidationError::Value("page 3 out of order".to_string()))
        })
        .await
        .unwrap_err();

        assert_eq!(stub.calls(), 0);
        assert_eq!(error.message, "page 3 out of order");
    }
}
