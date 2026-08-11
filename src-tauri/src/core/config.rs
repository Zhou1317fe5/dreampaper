use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

use super::store::Store;

const CONFIG_KEY: &str = "app_config";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppConfig {
    pub version: i64,
    pub active_design_profile: String,
    pub active_implement_profile: String,
    #[serde(default = "default_search_profile_id")]
    pub active_search_profile: String,
    pub proxy_url: Option<String>,
    pub ppt_page_plan_concurrency: Option<i64>,
    pub ppt_image_concurrency: Option<i64>,
    pub model_profiles: Vec<ModelProfile>,
}

fn default_search_profile_id() -> String {
    "search-default".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelProfile {
    pub id: String,
    pub role: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub api_version: Option<String>,
    pub headers: serde_json::Map<String, serde_json::Value>,
    pub timeout_seconds: i64,
    pub max_retries: i64,
    pub output_defaults: serde_json::Map<String, serde_json::Value>,
    pub has_api_key: Option<bool>,
    pub api_key_hint: Option<String>,
}

pub struct ConfigService<'a> {
    store: &'a Store,
}

impl<'a> ConfigService<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn get_config(&self) -> AppResult<AppConfig> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT value FROM config WHERE key = ?1")?;
        let result: Result<String, rusqlite::Error> =
            stmt.query_row(params![CONFIG_KEY], |row| row.get(0));
        match result {
            Ok(value) => {
                let mut config: AppConfig = serde_json::from_str(&value)?;
                ensure_search_profile(&mut config);
                Ok(public_config(config))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                let config = default_config();
                self.persist_config(&config)?;
                Ok(public_config(config))
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn save_config(&self, config: AppConfig) -> AppResult<AppConfig> {
        let existing = self.load_private_config()?;
        let normalized = normalize_config(config, Some(existing));
        self.persist_config(&normalized)?;
        Ok(public_config(normalized))
    }

    pub fn runtime_config(&self) -> AppResult<AppConfig> {
        let mut config = self.load_private_config()?;
        ensure_search_profile(&mut config);
        config.proxy_url = normalize_proxy_url(config.proxy_url);
        Ok(config)
    }

    pub fn active_profile<'c>(
        config: &'c AppConfig,
        role: &str,
    ) -> AppResult<&'c ModelProfile> {
        let active_id = match role {
            "design" => &config.active_design_profile,
            "implement" => &config.active_implement_profile,
            "search" => &config.active_search_profile,
            other => {
                return Err(AppError::new(
                    "invalid_role",
                    format!("Unknown model role: {other}"),
                ))
            }
        };
        config
            .model_profiles
            .iter()
            .find(|profile| &profile.id == active_id && profile.role == role)
            .or_else(|| {
                config
                    .model_profiles
                    .iter()
                    .find(|profile| profile.role == role)
            })
            .ok_or_else(|| {
                AppError::new(
                    "missing_profile",
                    format!("Missing active {role} model profile"),
                )
            })
    }

    fn load_private_config(&self) -> AppResult<AppConfig> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT value FROM config WHERE key = ?1")?;
        let result: Result<String, rusqlite::Error> =
            stmt.query_row(params![CONFIG_KEY], |row| row.get(0));
        match result {
            Ok(value) => Ok(serde_json::from_str(&value)?),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(default_config()),
            Err(error) => Err(error.into()),
        }
    }

    fn persist_config(&self, config: &AppConfig) -> AppResult<()> {
        let conn = self.store.connection()?;
        let value = serde_json::to_string_pretty(config)?;
        conn.execute(
            "INSERT INTO config(key, value, updated_at) VALUES (?1, ?2, ?3)\
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![CONFIG_KEY, value, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }
}

fn normalize_config(mut incoming: AppConfig, existing: Option<AppConfig>) -> AppConfig {
    incoming.proxy_url = normalize_proxy_url(incoming.proxy_url);
    incoming.ppt_page_plan_concurrency = normalize_concurrency(incoming.ppt_page_plan_concurrency);
    incoming.ppt_image_concurrency = normalize_concurrency(incoming.ppt_image_concurrency);
    for profile in &mut incoming.model_profiles {
        if profile.protocol == "banna2" {
            profile.protocol = "banana2".to_string();
        }
        if profile.api_key.as_deref().unwrap_or_default().trim().is_empty() {
            profile.api_key = existing
                .as_ref()
                .and_then(|config| config.model_profiles.iter().find(|item| item.id == profile.id))
                .and_then(|profile| profile.api_key.clone());
        }
        profile.has_api_key = None;
        profile.api_key_hint = None;
    }
    ensure_search_profile(&mut incoming);
    incoming
}

fn ensure_search_profile(config: &mut AppConfig) {
    if !config.model_profiles.iter().any(|item| item.role == "search") {
        config.model_profiles.push(default_search());
    }
    if config.active_search_profile.trim().is_empty() {
        config.active_search_profile = config
            .model_profiles
            .iter()
            .find(|item| item.role == "search")
            .map(|item| item.id.clone())
            .unwrap_or_else(default_search_profile_id);
    }
}

fn normalize_proxy_url(proxy_url: Option<String>) -> Option<String> {
    let value = proxy_url.unwrap_or_default().trim().to_string();
    if value.is_empty() {
        return None;
    }
    if value.contains("://") {
        Some(value)
    } else if value.chars().all(|ch| ch.is_ascii_digit()) {
        Some(format!("http://127.0.0.1:{value}"))
    } else {
        Some(format!("http://{value}"))
    }
}

fn normalize_concurrency(value: Option<i64>) -> Option<i64> {
    value.map(|number| number.clamp(1, 20))
}

fn public_config(mut config: AppConfig) -> AppConfig {
    for profile in &mut config.model_profiles {
        profile.api_key_hint = profile.api_key.as_deref().map(mask_key);
        profile.has_api_key = Some(profile.api_key.as_deref().is_some_and(|value| !value.trim().is_empty()));
        profile.api_key = None;
    }
    config
}

fn mask_key(value: &str) -> String {
    if value.len() >= 4 {
        format!("••••{}", &value[value.len() - 4..])
    } else {
        "••••".to_string()
    }
}

fn default_config() -> AppConfig {
    AppConfig {
        version: 1,
        active_design_profile: "design-default".to_string(),
        active_implement_profile: "implement-default".to_string(),
        active_search_profile: default_search_profile_id(),
        proxy_url: None,
        ppt_page_plan_concurrency: None,
        ppt_image_concurrency: None,
        model_profiles: vec![default_design(), default_implement(), default_search()],
    }
}

fn default_search() -> ModelProfile {
    let mut output_defaults = serde_json::Map::new();
    output_defaults.insert(
        "max_results".to_string(),
        serde_json::Value::String("3".to_string()),
    );
    ModelProfile {
        id: "search-default".to_string(),
        role: "search".to_string(),
        name: "Search model".to_string(),
        protocol: "duckduckgo_html".to_string(),
        base_url: "https://duckduckgo.com".to_string(),
        model: "duckduckgo-html".to_string(),
        api_key: None,
        api_version: None,
        headers: serde_json::Map::new(),
        timeout_seconds: 15,
        max_retries: 1,
        output_defaults,
        has_api_key: Some(false),
        api_key_hint: None,
    }
}

fn default_design() -> ModelProfile {
    ModelProfile {
        id: "design-default".to_string(),
        role: "design".to_string(),
        name: "Design model".to_string(),
        protocol: "openai_responses".to_string(),
        base_url: "https://api.openai.com".to_string(),
        model: "gpt-5.4".to_string(),
        api_key: None,
        api_version: None,
        headers: serde_json::Map::new(),
        timeout_seconds: 120,
        max_retries: 2,
        output_defaults: serde_json::Map::new(),
        has_api_key: Some(false),
        api_key_hint: None,
    }
}

fn default_implement() -> ModelProfile {
    let mut output_defaults = serde_json::Map::new();
    for (key, value) in [
        ("size", "1200x675"),
        ("quality", "auto"),
        ("output_format", "png"),
        ("response_format", "url"),
        ("aspect_ratio", "16:9"),
        ("image_size", "4K"),
        ("thinking_level", "high"),
        ("mime_type", "image/png"),
    ] {
        output_defaults.insert(key.to_string(), serde_json::Value::String(value.to_string()));
    }
    ModelProfile {
        id: "implement-default".to_string(),
        role: "implement".to_string(),
        name: "Implement model".to_string(),
        protocol: "image2".to_string(),
        base_url: "https://api.openai.com".to_string(),
        model: "gpt-image-2".to_string(),
        api_key: None,
        api_version: None,
        headers: serde_json::Map::new(),
        timeout_seconds: 600,
        max_retries: 3,
        output_defaults,
        has_api_key: Some(false),
        api_key_hint: None,
    }
}
