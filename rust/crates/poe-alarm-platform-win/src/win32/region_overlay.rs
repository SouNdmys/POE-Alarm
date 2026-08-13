use std::ffi::c_void;
use std::mem::size_of;
use std::sync::OnceLock;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetStockObject,
    HGDIOBJ, InvalidateRect, NULL_BRUSH, PAINTSTRUCT, PS_SOLID, Rectangle, SelectObject,
    UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetClientRect, GetCursorPos, GetMessageW, IDC_CROSS, IsWindow,
    LWA_ALPHA, LoadCursorW, PostQuitMessage, RegisterClassExW, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW, SetForegroundWindow,
    SetLayeredWindowAttributes, SetWindowLongPtrW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
    WM_CAPTURECHANGED, WM_CLOSE, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::region_selection::{RegionSelectionState, SelectionOverlayConfig, SelectionUpdate};
use crate::{PlatformError, PointI, RectI};

use super::error_from_windows;

const OVERLAY_CLASS: PCWSTR = w!("PoeAlarmPlatformRegionSelector");
static OVERLAY_CLASS_READY: OnceLock<Result<(), PlatformError>> = OnceLock::new();

struct OverlayContext {
    state: RegionSelectionState,
    hwnd: HWND,
    done: bool,
    selected: Option<RectI>,
}

pub(crate) fn select_region(
    config: SelectionOverlayConfig,
) -> Result<Option<RectI>, PlatformError> {
    let _dpi_context = ThreadDpiContext::per_monitor_v2();
    let desktop = virtual_desktop()?;
    let state = RegionSelectionState::with_minimum_extent(desktop, config.minimum_extent)
        .expect("public overlay configuration validates the minimum extent");
    let mut context = Box::new(OverlayContext {
        state,
        hwnd: HWND::default(),
        done: false,
        selected: None,
    });
    let hwnd = create_overlay_window(&mut context, config.shade_alpha)?;
    context.hwnd = hwnd;
    // SAFETY: The interactive overlay intentionally becomes foreground and is
    // shown only after its context pointer has been attached.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        let _ = SetForegroundWindow(hwnd);
    }

    let mut message = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    while !context.done {
        // SAFETY: Standard message loop with a live MSG out pointer.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        match result.0 {
            -1 => {
                if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                    let _ = unsafe { DestroyWindow(hwnd) };
                }
                return Err(error_from_windows(
                    "GetMessageW(region selection)",
                    windows::core::Error::from_win32(),
                ));
            }
            0 => {
                // Preserve WM_QUIT for the owning application's outer loop.
                unsafe { PostQuitMessage(message.wParam.0 as i32) };
                context.done = true;
                context.selected = None;
            }
            _ => unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            },
        }
    }
    if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        let _ = unsafe { DestroyWindow(hwnd) };
    }
    Ok(context.selected)
}

struct ThreadDpiContext(Option<DPI_AWARENESS_CONTEXT>);

impl ThreadDpiContext {
    fn per_monitor_v2() -> Self {
        // SAFETY: This changes only the calling thread and the previous value
        // is restored by Drop after the modal window has been destroyed.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        Self((!previous.0.is_null()).then_some(previous))
    }
}

impl Drop for ThreadDpiContext {
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            // SAFETY: previous was returned for this same calling thread.
            unsafe { SetThreadDpiAwarenessContext(previous) };
        }
    }
}

fn virtual_desktop() -> Result<RectI, PlatformError> {
    // SAFETY: GetSystemMetrics has no pointer/lifetime preconditions.
    let (x, y, width, height) = unsafe {
        (
            windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_XVIRTUALSCREEN),
            windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_YVIRTUALSCREEN),
            windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_CXVIRTUALSCREEN),
            windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    RectI::new(x, y, width, height).ok_or_else(|| PlatformError::Win32 {
        operation: "GetSystemMetrics(virtual desktop)",
        code: 0,
        message: "Windows returned invalid virtual desktop bounds".to_owned(),
    })
}

fn create_overlay_window(
    context: &mut OverlayContext,
    shade_alpha: u8,
) -> Result<HWND, PlatformError> {
    ensure_class()?;
    let desktop = context.state.desktop();
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|error| error_from_windows("GetModuleHandleW", error))?;
    let extended = WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0);
    // SAFETY: Context is Box-backed by the caller and remains stable through
    // window destruction. WM_NCCREATE stores this pointer as GWLP_USERDATA.
    let hwnd = unsafe {
        CreateWindowExW(
            extended,
            OVERLAY_CLASS,
            w!("POE Alarm region selection"),
            WS_POPUP,
            desktop.x,
            desktop.y,
            desktop.width,
            desktop.height,
            None,
            None,
            Some(module.into()),
            Some((context as *mut OverlayContext).cast::<c_void>()),
        )
    }
    .map_err(|error| error_from_windows("CreateWindowExW(region selection)", error))?;
    // SAFETY: hwnd is a layered window owned by this adapter.
    if let Err(error) =
        unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), shade_alpha, LWA_ALPHA) }
    {
        let _ = unsafe { DestroyWindow(hwnd) };
        return Err(error_from_windows(
            "SetLayeredWindowAttributes(region selection)",
            error,
        ));
    }
    Ok(hwnd)
}

