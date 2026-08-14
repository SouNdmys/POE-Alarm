//! Release-only resource gate for the production runtime composition.
//!
//! This diagnostic intentionally lives outside the application hot path. `live`
//! uses the production GDI capture, WinRT/Paddle recognizer, runtime actor and
//! native alert service. `screenshot` replaces only the unavailable live game
//! frame source with a fixed real PNG/JPEG while retaining production WIC,
//! recognition and actor ownership.

use std::env;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use poe_alarm_alert_win::AlertServiceConfig;
use poe_alarm_platform_win::ValidatedWave;
use poe_alarm_recognition::PaddleBackendConfig;
use poe_alarm_runtime::{
    ProductionRuntimeConfig, RuntimeEvent, RuntimeHandle, RuntimeOperation, RuntimeRequestId,
    RuntimeState, ScreenshotRequest,
};
use poe_alarm_settings::{AppSettings, GameProfile, ScreenRegion};

const POLL_DELAY: Duration = Duration::from_millis(2);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Live,
    Screenshot,
}

struct Arguments {
    mode: Mode,
    duration: Duration,
    image: Option<PathBuf>,
    game: GameProfile,
    language: String,
    template: String,
    region: ScreenRegion,
    paddle: PaddleBackendConfig,
    wave: PathBuf,
    maximum_cycles: Option<u64>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("poe-alarm-runtime-soak: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments = Arguments::parse()?;
    let wave = ValidatedWave::open(&arguments.wave)
        .map_err(|error| format!("invalid diagnostic WAV: {error}"))?;
    let runtime = RuntimeHandle::start_production(ProductionRuntimeConfig {
        alert: AlertServiceConfig::new(wave),
        paddle: Some(arguments.paddle.clone()),
    })
    .map_err(|error| format!("could not start production runtime: {error}"))?;
    wait_for_ready(&runtime)?;

    println!("SOAK_READY pid={}", std::process::id());
    println!(
        "SOAK_SCOPE mode={:?} duration_seconds={:.3} frame_source={} components=production-runtime+native-alert+WinRT+Paddle+{}",
        arguments.mode,
        arguments.duration.as_secs_f64(),
        match arguments.mode {
            Mode::Live => "live-GDI-desktop",
            Mode::Screenshot => "fixed-real-screenshot-not-live-gameplay",
        },
        match arguments.mode {
            Mode::Live => "GDI",
            Mode::Screenshot => "WIC",
        }
    );

    let result = match arguments.mode {
        Mode::Live => run_live(&runtime, &arguments),
        Mode::Screenshot => run_screenshots(&runtime, &arguments),
    };
    let shutdown_result = shutdown(&runtime);
    result?;
    shutdown_result?;
    Ok(())
}

fn run_live(runtime: &RuntimeHandle, arguments: &Arguments) -> Result<(), String> {
    let settings = arguments.settings();
    runtime
        .start(settings)
        .map_err(|error| format!("could not queue live monitoring: {error}"))?;
    wait_for_state(runtime, RuntimeState::Monitoring, STARTUP_TIMEOUT)?;
    let started = Instant::now();
    let mut snapshots = 0_u64;
    let mut maximum_scan_count = 0_u64;
    while started.elapsed() < arguments.duration {
        drain_events(runtime, |event| match event {
            RuntimeEvent::MonitorSnapshot { snapshot, .. } => {
                snapshots = snapshots.saturating_add(1);
                maximum_scan_count = maximum_scan_count.max(snapshot.scan_count);
                Ok(())
            }
            RuntimeEvent::MatchFound { .. } => Err(
                "the live diagnostic target unexpectedly matched; choose an impossible template"
                    .to_owned(),
            ),
            RuntimeEvent::Fault {
                operation, detail, ..
            } => Err(format!(
                "production runtime {operation:?} faulted: {detail}"
            )),
            _ => Ok(()),
        })?;
        thread::sleep(POLL_DELAY);
    }
    runtime
        .stop()
        .map_err(|error| format!("could not queue live stop: {error}"))?;
    wait_for_state(runtime, RuntimeState::Idle, SHUTDOWN_TIMEOUT)?;
    println!(
        "SOAK_COMPLETE mode=live wall_seconds={:.3} snapshots={snapshots} scans={maximum_scan_count}",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn run_screenshots(runtime: &RuntimeHandle, arguments: &Arguments) -> Result<(), String> {
    let image = arguments
        .image
        .as_ref()
        .ok_or_else(|| "--image is required for screenshot mode".to_owned())?;
    if !image.is_file() {
        return Err(format!(
            "fixed screenshot does not exist: {}",
            image.display()
        ));
    }
    let settings = arguments.settings();
    let started = Instant::now();
    let mut cycles = 0_u64;
    let mut recognition = Duration::ZERO;
    while started.elapsed() < arguments.duration
        && arguments
            .maximum_cycles
            .is_none_or(|maximum| cycles < maximum)
    {
        let request_id = RuntimeRequestId(cycles.saturating_add(1));
        runtime
            .test_screenshot(ScreenshotRequest::new(request_id, settings.clone(), image))
            .map_err(|error| format!("could not queue screenshot {request_id:?}: {error}"))?;
        loop {
            let Some(event) = runtime.try_next_event() else {
                thread::sleep(POLL_DELAY);
                continue;
            };
            match event {
                RuntimeEvent::ScreenshotCompleted(report) if report.request_id == request_id => {
                    if report.evaluation.is_match {
                        return Err(
                            "the fixed screenshot target unexpectedly matched; choose an impossible template"
                                .to_owned(),
                        );
                    }
                    recognition = recognition.saturating_add(report.recognition_elapsed);
                    cycles = cycles.saturating_add(1);
                    break;
                }
                RuntimeEvent::Fault {
                    operation: RuntimeOperation::Screenshot,
                    detail,
                    ..
                } => return Err(format!("production screenshot faulted: {detail}")),
                RuntimeEvent::MatchFound { .. } => {
                    return Err(
                        "the fixed screenshot target unexpectedly matched; choose an impossible template"
                            .to_owned(),
                    );
                }
                _ => {}
            }
        }
    }
    println!(
        "SOAK_COMPLETE mode=screenshot wall_seconds={:.3} cycles={cycles} recognition_seconds={:.3}",
        started.elapsed().as_secs_f64(),
        recognition.as_secs_f64()
    );
    Ok(())
}

fn wait_for_ready(runtime: &RuntimeHandle) -> Result<(), String> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        match runtime.try_next_event() {
            Some(RuntimeEvent::Ready) => return Ok(()),
            Some(RuntimeEvent::Fault { detail, .. }) => {
                return Err(format!("production runtime startup faulted: {detail}"));
            }
            _ => thread::sleep(POLL_DELAY),
        }
    }
    Err("production runtime did not become ready within 15 seconds".to_owned())
}

fn wait_for_state(
    runtime: &RuntimeHandle,
    expected: RuntimeState,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match runtime.try_next_event() {
            Some(RuntimeEvent::StateChanged { state, .. }) if state == expected => return Ok(()),
            Some(RuntimeEvent::Fault {
                operation, detail, ..
            }) => {
                return Err(format!(
                    "production runtime {operation:?} faulted: {detail}"
                ));
            }
            _ => thread::sleep(POLL_DELAY),
        }
    }
    Err(format!(
        "production runtime did not reach {expected:?} within {:.1} seconds",
        timeout.as_secs_f64()
    ))
}

