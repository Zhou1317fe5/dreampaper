use std::path::PathBuf;
use std::sync::Arc;

use tauri::path::BaseDirectory;
use tauri::Manager;

use crate::core::Core;
use crate::error::{AppError, AppResult};

pub struct AppState {
    core: Arc<Core>,
}

impl AppState {
    pub fn new(app: &tauri::AppHandle) -> AppResult<Self> {
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|error| AppError::new("path_error", error.to_string()))?;
        let prompt_root = app
            .path()
            .resolve("prompts", BaseDirectory::Resource)
            .unwrap_or_else(|_| fallback_prompt_root());
        Ok(Self {
            core: Arc::new(Core::new(app_data, prompt_root)?),
        })
    }

    pub fn core(&self) -> &Core {
        &self.core
    }
}

fn fallback_prompt_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for candidate in [cwd.join("prompts"), cwd.join("..").join("prompts")] {
        if candidate.exists() {
            return candidate;
        }
    }
    cwd.join("prompts")
}
