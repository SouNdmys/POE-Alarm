use crate::hotkeys::{HotKeyBinding, HotKeyTarget};
use crate::hud::{CaptureAffinity, HudWindowConfig, HudWindowPolicy};
use crate::mouse_guard::{GuardMode, GuardSnapshot, MouseButtons};
use crate::region_selection::SelectionOverlayConfig;
use crate::{NativeWindowHandle, PlatformError, PlatformSelfTestReport, RectI};

pub(crate) fn register_hot_key(
    _target: HotKeyTarget,
    _identifier: i32,
    _binding: HotKeyBinding,
) -> Result<(), PlatformError> {
    Err(PlatformError::unsupported("global hot keys"))
}

pub(crate) fn unregister_hot_key(_target: HotKeyTarget, _identifier: i32) {}

pub(crate) fn play_wave(_bytes: &[u8]) -> Result<(), PlatformError> {
    Err(PlatformError::unsupported("WinMM WAV playback"))
}

pub(crate) fn stop_wave() -> Result<(), PlatformError> {
    Err(PlatformError::unsupported("WinMM WAV playback"))
}

#[derive(Debug)]
pub(crate) struct NativeHudWindow;

impl NativeHudWindow {
    pub(crate) fn create(_config: HudWindowConfig) -> Result<Self, PlatformError> {
        Err(PlatformError::unsupported("status HUD window"))
    }

    pub(crate) fn window_handle(&self) -> NativeWindowHandle {
        unreachable!("a native HUD cannot be created outside Windows")
    }

    pub(crate) fn set_content(
        &mut self,
        _content: crate::hud::HudContent,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    pub(crate) fn apply_policy(&mut self, _policy: HudWindowPolicy) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("status HUD window"))
    }

    pub(crate) fn set_capture_affinity(
        &mut self,
        _affinity: CaptureAffinity,
    ) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("window display affinity"))
    }

    pub(crate) fn set_bounds(
        &mut self,
        _bounds: RectI,
        _policy: HudWindowPolicy,
    ) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("status HUD window"))
    }

    pub(crate) fn show(&mut self, _policy: HudWindowPolicy) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("status HUD window"))
    }

    pub(crate) fn hide(&mut self) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("status HUD window"))
    }

    pub(crate) fn take_user_move(&mut self) -> Option<(i32, i32)> {
        None
    }
}

#[derive(Debug, Default)]
pub(crate) struct NativePendingMouseGuard;

impl NativePendingMouseGuard {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn prepare(&mut self) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("WH_MOUSE_LL input guard"))
    }

    pub(crate) fn arm(&mut self) -> Result<(), PlatformError> {
        self.prepare()
    }

    pub(crate) fn release(&mut self) {}

    pub(crate) fn force_release_and_stop(&mut self) {}

    pub(crate) fn is_installed(&self) -> bool {
        false
    }

    pub(crate) fn snapshot(&self) -> GuardSnapshot {
        GuardSnapshot {
            mode: GuardMode::Released,
            physical_buttons: MouseButtons::NONE,
            suppressed_buttons: MouseButtons::NONE,
        }
    }
}

pub(crate) fn select_region(
    _config: SelectionOverlayConfig,
) -> Result<Option<RectI>, PlatformError> {
    Err(PlatformError::unsupported("native region selection"))
}

pub(crate) fn run_self_test() -> Result<PlatformSelfTestReport, PlatformError> {
    Err(PlatformError::unsupported("Win32 platform self-test"))
}
