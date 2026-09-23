//! 状态浮窗服务(Windows):独立线程持有 platform-win HudWindow,
//! 泵消息并接收内容/显隐/交互命令。监控中点击穿透、置顶,未监控时可拖动;
//! 拖动结束把相对位置回投给 UI 落盘。

use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use poe_alarm_platform_win::{
    CaptureAffinity, HudInteractionMode, HudWindow, HudWindowConfig, HudWindowPolicy, PointI,
    RectI, SizeI, resolve_hud_position,
};

use crate::backend::PlatformEvent;

pub use poe_alarm_platform_win::HudContent;

const HUD_WIDTH: i32 = 296;
const HUD_HEIGHT: i32 = 52;

pub enum HudCommand {
    Content(HudContent),
    Visible(bool),
    Interactive(bool),
    /// 录屏可见性:true = 出现在录屏/截图里。
    Capture(bool),
}

pub struct HudService {
    tx: Sender<HudCommand>,
}

impl HudService {
    /// 启动 HUD 线程;失败只打日志(浮窗不可用不阻塞主功能)。
    pub fn start(
        allow_overlay_capture: bool,
        visible: bool,
        placement: poe_alarm_settings::HudPlacement,
        initial_status: String,
        events: Sender<PlatformEvent>,
    ) -> Self {
        let (tx, rx) = channel::<HudCommand>();
        std::thread::spawn(move || {
            use windows::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_DISPLAYCHANGE,
                WM_SETTINGCHANGE,
            };
            let mut placement = placement;
            let policy = HudWindowPolicy {
                interaction: HudInteractionMode::Passive,
                capture_affinity: if allow_overlay_capture {
                    CaptureAffinity::Include
                } else {
                    CaptureAffinity::Exclude
                },
            };
            let bounds = placement_bounds(&monitor_work_areas(), &placement);
            let mut window = match HudWindow::create(HudWindowConfig {
                bounds,
                policy,
                visible,
            }) {
                Ok(window) => window,
                Err(error) => {
                    eprintln!("HUD window creation failed: {error}");
                    return;
                }
            };
            let _ = window.set_content(HudContent {
                monitoring: false,
                status_text: initial_status,
                elapsed: "--:--".to_owned(),
                target: String::new(),
            });
            let mut shown = visible;
            let mut interactive = false;
            loop {
                // 泵本线程消息(WM_PAINT / 拖动等)。
                let mut message = MSG::default();
                let mut displays_changed = false;
                // SAFETY: standard thread-local message pump.
                while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                    displays_changed |=
                        matches!(message.message, WM_DISPLAYCHANGE | WM_SETTINGCHANGE);
                    unsafe {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                if displays_changed {
                    // Keep the saved device name when a display is unplugged:
                    // show on the primary for now and restore when it returns.
                    let _ = window.set_bounds(placement_bounds(&monitor_work_areas(), &placement));
                }
                // 拖动结束:换算为工作区相对坐标(0..=1)回投给 UI。
                if let Some(point) = window.take_user_move() {
                    let monitors = monitor_work_areas();
                    if let Some(moved) = placement_for_move(&monitors, point) {
                        placement = moved;
                        // Clamp a partly offscreen drop to its chosen work
                        // area so the saved and currently visible positions agree.
                        let _ = window.set_bounds(placement_bounds(&monitors, &placement));
                        let _ = events.send(PlatformEvent::HudMoved(placement.clone()));
                    }
                }
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(HudCommand::Content(content)) => {
                        let _ = window.set_content(content);
                    }
                    Ok(HudCommand::Visible(show)) => {
                        if show != shown {
                            shown = show;
                            let _ = if show { window.show() } else { window.hide() };
                        }
                    }
                    Ok(HudCommand::Interactive(want)) => {
                        if want != interactive {
                            interactive = want;
                            let mode = if want {
                                HudInteractionMode::Placement
                            } else {
                                HudInteractionMode::Passive
                            };
                            let _ = window.set_interaction_mode(mode);
                        }
                    }
                    Ok(HudCommand::Capture(allow)) => {
                        let _ = window.set_capture_affinity(if allow {
                            CaptureAffinity::Include
                        } else {
                            CaptureAffinity::Exclude
                        });
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Self { tx }
    }

    pub fn update(&self, content: HudContent) {
        let _ = self.tx.send(HudCommand::Content(content));
    }

    pub fn set_visible(&self, visible: bool) {
        let _ = self.tx.send(HudCommand::Visible(visible));
    }

    pub fn set_interactive(&self, interactive: bool) {
        let _ = self.tx.send(HudCommand::Interactive(interactive));
    }

    pub fn set_capture(&self, allow: bool) {
        let _ = self.tx.send(HudCommand::Capture(allow));
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MonitorWorkArea {
    device_name: Option<String>,
    bounds: RectI,
    work: RectI,
    primary: bool,
}

fn preferred_monitor<'a>(
    monitors: &'a [MonitorWorkArea],
    device_name: Option<&str>,
) -> Option<&'a MonitorWorkArea> {
    device_name
        .and_then(|name| {
            monitors.iter().find(|monitor| {
                monitor
                    .device_name
                    .as_deref()
                    .is_some_and(|device| device.eq_ignore_ascii_case(name))
            })
        })
        .or_else(|| monitors.iter().find(|monitor| monitor.primary))
        .or_else(|| monitors.first())
}

fn placement_bounds(
    monitors: &[MonitorWorkArea],
    placement: &poe_alarm_settings::HudPlacement,
) -> RectI {
    let work = preferred_monitor(monitors, placement.monitor_device_name.as_deref())
        .map_or_else(fallback_work_area, |monitor| monitor.work);
    let native_placement = match (placement.relative_x, placement.relative_y) {
        (Some(x), Some(y)) => {
            poe_alarm_platform_win::HudPlacement::manual(x, y).unwrap_or_default()
        }
        _ => poe_alarm_platform_win::HudPlacement::Automatic,
    };
    let size = SizeI::new(HUD_WIDTH, HUD_HEIGHT).expect("HUD size is positive");
    let origin = resolve_hud_position(work, size, native_placement, None);
    RectI::new(origin.x, origin.y, HUD_WIDTH, HUD_HEIGHT).expect("HUD bounds are positive")
}

fn placement_for_move(
    monitors: &[MonitorWorkArea],
    point: PointI,
) -> Option<poe_alarm_settings::HudPlacement> {
    let bounds = RectI::new(point.x, point.y, HUD_WIDTH, HUD_HEIGHT)?;
    // Like MonitorFromRect: prefer the largest intersecting area, then the
    // nearest display if the rectangle lies in a gap between displays.
    let monitor = monitors.iter().min_by_key(|monitor| {
        let width = (i64::from(bounds.right().min(monitor.bounds.right()))
            - i64::from(bounds.x.max(monitor.bounds.x)))
        .max(0);
        let height = (i64::from(bounds.bottom().min(monitor.bounds.bottom()))
            - i64::from(bounds.y.max(monitor.bounds.y)))
        .max(0);
        let cx = i64::from(bounds.x) + i64::from(bounds.width) / 2;
        let cy = i64::from(bounds.y) + i64::from(bounds.height) / 2;
        let dx = i128::from(
            cx - cx.clamp(
                i64::from(monitor.bounds.x),
                i64::from(monitor.bounds.right()),
            ),
        );
        let dy = i128::from(
            cy - cy.clamp(
                i64::from(monitor.bounds.y),
                i64::from(monitor.bounds.bottom()),
            ),
        );
        (-(width * height), dx * dx + dy * dy, !monitor.primary)
    })?;
    let relative = |position: i32, origin: i32, span: i32| {
        ((f64::from(position) - f64::from(origin)) / f64::from(span.max(1))).clamp(0.0, 1.0)
    };
    // An enumeration failure must not erase a previously saved monitor name.
    // Keep the visible drag but wait for an identifiable display before saving.
    let device_name = monitor.device_name.clone()?;
    Some(poe_alarm_settings::HudPlacement {
        monitor_device_name: Some(device_name),
        relative_x: Some(relative(
            point.x,
            monitor.work.x,
            monitor.work.width - HUD_WIDTH,
        )),
        relative_y: Some(relative(
            point.y,
            monitor.work.y,
            monitor.work.height - HUD_HEIGHT,
        )),
    })
}

fn monitor_work_areas() -> Vec<MonitorWorkArea> {
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::Graphics::Gdi::EnumDisplayMonitors;
    let mut monitors = Vec::<MonitorWorkArea>::new();
    // SAFETY: the callback borrows this vector only during synchronous enumeration.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM((&raw mut monitors) as isize),
        )
    };
    if monitors.is_empty() {
        let work = fallback_work_area();
        monitors.push(MonitorWorkArea {
            device_name: None,
            bounds: work,
            work,
            primary: true,
        });
    }
    monitors
}

unsafe extern "system" fn collect_monitor(
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
    _dc: windows::Win32::Graphics::Gdi::HDC,
    _rect: *mut windows::Win32::Foundation::RECT,
    state: windows::Win32::Foundation::LPARAM,
) -> windows::core::BOOL {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO, MONITORINFOEXW};
    use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: MONITORINFOEXW starts with MONITORINFO, and cbSize includes szDevice.
    if unsafe { GetMonitorInfoW(monitor, &raw mut info.monitorInfo) }.as_bool() {
        let convert = |rect: windows::Win32::Foundation::RECT| {
            RectI::new(
                rect.left,
                rect.top,
                rect.right.checked_sub(rect.left)?,
                rect.bottom.checked_sub(rect.top)?,
            )
        };
        if let (Some(bounds), Some(work)) = (
            convert(info.monitorInfo.rcMonitor),
            convert(info.monitorInfo.rcWork),
        ) {
            let length = info
                .szDevice
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(info.szDevice.len());
            let device_name =
                (length > 0).then(|| String::from_utf16_lossy(&info.szDevice[..length]));
            // SAFETY: state is the live vector passed to EnumDisplayMonitors above.
            unsafe { &mut *(state.0 as *mut Vec<MonitorWorkArea>) }.push(MonitorWorkArea {
                device_name,
                bounds,
                work,
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
    }
    true.into()
}

/// Enumeration failure fallback: retain the system primary work area when available.
fn fallback_work_area() -> RectI {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut rect = RECT::default();
    // SAFETY: SPI_GETWORKAREA writes a RECT into the provided pointer.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(std::ptr::from_mut(&mut rect).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok();
    if ok
        && let Some(area) = RectI::new(
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
        )
    {
        return area;
    }
    RectI::new(0, 0, 1920, 1080).expect("fallback work area is positive")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn displays() -> Vec<MonitorWorkArea> {
        vec![
            MonitorWorkArea {
                device_name: Some("\\\\.\\DISPLAY2".to_owned()),
                bounds: RectI::new(-1920, -200, 1920, 1080).unwrap(),
                work: RectI::new(-1920, -160, 1920, 1040).unwrap(),
                primary: false,
            },
            MonitorWorkArea {
                device_name: Some("\\\\.\\DISPLAY1".to_owned()),
                bounds: RectI::new(0, 0, 2560, 1440).unwrap(),
                work: RectI::new(0, 0, 2560, 1400).unwrap(),
                primary: true,
            },
        ]
    }

    #[test]
    fn saved_monitor_restores_relative_position_in_its_own_work_area() {
        let monitors = displays();
        let placement = poe_alarm_settings::HudPlacement {
            monitor_device_name: Some("\\\\.\\display2".to_owned()),
            relative_x: Some(0.5),
            relative_y: Some(0.5),
        };
        let bounds = placement_bounds(&monitors, &placement);
        assert_eq!(bounds.x, -1920 + (1920 - HUD_WIDTH) / 2);
        assert_eq!(bounds.y, -160 + (1040 - HUD_HEIGHT) / 2);
        assert!(bounds.right() <= 0);
    }

    #[test]
    fn dragging_to_a_negative_coordinate_display_round_trips_its_device_and_position() {
        let monitors = displays();
        let point = PointI::new(-1567, 321);
        let placement = placement_for_move(&monitors, point).unwrap();
        assert_eq!(
            placement.monitor_device_name.as_deref(),
            Some("\\\\.\\DISPLAY2")
        );
        let restored = placement_bounds(&monitors, &placement);
        assert_eq!((restored.x, restored.y), (point.x, point.y));
        assert!(placement.is_valid());
    }

    #[test]
    fn missing_display_falls_back_without_forgetting_the_saved_device() {
        let monitors = displays();
        let placement = poe_alarm_settings::HudPlacement {
            monitor_device_name: Some("\\\\.\\DISPLAY2".to_owned()),
            relative_x: Some(1.0),
            relative_y: Some(1.0),
        };
        let fallback = placement_bounds(&monitors[1..], &placement);
        assert_eq!((fallback.right(), fallback.bottom()), (2560, 1400));
        let reconnected = placement_bounds(&monitors, &placement);
        assert_eq!((reconnected.right(), reconnected.bottom()), (0, 880));
        assert_eq!(
            placement.monitor_device_name.as_deref(),
            Some("\\\\.\\DISPLAY2")
        );
        assert!(preferred_monitor(&[], None).is_none());
        assert!(preferred_monitor(&monitors, None).unwrap().primary);
    }

    #[test]
    fn crossing_or_offscreen_drops_choose_visible_monitor_and_clamp_inside_work_area() {
        let monitors = displays();
        // More of the HUD is on DISPLAY2 even though it crosses the seam.
        let crossing = placement_for_move(&monitors, PointI::new(-200, 100)).unwrap();
        assert_eq!(
            crossing.monitor_device_name.as_deref(),
            Some("\\\\.\\DISPLAY2")
        );
        assert_eq!(placement_bounds(&monitors, &crossing).right(), 0);
        // A drop beyond the left display remains reachable on that nearest display.
        let offscreen = placement_for_move(&monitors, PointI::new(-2500, -500)).unwrap();
        assert_eq!(
            offscreen.monitor_device_name.as_deref(),
            Some("\\\\.\\DISPLAY2")
        );
        let clamped = placement_bounds(&monitors, &offscreen);
        assert_eq!((clamped.x, clamped.y), (-1920, -160));
    }

    #[test]
    fn unavailable_monitor_identity_does_not_generate_a_destructive_save() {
        let mut monitors = displays();
        monitors[0].device_name = None;
        assert!(placement_for_move(&monitors, PointI::new(-1500, 100)).is_none());
    }
}
