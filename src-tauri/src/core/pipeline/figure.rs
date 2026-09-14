use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

use crate::core::config::{AppConfig, ModelProfile};
use crate::core::memory::{Fingerprint, MemoryCase};
use crate::core::model::design::ImageInput;
use crate::core::model::implement::ImplementClient;
use crate::core::pipeline::advisor::{AdvisorHook, AdvisorRun};
use crate::core::pipeline::hook::{injected_summary, ContextHooks, Section, StaticContext};
use crate::core::pipeline::runner::{parse_validate_or_fill, DesignCall, DesignStep};
use crate::core::pipeline::{contract, validate};
use crate::core::prompt::PromptStore;
use crate::error::{AppError, AppResult};
use crate::event::DesignSink;

/// Stage keys the context hooks are registered against.
pub const STAGE_STRUCTURE: &str = "paper_structure";
pub const STAGE_DESIGN: &str = "paper_design";

#[derive(Clone, Debug, Deserialize)]
pub struct PaperFigurePayload {
    #[serde(default)]
    pub figure_title: String,
    #[serde(default)]
    pub section_description: String,
    #[serde(default)]
    pub template_ids: Vec<String>,
    #[serde(default = "default_ratio")]
    pub aspect_ratio: String,
    #[serde(default = "default_fidelity")]
    pub layout_fidelity: String,
    #[serde(default = "default_strength")]
    pub style_strength: String,
    #[serde(default)]
    pub custom_prompt: Option<String>,
}

fn default_ratio() -> String {
    "inherit".to_string()
}
fn default_fidelity() -> String {
    "balanced".to_string()
}
fn default_strength() -> String {
    "high".to_string()
}

pub type StageSink<'a> = &'a (dyn Fn(&str, &str) + Send + Sync);

pub fn validate_payload(payload: &PaperFigurePayload) -> AppResult<()> {
    if payload.template_ids.is_empty() {
        return Err(AppError::new(
            "invalid_payload",
            "Paper figure requires at least one template",
        ));
    }
    Ok(())
}

pub struct FigureRun<'a> {
    pub prompts: &'a PromptStore,
    pub config: &'a AppConfig,
    pub design_profile: &'a ModelProfile,
    pub implement_profile: &'a ModelProfile,
    pub template_images: Vec<ImageInput>,
    pub template_metadata: Value,
    pub app_data: &'a Path,
    pub job_id: &'a str,
    pub design_log: DesignSink<'a>,
    /// Recalled cases for the advisor step; empty means the step is skipped.
    pub similar_cases: Vec<MemoryCase>,
    pub fingerprint: Option<Fingerprint>,
}

/// What a figure run hands back: the image and the design product that the
/// memory keeps as the job's case.
pub struct FigureOutput {
    pub image_b64: String,
    pub design: Value,
}

fn save_design_diagnostic(
    app_data: &Path,
    job_id: &str,
    name: &str,
    text: &str,
) -> AppResult<PathBuf> {
    let dir = app_data.join("logs").join(job_id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    std::fs::write(&path, text)?;
    Ok(path)
}

fn visible_text_diagnostic(text: &str) -> Value {
    let parsed = serde_json::from_str::<Value>(text).ok();
    let value = parsed
        .as_ref()
        .and_then(|root| root.get("figure").or(Some(root)))
        .and_then(|figure| figure.get("visible_text"));
    match value {
        Some(Value::Array(items)) => {
            let invalid = items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| match item.as_str() {
                    Some(text) if text.trim().chars().count() <= 80 => None,
                    Some(text) => Some(serde_json::json!({
                        "index": index,
                        "type": "string",
                        "length": text.trim().chars().count(),
                        "excerpt": text.chars().take(120).collect::<String>(),
                    })),
                    None => Some(serde_json::json!({
                        "index": index,
                        "type": match item {
                            Value::Object(_) => "object",
                            Value::Array(_) => "array",
                            Value::Number(_) => "number",
                            Value::Bool(_) => "boolean",
                            Value::Null => "null",
                            Value::String(_) => "string",
                        },
                    })),
                })
                .collect::<Vec<_>>();
            serde_json::json!({"type": "array", "count": items.len(), "invalid_items": invalid})
        }
        Some(value) => serde_json::json!({
            "type": match value {
                Value::Object(_) => "object",
                Value::Array(_) => "array",
                Value::String(_) => "string",
                Value::Number(_) => "number",
                Value::Bool(_) => "boolean",
                Value::Null => "null",
            }
        }),
        None => serde_json::json!({"type": "missing_or_unparseable"}),
    }
}

