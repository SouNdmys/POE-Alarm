use std::collections::VecDeque;
use std::ffi::c_void;
use std::mem::{size_of, size_of_val};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use poe_alarm_alert_win::AlertServiceConfig;
use poe_alarm_core::{NumericConstraintMode, ResultGroupMode};
use poe_alarm_platform_win::{
    CaptureAffinity, HotKeyAction, HotKeyConfig, HotKeyManager, HotKeyTarget,
    HudPlacement as PlatformHudPlacement, HudWindow, HudWindowConfig, HudWindowPolicy,
    NativeWindowHandle, RectI, RegionSelectionOverlay, SelectionOverlayConfig, SizeI,
    StartMonitoringHotKey, ValidatedWave, resolve_hud_position,
};
use poe_alarm_runtime::{
    CompiledUiBindings, ProductionRuntimeConfig, RuntimeEvent, RuntimeGeneration, RuntimeHandle,
    RuntimeRequestId, RuntimeState, ScreenshotRequest,
};
use poe_alarm_settings::{
    AppSettings, ScreenRegion, SettingsError, SettingsStore, preview_settings_path,
    release_settings_path,
};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWM_WINDOW_CORNER_PREFERENCE, DWMSBT_MAINWINDOW,
    DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateRoundRectRgn,
    CreateSolidBrush, DEFAULT_CHARSET, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE,
    DT_VCENTER, DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint, FW_NORMAL, FW_SEMIBOLD, FillRect,
    FillRgn, GetMonitorInfoW, HBRUSH, HDC, HFONT, HGDIOBJ, InvalidateRect,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MapWindowPoints, MonitorFromRect, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, SelectObject, SetBkColor, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
    UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, EM_SETMARGINS, EM_SETSEL, ODS_DISABLED, ODS_FOCUS, ODS_HOTLIGHT, ODS_SELECTED,
    ODT_BUTTON, SetWindowTheme,
};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem,
    GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetKeyState, IsWindowEnabled, VK_A, VK_CONTROL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_OWNERDRAW, CB_ADDSTRING,
    CB_GETCURSEL, CB_RESETCONTENT, CB_SETCURSEL, CB_SETITEMHEIGHT, CBN_SELCHANGE, CBS_DROPDOWNLIST,
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, EC_LEFTMARGIN, EC_RIGHTMARGIN, EN_KILLFOCUS, ES_AUTOHSCROLL, ES_AUTOVSCROLL,
    ES_MULTILINE, ES_READONLY, ES_WANTRETURN, GWLP_USERDATA, GetClientRect, GetCursorPos,
    GetMessageW, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, HMENU,
    HTCAPTION, IDC_ARROW, IDYES, IsWindowVisible, KillTimer, LoadCursorW, LoadIconW,
    MB_ICONINFORMATION, MB_YESNO, MSG, MessageBoxW, MoveWindow, PostMessageW, PostQuitMessage,
    RegisterClassExW, SW_HIDE, SW_MINIMIZE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE as WinWindowStyle, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC,
    WM_DESTROY, WM_DPICHANGED, WM_DRAWITEM, WM_ERASEBKGND, WM_HOTKEY, WM_KEYDOWN, WM_NCCREATE,
    WM_NCDESTROY, WM_PAINT, WM_SETFONT, WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_CHILD,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, PWSTR, w};

use crate::theme::{ControlRole, ControlState, Rgb, SEA_GLASS_PALETTE, StatusTone};
use crate::{
    AppState, BackgroundCommand, BackgroundCompletion, ConditionEdit, EditorError, EditorForm,
    Game, GlobalEdit, GroupEdit, MonitorStatus, NumericConstraintEdit, OcrLanguage, Operation,
    RegionEdit, RuleMode, UiAction, UiLanguage, UiText, WorkFailure, WorkOutcome,
};

const CLASS_NAME: PCWSTR = w!("PoeAlarmNativeConfigWindow");
const FONT_SEGOE_TEXT: PCWSTR = w!("Segoe UI Variable Text");
const FONT_SEGOE_DISPLAY: PCWSTR = w!("Segoe UI Variable Display");
const FONT_YAHEI_UI: PCWSTR = w!("Microsoft YaHei UI");
const APP_ICON_RESOURCE_ID: usize = 101;
const CLIENT_WIDTH: i32 = 1080;
const CLIENT_HEIGHT: i32 = 900;
#[cfg(test)]
const RIGHTMOST_CONTROL_EDGE: i32 = 1050;
#[cfg(test)]
const BOTTOMMOST_CONTROL_EDGE: i32 = 892;
const WM_APP_WORK_COMPLETED: u32 = WM_APP + 1;
const WM_APP_SELF_TEST: u32 = WM_APP + 2;
const WM_APP_CLOSE_SAVE_COMPLETED: u32 = WM_APP + 3;
const RUNTIME_TIMER_ID: usize = 1;
const RUNTIME_TIMER_INTERVAL_MS: u32 = 16;
const CLOSE_DEADLINE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
struct UiGeometry {
    rules_bottom: i32,
    settings_top: i32,
    settings_bottom: i32,
    status_top: i32,
    status_bottom: i32,
    results_label_y: i32,
    results_top: i32,
    action_top: i32,
}

const fn ui_geometry(mode: RuleMode) -> UiGeometry {
    let settings_top = match mode {
        RuleMode::Quick => 322,
        RuleMode::MultipleAffixes => 508,
    };
    let settings_bottom = settings_top + 222;
    let status_top = settings_bottom + 10;
    let status_bottom = status_top + 34;
    let results_label_y = status_bottom + 8;
    UiGeometry {
        rules_bottom: match mode {
            RuleMode::Quick => 312,
            RuleMode::MultipleAffixes => 500,
        },
        settings_top,
        settings_bottom,
        status_top,
        status_bottom,
        results_label_y,
        results_top: results_label_y + 22,
        action_top: results_label_y,
    }
}

const fn shows_match_option_layer(group_count: usize) -> bool {
    group_count > 1
}

const ID_GAME: u16 = 101;
const ID_LANGUAGE: u16 = 102;
const ID_RULE_MODE: u16 = 103;
const ID_SAVE: u16 = 104;
const ID_QUICK_TEMPLATE: u16 = 110;
const ID_OCR_LANGUAGE: u16 = 111;
const ID_REGION_X: u16 = 112;
const ID_REGION_Y: u16 = 113;
const ID_REGION_WIDTH: u16 = 114;
const ID_REGION_HEIGHT: u16 = 115;
const ID_SELECT_REGION: u16 = 116;
const ID_RESULTS: u16 = 120;
const ID_ADD_RESULT: u16 = 121;
const ID_DELETE_RESULT: u16 = 122;
const ID_GROUP_NAME: u16 = 123;
const ID_GROUP_MODE: u16 = 124;
const ID_REQUIRED_COUNT: u16 = 125;
const ID_CONDITIONS: u16 = 130;
const ID_ADD_CONDITION: u16 = 131;
const ID_DELETE_CONDITION: u16 = 132;
const ID_CONDITION_NAME: u16 = 133;
const ID_CONDITION_TEMPLATE: u16 = 134;
const ID_NUMERIC_RULES: u16 = 140;
const ID_ADD_NUMERIC: u16 = 141;
const ID_DELETE_NUMERIC: u16 = 142;
const ID_NUMERIC_MODE: u16 = 143;
const ID_NUMERIC_FIRST: u16 = 144;
const ID_NUMERIC_SECOND: u16 = 145;
const ID_KEEP_HUD: u16 = 150;
const ID_ALLOW_OVERLAY_CAPTURE: u16 = 151;
const ID_HUD_MONITOR: u16 = 152;
const ID_HUD_X: u16 = 153;
const ID_HUD_Y: u16 = 154;
const ID_ALERT_SOUND: u16 = 155;
const ID_HOTKEY: u16 = 156;
const ID_BROWSE_SOUND: u16 = 157;
const ID_DEFAULT_SOUND: u16 = 158;
const ID_PLACE_HUD: u16 = 159;
const ID_START: u16 = 201;
const ID_STOP: u16 = 202;
const ID_SCREENSHOT: u16 = 203;
const ID_SCREENSHOT_RESULT: u16 = 204;

const APP_WINDOW_STYLE: WinWindowStyle =
    WinWindowStyle(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NonClientSize {
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowMetrics {
    layout_dpi: u32,
    client_width: i32,
    client_height: i32,
    outer_width: i32,
    outer_height: i32,
}

fn fit_window_metrics(
    physical_dpi: u32,
    work_width: i32,
    work_height: i32,
    non_client: NonClientSize,
) -> WindowMetrics {
    let available_width = (work_width - non_client.width).max(1);
    let available_height = (work_height - non_client.height).max(1);
    let width_fit = ((i64::from(available_width) * 96) / i64::from(CLIENT_WIDTH)) as u32;
    let height_fit = ((i64::from(available_height) * 96) / i64::from(CLIENT_HEIGHT)) as u32;
    let mut layout_dpi = physical_dpi
        .max(1)
        .min(width_fit.max(1))
        .min(height_fit.max(1));
    while layout_dpi > 1
        && (scale(CLIENT_WIDTH, layout_dpi) > available_width
            || scale(CLIENT_HEIGHT, layout_dpi) > available_height)
    {
        layout_dpi -= 1;
    }
    let client_width = scale(CLIENT_WIDTH, layout_dpi);
    let client_height = scale(CLIENT_HEIGHT, layout_dpi);
    WindowMetrics {
        layout_dpi,
        client_width,
        client_height,
        outer_width: client_width + non_client.width,
        outer_height: client_height + non_client.height,
    }
}

fn window_metrics(
    physical_dpi: u32,
    work_width: i32,
    work_height: i32,
) -> Result<WindowMetrics, String> {
    let mut rect = RECT::default();
    unsafe {
        AdjustWindowRectExForDpi(
            &mut rect,
            APP_WINDOW_STYLE,
            false,
            WINDOW_EX_STYLE(0),
            physical_dpi.max(1),
        )
    }
    .map_err(win_error)?;
    let non_client = NonClientSize {
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
    };
    Ok(fit_window_metrics(
        physical_dpi,
        work_width,
        work_height,
        non_client,
    ))
}

fn monitor_work_area(anchor: &RECT) -> Result<RECT, String> {
    let monitor = unsafe { MonitorFromRect(anchor, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Err(win_error(windows::core::Error::from_win32()));
    }
    Ok(info.rcWork)
}

fn clamp_window_origin(
    preferred_x: i32,
    preferred_y: i32,
    work: RECT,
    metrics: WindowMetrics,
) -> (i32, i32) {
    let maximum_x = (work.right - metrics.outer_width).max(work.left);
    let maximum_y = (work.bottom - metrics.outer_height).max(work.top);
    (
        preferred_x.clamp(work.left, maximum_x),
        preferred_y.clamp(work.top, maximum_y),
    )
}

pub fn run() -> Result<(), String> {
    run_window(false)
}

pub fn run_self_test() -> Result<(), String> {
    run_window(true)
}

fn run_window(self_test: bool) -> Result<(), String> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let module = unsafe { GetModuleHandleW(None) }.map_err(win_error)?;
    let instance = HINSTANCE(module.0);
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(win_error)?;
    let icon = unsafe { LoadIconW(Some(instance), PCWSTR(APP_ICON_RESOURCE_ID as *const u16)) }
        .map_err(|_| "could not load the embedded POE Alarm application icon".to_owned())?;
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hIcon: icon,
        hCursor: cursor,
        hIconSm: icon,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err("could not register the native configuration window".to_owned());
    }

    let primary_probe = RECT {
        left: 0,
        top: 0,
        right: 1,
        bottom: 1,
    };
    let work_area = monitor_work_area(&primary_probe)?;
    let physical_dpi = unsafe { GetDpiForSystem() }.max(96);
    let work_width = work_area.right - work_area.left;
    let work_height = work_area.bottom - work_area.top;
    let metrics = window_metrics(physical_dpi, work_width, work_height)?;
    let initial_x = work_area.left + (work_width - metrics.outer_width) / 2;
    let initial_y = work_area.top + (work_height - metrics.outer_height) / 2;
    let state = Box::new(WindowState::new(self_test)?);
    let state_pointer = Box::into_raw(state);
    let initial_title = wide(UiLanguage::SimplifiedChinese.text().window_title);
    let window_result = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR(initial_title.as_ptr()),
            APP_WINDOW_STYLE,
            initial_x,
            initial_y,
            metrics.outer_width,
            metrics.outer_height,
            None,
            None,
            Some(instance),
            Some(state_pointer.cast()),
        )
    };
    let window = match window_result {
        Ok(window) => window,
        Err(error) => {
            unsafe { drop(Box::from_raw(state_pointer)) };
            return Err(win_error(error));
        }
    };
    unsafe {
        if !self_test {
            let _ = ShowWindow(window, SW_SHOW);
        }
        let _ = UpdateWindow(window);
    }

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        match result.0 {
            -1 => return Err("the native message loop failed".to_owned()),
            0 => {
                if message.wParam.0 != 0 {
                    return Err(format!(
                        "native window self-test failed with code {}",
                        message.wParam.0
                    ));
                }
                break;
            }
            _ if unsafe { handle_edit_shortcut(window, &message) } => {}
            _ => unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            },
        }
    }
    Ok(())
}

unsafe fn handle_edit_shortcut(window: HWND, message: &MSG) -> bool {
    if message.message != WM_KEYDOWN
        || message.wParam.0 as u16 != VK_A.0
        || unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } >= 0
    {
        return false;
    }
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const WindowState;
    if state.is_null() {
        return false;
    }
    let controls = unsafe { &(*state).controls };
    if message.hwnd != controls.quick_template && message.hwnd != controls.condition_template {
        return false;
    }
    unsafe {
        SendMessageW(message.hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
    }
    true
}

struct WindowState {
    hwnd: HWND,
    model: AppState,
    settings_path: PathBuf,
    controls: Controls,
    theme_brushes: ThemeBrushes,
    fonts: Fonts,
    dpi: u32,
    layout_key: Option<(u32, RuleMode, bool)>,
    save_worker: Option<mpsc::Sender<SaveRequest>>,
    runtime: Box<dyn RuntimeClient>,
    runtime_alert_key: (Option<String>, bool),
    hot_keys: Option<HotKeyManager>,
    hud: Option<StatusHud>,
    hud_session: Option<HudSession>,
    last_hud_elapsed_second: Option<u64>,
    self_test: bool,
    screenshot_result: String,
    status_override: Option<String>,
    next_request_id: u64,
    closing: bool,
    close_deadline: Option<Instant>,
    runtime_shutdown_complete: bool,
    close_save_pending: bool,
    rebuilding_runtime: bool,
    rebuild_deadline: Option<Instant>,
    select_region_after_stop: bool,
    offer_legacy_import: bool,
    hud_placement_active: bool,
    exit_code: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SavePurpose {
    Normal,
    Closing,
}

struct SaveRequest {
    settings: Box<AppSettings>,
    purpose: SavePurpose,
}

struct HudSession {
    generation: RuntimeGeneration,
    settings: AppSettings,
    target_summary: String,
    started_at: Option<Instant>,
}

trait RuntimeClient {
    fn start(&mut self, settings: AppSettings) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    fn test_screenshot(&mut self, request: ScreenshotRequest) -> Result<(), String>;
    fn acknowledge_alert(&mut self) -> Result<(), String>;
    fn shutdown(&mut self) -> Result<(), String>;
    fn try_next_event(&mut self) -> Option<RuntimeEvent>;
}

struct ProductionRuntimeClient {
    handle: RuntimeHandle,
}

impl ProductionRuntimeClient {
    fn start(config: ProductionRuntimeConfig) -> Result<Self, String> {
        RuntimeHandle::start_production(config)
            .map(|handle| Self { handle })
            .map_err(|error| error.to_string())
    }
}

impl RuntimeClient for ProductionRuntimeClient {
    fn start(&mut self, settings: AppSettings) -> Result<(), String> {
        self.handle
            .start(settings)
            .map_err(|error| error.to_string())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.handle.stop().map_err(|error| error.to_string())
    }

    fn test_screenshot(&mut self, request: ScreenshotRequest) -> Result<(), String> {
        self.handle
            .test_screenshot(request)
            .map_err(|error| error.to_string())
    }

    fn acknowledge_alert(&mut self) -> Result<(), String> {
        self.handle
            .acknowledge_alert()
            .map_err(|error| error.to_string())
    }

    fn shutdown(&mut self) -> Result<(), String> {
        self.handle.shutdown().map_err(|error| error.to_string())
    }

    fn try_next_event(&mut self) -> Option<RuntimeEvent> {
        self.handle.try_next_event()
    }
}

struct FakeRuntimeClient {
    events: VecDeque<RuntimeEvent>,
    generation: u64,
}

impl FakeRuntimeClient {
    fn new() -> Self {
        let mut events = VecDeque::new();
        events.push_back(RuntimeEvent::Ready);
        events.push_back(RuntimeEvent::StateChanged {
            generation: None,
            state: RuntimeState::Idle,
        });
        Self {
            events,
            generation: 0,
        }
    }

    fn next_generation(&mut self) -> poe_alarm_runtime::RuntimeGeneration {
        self.generation = self.generation.saturating_add(1).max(1);
        poe_alarm_runtime::RuntimeGeneration(self.generation)
    }
}

impl RuntimeClient for FakeRuntimeClient {
    fn start(&mut self, _settings: AppSettings) -> Result<(), String> {
        let generation = self.next_generation();
        self.events.push_back(RuntimeEvent::StateChanged {
            generation: Some(generation),
            state: RuntimeState::Monitoring,
        });
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.events.push_back(RuntimeEvent::StateChanged {
            generation: None,
            state: RuntimeState::Idle,
        });
        Ok(())
    }

    fn test_screenshot(&mut self, request: ScreenshotRequest) -> Result<(), String> {
        let generation = self.next_generation();
        poe_alarm_runtime::compile_settings(&request.settings)
            .map_err(|error| error.to_string())?;
        self.events.push_back(RuntimeEvent::ScreenshotCompleted(
            poe_alarm_runtime::ScreenshotReport {
                request_id: request.request_id,
                generation,
                lines: vec!["+#% to Critical Strike Chance".to_owned()],
                modifier_count: 1,
                parse_elapsed: Duration::from_millis(1),
                evaluation_elapsed: Duration::from_millis(1),
                evaluation: poe_alarm_runtime::ScreenshotEvaluation {
                    is_match: true,
                    detail: Some("fake self-test match".to_owned()),
                    matched_group: Some("Critical result".to_owned()),
                },
            },
        ));
        self.events.push_back(RuntimeEvent::StateChanged {
            generation: None,
            state: RuntimeState::Idle,
        });
        Ok(())
    }

