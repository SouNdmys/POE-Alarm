use std::marker::PhantomData;
use std::mem::size_of;
use std::num::NonZeroIsize;
use std::rc::Rc;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_WINDOW, EndPaint, GetSysColorBrush, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_EXSTYLE,
    GetWindowLongPtrW, HTTRANSPARENT, HWND_TOPMOST, IsWindow, MA_NOACTIVATE, RegisterClassExW,
    SET_WINDOW_POS_FLAGS, SW_HIDE, SWP_FRAMECHANGED, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowDisplayAffinity, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WINDOW_EX_STYLE, WM_MOUSEACTIVATE,
    WM_NCHITTEST, WM_PAINT, WNDCLASSEXW, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::hud::{CaptureAffinity, HudWindowConfig, HudWindowPolicy};
use crate::{NativeWindowHandle, PlatformError, RectI};

use super::error_from_windows;

const HUD_CLASS: PCWSTR = w!("PoeAlarmPlatformStatusHud");
const PASSIVE_STYLE_BITS: isize = 0x0800_0020; // WS_EX_NOACTIVATE | WS_EX_TRANSPARENT

static HUD_CLASS_READY: OnceLock<Result<(), PlatformError>> = OnceLock::new();

/// Thread-bound owner of the native status HUD window.
pub(crate) struct NativeHudWindow {
    hwnd: HWND,
    _thread_bound: PhantomData<Rc<()>>,
}

impl std::fmt::Debug for NativeHudWindow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeHudWindow")
            .field("hwnd", &(self.hwnd.0 as usize))
            .finish()
    }
}

impl NativeHudWindow {
    pub(crate) fn create(config: HudWindowConfig) -> Result<Self, PlatformError> {
        ensure_class()?;
        let module = unsafe { GetModuleHandleW(None) }
            .map_err(|error| error_from_windows("GetModuleHandleW", error))?;
        let extended = WINDOW_EX_STYLE(config.policy.compose_extended_style(0));
        // SAFETY: The registered class and all pointer parameters are valid;
        // this owner destroys the returned top-level HWND on the same thread.
        let hwnd = unsafe {
            CreateWindowExW(
                extended,
                HUD_CLASS,
                w!("POE Alarm status"),
                WS_POPUP,
                config.bounds.x,
                config.bounds.y,
                config.bounds.width,
                config.bounds.height,
                None,
                None,
                Some(module.into()),
                None,
            )
        }
        .map_err(|error| error_from_windows("CreateWindowExW(status HUD)", error))?;
        let mut window = Self {
            hwnd,
            _thread_bound: PhantomData,
        };
        if let Err(error) = window.set_capture_affinity(config.policy.capture_affinity) {
            drop(window);
            return Err(error);
        }
        window.set_bounds(config.bounds, config.policy)?;
        if config.visible {
            window.show(config.policy)?;
        }
        Ok(window)
    }

    pub(crate) fn window_handle(&self) -> NativeWindowHandle {
        let raw = self.hwnd.0 as isize;
        NativeWindowHandle::from_known_valid(
            NonZeroIsize::new(raw).expect("CreateWindowExW returned a non-null HWND"),
        )
    }

