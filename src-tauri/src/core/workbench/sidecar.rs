//! The native OCR engine as the host sees it: where the sidecar binary and
//! the ONNX Runtime library live, one lazily started supervisor, the padded
//! crop it is fed (D1) and the mapping of its answer back to region pixels.
//!
//! The webview never sees image bytes or file paths. It submits a project id,
//! an integer rectangle and its own request id; everything else happens here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dreampaper_ocr::pad::{pad_to_multiple, DET_MIN_PAD};
use dreampaper_ocr::protocol::{AnalyzeResult, MAX_PIXELS};
use dreampaper_ocr::supervisor::{Config, ControlError, JobError, Supervisor};
use image::{Rgb, RgbImage};
use serde::Serialize;

use super::geom::PixelRect;
use super::source::DecodedSource;
use crate::error::{AppError, AppResult};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CANCEL_GRACE: Duration = Duration::from_millis(150);
/// Initial value from the E1 gate; the process is cheap to restart (~2.5 s
/// cold) so idle memory is released before a typical coffee break ends.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const CANCEL_ACK: Duration = Duration::from_millis(200);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// After this many consecutive engine failures the breaker trips.
const BREAKER_FAILURES: u32 = 3;
const BREAKER_HOLD: Duration = Duration::from_secs(60);

/// Where the engine pieces were found on this machine. Resolved once at
/// startup by the state layer, which knows the bundle layout.
#[derive(Clone, Debug, Default)]
pub struct SidecarLocation {
    pub program: Option<PathBuf>,
    pub runtime: Option<PathBuf>,
    pub problem: Option<String>,
}

