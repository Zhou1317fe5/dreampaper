use std::{
    env, fs,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use dreampaper_ocr::{
    engine::DET_MIN_PAD,
    ppocr::utils::pad_to_multiple,
    protocol::{
        read_response, write_analyze, AnalyzeResult, Failure, Ready, Response, StageTimes,
        STATUS_ERROR, STATUS_OK, STATUS_READY,
    },
};
use image::{Rgb, RgbImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// P2 requires at least 50 warm requests; smaller values are smoke runs only.
const HOT_RUNS: usize = 50;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const FORCE_STOP_LIMIT_MS: u64 = 2_000;

#[derive(Serialize)]
struct Report {
    ok: bool,
    gate: &'static str,
    missing_gates: [&'static str; 3],
    sample: String,
    sample_width: u32,
    sample_height: u32,
    padded_width: u32,
    padded_height: u32,
    pad_offset: [u32; 2],
    pad_color: [u8; 3],
    cpu_speed_limit: Option<u32>,
    ready_ms: u64,
    init_ms: u64,
    cold_first_ms: u64,
    first_inference_ms: u64,
    hot_p95_ms: u64,
    hot_runs: usize,
    threads: usize,
    stage_p95_us: StageTimes,
    peak_rss_bytes: u64,
    force_stop_ms: u64,
    runtime: String,
    runtime_sha256: String,
    model_manifest: String,
    lines: Vec<dreampaper_ocr::protocol::Line>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ModelManifest {
    version: String,
    files: Vec<ModelFile>,
}

#[derive(Deserialize)]
struct ModelFile {
    name: String,
    bytes: u64,
    sha256: String,
}

struct Paths {
    sidecar: PathBuf,
    runtime: PathBuf,
    models: PathBuf,
    sample: PathBuf,
    report: PathBuf,
    threads: usize,
    hot_runs: usize,
}

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("native OCR gate: {error}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<(), String> {
    let paths = parse_args()?;
    let model_manifest = verify_models(&paths.models)?;
    let runtime_sha256 = sha256_file(&paths.runtime)?;
    let sample_bytes = fs::read(&paths.sample).map_err(|error| format!("无法读取样图：{error}"))?;
    let image = decode_sample(&sample_bytes)?;
    if image.width() > 1200 || image.height() > 800 {
        return Err(format!(
            "性能样图必须在 1200×800 内，当前 {}×{}",
            image.width(),
            image.height()
        ));
    }
    // Emulates the main process: pad with the region background to a
    // 32-multiple so detection runs on unscaled pixels (D1 boundary).
    let pad_color = border_mean(&image);
    let padded = pad_to_multiple(&image, DET_MIN_PAD, pad_color);
    let region_width = image.width();
    let region_height = image.height();
    let width = padded.image.width();
    let height = padded.image.height();
    let rgb = padded.image.into_raw();

    let cold_started = Instant::now();
    let mut child = Running::spawn(&paths)?;
    let monitor = RssMonitor::start(child.child.id());
    let ready = child.expect_ready(RESPONSE_TIMEOUT)?;
    let ready_ms = millis(cold_started.elapsed());

    let first_started = Instant::now();
    child.send_analyze(1, width, height, &rgb)?;
    let first = child.expect_result(1, RESPONSE_TIMEOUT)?;
    let first_inference_ms = millis(first_started.elapsed());
    let cold_first_ms = millis(cold_started.elapsed());

    let mut hot = Vec::with_capacity(paths.hot_runs);
    let mut stages = StageSamples::with_capacity(paths.hot_runs);
    for run in 0..paths.hot_runs {
        let started = Instant::now();
        let id = run as u64 + 2;
        child.send_analyze(id, first.width, first.height, &rgb)?;
        let result = child.expect_result(id, RESPONSE_TIMEOUT)?;
        if result.width != first.width || result.height != first.height {
            return Err("sidecar 返回尺寸不一致".into());
        }
        hot.push(millis(started.elapsed()));
        stages.push(result.stages);
    }
    hot.sort_unstable();
    let hot_p95_ms = percentile_95(&mut hot);
    let stage_p95_us = stages.p95();

    child.send_analyze(u64::MAX, first.width, first.height, &rgb)?;
    thread::sleep(Duration::from_millis(10));
    let stop_started = Instant::now();
    child.force_stop()?;
    let force_stop_ms = millis(stop_started.elapsed());
    let peak_rss_bytes = monitor.stop();

    let ok = paths.hot_runs >= HOT_RUNS
        && cold_first_ms <= 8_000
        && hot_p95_ms <= 1_500
        && peak_rss_bytes <= 1024 * 1024 * 1024
        && force_stop_ms <= FORCE_STOP_LIMIT_MS
        && !first.lines.is_empty();
    let lines = first
        .lines
        .into_iter()
        .map(|mut line| {
            for point in &mut line.points {
                point.x = point.x.saturating_sub(padded.offset_x).min(region_width);
                point.y = point.y.saturating_sub(padded.offset_y).min(region_height);
            }
            line
        })
        .collect();
    let report = Report {
        ok,
        gate: "native-engine-connectivity-and-p2-partial",
        missing_gates: [
            "Tauri supervisor UI cancel <= 200 ms",
            "signed production bundle",
            "Q1 versioned 300-region corpus",
        ],
        sample: paths.sample.display().to_string(),
        sample_width: region_width,
        sample_height: region_height,
        padded_width: width,
        padded_height: height,
        pad_offset: [padded.offset_x, padded.offset_y],
        pad_color: pad_color.0,
        cpu_speed_limit: cpu_speed_limit(),
        ready_ms,
        init_ms: ready.init_ms,
        cold_first_ms,
        first_inference_ms,
        hot_p95_ms,
        hot_runs: paths.hot_runs,
        threads: paths.threads,
        stage_p95_us,
        peak_rss_bytes,
        force_stop_ms,
        runtime: ready.runtime,
        runtime_sha256,
        model_manifest: model_manifest.version,
        lines,
        error: (!ok).then(|| "原生引擎连通性/P2 部分探针未通过；Q1 尚未执行".into()),
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    fs::write(&paths.report, bytes).map_err(|error| format!("写入报告失败：{error}"))?;
    println!("{}", paths.report.display());
    if ok {
        Ok(())
    } else {
        Err("原生引擎连通性/P2 部分探针未通过".into())
    }
}

struct StageSamples {
    prepare: Vec<u64>,
    detection: Vec<u64>,
    crop: Vec<u64>,
    classification: Vec<u64>,
    recognition: Vec<u64>,
    assemble: Vec<u64>,
}

impl StageSamples {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            prepare: Vec::with_capacity(capacity),
            detection: Vec::with_capacity(capacity),
            crop: Vec::with_capacity(capacity),
            classification: Vec::with_capacity(capacity),
            recognition: Vec::with_capacity(capacity),
            assemble: Vec::with_capacity(capacity),
        }
    }

    fn push(&mut self, stages: StageTimes) {
        self.prepare.push(stages.prepare_us);
        self.detection.push(stages.detection_us);
        self.crop.push(stages.crop_us);
        self.classification.push(stages.classification_us);
        self.recognition.push(stages.recognition_us);
        self.assemble.push(stages.assemble_us);
    }

    fn p95(mut self) -> StageTimes {
        StageTimes {
            prepare_us: percentile_95(&mut self.prepare),
            detection_us: percentile_95(&mut self.detection),
            crop_us: percentile_95(&mut self.crop),
            classification_us: percentile_95(&mut self.classification),
            recognition_us: percentile_95(&mut self.recognition),
            assemble_us: percentile_95(&mut self.assemble),
        }
    }
}

fn percentile_95(values: &mut [u64]) -> u64 {
    values.sort_unstable();
    values[((values.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)]
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<Result<Response, String>>,
    reader: Option<JoinHandle<()>>,
    stopped: bool,
}

impl Running {
    fn spawn(paths: &Paths) -> Result<Self, String> {
        let mut child = Command::new(&paths.sidecar)
            .args([
                "--runtime",
                path(paths.runtime.as_path())?,
                "--det",
                path(&paths.models.join("det.onnx"))?,
                "--cls",
                path(&paths.models.join("cls.onnx"))?,
                "--rec",
                path(&paths.models.join("rec.onnx"))?,
                "--dict",
                path(&paths.models.join("dict.txt"))?,
                "--threads",
                &paths.threads.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("无法启动 sidecar：{error}"))?;
        let stdin = child.stdin.take().ok_or("sidecar stdin 不可用")?;
        let stdout = child.stdout.take().ok_or("sidecar stdout 不可用")?;
        let (sender, responses) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let response = read_response(&mut stdout)
                    .map_err(|error| error.to_string())
                    .and_then(|response| response.ok_or_else(|| "sidecar 提前关闭 stdout".into()));
                let stop = response.is_err();
                if sender.send(response).is_err() || stop {
                    return;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            responses,
            reader: Some(reader),
            stopped: false,
        })
    }

    fn expect_ready(&self, timeout: Duration) -> Result<Ready, String> {
        let response = self.required_response(timeout)?;
        if response.status != STATUS_READY || response.id != 0 {
            return Err("sidecar 未返回 ready 帧".into());
        }
        serde_json::from_slice(&response.payload)
            .map_err(|error| format!("ready 响应无效：{error}"))
    }

    fn send_analyze(&mut self, id: u64, width: u32, height: u32, rgb: &[u8]) -> Result<(), String> {
        write_analyze(&mut self.stdin, id, width, height, rgb).map_err(|error| error.to_string())
    }

    fn expect_result(&self, id: u64, timeout: Duration) -> Result<AnalyzeResult, String> {
        let response = self.required_response(timeout)?;
        if response.id != id {
            return Err(format!("响应 ID 不匹配：期望 {id}，收到 {}", response.id));
        }
        match response.status {
            STATUS_OK => serde_json::from_slice(&response.payload)
                .map_err(|error| format!("OCR 响应无效：{error}")),
            STATUS_ERROR => {
                let failure: Failure<'_> = serde_json::from_slice(&response.payload)
                    .map_err(|error| format!("错误响应无效：{error}"))?;
                Err(format!("{}：{}", failure.code, failure.message))
            }
            status => Err(format!("OCR 响应状态无效：{status}")),
        }
    }

    fn required_response(&self, timeout: Duration) -> Result<Response, String> {
        self.responses
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => "等待 sidecar 响应超时".to_owned(),
                mpsc::RecvTimeoutError::Disconnected => "sidecar 响应通道已关闭".to_owned(),
            })?
    }

    fn force_stop(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        if self
            .child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            self.child.kill().map_err(|error| error.to_string())?;
        }
        self.child.wait().map_err(|error| error.to_string())?;
        self.stopped = true;
        if let Some(reader) = self.reader.take() {
            reader
                .join()
                .map_err(|_| "响应读取线程异常退出".to_owned())?;
        }
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

struct RssMonitor {
    peak: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RssMonitor {
    fn start(pid: u32) -> Self {
        let peak = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let peak_for_thread = Arc::clone(&peak);
        let stop_for_thread = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !stop_for_thread.load(Ordering::Relaxed) {
                if let Some(rss) = rss_bytes(pid) {
                    peak_for_thread.fetch_max(rss, Ordering::Relaxed);
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            peak,
            stop,
            thread: Some(thread),
        }
    }

    fn stop(mut self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.peak.load(Ordering::Relaxed)
    }
}

impl Drop for RssMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn verify_models(directory: &Path) -> Result<ModelManifest, String> {
    const MANIFEST: &str = include_str!("../../manifest.json");
    const EXPECTED: [&str; 4] = ["det.onnx", "cls.onnx", "rec.onnx", "dict.txt"];

    let manifest: ModelManifest =
        serde_json::from_str(MANIFEST).map_err(|error| format!("模型 manifest 无效：{error}"))?;
    if manifest.files.len() != EXPECTED.len() {
        return Err("模型 manifest 文件数量无效".into());
    }
    for expected in EXPECTED {
        let entries = manifest
            .files
            .iter()
            .filter(|file| file.name == expected)
            .collect::<Vec<_>>();
        if entries.len() != 1 {
            return Err(format!("模型 manifest 缺少或重复文件：{expected}"));
        }
        let entry = entries[0];
        let file = directory.join(expected);
        let metadata = file
            .metadata()
            .map_err(|error| format!("读取模型元数据失败 {expected}：{error}"))?;
        if !metadata.is_file() || metadata.len() != entry.bytes {
            return Err(format!("模型大小不匹配：{expected}"));
        }
        let actual = sha256_file(&file)?;
        if actual != entry.sha256 {
            return Err(format!("模型摘要不匹配：{expected}"));
        }
    }
    Ok(manifest)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Mean colour of the one pixel border; stands in for the main process's
/// dominant background colour so padding does not create artificial edges.
fn border_mean(image: &RgbImage) -> Rgb<u8> {
    let (width, height) = image.dimensions();
    let mut sum = [0_u64; 3];
    let mut count = 0_u64;
    for (x, y, pixel) in image.enumerate_pixels() {
        if x == 0 || y == 0 || x + 1 == width || y + 1 == height {
            for channel in 0..3 {
                sum[channel] += u64::from(pixel[channel]);
            }
            count += 1;
        }
    }
    if count == 0 {
        return Rgb([255, 255, 255]);
    }
    Rgb([
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    ])
}

/// macOS reports thermal throttling as `CPU_Speed_Limit` (percent); recorded so
/// a slow run on a hot laptop is not mistaken for an engine regression.
fn cpu_speed_limit() -> Option<u32> {
    let output = Command::new("pmset").args(["-g", "therm"]).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    text.lines()
        .find(|line| line.contains("CPU_Speed_Limit"))
        .and_then(|line| line.rsplit('=').next())
        .and_then(|value| value.trim().parse().ok())
}

fn decode_sample(bytes: &[u8]) -> Result<image::RgbImage, String> {
    image::load_from_memory(bytes)
        .map_err(|error| format!("无法解码样图：{error}"))
        .map(|image| image.to_rgb8())
}

fn rss_bytes(pid: u32) -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let kib: u64 = String::from_utf8(output.stdout).ok()?.trim().parse().ok()?;
    kib.checked_mul(1024)
}

fn parse_args() -> Result<Paths, String> {
    let mut args = env::args().skip(1);
    let values = ["sidecar", "runtime", "models", "sample", "report"]
        .into_iter()
        .map(|name| {
            args.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("缺少 {name} 参数"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let threads = args
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| "线程数必须是整数".to_owned())?
        .unwrap_or(4);
    if !matches!(threads, 1 | 2 | 4 | 6 | 8 | 12) {
        return Err("线程数必须是 1、2、4、6、8 或 12".into());
    }
    let hot_runs = args
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| "热请求次数必须是整数".to_owned())?
        .unwrap_or(HOT_RUNS);
    if hot_runs == 0 {
        return Err("热请求次数必须大于 0".into());
    }
    if args.next().is_some() {
        return Err("gate 参数过多".into());
    }
    for input in [&values[0], &values[1], &values[2], &values[3]] {
        if !input.exists() {
            return Err(format!("gate 输入不存在：{}", input.display()));
        }
    }
    Ok(Paths {
        sidecar: values[0].clone(),
        runtime: values[1].clone(),
        models: values[2].clone(),
        sample: values[3].clone(),
        report: values[4].clone(),
        threads,
        hot_runs,
    })
}

fn path(path: &Path) -> Result<&str, String> {
    path.to_str().ok_or_else(|| "路径不是 UTF-8".into())
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use image::{codecs::jpeg::JpegEncoder, Rgb, RgbImage};

    use super::decode_sample;

    #[test]
    fn decodes_by_signature_instead_of_extension() {
        let source = RgbImage::from_pixel(2, 3, Rgb([12, 34, 56]));
        let mut bytes = Vec::new();
        JpegEncoder::new(&mut bytes).encode_image(&source).unwrap();

        let decoded = decode_sample(&bytes).unwrap();
        assert_eq!(decoded.dimensions(), (2, 3));
    }
}
