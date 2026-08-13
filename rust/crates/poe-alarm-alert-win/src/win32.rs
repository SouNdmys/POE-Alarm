use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use poe_alarm_platform_win::{LoopingWavePlayer, MouseButtons, RectI};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush,
    DEFAULT_CHARSET, DT_CENTER, DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK,
    DeleteObject, DrawTextW, EndPaint, FW_NORMAL, FW_SEMIBOLD, FillRect, FrameRect,
    GetMonitorInfoW, HBRUSH, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT, UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetAsyncKeyState, IsWindowEnabled, VK_CONTROL, VK_F12, VK_LBUTTON, VK_MBUTTON,
    VK_RBUTTON, VK_SHIFT, VK_XBUTTON1, VK_XBUTTON2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_PUSHBUTTON, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, GA_ROOT, GWL_EXSTYLE, GWLP_USERDATA,
    GetAncestor, GetClientRect, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect,
    HMENU, HTCLIENT, HWND_TOPMOST, IsWindow, IsWindowVisible, KillTimer, MA_NOACTIVATE, MSG,
    MoveWindow, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, RegisterClassExW,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetTimer, SetWindowDisplayAffinity,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_CREATE, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCCREATE,
    WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_CHILD, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE, WindowFromPoint,
};
use windows::core::{PCWSTR, w};

use crate::protocol::{AlertLifecycle, TriggerDecision};
use crate::service::{
    RuntimePhase, RuntimeShared, ServiceOwnership, TriggerCommand, WorkerCommand,
};
use crate::{AlertEvent, AlertFailure, AlertFailureKind, AlertId, AlertServiceConfig, AlertText};

const CLASS_NAME: PCWSTR = w!("PoeAlarmNativeBlockingAlert");
const WINDOW_TITLE: PCWSTR = w!("POE Alarm");
const BUTTON_ID: u16 = 0x504F;
const POLL_TIMER_ID: usize = 0x414C_5254;
const POLL_INTERVAL_MS: u32 = 20;
const WM_ALERT_COMMANDS: u32 = WM_APP + 0x451;
const WM_ALERT_ACKNOWLEDGE: u32 = WM_APP + 0x452;
const WM_ALERT_STOP: u32 = WM_APP + 0x453;
const CARD_WIDTH: i32 = 720;
const CARD_HEIGHT: i32 = 360;
const CARD_MARGIN: i32 = 28;
const BUTTON_WIDTH: i32 = 360;
const BUTTON_HEIGHT: i32 = 58;

static CLASS_READY: OnceLock<Result<(), AlertFailure>> = OnceLock::new();

pub(crate) fn spawn_worker(
    config: AlertServiceConfig,
    commands: mpsc::Receiver<WorkerCommand>,
    events: mpsc::Sender<AlertEvent>,
    warnings: mpsc::Sender<AlertFailure>,
    runtime: Arc<RuntimeShared>,
    ready: mpsc::SyncSender<Result<(), AlertFailure>>,
    ownership: ServiceOwnership,
) -> Result<JoinHandle<()>, AlertFailure> {
    thread::Builder::new()
        .name("poe-alarm-red-alert".to_owned())
        .spawn(move || {
            worker_thread(config, commands, events.clone(), warnings, runtime, ready);
            drop(ownership);
            let _ = events.send(AlertEvent::Stopped);
        })
        .map_err(|error| {
            AlertFailure::new(
                AlertFailureKind::ThreadStart,
                format!("could not start native alert thread: {error}"),
            )
        })
}

pub(crate) fn wake_commands(thread_id: u32) -> Result<(), AlertFailure> {
    post_thread(thread_id, WM_ALERT_COMMANDS, "dispatch alert trigger")
}

pub(crate) fn wake_acknowledge(thread_id: u32) -> Result<(), AlertFailure> {
    post_thread(
        thread_id,
        WM_ALERT_ACKNOWLEDGE,
        "dispatch alert acknowledgement",
    )
}

pub(crate) fn wake_stop(thread_id: u32) -> Result<(), AlertFailure> {
    post_thread(thread_id, WM_ALERT_STOP, "dispatch alert shutdown")
}

fn post_thread(thread_id: u32, message: u32, operation: &'static str) -> Result<(), AlertFailure> {
    if thread_id == 0 {
        return Err(AlertFailure::new(
            AlertFailureKind::Dispatch,
            format!("{operation}: native alert thread id is unavailable"),
        ));
    }
    // SAFETY: thread_id is published only after the worker message queue exists.
    unsafe { PostThreadMessageW(thread_id, message, WPARAM(0), LPARAM(0)) }
        .map_err(|error| windows_failure(AlertFailureKind::Dispatch, operation, error))
}

