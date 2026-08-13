use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use poe_alarm_ocr_paddle::{
    PaddleAssetPaths, PaddleAssets, PaddleCtcSession, PaddleSessionConfig, initialize_onnx_runtime,
    run_self_test,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("paddle-ocr-probe: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let arguments = parse_arguments()?;
    let assets = PaddleAssetPaths::new(arguments.model, arguments.dictionary);
    let report = run_self_test(&arguments.runtime, &assets).map_err(|error| error.to_string())?;
    println!("self-test: ok");
    println!(
        "model: {} bytes sha256={}",
        report.model_bytes, report.model_sha256
    );
    println!(
        "dictionary: {} bytes entries={} sha256={}",
        report.dictionary_bytes, report.dictionary_entries, report.dictionary_sha256
    );
    println!(
        "model I/O: {}{:?} -> {}{:?}",
        report.contract.input_name,
        report.contract.input_shape,
        report.contract.output_name,
        report.contract.output_shape
    );
    println!(
        "cold: session={:.3}ms inference={:.3}ms total={:.3}ms text={:?}",
        milliseconds(report.session_creation_elapsed),
        milliseconds(report.inference.total_elapsed()),
        milliseconds(report.total_elapsed),
        report.inference.text
    );

    initialize_onnx_runtime(&arguments.runtime).map_err(|error| error.to_string())?;
    let loaded = PaddleAssets::load(&assets).map_err(|error| error.to_string())?;
    let mut session = PaddleCtcSession::from_assets(&loaded, PaddleSessionConfig::default())
        .map_err(|error| error.to_string())?;
    let mut pixels = vec![0_u8; 64 * 48 * 4];
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = u8::MAX;
    }
    for _ in 0..arguments.warmup {
        let image = poe_alarm_ocr_paddle::ImageView::bgra8(64, 48, 64 * 4, &pixels)
            .map_err(|error| error.to_string())?;
        session
            .recognize(image)
            .map_err(|error| error.to_string())?;
    }
    let mut samples = Vec::with_capacity(arguments.iterations);
    for _ in 0..arguments.iterations {
        let image = poe_alarm_ocr_paddle::ImageView::bgra8(64, 48, 64 * 4, &pixels)
            .map_err(|error| error.to_string())?;
        samples.push(
            session
                .recognize(image)
                .map_err(|error| error.to_string())?
                .total_elapsed(),
        );
    }
    samples.sort_unstable();
    println!(
        "warm synthetic 64x48 BGRA (n={}): min={:.3}ms p50={:.3}ms p95={:.3}ms p99={:.3}ms max={:.3}ms",
        samples.len(),
        milliseconds(samples[0]),
        milliseconds(percentile(&samples, 0.50)),
        milliseconds(percentile(&samples, 0.95)),
        milliseconds(percentile(&samples, 0.99)),
        milliseconds(*samples.last().expect("samples are non-empty")),
    );
    Ok(())
}

struct Arguments {
    runtime: PathBuf,
    model: PathBuf,
    dictionary: PathBuf,
    warmup: usize,
    iterations: usize,
}

fn parse_arguments() -> Result<Arguments, String> {
    let source_assets = PaddleAssetPaths::source_tree();
    let mut runtime = env::var_os("POE_ALARM_ONNXRUNTIME_DLL").map(PathBuf::from);
    let mut model = source_assets.model;
    let mut dictionary = source_assets.dictionary;
    let mut warmup = 3;
    let mut iterations = 20;
    let mut arguments = env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--ort-dylib" => runtime = Some(next_path(&mut arguments, "--ort-dylib")?),
            "--model" => model = next_path(&mut arguments, "--model")?,
            "--dictionary" => dictionary = next_path(&mut arguments, "--dictionary")?,
            "--warmup" => warmup = next_usize(&mut arguments, "--warmup")?,
            "--iterations" => iterations = next_usize(&mut arguments, "--iterations")?,
            "--help" | "-h" => return Err(usage().to_owned()),
            unknown => return Err(format!("unknown argument '{unknown}'\n{}", usage())),
        }
    }
    let runtime = runtime.or_else(discover_repository_runtime).ok_or_else(|| {
        format!(
            "no packaged ONNX Runtime library found; pass --ort-dylib PATH or set POE_ALARM_ONNXRUNTIME_DLL\n{}",
            usage()
        )
    })?;
    if warmup == 0 || iterations == 0 {
        return Err("--warmup and --iterations must be positive".to_owned());
    }
    Ok(Arguments {
        runtime,
        model,
        dictionary,
        warmup,
        iterations,
    })
}

fn next_path(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
    option: &str,
) -> Result<PathBuf, String> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{option} requires a path"))
}

fn next_usize(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
    option: &str,
) -> Result<usize, String> {
    let value = arguments
        .next()
        .ok_or_else(|| format!("{option} requires a number"))?;
    value
        .to_string_lossy()
        .parse()
        .map_err(|_| format!("{option} requires a positive integer"))
}

fn discover_repository_runtime() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    [
        root.join(".packages/microsoft.ml.onnxruntime/1.28.0/runtimes/win-x64/native/onnxruntime.dll"),
        root.join(".dotnet-home/.nuget/packages/microsoft.ml.onnxruntime/1.28.0/runtimes/win-x64/native/onnxruntime.dll"),
        root.join("src/PoeAlarm.App/bin/Release/net10.0-windows10.0.19041.0/win-x64/onnxruntime.dll"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn percentile(sorted: &[Duration], percentile: f64) -> Duration {
    let rank = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[rank]
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn usage() -> &'static str {
    "usage: paddle-ocr-probe [--ort-dylib PATH] [--model PATH] [--dictionary PATH] [--warmup N] [--iterations N]"
}
