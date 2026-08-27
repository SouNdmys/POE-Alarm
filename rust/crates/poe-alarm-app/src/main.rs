//! POE Alarm GPUI 前端(Ledger v1)。
//!
//! Phase 5:单一 1180×620 规则台窗口;轻量查看由可拖动 HUD 浮窗承担。
// GUI 子系统:发布 exe 不再附带控制台窗口(stderr 诊断随之丢弃,
// 用户可见错误一律走界面通知,不依赖控制台)。
#![cfg_attr(windows, windows_subsystem = "windows")]

mod backend;
#[cfg(windows)]
mod hud_service;
mod i18n;
mod shell;
mod state;
mod theme;
mod ui;
mod workbench;

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use gpui_component::Root;

use shell::AppShell;
use state::WORKBENCH_SIZE;

/// Headless check that the packaged binary can actually reach the platform.
///
/// Runs before GPUI starts and exits without opening a window, so the release
/// pipeline can prove the executable it just assembled works rather than
/// assuming a successful link means a working program. Exercises the real
/// services: HUD window creation and teardown, global hotkey registration, the
/// mouse guard, and the built-in alert cue.
#[cfg(windows)]
fn run_self_test() -> i32 {
    match poe_alarm_platform_win::run_windows_self_test() {
        Ok(report) => {
            if poe_alarm_platform_win::built_in_alert_wave().is_err() {
                return 2;
            }
            i32::from(!report.is_healthy())
        }
        Err(_) => 1,
    }
}

fn main() {
    // Held for the whole process. Without it Windows rounds every wait up to a
    // 15.6ms tick, which flattens the monitor's 4ms and 8ms polling constants
    // into the same 15.5ms and adds that difference to how long a winning roll
    // sits unnoticed. Dropped on the way out, restoring the default.
    #[cfg(windows)]
    let _timer = poe_alarm_platform_win::TimerResolutionGuard::acquire();

    #[cfg(windows)]
    if std::env::args()
        .skip(1)
        .any(|argument| argument == "--self-test")
    {
        std::process::exit(run_self_test());
    }

    Application::new().run(move |cx: &mut App| {
        gpui_component::init(cx);
        theme::apply_ledger_theme(cx);

        let (w, h) = WORKBENCH_SIZE;
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("POE Alarm".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| AppShell::new(window, cx));
                let focus = view.read(cx).focus_handle.clone();
                window.focus(&focus);
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
