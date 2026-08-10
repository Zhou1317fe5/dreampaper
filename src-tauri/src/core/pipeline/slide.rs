//! 幻灯片管道编排，移植自 `jobs.py::_run_ppt`。
//!
//! 阶段顺序不能改：
//! `母版分析 → deck outline → 逐页规划(并发) → 合并校验 → 注入母版 prefix → 逐页出图(并发)`
//!
//! 两个关键先后关系：
//! - 先出整套 outline 再逐页展开：逐页 worker 只看得到自己那页，
//!   叙事连贯与「不互相重复」全靠 outline 这一层统一裁定。
//! - prefix 必须在合并校验**之后**注入：校验要看模型自己写的 implement_prompt，
//!   prefix 一注入就有几百字模板文案，长度与关键词检查会全部失真。
//!
//! 本模块不碰数据库：template 图与资料文本由 `execute.rs` 解析后传入，
//! 因此整条编排可以脱离 Tauri 单测。

use std::future::Future;

use futures::stream::{StreamExt, TryStreamExt};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::core::config::{AppConfig, ModelProfile};
use crate::core::model::design::{DesignClient, ImageInput};
use crate::core::model::implement::ImplementClient;
use crate::core::pipeline::contract;
use crate::core::pipeline::figure::StageSink;
use crate::core::pipeline::runner::{parse_validate_or_fill, DesignCall};
use crate::core::pipeline::slide_validate::{
    apply_master_prompt_prefix, validate_ppt_outline, validate_ppt_pages, validate_ppt_single_page,
    validate_template_analysis,
};
use crate::core::pipeline::visual::{build_visual_asset_context, visual_asset_context_text};
use crate::core::prompt::{compose_prompt, PromptAsset, PromptStore};
use crate::error::{AppError, AppResult};

/// design 侧的超时下限：母版分析与逐页规划都是长 JSON 输出，
/// profile 默认的 120s 经常不够，取 profile 与本值的较大者。
const PPT_DESIGN_TIMEOUT_SECONDS: u64 = 300;

/// 逐页 prompt 的压缩上限。每页都要重发一遍这三段，
/// 不压缩的话 N 页会把同样的长文重复 N 次，既慢又容易触发上下文上限。
const PPT_PLAN_TEMPLATE_LIMIT: usize = 2600;
const PPT_PLAN_MATERIAL_LIMIT: usize = 3200;
const PPT_PLAN_VISUAL_CONTEXT_LIMIT: usize = 1400;

/// 单个资料文件送进 prompt 的字符上限。
pub const MATERIAL_TEXT_LIMIT: usize = 6000;

const MAX_PAGE_COUNT: usize = 20;

/// image2 未配置时的兜底出图参数，对齐 `IMAGE2_FALLBACK_OUTPUT`。
const IMAGE2_FALLBACK_OUTPUT: [(&str, &str); 4] = [
    ("size", "1200x675"),
    ("quality", "auto"),
    ("output_format", "png"),
    ("response_format", "b64_json"),
];

#[derive(Clone, Debug, Deserialize)]
pub struct PptSlidePayload {
    /// 上传得到的母版资源 id（网页版路径）。
    #[serde(default)]
    pub template_asset_id: String,
    /// 模板库中的母版 id（桌面版路径）。两者取其一即可，
    /// 桌面版把母版收进模板库后走这条，网页版仍走 asset 上传。
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub material_text: String,
    #[serde(default)]
    pub material_asset_ids: Vec<String>,
    #[serde(default = "default_page_count")]
    pub page_count: usize,
    #[serde(default)]
    pub custom_prompt: Option<String>,
}

