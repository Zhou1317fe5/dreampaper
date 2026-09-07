use std::{
    env,
    io::{self, BufReader, BufWriter},
    path::PathBuf,
    process::ExitCode,
    time::Instant,
};

use dreampaper_ocr::{
    engine::{Engine, ModelPaths},
    protocol::{
        read_request, write_response, Failure, Ready, Request, STATUS_ERROR, STATUS_OK,
        STATUS_READY, VERSION,
    },
};

struct Args {
    runtime: PathBuf,
    detection: PathBuf,
    classification: PathBuf,
    recognition: PathBuf,
    dictionary: PathBuf,
    threads: usize,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dreampaper-ocr: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let started = Instant::now();
    let mut engine = Engine::load(
        ModelPaths {
            runtime: &args.runtime,
            detection: &args.detection,
            classification: &args.classification,
            recognition: &args.recognition,
            dictionary: &args.dictionary,
        },
        args.threads,
    )
    .map_err(|error| error.to_string())?;

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    write_response(
        &mut writer,
        STATUS_READY,
        0,
        &Ready {
            protocol: VERSION,
            engine: "dreampaper-ppocr".into(),
            init_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            runtime: ort::info().to_owned(),
        },
    )
    .map_err(|error| error.to_string())?;

    loop {
        match read_request(&mut reader) {
            Ok(Some(Request::Analyze {
                id,
                width,
                height,
                rgb,
            })) => match engine.analyze(width, height, rgb) {
                Ok(result) => write_response(&mut writer, STATUS_OK, id, &result)
                    .map_err(|error| error.to_string())?,
                Err(error) => write_response(
                    &mut writer,
                    STATUS_ERROR,
                    id,
                    &Failure {
                        code: "ocr_failed",
                        message: error.to_string(),
                    },
                )
                .map_err(|error| error.to_string())?,
            },
            Ok(Some(Request::Shutdown { id })) => {
                write_response(
                    &mut writer,
                    STATUS_OK,
                    id,
                    &serde_json::json!({"stopped": true}),
                )
                .map_err(|error| error.to_string())?;
                return Ok(());
            }
            Ok(None) => return Ok(()),
            Err(error) => {
                let _ = write_response(
                    &mut writer,
                    STATUS_ERROR,
                    0,
                    &Failure {
                        code: "protocol_error",
                        message: error.to_string(),
                    },
                );
                return Err(error.to_string());
            }
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let mut values = env::args().skip(1);
    let mut runtime = None;
    let mut detection = None;
    let mut classification = None;
    let mut recognition = None;
    let mut dictionary = None;
    let mut threads = 4_usize;

    while let Some(flag) = values.next() {
        let value = values.next().ok_or_else(|| format!("参数缺少值：{flag}"))?;
        match flag.as_str() {
            "--runtime" => runtime = Some(PathBuf::from(value)),
            "--det" => detection = Some(PathBuf::from(value)),
            "--cls" => classification = Some(PathBuf::from(value)),
            "--rec" => recognition = Some(PathBuf::from(value)),
            "--dict" => dictionary = Some(PathBuf::from(value)),
            "--threads" => {
                threads = value.parse().map_err(|_| "线程数必须是整数".to_owned())?;
                if !matches!(threads, 1 | 2 | 4 | 6 | 8 | 12) {
                    return Err("线程数必须是 1、2、4、6、8 或 12".into());
                }
            }
            _ => return Err(format!("不支持的参数：{flag}")),
        }
    }

    Ok(Args {
        runtime: runtime.ok_or_else(|| "缺少 --runtime".to_owned())?,
        detection: detection.ok_or_else(|| "缺少 --det".to_owned())?,
        classification: classification.ok_or_else(|| "缺少 --cls".to_owned())?,
        recognition: recognition.ok_or_else(|| "缺少 --rec".to_owned())?,
        dictionary: dictionary.ok_or_else(|| "缺少 --dict".to_owned())?,
        threads,
    })
}
