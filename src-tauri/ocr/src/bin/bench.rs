//! Gate-only probe: splits detection latency into preprocessing, ORT run and
//! DB post-processing so P2 tuning decisions rest on measurements instead of
//! guesses. It mirrors `engine.rs` session options and geometry and is never
//! shipped. Post-processing is timed directly on the extracted probability map;
//! deriving it as `total - run` is unreliable because the laptop throttles
//! during long runs.

use std::{env, path::PathBuf, process::ExitCode, time::Instant};

use dreampaper_ocr::{
    engine::{det_session_builder, DETECTION, DET_MAX_SIDE_LEN, DET_MIN_PAD},
    ppocr::{det::DbNet, scale::ScaleParam, utils},
};
use image::Rgb;
use ort::{inputs, session::Session, value::Tensor};
use serde::Serialize;

const RUNS: usize = 20;
const MEAN_VALUES: [f32; 3] = [0.485 * 255.0, 0.456 * 255.0, 0.406 * 255.0];
const NORM_VALUES: [f32; 3] = [
    1.0 / 0.229 / 255.0,
    1.0 / 0.224 / 255.0,
    1.0 / 0.225 / 255.0,
];

#[derive(Serialize)]
struct Report {
    sample_width: u32,
    sample_height: u32,
    padded_width: u32,
    padded_height: u32,
    det_input_width: u32,
    det_input_height: u32,
    threads: usize,
    runs: usize,
    pre_p50_us: u64,
    pre_p95_us: u64,
    run_p50_us: u64,
    run_p95_us: u64,
    post_p50_us: u64,
    post_p95_us: u64,
    total_p50_us: u64,
    total_p95_us: u64,
    boxes: usize,
    alternatives: Vec<Alternative>,
}

#[derive(Serialize)]
struct Alternative {
    det_input_width: u32,
    det_input_height: u32,
    run_p50_us: u64,
    run_p95_us: u64,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native OCR det bench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let runtime = next_path(&mut args, "runtime")?;
    let det = next_path(&mut args, "det")?;
    let sample = next_path(&mut args, "sample")?;
    let threads: usize = args
        .next()
        .map(|value| value.parse().map_err(|_| "线程数必须是整数".to_owned()))
        .transpose()?
        .unwrap_or(4);

    ort::init_from(runtime.to_string_lossy())
        .with_name("dreampaper-ocr-bench")
        .with_telemetry(false)
        .commit()
        .map_err(|error| error.to_string())?;

    let bytes = std::fs::read(&sample).map_err(|error| error.to_string())?;
    let image = image::load_from_memory(&bytes)
        .map_err(|error| error.to_string())?
        .to_rgb8();
    let padded = utils::pad_to_multiple(&image, DET_MIN_PAD, Rgb([255, 255, 255])).image;
    let scale = ScaleParam::for_detection(&padded, DET_MAX_SIDE_LEN);

