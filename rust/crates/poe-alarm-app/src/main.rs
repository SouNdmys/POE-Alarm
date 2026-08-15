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

fn main() {
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