fn ensure_class() -> Result<(), PlatformError> {
    OVERLAY_CLASS_READY
        .get_or_init(|| {
            let module = unsafe { GetModuleHandleW(None) }
                .map_err(|error| error_from_windows("GetModuleHandleW", error))?;
            let cursor = unsafe { LoadCursorW(None, IDC_CROSS) }
                .map_err(|error| error_from_windows("LoadCursorW(IDC_CROSS)", error))?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(overlay_window_proc),
                hInstance: module.into(),
                hCursor: cursor,
                lpszClassName: OVERLAY_CLASS,
                ..Default::default()
            };
            // SAFETY: The class contains only static function/string pointers.
            if unsafe { RegisterClassExW(&class) } == 0 {
                return Err(error_from_windows(
                    "RegisterClassExW(region selection)",
                    windows::core::Error::from_win32(),
                ));
            }
            Ok(())
        })
        .clone()
}

unsafe extern "system" fn overlay_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: Windows specifies lParam as a valid CREATESTRUCTW here.
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        let context = create.lpCreateParams.cast::<OverlayContext>();
        if context.is_null() {
            return LRESULT(0);
        }
        // SAFETY: Box-backed context remains live through the modal loop.
        unsafe {
            (*context).hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, context as isize);
        }
        return LRESULT(1);
    }

    // SAFETY: We only store OverlayContext pointers in this class's user data.
    let context = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA)
            as *mut OverlayContext
    };
    if message == WM_NCDESTROY {
        if !context.is_null() {
            unsafe {
                (*context).hwnd = HWND::default();
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
        }
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    if context.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    // SAFETY: The modal Box outlives the native window.
    let context = unsafe { &mut *context };
    match message {
        WM_LBUTTONDOWN => {
            if let Some(point) = cursor_position() {
                context.state.pointer_down(point);
                // SAFETY: Capture is assigned to the active overlay HWND.
                unsafe { SetCapture(hwnd) };
                // SAFETY: Invalidates our own window without dereferencing pointers.
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if let Some(point) = cursor_position()
                && context.state.pointer_move(point) == SelectionUpdate::Changed
            {
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let update = cursor_position().map(|point| context.state.pointer_up(point));
            let _ = unsafe { ReleaseCapture() };
            if let Some(update) = update {
                match update {
                    SelectionUpdate::Selected(region) => {
                        context.selected = Some(region);
                        context.done = true;
                        let _ = unsafe { DestroyWindow(hwnd) };
                    }
                    SelectionUpdate::TooSmall => {
                        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if context.state.phase() == crate::SelectionPhase::Dragging {
                context.state.reset();
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
            cancel(context, hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            cancel(context, hwnd);
            LRESULT(0)
        }
        WM_PAINT => {
            paint(context, hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn cursor_position() -> Option<PointI> {
    let mut point = POINT::default();
    // SAFETY: point is a valid writable POINT.
    unsafe { GetCursorPos(&mut point) }
        .ok()
        .map(|()| PointI::new(point.x, point.y))
}

fn cancel(context: &mut OverlayContext, hwnd: HWND) {
    context.state.cancel();
    context.selected = None;
    context.done = true;
    let _ = unsafe { ReleaseCapture() };
    // SAFETY: hwnd is the live modal overlay.
    let _ = unsafe { DestroyWindow(hwnd) };
}

fn paint(context: &OverlayContext, hwnd: HWND) {
    let mut paint = PAINTSTRUCT::default();
    let mut client = RECT::default();
    // SAFETY: Standard balanced paint operations on the live overlay.
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    if unsafe { GetClientRect(hwnd, &mut client) }.is_ok() {
        // SAFETY: GDI objects are selected/deleted according to their contracts.
        let shade = unsafe { CreateSolidBrush(COLORREF(0)) };
        unsafe { FillRect(dc, &client, shade) };
        let _ = unsafe { DeleteObject(HGDIOBJ(shade.0)) };

        if let Some(region) = context.state.preview_region() {
            let desktop = context.state.desktop();
            let pen = unsafe { CreatePen(PS_SOLID, 2, COLORREF(0x00FF_FFFF)) };
            let old_pen = unsafe { SelectObject(dc, HGDIOBJ(pen.0)) };
            let null_brush = unsafe { GetStockObject(NULL_BRUSH) };
            let old_brush = unsafe { SelectObject(dc, null_brush) };
            let _ = unsafe {
                Rectangle(
                    dc,
                    region.x - desktop.x,
                    region.y - desktop.y,
                    region.right() - desktop.x,
                    region.bottom() - desktop.y,
                )
            };
            unsafe {
                SelectObject(dc, old_brush);
                SelectObject(dc, old_pen);
            }
            let _ = unsafe { DeleteObject(HGDIOBJ(pen.0)) };
        }
    }
    let _ = unsafe { EndPaint(hwnd, &paint) };
}

pub(super) fn create_destroy_self_test() -> Result<(), PlatformError> {
    let state = RegionSelectionState::new(virtual_desktop()?);
    let mut context = Box::new(OverlayContext {
        state,
        hwnd: HWND::default(),
        done: false,
        selected: None,
    });
    let hwnd = create_overlay_window(&mut context, 1)?;
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return Err(PlatformError::Win32 {
            operation: "IsWindow(region selection self-test)",
            code: 0,
            message: "new overlay HWND was not live".to_owned(),
        });
    }
    let _ = unsafe { DestroyWindow(hwnd) };
    if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return Err(PlatformError::Win32 {
            operation: "DestroyWindow(region selection self-test)",
            code: 0,
            message: "overlay HWND remained live after cleanup".to_owned(),
        });
    }
    Ok(())
}
