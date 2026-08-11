use regex::Regex;
use serde_json::Value;

use crate::core::pipeline::validate::{
    is_filled, require_fields, validate_no_copy_request, ValidationError,
};

type Checked<T> = Result<T, ValidationError>;

fn schema<T>(message: impl Into<String>) -> Checked<T> {
    Err(ValidationError::Schema(message.into()))
}

fn value_err<T>(message: impl Into<String>) -> Checked<T> {
    Err(ValidationError::Value(message.into()))
}

pub const MASTER_STYLE_FIELDS: [&str; 12] = [
    "canvas", "title_region", "safe_margins", "header_footer", "divider_lines", "palette",
    "typography", "module_style", "decorative_elements", "immutable_elements",
    "page_layout_rules", "forbidden_deviations",
];

pub const PAGE_MASTER_BINDING_FIELDS: [&str; 8] = [
    "title_region", "safe_margins", "header_footer", "divider_lines", "palette", "typography",
    "module_style", "background",
];

pub const PAGE_VISUAL_PLAN_FIELDS: [&str; 3] =
    ["usage_decision", "elements", "text_visual_balance"];

pub const PAGE_VISUAL_ELEMENT_FIELDS: [&str; 7] = [
    "type", "subject", "appearance", "source_reference", "placement", "style", "size_ratio",
];

pub const PAGE_EMPHASIS_PLAN_FIELDS: [&str; 2] = ["keywords", "style_rules"];
pub const PAGE_EMPHASIS_KEYWORD_FIELDS: [&str; 3] = ["text", "style", "reason"];

const PAGE_FIELDS: [&str; 10] = [
    "page", "selected_template", "title", "slide_type", "body_layout_plan",
    "master_style_binding", "visual_element_plan", "emphasis_plan", "visible_text",
    "implement_prompt",
];

fn api_output_pattern() -> Regex {
    Regex::new(
        r"(?i)\b(?:size|quality|output_format|response_format|aspect_ratio|image_size|thinking_level|mime_type)\s*=\s*[^,.;\n]+[,.;]?\s*",
    )
    .expect("valid regex")
}

fn output_settings_sentence_pattern() -> Regex {
    Regex::new(r"(?i)output settings preserved exactly:\s*[^.\n]*(?:\.|\n)?").expect("valid regex")
}

pub fn master_style_spec(analysis: &Value) -> Checked<Value> {
    let wrapper = if analysis.get("template_analysis").is_some_and(Value::is_object) {
        &analysis["template_analysis"]
    } else {
        analysis
    };
    if !wrapper.is_object() {
        return schema("Template analysis must be a JSON object");
    }
    let master = wrapper.get("master_style_spec");
    if !master.is_some_and(Value::is_object) {
        return schema("Template analysis missing master_style_spec");
    }
    Ok(master.expect("checked above").clone())
}

pub fn validate_template_analysis(analysis: &Value) -> Checked<()> {
    let wrapper = if analysis.get("template_analysis").is_some_and(Value::is_object) {
        &analysis["template_analysis"]
    } else {
        analysis
    };
    if !wrapper.is_object() {
        return schema("Template analysis must contain template_analysis object");
    }
    let master = master_style_spec(analysis)?;
    require_fields(&master, &MASTER_STYLE_FIELDS, "PPT master_style_spec")?;
    require_fields(
        wrapper,
        &["global_constraints", "immutable_elements", "page_layout_rules"],
        "PPT template_analysis",
    )?;
    let immutable = wrapper["immutable_elements"].as_array().map(Vec::len).unwrap_or(0);
    if immutable < 3 {
        return schema(
            "PPT immutable_elements must list at least title/page marker/divider or equivalent master elements",
        );
    }
    Ok(())
}