    fn acknowledge_alert(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), String> {
        self.events.push_back(RuntimeEvent::ShutdownComplete {
            elapsed: Duration::ZERO,
            within_one_second: true,
        });
        Ok(())
    }

    fn try_next_event(&mut self) -> Option<RuntimeEvent> {
        self.events.pop_front()
    }
}

fn resolve_alert_wave(settings: &AppSettings) -> Result<(ValidatedWave, bool), String> {
    if let Some(path) = settings.custom_alert_sound_path.as_deref() {
        match ValidatedWave::open(path) {
            Ok(wave) => return Ok((wave, false)),
            Err(error) => eprintln!("custom alert sound was rejected: {error}"),
        }
    }
    crate::alert_cue::built_in_alert_wave()
        .map(|wave| (wave, settings.custom_alert_sound_path.is_some()))
        .map_err(|error| format!("could not build the bundled alert sound: {error}"))
}

fn active_rule_summary(settings: &AppSettings, language: UiLanguage) -> String {
    let profile = settings.selected_profile();
    let rules_profile = profile.selected_rules();
    let summary = match rules_profile.rule_editor_mode {
        poe_alarm_settings::RuleEditorMode::Quick => rules_profile.target_affix.trim().to_owned(),
        poe_alarm_settings::RuleEditorMode::Structured => rules_profile
            .structured_rule_set
            .as_ref()
            .map(|rules| {
                let condition_count = rules
                    .groups
                    .iter()
                    .map(|group| group.conditions.len())
                    .sum();
                language.text().structured_rule_summary(
                    &rules.name,
                    rules.groups.len(),
                    condition_count,
                )
            })
            .unwrap_or_default(),
    };
    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.is_empty() {
        language.text().hud_empty_target().to_owned()
    } else {
        summary
    }
}

fn compiled_hud_settings(
    base: &AppSettings,
    ui: &CompiledUiBindings,
    region_x: i32,
    region_y: i32,
    region_width: u32,
    region_height: u32,
) -> AppSettings {
    let mut settings = base.clone();
    settings.keep_hud_visible = ui.keep_hud_visible;
    settings.hud_placement = ui.hud_placement.clone();
    settings.allow_overlay_capture = ui.allow_overlay_capture;
    if let (Ok(width), Ok(height)) = (i32::try_from(region_width), i32::try_from(region_height)) {
        settings
            .profiles
            .get_mut(settings.selected_game_profile)
            .capture_region = Some(ScreenRegion::new(region_x, region_y, width, height));
    }
    settings
}

fn format_hud_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

const fn hud_should_be_visible(status: MonitorStatus, keep_hud_visible: bool) -> bool {
    keep_hud_visible && !matches!(status, MonitorStatus::MatchFound)
}

const fn start_hot_key_requests_start(is_monitoring: bool, has_pending_operation: bool) -> bool {
    !is_monitoring && !has_pending_operation
}

const fn should_minimize_after_monitoring_started(self_test: bool) -> bool {
    !self_test
}

fn close_needs_save(
    future_schema: Option<u32>,
    was_dirty: bool,
    settings_before_commit: &AppSettings,
    settings_after_commit: &AppSettings,
) -> bool {
    future_schema.is_none() && (was_dirty || settings_before_commit != settings_after_commit)
}

const HUD_CONTENT_CLASS: PCWSTR = w!("PoeAlarmAppStatusHudContent");
const HUD_WIDTH: i32 = 320;
const HUD_HEIGHT: i32 = 68;
const HUD_PRIMARY_FONT_SIZE: i32 = 13;
const HUD_SECONDARY_FONT_SIZE: i32 = 12;

struct HudPaintState {
    primary_text: String,
    secondary_text: String,
    color: COLORREF,
    primary_font: HFONT,
    secondary_font: HFONT,
    font_language: UiLanguage,
    placement: bool,
}

struct StatusHud {
    shell: HudWindow,
    child: HWND,
    paint: Box<HudPaintState>,
    dpi: u32,
    bounds: RectI,
    capture_affinity: CaptureAffinity,
    visible: bool,
    headless: bool,
    placement: bool,
}

impl StatusHud {
    fn create(
        settings: &AppSettings,
        headless: bool,
        language: UiLanguage,
    ) -> Result<Self, String> {
        ensure_hud_content_class()?;
        let bounds = hud_bounds(settings, 96)?;
        let policy = HudWindowPolicy {
            capture_affinity: if settings.allow_overlay_capture {
                CaptureAffinity::Include
            } else {
                CaptureAffinity::Exclude
            },
            ..HudWindowPolicy::default()
        };
        let mut shell = HudWindow::create(HudWindowConfig {
            bounds,
            policy,
            visible: false,
        })
        .map_err(|error| error.to_string())?;
        let parent = HWND(shell.window_handle().as_raw() as *mut c_void);
        let dpi = unsafe { GetDpiForWindow(parent) }.max(96);
        let bounds = hud_bounds(settings, dpi)?;
        shell
            .set_bounds(bounds)
            .map_err(|error| error.to_string())?;
        let (primary_font, secondary_font) = unsafe { create_hud_fonts(dpi, language) };
        let mut paint = Box::new(HudPaintState {
            primary_text: String::new(),
            secondary_text: String::new(),
            color: theme_color(SEA_GLASS_PALETTE.info),
            primary_font,
            secondary_font,
            font_language: language,
            placement: false,
        });
        let child = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                HUD_CONTENT_CLASS,
                w!(""),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                bounds.width,
                bounds.height,
                Some(parent),
                None,
                None,
                Some((&mut *paint as *mut HudPaintState).cast()),
            )
        }
        .map_err(win_error)?;
        unsafe { apply_hud_shape(parent, bounds.width, bounds.height, dpi) };
        Ok(Self {
            shell,
            child,
            paint,
            dpi,
            bounds,
            capture_affinity: policy.capture_affinity,
            visible: false,
            headless,
            placement: false,
        })
    }

    fn resize_for_dpi(&mut self, dpi: u32, bounds: RectI) -> Result<(), String> {
        let dpi = dpi.max(96);
        let dpi_changed = self.dpi != dpi;
        if dpi_changed {
            let language = self.paint.font_language;
            unsafe { replace_hud_fonts(&mut self.paint, dpi, language) };
            self.dpi = dpi;
        }
        if !dpi_changed && self.bounds == bounds {
            return Ok(());
        }
        self.shell
            .set_bounds(bounds)
            .map_err(|error| error.to_string())?;
        unsafe {
            let _ = MoveWindow(self.child, 0, 0, bounds.width, bounds.height, true);
            let shell = HWND(self.shell.window_handle().as_raw() as *mut c_void);
            apply_hud_shape(shell, bounds.width, bounds.height, dpi);
        }
        self.bounds = bounds;
        Ok(())
    }

    fn sync_layout(&mut self, settings: &AppSettings) -> Result<(), String> {
        let shell = HWND(self.shell.window_handle().as_raw() as *mut c_void);
        let initial_dpi = unsafe { GetDpiForWindow(shell) }.max(96);
        self.resize_for_dpi(initial_dpi, hud_bounds(settings, initial_dpi)?)?;

        // Moving a saved HUD position to another monitor can change its physical DPI. A
        // second, bounded pass keeps the shell, child surface and font on one scale.
        let target_dpi = unsafe { GetDpiForWindow(shell) }.max(96);
        if target_dpi != initial_dpi {
            self.resize_for_dpi(target_dpi, hud_bounds(settings, target_dpi)?)?;
        }
        Ok(())
    }

    fn update(
        &mut self,
        status: MonitorStatus,
        language: UiLanguage,
        settings: &AppSettings,
        target_summary: &str,
        monitoring_elapsed: Option<Duration>,
    ) -> Result<(), String> {
        if self.placement {
            return Ok(());
        }
        let elapsed = monitoring_elapsed.map(format_hud_elapsed);
        let text = language
            .text()
            .hud_text(status, target_summary, elapsed.as_deref());
        let (primary_text, secondary_text) = text.split_once("\r\n").map_or_else(
            || (text.as_str(), ""),
            |(primary, secondary)| (primary, secondary),
        );
        let color = match status {
            MonitorStatus::MatchFound => theme_color(SEA_GLASS_PALETTE.danger),
            MonitorStatus::Monitoring => theme_color(SEA_GLASS_PALETTE.success),
            _ => theme_color(SEA_GLASS_PALETTE.info),
        };
        let font_changed = self.paint.font_language != language;
        if font_changed {
            unsafe { replace_hud_fonts(&mut self.paint, self.dpi, language) };
        }
        let paint_changed = self.paint.primary_text != primary_text
            || self.paint.secondary_text != secondary_text
            || self.paint.color != color
            || font_changed;
        self.paint.primary_text = primary_text.to_owned();
        self.paint.secondary_text = secondary_text.to_owned();
        self.paint.color = color;
        let affinity = if settings.allow_overlay_capture {
            CaptureAffinity::Include
        } else {
            CaptureAffinity::Exclude
        };
        if self.capture_affinity != affinity {
            self.shell
                .set_capture_affinity(affinity)
                .map_err(|error| error.to_string())?;
            self.capture_affinity = affinity;
        }
        self.sync_layout(settings)?;
        if paint_changed {
            unsafe {
                let _ = InvalidateRect(Some(self.child), None, false);
            }
        }
        let visible = hud_should_be_visible(status, settings.keep_hud_visible);
        let should_show = visible && !self.headless;
        if should_show != self.visible {
            if should_show {
                self.shell.show().map_err(|error| error.to_string())?;
            } else {
                self.shell.hide().map_err(|error| error.to_string())?;
            }
            self.visible = should_show;
        }
        Ok(())
    }

    fn begin_placement(&mut self, language: UiLanguage) -> Result<(), String> {
        self.shell
            .set_interaction_mode(poe_alarm_platform_win::HudInteractionMode::Placement)
            .map_err(|error| error.to_string())?;
        self.placement = true;
        self.paint.placement = true;
        if self.paint.font_language != language {
            unsafe { replace_hud_fonts(&mut self.paint, self.dpi, language) };
        }
        self.paint.color = theme_color(SEA_GLASS_PALETTE.info);
        self.paint.primary_text = language.text().hud_placement_instruction().to_owned();
        self.paint.secondary_text.clear();
        if !self.headless && !self.visible {
            self.shell.show().map_err(|error| error.to_string())?;
            self.visible = true;
        }
        unsafe {
            let _ = InvalidateRect(Some(self.child), None, false);
        }
        Ok(())
    }

    fn finish_placement(&mut self) -> Result<(f64, f64), String> {
        self.shell
            .set_interaction_mode(poe_alarm_platform_win::HudInteractionMode::Passive)
            .map_err(|error| error.to_string())?;
        self.placement = false;
        self.paint.placement = false;
        let shell = HWND(self.shell.window_handle().as_raw() as *mut c_void);
        let mut bounds = RECT::default();
        if unsafe { GetWindowRect(shell, &mut bounds) }.is_err() {
            return Err(win_error(windows::core::Error::from_win32()));
        }
        let dpi = unsafe { GetDpiForWindow(shell) }.max(96);
        let work = monitor_work_area(&bounds)?;
        let width = scale(HUD_WIDTH, dpi);
        let height = scale(HUD_HEIGHT, dpi);
        let x = bounds
            .left
            .clamp(work.left, (work.right - width).max(work.left));
        let y = bounds
            .top
            .clamp(work.top, (work.bottom - height).max(work.top));
        let scaled = RectI::new(x, y, width, height)
            .ok_or_else(|| "the status window size is invalid".to_owned())?;
        self.resize_for_dpi(dpi, scaled)?;
        if unsafe { GetWindowRect(shell, &mut bounds) }.is_err() {
            return Err(win_error(windows::core::Error::from_win32()));
        }
        let monitor = unsafe { MonitorFromRect(&bounds, MONITOR_DEFAULTTOPRIMARY) };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            return Err(win_error(windows::core::Error::from_win32()));
        }
        let available_width =
            (info.rcWork.right - info.rcWork.left - (bounds.right - bounds.left)).max(1);
        let available_height =
            (info.rcWork.bottom - info.rcWork.top - (bounds.bottom - bounds.top)).max(1);
        let x = f64::from(bounds.left - info.rcWork.left) / f64::from(available_width);
        let y = f64::from(bounds.top - info.rcWork.top) / f64::from(available_height);
        Ok((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
    }
}

impl Drop for StatusHud {
    fn drop(&mut self) {
        if !self.child.0.is_null() {
            unsafe {
                let _ = DestroyWindow(self.child);
            }
            self.child = HWND::default();
        }
        for font in [self.paint.primary_font, self.paint.secondary_font] {
            if !font.0.is_null() {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
            }
        }
        self.paint.primary_font = HFONT::default();
        self.paint.secondary_font = HFONT::default();
    }
}

fn ensure_hud_content_class() -> Result<(), String> {
    static CLASS: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    CLASS
        .get_or_init(|| {
            let module = unsafe { GetModuleHandleW(None) }.map_err(win_error)?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(hud_content_proc),
                hInstance: module.into(),
                lpszClassName: HUD_CONTENT_CLASS,
                ..Default::default()
            };
            if unsafe { RegisterClassExW(&class) } == 0 {
                return Err(win_error(windows::core::Error::from_win32()));
            }
            Ok(())
        })
        .clone()
}

fn hud_bounds(settings: &AppSettings, dpi: u32) -> Result<RectI, String> {
    let anchor = settings
        .selected_profile()
        .capture_region
        .and_then(|region| RectI::new(region.x, region.y, region.width, region.height));
    let native = anchor.map_or(
        RECT {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        },
        |region| RECT {
            left: region.x,
            top: region.y,
            right: region.right(),
            bottom: region.bottom(),
        },
    );
    let monitor = unsafe { MonitorFromRect(&native, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Err(win_error(windows::core::Error::from_win32()));
    }
    let work = RectI::new(
        info.rcWork.left,
        info.rcWork.top,
        info.rcWork.right - info.rcWork.left,
        info.rcWork.bottom - info.rcWork.top,
    )
    .ok_or_else(|| "the selected display has no usable work area".to_owned())?;
    let width = scale(HUD_WIDTH, dpi);
    let height = scale(HUD_HEIGHT, dpi);
    let size =
        SizeI::new(width, height).ok_or_else(|| "the status window size is invalid".to_owned())?;
    let placement = match (
        settings.hud_placement.relative_x,
        settings.hud_placement.relative_y,
    ) {
        (Some(x), Some(y)) => PlatformHudPlacement::manual(x, y).unwrap_or_default(),
        _ => PlatformHudPlacement::Automatic,
    };
    let position = resolve_hud_position(work, size, placement, anchor);
    RectI::new(position.x, position.y, width, height)
        .ok_or_else(|| "the status window position is invalid".to_owned())
}

unsafe fn apply_hud_shape(window: HWND, width: i32, height: i32, dpi: u32) {
    let diameter = scale(11, dpi).max(3) * 2;
    let region = unsafe { CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter) };
    if region.0.is_null() {
        return;
    }
    // SetWindowRgn owns the region after success; retain ownership only on failure.
    if unsafe { SetWindowRgn(window, Some(region), true) } == 0 {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
    }
}

