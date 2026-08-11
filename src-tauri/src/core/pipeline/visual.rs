use regex::Regex;
use serde::Serialize;

use crate::core::config::ModelProfile;
use crate::core::search::{SearchClient, SearchResult};

pub const VISUAL_ASSET_SEARCH_TERM_LIMIT: usize = 8;
pub const VISUAL_ASSET_SEARCH_RESULT_LIMIT: usize = 3;
const MATERIAL_TEXT_LIMIT: usize = 6000;

const KNOWN_TERMS: &[&str] = &[
    "PyTorch", "TensorFlow", "JAX", "Keras", "scikit-learn", "OpenCV", "Hugging Face",
    "LangChain", "Stable Diffusion", "OpenAI", "Claude", "Gemini", "Llama", "Qwen",
    "DeepSeek", "Nano Banana", "WisArt",
    "Docker", "Kubernetes", "GitHub", "GitLab", "Jenkins", "Nginx", "Kafka", "Spark",
    "Hadoop", "Elasticsearch", "AWS", "Azure", "GCP", "阿里云", "腾讯云", "华为云",
    "PostgreSQL", "MongoDB", "Redis", "MySQL", "SQLite", "ClickHouse", "Neo4j",
    "NVIDIA", "CUDA", "Jetson", "Raspberry Pi", "Arduino", "STM32", "FPGA", "Intel",
    "AMD", "ARM", "树莓派",
    "SEM", "TEM", "AFM", "XRD", "XPS", "NMR", "MRI", "PCR", "HPLC",
    "扫描电镜", "透射电镜", "原子力显微镜", "质谱仪", "光谱仪", "色谱仪", "离心机",
    "培养箱", "示波器", "激光器", "光刻机", "反应釜",
    "无人机", "机械臂", "机器人", "自动驾驶", "激光雷达", "卫星",
    "MATLAB", "Simulink", "SolidWorks", "AutoCAD", "Blender", "Figma", "Notion",
    "Slack", "Jira", "Confluence", "LaTeX", "Origin", "ImageJ",
];

const CN_SUFFIXES: &[&str] = &[
    "无人机", "机器人", "机械臂", "传感器", "反应釜", "培养皿", "培养箱", "显微镜",
    "光谱仪", "离心机", "示波器", "激光器", "发动机", "换热器", "催化剂", "电解槽",
    "晶圆", "芯片", "电极", "电池", "薄膜", "涂层", "探头", "模组", "阵列", "支架",
    "导管", "样机", "样品", "试剂", "装置", "设备", "仪器", "机床", "产线", "车间",
    "卫星", "雷达", "天线", "车辆", "船舶", "飞行器",
];

const TERM_STOPWORDS: &[&str] = &[
    "A", "An", "And", "Body", "Card", "Create", "Data", "Figure", "Flow", "Input",
    "Material", "Model", "Output", "Page", "Prompt", "Result", "Slide", "Template",
    "The", "Use", "User",
    "Bayes", "Bayesian", "Banach", "Cauchy", "Euler", "Fourier", "Gauss", "Gaussian",
    "Hessian", "Hilbert", "Jacobian", "Lagrange", "Laplace", "Lipschitz", "Lyapunov",
    "Markov", "Monte Carlo", "Nash", "Newton", "Pareto", "Poisson", "Taylor",
    "Bernoulli", "Frobenius", "Kullback", "Leibler", "Wasserstein",
];

const BOUNDARY_CHARS: &str = "对与和及或的了在从由被把将用以为并中后前时上下等则若使可将其该本此这那每各";

