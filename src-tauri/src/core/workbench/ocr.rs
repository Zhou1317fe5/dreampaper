//! The optional OCR model package: download, verification, removal, status.
//!
//! The native engine (Rust sidecar + ONNX Runtime) ships with the app; only
//! the models are fetched on first use from the URLs pinned in `MANIFEST`
//! together with their exact size and SHA-256. The official det/rec archives
//! are downloaded as-is and verified, then the ONNX files and configs are
//! extracted and verified again against their own digests; the recogniser
//! dictionary is derived from the rec config and verified the same way. A
//! package counts as installed only after every file passed and
//! `installed.json` was written atomically, so a half download or a bad
//! extraction is never mistaken for a model. Downloads resume from a `.part`
//! file, can be cancelled, and report progress through the callback the
//! command layer turns into `ocr://progress` events.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

#[derive(Deserialize)]
pub struct OcrDownload {
    pub name: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileSource {
    /// A downloaded file used verbatim.
    Download { download: &'static str },
    /// One member of a downloaded (uncompressed) tar archive.
    TarMember {
        download: &'static str,
        member: &'static str,
    },
    /// `PostProcess.character_dict` of an extracted PaddleX `inference.yml`,
    /// one entry per line with a trailing newline.
    CharacterDict { file: &'static str },
}

#[derive(Deserialize)]
pub struct OcrFile {
    pub name: &'static str,
    pub source: FileSource,
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Deserialize)]
pub struct OcrManifest {
    pub version: &'static str,
    pub license: &'static str,
    pub det_model: &'static str,
    pub cls_model: &'static str,
    pub rec_model: &'static str,
    pub downloads: Vec<OcrDownload>,
    pub files: Vec<OcrFile>,
}

pub static MANIFEST: std::sync::LazyLock<OcrManifest> = std::sync::LazyLock::new(|| {
    serde_json::from_str(include_str!("manifest.json")).expect("内置 OCR 清单无效")
});

/// The files the sidecar is started with, in the order its arguments expect.
pub const MODEL_FILES: [&str; 4] = ["det.onnx", "cls.onnx", "rec.onnx", "dict.txt"];

#[derive(Clone, Debug, Serialize)]
pub struct OcrProgress {
    pub file: String,
    pub received: u64,
    pub total: u64,
    pub overall_received: u64,
    pub overall_total: u64,
    /// `downloading` | `verifying` | `done` | `failed` | `cancelled`
    pub state: String,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OcrModelFile {
    pub name: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct OcrPackageStatus {
    pub installed: bool,
    pub version: String,
    pub download_bytes: u64,
    pub installed_bytes: u64,
    pub directory: String,
    pub downloading: bool,
    pub progress: Option<OcrProgress>,
    pub error: Option<String>,
    pub det_model_name: String,
    pub cls_model_name: String,
    pub rec_model_name: String,
    pub files: Vec<OcrModelFile>,
    pub license: String,
    pub sources: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct InstalledMarker {
    version: String,
    files: Vec<InstalledFile>,
    installed_at: String,
}

#[derive(Serialize, Deserialize)]
struct InstalledFile {
    name: String,
    bytes: u64,
    sha256: String,
}

struct Install {
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<OcrProgress>>,
    running: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
}

#[derive(Default)]
pub struct OcrPackages {
    install: Mutex<Option<Install>>,
}

pub fn package_dir(app_data: &Path) -> PathBuf {
    app_data.join("ocr").join(MANIFEST.version)
}

fn marker_path(app_data: &Path) -> PathBuf {
    package_dir(app_data).join("installed.json")
}

impl OcrPackages {
    pub fn status(&self, app_data: &Path) -> OcrPackageStatus {
        let dir = package_dir(app_data);
        let installed = is_installed(app_data);
        let installed_bytes = if installed {
            MANIFEST
                .files
                .iter()
                .filter_map(|file| std::fs::metadata(dir.join(file.name)).ok())
                .map(|meta| meta.len())
                .sum()
        } else {
            0
        };
        let guard = self
            .install
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (downloading, progress, error) = match guard.as_ref() {
            Some(install) => (
                install.running.load(Ordering::SeqCst),
                Some(
                    install
                        .progress
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone(),
                ),
                install
                    .error
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone(),
            ),
            None => (false, None, None),
        };
        OcrPackageStatus {
            installed,
            version: MANIFEST.version.to_string(),
            download_bytes: MANIFEST.downloads.iter().map(|file| file.bytes).sum(),
            installed_bytes,
            directory: dir.to_string_lossy().to_string(),
            downloading,
            progress,
            error,
            det_model_name: MANIFEST.det_model.to_string(),
            cls_model_name: MANIFEST.cls_model.to_string(),
            rec_model_name: MANIFEST.rec_model.to_string(),
            files: MANIFEST
                .files
                .iter()
                .map(|file| OcrModelFile {
                    name: file.name.to_string(),
                    bytes: file.bytes,
                })
                .collect(),
            license: MANIFEST.license.to_string(),
            sources: MANIFEST
                .downloads
                .iter()
                .map(|file| file.url.to_string())
                .collect(),
        }
    }

    /// Path of an installed model file, by manifest name only. Never resolves
    /// arbitrary paths: the sidecar is started from paths produced here.
    pub fn installed_file(&self, app_data: &Path, name: &str) -> AppResult<PathBuf> {
        if !is_installed(app_data) {
            return Err(AppError::new("ocr_not_installed", "OCR 组件尚未安装"));
        }
        let file = MANIFEST
            .files
            .iter()
            .find(|file| file.name == name)
            .ok_or_else(|| AppError::new("ocr_file_unknown", "未知的 OCR 组件文件"))?;
        Ok(package_dir(app_data).join(file.name))
    }

    /// The four files the sidecar loads, in argument order.
    pub fn model_paths(&self, app_data: &Path) -> AppResult<[PathBuf; 4]> {
        Ok([
            self.installed_file(app_data, MODEL_FILES[0])?,
            self.installed_file(app_data, MODEL_FILES[1])?,
            self.installed_file(app_data, MODEL_FILES[2])?,
            self.installed_file(app_data, MODEL_FILES[3])?,
        ])
    }

    /// Start the download in the background. `emit` receives every progress
    /// update, including the terminal one.
    pub fn start_install(
        &self,
        app_data: PathBuf,
        proxy_url: Option<String>,
        emit: impl Fn(OcrProgress) + Send + Sync + 'static,
    ) -> AppResult<()> {
        let mut guard = self
            .install
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if guard
            .as_ref()
            .is_some_and(|install| install.running.load(Ordering::SeqCst))
        {
            return Err(AppError::new("ocr_install_running", "OCR 组件正在下载中"));
        }
        if is_installed(&app_data) {
            return Err(AppError::new("ocr_already_installed", "OCR 组件已安装"));
        }
        let overall_total: u64 = MANIFEST.downloads.iter().map(|file| file.bytes).sum();
        let progress = Arc::new(Mutex::new(OcrProgress {
            file: MANIFEST.downloads[0].name.to_string(),
            received: 0,
            total: MANIFEST.downloads[0].bytes,
            overall_received: 0,
            overall_total,
            state: "downloading".into(),
            message: None,
        }));
        let install = Install {
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::clone(&progress),
            running: Arc::new(AtomicBool::new(true)),
            error: Arc::new(Mutex::new(None)),
        };
        let cancel = Arc::clone(&install.cancel);
        let running = Arc::clone(&install.running);
        let error_slot = Arc::clone(&install.error);
        *guard = Some(install);
        drop(guard);

        tauri::async_runtime::spawn(async move {
            let emit = Arc::new(emit);
            let report = {
                let progress = Arc::clone(&progress);
                let emit = Arc::clone(&emit);
                move |update: OcrProgress| {
                    *progress.lock().unwrap_or_else(|p| p.into_inner()) = update.clone();
                    emit(update);
                }
            };
            let result = run_install(&app_data, proxy_url.as_deref(), &cancel, &report).await;
            let mut final_state = progress.lock().unwrap_or_else(|p| p.into_inner()).clone();
            match result {
                Ok(()) => {
                    final_state.state = "done".into();
                    final_state.overall_received = final_state.overall_total;
                    final_state.received = final_state.total;
                }
                Err(error) if error.code == "ocr_install_cancelled" => {
                    final_state.state = "cancelled".into();
                    final_state.message = Some(error.message.clone());
                }
                Err(error) => {
                    final_state.state = "failed".into();
                    final_state.message = Some(error.message.clone());
                    *error_slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(error.message);
                }
            }
            running.store(false, Ordering::SeqCst);
            *progress.lock().unwrap_or_else(|p| p.into_inner()) = final_state.clone();
            emit(final_state);
        });
        Ok(())
    }

    pub fn cancel_install(&self) -> AppResult<()> {
        let guard = self
            .install
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        match guard.as_ref() {
            Some(install) if install.running.load(Ordering::SeqCst) => {
                install.cancel.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(AppError::new("ocr_install_idle", "当前没有进行中的下载")),
        }
    }

    /// Delete the package. The marker goes first so a failure part-way never
    /// leaves an "installed" package with files missing; the caller must have
    /// stopped the sidecar before calling this.
    pub fn remove(&self, app_data: &Path) -> AppResult<()> {
        if self
            .install
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .is_some_and(|install| install.running.load(Ordering::SeqCst))
        {
            return Err(AppError::new(
                "ocr_install_running",
                "OCR 组件正在下载中，请先取消",
            ));
        }
        let dir = package_dir(app_data);
        match std::fs::remove_file(marker_path(app_data)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(AppError::new(
                    "ocr_remove_failed",
                    format!("删除 OCR 组件失败：{error}"),
                ))
            }
        }
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|error| {
                AppError::new("ocr_remove_failed", format!("删除 OCR 组件失败：{error}"))
            })?;
        }
        let mut guard = self
            .install
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *guard = None;
        Ok(())
    }
}

/// Installed means the marker exists, names this manifest version, and every
/// file is present at its exact size. Hashes were verified at install time;
/// re-hashing 130 MiB on every status call is not worth it.
pub fn is_installed(app_data: &Path) -> bool {
    let Ok(raw) = std::fs::read(marker_path(app_data)) else {
        return false;
    };
    let Ok(marker) = serde_json::from_slice::<InstalledMarker>(&raw) else {
        return false;
    };
    if marker.version != MANIFEST.version {
        return false;
    }
    let dir = package_dir(app_data);
    MANIFEST.files.iter().all(|file| {
        marker
            .files
            .iter()
            .any(|f| f.name == file.name && f.sha256 == file.sha256)
            && std::fs::metadata(dir.join(file.name)).is_ok_and(|meta| meta.len() == file.bytes)
    })
}

async fn run_install(
    app_data: &Path,
    proxy_url: Option<&str>,
    cancel: &AtomicBool,
    report: &(dyn Fn(OcrProgress) + Send + Sync),
) -> AppResult<()> {
    let dir = package_dir(app_data);
    std::fs::create_dir_all(&dir)?;
    let overall_total: u64 = MANIFEST.downloads.iter().map(|file| file.bytes).sum();
    let mut overall_done = 0u64;
    let client = crate::core::net::build_client(6 * 3600, proxy_url)?;

    for download in &MANIFEST.downloads {
        let target = dir.join(download.name);
        let part = dir.join(format!("{}.part", download.name));
        if std::fs::metadata(&target).is_ok_and(|meta| meta.len() == download.bytes)
            && sha256_file(&target)? == download.sha256
        {
            overall_done += download.bytes;
            continue;
        }
        download_file(
            &client,
            download,
            &part,
            overall_done,
            overall_total,
            cancel,
            report,
        )
        .await?;
        report(OcrProgress {
            file: download.name.to_string(),
            received: download.bytes,
            total: download.bytes,
            overall_received: overall_done + download.bytes,
            overall_total,
            state: "verifying".into(),
            message: None,
        });
        verify_download(&part, download)?;
        super::asset::rename_replace(&part, &target)?;
        overall_done += download.bytes;
    }

    report(OcrProgress {
        file: "models".into(),
        received: overall_total,
        total: overall_total,
        overall_received: overall_total,
        overall_total,
        state: "verifying".into(),
        message: None,
    });
    // Extraction and derivation are CPU-bound and touch >100 MiB; keep them
    // off the async runtime.
    let dir_for_blocking = dir.clone();
    tauri::async_runtime::spawn_blocking(move || materialize_files(&dir_for_blocking))
        .await
        .map_err(|error| AppError::new("ocr_install_failed", error.to_string()))??;

    commit_marker(app_data)?;
    // The archives were only needed to produce the verified files.
    for download in &MANIFEST.downloads {
        if !MANIFEST.files.iter().any(|file| file.name == download.name) {
            let _ = std::fs::remove_file(dir.join(download.name));
        }
    }
    Ok(())
}

/// Write `installed.json` for the current manifest. Only valid after every
/// file in `MANIFEST.files` was produced and verified.
pub(crate) fn commit_marker(app_data: &Path) -> AppResult<()> {
    let marker = InstalledMarker {
        version: MANIFEST.version.to_string(),
        files: MANIFEST
            .files
            .iter()
            .map(|file| InstalledFile {
                name: file.name.to_string(),
                bytes: file.bytes,
                sha256: file.sha256.to_string(),
            })
            .collect(),
        installed_at: chrono::Utc::now().to_rfc3339(),
    };
    super::asset::write_atomic(&marker_path(app_data), &serde_json::to_vec_pretty(&marker)?)
}

fn verify_download(part: &Path, download: &OcrDownload) -> AppResult<()> {
    let size = std::fs::metadata(part)?.len();
    if size != download.bytes {
        let _ = std::fs::remove_file(part);
        return Err(AppError::new(
            "ocr_size_mismatch",
            format!(
                "{} 下载大小不符（{size} ≠ {}），请重试",
                download.name, download.bytes
            ),
        ));
    }
    let digest = sha256_file(part)?;
    if digest != download.sha256 {
        let _ = std::fs::remove_file(part);
        return Err(AppError::new(
            "ocr_digest_mismatch",
            format!("{} 校验失败，文件可能被篡改或损坏，请重试", download.name),
        ));
    }
    Ok(())
}

/// Produce every manifest file from the verified downloads, checking each
/// result against its pinned size and digest before it is committed.
pub(crate) fn materialize_files(dir: &Path) -> AppResult<()> {
    for file in &MANIFEST.files {
        let target = dir.join(file.name);
        if std::fs::metadata(&target).is_ok_and(|meta| meta.len() == file.bytes)
            && sha256_file(&target)? == file.sha256
        {
            continue;
        }
        let bytes = match &file.source {
            FileSource::Download { download } => {
                let path = dir.join(download);
                if path == target {
                    // The download is the file: it was verified on arrival.
                    continue;
                }
                std::fs::read(path)?
            }
            FileSource::TarMember { download, member } => tar_member(&dir.join(download), member)?,
            FileSource::CharacterDict { file: source } => {
                let yml = std::fs::read_to_string(dir.join(source))?;
                character_dict(&yml)
                    .ok_or_else(|| {
                        AppError::new(
                            "ocr_dict_missing",
                            format!("{source} 中没有 character_dict"),
                        )
                    })?
                    .into_bytes()
            }
        };
        if bytes.len() as u64 != file.bytes {
            return Err(AppError::new(
                "ocr_size_mismatch",
                format!(
                    "{} 提取后大小不符（{} ≠ {}）",
                    file.name,
                    bytes.len(),
                    file.bytes
                ),
            ));
        }
        if super::asset::sha256_hex(&bytes) != file.sha256 {
            return Err(AppError::new(
                "ocr_digest_mismatch",
                format!("{} 提取后校验失败，模型包内容与清单不一致", file.name),
            ));
        }
        super::asset::write_atomic(&target, &bytes)?;
    }
    Ok(())
}

/// Read one member of an uncompressed tar by exact path.
fn tar_member(archive: &Path, member: &str) -> AppResult<Vec<u8>> {
    let file = std::fs::File::open(archive)?;
    let mut archive = tar::Archive::new(std::io::BufReader::new(file));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path.to_string_lossy() == member {
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
    }
    Err(AppError::new(
        "ocr_archive_member_missing",
        format!("模型包中缺少 {member}"),
    ))
}

/// Extract `PostProcess.character_dict` from a PaddleX `inference.yml`. The
/// list is emitted by PyYAML, so entries are plain, single-quoted (with `''`
/// escapes) or double-quoted scalars; nothing else appears in the file.
pub(crate) fn character_dict(yml: &str) -> Option<String> {
    let mut lines = yml.lines();
    lines.find(|line| line.trim_end() == "  character_dict:")?;
    let mut out = String::new();
    let mut count = 0usize;
    for line in lines {
        let Some(raw) = line.strip_prefix("  - ") else {
            break;
        };
        out.push_str(&yaml_scalar(raw));
        out.push('\n');
        count += 1;
    }
    (count > 0).then_some(out)
}

/// `lines()` already dropped the line break; nothing else may be trimmed
/// because plain entries such as U+3000 (ideographic space) are real tokens.
fn yaml_scalar(raw: &str) -> String {
    if let Some(inner) = raw
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return inner.replace("''", "'");
    }
    if let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return unescape_double_quoted(inner);
    }
    raw.to_string()
}

fn unescape_double_quoted(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('x') => push_code_point(&mut out, &mut chars, 2),
            Some('u') => push_code_point(&mut out, &mut chars, 4),
            Some('U') => push_code_point(&mut out, &mut chars, 8),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn push_code_point(out: &mut String, chars: &mut std::str::Chars<'_>, digits: usize) {
    let hex: String = chars.by_ref().take(digits).collect();
    match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
        Some(ch) => out.push(ch),
        None => out.push_str(&hex),
    }
}

async fn download_file(
    client: &reqwest::Client,
    file: &OcrDownload,
    part: &Path,
    overall_done: u64,
    overall_total: u64,
    cancel: &AtomicBool,
    report: &(dyn Fn(OcrProgress) + Send + Sync),
) -> AppResult<()> {
    use futures::StreamExt;
    use std::io::Write;

    let mut existing = std::fs::metadata(part).map(|meta| meta.len()).unwrap_or(0);
    if existing >= file.bytes {
        // A stale part of the wrong size cannot be trusted; restart it.
        let _ = std::fs::remove_file(part);
        existing = 0;
    }
    let mut request = client.get(file.url);
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let response = request.send().await.map_err(|error| {
        AppError::new(
            "ocr_download_failed",
            format!(
                "下载失败：{}",
                crate::core::net::describe_transport_error(&error)
            ),
        )
    })?;
    let status = response.status();
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
    if !(status.is_success() || resumed) {
        return Err(AppError::new(
            "ocr_download_failed",
            format!("下载失败：HTTP {}", status.as_u16()),
        ));
    }
    if existing > 0 && !resumed {
        let _ = std::fs::remove_file(part);
        existing = 0;
    }
    let expected_len = response.content_length();
    if let Some(len) = expected_len {
        if existing + len != file.bytes {
            return Err(AppError::new(
                "ocr_size_mismatch",
                format!(
                    "{} 的远端大小 {} 与清单 {} 不符，已停止",
                    file.name,
                    existing + len,
                    file.bytes
                ),
            ));
        }
    }

    let mut output = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(part)?;
    let mut received = existing;
    let mut last_report = std::time::Instant::now();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            return Err(AppError::new("ocr_install_cancelled", "下载已取消"));
        }
        let chunk = chunk.map_err(|error| {
            AppError::new(
                "ocr_download_failed",
                format!(
                    "下载中断：{}",
                    crate::core::net::describe_transport_error(&error)
                ),
            )
        })?;
        output.write_all(&chunk)?;
        received += chunk.len() as u64;
        if received > file.bytes {
            return Err(AppError::new(
                "ocr_size_mismatch",
                format!("{} 下载数据超过清单大小", file.name),
            ));
        }
        if last_report.elapsed().as_millis() >= 150 {
            last_report = std::time::Instant::now();
            report(OcrProgress {
                file: file.name.to_string(),
                received,
                total: file.bytes,
                overall_received: overall_done + received,
                overall_total,
                state: "downloading".into(),
                message: None,
            });
        }
    }
    output.sync_all()?;
    Ok(())
}