fn worker_thread(
    config: AlertServiceConfig,
    commands: mpsc::Receiver<WorkerCommand>,
    events: mpsc::Sender<AlertEvent>,
    warnings: mpsc::Sender<AlertFailure>,
    runtime: Arc<RuntimeShared>,
    ready: mpsc::SyncSender<Result<(), AlertFailure>>,
) {
    let mut queue_message = MSG::default();
    // SAFETY: This creates the current thread's message queue before publishing its id.
    let _ = unsafe { PeekMessageW(&mut queue_message, None, 0, 0, PM_NOREMOVE) };
    // SAFETY: GetCurrentThreadId has no preconditions.
    runtime.set_native_thread_id(unsafe { GetCurrentThreadId() });

    #[cfg(test)]
    let force_display_affinity_failure = config.force_display_affinity_failure;
    #[cfg(not(test))]
    let force_display_affinity_failure = false;
    let overlay =
        NativeOverlay::create(config.allow_overlay_capture, force_display_affinity_failure);
    let overlay = match overlay {
        Ok((overlay, affinity_warning)) => {
            if let Some(warning) = affinity_warning {
                report_degradation(&warning);
                let _ = warnings.send(warning);
            }
            overlay
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            runtime.mark_stopped();
            return;
        }
    };
    #[cfg(test)]
    let stall_first_presentation = config.stall_first_presentation;
    #[cfg(test)]
    let physical_buttons_override = config.physical_buttons_override;
    let player = LoopingWavePlayer::new(config.wave);
    let mut worker = AlertWorker {
        overlay,
        player,
        commands,
        events,
        warnings,
        runtime,
        lifecycle: AlertLifecycle::default(),
        active: None,
        pending_presentation: None,
        #[cfg(test)]
        stall_first_presentation,
        #[cfg(test)]
        physical_buttons_override,
    };
    let _ = ready.send(Ok(()));
    if worker.runtime.stop_requested() {
        worker.shutdown();
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: Standard message loop owned by this native alert thread.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        match message.message {
            WM_ALERT_COMMANDS => {
                worker.repair_expired_presentation();
                worker.drain_commands();
            }
            WM_ALERT_ACKNOWLEDGE => worker.request_acknowledgement(),
            WM_ALERT_STOP => break,
            _ => unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            },
        }
        worker.poll_window_signals();
        worker.advance_acknowledgement();
        worker.repair_expired_presentation();
        if worker.runtime.stop_requested() {
            break;
        }
    }
    worker.shutdown();
}

struct ActiveAlert {
    alert_id: AlertId,
    acknowledgement_started: Option<Instant>,
}

struct AlertWorker {
    overlay: NativeOverlay,
    player: LoopingWavePlayer,
    commands: mpsc::Receiver<WorkerCommand>,
    events: mpsc::Sender<AlertEvent>,
    warnings: mpsc::Sender<AlertFailure>,
    runtime: Arc<RuntimeShared>,
    lifecycle: AlertLifecycle,
    active: Option<ActiveAlert>,
    pending_presentation: Option<AlertId>,
    #[cfg(test)]
    stall_first_presentation: bool,
    #[cfg(test)]
    physical_buttons_override: Option<MouseButtons>,
}