#[derive(Clone, Debug, Serialize)]
pub struct VisualAssetItem {
    pub term: String,
    pub query: String,
    pub results: Vec<SearchResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub provider: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct VisualAssetContext {
    pub enabled: bool,
    pub degraded: bool,
    pub terms: Vec<String>,
    pub items: Vec<VisualAssetItem>,
    pub message: String,
}

pub fn extract_visual_asset_terms(material: &str) -> Vec<String> {
    let text: String = material.chars().take(MATERIAL_TEXT_LIMIT).collect();
    let lowered = text.to_lowercase();

    let known: Vec<String> = KNOWN_TERMS
        .iter()
        .filter(|term| lowered.contains(&term.to_lowercase()))
        .map(|term| term.to_string())
        .collect();

    let suffix_group = CN_SUFFIXES
        .iter()
        .map(|suffix| regex::escape(suffix))
        .collect::<Vec<_>>()
        .join("|");
    let cn_pattern = Regex::new(&format!(
        r"[\x{{4e00}}-\x{{9fff}}--[{BOUNDARY_CHARS}]]{{0,4}}(?:{suffix_group})"
    ))
    .expect("valid regex");
    let mut chinese: Vec<String> = cn_pattern
        .find_iter(&text)
        .map(|m| m.as_str().to_string())
        .collect();
    chinese.sort_by_key(|term| std::cmp::Reverse(text.matches(term.as_str()).count()));

    let mut english: Vec<String> = Vec::new();
    for pattern in [
        r"\b[A-Z][A-Za-z0-9.+#-]{1,}(?:\s+[A-Z0-9][A-Za-z0-9.+#-]{1,}){0,2}\b",
        r"\b[A-Z]{2,}(?:[-\s][A-Z0-9]{2,}){0,2}\b",
    ] {
        let regex = Regex::new(pattern).expect("valid regex");
        english.extend(regex.find_iter(&text).map(|m| m.as_str().to_string()));
    }

    let mut candidates = known;
    candidates.extend(chinese);
    candidates.extend(english);
    dedupe_terms(&candidates)
}

fn dedupe_terms(candidates: &[String]) -> Vec<String> {
    let quantifier =
        Regex::new(r"^[一二三四五六七八九十百千两0-9]+[套台个批组种类款部只条张片辆架]")
            .expect("valid regex");
    let determiner =
        Regex::new(r"^(?:该|本|其|此|这|那|各|每|所述|上述|相应|对应)").expect("valid regex");
    let category = Regex::new(r"\s*(平台|工具|框架|模型|软件|系统|设备)$").expect("valid regex");
    let latin = Regex::new(r"[A-Za-z]").expect("valid regex");
    let spaces = Regex::new(r"\s+").expect("valid regex");
    let stopwords: Vec<String> = TERM_STOPWORDS.iter().map(|s| s.to_lowercase()).collect();

    let mut normalized: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for candidate in candidates {
        let mut term = spaces
            .replace_all(candidate, " ")
            .trim_matches(|ch: char| " ,.;:()[]{}<>，。；：（）【】".contains(ch))
            .to_string();
        term = quantifier.replace(&term, "").to_string();
        term = determiner.replace(&term, "").trim().to_string();
        let stripped = category.replace(&term, "").trim().to_string();
        if !stripped.is_empty() && stripped != term && latin.is_match(&stripped) {
            term = stripped;
        }
        let length = term.chars().count();
        if length < 2 || length > 48 {
            continue;
        }
        let key = term.to_lowercase();
        if stopwords.contains(&key) || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        normalized.push(term);
    }

    let mut terms: Vec<String> = Vec::new();
    for term in &normalized {
        if normalized
            .iter()
            .any(|other| other != term && term.contains(other.as_str()))
        {
            continue;
        }
        terms.push(term.clone());
        if terms.len() >= VISUAL_ASSET_SEARCH_TERM_LIMIT {
            break;
        }
    }
    terms
}

pub fn visual_asset_search_query(term: &str) -> String {
    let has_cjk = term.chars().any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch));
    if has_cjk {
        format!("{term} 实物外观 外形结构 产品图片 特征描述")
    } else {
        format!("{term} official logo product appearance what it looks like visual description")
    }
}

pub async fn build_visual_asset_context(
    material: &str,
    search_profile: &ModelProfile,
    proxy_url: Option<&str>,
) -> VisualAssetContext {
    let terms = extract_visual_asset_terms(material);
    if terms.is_empty() {
        return VisualAssetContext {
            enabled: true,
            degraded: true,
            terms: Vec::new(),
            items: Vec::new(),
            message: "No explicit product/tool/equipment terms were detected.".to_string(),
        };
    }

    let max_results = search_profile
        .output_defaults
        .get("max_results")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(VISUAL_ASSET_SEARCH_RESULT_LIMIT as u64)
        .clamp(1, 8) as usize;

    let futures = terms.iter().map(|term| async move {
        let query = visual_asset_search_query(term);
        match SearchClient::search(search_profile, &query, max_results, proxy_url).await {
            Ok(results) => VisualAssetItem {
                term: term.clone(),
                query,
                results,
                error: None,
                provider: search_profile.protocol.clone(),
            },
            Err(error) => VisualAssetItem {
                term: term.clone(),
                query,
                results: Vec::new(),
                error: Some(error.message),
                provider: search_profile.protocol.clone(),
            },
        }
    });
    let items: Vec<VisualAssetItem> = futures::future::join_all(futures).await;
    let has_sources = items.iter().any(|item| !item.results.is_empty());

    VisualAssetContext {
        enabled: true,
        degraded: !has_sources,
        terms,
        items,
        message: "Use only text summaries and source URLs; no network image is downloaded, \
                  cached, or passed to the implement model."
            .to_string(),
    }
}

