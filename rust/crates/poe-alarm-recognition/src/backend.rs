use std::time::Duration;

use poe_alarm_core::FullLineAffixMatcher;
use poe_alarm_monitoring::CancellationToken;
use poe_alarm_ocr_paddle::{
    CtcBatchRecognition, OwnedImage, PaddleAssetPaths, PaddleError, PaddleOcrWorker,
    PaddleSessionConfig,
};
use poe_alarm_ocr_win::{
    OcrError, OcrLane, OcrLanguagePreference, OcrRecognition, OcrWorker, OwnedBgraImage,
    RecognizedLine,
};

use crate::{PaddleBackendConfig, RecognitionError};

const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Clone, Debug)]
pub(crate) struct LayoutRecognition {
    pub elapsed: Duration,
    pub lines: Vec<RecognizedLine>,
}

pub(crate) trait CancellationProbe {
    fn is_cancelled(&self) -> bool;
}

impl CancellationProbe for CancellationToken {
    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NeverCancelled;

impl CancellationProbe for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

pub(crate) trait LayoutOcrBackend: Send {
    fn recognize(
        &mut self,
        language: OcrLanguagePreference,
        image: OwnedBgraImage,
        cancellation: &dyn CancellationProbe,
    ) -> Result<LayoutRecognition, RecognitionError>;

    fn requires_traditional_compatibility(&self) -> bool {
        false
    }
}

pub(crate) trait LocalizedOcrBackend: Send {
    fn recognize_batch(
        &mut self,
        image: OwnedImage,
        targets: &[FullLineAffixMatcher],
        cancellation: &dyn CancellationProbe,
    ) -> Result<LocalizedRecognition, RecognitionError>;

    fn recognize_segmented(
        &mut self,
        image: OwnedImage,
        maximum_segment_width: usize,
        cancellation: &dyn CancellationProbe,
    ) -> Result<LocalizedRecognition, RecognitionError>;
}

#[derive(Clone, Debug)]
pub(crate) struct LocalizedRecognition {
    pub text: String,
    pub elapsed: Duration,
    pub mean_confidence: f64,
    pub tensor_width: usize,
    pub target_supports: Vec<LocalizedTargetSupport>,
}

#[derive(Clone, Debug)]
pub(crate) struct LocalizedTargetSupport {
    pub canonical_target: String,
    pub strongly_supported: bool,
}

pub(crate) struct WinRtLayoutBackend {
    worker: OcrWorker,
    lane: OcrLane,
}

impl WinRtLayoutBackend {
    pub(crate) fn start() -> Result<Self, OcrError> {
        Self::start_on(OcrLane::Default)
    }

    pub(crate) fn start_confirmation() -> Result<Self, OcrError> {
        Self::start_on(OcrLane::Poe2Confirmation)
    }

