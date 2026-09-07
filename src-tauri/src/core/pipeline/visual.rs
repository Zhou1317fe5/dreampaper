use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::core::config::ModelProfile;
use crate::core::prompt::PromptStore;
use crate::core::search::{SearchClient, SearchResult};
use crate::error::{AppError, AppResult};

pub const VISUAL_ASSET_SEARCH_TERM_LIMIT: usize = 8;
pub const VISUAL_ASSET_SEARCH_RESULT_LIMIT: usize = 3;
const MATERIAL_TEXT_LIMIT: usize = 6000;

pub const VISUAL_TERMS_KEY: &str = "global/visual_terms.json";
const EMBEDDED_VISUAL_TERMS: &str = include_str!("../../../../prompts/global/visual_terms.json");

// Categories whose search should target a brand mark rather than a physical object.
const LOGO_CATEGORIES: &[&str] = &[
    "vendor",
    "model",
    "tool",
    "cloud",
    "infra",
    "database",
    "software",
    "robot_vendor",
];
const OBJECT_CATEGORIES: &[&str] = &["robot", "sensor", "chip", "instrument"];

#[derive(Clone, Debug, Deserialize)]
pub struct VisualTerm {
    pub term: String,
    pub brand: String,
    // Tolerated when absent (like the Python loader); an empty category just keeps the legacy query.
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Deserialize)]
struct VisualTermsFile {
    version: u64,
    entries: Vec<VisualTerm>,
}

#[derive(Clone, Debug)]
enum TermPattern {
    Latin(Regex),
    Cjk(String),
}

#[derive(Clone, Debug)]
struct TermMatcher {
    entry: usize,
    pattern: TermPattern,
}

/// Parsed vocabulary with one precompiled matcher per term/alias string.
#[derive(Clone, Debug)]
pub struct VisualTerms {
    entries: Vec<VisualTerm>,
    matchers: Vec<TermMatcher>,
}

/// One extracted subject. Vocabulary hits carry brand/category/matched; heuristic hits leave them `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualHit {
    pub term: String,
    pub brand: Option<String>,
    pub category: Option<String>,
    pub matched: Option<String>,
    pub position: usize,
}

impl VisualTerms {
    pub fn parse(json: &str) -> AppResult<Self> {
        let file: VisualTermsFile = serde_json::from_str(json).map_err(|error| {
            AppError::new(
                "visual_terms_invalid",
                format!("visual_terms.json is not valid JSON: {error}"),
            )
        })?;
        if file.version != 1 {
            return Err(AppError::new(
                "visual_terms_invalid",
                format!(
                    "visual_terms.json version {} is not supported",
                    file.version
                ),
            ));
        }
        if file.entries.is_empty() {
            return Err(AppError::new(
                "visual_terms_invalid",
                "visual_terms.json has no entries",
            ));
        }
        let mut matchers = Vec::new();
        for (index, entry) in file.entries.iter().enumerate() {
            if entry.term.trim().is_empty() || entry.brand.trim().is_empty() {
                return Err(AppError::new(
                    "visual_terms_invalid",
                    format!("visual_terms.json entry {index} needs a non-empty term and brand"),
                ));
            }
            for alias in std::iter::once(&entry.term).chain(entry.aliases.iter()) {
                if alias.trim().is_empty() {
                    return Err(AppError::new(
                        "visual_terms_invalid",
                        format!("visual_terms.json entry {} has an empty alias", entry.term),
                    ));
                }
                matchers.push(TermMatcher {
                    entry: index,
                    pattern: build_term_pattern(alias)?,
                });
            }
        }
        Ok(Self {
            entries: file.entries,
            matchers,
        })
    }