impl SidecarLocation {
    /// The first existing candidate of each kind wins; a missing piece is
    /// reported in `problem` so the settings page can say what is wrong.
    pub fn discover(program_candidates: &[PathBuf], runtime_candidates: &[PathBuf]) -> Self {
        let program = program_candidates.iter().find(|p| p.is_file()).cloned();
        let runtime = runtime_candidates.iter().find(|p| p.is_file()).cloned();
        let problem = match (&program, &runtime) {
            (Some(_), Some(_)) => None,
            (None, Some(_)) => Some("安装包缺少 OCR 辅助程序".to_string()),
            (Some(_), None) => Some("安装包缺少 ONNX Runtime 动态库".to_string()),
            (None, None) => Some("安装包缺少 OCR 辅助程序与 ONNX Runtime".to_string()),
        };
        Self {
            program,
            runtime,
            problem,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct OcrEngineStatus {
    pub available: bool,
    pub version: String,
    pub runtime_version: String,
    pub program: Option<String>,
    pub runtime: Option<String>,
    pub problem: Option<String>,
    pub running: bool,
    pub tripped: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct OcrPoint {
    pub x: i64,
    pub y: i64,
}

/// One recognised line in region-crop pixels (origin at the rect's corner).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct OcrItem {
    pub poly: Vec<OcrPoint>,
    pub text: String,
    pub score: f32,
    pub box_score: f32,
    pub angle: u16,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecognizeResult {
    pub items: Vec<OcrItem>,
    pub elapsed_ms: u64,
    pub padded_width: u32,
    pub padded_height: u32,
}

#[derive(Default)]
struct Breaker {
    failures: u32,
    tripped_until: Option<Instant>,
}

pub struct OcrSidecar {
    location: SidecarLocation,
    supervisor: Mutex<Option<Supervisor>>,
    /// Frontend request id → supervisor job id, for explicit cancellation.
    jobs: Mutex<HashMap<u64, u64>>,
    breaker: Mutex<Breaker>,
}

impl OcrSidecar {
    pub fn new(location: SidecarLocation) -> Self {
        Self {
            location,
            supervisor: Mutex::new(None),
            jobs: Mutex::new(HashMap::new()),
            breaker: Mutex::new(Breaker::default()),
        }
    }

    pub fn status(&self) -> OcrEngineStatus {
        let running = self
            .supervisor
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|supervisor| supervisor.process_id().is_some());
        let tripped = self
            .breaker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .tripped_until
            .is_some_and(|until| Instant::now() < until);
        OcrEngineStatus {
            available: self.location.program.is_some() && self.location.runtime.is_some(),
            version: dreampaper_ocr::VERSION.to_string(),
            runtime_version: dreampaper_ocr::RUNTIME_VERSION.to_string(),
            program: self
                .location
                .program
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            runtime: self
                .location
                .runtime
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            problem: self.location.problem.clone(),
            running,
            tripped,
        }
    }

    /// Run OCR on `rect` of `source`. Blocks until the sidecar answers; call
    /// from a blocking task. `background` pads the crop so lines near the
    /// selection edge get context, exactly like the gate probes.
    pub fn recognize(
        &self,
        models: &[PathBuf; 4],
        source: &DecodedSource,
        rect: PixelRect,
        background: [u8; 3],
        request_id: u64,
    ) -> AppResult<RecognizeResult> {
        let (program, runtime) = match (&self.location.program, &self.location.runtime) {
            (Some(program), Some(runtime)) => (program.clone(), runtime.clone()),
            _ => {
                return Err(AppError::new(
                    "ocr_engine_missing",
                    self.location
                        .problem
                        .clone()
                        .unwrap_or_else(|| "OCR 引擎不可用".into()),
                ))
            }
        };
        self.check_breaker()?;
        if !rect.within(source.width, source.height) {
            return Err(AppError::new(
                "workbench_region_invalid",
                "选区不在图片范围内",
            ));
        }
        let crop = crop_rgb(source, rect);
        let padded = pad_to_multiple(&crop, DET_MIN_PAD, Rgb(background));
        let (width, height) = padded.image.dimensions();
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(AppError::new(
                "ocr_region_too_large",
                "选区过大，无法识别；请缩小选区",
            ));
        }

        let job = self.submit(
            &program,
            &runtime,
            models,
            width,
            height,
            padded.image.into_raw(),
        )?;
        let job_id = job.id();
        self.jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(request_id, job_id);
        let outcome = job.recv_timeout(STARTUP_TIMEOUT + REQUEST_TIMEOUT);
        self.jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&request_id);

        let result = match outcome {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => return Err(self.map_job_error(error)),
            Err(_) => {
                self.record_failure();
                return Err(AppError::new(
                    "ocr_timeout",
                    "OCR 引擎无响应，已停止本次识别",
                ));
            }
        };
        self.record_success();
        Ok(map_result(result, &rect, padded.offset_x, padded.offset_y))
    }

    /// Cancel the request the frontend knows as `request_id`. Returns whether
    /// the sidecar had a matching job to cancel.
    pub fn cancel(&self, request_id: u64) -> AppResult<bool> {
        let job_id = self
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&request_id)
            .copied();
        let Some(job_id) = job_id else {
            return Ok(false);
        };
        let guard = self.supervisor.lock().unwrap_or_else(|p| p.into_inner());
        let Some(supervisor) = guard.as_ref() else {
            return Ok(false);
        };
        match supervisor.cancel(job_id, CANCEL_ACK) {
            Ok(cancelled) => Ok(cancelled),
            Err(ControlError::CancelTimeout) => Err(AppError::new(
                "ocr_cancel_timeout",
                "OCR 引擎未及时确认取消",
            )),
            Err(_) => Ok(false),
        }
    }

    /// Stop the sidecar now: before deleting or replacing models and at app
    /// exit. Safe to call when nothing is running.
    pub fn shutdown(&self) {
        let supervisor = self
            .supervisor
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(supervisor) = supervisor {
            let _ = supervisor.shutdown(SHUTDOWN_TIMEOUT);
        }
        self.jobs.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    fn submit(
        &self,
        program: &Path,
        runtime: &Path,
        models: &[PathBuf; 4],
        width: u32,
        height: u32,
        rgb: Vec<u8>,
    ) -> AppResult<dreampaper_ocr::supervisor::Job> {
        let mut guard = self.supervisor.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(Supervisor::start(config(program, runtime, models)));
        }
        let first = guard
            .as_ref()
            .expect("supervisor present")
            .submit(width, height, rgb.clone());
        match first {
            Ok(job) => Ok(job),
            Err(ControlError::Image) => Err(AppError::new(
                "ocr_region_too_large",
                "选区过大，无法识别；请缩小选区",
            )),
            Err(ControlError::Stopped) => {
                // The supervisor thread is gone (it only exits after
                // shutdown); start a fresh one once.
                let supervisor = Supervisor::start(config(program, runtime, models));
                let job = supervisor
                    .submit(width, height, rgb)
                    .map_err(|_| AppError::new("ocr_engine_failed", "OCR 引擎无法启动"))?;
                *guard = Some(supervisor);
                Ok(job)
            }
            Err(ControlError::CancelTimeout) => {
                Err(AppError::new("ocr_engine_failed", "OCR 引擎状态异常"))
            }
        }
    }

