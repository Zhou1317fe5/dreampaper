use std::path::PathBuf;
use std::sync::Arc;

use tauri::path::BaseDirectory;
use tauri::Manager;

use crate::core::workbench::sidecar::SidecarLocation;
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
        let font_root = font_root(app);
        let sidecar = sidecar_location(app);
        Ok(Self {
            core: Arc::new(Core::new(app_data, prompt_root, font_root, sidecar)?),
        })
    }

    pub fn core(&self) -> &Core {
        &self.core
    }

    pub fn core_arc(&self) -> Arc<Core> {
        Arc::clone(&self.core)
    }
}

/// The bundled font ships as a resource; under `tauri dev` the resource dir
/// may not have been populated yet, so fall back to the crate's own `fonts/`.
fn font_root(app: &tauri::AppHandle) -> PathBuf {
    let bundled_name = crate::core::workbench::text::BUNDLED_FILE;
    if let Ok(dir) = app.path().resolve("fonts", BaseDirectory::Resource) {
        if dir.join(bundled_name).exists() {
            return dir;
        }
    }
    let crate_fonts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fonts");
    if crate_fonts.join(bundled_name).exists() {
        return crate_fonts;
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("fonts")
}

/// Rust target triple this binary was built for (tauri-build exports it).
fn target_triple() -> String {
    if let Some(triple) = option_env!("TAURI_ENV_TARGET_TRIPLE") {
        return triple.to_string();
    }
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => format!("{arch}-apple-darwin"),
        "windows" => format!("{arch}-pc-windows-msvc"),
        other => format!("{arch}-unknown-{other}"),
    }
}

fn runtime_file_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "libonnxruntime.dylib",
        "windows" => "onnxruntime.dll",
        _ => "libonnxruntime.so",
    }
}

fn sidecar_file_name() -> &'static str {
    if cfg!(windows) {
        "dreampaper-ocr.exe"
    } else {
        "dreampaper-ocr"
    }
}

/// Where the OCR sidecar and the ONNX Runtime library live.
///
/// Bundled: the sidecar sits next to the main executable (Tauri
/// `externalBin`); the runtime is a macOS framework dylib or a Windows
/// resource. Under `tauri dev` the sidecar is copied next to the debug
/// executable too, and the checked-in staging directories are consulted as a
/// fallback. Debug builds additionally honour `DREAMPAPER_OCR_SIDECAR` /
/// `DREAMPAPER_OCR_RUNTIME` so gates can point at fresh builds.
fn sidecar_location(app: &tauri::AppHandle) -> SidecarLocation {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()));
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let triple = target_triple();
    let runtime_name = runtime_file_name();

    let mut programs = Vec::new();
    let mut runtimes = Vec::new();
    if cfg!(debug_assertions) {
        if let Ok(path) = std::env::var("DREAMPAPER_OCR_SIDECAR") {
            programs.push(PathBuf::from(path));
        }
        if let Ok(path) = std::env::var("DREAMPAPER_OCR_RUNTIME") {
            runtimes.push(PathBuf::from(path));
        }
    }
    if let Some(dir) = &exe_dir {
        programs.push(dir.join(sidecar_file_name()));
        // macOS bundle: Contents/MacOS/../Frameworks/libonnxruntime.dylib
        if let Some(contents) = dir.parent() {
            runtimes.push(contents.join("Frameworks").join(runtime_name));
        }
        runtimes.push(dir.join("ort").join(runtime_name));
    }
    if let Ok(path) = app
        .path()
        .resolve(format!("ort/{runtime_name}"), BaseDirectory::Resource)
    {
        runtimes.push(path);
    }
    programs.push(
        manifest_dir
            .join("binaries")
            .join(format!("dreampaper-ocr-{triple}{}", exe_suffix())),
    );
    programs.push(
        manifest_dir
            .join("ocr")
            .join("target")
            .join("release")
            .join(sidecar_file_name()),
    );
    runtimes.push(
        manifest_dir
            .join("runtime")
            .join(&triple)
            .join(runtime_name),
    );
    runtimes.push(
        manifest_dir
            .join("target")
            .join("gate")
            .join(format!("ort-build-{}", std::env::consts::ARCH))
            .join("Release")
            .join(runtime_name),
    );
    SidecarLocation::discover(&programs, &runtimes)
}

fn exe_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
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
