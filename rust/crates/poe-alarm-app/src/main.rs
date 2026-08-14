//! Phase 1 spike: minimal GPUI + gpui-component window used to validate the
//! riskiest assumptions on the user's real Windows machine before the full
//! Ledger frontend is built:
//! - window creation / DirectX rendering
//! - CJK text rendering (Microsoft YaHei UI + monospace data font)
//! - IME input into a text field (Traditional/Simplified Chinese)
//! - DPI scaling of 1px hairlines at 96/120/144
//!
//! This binary is deliberately throwaway; the real frontend lives in the
//! same crate afterwards.

use gpui::{
    App, Application, Bounds, Context, Entity, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
use gpui_component::{
    ActiveTheme, Root, StyledExt,
    button::{Button, ButtonVariants},
    input::{InputState, TextInput},
};

// --- Ledger v1 tokens (subset used by the spike) ---
const CANVAS: u32 = 0xF5F2EC;
const PANEL: u32 = 0xFBF8F2;
const RAIL: u32 = 0xF1EDE4;
const WELL: u32 = 0xFFFDF9;
const HAIRLINE_SOFT: u32 = 0xE9E3D8;
const HAIRLINE: u32 = 0xD8D0C2;
const HAIRLINE_STRONG: u32 = 0xCBC2B2;
const TEXT_PRIMARY: u32 = 0x1D1A15;
const TEXT_SECONDARY: u32 = 0x524C41;
const TEXT_META: u32 = 0x6F6759;
const ACCENT: u32 = 0x0E6A64;
const ACCENT_TEXT: u32 = 0x0B534E;
const ACCENT_WASH: u32 = 0xE3EEEB;

struct Spike {
    affix_input: Entity<InputState>,
    name_input: Entity<InputState>,
    log: Vec<SharedString>,
}

impl Spike {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let affix_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("粘贴完整词缀,例如:若近期有造成暴擊,增加 (6—8)% 攻擊速度")
        });
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("条件名称(中文 IME 测试)"));
        Self {
            affix_input,
            name_input,
            log: vec!["spike ready · 等待输入".into()],
        }
    }

    fn record(&mut self, cx: &mut Context<Self>) {
        let affix = self.affix_input.read(cx).value();
        let name = self.name_input.read(cx).value();
        self.log
            .push(format!("名称[{}] 模板[{}]", name, affix).into());
        cx.notify();
    }
}

impl Render for Spike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let log_lines = self.log.iter().rev().take(6).cloned().collect::<Vec<_>>();
        div()
            .v_flex()
            .size_full()
            .bg(rgb(CANVAS))
            .text_color(rgb(TEXT_PRIMARY))
            .font_family("Microsoft YaHei UI")
            .text_size(px(12.))
            // title rail
            .child(
                div()
                    .h_flex()
                    .h(px(30.))
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .bg(rgb(RAIL))
                    .border_b_1()
                    .border_color(rgb(HAIRLINE))
                    .child(div().size(px(9.)).bg(rgb(ACCENT)))
                    .child(
                        div()
                            .font_family("Cascadia Mono")
                            .text_size(px(11.))
                            .text_color(rgb(TEXT_SECONDARY))
                            .child("POE ALARM · GPUI SPIKE · Ledger token 检验"),
                    ),
            )
            // body panel
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .gap_3()
                    .p_4()
                    .m_3()
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(HAIRLINE_STRONG))
                    .child(
                        div()
                            .text_color(rgb(TEXT_META))
                            .text_size(px(11.5))
                            .child("下面两个输入框验证:中文 IME(候选框跟随)、繁中粘贴、等宽数据字。"),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .w(px(96.))
                                    .flex_none()
                                    .text_size(px(11.5))
                                    .text_color(rgb(TEXT_META))
                                    .child("条件名称"),
                            )
                            .child(div().flex_1().bg(rgb(WELL)).child(TextInput::new(&self.name_input))),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .w(px(96.))
                                    .flex_none()
                                    .text_size(px(11.5))
                                    .text_color(rgb(TEXT_META))
                                    .child("完整词缀模板"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .bg(rgb(WELL))
                                    .border_l_2()
                                    .border_color(rgb(ACCENT))
                                    .child(TextInput::new(&self.affix_input)),
                            ),
                    )
                    .child(
                        div().h_flex().gap_2().child(
                            Button::new("record")
                                .primary()
                                .label("记录到日志")
                                .on_click(cx.listener(|this, _, _, cx| this.record(cx))),
                        ),
                    )
                    // log pane
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .gap_1()
                            .p_2()
                            .bg(rgb(PANEL))
                            .border_1()
                            .border_color(rgb(HAIRLINE))
                            .font_family("Cascadia Mono")
                            .text_size(px(10.5))
                            .text_color(rgb(TEXT_SECONDARY))
                            .children(log_lines),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .p_2()
                            .bg(rgb(ACCENT_WASH))
                            .border_l_2()
                            .border_color(rgb(ACCENT))
                            .text_color(rgb(ACCENT_TEXT))
                            .child("状态样例 · 正在监控词缀(墨青 wash + 2px 左边框)"),
                    ),
            )
            // status bar
            .child(
                div()
                    .h_flex()
                    .h(px(24.))
                    .flex_none()
                    .items_center()
                    .bg(rgb(RAIL))
                    .border_t_1()
                    .border_color(rgb(HAIRLINE))
                    .font_family("Cascadia Mono")
                    .text_size(px(10.))
                    .text_color(rgb(TEXT_SECONDARY))
                    .child(div().px_2().child("spike v0 · 96/120/144 DPI 请各看一眼发丝线"))
                    .child(
                        div()
                            .px_2()
                            .border_l_1()
                            .border_color(rgb(HAIRLINE_SOFT))
                            .child("字号 10/10.5/11.5/12 · 行高按控件"),
                    ),
            )
    }
}

fn main() {
    Application::new().run(move |cx: &mut App| {
        gpui_component::init(cx);
        let bounds = Bounds::centered(None, size(px(720.), px(560.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("POE Alarm · GPUI Spike".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| Spike::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