    let builder = det_session_builder(threads).map_err(|error| error.to_string())?;
    let session = builder(Session::builder().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?
        .commit_from_file(det.to_string_lossy().as_ref())
        .map_err(|error| error.to_string())?;
    let input_name = session.inputs[0].name.clone();

    let params = DETECTION;
    let mut pre = Vec::with_capacity(RUNS);
    let mut run_times = Vec::with_capacity(RUNS);
    let mut post = Vec::with_capacity(RUNS);
    // The first iteration is a warm-up and is excluded from the statistics.
    for index in 0..RUNS + 1 {
        let started = Instant::now();
        let resized = if scale.is_identity() {
            padded.clone()
        } else {
            image::imageops::resize(
                &padded,
                scale.dst_width,
                scale.dst_height,
                image::imageops::FilterType::Triangle,
            )
        };
        let array = utils::normalize_bgr_nchw(&resized, &MEAN_VALUES, &NORM_VALUES);
        let tensor = Tensor::from_array(array).map_err(|error| error.to_string())?;
        let pre_us = micros(started.elapsed());
        let started = Instant::now();
        let outputs = session
            .run(inputs![input_name.clone() => tensor].map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        let run_us = micros(started.elapsed());
        let started = Instant::now();
        let (_, output) = outputs.iter().next().ok_or("检测模型没有输出")?;
        let probabilities: Vec<f32> = output
            .try_extract_tensor::<f32>()
            .map_err(|error| error.to_string())?
            .iter()
            .copied()
            .collect();
        let boxes = DbNet::boxes_from_probabilities(
            probabilities,
            scale.dst_height,
            scale.dst_width,
            &scale,
            params,
        );
        let post_us = micros(started.elapsed());
        drop(outputs);
        if index > 0 {
            pre.push(pre_us);
            run_times.push(run_us);
            post.push(post_us);
        }
        if boxes.is_empty() {
            return Err("检测样图没有文本框".into());
        }
    }

    // Alternative input sizes are interleaved per iteration so slow thermal
    // drift on the laptop affects every size equally instead of the last one.
    let sizes = [(960_u32, 672_u32), (1120, 768), (1280, 896)];
    let mut alternative_times = vec![Vec::with_capacity(RUNS); sizes.len()];
    for _ in 0..RUNS {
        for (slot, (width, height)) in sizes.iter().enumerate() {
            let resized = image::imageops::resize(
                &padded,
                *width,
                *height,
                image::imageops::FilterType::Triangle,
            );
            let array = utils::normalize_bgr_nchw(&resized, &MEAN_VALUES, &NORM_VALUES);
            let tensor = Tensor::from_array(array).map_err(|error| error.to_string())?;
            let started = Instant::now();
            let outputs = session
                .run(inputs![input_name.clone() => tensor].map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
            drop(outputs);
            alternative_times[slot].push(micros(started.elapsed()));
        }
    }
    let alternatives = sizes
        .iter()
        .zip(alternative_times.iter_mut())
        .map(|((width, height), times)| Alternative {
            det_input_width: *width,
            det_input_height: *height,
            run_p50_us: percentile(times, 0.5),
            run_p95_us: percentile(times, 0.95),
        })
        .collect();
    drop(session);

    let mut detector = DbNet::load(&det, builder).map_err(|error| error.to_string())?;
    let mut total = Vec::with_capacity(RUNS);
    let mut boxes = 0;
    for index in 0..RUNS + 1 {
        let started = Instant::now();
        let result = detector
            .detect(&padded, &scale, params)
            .map_err(|error| error.to_string())?;
        if index > 0 {
            total.push(micros(started.elapsed()));
        }
        boxes = result.len();
    }

    let report = Report {
        sample_width: image.width(),
        sample_height: image.height(),
        padded_width: padded.width(),
        padded_height: padded.height(),
        det_input_width: scale.dst_width,
        det_input_height: scale.dst_height,
        threads,
        runs: RUNS,
        pre_p50_us: percentile(&mut pre, 0.5),
        pre_p95_us: percentile(&mut pre, 0.95),
        run_p50_us: percentile(&mut run_times, 0.5),
        run_p95_us: percentile(&mut run_times, 0.95),
        post_p50_us: percentile(&mut post, 0.5),
        post_p95_us: percentile(&mut post, 0.95),
        total_p50_us: percentile(&mut total, 0.5),
        total_p95_us: percentile(&mut total, 0.95),
        boxes,
        alternatives,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn next_path(args: &mut impl Iterator<Item = String>, name: &str) -> Result<PathBuf, String> {
    let path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("缺少 {name} 参数"))?;
    if !path.exists() {
        return Err(format!("{name} 不存在：{}", path.display()));
    }
    Ok(path)
}

fn percentile(values: &mut [u64], quantile: f64) -> u64 {
    values.sort_unstable();
    values[((values.len() as f64 * quantile).ceil() as usize).saturating_sub(1)]
}

fn micros(duration: std::time::Duration) -> u64 {
    duration.as_micros().try_into().unwrap_or(u64::MAX)
}