impl PptSlidePayload {
    /// 母版来自模板库时返回其 id。空字符串按「未提供」处理，
    /// 免得前端传了个空串却当成有效来源。
    pub fn template_ref(&self) -> Option<&str> {
        self.template_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// 母版来自上传资源时返回其 id。
    pub fn asset_ref(&self) -> Option<&str> {
        Some(self.template_asset_id.trim()).filter(|value| !value.is_empty())
    }
}

fn default_page_count() -> usize {
    1
}

impl PptSlidePayload {
    pub fn custom_prompt_text(&self) -> String {
        match self.custom_prompt.as_deref().map(str::trim) {
            Some(value) if !value.is_empty() => value.to_string(),
            _ => "None".to_string(),
        }
    }
}

/// 资料文件摘要，对齐 `_material_asset_summary`。
/// （Python 版还带 asset id，那是给 internal_artifacts 用的；桌面端不落 artifacts，故省去。）
#[derive(Clone, Debug)]
pub struct MaterialAsset {
    pub filename: String,
    pub mime_type: String,
    pub bytes: u64,
    pub parser: String,
    pub text_excerpt: String,
    pub text_length: usize,
    pub truncated: bool,
}

impl MaterialAsset {
    pub fn new(
        filename: String,
        mime_type: String,
        bytes: u64,
        parser: String,
        text: &str,
    ) -> Self {
        let text_length = text.chars().count();
        let text_excerpt: String = text.chars().take(MATERIAL_TEXT_LIMIT).collect();
        Self {
            filename,
            mime_type,
            bytes,
            parser,
            truncated: text_length > text_excerpt.chars().count(),
            text_excerpt,
            text_length,
        }
    }
}

/// 校验 payload。`execute.rs` 在 `ppt_validate` 阶段调用。
pub fn validate_payload(payload: &PptSlidePayload) -> AppResult<()> {
    // 母版可以来自模板库，也可以来自上传；两条来源都没有才算缺参数。
    if payload.template_ref().is_none() && payload.asset_ref().is_none() {
        return Err(AppError::new(
            "invalid_payload",
            "PPT slide requires a template asset",
        ));
    }
    if payload.page_count < 1 || payload.page_count > MAX_PAGE_COUNT {
        return Err(AppError::new(
            "invalid_payload",
            format!("PPT page_count must be between 1 and {MAX_PAGE_COUNT}"),
        ));
    }
    if payload.material_asset_ids.len() > 10 {
        return Err(AppError::new(
            "invalid_payload",
            "PPT slide accepts at most 10 material files",
        ));
    }
    Ok(())
}

/// 拼资料上下文，移植自 `_compose_material_context`。
pub fn compose_material_context(material_text: &str, assets: &[MaterialAsset]) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !material_text.trim().is_empty() {
        sections.push(format!("User text material:\n{}", material_text.trim()));
    }
    for asset in assets {
        let mut header = format!(
            "File material: {} ({}, {} bytes, parser={})",
            asset.filename, asset.mime_type, asset.bytes, asset.parser
        );
        if asset.truncated {
            header.push_str(&format!(
                ", excerpt={}/{} chars",
                asset.text_excerpt.chars().count(),
                asset.text_length
            ));
        }
        let excerpt = if asset.text_excerpt.is_empty() {
            "No extractable text available; use filename and file type as context only."
        } else {
            asset.text_excerpt.as_str()
        };
        sections.push(format!("{header}\n{excerpt}"));
    }
    sections.join("\n\n---\n\n")
}

/// 并发数解析，移植自 `_resolve_ppt_concurrency`：
/// 未配置时按页数全开，配置了也不会超过页数（多开的槽位没有任务可跑）。
pub fn resolve_ppt_concurrency(configured: Option<i64>, page_count: usize) -> usize {
    let default_concurrency = page_count.max(1);
    let requested = match configured {
        Some(value) if value > 0 => value as usize,
        Some(_) => 1,
        None => default_concurrency,
    };
    requested.clamp(1, default_concurrency)
}

/// 超限时截断并附一行说明，让模型知道自己看到的是节选而非全文。
pub fn truncate_text(text: &str, limit: usize) -> String {
    let length = text.chars().count();
    if length <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit).collect();
    format!(
        "{head}\n\n[Truncated from {length} chars to {limit} chars for page planning latency.]"
    )
}

