//! 检索客户端，移植自 `backend/app/search.py`。
//!
//! 只回传标题/URL/摘要三段文本，不下载任何网络图片——这些文本是 design model
//! 判断「主体长什么样」的唯一依据，implement model 永远拿不到网络图。

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::config::ModelProfile;
use crate::core::net::{
    build_client, model_error, normalize_base_url, post_json_with_retries, require_api_key,
};
use crate::error::AppResult;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub struct SearchClient;

impl SearchClient {
    pub async fn search(
        profile: &ModelProfile,
        query: &str,
        max_results: usize,
        proxy_url: Option<&str>,
    ) -> AppResult<Vec<SearchResult>> {
        let protocol = profile.protocol.trim().to_lowercase();
        match protocol.as_str() {
            "duckduckgo_html" => Self::duckduckgo(profile, query, max_results, proxy_url).await,
            "tavily" => Self::tavily(profile, query, max_results, proxy_url).await,
            // openai_responses 也走 chat 兼容路径
            "openai_chat" | "openai_responses" => {
                Self::model_search(profile, query, max_results, proxy_url).await
            }
            other => Err(model_error(format!("Unsupported search protocol: {other}"))),
        }
    }

    async fn duckduckgo(
        profile: &ModelProfile,
        query: &str,
        max_results: usize,
        proxy_url: Option<&str>,
    ) -> AppResult<Vec<SearchResult>> {
        let base = if profile.base_url.trim().is_empty() {
            "https://duckduckgo.com".to_string()
        } else {
            profile.base_url.trim_end_matches('/').to_string()
        };
        let encoded = urlencode(query);
        let url = if base.ends_with("/html") {
            format!("{base}/?q={encoded}")
        } else {
            format!("{base}/html/?q={encoded}")
        };
        let timeout = profile.timeout_seconds.max(1) as u64;
        let client = build_client(timeout, proxy_url)?;
        let mut request = client
            .get(&url)
            .header("User-Agent", "Mozilla/5.0 dreampaper visual asset search");
        for (key, value) in &profile.headers {
            if let Some(text) = value.as_str() {
                request = request.header(key.as_str(), text);
            }
        }
        let response = request
            .send()
            .await
            .map_err(|error| model_error(format!("DuckDuckGo search failed: {error}")))?;
        let status = response.status().as_u16();
        if status >= 400 {
            return Err(model_error(format!("DuckDuckGo search failed: HTTP {status}")));
        }
        let body = response
            .text()
            .await
            .map_err(|error| model_error(format!("DuckDuckGo search failed: {error}")))?;
        Ok(Self::parse_duckduckgo_html(&body, max_results))
    }

    async fn tavily(
        profile: &ModelProfile,
        query: &str,
        max_results: usize,
        proxy_url: Option<&str>,
    ) -> AppResult<Vec<SearchResult>> {
        let base = if profile.base_url.trim().is_empty() {
            "https://api.tavily.com".to_string()
        } else {
            profile.base_url.trim_end_matches('/').to_string()
        };
        let depth = profile
            .output_defaults
            .get("search_depth")
            .and_then(|value| value.as_str())
            .unwrap_or("basic");
        let payload = json!({
            "api_key": require_api_key(profile)?,
            "query": query,
            "max_results": max_results,
            "include_answer": false,
            "search_depth": depth
        });
        let outcome = post_json_with_retries(
            profile,
            &format!("{base}/search"),
            &payload,
            vec![],
            None,
            proxy_url,
            None,
        )
        .await?;
        if outcome.status >= 400 {
            return Err(model_error(format!(
                "Tavily search failed: HTTP {} {}",
                outcome.status,
                outcome.body.chars().take(200).collect::<String>()
            )));
        }
        let data: Value = serde_json::from_str(&outcome.body)
            .map_err(|error| model_error(format!("Tavily 响应不是 JSON: {error}")))?;
        let mut results = Vec::new();
        if let Some(items) = data["results"].as_array() {
            for item in items.iter().take(max_results) {
                results.push(SearchResult {
                    title: item["title"].as_str().unwrap_or_default().trim().to_string(),
                    url: item["url"].as_str().unwrap_or_default().trim().to_string(),
                    snippet: item["content"]
                        .as_str()
                        .or_else(|| item["snippet"].as_str())
                        .unwrap_or_default()
                        .chars()
                        .take(500)
                        .collect(),
                });
            }
        }
        Ok(results)
    }