pub fn validate_ppt_outline(outline_json: &Value, page_count: usize) -> Checked<Value> {
    let outline = if outline_json.get("deck_outline").is_some_and(Value::is_object) {
        &outline_json["deck_outline"]
    } else {
        outline_json
    };
    if !outline.is_object() {
        return schema("PPT outline response must be a JSON object");
    }
    require_fields(
        outline,
        &["deck_title", "deck_goal", "narrative_arc", "shared_prompt", "page_briefs"],
        "PPT deck_outline",
    )?;
    let Some(briefs) = outline["page_briefs"].as_array() else {
        return schema("PPT deck_outline.page_briefs must be a list");
    };
    if briefs.len() != page_count {
        return value_err(format!(
            "Deck outline returned {} page briefs, expected {page_count}",
            briefs.len()
        ));
    }
    let mut sorted = briefs.clone();
    sorted.sort_by_key(|item| item["page"].as_i64().unwrap_or(0));
    let actual: Vec<i64> = sorted.iter().map(|item| item["page"].as_i64().unwrap_or(0)).collect();
    let expected: Vec<i64> = (1..=page_count as i64).collect();
    if actual != expected {
        return value_err(format!(
            "PPT outline page briefs must be ordered 1..{page_count}; got {actual:?}"
        ));
    }
    for brief in &sorted {
        if !brief.is_object() {
            return schema("Each PPT page brief must be an object");
        }
        let label = format!("Page brief {}", brief["page"]);
        require_fields(
            brief,
            &[
                "page", "title", "role", "main_message", "content_points", "suggested_template",
                "visual_direction", "transition_from_previous", "transition_to_next",
            ],
            &label,
        )?;
        if brief["content_points"].as_array().map(Vec::is_empty).unwrap_or(true) {
            return schema(format!("{label} content_points must be a non-empty list"));
        }
    }
    let mut normalized = outline.clone();
    normalized["page_briefs"] = Value::Array(sorted);
    Ok(normalized)
}

pub fn validate_ppt_page_fields(page: &Value) -> Checked<()> {
    if !page.is_object() {
        return schema("Each PPT page must be an object");
    }
    let label = format!("Page {}", page["page"]);
    require_fields(page, &PAGE_FIELDS, &label)?;

    let binding = &page["master_style_binding"];
    if !binding.is_object() {
        return schema(format!("{label} missing master_style_binding object"));
    }
    require_fields(binding, &PAGE_MASTER_BINDING_FIELDS, &format!("{label} master_style_binding"))?;

    let visual_plan = &page["visual_element_plan"];
    if !visual_plan.is_object() {
        return schema(format!("{label} missing visual_element_plan object"));
    }
    require_fields(visual_plan, &PAGE_VISUAL_PLAN_FIELDS, &format!("{label} visual_element_plan"))?;
    let Some(elements) = visual_plan["elements"].as_array() else {
        return schema(format!("{label} visual_element_plan.elements must be a list"));
    };
    let decision = visual_plan["usage_decision"].as_str().unwrap_or_default();
    if elements.is_empty()
        && !decision.to_lowercase().contains("none")
        && !decision.contains("不用")
    {
        return schema(format!(
            "{label} visual_element_plan must list elements or explicitly justify using none"
        ));
    }
    for (index, element) in elements.iter().enumerate() {
        if !element.is_object() {
            return schema(format!("{label} visual element {} must be an object", index + 1));
        }
        require_fields(
            element,
            &PAGE_VISUAL_ELEMENT_FIELDS,
            &format!("{label} visual element {}", index + 1),
        )?;
    }

    let emphasis = &page["emphasis_plan"];
    if !emphasis.is_object() {
        return schema(format!("{label} missing emphasis_plan object"));
    }
    require_fields(emphasis, &PAGE_EMPHASIS_PLAN_FIELDS, &format!("{label} emphasis_plan"))?;
    let Some(keywords) = emphasis["keywords"].as_array() else {
        return schema(format!("{label} emphasis_plan.keywords must be a list"));
    };
    if keywords.len() > 5 {
        return schema(format!("{label} emphasis_plan may highlight at most 5 key phrases"));
    }
    for (index, keyword) in keywords.iter().enumerate() {
        if !keyword.is_object() {
            return schema(format!("{label} emphasis keyword {} must be an object", index + 1));
        }
        require_fields(
            keyword,
            &PAGE_EMPHASIS_KEYWORD_FIELDS,
            &format!("{label} emphasis keyword {}", index + 1),
        )?;
        let text = keyword["text"].as_str().unwrap_or_default().trim();
        if text.chars().count() > 24 {
            return schema(format!(
                "{label} emphasis keyword {} must be a short phrase",
                index + 1
            ));
        }
    }

    match page["visible_text"].as_array() {
        Some(items)
            if items.iter().all(|item| {
                item.as_str().is_some_and(|text| text.trim().chars().count() <= 120)
            }) => {}
        _ => return schema(format!("{label} visible_text must be short strings")),
    }

    let prompt = page["implement_prompt"].as_str().unwrap_or_default();
    if prompt.trim().chars().count() < 80 {
        return schema(format!("{label} missing usable page-specific implement_prompt"));
    }
    if api_output_pattern().is_match(prompt) || output_settings_sentence_pattern().is_match(prompt) {
        return schema(format!("{label} implement_prompt must not include API output settings"));
    }
    validate_no_copy_request(prompt)
}