impl AlertWorker {
    fn drain_commands(&mut self) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                WorkerCommand::Trigger(command) => self.present(command),
            }
        }
    }

    fn present(&mut self, command: TriggerCommand) {
        if !self
            .runtime
            .is_current(RuntimePhase::Presenting, command.alert_id)
        {
            command.handoff.fail_open();
            return;
        }
        match self.lifecycle.begin_trigger(command.alert_id) {
            TriggerDecision::Accepted => {}
            TriggerDecision::Duplicate(_) | TriggerDecision::Stopped => {
                self.fail_presentation(
                    command.alert_id,
                    &command.handoff,
                    AlertFailure::new(
                        AlertFailureKind::InternalProtocol,
                        "native alert worker received a trigger in a non-idle phase",
                    ),
                );
                return;
            }
        }
        self.pending_presentation = Some(command.alert_id);

        #[cfg(test)]
        if std::mem::take(&mut self.stall_first_presentation) {
            // The message pump keeps running while the trigger intentionally
            // remains unpresented, so the independent handoff watchdog can be tested.
            return;
        }

        if let Err(error) = self
            .overlay
            .present(&command.request.text, command.request.anchor_region)
        {
            self.fail_presentation(command.alert_id, &command.handoff, error);
            return;
        }
        if !self.lifecycle.mark_presentation_verified(command.alert_id) {
            self.overlay.hide();
            self.fail_presentation(
                command.alert_id,
                &command.handoff,
                AlertFailure::new(
                    AlertFailureKind::InternalProtocol,
                    "presentation verification could not advance the alert lifecycle",
                ),
            );
            return;
        }

        // This is the safety hand-off: verification happened above, and the
        // timeout path competes for the same GuardHandoff mutex.
        if !command.handoff.transfer_to_visible_overlay() {
            self.overlay.hide();
            let _ = self.lifecycle.fail(command.alert_id);
            self.pending_presentation = None;
            return;
        }
        if !self.lifecycle.mark_guard_transferred(command.alert_id)
            || !self
                .runtime
                .mark_blocking(command.alert_id, &command.handoff)
        {
            self.overlay.hide();
            let _ = self.lifecycle.fail(command.alert_id);
            self.pending_presentation = None;
            return;
        }
        self.pending_presentation = None;

        match self.player.start() {
            Ok(()) => {}
            Err(error) => {
                let _ = self.events.send(AlertEvent::SoundFailed {
                    alert_id: command.alert_id,
                    failure: AlertFailure::new(AlertFailureKind::SoundPlayback, error.to_string()),
                });
            }
        }
        self.active = Some(ActiveAlert {
            alert_id: command.alert_id,
            acknowledgement_started: None,
        });
        let _ = self.events.send(AlertEvent::Presented {
            alert_id: command.alert_id,
        });
    }

    fn fail_presentation(
        &mut self,
        alert_id: AlertId,
        handoff: &Arc<crate::service::GuardHandoff>,
        failure: AlertFailure,
    ) {
        handoff.fail_open();
        self.overlay.hide();
        let _ = self.lifecycle.fail(alert_id);
        if self.pending_presentation == Some(alert_id) {
            self.pending_presentation = None;
        }
        if self.runtime.fail_pending(alert_id, handoff) {
            let _ = self.events.send(AlertEvent::Failed { alert_id, failure });
        }
    }

    fn poll_window_signals(&mut self) {
        if self.overlay.take_topology_refresh_request()
            && self.active.is_some()
            && let Err(warning) = self.overlay.refresh_topology()
        {
            report_degradation(&warning);
            let _ = self.warnings.send(warning);
        }
        if self.overlay.take_acknowledgement_request() {
            self.request_acknowledgement();
        }
    }

    fn request_acknowledgement(&mut self) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.acknowledgement_started.is_some()
            || !self.lifecycle.begin_acknowledgement(active.alert_id)
            || !self.runtime.begin_acknowledgement(active.alert_id)
        {
            return;
        }
        let _ = self.player.stop();
        active.acknowledgement_started = Some(Instant::now());
        self.overlay.set_acknowledgement_pending();
    }

    fn advance_acknowledgement(&mut self) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let Some(started) = active.acknowledgement_started else {
            return;
        };
        #[cfg(test)]
        let physical_buttons = self
            .physical_buttons_override
            .unwrap_or_else(read_physical_buttons);
        #[cfg(not(test))]
        let physical_buttons = read_physical_buttons();
        if !self.lifecycle.acknowledgement_ready(
            active.alert_id,
            started.elapsed(),
            physical_buttons,
        ) {
            return;
        }
        let alert_id = active.alert_id;
        self.overlay.hide();
        if self.lifecycle.finish_acknowledgement(alert_id)
            && self.runtime.finish_acknowledgement(alert_id)
        {
            self.active = None;
            let _ = self.events.send(AlertEvent::Acknowledged { alert_id });
        }
    }

    fn repair_expired_presentation(&mut self) {
        if let Some(alert_id) = self.pending_presentation
            && !self.runtime.is_current(RuntimePhase::Presenting, alert_id)
        {
            // A test-stalled or genuinely delayed presentation was failed by
            // the watchdog. Reset the thread-local protocol before a later trigger.
            let _ = self.lifecycle.fail(alert_id);
            self.pending_presentation = None;
            self.overlay.hide();
        }
    }

    fn shutdown(&mut self) {
        let _ = self.player.stop();
        self.active = None;
        self.overlay.hide();
        self.lifecycle.stop();
        self.runtime.mark_stopped();
    }
}

struct WindowContext {
    native_ownership_claimed: Arc<AtomicBool>,
    button: HWND,
    text: AlertText,
    card: RECT,
    acknowledgement_requested: bool,
    acknowledgement_chord_was_down: bool,
    topology_refresh_requested: bool,
}

impl WindowContext {
    fn placeholder(native_ownership_claimed: Arc<AtomicBool>) -> Self {
        Self {
            native_ownership_claimed,
            button: HWND::default(),
            text: AlertText {
                title: String::new(),
                detail: String::new(),
                button: String::new(),
            },
            card: RECT::default(),
            acknowledgement_requested: false,
            acknowledgement_chord_was_down: false,
            topology_refresh_requested: false,
        }
    }
}

