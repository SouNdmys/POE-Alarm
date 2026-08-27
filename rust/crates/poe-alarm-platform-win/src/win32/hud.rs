use std::marker::PhantomData;
use std::mem::size_of;
use std::num::NonZeroIsize;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, CreateFontW,
    CreateSolidBrush, DEFAULT_CHARSET, DT_END_ELLIPSIS, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    DeleteObject, DrawTextW, EndPaint, FW_NORMAL, FW_SEMIBOLD, FillRect, FrameRect,
    GetSysColorBrush, HFONT, HGDIOBJ, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_EXSTYLE,
    GetWindowLongPtrW, HTCAPTION, HTTRANSPARENT, HWND_TOPMOST, IsWindow, MA_NOACTIVATE,
    RegisterClassExW, SET_WINDOW_POS_FLAGS, SW_HIDE, SWP_FRAMECHANGED, SWP_HIDEWINDOW,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowDisplayAffinity,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WINDOW_EX_STYLE,
    WM_EXITSIZEMOVE, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT, WNDCLASSEXW, WS_POPUP,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};
use windows::core::{PCWSTR, w};

use crate::hud::{CaptureAffinity, HudContent, HudWindowConfig, HudWindowPolicy};
use crate::{NativeWindowHandle, PlatformError, RectI};

use super::error_from_windows;

const HUD_CLASS: PCWSTR = w!("PoeAlarmPlatformStatusHud");
const PASSIVE_STYLE_BITS: isize = 0x0800_0020; // WS_EX_NOACTIVATE | WS_EX_TRANSPARENT

static HUD_CLASS_READY: OnceLock<Result<(), PlatformError>> = OnceLock::new();

/// 单实例 HUD 的当前内容;wndproc 在 WM_PAINT 里读取。
static HUD_CONTENT: Mutex<Option<HudContent>> = Mutex::new(None);

/// 用户拖动结束后的窗口左上角(屏幕坐标);服务线程轮询取走。
static HUD_USER_MOVE: Mutex<Option<(i32, i32)>> = Mutex::new(None);

const fn hud_rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

fn hud_font(height: i32, weight: i32) -> HFONT {
    // SAFETY: CreateFontW with a fixed family name; caller deletes via DeleteObject.
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Microsoft YaHei UI"),
        )
    }
}

unsafe fn hud_text(
    dc: windows::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    color: COLORREF,
    text: &str,
    mut rect: RECT,
    flags: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    if text.is_empty() {
        return;
    }
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: dc/font live for this paint; DrawTextW bounds by rect.
    unsafe {
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        SetTextColor(dc, color);
        DrawTextW(dc, &mut wide, &mut rect, flags);
        SelectObject(dc, previous);
    }
}

/// Ledger 两态状态卡绘制。
unsafe fn paint_hud(hwnd: HWND) {
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: balanced Begin/EndPaint on our HWND.
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    let content = HUD_CONTENT
        .lock()
        .map(|c| c.clone())
        .unwrap_or_default()
        .unwrap_or_default();

    let (bar, border, dot, status_color, meta) = if content.monitoring {
        (
            hud_rgb(0x0E, 0x6A, 0x64),
            hud_rgb(0xA6, 0xC9, 0xC4),
            hud_rgb(0x0E, 0x6A, 0x64),
            hud_rgb(0x0B, 0x53, 0x4E),
            hud_rgb(0x52, 0x4C, 0x41),
        )
    } else {
        (
            hud_rgb(0xA7, 0x9E, 0x8E),
            hud_rgb(0xCB, 0xC2, 0xB2),
            hud_rgb(0xA7, 0x9E, 0x8E),
            hud_rgb(0x52, 0x4C, 0x41),
            hud_rgb(0x6F, 0x67, 0x59),
        )
    };

    // SAFETY: brushes/fonts created and released within this scope.
    unsafe {
        let canvas = CreateSolidBrush(hud_rgb(0xF5, 0xF2, 0xEC));
        let border_brush = CreateSolidBrush(border);
        let bar_brush = CreateSolidBrush(bar);
        let dot_brush = CreateSolidBrush(dot);
        FillRect(dc, &client, canvas);
        FrameRect(dc, &client, border_brush);
        let left_bar = RECT {
            left: client.left,
            top: client.top,
            right: client.left + 2,
            bottom: client.bottom,
        };
        FillRect(dc, &left_bar, bar_brush);
        let dot_rect = RECT {
            left: client.left + 11,
            top: client.top + 12,
            right: client.left + 17,
            bottom: client.top + 18,
        };
        FillRect(dc, &dot_rect, dot_brush);
        SetBkMode(dc, TRANSPARENT);

        let status_font = hud_font(16, FW_SEMIBOLD.0 as i32);
        let mono_font = hud_font(14, FW_NORMAL.0 as i32);
        hud_text(
            dc,
            status_font,
            status_color,
            &content.status_text,
            RECT {
                left: client.left + 24,
                top: client.top + 6,
                right: client.right - 70,
                bottom: client.top + 26,
            },
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        hud_text(
            dc,
            mono_font,
            meta,
            &content.elapsed,
            RECT {
                left: client.right - 68,
                top: client.top + 6,
                right: client.right - 10,
                bottom: client.top + 26,
            },
            DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
        );
        hud_text(
            dc,
            mono_font,
            meta,
            &content.target,
            RECT {
                left: client.left + 11,
                top: client.top + 28,
                right: client.right - 10,
                bottom: client.bottom - 4,
            },
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        let _ = DeleteObject(HGDIOBJ(status_font.0));
        let _ = DeleteObject(HGDIOBJ(mono_font.0));
        let _ = DeleteObject(HGDIOBJ(canvas.0));
        let _ = DeleteObject(HGDIOBJ(border_brush.0));
        let _ = DeleteObject(HGDIOBJ(bar_brush.0));
        let _ = DeleteObject(HGDIOBJ(dot_brush.0));
        let _ = EndPaint(hwnd, &paint);
    }
}

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

    pub(crate) fn set_content(&mut self, content: HudContent) -> Result<(), PlatformError> {
        if let Ok(mut slot) = HUD_CONTENT.lock() {
            *slot = Some(content);
        }
        // SAFETY: hwnd is live; invalidation schedules WM_PAINT on its thread.
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
        Ok(())
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

    /// 取走用户拖动结束后的窗口左上角(无新拖动时为 None)。
    pub(crate) fn take_user_move(&mut self) -> Option<(i32, i32)> {
        HUD_USER_MOVE.lock().ok().and_then(|mut slot| slot.take())
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
                // Placement 模式:整张卡都是拖动把手。
                LRESULT(HTCAPTION as isize)
            }
        }
        WM_EXITSIZEMOVE => {
            let mut rect = RECT::default();
            // SAFETY: hwnd is the live HUD window that just finished a user move.
            if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok()
                && let Ok(mut slot) = HUD_USER_MOVE.lock()
            {
                *slot = Some((rect.left, rect.top));
            }
            // SAFETY: Forward to the system procedure after recording the move.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
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
            // SAFETY: hwnd is the live HUD window being painted.
            unsafe {
                paint_hud(hwnd);
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
