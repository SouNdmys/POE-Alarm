use std::fmt;
use std::time::Duration;

use crate::CancellationToken;
use poe_alarm_core::{
    AssistedModifierObservation, CompiledRuleSet, FullLineAffixMatcher, LogicalAffixMatch,
    PhysicalLineIdentity, RuleEvaluationResult,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MonitorState {
    #[default]
    Idle,
    Running,
    MatchFound,
    Faulted,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(pub u64);

#[derive(Clone, Debug)]
pub enum MonitorPlan {
    Quick(FullLineAffixMatcher),
    Structured(CompiledRuleSet),
}

/// Whether a source can judge a whole rule set from one reading.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StructuredOcrSupport {
    #[default]
    Unsupported,
    StrictBatch,
    ConfirmedStrictBatch,
}

/// Evidence returned from one reading of the item under the cursor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecognitionResult {
    pub lines: Vec<String>,
    pub preprocessing_elapsed: Duration,
    pub recognition_elapsed: Duration,
    pub was_cached: bool,
    pub requires_rescan: bool,
    /// Quick-only target-conditioned evidence. Structured evaluation deliberately ignores it.
    pub target_assisted_match: Option<LogicalAffixMatch>,
    /// Structured batch evidence, retaining one-to-one physical component identity.
    pub assisted_observations: Vec<AssistedModifierObservation>,
    pub physical_line_identities: Vec<PhysicalLineIdentity>,
}

impl RecognitionResult {
    pub fn total_elapsed(&self) -> Duration {
        self.preprocessing_elapsed
            .saturating_add(self.recognition_elapsed)
    }
}

/// Where the affix lines come from.
///
/// Deliberately says nothing about how a reading is obtained. The pixel
/// pipeline that preceded this needed the monitor to capture a frame and build
/// a mask before the recognizer could be called, which welded the loop to the
/// screen; asking the client for the item text needs none of that. Keeping the
/// boundary at "give me the lines" is what let the loop stop knowing.
///
/// Deduplication belongs to the implementation, not the caller: only the source
/// knows what "unchanged" means for its medium. Reporting
/// [`RecognitionResult::was_cached`] lets the loop skip an evaluation that
/// would reach the same verdict, and pace down while nothing is happening.
pub trait AffixSource: Send + 'static {
    type Error: fmt::Display + Send + 'static;

    fn structured_support(&self) -> StructuredOcrSupport {
        StructuredOcrSupport::Unsupported
    }

    /// Reads the item under the cursor once.
    ///
    /// `plan` is supplied because a source may narrow its work to the targets
    /// actually being watched.
    fn read(
        &mut self,
        plan: &MonitorPlan,
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, Self::Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonitorSnapshot {
    pub session_id: SessionId,
    pub state: MonitorState,
    pub scan_count: u64,
    pub capture_elapsed: Duration,
    pub ocr_elapsed: Duration,
    pub last_lines: Vec<String>,
    pub detail: Option<String>,
    pub ocr_was_cached: bool,
}

impl MonitorSnapshot {
    pub(crate) fn idle(session_id: SessionId) -> Self {
        Self {
            session_id,
            state: MonitorState::Idle,
            scan_count: 0,
            capture_elapsed: Duration::ZERO,
            ocr_elapsed: Duration::ZERO,
            last_lines: Vec::new(),
            detail: None,
            ocr_was_cached: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MonitorDetection {
    Quick {
        matched: LogicalAffixMatch,
        snapshot: MonitorSnapshot,
    },
    Structured {
        evaluation: RuleEvaluationResult,
        detected_text: String,
        snapshot: MonitorSnapshot,
    },
}

impl MonitorDetection {
    pub fn snapshot(&self) -> &MonitorSnapshot {
        match self {
            Self::Quick { snapshot, .. } | Self::Structured { snapshot, .. } => snapshot,
        }
    }
}

/// Synchronously delivered monitor output.
///
/// In particular, a consumer must latch alert ownership before returning from a `Detection`
/// callback. The worker releases its short input-guard request immediately afterwards. Event
/// callbacks must not panic or call lifecycle methods on the same `Monitor` reentrantly.
#[derive(Clone, Debug, PartialEq)]
pub enum MonitorEvent {
    Snapshot(MonitorSnapshot),
    Detection(MonitorDetection),
    InputGuardRequested { session_id: SessionId },
    InputGuardReleased { session_id: SessionId },
}

pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: MonitorEvent);
}

impl<F> EventSink for F
where
    F: Fn(MonitorEvent) + Send + Sync + 'static,
{
    fn emit(&self, event: MonitorEvent) {
        self(event);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: MonitorEvent) {}
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartError {
    AlreadyRunning,
    StopInProgress,
    StructuredOcrUnsupported,
    WorkerPanicked,
}

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => formatter.write_str("monitoring is already running"),
            Self::StopInProgress => formatter.write_str("monitoring is still stopping"),
            Self::StructuredOcrUnsupported => {
                formatter.write_str("the OCR adapter does not support strict Structured batches")
            }
            Self::WorkerPanicked => formatter.write_str("the previous monitoring worker panicked"),
        }
    }
}

impl std::error::Error for StartError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StopError;

impl fmt::Display for StopError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the monitoring worker panicked while stopping")
    }
}

impl std::error::Error for StopError {}
