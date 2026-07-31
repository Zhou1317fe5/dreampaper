pub mod asset;
pub mod config;
pub mod doc;
pub mod job;
pub mod model;
pub mod net;
pub mod prompt;
pub mod search;
pub mod store;
pub mod tpl;

use std::path::PathBuf;

use crate::error::AppResult;

use self::asset::{AssetFile, AssetService};
use self::config::{AppConfig, ConfigService};
use self::doc::{DocumentChunkHit, DocumentService, DocumentSummary};
use self::job::{JobRecord, JobService};
use self::prompt::PromptStore;
use self::store::Store;
use self::tpl::{TemplateFile, TemplatePackSummary, TemplateService, TemplateSummary};

pub struct Core {
    pub app_data: PathBuf,
    pub store: Store,
    pub prompts: PromptStore,
}

impl Core {
    pub fn new(app_data: PathBuf, prompt_root: PathBuf) -> AppResult<Self> {
        let store = Store::initialize(&app_data)?;
        Ok(Self {
            app_data,
            store,
            prompts: PromptStore::new(prompt_root),
        })
    }

    pub fn get_config(&self) -> AppResult<AppConfig> {
        ConfigService::new(&self.store).get_config()
    }

    pub fn save_config(&self, config: AppConfig) -> AppResult<AppConfig> {
        ConfigService::new(&self.store).save_config(config)
    }

    pub fn import_asset(
        &self,
        filename: String,
        mime_type: String,
        bytes: Vec<u8>,
    ) -> AppResult<asset::AssetUpload> {
        AssetService::new(&self.store, &self.app_data).import_asset(filename, mime_type, bytes)
    }

    pub fn asset_file(&self, id: &str) -> AppResult<AssetFile> {
        AssetService::new(&self.store, &self.app_data).asset_file(id)
    }

    pub fn list_templates(&self, kind: String, query: String) -> AppResult<Vec<TemplateSummary>> {
        TemplateService::new(&self.store, &self.app_data).list_templates(kind, query)
    }

    pub fn import_template_image(
        &self,
        filename: String,
        mime_type: String,
        bytes: Vec<u8>,
        kind: String,
        category: Option<String>,
        visual_intent: Option<String>,
        content_summary: Option<String>,
    ) -> AppResult<TemplateSummary> {
        TemplateService::new(&self.store, &self.app_data).import_template_image(
            filename,
            mime_type,
            bytes,
            kind,
            category,
            visual_intent,
            content_summary,
        )
    }

    pub fn import_template_pack(&self, path: String) -> AppResult<TemplatePackSummary> {
        TemplateService::new(&self.store, &self.app_data).import_template_pack(path)
    }

    pub fn template_file(&self, id: &str) -> AppResult<TemplateFile> {
        TemplateService::new(&self.store, &self.app_data).template_file(id)
    }

    pub fn import_document(&self, path: String) -> AppResult<DocumentSummary> {
        DocumentService::new(&self.store, &self.app_data).import_document(path)
    }

    pub fn import_document_asset(&self, asset_id: String) -> AppResult<DocumentSummary> {
        let asset = self.asset_file(&asset_id)?;
        DocumentService::new(&self.store, &self.app_data)
            .import_document(asset.path.to_string_lossy().to_string())
    }

    pub fn search_documents(
        &self,
        query: String,
        limit: usize,
    ) -> AppResult<Vec<DocumentChunkHit>> {
        DocumentService::new(&self.store, &self.app_data).search_documents(query, limit)
    }

    pub fn create_job(&self, payload: serde_json::Value) -> AppResult<JobRecord> {
        JobService::new(&self.store).create_job(payload)
    }

    pub fn get_job(&self, id: String) -> AppResult<JobRecord> {
        JobService::new(&self.store).get_job(id)
    }

    pub fn list_jobs(&self, limit: usize, offset: usize) -> AppResult<Vec<JobRecord>> {
        JobService::new(&self.store).list_jobs(limit, offset)
    }
}