pub fn validate_ppt_single_page(
    page_json: &Value,
    expected_page: usize,
    template_analysis: Option<&Value>,
) -> Checked<Value> {
    if let Some(analysis) = template_analysis {
        validate_template_analysis(analysis)?;
    }
    let page = if page_json.get("page").is_some_and(Value::is_object) {
        page_json["page"].clone()
    } else {
        match page_json["pages"].as_array() {
            Some(items) if items.len() == 1 && items[0].is_object() => items[0].clone(),
            _ => Value::Null,
        }
    };
    if !page.is_object() {
        return schema("Single-page worker response must contain one page object");
    }
    let actual = page["page"].as_i64().unwrap_or(0);
    if actual != expected_page as i64 {
        return value_err(format!(
            "Single-page worker returned page {actual}, expected {expected_page}"
        ));
    }
    validate_ppt_page_fields(&page)?;
    Ok(page)
}

pub fn validate_ppt_pages(
    pages_json: &Value,
    page_count: usize,
    template_analysis: Option<&Value>,
) -> Checked<Vec<Value>> {
    let Some(pages) = pages_json["pages"].as_array() else {
        return schema("PPT design response pages must be a list");
    };
    if pages.len() != page_count {
        return value_err(format!(
            "Design model returned {} pages, expected {page_count}",
            pages.len()
        ));
    }
    let mut sorted = pages.clone();
    sorted.sort_by_key(|item| item["page"].as_i64().unwrap_or(0));
    let actual: Vec<i64> = sorted.iter().map(|item| item["page"].as_i64().unwrap_or(0)).collect();
    let expected: Vec<i64> = (1..=page_count as i64).collect();
    if actual != expected {
        return value_err(format!("PPT pages must be ordered 1..{page_count}; got {actual:?}"));
    }
    if let Some(analysis) = template_analysis {
        validate_template_analysis(analysis)?;
    }
    for page in &sorted {
        validate_ppt_page_fields(page)?;
    }
    Ok(sorted)
}

