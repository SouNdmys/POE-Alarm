//! Win32 platform services that preserve the public behavior of POE Alarm 1.0.
//!
//! Geometry, hot-key configuration, WAV parsing, HUD policy, and the pending
//! mouse-input state machine are platform-neutral. All native calls and unsafe
//! code are confined to the private `win32` adapter.

#![forbid(unsafe_op_in_unsafe_fn)]

mod alert_cue;
mod clipboard;
mod error;
mod geometry;
mod handle;
mod hotkeys;
mod hud;
mod mouse_guard;
#[cfg(not(windows))]
mod non_windows;
mod self_test;
mod wave;
#[cfg(windows)]
mod win32;

pub use alert_cue::built_in_alert_wave;
pub use clipboard::{
    ClipboardError, CopyOutcome, ElevateError, HeldModifiers, KeyMethod, SYNTHETIC_INPUT_SIGNATURE,
    confirm_relaunch_elevated, copy_hovered_item, cursor_position, describes_game,
    foreground_window_description, game_is_foreground, game_process_outranks_us, game_window_rect,
    held_modifiers, primary_button_down, process_is_elevated, read_text, relaunch_elevated,
    sequence_number,
};
pub use error::PlatformError;
pub use geometry::{PointI, RectI, SizeI};
pub use handle::NativeWindowHandle;
pub use hotkeys::{
    HotKeyAction, HotKeyBinding, HotKeyConfig, HotKeyError, HotKeyErrorKind, HotKeyManager,
    HotKeyModifiers, HotKeyTarget, StartMonitoringHotKey,
};
pub use hud::{
    CaptureAffinity, HudContent, HudInteractionMode, HudPlacement, HudWindow, HudWindowConfig,
    HudWindowPolicy, resolve_hud_position,
};
pub use mouse_guard::{
    GuardDecision, GuardMode, GuardRelease, GuardSnapshot, MouseButton, MouseButtons,
    MouseGuardStateMachine, MouseInput, PendingMouseInputGuard,
};
pub use self_test::{PlatformSelfTestReport, run_windows_self_test};
pub use wave::{
    LoopingWavePlayer, PcmWaveFormat, ValidatedWave, WaveValidationError, WaveValidationErrorKind,
    validate_pcm_wave,
};
