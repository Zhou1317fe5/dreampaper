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