struct NativeOverlay {
    hwnd: HWND,
    anchor: Option<RectI>,
    #[cfg(test)]
    topology_query_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct OverlayGeometry {
    card: RECT,
    card_width: i32,
}

impl OverlayGeometry {
    fn for_topology(desktop: RECT, monitor: RECT) -> Self {
        let monitor_width = monitor.right - monitor.left;
        let monitor_height = monitor.bottom - monitor.top;
        let card_width = CARD_WIDTH.min((monitor_width - CARD_MARGIN * 2).max(320));
        let card_height = CARD_HEIGHT.min((monitor_height - CARD_MARGIN * 2).max(240));
        let card_left = monitor.left - desktop.left + (monitor_width - card_width) / 2;
        let card_top = monitor.top - desktop.top + (monitor_height - card_height) / 2;
        Self {
            card: RECT {
                left: card_left,
                top: card_top,
                right: card_left + card_width,
                bottom: card_top + card_height,
            },
            card_width,
        }
    }
}

impl NativeOverlay {
    fn create(
        allow_overlay_capture: bool,
        force_display_affinity_failure: bool,
    ) -> Result<(Self, Option<AlertFailure>), AlertFailure> {
        ensure_class()?;
        let module = unsafe { GetModuleHandleW(None) }.map_err(|error| {
            windows_failure(AlertFailureKind::WindowCreation, "GetModuleHandleW", error)
        })?;
        let native_ownership_claimed = Arc::new(AtomicBool::new(false));
        let context = Box::new(WindowContext::placeholder(Arc::clone(
            &native_ownership_claimed,
        )));
        let context_pointer = Box::into_raw(context);
        let extended = WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0);
        // SAFETY: Class is registered, dimensions are validated, and the
        // WindowContext pointer is reclaimed by WM_NCDESTROY or below on failure.
        let created = unsafe {
            CreateWindowExW(
                extended,
                CLASS_NAME,
                WINDOW_TITLE,
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(module.into()),
                Some(context_pointer.cast()),
            )
        };
        let hwnd = match created {
            Ok(hwnd) => hwnd,
            Err(error) => {
                // WM_NCCREATE explicitly publishes native ownership. Before
                // that point CreateWindowExW cannot send WM_NCDESTROY, so the
                // caller remains responsible for reclaiming the context.
                if !native_ownership_claimed.load(Ordering::Acquire) {
                    unsafe { drop(Box::from_raw(context_pointer)) };
                }
                return Err(windows_failure(
                    AlertFailureKind::WindowCreation,
                    "CreateWindowExW(blocking alert)",
                    error,
                ));
            }
        };
        let affinity = if allow_overlay_capture {
            WDA_NONE
        } else {
            WDA_EXCLUDEFROMCAPTURE
        };
        let affinity_warning = if force_display_affinity_failure {
            Some(AlertFailure::new(
                AlertFailureKind::DisplayAffinity,
                "SetWindowDisplayAffinity(blocking alert) was forced to fail in a test",
            ))
        } else {
            unsafe { SetWindowDisplayAffinity(hwnd, affinity) }
                .err()
                .map(|error| {
                    windows_failure(
                        AlertFailureKind::DisplayAffinity,
                        "SetWindowDisplayAffinity(blocking alert)",
                        error,
                    )
                })
        };
        // SAFETY: hwnd is a live thread-owned window; the timer only polls the
        // Ctrl+Shift+F12 acknowledgement chord.
        if unsafe { SetTimer(Some(hwnd), POLL_TIMER_ID, POLL_INTERVAL_MS, None) } == 0 {
            let error = windows::core::Error::from_win32();
            let _ = unsafe { DestroyWindow(hwnd) };
            return Err(windows_failure(
                AlertFailureKind::WindowCreation,
                "SetTimer(blocking alert)",
                error,
            ));
        }
        Ok((
            Self {
                hwnd,
                anchor: None,
                #[cfg(test)]
                topology_query_count: 0,
            },
            affinity_warning,
        ))
    }

    fn present(&mut self, text: &AlertText, anchor: Option<RectI>) -> Result<(), AlertFailure> {
        self.anchor = anchor;
        let context = self.context_mut()?;
        context.text = text.clone();
        context.acknowledgement_requested = false;
        context.acknowledgement_chord_was_down = acknowledgement_chord_is_down();
        context.topology_refresh_requested = false;
        let button_text = wide(&text.button);
        unsafe { SetWindowTextW(context.button, PCWSTR(button_text.as_ptr())) }.map_err(
            |error| {
                windows_failure(
                    AlertFailureKind::WindowPresentation,
                    "SetWindowTextW(alert acknowledgement)",
                    error,
                )
            },
        )?;
        self.refresh_topology()
    }