pub fn visual_asset_context_text(context: &VisualAssetContext) -> String {
    if context.terms.is_empty() {
        return "No specific product/tool/equipment terms were detected in the material. \
                Still prefer concrete visual representation over plain labeled rectangles: use recognizable object \
                silhouettes, equipment/device illustrations, schematic cutaways, or semantic icons that depict the \
                actual subject discussed on the page. Only fall back to a plain text card when the content is purely \
                abstract. Do not invent a specific real brand logo that you are not confident about."
            .to_string();
    }

    let provider = context
        .items
        .first()
        .map(|item| item.provider.clone())
        .unwrap_or_else(|| "configured search model".to_string());

    let mut lines = vec![
        "Runtime visual asset search context. Use this as text-only evidence describing what these subjects \
         actually look like; no images are downloaded or passed to the implement model."
            .to_string(),
        format!("Search provider: {provider}."),
        "GOAL: turn these subjects into real visual depictions on the slide instead of text inside a box. \
         A slide that draws the actual device/product/object reads far better than one that writes its name in a rectangle."
            .to_string(),
        "For each grounded term below, describe its concrete appearance in the implement prompt: overall shape and \
         proportion, dominant materials and colors, defining structural features, and typical orientation. \
         Recolor into the template palette rather than copying source colors verbatim."
            .to_string(),
        "If a term has no reliable source, still depict it generically from domain knowledge (a generic microscope, \
         a generic drone) rather than degrading to a text-only card. Only avoid rendering a specific brand logo \
         when no reliable source describes it."
            .to_string(),
    ];

    for item in &context.items {
        lines.push(format!("- Term: {}; query: {}", item.term, item.query));
        if item.results.is_empty() {
            let reason = item.error.clone().unwrap_or_else(|| "no reliable result".to_string());
            lines.push(format!(
                "  Source status: {reason}. Depict this subject generically from domain knowledge; \
                 avoid brand-specific marks."
            ));
            continue;
        }
        for result in &item.results {
            lines.push(format!(
                "  Source: {} | {} | {}",
                result.title, result.url, result.snippet
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_known_english_terms() {
        let terms = extract_visual_asset_terms("本方案使用 Docker、Kubernetes 和 OpenAI API 构建部署流程。");
        assert!(terms.contains(&"Docker".to_string()), "{terms:?}");
        assert!(terms.contains(&"Kubernetes".to_string()), "{terms:?}");
    }

    #[test]
    fn extracts_chinese_object_terms() {
        let material = "实验采用高分辨质谱仪对样品进行检测，随后使用离心机分离，\
                        并通过六旋翼无人机完成野外采样。培养皿中的样品由机械臂转运。";
        let terms = extract_visual_asset_terms(material);
        assert!(terms.iter().any(|t| t.contains("质谱仪")), "{terms:?}");
        assert!(terms.contains(&"离心机".to_string()), "{terms:?}");
        assert!(terms.iter().any(|t| t.contains("无人机")), "{terms:?}");
        assert!(terms.contains(&"培养皿".to_string()), "{terms:?}");
        assert!(terms.contains(&"机械臂".to_string()), "{terms:?}");
    }

    #[test]
    fn chinese_nouns_keep_category_suffix() {
        let terms = extract_visual_asset_terms("现场部署了一套检测设备，配合 PyTorch框架 完成推理。");
        assert!(terms.contains(&"检测设备".to_string()), "{terms:?}");
        assert!(terms.contains(&"PyTorch".to_string()), "{terms:?}");
        assert!(!terms.contains(&"PyTorch框架".to_string()), "{terms:?}");
    }

    #[test]
    fn abstract_material_yields_nothing() {
        let material = "针对带非光滑正则项的复合优化问题，提出一种自适应步长的近端梯度算法。\
                        通过构造 Lyapunov 函数证明算法在弱凸假设下收敛到稳定点，并给出 O(1/k) 收敛速率。\
                        进一步用 Jacobian 与 Hessian 分析步长参数对收敛常数的影响。";
        assert!(extract_visual_asset_terms(material).is_empty());
    }

    #[test]
    fn query_splits_by_language() {
        assert!(visual_asset_search_query("离心机").contains("实物外观"));
        assert!(visual_asset_search_query("Docker").contains("official logo"));
    }

    #[test]
    fn context_text_pushes_real_depiction() {
        let context = VisualAssetContext {
            enabled: true,
            degraded: false,
            terms: vec!["离心机".to_string()],
            items: vec![VisualAssetItem {
                term: "离心机".to_string(),
                query: visual_asset_search_query("离心机"),
                results: vec![SearchResult {
                    title: "离心机产品页".to_string(),
                    url: "https://example.com".to_string(),
                    snippet: "台式高速离心机".to_string(),
                }],
                error: None,
                provider: "duckduckgo_html".to_string(),
            }],
            message: String::new(),
        };
        let text = visual_asset_context_text(&context);
        assert!(text.contains("text-only evidence"));
        assert!(text.contains("instead of text inside a box"));
        assert!(text.contains("离心机产品页"));
        assert!(!text.to_lowercase().contains("base64"));
    }
}
