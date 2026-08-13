//! Non-blocking application runtime for native POE Alarm.
//!
//! The Win32 UI owns only a bounded command sender and a non-blocking event
//! receiver. Capture, OCR, monitor shutdown, screenshot replay and alert
//! coordination run on background threads. There is intentionally no careful
//! or yellow-alert mode in this runtime.

#![forbid(unsafe_code)]

mod actor;
mod backend;
mod compile;
mod model;
mod protection;

#[cfg(test)]
mod tests;

pub use actor::{ProductionRuntimeConfig, RuntimeHandle, RuntimeSendError};
pub use backend::{
    BackendError, BackendFactory, CaptureBackend, ProductionBackendFactory, RecognizerBackend,
};
pub use compile::{
    CompiledRuntimeSettings, SettingsFieldError, SettingsValidationError, compile_settings,
};
pub use model::{
    AlertCopy, CompiledUiBindings, DetectionSummary, RuntimeCommand, RuntimeEvent,
    RuntimeGeneration, RuntimeOperation, RuntimeRequestId, RuntimeState, ScreenshotEvaluation,
    ScreenshotReport, ScreenshotRequest,
};
pub use protection::{
    AlertLatchStatus, AlertPresentation, NativeProtection, ProtectionError, ProtectionEvent,
    ProtectionService,
};