unsafe extern "system" fn hud_content_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        return LRESULT(1);
    }
    let paint_state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const HudPaintState;
    match message {
        WM_PAINT if !paint_state.is_null() => {
            let state = unsafe { &*paint_state };
            let mut paint = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(hwnd, &mut paint) };
            let mut client = RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &mut client);
            }
            let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
            let canvas = Brush::new(theme_color(SEA_GLASS_PALETTE.canvas));
            let border = Brush::new(theme_color(SEA_GLASS_PALETTE.border));
            let surface = Brush::new(theme_color(SEA_GLASS_PALETTE.card_raised));
            let accent = Brush::new(state.color);
            unsafe {
                FillRect(dc, &client, canvas.0);
                draw_rounded_surface(dc, client, border.0, scale(11, dpi));
                let border_width = scale(1, dpi).max(1);
                draw_rounded_surface(
                    dc,
                    RECT {
                        left: client.left + border_width,
                        top: client.top + border_width,
                        right: client.right - border_width,
                        bottom: client.bottom - border_width,
                    },
                    surface.0,
                    (scale(11, dpi) - border_width).max(2),
                );
                draw_rounded_surface(
                    dc,
                    RECT {
                        left: scale(10, dpi),
                        top: scale(12, dpi),
                        right: scale(14, dpi),
                        bottom: client.bottom - scale(12, dpi),
                    },
                    accent.0,
                    scale(2, dpi).max(1),
                );
                if state.placement {
                    let mut placement_bounds = client;
                    placement_bounds.left += scale(22, dpi);
                    placement_bounds.top += scale(8, dpi);
                    placement_bounds.right -= scale(12, dpi);
                    placement_bounds.bottom -= scale(8, dpi);
                    draw_text(
                        dc,
                        state.primary_font,
                        theme_color(SEA_GLASS_PALETTE.text_primary),
                        &state.primary_text,
                        placement_bounds,
                        DT_CENTER | DT_WORDBREAK | DT_END_ELLIPSIS,
                    );
                } else {
                    draw_text(
                        dc,
                        state.primary_font,
                        state.color,
                        &state.primary_text,
                        rect(
                            scale(24, dpi),
                            scale(7, dpi),
                            client.right - scale(12, dpi),
                            scale(31, dpi),
                        ),
                        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                    );
                    draw_text(
                        dc,
                        state.secondary_font,
                        theme_color(SEA_GLASS_PALETTE.text_secondary),
                        &state.secondary_text,
                        rect(
                            scale(24, dpi),
                            scale(31, dpi),
                            client.right - scale(12, dpi),
                            client.bottom - scale(7, dpi),
                        ),
                        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                    );
                }
                let _ = EndPaint(hwnd, &paint);
            }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_NCHITTEST if !paint_state.is_null() => {
            if unsafe { (*paint_state).placement } {
                LRESULT(windows::Win32::UI::WindowsAndMessaging::HTCLIENT as isize)
            } else {
                LRESULT(windows::Win32::UI::WindowsAndMessaging::HTTRANSPARENT as isize)
            }
        }
        windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDOWN
            if !paint_state.is_null() && unsafe { (*paint_state).placement } =>
        {
            let mut point = POINT::default();
            if unsafe { GetCursorPos(&mut point) }.is_ok() {
                let parent = unsafe { windows::Win32::UI::WindowsAndMessaging::GetParent(hwnd) }
                    .unwrap_or_default();
                unsafe {
                    let _ = SendMessageW(
                        parent,
                        windows::Win32::UI::WindowsAndMessaging::WM_NCLBUTTONDOWN,
                        Some(WPARAM(HTCAPTION as usize)),
                        Some(LPARAM((point.x as u32 | ((point.y as u32) << 16)) as isize)),
                    );
                }
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCDESTROY => {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

impl WindowState {
    fn new(self_test: bool) -> Result<Self, String> {
        let (model, settings_path, offer_legacy_import) = if self_test {
            let mut settings = AppSettings::default();
            settings.profiles.poe1.capture_region = Some(ScreenRegion::new(0, 0, 640, 800));
            (
                AppState::from_settings(settings, None),
                std::env::temp_dir().join(format!(
                    "poe-alarm-native-ui-self-test-{}.json",
                    std::process::id()
                )),
                false,
            )
        } else {
            // This reads one small local JSON file before the native window exists. Capture,
            // recognition, persistence, and all other potentially slow work stay off the UI loop.
            let path = preview_settings_path().map_err(|error| error.to_string())?;
            let offer_import = !path.exists()
                && release_settings_path()
                    .map(|legacy| legacy.is_file())
                    .unwrap_or(false);
            let mut store = SettingsStore::new(path.clone());
            let settings = store.load();
            let future_schema = store.is_read_only().then(|| {
                store
                    .detected_schema_version()
                    .unwrap_or(poe_alarm_settings::CURRENT_SCHEMA_VERSION + 1)
            });
            (
                AppState::from_settings(settings, future_schema),
                path,
                offer_import,
            )
        };
        let runtime_alert_key = (
            model.settings().custom_alert_sound_path.clone(),
            model.settings().allow_overlay_capture,
        );
        let (runtime, sound_fell_back): (Box<dyn RuntimeClient>, bool) = if self_test {
            (Box::new(FakeRuntimeClient::new()), false)
        } else {
            let (wave, fell_back) = resolve_alert_wave(model.settings())?;
            let mut alert = AlertServiceConfig::new(wave);
            alert.allow_overlay_capture = model.settings().allow_overlay_capture;
            (
                Box::new(ProductionRuntimeClient::start(ProductionRuntimeConfig {
                    alert,
                })?),
                fell_back,
            )
        };
        let status_override =
            sound_fell_back.then(|| model.language().text().sound_fallback().to_owned());
        Ok(Self {
            hwnd: HWND::default(),
            model,
            settings_path,
            controls: Controls::default(),
            theme_brushes: ThemeBrushes::new(),
            fonts: Fonts::default(),
            dpi: 96,
            layout_key: None,
            save_worker: None,
            runtime,
            runtime_alert_key,
            hot_keys: None,
            hud: None,
            hud_session: None,
            last_hud_elapsed_second: None,
            self_test,
            screenshot_result: String::new(),
            status_override,
            next_request_id: 1,
            closing: false,
            close_deadline: None,
            runtime_shutdown_complete: false,
            close_save_pending: false,
            rebuilding_runtime: false,
            rebuild_deadline: None,
            select_region_after_stop: false,
            offer_legacy_import,
            hud_placement_active: false,
            exit_code: 0,
        })
    }

    unsafe fn fit_window_to_monitor(
        &mut self,
        physical_dpi: u32,
        monitor_anchor: &RECT,
        preferred_x: i32,
        preferred_y: i32,
    ) -> Result<(), String> {
        let work = monitor_work_area(monitor_anchor)?;
        let metrics = window_metrics(physical_dpi, work.right - work.left, work.bottom - work.top)?;
        let (x, y) = clamp_window_origin(preferred_x, preferred_y, work, metrics);
        self.dpi = metrics.layout_dpi;
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                metrics.outer_width,
                metrics.outer_height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
        .map_err(win_error)
    }

    unsafe fn initialize(&mut self) -> Result<(), String> {
        let physical_dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96);
        let mut window_bounds = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut window_bounds) }.map_err(win_error)?;
        unsafe {
            self.fit_window_to_monitor(
                physical_dpi,
                &window_bounds,
                window_bounds.left,
                window_bounds.top,
            )
        }?;
        unsafe { apply_native_window_theme(self.hwnd) };
        self.controls = unsafe { Controls::create(self.hwnd) }?;
        unsafe { self.controls.apply_visual_theme() };
        self.fonts = unsafe { Fonts::create(self.dpi, self.model.language()) };
        match StatusHud::create(self.model.settings(), self.self_test, self.model.language()) {
            Ok(hud) => self.hud = Some(hud),
            Err(error) => {
                eprintln!("status window creation failed: {error}");
                self.status_override =
                    Some(self.model.language().text().runtime_failed().to_owned());
            }
        }
        unsafe {
            self.apply_fonts();
            self.controls.apply_control_metrics(self.dpi);
            self.layout();
            self.refresh_all();
        }
        self.save_worker = Some(start_save_worker(self.hwnd, self.settings_path.clone())?);
        unsafe {
            let _ = SetTimer(
                Some(self.hwnd),
                RUNTIME_TIMER_ID,
                RUNTIME_TIMER_INTERVAL_MS,
                None,
            );
        }
        self.register_hot_keys();
        if self.offer_legacy_import {
            self.offer_legacy_import = false;
            unsafe { self.prompt_legacy_import() };
        }
        if self.self_test {
            unsafe { PostMessageW(Some(self.hwnd), WM_APP_SELF_TEST, WPARAM(0), LPARAM(0)) }
                .map_err(win_error)?;
        }
        Ok(())
    }

    unsafe fn prompt_legacy_import(&mut self) {
        let text = self.model.language().text();
        let (message, title) = text.import_prompt();
        let message = wide(message);
        let title = wide(title);
        if unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(message.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_YESNO | MB_ICONINFORMATION,
            )
        } != IDYES
        {
            return;
        }
        let legacy = match release_settings_path() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("could not resolve previous settings path: {error}");
                return;
            }
        };
        match import_legacy_settings(&legacy, &self.settings_path) {
            Ok(Some(settings)) => {
                self.model = AppState::from_settings(settings, None);
                self.status_override = None;
                self.reconfigure_start_hot_key();
                self.request_runtime_rebuild_if_needed();
                unsafe { self.refresh_all() };
            }
            Ok(None) => {
                self.status_override = Some(text.import_rejected().to_owned());
                unsafe { self.refresh_all() };
            }
            Err(error) => {
                eprintln!("previous settings import failed: {error}");
                self.status_override = Some(text.import_rejected().to_owned());
                unsafe { self.refresh_all() };
            }
        }
    }

    fn register_hot_keys(&mut self) {
        if self.self_test {
            return;
        }
        let target = unsafe { NativeWindowHandle::from_raw(self.hwnd.0 as isize) };
        let Ok(target) = target else {
            self.status_override = Some(self.model.language().text().runtime_failed().to_owned());
            return;
        };
        let config = HotKeyConfig {
            start: StartMonitoringHotKey::parse_or_default(Some(
                &self.model.settings().start_monitoring_hot_key,
            )),
        };
        let mut manager = HotKeyManager::unregistered(HotKeyTarget::Window(target), config);
        for action in [
            HotKeyAction::StartMonitoring,
            HotKeyAction::SelectRegion,
            HotKeyAction::StopOrAcknowledge,
        ] {
            if let Err(error) = manager.register(action) {
                eprintln!("global shortcut registration failed: {error}");
                self.status_override =
                    Some(self.model.language().text().hot_key_conflict().to_owned());
            }
        }
        self.hot_keys = Some(manager);
    }

    unsafe fn pump_runtime_events(&mut self) {
        let mut processed = 0usize;
        while processed < 128 {
            let Some(event) = self.runtime.try_next_event() else {
                break;
            };
            processed += 1;
            if self.closing {
                if matches!(event, RuntimeEvent::ShutdownComplete { .. }) {
                    self.runtime_shutdown_complete = true;
                    if unsafe { self.finish_close_if_ready() } {
                        return;
                    }
                }
                continue;
            }
            unsafe { self.handle_runtime_event(event) };
        }

        if self.model.status() == MonitorStatus::Monitoring {
            let elapsed_second = self
                .hud_session
                .as_ref()
                .and_then(|session| session.started_at)
                .map(|started| started.elapsed().as_secs());
            if elapsed_second != self.last_hud_elapsed_second {
                unsafe { self.refresh_hud() };
            }
        }

        if self.closing
            && self
                .close_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            eprintln!("runtime did not confirm shutdown within one second; closing fail-open");
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
        if self.rebuilding_runtime
            && self
                .rebuild_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            eprintln!("old runtime did not confirm reconfiguration shutdown within one second");
            self.finish_runtime_rebuild();
            unsafe { self.refresh_all() };
        }
    }

    unsafe fn handle_runtime_event(&mut self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::Ready => {}
            RuntimeEvent::StateChanged { generation, state } => match state {
                RuntimeState::Monitoring => {
                    self.status_override = None;
                    if let Some(session) = self
                        .hud_session
                        .as_mut()
                        .filter(|session| Some(session.generation) == generation)
                    {
                        session.started_at.get_or_insert_with(Instant::now);
                    }
                    self.model.apply(UiAction::BackgroundCompleted(
                        BackgroundCompletion::succeeded(Operation::StartMonitoring),
                    ));
                    if should_minimize_after_monitoring_started(self.self_test) {
                        unsafe {
                            let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
                        }
                    }
                    unsafe { self.advance_self_test(Operation::StartMonitoring) };
                }
                RuntimeState::Idle => {
                    if self.model.pending_operation() == Some(Operation::StopMonitoring) {
                        self.model.apply(UiAction::BackgroundCompleted(
                            BackgroundCompletion::succeeded(Operation::StopMonitoring),
                        ));
                        unsafe { self.advance_self_test(Operation::StopMonitoring) };
                    }
                    if self.select_region_after_stop && !self.model.is_monitoring() {
                        self.select_region_after_stop = false;
                        unsafe { self.select_region() };
                    }
                    self.hud_session = None;
                }
                RuntimeState::MatchFound => self.model.report_match_found(),
                RuntimeState::Faulted => {
                    self.hud_session = None;
                    self.model.report_runtime_failure();
                }
                RuntimeState::Starting
                | RuntimeState::TestingScreenshot
                | RuntimeState::ShuttingDown
                | RuntimeState::Stopped => {}
            },
            RuntimeEvent::MatchFound { detection, .. } => {
                self.model.report_match_found();
                self.screenshot_result = detection.lines.join("\r\n");
            }
            RuntimeEvent::SettingsCompiled {
                generation,
                region,
                ui,
                ..
            } => {
                let settings = compiled_hud_settings(
                    self.model.settings(),
                    &ui,
                    region.x,
                    region.y,
                    region.width,
                    region.height,
                );
                self.hud_session = Some(HudSession {
                    generation,
                    target_summary: active_rule_summary(&settings, self.model.language()),
                    settings,
                    started_at: None,
                });
            }
            RuntimeEvent::MonitorSnapshot {
                generation,
                snapshot,
            } => {
                if self
                    .hud_session
                    .as_ref()
                    .is_none_or(|session| session.generation != generation)
                {
                    return;
                }
                match RuntimeEvent::monitor_state(&snapshot) {
                    RuntimeState::Monitoring => {
                        if let Some(session) = self.hud_session.as_mut() {
                            session.started_at.get_or_insert_with(Instant::now);
                        }
                        // Live snapshots are coalesced status data, not UI model changes. A full
                        // form refresh here used to rewrite every control and repeatedly Show/Move
                        // the HUD at the 16 ms runtime pump rate, making both windows visibly flash.
                        // The pump below refreshes only the HUD text when its displayed second
                        // actually changes.
                        return;
                    }
                    RuntimeState::MatchFound => self.model.report_match_found(),
                    RuntimeState::Faulted => {
                        self.hud_session = None;
                        self.model.report_runtime_failure();
                    }
                    RuntimeState::Idle
                    | RuntimeState::Starting
                    | RuntimeState::TestingScreenshot
                    | RuntimeState::ShuttingDown
                    | RuntimeState::Stopped => {}
                }
            }
            RuntimeEvent::ScreenshotCompleted(report) => {
                self.screenshot_result = self.model.language().text().screenshot_report(
                    report.evaluation.is_match,
                    &report.lines,
                    report.modifier_count,
                    report.parse_elapsed.as_secs_f64() * 1_000.0,
                    report.evaluation_elapsed.as_secs_f64() * 1_000.0,
                );
                self.model.apply(UiAction::BackgroundCompleted(
                    BackgroundCompletion::succeeded(Operation::TestScreenshot),
                ));
                unsafe { self.advance_self_test(Operation::TestScreenshot) };
            }
            RuntimeEvent::ScreenshotCancelled { .. } => {
                if self.model.pending_operation() == Some(Operation::TestScreenshot) {
                    self.model
                        .apply(UiAction::BackgroundCompleted(BackgroundCompletion {
                            operation: Operation::TestScreenshot,
                            outcome: WorkOutcome::Failed(WorkFailure::BackgroundStopped),
                        }));
                }
            }
            RuntimeEvent::AlertAcknowledged { .. } => {
                if self.model.is_monitoring() && self.model.pending_operation().is_none() {
                    unsafe { self.apply_action(UiAction::StopMonitoring) };
                }
            }
            RuntimeEvent::AlertSoundFailed { detail, .. } => {
                eprintln!("alert sound playback failed: {detail}");
                self.status_override = Some(
                    self.model
                        .language()
                        .text()
                        .sound_playback_failed()
                        .to_owned(),
                );
            }
            RuntimeEvent::Fault {
                operation, detail, ..
            } => {
                eprintln!("runtime {operation:?} failed: {detail}");
                self.hud_session = None;
                self.model.report_runtime_failure();
                self.status_override =
                    Some(self.model.language().text().runtime_failed().to_owned());
            }
            RuntimeEvent::ShutdownComplete { .. } if self.rebuilding_runtime => {
                self.finish_runtime_rebuild();
            }
            RuntimeEvent::ShutdownComplete { .. } | RuntimeEvent::AlertPresented { .. } => {}
        }
        unsafe { self.refresh_all() };
    }

    fn request_runtime_rebuild_if_needed(&mut self) {
        if self.self_test || self.closing || self.rebuilding_runtime {
            return;
        }
        let next_key = (
            self.model.settings().custom_alert_sound_path.clone(),
            self.model.settings().allow_overlay_capture,
        );
        if next_key == self.runtime_alert_key {
            return;
        }
        self.rebuilding_runtime = true;
        self.rebuild_deadline = Some(Instant::now() + CLOSE_DEADLINE);
        self.status_override = Some(
            self.model
                .language()
                .text()
                .applying_alert_settings()
                .to_owned(),
        );
        if let Err(error) = self.runtime.shutdown() {
            eprintln!("runtime reconfiguration shutdown request failed: {error}");
            self.finish_runtime_rebuild();
        }
    }

    fn finish_runtime_rebuild(&mut self) {
        if !self.rebuilding_runtime {
            return;
        }
        self.rebuilding_runtime = false;
        self.rebuild_deadline = None;
        let settings = self.model.settings().clone();
        let (wave, fell_back) = match resolve_alert_wave(&settings) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("runtime alert sound reconfiguration failed: {error}");
                self.model.report_runtime_failure();
                self.status_override =
                    Some(self.model.language().text().runtime_failed().to_owned());
                return;
            }
        };
        let mut alert = AlertServiceConfig::new(wave);
        alert.allow_overlay_capture = settings.allow_overlay_capture;
        match ProductionRuntimeClient::start(ProductionRuntimeConfig { alert }) {
            Ok(runtime) => {
                self.runtime = Box::new(runtime);
                self.runtime_alert_key = (
                    settings.custom_alert_sound_path.clone(),
                    settings.allow_overlay_capture,
                );
                self.status_override =
                    fell_back.then(|| self.model.language().text().sound_fallback().to_owned());
            }
            Err(error) => {
                eprintln!("replacement runtime failed to start: {error}");
                self.model.report_runtime_failure();
                self.status_override =
                    Some(self.model.language().text().runtime_failed().to_owned());
            }
        }
    }

    fn reconfigure_start_hot_key(&mut self) {
        let Some(manager) = self.hot_keys.as_mut() else {
            return;
        };
        let start = StartMonitoringHotKey::parse_or_default(Some(
            &self.model.settings().start_monitoring_hot_key,
        ));
        if let Err(error) = manager.reconfigure_start(start) {
            eprintln!("global start shortcut reconfiguration failed: {error}");
            self.status_override = Some(self.model.language().text().hot_key_conflict().to_owned());
        }
    }

    unsafe fn begin_close(&mut self) {
        if self.closing {
            return;
        }
        let settings_before_commit = self.model.settings().clone();
        let was_dirty = self.model.is_dirty();
        if self.model.future_schema().is_none() {
            let _ = unsafe { self.commit_visible_form() };
        }
        let needs_save = close_needs_save(
            self.model.future_schema(),
            was_dirty,
            &settings_before_commit,
            self.model.settings(),
        );
        self.closing = true;
        self.close_deadline = Some(Instant::now() + CLOSE_DEADLINE);
        self.runtime_shutdown_complete = false;
        self.close_save_pending = false;
        self.hot_keys.take();
        self.status_override = Some(self.model.language().text().closing().to_owned());
        unsafe { self.refresh_all() };
        if needs_save {
            self.close_save_pending = self.save_worker.as_ref().is_some_and(|worker| {
                worker
                    .send(SaveRequest {
                        settings: Box::new(self.model.settings().clone().normalize()),
                        purpose: SavePurpose::Closing,
                    })
                    .is_ok()
            });
            if !self.close_save_pending {
                eprintln!("dirty settings could not be queued while closing");
            }
        }
        if let Err(error) = self.runtime.shutdown() {
            eprintln!("runtime shutdown request failed: {error}");
            self.runtime_shutdown_complete = true;
            let _ = unsafe { self.finish_close_if_ready() };
        }
    }

    unsafe fn finish_close_if_ready(&mut self) -> bool {
        if self.closing && self.runtime_shutdown_complete && !self.close_save_pending {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
            true
        } else {
            false
        }
    }

    unsafe fn on_hot_key(&mut self, wparam: WPARAM) {
        let action = self
            .hot_keys
            .as_ref()
            .and_then(|manager| manager.action_for_message(WM_HOTKEY, wparam.0));
        match action {
            Some(HotKeyAction::StartMonitoring) => {
                if self.hud_placement_active {
                    unsafe { self.toggle_hud_placement() };
                    return;
                }
                if start_hot_key_requests_start(
                    self.model.is_monitoring(),
                    self.model.pending_operation().is_some(),
                ) && unsafe { self.commit_visible_form() }
                {
                    unsafe { self.apply_action(UiAction::StartMonitoring) };
                }
            }
            Some(HotKeyAction::SelectRegion) => {
                if self.hud_placement_active {
                    unsafe { self.toggle_hud_placement() };
                    return;
                }
                if self.model.is_monitoring() {
                    self.select_region_after_stop = true;
                    unsafe { self.apply_action(UiAction::StopMonitoring) };
                } else if self.model.pending_operation().is_none() {
                    unsafe { self.select_region() };
                }
            }
            Some(HotKeyAction::StopOrAcknowledge) => {
                if self.model.is_monitoring() {
                    unsafe { self.apply_action(UiAction::StopMonitoring) };
                } else if let Err(error) = self.runtime.acknowledge_alert() {
                    eprintln!("alert acknowledgement failed: {error}");
                }
            }
            None => {}
        }
    }

    unsafe fn select_region(&mut self) {
        if self.model.future_schema().is_some() || self.model.pending_operation().is_some() {
            return;
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        let selection = RegionSelectionOverlay::select(SelectionOverlayConfig::default());
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
        match selection {
            Ok(Some(region)) => {
                let selected = ScreenRegion::new(region.x, region.y, region.width, region.height);
                if let Err(error) = self.model.set_capture_region(selected) {
                    unsafe { self.show_editor_error(error, true) };
                    return;
                }
                unsafe { self.refresh_all() };
                if let Some(command) = self.model.begin_automatic_save() {
                    unsafe { self.dispatch_background_command(command) };
                }
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("region selection failed: {error}");
                self.status_override =
                    Some(self.model.language().text().runtime_failed().to_owned());
                unsafe { self.refresh_all() };
            }
        }
    }

    unsafe fn toggle_hud_placement(&mut self) {
        if self.model.is_monitoring()
            || self.model.pending_operation().is_some()
            || self.model.future_schema().is_some()
        {
            return;
        }
        let Some(hud) = self.hud.as_mut() else {
            return;
        };
        if !self.hud_placement_active {
            if let Err(error) = hud.begin_placement(self.model.language()) {
                eprintln!("could not enter status-window placement mode: {error}");
                return;
            }
            self.hud_placement_active = true;
            unsafe {
                set_text(
                    self.controls.place_hud,
                    self.model.language().text().save_hud_position_button,
                );
            }
            return;
        }
        match hud.finish_placement() {
            Ok((x, y)) => {
                self.hud_placement_active = false;
                if let Err(error) = self.model.set_hud_position(x, y) {
                    unsafe { self.show_editor_error(error, true) };
                } else {
                    unsafe { self.refresh_all() };
                    unsafe { self.apply_action(UiAction::SaveSettings) };
                }
            }
            Err(error) => eprintln!("could not save status-window placement: {error}"),
        }
    }

    unsafe fn apply_action(&mut self, action: UiAction) {
        let command = self.model.apply(action);
        unsafe { self.refresh_all() };
        if let Some(command) = command {
            unsafe { self.dispatch_background_command(command) };
        }
    }

    unsafe fn dispatch_background_command(&mut self, command: BackgroundCommand) {
        let operation = command.operation();
        let result = match command {
            BackgroundCommand::StartMonitoring(config) => self.runtime.start(config.settings),
            BackgroundCommand::StopMonitoring => self.runtime.stop(),
            BackgroundCommand::TestScreenshot { config, path } => {
                let request_id = RuntimeRequestId(self.next_request_id.max(1));
                self.next_request_id = self.next_request_id.saturating_add(1).max(1);
                // The file now holds saved item text rather than a screenshot.
                // Reading it here keeps the runtime free of the filesystem.
                std::fs::read_to_string(&path)
                    .map_err(|error| format!("could not read {}: {error}", path.display()))
                    .and_then(|text| {
                        self.runtime.test_screenshot(ScreenshotRequest::new(
                            request_id,
                            config.settings,
                            text,
                        ))
                    })
            }
            BackgroundCommand::SaveSettings(settings) => self
                .save_worker
                .as_ref()
                .ok_or_else(|| "the settings worker is unavailable".to_owned())
                .and_then(|worker| {
                    worker
                        .send(SaveRequest {
                            settings,
                            purpose: SavePurpose::Normal,
                        })
                        .map_err(|_| "the settings worker stopped".to_owned())
                }),
        };
        if result.is_err() {
            self.model
                .apply(UiAction::BackgroundCompleted(BackgroundCompletion {
                    operation,
                    outcome: WorkOutcome::Failed(WorkFailure::BackgroundStopped),
                }));
            unsafe { self.refresh_all() };
        }
    }

    unsafe fn refresh_all(&mut self) {
        unsafe {
            if self.fonts.language != self.model.language() {
                self.recreate_fonts();
            }
            let option_layer = self.shows_match_option_layer();
            if self.layout_key != Some((self.dpi, self.model.rule_mode(), option_layer)) {
                self.layout();
            }
            self.refresh_localized_controls();
            self.refresh_editor_lists();
            self.refresh_editor_fields();
            self.refresh_visibility();
            self.refresh_enabled_state();
            self.refresh_hud();
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn shows_match_option_layer(&self) -> bool {
        shows_match_option_layer(
            self.model
                .current_rule_set()
                .map_or(0, |rules| rules.groups.len()),
        )
    }

    unsafe fn refresh_hud(&mut self) {
        let (settings, target_summary, elapsed) = self.hud_session.as_ref().map_or_else(
            || {
                (
                    self.model.settings().clone(),
                    active_rule_summary(self.model.settings(), self.model.language()),
                    None,
                )
            },
            |session| {
                (
                    session.settings.clone(),
                    session.target_summary.clone(),
                    session.started_at.map(|started| started.elapsed()),
                )
            },
        );
        self.last_hud_elapsed_second = elapsed.map(|duration| duration.as_secs());
        let Some(hud) = self.hud.as_mut() else {
            return;
        };
        if let Err(error) = hud.update(
            self.model.status(),
            self.model.language(),
            &settings,
            &target_summary,
            elapsed,
        ) {
            eprintln!("status window update failed: {error}");
        }
    }

    unsafe fn refresh_localized_controls(&self) {
        let text = self.model.language().text();
        unsafe {
            set_text(self.hwnd, text.window_title);
            set_text(self.controls.save, text.save_button);
            set_text(self.controls.add_result, text.add_result);
            set_text(self.controls.delete_result, text.delete_result);
            set_text(self.controls.add_condition, text.add_condition);
            set_text(self.controls.delete_condition, text.delete_condition);
            set_text(self.controls.add_numeric, text.add_numeric_rule);
            set_text(self.controls.delete_numeric, text.delete_numeric_rule);
            set_text(self.controls.select_region, text.select_region_button);
            set_text(self.controls.keep_hud, text.keep_hud_visible);
            set_text(
                self.controls.allow_overlay_capture,
                text.allow_overlay_capture,
            );
            set_text(self.controls.start, text.start_button);
            set_text(self.controls.stop, text.stop_button);
            set_text(self.controls.screenshot, text.screenshot_button);
            set_text(self.controls.browse_sound, text.browse_sound_button);
            set_text(self.controls.default_sound, text.default_sound_button);
            set_text(
                self.controls.place_hud,
                if self.hud_placement_active {
                    text.save_hud_position_button
                } else {
                    text.place_hud_button
                },
            );
            set_text(self.controls.screenshot_result, &self.screenshot_result);
            set_combo_items(
                self.controls.game,
                &[text.game_poe1, text.game_poe2],
                match self.model.game() {
                    Game::Poe1 => 0,
                    Game::Poe2 => 1,
                },
            );
            set_combo_items(
                self.controls.language,
                &[text.language_zh, text.language_en],
                match self.model.language() {
                    UiLanguage::SimplifiedChinese => 0,
                    UiLanguage::English => 1,
                },
            );
            set_combo_items(
                self.controls.rule_mode,
                &[text.mode_quick, text.mode_multiple],
                match self.model.rule_mode() {
                    RuleMode::Quick => 0,
                    RuleMode::MultipleAffixes => 1,
                },
            );
            set_combo_items(
                self.controls.ocr_language,
                &[text.ocr_en, text.ocr_zh],
                match self.model.ocr_language() {
                    OcrLanguage::English => 0,
                    OcrLanguage::TraditionalChinese => 1,
                },
            );
            set_combo_items(
                self.controls.group_mode,
                &[text.group_any, text.group_all, text.group_at_least],
                match self.model.current_group().map(|group| group.mode) {
                    Some(ResultGroupMode::All) => 1,
                    Some(ResultGroupMode::AtLeast) => 2,
                    _ => 0,
                },
            );
            set_combo_items(
                self.controls.numeric_mode,
                &[
                    text.numeric_ignore,
                    text.numeric_range,
                    text.numeric_at_least,
                    text.numeric_at_most,
                    text.numeric_exact,
                ],
                match self
                    .model
                    .current_numeric_constraint()
                    .map(|constraint| constraint.mode)
                {
                    Some(NumericConstraintMode::RangeInclusive) => 1,
                    Some(NumericConstraintMode::AtLeast) => 2,
                    Some(NumericConstraintMode::AtMost) => 3,
                    Some(NumericConstraintMode::Exactly) => 4,
                    _ => 0,
                },
            );
            set_combo_items(
                self.controls.hotkey,
                &["Ctrl+Shift+F10", "Ctrl+Alt+F10", "Alt+F10"],
                hotkey_index(&self.model.settings().start_monitoring_hot_key),
            );
        }
    }

    unsafe fn refresh_editor_lists(&self) {
        let text = self.model.language().text();
        let results = self
            .model
            .current_rule_set()
            .map(|rules| {
                rules
                    .groups
                    .iter()
                    .enumerate()
                    .map(|(index, group)| text.result_item(index, &group.name))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let conditions = self
            .model
            .current_group()
            .map(|group| {
                group
                    .conditions
                    .iter()
                    .enumerate()
                    .map(|(index, condition)| text.condition_item(index, &condition.name))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let numeric_rules = self
            .model
            .current_condition()
            .map(|condition| {
                (0..condition.numeric_constraints.len())
                    .map(|index| text.numeric_item(index))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        unsafe {
            set_combo_strings(
                self.controls.results,
                &results,
                self.model.selected_group_index(),
            );
            set_combo_strings(
                self.controls.conditions,
                &conditions,
                self.model.selected_condition_index(),
            );
            set_combo_strings(
                self.controls.numeric_rules,
                &numeric_rules,
                self.model.selected_numeric_constraint_index(),
            );
        }
    }

    unsafe fn refresh_editor_fields(&self) {
        let profile = self.model.current_profile();
        let rules_profile = profile.selected_rules();
        unsafe {
            set_text(self.controls.quick_template, &rules_profile.target_affix);
            set_region_fields(&self.controls, profile.capture_region);
            set_check(
                self.controls.keep_hud,
                self.model.settings().keep_hud_visible,
            );
            set_check(
                self.controls.allow_overlay_capture,
                self.model.settings().allow_overlay_capture,
            );
            let hud = &self.model.settings().hud_placement;
            set_text(
                self.controls.hud_monitor,
                hud.monitor_device_name.as_deref().unwrap_or_default(),
            );
            set_text(
                self.controls.hud_x,
                &hud.relative_x
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            );
            set_text(
                self.controls.hud_y,
                &hud.relative_y
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            );
            set_text(
                self.controls.alert_sound,
                self.model
                    .settings()
                    .custom_alert_sound_path
                    .as_deref()
                    .unwrap_or_default(),
            );
            if let Some(group) = self.model.current_group() {
                set_text(self.controls.group_name, &group.name);
                set_text(
                    self.controls.required_count,
                    &group.required_count.to_string(),
                );
            } else {
                set_text(self.controls.group_name, "");
                set_text(self.controls.required_count, "1");
            }
            if let Some(condition) = self.model.current_condition() {
                set_text(self.controls.condition_name, &condition.name);
                set_text(self.controls.condition_template, &condition.template);
            } else {
                set_text(self.controls.condition_name, "");
                set_text(self.controls.condition_template, "");
            }
            let (first, second) = numeric_values(self.model.current_numeric_constraint());
            set_text(self.controls.numeric_first, &first);
            set_text(self.controls.numeric_second, &second);
        }
    }

    unsafe fn refresh_visibility(&self) {
        let quick = self.model.rule_mode() == RuleMode::Quick;
        for control in self.controls.quick_only() {
            unsafe {
                let _ = ShowWindow(control, if quick { SW_SHOW } else { SW_HIDE });
            }
        }
        for control in self.controls.structured_only() {
            unsafe {
                let _ = ShowWindow(control, if quick { SW_HIDE } else { SW_SHOW });
            }
        }
        let show_option_layer = !quick && self.shows_match_option_layer();
        for control in [
            self.controls.results,
            self.controls.delete_result,
            self.controls.group_name,
        ] {
            unsafe {
                let _ = ShowWindow(control, if show_option_layer { SW_SHOW } else { SW_HIDE });
            }
        }
        // Numeric slots are derived from the affix template. The selector exposes every slot the
        // model generated; manual add/remove buttons only create a second, error-prone workflow.
        for control in [self.controls.add_numeric, self.controls.delete_numeric] {
            unsafe {
                let _ = ShowWindow(control, SW_HIDE);
            }
        }
        let show_required_count = !quick
            && unsafe { combo_selection(self.controls.group_mode) }.is_some_and(|index| index == 2);
        unsafe {
            let _ = ShowWindow(
                self.controls.required_count,
                if show_required_count {
                    SW_SHOW
                } else {
                    SW_HIDE
                },
            );
        }
    }

    unsafe fn refresh_enabled_state(&self) {
        let availability = self.model.availability();
        for control in self.controls.configuration_controls() {
            unsafe {
                let _ = EnableWindow(control, availability.configuration_enabled);
            }
        }
        unsafe {
            let _ = EnableWindow(self.controls.game, availability.selectors_enabled);
            let _ = EnableWindow(self.controls.rule_mode, availability.selectors_enabled);
            let _ = EnableWindow(
                self.controls.language,
                self.model.pending_operation().is_none(),
            );
            let _ = EnableWindow(self.controls.save, availability.save_enabled);
            let _ = EnableWindow(self.controls.start, availability.start_enabled);
            let _ = EnableWindow(self.controls.stop, availability.stop_enabled);
            let _ = EnableWindow(
                self.controls.screenshot,
                availability.screenshot_test_enabled,
            );
        }

        let group_mode = unsafe { combo_selection(self.controls.group_mode) }.unwrap_or(0);
        let has_numeric = self.model.current_numeric_constraint().is_some();
        let numeric_mode = unsafe { combo_selection(self.controls.numeric_mode) }.unwrap_or(0);
        unsafe {
            let _ = EnableWindow(
                self.controls.required_count,
                availability.configuration_enabled
                    && self.model.rule_mode() == RuleMode::MultipleAffixes
                    && group_mode == 2,
            );
            let _ = EnableWindow(
                self.controls.numeric_mode,
                availability.configuration_enabled && has_numeric,
            );
            let _ = EnableWindow(
                self.controls.numeric_first,
                availability.configuration_enabled && has_numeric && numeric_mode != 0,
            );
            let _ = EnableWindow(
                self.controls.numeric_second,
                availability.configuration_enabled && has_numeric && numeric_mode == 1,
            );
            if self.closing {
                for control in self.controls.all() {
                    let _ = EnableWindow(control, false);
                }
            } else if self.rebuilding_runtime {
                let _ = EnableWindow(self.controls.start, false);
                let _ = EnableWindow(self.controls.stop, false);
                let _ = EnableWindow(self.controls.screenshot, false);
                let _ = EnableWindow(self.controls.save, false);
            }
        }
    }

    unsafe fn apply_fonts(&self) {
        for control in self.controls.all() {
            unsafe {
                SendMessageW(
                    control,
                    WM_SETFONT,
                    Some(WPARAM(self.fonts.body.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
    }

    unsafe fn recreate_fonts(&mut self) {
        self.fonts = unsafe { Fonts::create(self.dpi, self.model.language()) };
        unsafe {
            self.apply_fonts();
            self.controls.apply_control_metrics(self.dpi);
        }
    }

    unsafe fn layout(&mut self) {
        let s = |value| scale(value, self.dpi);
        let geometry = ui_geometry(self.model.rule_mode());
        let show_option_layer = self.shows_match_option_layer();
        macro_rules! place {
            ($control:expr, $x:expr, $y:expr, $width:expr, $height:expr) => {
                unsafe {
                    move_control($control, s($x), s($y), s($width), s($height));
                }
            };
        }
        macro_rules! place_edit {
            ($control:expr, $x:expr, $y:expr, $width:expr, $height:expr) => {
                unsafe {
                    move_control(
                        $control,
                        s($x + 2),
                        s($y + 2),
                        s($width - 4),
                        s($height - 4),
                    );
                }
            };
        }
        place!(self.controls.game, 30, 116, 210, 220);
        place!(self.controls.language, 260, 116, 210, 220);
        place!(self.controls.rule_mode, 490, 116, 260, 220);
        place!(self.controls.save, 870, 114, 180, 36);
        place_edit!(self.controls.quick_template, 30, 220, 1020, 58);

        if show_option_layer {
            place!(self.controls.results, 30, 218, 300, 250);
            place!(self.controls.add_result, 340, 217, 230, 34);
            place!(self.controls.delete_result, 580, 217, 120, 34);
            place_edit!(self.controls.group_name, 710, 218, 130, 34);
            place!(self.controls.group_mode, 850, 218, 170, 220);
            place_edit!(self.controls.required_count, 930, 268, 90, 32);
        } else {
            place!(self.controls.group_mode, 30, 218, 330, 220);
            place_edit!(self.controls.required_count, 370, 218, 120, 34);
            place!(self.controls.add_result, 660, 217, 360, 34);
        }

        place!(self.controls.conditions, 30, 326, 330, 250);
        place!(self.controls.add_condition, 370, 325, 130, 34);
        place!(self.controls.delete_condition, 510, 325, 130, 34);
        place_edit!(self.controls.condition_name, 660, 326, 360, 34);
        place_edit!(self.controls.condition_template, 30, 400, 610, 50);
        place!(self.controls.numeric_rules, 660, 400, 370, 220);
        place!(self.controls.numeric_mode, 660, 462, 160, 220);
        place_edit!(self.controls.numeric_first, 830, 462, 100, 32);
        place_edit!(self.controls.numeric_second, 940, 462, 90, 32);

        let settings_top = geometry.settings_top;
        place!(self.controls.ocr_language, 30, settings_top + 30, 210, 220);
        place_edit!(self.controls.region_x, 390, settings_top + 30, 100, 34);
        place_edit!(self.controls.region_y, 500, settings_top + 30, 100, 34);
        place_edit!(self.controls.region_width, 610, settings_top + 30, 100, 34);
        place_edit!(self.controls.region_height, 720, settings_top + 30, 100, 34);
        place!(self.controls.select_region, 840, settings_top + 29, 190, 36);

        place!(self.controls.keep_hud, 30, settings_top + 100, 300, 26);
        place!(
            self.controls.allow_overlay_capture,
            350,
            settings_top + 100,
            340,
            26
        );
        place!(self.controls.place_hud, 30, settings_top + 126, 220, 36);
        place_edit!(self.controls.alert_sound, 30, settings_top + 188, 510, 34);
        place!(self.controls.browse_sound, 550, settings_top + 187, 120, 36);
        place!(
            self.controls.default_sound,
            680,
            settings_top + 187,
            120,
            36
        );
        place!(self.controls.hotkey, 820, settings_top + 188, 210, 220);

        place_edit!(
            self.controls.screenshot_result,
            30,
            geometry.results_top,
            690,
            892 - geometry.results_top
        );
        place!(self.controls.start, 740, geometry.action_top, 310, 34);
        place!(self.controls.stop, 740, geometry.action_top + 38, 310, 34);
        place!(
            self.controls.screenshot,
            740,
            geometry.action_top + 76,
            310,
            34
        );
        self.layout_key = Some((
            self.dpi,
            self.model.rule_mode(),
            self.shows_match_option_layer(),
        ));
    }

    unsafe fn on_command(&mut self, wparam: WPARAM) {
        let id = (wparam.0 & 0xffff) as u16;
        let notification = ((wparam.0 >> 16) & 0xffff) as u32;
        match (id, notification) {
            (ID_GAME, CBN_SELCHANGE) => {
                if unsafe { self.commit_visible_form() } {
                    let game = match unsafe { combo_selection(self.controls.game) } {
                        Some(0) => Game::Poe1,
                        Some(1) => Game::Poe2,
                        _ => self.model.game(),
                    };
                    unsafe { self.apply_action(UiAction::SelectGame(game)) };
                } else {
                    unsafe { self.restore_primary_selections() };
                }
            }
            (ID_LANGUAGE, CBN_SELCHANGE) => {
                let can_commit = self.model.availability().configuration_enabled;
                if !can_commit || unsafe { self.commit_visible_form() } {
                    let language = match unsafe { combo_selection(self.controls.language) } {
                        Some(1) => UiLanguage::English,
                        _ => UiLanguage::SimplifiedChinese,
                    };
                    unsafe { self.apply_action(UiAction::SelectLanguage(language)) };
                } else {
                    unsafe { self.restore_primary_selections() };
                }
            }
            (ID_OCR_LANGUAGE, CBN_SELCHANGE) => {
                if unsafe { self.commit_visible_form() } {
                    let language = match unsafe { combo_selection(self.controls.ocr_language) } {
                        Some(1) => OcrLanguage::TraditionalChinese,
                        _ => OcrLanguage::English,
                    };
                    unsafe { self.apply_action(UiAction::SelectOcrLanguage(language)) };
                } else {
                    unsafe { self.restore_primary_selections() };
                }
            }
            (ID_RULE_MODE, CBN_SELCHANGE) => {
                if unsafe { self.commit_visible_form() } {
                    let mode = match unsafe { combo_selection(self.controls.rule_mode) } {
                        Some(1) => RuleMode::MultipleAffixes,
                        _ => RuleMode::Quick,
                    };
                    unsafe { self.apply_action(UiAction::SelectRuleMode(mode)) };
                } else {
                    unsafe { self.restore_primary_selections() };
                }
            }
            (ID_RESULTS, CBN_SELCHANGE) => {
                if unsafe { self.commit_visible_form() } {
                    if let Some(index) = unsafe { combo_selection(self.controls.results) } {
                        self.model.select_group(index);
                    }
                    unsafe { self.refresh_all() };
                } else {
                    unsafe { self.restore_editor_selections() };
                }
            }
            (ID_CONDITIONS, CBN_SELCHANGE) => {
                if unsafe { self.commit_visible_form() } {
                    if let Some(index) = unsafe { combo_selection(self.controls.conditions) } {
                        self.model.select_condition(index);
                    }
                    unsafe { self.refresh_all() };
                } else {
                    unsafe { self.restore_editor_selections() };
                }
            }
            (ID_NUMERIC_RULES, CBN_SELCHANGE) => {
                if unsafe { self.commit_visible_form() } {
                    if let Some(index) = unsafe { combo_selection(self.controls.numeric_rules) } {
                        self.model.select_numeric_constraint(index);
                    }
                    unsafe { self.refresh_all() };
                } else {
                    unsafe { self.restore_editor_selections() };
                }
            }
            (ID_CONDITION_TEMPLATE, EN_KILLFOCUS) => {
                if unsafe { self.commit_visible_form() } {
                    unsafe { self.refresh_all() };
                }
            }
            (ID_GROUP_MODE | ID_NUMERIC_MODE, CBN_SELCHANGE) => unsafe {
                self.refresh_visibility();
                self.refresh_enabled_state();
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            },
            (ID_ADD_RESULT, BN_CLICKED) => unsafe { self.edit_then(|model| model.add_result()) },
            (ID_DELETE_RESULT, BN_CLICKED) => unsafe {
                self.edit_then(|model| model.remove_result())
            },
            (ID_ADD_CONDITION, BN_CLICKED) => unsafe {
                self.edit_then(|model| model.add_condition())
            },
            (ID_DELETE_CONDITION, BN_CLICKED) => unsafe {
                self.edit_then(|model| model.remove_condition())
            },
            (ID_ADD_NUMERIC, BN_CLICKED) => unsafe {
                self.edit_then(|model| model.add_numeric_constraint())
            },
            (ID_DELETE_NUMERIC, BN_CLICKED) => unsafe {
                self.edit_then(|model| model.remove_numeric_constraint())
            },
            (ID_SELECT_REGION, BN_CLICKED) => {
                if unsafe { self.commit_visible_form() } {
                    unsafe { self.select_region() };
                }
            }
            (ID_BROWSE_SOUND, BN_CLICKED) => {
                if let Some(path) = unsafe {
                    open_file_dialog(self.hwnd, FileDialogKind::Sound, self.model.language())
                } {
                    match ValidatedWave::open(&path) {
                        Ok(_) => {
                            unsafe { set_text(self.controls.alert_sound, &path.to_string_lossy()) };
                            self.status_override = None;
                            let _ = unsafe { self.commit_visible_form() };
                        }
                        Err(error) => {
                            eprintln!("selected alert sound was rejected: {error}");
                            self.status_override =
                                Some(self.model.language().text().sound_fallback().to_owned());
                            unsafe { self.refresh_all() };
                        }
                    }
                }
            }
            (ID_DEFAULT_SOUND, BN_CLICKED) => {
                unsafe { set_text(self.controls.alert_sound, "") };
                self.status_override = None;
                let _ = unsafe { self.commit_visible_form() };
            }
            (ID_PLACE_HUD, BN_CLICKED) => unsafe { self.toggle_hud_placement() },
            (ID_SAVE, BN_CLICKED) => {
                if self.hud_placement_active {
                    unsafe { self.toggle_hud_placement() };
                    return;
                }
                if unsafe { self.commit_visible_form() } {
                    unsafe { self.apply_action(UiAction::SaveSettings) };
                }
            }
            (ID_START, BN_CLICKED) => {
                if self.hud_placement_active {
                    unsafe { self.toggle_hud_placement() };
                    return;
                }
                if unsafe { self.commit_visible_form() } {
                    unsafe { self.apply_action(UiAction::StartMonitoring) };
                }
            }
            (ID_STOP, BN_CLICKED) => unsafe { self.apply_action(UiAction::StopMonitoring) },
            (ID_SCREENSHOT, BN_CLICKED) => {
                if self.hud_placement_active {
                    unsafe { self.toggle_hud_placement() };
                    return;
                }
                if let Some(path) = unsafe {
                    open_file_dialog(self.hwnd, FileDialogKind::Screenshot, self.model.language())
                } && unsafe { self.commit_visible_form() }
                {
                    self.screenshot_result.clear();
                    unsafe { self.apply_action(UiAction::TestScreenshot(path)) };
                }
            }
            _ => {}
        }
    }

    unsafe fn edit_then(
        &mut self,
        operation: impl FnOnce(&mut AppState) -> Result<(), EditorError>,
    ) {
        if !unsafe { self.commit_visible_form() } {
            return;
        }
        match operation(&mut self.model) {
            Ok(()) => unsafe { self.refresh_all() },
            Err(error) => unsafe { self.show_editor_error(error, true) },
        }
    }

    unsafe fn commit_visible_form(&mut self) -> bool {
        let form = unsafe { self.read_form() };
        let sound_path = form.global.custom_alert_sound_path.trim();
        if !sound_path.is_empty() && ValidatedWave::open(sound_path).is_err() {
            unsafe { self.show_editor_error(EditorError::InvalidAlertSound, false) };
            return false;
        }
        match self.model.commit_form(form) {
            Ok(()) => true,
            Err(error) => {
                unsafe { self.show_editor_error(error, false) };
                false
            }
        }
    }

    unsafe fn show_editor_error(&mut self, error: EditorError, refresh_fields: bool) {
        self.model.report_validation_error(error);
        if refresh_fields {
            unsafe { self.refresh_all() };
        } else {
            unsafe {
                self.refresh_enabled_state();
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    unsafe fn read_form(&self) -> EditorForm {
        let structured = self.model.rule_mode() == RuleMode::MultipleAffixes;
        EditorForm {
            quick_template: (!structured)
                .then(|| unsafe { get_text(self.controls.quick_template) }),
            ocr_language: match unsafe { combo_selection(self.controls.ocr_language) } {
                Some(1) => OcrLanguage::TraditionalChinese,
                _ => OcrLanguage::English,
            },
            region: RegionEdit {
                x: unsafe { get_text(self.controls.region_x) },
                y: unsafe { get_text(self.controls.region_y) },
                width: unsafe { get_text(self.controls.region_width) },
                height: unsafe { get_text(self.controls.region_height) },
            },
            global: GlobalEdit {
                keep_hud_visible: unsafe { is_checked(self.controls.keep_hud) },
                allow_overlay_capture: unsafe { is_checked(self.controls.allow_overlay_capture) },
                hud_monitor: unsafe { get_text(self.controls.hud_monitor) },
                hud_x: unsafe { get_text(self.controls.hud_x) },
                hud_y: unsafe { get_text(self.controls.hud_y) },
                custom_alert_sound_path: unsafe { get_text(self.controls.alert_sound) },
                start_monitoring_hot_key: hotkey_value(unsafe {
                    combo_selection(self.controls.hotkey)
                })
                .to_owned(),
            },
            group: structured.then(|| GroupEdit {
                name: unsafe { get_text(self.controls.group_name) },
                mode: match unsafe { combo_selection(self.controls.group_mode) } {
                    Some(1) => ResultGroupMode::All,
                    Some(2) => ResultGroupMode::AtLeast,
                    _ => ResultGroupMode::Any,
                },
                required_count: unsafe { get_text(self.controls.required_count) },
            }),
            condition: structured.then(|| ConditionEdit {
                name: unsafe { get_text(self.controls.condition_name) },
                template: unsafe { get_text(self.controls.condition_template) },
            }),
            numeric_constraint: (structured && self.model.current_numeric_constraint().is_some())
                .then(|| NumericConstraintEdit {
                    mode: match unsafe { combo_selection(self.controls.numeric_mode) } {
                        Some(1) => NumericConstraintMode::RangeInclusive,
                        Some(2) => NumericConstraintMode::AtLeast,
                        Some(3) => NumericConstraintMode::AtMost,
                        Some(4) => NumericConstraintMode::Exactly,
                        _ => NumericConstraintMode::Ignore,
                    },
                    first_value: unsafe { get_text(self.controls.numeric_first) },
                    second_value: unsafe { get_text(self.controls.numeric_second) },
                }),
        }
    }

    unsafe fn restore_primary_selections(&self) {
        unsafe {
            SendMessageW(
                self.controls.game,
                CB_SETCURSEL,
                Some(WPARAM(match self.model.game() {
                    Game::Poe1 => 0,
                    Game::Poe2 => 1,
                })),
                None,
            );
            SendMessageW(
                self.controls.language,
                CB_SETCURSEL,
                Some(WPARAM(match self.model.language() {
                    UiLanguage::SimplifiedChinese => 0,
                    UiLanguage::English => 1,
                })),
                None,
            );
            SendMessageW(
                self.controls.rule_mode,
                CB_SETCURSEL,
                Some(WPARAM(match self.model.rule_mode() {
                    RuleMode::Quick => 0,
                    RuleMode::MultipleAffixes => 1,
                })),
                None,
            );
            SendMessageW(
                self.controls.ocr_language,
                CB_SETCURSEL,
                Some(WPARAM(match self.model.ocr_language() {
                    OcrLanguage::English => 0,
                    OcrLanguage::TraditionalChinese => 1,
                })),
                None,
            );
        }
    }

    unsafe fn restore_editor_selections(&self) {
        unsafe {
            SendMessageW(
                self.controls.results,
                CB_SETCURSEL,
                Some(WPARAM(self.model.selected_group_index())),
                None,
            );
            SendMessageW(
                self.controls.conditions,
                CB_SETCURSEL,
                Some(WPARAM(self.model.selected_condition_index())),
                None,
            );
            SendMessageW(
                self.controls.numeric_rules,
                CB_SETCURSEL,
                Some(WPARAM(self.model.selected_numeric_constraint_index())),
                None,
            );
        }
    }

    unsafe fn paint_control_background(
        &self,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let dc = HDC(wparam.0 as *mut c_void);
        let control = HWND(lparam.0 as *mut c_void);
        let disabled = !unsafe { IsWindowEnabled(control) }.as_bool();
        let foreground = if disabled {
            SEA_GLASS_PALETTE.text_disabled
        } else {
            SEA_GLASS_PALETTE.text_primary
        };
        let input_surface = matches!(message, WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX)
            || control == self.controls.screenshot_result;
        let (background, brush) = if input_surface {
            (SEA_GLASS_PALETTE.input, &self.theme_brushes.input)
        } else {
            (SEA_GLASS_PALETTE.card, &self.theme_brushes.card)
        };
        unsafe {
            SetTextColor(dc, theme_color(foreground));
            SetBkColor(dc, theme_color(background));
        }
        LRESULT(brush.0.0 as isize)
    }

    unsafe fn draw_button(&self, lparam: LPARAM) -> LRESULT {
        if lparam.0 == 0 {
            return LRESULT(0);
        }
        let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
        if item.CtlType != ODT_BUTTON {
            return LRESULT(0);
        }
        let id = item.CtlID as u16;
        let role = match id {
            ID_SAVE | ID_START => ControlRole::Primary,
            ID_DELETE_RESULT | ID_DELETE_CONDITION | ID_DELETE_NUMERIC | ID_STOP => {
                ControlRole::Destructive
            }
            _ => ControlRole::Secondary,
        };
        let focused = item.itemState.0 & ODS_FOCUS.0 != 0;
        let state = if item.itemState.0 & ODS_DISABLED.0 != 0 {
            ControlState::Disabled
        } else if item.itemState.0 & ODS_SELECTED.0 != 0 {
            ControlState::Pressed
        } else if item.itemState.0 & ODS_HOTLIGHT.0 != 0 {
            ControlState::Hovered
        } else if focused {
            ControlState::Focused
        } else {
            ControlState::Normal
        };
        let colors = SEA_GLASS_PALETTE.control_colors(role, state);
        let parent_background = if matches!(id, ID_SAVE | ID_START | ID_STOP | ID_SCREENSHOT) {
            SEA_GLASS_PALETTE.canvas
        } else {
            SEA_GLASS_PALETTE.card
        };
        let parent_brush = Brush::new(theme_color(parent_background));
        let fill = Brush::new(theme_color(colors.background));
        let border_color = if focused && !matches!(state, ControlState::Disabled) {
            SEA_GLASS_PALETTE.focus
        } else {
            colors.focus_ring.unwrap_or(colors.border)
        };
        let border = Brush::new(theme_color(border_color));
        let shadow = Brush::new(theme_color(SEA_GLASS_PALETTE.shadow));
        let dpi = unsafe { GetDpiForWindow(item.hwndItem) }.max(96);
        let inset = scale(1, dpi).max(1);
        let radius = scale(7, dpi).max(3);
        let shadow_offset = scale(1, dpi).max(1);
        let button_bounds = RECT {
            left: item.rcItem.left,
            top: item.rcItem.top,
            right: item.rcItem.right,
            bottom: item.rcItem.bottom - shadow_offset,
        };
        unsafe {
            FillRect(item.hDC, &item.rcItem, parent_brush.0);
            draw_rounded_surface(
                item.hDC,
                RECT {
                    left: button_bounds.left,
                    top: button_bounds.top + shadow_offset,
                    right: button_bounds.right,
                    bottom: button_bounds.bottom + shadow_offset,
                },
                shadow.0,
                radius,
            );
            draw_rounded_surface(item.hDC, button_bounds, border.0, radius);
            draw_rounded_surface(
                item.hDC,
                RECT {
                    left: button_bounds.left + inset,
                    top: button_bounds.top + inset,
                    right: button_bounds.right - inset,
                    bottom: button_bounds.bottom - inset,
                },
                fill.0,
                (radius - inset).max(2),
            );
            let mut text_bounds = button_bounds;
            if matches!(state, ControlState::Pressed) {
                text_bounds.top += inset;
                text_bounds.bottom += inset;
            }
            draw_text(
                item.hDC,
                self.fonts.body,
                theme_color(colors.foreground),
                &get_text(item.hwndItem),
                text_bounds,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
        LRESULT(1)
    }

    unsafe fn paint_edit_shells(&self, dc: HDC) {
        let fill = Brush::new(theme_color(SEA_GLASS_PALETTE.input));
        let border = Brush::new(theme_color(SEA_GLASS_PALETTE.border));
        let expansion = scale(2, self.dpi).max(1);
        let radius = scale(7, self.dpi).max(3);
        for edit in self.controls.text_inputs() {
            if !unsafe { IsWindowVisible(edit) }.as_bool() {
                continue;
            }
            let bounds = unsafe { control_bounds_in_parent(edit, self.hwnd) };
            let outer = RECT {
                left: bounds.left - expansion,
                top: bounds.top - expansion,
                right: bounds.right + expansion,
                bottom: bounds.bottom + expansion,
            };
            unsafe {
                draw_rounded_surface(dc, outer, border.0, radius);
                draw_rounded_surface(
                    dc,
                    RECT {
                        left: outer.left + 1,
                        top: outer.top + 1,
                        right: outer.right - 1,
                        bottom: outer.bottom - 1,
                    },
                    fill.0,
                    (radius - 1).max(2),
                );
            }
        }
    }

    unsafe fn paint(&self) {
        let mut paint = PAINTSTRUCT::default();
        let dc = unsafe { BeginPaint(self.hwnd, &mut paint) };
        let mut client = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut client) }.is_err() {
            unsafe {
                let _ = EndPaint(self.hwnd, &paint);
            }
            return;
        }
        let background = Brush::new(theme_color(SEA_GLASS_PALETTE.canvas));
        let header = Brush::new(theme_color(SEA_GLASS_PALETTE.header));
        let section = Brush::new(theme_color(SEA_GLASS_PALETTE.card));
        let raised_section = Brush::new(theme_color(SEA_GLASS_PALETTE.card_raised));
        let section_border = Brush::new(theme_color(SEA_GLASS_PALETTE.border));
        let shadow = Brush::new(theme_color(SEA_GLASS_PALETTE.shadow));
        let accent_rule = Brush::new(theme_color(SEA_GLASS_PALETTE.accent));
        let accent = Brush::new(self.status_color());
        unsafe {
            FillRect(dc, &client, background.0);
        }
        let s = |value| scale(value, self.dpi);
        let geometry = ui_geometry(self.model.rule_mode());
        unsafe {
            FillRect(
                dc,
                &RECT {
                    left: 0,
                    top: 0,
                    right: client.right,
                    bottom: s(80),
                },
                header.0,
            );
            FillRect(
                dc,
                &RECT {
                    left: 0,
                    top: s(80),
                    right: client.right,
                    bottom: s(82),
                },
                accent_rule.0,
            );
            draw_glass_card(
                dc,
                RECT {
                    left: s(18),
                    top: s(160),
                    right: s(1062),
                    bottom: s(geometry.rules_bottom),
                },
                self.dpi,
                section.0,
                section_border.0,
                shadow.0,
            );
            draw_glass_card(
                dc,
                RECT {
                    left: s(18),
                    top: s(geometry.settings_top),
                    right: s(1062),
                    bottom: s(geometry.settings_bottom),
                },
                self.dpi,
                section.0,
                section_border.0,
                shadow.0,
            );
            draw_rounded_surface(
                dc,
                RECT {
                    left: s(18),
                    top: s(geometry.status_top),
                    right: s(1062),
                    bottom: s(geometry.status_bottom),
                },
                section_border.0,
                s(10),
            );
            let status_inset = s(1).max(1);
            draw_rounded_surface(
                dc,
                RECT {
                    left: s(18) + status_inset,
                    top: s(geometry.status_top) + status_inset,
                    right: s(1062) - status_inset,
                    bottom: s(geometry.status_bottom) - status_inset,
                },
                raised_section.0,
                (s(10) - status_inset).max(2),
            );
            draw_rounded_surface(
                dc,
                RECT {
                    left: s(24),
                    top: s(geometry.status_top + 8),
                    right: s(28),
                    bottom: s(geometry.status_bottom - 8),
                },
                accent.0,
                s(2).max(1),
            );
            self.paint_edit_shells(dc);
            SetBkMode(dc, TRANSPARENT);
        }
        let text = self.model.language().text();
        unsafe {
            draw_text(
                dc,
                self.fonts.title,
                theme_color(SEA_GLASS_PALETTE.text_primary),
                text.heading,
                rect(s(30), s(16), s(650), s(48)),
                DT_LEFT | DT_SINGLELINE | DT_VCENTER,
            );
            draw_text(
                dc,
                self.fonts.subtitle,
                theme_color(SEA_GLASS_PALETTE.text_muted),
                text.subtitle,
                rect(s(30), s(49), s(1020), s(76)),
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
            draw_label(dc, self.fonts.label, text.game_label, s(30), s(90), s(210));
            draw_label(
                dc,
                self.fonts.label,
                text.language_label,
                s(260),
                s(90),
                s(210),
            );
            draw_label(
                dc,
                self.fonts.label,
                text.rule_mode_label,
                s(490),
                s(90),
                s(260),
            );

            if self.model.rule_mode() == RuleMode::Quick {
                draw_help(dc, self.fonts.body, text.quick_help, s(30), s(168), s(1020));
                draw_label(
                    dc,
                    self.fonts.label,
                    text.quick_template_label,
                    s(30),
                    s(194),
                    s(300),
                );
            } else {
                let show_option_layer = self.shows_match_option_layer();
                draw_help(
                    dc,
                    self.fonts.body,
                    if show_option_layer {
                        text.structured_help
                    } else {
                        text.structured_single_help
                    },
                    s(30),
                    s(168),
                    s(1020),
                );
                let show_required_count =
                    combo_selection(self.controls.group_mode).is_some_and(|index| index == 2);
                if show_option_layer {
                    draw_label(
                        dc,
                        self.fonts.label,
                        text.results_label,
                        s(30),
                        s(194),
                        s(300),
                    );
                    draw_label(
                        dc,
                        self.fonts.label,
                        text.result_name,
                        s(710),
                        s(194),
                        s(130),
                    );
                    draw_label(
                        dc,
                        self.fonts.label,
                        text.group_rule,
                        s(850),
                        s(194),
                        s(190),
                    );
                    if show_required_count {
                        draw_label(
                            dc,
                            self.fonts.label,
                            text.required_count,
                            s(850),
                            s(268),
                            s(75),
                        );
                    }
                } else {
                    draw_label(dc, self.fonts.label, text.group_rule, s(30), s(194), s(330));
                    if show_required_count {
                        draw_label(
                            dc,
                            self.fonts.label,
                            text.required_count,
                            s(370),
                            s(194),
                            s(120),
                        );
                    }
                }
                draw_label(
                    dc,
                    self.fonts.label,
                    text.conditions_label,
                    s(30),
                    s(302),
                    s(300),
                );
                draw_label(
                    dc,
                    self.fonts.label,
                    text.condition_name,
                    s(660),
                    s(302),
                    s(300),
                );
                draw_label(
                    dc,
                    self.fonts.label,
                    text.condition_template,
                    s(30),
                    s(376),
                    s(400),
                );
                draw_label(
                    dc,
                    self.fonts.label,
                    text.numeric_rules_label,
                    s(660),
                    s(376),
                    s(200),
                );
                draw_label(
                    dc,
                    self.fonts.label,
                    text.numeric_comparison,
                    s(660),
                    s(438),
                    s(160),
                );
                draw_label(
                    dc,
                    self.fonts.label,
                    text.first_value,
                    s(830),
                    s(438),
                    s(105),
                );
                draw_label(
                    dc,
                    self.fonts.label,
                    text.second_value,
                    s(940),
                    s(438),
                    s(100),
                );
                draw_help(
                    dc,
                    self.fonts.body,
                    text.numeric_value_help,
                    s(30),
                    s(452),
                    s(610),
                );
            }

            draw_label(
                dc,
                self.fonts.label,
                text.ocr_language_label,
                s(30),
                s(geometry.settings_top + 8),
                s(220),
            );
            draw_label(
                dc,
                self.fonts.label,
                text.capture_region_label,
                s(270),
                s(geometry.settings_top + 8),
                s(560),
            );
            draw_small_label(
                dc,
                self.fonts.body,
                text.region_x,
                s(390),
                s(geometry.settings_top + 64),
                s(100),
            );
            draw_small_label(
                dc,
                self.fonts.body,
                text.region_y,
                s(500),
                s(geometry.settings_top + 64),
                s(100),
            );
            draw_small_label(
                dc,
                self.fonts.body,
                text.region_width,
                s(610),
                s(geometry.settings_top + 64),
                s(100),
            );
            draw_small_label(
                dc,
                self.fonts.body,
                text.region_height,
                s(720),
                s(geometry.settings_top + 64),
                s(100),
            );
            draw_label(
                dc,
                self.fonts.label,
                text.general_settings_label,
                s(30),
                s(geometry.settings_top + 78),
                s(300),
            );
            draw_small_label(
                dc,
                self.fonts.body,
                text.hud_position_help,
                s(270),
                s(geometry.settings_top + 132),
                s(520),
            );
            draw_label(
                dc,
                self.fonts.label,
                text.alert_sound,
                s(30),
                s(geometry.settings_top + 164),
                s(560),
            );
            draw_label(
                dc,
                self.fonts.label,
                text.hotkey,
                s(820),
                s(geometry.settings_top + 164),
                s(220),
            );
            draw_text(
                dc,
                self.fonts.label,
                theme_color(SEA_GLASS_PALETTE.text_muted),
                text.status_label,
                rect(
                    s(36),
                    s(geometry.status_top),
                    s(160),
                    s(geometry.status_bottom),
                ),
                DT_LEFT | DT_SINGLELINE | DT_VCENTER,
            );
            let status = if self.closing || matches!(self.model.notice(), crate::UiNotice::None) {
                self.status_override
                    .clone()
                    .unwrap_or_else(|| text.status(self.model.status(), self.model.notice()))
            } else {
                text.status(self.model.status(), self.model.notice())
            };
            draw_text(
                dc,
                self.fonts.status,
                self.status_color(),
                &status,
                rect(
                    s(160),
                    s(geometry.status_top),
                    s(1040),
                    s(geometry.status_bottom),
                ),
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
            draw_small_label(
                dc,
                self.fonts.label,
                text.screenshot_results_label,
                s(30),
                s(geometry.results_label_y),
                s(360),
            );
            let _ = EndPaint(self.hwnd, &paint);
        }
    }

    fn status_color(&self) -> COLORREF {
        let tone = match self.model.status() {
            MonitorStatus::Monitoring | MonitorStatus::SettingsSaved => StatusTone::Success,
            MonitorStatus::MatchFound
            | MonitorStatus::Error
            | MonitorStatus::ValidationError
            | MonitorStatus::ReadOnly => StatusTone::Danger,
            MonitorStatus::Starting
            | MonitorStatus::Stopping
            | MonitorStatus::TestingScreenshot
            | MonitorStatus::SavingSettings => StatusTone::Warning,
            _ => StatusTone::Info,
        };
        theme_color(SEA_GLASS_PALETTE.status_color(tone))
    }

    unsafe fn begin_self_test(&mut self) {
        unsafe {
            self.apply_action(UiAction::SelectLanguage(UiLanguage::English));
            self.apply_action(UiAction::SelectGame(Game::Poe1));
            self.apply_action(UiAction::SelectRuleMode(RuleMode::MultipleAffixes));
            set_text(self.controls.group_name, "Critical result");
            SendMessageW(
                self.controls.group_mode,
                CB_SETCURSEL,
                Some(WPARAM(2)),
                None,
            );
            set_text(self.controls.required_count, "1");
            set_text(self.controls.condition_name, "Critical chance");
            set_text(
                self.controls.condition_template,
                "+#% to Critical Strike Chance",
            );
        }
        if !unsafe { self.commit_visible_form() } || self.model.add_numeric_constraint().is_err() {
            self.finish_self_test(false);
            return;
        }
        unsafe {
            self.refresh_all();
            SendMessageW(
                self.controls.numeric_mode,
                CB_SETCURSEL,
                Some(WPARAM(2)),
                None,
            );
            set_text(self.controls.numeric_first, "3.1");
        }
        if !unsafe { self.commit_visible_form() } || self.model.validate_for_save().is_err() {
            self.finish_self_test(false);
            return;
        }
        unsafe { self.apply_action(UiAction::StartMonitoring) };
    }

    unsafe fn advance_self_test(&mut self, completed: Operation) {
        if !self.self_test {
            return;
        }
        match completed {
            Operation::StartMonitoring => unsafe { self.apply_action(UiAction::StopMonitoring) },
            Operation::StopMonitoring => unsafe {
                self.apply_action(UiAction::TestScreenshot(PathBuf::from(
                    "self-test-screenshot.png",
                )))
            },
            Operation::TestScreenshot => {
                let minimum = self
                    .model
                    .current_numeric_constraint()
                    .and_then(|constraint| constraint.minimum)
                    .map(|value| value.to_string());
                let completed_pipeline = self.model.language() == UiLanguage::English
                    && self.model.game() == Game::Poe1
                    && self.model.rule_mode() == RuleMode::MultipleAffixes
                    && self.model.status() == MonitorStatus::ScreenshotComplete
                    && !self.model.is_monitoring()
                    && self.model.pending_operation().is_none()
                    && self.model.current_condition().is_some_and(|condition| {
                        condition.template == "+#% to Critical Strike Chance"
                    })
                    && minimum.as_deref() == Some("3.1")
                    && unsafe { get_text(self.controls.condition_template) }
                        == "+#% to Critical Strike Chance"
                    && self.screenshot_result.contains("Result: target matched")
                    && self.screenshot_result.contains("recognition 3.0 ms")
                    && self
                        .screenshot_result
                        .contains("+#% to Critical Strike Chance")
                    && self.hud.is_some();
                self.model = AppState::from_settings(AppSettings::default(), Some(99));
                unsafe { self.refresh_all() };
                let read_only_controls = !unsafe { IsWindowEnabled(self.controls.save) }.as_bool()
                    && !unsafe { IsWindowEnabled(self.controls.quick_template) }.as_bool()
                    && !unsafe { IsWindowEnabled(self.controls.game) }.as_bool()
                    && !unsafe { IsWindowEnabled(self.controls.start) }.as_bool();
                self.finish_self_test(completed_pipeline && read_only_controls);
            }
            Operation::SaveSettings => self.finish_self_test(false),
        }
    }

    fn finish_self_test(&mut self, passed: bool) {
        self.exit_code = if passed { 0 } else { 2 };
        unsafe { self.begin_close() };
    }
}

#[derive(Default)]
struct Controls {
    game: HWND,
    language: HWND,
    rule_mode: HWND,
    save: HWND,
    quick_template: HWND,
    ocr_language: HWND,
    region_x: HWND,
    region_y: HWND,
    region_width: HWND,
    region_height: HWND,
    select_region: HWND,
    results: HWND,
    add_result: HWND,
    delete_result: HWND,
    group_name: HWND,
    group_mode: HWND,
    required_count: HWND,
    conditions: HWND,
    add_condition: HWND,
    delete_condition: HWND,
    condition_name: HWND,
    condition_template: HWND,
    numeric_rules: HWND,
    add_numeric: HWND,
    delete_numeric: HWND,
    numeric_mode: HWND,
    numeric_first: HWND,
    numeric_second: HWND,
    keep_hud: HWND,
    allow_overlay_capture: HWND,
    hud_monitor: HWND,
    hud_x: HWND,
    hud_y: HWND,
    place_hud: HWND,
    alert_sound: HWND,
    browse_sound: HWND,
    default_sound: HWND,
    hotkey: HWND,
    start: HWND,
    stop: HWND,
    screenshot: HWND,
    screenshot_result: HWND,
}

impl Controls {
    unsafe fn create(parent: HWND) -> Result<Self, String> {
        Ok(Self {
            game: unsafe { create_combo(parent, ID_GAME) }?,
            language: unsafe { create_combo(parent, ID_LANGUAGE) }?,
            rule_mode: unsafe { create_combo(parent, ID_RULE_MODE) }?,
            save: unsafe { create_button(parent, ID_SAVE) }?,
            quick_template: unsafe { create_multiline_edit(parent, ID_QUICK_TEMPLATE) }?,
            ocr_language: unsafe { create_combo(parent, ID_OCR_LANGUAGE) }?,
            region_x: unsafe { create_edit(parent, ID_REGION_X) }?,
            region_y: unsafe { create_edit(parent, ID_REGION_Y) }?,
            region_width: unsafe { create_edit(parent, ID_REGION_WIDTH) }?,
            region_height: unsafe { create_edit(parent, ID_REGION_HEIGHT) }?,
            select_region: unsafe { create_button(parent, ID_SELECT_REGION) }?,
            results: unsafe { create_combo(parent, ID_RESULTS) }?,
            add_result: unsafe { create_button(parent, ID_ADD_RESULT) }?,
            delete_result: unsafe { create_button(parent, ID_DELETE_RESULT) }?,
            group_name: unsafe { create_edit(parent, ID_GROUP_NAME) }?,
            group_mode: unsafe { create_combo(parent, ID_GROUP_MODE) }?,
            required_count: unsafe { create_edit(parent, ID_REQUIRED_COUNT) }?,
            conditions: unsafe { create_combo(parent, ID_CONDITIONS) }?,
            add_condition: unsafe { create_button(parent, ID_ADD_CONDITION) }?,
            delete_condition: unsafe { create_button(parent, ID_DELETE_CONDITION) }?,
            condition_name: unsafe { create_edit(parent, ID_CONDITION_NAME) }?,
            condition_template: unsafe { create_multiline_edit(parent, ID_CONDITION_TEMPLATE) }?,
            numeric_rules: unsafe { create_combo(parent, ID_NUMERIC_RULES) }?,
            add_numeric: unsafe { create_button(parent, ID_ADD_NUMERIC) }?,
            delete_numeric: unsafe { create_button(parent, ID_DELETE_NUMERIC) }?,
            numeric_mode: unsafe { create_combo(parent, ID_NUMERIC_MODE) }?,
            numeric_first: unsafe { create_edit(parent, ID_NUMERIC_FIRST) }?,
            numeric_second: unsafe { create_edit(parent, ID_NUMERIC_SECOND) }?,
            keep_hud: unsafe { create_checkbox(parent, ID_KEEP_HUD) }?,
            allow_overlay_capture: unsafe { create_checkbox(parent, ID_ALLOW_OVERLAY_CAPTURE) }?,
            // Kept as hidden backing fields so existing settings round-trip unchanged. Users
            // position the HUD directly instead of editing monitor names or relative decimals.
            hud_monitor: unsafe { create_hidden_edit(parent, ID_HUD_MONITOR) }?,
            hud_x: unsafe { create_hidden_edit(parent, ID_HUD_X) }?,
            hud_y: unsafe { create_hidden_edit(parent, ID_HUD_Y) }?,
            place_hud: unsafe { create_button(parent, ID_PLACE_HUD) }?,
            alert_sound: unsafe { create_edit(parent, ID_ALERT_SOUND) }?,
            browse_sound: unsafe { create_button(parent, ID_BROWSE_SOUND) }?,
            default_sound: unsafe { create_button(parent, ID_DEFAULT_SOUND) }?,
            hotkey: unsafe { create_combo(parent, ID_HOTKEY) }?,
            start: unsafe { create_button(parent, ID_START) }?,
            stop: unsafe { create_button(parent, ID_STOP) }?,
            screenshot: unsafe { create_button(parent, ID_SCREENSHOT) }?,
            screenshot_result: unsafe {
                create_readonly_multiline_edit(parent, ID_SCREENSHOT_RESULT)
            }?,
        })
    }

    unsafe fn apply_visual_theme(&self) {
        // Windows owns the keyboard, focus, IME and accessibility behaviour of these
        // controls. Keep the documented light Explorer rendering instead of replacing
        // that mature interaction layer.
        for control in self.all() {
            unsafe {
                let _ = SetWindowTheme(control, w!("Explorer"), PCWSTR::null());
            }
        }
    }

    unsafe fn apply_control_metrics(&self, layout_dpi: u32) {
        for edit in self.text_inputs() {
            let margin = scale(8, layout_dpi).clamp(1, i32::from(u16::MAX)) as u16;
            let packed_margins = u32::from(margin) | (u32::from(margin) << 16);
            unsafe {
                SendMessageW(
                    edit,
                    EM_SETMARGINS,
                    Some(WPARAM((EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize)),
                    Some(LPARAM(packed_margins as isize)),
                );
            }
        }
        for combo in self.combos() {
            unsafe {
                SendMessageW(
                    combo,
                    CB_SETITEMHEIGHT,
                    Some(WPARAM(usize::MAX)),
                    Some(LPARAM(scale(27, layout_dpi) as isize)),
                );
                SendMessageW(
                    combo,
                    CB_SETITEMHEIGHT,
                    Some(WPARAM(0)),
                    Some(LPARAM(scale(25, layout_dpi) as isize)),
                );
            }
        }
    }

    fn text_inputs(&self) -> [HWND; 13] {
        [
            self.quick_template,
            self.region_x,
            self.region_y,
            self.region_width,
            self.region_height,
            self.group_name,
            self.required_count,
            self.condition_name,
            self.condition_template,
            self.numeric_first,
            self.numeric_second,
            self.alert_sound,
            self.screenshot_result,
        ]
    }

    fn combos(&self) -> [HWND; 10] {
        [
            self.game,
            self.language,
            self.rule_mode,
            self.ocr_language,
            self.results,
            self.group_mode,
            self.conditions,
            self.numeric_rules,
            self.numeric_mode,
            self.hotkey,
        ]
    }

    fn all(&self) -> Vec<HWND> {
        vec![
            self.game,
            self.language,
            self.rule_mode,
            self.save,
            self.quick_template,
            self.ocr_language,
            self.region_x,
            self.region_y,
            self.region_width,
            self.region_height,
            self.select_region,
            self.results,
            self.add_result,
            self.delete_result,
            self.group_name,
            self.group_mode,
            self.required_count,
            self.conditions,
            self.add_condition,
            self.delete_condition,
            self.condition_name,
            self.condition_template,
            self.numeric_rules,
            self.add_numeric,
            self.delete_numeric,
            self.numeric_mode,
            self.numeric_first,
            self.numeric_second,
            self.keep_hud,
            self.allow_overlay_capture,
            self.hud_monitor,
            self.hud_x,
            self.hud_y,
            self.place_hud,
            self.alert_sound,
            self.browse_sound,
            self.default_sound,
            self.hotkey,
            self.start,
            self.stop,
            self.screenshot,
            self.screenshot_result,
        ]
    }

    fn quick_only(&self) -> [HWND; 1] {
        [self.quick_template]
    }

    fn structured_only(&self) -> [HWND; 17] {
        [
            self.results,
            self.add_result,
            self.delete_result,
            self.group_name,
            self.group_mode,
            self.required_count,
            self.conditions,
            self.add_condition,
            self.delete_condition,
            self.condition_name,
            self.condition_template,
            self.numeric_rules,
            self.add_numeric,
            self.delete_numeric,
            self.numeric_mode,
            self.numeric_first,
            self.numeric_second,
        ]
    }

    fn configuration_controls(&self) -> Vec<HWND> {
        vec![
            self.quick_template,
            self.ocr_language,
            self.region_x,
            self.region_y,
            self.region_width,
            self.region_height,
            self.select_region,
            self.results,
            self.add_result,
            self.delete_result,
            self.group_name,
            self.group_mode,
            self.required_count,
            self.conditions,
            self.add_condition,
            self.delete_condition,
            self.condition_name,
            self.condition_template,
            self.numeric_rules,
            self.add_numeric,
            self.delete_numeric,
            self.numeric_mode,
            self.numeric_first,
            self.numeric_second,
            self.keep_hud,
            self.allow_overlay_capture,
            self.hud_monitor,
            self.hud_x,
            self.hud_y,
            self.place_hud,
            self.alert_sound,
            self.browse_sound,
            self.default_sound,
            self.hotkey,
        ]
    }
}

#[derive(Default)]
struct Fonts {
    body: HFONT,
    label: HFONT,
    title: HFONT,
    subtitle: HFONT,
    status: HFONT,
    language: UiLanguage,
}

impl Fonts {
    unsafe fn create(dpi: u32, language: UiLanguage) -> Self {
        let text_face = ui_font_face(language, false);
        let display_face = ui_font_face(language, true);
        Self {
            body: unsafe { create_font(dpi, 14, FW_NORMAL.0 as i32, text_face) },
            label: unsafe { create_font(dpi, 12, FW_SEMIBOLD.0 as i32, text_face) },
            title: unsafe { create_font(dpi, 26, FW_SEMIBOLD.0 as i32, display_face) },
            subtitle: unsafe { create_font(dpi, 13, FW_NORMAL.0 as i32, text_face) },
            status: unsafe { create_font(dpi, 14, FW_SEMIBOLD.0 as i32, text_face) },
            language,
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [
            self.body,
            self.label,
            self.title,
            self.subtitle,
            self.status,
        ] {
            if !font.0.is_null() {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
            }
        }
    }
}

struct Brush(HBRUSH);

impl Brush {
    fn new(color: COLORREF) -> Self {
        Self(unsafe { CreateSolidBrush(color) })
    }
}

impl Drop for Brush {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(self.0.0));
            }
        }
    }
}

struct ThemeBrushes {
    card: Brush,
    input: Brush,
}

impl ThemeBrushes {
    fn new() -> Self {
        Self {
            card: Brush::new(theme_color(SEA_GLASS_PALETTE.card)),
            input: Brush::new(theme_color(SEA_GLASS_PALETTE.input)),
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        let state = create.lpCreateParams.cast::<WindowState>();
        if state.is_null() {
            return LRESULT(0);
        }
        unsafe {
            (*state).hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        }
        return LRESULT(1);
    }

    let state_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if message == WM_NCDESTROY {
        let result = unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
        if !state_pointer.is_null() {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(state_pointer));
            }
        }
        return result;
    }
    if state_pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let state = unsafe { &mut *state_pointer };
    match message {
        WM_CREATE => match unsafe { state.initialize() } {
            Ok(()) => LRESULT(0),
            Err(_) => LRESULT(-1),
        },
        WM_COMMAND => {
            unsafe { state.on_command(wparam) };
            LRESULT(0)
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => unsafe {
            state.paint_control_background(message, wparam, lparam)
        },
        WM_DRAWITEM => unsafe { state.draw_button(lparam) },
        WM_APP_WORK_COMPLETED => {
            if lparam.0 != 0 {
                let completion = unsafe { Box::from_raw(lparam.0 as *mut BackgroundCompletion) };
                let operation = completion.operation;
                let succeeded = completion.outcome == WorkOutcome::Succeeded;
                state
                    .model
                    .apply(UiAction::BackgroundCompleted(*completion));
                if operation == Operation::SaveSettings && succeeded {
                    state.reconfigure_start_hot_key();
                    state.request_runtime_rebuild_if_needed();
                }
                unsafe {
                    state.refresh_all();
                    state.advance_self_test(operation);
                }
            }
            LRESULT(0)
        }
        WM_APP_CLOSE_SAVE_COMPLETED => {
            if lparam.0 != 0 {
                let outcome = unsafe { Box::from_raw(lparam.0 as *mut WorkOutcome) };
                if *outcome != WorkOutcome::Succeeded {
                    eprintln!("settings could not be saved while closing: {outcome:?}");
                }
            }
            state.close_save_pending = false;
            if unsafe { state.finish_close_if_ready() } {
                return LRESULT(0);
            }
            LRESULT(0)
        }
        WM_APP_SELF_TEST => {
            unsafe { state.begin_self_test() };
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == RUNTIME_TIMER_ID => {
            unsafe { state.pump_runtime_events() };
            LRESULT(0)
        }
        WM_HOTKEY => {
            unsafe { state.on_hot_key(wparam) };
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe { state.begin_close() };
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            let physical_dpi = ((wparam.0 >> 16) as u32).max(96);
            if let Err(error) = unsafe {
                state.fit_window_to_monitor(physical_dpi, suggested, suggested.left, suggested.top)
            } {
                eprintln!("could not fit the configuration window to the display: {error}");
            } else {
                unsafe {
                    state.recreate_fonts();
                    state.layout();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_PAINT => {
            unsafe { state.paint() };
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), RUNTIME_TIMER_ID);
            }
            state.hot_keys.take();
            state.save_worker.take();
            unsafe { PostQuitMessage(state.exit_code) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn start_save_worker(
    hwnd: HWND,
    settings_path: PathBuf,
) -> Result<mpsc::Sender<SaveRequest>, String> {
    let (sender, receiver) = mpsc::channel::<SaveRequest>();
    let window_value = hwnd.0 as usize;
    std::thread::Builder::new()
        .name("poe-alarm-native-ui-worker".to_owned())
        .spawn(move || {
            let window = HWND(window_value as *mut c_void);
            while let Ok(request) = receiver.recv() {
                let outcome = save_settings(&settings_path, &request.settings);
                match request.purpose {
                    SavePurpose::Normal => post_completion(
                        window,
                        BackgroundCompletion {
                            operation: Operation::SaveSettings,
                            outcome,
                        },
                    ),
                    SavePurpose::Closing => post_close_save_completion(window, outcome),
                }
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(sender)
}

fn save_settings(path: &Path, settings: &AppSettings) -> WorkOutcome {
    let mut store = SettingsStore::new(path.to_path_buf());
    let _ = store.load();
    match store.save(settings) {
        Ok(()) => WorkOutcome::Succeeded,
        Err(SettingsError::SchemaTooNew {
            detected,
            supported,
        }) => WorkOutcome::Failed(WorkFailure::FutureSchema {
            detected,
            supported,
        }),
        Err(_) => WorkOutcome::Failed(WorkFailure::SaveFailed),
    }
}

/// One-time copy of the released settings into the isolated Rust preview path.
/// The source is read only, a future schema is rejected, and an existing destination wins.
fn import_legacy_settings(
    source: &Path,
    destination: &Path,
) -> Result<Option<AppSettings>, String> {
    if destination.exists() {
        return Ok(None);
    }
    let before = std::fs::read(source).map_err(|error| error.to_string())?;
    let value = match serde_json::from_slice::<serde_json::Value>(&before) {
        Ok(value) if value.is_object() => value,
        _ => return Ok(None),
    };
    let schema = value.as_object().and_then(|object| {
        object
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("SchemaVersion"))
            .and_then(|(_, value)| value.as_u64())
    });
    if schema.is_some_and(|version| version > u64::from(poe_alarm_settings::CURRENT_SCHEMA_VERSION))
    {
        return Ok(None);
    }
    let settings = match serde_json::from_slice::<AppSettings>(&before) {
        Ok(settings) => settings,
        Err(_) => return Ok(None),
    };
    let after_load = std::fs::read(source).map_err(|error| error.to_string())?;
    if before != after_load {
        return Err("the previous settings file changed while being imported".to_owned());
    }
    let mut destination_store = SettingsStore::new(destination);
    destination_store
        .save(&settings)
        .map_err(|error| error.to_string())?;
    let after_save = std::fs::read(source).map_err(|error| error.to_string())?;
    if before != after_save {
        return Err("the previous settings file was modified during import".to_owned());
    }
    Ok(Some(settings))
}

fn post_completion(window: HWND, completion: BackgroundCompletion) {
    let pointer = Box::into_raw(Box::new(completion));
    if unsafe {
        PostMessageW(
            Some(window),
            WM_APP_WORK_COMPLETED,
            WPARAM(0),
            LPARAM(pointer as isize),
        )
    }
    .is_err()
    {
        unsafe { drop(Box::from_raw(pointer)) };
    }
}

fn post_close_save_completion(window: HWND, outcome: WorkOutcome) {
    let pointer = Box::into_raw(Box::new(outcome));
    if unsafe {
        PostMessageW(
            Some(window),
            WM_APP_CLOSE_SAVE_COMPLETED,
            WPARAM(0),
            LPARAM(pointer as isize),
        )
    }
    .is_err()
    {
        unsafe { drop(Box::from_raw(pointer)) };
    }
}

#[derive(Clone, Copy)]
enum FileDialogKind {
    Screenshot,
    Sound,
}

unsafe fn open_file_dialog(
    owner: HWND,
    kind: FileDialogKind,
    language: UiLanguage,
) -> Option<PathBuf> {
    let (filter_label, pattern, title) = match (kind, language) {
        (FileDialogKind::Screenshot, UiLanguage::SimplifiedChinese) => {
            ("图片文件", "*.png;*.jpg;*.jpeg", "选择要识别的截图")
        }
        (FileDialogKind::Screenshot, UiLanguage::English) => (
            "Image files",
            "*.png;*.jpg;*.jpeg",
            "Choose a screenshot to analyze",
        ),
        (FileDialogKind::Sound, UiLanguage::SimplifiedChinese) => {
            ("声音文件", "*.wav", "选择提醒声音")
        }
        (FileDialogKind::Sound, UiLanguage::English) => {
            ("Wave audio files", "*.wav", "Choose an alert sound")
        }
    };
    let filter = wide_file_filter(filter_label, pattern, language);
    let title = wide(title);
    let mut file = vec![0_u16; 32_768];
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
        return None;
    }
    let length = file
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(file.len());
    (length > 0).then(|| PathBuf::from(String::from_utf16_lossy(&file[..length])))
}

fn wide_file_filter(label: &str, pattern: &str, language: UiLanguage) -> Vec<u16> {
    let all_files = match language {
        UiLanguage::SimplifiedChinese => "所有文件",
        UiLanguage::English => "All files",
    };
    let mut value = Vec::new();
    for part in [label, pattern, all_files, "*.*"] {
        value.extend(part.encode_utf16());
        value.push(0);
    }
    value.push(0);
    value
}

unsafe fn create_combo(parent: HWND, id: u16) -> Result<HWND, String> {
    unsafe {
        create_control(
            parent,
            w!("COMBOBOX"),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WinWindowStyle(CBS_DROPDOWNLIST as u32),
            id,
        )
    }
}

unsafe fn create_button(parent: HWND, id: u16) -> Result<HWND, String> {
    unsafe {
        create_control(
            parent,
            w!("BUTTON"),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WinWindowStyle(BS_OWNERDRAW as u32),
            id,
        )
    }
}

unsafe fn create_checkbox(parent: HWND, id: u16) -> Result<HWND, String> {
    unsafe {
        create_control(
            parent,
            w!("BUTTON"),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WinWindowStyle(BS_AUTOCHECKBOX as u32),
            id,
        )
    }
}

unsafe fn create_edit(parent: HWND, id: u16) -> Result<HWND, String> {
    unsafe {
        create_control(
            parent,
            w!("EDIT"),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WinWindowStyle(ES_AUTOHSCROLL as u32),
            id,
        )
    }
}

unsafe fn create_hidden_edit(parent: HWND, id: u16) -> Result<HWND, String> {
    unsafe { create_control(parent, w!("EDIT"), WS_CHILD, id) }
}

unsafe fn create_multiline_edit(parent: HWND, id: u16) -> Result<HWND, String> {
    unsafe {
        create_control(
            parent,
            w!("EDIT"),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | WinWindowStyle((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32),
            id,
        )
    }
}

unsafe fn create_readonly_multiline_edit(parent: HWND, id: u16) -> Result<HWND, String> {
    unsafe {
        create_control(
            parent,
            w!("EDIT"),
            WS_CHILD
                | WS_VISIBLE
                | WS_VSCROLL
                | WinWindowStyle((ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY) as u32),
            id,
        )
    }
}

unsafe fn create_control(
    parent: HWND,
    class: PCWSTR,
    style: WinWindowStyle,
    id: u16,
) -> Result<HWND, String> {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!(""),
            style,
            0,
            0,
            100,
            30,
            Some(parent),
            Some(control_id(id)),
            None,
            None,
        )
    }
    .map_err(win_error)
}

fn control_id(id: u16) -> HMENU {
    HMENU(id as usize as *mut c_void)
}

unsafe fn set_combo_items(combo: HWND, items: &[&str], selected: usize) {
    unsafe {
        SendMessageW(combo, CB_RESETCONTENT, None, None);
    }
    for item in items {
        let value = wide(item);
        unsafe {
            SendMessageW(
                combo,
                CB_ADDSTRING,
                None,
                Some(LPARAM(value.as_ptr() as isize)),
            );
        }
    }
    if selected < items.len() {
        unsafe {
            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(selected)), None);
        }
    }
}

unsafe fn set_combo_strings(combo: HWND, items: &[String], selected: usize) {
    unsafe {
        SendMessageW(combo, CB_RESETCONTENT, None, None);
    }
    for item in items {
        let value = wide(item);
        unsafe {
            SendMessageW(
                combo,
                CB_ADDSTRING,
                None,
                Some(LPARAM(value.as_ptr() as isize)),
            );
        }
    }
    if selected < items.len() {
        unsafe {
            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(selected)), None);
        }
    }
}

unsafe fn combo_selection(combo: HWND) -> Option<usize> {
    let selected = unsafe { SendMessageW(combo, CB_GETCURSEL, None, None) }.0;
    usize::try_from(selected).ok()
}

unsafe fn set_text(window: HWND, text: &str) {
    let value = wide(text);
    unsafe {
        let _ = SetWindowTextW(window, PCWSTR(value.as_ptr()));
    }
}

unsafe fn get_text(window: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(window) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    let written = unsafe { GetWindowTextW(window, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..written])
}

unsafe fn set_check(window: HWND, checked: bool) {
    unsafe {
        SendMessageW(
            window,
            BM_SETCHECK,
            Some(WPARAM(usize::from(checked))),
            None,
        );
    }
}

unsafe fn is_checked(window: HWND) -> bool {
    unsafe { SendMessageW(window, BM_GETCHECK, None, None) }.0 == 1
}

unsafe fn set_region_fields(controls: &Controls, region: Option<poe_alarm_settings::ScreenRegion>) {
    let values = region.map_or_else(
        || [String::new(), String::new(), String::new(), String::new()],
        |region| {
            [
                region.x.to_string(),
                region.y.to_string(),
                region.width.to_string(),
                region.height.to_string(),
            ]
        },
    );
    unsafe {
        set_text(controls.region_x, &values[0]);
        set_text(controls.region_y, &values[1]);
        set_text(controls.region_width, &values[2]);
        set_text(controls.region_height, &values[3]);
    }
}

fn numeric_values(constraint: Option<&poe_alarm_core::NumericConstraint>) -> (String, String) {
    let Some(constraint) = constraint else {
        return (String::new(), String::new());
    };
    match constraint.mode {
        NumericConstraintMode::Ignore => (String::new(), String::new()),
        NumericConstraintMode::RangeInclusive => (
            constraint
                .minimum
                .map(|value| value.to_string())
                .unwrap_or_default(),
            constraint
                .maximum
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ),
        NumericConstraintMode::AtLeast => (
            constraint
                .minimum
                .map(|value| value.to_string())
                .unwrap_or_default(),
            String::new(),
        ),
        NumericConstraintMode::AtMost => (
            constraint
                .maximum
                .map(|value| value.to_string())
                .unwrap_or_default(),
            String::new(),
        ),
        NumericConstraintMode::Exactly => (
            constraint
                .expected
                .map(|value| value.to_string())
                .unwrap_or_default(),
            String::new(),
        ),
    }
}

fn hotkey_index(value: &str) -> usize {
    if value.eq_ignore_ascii_case("Ctrl+Alt+F10") {
        1
    } else if value.eq_ignore_ascii_case("Alt+F10") {
        2
    } else {
        0
    }
}

fn hotkey_value(selection: Option<usize>) -> &'static str {
    match selection {
        Some(1) => "Ctrl+Alt+F10",
        Some(2) => "Alt+F10",
        _ => "Ctrl+Shift+F10",
    }
}

unsafe fn move_control(control: HWND, x: i32, y: i32, width: i32, height: i32) {
    unsafe {
        let _ = MoveWindow(control, x, y, width, height, true);
    }
}

unsafe fn control_bounds_in_parent(control: HWND, parent: HWND) -> RECT {
    let mut bounds = RECT::default();
    unsafe {
        let _ = GetWindowRect(control, &mut bounds);
    }
    let mut points = [
        POINT {
            x: bounds.left,
            y: bounds.top,
        },
        POINT {
            x: bounds.right,
            y: bounds.bottom,
        },
    ];
    unsafe {
        MapWindowPoints(None, Some(parent), &mut points);
    }
    RECT {
        left: points[0].x,
        top: points[0].y,
        right: points[1].x,
        bottom: points[1].y,
    }
}

const fn ui_font_face(language: UiLanguage, display: bool) -> PCWSTR {
    match language {
        UiLanguage::SimplifiedChinese => FONT_YAHEI_UI,
        UiLanguage::English if display => FONT_SEGOE_DISPLAY,
        UiLanguage::English => FONT_SEGOE_TEXT,
    }
}

unsafe fn create_hud_fonts(dpi: u32, language: UiLanguage) -> (HFONT, HFONT) {
    let face = ui_font_face(language, false);
    (
        unsafe { create_font(dpi, HUD_PRIMARY_FONT_SIZE, FW_SEMIBOLD.0 as i32, face) },
        unsafe { create_font(dpi, HUD_SECONDARY_FONT_SIZE, FW_NORMAL.0 as i32, face) },
    )
}

unsafe fn replace_hud_fonts(paint: &mut HudPaintState, dpi: u32, language: UiLanguage) {
    let (primary, secondary) = unsafe { create_hud_fonts(dpi, language) };
    let old_primary = std::mem::replace(&mut paint.primary_font, primary);
    let old_secondary = std::mem::replace(&mut paint.secondary_font, secondary);
    for font in [old_primary, old_secondary] {
        if !font.0.is_null() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        }
    }
    paint.font_language = language;
}

unsafe fn create_font(dpi: u32, logical_pixels: i32, weight: i32, face: PCWSTR) -> HFONT {
    unsafe {
        CreateFontW(
            -scale(logical_pixels, dpi),
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
            face,
        )
    }
}

unsafe fn draw_label(
    dc: windows::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
) {
    unsafe {
        draw_text(
            dc,
            font,
            theme_color(SEA_GLASS_PALETTE.text_secondary),
            text,
            rect(x, y, x + width, y + 24),
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
    }
}

unsafe fn draw_small_label(
    dc: windows::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
) {
    unsafe {
        draw_text(
            dc,
            font,
            theme_color(SEA_GLASS_PALETTE.text_muted),
            text,
            rect(x, y, x + width, y + 20),
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
    }
}

unsafe fn draw_help(
    dc: windows::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    text: &str,
    x: i32,
    y: i32,
    width: i32,
) {
    unsafe {
        draw_text(
            dc,
            font,
            theme_color(SEA_GLASS_PALETTE.text_muted),
            text,
            rect(x, y, x + width, y + 42),
            DT_LEFT | DT_WORDBREAK,
        );
    }
}

unsafe fn draw_text(
    dc: windows::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    color: COLORREF,
    text: &str,
    mut bounds: RECT,
    format: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    let previous = unsafe { SelectObject(dc, HGDIOBJ(font.0)) };
    // A fresh window paint DC defaults to an opaque text background. The HUD uses white text,
    // so leaving that default in place paints an opaque white rectangle behind every rendered
    // line and makes the glyphs appear completely blank. Keep all shared text drawing explicitly
    // transparent and restore the caller's mode afterwards.
    let previous_background_mode = unsafe { SetBkMode(dc, TRANSPARENT) };
    unsafe {
        SetTextColor(dc, color);
    }
    let mut value = text.encode_utf16().collect::<Vec<_>>();
    unsafe {
        DrawTextW(dc, &mut value, &mut bounds, format);
        if previous_background_mode != 0 {
            SetBkMode(
                dc,
                windows::Win32::Graphics::Gdi::BACKGROUND_MODE(previous_background_mode as u32),
            );
        }
        SelectObject(dc, previous);
    }
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

unsafe fn draw_rounded_surface(dc: HDC, bounds: RECT, brush: HBRUSH, corner_radius: i32) {
    let diameter = corner_radius.max(1) * 2;
    let region = unsafe {
        CreateRoundRectRgn(
            bounds.left,
            bounds.top,
            bounds.right,
            bounds.bottom,
            diameter,
            diameter,
        )
    };
    if region.0.is_null() {
        return;
    }
    unsafe {
        let _ = FillRgn(dc, region, brush);
        let _ = DeleteObject(HGDIOBJ(region.0));
    }
}

unsafe fn draw_glass_card(
    dc: HDC,
    bounds: RECT,
    dpi: u32,
    fill: HBRUSH,
    border: HBRUSH,
    shadow: HBRUSH,
) {
    let one = scale(1, dpi).max(1);
    let shadow_offset = scale(2, dpi).max(1);
    let radius = scale(13, dpi).max(3);
    let shadow_bounds = RECT {
        left: bounds.left,
        top: bounds.top + shadow_offset,
        right: bounds.right,
        bottom: bounds.bottom + shadow_offset,
    };
    unsafe {
        draw_rounded_surface(dc, shadow_bounds, shadow, radius);
        draw_rounded_surface(dc, bounds, border, radius);
        draw_rounded_surface(
            dc,
            RECT {
                left: bounds.left + one,
                top: bounds.top + one,
                right: bounds.right - one,
                bottom: bounds.bottom - one,
            },
            fill,
            (radius - one).max(2),
        );
    }
}

fn scale(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn apply_native_window_theme(window: HWND) {
    let dark_mode = 0_i32;
    unsafe {
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark_mode as *const i32).cast(),
            size_of::<i32>() as u32,
        );
        let corner_preference: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&corner_preference as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
            size_of_val(&corner_preference) as u32,
        );
        let backdrop: DWM_SYSTEMBACKDROP_TYPE = DWMSBT_MAINWINDOW;
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&backdrop as *const DWM_SYSTEMBACKDROP_TYPE).cast(),
            size_of_val(&backdrop) as u32,
        );
        // Never extend DWM glass into this GDI client area. Classic EDIT, COMBOBOX and
        // owner-drawn controls do not publish a valid premultiplied alpha channel there; DWM
        // consequently interprets dark glyph pixels as transparency and drops strokes and
        // borders. The client stays fully opaque and paints the sea-glass look explicitly.
    }
}

const fn theme_color(color: Rgb) -> COLORREF {
    COLORREF(color.colorref())
}

fn win_error(error: windows::core::Error) -> String {
    format!(
        "HRESULT 0x{:08X}: {}",
        error.code().0 as u32,
        error.message()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_scaling_is_stable_at_common_values() {
        assert_eq!(scale(100, 96), 100);
        assert_eq!(scale(100, 144), 150);
        assert_eq!(scale(100, 192), 200);
    }

    #[test]
    fn match_option_layer_appears_only_for_real_alternatives() {
        assert!(!shows_match_option_layer(0));
        assert!(!shows_match_option_layer(1));
        assert!(shows_match_option_layer(2));
        assert!(shows_match_option_layer(8));
    }

    #[test]
    fn configuration_window_fits_common_dpi_and_work_areas() {
        for physical_dpi in [96, 120, 144] {
            for (work_width, work_height) in [(1024, 768), (1920, 1080)] {
                let non_client = NonClientSize {
                    width: scale(16, physical_dpi),
                    height: scale(39, physical_dpi),
                };
                let metrics = fit_window_metrics(physical_dpi, work_width, work_height, non_client);

                assert!(metrics.layout_dpi <= physical_dpi);
                assert!(metrics.outer_width <= work_width);
                assert!(metrics.outer_height <= work_height);
                assert!(scale(RIGHTMOST_CONTROL_EDGE, metrics.layout_dpi) <= metrics.client_width);
                assert!(
                    scale(BOTTOMMOST_CONTROL_EDGE, metrics.layout_dpi) <= metrics.client_height
                );
            }
        }
    }

    #[test]
    fn window_origin_is_clamped_to_the_selected_work_area() {
        let work = RECT {
            left: -1920,
            top: 40,
            right: 0,
            bottom: 1080,
        };
        let metrics = fit_window_metrics(
            144,
            work.right - work.left,
            work.bottom - work.top,
            NonClientSize {
                width: 24,
                height: 59,
            },
        );
        let (x, y) = clamp_window_origin(i32::MIN, i32::MAX, work, metrics);
        assert_eq!(x, work.left);
        assert_eq!(y, work.bottom - metrics.outer_height);
    }

    #[test]
    fn worker_message_codes_round_trip() {
        for operation in [
            Operation::StartMonitoring,
            Operation::StopMonitoring,
            Operation::TestScreenshot,
            Operation::SaveSettings,
        ] {
            assert_eq!(
                Operation::from_message_code(operation.message_code()),
                Some(operation)
            );
        }
        assert_eq!(Operation::from_message_code(99), None);
    }

    #[test]
    fn numeric_fields_follow_constraint_mode() {
        let range = poe_alarm_core::NumericConstraint::range(3, 4);
        assert_eq!(numeric_values(Some(&range)), ("3".into(), "4".into()));
        let exact = poe_alarm_core::NumericConstraint::exactly(7);
        assert_eq!(numeric_values(Some(&exact)), ("7".into(), String::new()));
    }

    #[test]
    fn hud_visibility_matches_released_preference_and_alert_behavior() {
        assert!(hud_should_be_visible(MonitorStatus::Ready, true));
        assert!(hud_should_be_visible(MonitorStatus::Monitoring, true));
        assert!(!hud_should_be_visible(MonitorStatus::Monitoring, false));
        assert!(!hud_should_be_visible(MonitorStatus::MatchFound, true));
        assert!(!hud_should_be_visible(MonitorStatus::MatchFound, false));
    }

    #[test]
    fn hud_summary_and_timer_are_compact_and_stable() {
        let mut settings = AppSettings::default();
        settings.profiles.poe1.selected_rules_mut().target_affix =
            "  +#% to\r\nCritical Hit Chance  ".to_owned();
        assert_eq!(
            active_rule_summary(&settings, UiLanguage::English),
            "+#% to Critical Hit Chance"
        );
        assert_eq!(format_hud_elapsed(Duration::from_secs(59)), "00:59");
        assert_eq!(format_hud_elapsed(Duration::from_secs(3_661)), "01:01:01");
    }

    #[test]
    fn compiled_ui_bindings_drive_the_live_hud_snapshot() {
        let base = AppSettings::default();
        let ui = CompiledUiBindings {
            hot_keys: poe_alarm_platform_win::HotKeyConfig::default(),
            keep_hud_visible: false,
            hud_placement: poe_alarm_settings::HudPlacement {
                monitor_device_name: Some("DISPLAY2".to_owned()),
                relative_x: Some(0.25),
                relative_y: Some(0.75),
            },
            allow_overlay_capture: false,
        };
        let effective = compiled_hud_settings(&base, &ui, -20, 40, 600, 800);
        assert!(!effective.keep_hud_visible);
        assert!(!effective.allow_overlay_capture);
        assert_eq!(effective.hud_placement, ui.hud_placement);
        assert_eq!(
            effective.selected_profile().capture_region,
            Some(ScreenRegion::new(-20, 40, 600, 800))
        );
    }

    #[test]
    fn start_hot_key_never_toggles_an_active_or_pending_monitor() {
        assert!(start_hot_key_requests_start(false, false));
        assert!(!start_hot_key_requests_start(true, false));
        assert!(!start_hot_key_requests_start(false, true));
        assert!(!start_hot_key_requests_start(true, true));
        assert!(should_minimize_after_monitoring_started(false));
        assert!(!should_minimize_after_monitoring_started(true));
    }

    #[test]
    fn close_save_gate_preserves_dirty_changes_but_never_overwrites_future_schema() {
        let original = AppSettings::default();
        let mut edited = original.clone();
        edited.keep_hud_visible = false;
        assert!(!close_needs_save(None, false, &original, &original));
        assert!(close_needs_save(None, true, &original, &original));
        assert!(close_needs_save(None, false, &original, &edited));
        assert!(!close_needs_save(Some(999), true, &original, &edited));
    }

    #[test]
    fn preview_save_worker_uses_real_settings_store_contract() {
        let directory = std::env::temp_dir().join(format!(
            "poe-alarm-native-ui-save-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("settings.json");
        let mut settings = AppSettings::default();
        settings.profiles.poe1.selected_rules_mut().target_affix =
            "#% increased Attack Speed".into();
        let outcome = save_settings(&path, &settings);
        assert_eq!(outcome, WorkOutcome::Succeeded, "{outcome:?}");
        let mut store = SettingsStore::new(&path);
        assert_eq!(
            store.load().profiles.poe1.selected_rules().target_affix,
            "#% increased Attack Speed"
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&directory);
    }

    fn import_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "poe-alarm-native-ui-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn one_time_import_preserves_source_bytes_and_all_profile_settings() {
        let directory = import_directory("import-test");
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("release.json");
        let destination = directory.join("preview").join("settings.json");
        let mut settings = AppSettings::default();
        settings.profiles.poe1.selected_rules_mut().target_affix = "poe1 target".to_owned();
        settings.profiles.poe1.capture_region = Some(ScreenRegion::new(10, 20, 300, 400));
        settings.profiles.poe2.selected_rules_mut().target_affix = "poe2 target".to_owned();
        settings.profiles.poe2.capture_region = Some(ScreenRegion::new(-12, 8, 500, 700));
        settings.selected_game_profile = poe_alarm_settings::GameProfile::Poe2;
        let mut source_store = SettingsStore::new(&source);
        source_store.save(&settings).unwrap();
        let before = std::fs::read(&source).unwrap();

        let imported = import_legacy_settings(&source, &destination)
            .unwrap()
            .expect("first import should succeed");
        assert_eq!(imported.profiles.poe1, settings.profiles.poe1);
        assert_eq!(imported.profiles.poe2, settings.profiles.poe2);
        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert!(destination.is_file());

        assert!(
            import_legacy_settings(&source, &destination)
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read(&source).unwrap(), before);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn future_schema_import_is_rejected_without_writing_or_changing_source() {
        let directory = import_directory("future-import-test");
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("release.json");
        let destination = directory.join("preview").join("settings.json");
        let bytes = br#"{"SchemaVersion":999,"sentinel":"keep exactly"}"#;
        std::fs::write(&source, bytes).unwrap();
        assert!(
            import_legacy_settings(&source, &destination)
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        assert!(!destination.exists());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
