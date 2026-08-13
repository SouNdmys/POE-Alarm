#[cfg(windows)]
mod common;

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("recognition-manifest-probe: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("recognition-manifest-probe requires Windows.Media.Ocr and WIC");
    std::process::exit(2);
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct Manifest {
    #[serde(default)]
    locale: String,
    #[serde(default)]
    game: String,
    #[serde(default)]
    roi: Option<[i64; 4]>,
    screenshots: Vec<Screenshot>,
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct Screenshot {
    image: String,
    #[serde(default)]
    roi: Option<[i64; 4]>,
    #[serde(default, rename = "expectedBandCount")]
    expected_band_count: Option<usize>,
    cases: Vec<Case>,
    #[serde(default, rename = "negativeCases", alias = "negative_cases")]
    negative_cases: Vec<NegativeCase>,
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct NegativeCase {
    id: String,
    kind: NegativeKind,
    template: String,
    #[serde(default, rename = "evidenceTemplate", alias = "evidence_template")]
    evidence_template: Option<String>,
    #[serde(default, rename = "positiveImage", alias = "positive_image")]
    positive_image: Option<String>,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum NegativeKind {
    SemanticNeighbor,
    CrossScreenshot,
}

#[cfg(windows)]
impl NegativeKind {
    fn label(self) -> &'static str {
        match self {
            Self::SemanticNeighbor => "semantic-neighbor",
            Self::CrossScreenshot => "cross-screenshot",
        }
    }
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Case {
    Template(String),
    Detailed {
        id: String,
        band: usize,
        text: String,
        template: String,
    },
}

#[cfg(windows)]
impl Case {
    fn template(&self) -> &str {
        match self {
            Self::Template(value)
            | Self::Detailed {
                template: value, ..
            } => value,
        }
    }

    fn detail(&self) -> Option<(&str, usize, &str)> {
        match self {
            Self::Template(_) => None,
            Self::Detailed { id, band, text, .. } => Some((id, *band, text)),
        }
    }
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    use std::fs;
    use std::path::{Path, PathBuf};

    use poe_alarm_core::FullLineAffixMatcher;
    use poe_alarm_ocr_paddle::{
        PaddleAssetPaths, PaddleAssets, PaddleCtcSession, PaddleSessionConfig,
        initialize_onnx_runtime,
    };
    use poe_alarm_recognition::{ProductionRecognizer, RecognitionLanguage};
    use poe_alarm_vision::{CaptureRegion, WicScreenshotDecoder};

    fn manifest_roi(values: [i64; 4]) -> Result<CaptureRegion, String> {
        common::parse_roi(&format!(
            "{},{},{},{}",
            values[0], values[1], values[2], values[3]
        ))
    }

    fn find_image(
        image_root: &Path,
        manifest_path: &Path,
        image_name: &str,
    ) -> Result<PathBuf, String> {
        let section = manifest_path
            .parent()
            .and_then(Path::file_name)
            .map(PathBuf::from);
        let candidates = section
            .into_iter()
            .map(|section| image_root.join(section).join(image_name))
            .chain(std::iter::once(image_root.join(image_name)))
            .collect::<Vec<_>>();
        candidates
            .into_iter()
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| {
                format!(
                    "image '{}' was not found under --image-root {}",
                    image_name,
                    image_root.display()
                )
            })
    }

    let arguments = common::Arguments::collect();
    if arguments.has("--help") {
        println!(
            "The POE1 English release gate also accepts --mode quick|structured and --require-negative-contract."
        );
        println!(
            "usage: recognition-manifest-probe --manifest JSON --image-root PATH \\\n+             [--game poe1|poe2] [--language en|zh-TW] \\\n+             [--onnx-runtime PATH --model PATH --dictionary PATH]"
        );
        println!(
            "physical layout audit: --show-physical-matches --physical-match-csv PATH [--audit-physical-matches]"
        );
        return Ok(());
    }
    let manifest_path = Path::new(arguments.required("--manifest")?);
    let image_root = Path::new(arguments.required("--image-root")?);
    let json = fs::read_to_string(manifest_path)
        .map_err(|error| format!("could not read {}: {error}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&json)
        .map_err(|error| format!("invalid manifest {}: {error}", manifest_path.display()))?;
    let inferred_game = if manifest.game.contains('2')
        || manifest_path
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("poe2")
    {
        "poe2"
    } else {
        "poe1"
    };
    let inferred_language = if manifest.locale.to_ascii_lowercase().starts_with("en") {
        "en"
    } else {
        "zh-TW"
    };
    let profile = common::parse_profile(
        arguments.value("--game").unwrap_or(inferred_game),
        arguments.value("--language").unwrap_or(inferred_language),
    )?;
    let negative_contract = if arguments.has("--audit-physical-matches") {
        NegativeContractSummary::default()
    } else {
        validate_negative_contract(&manifest)?
    };
    if arguments.has("--require-negative-contract") {
        if profile.game != poe_alarm_recognition::GameVersion::Poe1
            || profile.language != RecognitionLanguage::English
        {
            return Err(
                "--require-negative-contract is the POE1 English release gate; use --game poe1 --language en"
                    .to_owned(),
            );
        }
        if manifest.screenshots.len() < 2 {
            return Err(
                "POE1 English release gate requires at least two independent screenshots"
                    .to_owned(),
            );
        }
        if negative_contract.positive_cases == 0 {
            return Err("POE1 English release gate requires positive cases".to_owned());
        }
        if negative_contract.semantic_neighbors == 0 {
            return Err(
                "POE1 English release gate requires at least one semantic-neighbor negative"
                    .to_owned(),
            );
        }
        if negative_contract.cross_screenshots == 0 {
            return Err(
                "POE1 English release gate requires at least one cross-screenshot negative"
                    .to_owned(),
            );
        }
    }
    let mode = arguments.value("--mode").unwrap_or("structured");
    if !matches!(mode, "quick" | "structured") {
        return Err("--mode must be quick or structured".to_owned());
    }
    let paddle = common::paddle_configuration(&arguments)?;
    if paddle.is_none() && !arguments.has("--allow-no-fallback") {
        return Err(
            "Paddle fallback is required for the regression gate; pass --onnx-runtime PATH or explicitly opt out with --allow-no-fallback"
                .to_owned(),
        );
    }
    let has_detailed_cases = manifest
        .screenshots
        .iter()
        .flat_map(|screenshot| &screenshot.cases)
        .any(|case| case.detail().is_some());
    let mut detailed_session = if has_detailed_cases {
        let configuration = paddle.as_ref().ok_or_else(|| {
            "detailed physical-band verification requires configured Paddle assets".to_owned()
        })?;
        initialize_onnx_runtime(&configuration.runtime_library)
            .map_err(|error| error.to_string())?;
        let assets = PaddleAssets::load(&PaddleAssetPaths::new(
            &configuration.model,
            &configuration.dictionary,
        ))
        .map_err(|error| error.to_string())?;
        let session = PaddleSessionConfig {
            threads: configuration.threads,
            right_padding: 8,
            allow_latin_target_support: profile.language == RecognitionLanguage::English,
            ..PaddleSessionConfig::default()
        };
        Some(PaddleCtcSession::from_assets(&assets, session).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let mut recognizer = if let Some(configuration) = paddle {
        ProductionRecognizer::start(profile, Some(configuration))
    } else {
        ProductionRecognizer::start_without_localized_fallback_for_diagnostics(profile)
    }
    .map_err(|error| error.to_string())?;
    let decoder = WicScreenshotDecoder::new().map_err(|error| error.to_string())?;
    let mut passed = 0_usize;
    let mut failed = 0_usize;
    let mut primary_passed = 0_usize;
    let mut primary_failed = 0_usize;
    let mut confirmation_passed = 0_usize;
    let mut confirmation_failed = 0_usize;
    let mut assisted_sibling_collisions = 0_usize;
    let mut detailed_row_passed = 0_usize;
    let mut detailed_row_failed = 0_usize;
    let mut detailed_cross_negative_alerts = 0_usize;
    let mut negative_passed = 0_usize;
    let mut negative_failed = 0_usize;
    let mut semantic_negative_passed = 0_usize;
    let mut semantic_negative_failed = 0_usize;
    let mut cross_negative_passed = 0_usize;
    let mut cross_negative_failed = 0_usize;
    let mut latencies = LatencySeries::default();
    let mut strict_physical_matches = Vec::new();
    let mut sequence_index = 0_usize;
    for screenshot in manifest.screenshots {
        let image = find_image(image_root, manifest_path, &screenshot.image)?;
        let roi = screenshot
            .roi
            .or(manifest.roi)
            .map(manifest_roi)
            .transpose()?;
        let frame = common::decode(&decoder, &image, roi)?;
        let mut targets = Vec::with_capacity(screenshot.cases.len());
        for case in &screenshot.cases {
            targets.push(
                FullLineAffixMatcher::new(case.template())
                    .map_err(|error| format!("{}: {error}", case.template()))?,
            );
        }
        let negative_targets = screenshot
            .negative_cases
            .iter()
            .map(|case| {
                FullLineAffixMatcher::new(&case.template)
                    .map_err(|error| format!("negative {} ({}): {error}", case.id, case.template))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !arguments.has("--audit-physical-matches")
            && screenshot.cases.iter().any(|case| case.detail().is_some())
        {
            let outcome = verify_detailed_physical_rows(
                &frame,
                &screenshot,
                &targets,
                detailed_session
                    .as_mut()
                    .expect("detailed cases created a Paddle session"),
                profile.language == RecognitionLanguage::TraditionalChinese,
            )?;
            detailed_row_passed += outcome.passed;
            detailed_row_failed += outcome.failed;
            detailed_cross_negative_alerts += outcome.cross_negative_alerts;
        }
        if mode == "quick" {
            println!(
                "image={} size={}x{} mode=quick",
                screenshot.image,
                frame.width(),
                frame.height()
            );
            for (case, target) in screenshot.cases.iter().zip(&targets) {
                let sequence = common::recognize_quick_sequence(&mut recognizer, &frame, target)?;
                latencies.record(
                    &sequence,
                    &screenshot.image,
                    case.template(),
                    sequence_index == 0,
                );
                sequence_index += 1;
                if common::strict_target_present(&sequence.primary, target) {
                    primary_passed += 1;
                } else {
                    primary_failed += 1;
                }
                if let Some(confirmation) = &sequence.confirmation {
                    if common::strict_target_present(confirmation, target) {
                        confirmation_passed += 1;
                    } else {
                        confirmation_failed += 1;
                    }
                }
                let result = sequence.final_result_ref();
                if arguments.has("--show-physical-matches") {
                    if let Some(evidence) = target.find_match(&result.lines) {
                        strict_physical_matches.push(PhysicalMatchSample {
                            image: screenshot.image.clone(),
                            target: target.source_template().to_owned(),
                            physical_lines: evidence.physical_line_count,
                            start_line: evidence.start_line_index,
                        });
                        println!(
                            "    PHYSICAL-MATCH lines={} start={} target={}",
                            evidence.physical_line_count,
                            evidence.start_line_index,
                            target.source_template()
                        );
                    } else {
                        println!(
                            "    PHYSICAL-MATCH strict=none target={}",
                            target.source_template()
                        );
                    }
                }
                if common::strict_target_present(result, target) {
                    passed += 1;
                    println!("  PASS {}", case.template());
                } else {
                    failed += 1;
                    println!("  FAIL {}", case.template());
                }
                if arguments.has("--show-lines") {
                    for (index, line) in result.lines.iter().enumerate() {
                        println!("    line[{index}]={line}");
                    }
                }
                if !arguments.has("--skip-cross-negatives") {
                    // Other expected modifiers legitimately coexist in the same screenshot, so
                    // cross-negative safety is applied to this target-conditioned assisted
                    // transcript only. Strict lines remain governed by the normal matcher.
                    if let Some(assisted) = &result.target_assisted_match {
                        for other in &targets {
                            if std::ptr::eq(other, target) {
                                continue;
                            }
                            if other.is_match(&assisted.original_text) {
                                assisted_sibling_collisions += 1;
                                println!(
                                    "    ASSISTED-SIBLING-COLLISION assisted={} also_matches={}",
                                    target.source_template(),
                                    other.source_template()
                                );
                            }
                        }
                    }
                }
            }
            for (case, target) in screenshot.negative_cases.iter().zip(&negative_targets) {
                let sequence = common::recognize_quick_sequence(&mut recognizer, &frame, target)?;
                latencies.record(
                    &sequence,
                    &screenshot.image,
                    &format!("<negative:{}:{}>", case.kind.label(), case.template),
                    sequence_index == 0,
                );
                sequence_index += 1;
                let accepted = common::strict_target_present(sequence.final_result_ref(), target);
                record_negative_outcome(
                    case,
                    accepted,
                    &mut negative_passed,
                    &mut negative_failed,
                    &mut semantic_negative_passed,
                    &mut semantic_negative_failed,
                    &mut cross_negative_passed,
                    &mut cross_negative_failed,
                );
                println!(
                    "  {} NEGATIVE id={} kind={} template={}",
                    if accepted { "FAIL" } else { "PASS" },
                    case.id,
                    case.kind.label(),
                    case.template
                );
                if arguments.has("--show-lines") {
                    for (index, line) in sequence.final_result_ref().lines.iter().enumerate() {
                        println!("    negative_line[{index}]={line}");
                    }
                }
            }
        } else {
            let mut batch_targets = Vec::with_capacity(targets.len() + negative_targets.len());
            batch_targets.extend(targets.iter().cloned());
            batch_targets.extend(negative_targets.iter().cloned());
            let sequence =
                common::recognize_structured_sequence(&mut recognizer, &frame, &batch_targets)?;
            latencies.record(
                &sequence,
                &screenshot.image,
                &format!("<batch:{}>", targets.len()),
                sequence_index == 0,
            );
            sequence_index += 1;
            let result = sequence.final_result_ref();
            println!(
                "image={} size={}x{} ocr_ms={:.3} lines={} mode=structured",
                screenshot.image,
                frame.width(),
                frame.height(),
                result.recognition_elapsed.as_secs_f64() * 1_000.0,
                result.lines.len()
            );
            for (case, target) in screenshot.cases.iter().zip(&targets) {
                if arguments.has("--show-physical-matches") {
                    if let Some(evidence) = target.find_match(&result.lines) {
                        strict_physical_matches.push(PhysicalMatchSample {
                            image: screenshot.image.clone(),
                            target: target.source_template().to_owned(),
                            physical_lines: evidence.physical_line_count,
                            start_line: evidence.start_line_index,
                        });
                        println!(
                            "    PHYSICAL-MATCH lines={} start={} target={}",
                            evidence.physical_line_count,
                            evidence.start_line_index,
                            target.source_template()
                        );
                    } else {
                        println!(
                            "    PHYSICAL-MATCH strict=none target={}",
                            target.source_template()
                        );
                    }
                }
                if common::strict_target_present(&sequence.primary, target) {
                    primary_passed += 1;
                } else {
                    primary_failed += 1;
                }
                if let Some(confirmation) = &sequence.confirmation {
                    if common::strict_target_present(confirmation, target) {
                        confirmation_passed += 1;
                    } else {
                        confirmation_failed += 1;
                    }
                }
                if common::strict_target_present(result, target) {
                    passed += 1;
                    println!("  PASS {}", case.template());
                } else {
                    failed += 1;
                    println!("  FAIL {}", case.template());
                }
            }
            for (case, target) in screenshot.negative_cases.iter().zip(&negative_targets) {
                let accepted = common::strict_target_present(result, target);
                record_negative_outcome(
                    case,
                    accepted,
                    &mut negative_passed,
                    &mut negative_failed,
                    &mut semantic_negative_passed,
                    &mut semantic_negative_failed,
                    &mut cross_negative_passed,
                    &mut cross_negative_failed,
                );
                println!(
                    "  {} NEGATIVE id={} kind={} template={}",
                    if accepted { "FAIL" } else { "PASS" },
                    case.id,
                    case.kind.label(),
                    case.template
                );
            }
            if arguments.has("--show-lines") {
                for (index, line) in result.lines.iter().enumerate() {
                    println!("    line[{index}]={line}");
                }
            }
            if !arguments.has("--skip-cross-negatives") {
                for assisted in &result.assisted_observations {
                    for other in &targets {
                        if other.template().text != assisted.canonical_target
                            && other.is_match(&assisted.original_text)
                        {
                            assisted_sibling_collisions += 1;
                            println!(
                                "    ASSISTED-SIBLING-COLLISION assisted={} also_matches={}",
                                assisted.canonical_target,
                                other.source_template()
                            );
                        }
                    }
                }
            }
        }
    }
    if let Some(csv_path) = arguments.value("--csv") {
        latencies.write_csv(
            std::path::Path::new(csv_path),
            arguments.value("--run-label").unwrap_or("unlabelled"),
            mode,
        )?;
    }
    if let Some(csv_path) = arguments.value("--physical-match-csv") {
        write_physical_match_csv(
            std::path::Path::new(csv_path),
            manifest_path,
            &strict_physical_matches,
        )?;
    }
    latencies.sort();
    let percentile = |values: &[f64], ratio: f64| {
        if values.is_empty() {
            0.0
        } else {
            values[((values.len() - 1) as f64 * ratio).round() as usize]
        }
    };
    println!(
        "summary mode={mode} passed={passed} failed={failed} primary_passed={primary_passed} primary_failed={primary_failed} confirmation_passed={confirmation_passed} confirmation_failed={confirmation_failed} negative_passed={negative_passed} negative_failed={negative_failed} semantic_negative_passed={semantic_negative_passed} semantic_negative_failed={semantic_negative_failed} cross_negative_passed={cross_negative_passed} cross_negative_failed={cross_negative_failed} assisted_sibling_collisions={assisted_sibling_collisions} detailed_row_passed={detailed_row_passed} detailed_row_failed={detailed_row_failed} detailed_cross_negative_alerts={detailed_cross_negative_alerts}\n  total_decision_wall_ms p50={:.3} p95={:.3} max={:.3}\n  primary_wall_ms p50={:.3} p95={:.3} max={:.3}\n  confirmation_wall_ms p50={:.3} p95={:.3} max={:.3}\n  primary_returned_preprocess_ms p50={:.3} p95={:.3} max={:.3}\n  primary_returned_ocr_ms p50={:.3} p95={:.3} max={:.3}\n  final_returned_preprocess_ms p50={:.3} p95={:.3} max={:.3}\n  final_returned_ocr_ms p50={:.3} p95={:.3} max={:.3}",
        percentile(&latencies.total_wall, 0.50),
        percentile(&latencies.total_wall, 0.95),
        percentile(&latencies.total_wall, 1.0),
        percentile(&latencies.primary_wall, 0.50),
        percentile(&latencies.primary_wall, 0.95),
        percentile(&latencies.primary_wall, 1.0),
        percentile(&latencies.confirmation_wall, 0.50),
        percentile(&latencies.confirmation_wall, 0.95),
        percentile(&latencies.confirmation_wall, 1.0),
        percentile(&latencies.primary_preprocessing, 0.50),
        percentile(&latencies.primary_preprocessing, 0.95),
        percentile(&latencies.primary_preprocessing, 1.0),
        percentile(&latencies.primary_ocr, 0.50),
        percentile(&latencies.primary_ocr, 0.95),
        percentile(&latencies.primary_ocr, 1.0),
        percentile(&latencies.final_preprocessing, 0.50),
        percentile(&latencies.final_preprocessing, 0.95),
        percentile(&latencies.final_preprocessing, 1.0),
        percentile(&latencies.final_ocr, 0.50),
        percentile(&latencies.final_ocr, 0.95),
        percentile(&latencies.final_ocr, 1.0)
    );
    if failed > 0
        || negative_failed > 0
        || assisted_sibling_collisions > 0
        || detailed_row_failed > 0
        || detailed_cross_negative_alerts > 0
    {
        return Err(format!(
            "{failed} positive manifest case(s) failed; {negative_failed} declared negative case(s) false-accepted; {assisted_sibling_collisions} assisted sibling-template collision(s); {detailed_row_failed} detailed row failure(s); {detailed_cross_negative_alerts} detailed cross-negative alert(s)"
        ));
    }
    Ok(())
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct NegativeContractSummary {
    positive_cases: usize,
    semantic_neighbors: usize,
    cross_screenshots: usize,
}

#[cfg(windows)]
fn validate_negative_contract(manifest: &Manifest) -> Result<NegativeContractSummary, String> {
    use std::collections::{HashMap, HashSet};

    let positives = manifest
        .screenshots
        .iter()
        .map(|screenshot| {
            (
                screenshot.image.as_str(),
                screenshot
                    .cases
                    .iter()
                    .map(Case::template)
                    .collect::<HashSet<_>>(),
            )
        })
        .collect::<HashMap<_, _>>();
    if positives.len() != manifest.screenshots.len() {
        return Err("manifest image names must be unique".to_owned());
    }

    let mut ids = HashSet::new();
    let mut summary = NegativeContractSummary {
        positive_cases: manifest
            .screenshots
            .iter()
            .map(|screenshot| screenshot.cases.len())
            .sum(),
        ..NegativeContractSummary::default()
    };
    for screenshot in &manifest.screenshots {
        let own_positives = positives
            .get(screenshot.image.as_str())
            .expect("the screenshot map was built from this manifest");
        for negative in &screenshot.negative_cases {
            if negative.id.trim().is_empty() {
                return Err(format!(
                    "{} has a negative case with an empty id",
                    screenshot.image
                ));
            }
            if !ids.insert(negative.id.as_str()) {
                return Err(format!("duplicate negative case id '{}'", negative.id));
            }
            if own_positives.contains(negative.template.as_str()) {
                return Err(format!(
                    "negative '{}' repeats a positive template on {}",
                    negative.id, screenshot.image
                ));
            }
            match negative.kind {
                NegativeKind::SemanticNeighbor => {
                    summary.semantic_neighbors += 1;
                    let evidence = negative.evidence_template.as_deref().ok_or_else(|| {
                        format!(
                            "semantic-neighbor '{}' requires evidenceTemplate naming the similar positive on the same screenshot",
                            negative.id
                        )
                    })?;
                    if !own_positives.contains(evidence) {
                        return Err(format!(
                            "semantic-neighbor '{}' evidenceTemplate '{}' is not a positive case on {}",
                            negative.id, evidence, screenshot.image
                        ));
                    }
                    if negative.positive_image.is_some() {
                        return Err(format!(
                            "semantic-neighbor '{}' must not set positiveImage",
                            negative.id
                        ));
                    }
                }
                NegativeKind::CrossScreenshot => {
                    summary.cross_screenshots += 1;
                    if negative.evidence_template.is_some() {
                        return Err(format!(
                            "cross-screenshot '{}' must not set evidenceTemplate",
                            negative.id
                        ));
                    }
                    let positive_image = negative.positive_image.as_deref().ok_or_else(|| {
                        format!("cross-screenshot '{}' requires positiveImage", negative.id)
                    })?;
                    if positive_image == screenshot.image {
                        return Err(format!(
                            "cross-screenshot '{}' positiveImage must name a different screenshot",
                            negative.id
                        ));
                    }
                    let source_positives = positives.get(positive_image).ok_or_else(|| {
                        format!(
                            "cross-screenshot '{}' references unknown positiveImage '{}'",
                            negative.id, positive_image
                        )
                    })?;
                    if !source_positives.contains(negative.template.as_str()) {
                        return Err(format!(
                            "cross-screenshot '{}' template '{}' is not a positive case on {}",
                            negative.id, negative.template, positive_image
                        ));
                    }
                }
            }
        }
    }
    Ok(summary)
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn record_negative_outcome(
    case: &NegativeCase,
    accepted: bool,
    negative_passed: &mut usize,
    negative_failed: &mut usize,
    semantic_passed: &mut usize,
    semantic_failed: &mut usize,
    cross_passed: &mut usize,
    cross_failed: &mut usize,
) {
    let (total, kind) = if accepted {
        (
            negative_failed,
            match case.kind {
                NegativeKind::SemanticNeighbor => semantic_failed,
                NegativeKind::CrossScreenshot => cross_failed,
            },
        )
    } else {
        (
            negative_passed,
            match case.kind {
                NegativeKind::SemanticNeighbor => semantic_passed,
                NegativeKind::CrossScreenshot => cross_passed,
            },
        )
    };
    *total += 1;
    *kind += 1;
}

#[cfg(windows)]
#[derive(Default)]
struct LatencySeries {
    total_wall: Vec<f64>,
    primary_wall: Vec<f64>,
    confirmation_wall: Vec<f64>,
    primary_preprocessing: Vec<f64>,
    primary_ocr: Vec<f64>,
    final_preprocessing: Vec<f64>,
    final_ocr: Vec<f64>,
    samples: Vec<LatencySample>,
}

#[cfg(windows)]
impl LatencySeries {
    fn record(
        &mut self,
        sequence: &common::RecognitionSequence,
        image: &str,
        target: &str,
        cold: bool,
    ) {
        let milliseconds = |duration: std::time::Duration| duration.as_secs_f64() * 1_000.0;
        self.total_wall.push(milliseconds(sequence.total_wall()));
        self.primary_wall.push(milliseconds(sequence.primary_wall));
        if sequence.confirmation.is_some() {
            self.confirmation_wall
                .push(milliseconds(sequence.confirmation_wall));
        }
        self.primary_preprocessing
            .push(milliseconds(sequence.primary.preprocessing_elapsed));
        self.primary_ocr
            .push(milliseconds(sequence.primary.recognition_elapsed));
        let final_result = sequence.final_result_ref();
        self.final_preprocessing
            .push(milliseconds(final_result.preprocessing_elapsed));
        self.final_ocr
            .push(milliseconds(final_result.recognition_elapsed));
        self.samples.push(LatencySample {
            image: image.to_owned(),
            target: target.to_owned(),
            cold,
            total_wall_ms: milliseconds(sequence.total_wall()),
            primary_wall_ms: milliseconds(sequence.primary_wall),
            confirmation_wall_ms: milliseconds(sequence.confirmation_wall),
            confirmation_ran: sequence.confirmation.is_some(),
            primary_preprocess_ms: milliseconds(sequence.primary.preprocessing_elapsed),
            primary_ocr_ms: milliseconds(sequence.primary.recognition_elapsed),
            primary_cached: sequence.primary.was_cached,
            primary_rescan: sequence.primary.requires_rescan,
            final_preprocess_ms: milliseconds(final_result.preprocessing_elapsed),
            final_ocr_ms: milliseconds(final_result.recognition_elapsed),
            final_cached: final_result.was_cached,
            final_rescan: final_result.requires_rescan,
        });
    }

    fn write_csv(&self, path: &std::path::Path, run_label: &str, mode: &str) -> Result<(), String> {
        use std::io::Write;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        let file = std::fs::File::create(path)
            .map_err(|error| format!("could not create {}: {error}", path.display()))?;
        let mut output = std::io::BufWriter::new(file);
        writeln!(output, "run_label,sequence,mode,image,target,cold,total_wall_ms,primary_wall_ms,confirmation_wall_ms,confirmation_ran,primary_preprocess_ms,primary_ocr_ms,primary_cached,primary_rescan,final_preprocess_ms,final_ocr_ms,final_cached,final_rescan")
            .map_err(|error| error.to_string())?;
        for (sequence, sample) in self.samples.iter().enumerate() {
            writeln!(
                output,
                "{},{sequence},{},{},{},{},{:.6},{:.6},{:.6},{},{:.6},{:.6},{},{},{:.6},{:.6},{},{}",
                csv_field(run_label),
                csv_field(mode),
                csv_field(&sample.image),
                csv_field(&sample.target),
                sample.cold,
                sample.total_wall_ms,
                sample.primary_wall_ms,
                sample.confirmation_wall_ms,
                sample.confirmation_ran,
                sample.primary_preprocess_ms,
                sample.primary_ocr_ms,
                sample.primary_cached,
                sample.primary_rescan,
                sample.final_preprocess_ms,
                sample.final_ocr_ms,
                sample.final_cached,
                sample.final_rescan,
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn sort(&mut self) {
        for values in [
            &mut self.total_wall,
            &mut self.primary_wall,
            &mut self.confirmation_wall,
            &mut self.primary_preprocessing,
            &mut self.primary_ocr,
            &mut self.final_preprocessing,
            &mut self.final_ocr,
        ] {
            values.sort_by(f64::total_cmp);
        }
    }
}

#[cfg(windows)]
struct LatencySample {
    image: String,
    target: String,
    cold: bool,
    total_wall_ms: f64,
    primary_wall_ms: f64,
    confirmation_wall_ms: f64,
    confirmation_ran: bool,
    primary_preprocess_ms: f64,
    primary_ocr_ms: f64,
    primary_cached: bool,
    primary_rescan: bool,
    final_preprocess_ms: f64,
    final_ocr_ms: f64,
    final_cached: bool,
    final_rescan: bool,
}

#[cfg(windows)]
struct PhysicalMatchSample {
    image: String,
    target: String,
    physical_lines: usize,
    start_line: usize,
}

#[cfg(windows)]
fn write_physical_match_csv(
    path: &std::path::Path,
    manifest: &std::path::Path,
    samples: &[PhysicalMatchSample],
) -> Result<(), String> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let file = std::fs::File::create(path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    let mut output = std::io::BufWriter::new(file);
    writeln!(output, "manifest,image,target,physical_lines,start_line")
        .map_err(|error| error.to_string())?;
    for sample in samples {
        writeln!(
            output,
            "{},{},{},{},{}",
            csv_field(&manifest.display().to_string()),
            csv_field(&sample.image),
            csv_field(&sample.target),
            sample.physical_lines,
            sample.start_line
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(windows)]
fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(windows)]
#[derive(Default)]
struct DetailedRowOutcome {
    passed: usize,
    failed: usize,
    cross_negative_alerts: usize,
}

#[cfg(windows)]
struct DetailedRowRecognition {
    accepted: std::collections::HashSet<String>,
    greedy: String,
    segmented: Vec<String>,
}

#[cfg(windows)]
fn verify_detailed_physical_rows(
    frame: &poe_alarm_vision::CapturedFrame,
    screenshot: &Screenshot,
    targets: &[poe_alarm_core::FullLineAffixMatcher],
    session: &mut poe_alarm_ocr_paddle::PaddleCtcSession,
    traditional_chinese: bool,
) -> Result<DetailedRowOutcome, String> {
    use std::collections::{HashMap, HashSet};

    use poe_alarm_ocr_paddle::ImageView;
    use poe_alarm_vision::{
        BandDetectionSettings, BlueMaskIntensityMode, BlueMaskSettings, PhysicalBandDetector,
        build_blue_mask, crop_mask_bgra,
    };

    let settings = if traditional_chinese {
        BlueMaskSettings {
            minimum_blue: 100,
            minimum_blue_dominance: 14,
            maximum_warm_channel_difference: 72,
            intensity_mode: BlueMaskIntensityMode::Dominance,
        }
    } else {
        BlueMaskSettings::default()
    };
    let mask = build_blue_mask(frame, settings);
    let detected = PhysicalBandDetector::new()
        .detect(&mask, BandDetectionSettings::default())
        .map_err(|error| error.to_string())?;
    if let Some(expected) = screenshot.expected_band_count
        && detected.bands.len() != expected
    {
        return Err(format!(
            "{} detailed band count: expected {expected}, got {}",
            screenshot.image,
            detected.bands.len()
        ));
    }
    let detailed_targets = screenshot
        .cases
        .iter()
        .zip(targets)
        .filter_map(|(case, target)| case.detail().map(|_| target.clone()))
        .collect::<Vec<_>>();
    let mut rows = Vec::with_capacity(detected.bands.len());
    for band in &detected.bands {
        let crop = crop_mask_bgra(&mask, &band.crop).map_err(|error| error.to_string())?;
        let image = ImageView::bgra8(crop.width, crop.height, crop.stride, &crop.bgra_pixels)
            .and_then(|image| {
                image.with_logical_content(
                    crop.metadata.logical_content_top,
                    crop.metadata.logical_content_bottom,
                )
            })
            .map_err(|error| error.to_string())?;
        let batch = session
            .recognize_batch(image, &detailed_targets)
            .map_err(|error| error.to_string())?;
        let mut accepted = detailed_targets
            .iter()
            .filter(|target| target.is_match(&batch.recognition.text))
            .map(|target| target.template().text.clone())
            .collect::<HashSet<_>>();
        accepted.extend(
            batch
                .target_supports
                .iter()
                .filter(|evaluation| evaluation.support.strongly_supported)
                .map(|evaluation| evaluation.canonical_target.clone()),
        );
        let mut segmented = Vec::with_capacity(2);
        for maximum_segment_width in [320, 400] {
            if accepted.len() == detailed_targets.len() {
                break;
            }
            let recognition = session
                .recognize_with_segment_width(image, Some(maximum_segment_width))
                .map_err(|error| error.to_string())?;
            for target in &detailed_targets {
                if !accepted.contains(&target.template().text) && target.is_match(&recognition.text)
                {
                    accepted.insert(target.template().text.clone());
                }
            }
            segmented.push(recognition.text);
        }
        rows.push(DetailedRowRecognition {
            accepted,
            greedy: batch.recognition.text,
            segmented,
        });
    }

    let mut outcome = DetailedRowOutcome::default();
    let mut expected_bands: HashMap<String, HashSet<usize>> = HashMap::new();
    for (case, target) in screenshot.cases.iter().zip(targets) {
        let Some((id, band, expected_text)) = case.detail() else {
            continue;
        };
        expected_bands
            .entry(target.template().text.clone())
            .or_default()
            .insert(band);
        let valid_manifest_text = target.is_match(expected_text);
        let accepted = rows
            .get(band)
            .is_some_and(|row| row.accepted.contains(&target.template().text));
        if valid_manifest_text && accepted {
            outcome.passed += 1;
            println!("    DETAILED-ROW-PASS id={id} band={band}");
        } else {
            outcome.failed += 1;
            if let Some(row) = rows.get(band) {
                println!(
                    "    DETAILED-ROW-FAIL id={id} band={band} manifest_text_valid={valid_manifest_text} greedy={:?} segmented={:?}",
                    row.greedy, row.segmented
                );
            } else {
                println!(
                    "    DETAILED-ROW-FAIL id={id} band={band} is outside detected band count {}",
                    rows.len()
                );
            }
        }
    }
    for (band, row) in rows.iter().enumerate() {
        for canonical in &row.accepted {
            if expected_bands
                .get(canonical)
                .is_some_and(|expected| !expected.contains(&band))
            {
                outcome.cross_negative_alerts += 1;
                println!(
                    "    DETAILED-CROSS-NEGATIVE band={band} target={canonical:?} greedy={:?} segmented={:?}",
                    row.greedy, row.segmented
                );
            }
        }
    }
    Ok(outcome)
}

#[cfg(all(test, windows))]
mod tests {
    use super::{Manifest, validate_negative_contract};

    const VALID: &str =
        include_str!("../../tests/fixtures/poe1_english_negative_contract.valid.json");

    #[test]
    fn poe1_english_negative_contract_fixture_is_valid() {
        let manifest: Manifest = serde_json::from_str(VALID).expect("fixture must deserialize");
        let summary = validate_negative_contract(&manifest).expect("fixture must validate");

        assert_eq!(summary.positive_cases, 4);
        assert_eq!(summary.semantic_neighbors, 2);
        assert_eq!(summary.cross_screenshots, 2);
    }

    #[test]
    fn semantic_negative_must_name_a_positive_on_the_same_screenshot() {
        let json = VALID.replace(
            "\"evidenceTemplate\": \"#% increased Critical Strike Chance\"",
            "\"evidenceTemplate\": \"#% increased Cast Speed\"",
        );
        let manifest: Manifest = serde_json::from_str(&json).expect("fixture must deserialize");
        let error = validate_negative_contract(&manifest).expect_err("contract must fail");

        assert!(error.contains("is not a positive case"), "{error}");
    }

    #[test]
    fn cross_screenshot_negative_must_be_positive_on_the_named_other_image() {
        let json = VALID.replace(
            "\"template\": \"+# to maximum Life\",\n          \"positiveImage\": \"poe1-en-b.png\"",
            "\"template\": \"+# to maximum Mana\",\n          \"positiveImage\": \"poe1-en-b.png\"",
        );
        let manifest: Manifest = serde_json::from_str(&json).expect("fixture must deserialize");
        let error = validate_negative_contract(&manifest).expect_err("contract must fail");

        assert!(error.contains("is not a positive case"), "{error}");
    }

    #[test]
    fn negative_case_cannot_repeat_a_positive_on_its_test_image() {
        let json = VALID.replace(
            "\"template\": \"#% increased Cast Speed\"",
            "\"template\": \"#% increased Attack Speed\"",
        );
        let manifest: Manifest = serde_json::from_str(&json).expect("fixture must deserialize");
        let error = validate_negative_contract(&manifest).expect_err("contract must fail");

        assert!(error.contains("repeats a positive template"), "{error}");
    }
}
