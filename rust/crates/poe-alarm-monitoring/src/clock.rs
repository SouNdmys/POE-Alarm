use std::time::{Duration, Instant};

use crate::CancellationToken;

pub const UNCACHED_SCAN_DELAY: Duration = Duration::from_millis(4);
pub const CACHED_SCAN_DELAY: Duration = Duration::from_millis(8);

/// The cooperative pacing selected after one monitor decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanPace {
    /// Progressive evidence is incomplete. Yield and immediately take a fresh capture.
    ProgressiveYield,
    /// A changed frame completed without using cached OCR evidence.
    UncachedDelay,
    /// An unchanged frame, or a changed frame completed from cached OCR evidence.
    CachedDelay,
}

impl ScanPace {
    pub const fn delay(self) -> Option<Duration> {
        match self {
            Self::ProgressiveYield => None,
            Self::UncachedDelay => Some(UNCACHED_SCAN_DELAY),
            Self::CachedDelay => Some(CACHED_SCAN_DELAY),
        }
    }
}

/// Monotonic time and cancellation-aware scan pacing.
///
/// Tests can provide a clock whose `pace` method records and gates each scan. Production clocks
/// must return promptly after `cancellation` is requested.
pub trait MonitorClock: Send + 'static {
    fn now(&self) -> Duration;

    fn pace(&mut self, pace: ScanPace, cancellation: &CancellationToken);
}

/// `Instant`-backed clock used by the application worker.
#[derive(Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl MonitorClock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn pace(&mut self, pace: ScanPace, cancellation: &CancellationToken) {
        match pace.delay() {
            Some(delay) => {
                let _ = cancellation.wait_timeout(delay);
            }
            None if !cancellation.is_cancelled() => std::thread::yield_now(),
            None => {}
        }
    }
}