impl FigureRun<'_> {
    pub async fn run(
        &self,
        payload: &PaperFigurePayload,
        stage: StageSink<'_>,
    ) -> AppResult<FigureOutput> {
        let proxy = self.config.proxy_url.as_deref();

        let system = self.prompts.load("global/system.md")?;
        let template_summary = serde_json::to_string_pretty(&self.template_metadata)?;

        // Everything the stages share is a hook; the stage code only adds
        // what is intrinsic to its own call (the task line, the plan it just
        // produced). Contracts are registered per stage and always close the
        // prompt.
        let mut hooks = ContextHooks::default();
        hooks.register(StaticContext::new(
            "template-metadata",
            &[STAGE_STRUCTURE, STAGE_DESIGN],
            "Selected Template Metadata",
            template_summary,
        ));
        hooks.register(
            StaticContext::new(
                "structure-contract",
                &[STAGE_STRUCTURE],
                "Output Contract",
                contract::structure_plan(),
            )
            .contract(),
        );
        let user_context = serde_json::json!({
            "Figure title": payload.figure_title.trim(),
            "Section description": payload.section_description.trim(),
            "Aspect ratio": payload.aspect_ratio,
            "Layout fidelity": payload.layout_fidelity,
            "Style strength": payload.style_strength,
            "Custom prompt": payload.custom_prompt.clone().unwrap_or_else(|| "None".to_string()),
        });
        hooks.register(
            StaticContext::new(
                "content-inventory",
                &[STAGE_DESIGN],
                "User Input",
                serde_json::to_string_pretty(&user_context)?,
            )
            .at(-5),
        );
        hooks.register(
            StaticContext::new(
                "paper-contract",
                &[STAGE_DESIGN],
                "Output Contract",
                contract::paper(),
            )
            .contract(),
        );

        stage("paper_structure_prompt", "拼接 template 结构分析 prompt");
        let structure_assets = self.prompts.load_all(&[
            "global/system.md",
            "global/figure_style.md",
            "modes/paper_figure/structure.md",
        ])?;
        let structure_composed = hooks.compose(
            STAGE_STRUCTURE,
            &structure_assets,
            vec![Section::new(
                "Task",
                "Analyze the attached template figure image(s) only. \
                 Return a reusable structure_plan JSON. Do not invent the user's research content.",
            )
            .at(10)],
        );
        let structure_prompt = structure_composed.prompt;

        let structure_retry_count = std::sync::Mutex::new(0usize);
        let structure_app_data = self.app_data;
        let structure_job_id = self.job_id;
        let structure_retry_sink = move |text: &str| {
            let mut count = structure_retry_count.lock().unwrap();
            *count += 1;
            let name = format!("paper_structure_raw_retry{count}.txt");
            if save_design_diagnostic(structure_app_data, structure_job_id, &name, text).is_ok() {
                stage(
                    "paper_structure_parse",
                    &format!("校验未通过,自动重新请求 design model(第 {count} 次)"),
                );
            }
        };
        let structure_call = DesignCall {
            profile: self.design_profile,
            system_prompt: &system.content,
            user_prompt: &structure_prompt,
            images: &self.template_images,
            proxy_url: proxy,
            response_sink: Some(&structure_retry_sink),
            log: Some(DesignStep {
                sink: self.design_log,
                step: "paper_structure",
                label: "分析 template 结构",
            }),
        };

        stage("paper_structure", "调用 design model 分析 template 结构");
        let structure_text = structure_call.first().await?;
        save_design_diagnostic(
            self.app_data,
            self.job_id,
            "paper_structure_raw.txt",
            &structure_text,
        )?;

        stage("paper_structure_parse", "解析并校验结构规划 JSON");
        let structure = parse_validate_or_fill(&structure_call, &structure_text, |value| {
            validate::validate_structure_plan(value)
        })
        .await?;

        // The plan is stage-1 output, so it can only join the chain now.
        hooks.register(
            StaticContext::new(
                "structure-plan",
                &[STAGE_DESIGN],
                "Structure Plan From Templates",
                serde_json::to_string_pretty(&structure.value)?,
            )
            .at(-10),
        );

        if let Some(fingerprint) = self
            .fingerprint
            .as_ref()
            .filter(|_| !self.similar_cases.is_empty())
        {
            stage(
                "paper_advisor",
                &format!("advisor 比对 {} 个相似历史案例", self.similar_cases.len()),
            );
            let advisor = AdvisorRun {
                prompts: self.prompts,
                profile: self.design_profile,
                proxy_url: proxy,
                design_log: self.design_log,
            };
            match advisor.advise(fingerprint, &self.similar_cases).await {
                Some(advice) => hooks.register(AdvisorHook::new(&[STAGE_DESIGN], advice)),
                None => stage("paper_advisor", "advisor 未给出可用建议，按无历史案例继续"),
            }
        }

        let design_assets = self.prompts.load_all(&[
            "global/system.md",
            "global/figure_style.md",
            "global/expression.md",
            "modes/paper_figure/design.md",
            "modes/paper_figure/diagram_rules.md",
            "modes/paper_figure/plot_rules.md",
            "modes/paper_figure/validator.md",
        ])?;
        let design_composed = hooks.compose(STAGE_DESIGN, &design_assets, Vec::new());
        stage(
            "paper_prompt",
            &format!(
                "拼接内容填充 prompt（注入 {}）",
                injected_summary(&design_composed)
            ),
        );
        let design_prompt = design_composed.prompt;

        let no_images: Vec<ImageInput> = Vec::new();
        let retry_count = std::sync::Mutex::new(0usize);
        let app_data = self.app_data;
        let job_id = self.job_id;
        let retry_sink = move |text: &str| {
            let mut count = retry_count.lock().unwrap();
            *count += 1;
            let name = format!("paper_design_raw_retry{count}.txt");
            if save_design_diagnostic(app_data, job_id, &name, text).is_ok() {
                stage(
                    "paper_parse",
                    &format!("校验未通过,自动重新请求 design model(第 {count} 次)"),
                );
            }
        };
        let design_call = DesignCall {
            profile: self.design_profile,
            system_prompt: &system.content,
            user_prompt: &design_prompt,
            images: &no_images,
            proxy_url: proxy,
            response_sink: Some(&retry_sink),
            log: Some(DesignStep {
                sink: self.design_log,
                step: "paper_design",
                label: "映射内容并生成制图方案",
            }),
        };

        stage("paper_design", "调用 design model 映射内容并生成制图方案");
        let design_text = design_call.first().await?;
        let diagnostic_path = save_design_diagnostic(
            self.app_data,
            self.job_id,
            "paper_design_raw.txt",
            &design_text,
        )?;

        stage("paper_parse", "解析并校验 design JSON");
        let title = payload.figure_title.trim().to_string();
        let section = payload.section_description.trim().to_string();
        let design_result = parse_validate_or_fill(&design_call, &design_text, move |value| {
            validate::validate_paper_design(value)?;
            validate::validate_design_content_coverage(value, &title, &section)?;
            Ok(())
        })
        .await;
        let design = design_result.map_err(|error| {
            let mut detail = error.detail.unwrap_or_else(|| serde_json::json!({}));
            if !detail.is_object() {
                detail = serde_json::json!({"upstream_detail": detail});
            }
            if let Some(object) = detail.as_object_mut() {
                object.insert(
                    "design_raw_path".to_string(),
                    Value::String(diagnostic_path.display().to_string()),
                );
                object.insert(
                    "visible_text".to_string(),
                    visible_text_diagnostic(&design_text),
                );
                object.insert("summary".to_string(), Value::String(error.message.clone()));
                object.insert("code".to_string(), Value::String(error.code.clone()));
            }
            AppError::with_detail(error.code, error.message, detail)
        })?;

        let implement_prompt =
            validate::implement_prompt(&design.parsed).map_err(AppError::from)?;

        stage("paper_implement", "调用 implement model 生成图片");
        let image_b64 = ImplementClient::generate(
            self.implement_profile,
            &implement_prompt,
            &[],
            &Map::new(),
            proxy,
        )
        .await?;

        stage("paper_save", "保存生成图片");
        Ok(FigureOutput {
            image_b64,
            design: serde_json::json!({
                "structure_plan": structure.value,
                "design": design.parsed,
            }),
        })
    }
}
