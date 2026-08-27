//! Non-blocking application runtime for native POE Alarm.
//!
//! The Win32 UI owns only a bounded command sender and a non-blocking event
//! receiver. Reading the item under the cursor, monitor shutdown and alert
//! coordination run on background threads. There is intentionally no careful
//! or yellow-alert mode in this runtime.

#![forbid(unsafe_code)]

mod actor;
mod backend;
mod clipboard_source;
mod compile;
mod model;
mod protection;

#[cfg(test)]
mod tests;

pub use actor::{ProductionRuntimeConfig, RuntimeHandle, RuntimeSendError};
pub use backend::{
    AffixSourceFactory, BackendError, BoxedAffixSource, DynamicSource, ProductionSourceFactory,
};
pub use clipboard_source::{ClipboardSource, SourceError};
pub use compile::{
    CompiledRuntimeSettings, SettingsFieldError, SettingsValidationError, compile_settings,
};
pub use model::{
    AlertCopy, CompiledUiBindings, DetectionSummary, ItemCheckEvaluation, ItemCheckReport,
    ItemCheckRequest, RuntimeCommand, RuntimeEvent, RuntimeGeneration, RuntimeOperation,
    RuntimeRequestId, RuntimeState,
};
pub use protection::{
    AlertLatchStatus, AlertPresentation, NativeProtection, ProtectionError, ProtectionEvent,
    ProtectionService,
};
