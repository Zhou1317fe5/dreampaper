use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

use dreampaper_ocr::{
    engine::DET_MIN_PAD,
    ppocr::utils::pad_to_multiple,
    supervisor::{Config, JobError, Supervisor},
};
use image::{Rgb, RgbImage};
use serde::Serialize;

const TIMEOUT: Duration = Duration::from_secs(30);
const CANCEL_TIMEOUT: Duration = Duration::from_millis(200);
const CANCEL_GRACE: Duration = Duration::from_millis(120);
const IDLE_TIMEOUT: Duration = Duration::from_millis(120);

#[derive(Debug, Default, Serialize)]
struct Report {
    ok: bool,
    gate: &'static str,
    first_start_ms: u64,
    cancel_return_ms: u64,
    cancel_result: String,
    reap_after_cancel_ms: u64,
    first_pid: Option<u32>,
    second_pid: Option<u32>,
    launches_after_cancel: u64,
    restart_result_ms: u64,
    idle_exit_ms: u64,
    shutdown_ms: u64,
    error: Option<String>,
}

struct Paths {
    sidecar: PathBuf,
    runtime: PathBuf,
    models: PathBuf,
    sample: PathBuf,
    report: PathBuf,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native OCR lifecycle gate: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let paths = parse_args()?;
    let image = decode_sample(&paths.sample)?;
    let padded = pad_to_multiple(&image, DET_MIN_PAD, Rgb([255, 255, 255])).image;
    let (width, height) = padded.dimensions();
    let rgb = padded.into_raw();
    let arguments = sidecar_arguments(&paths);
    let config = Config {
        program: paths.sidecar.clone(),
        arguments,
        startup_timeout: TIMEOUT,
        request_timeout: TIMEOUT,
        cancel_grace: CANCEL_GRACE,
        idle_timeout: IDLE_TIMEOUT,
    };
    let supervisor = Supervisor::start(config);
    let mut report = Report {
        gate: "native-supervisor-lifecycle",
        ..Report::default()
    };

    let first_started = Instant::now();
    let first = supervisor
        .submit(width, height, rgb.clone())
        .map_err(|error| error.to_string())?;
    first
        .wait_started(TIMEOUT)
        .map_err(|error| format!("首个请求未完成写入：{error}"))?;
    report.first_pid = Some(wait_for_pid(&supervisor, TIMEOUT)?);
    report.first_start_ms = millis(first_started.elapsed());
    thread::sleep(Duration::from_millis(50));

    let cancel_started = Instant::now();
    let cancelled = supervisor
        .cancel(first.id(), CANCEL_TIMEOUT)
        .map_err(|error| error.to_string())?;
    report.cancel_return_ms = millis(cancel_started.elapsed());
    if !cancelled {
        report.cancel_result = "not_cancelled".into();
        return write_failure(
            &paths.report,
            report,
            "运行中的请求未被 supervisor 接受取消",
        );
    }
    report.cancel_result = match first.recv_timeout(TIMEOUT) {
        Ok(Err(JobError::Cancelled)) => "cancelled".to_owned(),
        Ok(Err(error)) => format!("unexpected_error:{error}"),
        Ok(Ok(_)) => "completed_before_cancel".to_owned(),
        Err(error) => format!("result_timeout:{error}"),
    };
    if report.cancel_result != "cancelled" {
        return write_failure(&paths.report, report, "取消请求未返回稳定的 Cancelled 结果");
    }
    wait_for_pid_absent(&supervisor, TIMEOUT)?;
    report.reap_after_cancel_ms = millis(cancel_started.elapsed());
    report.launches_after_cancel = supervisor.launch_count();

    let restart_started = Instant::now();
    let second = supervisor
        .submit(width, height, rgb)
        .map_err(|error| error.to_string())?;
    second
        .wait_started(TIMEOUT)
        .map_err(|error| format!("重启请求未完成写入：{error}"))?;
    let second_pid = wait_for_pid(&supervisor, TIMEOUT)?;
    report.second_pid = Some(second_pid);
    if Some(second_pid) == report.first_pid {
        return write_failure(&paths.report, report, "取消后的请求未启动新的辅助进程");
    }
    let result = second
        .recv_timeout(TIMEOUT)
        .map_err(|error| format!("重启请求结果超时：{error}"))?
        .map_err(|error| error.to_string())?;
    report.restart_result_ms = millis(restart_started.elapsed());
    if result.lines.is_empty() {
        return write_failure(&paths.report, report, "重启后的辅助进程未返回文字结果");
    }

    let idle_started = Instant::now();
    wait_for_pid_absent(&supervisor, TIMEOUT)?;
    report.idle_exit_ms = millis(idle_started.elapsed());
    let shutdown_started = Instant::now();
    supervisor
        .shutdown(TIMEOUT)
        .map_err(|error| error.to_string())?;
    report.shutdown_ms = millis(shutdown_started.elapsed());
    report.ok = report.cancel_return_ms <= 200
        && report.launches_after_cancel >= 1
        && report.first_pid.is_some()
        && report.second_pid != report.first_pid;
    if !report.ok {
        return write_failure(&paths.report, report, "生命周期判定未通过");
    }
    write_report(&paths.report, &report)
}

fn sidecar_arguments(paths: &Paths) -> Vec<OsString> {
    vec![
        "--runtime".into(),
        paths.runtime.clone().into_os_string(),
        "--det".into(),
        paths.models.join("det.onnx").into_os_string(),
        "--cls".into(),
        paths.models.join("cls.onnx").into_os_string(),
        "--rec".into(),
        paths.models.join("rec.onnx").into_os_string(),
        "--dict".into(),
        paths.models.join("dict.txt").into_os_string(),
        "--threads".into(),
        "4".into(),
    ]
}

fn wait_for_pid(supervisor: &Supervisor, timeout: Duration) -> Result<u32, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(pid) = supervisor.process_id() {
            return Ok(pid);
        }
        if Instant::now() >= deadline {
            return Err("等待辅助进程 PID 超时".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_pid_absent(supervisor: &Supervisor, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if supervisor.process_id().is_none() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("辅助进程未在期限内退出".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn decode_sample(path: &Path) -> Result<RgbImage, String> {
    let bytes = fs::read(path).map_err(|error| format!("读取样图失败：{error}"))?;
    image::load_from_memory(&bytes)
        .map_err(|error| format!("解码样图失败：{error}"))
        .map(|image| image.to_rgb8())
}

fn parse_args() -> Result<Paths, String> {
    let mut args = env::args().skip(1);
    let values = (0..5)
        .map(|_| {
            args.next()
                .map(PathBuf::from)
                .ok_or_else(|| "生命周期 gate 参数不足".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if args.next().is_some() {
        return Err("生命周期 gate 参数过多".into());
    }
    for path in &values[..4] {
        if !path.exists() {
            return Err(format!("生命周期 gate 输入不存在：{}", path.display()));
        }
    }
    Ok(Paths {
        sidecar: values[0].clone(),
        runtime: values[1].clone(),
        models: values[2].clone(),
        sample: values[3].clone(),
        report: values[4].clone(),
    })
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn write_report(path: &Path, report: &Report) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| format!("写入生命周期报告失败：{error}"))
}

fn write_failure(path: &Path, mut report: Report, reason: &str) -> Result<(), String> {
    report.ok = false;
    report.error = Some(reason.to_owned());
    write_report(path, &report)?;
    Err(format!("原生 supervisor 生命周期 gate 未通过：{reason}"))
}
