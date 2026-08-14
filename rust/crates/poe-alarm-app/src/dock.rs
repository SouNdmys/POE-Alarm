//! Dock 400×620(章节 10):游戏内长期停靠档。
//! 命令行 34 | 当前目标卡 | 六行 32 摘要 | 状态块 | 40px 主操作 + 两个 28 次操作。

use gpui::{Context, Div, SharedString, Window, div, prelude::*, px};
use gpui_component::{StyledExt, input::Input};

use crate::shell::AppShell;
use crate::state::*;
use crate::theme::*;
use crate::ui::*;

impl AppShell {
    pub fn render_dock(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let summary_row = |label: &str, value: &str, mono_value: bool| {
            let mut row = div()
                .h(px(H_DOCK_ROW))
                .flex_none()
                .h_flex()
                .items_center()
                .px_3()
                .border_b_1()
                .border_color(c(HAIRLINE_SOFT))
                .child(
                    div()
                        .text_size(fs(FS_12))
                        .text_color(c(TEXT_PRIMARY))
                        .child(SharedString::from(label.to_string())),
                );
            row = if mono_value {
                row.child(
                    div()
                        .ml_auto()
                        .font_family(FONT_MONO)
                        .text_size(fs(FS_10_5))
                        .text_color(c(TEXT_SECONDARY))
                        .child(SharedString::from(value.to_string())),
                )
            } else {
                row.child(
                    div()
                        .ml_auto()
                        .text_size(fs(FS_11_5))
                        .text_color(c(TEXT_SECONDARY))
                        .child(SharedString::from(value.to_string())),
                )
            };
            row.child(
                div()
                    .ml_2()
                    .text_size(px(9.))
                    .text_color(c(TEXT_DISABLED))
                    .child("▾"),
            )
        };

        let primary_label = self.s.run.primary_label();
        let hit_line = match self.s.run {
            RunPhase::Idle => "等待启动",
            RunPhase::Monitoring => "扫描中 · 未命中",
            RunPhase::Hit => "命中 · 8(6-8)% 攻擊速度",
        };

        div()
            .v_flex()
            .size_full()
            // 顶栏
            .child(
                div()
                    .h(px(H_BUTTON))
                    .flex_none()
                    .h_flex()
                    .items_center()
                    .gap(px(9.))
                    .px(px(11.))
                    .bg(c(RAIL))
                    .border_b_1()
                    .border_color(c(HAIRLINE))
                    .child(div().size(px(8.)).flex_none().bg(c(ACCENT)))
                    .child(micro_title_sm("DOCK"))
                    .child(div().ml_auto().child(self.tier_switcher(cx))),
            )
            // 命令行:粘贴词缀 → 设为当前单条目标并保存
            .child(
                div()
                    .flex_none()
                    .h_flex()
                    .items_center()
                    .gap(px(7.))
                    .px_2()
                    .py(px(4.))
                    .bg(c(PANEL))
                    .border_b_1()
                    .border_color(c(HAIRLINE))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.s.command_input)),
                    )
                    .child(
                        button("dock-apply", LedgerButton::Quiet, "设为目标", cx).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.apply_command_as_target(window, cx)
                            }),
                        ),
                    ),
            )
            // 当前目标卡
            .child(
                div()
                    .flex_none()
                    .v_flex()
                    .gap(px(6.))
                    .p_3()
                    .pb(px(13.))
                    .bg(c(PANEL))
                    .border_b_1()
                    .border_color(c(HAIRLINE_SOFT))
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(micro_title_sm("当前目标"))
                            .child(div().flex_1().h(px(1.)).bg(c(HAIRLINE_SOFT)))
                            .child(
                                div()
                                    .text_size(fs(FS_10_5))
                                    .text_color(c(TEXT_META))
                                    .child("POE2 · 繁中 · 单条"),
                            ),
                    )
                    .child({
                        let target = self.s.template_input.read(cx).value();
                        let target = if target.trim().is_empty() {
                            SharedString::from("尚未设置目标词缀")
                        } else {
                            SharedString::from(target.trim().to_string())
                        };
                        div()
                            .text_size(fs(FS_13))
                            .font_semibold()
                            .line_height(px(FS_13 * 1.55))
                            .text_color(c(TEXT_PRIMARY))
                            .child(target)
                    })
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_10_5))
                            .text_color(c(TEXT_META))
                            .child(self.normalized_preview(cx)),
                    ),
            )
            // 摘要(真实设置值)
            .child({
                let (region, ocr, sound, hotkey) = match &self.backend {
                    Some(b) => (
                        b.region_label(),
                        b.ocr_language_label(),
                        b.sound_label(),
                        b.hotkey_label(),
                    ),
                    None => (
                        "—".to_owned(),
                        "—".to_owned(),
                        "—".to_owned(),
                        "—".to_owned(),
                    ),
                };
                div()
                    .flex_none()
                    .v_flex()
                    .child(
                        div()
                            .id("dock-region")
                            .on_click(cx.listener(|this, _, _, cx| this.begin_region_selection(cx)))
                            .child(summary_row("识别区域", &region, true)),
                    )
                    .child(summary_row("词缀文字", &ocr, true))
                    .child(summary_row(
                        "命中方式",
                        match self.s.target_mode {
                            TargetMode::Single => "单条词缀",
                            TargetMode::Multi => "多词缀组合",
                        },
                        false,
                    ))
                    .child(summary_row("提醒声音", &sound, true))
                    .child(summary_row("启动热键", &hotkey, true))
            })
            // 状态块 + 最近命中
            .child(
                div()
                    .mt_auto()
                    .flex_none()
                    .v_flex()
                    .gap(px(7.))
                    .p_3()
                    .bg(c(PANEL))
                    .border_t_1()
                    .border_color(c(HAIRLINE))
                    .child(self.run_status_block())
                    .child(
                        div()
                            .v_flex()
                            .gap(px(3.))
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_10))
                            .text_color(c(TEXT_META))
                            .child(SharedString::from(format!(
                                "p50 31.0ms · p95 47.9ms · {hit_line}"
                            )))
                            .child("21:04:14 · 8(6-8)% 攻擊速度 · 02:11")
                            .child("20:51:02 · 7(6-8)% 攻擊速度 · 02:44"),
                    ),
            )
            // 主操作区
            .child(
                div()
                    .flex_none()
                    .v_flex()
                    .gap(px(7.))
                    .p_3()
                    .bg(c(CANVAS))
                    .border_t_1()
                    .border_color(c(HAIRLINE_SOFT))
                    .child(
                        button("dock-primary", LedgerButton::Primary, primary_label, cx)
                            .w_full()
                            .h(px(H_DOCK_PRIMARY))
                            .text_size(fs(FS_13))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_run(cx))),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap(px(7.))
                            .child(
                                div().flex_1().child(
                                    button("dock-shot", LedgerButton::Secondary, "识别截图", cx)
                                        .w_full(),
                                ),
                            )
                            .child(
                                div().flex_1().child(
                                    button("dock-save", LedgerButton::Secondary, "保存设置", cx)
                                        .w_full()
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.save_settings(cx)),
                                        ),
                                ),
                            ),
                    ),
            )
    }
}