fn pick(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

/// 压缩母版分析，移植自 `_compact_template_analysis`。
/// 只丢 `page_layout_rules` 这类逐页 worker 用不上的长文，
/// 保留全部样式字段——它们是 prefix 的事实来源。
pub fn compact_template_analysis(analysis: &Value) -> String {
    let wrapper = if analysis.get("template_analysis").is_some_and(Value::is_object) {
        &analysis["template_analysis"]
    } else {
        analysis
    };
    if !wrapper.is_object() {
        let raw = serde_json::to_string(analysis).unwrap_or_default();
        return truncate_text(&raw, PPT_PLAN_TEMPLATE_LIMIT);
    }
    let empty = Value::Object(Map::new());
    let master = wrapper
        .get("master_style_spec")
        .filter(|value| value.is_object())
        .unwrap_or(&empty);
    let compact = json!({
        "master_style_summary": pick(wrapper, "master_style_summary"),
        "master_style_spec": {
            "canvas": pick(master, "canvas"),
            "title_region": pick(master, "title_region"),
            "safe_margins": pick(master, "safe_margins"),
            "header_footer": pick(master, "header_footer"),
            "divider_lines": pick(master, "divider_lines"),
            "palette": pick(master, "palette"),
            "typography": pick(master, "typography"),
            "module_style": pick(master, "module_style"),
            "decorative_elements": pick(master, "decorative_elements"),
            "immutable_elements": pick(master, "immutable_elements"),
            "forbidden_deviations": pick(master, "forbidden_deviations"),
        },
        "global_constraints": pick(wrapper, "global_constraints"),
        "immutable_elements": pick(wrapper, "immutable_elements"),
        "page_layout_rules": pick(wrapper, "page_layout_rules"),
    });
    truncate_text(
        &serde_json::to_string(&compact).unwrap_or_default(),
        PPT_PLAN_TEMPLATE_LIMIT,
    )
}

/// 前后页 brief，移植自 `_adjacent_page_context`。
/// 逐页 worker 靠它衔接转场，缺了各页会各写各的开场白。
pub fn adjacent_page_context(deck_outline: &Value, page_number: i64) -> Value {
    let briefs = deck_outline["page_briefs"].as_array();
    let find = |target: i64| -> Value {
        briefs
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.is_object() && item["page"].as_i64() == Some(target))
            })
            .cloned()
            .unwrap_or(Value::Null)
    };
    json!({"previous": find(page_number - 1), "next": find(page_number + 1)})
}

/// 出图参数，移植自 `_ppt_output_defaults`。
pub fn ppt_output_defaults(profile: &ModelProfile) -> Map<String, Value> {
    let protocol = if profile.protocol == "banna2" {
        "banana2"
    } else {
        profile.protocol.as_str()
    };
    let defaults: Map<String, Value> = profile
        .output_defaults
        .iter()
        .filter(|(_, value)| !value.is_null() && value.as_str() != Some(""))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    match protocol {
        "image2" => {
            let mut merged: Map<String, Value> = IMAGE2_FALLBACK_OUTPUT
                .iter()
                .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
                .collect();
            merged.extend(defaults);
            merged
        }
        "banana2" => {
            let take = |key: &str, fallback: &str| -> Value {
                defaults
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| Value::String(fallback.to_string()))
            };
            let mut merged = Map::new();
            merged.insert("aspect_ratio".to_string(), take("aspect_ratio", "16:9"));
            merged.insert("image_size".to_string(), take("image_size", "4K"));
            merged.insert("thinking_level".to_string(), take("thinking_level", "high"));
            merged.insert("mime_type".to_string(), take("mime_type", "image/png"));
            merged
        }
        _ => defaults,
    }
}

/// 并发上限内保序执行，对应 `_run_in_ordered_batches`。
///
/// `buffered` 与 Python 的分批 gather 结果完全一致（输出顺序 = 输入顺序，
/// 在飞请求不超过 concurrency），但不必等整批排空：慢的一页不会拖住下一批起跑。
async fn run_ordered<T, F>(tasks: Vec<F>, concurrency: usize) -> AppResult<Vec<T>>
where
    F: Future<Output = AppResult<T>>,
{
    futures::stream::iter(tasks)
        .buffered(concurrency.max(1))
        .try_collect()
        .await
}