fn drain_events(
    runtime: &RuntimeHandle,
    mut consume: impl FnMut(RuntimeEvent) -> Result<(), String>,
) -> Result<(), String> {
    while let Some(event) = runtime.try_next_event() {
        consume(event)?;
    }
    Ok(())
}

fn shutdown(runtime: &RuntimeHandle) -> Result<(), String> {
    runtime
        .shutdown()
        .map_err(|error| format!("could not queue runtime shutdown: {error}"))?;
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    while Instant::now() < deadline {
        match runtime.try_next_event() {
            Some(RuntimeEvent::ShutdownComplete { elapsed, .. }) => {
                println!(
                    "SOAK_SHUTDOWN milliseconds={:.3}",
                    elapsed.as_secs_f64() * 1_000.0
                );
                return Ok(());
            }
            Some(RuntimeEvent::Fault { detail, .. }) => {
                return Err(format!("runtime faulted while shutting down: {detail}"));
            }
            _ => thread::sleep(POLL_DELAY),
        }
    }
    Err("production runtime did not confirm shutdown within 3 seconds".to_owned())
}

impl Arguments {
    fn parse() -> Result<Self, String> {
        let values = env::args().skip(1).collect::<Vec<_>>();
        if values.iter().any(|value| value == "--help") {
            println!(
                "usage: poe-alarm-runtime-soak --mode live|screenshot --duration-seconds N \
                 --game poe1|poe2 --language en|zh-TW --template TEXT --region x,y,w,h \
                 --onnx-runtime PATH --model PATH --dictionary PATH --wave PATH \
                 [--image PATH] [--maximum-cycles N]"
            );
            std::process::exit(0);
        }
        let mode = match required(&values, "--mode")? {
            "live" => Mode::Live,
            "screenshot" => Mode::Screenshot,
            value => return Err(format!("unknown --mode '{value}'")),
        };
        let duration_seconds = parse_f64(&values, "--duration-seconds")?;
        if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
            return Err("--duration-seconds must be a finite positive number".to_owned());
        }
        let game = match required(&values, "--game")?.to_ascii_lowercase().as_str() {
            "poe1" | "1" => GameProfile::Poe1,
            "poe2" | "2" => GameProfile::Poe2,
            value => return Err(format!("unknown --game '{value}'")),
        };
        let language = match required(&values, "--language")? {
            value if value.eq_ignore_ascii_case("en") => "en".to_owned(),
            value if value.eq_ignore_ascii_case("zh-TW") => "zh-TW".to_owned(),
            value => return Err(format!("unknown --language '{value}'")),
        };
        let region = parse_region(required(&values, "--region")?)?;
        let mut paddle = PaddleBackendConfig::new(
            required_path(&values, "--onnx-runtime")?,
            required_path(&values, "--model")?,
            required_path(&values, "--dictionary")?,
        );
        if let Some(threads) = optional(&values, "--paddle-threads") {
            paddle.threads = threads
                .parse::<usize>()
                .map_err(|error| format!("invalid --paddle-threads: {error}"))?;
        }
        let maximum_cycles = optional(&values, "--maximum-cycles")
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|error| format!("invalid --maximum-cycles: {error}"))
            })
            .transpose()?;
        Ok(Self {
            mode,
            duration: Duration::from_secs_f64(duration_seconds),
            image: optional(&values, "--image").map(PathBuf::from),
            game,
            language,
            template: required(&values, "--template")?.to_owned(),
            region,
            paddle,
            wave: required_path(&values, "--wave")?,
            maximum_cycles,
        })
    }

    fn settings(&self) -> AppSettings {
        let mut settings = AppSettings {
            selected_game_profile: self.game,
            keep_hud_visible: false,
            custom_alert_sound_path: Some(self.wave.to_string_lossy().into_owned()),
            ..AppSettings::default()
        };
        let profile = settings.profiles.get_mut(self.game);
        profile.capture_region = Some(self.region);
        profile.ocr_language.clone_from(&self.language);
        profile
            .selected_rules_mut()
            .target_affix
            .clone_from(&self.template);
        settings
    }
}

