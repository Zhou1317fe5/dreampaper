//! 科研图两阶段管道，移植自 `jobs.py::_run_paper`。
//!
//! 拆成两阶段的理由（不能合并）：
//! 阶段①只喂 template 图，模型不会把 template 里的研究内容误当成用户内容；
//! 阶段②只喂结构骨架和用户文字、不再带图，避免生成内容被图面「带跑」。

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::core::config::{AppConfig, ModelProfile};
use crate::core::model::design::ImageInput;
use crate::core::model::implement::ImplementClient;
use crate::core::pipeline::runner::{parse_validate_or_fill, DesignCall};
use crate::core::pipeline::{contract, validate};
use crate::core::prompt::{compose_prompt, PromptStore};
use crate::error::{AppError, AppResult};

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

/// 每个阶段回调一次，供上层写 job_stages 并推送事件。
pub type StageSink<'a> = &'a (dyn Fn(&str, &str) + Send + Sync);

/// 校验 payload。`execute.rs` 在 `paper_validate` 阶段调用——
/// 校验要排在读 template 之前，取不到的 id 才不会先报成「模板不存在」。
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
}

impl FigureRun<'_> {
    /// 返回生成图片的 base64。
    ///
    /// 前两个阶段（`paper_validate` / `paper_templates`）由 `execute.rs`
    /// 在校验与解析 template 时上报，本方法从 `paper_structure_prompt` 接手。
    pub async fn run(
        &self,
        payload: &PaperFigurePayload,
        stage: StageSink<'_>,
    ) -> AppResult<String> {
        let proxy = self.config.proxy_url.as_deref();

        let system = self.prompts.load("global/system.md")?;
        let template_summary = serde_json::to_string_pretty(&self.template_metadata)?;

        // —— 阶段 1：只看 template 图，抽可复用的结构骨架 ——
        stage("paper_structure_prompt", "拼接 template 结构分析 prompt");
        let structure_assets = self.prompts.load_all(&[
            "global/system.md",
            "global/figure_style.md",
            "modes/paper_figure/structure.md",
        ])?;
        let structure_prompt = compose_prompt(
            &structure_assets,
            &[
                ("Selected Template Metadata", template_summary.clone()),
                (
                    "Task",
                    "Analyze the attached template figure image(s) only. \
                     Return a reusable structure_plan JSON. Do not invent the user's research content."
                        .to_string(),
                ),
                ("Output Contract", contract::structure_plan().to_string()),
            ],
        );

        stage("paper_structure", "调用 design model 分析 template 结构");
        let structure_call = DesignCall {
            profile: self.design_profile,
            system_prompt: &system.content,
            user_prompt: &structure_prompt,
            images: &self.template_images,
            timeout_seconds: None,
            proxy_url: proxy,
        };
        let structure_text = crate::core::model::design::DesignClient::generate(
            self.design_profile,
            &system.content,
            &structure_prompt,
            &self.template_images,
            None,
            proxy,
        )
        .await?;

        stage("paper_structure_parse", "解析并校验结构规划 JSON");
        let structure = parse_validate_or_fill(&structure_call, &structure_text, |value| {
            validate::validate_structure_plan(value)
        })
        .await?;

        // —— 阶段 2：结构骨架 + 用户内容 → 完整 design JSON（不再带图）——
        stage("paper_prompt", "拼接内容填充与 implement prompt");
        let design_assets = self.prompts.load_all(&[
            "global/system.md",
            "global/figure_style.md",
            "modes/paper_figure/design.md",
            "modes/paper_figure/diagram_rules.md",
            "modes/paper_figure/plot_rules.md",
            "modes/paper_figure/validator.md",
        ])?;
        let user_context = serde_json::json!({
            "Figure title": payload.figure_title.trim(),
            "Section description": payload.section_description.trim(),
            "Aspect ratio": payload.aspect_ratio,
            "Layout fidelity": payload.layout_fidelity,
            "Style strength": payload.style_strength,
            "Custom prompt": payload.custom_prompt.clone().unwrap_or_else(|| "None".to_string()),
        });
        let design_prompt = compose_prompt(
            &design_assets,
            &[
                (
                    "Structure Plan From Templates",
                    serde_json::to_string_pretty(&structure.value)?,
                ),
                ("User Input", serde_json::to_string_pretty(&user_context)?),
                ("Selected Template Metadata", template_summary),
                ("Output Contract", contract::paper().to_string()),
            ],
        );

        stage("paper_design", "调用 design model 映射内容并生成制图方案");
        let no_images: Vec<ImageInput> = Vec::new();
        let design_text = crate::core::model::design::DesignClient::generate(
            self.design_profile,
            &system.content,
            &design_prompt,
            &no_images,
            None,
            proxy,
        )
        .await?;

        stage("paper_parse", "解析并校验 design JSON");
        let design_call = DesignCall {
            profile: self.design_profile,
            system_prompt: &system.content,
            user_prompt: &design_prompt,
            images: &no_images,
            timeout_seconds: None,
            proxy_url: proxy,
        };
        let title = payload.figure_title.trim().to_string();
        let section = payload.section_description.trim().to_string();
        let design = parse_validate_or_fill(&design_call, &design_text, move |value| {
            validate::validate_paper_design(value)?;
            validate::validate_design_content_coverage(value, &title, &section)?;
            Ok(())
        })
        .await?;

        let implement_prompt = validate::implement_prompt(&design.parsed).map_err(AppError::from)?;

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
        Ok(image_b64)
    }
}
