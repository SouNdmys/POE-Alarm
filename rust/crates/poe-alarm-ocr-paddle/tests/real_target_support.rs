#![cfg(windows)]

use std::env;
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

/// Frozen 1.0 CTC-assisted case documented by the transient-replay suite.
///
/// The screenshot is intentionally private and must be supplied from the read-only
/// historical image root. The ignored test is a release gate on machines that have
/// that corpus and the packaged ONNX Runtime DLL.
#[test]
#[ignore = "requires private 1.0 screenshot corpus and locally packaged ONNX Runtime"]
fn old_traditional_ctc_assisted_crop_is_strong_and_near_neighbor_is_rejected() {
    let screenshot_root = env::var_os("POE_ALARM_PRIVATE_SCREENSHOT_ROOT")
        .map(PathBuf::from)
        .expect("POE_ALARM_PRIVATE_SCREENSHOT_ROOT must point at tests/screenshots");
    let screenshot = screenshot_root.join("Weixin Image_20260810210846_5712_114.png");
    assert!(screenshot.is_file(), "missing {}", screenshot.display());

    let runtime = find_runtime().expect("locally packaged ONNX Runtime 1.28 DLL");
    initialize_onnx_runtime(runtime).unwrap();
    let assets = PaddleAssets::load(&PaddleAssetPaths::source_tree()).unwrap();
    let mut session =
        PaddleCtcSession::from_assets(&assets, PaddleSessionConfig::default()).unwrap();

    let frame = WicScreenshotDecoder::new()
        .unwrap()
        .decode(
            screenshot,
            Some(CaptureRegion::new(415, 170, 280, 220).unwrap()),
        )
        .unwrap();
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
    assert_eq!(detected.bands.len(), 5, "frozen manifest band count");
    let metadata = detected.bands[4].crop.clone();
    let crop = crop_mask_bgra(&mask, &metadata).unwrap();
    let image = ImageView::bgra8(crop.width, crop.height, crop.stride, &crop.bgra_pixels)
        .unwrap()
        .with_logical_content(
            metadata.logical_content_top,
            metadata.logical_content_bottom,
        )
        .unwrap();

    let correct = FullLineAffixMatcher::new("增加 #% 暈眩恢復和格擋恢復").unwrap();
    // One semantic character differs from the valid target. It stays within the
    // two-substitution shape allowance, so rejection must come from weak CTC
    // evidence rather than only the mismatch-count cap.
    let wrong_neighbor = FullLineAffixMatcher::new("增加 #% 暈眩恢復和格擋回復").unwrap();
    let batch = session
        .recognize_batch(image, &[correct.clone(), wrong_neighbor.clone()])
        .unwrap();
    let correct_support = batch.support_for(&correct).unwrap();
    let wrong_support = batch.support_for(&wrong_neighbor).unwrap();

    eprintln!(
        "text={:?}; correct={correct_support:?}; wrong={wrong_support:?}; analysis={:.3}ms",
        batch.recognition.text,
        batch.target_analysis_elapsed.as_secs_f64() * 1_000.0,
    );
    assert!(correct_support.shape_compatible);
    assert!(correct_support.strongly_supported);
    assert!(!wrong_support.strongly_supported);

    let mut analysis_samples = Vec::with_capacity(30);
    for _ in 0..30 {
        let measured = session
            .recognize_batch(image, &[correct.clone(), wrong_neighbor.clone()])
            .unwrap();
        analysis_samples.push(measured.target_analysis_elapsed);
    }
    analysis_samples.sort_unstable();
    let percentile = |fraction: f64| {
        analysis_samples[((analysis_samples.len() - 1) as f64 * fraction).ceil() as usize]
    };
    eprintln!(
        "two-target analysis n=30 p50={:.3}ms p95={:.3}ms; retained class-index payload={} bytes",
        percentile(0.50).as_secs_f64() * 1_000.0,
        percentile(0.95).as_secs_f64() * 1_000.0,
        session.target_support_index_bytes(),
    );
}

fn find_runtime() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    [
        root.join(".packages/microsoft.ml.onnxruntime/1.28.0/runtimes/win-x64/native/onnxruntime.dll"),
        root.join(".dotnet-home/.nuget/packages/microsoft.ml.onnxruntime/1.28.0/runtimes/win-x64/native/onnxruntime.dll"),
        root.join("src/PoeAlarm.App/bin/Release/net10.0-windows10.0.19041.0/win-x64/onnxruntime.dll"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}