fn optional<'a>(values: &'a [String], name: &str) -> Option<&'a str> {
    values
        .iter()
        .position(|value| value == name)
        .and_then(|index| values.get(index + 1))
        .map(String::as_str)
}

fn required<'a>(values: &'a [String], name: &str) -> Result<&'a str, String> {
    optional(values, name).ok_or_else(|| format!("missing required option {name}"))
}

fn required_path(values: &[String], name: &str) -> Result<PathBuf, String> {
    let path = Path::new(required(values, name)?).to_path_buf();
    if !path.is_file() {
        return Err(format!("{name} is not a file: {}", path.display()));
    }
    Ok(path)
}

fn parse_f64(values: &[String], name: &str) -> Result<f64, String> {
    required(values, name)?
        .parse::<f64>()
        .map_err(|error| format!("invalid {name}: {error}"))
}

fn parse_region(value: &str) -> Result<ScreenRegion, String> {
    let values = value
        .split(',')
        .map(str::trim)
        .map(str::parse::<i32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("invalid --region: {error}"))?;
    if values.len() != 4 {
        return Err("--region must be x,y,width,height".to_owned());
    }
    let region = ScreenRegion::new(values[0], values[1], values[2], values[3]);
    if !region.is_valid() {
        return Err("--region width and height must be positive".to_owned());
    }
    Ok(region)
}
