//! Platform-neutral monitoring state machine for POE Alarm.
//!
//! The worker owns the hot sequence: reusable capture, semantic blue-text fingerprint, changed
//! frame OCR, and core matcher/rule evaluation. Identical accepted frames never enter OCR.
//! Structured rules always use one shared batch request per changed frame. This crate emits the
//! short input-guard request/release signal but deliberately contains no Win32 hook or UI code.

#![forbid(unsafe_code)]

mod cancellation;
mod clock;
mod engine;
mod model;

pub use cancellation::CancellationToken;
pub use clock::{CACHED_SCAN_DELAY, MonitorClock, ScanPace, SystemClock, UNCACHED_SCAN_DELAY};
pub use engine::Monitor;
pub use model::{
    AffixSource, EventSink, MonitorDetection, MonitorEvent, MonitorPlan, MonitorSnapshot,
    MonitorState, NoopEventSink, RecognitionResult, SessionId, StartError, StopError,
    StructuredOcrSupport,
};