    fn map_job_error(&self, error: JobError) -> AppError {
        match error {
            JobError::Superseded => AppError::new("ocr_superseded", "识别请求已被新的选区替代"),
            JobError::Cancelled => AppError::new("ocr_cancelled", "识别已取消"),
            JobError::StartupTimeout => {
                self.record_failure();
                AppError::new("ocr_timeout", "OCR 引擎启动超时")
            }
            JobError::RequestTimeout => {
                self.record_failure();
                AppError::new("ocr_timeout", "OCR 识别超时，可重试或继续手工编辑")
            }
            JobError::Stopped | JobError::Process(_) | JobError::Protocol(_) => {
                self.record_failure();
                AppError::new(
                    "ocr_engine_failed",
                    "OCR 引擎异常退出，已自动恢复；可重试或继续手工编辑",
                )
            }
        }
    }

    fn check_breaker(&self) -> AppResult<()> {
        let mut breaker = self.breaker.lock().unwrap_or_else(|p| p.into_inner());
        match breaker.tripped_until {
            Some(until) if Instant::now() < until => Err(AppError::new(
                "ocr_engine_unavailable",
                "OCR 引擎连续失败，已暂停一分钟；纯色修补和手工文字仍可使用",
            )),
            Some(_) => {
                breaker.tripped_until = None;
                breaker.failures = 0;
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn record_failure(&self) {
        let mut breaker = self.breaker.lock().unwrap_or_else(|p| p.into_inner());
        breaker.failures += 1;
        if breaker.failures >= BREAKER_FAILURES {
            breaker.tripped_until = Some(Instant::now() + BREAKER_HOLD);
        }
    }

    fn record_success(&self) {
        let mut breaker = self.breaker.lock().unwrap_or_else(|p| p.into_inner());
        breaker.failures = 0;
        breaker.tripped_until = None;
    }
}

fn config(program: &Path, runtime: &Path, models: &[PathBuf; 4]) -> Config {
    let mut arguments = vec!["--runtime".into(), runtime.as_os_str().to_owned()];
    for (flag, path) in ["--det", "--cls", "--rec", "--dict"].iter().zip(models) {
        arguments.push((*flag).into());
        arguments.push(path.as_os_str().to_owned());
    }
    arguments.push("--threads".into());
    arguments.push(threads().to_string().into());
    Config {
        program: program.to_path_buf(),
        arguments,
        startup_timeout: STARTUP_TIMEOUT,
        request_timeout: REQUEST_TIMEOUT,
        cancel_grace: CANCEL_GRACE,
        idle_timeout: IDLE_TIMEOUT,
    }
}

/// Intra-op threads for the sidecar: the P2 gate ran with 4; smaller
/// machines get fewer so the UI thread keeps a core.
fn threads() -> usize {
    match std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
    {
        0..=1 => 1,
        2..=3 => 2,
        _ => 4,
    }
}

fn crop_rgb(source: &DecodedSource, rect: PixelRect) -> RgbImage {
    let width = rect.width as u32;
    let height = rect.height as u32;
    let stride = source.width as usize * 3;
    let mut out = Vec::with_capacity(width as usize * height as usize * 3);
    for row in 0..height as usize {
        let start = (rect.y as usize + row) * stride + rect.x as usize * 3;
        out.extend_from_slice(&source.rgb[start..start + width as usize * 3]);
    }
    RgbImage::from_raw(width, height, out).expect("crop dimensions match buffer")
}

/// Map sidecar points (padded-crop space) to region-crop space and clamp
/// them to the rectangle. Lines that end up with a degenerate box are kept;
/// the frontend applies its own text and geometry filters.
fn map_result(
    result: AnalyzeResult,
    rect: &PixelRect,
    offset_x: u32,
    offset_y: u32,
) -> RecognizeResult {
    let items = result
        .lines
        .into_iter()
        .map(|line| OcrItem {
            poly: line
                .points
                .iter()
                .map(|point| OcrPoint {
                    x: (i64::from(point.x) - i64::from(offset_x)).clamp(0, rect.width),
                    y: (i64::from(point.y) - i64::from(offset_y)).clamp(0, rect.height),
                })
                .collect(),
            text: line.text,
            score: line.score,
            box_score: line.box_score,
            angle: line.angle,
        })
        .collect();
    RecognizeResult {
        items,
        elapsed_ms: result.elapsed_ms,
        padded_width: result.width,
        padded_height: result.height,
    }
}

/// `#rrggbb` → RGB, as `color::analyze` reports it.
pub fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use dreampaper_ocr::protocol::{Line, Point, StageTimes};

    fn source(width: u32, height: u32) -> DecodedSource {
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                rgb.extend_from_slice(&[x as u8, y as u8, 7]);
            }
        }
        DecodedSource {
            width,
            height,
            rgb,
            had_alpha: false,
            icc: None,
            png: Default::default(),
            jfif_dpi: None,
        }
    }

