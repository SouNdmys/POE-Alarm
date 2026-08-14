//! Instrument 720×560(章节 10):单目标仪表档。
//! 左:目标 + 摘要行 + 主操作;右 264 运行栏;底部紧凑状态栏。

use gpui::{Context, Div, SharedString, Window, div, prelude::*, px};
use gpui_component::{StyledExt, input::Input};

use crate::shell::AppShell;
use crate::state::*;
use crate::theme::*;
use crate::ui::*;

impl AppShell {
    pub fn render_instrument(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        div()
            .v_flex()
            .size_full()
            .child(self.ins_titlebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(self.ins_main(cx))
                    .child(self.ins_rail()),
            )
            .child(self.shell_status_bar(true))
    }

    fn ins_titlebar(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(H_TITLEBAR))
            .flex_none()
            .h_flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .bg(c(RAIL))
            .border_b_1()
            .border_color(c(HAIRLINE))
            .child(div().size(px(9.)).flex_none().bg(c(ACCENT)))
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(fs(FS_11_5))
                    .text_color(c(TEXT_SECONDARY))
                    .child("POE2"),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(fs(FS_11_5))
                    .text_color(c(TEXT_DISABLED))
                    .child("›"),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(fs(FS_11_5))
                    .text_color(c(TEXT_PRIMARY))
                    .child(match self.s.target_mode {
                        TargetMode::Single => "单条词缀",
                        TargetMode::Multi => "多词缀组合",
                    }),
            )
            .child(div().ml_auto().child(self.tier_switcher(cx)))
    }

    fn ins_main(&mut self, cx: &mut Context<Self>) -> Div {
        let seg = {
            let selected = match self.s.target_mode {
                TargetMode::Single => 0,
                TargetMode::Multi => 1,
            };
            let mut row = div()
                .h_flex()
                .flex_none()
                .border_1()
                .border_color(c(HAIRLINE));
            for (i, (label, mode)) in [("单条", TargetMode::Single), ("多词缀", TargetMode::Multi)]
                .into_iter()
                .enumerate()
            {
                let mut cell = div()
                    .id(("ins-seg", i))
                    .h(px(H_CHIP - 2.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .text_size(fs(FS_11_5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.s.target_mode = mode;
                        cx.notify();
                    }));
                if i > 0 {
                    cell = cell.border_l_1().border_color(c(HAIRLINE));
                }
                cell = if i == selected {
                    cell.bg(c(ACCENT_WASH)).text_color(c(ACCENT_TEXT))
                } else {
                    cell.bg(c(PANEL))
                        .text_color(c(TEXT_SECONDARY))
                        .hover(|s| s.bg(c(HOVER)))
                };
                row = row.child(cell.child(label));
            }
            row
        };

        let summary_row = |label: &str, value: &str, mono_value: bool, action: Option<&str>| {
            let mut row = div()
                .h(px(H_INPUT))
                .flex_none()
                .h_flex()
                .items_center()
                .border_b_1()
                .border_color(c(HAIRLINE_SOFT))
                .child(
                    div()
                        .w(px(LABEL_COL))
                        .flex_none()
                        .text_size(fs(FS_11_5))
                        .text_color(c(TEXT_META))
                        .child(SharedString::from(label.to_string())),
                );
            row = if mono_value {
                row.child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(fs(FS_12))
                        .text_color(c(TEXT_PRIMARY))
                        .child(SharedString::from(value.to_string())),
                )
            } else {
                row.child(
                    div()
                        .text_size(fs(FS_12))
                        .text_color(c(TEXT_PRIMARY))
                        .child(SharedString::from(value.to_string())),
                )
            };
            if let Some(a) = action {
                row = row.child(
                    div()
                        .ml_auto()
                        .text_size(fs(FS_11_5))
                        .text_color(c(ACCENT_TEXT))
                        .child(SharedString::from(a.to_string())),
                );
            }
            row
        };

        let primary_label = self.s.run.primary_label();
        div()
            .flex_1()
            .min_w_0()
            .v_flex()
            .bg(c(PANEL))
            .border_r_1()
            .border_color(c(HAIRLINE))
            .child(
                div()
                    .p(px(14.))
                    .pb(px(10.))
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap(px(9.))
                            .child(micro_title_sm("目标"))
                            .child(div().flex_1().h(px(1.)).bg(c(HAIRLINE_SOFT)))
                            .child(seg),
                    )
                    .child(
                        div()
                            .border_l_2()
                            .border_color(c(ACCENT))
                            .child(Input::new(&self.s.template_input)),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_10_5))
                            .text_color(c(TEXT_META))
                            .child(self.normalized_preview(cx)),
                    ),
            )
            .child({
                let (region, ocr, sound) = match &self.backend {
                    Some(b) => (b.region_label(), b.ocr_language_label(), b.sound_label()),
                    None => ("—".to_owned(), "—".to_owned(), "—".to_owned()),
                };
                div()
                    .px(px(14.))
                    .v_flex()
                    .child(summary_row("词缀文字", &ocr, true, None))
                    .child(
                        div()
                            .id("ins-region")
                            .on_click(cx.listener(|this, _, _, cx| this.begin_region_selection(cx)))
                            .child(summary_row("识别区域", &region, true, Some("框选 F11"))),
                    )
                    .child(summary_row("提醒声音", &sound, true, None))
            })
            .child(div().mx(px(14.)).mt(px(12.)).child(warning_band(
                "注意",
                "系统没有 zh-TW OCR 能力时会自动切到内置引擎,速度更慢。",
            )))
            .child(
                div()
                    .mt_auto()
                    .flex_none()
                    .h_flex()
                    .gap_2()
                    .p(px(14.))
                    .border_t_1()
                    .border_color(c(HAIRLINE))
                    .child(
                        div().flex_1().child(
                            button("ins-primary", LedgerButton::Primary, primary_label, cx)
                                .w_full()
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_run(cx))),
                        ),
                    )
                    .child(button("ins-shot", LedgerButton::Secondary, "识别截图", cx).w(px(96.))),
            )
    }

    fn ins_rail(&self) -> Div {
        div()
            .w(px(264.))
            .flex_none()
            .v_flex()
            .bg(c(RAIL))
            .child(
                div()
                    .h(px(H_BUTTON))
                    .flex_none()
                    .h_flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(c(HAIRLINE_SOFT))
                    .child(micro_title_sm("运行"))
                    .child(
                        div()
                            .ml_auto()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_9_5))
                            .text_color(c(TEXT_META))
                            .child("繁中 · Windows"),
                    ),
            )
            .child(
                div()
                    .p_3()
                    .flex_none()
                    .v_flex()
                    .gap_2()
                    .border_b_1()
                    .border_color(c(HAIRLINE_SOFT))
                    .child(self.run_status_block())
                    .child(self.metrics_block()),
            )
            .child(div().flex_1().min_h_0().child(self.log_block(10)))
    }
}
