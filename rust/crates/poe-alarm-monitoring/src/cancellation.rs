use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
struct CancellationState {
    cancelled: Mutex<bool>,
    wake: Condvar,
}

/// A cooperative cancellation token passed to OCR and pacing adapters.
///
/// Implementations of [`crate::OcrRecognizer`] must observe this token while doing potentially
/// blocking work. That is what lets `Monitor::stop` wait for its sole worker without leaving a
/// detached OCR operation behind.
#[derive(Clone, Default)]
pub struct CancellationToken {
    state: Arc<CancellationState>,
}

impl CancellationToken {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        *self
            .state
            .cancelled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Waits until cancellation is requested.
    pub fn wait_cancelled(&self) {
        let mut cancelled = self
            .state
            .cancelled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*cancelled {
            cancelled = self
                .state
                .wake
                .wait(cancelled)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// Returns `true` when cancellation won, or `false` when the timeout elapsed.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let cancelled = self
            .state
            .cancelled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *cancelled {
            return true;
        }
        let (cancelled, _) = self
            .state
            .wake
            .wait_timeout_while(cancelled, timeout, |value| !*value)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cancelled
    }

    pub(crate) fn cancel(&self) {
        let mut cancelled = self
            .state
            .cancelled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !*cancelled {
            *cancelled = true;
            self.state.wake.notify_all();
        }
    }
}

impl fmt::Debug for CancellationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CancellationToken")
            .field("is_cancelled", &self.is_cancelled())
            .finish()
    }
}
