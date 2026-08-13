use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use poe_alarm_core::{
    AffixToken, AssistedModifierObservation, FullLineAffixMatcher, LogicalAffixMatch, canonicalize,
};
use poe_alarm_monitoring::{
    CancellationToken, OcrRecognizer, PreparedFrame, RecognitionResult, StructuredOcrSupport,
};
use poe_alarm_ocr_paddle::{OwnedImage, PaddleError};
use poe_alarm_ocr_win::{OcrError, OcrLanguagePreference, OwnedBgraImage};
use poe_alarm_vision::{
    BandDetectionError, BandDetectionSettings, BlueMaskIntensityMode, BlueMaskSettings,
    BlueTextMask, CapturedFrame, CropMetadata, PhysicalBandDetector, PixelRect,
    build_blue_mask_into, combined_crop_metadata,
};

use crate::backend::{
    CancellationProbe, LayoutOcrBackend, LocalizedOcrBackend, LocalizedRecognition, NeverCancelled,
    PaddleLocalizedBackend, UnavailableTraditionalLayoutBackend, WinRtLayoutBackend,
};
use crate::projection::{
    BandedView, ChineseProjection, FloatRect, band_id, project_banded_lines, project_chinese_lines,
};

const MAXIMUM_CACHED_BANDS: usize = 256;
const MAXIMUM_LOCALIZED_WORK_UNITS: usize = 2;
const TRADITIONAL_PADDLE_MASK_SETTINGS: BlueMaskSettings = BlueMaskSettings {
    minimum_blue: 100,
    minimum_blue_dominance: 14,
    maximum_warm_channel_difference: 72,
    intensity_mode: BlueMaskIntensityMode::Dominance,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GameVersion {
    Poe1,
    Poe2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecognitionLanguage {
    English,
    TraditionalChinese,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecognitionProfile {
    pub game: GameVersion,
    pub language: RecognitionLanguage,
}

impl RecognitionProfile {
    pub const POE1_ENGLISH: Self = Self {
        game: GameVersion::Poe1,
        language: RecognitionLanguage::English,
    };
    pub const POE1_TRADITIONAL_CHINESE: Self = Self {
        game: GameVersion::Poe1,
        language: RecognitionLanguage::TraditionalChinese,
    };
    pub const POE2_ENGLISH: Self = Self {
        game: GameVersion::Poe2,
        language: RecognitionLanguage::English,
    };
    pub const POE2_TRADITIONAL_CHINESE: Self = Self {
        game: GameVersion::Poe2,
        language: RecognitionLanguage::TraditionalChinese,
    };
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaddleBackendConfig {
    pub runtime_library: PathBuf,
    pub model: PathBuf,
    pub dictionary: PathBuf,
    pub threads: usize,
}

impl PaddleBackendConfig {
    pub fn new(
        runtime_library: impl Into<PathBuf>,
        model: impl Into<PathBuf>,
        dictionary: impl Into<PathBuf>,
    ) -> Self {
        Self {
            runtime_library: runtime_library.into(),
            model: model.into(),
            dictionary: dictionary.into(),
            threads: 2,
        }
    }

    /// Resolves the production side-by-side assets next to the running executable.
    pub fn beside_current_executable() -> Result<Self, RecognitionError> {
        let executable = std::env::current_exe().map_err(|error| {
            RecognitionError::MissingLocalizedBackend(format!(
                "cannot locate the running executable: {error}"
            ))
        })?;
        let directory = executable.parent().ok_or_else(|| {
            RecognitionError::MissingLocalizedBackend(
                "the running executable has no parent directory".to_owned(),
            )
        })?;
        Ok(Self::new(
            directory.join("onnxruntime.dll"),
            directory.join("PP-OCRv5_mobile_rec.onnx"),
            directory.join("ppocrv5_dict.txt"),
        ))
    }

    fn validate_paths(&self) -> Result<(), RecognitionError> {
        for (label, path) in [
            ("ONNX Runtime", &self.runtime_library),
            ("PP-OCRv5 model", &self.model),
            ("PP-OCRv5 dictionary", &self.dictionary),
        ] {
            if !path.is_file() {
                return Err(RecognitionError::MissingLocalizedBackend(format!(
                    "{label} is missing: {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum RecognitionError {
    Cancelled,
    Vision(BandDetectionError),
    Windows(OcrError),
    Paddle(PaddleError),
    InvalidImage(String),
    MissingLocalizedBackend(String),
}

impl fmt::Display for RecognitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("recognition was cancelled"),
            Self::Vision(error) => write!(formatter, "blue-text preparation failed: {error}"),
            Self::Windows(error) => write!(formatter, "Windows OCR failed: {error}"),
            Self::Paddle(error) => write!(formatter, "localized Paddle OCR failed: {error}"),
            Self::InvalidImage(message) => write!(formatter, "invalid OCR crop: {message}"),
            Self::MissingLocalizedBackend(message) => write!(
                formatter,
                "localized OCR fallback is not configured: {message}"
            ),
        }
    }
}

impl std::error::Error for RecognitionError {}

impl From<BandDetectionError> for RecognitionError {
    fn from(value: BandDetectionError) -> Self {
        Self::Vision(value)
    }
}

impl From<OcrError> for RecognitionError {
    fn from(value: OcrError) -> Self {
        if value == OcrError::Cancelled {
            Self::Cancelled
        } else {
            Self::Windows(value)
        }
    }
}

impl From<PaddleError> for RecognitionError {
    fn from(value: PaddleError) -> Self {
        Self::Paddle(value)
    }
}

/// Production wrapper implementing the monitoring crate's target-aware OCR boundary.
pub struct ProductionRecognizer {
    adapter: RecognitionAdapter,
}

impl ProductionRecognizer {
    pub fn start(
        profile: RecognitionProfile,
        paddle: Option<PaddleBackendConfig>,
    ) -> Result<Self, RecognitionError> {
        let paddle = paddle.ok_or_else(|| {
            RecognitionError::MissingLocalizedBackend(
                "production startup requires configured Paddle assets; use start_packaged or pass an explicit PaddleBackendConfig"
                    .to_owned(),
            )
        })?;
        Self::start_internal(profile, Some(paddle))
    }

    /// Explicit opt-out for offline diagnostics that intentionally measure Windows OCR alone.
    /// Production composition must use [`Self::start`] or [`Self::start_packaged`].
    pub fn start_without_localized_fallback_for_diagnostics(
        profile: RecognitionProfile,
    ) -> Result<Self, RecognitionError> {
        Self::start_internal(profile, None)
    }

    fn start_internal(
        profile: RecognitionProfile,
        paddle: Option<PaddleBackendConfig>,
    ) -> Result<Self, RecognitionError> {
        let layout: Box<dyn LayoutOcrBackend> = match WinRtLayoutBackend::start() {
            Ok(layout) => Box::new(layout),
            Err(_) if profile.language == RecognitionLanguage::TraditionalChinese => {
                Box::new(UnavailableTraditionalLayoutBackend)
            }
            Err(error) => return Err(error.into()),
        };
        let confirmation_layout = (profile == RecognitionProfile::POE2_ENGLISH)
            .then(WinRtLayoutBackend::start_confirmation)
            .transpose()?
            .map(|backend| Box::new(backend) as Box<dyn LayoutOcrBackend>);
        let localized = paddle
            .map(|configuration| -> Result<_, RecognitionError> {
                configuration.validate_paths()?;
                Ok(configuration)
            })
            .transpose()?
            .map(|configuration| {
                PaddleLocalizedBackend::start(
                    configuration,
                    profile.language == RecognitionLanguage::English,
                )
            })
            .transpose()?
            .map(|backend| Box::new(backend) as Box<dyn LocalizedOcrBackend>);
        Ok(Self {
            adapter: RecognitionAdapter::new_with_confirmation(
                profile,
                layout,
                confirmation_layout,
                localized,
            ),
        })
    }

    /// Starts the production recognizer with the packaged side-by-side Paddle assets required by
    /// target-assisted recovery. Missing assets fail startup explicitly instead of silently
    /// reducing recall.
    pub fn start_packaged(profile: RecognitionProfile) -> Result<Self, RecognitionError> {
        Self::start(
            profile,
            Some(PaddleBackendConfig::beside_current_executable()?),
        )
    }

    /// Whether the optional independent localized verifier is ready.
    pub fn has_localized_fallback(&self) -> bool {
        self.adapter.localized.is_some()
    }

    /// Starts a logically independent screenshot request while retaining the
    /// heavyweight WinRT/Paddle workers and ONNX session.
    pub fn begin_screenshot_request(&mut self) {
        self.adapter.begin_screenshot_request();
    }

    /// Offline/screenshot Quick entry point. Live monitoring uses the cancellable trait method.
    pub fn recognize_quick_prepared(
        &mut self,
        frame: &CapturedFrame,
        blue_mask: &BlueTextMask,
        target: &FullLineAffixMatcher,
    ) -> Result<RecognitionResult, RecognitionError> {
        self.adapter.recognize_quick_with(
            PreparedFrame {
                frame,
                blue_mask,
                semantic_fingerprint: blue_mask.fingerprint(),
            },
            target,
            &NeverCancelled,
        )
    }

    /// Offline/screenshot Structured entry point. The target set is processed once per pass.
    pub fn recognize_structured_prepared(
        &mut self,
        frame: &CapturedFrame,
        blue_mask: &BlueTextMask,
        targets: &[FullLineAffixMatcher],
    ) -> Result<RecognitionResult, RecognitionError> {
        self.adapter.recognize_structured_with(
            PreparedFrame {
                frame,
                blue_mask,
                semantic_fingerprint: blue_mask.fingerprint(),
            },
            targets,
            &NeverCancelled,
        )
    }
}

impl OcrRecognizer for ProductionRecognizer {
    type Error = RecognitionError;

    fn structured_support(&self) -> StructuredOcrSupport {
        self.adapter.structured_support()
    }

    fn recognize_quick(
        &mut self,
        prepared: PreparedFrame<'_>,
        target: &FullLineAffixMatcher,
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, Self::Error> {
        self.adapter
            .recognize_quick_with(prepared, target, cancellation)
    }

    fn recognize_structured(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, Self::Error> {
        self.adapter
            .recognize_structured_with(prepared, targets, cancellation)
    }
}

pub struct RecognitionAdapter {
    profile: RecognitionProfile,
    layout: Box<dyn LayoutOcrBackend>,
    confirmation_layout: Option<Box<dyn LayoutOcrBackend>>,
    localized: Option<Box<dyn LocalizedOcrBackend>>,
    detector: PhysicalBandDetector,
    primary: EnglishLane,
    confirmation: EnglishLane,
    quick_pending: Option<PendingEvidence>,
    quick_confirmed: Option<ConfirmedEvidence>,
    structured_pending: Option<PendingEvidence>,
    structured_confirmed: Option<ConfirmedEvidence>,
    pending_structured_assisted: Vec<AssistedModifierObservation>,
    chinese_quick_cache: Option<TargetedCache>,
    chinese_structured_cache: Option<TargetedCache>,
    segmented_cache: BoundedSegmentedCache,
    chinese_paddle_mask: ChinesePaddleMaskCache,
    chinese_force_paddle: bool,
    chinese_paddle_progress: Option<ChinesePaddleCompatibilityProgress>,
}

impl RecognitionAdapter {
    #[cfg(test)]
    fn new(
        profile: RecognitionProfile,
        layout: Box<dyn LayoutOcrBackend>,
        localized: Option<Box<dyn LocalizedOcrBackend>>,
    ) -> Self {
        Self::new_with_confirmation(profile, layout, None, localized)
    }

    fn new_with_confirmation(
        profile: RecognitionProfile,
        layout: Box<dyn LayoutOcrBackend>,
        confirmation_layout: Option<Box<dyn LayoutOcrBackend>>,
        localized: Option<Box<dyn LocalizedOcrBackend>>,
    ) -> Self {
        Self {
            profile,
            layout,
            confirmation_layout,
            localized,
            detector: PhysicalBandDetector::new(),
            primary: EnglishLane::default(),
            confirmation: EnglishLane::default(),
            quick_pending: None,
            quick_confirmed: None,
            structured_pending: None,
            structured_confirmed: None,
            pending_structured_assisted: Vec::new(),
            chinese_quick_cache: None,
            chinese_structured_cache: None,
            segmented_cache: BoundedSegmentedCache::default(),
            chinese_paddle_mask: ChinesePaddleMaskCache::default(),
            chinese_force_paddle: false,
            chinese_paddle_progress: None,
        }
    }

    pub fn structured_support(&self) -> StructuredOcrSupport {
        if self.profile == RecognitionProfile::POE2_ENGLISH {
            StructuredOcrSupport::ConfirmedStrictBatch
        } else {
            StructuredOcrSupport::StrictBatch
        }
    }

    fn begin_screenshot_request(&mut self) {
        self.quick_pending = None;
        self.quick_confirmed = None;
        self.structured_pending = None;
        self.structured_confirmed = None;
        self.pending_structured_assisted.clear();
        self.chinese_quick_cache = None;
        self.chinese_structured_cache = None;
        self.segmented_cache = BoundedSegmentedCache::default();
        self.chinese_paddle_mask.source_fingerprint = None;
        self.chinese_paddle_progress = None;
    }

    fn recognize_quick_with(
        &mut self,
        prepared: PreparedFrame<'_>,
        target: &FullLineAffixMatcher,
        cancellation: &dyn CancellationProbe,
    ) -> Result<RecognitionResult, RecognitionError> {
        match self.profile.language {
            RecognitionLanguage::English => {
                self.recognize_english_quick(prepared, target, cancellation)
            }
            RecognitionLanguage::TraditionalChinese => {
                self.recognize_chinese_quick(prepared, target, cancellation)
            }
        }
    }

    fn recognize_structured_with(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancellation: &dyn CancellationProbe,
    ) -> Result<RecognitionResult, RecognitionError> {
        match self.profile.language {
            RecognitionLanguage::English => {
                self.recognize_english_structured(prepared, targets, cancellation)
            }
            RecognitionLanguage::TraditionalChinese => {
                self.recognize_chinese_structured(prepared, targets, cancellation)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct BandCacheKey {
    fingerprint: u64,
    width: usize,
    height: usize,
}

#[derive(Default)]
struct BoundedBandCache {
    values: HashMap<BandCacheKey, Vec<String>>,
    insertion_order: VecDeque<BandCacheKey>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SegmentedCacheKey {
    fingerprint: u64,
    image_width: usize,
    image_height: usize,
    maximum_segment_width: usize,
}

#[derive(Default)]
struct BoundedSegmentedCache {
    values: HashMap<SegmentedCacheKey, LocalizedRecognition>,
    insertion_order: VecDeque<SegmentedCacheKey>,
}

#[derive(Default)]
struct ChinesePaddleMaskCache {
    source_fingerprint: Option<u64>,
    mask: BlueTextMask,
}

impl ChinesePaddleMaskCache {
    fn prepare(&mut self, prepared: PreparedFrame<'_>) -> &BlueTextMask {
        if self.source_fingerprint != Some(prepared.semantic_fingerprint) {
            build_blue_mask_into(
                prepared.frame,
                TRADITIONAL_PADDLE_MASK_SETTINGS,
                &mut self.mask,
            );
            self.source_fingerprint = Some(prepared.semantic_fingerprint);
        }
        &self.mask
    }
}

impl BoundedSegmentedCache {
    fn get(&self, key: &SegmentedCacheKey) -> Option<LocalizedRecognition> {
        self.values.get(key).cloned().map(|mut recognition| {
            recognition.elapsed = Duration::ZERO;
            recognition
        })
    }

    fn insert(&mut self, key: SegmentedCacheKey, recognition: LocalizedRecognition) {
        if self.values.contains_key(&key) {
            return;
        }
        self.values.insert(key.clone(), recognition);
        self.insertion_order.push_back(key);
        while self.values.len() > MAXIMUM_CACHED_BANDS {
            if let Some(oldest) = self.insertion_order.pop_front() {
                self.values.remove(&oldest);
            }
        }
    }
}

impl BoundedBandCache {
    fn get(&self, key: &BandCacheKey) -> Option<&Vec<String>> {
        self.values.get(key)
    }

    fn insert(&mut self, key: BandCacheKey, lines: Vec<String>) {
        if self.values.contains_key(&key) {
            return;
        }
        self.values.insert(key.clone(), lines);
        self.insertion_order.push_back(key);
        while self.values.len() > MAXIMUM_CACHED_BANDS {
            if let Some(oldest) = self.insertion_order.pop_front() {
                self.values.remove(&oldest);
            }
        }
    }
}

#[derive(Default)]
struct EnglishLane {
    bands: BoundedBandCache,
}

#[derive(Clone)]
struct EnglishRecognition {
    result: RecognitionResult,
    views: Vec<BandedView>,
}

#[derive(Clone)]
struct PendingEvidence {
    fingerprint: u64,
    target_key: String,
    localized_recovery_attempted: bool,
}

#[derive(Clone)]
struct ConfirmedEvidence {
    fingerprint: u64,
    target_key: String,
    result: RecognitionResult,
}

#[derive(Clone)]
struct TargetedCache {
    fingerprint: u64,
    target_key: String,
    result: RecognitionResult,
}

impl TargetedCache {
    fn reuse(&self, fingerprint: u64, target_key: &str) -> Option<RecognitionResult> {
        (self.fingerprint == fingerprint && self.target_key == target_key).then(|| {
            let mut result = self.result.clone();
            result.preprocessing_elapsed = Duration::ZERO;
            result.recognition_elapsed = Duration::ZERO;
            result.was_cached = true;
            result.requires_rescan = false;
            result
        })
    }
}

impl RecognitionAdapter {
    fn recognize_english_quick(
        &mut self,
        prepared: PreparedFrame<'_>,
        target: &FullLineAffixMatcher,
        cancellation: &dyn CancellationProbe,
    ) -> Result<RecognitionResult, RecognitionError> {
        self.structured_pending = None;
        self.structured_confirmed = None;
        self.pending_structured_assisted.clear();
        let scale = if self.profile.game == GameVersion::Poe1 {
            2
        } else {
            1
        };
        if self.profile.game == GameVersion::Poe1 {
            return recognize_english_lane(
                &mut self.detector,
                self.layout.as_mut(),
                &mut self.primary,
                EnglishLaneRequest {
                    prepared,
                    scale,
                    namespace: "win",
                    structured: false,
                    cancellation,
                },
            )
            .map(|recognition| recognition.result);
        }

        let target_key = target_key(target);
        if let Some(confirmed) = &self.quick_confirmed
            && confirmed.fingerprint == prepared.semantic_fingerprint
            && confirmed.target_key == target_key
        {
            return Ok(cached_result(&confirmed.result));
        }
        let confirming = self.quick_pending.as_ref().is_some_and(|pending| {
            pending.fingerprint == prepared.semantic_fingerprint && pending.target_key == target_key
        });
        if !confirming {
            self.quick_pending = None;
            self.quick_confirmed = None;
        }
        let request = EnglishLaneRequest {
            prepared,
            scale,
            namespace: "poe2",
            structured: false,
            cancellation,
        };
        let mut recognized = if confirming {
            let layout = self
                .confirmation_layout
                .as_deref_mut()
                .unwrap_or(self.layout.as_mut());
            recognize_english_lane(&mut self.detector, layout, &mut self.confirmation, request)?
        } else {
            recognize_english_lane(
                &mut self.detector,
                self.layout.as_mut(),
                &mut self.primary,
                request,
            )?
        };
        if target.find_match(&recognized.result.lines).is_some() {
            self.quick_pending = None;
            self.quick_confirmed = None;
            return Ok(recognized.result);
        }

        let localized_recovery_attempted =
            contains_one_localized_candidate(&recognized.views, target);
        let recovery_already_attempted = confirming
            && self
                .quick_pending
                .as_ref()
                .is_some_and(|pending| pending.localized_recovery_attempted);
        let recovery = if recovery_already_attempted {
            None
        } else {
            recover_english_quick(
                self.localized.as_deref_mut(),
                &mut self.segmented_cache,
                prepared.blue_mask,
                &recognized.views,
                target,
                cancellation,
            )?
        };
        if let Some(recovery) = recovery {
            recognized.result.recognition_elapsed = recognized
                .result
                .recognition_elapsed
                .saturating_add(recovery.elapsed);
            recognized.result.was_cached = false;
            recognized.result.target_assisted_match = Some(LogicalAffixMatch {
                start_line_index: 0,
                physical_line_count: 1,
                original_text: recovery.text,
                canonical_text: recovery.canonical_target,
            });
            self.quick_pending = None;
            self.quick_confirmed = None;
            return Ok(recognized.result);
        }

        if !confirming {
            self.quick_pending = Some(PendingEvidence {
                fingerprint: prepared.semantic_fingerprint,
                target_key,
                localized_recovery_attempted,
            });
            recognized.result.requires_rescan = true;
            return Ok(recognized.result);
        }

        self.quick_pending = None;
        recognized.result.requires_rescan = false;
        self.quick_confirmed = Some(ConfirmedEvidence {
            fingerprint: prepared.semantic_fingerprint,
            target_key,
            result: recognized.result.clone(),
        });
        Ok(recognized.result)
    }

    fn recognize_english_structured(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancellation: &dyn CancellationProbe,
    ) -> Result<RecognitionResult, RecognitionError> {
        self.quick_pending = None;
        self.quick_confirmed = None;
        let target_set = normalize_targets(targets);
        let scale = if self.profile.game == GameVersion::Poe1 {
            2
        } else {
            1
        };
        if self.profile.game == GameVersion::Poe1 {
            return recognize_english_lane(
                &mut self.detector,
                self.layout.as_mut(),
                &mut self.primary,
                EnglishLaneRequest {
                    prepared,
                    scale,
                    namespace: "win",
                    structured: true,
                    cancellation,
                },
            )
            .map(|recognition| recognition.result);
        }

        let signature = target_set_signature(&target_set);
        if let Some(confirmed) = &self.structured_confirmed
            && confirmed.fingerprint == prepared.semantic_fingerprint
            && confirmed.target_key == signature
        {
            self.pending_structured_assisted.clear();
            return Ok(cached_result(&confirmed.result));
        }
        let confirming = self.structured_pending.as_ref().is_some_and(|pending| {
            pending.fingerprint == prepared.semantic_fingerprint && pending.target_key == signature
        });
        if !confirming {
            self.structured_pending = None;
            self.structured_confirmed = None;
            self.pending_structured_assisted.clear();
        }
        // This is the only exhaustive pass for this call. Targets are checked in memory below;
        // the Structured path never dispatches the Quick entry point N times.
        let request = EnglishLaneRequest {
            prepared,
            scale,
            namespace: "poe2",
            structured: true,
            cancellation,
        };
        let mut recognized = if confirming {
            let layout = self
                .confirmation_layout
                .as_deref_mut()
                .unwrap_or(self.layout.as_mut());
            recognize_english_lane(&mut self.detector, layout, &mut self.confirmation, request)?
        } else {
            recognize_english_lane(
                &mut self.detector,
                self.layout.as_mut(),
                &mut self.primary,
                request,
            )?
        };

        let recovered = if confirming {
            self.pending_structured_assisted.clone()
        } else {
            let (assisted, elapsed) = recover_english_structured(
                self.localized.as_deref_mut(),
                &mut self.segmented_cache,
                prepared.blue_mask,
                &recognized.views,
                &recognized.result.lines,
                &target_set,
                cancellation,
            )?;
            recognized.result.recognition_elapsed = recognized
                .result
                .recognition_elapsed
                .saturating_add(elapsed);
            self.pending_structured_assisted = assisted.clone();
            assisted
        };
        recognized.result.assisted_observations = recovered;
        if !confirming {
            self.structured_pending = Some(PendingEvidence {
                fingerprint: prepared.semantic_fingerprint,
                target_key: signature,
                localized_recovery_attempted: false,
            });
            recognized.result.requires_rescan = true;
            return Ok(recognized.result);
        }

        self.structured_pending = None;
        self.pending_structured_assisted.clear();
        recognized.result.requires_rescan = false;
        self.structured_confirmed = Some(ConfirmedEvidence {
            fingerprint: prepared.semantic_fingerprint,
            target_key: signature,
            result: recognized.result.clone(),
        });
        Ok(recognized.result)
    }
}

struct EnglishLaneRequest<'frame, 'control> {
    prepared: PreparedFrame<'frame>,
    scale: usize,
    namespace: &'static str,
    structured: bool,
    cancellation: &'control dyn CancellationProbe,
}

fn recognize_english_lane(
    detector: &mut PhysicalBandDetector,
    layout: &mut dyn LayoutOcrBackend,
    lane: &mut EnglishLane,
    request: EnglishLaneRequest<'_, '_>,
) -> Result<EnglishRecognition, RecognitionError> {
    let EnglishLaneRequest {
        prepared,
        scale,
        namespace,
        structured,
        cancellation,
    } = request;
    let preprocessing_started = Instant::now();
    let detected = detector.detect(prepared.blue_mask, BandDetectionSettings::default())?;
    let mut metadata: Vec<CropMetadata> =
        detected.bands.into_iter().map(|band| band.crop).collect();
    if let Some(fallback) = detected.fallback_crop {
        metadata.push(fallback);
    }
    let preprocessing_elapsed = preprocessing_started.elapsed();
    let mut views = Vec::with_capacity(metadata.len());
    let mut recognition_elapsed = Duration::ZERO;
    let mut every_band_was_cached = true;
    for crop in metadata {
        if cancellation.is_cancelled() {
            return Err(RecognitionError::Cancelled);
        }
        let width =
            crop.source_rect.width.checked_mul(scale).ok_or_else(|| {
                RecognitionError::InvalidImage("scaled width overflow".to_owned())
            })?;
        let height =
            crop.source_rect.height.checked_mul(scale).ok_or_else(|| {
                RecognitionError::InvalidImage("scaled height overflow".to_owned())
            })?;
        let key = BandCacheKey {
            fingerprint: crop.content_fingerprint,
            width,
            height,
        };
        let lines = if let Some(cached) = lane.bands.get(&key) {
            cached.clone()
        } else {
            every_band_was_cached = false;
            let image = scaled_mask_bgra(prepared.blue_mask, &crop, scale)?;
            let recognized =
                layout.recognize(OcrLanguagePreference::English, image, cancellation)?;
            recognition_elapsed = recognition_elapsed.saturating_add(recognized.elapsed);
            let lines = recognized
                .lines
                .into_iter()
                .map(|line| line.text.trim().to_owned())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>();
            lane.bands.insert(key, lines.clone());
            lines
        };
        views.push(BandedView {
            metadata: crop,
            lines,
        });
    }
    let (lines, physical_line_identities) =
        project_banded_lines(&views, prepared.frame.width(), scale, namespace, structured);
    Ok(EnglishRecognition {
        result: RecognitionResult {
            lines,
            preprocessing_elapsed,
            recognition_elapsed,
            was_cached: every_band_was_cached,
            physical_line_identities,
            ..RecognitionResult::default()
        },
        views,
    })
}

fn scaled_mask_bgra(
    mask: &BlueTextMask,
    metadata: &CropMetadata,
    scale: usize,
) -> Result<OwnedBgraImage, RecognitionError> {
    metadata
        .source_rect
        .validate_within(mask.width(), mask.height())
        .map_err(|error| RecognitionError::InvalidImage(error.to_string()))?;
    if scale == 0 {
        return Err(RecognitionError::InvalidImage(
            "OCR scale must be positive".to_owned(),
        ));
    }
    let width = metadata.source_rect.width * scale;
    let height = metadata.source_rect.height * scale;
    let stride = width;
    let mut pixels = vec![0; stride * height];
    for output_y in 0..height {
        let source_y = metadata.source_rect.y + output_y / scale;
        for output_x in 0..width {
            let source_x = metadata.source_rect.x + output_x / scale;
            let intensity = mask.intensities()[source_y * mask.width() + source_x];
            pixels[output_y * stride + output_x] = intensity;
        }
    }
    OwnedBgraImage::gray8(width, height, stride, pixels).map_err(RecognitionError::from)
}

fn mask_gray8(
    mask: &BlueTextMask,
    metadata: &CropMetadata,
) -> Result<OwnedImage, RecognitionError> {
    metadata
        .source_rect
        .validate_within(mask.width(), mask.height())
        .map_err(|error| RecognitionError::InvalidImage(error.to_string()))?;
    let width = metadata.source_rect.width;
    let height = metadata.source_rect.height;
    let mut pixels = vec![0; width * height];
    for output_y in 0..height {
        let source_y = metadata.source_rect.y + output_y;
        let source = &mask.intensities()[source_y * mask.width() + metadata.source_rect.x
            ..source_y * mask.width() + metadata.source_rect.right_exclusive()];
        pixels[output_y * width..(output_y + 1) * width].copy_from_slice(source);
    }
    OwnedImage::gray8(width, height, width, pixels)
        .and_then(|image| {
            image.with_logical_content(
                metadata.logical_content_top,
                metadata.logical_content_bottom,
            )
        })
        .map_err(RecognitionError::from)
}

struct RecoveryText {
    text: String,
    canonical_target: String,
    elapsed: Duration,
}

fn recover_english_quick(
    localized: Option<&mut (dyn LocalizedOcrBackend + '_)>,
    segmented_cache: &mut BoundedSegmentedCache,
    mask: &BlueTextMask,
    views: &[BandedView],
    target: &FullLineAffixMatcher,
    cancellation: &dyn CancellationProbe,
) -> Result<Option<RecoveryText>, RecognitionError> {
    let Some(localized) = localized else {
        return Ok(None);
    };
    let candidates: Vec<&BandedView> = views
        .iter()
        .filter(|view| !view.metadata.is_fallback)
        .filter(|view| contains_localized_candidate(&view.lines, target))
        .take(2)
        .collect();
    if candidates.len() != 1 {
        return Ok(None);
    }
    let recognized = localized.recognize_batch(
        mask_gray8(mask, &candidates[0].metadata)?,
        std::slice::from_ref(target),
        cancellation,
    )?;
    let canonical_target = strict_or_strong_targets(&recognized, std::slice::from_ref(target))
        .into_iter()
        .next();
    if let Some(canonical_target) = canonical_target {
        return Ok(Some(RecoveryText {
            text: recognized.text,
            canonical_target,
            elapsed: recognized.elapsed,
        }));
    }
    let (segmented, segmented_elapsed) = recover_segmented_strict(
        localized,
        segmented_cache,
        mask,
        &candidates[0].metadata,
        &recognized,
        std::slice::from_ref(target),
        cancellation,
    )?;
    Ok(segmented.into_iter().next().map(|matched| RecoveryText {
        text: matched.original_text,
        canonical_target: matched.canonical_target,
        elapsed: recognized.elapsed.saturating_add(segmented_elapsed),
    }))
}

fn contains_one_localized_candidate(views: &[BandedView], target: &FullLineAffixMatcher) -> bool {
    views
        .iter()
        .filter(|view| !view.metadata.is_fallback)
        .filter(|view| contains_localized_candidate(&view.lines, target))
        .take(2)
        .count()
        == 1
}

fn recover_english_structured(
    localized: Option<&mut (dyn LocalizedOcrBackend + '_)>,
    segmented_cache: &mut BoundedSegmentedCache,
    mask: &BlueTextMask,
    views: &[BandedView],
    logical_lines: &[String],
    targets: &[&FullLineAffixMatcher],
    cancellation: &dyn CancellationProbe,
) -> Result<(Vec<AssistedModifierObservation>, Duration), RecognitionError> {
    let Some(localized) = localized else {
        return Ok((Vec::new(), Duration::ZERO));
    };
    let mut job_indices = Vec::new();
    for target in targets {
        if target.find_match(logical_lines).is_some() {
            continue;
        }
        for (index, view) in views.iter().enumerate() {
            if !view.metadata.is_fallback && contains_localized_candidate(&view.lines, target) {
                job_indices.push((index, *target));
            }
        }
    }
    job_indices.sort_by_key(|(index, target)| (*index, target_key(target)));
    let mut grouped: Vec<(usize, Vec<&FullLineAffixMatcher>)> = Vec::new();
    for (index, target) in job_indices {
        if let Some((_, targets)) = grouped
            .iter_mut()
            .find(|(candidate, _)| *candidate == index)
        {
            targets.push(target);
        } else {
            grouped.push((index, vec![target]));
        }
    }
    grouped.truncate(MAXIMUM_LOCALIZED_WORK_UNITS);
    let mut assisted = Vec::new();
    let mut elapsed = Duration::ZERO;
    for (index, targets) in grouped {
        let view = &views[index];
        let owned_targets = targets
            .iter()
            .map(|target| (*target).clone())
            .collect::<Vec<_>>();
        let recognized = localized.recognize_batch(
            mask_gray8(mask, &view.metadata)?,
            &owned_targets,
            cancellation,
        )?;
        elapsed = elapsed.saturating_add(recognized.elapsed);
        let id = band_id("poe2", &view.metadata);
        let verified = strict_or_strong_targets(&recognized, &owned_targets);
        for canonical_target in &verified {
            assisted.push(AssistedModifierObservation::new(
                id.clone(),
                recognized.text.clone(),
                canonical_target.clone(),
            ));
        }
        let unresolved = owned_targets
            .iter()
            .filter(|target| !verified.contains(&target.template().text))
            .cloned()
            .collect::<Vec<_>>();
        let (segmented, segmented_elapsed) = recover_segmented_strict(
            localized,
            segmented_cache,
            mask,
            &view.metadata,
            &recognized,
            &unresolved,
            cancellation,
        )?;
        elapsed = elapsed.saturating_add(segmented_elapsed);
        for matched in segmented {
            assisted.push(AssistedModifierObservation::new(
                id.clone(),
                matched.original_text,
                matched.canonical_target,
            ));
        }
    }
    assisted.sort_by(|left, right| {
        left.physical_band_id
            .cmp(&right.physical_band_id)
            .then_with(|| left.canonical_target.cmp(&right.canonical_target))
    });
    assisted.dedup_by(|left, right| {
        left.physical_band_id == right.physical_band_id
            && left.canonical_target == right.canonical_target
    });
    Ok((assisted, elapsed))
}

impl RecognitionAdapter {
    fn recognize_chinese_quick(
        &mut self,
        prepared: PreparedFrame<'_>,
        target: &FullLineAffixMatcher,
        cancellation: &dyn CancellationProbe,
    ) -> Result<RecognitionResult, RecognitionError> {
        let key = target_key(target);
        if let Some(cached) = &self.chinese_quick_cache
            && let Some(result) = cached.reuse(prepared.semantic_fingerprint, &key)
        {
            return Ok(result);
        }
        if self.layout.requires_traditional_compatibility() {
            self.chinese_force_paddle = true;
        }
        if self.chinese_force_paddle {
            let targets = [target];
            let localized_mask = self.chinese_paddle_mask.prepare(prepared);
            return recognize_chinese_paddle_compatibility(
                self.localized.as_deref_mut(),
                &mut self.detector,
                &mut self.chinese_paddle_progress,
                ChinesePaddleCompatibilityRequest {
                    prepared,
                    localized_mask,
                    targets: &targets,
                    structured: false,
                    cancellation,
                },
            );
        }
        let mut primary =
            match recognize_chinese_primary(self.layout.as_mut(), prepared, cancellation) {
                Ok(primary) => primary,
                Err(RecognitionError::Windows(OcrError::LanguageUnavailable { .. })) => {
                    self.chinese_force_paddle = true;
                    let targets = [target];
                    let localized_mask = self.chinese_paddle_mask.prepare(prepared);
                    return recognize_chinese_paddle_compatibility(
                        self.localized.as_deref_mut(),
                        &mut self.detector,
                        &mut self.chinese_paddle_progress,
                        ChinesePaddleCompatibilityRequest {
                            prepared,
                            localized_mask,
                            targets: &targets,
                            structured: false,
                            cancellation,
                        },
                    );
                }
                Err(error) => return Err(error),
            };
        if target.find_match(&primary.result.lines).is_none() {
            let localized_mask = self.chinese_paddle_mask.prepare(prepared);
            if let Some(recovery) = recover_chinese_quick(
                self.layout.as_mut(),
                self.localized.as_deref_mut(),
                &mut self.segmented_cache,
                target,
                ChineseRecoveryRequest {
                    prepared,
                    localized_mask,
                    combined: &primary.combined,
                    projection: &primary.projection,
                    cancellation,
                },
            )? {
                primary.result.recognition_elapsed = primary
                    .result
                    .recognition_elapsed
                    .saturating_add(recovery.elapsed);
                primary.result.target_assisted_match = Some(LogicalAffixMatch {
                    start_line_index: recovery.start_line_index,
                    physical_line_count: recovery.physical_line_count,
                    original_text: recovery.text,
                    canonical_text: recovery.canonical_target,
                });
            }
        }
        self.chinese_quick_cache = Some(TargetedCache {
            fingerprint: prepared.semantic_fingerprint,
            target_key: key,
            result: primary.result.clone(),
        });
        Ok(primary.result)
    }

    fn recognize_chinese_structured(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancellation: &dyn CancellationProbe,
    ) -> Result<RecognitionResult, RecognitionError> {
        let target_set = normalize_targets(targets);
        let signature = target_set_signature(&target_set);
        if let Some(cached) = &self.chinese_structured_cache
            && let Some(result) = cached.reuse(prepared.semantic_fingerprint, &signature)
        {
            return Ok(result);
        }
        if self.layout.requires_traditional_compatibility() {
            self.chinese_force_paddle = true;
        }
        if self.chinese_force_paddle {
            let localized_mask = self.chinese_paddle_mask.prepare(prepared);
            return recognize_chinese_paddle_compatibility(
                self.localized.as_deref_mut(),
                &mut self.detector,
                &mut self.chinese_paddle_progress,
                ChinesePaddleCompatibilityRequest {
                    prepared,
                    localized_mask,
                    targets: &target_set,
                    structured: true,
                    cancellation,
                },
            );
        }
        // Exactly one full combined-layout pass supplies evidence for the whole target set.
        let mut primary =
            match recognize_chinese_primary(self.layout.as_mut(), prepared, cancellation) {
                Ok(primary) => primary,
                Err(RecognitionError::Windows(OcrError::LanguageUnavailable { .. })) => {
                    self.chinese_force_paddle = true;
                    let localized_mask = self.chinese_paddle_mask.prepare(prepared);
                    return recognize_chinese_paddle_compatibility(
                        self.localized.as_deref_mut(),
                        &mut self.detector,
                        &mut self.chinese_paddle_progress,
                        ChinesePaddleCompatibilityRequest {
                            prepared,
                            localized_mask,
                            targets: &target_set,
                            structured: true,
                            cancellation,
                        },
                    );
                }
                Err(error) => return Err(error),
            };
        let localized_mask = self.chinese_paddle_mask.prepare(prepared);
        let (assisted, elapsed) = recover_chinese_structured(
            self.layout.as_mut(),
            self.localized.as_deref_mut(),
            &mut self.segmented_cache,
            &target_set,
            ChineseRecoveryRequest {
                prepared,
                localized_mask,
                combined: &primary.combined,
                projection: &primary.projection,
                cancellation,
            },
        )?;
        primary.result.recognition_elapsed =
            primary.result.recognition_elapsed.saturating_add(elapsed);
        primary.result.assisted_observations = assisted;
        self.chinese_structured_cache = Some(TargetedCache {
            fingerprint: prepared.semantic_fingerprint,
            target_key: signature,
            result: primary.result.clone(),
        });
        Ok(primary.result)
    }
}

struct ChineseRecognition {
    result: RecognitionResult,
    combined: CropMetadata,
    projection: ChineseProjection,
}

struct ChinesePaddleCompatibilityProgress {
    fingerprint: u64,
    target_signature: String,
    structured: bool,
    bands: Vec<CropMetadata>,
    evidence: Vec<Option<ChinesePaddleBandEvidence>>,
    next_band: usize,
    recovery_queue: VecDeque<(usize, usize)>,
}

struct ChinesePaddleBandEvidence {
    text: String,
    strongly_supported: Vec<String>,
    segmented_supported: Vec<(String, String)>,
}

struct ChinesePaddleCompatibilityRequest<'frame, 'request> {
    prepared: PreparedFrame<'frame>,
    localized_mask: &'request BlueTextMask,
    targets: &'request [&'request FullLineAffixMatcher],
    structured: bool,
    cancellation: &'request dyn CancellationProbe,
}

fn recognize_chinese_paddle_compatibility(
    localized: Option<&mut (dyn LocalizedOcrBackend + '_)>,
    detector: &mut PhysicalBandDetector,
    progress: &mut Option<ChinesePaddleCompatibilityProgress>,
    request: ChinesePaddleCompatibilityRequest<'_, '_>,
) -> Result<RecognitionResult, RecognitionError> {
    let ChinesePaddleCompatibilityRequest {
        prepared,
        localized_mask,
        targets,
        structured,
        cancellation,
    } = request;
    let Some(localized) = localized else {
        return Err(RecognitionError::MissingLocalizedBackend(
            "Windows Traditional Chinese OCR is unavailable and packaged Paddle fallback was not configured"
                .to_owned(),
        ));
    };
    if cancellation.is_cancelled() {
        return Err(RecognitionError::Cancelled);
    }
    let target_signature = target_set_signature(targets);
    let same_progress = progress.as_ref().is_some_and(|progress| {
        progress.fingerprint == prepared.semantic_fingerprint
            && progress.target_signature == target_signature
            && progress.structured == structured
    });
    let preprocessing_started = Instant::now();
    if !same_progress {
        let detected = detector.detect(localized_mask, BandDetectionSettings::default())?;
        let mut bands = detected
            .bands
            .into_iter()
            .map(|band| band.crop)
            .collect::<Vec<_>>();
        if let Some(fallback) = detected.fallback_crop {
            bands.push(fallback);
        }
        let evidence = (0..bands.len()).map(|_| None).collect();
        *progress = Some(ChinesePaddleCompatibilityProgress {
            fingerprint: prepared.semantic_fingerprint,
            target_signature,
            structured,
            bands,
            evidence,
            next_band: 0,
            recovery_queue: VecDeque::new(),
        });
    }
    let preprocessing_elapsed = preprocessing_started.elapsed();
    let owned_targets = targets
        .iter()
        .map(|target| (*target).clone())
        .collect::<Vec<_>>();
    let mut recognition_elapsed = Duration::ZERO;
    let progress = progress
        .as_mut()
        .expect("compatibility progress was initialized above");
    let primary_started_at = progress.next_band;
    let end = progress
        .next_band
        .saturating_add(MAXIMUM_LOCALIZED_WORK_UNITS)
        .min(progress.bands.len());
    while progress.next_band < end {
        if cancellation.is_cancelled() {
            return Err(RecognitionError::Cancelled);
        }
        let index = progress.next_band;
        let band = &progress.bands[index];
        let recognized = localized.recognize_batch(
            mask_gray8(localized_mask, band)?,
            &owned_targets,
            cancellation,
        )?;
        recognition_elapsed = recognition_elapsed.saturating_add(recognized.elapsed);
        let text = recognized.text.trim().to_owned();
        let unresolved = owned_targets
            .iter()
            .filter(|target| {
                !target.is_match(&text)
                    && !recognized.target_supports.iter().any(|support| {
                        support.canonical_target == target.template().text
                            && support.strongly_supported
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        if !band.is_fallback {
            for width in segmentation_widths(&recognized, &unresolved) {
                progress.recovery_queue.push_back((index, width));
            }
        }
        let strongly_supported = recognized
            .target_supports
            .into_iter()
            .filter(|support| support.strongly_supported)
            .map(|support| support.canonical_target)
            .collect();
        progress.evidence[index] = Some(ChinesePaddleBandEvidence {
            text,
            strongly_supported,
            segmented_supported: Vec::new(),
        });
        progress.next_band += 1;
    }
    // Keep the same hard ceiling as the established Paddle compatibility path: scans that run
    // primary rows do no extra recovery; after all rows are available a later scan advances at
    // most two strict segmented work units.
    if progress.next_band == primary_started_at && progress.next_band >= progress.bands.len() {
        for _ in 0..MAXIMUM_LOCALIZED_WORK_UNITS {
            let Some((index, width)) = progress.recovery_queue.pop_front() else {
                break;
            };
            if cancellation.is_cancelled() {
                return Err(RecognitionError::Cancelled);
            }
            let segmented = localized.recognize_segmented(
                mask_gray8(localized_mask, &progress.bands[index])?,
                width,
                cancellation,
            )?;
            recognition_elapsed = recognition_elapsed.saturating_add(segmented.elapsed);
            let evidence = progress.evidence[index]
                .as_mut()
                .expect("recovery is only queued after primary evidence");
            for target in &owned_targets {
                if target.is_match(&segmented.text)
                    && !evidence
                        .segmented_supported
                        .iter()
                        .any(|(_, canonical)| canonical == &target.template().text)
                {
                    evidence
                        .segmented_supported
                        .push((segmented.text.clone(), target.template().text.clone()));
                }
            }
        }
    }
    let views = progress
        .bands
        .iter()
        .zip(&progress.evidence)
        .filter_map(|(metadata, evidence)| {
            evidence.as_ref().map(|evidence| BandedView {
                metadata: metadata.clone(),
                lines: (!evidence.text.is_empty())
                    .then(|| evidence.text.clone())
                    .into_iter()
                    .collect(),
            })
        })
        .collect::<Vec<_>>();
    let (lines, physical_line_identities) =
        project_banded_lines(&views, prepared.frame.width(), 1, "paddle-zh", structured);
    let mut assisted = Vec::new();
    for (metadata, evidence) in progress.bands.iter().zip(&progress.evidence) {
        let Some(evidence) = evidence else { continue };
        let id = band_id("paddle-zh", metadata);
        for canonical in &evidence.strongly_supported {
            let strict = targets
                .iter()
                .find(|target| target.template().text == *canonical)
                .is_some_and(|target| target.is_match(&evidence.text));
            if !strict {
                assisted.push(AssistedModifierObservation::new(
                    id.clone(),
                    evidence.text.clone(),
                    canonical.clone(),
                ));
            }
        }
        for (text, canonical) in &evidence.segmented_supported {
            assisted.push(AssistedModifierObservation::new(
                id.clone(),
                text.clone(),
                canonical.clone(),
            ));
        }
    }
    assisted.sort_by(|left, right| {
        left.physical_band_id
            .cmp(&right.physical_band_id)
            .then_with(|| left.canonical_target.cmp(&right.canonical_target))
    });
    assisted.dedup_by(|left, right| {
        left.physical_band_id == right.physical_band_id
            && left.canonical_target == right.canonical_target
    });
    Ok(RecognitionResult {
        lines,
        preprocessing_elapsed,
        recognition_elapsed,
        was_cached: false,
        requires_rescan: progress.next_band < progress.bands.len()
            || !progress.recovery_queue.is_empty(),
        assisted_observations: assisted,
        physical_line_identities,
        ..RecognitionResult::default()
    })
}

fn recognize_chinese_primary(
    layout: &mut dyn LayoutOcrBackend,
    prepared: PreparedFrame<'_>,
    cancellation: &dyn CancellationProbe,
) -> Result<ChineseRecognition, RecognitionError> {
    let preprocessing_started = Instant::now();
    let combined = combined_crop_metadata(prepared.blue_mask, 8)?;
    let image = scaled_mask_bgra(prepared.blue_mask, &combined, 1)?;
    let preprocessing_elapsed = preprocessing_started.elapsed();
    let recognized = layout.recognize(
        OcrLanguagePreference::TraditionalChinese,
        image,
        cancellation,
    )?;
    let projection = project_chinese_lines(&recognized.lines, &combined);
    Ok(ChineseRecognition {
        result: RecognitionResult {
            lines: projection.lines.clone(),
            preprocessing_elapsed,
            recognition_elapsed: recognized.elapsed,
            was_cached: false,
            physical_line_identities: projection.identities.clone(),
            ..RecognitionResult::default()
        },
        combined,
        projection,
    })
}

#[derive(Clone)]
struct ChineseCandidate {
    start_detected: usize,
    line_count: usize,
    bounds: FloatRect,
    edit_distance: usize,
}

fn chinese_candidates(
    projection: &ChineseProjection,
    target: &FullLineAffixMatcher,
) -> Vec<ChineseCandidate> {
    if target.template().tokens.len() < 2 || projection.detected.is_empty() {
        return Vec::new();
    }
    let token_count = target.template().tokens.len();
    let allowance = if token_count <= 8 {
        4.min(token_count)
    } else {
        token_count.div_ceil(2).clamp(4, 8)
    };
    let mut candidates = Vec::new();
    for start in 0..projection.detected.len() {
        let mut combined = String::new();
        let mut bounds = projection.detected[start].bounds;
        for span in 1..=target
            .maximum_line_span()
            .min(projection.detected.len() - start)
        {
            let current = &projection.detected[start + span - 1];
            if span > 1 {
                let previous = &projection.detected[start + span - 2];
                if current.logical_line_index != previous.logical_line_index + 1 {
                    break;
                }
                bounds = bounds.union(current.bounds);
                combined.push(' ');
            }
            combined.push_str(&current.text);
            let actual = canonicalize(&combined);
            let distance =
                token_edit_distance(&target.template().tokens, &actual.tokens, allowance);
            if (1..=allowance).contains(&distance) {
                candidates.push(ChineseCandidate {
                    start_detected: start,
                    line_count: span,
                    bounds,
                    edit_distance: distance,
                });
            }
        }
    }
    candidates.sort_by_key(|candidate| {
        (
            candidate.edit_distance,
            candidate.line_count,
            candidate.start_detected,
        )
    });
    candidates
}

struct ChineseRecovery {
    text: String,
    canonical_target: String,
    elapsed: Duration,
    start_line_index: usize,
    physical_line_count: usize,
}

struct ChineseRecoveryRequest<'a> {
    prepared: PreparedFrame<'a>,
    localized_mask: &'a BlueTextMask,
    combined: &'a CropMetadata,
    projection: &'a ChineseProjection,
    cancellation: &'a dyn CancellationProbe,
}

fn recover_chinese_quick(
    layout: &mut dyn LayoutOcrBackend,
    localized: Option<&mut (dyn LocalizedOcrBackend + '_)>,
    segmented_cache: &mut BoundedSegmentedCache,
    target: &FullLineAffixMatcher,
    request: ChineseRecoveryRequest<'_>,
) -> Result<Option<ChineseRecovery>, RecognitionError> {
    let ChineseRecoveryRequest {
        prepared,
        localized_mask,
        combined,
        projection,
        cancellation,
    } = request;
    let attempts = if target.template().tokens.len() >= 10 {
        4
    } else {
        2
    };
    let mut localized = localized;
    let mut attempted_paddle_bands = HashSet::new();
    for candidate in chinese_candidates(projection, target)
        .into_iter()
        .take(attempts)
    {
        let (crop, rect) = original_candidate_crop(prepared.frame, combined, candidate.bounds)?;
        let refined = layout.recognize(
            OcrLanguagePreference::TraditionalChinese,
            crop,
            cancellation,
        )?;
        let refined_lines = refined
            .lines
            .iter()
            .map(|line| crate::projection::normalize_chinese_numeric_dashes(line.text.trim()))
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if let Some(found) = target.find_match(&refined_lines) {
            return Ok(Some(ChineseRecovery {
                text: found.original_text,
                canonical_target: target.template().text.clone(),
                elapsed: refined.elapsed,
                start_line_index: projection.detected[candidate.start_detected].logical_line_index,
                physical_line_count: candidate.line_count,
            }));
        }
        if let Some(backend) = localized.as_deref_mut() {
            let mut supported = Vec::new();
            let mut paddle_elapsed = Duration::ZERO;
            for band in localized_bands_in_rect(localized_mask, rect, 4)? {
                if !attempted_paddle_bands.insert(band.content_fingerprint) {
                    continue;
                }
                let paddle = backend.recognize_batch(
                    mask_gray8(localized_mask, &band)?,
                    std::slice::from_ref(target),
                    cancellation,
                )?;
                paddle_elapsed = paddle_elapsed.saturating_add(paddle.elapsed);
                let verified = strict_or_strong_targets(&paddle, std::slice::from_ref(target));
                for canonical_target in &verified {
                    supported.push((paddle.text.clone(), canonical_target.clone()));
                }
                if verified.is_empty() {
                    let (segmented, segmented_elapsed) = recover_segmented_strict(
                        backend,
                        segmented_cache,
                        localized_mask,
                        &band,
                        &paddle,
                        std::slice::from_ref(target),
                        cancellation,
                    )?;
                    paddle_elapsed = paddle_elapsed.saturating_add(segmented_elapsed);
                    for matched in segmented {
                        supported.push((matched.original_text, matched.canonical_target));
                    }
                }
            }
            if let [(text, canonical_target)] = supported.as_slice() {
                return Ok(Some(ChineseRecovery {
                    text: text.clone(),
                    canonical_target: canonical_target.clone(),
                    elapsed: refined.elapsed.saturating_add(paddle_elapsed),
                    start_line_index: projection.detected[candidate.start_detected]
                        .logical_line_index,
                    physical_line_count: candidate.line_count,
                }));
            }
        }
    }
    Ok(None)
}

fn recover_chinese_structured(
    layout: &mut dyn LayoutOcrBackend,
    localized: Option<&mut (dyn LocalizedOcrBackend + '_)>,
    segmented_cache: &mut BoundedSegmentedCache,
    targets: &[&FullLineAffixMatcher],
    request: ChineseRecoveryRequest<'_>,
) -> Result<(Vec<AssistedModifierObservation>, Duration), RecognitionError> {
    let ChineseRecoveryRequest {
        prepared,
        localized_mask,
        combined,
        projection,
        cancellation,
    } = request;
    let mut jobs: Vec<(ChineseCandidate, Vec<&FullLineAffixMatcher>)> = Vec::new();
    for target in targets {
        if target.find_match(&projection.lines).is_some() {
            continue;
        }
        for candidate in chinese_candidates(projection, target) {
            if let Some((_, grouped)) = jobs.iter_mut().find(|(existing, _)| {
                existing.start_detected == candidate.start_detected
                    && existing.line_count == candidate.line_count
            }) {
                grouped.push(*target);
            } else {
                jobs.push((candidate, vec![*target]));
            }
        }
    }
    jobs.sort_by_key(|(candidate, _)| {
        (
            candidate.edit_distance,
            candidate.line_count,
            candidate.start_detected,
        )
    });
    jobs.truncate(MAXIMUM_LOCALIZED_WORK_UNITS);
    let mut localized = localized;
    let mut assisted = Vec::new();
    let mut elapsed = Duration::ZERO;
    for (candidate, targets) in jobs {
        let (crop, rect) = original_candidate_crop(prepared.frame, combined, candidate.bounds)?;
        let refined = layout.recognize(
            OcrLanguagePreference::TraditionalChinese,
            crop,
            cancellation,
        )?;
        elapsed = elapsed.saturating_add(refined.elapsed);
        let refined_lines = refined
            .lines
            .iter()
            .map(|line| crate::projection::normalize_chinese_numeric_dashes(line.text.trim()))
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        let physical_id = format!(
            "winzh:{:016x}:{}:{}",
            combined.content_fingerprint,
            candidate.bounds.top.floor() as i32,
            candidate.bounds.bottom.ceil() as i32
        );
        let related = projection.detected
            [candidate.start_detected..candidate.start_detected + candidate.line_count]
            .iter()
            .map(|line| line.physical_band_id.clone())
            .collect::<Vec<_>>();
        let mut unresolved = Vec::new();
        for target in targets {
            if let Some(found) = target.find_match(&refined_lines) {
                assisted.push(AssistedModifierObservation {
                    physical_band_id: physical_id.clone(),
                    original_text: found.original_text,
                    canonical_target: target.template().text.clone(),
                    related_physical_band_ids: related.clone(),
                });
            } else {
                unresolved.push(target);
            }
        }
        if !unresolved.is_empty()
            && let Some(backend) = localized.as_deref_mut()
        {
            let owned_targets = unresolved
                .iter()
                .map(|target| (*target).clone())
                .collect::<Vec<_>>();
            let bands = localized_bands_in_rect(localized_mask, rect, 2)?;
            if let [band] = bands.as_slice() {
                let paddle = backend.recognize_batch(
                    mask_gray8(localized_mask, band)?,
                    &owned_targets,
                    cancellation,
                )?;
                elapsed = elapsed.saturating_add(paddle.elapsed);
                let verified = strict_or_strong_targets(&paddle, &owned_targets);
                for canonical_target in &verified {
                    assisted.push(AssistedModifierObservation {
                        physical_band_id: physical_id.clone(),
                        original_text: paddle.text.clone(),
                        canonical_target: canonical_target.clone(),
                        related_physical_band_ids: related.clone(),
                    });
                }
                let unresolved = owned_targets
                    .iter()
                    .filter(|target| !verified.contains(&target.template().text))
                    .cloned()
                    .collect::<Vec<_>>();
                let (segmented, segmented_elapsed) = recover_segmented_strict(
                    backend,
                    segmented_cache,
                    localized_mask,
                    band,
                    &paddle,
                    &unresolved,
                    cancellation,
                )?;
                elapsed = elapsed.saturating_add(segmented_elapsed);
                for matched in segmented {
                    assisted.push(AssistedModifierObservation {
                        physical_band_id: physical_id.clone(),
                        original_text: matched.original_text,
                        canonical_target: matched.canonical_target,
                        related_physical_band_ids: related.clone(),
                    });
                }
            }
        }
    }
    assisted.sort_by(|left, right| {
        left.physical_band_id
            .cmp(&right.physical_band_id)
            .then_with(|| left.canonical_target.cmp(&right.canonical_target))
    });
    assisted.dedup_by(|left, right| {
        left.physical_band_id == right.physical_band_id
            && left.canonical_target == right.canonical_target
    });
    Ok((assisted, elapsed))
}

fn original_candidate_crop(
    frame: &CapturedFrame,
    combined: &CropMetadata,
    bounds: FloatRect,
) -> Result<(OwnedBgraImage, PixelRect), RecognitionError> {
    const HORIZONTAL_MARGIN: usize = 40;
    const VERTICAL_MARGIN: usize = 8;
    let source_left = combined.source_rect.x;
    let source_top = combined.source_rect.y;
    let detected_left = bounds.left.floor().max(0.0) as usize;
    let detected_top = bounds.top.floor().max(0.0) as usize;
    let detected_right = bounds.right.ceil().max(0.0) as usize;
    let detected_bottom = bounds.bottom.ceil().max(0.0) as usize;
    let left = source_left
        .saturating_add(detected_left)
        .saturating_sub(HORIZONTAL_MARGIN)
        .min(frame.width() - 1);
    let top = source_top
        .saturating_add(detected_top)
        .saturating_sub(VERTICAL_MARGIN)
        .min(frame.height() - 1);
    let right = source_left
        .saturating_add(detected_right)
        .saturating_add(HORIZONTAL_MARGIN)
        .min(frame.width() - 1)
        .max(left);
    let bottom = source_top
        .saturating_add(detected_bottom)
        .saturating_add(VERTICAL_MARGIN)
        .min(frame.height() - 1)
        .max(top);
    let rect = PixelRect::new(left, top, right - left + 1, bottom - top + 1)
        .map_err(|error| RecognitionError::InvalidImage(error.to_string()))?;
    Ok((original_bgra_rect(frame, rect)?, rect))
}

fn original_bgra_rect(
    frame: &CapturedFrame,
    rect: PixelRect,
) -> Result<OwnedBgraImage, RecognitionError> {
    rect.validate_within(frame.width(), frame.height())
        .map_err(|error| RecognitionError::InvalidImage(error.to_string()))?;
    let stride = rect.width * 4;
    let mut pixels = vec![0; stride * rect.height];
    for row in 0..rect.height {
        let source_start = (rect.y + row) * frame.stride() + rect.x * 4;
        pixels[row * stride..(row + 1) * stride]
            .copy_from_slice(&frame.bgra_pixels()[source_start..source_start + stride]);
    }
    OwnedBgraImage::new(rect.width, rect.height, stride, pixels).map_err(RecognitionError::from)
}

fn localized_bands_in_rect(
    mask: &BlueTextMask,
    rect: PixelRect,
    maximum: usize,
) -> Result<Vec<CropMetadata>, RecognitionError> {
    let mut detector = PhysicalBandDetector::new();
    let mut bands = detector
        .detect(mask, BandDetectionSettings::default())?
        .bands
        .into_iter()
        .filter(|band| {
            let content = band.crop.content_rect;
            content.y >= rect.y
                && content.bottom_inclusive() <= rect.bottom_inclusive()
                && content.x < rect.right_exclusive()
                && content.right_exclusive() > rect.x
        })
        .collect::<Vec<_>>();
    let center = rect.y.saturating_add(rect.height / 2);
    bands.sort_by_key(|band| {
        let content_center = band
            .crop
            .content_rect
            .y
            .saturating_add(band.crop.content_rect.height / 2);
        content_center.abs_diff(center)
    });
    bands.truncate(maximum);
    Ok(bands.into_iter().map(|band| band.crop).collect())
}

fn target_key(target: &FullLineAffixMatcher) -> String {
    format!("{}:{}", target.maximum_line_span(), target.template().text)
}

fn strict_or_strong_targets(
    recognized: &crate::backend::LocalizedRecognition,
    requested: &[FullLineAffixMatcher],
) -> Vec<String> {
    // A normal strict product-matcher hit remains authoritative, exactly as on the primary OCR
    // route. Target-conditioned evidence is consumed only when its typed support is strong.
    let mut verified = requested
        .iter()
        .filter(|target| target.is_match(&recognized.text))
        .map(|target| target.template().text.clone())
        .collect::<Vec<_>>();
    verified.extend(
        recognized
            .target_supports
            .iter()
            .filter(|support| support.strongly_supported)
            .filter(|support| {
                requested
                    .iter()
                    .any(|target| target.template().text == support.canonical_target)
            })
            .map(|support| support.canonical_target.clone()),
    );
    verified.sort();
    verified.dedup();
    verified
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SegmentedTargetMatch {
    original_text: String,
    canonical_target: String,
}

fn recover_segmented_strict(
    localized: &mut dyn LocalizedOcrBackend,
    cache: &mut BoundedSegmentedCache,
    mask: &BlueTextMask,
    metadata: &CropMetadata,
    preliminary: &LocalizedRecognition,
    targets: &[FullLineAffixMatcher],
    cancellation: &dyn CancellationProbe,
) -> Result<(Vec<SegmentedTargetMatch>, Duration), RecognitionError> {
    let mut unresolved = targets.iter().collect::<Vec<_>>();
    let mut matches = Vec::new();
    let mut elapsed = Duration::ZERO;
    for maximum_segment_width in segmentation_widths(preliminary, targets) {
        let key = SegmentedCacheKey {
            fingerprint: metadata.content_fingerprint,
            image_width: metadata.source_rect.width,
            image_height: metadata.source_rect.height,
            maximum_segment_width,
        };
        let segmented = if let Some(cached) = cache.get(&key) {
            cached
        } else {
            let recognized = localized.recognize_segmented(
                mask_gray8(mask, metadata)?,
                maximum_segment_width,
                cancellation,
            )?;
            cache.insert(key, recognized.clone());
            recognized
        };
        elapsed = elapsed.saturating_add(segmented.elapsed);
        unresolved.retain(|target| {
            if target.is_match(&segmented.text) {
                matches.push(SegmentedTargetMatch {
                    original_text: segmented.text.clone(),
                    canonical_target: target.template().text.clone(),
                });
                false
            } else {
                true
            }
        });
        if unresolved.is_empty() {
            break;
        }
    }
    Ok((matches, elapsed))
}

fn segmentation_widths(
    preliminary: &LocalizedRecognition,
    targets: &[FullLineAffixMatcher],
) -> Vec<usize> {
    let mut widths = Vec::with_capacity(2);
    for target in targets {
        if !could_benefit_from_segmentation(preliminary, target) {
            continue;
        }
        let prefer_long = preliminary.tensor_width >= 800 && preliminary.mean_confidence < 0.82;
        if prefer_long && preliminary.tensor_width > 400 && !widths.contains(&400) {
            widths.push(400);
        }
        if preliminary.tensor_width > 320 && !widths.contains(&320) {
            widths.push(320);
        }
        if !prefer_long
            && preliminary.tensor_width > 400
            && (preliminary.tensor_width >= 720 || preliminary.mean_confidence < 0.75)
            && !widths.contains(&400)
        {
            widths.push(400);
        }
    }
    widths
}

fn could_benefit_from_segmentation(
    preliminary: &LocalizedRecognition,
    target: &FullLineAffixMatcher,
) -> bool {
    if preliminary.tensor_width >= 800 && preliminary.mean_confidence < 0.82 {
        return true;
    }
    let actual = canonicalize(&preliminary.text);
    let expected = &target.template().tokens;
    let allowance = (expected.len() / 8).clamp(2, 4);
    expected.len().abs_diff(actual.tokens.len()) <= allowance
        && token_edit_distance(expected, &actual.tokens, allowance) <= allowance
}

fn normalize_targets(targets: &[FullLineAffixMatcher]) -> Vec<&FullLineAffixMatcher> {
    let mut by_key = HashMap::new();
    for target in targets {
        by_key.entry(target_key(target)).or_insert(target);
    }
    let mut normalized: Vec<_> = by_key.into_iter().collect();
    normalized.sort_by(|left, right| left.0.cmp(&right.0));
    normalized.into_iter().map(|(_, target)| target).collect()
}

fn target_set_signature(targets: &[&FullLineAffixMatcher]) -> String {
    let mut signature = String::new();
    for target in targets {
        let key = target_key(target);
        signature.push_str(&format!("{}:{key};", key.len()));
    }
    signature
}

fn cached_result(source: &RecognitionResult) -> RecognitionResult {
    let mut result = source.clone();
    result.preprocessing_elapsed = Duration::ZERO;
    result.recognition_elapsed = Duration::ZERO;
    result.was_cached = true;
    result.requires_rescan = false;
    result
}

fn contains_localized_candidate(lines: &[String], target: &FullLineAffixMatcher) -> bool {
    let expected = word_tokens(&target.template().tokens);
    for start in 0..lines.len() {
        if lines[start].trim().is_empty() {
            continue;
        }
        let mut combined = String::new();
        for span in 1..=target.maximum_line_span().min(lines.len() - start) {
            let line = lines[start + span - 1].trim();
            if line.is_empty() {
                break;
            }
            if !combined.is_empty() {
                combined.push(' ');
            }
            combined.push_str(line);
            let candidate = canonicalize(&combined);
            if exact_or_glyph_localizable(&expected, &word_tokens(&candidate.tokens)) {
                return true;
            }
        }
    }
    false
}

fn word_tokens(tokens: &[AffixToken]) -> Vec<&str> {
    tokens
        .iter()
        .filter(|token| token.kind.is_word())
        .map(|token| token.text.as_str())
        .collect()
}

fn exact_or_glyph_localizable(expected: &[&str], actual: &[&str]) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let mut substitutions = 0;
    for (expected, actual) in expected.iter().zip(actual) {
        if expected == actual {
            continue;
        }
        if expected.chars().count() < 4 || expected.chars().count() != actual.chars().count() {
            return false;
        }
        for (left, right) in expected.chars().zip(actual.chars()) {
            if left != right {
                substitutions += 1;
                if substitutions > 2 {
                    return false;
                }
            }
        }
    }
    true
}

fn token_edit_distance(expected: &[AffixToken], actual: &[AffixToken], maximum: usize) -> usize {
    if expected.len().abs_diff(actual.len()) > maximum {
        return maximum + 1;
    }
    let mut previous: Vec<usize> = (0..=actual.len()).collect();
    let mut current = vec![0; actual.len() + 1];
    for (row, expected_token) in expected.iter().enumerate() {
        current[0] = row + 1;
        let mut row_minimum = current[0];
        for (column, actual_token) in actual.iter().enumerate() {
            let substitution = usize::from(expected_token != actual_token);
            current[column + 1] = (current[column] + 1)
                .min(previous[column + 1] + 1)
                .min(previous[column] + substitution);
            row_minimum = row_minimum.min(current[column + 1]);
        }
        if row_minimum > maximum {
            return maximum + 1;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[actual.len()]
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    use poe_alarm_core::{Decimal, extract_values};
    use poe_alarm_ocr_win::RecognizedLine;
    use poe_alarm_vision::{BlueMaskSettings, CaptureRegion, CapturedFrame, build_blue_mask};

    use super::*;
    use crate::backend::{
        LayoutRecognition, LocalizedRecognition, LocalizedTargetSupport, NeverCancelled,
    };

    type SharedLayoutCalls = Arc<Mutex<Vec<(OcrLanguagePreference, usize, usize)>>>;
    type SharedBatchCalls = Arc<Mutex<Vec<Vec<String>>>>;
    type SharedSegmentedCalls = Arc<Mutex<Vec<usize>>>;

    struct FakeLayout {
        responses: VecDeque<Vec<RecognizedLine>>,
        errors: VecDeque<RecognitionError>,
        unavailable_at_start: bool,
        calls: SharedLayoutCalls,
    }

    impl FakeLayout {
        fn new(responses: Vec<Vec<RecognizedLine>>) -> (Self, SharedLayoutCalls) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    responses: responses.into(),
                    errors: VecDeque::new(),
                    unavailable_at_start: false,
                    calls: Arc::clone(&calls),
                },
                calls,
            )
        }

        fn failing(errors: Vec<RecognitionError>) -> (Self, SharedLayoutCalls) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    responses: VecDeque::new(),
                    errors: errors.into(),
                    unavailable_at_start: false,
                    calls: Arc::clone(&calls),
                },
                calls,
            )
        }

        fn unavailable_at_start() -> (Self, SharedLayoutCalls) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    responses: VecDeque::new(),
                    errors: VecDeque::new(),
                    unavailable_at_start: true,
                    calls: Arc::clone(&calls),
                },
                calls,
            )
        }
    }

    impl LayoutOcrBackend for FakeLayout {
        fn recognize(
            &mut self,
            language: OcrLanguagePreference,
            image: OwnedBgraImage,
            _cancellation: &dyn CancellationProbe,
        ) -> Result<crate::backend::LayoutRecognition, RecognitionError> {
            self.calls
                .lock()
                .unwrap()
                .push((language, image.width(), image.height()));
            if let Some(error) = self.errors.pop_front() {
                return Err(error);
            }
            Ok(LayoutRecognition {
                elapsed: Duration::from_millis(1),
                lines: self.responses.pop_front().unwrap_or_default(),
            })
        }

        fn requires_traditional_compatibility(&self) -> bool {
            self.unavailable_at_start
        }
    }

    struct FakeLocalized {
        batch_responses: VecDeque<LocalizedRecognition>,
        segmented_responses: VecDeque<LocalizedRecognition>,
        batch_calls: SharedBatchCalls,
        segmented_calls: SharedSegmentedCalls,
    }

    impl FakeLocalized {
        fn new(responses: Vec<LocalizedRecognition>) -> (Self, SharedBatchCalls) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    batch_responses: responses.into(),
                    segmented_responses: VecDeque::new(),
                    batch_calls: Arc::clone(&calls),
                    segmented_calls: Arc::new(Mutex::new(Vec::new())),
                },
                calls,
            )
        }

        fn with_segmented(
            batch_responses: Vec<LocalizedRecognition>,
            segmented_responses: Vec<LocalizedRecognition>,
        ) -> (Self, SharedBatchCalls, SharedSegmentedCalls) {
            let batch_calls = Arc::new(Mutex::new(Vec::new()));
            let segmented_calls = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    batch_responses: batch_responses.into(),
                    segmented_responses: segmented_responses.into(),
                    batch_calls: Arc::clone(&batch_calls),
                    segmented_calls: Arc::clone(&segmented_calls),
                },
                batch_calls,
                segmented_calls,
            )
        }
    }

    impl LocalizedOcrBackend for FakeLocalized {
        fn recognize_batch(
            &mut self,
            _image: OwnedImage,
            targets: &[FullLineAffixMatcher],
            _cancellation: &dyn CancellationProbe,
        ) -> Result<LocalizedRecognition, RecognitionError> {
            self.batch_calls.lock().unwrap().push(
                targets
                    .iter()
                    .map(|target| target.template().text.clone())
                    .collect(),
            );
            Ok(self
                .batch_responses
                .pop_front()
                .unwrap_or(LocalizedRecognition {
                    text: String::new(),
                    elapsed: Duration::from_millis(2),
                    mean_confidence: 1.0,
                    tensor_width: 320,
                    target_supports: Vec::new(),
                }))
        }

        fn recognize_segmented(
            &mut self,
            _image: OwnedImage,
            maximum_segment_width: usize,
            _cancellation: &dyn CancellationProbe,
        ) -> Result<LocalizedRecognition, RecognitionError> {
            self.segmented_calls
                .lock()
                .unwrap()
                .push(maximum_segment_width);
            Ok(self
                .segmented_responses
                .pop_front()
                .unwrap_or(LocalizedRecognition {
                    text: String::new(),
                    elapsed: Duration::from_millis(2),
                    mean_confidence: 1.0,
                    tensor_width: maximum_segment_width,
                    target_supports: Vec::new(),
                }))
        }
    }

    fn localized_response(
        text: &str,
        supports: &[(&FullLineAffixMatcher, bool)],
    ) -> LocalizedRecognition {
        LocalizedRecognition {
            text: text.to_owned(),
            elapsed: Duration::from_millis(2),
            mean_confidence: 1.0,
            tensor_width: 320,
            target_supports: supports
                .iter()
                .map(|(target, strongly_supported)| LocalizedTargetSupport {
                    canonical_target: target.template().text.clone(),
                    strongly_supported: *strongly_supported,
                })
                .collect(),
        }
    }

    fn text_line(text: &str) -> RecognizedLine {
        RecognizedLine {
            text: text.to_owned(),
            left: 0.0,
            top: 0.0,
            width: 180.0,
            height: 18.0,
        }
    }

    fn chinese_lines(lines: &[(&str, f32)]) -> Vec<RecognizedLine> {
        lines
            .iter()
            .map(|(text, top)| RecognizedLine {
                text: (*text).to_owned(),
                left: 2.0,
                top: *top,
                width: 160.0,
                height: 18.0,
            })
            .collect()
    }

    fn fixture() -> (CapturedFrame, BlueTextMask) {
        let width = 240;
        let height = 80;
        let stride = width * 4;
        let mut pixels = vec![0_u8; stride * height];
        for y in 20..29 {
            for x in 20..201 {
                if x % 3 == 0 || y % 2 == 0 {
                    let offset = y * stride + x * 4;
                    pixels[offset] = 220;
                    pixels[offset + 1] = 100;
                    pixels[offset + 2] = 80;
                    pixels[offset + 3] = 255;
                }
            }
        }
        let frame = CapturedFrame::from_bgra(
            CaptureRegion::new(0, 0, width as u32, height as u32).unwrap(),
            stride,
            pixels,
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        let mask = build_blue_mask(&frame, BlueMaskSettings::default());
        (frame, mask)
    }

    fn multi_band_fixture() -> (CapturedFrame, BlueTextMask) {
        let width = 240;
        let height = 180;
        let stride = width * 4;
        let mut pixels = vec![0_u8; stride * height];
        for top in [10, 40, 70, 100, 130] {
            for y in top..top + 9 {
                for x in 20..201 {
                    if x % 3 == 0 || y % 2 == 0 {
                        let offset = y * stride + x * 4;
                        // Visible to the established Traditional-Chinese Paddle mask (100/14),
                        // deliberately invisible to the normal Windows-first mask (105/18).
                        pixels[offset] = 104;
                        pixels[offset + 1] = 88;
                        pixels[offset + 2] = 88;
                        pixels[offset + 3] = 255;
                    }
                }
            }
        }
        let frame = CapturedFrame::from_bgra(
            CaptureRegion::new(0, 0, width as u32, height as u32).unwrap(),
            stride,
            pixels,
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        let mask = build_blue_mask(&frame, BlueMaskSettings::default());
        (frame, mask)
    }

    fn prepared<'a>(frame: &'a CapturedFrame, mask: &'a BlueTextMask) -> PreparedFrame<'a> {
        PreparedFrame {
            frame,
            blue_mask: mask,
            semantic_fingerprint: mask.fingerprint(),
        }
    }

    #[test]
    fn poe1_english_uses_scale_two_and_reuses_the_band_cache() {
        let (layout, calls) =
            FakeLayout::new(vec![vec![text_line("170% increased Physical Damage")]]);
        let mut adapter =
            RecognitionAdapter::new(RecognitionProfile::POE1_ENGLISH, Box::new(layout), None);
        let (frame, mask) = fixture();
        let target = FullLineAffixMatcher::new("#% increased Physical Damage").unwrap();
        let first = adapter
            .recognize_quick_with(prepared(&frame, &mask), &target, &NeverCancelled)
            .unwrap();
        let second = adapter
            .recognize_quick_with(prepared(&frame, &mask), &target, &NeverCancelled)
            .unwrap();
        assert!(target.find_match(&first.lines).is_some());
        assert!(second.was_cached);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1 > 180);
    }

    #[test]
    fn poe2_structured_is_one_batch_per_pass_and_confirmation_is_cached() {
        let transcript = vec![text_line("25% increased Attack Speed")];
        let (layout, primary_calls) = FakeLayout::new(vec![transcript.clone()]);
        let (confirmation, confirmation_calls) = FakeLayout::new(vec![transcript]);
        let mut adapter = RecognitionAdapter::new_with_confirmation(
            RecognitionProfile::POE2_ENGLISH,
            Box::new(layout),
            Some(Box::new(confirmation)),
            None,
        );
        let (frame, mask) = fixture();
        let targets = vec![
            FullLineAffixMatcher::new("#% increased Attack Speed").unwrap(),
            FullLineAffixMatcher::new("#% increased Cast Speed").unwrap(),
            FullLineAffixMatcher::new("#% increased Attack Speed").unwrap(),
        ];
        let first = adapter
            .recognize_structured_with(prepared(&frame, &mask), &targets, &NeverCancelled)
            .unwrap();
        assert!(first.requires_rescan);
        assert!(!first.physical_line_identities.is_empty());
        let reordered = vec![targets[1].clone(), targets[0].clone()];
        let second = adapter
            .recognize_structured_with(prepared(&frame, &mask), &reordered, &NeverCancelled)
            .unwrap();
        assert!(!second.requires_rescan);
        let third = adapter
            .recognize_structured_with(prepared(&frame, &mask), &targets, &NeverCancelled)
            .unwrap();
        assert!(third.was_cached);
        assert_eq!(
            third.physical_line_identities,
            second.physical_line_identities
        );
        assert_eq!(third.assisted_observations, second.assisted_observations);
        assert_eq!(primary_calls.lock().unwrap().len(), 1);
        assert_eq!(confirmation_calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn switching_routes_clears_stale_progressive_confirmation_state() {
        let miss = vec![text_line("25% increased Attack Speed")];
        let (layout, calls) = FakeLayout::new(vec![miss]);
        let mut adapter =
            RecognitionAdapter::new(RecognitionProfile::POE2_ENGLISH, Box::new(layout), None);
        let (frame, mask) = fixture();
        let target = FullLineAffixMatcher::new("#% increased Cast Speed").unwrap();

        let quick = adapter
            .recognize_quick_with(prepared(&frame, &mask), &target, &NeverCancelled)
            .unwrap();
        assert!(quick.requires_rescan);
        let structured = adapter
            .recognize_structured_with(
                prepared(&frame, &mask),
                std::slice::from_ref(&target),
                &NeverCancelled,
            )
            .unwrap();
        assert!(structured.requires_rescan);
        let quick_again = adapter
            .recognize_quick_with(prepared(&frame, &mask), &target, &NeverCancelled)
            .unwrap();

        assert!(quick_again.requires_rescan);
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn changed_fingerprint_or_target_cannot_confirm_stale_quick_evidence() {
        let miss = vec![text_line("25% increased Attack Speed")];
        let (layout, _) = FakeLayout::new(vec![miss]);
        let mut adapter =
            RecognitionAdapter::new(RecognitionProfile::POE2_ENGLISH, Box::new(layout), None);
        let (frame, mask) = fixture();
        let cast = FullLineAffixMatcher::new("#% increased Cast Speed").unwrap();
        let critical = FullLineAffixMatcher::new("#% increased Critical Hit Chance").unwrap();

        assert!(
            adapter
                .recognize_quick_with(prepared(&frame, &mask), &cast, &NeverCancelled)
                .unwrap()
                .requires_rescan
        );
        let changed_fingerprint = PreparedFrame {
            semantic_fingerprint: mask.fingerprint().wrapping_add(1),
            ..prepared(&frame, &mask)
        };
        assert!(
            adapter
                .recognize_quick_with(changed_fingerprint, &cast, &NeverCancelled)
                .unwrap()
                .requires_rescan
        );
        assert!(
            adapter
                .recognize_quick_with(prepared(&frame, &mask), &critical, &NeverCancelled)
                .unwrap()
                .requires_rescan
        );
    }

    #[test]
    fn traditional_language_unavailable_switches_once_to_full_paddle_compatibility() {
        let unavailable = RecognitionError::Windows(OcrError::LanguageUnavailable {
            requested: "Traditional Chinese".to_owned(),
            available: vec!["en-US".to_owned()],
        });
        let (layout, layout_calls) = FakeLayout::failing(vec![unavailable]);
        let target = FullLineAffixMatcher::new("#% increased Physical Damage").unwrap();
        let paddle = localized_response("170% increased Physical Damage", &[(&target, true)]);
        let (localized, paddle_calls) = FakeLocalized::new(vec![paddle.clone(), paddle]);
        let mut adapter = RecognitionAdapter::new(
            RecognitionProfile::POE1_TRADITIONAL_CHINESE,
            Box::new(layout),
            Some(Box::new(localized)),
        );
        let (frame, mask) = fixture();

        let first = adapter
            .recognize_quick_with(prepared(&frame, &mask), &target, &NeverCancelled)
            .unwrap();
        let mut changed = prepared(&frame, &mask);
        changed.semantic_fingerprint = changed.semantic_fingerprint.wrapping_add(1);
        let second = adapter
            .recognize_quick_with(changed, &target, &NeverCancelled)
            .unwrap();

        assert!(target.find_match(&first.lines).is_some());
        assert!(target.find_match(&second.lines).is_some());
        assert_eq!(layout_calls.lock().unwrap().len(), 1);
        assert_eq!(paddle_calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn traditional_winrt_start_failure_uses_bounded_progressive_paddle_work() {
        let (layout, layout_calls) = FakeLayout::unavailable_at_start();
        let target = FullLineAffixMatcher::new("#% increased Physical Damage").unwrap();
        let responses = (0..5)
            .map(|index| {
                localized_response(
                    &format!("{}% increased Physical Damage", 170 + index),
                    &[(&target, true)],
                )
            })
            .collect();
        let (localized, paddle_calls) = FakeLocalized::new(responses);
        let mut adapter = RecognitionAdapter::new(
            RecognitionProfile::POE1_TRADITIONAL_CHINESE,
            Box::new(layout),
            Some(Box::new(localized)),
        );
        let (frame, mask) = multi_band_fixture();
        assert!(mask.intensities().iter().all(|intensity| *intensity == 0));
        assert_eq!(TRADITIONAL_PADDLE_MASK_SETTINGS.minimum_blue, 100);
        assert_eq!(TRADITIONAL_PADDLE_MASK_SETTINGS.minimum_blue_dominance, 14);
        let localized_mask = build_blue_mask(&frame, TRADITIONAL_PADDLE_MASK_SETTINGS);
        assert!(
            localized_mask
                .intensities()
                .iter()
                .any(|intensity| *intensity != 0)
        );
        assert_eq!(
            PhysicalBandDetector::new()
                .detect(&localized_mask, BandDetectionSettings::default())
                .unwrap()
                .bands
                .len(),
            5
        );

        let first = adapter
            .recognize_structured_with(
                prepared(&frame, &mask),
                std::slice::from_ref(&target),
                &NeverCancelled,
            )
            .unwrap();

        assert!(first.requires_rescan);
        assert_eq!(layout_calls.lock().unwrap().len(), 0);
        assert_eq!(paddle_calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn traditional_non_language_winrt_error_does_not_silently_fallback() {
        let winrt = RecognitionError::Windows(OcrError::WinRt {
            operation: "recognize",
            hresult: -1,
            message: "synthetic failure".to_owned(),
        });
        let (layout, _) = FakeLayout::failing(vec![winrt]);
        let target = FullLineAffixMatcher::new("#% increased Physical Damage").unwrap();
        let (localized, paddle_calls) = FakeLocalized::new(vec![localized_response(
            "170% increased Physical Damage",
            &[(&target, true)],
        )]);
        let mut adapter = RecognitionAdapter::new(
            RecognitionProfile::POE1_TRADITIONAL_CHINESE,
            Box::new(layout),
            Some(Box::new(localized)),
        );
        let (frame, mask) = fixture();

        assert!(matches!(
            adapter.recognize_quick_with(prepared(&frame, &mask), &target, &NeverCancelled),
            Err(RecognitionError::Windows(OcrError::WinRt { .. }))
        ));
        assert!(paddle_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn localized_structured_batch_accepts_only_strong_support_and_keeps_physical_identity() {
        let (layout, _) =
            FakeLayout::new(vec![vec![text_line("10% increased Critical Hit Chanee")]]);
        let strong = FullLineAffixMatcher::new("#% increased Critical Hit Chance").unwrap();
        let weak_neighbor = FullLineAffixMatcher::new("#% increased Critical Hit Change").unwrap();
        let response = localized_response(
            "10% increased Critical Hit Chanee",
            &[(&strong, true), (&weak_neighbor, false)],
        );
        let (localized, localized_calls) = FakeLocalized::new(vec![response]);
        let mut adapter = RecognitionAdapter::new(
            RecognitionProfile::POE2_ENGLISH,
            Box::new(layout),
            Some(Box::new(localized)),
        );
        let (frame, mask) = fixture();
        let targets = vec![strong, weak_neighbor];
        let result = adapter
            .recognize_structured_with(prepared(&frame, &mask), &targets, &NeverCancelled)
            .unwrap();
        let calls = localized_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].len(), 2);
        assert_eq!(result.assisted_observations.len(), 1);
        assert_eq!(
            result.assisted_observations[0].canonical_target,
            targets[0].template().text
        );
        assert_eq!(
            result.assisted_observations[0].original_text,
            "10% increased Critical Hit Chanee"
        );
        assert_eq!(result.physical_line_identities.len(), 1);
        assert_eq!(
            result.assisted_observations[0].physical_band_id,
            result.physical_line_identities[0].physical_band_id
        );
    }

    #[test]
    fn segmented_recovery_is_shared_by_targets_and_rejects_a_near_neighbor() {
        let (layout, _) =
            FakeLayout::new(vec![vec![text_line("10% increased Critical Hit Chanee")]]);
        let expected = FullLineAffixMatcher::new("#% increased Critical Hit Chance").unwrap();
        let near_neighbor = FullLineAffixMatcher::new("#% increased Critical Hit Change").unwrap();
        let mut weak = localized_response(
            "10% increased Critical Hit Chanee",
            &[(&expected, false), (&near_neighbor, false)],
        );
        weak.tensor_width = 321;
        weak.mean_confidence = 0.9;
        let segmented = localized_response("10% increased Critical Hit Chance", &[]);
        let (localized, batch_calls, segmented_calls) =
            FakeLocalized::with_segmented(vec![weak], vec![segmented]);
        let mut adapter = RecognitionAdapter::new(
            RecognitionProfile::POE2_ENGLISH,
            Box::new(layout),
            Some(Box::new(localized)),
        );
        let (frame, mask) = fixture();
        let targets = vec![expected, near_neighbor];

        let result = adapter
            .recognize_structured_with(prepared(&frame, &mask), &targets, &NeverCancelled)
            .unwrap();

        let batch_calls = batch_calls.lock().unwrap();
        assert_eq!(batch_calls.len(), 1);
        assert_eq!(batch_calls[0].len(), 2);
        assert_eq!(*segmented_calls.lock().unwrap(), vec![320]);
        assert_eq!(result.assisted_observations.len(), 1);
        assert_eq!(
            result.assisted_observations[0].canonical_target,
            targets[0].template().text
        );
        assert_eq!(
            result.assisted_observations[0].original_text,
            "10% increased Critical Hit Chance"
        );
        assert_eq!(
            result.assisted_observations[0].physical_band_id,
            result.physical_line_identities[0].physical_band_id
        );
    }

    #[test]
    fn quick_confirmation_does_not_repeat_a_rejected_localized_recovery() {
        let miss = vec![text_line("10% increased Critical Hit Chanee")];
        let (layout, _) = FakeLayout::new(vec![miss.clone(), miss]);
        let target = FullLineAffixMatcher::new("#% increased Critical Hit Chance").unwrap();
        let weak = localized_response("10% increased Critical Hit Chanee", &[(&target, false)]);
        let (localized, batch_calls) = FakeLocalized::new(vec![weak]);
        let mut adapter = RecognitionAdapter::new(
            RecognitionProfile::POE2_ENGLISH,
            Box::new(layout),
            Some(Box::new(localized)),
        );
        let (frame, mask) = fixture();

        let first = adapter
            .recognize_quick_with(prepared(&frame, &mask), &target, &NeverCancelled)
            .unwrap();
        assert!(first.requires_rescan);
        let second = adapter
            .recognize_quick_with(prepared(&frame, &mask), &target, &NeverCancelled)
            .unwrap();

        assert!(!second.requires_rescan);
        assert_eq!(batch_calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn traditional_decimal_result_preserves_the_displayed_value() {
        let (layout, calls) = FakeLayout::new(vec![chinese_lines(&[("+ 3 · 73 % 暴 擊 率", 2.0)])]);
        let mut adapter = RecognitionAdapter::new(
            RecognitionProfile::POE2_TRADITIONAL_CHINESE,
            Box::new(layout),
            None,
        );
        let (frame, mask) = fixture();
        let target = FullLineAffixMatcher::new("+(3.11—3.8)%暴擊率").unwrap();
        let result = adapter
            .recognize_quick_with(prepared(&frame, &mask), &target, &NeverCancelled)
            .unwrap();
        assert!(target.find_match(&result.lines).is_some());
        assert_eq!(
            extract_values(&result.lines[0]),
            vec![Some(Decimal::from(3.73))]
        );
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn band_cache_is_strictly_bounded_to_256_fifo_entries() {
        let mut cache = BoundedBandCache::default();
        for fingerprint in 0..=MAXIMUM_CACHED_BANDS as u64 {
            cache.insert(
                BandCacheKey {
                    fingerprint,
                    width: 1,
                    height: 1,
                },
                vec![fingerprint.to_string()],
            );
        }
        assert_eq!(cache.values.len(), MAXIMUM_CACHED_BANDS);
        assert!(
            cache
                .get(&BandCacheKey {
                    fingerprint: 0,
                    width: 1,
                    height: 1,
                })
                .is_none()
        );
    }
}
