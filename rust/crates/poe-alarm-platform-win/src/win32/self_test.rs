use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::VK_F24;

use crate::hotkeys::{HotKeyBinding, HotKeyModifiers, HotKeyTarget};
use crate::hud::{HudWindowConfig, HudWindowPolicy};
use crate::mouse_guard::{GuardMode, MouseButtons};
use crate::{PlatformError, PlatformSelfTestReport, RectI};

use super::hotkeys::{register_hot_key, unregister_hot_key};
use super::hud::{NativeHudWindow, is_window, required_passive_style_bits};
use super::mouse_hook::{NativePendingMouseGuard, installed_for_self_test};
use super::region_overlay::create_destroy_self_test;

const SELF_TEST_HOT_KEY_ID: i32 = 0x50_7E_24;

pub(crate) fn run_self_test() -> Result<PlatformSelfTestReport, PlatformError> {
    let config = HudWindowConfig {
        bounds: RectI::new(0, 0, 64, 64).expect("fixed self-test bounds are valid"),
        policy: HudWindowPolicy::default(),
        visible: false,
    };
    let hud = NativeHudWindow::create(config)?;
    let raw_hud = hud.raw_handle();
    let required = required_passive_style_bits();
    if hud.extended_style() & required != required {
        return Err(PlatformError::Win32 {
            operation: "verify status HUD styles",
            code: 0,
            message: "topmost/no-activate/click-through style bits were incomplete".to_owned(),
        });
    }

    let target = HotKeyTarget::Window(hud.window_handle());
    let binding = HotKeyBinding::new(
        HotKeyModifiers::CONTROL | HotKeyModifiers::ALT | HotKeyModifiers::SHIFT,
        u32::from(VK_F24.0),
    );
    register_hot_key(target, SELF_TEST_HOT_KEY_ID, binding)?;
    unregister_hot_key(target, SELF_TEST_HOT_KEY_ID);

    drop(hud);
    if is_window(raw_hud) {
        return Err(PlatformError::Win32 {
            operation: "DestroyWindow(status HUD self-test)",
            code: 0,
            message: "HUD HWND remained live after cleanup".to_owned(),
        });
    }

    create_destroy_self_test()?;

    let mut guard = NativePendingMouseGuard::new();
    guard.prepare()?;
    if !guard.is_installed() || !installed_for_self_test() {
        return Err(PlatformError::Thread {
            operation: "verify pending mouse guard",
            detail: "WH_MOUSE_LL was not installed".to_owned(),
        });
    }
    guard.force_release_and_stop();
    let released = guard.snapshot();
    if released.mode != GuardMode::Released || released.suppressed_buttons != MouseButtons::NONE {
        return Err(PlatformError::Thread {
            operation: "fail open pending mouse guard",
            detail: "input remained suppressed during terminal cleanup".to_owned(),
        });
    }
    drop(guard);
    // Normal cleanup is asynchronous so a broken native hook thread can never
    // block the product shutdown path. The self-test may wait, but only within
    // this explicit diagnostic deadline, to catch an ordinary resource leak.
    let cleanup_deadline = Instant::now() + Duration::from_millis(250);
    while installed_for_self_test() && Instant::now() < cleanup_deadline {
        thread::sleep(Duration::from_millis(1));
    }
    if installed_for_self_test() {
        return Err(PlatformError::Thread {
            operation: "clean pending mouse guard",
            detail: "WH_MOUSE_LL remained installed after the 250 ms diagnostic deadline"
                .to_owned(),
        });
    }

    Ok(PlatformSelfTestReport {
        hud_window_create_cleanup: true,
        hot_key_register_cleanup: true,
        region_window_create_cleanup: true,
        mouse_hook_create_cleanup: true,
    })
}