    #[test]
    fn crop_copies_exact_source_pixels() {
        let src = source(10, 6);
        let crop = crop_rgb(&src, PixelRect::new(3, 2, 4, 3));
        assert_eq!(crop.dimensions(), (4, 3));
        assert_eq!(crop.get_pixel(0, 0), &Rgb([3, 2, 7]));
        assert_eq!(crop.get_pixel(3, 2), &Rgb([6, 4, 7]));
    }

    #[test]
    fn padded_points_map_back_into_the_region() {
        let result = AnalyzeResult {
            width: 64,
            height: 64,
            elapsed_ms: 5,
            stages: StageTimes::default(),
            lines: vec![Line {
                text: "A".into(),
                score: 0.9,
                box_score: 0.8,
                angle: 0,
                angle_score: 0.5,
                points: vec![
                    Point { x: 20, y: 30 },
                    Point { x: 63, y: 30 },
                    Point { x: 63, y: 40 },
                    Point { x: 20, y: 40 },
                ],
            }],
        };
        let mapped = map_result(result, &PixelRect::new(100, 100, 12, 8), 24, 28);
        assert_eq!(
            mapped.items[0].poly,
            vec![
                OcrPoint { x: 0, y: 2 },
                OcrPoint { x: 12, y: 2 },
                OcrPoint { x: 12, y: 8 },
                OcrPoint { x: 0, y: 8 },
            ]
        );
        assert_eq!(mapped.items[0].text, "A");
        assert_eq!(mapped.padded_width, 64);
    }

    #[test]
    fn hex_colours_parse_strictly() {
        assert_eq!(parse_hex_color("#0ac81e"), Some([10, 200, 30]));
        assert_eq!(parse_hex_color("#FFFFFF"), Some([255, 255, 255]));
        assert_eq!(parse_hex_color("0ac81e"), None);
        assert_eq!(parse_hex_color("#0ac8"), None);
        assert_eq!(parse_hex_color("#0ac81g"), None);
    }

    #[test]
    fn missing_engine_is_reported_before_any_work() {
        let sidecar = OcrSidecar::new(SidecarLocation::discover(&[], &[]));
        let status = sidecar.status();
        assert!(!status.available);
        assert!(status.problem.is_some());
        let models = [
            PathBuf::from("a"),
            PathBuf::from("b"),
            PathBuf::from("c"),
            PathBuf::from("d"),
        ];
        let error = sidecar
            .recognize(
                &models,
                &source(4, 4),
                PixelRect::new(0, 0, 2, 2),
                [255, 255, 255],
                1,
            )
            .unwrap_err();
        assert_eq!(error.code, "ocr_engine_missing");
        assert!(!sidecar.cancel(1).unwrap());
        sidecar.shutdown();
    }

