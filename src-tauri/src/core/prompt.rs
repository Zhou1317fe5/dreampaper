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
}

fn short_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    format!("{digest:x}").chars().take(16).collect()
}
