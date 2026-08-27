mod clipboard;
mod hotkeys;
mod hud;
mod mouse_hook;
mod self_test;
mod wave;

pub(crate) use clipboard::{
    clipboard_sequence_number, confirm_relaunch_elevated, copy_hovered_item,
    foreground_process_outranks_us, foreground_window_description, game_window_rect,
    held_modifiers, process_is_elevated, read_clipboard_text, relaunch_elevated,
};
pub(crate) use hotkeys::{register_hot_key, unregister_hot_key};
pub(crate) use hud::NativeHudWindow;
pub(crate) use mouse_hook::NativePendingMouseGuard;
pub(crate) use self_test::run_self_test;
pub(crate) use wave::{play_wave, stop_wave};

use crate::PlatformError;

pub(super) fn error_from_windows(
    operation: &'static str,
    error: windows::core::Error,
) -> PlatformError {
    let hresult = error.code().0 as u32;
    let code = if hresult & 0xFFFF_0000 == 0x8007_0000 {
        hresult & 0x0000_FFFF
    } else {
        hresult
    };
    PlatformError::Win32 {
        operation,
        code,
        message: error.message(),
    }
}
