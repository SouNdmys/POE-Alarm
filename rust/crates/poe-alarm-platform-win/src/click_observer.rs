//! Portable face of the click observer: a counter of the user's left clicks.
//!
//! Exists for the click-invoked copy. The observer never suppresses or
//! synthesizes anything; it counts `WM_LBUTTONDOWN` and passes every event
//! through. See `win32::click_observer` for the native half.

use crate::PlatformError;

/// A running click counter; dropping it stops the native observer.
pub struct ClickObserver {
    #[cfg(windows)]
    _native: crate::win32::NativeClickObserver,
}

/// Starts the process-wide click observer.
pub fn start_click_observer() -> Result<ClickObserver, PlatformError> {
    #[cfg(windows)]
    {
        crate::win32::start_click_observer().map(|native| ClickObserver { _native: native })
    }
    #[cfg(not(windows))]
    {
        Err(PlatformError::unsupported("click observer"))
    }
}

/// Total left clicks observed since process start.
#[must_use]
pub fn observed_clicks() -> u64 {
    #[cfg(windows)]
    {
        crate::win32::observed_clicks()
    }
    #[cfg(not(windows))]
    {
        0
    }
}
