use regex::Regex;
use serde_json::Value;

use crate::error::AppError;

#[derive(Debug)]
pub enum ValidationError {
    Schema(String),
    Value(String),
}

impl ValidationError {
    pub fn message(&self) -> &str {
        match self {
            ValidationError::Schema(message) | ValidationError::Value(message) => message,
        }
    }

    pub fn is_schema(&self) -> bool {
        matches!(self, ValidationError::Schema(_))
    }
}

impl From<ValidationError> for AppError {
    fn from(error: ValidationError) -> Self {
        let code = if error.is_schema() { "design_schema" } else { "design_value" };
        AppError::new(code, error.message())
    }
}

type Checked<T> = Result<T, ValidationError>;

fn schema<T>(message: impl Into<String>) -> Checked<T> {
    Err(ValidationError::Schema(message.into()))
}

fn value_err<T>(message: impl Into<String>) -> Checked<T> {
    Err(ValidationError::Value(message.into()))
}

pub const FIGURE_COMMON_FIELDS: [&str; 8] = [
    "visual_type",
    "layout_constraints",
    "semantic_constraints",
    "visual_constraints",
    "forbidden_errors",
    "quality_rubric",
    "visible_text",
    "implement_prompt",
];

pub const DIAGRAM_SPEC_FIELDS: [&str; 7] = [
    "modules",
    "entities",
    "connections",
    "flow_direction",
    "grouping_hierarchy",
    "arrow_routing",
    "label_strategy",
];

pub const PLOT_SPEC_FIELDS: [&str; 8] = [
    "chart_type",
    "data_fields",
    "axes",
    "units",
    "series_or_categories",
    "legend",
    "statistical_annotations",
    "data_integrity_rules",
];

pub const FIGURE_ASPECT_RATIOS: [&str; 7] =
    ["inherit", "16:9", "4:3", "1:1", "3:2", "2:3", "9:16"];

const FORBIDDEN_TEMPLATE_COPY_PHRASES: [&str; 6] = [
    "copy the template",
    "duplicate the template",
    "trace the template",
    "use the template as a base image",
    "edit the template image",
    "background edit",
];

pub fn is_filled(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
        Some(_) => true,
    }
}

pub fn require_fields(data: &Value, fields: &[&str], label: &str) -> Checked<()> {
    let missing: Vec<&str> = fields
        .iter()
        .copied()
        .filter(|field| !is_filled(data.get(field)))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        schema(format!("{label} missing required fields: {}", missing.join(", ")))
    }
}

pub fn figure_object(design: &Value) -> &Value {
    if design.get("figure").is_some_and(Value::is_object) {
        &design["figure"]
    } else {
        design
    }
}

pub fn implement_prompt(design: &Value) -> Checked<String> {
    let figure = figure_object(design);
    if !figure.is_object() {
        return value_err("Design model response must be a JSON object");
    }
    match figure.get("implement_prompt").and_then(Value::as_str) {
        Some(prompt) => Ok(prompt.to_string()),
        None => schema("Design model response missing implement_prompt"),
    }
}

fn keyword_groups_present(text: &str, groups: &[(&str, &[&str])]) -> usize {
    let lowered = text.to_lowercase();
    groups
        .iter()
        .filter(|(_, keywords)| {
            keywords
                .iter()
                .any(|keyword| lowered.contains(&keyword.to_lowercase()))
        })
        .count()
}

pub fn validate_no_copy_request(prompt: &str) -> Checked<()> {
    let lowered = prompt.to_lowercase();
    let negations = [
        "do not ", "don't ", "dont ", "never ", " no ", "not ", "avoid ", "must not ", "cannot ",
        "can't ", "without ", "rather than ", "instead of ", "forbid", "forbidden", "prohibit",
        "refrain", "禁止", "不要", "不得", "不能", "不可", "严禁", "避免", "勿",
    ];
    let allow_context = [
        "reference only", "as reference", "style only", "layout only", "not as base", "not a base",
        "not base image", "inspiration only", "few-shot", "few shot",
    ];
    let splitter = Regex::new(r"(?:[\.\!\?\n；;。！？])").expect("valid regex");

    for sentence in splitter.split(&lowered) {
        let sentence = sentence.trim();
        if sentence.is_empty() {
            continue;
        }
        for phrase in FORBIDDEN_TEMPLATE_COPY_PHRASES {
            let mut start = 0;
            while let Some(index) = sentence[start..].find(phrase) {
                let absolute = start + index;
                let before = format!(" {}", &sentence[..absolute]);
                let negated = negations.iter().any(|neg| before.contains(neg))
                    || negations
                        .iter()
                        .any(|neg| sentence.starts_with(neg.trim()));
                let contextual_allow = allow_context.iter().any(|token| sentence.contains(token))
                    && negations.iter().any(|neg| sentence.contains(neg));
                if !negated && !contextual_allow {
                    return schema(format!(
                        "Implement prompt requests direct template copying or editing \
                         (matched {phrase:?}). Rewrite implement_prompt so templates are \
                         style/layout reference only; never copy, trace, edit, or use as base image."
                    ));
                }
                start = absolute + phrase.len();
            }
        }
    }
    Ok(())
}

