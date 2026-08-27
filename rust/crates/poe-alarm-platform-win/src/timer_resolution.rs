//! Raises the process timer resolution for the lifetime of the app.
//!
//! Windows schedules waits against a periodic clock that ticks every 15.6ms by
//! default, and rounds every timeout up to it. The monitor paces its polling
//! with 4ms and 8ms waits; measured on the default resolution both come back at
//! 15.5ms, which makes the two constants indistinguishable at run time and adds
//! roughly 7ms to a healthy poll and 11ms to one that first spent its copy
//! deadline. Those milliseconds are the difference between catching a reroll
//! and watching the next click take it away.
//!
//! Since Windows 10 2004 the raised resolution applies to the calling process
//! rather than the machine, so this cannot slow anyone else down. The game is
//! almost certainly already holding the timer at 1ms anyway.

/// Holds the raised resolution, restoring it when dropped.
///
/// Every `timeBeginPeriod` must be matched by a `timeEndPeriod`, so ownership
/// rather than a bare call: whatever unwinds or returns, the restore happens.
#[derive(Debug)]
pub struct TimerResolutionGuard {
    /// The period actually granted, or `None` if the request was refused.
    period: Option<u32>,
}

impl TimerResolutionGuard {
    /// Asks for 1ms scheduling, falling back to whatever the system allows.
    ///
    /// A refusal is not an error worth surfacing: the app still runs, polling is
    /// simply paced at the default tick, which is exactly what shipped before.
    #[must_use]
    pub fn acquire() -> Self {
        #[cfg(windows)]
        {
            const PERIOD_MS: u32 = 1;
            // SAFETY: timeBeginPeriod takes a period in milliseconds and is
            // paired with timeEndPeriod in Drop.
            let granted = unsafe { windows::Win32::Media::timeBeginPeriod(PERIOD_MS) };
            // TIMERR_NOERROR is 0; anything else means the request was refused.
            Self {
                period: (granted == 0).then_some(PERIOD_MS),
            }
        }
        #[cfg(not(windows))]
        {
            Self { period: None }
        }
    }

    /// True when the process is actually running at the raised resolution.
    #[must_use]
    pub fn is_raised(&self) -> bool {
        self.period.is_some()
    }
}

impl Drop for TimerResolutionGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(period) = self.period.take() {
            // SAFETY: matches the timeBeginPeriod above exactly once.
            unsafe { windows::Win32::Media::timeEndPeriod(period) };
        }
    }
}