    pub fn embedded() -> &'static VisualTerms {
        static EMBEDDED: OnceLock<VisualTerms> = OnceLock::new();
        EMBEDDED.get_or_init(|| {
            VisualTerms::parse(EMBEDDED_VISUAL_TERMS)
                .expect("embedded prompts/global/visual_terms.json must be valid")
        })
    }

    /// External `prompts/` directory wins; any read or parse problem falls back to the embedded
    /// vocabulary so a broken override never fails the job.
    pub fn from_store(store: &PromptStore) -> VisualTerms {
        match store.load(VISUAL_TERMS_KEY) {
            Ok(asset) if asset.content != EMBEDDED_VISUAL_TERMS => {
                VisualTerms::parse(&asset.content).unwrap_or_else(|_| Self::embedded().clone())
            }
            _ => Self::embedded().clone(),
        }
    }

    pub fn find(&self, text: &str) -> Vec<VisualHit> {
        // Per entry keep the longest matched string (char count, like Python's len()).
        let mut best: Vec<Option<(String, usize)>> = vec![None; self.entries.len()];
        for matcher in &self.matchers {
            let found = match &matcher.pattern {
                TermPattern::Latin(regex) => regex
                    .captures(text)
                    .and_then(|caps| caps.get(1))
                    .map(|group| (group.as_str().to_string(), group.start())),
                TermPattern::Cjk(alias) => {
                    text.find(alias.as_str()).map(|pos| (alias.clone(), pos))
                }
            };
            let Some((matched, position)) = found else {
                continue;
            };
            let slot = &mut best[matcher.entry];
            let longer = slot
                .as_ref()
                .is_none_or(|(current, _)| matched.chars().count() > current.chars().count());
            if longer {
                *slot = Some((matched, position));
            }
        }

        let candidates: Vec<(usize, String, usize)> = best
            .into_iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.map(|(matched, position)| (index, matched, position)))
            .collect();
        let lowered: Vec<String> = candidates
            .iter()
            .map(|(_, matched, _)| matched.to_lowercase())
            .collect();
        let mut hits: Vec<VisualHit> = candidates
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                // Longest match across entries: drop a hit whose matched string is a proper
                // substring of another hit's matched string (`Unitree` loses to `Unitree G1`).
                !lowered.iter().enumerate().any(|(j, other)| {
                    j != *i && other != &lowered[*i] && other.contains(lowered[*i].as_str())
                })
            })
            .map(|(_, (index, matched, position))| {
                let entry = &self.entries[*index];
                VisualHit {
                    term: entry.term.clone(),
                    brand: Some(entry.brand.clone()),
                    category: Some(entry.category.clone()),
                    matched: Some(matched.clone()),
                    position: *position,
                }
            })
            .collect();
        hits.sort_by_key(|hit| hit.position);
        hits
    }
}

fn has_cjk(text: &str) -> bool {
    text.chars()
        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
}

/// Latin strings match on ASCII alphanumeric boundaries (`\b` treats CJK as `\w`, so
/// `PyTorch框架` would miss); all-caps acronyms stay case-sensitive so `robot arm` never hits `ARM`.
fn build_term_pattern(alias: &str) -> AppResult<TermPattern> {
    if has_cjk(alias) {
        return Ok(TermPattern::Cjk(alias.to_string()));
    }
    let body = alias
        .split_whitespace()
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(r"\s+");
    let compact = alias.replace(' ', "");
    let all_caps = !compact.is_empty()
        && compact
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit());
    let flags = if all_caps { "" } else { "(?i)" };
    Regex::new(&format!(
        "{flags}(?:^|[^A-Za-z0-9])({body})(?:[^A-Za-z0-9]|$)"
    ))
    .map(TermPattern::Latin)
    .map_err(|error| {
        AppError::new(
            "visual_terms_invalid",
            format!("visual_terms.json alias {alias:?} is not matchable: {error}"),
        )
    })
}

const CN_SUFFIXES: &[&str] = &[
    "无人机",
    "机器人",
    "机械臂",
    "传感器",
    "反应釜",
    "培养皿",
    "培养箱",
    "显微镜",
    "光谱仪",
    "离心机",
    "示波器",
    "激光器",
    "发动机",
    "换热器",
    "催化剂",
    "电解槽",
    "晶圆",
    "芯片",
    "电极",
    "电池",
    "薄膜",
    "涂层",
    "探头",
    "模组",
    "阵列",
    "支架",
    "导管",
    "样机",
    "样品",
    "试剂",
    "装置",
    "设备",
    "仪器",
    "机床",
    "产线",
    "车间",
    "卫星",
    "雷达",
    "天线",
    "车辆",
    "船舶",
    "飞行器",
];

const TERM_STOPWORDS: &[&str] = &[
    "A",
    "An",
    "And",
    "Body",
    "Card",
    "Create",
    "Data",
    "Figure",
    "Flow",
    "Input",
    "Material",
    "Model",
    "Output",
    "Page",
    "Prompt",
    "Result",
    "Slide",
    "Template",
    "The",
    "Use",
    "User",
    "Bayes",
    "Bayesian",
    "Banach",
    "Cauchy",
    "Euler",
    "Fourier",
    "Gauss",
    "Gaussian",
    "Hessian",
    "Hilbert",
    "Jacobian",
    "Lagrange",
    "Laplace",
    "Lipschitz",
    "Lyapunov",
    "Markov",
    "Monte Carlo",
    "Nash",
    "Newton",
    "Pareto",
    "Poisson",
    "Taylor",
    "Bernoulli",
    "Frobenius",
    "Kullback",
    "Leibler",
    "Wasserstein",
];

