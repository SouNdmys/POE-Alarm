#![cfg(windows)]

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use poe_alarm_recognition::{
    PaddleBackendConfig, ProductionRecognizer, RecognitionError, RecognitionProfile,
};

#[test]
fn packaged_fallback_uses_the_documented_side_by_side_asset_names() {
    let configuration = PaddleBackendConfig::beside_current_executable()
        .expect("the Cargo test executable must have a parent directory");

    assert_eq!(
        configuration.runtime_library.file_name(),
        Some(Path::new("onnxruntime.dll").as_os_str())
    );
    assert_eq!(
        configuration.model.file_name(),
        Some(Path::new("PP-OCRv5_mobile_rec.onnx").as_os_str())
    );
    assert_eq!(
        configuration.dictionary.file_name(),
        Some(Path::new("ppocrv5_dict.txt").as_os_str())
    );
}

#[test]
fn explicitly_configured_missing_fallback_fails_startup() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock must be after the Unix epoch")
        .as_nanos();
    let missing_root = std::env::temp_dir().join(format!(
        "poe-alarm-recognition-missing-assets-{}-{nonce}",
        std::process::id()
    ));
    let missing_runtime = missing_root.join("onnxruntime.dll");
    let configuration = PaddleBackendConfig::new(
        &missing_runtime,
        missing_root.join("PP-OCRv5_mobile_rec.onnx"),
        missing_root.join("ppocrv5_dict.txt"),
    );

    let error =
        match ProductionRecognizer::start(RecognitionProfile::POE2_ENGLISH, Some(configuration)) {
            Ok(_) => panic!("startup unexpectedly accepted missing localized OCR assets"),
            Err(error) => error,
        };

    match error {
        RecognitionError::MissingLocalizedBackend(message) => {
            assert!(message.contains("ONNX Runtime"));
            assert!(message.contains(&missing_runtime.display().to_string()));
        }
        other => panic!("expected a missing-backend diagnostic, got {other}"),
    }
}

#[test]
fn production_startup_cannot_silently_disable_localized_recovery() {
    let error = match ProductionRecognizer::start(RecognitionProfile::POE2_ENGLISH, None) {
        Ok(_) => panic!("production startup unexpectedly disabled localized recovery"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        RecognitionError::MissingLocalizedBackend(message)
            if message.contains("production startup requires configured Paddle assets")
    ));
}
