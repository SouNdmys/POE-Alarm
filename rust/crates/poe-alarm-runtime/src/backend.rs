//! The one seam between the actor and whatever produces affix lines.
//!
//! This file used to carry a `CaptureBackend` / `RecognizerBackend` /
//! `ScreenshotBackend` / `BackendFactory` stack whose only purpose was to keep
//! the OCR engine swappable — a frame producer, a recognizer over that frame, a
//! screenshot decoder, and adapters bridging each to the monitor. Reading the
//! client's own item text needs no frame and no model, so all that survives is
//! a way to hand the actor a source it did not construct itself, which is what
//! lets the actor's generation, protection and shutdown contracts be tested
//! without a running game.

use std::fmt;

use poe_alarm_monitoring::{
    AffixSource, CancellationToken, MonitorPlan, RecognitionResult, StructuredOcrSupport,
};

use crate::clipboard_source::{ClipboardSource, SourceError};

/// A runtime service could not be started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendError(pub String);

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BackendError {}

/// An affix source chosen at run time.
pub type BoxedAffixSource = Box<dyn AffixSource<Error = SourceError> + Send>;

/// Supplies the live monitor with a source.
///
/// Production always answers with a [`ClipboardSource`]; tests answer with a
/// fake that can be told to match, to block, or to fail.
pub trait AffixSourceFactory: Send + Sync + 'static {
    fn create(&self) -> Result<BoxedAffixSource, BackendError>;
}

/// Adapts a boxed source back into the concrete type the monitor is generic over.
///
/// The blanket impl this replaces cannot be written here: both `Box` and
/// `AffixSource` are foreign, so the newtype is what makes the impl legal.
pub struct DynamicSource(pub BoxedAffixSource);

impl AffixSource for DynamicSource {
    type Error = SourceError;

    fn structured_support(&self) -> StructuredOcrSupport {
        self.0.structured_support()
    }

    fn read(
        &mut self,
        plan: &MonitorPlan,
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, Self::Error> {
        self.0.read(plan, cancellation)
    }
}

/// The shipped source factory.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProductionSourceFactory;

impl AffixSourceFactory for ProductionSourceFactory {
    fn create(&self) -> Result<BoxedAffixSource, BackendError> {
        Ok(Box::new(ClipboardSource::new()))
    }
}