pub fn normalize_inventory_item(value: &Value) -> String {
    let text = value.as_str().unwrap_or_default();
    let spaces = Regex::new(r"\s+").expect("valid regex");
    spaces.replace_all(text, " ").trim().to_string()
}

fn strip_ws(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

pub fn item_grounded_in_source(item: &str, source: &str) -> bool {
    if item.is_empty() {
        return false;
    }
    if source.contains(item) || source.to_lowercase().contains(&item.to_lowercase()) {
        return true;
    }
    if strip_ws(&source.to_lowercase()).contains(&strip_ws(&item.to_lowercase())) {
        return true;
    }
    let cn = Regex::new(r"[\x{4e00}-\x{9fff}]{4,}").expect("valid regex");
    if cn.find_iter(item).any(|m| source.contains(m.as_str())) {
        return true;
    }
    let en = Regex::new(r"[A-Za-z][A-Za-z0-9\-+_/]{1,}").expect("valid regex");
    let lowered_source = source.to_lowercase();
    let matched = en
        .find_iter(item)
        .filter(|m| m.as_str().len() >= 2)
        .any(|m| lowered_source.contains(&m.as_str().to_lowercase()));
    matched
}

pub fn item_present_in_output(item: &str, haystack: &str) -> bool {
    if item.is_empty() {
        return false;
    }
    if haystack.contains(item) || haystack.to_lowercase().contains(&item.to_lowercase()) {
        return true;
    }
    if strip_ws(&haystack.to_lowercase()).contains(&strip_ws(&item.to_lowercase())) {
        return true;
    }
    let cn = Regex::new(r"[\x{4e00}-\x{9fff}]{3,8}").expect("valid regex");
    if cn.find_iter(item).any(|m| haystack.contains(m.as_str())) {
        return true;
    }
    let en = Regex::new(r"[A-Za-z][A-Za-z0-9\-+_/]{2,}").expect("valid regex");
    let lowered_haystack = haystack.to_lowercase();
    let matched = en
        .find_iter(item)
        .any(|m| lowered_haystack.contains(&m.as_str().to_lowercase()));
    matched
}

pub fn validate_design_content_coverage(design: &Value, title: &str, section: &str) -> Checked<()> {
    let figure = figure_object(design);
    if !figure.is_object() {
        return Ok(());
    }
    let prompt = implement_prompt(design)?;
    let source = format!("{title}\n{section}").trim().to_string();

    let mut inventory: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    if let Some(items) = figure.get("content_inventory").and_then(Value::as_array) {
        for item in items {
            if !is_filled(Some(item)) {
                continue;
            }
            let normalized = normalize_inventory_item(item);
            let key = normalized.to_lowercase();
            if normalized.is_empty() || seen.contains(&key) {
                continue;
            }
            seen.push(key);
            inventory.push(normalized);
        }
    }

    let modules_text = figure["diagram_spec"]["modules"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| is_filled(Some(item)))
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let haystack = format!("{prompt}\n{modules_text}\n{}\n{title}", inventory.join(" "));

    if source.chars().count() < 180 {
        if prompt.trim().chars().count() < 360 {
            return schema(
                "implement_prompt too short; expand grounded operational detail from the user section",
            );
        }
        return Ok(());
    }

    if inventory.len() < 8 {
        return schema(
            "content_inventory too small for a rich method section; extract at least 8 short grounded \
             operation/component labels from the user text (e.g. OCR, BM25, 双塔编码, 交叉重排)",
        );
    }

    let ungrounded: Vec<&String> = inventory
        .iter()
        .filter(|item| !item_grounded_in_source(item, &source))
        .collect();
    if ungrounded.len() > std::cmp::max(2, inventory.len() / 3) {
        let listed: Vec<&str> = ungrounded.iter().take(10).map(|item| item.as_str()).collect();
        return schema(format!(
            "content_inventory contains too many items not grounded in the user section. \
             Fix or remove: {}",
            listed.join(", ")
        ));
    }

    let missing: Vec<&String> = inventory
        .iter()
        .filter(|item| !item_present_in_output(item, &haystack))
        .collect();
    let covered = inventory.len() - missing.len();
    let coverage = covered as f64 / inventory.len().max(1) as f64;
    if coverage < 0.55 {
        let listed: Vec<&str> = missing.iter().take(12).map(|item| item.as_str()).collect();
        return schema(format!(
            "implement_prompt/modules miss too many content_inventory items (coverage {:.0}%). \
             Re-include short labels such as: {}",
            coverage * 100.0,
            listed.join(", ")
        ));
    }

    let lowered = prompt.to_lowercase();
    let detail_ok = ["module detail", "模块细节", "per stage", "各阶段", "子模块", "leaf module"]
        .iter()
        .any(|marker| lowered.contains(marker) || prompt.contains(marker));
    if !detail_ok && (!prompt.contains("阶段") || prompt.chars().count() < 450) {
        return schema(
            "implement_prompt must include a MODULE DETAIL / 模块细节 section listing leaf steps per stage",
        );
    }
    Ok(())
}

pub fn validate_paper_design(design: &Value) -> Checked<()> {
    let figure = figure_object(design);
    if !figure.is_object() {
        return schema("Design model response must contain a figure object");
    }
    require_fields(figure, &FIGURE_COMMON_FIELDS, "Paper figure")?;
    let prompt = implement_prompt(design)?;
    if prompt.trim().chars().count() < 360 {
        return schema(
            "Paper figure implement_prompt is too short; keep leaf-level operations from the user section",
        );
    }
    if let Some(ratio) = figure.get("aspect_ratio").and_then(Value::as_str) {
        if !ratio.is_empty() && !FIGURE_ASPECT_RATIOS.contains(&ratio) {
            return value_err(format!("Invalid paper figure aspect_ratio: {ratio}"));
        }
    }
    match figure.get("visible_text") {
        Some(Value::Array(items))
            if items
                .iter()
                .all(|item| item.as_str().is_some_and(|text| text.trim().chars().count() <= 80)) => {}
        _ => return value_err("Paper figure visible_text must be short label strings"),
    }
    validate_no_copy_request(&prompt)?;

    let groups: [(&str, &[&str]); 5] = [
        ("publication", &["publication", "academic", "paper", "论文", "出版"]),
        ("faithfulness", &["faithful", "faithfulness", "grounded", "no hallucination", "忠实", "不虚构"]),
        ("conciseness", &["concise", "abstraction", "short label", "简洁", "抽象", "短标签"]),
        ("readability", &["readable", "legible", "contrast", "可读", "对比"]),
        ("template_boundary", &["template", "reference", "not copy", "参考", "模板"]),
    ];
    if keyword_groups_present(&prompt, &groups) < 4 {
        return schema(
            "Paper figure implement_prompt must cover publication quality, faithfulness, conciseness, readability, and template boundary",
        );
    }

    let visual_type = figure
        .get("visual_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();

    if ["diagram", "workflow", "comparison", "mechanism"].contains(&visual_type.as_str()) {
        let spec = figure.get("diagram_spec");
        if !spec.is_some_and(Value::is_object) {
            return schema("Diagram figure missing diagram_spec");
        }
        let spec = spec.expect("checked above");
        require_fields(spec, &DIAGRAM_SPEC_FIELDS, "Diagram spec")?;
        let module_count = spec["modules"]
            .as_array()
            .map(|items| items.iter().filter(|item| is_filled(Some(item))).count())
            .unwrap_or(0);
        if module_count < 8 {
            return schema(
                "Diagram modules too few; expand grounded leaf steps from the user section \
                 (need at least 8 named modules for multi-step methods)",
            );
        }
        let connections = spec["connections"].as_array().cloned().unwrap_or_default();
        if connections.len() < 7 {
            return schema(
                "Diagram connections too few; need at least 7 grounded edges for multi-stage flow",
            );
        }
        for (index, connection) in connections.iter().enumerate() {
            if !connection.is_object() {
                return schema(format!("Diagram connection {} must be an object", index + 1));
            }
            require_fields(
                connection,
                &["source", "target", "meaning"],
                &format!("Diagram connection {}", index + 1),
            )?;
        }
        let grouping = spec["grouping_hierarchy"].as_str().unwrap_or_default().trim();
        if grouping.chars().count() < 12 {
            return schema("Diagram grouping_hierarchy must describe multi-stage/lane structure");
        }
        let markers = [
            "stage", "pipeline", "multi", "branch", "group", "layer", "阶段", "支路", "分层",
            "模块", "流程", "module detail", "模块细节",
        ];
        let lowered = prompt.to_lowercase();
        if !markers.iter().any(|m| lowered.contains(m) || prompt.contains(m)) {
            return schema(
                "implement_prompt must describe multi-stage/hierarchical flowchart layout \
                 with leaf module details from the user section",
            );
        }
        return Ok(());
    }

    if ["plot", "chart"].contains(&visual_type.as_str()) {
        let spec = figure.get("plot_spec");
        if !spec.is_some_and(Value::is_object) {
            return schema("Plot/chart figure missing plot_spec");
        }
        let spec = spec.expect("checked above");
        require_fields(spec, &PLOT_SPEC_FIELDS, "Plot spec")?;
        let axes = &spec["axes"];
        if !axes.is_object() || !is_filled(axes.get("x")) || !is_filled(axes.get("y")) {
            return schema("Plot spec axes must include x and y definitions");
        }
        let integrity = spec["data_integrity_rules"].as_str().unwrap_or_default();
        if integrity.chars().count() < 40 {
            return schema(
                "Plot spec data_integrity_rules must explicitly describe anti-distortion constraints",
            );
        }
        return Ok(());
    }

    schema("Paper figure visual_type must be diagram, workflow, comparison, mechanism, plot, or chart")
}

pub fn validate_structure_plan(data: &Value) -> Checked<Value> {
    let plan = if data.get("structure_plan").is_some_and(Value::is_object) {
        &data["structure_plan"]
    } else {
        data
    };
    if !plan.is_object() {
        return schema("Structure plan must be a JSON object under structure_plan");
    }
    require_fields(
        plan,
        &[
            "visual_family",
            "primary_flow",
            "lanes_or_stages",
            "module_slots",
            "connection_slots",
            "grouping",
            "information_density",
            "layout_skeleton",
        ],
        "Structure plan",
    )?;
    if plan["lanes_or_stages"].as_array().map(Vec::len).unwrap_or(0) < 2 {
        return schema("Structure plan lanes_or_stages needs at least 2 stages/lanes");
    }
    if plan["module_slots"].as_array().map(Vec::len).unwrap_or(0) < 6 {
        return schema(
            "Structure plan module_slots needs at least 6 abstract slots for template-like density",
        );
    }
    if plan["connection_slots"].as_array().map(Vec::len).unwrap_or(0) < 5 {
        return schema("Structure plan connection_slots needs at least 5 abstract edges");
    }
    if plan["layout_skeleton"].as_str().unwrap_or_default().trim().chars().count() < 80 {
        return schema("Structure plan layout_skeleton is too short");
    }
    Ok(plan.clone())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    fn long_figure_prompt(extra: &str) -> String {
        format!(
            "Create a publication quality academic paper figure with faithful grounded content and no hallucination. \
             Use multi-stage pipeline layout with grouped modules, short labels, readable legible typography, high contrast, and a clean white background. \
             Use selected templates as reference style density and layout inspiration only, never as editable base imagery. \
             Avoid scope drift, gibberish, fake formulas, unreadable arrows, visual noise, and unsupported claims. \
             Keep all modules and data encodings aligned with the user-provided section. {extra}"
        )
    }

    pub(crate) fn valid_diagram_design() -> Value {
        json!({
            "figure": {
                "title": "Pipeline overview",
                "visual_type": "diagram",
                "aspect_ratio": "16:9",
                "template_usage": "balanced",
                "layout_constraints": ["16:9 compact rectangular canvas", "left-to-right multi-stage module rhythm"],
                "semantic_constraints": ["all modules grounded in user method", "preserve flow direction"],
                "visual_constraints": ["high contrast", "publication friendly white background"],
                "forbidden_errors": ["hallucination", "reversed flow", "scope violation", "text overload"],
                "quality_rubric": {
                    "faithfulness": "ground every block in source text",
                    "conciseness": "use visual abstraction and keywords",
                    "readability": "legible labels and clear arrows",
                    "aesthetics": "restrained academic style"
                },
                "content_inventory": [
                    "原始输入", "解析", "OCR", "本地资产库", "多模态编码", "统一索引",
                    "混合检索", "交叉重排", "规划器", "推理器", "生成器", "评估反馈"
                ],
                "visible_text": ["输入", "解析", "编码", "检索", "重排", "生成"],
                "diagram_spec": {
                    "modules": ["输入", "解析", "OCR", "资产库", "编码", "索引", "检索", "重排", "规划", "推理", "生成"],
                    "entities": ["文档", "向量", "证据包"],
                    "connections": [
                        {"source": "输入", "target": "解析", "meaning": "原始资产"},
                        {"source": "解析", "target": "OCR", "meaning": "扫描增强"},
                        {"source": "OCR", "target": "资产库", "meaning": "三类资产"},
                        {"source": "资产库", "target": "编码", "meaning": "结构化块"},
                        {"source": "编码", "target": "索引", "meaning": "统一向量空间"},
                        {"source": "索引", "target": "检索", "meaning": "候选召回"},
                        {"source": "检索", "target": "重排", "meaning": "相关性过滤"},
                        {"source": "重排", "target": "规划", "meaning": "Top-K 证据"}
                    ],
                    "flow_direction": "left to right with bottom feedback branch",
                    "grouping_hierarchy": "五阶段外层 + 各阶段内部叶子模块；底部训练反馈支路",
                    "arrow_routing": "solid main path, dashed feedback",
                    "label_strategy": "short Chinese noun labels only"
                },
                "implement_prompt": long_figure_prompt(
                    "MODULE DETAIL / 模块细节: stage-by-stage leaf modules. \
                     Render a multi-stage pipeline with nested groups and solid/dashed arrows."
                )
            },
            "quality_checklist": ["faithful", "concise", "readable"]
        })
    }

    #[test]
    fn validates_diagram_contract() {
        validate_paper_design(&valid_diagram_design()).expect("valid diagram should pass");

        let mut missing_spec = valid_diagram_design();
        missing_spec["figure"].as_object_mut().unwrap().remove("diagram_spec");
        assert!(validate_paper_design(&missing_spec).unwrap_err().is_schema());

        let mut copy_request = valid_diagram_design();
        copy_request["figure"]["implement_prompt"] =
            json!(long_figure_prompt("Copy the template exactly as a base image."));
        assert!(validate_paper_design(&copy_request).unwrap_err().is_schema());
    }

    #[test]
    fn negated_copy_phrases_are_allowed() {
        let mut design = valid_diagram_design();
        design["figure"]["implement_prompt"] = json!(long_figure_prompt(
            "MODULE DETAIL / 模块细节 per stage. Do not copy the template. Never edit the template image \
             or use the template as a base image. Forbidden: copy the template, duplicate the template."
        ));
        validate_paper_design(&design).expect("negated phrasing should pass");
    }

    #[test]
    fn english_inventory_items_stay_grounded() {
        let source = "RankRAG unifies context ranking and answer generation in one instruction-tuned LLM. \
                      At inference, an external retriever first fetches a large top-N candidate set. \
                      The same LLM then selects a high-quality top-k subset. \
                      Training has two stages: Stage I supervised fine-tuning on general instruction data.";
        for raw in [
            "External retriever",
            "Top-N candidate contexts",
            "Instruction-tuned LLM (shared)",
            "High-quality top-k subset",
            "Stage I supervised fine-tuning",
        ] {
            let item = normalize_inventory_item(&json!(raw));
            assert!(item.contains(' '), "归一化不应删除空格: {item}");
            assert!(item_grounded_in_source(&item, source), "{raw} 应判定为 grounded");
        }
    }

    #[test]
    fn chinese_inventory_tolerates_inserted_spaces() {
        let item = normalize_inventory_item(&json!("混合 检索"));
        assert!(item_grounded_in_source(&item, "本文采用混合检索与交叉重排流程"));
        assert!(item_present_in_output(&item, "模块包含混合检索"));
    }

    #[test]
    fn structure_plan_enforces_minimums() {
        let thin = json!({"structure_plan": {
            "visual_family": "pipeline",
            "primary_flow": "left-to-right",
            "lanes_or_stages": [{"name": "a"}],
            "module_slots": [{"id": "m1"}],
            "connection_slots": [{"from": "m1", "to": "m2"}],
            "grouping": "cards",
            "information_density": "high",
            "layout_skeleton": "too short"
        }});
        assert!(validate_structure_plan(&thin).unwrap_err().is_schema());
    }
}