fn stringify(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Null => "None".to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn sanitize_page_prompt(prompt: &str) -> String {
    let cleaned = output_settings_sentence_pattern().replace_all(prompt, "");
    let cleaned = api_output_pattern().replace_all(&cleaned, "");
    let collapsed = Regex::new(r"\s{2,}").expect("valid regex").replace_all(&cleaned, " ");
    Regex::new(r"\s+([,.;:])")
        .expect("valid regex")
        .replace_all(&collapsed, "$1")
        .trim()
        .to_string()
}

fn master_prefix(analysis: &Value, page: &Value) -> Checked<String> {
    let master = master_style_spec(analysis)?;
    let binding = &page["master_style_binding"];
    let pick = |key: &str| -> String {
        let bound = binding.get(key).filter(|value| is_filled(Some(value)));
        stringify(bound.unwrap_or(&master[key]))
    };

    let parts = vec![
        "Create one 16:9 academic PowerPoint-style slide.".to_string(),
        "All visible slide text must be Simplified Chinese only.".to_string(),
        "Use the extracted template master specification below as immutable; the implement model does not receive the template image, so these text constraints are the source of truth.".to_string(),
        "Use only the page-specific title and page number supplied in the page-specific section; keep them in the extracted title and page-number regions.".to_string(),
        format!("Background/canvas: {}", stringify(&master["canvas"])),
        format!("Title region: {}", pick("title_region")),
        format!("Safe margins/body area: {}", pick("safe_margins")),
        format!("Header/footer/page number/logo/corner marks: {}", pick("header_footer")),
        format!("Divider lines: {}", pick("divider_lines")),
        format!("Palette/background colors: {}", pick("palette")),
        format!("Typography/font hierarchy: {}", pick("typography")),
        format!("Module/card/border style: {}", pick("module_style")),
        format!(
            "Decorative/immutable elements: {}; {}",
            stringify(&master["decorative_elements"]),
            stringify(&master["immutable_elements"])
        ),
        format!("Forbidden deviations: {}", stringify(&master["forbidden_deviations"])),
        "Do not add API output settings to the prompt text. Do not add extra page numbers, random logos, new corner marks, unrelated footer citations, gradients, editing grids, or decorative noise.".to_string(),
    ];
    Ok(parts
        .into_iter()
        .filter(|part| !part.is_empty() && part != "None")
        .collect::<Vec<_>>()
        .join("\n"))
}

pub fn apply_master_prompt_prefix(
    pages: &[Value],
    analysis: &Value,
) -> Checked<Vec<Value>> {
    let mut updated = Vec::with_capacity(pages.len());
    for page in pages {
        let page_prompt = sanitize_page_prompt(page["implement_prompt"].as_str().unwrap_or_default());
        let prefix = master_prefix(analysis, page)?;
        let title = page["title"].as_str().unwrap_or_default().trim().to_string();
        let page_number = page["page"].as_i64().unwrap_or(0);

        let context = vec![
            if title.is_empty() { String::new() } else { format!("Slide title: {title}.") },
            if page_number > 0 {
                format!("Use page number {page_number} only in the extracted page-number position.")
            } else {
                String::new()
            },
            format!(
                "Selected body skeleton: {}. Slide type: {}.",
                stringify(&page["selected_template"]),
                stringify(&page["slide_type"])
            ),
            format!("Body layout plan: {}.", stringify(&page["body_layout_plan"])),
            format!("Visual element plan: {}.", stringify(&page["visual_element_plan"])),
            format!("Keyword emphasis plan: {}.", stringify(&page["emphasis_plan"])),
            "Render the planned visual elements as actual depictions of their subject — draw the device, product, \
             specimen, or scene itself with recognizable shape, structure and proportion. Do not substitute a \
             labeled rectangle, a bare text card, or a generic placeholder box for a subject that can be drawn.".to_string(),
            "Recolor every visual into the template palette and match the template line weight and card/border \
             style, so depicted objects read as part of the deck rather than pasted stock art.".to_string(),
            "Use visual elements only inside the body safe area. Keep them proportional to text, aligned to the template palette, and avoid inventing real brand logos when source context is missing.".to_string(),
            "Highlight only the planned key phrases using bold weight or the template primary/accent red; do not over-highlight full sentences.".to_string(),
            page_prompt,
        ];
        let merged = format!(
            "{prefix}\n\nPage-specific body layout and content:\n{}",
            context
                .into_iter()
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        );
        let mut next = page.clone();
        next["implement_prompt"] = Value::String(merged.trim().to_string());
        updated.push(next);
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_template_analysis() -> Value {
        json!({"template_analysis": {
            "master_style_summary": "Red-gray academic master.",
            "master_style_spec": {
                "canvas": {"aspect_ratio": "16:9", "background": "pure white"},
                "title_region": {"position": "x=6%", "alignment": "left"},
                "safe_margins": {"top": "18%", "body_area": "central rectangle"},
                "header_footer": {"page_number": "bottom right", "logo": "top right"},
                "divider_lines": [{"position": "under title", "stroke": "2px"}],
                "palette": {"background": "#FFFFFF", "primary": "#B40000"},
                "typography": {"title": "32px bold", "body": "18px"},
                "module_style": {"border": "1px solid", "radius": "10px"},
                "decorative_elements": ["red divider"],
                "immutable_elements": ["title region", "page number", "divider lines"],
                "page_layout_rules": "A/B/C stay inside safe area.",
                "forbidden_deviations": ["moving title", "palette drift"]
            },
            "global_constraints": ["keep title band"],
            "immutable_elements": ["title region", "page number", "divider lines"],
            "page_layout_rules": "Body varies inside safe margins."
        }})
    }

    fn valid_page() -> Value {
        json!({
            "page": 1,
            "selected_template": "Template B",
            "title": "研究路线",
            "slide_type": "technical body",
            "body_layout_plan": "Three cards inside central safe body area.",
            "master_style_binding": {
                "title_region": "same title region",
                "safe_margins": "same safe area",
                "header_footer": "same page marker",
                "divider_lines": "same red divider",
                "palette": "same palette",
                "typography": "same hierarchy",
                "module_style": "same card style",
                "background": "same white background"
            },
            "visual_element_plan": {
                "usage_decision": "Depict the acquisition device because the page explains a technical route.",
                "elements": [{
                    "type": "object/device render",
                    "subject": "数据采集设备",
                    "appearance": "矮箱体主机，正面横向散热格栅，右上角圆形指示灯，顶部短天线",
                    "source_reference": "domain knowledge, generic form",
                    "placement": "left of first card",
                    "style": "recolored into template palette",
                    "size_ratio": "small, about 8%"
                }],
                "text_visual_balance": "Device render anchors the first card."
            },
            "emphasis_plan": {
                "keywords": [{"text": "数据处理", "style": "bold", "reason": "first step"}],
                "style_rules": "Only two short phrases."
            },
            "visible_text": ["研究路线", "数据处理"],
            "implement_prompt": "Create one 16:9 academic PowerPoint-style slide. Use Template B with three academic cards in the body safe area showing 数据处理, 模型训练, 结果验证."
        })
    }

    #[test]
    fn validates_pages_and_rejects_gaps() {
        let analysis = valid_template_analysis();
        validate_template_analysis(&analysis).expect("analysis should pass");
        let page = validate_ppt_single_page(&json!({"page": valid_page()}), 1, Some(&analysis))
            .expect("page should pass");
        assert_eq!(page["page"], json!(1));

        let wrong = json!({"page": {"page": 2}});
        assert!(!validate_ppt_single_page(&wrong, 1, Some(&analysis)).unwrap_err().is_schema());

        let mut broken = valid_page();
        broken["master_style_binding"].as_object_mut().unwrap().remove("divider_lines");
        let pages = json!({"pages": [broken]});
        assert!(validate_ppt_pages(&pages, 1, Some(&analysis)).unwrap_err().is_schema());

        let mut too_much = valid_page();
        too_much["emphasis_plan"]["keywords"] = json!([
            {"text": "a", "style": "bold", "reason": "r"},
            {"text": "b", "style": "bold", "reason": "r"},
            {"text": "c", "style": "bold", "reason": "r"},
            {"text": "d", "style": "bold", "reason": "r"},
            {"text": "e", "style": "bold", "reason": "r"},
            {"text": "f", "style": "bold", "reason": "r"}
        ]);
        assert!(validate_ppt_pages(&json!({"pages": [too_much]}), 1, Some(&analysis))
            .unwrap_err()
            .is_schema());

        let mut api_fields = valid_page();
        api_fields["implement_prompt"] = json!(format!(
            "{} size=2048x1152, quality=auto.",
            valid_page()["implement_prompt"].as_str().unwrap()
        ));
        assert!(validate_ppt_pages(&json!({"pages": [api_fields]}), 1, Some(&analysis))
            .unwrap_err()
            .is_schema());
    }

    #[test]
    fn missing_appearance_is_rejected() {
        let mut page = valid_page();
        page["visual_element_plan"]["elements"][0]
            .as_object_mut()
            .unwrap()
            .remove("appearance");
        assert!(validate_ppt_page_fields(&page).unwrap_err().is_schema());
    }

    #[test]
    fn master_prefix_is_uniform_and_demands_real_depiction() {
        let analysis = valid_template_analysis();
        let mut second = valid_page();
        second["page"] = json!(2);
        second["title"] = json!("第二页");
        second["implement_prompt"] =
            json!("Create one 16:9 slide, size=2048x1152, image_size=4K. Show a timeline.");
        let merged = apply_master_prompt_prefix(&[valid_page(), second], &analysis).unwrap();

        let split = "\n\nPage-specific body layout and content:\n";
        let first_prefix = merged[0]["implement_prompt"].as_str().unwrap().split(split).next().unwrap();
        let second_prefix = merged[1]["implement_prompt"].as_str().unwrap().split(split).next().unwrap();
        assert_eq!(first_prefix, second_prefix, "各页 prefix 必须逐字相同");

        for item in &merged {
            let prompt = item["implement_prompt"].as_str().unwrap();
            assert!(prompt.contains("Use the extracted template master specification below as immutable"));
            assert!(!prompt.contains("size="));
            assert!(!prompt.contains("image_size="));
            assert!(prompt.contains("Create one 16:9 academic PowerPoint-style slide"));
            assert!(prompt.contains("Visual element plan"));
            assert!(prompt.contains("Keyword emphasis plan"));
            assert!(prompt.contains("Highlight only the planned key phrases"));
            assert!(prompt.contains("actual depictions of their subject"));
            assert!(prompt.contains("Do not substitute a labeled rectangle"));
        }
        assert!(merged[0]["implement_prompt"].as_str().unwrap().contains("散热格栅"));
    }
}