    pub(crate) fn apply_policy(&mut self, policy: HudWindowPolicy) -> Result<(), PlatformError> {
        // SAFETY: hwnd is live and thread-owned by self.
        let existing = unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) } as u32;
        let composed = policy.compose_extended_style(existing);
        // SAFETY: Only extended style bits are replaced on our live HWND.
        unsafe { SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, composed as isize) };
        // SetWindowLongPtrW can return zero both on success and failure, so
        // verify the resulting value instead of interpreting its return value.
        let observed = unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) } as u32;
        if observed != composed {
            return Err(PlatformError::Win32 {
                operation: "SetWindowLongPtrW(status HUD policy)",
                code: 0,
                message: format!(
                    "extended style verification failed (wanted 0x{composed:08X}, got 0x{observed:08X})"
                ),
            });
        }
        // SAFETY: Flags request a non-moving/non-sizing frame refresh.
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
        }
        .map_err(|error| error_from_windows("SetWindowPos(apply HUD policy)", error))
    }

    pub(crate) fn set_capture_affinity(
        &mut self,
        affinity: CaptureAffinity,
    ) -> Result<(), PlatformError> {
        let native = match affinity {
            CaptureAffinity::Include => WDA_NONE,
            CaptureAffinity::Exclude => WDA_EXCLUDEFROMCAPTURE,
        };
        // SAFETY: SetWindowDisplayAffinity accepts this live top-level window.
        unsafe { SetWindowDisplayAffinity(self.hwnd, native) }
            .map_err(|error| error_from_windows("SetWindowDisplayAffinity", error))
    }

    pub(crate) fn set_bounds(
        &mut self,
        bounds: RectI,
        policy: HudWindowPolicy,
    ) -> Result<(), PlatformError> {
        let flags = SET_WINDOW_POS_FLAGS(policy.show_position_flags() & !SWP_SHOWWINDOW.0);
        // SAFETY: hwnd and HWND_TOPMOST are valid; RectI proves positive dimensions.
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                flags,
            )
        }
        .map_err(|error| error_from_windows("SetWindowPos(position HUD)", error))
    }

    pub(crate) fn show(&mut self, policy: HudWindowPolicy) -> Result<(), PlatformError> {
        let flags =
            SET_WINDOW_POS_FLAGS(policy.show_position_flags() | SWP_NOMOVE.0 | SWP_NOSIZE.0);
        // SAFETY: hwnd is live; this reasserts topmost without changing geometry.
        unsafe { SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags) }
            .map_err(|error| error_from_windows("SetWindowPos(show HUD)", error))
    }

    pub(crate) fn hide(&mut self) -> Result<(), PlatformError> {
        // SAFETY: hwnd is live and SW_HIDE does not activate it.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW,
            )
        }
        .map_err(|error| error_from_windows("SetWindowPos(hide HUD)", error))
    }

    pub(super) fn raw_handle(&self) -> HWND {
        self.hwnd
    }

    pub(super) fn extended_style(&self) -> u32 {
        // SAFETY: self owns a live HWND.
        unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32 }
    }
}

impl Drop for NativeHudWindow {
    fn drop(&mut self) {
        // SAFETY: This type is !Send and destroys its owned HWND at most once.
        if unsafe { IsWindow(Some(self.hwnd)) }.as_bool() {
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

fn ensure_class() -> Result<(), PlatformError> {
    HUD_CLASS_READY
        .get_or_init(|| {
            let module = unsafe { GetModuleHandleW(None) }
                .map_err(|error| error_from_windows("GetModuleHandleW", error))?;
            // SAFETY: System color brushes are process-stable and must not be deleted.
            let background = unsafe { GetSysColorBrush(COLOR_WINDOW) };
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(hud_window_proc),
                hInstance: module.into(),
                hbrBackground: background,
                lpszClassName: HUD_CLASS,
                ..Default::default()
            };
            // SAFETY: WNDCLASSEXW points only to static class data.
            if unsafe { RegisterClassExW(&class) } == 0 {
                return Err(error_from_windows(
                    "RegisterClassExW(status HUD)",
                    windows::core::Error::from_win32(),
                ));
            }
            Ok(())
        })
        .clone()
}

unsafe extern "system" fn hud_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => {
            // SAFETY: Querying styles for the window currently dispatching the message.
            let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
            if style & PASSIVE_STYLE_BITS == PASSIVE_STYLE_BITS {
                LRESULT(HTTRANSPARENT as isize)
            } else {
                // SAFETY: Forward unhandled hit testing to the system procedure.
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
        WM_MOUSEACTIVATE => {
            // SAFETY: Querying styles for the window currently dispatching the message.
            let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
            if style & 0x0800_0000 != 0 {
                LRESULT(MA_NOACTIVATE as isize)
            } else {
                // SAFETY: Placement mode deliberately permits ordinary activation behavior.
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            // SAFETY: Standard balanced paint pair for this HWND.
            unsafe {
                BeginPaint(hwnd, &mut paint);
                let _ = EndPaint(hwnd, &paint);
            }
            LRESULT(0)
        }
        _ => {
            // SAFETY: Default handling for all messages not owned by this adapter.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

pub(super) fn is_window(hwnd: HWND) -> bool {
    // SAFETY: IsWindow accepts stale handles for diagnostic probing.
    unsafe { IsWindow(Some(hwnd)) }.as_bool()
}

pub(super) const fn required_passive_style_bits() -> u32 {
    0x0800_00A8 // TOPMOST | TRANSPARENT | TOOLWINDOW | NOACTIVATE
}