    fn refresh_topology(&mut self) -> Result<(), AlertFailure> {
        #[cfg(test)]
        {
            self.topology_query_count += 1;
        }
        let desktop = virtual_desktop()?;
        let monitor = monitor_bounds(self.anchor)?;
        let geometry = OverlayGeometry::for_topology(desktop, monitor);
        let context = self.context_mut()?;
        context.card = geometry.card;
        context.topology_refresh_requested = false;
        let button_width = BUTTON_WIDTH.min(geometry.card_width - CARD_MARGIN * 2);
        let button_x = geometry.card.left + (geometry.card_width - button_width) / 2;
        let button_y = geometry.card.bottom - CARD_MARGIN - BUTTON_HEIGHT;
        unsafe {
            MoveWindow(
                context.button,
                button_x,
                button_y,
                button_width,
                BUTTON_HEIGHT,
                true,
            )
        }
        .map_err(|error| {
            windows_failure(
                AlertFailureKind::WindowPresentation,
                "MoveWindow(alert acknowledgement)",
                error,
            )
        })?;
        unsafe {
            let _ = EnableWindow(context.button, true);
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                desktop.left,
                desktop.top,
                desktop.right - desktop.left,
                desktop.bottom - desktop.top,
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            )
            .map_err(|error| {
                windows_failure(
                    AlertFailureKind::WindowPresentation,
                    "SetWindowPos(show blocking alert)",
                    error,
                )
            })?;
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            if !UpdateWindow(self.hwnd).as_bool() {
                return Err(windows_failure(
                    AlertFailureKind::WindowPresentation,
                    "UpdateWindow(blocking alert)",
                    windows::core::Error::from_win32(),
                ));
            }
        }
        self.verify_blocking(desktop, geometry.card)
    }

    fn verify_blocking(&self, desktop: RECT, card: RECT) -> Result<(), AlertFailure> {
        if !unsafe { IsWindowVisible(self.hwnd) }.as_bool()
            || !unsafe { IsWindowEnabled(self.hwnd) }.as_bool()
        {
            return Err(AlertFailure::new(
                AlertFailureKind::HitTestVerification,
                "blocking alert HWND is not visible and enabled",
            ));
        }
        let extended = unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) } as u32;
        if extended & WS_EX_NOACTIVATE.0 == 0
            || extended & WS_EX_TOOLWINDOW.0 == 0
            || extended & WS_EX_TOPMOST.0 == 0
            || extended & 0x20 != 0
        {
            return Err(AlertFailure::new(
                AlertFailureKind::HitTestVerification,
                format!("blocking alert extended styles are unsafe: 0x{extended:08X}"),
            ));
        }
        let mut observed = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut observed) }.map_err(|error| {
            windows_failure(
                AlertFailureKind::HitTestVerification,
                "GetWindowRect(blocking alert)",
                error,
            )
        })?;
        if observed != desktop {
            return Err(AlertFailure::new(
                AlertFailureKind::HitTestVerification,
                "blocking alert does not cover the complete virtual desktop",
            ));
        }
        let point = POINT {
            x: desktop.left + (card.left + card.right) / 2,
            y: desktop.top + (card.top + card.bottom) / 2,
        };
        let target = unsafe { WindowFromPoint(point) };
        let root = unsafe { GetAncestor(target, GA_ROOT) };
        if root != self.hwnd {
            return Err(AlertFailure::new(
                AlertFailureKind::HitTestVerification,
                "the visible alert did not own hit testing at its central card",
            ));
        }
        Ok(())
    }

    fn take_acknowledgement_request(&mut self) -> bool {
        self.context_mut().is_ok_and(|context| {
            let requested = context.acknowledgement_requested;
            context.acknowledgement_requested = false;
            requested
        })
    }

    fn take_topology_refresh_request(&mut self) -> bool {
        self.context_mut().is_ok_and(|context| {
            let requested = context.topology_refresh_requested;
            context.topology_refresh_requested = false;
            requested
        })
    }

    fn set_acknowledgement_pending(&mut self) {
        if let Ok(context) = self.context_mut() {
            let _ = unsafe { EnableWindow(context.button, false) };
        }
    }

    fn hide(&mut self) {
        if !unsafe { IsWindow(Some(self.hwnd)) }.as_bool() {
            return;
        }
        if let Ok(context) = self.context_mut() {
            context.acknowledgement_requested = false;
            context.acknowledgement_chord_was_down = false;
            let _ = unsafe { EnableWindow(context.button, true) };
        }
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    fn context_mut(&mut self) -> Result<&mut WindowContext, AlertFailure> {
        let pointer = unsafe { GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) } as *mut WindowContext;
        if pointer.is_null() {
            Err(AlertFailure::new(
                AlertFailureKind::InternalProtocol,
                "blocking alert window context is unavailable",
            ))
        } else {
            Ok(unsafe { &mut *pointer })
        }
    }
}

