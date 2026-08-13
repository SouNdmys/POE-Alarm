use std::cell::RefCell;
use std::fmt;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use poe_alarm_core::FullLineAffixMatcher;
use poe_alarm_monitoring::{
    CancellationToken, FrameCapture, OcrRecognizer, PreparedFrame, RecognitionResult,
    StructuredOcrSupport,
};
use poe_alarm_recognition::{PaddleBackendConfig, ProductionRecognizer, RecognitionProfile};
use poe_alarm_vision::{
    CaptureRegion, CapturedFrame, GdiScreenCapture, ScreenCapture, WicScreenshotDecoder,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendError(pub String);

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BackendError {}

pub trait CaptureBackend: Send + 'static {
    fn capture_into(
        &mut self,
        region: CaptureRegion,
        destination: &mut CapturedFrame,
    ) -> Result<(), BackendError>;
}

pub trait RecognizerBackend: Send + 'static {
    fn structured_support(&self) -> StructuredOcrSupport;

    fn recognize_quick_live(
        &mut self,
        prepared: PreparedFrame<'_>,
        target: &FullLineAffixMatcher,
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, BackendError>;

    fn recognize_structured_live(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, BackendError>;

    fn recognize_quick_screenshot(
        &mut self,
        prepared: PreparedFrame<'_>,
        target: &FullLineAffixMatcher,
        cancelled: &AtomicBool,
    ) -> Result<RecognitionResult, BackendError>;

    fn recognize_structured_screenshot(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancelled: &AtomicBool,
    ) -> Result<RecognitionResult, BackendError>;
}

pub trait BackendFactory: Send + Sync + 'static {
    fn create_capture(&self) -> Result<Box<dyn CaptureBackend>, BackendError>;

    fn create_recognizer(
        &self,
        profile: RecognitionProfile,
    ) -> Result<Box<dyn RecognizerBackend>, BackendError>;

    /// Decodes the complete image. Runtime-owned crop validation then applies
    /// the configured region or deliberately falls back to the whole image.
    fn decode_screenshot(&self, path: &Path) -> Result<CapturedFrame, BackendError>;
}

pub(crate) struct CaptureAdapter(pub Box<dyn CaptureBackend>);

impl FrameCapture for CaptureAdapter {
    type Error = BackendError;

    fn capture_into(
        &mut self,
        region: CaptureRegion,
        destination: &mut CapturedFrame,
    ) -> Result<(), Self::Error> {
        self.0.capture_into(region, destination)
    }
}

pub(crate) struct RecognizerAdapter(pub Box<dyn RecognizerBackend>);

impl OcrRecognizer for RecognizerAdapter {
    type Error = BackendError;

    fn structured_support(&self) -> StructuredOcrSupport {
        self.0.structured_support()
    }

    fn recognize_quick(
        &mut self,
        prepared: PreparedFrame<'_>,
        target: &FullLineAffixMatcher,
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, Self::Error> {
        self.0.recognize_quick_live(prepared, target, cancellation)
    }

    fn recognize_structured(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, Self::Error> {
        self.0
            .recognize_structured_live(prepared, targets, cancellation)
    }
}

#[derive(Clone, Debug)]
pub struct ProductionBackendFactory {
    paddle: Option<PaddleBackendConfig>,
}

impl ProductionBackendFactory {
    #[must_use]
    pub const fn new(paddle: Option<PaddleBackendConfig>) -> Self {
        Self { paddle }
    }

    #[must_use]
    pub const fn packaged() -> Self {
        Self { paddle: None }
    }
}

impl BackendFactory for ProductionBackendFactory {
    fn create_capture(&self) -> Result<Box<dyn CaptureBackend>, BackendError> {
        Ok(Box::new(ProductionCapture))
    }

    fn create_recognizer(
        &self,
        profile: RecognitionProfile,
    ) -> Result<Box<dyn RecognizerBackend>, BackendError> {
        let recognizer = match &self.paddle {
            Some(configuration) => {
                ProductionRecognizer::start(profile, Some(configuration.clone()))
            }
            None => ProductionRecognizer::start_packaged(profile),
        }
        .map_err(|error| BackendError(error.to_string()))?;
        Ok(Box::new(ProductionRecognition(recognizer)))
    }

    fn decode_screenshot(&self, path: &Path) -> Result<CapturedFrame, BackendError> {
        WicScreenshotDecoder::new()
            .map_err(|error| BackendError(error.to_string()))?
            .decode(path, None)
            .map_err(|error| BackendError(error.to_string()))
    }
}

/// Zero-sized, movable factory product. The actual GDI handles are created by
/// and remain on the monitor worker that performs the first capture.
struct ProductionCapture;

thread_local! {
    static WORKER_GDI_CAPTURE: RefCell<GdiScreenCapture> =
        RefCell::new(GdiScreenCapture::new());
}

impl CaptureBackend for ProductionCapture {
    fn capture_into(
        &mut self,
        region: CaptureRegion,
        destination: &mut CapturedFrame,
    ) -> Result<(), BackendError> {
        WORKER_GDI_CAPTURE.with(|capture| {
            ScreenCapture::capture_into(&mut *capture.borrow_mut(), region, destination)
                .map_err(|error| BackendError(error.to_string()))
        })
    }
}

struct ProductionRecognition(ProductionRecognizer);

impl RecognizerBackend for ProductionRecognition {
    fn structured_support(&self) -> StructuredOcrSupport {
        OcrRecognizer::structured_support(&self.0)
    }

    fn recognize_quick_live(
        &mut self,
        prepared: PreparedFrame<'_>,
        target: &FullLineAffixMatcher,
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, BackendError> {
        OcrRecognizer::recognize_quick(&mut self.0, prepared, target, cancellation)
            .map_err(|error| BackendError(error.to_string()))
    }

    fn recognize_structured_live(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, BackendError> {
        OcrRecognizer::recognize_structured(&mut self.0, prepared, targets, cancellation)
            .map_err(|error| BackendError(error.to_string()))
    }

    fn recognize_quick_screenshot(
        &mut self,
        prepared: PreparedFrame<'_>,
        target: &FullLineAffixMatcher,
        cancelled: &AtomicBool,
    ) -> Result<RecognitionResult, BackendError> {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(BackendError(
                "screenshot recognition was cancelled".to_owned(),
            ));
        }
        self.0
            .recognize_quick_prepared(prepared.frame, prepared.blue_mask, target)
            .map_err(|error| BackendError(error.to_string()))
    }

    fn recognize_structured_screenshot(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancelled: &AtomicBool,
    ) -> Result<RecognitionResult, BackendError> {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(BackendError(
                "screenshot recognition was cancelled".to_owned(),
            ));
        }
        self.0
            .recognize_structured_prepared(prepared.frame, prepared.blue_mask, targets)
            .map_err(|error| BackendError(error.to_string()))
    }
}
