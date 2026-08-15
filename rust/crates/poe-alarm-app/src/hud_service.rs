//! 状态浮窗服务(Windows):独立线程持有 platform-win HudWindow,
//! 泵消息并接收内容/显隐/交互命令。监控中点击穿透、置顶,未监控时可拖动;
//! 拖动结束把相对位置回投给 UI 落盘。

#![cfg(windows)]

use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use poe_alarm_platform_win::{
    CaptureAffinity, HudInteractionMode, HudWindow, HudWindowConfig, HudWindowPolicy, RectI,
    SizeI, resolve_hud_position,
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
        events: Sender<PlatformEvent>,
    ) -> Self {
        let (tx, rx) = channel::<HudCommand>();
        std::thread::spawn(move || {
            use windows::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
            };
            let policy = HudWindowPolicy {
                interaction: HudInteractionMode::Passive,
                capture_affinity: if allow_overlay_capture {
                    CaptureAffinity::Include
                } else {
                    CaptureAffinity::Exclude
                },
            };
            let size = SizeI::new(HUD_WIDTH, HUD_HEIGHT).expect("HUD size is positive");
            let work = work_area();
            let native_placement = match (placement.relative_x, placement.relative_y) {
                (Some(x), Some(y)) => poe_alarm_platform_win::HudPlacement::manual(x, y)
                    .unwrap_or_default(),
                _ => poe_alarm_platform_win::HudPlacement::Automatic,
            };
            let origin = resolve_hud_position(work, size, native_placement, None);
            let bounds = RectI::new(origin.x, origin.y, HUD_WIDTH, HUD_HEIGHT)
                .expect("HUD bounds are positive");
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
                status_text: "未监控".to_owned(),
                elapsed: "--:--".to_owned(),
                target: String::new(),
            });
            let mut shown = visible;
            let mut interactive = false;
            loop {
                // 泵本线程消息(WM_PAINT / 拖动等)。
                let mut message = MSG::default();
                // SAFETY: standard thread-local message pump.
                while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                    unsafe {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                // 拖动结束:换算为工作区相对坐标(0..=1)回投给 UI。
                if let Some(point) = window.take_user_move() {
                    let work = work_area();
                    let span_x = (work.width - HUD_WIDTH).max(1) as f64;
                    let span_y = (work.height - HUD_HEIGHT).max(1) as f64;
                    let rx = (f64::from(point.x - work.x) / span_x).clamp(0.0, 1.0);
                    let ry = (f64::from(point.y - work.y) / span_y).clamp(0.0, 1.0);
                    let _ = events.send(PlatformEvent::HudMoved(rx, ry));
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

/// 主显示器工作区(排除任务栏)。失败时退回 1080p 全屏。
fn work_area() -> RectI {
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
