use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Emitter};

use crate::core::asset::AssetService;
use crate::core::config::ConfigService;
use crate::core::doc::extract_material_text;
use crate::core::job::{JobImage, JobService};
use crate::core::model::design::ImageInput;
use crate::core::net::{decode_b64, encode_b64};
use crate::core::pipeline::figure::{self, FigureRun, PaperFigurePayload};
use crate::core::pipeline::slide::{
    compose_material_context, validate_payload, MaterialAsset, PptSlidePayload, SlideRun,
};
use crate::core::tpl::TemplateService;
use crate::core::Core;
use crate::error::{AppError, AppResult};
use crate::event::JobEventPayload;

const MAX_FIGURE_TEMPLATES: usize = 3;

pub fn spawn(app: AppHandle, core: Arc<Core>, job_id: String) {
    let cancelled = Arc::new(AtomicBool::new(false));
    let handle = tauri::async_runtime::spawn({
        let core = Arc::clone(&core);
        let cancelled = Arc::clone(&cancelled);
        let job_id = job_id.clone();
        async move {
            let emit = |stage: &str, message: &str, status: &str| {
                let _ = app.emit(
                    "job://stage",
                    JobEventPayload {
                        job_id: job_id.clone(),
                        status: status.to_string(),
                        stage: stage.to_string(),
                        message: message.to_string(),
                        timestamp: Utc::now(),
                    },
                );
            };
            let sink = |stage: &str, message: &str| {
                let _ = JobService::new(&core.store).mark_stage(&job_id, stage, message, "running");
                emit(stage, message, "running");
            };

            let outcome = run(&core, &job_id, &sink).await;
            if cancelled.load(Ordering::SeqCst) {
                return;
            }
            match outcome {
                Ok(images) => {
                    let _ = JobService::new(&core.store).finish(&job_id, &images);
                    emit("completed", "任务完成", "succeeded");
                }
                Err(error) => {
                    let _ = JobService::new(&core.store).fail(&job_id, &error.message);
                    emit("failed", &error.message, "failed");
                }
            }
            core.cancels.finish(&job_id);
        }
    });
    core.cancels
        .register(&job_id, cancelled, move || handle.abort());
}

async fn run(
    core: &Core,
    job_id: &str,
    stage: &(dyn Fn(&str, &str) + Send + Sync),
) -> AppResult<Vec<JobImage>> {
    let envelope = JobService::new(&core.store).payload(job_id)?;
    let mode = envelope
        .get("mode")
        .and_then(|value| value.as_str())
        .unwrap_or("paper_figure")
        .to_string();
    let payload = envelope
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let config = ConfigService::new(&core.store).runtime_config()?;
    let design_profile = ConfigService::active_profile(&config, "design")?.clone();
    let implement_profile = ConfigService::active_profile(&config, "implement")?.clone();

    match mode.as_str() {
        "paper_figure" => {
            let payload: PaperFigurePayload = serde_json::from_value(payload)?;
            stage("paper_validate", "校验 Figure 输入");
            figure::validate_payload(&payload)?;

            stage("paper_templates", "读取 template 和 few-shot 参考");
            let templates = TemplateService::new(&core.store, &core.app_data);
            let mut images = Vec::new();
            let mut metadata = Vec::new();
            for id in payload.template_ids.iter().take(MAX_FIGURE_TEMPLATES) {
                let detail = templates.template_detail(id)?;
                metadata.push(detail.metadata());
                images.push(read_image_input(
                    &detail.image_path,
                    &detail.mime_type,
                )?);
            }

            let run = FigureRun {
                prompts: &core.prompts,
                config: &config,
                design_profile: &design_profile,
                implement_profile: &implement_profile,
                template_images: images,
                template_metadata: serde_json::Value::Array(metadata),
            };
            let image_b64 = run.run(&payload, stage).await?;
            Ok(vec![save_image(core, job_id, "figure.png", &image_b64)?])
        }
        "ppt_slide" => {
            let payload: PptSlidePayload = serde_json::from_value(payload)?;
            stage("ppt_validate", "校验 Slide 输入");
            validate_payload(&payload)?;
            let search_profile = ConfigService::active_profile(&config, "search")?.clone();

            let assets = AssetService::new(&core.store, &core.app_data);
            stage("ppt_template", "读取 template 图片");
            let template_image = if let Some(template_id) = payload.template_ref() {
                let templates = TemplateService::new(&core.store, &core.app_data);
                let detail = templates.template_detail(template_id)?;
                read_image_input(&detail.image_path, &detail.mime_type)?
            } else {
                let template = assets.asset_file(&payload.template_asset_id)?;
                read_image_input(&template.path, &template.mime_type)?
            };

            stage("ppt_material", "整理资料输入");
            let mut material_assets = Vec::new();
            for id in &payload.material_asset_ids {
                material_assets.push(material_summary(&assets, id)?);
            }
            let material_context =
                compose_material_context(&payload.material_text, &material_assets);

            let run = SlideRun {
                prompts: &core.prompts,
                config: &config,
                design_profile: &design_profile,
                implement_profile: &implement_profile,
                search_profile: &search_profile,
                template_image,
                material_context,
            };
            let pages = run.run(&payload, stage).await?;
            stage("ppt_save", "保存生成图片");
            pages
                .iter()
                .enumerate()
                .map(|(index, image_b64)| {
                    save_image(core, job_id, &format!("slide_{}.png", index + 1), image_b64)
                })
                .collect()
        }
        other => Err(AppError::new(
            "invalid_mode",
            format!("Unsupported job mode: {other}"),
        )),
    }
}

fn read_image_input(path: &std::path::Path, mime_type: &str) -> AppResult<ImageInput> {
    let bytes = std::fs::read(path).map_err(|error| {
        AppError::new(
            "template_image_read_failed",
            format!("Failed to read template image {}: {error}", path.display()),
        )
    })?;
    Ok(ImageInput {
        filename: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("template.png")
            .to_string(),
        mime_type: mime_type.to_string(),
        b64: encode_b64(&bytes),
    })
}

fn material_summary(assets: &AssetService<'_>, asset_id: &str) -> AppResult<MaterialAsset> {
    let asset = assets.asset_file(asset_id)?;
    let (text, parser) = extract_material_text(&asset.path);
    let bytes = std::fs::metadata(&asset.path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    Ok(MaterialAsset::new(
        asset.filename,
        asset.mime_type,
        bytes,
        parser,
        &text,
    ))
}

fn save_image(core: &Core, job_id: &str, name: &str, image_b64: &str) -> AppResult<JobImage> {
    let bytes = decode_b64(image_b64)?;
    let saved =
        AssetService::new(&core.store, &core.app_data).save_job_image(job_id, name, &bytes)?;
    Ok(JobImage {
        name: name.to_string(),
        url: saved.url,
    })
}
