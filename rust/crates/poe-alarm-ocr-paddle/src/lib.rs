//! Offline PP-OCRv5 recognition for POE Alarm.
//!
//! The crate loads a caller-supplied, packaged ONNX Runtime shared library and
//! validates the checked-in official model and dictionary before creating a
//! session. Inference is serial and allocation-reusing by design so a session
//! can live for the process lifetime on a dedicated worker thread.

#![forbid(unsafe_code)]

mod assets;
mod error;
mod image;
mod preprocess;
mod self_test;
mod session;
mod target_support;
mod worker;

pub use assets::{
    DICTIONARY_FILE_NAME, EXPECTED_CLASS_COUNT, EXPECTED_DICTIONARY_BYTES,
    EXPECTED_DICTIONARY_ENTRIES, EXPECTED_DICTIONARY_FIRST, EXPECTED_DICTIONARY_LAST,
    EXPECTED_DICTIONARY_SHA256, EXPECTED_MODEL_BYTES, EXPECTED_MODEL_SHA256, MODEL_FILE_NAME,
    PaddleAssetPaths, PaddleAssets,
};
pub use error::PaddleError;
pub use image::{ImageView, OwnedImage, PixelFormat};
pub use preprocess::{MAXIMUM_MODEL_WIDTH, MINIMUM_MODEL_WIDTH, MODEL_HEIGHT};
pub use self_test::{PaddleSelfTestReport, run_self_test};
pub use session::{
    CtcEmission, CtcRecognition, ModelContract, PaddleCtcSession, PaddleSessionConfig,
    TextPolarity, initialize_onnx_runtime,
};
pub use target_support::{
    CtcBatchRecognition, CtcMismatchSupport, CtcTargetEvaluation, CtcTargetRecognition,
    CtcTargetSupport, CtcTargetSupportReason,
};
pub use worker::{PaddleBatchPending, PaddleOcrWorker, PaddlePending, PaddleTargetPending};