/// 逐页规划所需的共享上下文，对应 `_plan_single_ppt_page` 的一串关键字参数。
struct PagePlanContext<'a> {
    profile: &'a ModelProfile,
    system_prompt: &'a str,
    page_assets: &'a [PromptAsset],
    template_analysis: &'a Value,
    compact_template_analysis: &'a str,
    compact_material_context: &'a str,
    compact_visual_asset_prompt: &'a str,
    deck_outline: &'a Value,
    custom_prompt: &'a str,
    timeout_seconds: u64,
    proxy_url: Option<&'a str>,
}

impl PagePlanContext<'_> {
    async fn plan(&self, page_brief: &Value, stage: StageSink<'_>) -> AppResult<Value> {
        let page_number = page_brief["page"].as_i64().unwrap_or(0);
        stage(
            &format!("ppt_page_prompt_{page_number}"),
            &format!("拼接第 {page_number} 页规划 prompt"),
        );
        let page_prompt = compose_prompt(
            self.page_assets,
            &[
                (
                    "Task Mode",
                    "Single-page worker mode. Return exactly one page object only. \
                     Do not plan or output other pages."
                        .to_string(),
                ),
                ("Template Analysis", self.compact_template_analysis.to_string()),
                ("Deck Outline", serde_json::to_string(self.deck_outline)?),
                ("Current Page Brief", serde_json::to_string_pretty(page_brief)?),
                (
                    "Adjacent Page Context",
                    serde_json::to_string_pretty(&adjacent_page_context(
                        self.deck_outline,
                        page_number,
                    ))?,
                ),
                ("Material", self.compact_material_context.to_string()),
                (
                    "Visual Asset Search Context",
                    self.compact_visual_asset_prompt.to_string(),
                ),
                ("Custom Prompt", self.custom_prompt.to_string()),
                (
                    "Output Contract",
                    contract::ppt_single_page(page_number.max(0) as usize),
                ),
            ],
        );

        stage(
            &format!("ppt_page_plan_{page_number}"),
            &format!("规划第 {page_number} 页内容"),
        );
        let no_images: Vec<ImageInput> = Vec::new();
        let page_text = DesignClient::generate(
            self.profile,
            self.system_prompt,
            &page_prompt,
            &no_images,
            Some(self.timeout_seconds),
            self.proxy_url,
        )
        .await?;

        let call = DesignCall {
            profile: self.profile,
            system_prompt: self.system_prompt,
            user_prompt: &page_prompt,
            images: &no_images,
            timeout_seconds: Some(self.timeout_seconds),
            proxy_url: self.proxy_url,
        };
        let expected = page_number.max(0) as usize;
        let validated = parse_validate_or_fill(&call, &page_text, |value| {
            validate_ppt_single_page(value, expected, Some(self.template_analysis))
        })
        .await?;
        Ok(validated.value)
    }
}

pub struct SlideRun<'a> {
    pub prompts: &'a PromptStore,
    pub config: &'a AppConfig,
    pub design_profile: &'a ModelProfile,
    pub implement_profile: &'a ModelProfile,
    pub search_profile: &'a ModelProfile,
    pub template_image: ImageInput,
    pub material_context: String,
}