impl Drop for NativeOverlay {
    fn drop(&mut self) {
        if unsafe { IsWindow(Some(self.hwnd)) }.as_bool() {
            let _ = unsafe { KillTimer(Some(self.hwnd), POLL_TIMER_ID) };
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

fn ensure_class() -> Result<(), AlertFailure> {
    CLASS_READY
        .get_or_init(|| {
            let module = unsafe { GetModuleHandleW(None) }.map_err(|error| {
                windows_failure(AlertFailureKind::WindowCreation, "GetModuleHandleW", error)
            })?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: module.into(),
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            if unsafe { RegisterClassExW(&class) } == 0 {
                return Err(windows_failure(
                    AlertFailureKind::WindowCreation,
                    "RegisterClassExW(blocking alert)",
                    windows::core::Error::from_win32(),
                ));
            }
            Ok(())
        })
        .clone()
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        let context = create.lpCreateParams.cast::<WindowContext>();
        if context.is_null() {
            return LRESULT(0);
        }
        unsafe { &*context }
            .native_ownership_claimed
            .store(true, Ordering::Release);
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, context as isize) };
        return LRESULT(1);
    }
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowContext;
    if message == WM_NCDESTROY {
        let result = unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
        if !pointer.is_null() {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(pointer));
            }
        }
        return result;
    }
    if pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let context = unsafe { &mut *pointer };
    match message {
        WM_CREATE => match unsafe { create_acknowledgement_button(hwnd) } {
            Ok(button) => {
                context.button = button;
                LRESULT(0)
            }
            Err(_) => LRESULT(-1),
        },
        WM_COMMAND => {
            let identifier = (wparam.0 & 0xFFFF) as u16;
            let notification = ((wparam.0 >> 16) & 0xFFFF) as u16;
            if identifier == BUTTON_ID && notification == BN_CLICKED as u16 {
                context.acknowledgement_requested = true;
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == POLL_TIMER_ID => {
            let chord_down = acknowledgement_chord_is_down();
            if chord_down && !context.acknowledgement_chord_was_down {
                context.acknowledgement_requested = true;
            }
            context.acknowledgement_chord_was_down = chord_down;
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            context.topology_refresh_requested = true;
            LRESULT(0)
        }
        WM_PAINT => {
            unsafe { paint(hwnd, context) };
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_CLOSE => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn acknowledgement_chord_is_down() -> bool {
    acknowledgement_chord(
        unsafe { GetAsyncKeyState(i32::from(VK_F12.0)) } & i16::MIN != 0,
        unsafe { GetAsyncKeyState(i32::from(VK_CONTROL.0)) } & i16::MIN != 0,
        unsafe { GetAsyncKeyState(i32::from(VK_SHIFT.0)) } & i16::MIN != 0,
    )
}

const fn acknowledgement_chord(f12: bool, control: bool, shift: bool) -> bool {
    f12 && control && shift
}

unsafe fn create_acknowledgement_button(parent: HWND) -> Result<HWND, windows::core::Error> {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("BUTTON"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
            0,
            0,
            BUTTON_WIDTH,
            BUTTON_HEIGHT,
            Some(parent),
            Some(control_id(BUTTON_ID)),
            None,
            None,
        )
    }
}

unsafe fn paint(hwnd: HWND, context: &WindowContext) {
    let mut paint = PAINTSTRUCT::default();
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    let red = Brush::new(rgb(165, 18, 28));
    let card = Brush::new(rgb(40, 18, 22));
    let border = Brush::new(rgb(255, 218, 225));
    unsafe {
        FillRect(dc, &client, red.0);
        FillRect(dc, &context.card, card.0);
        FrameRect(dc, &context.card, border.0);
        SetBkMode(dc, TRANSPARENT);
    }
    let title_font = Font::new(34, FW_SEMIBOLD.0 as i32);
    let detail_font = Font::new(21, FW_NORMAL.0 as i32);
    let horizontal = 42;
    unsafe {
        draw_text(
            dc,
            title_font.0,
            rgb(255, 236, 239),
            &context.text.title,
            RECT {
                left: context.card.left + horizontal,
                top: context.card.top + 42,
                right: context.card.right - horizontal,
                bottom: context.card.top + 100,
            },
            DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        draw_text(
            dc,
            detail_font.0,
            rgb(255, 225, 230),
            &context.text.detail,
            RECT {
                left: context.card.left + horizontal,
                top: context.card.top + 118,
                right: context.card.right - horizontal,
                bottom: context.card.bottom - 112,
            },
            DT_CENTER | DT_WORDBREAK,
        );
        let _ = EndPaint(hwnd, &paint);
    }
}

fn virtual_desktop() -> Result<RECT, AlertFailure> {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        return Err(AlertFailure::new(
            AlertFailureKind::WindowCreation,
            "Windows reported an empty virtual desktop",
        ));
    }
    Ok(RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    })
}

fn monitor_bounds(anchor: Option<RectI>) -> Result<RECT, AlertFailure> {
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
        return Err(windows_failure(
            AlertFailureKind::WindowPresentation,
            "GetMonitorInfoW(alert card)",
            windows::core::Error::from_win32(),
        ));
    }
    Ok(info.rcMonitor)
}

fn read_physical_buttons() -> MouseButtons {
    let mut buttons = MouseButtons::NONE;
    for (key, button) in [
        (VK_LBUTTON, MouseButtons::LEFT),
        (VK_RBUTTON, MouseButtons::RIGHT),
        (VK_MBUTTON, MouseButtons::MIDDLE),
        (VK_XBUTTON1, MouseButtons::X1),
        (VK_XBUTTON2, MouseButtons::X2),
    ] {
        if unsafe { GetAsyncKeyState(i32::from(key.0)) } & i16::MIN != 0 {
            buttons = buttons | button;
        }
    }
    buttons
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
            let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) };
        }
    }
}

struct Font(windows::Win32::Graphics::Gdi::HFONT);

impl Font {
    fn new(pixels: i32, weight: i32) -> Self {
        Self(unsafe {
            CreateFontW(
                -pixels,
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
                w!("Segoe UI"),
            )
        })
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) };
        }
    }
}