const BOUNDARY_CHARS: &str =
    "对与和及或的了在从由被把将用以为并中后前时上下等则若使可将其该本此这那每各";

#[derive(Clone, Debug, Serialize)]
pub struct VisualAssetItem {
    pub term: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
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

/// Canonical terms only; the pipeline itself consumes `extract_visual_asset_hits`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn extract_visual_asset_terms(material: &str, vocabulary: &VisualTerms) -> Vec<String> {
    extract_visual_asset_hits(material, vocabulary)
        .into_iter()
        .map(|hit| hit.term)
        .collect()
}

/// Vocabulary hits (by first occurrence) followed by heuristic candidates that the vocabulary
/// does not already cover, capped at `VISUAL_ASSET_SEARCH_TERM_LIMIT`.
pub fn extract_visual_asset_hits(material: &str, vocabulary: &VisualTerms) -> Vec<VisualHit> {
    let text: String = material.chars().take(MATERIAL_TEXT_LIMIT).collect();
    let vocabulary_hits = vocabulary.find(&text);

    let candidates: Vec<String> = heuristic_candidates(&text)
        .into_iter()
        .filter(|candidate| !covered_by_vocabulary(candidate, &vocabulary_hits))
        .collect();
    // Normalization inside dedupe_terms can fold a candidate onto a vocabulary term
    // (`PyTorch框架` -> `PyTorch`), so the coverage check runs again afterwards.
    let heuristic: Vec<VisualHit> = dedupe_terms(&candidates)
        .into_iter()
        .filter(|term| !covered_by_vocabulary(term, &vocabulary_hits))
        .map(|term| VisualHit {
            position: text.find(term.as_str()).unwrap_or(text.len()),
            term,
            brand: None,
            category: None,
            matched: None,
        })
        .collect();

    let mut hits = vocabulary_hits;
    hits.extend(heuristic);
    hits.truncate(VISUAL_ASSET_SEARCH_TERM_LIMIT);
    hits
}

/// A heuristic candidate is redundant when it equals or is a fragment of a vocabulary hit's
/// matched string / canonical term (`GLM-4.5` vs `Zhipu`), or when it contains that string as a
/// whole word (`ARM Cortex-A78` vs `ARM`; but `Spectrum` is not covered by `CT`).
fn covered_by_vocabulary(candidate: &str, hits: &[VisualHit]) -> bool {
    let normalized = normalize_for_coverage(candidate);
    hits.iter().any(|hit| {
        hit.matched
            .as_deref()
            .into_iter()
            .chain(std::iter::once(hit.term.as_str()))
            .map(normalize_for_coverage)
            .any(|known| known.contains(normalized.as_str()) || contains_word(&normalized, &known))
    })
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(start, found)| {
        let before = haystack[..start].chars().next_back();
        let after = haystack[start + found.len()..].chars().next();
        !before.is_some_and(|ch| ch.is_ascii_alphanumeric())
            && !after.is_some_and(|ch| ch.is_ascii_alphanumeric())
    })
}