impl SlideRun<'_> {
    /// 返回逐页图片的 base64，顺序即页序。
    ///
    /// 前三个阶段（`ppt_validate` / `ppt_template` / `ppt_material`）由
    /// `execute.rs` 在解析资源时上报，本方法从 `ppt_visual_assets` 接手。
    pub async fn run(
        &self,
        payload: &PptSlidePayload,
        stage: StageSink<'_>,
    ) -> AppResult<Vec<String>> {
        let proxy = self.config.proxy_url.as_deref();
        let page_count = payload.page_count;
        let custom_prompt = payload.custom_prompt_text();
        let page_plan_concurrency =
            resolve_ppt_concurrency(self.config.ppt_page_plan_concurrency, page_count);
        let image_concurrency =
            resolve_ppt_concurrency(self.config.ppt_image_concurrency, page_count);
        let design_timeout =
            (self.design_profile.timeout_seconds.max(0) as u64).max(PPT_DESIGN_TIMEOUT_SECONDS);

        if self.material_context.trim().is_empty() {
            return Err(AppError::new(
                "invalid_payload",
                "PPT slide requires material text or material files",
            ));
        }

        // —— 视觉素材检索：给逐页规划提供「这东西长什么样」的文字证据 ——
        stage("ppt_visual_assets", "检索产品/工具视觉素材线索");
        let visual_asset_context =
            build_visual_asset_context(&self.material_context, self.search_profile, proxy).await;
        let visual_asset_prompt = visual_asset_context_text(&visual_asset_context);
        let ppt_output = ppt_output_defaults(self.implement_profile);

        // —— 母版分析：唯一一次把 template 图喂给模型 ——
        let analyzer_assets = self.prompts.load_all(&[
            "global/system.md",
            "modes/ppt_slide/analyzer.md",
            "modes/ppt_slide/master_rules.md",
        ])?;
        let analyzer_prompt = compose_prompt(
            &analyzer_assets,
            &[("Output Contract", contract::template_analysis().to_string())],
        );
        let template_images = vec![self.template_image.clone()];

        stage("ppt_analyze", "调用 design model 分析 template");
        let analysis_text = DesignClient::generate(
            self.design_profile,
            &analyzer_assets[0].content,
            &analyzer_prompt,
            &template_images,
            Some(design_timeout),
            proxy,
        )
        .await?;

        stage("ppt_parse_template", "解析 template 分析结果");
        let analyzer_call = DesignCall {
            profile: self.design_profile,
            system_prompt: &analyzer_assets[0].content,
            user_prompt: &analyzer_prompt,
            images: &template_images,
            timeout_seconds: Some(design_timeout),
            proxy_url: proxy,
        };
        let template_analysis =
            parse_validate_or_fill(&analyzer_call, &analysis_text, validate_template_analysis)
                .await?
                .parsed;

        // —— deck outline：一次性裁定整套叙事，逐页 worker 只填自己那页 ——
        let page_assets = self.prompts.load_all(&[
            "global/system.md",
            "modes/ppt_slide/design.md",
            "styles/academic_ppt.md",
        ])?;
        let compact_template_analysis = compact_template_analysis(&template_analysis);
        let compact_material_context =
            truncate_text(&self.material_context, PPT_PLAN_MATERIAL_LIMIT);
        let compact_visual_asset_prompt =
            truncate_text(&visual_asset_prompt, PPT_PLAN_VISUAL_CONTEXT_LIMIT);

        stage("ppt_outline_prompt", "拼接整套大纲规划 prompt");
        let outline_prompt = compose_prompt(
            &page_assets,
            &[
                (
                    "Task Mode",
                    "Deck outline mode. Plan only the deck narrative and lightweight page briefs. \
                     Do not write page-level implement_prompt."
                        .to_string(),
                ),
                ("Template Analysis", compact_template_analysis.clone()),
                ("Material", compact_material_context.clone()),
                (
                    "Visual Asset Search Context",
                    compact_visual_asset_prompt.clone(),
                ),
                ("Page Count", page_count.to_string()),
                ("Custom Prompt", custom_prompt.clone()),
                ("Output Contract", contract::ppt_outline(page_count)),
            ],
        );

        stage("ppt_outline", "调用 design model 规划整套大纲");
        let no_images: Vec<ImageInput> = Vec::new();
        let outline_text = DesignClient::generate(
            self.design_profile,
            &page_assets[0].content,
            &outline_prompt,
            &no_images,
            Some(design_timeout),
            proxy,
        )
        .await?;

        stage("ppt_parse_outline", "解析并校验整套大纲");
        let outline_call = DesignCall {
            profile: self.design_profile,
            system_prompt: &page_assets[0].content,
            user_prompt: &outline_prompt,
            images: &no_images,
            timeout_seconds: Some(design_timeout),
            proxy_url: proxy,
        };
        let deck_outline = parse_validate_or_fill(&outline_call, &outline_text, |value| {
            validate_ppt_outline(value, page_count)
        })
        .await?
        .value;
        let page_briefs = deck_outline["page_briefs"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        // —— 逐页规划（并发）——
        stage(
            "ppt_page_plan_queue",
            &format!("按并发 {page_plan_concurrency} 排队规划 {page_count} 页"),
        );
        let plan_context = PagePlanContext {
            profile: self.design_profile,
            system_prompt: &page_assets[0].content,
            page_assets: &page_assets,
            template_analysis: &template_analysis,
            compact_template_analysis: &compact_template_analysis,
            compact_material_context: &compact_material_context,
            compact_visual_asset_prompt: &compact_visual_asset_prompt,
            deck_outline: &deck_outline,
            custom_prompt: &custom_prompt,
            timeout_seconds: design_timeout,
            proxy_url: proxy,
        };
        let planned = run_ordered(
            page_briefs
                .iter()
                .map(|brief| plan_context.plan(brief, stage))
                .collect(),
            page_plan_concurrency,
        )
        .await?;

        // —— 合并校验后再注入 prefix，顺序不可换 ——
        stage("ppt_merge_pages", "合并并校验页面规划");
        let pages_json = json!({ "pages": planned });
        let pages = validate_ppt_pages(&pages_json, page_count, Some(&template_analysis))?;
        let pages = apply_master_prompt_prefix(&pages, &template_analysis)?;

        // —— 逐页出图（并发）——
        stage(
            "ppt_implement_queue",
            &format!("按并发 {image_concurrency} 排队生成图片"),
        );
        run_ordered(
            pages
                .iter()
                .map(|page| self.implement_page(page, &ppt_output, proxy, stage))
                .collect(),
            image_concurrency,
        )
        .await
    }

    async fn implement_page(
        &self,
        page: &Value,
        ppt_output: &Map<String, Value>,
        proxy: Option<&str>,
        stage: StageSink<'_>,
    ) -> AppResult<String> {
        let page_number = page["page"].as_i64().unwrap_or(0);
        let prompt = page["implement_prompt"].as_str().unwrap_or_default();
        stage(
            &format!("ppt_implement_{page_number}"),
            &format!("生成第 {page_number} 页图片"),
        );
        ImplementClient::generate(self.implement_profile, prompt, &[], ppt_output, proxy).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(protocol: &str, defaults: &[(&str, &str)]) -> ModelProfile {
        let mut output_defaults = Map::new();
        for (key, value) in defaults {
            output_defaults.insert((*key).to_string(), Value::String((*value).to_string()));
        }
        ModelProfile {
            id: "test".to_string(),
            role: "implement".to_string(),
            name: "test".to_string(),
            protocol: protocol.to_string(),
            base_url: "https://example.com".to_string(),
            model: "test-model".to_string(),
            api_key: None,
            api_version: None,
            headers: Map::new(),
            timeout_seconds: 600,
            max_retries: 1,
            output_defaults,
            has_api_key: None,
            api_key_hint: None,
        }
    }

    /// 未配置时按页数全开；配置值不会超过页数，也不会低于 1。
    #[test]
    fn concurrency_is_clamped_to_page_count() {
        assert_eq!(resolve_ppt_concurrency(None, 5), 5);
        assert_eq!(resolve_ppt_concurrency(Some(3), 5), 3);
        assert_eq!(resolve_ppt_concurrency(Some(9), 5), 5);
        assert_eq!(resolve_ppt_concurrency(Some(0), 5), 1);
        assert_eq!(resolve_ppt_concurrency(Some(-2), 5), 1);
        assert_eq!(resolve_ppt_concurrency(None, 0), 1);
    }

    /// 截断按字符计，中文资料不能按字节切（会切出半个字）。
    #[test]
    fn truncate_counts_characters() {
        let text = "科研图".repeat(10);
        let truncated = truncate_text(&text, 12);
        assert!(truncated.starts_with(&"科研图".repeat(4)));
        assert!(truncated.contains("[Truncated from 30 chars to 12 chars"));
        assert_eq!(truncate_text("short", 12), "short");
    }

    /// image2 用兜底补齐，profile 已配置的字段优先。
    #[test]
    fn image2_output_merges_over_fallback() {
        let defaults = ppt_output_defaults(&profile("image2", &[("response_format", "url")]));
        assert_eq!(defaults["response_format"], json!("url"));
        assert_eq!(defaults["size"], json!("1200x675"));
        assert_eq!(defaults["output_format"], json!("png"));
    }

    /// banana2 只认这四个字段，image2 的 size/quality 不能漏过去。
    #[test]
    fn banana2_output_is_restricted_to_known_fields() {
        let defaults = ppt_output_defaults(&profile(
            "banna2",
            &[("image_size", "2K"), ("size", "1200x675")],
        ));
        assert_eq!(defaults["image_size"], json!("2K"));
        assert_eq!(defaults["aspect_ratio"], json!("16:9"));
        assert!(!defaults.contains_key("size"));
        assert!(!defaults.contains_key("quality"));
    }

    /// 空值字段要丢掉，否则会把 `size=""` 发给上游。
    #[test]
    fn empty_output_values_are_dropped() {
        let defaults = ppt_output_defaults(&profile("openai_chat", &[("size", ""), ("quality", "auto")]));
        assert!(!defaults.contains_key("size"));
        assert_eq!(defaults["quality"], json!("auto"));
    }

    #[test]
    fn material_context_marks_truncated_files() {
        let long_text = "数".repeat(MATERIAL_TEXT_LIMIT + 40);
        let asset = MaterialAsset::new(
            "notes.md".to_string(),
            "text/plain".to_string(),
            2048,
            "markdown".to_string(),
            &long_text,
        );
        assert!(asset.truncated);
        assert_eq!(asset.text_length, MATERIAL_TEXT_LIMIT + 40);

        let context = compose_material_context("  用户输入  ", &[asset]);
        assert!(context.starts_with("User text material:\n用户输入"));
        assert!(context.contains("\n\n---\n\n"));
        assert!(context.contains("parser=markdown"));
        assert!(context.contains(&format!("excerpt={MATERIAL_TEXT_LIMIT}/{} chars", MATERIAL_TEXT_LIMIT + 40)));
    }

    /// 无可提取文本时给出说明，而不是留空段落。
    #[test]
    fn material_context_falls_back_for_empty_files() {
        let asset = MaterialAsset::new(
            "scan.pdf".to_string(),
            "application/pdf".to_string(),
            10,
            "pdf".to_string(),
            "",
        );
        let context = compose_material_context("", &[asset]);
        assert!(context.contains("No extractable text available"));
        assert!(!context.contains("User text material"));
    }

    #[test]
    fn adjacent_context_covers_deck_edges() {
        let outline = json!({"page_briefs": [
            {"page": 1, "title": "封面"},
            {"page": 2, "title": "方法"},
            {"page": 3, "title": "结论"}
        ]});
        let first = adjacent_page_context(&outline, 1);
        assert!(first["previous"].is_null());
        assert_eq!(first["next"]["title"], json!("方法"));

        let middle = adjacent_page_context(&outline, 2);
        assert_eq!(middle["previous"]["title"], json!("封面"));
        assert_eq!(middle["next"]["title"], json!("结论"));

        let last = adjacent_page_context(&outline, 3);
        assert_eq!(last["previous"]["title"], json!("方法"));
        assert!(last["next"].is_null());
    }

    /// 压缩保留全部样式字段（prefix 的事实来源），只丢 worker 用不上的长文。
    /// 对齐 Python `test_ppt_page_planner_context_is_compacted`。
    #[test]
    fn compact_analysis_keeps_master_style_fields() {
        let analysis = json!({"template_analysis": {
            "master_style_summary": "红灰学术母版",
            "master_style_spec": {
                "canvas": {"aspect_ratio": "16:9"},
                "palette": {"primary": "#B40000"},
                "typography": {"title": "32px"},
                "page_layout_rules": "worker 用不上的长文"
            },
            "global_constraints": ["keep title band"],
            "immutable_elements": ["title region"],
            "page_layout_rules": "Body varies inside safe margins."
        }});
        let compact = compact_template_analysis(&analysis);
        for token in ["canvas", "palette", "typography", "forbidden_deviations", "红灰学术母版"] {
            assert!(compact.contains(token), "compact analysis missing {token}");
        }
        // master_style_spec 内层的 page_layout_rules 不在保留名单里
        assert!(!compact.contains("worker 用不上的长文"));
        assert!(compact.contains("Body varies inside safe margins."));

        let full = serde_json::to_string_pretty(&analysis).unwrap();
        assert!(compact.chars().count() < full.chars().count());
        assert!(compact.chars().count() <= PPT_PLAN_TEMPLATE_LIMIT + 120);
    }

    #[test]
    fn payload_validation_rejects_out_of_range_pages() {
        let base = PptSlidePayload {
            template_asset_id: "asset-1".to_string(),
            template_id: None,
            material_text: "内容".to_string(),
            material_asset_ids: Vec::new(),
            page_count: 3,
            custom_prompt: None,
        };
        assert!(validate_payload(&base).is_ok());
        assert_eq!(base.custom_prompt_text(), "None");

        let mut too_many = base.clone();
        too_many.page_count = MAX_PAGE_COUNT + 1;
        assert!(validate_payload(&too_many).is_err());

        let mut zero = base.clone();
        zero.page_count = 0;
        assert!(validate_payload(&zero).is_err());

        let mut no_template = base.clone();
        no_template.template_asset_id = "   ".to_string();
        assert!(validate_payload(&no_template).is_err());
    }

    /// 桌面版从模板库选母版：payload 里没有 asset，只有 template_id。
    /// 这条通不过就意味着桌面版的幻灯片一律报「缺少母版」。
    #[test]
    fn payload_validation_accepts_master_from_template_library() {
        let from_library = PptSlidePayload {
            template_asset_id: String::new(),
            template_id: Some("tpl-1".to_string()),
            material_text: "内容".to_string(),
            material_asset_ids: Vec::new(),
            page_count: 2,
            custom_prompt: None,
        };
        assert!(validate_payload(&from_library).is_ok());
        assert_eq!(from_library.template_ref(), Some("tpl-1"));
        assert_eq!(from_library.asset_ref(), None);

        // 空白 template_id 不算来源，否则前端传空串会被当成有效母版，
        // 一路走到管道里才炸。
        let mut blank = from_library.clone();
        blank.template_id = Some("   ".to_string());
        assert!(validate_payload(&blank).is_err());
    }

    /// 对齐 Python `test_ordered_batches_queue_after_concurrency_limit`：
    /// 既要保序（否则 slide_1.png 会存成别页的图），也要真的卡住并发上限
    /// （否则 20 页会同时打满上游，配置项形同虚设）。
    #[tokio::test]
    async fn ordered_run_preserves_order_and_caps_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = (0..6)
            .map(|index| {
                let running = Arc::clone(&running);
                let peak = Arc::clone(&peak);
                async move {
                    let current = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    // 后面的任务睡得更久：乱序完成也必须按输入顺序返回
                    tokio::time::sleep(std::time::Duration::from_millis(20 - index * 3)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    Ok(index)
                }
            })
            .collect();

        let results = run_ordered(tasks, 2).await.unwrap();
        assert_eq!(results, vec![0, 1, 2, 3, 4, 5]);
        // 断言「恰好 2」而非「不超过 2」：串行执行也满足 <=2，会让用例白跑
        assert_eq!(peak.load(Ordering::SeqCst), 2, "并发上限未被正确执行");
    }

    #[tokio::test]
    async fn ordered_run_propagates_first_error() {
        let tasks: Vec<_> = (0..4)
            .map(|index| async move {
                if index == 2 {
                    return Err(AppError::new("boom", "page 3 failed"));
                }
                Ok(index)
            })
            .collect();
        let error = run_ordered(tasks, 2).await.unwrap_err();
        assert_eq!(error.message, "page 3 failed");
    }
}
