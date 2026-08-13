#![cfg(windows)]

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use poe_alarm_core::FullLineAffixMatcher;
use poe_alarm_ocr_paddle::{
    ImageView, PaddleAssetPaths, PaddleAssets, PaddleCtcSession, PaddleSessionConfig,
    initialize_onnx_runtime,
};
use poe_alarm_vision::{
    BandDetectionSettings, BlueMaskIntensityMode, BlueMaskSettings, CaptureRegion,
    PhysicalBandDetector, WicScreenshotDecoder, build_blue_mask, crop_mask_bgra,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    screenshots: Vec<Screenshot>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Screenshot {
    image: String,
    roi: [i64; 4],
    expected_band_count: usize,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    band: usize,
    template: String,
}

/// Private release gate equivalent to the .NET 1.0 row-specific manifest sweep.
///
/// Images stay outside the repository. Set `POE_ALARM_PRIVATE_SCREENSHOT_ROOT` to the historical
/// `tests/screenshots` directory and `POE_ALARM_ONNX_RUNTIME` to the packaged 1.28 runtime DLL.
#[test]
#[ignore = "requires private 1.0 screenshots and packaged ONNX Runtime"]
fn legacy_traditional_rows_are_strong_and_cross_negatives_are_rejected() {
    let screenshot_root = required_path("POE_ALARM_PRIVATE_SCREENSHOT_ROOT");
    let runtime = required_path("POE_ALARM_ONNX_RUNTIME");
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let manifest_path = source_root.join("tests/screenshots/traditional-ocr-cases.json");
    let manifest: Manifest = serde_json::from_str(
        &fs::read_to_string(&manifest_path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", manifest_path.display())),
    )
    .expect("valid Traditional-Chinese manifest");

    initialize_onnx_runtime(runtime).expect("initialize packaged ONNX Runtime");
    let assets = PaddleAssets::load(&PaddleAssetPaths::source_tree()).expect("load OCR assets");
    let config = PaddleSessionConfig {
        right_padding: 8,
        ..PaddleSessionConfig::default()
    };
    let mut session = PaddleCtcSession::from_assets(&assets, config).unwrap();
    let decoder = WicScreenshotDecoder::new().unwrap();
    let mut positives = 0_usize;
    let mut positive_failures = Vec::new();
    let mut false_alerts = Vec::new();

    for screenshot in manifest.screenshots {
        let image = screenshot_root.join(&screenshot.image);
        let roi = CaptureRegion::new(
            i32::try_from(screenshot.roi[0]).unwrap(),
            i32::try_from(screenshot.roi[1]).unwrap(),
            u32::try_from(screenshot.roi[2]).unwrap(),
            u32::try_from(screenshot.roi[3]).unwrap(),
        )
        .unwrap();
        let frame = decoder.decode(&image, Some(roi)).unwrap();
        let mask = build_blue_mask(
            &frame,
            BlueMaskSettings {
                minimum_blue: 100,
                minimum_blue_dominance: 14,
                maximum_warm_channel_difference: 72,
                intensity_mode: BlueMaskIntensityMode::Dominance,
            },
        );
        let detected = PhysicalBandDetector::new()
            .detect(&mask, BandDetectionSettings::default())
            .unwrap();
        assert_eq!(
            detected.bands.len(),
            screenshot.expected_band_count,
            "{} physical band count",
            screenshot.image
        );

        let crops = detected
            .bands
            .iter()
            .map(|band| crop_mask_bgra(&mask, &band.crop).unwrap())
            .collect::<Vec<_>>();

        let targets = screenshot
            .cases
            .iter()
            .map(|case| FullLineAffixMatcher::new(&case.template).unwrap())
            .collect::<Vec<_>>();
        // Each physical band shares exactly one target-conditioned inference across the complete
        // target set. The bounded target-independent segmented decodes are likewise shared by all
        // unresolved targets; only a strict transcript match can be accepted from that route.
        let rows = crops
            .iter()
            .map(|crop| evaluate_row(&mut session, crop, &targets))
            .collect::<Vec<_>>();

        for (case, target) in screenshot.cases.iter().zip(&targets) {
            let canonical = &target.template().text;
            let positive = &rows[case.band];
            if positive.accepted.contains(canonical) {
                positives += 1;
            } else {
                positive_failures.push(format!(
                    "{} target={} band={} greedy={:?} segmented={:?}",
                    screenshot.image, case.id, case.band, positive.greedy, positive.segmented
                ));
            }

            for (band_index, row) in rows.iter().enumerate() {
                if band_index != case.band && row.accepted.contains(canonical) {
                    false_alerts.push(format!(
                        "{} target={} expected_band={} wrong_band={} greedy={:?} segmented={:?}",
                        screenshot.image, case.id, case.band, band_index, row.greedy, row.segmented
                    ));
                }
            }
        }
    }

    assert_eq!(
        positives,
        26,
        "positive failures:\n{}",
        positive_failures.join("\n")
    );
    assert!(
        false_alerts.is_empty(),
        "cross-negative false alerts:\n{}",
        false_alerts.join("\n")
    );
}

struct RowEvaluation {
    accepted: HashSet<String>,
    greedy: String,
    segmented: Vec<String>,
}

fn evaluate_row(
    session: &mut PaddleCtcSession,
    crop: &poe_alarm_vision::CroppedMask,
    targets: &[FullLineAffixMatcher],
) -> RowEvaluation {
    let batch = session.recognize_batch(crop_view(crop), targets).unwrap();
    let mut accepted = targets
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
        if accepted.len() == targets.len() {
            break;
        }
        let recognition = session
            .recognize_with_segment_width(crop_view(crop), Some(maximum_segment_width))
            .unwrap();
        for target in targets {
            if !accepted.contains(&target.template().text) && target.is_match(&recognition.text) {
                accepted.insert(target.template().text.clone());
            }
        }
        segmented.push(recognition.text);
    }
    RowEvaluation {
        accepted,
        greedy: batch.recognition.text,
        segmented,
    }
}

fn crop_view(crop: &poe_alarm_vision::CroppedMask) -> ImageView<'_> {
    ImageView::bgra8(crop.width, crop.height, crop.stride, &crop.bgra_pixels)
        .unwrap()
        .with_logical_content(
            crop.metadata.logical_content_top,
            crop.metadata.logical_content_bottom,
        )
        .unwrap()
}

fn required_path(name: &str) -> PathBuf {
    env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} is required"))
}
