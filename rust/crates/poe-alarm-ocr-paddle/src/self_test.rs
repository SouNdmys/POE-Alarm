use std::path::Path;
use std::time::{Duration, Instant};

use crate::assets::{
    EXPECTED_DICTIONARY_BYTES, EXPECTED_DICTIONARY_ENTRIES, EXPECTED_DICTIONARY_SHA256,
    EXPECTED_MODEL_BYTES, EXPECTED_MODEL_SHA256, PaddleAssetPaths, PaddleAssets,
};
use crate::{
    CtcRecognition, ImageView, ModelContract, PaddleCtcSession, PaddleError, PaddleSessionConfig,
    initialize_onnx_runtime,
};

/// Result of hashing assets, checking the exact model contract, and running real inference.
#[derive(Clone, Debug)]
pub struct PaddleSelfTestReport {
    pub model_bytes: usize,
    pub model_sha256: String,
    pub dictionary_bytes: usize,
    pub dictionary_sha256: String,
    pub dictionary_entries: usize,
    pub contract: ModelContract,
    pub session_creation_elapsed: Duration,
    pub inference: CtcRecognition,
    pub total_elapsed: Duration,
}

/// Executes the same all-zero 64x48 BGRA inference used by the .NET packaged self-test.
pub fn run_self_test(
    runtime_library: impl AsRef<Path>,
    asset_paths: &PaddleAssetPaths,
) -> Result<PaddleSelfTestReport, PaddleError> {
    let total_started = Instant::now();
    initialize_onnx_runtime(runtime_library)?;
    let assets = PaddleAssets::load(asset_paths)?;
    debug_assert_eq!(assets.model().len(), EXPECTED_MODEL_BYTES);
    debug_assert_eq!(assets.model_sha256(), EXPECTED_MODEL_SHA256);
    debug_assert_eq!(assets.dictionary_bytes().len(), EXPECTED_DICTIONARY_BYTES);
    debug_assert_eq!(assets.dictionary_sha256(), EXPECTED_DICTIONARY_SHA256);
    debug_assert_eq!(assets.dictionary().len(), EXPECTED_DICTIONARY_ENTRIES);

    let mut session = PaddleCtcSession::from_assets(&assets, PaddleSessionConfig::default())?;
    let mut pixels = vec![0_u8; 64 * 48 * 4];
    for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = u8::MAX;
    }
    let image = ImageView::bgra8(64, 48, 64 * 4, &pixels)?;
    let inference = session.recognize(image)?;
    Ok(PaddleSelfTestReport {
        model_bytes: assets.model().len(),
        model_sha256: assets.model_sha256().to_owned(),
        dictionary_bytes: assets.dictionary_bytes().len(),
        dictionary_sha256: assets.dictionary_sha256().to_owned(),
        dictionary_entries: assets.dictionary().len(),
        contract: session.contract().clone(),
        session_creation_elapsed: session.session_creation_elapsed(),
        inference,
        total_elapsed: total_started.elapsed(),
    })
}