    fn start_on(lane: OcrLane) -> Result<Self, OcrError> {
        Ok(Self {
            worker: OcrWorker::start()?,
            lane,
        })
    }
}

impl LayoutOcrBackend for WinRtLayoutBackend {
    fn recognize(
        &mut self,
        language: OcrLanguagePreference,
        image: OwnedBgraImage,
        cancellation: &dyn CancellationProbe,
    ) -> Result<LayoutRecognition, RecognitionError> {
        if cancellation.is_cancelled() {
            return Err(RecognitionError::Cancelled);
        }
        let pending = self
            .worker
            .submit_recognition_on(self.lane, language, image)?;
        loop {
            if cancellation.is_cancelled() {
                // The process-lifetime WinRT actor owns the in-flight bitmap and may finish it in
                // the background. Dropping this receiver makes monitor stop fail-open immediately.
                return Err(RecognitionError::Cancelled);
            }
            if let Some(result) = pending.take_timeout(CANCELLATION_POLL_INTERVAL)? {
                match result {
                    Ok(result) => return convert_winrt(result),
                    Err(error @ OcrError::LanguageUnavailable { .. }) => return Err(error.into()),
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
}

pub(crate) struct UnavailableTraditionalLayoutBackend;

impl LayoutOcrBackend for UnavailableTraditionalLayoutBackend {
    fn recognize(
        &mut self,
        _language: OcrLanguagePreference,
        _image: OwnedBgraImage,
        _cancellation: &dyn CancellationProbe,
    ) -> Result<LayoutRecognition, RecognitionError> {
        Err(RecognitionError::MissingLocalizedBackend(
            "Windows OCR initialization failed before Traditional Chinese recognition".to_owned(),
        ))
    }

    fn requires_traditional_compatibility(&self) -> bool {
        true
    }
}

fn convert_winrt(result: OcrRecognition) -> Result<LayoutRecognition, RecognitionError> {
    Ok(LayoutRecognition {
        elapsed: result.elapsed,
        lines: result.lines,
    })
}

pub(crate) struct PaddleLocalizedBackend {
    worker: PaddleOcrWorker,
}

impl PaddleLocalizedBackend {
    pub(crate) fn start(
        configuration: PaddleBackendConfig,
        allow_latin_target_support: bool,
    ) -> Result<Self, PaddleError> {
        let assets = PaddleAssetPaths::new(configuration.model, configuration.dictionary);
        let session = PaddleSessionConfig {
            threads: configuration.threads,
            right_padding: 8,
            allow_latin_target_support,
            ..PaddleSessionConfig::default()
        };
        Ok(Self {
            worker: PaddleOcrWorker::start(configuration.runtime_library, assets, session)?,
        })
    }
}

impl LocalizedOcrBackend for PaddleLocalizedBackend {
    fn recognize_batch(
        &mut self,
        image: OwnedImage,
        targets: &[FullLineAffixMatcher],
        cancellation: &dyn CancellationProbe,
    ) -> Result<LocalizedRecognition, RecognitionError> {
        if cancellation.is_cancelled() {
            return Err(RecognitionError::Cancelled);
        }
        let pending = self.worker.recognize_batch(image, targets.to_vec())?;
        let result = loop {
            if cancellation.is_cancelled() {
                // The serial actor retains the input and safely finishes ONNX inference. Dropping
                // only this response receiver lets monitor stop return without waiting for it.
                return Err(RecognitionError::Cancelled);
            }
            if let Some(result) = pending.take_timeout(CANCELLATION_POLL_INTERVAL)? {
                break result?;
            }
        };
        Ok(convert_paddle(result))
    }

    fn recognize_segmented(
        &mut self,
        image: OwnedImage,
        maximum_segment_width: usize,
        cancellation: &dyn CancellationProbe,
    ) -> Result<LocalizedRecognition, RecognitionError> {
        if cancellation.is_cancelled() {
            return Err(RecognitionError::Cancelled);
        }
        let pending = self
            .worker
            .recognize_segmented(image, maximum_segment_width)?;
        let result = loop {
            if cancellation.is_cancelled() {
                return Err(RecognitionError::Cancelled);
            }
            if let Some(result) = pending.take_timeout(CANCELLATION_POLL_INTERVAL)? {
                break result?;
            }
        };
        Ok(convert_segmented_paddle(result))
    }
}

fn convert_paddle(result: CtcBatchRecognition) -> LocalizedRecognition {
    let elapsed = result
        .recognition
        .total_elapsed()
        .saturating_add(result.target_analysis_elapsed);
    LocalizedRecognition {
        text: result.recognition.text,
        elapsed,
        mean_confidence: result.recognition.mean_confidence,
        tensor_width: result.recognition.tensor_width,
        target_supports: result
            .target_supports
            .into_iter()
            .map(|support| LocalizedTargetSupport {
                canonical_target: support.canonical_target,
                strongly_supported: support.support.strongly_supported,
            })
            .collect(),
    }
}

fn convert_segmented_paddle(result: poe_alarm_ocr_paddle::CtcRecognition) -> LocalizedRecognition {
    let elapsed = result.total_elapsed();
    LocalizedRecognition {
        text: result.text,
        elapsed,
        mean_confidence: result.mean_confidence,
        tensor_width: result.tensor_width,
        target_supports: Vec::new(),
    }
}
