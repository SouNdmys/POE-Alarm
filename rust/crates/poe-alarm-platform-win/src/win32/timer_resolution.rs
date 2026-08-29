//! Holds the system timer at 1ms for the lifetime of a guard.
//!
//! The click-invoked source spends no injections on a timer, but its reaction
//! time is quantised by the scheduler tick: click observation, the copy
//! schedule and the late-answer catch each eat up to one tick of jitter.
//! Raising the resolution tightens all three at once and sends nothing to
//! anyone. The OCR-era doctrine against touching this was about the *timed
//! injector* — where a faster loop meant more chords and more client
//! contention; here the loop only reads counters.

use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};

/// RAII guard: 1ms scheduler resolution until dropped.
pub(crate) struct NativeTimerResolutionGuard;

pub(crate) fn request_fine_timer_resolution() -> Option<NativeTimerResolutionGuard> {
    // SAFETY: no preconditions; failure returns TIMERR_NOCANDO and holds nothing.
    (unsafe { timeBeginPeriod(1) } == 0).then_some(NativeTimerResolutionGuard)
}

impl Drop for NativeTimerResolutionGuard {
    fn drop(&mut self) {
        // SAFETY: paired with the successful timeBeginPeriod above.
        let _ = unsafe { timeEndPeriod(1) };
    }
}
