//! POE Alarm GPUI 前端(Ledger v1)。
//!
//! Phase 3:三档窗口骨架(Workbench 1180×620 / Instrument 720×560 / Dock 400×620),
//! Ctrl⇧1/2/3 切档,三档共用一份视图状态,切档不重建控件、不丢输入。
//! Phase 4 将接入 poe-alarm-runtime / poe-alarm-settings 真实链路。

mod backend;
mod dock;
mod instrument;
mod shell;
mod state;
mod theme;
mod ui;
mod workbench;

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use gpui_component::Root;

use gpui::KeyBinding;
use shell::{AppShell, SwitchDock, SwitchInstrument, SwitchWorkbench};
use state::Tier;

fn main() {
    Application::new().run(move |cx: &mut App| {
        gpui_component::init(cx);
        theme::apply_ledger_theme(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-shift-1", SwitchWorkbench, None),
            KeyBinding::new("ctrl-shift-2", SwitchInstrument, None),
            KeyBinding::new("ctrl-shift-3", SwitchDock, None),
        ]);

        let (w, h) = Tier::Workbench.size();
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
                // 让根节点获得初始焦点,否则无焦点时按键事件不会派发(Ctrl⇧1/2/3)。
                let focus = view.read(cx).focus_handle.clone();
                window.focus(&focus);
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
