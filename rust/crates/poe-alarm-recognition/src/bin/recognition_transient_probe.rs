#[cfg(windows)]
mod common;

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("recognition-transient-probe: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("recognition-transient-probe requires Windows.Media.Ocr and WIC");
    std::process::exit(2);
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct Manifest {
    screenshots: Vec<Screenshot>,
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct Screenshot {
    image: String,
    roi: [i64; 4],
    cases: Vec<Case>,
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct Case {
    id: String,
    band: usize,
    template: String,
}

#[cfg(windows)]
#[derive(Default)]
struct TrialState {
    target_visible: std::sync::atomic::AtomicBool,
    guard_held: std::sync::atomic::AtomicBool,
    detected: std::sync::atomic::AtomicBool,
    detected_on_target: std::sync::atomic::AtomicBool,
    target_captures: std::sync::atomic::AtomicUsize,
}

#[cfg(windows)]
struct ReplayCapture {
    state: std::sync::Arc<TrialState>,
    absent: poe_alarm_vision::CapturedFrame,
    target: poe_alarm_vision::CapturedFrame,
}

#[cfg(windows)]
impl poe_alarm_monitoring::FrameCapture for ReplayCapture {
    type Error = std::convert::Infallible;

    fn capture_into(
        &mut self,
        _region: poe_alarm_vision::CaptureRegion,
        destination: &mut poe_alarm_vision::CapturedFrame,
    ) -> Result<(), Self::Error> {
        use std::sync::atomic::Ordering;

        if self.state.target_visible.load(Ordering::Acquire) {
            self.state.target_captures.fetch_add(1, Ordering::AcqRel);
            *destination = self.target.clone();
        } else {
            *destination = self.absent.clone();
        }
        Ok(())
    }
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::{Duration, Instant};

    use poe_alarm_core::FullLineAffixMatcher;
    use poe_alarm_monitoring::{Monitor, MonitorEvent, MonitorPlan, SystemClock};
    use poe_alarm_recognition::{
        GameVersion, ProductionRecognizer, RecognitionLanguage, RecognitionProfile,
    };
    use poe_alarm_vision::{CaptureRegion, WicScreenshotDecoder};

    const SCENARIOS: [(&str, &str); 4] = [
        ("traditional-ocr-cases.json", "weapon-attack-speed"),
        ("traditional-ocr-cases.json", "weapon-shrine-on-kill"),
        (
            "traditional-ocr-new-a.json",
            "rare-belt-stun-block-recovery",
        ),
        ("traditional-ocr-cases.json", "cluster-sadist"),
    ];

    fn roi(values: [i64; 4]) -> Result<CaptureRegion, String> {
        common::parse_roi(&format!(
            "{},{},{},{}",
            values[0], values[1], values[2], values[3]
        ))
    }

    fn find_image(image_root: &Path, name: &str) -> Result<PathBuf, String> {
        let candidate = image_root.join(name);
        candidate
            .is_file()
            .then_some(candidate)
            .ok_or_else(|| format!("missing private screenshot {name}"))
    }

    let arguments = common::Arguments::collect();
    let image_root = Path::new(arguments.required("--image-root")?);
    let manifest_root = Path::new(arguments.required("--manifest-root")?);
    let output_path = Path::new(arguments.required("--csv")?);
    let trials = arguments
        .value("--trials")
        .unwrap_or("20")
        .parse::<usize>()
        .map_err(|error| format!("invalid --trials: {error}"))?;
    if trials < 20 {
        return Err("--trials must be at least 20".to_owned());
    }
    let durations = arguments
        .value("--durations")
        .unwrap_or("30,40,50,60,80,100")
        .split(',')
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("invalid dwell {value}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let paddle = common::paddle_configuration(&arguments)?
        .ok_or_else(|| "--onnx-runtime is required".to_owned())?;
    let decoder = WicScreenshotDecoder::new().map_err(|error| error.to_string())?;
    let profile = RecognitionProfile {
        game: GameVersion::Poe1,
        language: RecognitionLanguage::TraditionalChinese,
    };
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut output =
        std::io::BufWriter::new(fs::File::create(output_path).map_err(|error| error.to_string())?);
    writeln!(output, "case,dwell_ms,trial,hit,timely,overroll,captured_miss,false_alert,physical_clicks,accepted,swallowed,target_captures,onset_to_decision_ms")
        .map_err(|error| error.to_string())?;
    let mut failures = Vec::new();

    for (manifest_name, case_id) in SCENARIOS {
        let manifest: Manifest = serde_json::from_str(
            &fs::read_to_string(manifest_root.join(manifest_name))
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let (screenshot, case) = manifest
            .screenshots
            .iter()
            .find_map(|screenshot| {
                screenshot
                    .cases
                    .iter()
                    .find(|case| case.id == case_id)
                    .map(|case| (screenshot, case))
            })
            .ok_or_else(|| format!("missing case {case_id}"))?;
        let target_frame = decoder
            .decode(
                &find_image(image_root, &screenshot.image)?,
                Some(roi(screenshot.roi)?),
            )
            .map_err(|error| error.to_string())?;
        let absent_frame = erase_band(&target_frame, case.band)?;
        let target =
            FullLineAffixMatcher::new(&case.template).map_err(|error| error.to_string())?;

        for dwell_ms in &durations {
            for trial in 1..=trials {
                let state = Arc::new(TrialState::default());
                let event_state = Arc::clone(&state);
                let events = move |event: MonitorEvent| match event {
                    MonitorEvent::InputGuardRequested { .. } => {
                        event_state.guard_held.store(true, Ordering::Release);
                    }
                    MonitorEvent::InputGuardReleased { .. } => {
                        event_state.guard_held.store(false, Ordering::Release);
                    }
                    MonitorEvent::Detection(_) => {
                        event_state.detected_on_target.store(
                            event_state.target_visible.load(Ordering::Acquire),
                            Ordering::Release,
                        );
                        event_state.detected.store(true, Ordering::Release);
                    }
                    MonitorEvent::Snapshot(_) => {}
                };
                let capture = ReplayCapture {
                    state: Arc::clone(&state),
                    absent: absent_frame.clone(),
                    target: target_frame.clone(),
                };
                let recognizer = ProductionRecognizer::start(profile, Some(paddle.clone()))
                    .map_err(|error| error.to_string())?;
                let mut monitor = Monitor::new(capture, recognizer, SystemClock::default(), events);
                monitor
                    .start(MonitorPlan::Quick(target.clone()), target_frame.region())
                    .map_err(|error| error.to_string())?;
                let started = Instant::now();
                let deadline = started + Duration::from_secs(3);
                let dwell = Duration::from_millis(*dwell_ms);
                let mut next_click = started + dwell;
                let mut onset = None;
                let mut decision = None;
                let mut physical_clicks = 0_usize;
                let mut accepted = 0_usize;
                let mut swallowed = 0_usize;
                let mut overroll = 0_usize;
                while Instant::now() < deadline {
                    if state.detected.load(Ordering::Acquire) {
                        decision = Some(Instant::now());
                        break;
                    }
                    let now = Instant::now();
                    if now >= next_click {
                        physical_clicks += 1;
                        if state.guard_held.load(Ordering::Acquire) {
                            swallowed += 1;
                        } else {
                            accepted += 1;
                            let was_target = state.target_visible.fetch_xor(true, Ordering::AcqRel);
                            if was_target {
                                overroll += 1;
                            } else {
                                onset.get_or_insert(now);
                            }
                        }
                        next_click += dwell;
                    }
                    thread::sleep(Duration::from_micros(250));
                }
                monitor.stop().map_err(|error| error.to_string())?;
                let hit = state.detected.load(Ordering::Acquire)
                    && state.detected_on_target.load(Ordering::Acquire);
                let false_alert = state.detected.load(Ordering::Acquire) && !hit;
                let target_captures = state.target_captures.load(Ordering::Acquire);
                let captured_miss = target_captures > 0 && !hit;
                let timely = hit && overroll == 0;
                let onset_to_decision_ms = onset
                    .zip(decision)
                    .map(|(onset, decision)| decision.duration_since(onset).as_secs_f64() * 1_000.0)
                    .unwrap_or(-1.0);
                writeln!(
                    output,
                    "{case_id},{dwell_ms},{trial},{hit},{timely},{overroll},{captured_miss},{false_alert},{physical_clicks},{accepted},{swallowed},{target_captures},{onset_to_decision_ms:.6}"
                )
                .map_err(|error| error.to_string())?;
                if !hit || !timely || overroll != 0 || captured_miss || false_alert {
                    failures.push(format!(
                        "{case_id} dwell={dwell_ms} trial={trial}: hit={hit} timely={timely} overroll={overroll} captured_miss={captured_miss} false={false_alert}"
                    ));
                }
            }
        }
    }
    output.flush().map_err(|error| error.to_string())?;
    if !failures.is_empty() {
        return Err(format!(
            "{} transient trial(s) failed; first failures:\n{}",
            failures.len(),
            failures.into_iter().take(20).collect::<Vec<_>>().join("\n")
        ));
    }
    println!(
        "transient gate passed: {} cases x {} dwell intervals x {trials} trials",
        SCENARIOS.len(),
        durations.len()
    );
    Ok(())
}

#[cfg(windows)]
fn erase_band(
    frame: &poe_alarm_vision::CapturedFrame,
    band_index: usize,
) -> Result<poe_alarm_vision::CapturedFrame, String> {
    use std::time::SystemTime;

    use poe_alarm_vision::{
        BandDetectionSettings, BlueMaskIntensityMode, BlueMaskSettings, CapturedFrame,
        PhysicalBandDetector, build_blue_mask,
    };

    let mask = build_blue_mask(
        frame,
        BlueMaskSettings {
            minimum_blue: 100,
            minimum_blue_dominance: 14,
            maximum_warm_channel_difference: 72,
            intensity_mode: BlueMaskIntensityMode::Dominance,
        },
    );
    let detected = PhysicalBandDetector::new()
        .detect(&mask, BandDetectionSettings::default())
        .map_err(|error| error.to_string())?;
    let band = detected.bands.get(band_index).ok_or_else(|| {
        format!(
            "band {band_index} is outside {} bands",
            detected.bands.len()
        )
    })?;
    let content = band.crop.content_rect;
    let mut pixels = frame.bgra_pixels().to_vec();
    for y in content.y..content.bottom_exclusive() {
        for x in content.x..content.right_exclusive() {
            if mask.intensity_at(x, y).unwrap_or(0) == 0 {
                continue;
            }
            let pixel = y * frame.stride() + x * 4;
            pixels[pixel..pixel + 3].fill(0);
        }
    }
    CapturedFrame::from_bgra(frame.region(), frame.stride(), pixels, SystemTime::now())
        .map_err(|error| error.to_string())
}
