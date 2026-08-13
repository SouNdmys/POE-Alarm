use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};

use crate::PlatformError;
use crate::hotkeys::{HotKeyBinding, HotKeyTarget};

use super::error_from_windows;

pub(crate) fn register_hot_key(
    target: HotKeyTarget,
    identifier: i32,
    binding: HotKeyBinding,
) -> Result<(), PlatformError> {
    let window = target_window(target);
    let modifiers = HOT_KEY_MODIFIERS(binding.modifiers.bits() | MOD_NOREPEAT.0);
    // SAFETY: target_window returns either null (current thread) or the live
    // HWND guaranteed by NativeWindowHandle's caller contract.
    unsafe { RegisterHotKey(window, identifier, modifiers, binding.virtual_key) }
        .map_err(|error| error_from_windows("RegisterHotKey", error))
}

pub(crate) fn unregister_hot_key(target: HotKeyTarget, identifier: i32) {
    // SAFETY: Registration ownership is tracked by HotKeyManager and the target
    // is identical to the one used for RegisterHotKey.
    let _ = unsafe { UnregisterHotKey(target_window(target), identifier) };
}

fn target_window(target: HotKeyTarget) -> Option<HWND> {
    match target {
        HotKeyTarget::CurrentThread => None,
        HotKeyTarget::Window(window) => Some(HWND(window.as_raw() as usize as *mut c_void)),
    }
}
