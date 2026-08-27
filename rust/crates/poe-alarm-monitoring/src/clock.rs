use std::time::{Duration, Instant};

use crate::CancellationToken;

/// How long to pace between readings.
///
/// Both are 35ms because that is what measured best against a real client, by a
/// wide margin and against the intuition that guided every earlier value. Over
/// roughly 1500 crafts at a 50ms click cadence this produced one overshoot;
/// halving it to an effective 31ms produced more than ten times that rate, and
/// the 15.5ms these constants used to resolve to was worse still.
///
/// Two forces cross here. Poll faster and the client spends its time
/// serializing item text for us instead of applying the orb, so the roll itself
/// arrives later; poll slower and the roll is on time but sampled late. The
/// optimum is not deducible from this side of the boundary — three earlier
/// attempts to reason it out were wrong, twice in the direction of polling
/// harder — so it is a measurement, and `POE_ALARM_SCAN_MS` exists to repeat
/// that measurement rather than argue with it.
///
/// Both values are identical because Windows quantises these waits to its
/// 15.6ms tick: anything from 31 to 45 resolves to the same ~46.5ms, so a
/// distinction between them would be fiction.
pub const UNCACHED_SCAN_DELAY: Duration = Duration::from_millis(35);
pub const CACHED_SCAN_DELAY: Duration = Duration::from_millis(35);

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

/// Environment variable that overrides both scan delays, in milliseconds.
///
/// Exists because the only way to find the right polling rate is to measure it
/// against a real client, and every attempt to reason about it from here has
/// been wrong in both directions. Two changes that raised the rate both made
/// detection worse in the field: the client has to serialize the whole item
/// and write the clipboard for every request, so polling harder spends its time
/// answering us rather than applying the orb we are waiting for. Whether the
/// same is true going the other way is an open question that one person with a
/// macro can settle in ten minutes, and cannot be settled here at all.
///
/// Unset, pacing is exactly what shipped.
pub const SCAN_DELAY_OVERRIDE: &str = "POE_ALARM_SCAN_MS";

/// `Instant`-backed clock used by the application worker.
#[derive(Debug)]
pub struct SystemClock {
    origin: Instant,
    /// Replaces both `ScanPace` delays when the override is set.
    override_delay: Option<Duration>,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            override_delay: read_delay_override(),
        }
    }
}

/// Reads the override once, ignoring anything unparseable or absurd.
///
/// Capped at a second: a typo that pauses polling for an hour would look
/// exactly like the app being broken, and there is no legitimate reason to
/// pace slower than that.
fn read_delay_override() -> Option<Duration> {
    let raw = std::env::var(SCAN_DELAY_OVERRIDE).ok()?;
    let millis: u64 = raw.trim().parse().ok()?;
    (1..=1000)
        .contains(&millis)
        .then(|| Duration::from_millis(millis))
}

impl MonitorClock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn pace(&mut self, pace: ScanPace, cancellation: &CancellationToken) {
        // The override replaces the delay but not the decision to wait at all:
        // ProgressiveYield still yields, because that path means the evidence
        // is incomplete rather than that the item is unchanged.
        match pace
            .delay()
            .map(|delay| self.override_delay.unwrap_or(delay))
        {
            Some(delay) => {
                let _ = cancellation.wait_timeout(delay);
            }
            None if !cancellation.is_cancelled() => std::thread::yield_now(),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_progressive_yield_never_becomes_a_wait() {
        // The override is about how long to pace between finished readings.
        // ProgressiveYield means the evidence is incomplete and the loop must
        // come straight back, so it has no delay to replace.
        assert_eq!(ScanPace::ProgressiveYield.delay(), None);
    }

    #[test]
    fn both_finished_paces_carry_a_delay_to_override() {
        assert!(ScanPace::UncachedDelay.delay().is_some());
        assert!(ScanPace::CachedDelay.delay().is_some());
    }
}
