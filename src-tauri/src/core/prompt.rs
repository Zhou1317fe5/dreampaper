use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::AppResult;

#[derive(Clone, Debug)]
pub struct PromptStore {
    root: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct PromptAsset {
    pub key: String,
    pub version: String,
    pub hash: String,
    pub content: String,
}

impl PromptStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn load(&self, key: &str) -> AppResult<PromptAsset> {
        let path = self.root.join(Path::new(key));
        let content = std::fs::read_to_string(path)?;
        let hash = short_hash(&content);
        Ok(PromptAsset {
            key: key.to_string(),
            version: hash.clone(),
            hash,
            content,
        })
    }
    pub fn load_all(&self, keys: &[&str]) -> AppResult<Vec<PromptAsset>> {
        keys.iter().map(|key| self.load(key)).collect()
    }
}

/// 拼 prompt：先按顺序铺 prompt 资产，再铺调用方给的段落。
/// 段落顺序有意义，因此用 Vec 而非 Map。移植自 `prompts.py::compose_prompt`。
pub fn compose_prompt(assets: &[PromptAsset], sections: &[(&str, String)]) -> String {
    let mut parts: Vec<String> = assets
        .iter()
        .map(|asset| format!("## {}\n{}", asset.key, asset.content.trim()))
        .collect();
    parts.extend(
        sections
            .iter()
            .map(|(name, value)| format!("## {name}\n{value}")),
    );
    parts.join("\n\n")
}

fn short_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    format!("{digest:x}").chars().take(16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// prompts/ 与 src-tauri/ 同级；打包时由 tauri resource 复制过去。
    fn prompt_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri has a parent")
            .join("prompts")
    }

    /// 对齐 Python `test_prompt_assets_load_and_compose`：
    /// 顺序即 prompt 段落顺序，version 必须等于内容 hash（改了文件就换版本号）。
    #[test]
    fn prompt_assets_load_and_compose() {
        let store = PromptStore::new(prompt_root());
        let keys = [
            "modes/paper_figure/diagram_rules.md",
            "modes/paper_figure/plot_rules.md",
            "modes/ppt_slide/master_rules.md",
        ];
        let assets = store.load_all(&keys).expect("prompt assets should load");
        let prompt = compose_prompt(&assets, &[("Output Contract", "{}".to_string())]);

        assert!(prompt.contains("PaperBanana"));
        assert!(prompt.contains("master_style_spec"));
        assert_eq!(
            assets.iter().map(|asset| asset.key.as_str()).collect::<Vec<_>>(),
            keys
        );
        assert!(assets
            .iter()
            .all(|asset| !asset.hash.is_empty() && asset.version == asset.hash));
        // 段落顺序：先 prompt 资产，后调用方段落
        assert!(prompt.find("## modes/paper_figure/diagram_rules.md")
            < prompt.find("## Output Contract"));
    }

    /// 缺文件要报错而不是静默拼出半截 prompt。
    #[test]
    fn missing_prompt_asset_is_an_error() {
        let store = PromptStore::new(prompt_root());
        assert!(store.load("modes/does_not_exist.md").is_err());
    }
}