unsafe fn draw_text(
    dc: windows::Win32::Graphics::Gdi::HDC,
    font: windows::Win32::Graphics::Gdi::HFONT,
    color: COLORREF,
    text: &str,
    mut bounds: RECT,
    format: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    let previous = unsafe { SelectObject(dc, HGDIOBJ(font.0)) };
    unsafe {
        SetTextColor(dc, color);
    }
    let mut utf16 = text.encode_utf16().collect::<Vec<_>>();
    unsafe {
        DrawTextW(dc, &mut utf16, &mut bounds, format);
        SelectObject(dc, previous);
    }
}

fn control_id(id: u16) -> HMENU {
    HMENU(id as usize as *mut c_void)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16))
}

fn windows_failure(
    kind: AlertFailureKind,
    operation: &'static str,
    error: windows::core::Error,
) -> AlertFailure {
    AlertFailure::new(
        kind,
        format!(
            "{operation} failed (HRESULT 0x{:08X}): {}",
            error.code().0 as u32,
            error.message()
        ),
    )
}

fn report_degradation(warning: &AlertFailure) {
    eprintln!(
        "[poe-alarm-alert-win] compatibility warning: {warning}; the red alert remains active"
    );
}

#[cfg(all(test, windows))]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::{
        AlertServiceConfig, AlertText, AlertTrigger, AlertTriggerStatus, BlockingAlertService,
    };
    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

    static WINDOW_TEST_GATE: Mutex<()> = Mutex::new(());

    #[test]
    fn acknowledgement_shortcut_requires_control_shift_and_f12() {
        assert!(acknowledgement_chord(true, true, true));
        assert!(!acknowledgement_chord(true, false, false));
        assert!(!acknowledgement_chord(true, true, false));
        assert!(!acknowledgement_chord(true, false, true));
        assert!(!acknowledgement_chord(false, true, true));
    }

    fn test_wave() -> poe_alarm_platform_win::ValidatedWave {
        let channels = 1u16;
        let sample_rate = 8_000u32;
        let bits = 8u16;
        let samples = 80usize;
        let block_align = channels * bits / 8;
        let data_size = samples * usize::from(block_align);
        let mut wave = Vec::with_capacity(44 + data_size);
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&(36 + data_size as u32).to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&16u32.to_le_bytes());
        wave.extend_from_slice(&1u16.to_le_bytes());
        wave.extend_from_slice(&channels.to_le_bytes());
        wave.extend_from_slice(&sample_rate.to_le_bytes());
        wave.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        wave.extend_from_slice(&block_align.to_le_bytes());
        wave.extend_from_slice(&bits.to_le_bytes());
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&(data_size as u32).to_le_bytes());
        wave.resize(44 + data_size, 128);
        poe_alarm_platform_win::ValidatedWave::from_bytes("alert-test.wav", wave).unwrap()
    }

    fn trigger() -> AlertTrigger {
        AlertTrigger::new(AlertText::new("Match", "Check the item", "Acknowledge").unwrap())
    }

    fn wait_for(
        service: &BlockingAlertService,
        expected: &'static str,
        timeout: Duration,
        predicate: impl Fn(&AlertEvent) -> bool,
    ) -> AlertEvent {
        let started = Instant::now();
        let mut observed = Vec::new();
        loop {
            if let Some(event) = service.try_next_event() {
                if predicate(&event) {
                    return event;
                }
                observed.push(event);
            }
            assert!(
                started.elapsed() < timeout,
                "timed out waiting for {expected}; observed_events={observed:?}; {}; physical_buttons={:?}",
                service.diagnostic_snapshot(),
                read_physical_buttons(),
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_warning(
        service: &BlockingAlertService,
        expected: AlertFailureKind,
        timeout: Duration,
    ) -> AlertFailure {
        let started = Instant::now();
        loop {
            if let Some(warning) = service.try_next_warning()
                && warning.kind == expected
            {
                return warning;
            }
            assert!(
                started.elapsed() < timeout,
                "timed out waiting for {expected:?} compatibility warning"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn card_geometry_is_relative_to_the_current_virtual_desktop() {
        let desktop = RECT {
            left: -1_920,
            top: 0,
            right: 1_920,
            bottom: 1_080,
        };
        let left_monitor = RECT {
            left: -1_920,
            top: 0,
            right: 0,
            bottom: 1_080,
        };
        let right_monitor = RECT {
            left: 0,
            top: 0,
            right: 1_920,
            bottom: 1_080,
        };

        let left = OverlayGeometry::for_topology(desktop, left_monitor);
        let right = OverlayGeometry::for_topology(desktop, right_monitor);

        assert_eq!(left.card.left, 600);
        assert_eq!(left.card.right, 1_320);
        assert_eq!(right.card.left, 2_520);
        assert_eq!(right.card.right, 3_240);
    }

    #[test]
    fn every_presentation_requeries_topology_and_display_messages_request_refresh() {
        let _serial = WINDOW_TEST_GATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (mut overlay, _affinity_warning) = NativeOverlay::create(true, false).unwrap();
        let text = AlertText::new("Match", "Check the item", "Acknowledge").unwrap();

        overlay.present(&text, None).unwrap();
        assert_eq!(overlay.topology_query_count, 1);
        overlay.hide();
        overlay.present(&text, None).unwrap();
        assert_eq!(overlay.topology_query_count, 2);
        let _ = overlay.take_topology_refresh_request();

        unsafe {
            SendMessageW(
                overlay.hwnd,
                WM_DISPLAYCHANGE,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            );
        }
        assert!(overlay.take_topology_refresh_request());
        unsafe {
            SendMessageW(
                overlay.hwnd,
                WM_DPICHANGED,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            );
        }
        assert!(overlay.take_topology_refresh_request());
        overlay.refresh_topology().unwrap();
        assert_eq!(overlay.topology_query_count, 3);
        overlay.hide();
    }

    #[test]
    fn hidden_window_becomes_visible_and_acknowledges() {
        let _serial = WINDOW_TEST_GATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut config = AlertServiceConfig::new(test_wave());
        config.physical_buttons_override = Some(MouseButtons::NONE);
        let service = BlockingAlertService::start(config).unwrap();
        let concurrent = BlockingAlertService::start(AlertServiceConfig::new(test_wave()))
            .expect_err("the process may own only one blocking alert service");
        assert_eq!(concurrent.kind, AlertFailureKind::AlreadyInUse);
        let status = service.trigger(trigger(), None).unwrap();
        let AlertTriggerStatus::Accepted(alert_id) = status else {
            panic!("first trigger must be accepted");
        };
        assert_eq!(
            service.trigger(trigger(), None).unwrap(),
            AlertTriggerStatus::AlreadyLatched(alert_id)
        );
        let event = wait_for(
            &service,
            "Presented",
            Duration::from_secs(2),
            |event| matches!(event, AlertEvent::Presented { alert_id: id } if *id == alert_id),
        );
        assert!(matches!(event, AlertEvent::Presented { .. }));
        service.acknowledge().unwrap();
        let event = wait_for(
            &service,
            "Acknowledged",
            Duration::from_secs(2),
            |event| matches!(event, AlertEvent::Acknowledged { alert_id: id } if *id == alert_id),
        );
        assert!(matches!(event, AlertEvent::Acknowledged { .. }));
        let stop_started = Instant::now();
        service.stop().unwrap();
        let _ = wait_for(&service, "Stopped", Duration::from_secs(1), |event| {
            matches!(event, AlertEvent::Stopped)
        });
        assert!(stop_started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn display_affinity_failure_warns_but_does_not_block_the_alert() {
        let _serial = WINDOW_TEST_GATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut config = AlertServiceConfig::new(test_wave());
        config.allow_overlay_capture = false;
        config.force_display_affinity_failure = true;
        config.physical_buttons_override = Some(MouseButtons::NONE);
        let service = BlockingAlertService::start(config).unwrap();

        let warning = wait_for_warning(
            &service,
            AlertFailureKind::DisplayAffinity,
            Duration::from_secs(1),
        );
        assert!(warning.detail.contains("forced to fail"));

        let AlertTriggerStatus::Accepted(alert_id) = service.trigger(trigger(), None).unwrap()
        else {
            panic!("trigger must still be accepted after display-affinity degradation");
        };
        let _ = wait_for(
            &service,
            "Presented after display-affinity degradation",
            Duration::from_secs(2),
            |event| matches!(event, AlertEvent::Presented { alert_id: id } if *id == alert_id),
        );
        service.acknowledge().unwrap();
        let _ = wait_for(
            &service,
            "Acknowledged after display-affinity degradation",
            Duration::from_secs(2),
            |event| matches!(event, AlertEvent::Acknowledged { alert_id: id } if *id == alert_id),
        );
        service.stop().unwrap();
        let _ = wait_for(&service, "Stopped", Duration::from_secs(1), |event| {
            matches!(event, AlertEvent::Stopped)
        });
    }

    #[test]
    fn stalled_presentation_times_out_without_latching() {
        let _serial = WINDOW_TEST_GATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut config = AlertServiceConfig::new(test_wave());
        config.presentation_timeout = Duration::from_millis(80);
        config.stall_first_presentation = true;
        let service = BlockingAlertService::start(config).unwrap();
        let AlertTriggerStatus::Accepted(alert_id) = service.trigger(trigger(), None).unwrap()
        else {
            panic!("first trigger must be accepted");
        };
        let event = wait_for(
            &service,
            "PresentationTimeout failure",
            Duration::from_secs(1),
            |event| matches!(event, AlertEvent::Failed { alert_id: id, failure } if *id == alert_id && failure.kind == AlertFailureKind::PresentationTimeout),
        );
        assert!(matches!(event, AlertEvent::Failed { .. }));
        assert!(matches!(
            service.trigger(trigger(), None).unwrap(),
            AlertTriggerStatus::Accepted(_)
        ));
        service.stop().unwrap();
        let _ = wait_for(&service, "Stopped", Duration::from_secs(1), |event| {
            matches!(event, AlertEvent::Stopped)
        });
    }
}