fn normalize_for_coverage(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn heuristic_candidates(text: &str) -> Vec<String> {
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
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect();
    chinese.sort_by_key(|term| std::cmp::Reverse(text.matches(term.as_str()).count()));

    let mut english: Vec<String> = Vec::new();
    for pattern in [
        r"\b[A-Z][A-Za-z0-9.+#-]{1,}(?:\s+[A-Z0-9][A-Za-z0-9.+#-]{1,}){0,2}\b",
        r"\b[A-Z]{2,}(?:[-\s][A-Z0-9]{2,}){0,2}\b",
    ] {
        let regex = Regex::new(pattern).expect("valid regex");
        english.extend(regex.find_iter(text).map(|m| m.as_str().to_string()));
    }

    chinese.extend(english);
    chinese
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
        if !(2..=48).contains(&length) {
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

/// Logo-type categories search for the brand mark; object-type categories for the product
/// appearance. Unknown/absent categories keep the legacy sentences.
pub fn visual_asset_search_query(
    term: &str,
    brand: Option<&str>,
    category: Option<&str>,
) -> String {
    let brand = brand
        .map(str::trim)
        .filter(|brand| !brand.is_empty() && brand.to_lowercase() != term.to_lowercase());
    let is_logo = category.is_some_and(|category| LOGO_CATEGORIES.contains(&category));
    let is_object = category.is_some_and(|category| OBJECT_CATEGORIES.contains(&category));

    if has_cjk(term) {
        let subject = match brand {
            Some(brand) => format!("{term} {brand}"),
            None => term.to_string(),
        };
        if is_logo {
            format!("{subject} 官方logo 品牌标识 视觉描述")
        } else {
            format!("{subject} 实物外观 外形结构 产品图片 特征描述")
        }
    } else {
        let subject = match brand {
            Some(brand) => format!("{brand} {term}"),
            None => term.to_string(),
        };
        if is_logo {
            format!("{subject} official logo brand mark visual description")
        } else if is_object {
            format!("{subject} product appearance what it looks like visual description")
        } else {
            format!("{term} official logo product appearance what it looks like visual description")
        }
    }
}

pub async fn build_visual_asset_context(
    material: &str,
    vocabulary: &VisualTerms,
    search_profile: &ModelProfile,
    proxy_url: Option<&str>,
) -> VisualAssetContext {
    let hits = extract_visual_asset_hits(material, vocabulary);
    let terms: Vec<String> = hits.iter().map(|hit| hit.term.clone()).collect();
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

    let futures = hits.iter().map(|hit| async move {
        let query =
            visual_asset_search_query(&hit.term, hit.brand.as_deref(), hit.category.as_deref());
        let (results, error) =
            match SearchClient::search(search_profile, &query, max_results, proxy_url).await {
                Ok(results) => (results, None),
                Err(error) => (Vec::new(), Some(error.message)),
            };
        VisualAssetItem {
            term: hit.term.clone(),
            brand: hit.brand.clone(),
            category: hit.category.clone(),
            matched: hit.matched.clone(),
            query,
            results,
            error,
            provider: search_profile.protocol.clone(),
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
        let mut line = format!("- Term: {}", item.term);
        if let Some(brand) = &item.brand {
            line.push_str(&format!("; brand: {brand}"));
        }
        if let Some(category) = &item.category {
            line.push_str(&format!("; category: {category}"));
        }
        if let Some(matched) = &item.matched {
            line.push_str(&format!("; matched in material: {matched}"));
        }
        line.push_str(&format!("; query: {}", item.query));
        lines.push(line);
        if item.results.is_empty() {
            let reason = item
                .error
                .clone()
                .unwrap_or_else(|| "no reliable result".to_string());
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

    fn vocabulary() -> &'static VisualTerms {
        VisualTerms::embedded()
    }

    fn extract(material: &str) -> Vec<String> {
        extract_visual_asset_terms(material, vocabulary())
    }

    fn found_terms(text: &str) -> Vec<String> {
        vocabulary()
            .find(text)
            .into_iter()
            .map(|hit| hit.term)
            .collect()
    }

    fn assert_terms(actual: &[String], expected: &[&str], absent: &[&str]) {
        for term in expected {
            assert!(
                actual.iter().any(|t| t == term),
                "missing {term}: {actual:?}"
            );
        }
        for term in absent {
            assert!(
                !actual.iter().any(|t| t == term),
                "unexpected {term}: {actual:?}"
            );
        }
    }

    #[test]
    fn extracts_known_english_terms() {
        let terms = extract("本方案使用 Docker、Kubernetes 和 OpenAI API 构建部署流程。");
        assert!(terms.contains(&"Docker".to_string()), "{terms:?}");
        assert!(terms.contains(&"Kubernetes".to_string()), "{terms:?}");
    }

    #[test]
    fn extracts_chinese_object_terms() {
        let material = "实验采用高分辨质谱仪对样品进行检测，随后使用离心机分离，\
                        并通过六旋翼无人机完成野外采样。培养皿中的样品由机械臂转运。";
        let terms = extract(material);
        assert!(terms.iter().any(|t| t.contains("质谱仪")), "{terms:?}");
        assert!(terms.contains(&"离心机".to_string()), "{terms:?}");
        assert!(terms.iter().any(|t| t.contains("无人机")), "{terms:?}");
        assert!(terms.contains(&"培养皿".to_string()), "{terms:?}");
        assert!(terms.contains(&"机械臂".to_string()), "{terms:?}");
    }

    #[test]
    fn chinese_nouns_keep_category_suffix() {
        let terms = extract("现场部署了一套检测设备，配合 PyTorch框架 完成推理。");
        assert!(terms.contains(&"检测设备".to_string()), "{terms:?}");
        assert!(terms.contains(&"PyTorch".to_string()), "{terms:?}");
        assert!(!terms.contains(&"PyTorch框架".to_string()), "{terms:?}");
    }

    #[test]
    fn abstract_material_yields_nothing() {
        let material = "针对带非光滑正则项的复合优化问题，提出一种自适应步长的近端梯度算法。\
                        通过构造 Lyapunov 函数证明算法在弱凸假设下收敛到稳定点，并给出 O(1/k) 收敛速率。\
                        进一步用 Jacobian 与 Hessian 分析步长参数对收敛常数的影响。";
        assert!(extract(material).is_empty());
    }

    #[test]
    fn query_splits_by_language() {
        assert!(visual_asset_search_query("离心机", None, None).contains("实物外观"));
        assert!(visual_asset_search_query("Docker", None, None).contains("official logo"));
    }

    #[test]
    fn context_text_pushes_real_depiction() {
        let context = VisualAssetContext {
            enabled: true,
            degraded: false,
            terms: vec!["离心机".to_string()],
            items: vec![VisualAssetItem {
                term: "离心机".to_string(),
                brand: Some("离心机".to_string()),
                category: Some("instrument".to_string()),
                matched: Some("离心机".to_string()),
                query: visual_asset_search_query("离心机", Some("离心机"), Some("instrument")),
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
        assert!(text.contains(
            "- Term: 离心机; brand: 离心机; category: instrument; matched in material: 离心机; query: "
        ));
        assert!(!text.to_lowercase().contains("base64"));
    }

    #[test]
    fn embedded_vocabulary_is_well_formed() {
        let entries = &vocabulary().entries;
        assert!(entries.len() >= 330, "{}", entries.len());
        let mut seen: Vec<String> = Vec::new();
        for entry in entries {
            assert!(!entry.term.trim().is_empty());
            assert!(
                !entry.brand.trim().is_empty(),
                "{} has no brand",
                entry.term
            );
            assert!(
                LOGO_CATEGORIES.contains(&entry.category.as_str())
                    || OBJECT_CATEGORIES.contains(&entry.category.as_str()),
                "{} has unknown category {:?}",
                entry.term,
                entry.category
            );
            for alias in std::iter::once(&entry.term).chain(entry.aliases.iter()) {
                let key = alias.to_lowercase();
                assert!(!seen.contains(&key), "duplicate vocabulary string: {alias}");
                seen.push(key);
            }
        }
    }

    #[test]
    fn boundary_matching_and_case_rules() {
        assert!(found_terms("Pipeline").is_empty());
        assert!(found_terms("Yield").is_empty());
        assert!(found_terms("Co3O4").is_empty());
        assert_eq!(found_terms("PyTorch框架"), vec!["PyTorch"]);
        assert_eq!(found_terms("robot arm"), vec!["机械臂"]);
        assert_eq!(found_terms("ARM Cortex"), vec!["ARM"]);
        assert!(found_terms("ct scan").is_empty());
        assert_eq!(found_terms("CT scan"), vec!["CT"]);
        assert_eq!(found_terms("Hugging   Face"), vec!["Hugging Face"]);
    }

    #[test]
    fn aliases_resolve_to_canonical_term_and_brand() {
        let hits = vocabulary().find("Llama 3");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].term, "Llama");
        assert_eq!(hits[0].brand.as_deref(), Some("Meta"));
        assert_eq!(hits[0].matched.as_deref(), Some("Llama 3"));

        let kimi = vocabulary().find("Kimi K2");
        let moonshot = vocabulary().find("月之暗面");
        assert_eq!(kimi.len(), 1, "{kimi:?}");
        assert_eq!(moonshot.len(), 1, "{moonshot:?}");
        assert_eq!(kimi[0].term, "月之暗面");
        assert_eq!(kimi[0].term, moonshot[0].term);
        assert_eq!(kimi[0].brand, moonshot[0].brand);
    }

    #[test]
    fn longest_match_wins_across_entries() {
        assert_eq!(found_terms("Unitree G1"), vec!["Unitree G1"]);
        let xpeng = found_terms("Xpeng Iron");
        assert!(xpeng.contains(&"Xpeng Iron".to_string()), "{xpeng:?}");
        assert!(!xpeng.contains(&"Xpeng".to_string()), "{xpeng:?}");
    }

    #[test]
    fn heuristic_candidates_covered_by_vocabulary_are_dropped() {
        let terms = extract("智谱发布 GLM-4.5，性能领先。");
        assert_terms(&terms, &["Zhipu"], &["GLM-4.5", "GLM"]);

        // A longer heuristic phrase containing a hit as a whole word is redundant too,
        // but an accidental substring outside word boundaries is not coverage.
        let terms = extract("A CT image and a Spectrum Analyzer on the ARM Cortex-A78 board.");
        assert_terms(
            &terms,
            &["CT", "ARM", "Spectrum Analyzer"],
            &["ARM Cortex-A78"],
        );
    }

    #[test]
    fn query_uses_brand_and_category() {
        let llama = visual_asset_search_query("Llama", Some("Meta"), Some("model"));
        assert!(llama.contains("Meta"), "{llama}");
        assert!(llama.contains("official logo"), "{llama}");

        let g1 = visual_asset_search_query("Unitree G1", Some("Unitree"), Some("robot"));
        assert!(g1.contains("product appearance"), "{g1}");
        assert!(!g1.contains("official logo"), "{g1}");

        let hailuo = visual_asset_search_query("海螺AI", Some("MiniMax"), Some("vendor"));
        assert!(hailuo.contains("官方logo"), "{hailuo}");
        assert!(hailuo.contains("MiniMax"), "{hailuo}");

        // brand == term adds no duplicate prefix
        assert!(
            visual_asset_search_query("Docker", Some("Docker"), Some("infra"))
                .starts_with("Docker official logo")
        );
    }

    /// Same samples and expectations as the Python test suite (design.md §5).
    #[test]
    fn consistency_samples_match_reference() {
        let samples: [(&str, &[&str], &[&str]); 9] = [
            (
                "本方案使用 Docker、Kubernetes 和 OpenAI API 构建部署流程。",
                &["Docker", "Kubernetes", "OpenAI"],
                &[],
            ),
            (
                "我们在 Unitree G1 与宇树 Go2 上部署了基于 Qwen3 的 VLA 模型，并用 Isaac Sim 做仿真。",
                &["Unitree G1", "Unitree Go2", "Qwen", "Isaac Sim"],
                &["Unitree"],
            ),
            (
                "The pipeline yields Co3O4 nanosheets; a robot arm transfers samples to the SEM.",
                &["机械臂", "SEM"],
                &["Inflection AI", "01.AI", "ARM", "OpenAI"],
            ),
            (
                "月之暗面发布 Kimi K2，智谱发布 GLM-4.5，字节豆包持续迭代。",
                &["月之暗面", "Zhipu", "字节跳动"],
                &["GLM-4.5", "Kimi K2"],
            ),
            (
                "现场部署了一套检测设备，配合 PyTorch框架 完成推理。",
                &["PyTorch"],
                &["PyTorch框架"],
            ),
            (
                "We fit a GLM to the ROS data using a minimax estimator; the XAI module explains the step-3 flux. \
                 Heat flux and momenta are plotted; the brain atlas was registered.",
                &[],
                &[],
            ),
            (
                "Magnetic field of 1 Tesla; the Apollo mission; a Falcon 9 launch; whisper quietly; the mistral wind; \
                 the cortex; a granite countertop; solar cells; palm oil; a snowflake dendrite.",
                &[],
                &[],
            ),
            (
                "Tesla Optimus and Boston Dynamics Atlas walked; Figure 02 lifted boxes; Spot inspected the plant.",
                &["Tesla Optimus", "Boston Dynamics Atlas", "Figure AI"],
                &[],
            ),
            (
                "The pipeline runs on ARM Cortex-A78; the robot arm and the arm of the chair; ct scan vs CT scan.",
                &["ARM", "机械臂", "CT"],
                &[],
            ),
        ];
        for (material, expected, absent) in samples {
            assert_eq!(
                found_terms(material),
                expected,
                "vocabulary hits for {material}"
            );
            let extracted = extract(material);
            assert_terms(&extracted, expected, absent);
            assert!(extracted.len() <= VISUAL_ASSET_SEARCH_TERM_LIMIT);
        }
        assert_terms(
            &extract("现场部署了一套检测设备，配合 PyTorch框架 完成推理。"),
            &["检测设备"],
            &[],
        );
    }
}
