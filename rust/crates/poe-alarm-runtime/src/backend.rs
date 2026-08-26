//! Errors raised while bringing runtime services up.
//!
//! This file used to carry a `CaptureBackend` / `RecognizerBackend` /
//! `BackendFactory` stack whose only purpose was to keep the OCR engine
//! swappable — a frame producer, a recognizer over that frame, a screenshot
//! decoder, and adapters bridging each to the monitor. Reading the client's own
//! item text needs no frame, so a source constructs itself and the abstraction
//! has nothing left to abstract.

use std::fmt;

/// A runtime service could not be started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendError(pub String);

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BackendError {}
