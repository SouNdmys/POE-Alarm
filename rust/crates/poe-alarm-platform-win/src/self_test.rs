use crate::PlatformError;

/// Native resource lifecycle checks performed without showing interactive UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformSelfTestReport {
    pub hud_window_create_cleanup: bool,
    pub hot_key_register_cleanup: bool,
    pub region_window_create_cleanup: bool,
    pub mouse_hook_create_cleanup: bool,
}

/// Creates and cleans each native resource used by the platform crate.
///
/// The hot-key probe uses Ctrl+Alt+Shift+F24 rather than the product bindings,
/// so a running POE Alarm instance does not make the lifecycle test conflict.
pub fn run_windows_self_test() -> Result<PlatformSelfTestReport, PlatformError> {
    #[cfg(windows)]
    {
        crate::win32::run_self_test()
    }
    #[cfg(not(windows))]
    {
        crate::non_windows::run_self_test()
    }
}
