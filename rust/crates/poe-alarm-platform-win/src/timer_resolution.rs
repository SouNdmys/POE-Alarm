//! Portable face of the 1ms timer-resolution guard.

/// Holds the system timer at 1ms until dropped; a no-op off Windows or when
/// the request is refused.
pub struct TimerResolutionGuard {
    #[cfg(windows)]
    _native: Option<crate::win32::NativeTimerResolutionGuard>,
}

/// Requests 1ms scheduler resolution for the guard's lifetime.
#[must_use]
pub fn request_fine_timer_resolution() -> TimerResolutionGuard {
    #[cfg(windows)]
    {
        TimerResolutionGuard {
            _native: crate::win32::request_fine_timer_resolution(),
        }
    }
    #[cfg(not(windows))]
    {
        TimerResolutionGuard {}
    }
}