    /// Whole host path against the real sidecar, runtime and models. Runs
    /// only when the environment points at them:
    /// `DREAMPAPER_OCR_SIDECAR`, `DREAMPAPER_OCR_RUNTIME`,
    /// `DREAMPAPER_OCR_SEED` (dir with manifest downloads) and
    /// `DREAMPAPER_OCR_SAMPLE` (the 1200×800 gate sample).
    #[test]
    fn end_to_end_recognises_the_gate_sample_when_the_engine_is_present() {
        let Some((sidecar, runtime, seed, sample)) = std::env::var("DREAMPAPER_OCR_SIDECAR")
            .ok()
            .zip(std::env::var("DREAMPAPER_OCR_RUNTIME").ok())
            .zip(std::env::var("DREAMPAPER_OCR_SEED").ok())
            .zip(std::env::var("DREAMPAPER_OCR_SAMPLE").ok())
            .map(|(((a, b), c), d)| (a, b, c, d))
        else {
            return;
        };
        let dir = super::super::asset::tests::temp_dir("e2e");
        let fonts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fonts");
        let location =
            SidecarLocation::discover(&[PathBuf::from(sidecar)], &[PathBuf::from(runtime)]);
        assert!(location.problem.is_none(), "{:?}", location.problem);
        let core =
            crate::core::Core::new(dir.clone(), dir.join("prompts"), fonts, location).unwrap();

        // Seed the verified downloads, then let the installer derive the files.
        let package = super::super::ocr::package_dir(&dir);
        std::fs::create_dir_all(&package).unwrap();
        for download in &super::super::ocr::MANIFEST.downloads {
            let name = download.name;
            std::fs::copy(Path::new(&seed).join(name), package.join(name)).unwrap();
        }
        super::super::ocr::materialize_files(&package).unwrap();
        super::super::ocr::commit_marker(&dir).unwrap();
        assert!(super::super::ocr::is_installed(&dir));

        let opened = core
            .workbench()
            .open(
                super::super::ProjectSource::File {
                    path: sample.clone(),
                },
                false,
            )
            .unwrap();
        let id = opened.detail.summary.id.clone();
        let started = Instant::now();
        let result = core
            .recognize_region(&id, PixelRect::new(0, 0, 1200, 800), Some("#ffffff"), 1)
            .unwrap();
        let cold_ms = started.elapsed().as_millis();
        let texts: Vec<&str> = result.items.iter().map(|item| item.text.as_str()).collect();
        for expected in ["A", "K", "Crc", "V"] {
            assert!(texts.contains(&expected), "缺少 {expected}：{texts:?}");
        }
        for item in &result.items {
            for point in &item.poly {
                assert!((0..=1200).contains(&point.x) && (0..=800).contains(&point.y));
            }
        }
        assert!(core.sidecar.status().running);

        let started = Instant::now();
        let again = core
            .recognize_region(&id, PixelRect::new(0, 0, 1200, 800), Some("#ffffff"), 2)
            .unwrap();
        let hot_ms = started.elapsed().as_millis();
        assert_eq!(again.items.len(), result.items.len());
        eprintln!(
            "e2e cold {cold_ms} ms, hot {hot_ms} ms, {} lines",
            again.items.len()
        );

        // A superseded request must end as `ocr_superseded`, not as a failure.
        let stale = core.recognize_region(&id, PixelRect::new(10, 10, 300, 200), None, 3);
        assert!(stale.is_ok() || stale.as_ref().unwrap_err().code == "ocr_superseded");

        core.sidecar.shutdown();
        assert!(!core.sidecar.status().running);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn breaker_trips_after_repeated_failures_and_recovers() {
        let sidecar = OcrSidecar::new(SidecarLocation::default());
        for _ in 0..BREAKER_FAILURES {
            sidecar.record_failure();
        }
        assert_eq!(
            sidecar.check_breaker().unwrap_err().code,
            "ocr_engine_unavailable"
        );
        assert!(sidecar.status().tripped);
        sidecar.record_success();
        assert!(sidecar.check_breaker().is_ok());
    }
}