    /// 带联网能力的 OpenAI 兼容 chat 接口（如 Grok 中转），要求返回严格 JSON。
    async fn model_search(
        profile: &ModelProfile,
        query: &str,
        max_results: usize,
        proxy_url: Option<&str>,
    ) -> AppResult<Vec<SearchResult>> {
        let base = normalize_base_url(
            if profile.base_url.trim().is_empty() {
                "https://api.openai.com"
            } else {
                &profile.base_url
            },
            "openai_chat",
        );
        let system = format!(
            "You are a web search assistant for academic slide visual grounding. \
             Return strict JSON only: {{\"results\":[{{\"title\":\"\",\"url\":\"\",\"snippet\":\"\"}}]}}. \
             Return at most {max_results} results about official product/tool appearance or logo description. \
             Prefer official docs and product pages. No markdown fences."
        );
        let model = if profile.model.trim().is_empty() {
            "gpt-4o-mini"
        } else {
            &profile.model
        };
        let payload = json!({
            "model": model,
            "temperature": 0.1,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": format!("Search query: {query}")}
            ]
        });
        let outcome = post_json_with_retries(
            profile,
            &format!("{base}/chat/completions"),
            &payload,
            vec![(
                "Authorization".to_string(),
                format!("Bearer {}", require_api_key(profile)?),
            )],
            None,
            proxy_url,
            None,
        )
        .await?;
        if outcome.status >= 400 {
            return Err(model_error(format!(
                "Search model failed: HTTP {} {}",
                outcome.status,
                outcome.body.chars().take(200).collect::<String>()
            )));
        }
        let data: Value = serde_json::from_str(&outcome.body)
            .map_err(|error| model_error(format!("Search model 响应不是 JSON: {error}")))?;
        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default();
        let mut results = Self::parse_json_results(content)?;
        results.truncate(max_results);
        Ok(results)
    }

    fn parse_json_results(content: &str) -> AppResult<Vec<SearchResult>> {
        let value = crate::core::net::parse_json_response(content)?;
        let items = if value.is_array() {
            value.as_array().cloned().unwrap_or_default()
        } else {
            value["results"]
                .as_array()
                .cloned()
                .ok_or_else(|| model_error("Search model JSON missing results list"))?
        };
        Ok(items
            .iter()
            .filter(|item| item.is_object())
            .map(|item| SearchResult {
                title: item["title"].as_str().unwrap_or_default().trim().to_string(),
                url: item["url"].as_str().unwrap_or_default().trim().to_string(),
                snippet: item["snippet"]
                    .as_str()
                    .or_else(|| item["content"].as_str())
                    .unwrap_or_default()
                    .chars()
                    .take(500)
                    .collect(),
            })
            .collect())
    }

    pub fn parse_duckduckgo_html(html_text: &str, max_results: usize) -> Vec<SearchResult> {
        let link_pattern = Regex::new(
            r#"(?is)<a[^>]+class="[^"]*result__a[^"]*"[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#,
        )
        .expect("valid regex");
        let snippet_pattern = Regex::new(
            r#"(?is)<a[^>]+class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>|<td[^>]+class="[^"]*result-snippet[^"]*"[^>]*>(.*?)</td>"#,
        )
        .expect("valid regex");

        let snippets: Vec<String> = snippet_pattern
            .captures_iter(html_text)
            .map(|caps| {
                let raw = caps
                    .get(1)
                    .or_else(|| caps.get(2))
                    .map(|m| m.as_str())
                    .unwrap_or_default();
                clean_html(raw)
            })
            .collect();

        let mut results = Vec::new();
        for (index, caps) in link_pattern.captures_iter(html_text).take(max_results).enumerate() {
            let href = unescape_html(caps.get(1).map(|m| m.as_str()).unwrap_or_default());
            let url = normalize_result_url(&href);
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                continue;
            }
            results.push(SearchResult {
                title: clean_html(caps.get(2).map(|m| m.as_str()).unwrap_or_default()),
                url,
                snippet: snippets.get(index).cloned().unwrap_or_default(),
            });
        }
        results
    }
}

/// DuckDuckGo 的跳转链接把真实地址放在 uddg 参数里，需还原。
fn normalize_result_url(url: &str) -> String {
    let Some((_, query)) = url.split_once('?') else {
        return url.to_string();
    };
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("uddg=") {
            return urldecode(value);
        }
    }
    url.to_string()
}

fn clean_html(value: &str) -> String {
    let tags = Regex::new(r"<[^>]+>").expect("valid regex");
    let spaces = Regex::new(r"\s+").expect("valid regex");
    let text = tags.replace_all(value, " ");
    let text = unescape_html(&text);
    spaces.replace_all(&text, " ").trim().to_string()
}

fn unescape_html(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn urlencode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            b' ' => encoded.push('+'),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

fn urldecode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duckduckgo_results_and_unwraps_redirect() {
        let html = r#"
        <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.docker.com%2Fcompany%2Fnewsroom%2Fmedia-resources%2F">Docker Media Resources</a>
        <a class="result__snippet">Official Docker logos and brand resources.</a>
        "#;
        let results = SearchClient::parse_duckduckgo_html(html, 3);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://www.docker.com/company/newsroom/media-resources/");
        assert_eq!(results[0].title, "Docker Media Resources");
        assert!(results[0].snippet.contains("Official Docker logos"));
    }

    #[test]
    fn parses_model_search_json() {
        let content = "```json\n{\"results\":[{\"title\":\"T\",\"url\":\"https://a.b\",\"snippet\":\"S\"}]}\n```";
        let results = SearchClient::parse_json_results(content).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://a.b");
    }

    #[test]
    fn encodes_chinese_query() {
        assert_eq!(urlencode("离心机 实物"), "%E7%A6%BB%E5%BF%83%E6%9C%BA+%E5%AE%9E%E7%89%A9");
        assert_eq!(urldecode("%E7%A6%BB%E5%BF%83%E6%9C%BA"), "离心机");
    }
}