fn sha256_file(path: &Path) -> AppResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_consistent() {
        let total: u64 = MANIFEST.downloads.iter().map(|f| f.bytes).sum();
        assert_eq!(total, 139_347_772);
        for download in &MANIFEST.downloads {
            assert_eq!(download.sha256.len(), 64);
            assert!(download.url.starts_with("https://"));
        }
        for file in &MANIFEST.files {
            assert_eq!(file.sha256.len(), 64);
            match &file.source {
                FileSource::Download { download } | FileSource::TarMember { download, .. } => {
                    assert!(MANIFEST.downloads.iter().any(|d| d.name == *download));
                }
                FileSource::CharacterDict { file: source } => {
                    assert!(MANIFEST.files.iter().any(|f| f.name == *source));
                }
            }
        }
        for name in MODEL_FILES {
            assert!(MANIFEST.files.iter().any(|file| file.name == name));
        }
    }

    #[test]
    fn character_dict_handles_pyyaml_quoting() {
        let yml = "PostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '!'\n  - $\n  - ''''\n  - \\\n  - \"\\u4e2d\\t\"\n  - 中\n  - \u{3000}\n  - 🛅\nOther: 1\n";
        let dict = character_dict(yml).unwrap();
        assert_eq!(dict, "!\n$\n'\n\\\n中\t\n中\n\u{3000}\n🛅\n");
        assert!(character_dict("Global:\n  x: 1\n").is_none());
    }

    /// Runs against the real rec config when `DREAMPAPER_REC_YML` points at
    /// it; the derived dictionary must reproduce the pinned digest exactly.
    #[test]
    fn character_dict_reproduces_the_pinned_dictionary() {
        let Ok(path) = std::env::var("DREAMPAPER_REC_YML") else {
            return;
        };
        let yml = std::fs::read_to_string(path).unwrap();
        let dict = character_dict(&yml).unwrap();
        let pinned = MANIFEST
            .files
            .iter()
            .find(|file| file.name == "dict.txt")
            .unwrap();
        assert_eq!(dict.len() as u64, pinned.bytes);
        assert_eq!(
            super::super::asset::sha256_hex(dict.as_bytes()),
            pinned.sha256
        );
    }

    /// Network probe, only with `DREAMPAPER_NET_TEST=1`: every pinned source
    /// must answer a ranged GET through the app's own client (the ModelScope
    /// CDN returned 403 to reqwest before a User-Agent was set).
    #[test]
    fn pinned_sources_accept_the_app_client() {
        if std::env::var("DREAMPAPER_NET_TEST").as_deref() != Ok("1") {
            return;
        }
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let proxy = std::env::var("HTTPS_PROXY").ok();
            let client = crate::core::net::build_client(60, proxy.as_deref()).unwrap();
            for download in &MANIFEST.downloads {
                let response = client
                    .get(download.url)
                    .header(reqwest::header::RANGE, "bytes=0-1023")
                    .send()
                    .await
                    .unwrap();
                assert!(
                    response.status().is_success(),
                    "{} -> {}",
                    download.name,
                    response.status()
                );
            }
        });
    }

    #[test]
    fn tar_members_are_read_by_exact_path() {
        let dir = super::super::asset::tests::temp_dir("tar");
        let archive = dir.join("m.tar");
        {
            let mut builder = tar::Builder::new(std::fs::File::create(&archive).unwrap());
            let data = b"onnx-bytes";
            let mut header = tar::Header::new_ustar();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "pkg/inference.onnx", &data[..])
                .unwrap();
            builder.finish().unwrap();
        }
        assert_eq!(
            tar_member(&archive, "pkg/inference.onnx").unwrap(),
            b"onnx-bytes"
        );
        assert_eq!(
            tar_member(&archive, "pkg/other").unwrap_err().code,
            "ocr_archive_member_missing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_requires_marker_and_exact_sizes() {
        let dir = super::super::asset::tests::temp_dir("ocr");
        assert!(!is_installed(&dir));
        let pkg = package_dir(&dir);
        std::fs::create_dir_all(&pkg).unwrap();
        // A stray half-download is not an install.
        std::fs::write(pkg.join("det.tar.part"), b"xx").unwrap();
        assert!(!is_installed(&dir));
        // A marker without the files is not an install either.
        let marker = InstalledMarker {
            version: MANIFEST.version.into(),
            files: MANIFEST
                .files
                .iter()
                .map(|f| InstalledFile {
                    name: f.name.into(),
                    bytes: f.bytes,
                    sha256: f.sha256.into(),
                })
                .collect(),
            installed_at: "t".into(),
        };
        std::fs::write(marker_path(&dir), serde_json::to_vec(&marker).unwrap()).unwrap();
        assert!(!is_installed(&dir));
        // Files with the exact manifest sizes complete the picture.
        for file in &MANIFEST.files {
            let f = std::fs::File::create(pkg.join(file.name)).unwrap();
            f.set_len(file.bytes).unwrap();
        }
        assert!(is_installed(&dir));
        let packages = OcrPackages::default();
        let status = packages.status(&dir);
        assert!(status.installed);
        assert_eq!(status.installed_bytes, 139_399_761);
        assert_eq!(status.download_bytes, 139_347_772);
        assert!(packages.installed_file(&dir, "det.onnx").is_ok());
        assert_eq!(packages.model_paths(&dir).unwrap().len(), 4);
        assert_eq!(
            packages
                .installed_file(&dir, "../etc/passwd")
                .unwrap_err()
                .code,
            "ocr_file_unknown"
        );
        packages.remove(&dir).unwrap();
        assert!(!is_installed(&dir));
        assert!(!pkg.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_of_a_file_matches_the_in_memory_digest() {
        let dir = super::super::asset::tests::temp_dir("sha");
        let path = dir.join("x.bin");
        std::fs::write(&path, b"dream paper").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            super::super::asset::sha256_hex(b"dream paper")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
