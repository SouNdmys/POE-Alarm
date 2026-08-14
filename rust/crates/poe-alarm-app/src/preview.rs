//! Ledger v1 组件画廊(Phase 2 验收窗口)。
//!
//! 用来在真机上逐节对照《POE Alarm Ledger.dc.html》检查设计系统层:
//! 表面/发丝线、文字四级、状态三色、按钮四类、输入、分段、chip、
//! 树行六态、OCR 八态、指标行、日志面板、警告/错误条与状态栏。

mod theme;
mod ui;

use gpui::{
    App, Application, Bounds, Context, Entity, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use gpui_component::{
    Disableable, Root, StyledExt,
    checkbox::Checkbox,
    input::{Input, InputState},
};

use theme::*;
use ui::*;

struct Gallery {
    name_input: Entity<InputState>,
    affix_input: Entity<InputState>,
    value_input: Entity<InputState>,
    check_on: bool,
}

impl Gallery {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("条件名称(IME 测试)"));
        let affix_input = cx.new(|cx| {
            InputState::new(window, cx).default_value("#% increased Spell Critical Hit Chance")
        });
        let value_input = cx.new(|cx| InputState::new(window, cx).default_value("3.1"));
        Self {
            name_input,
            affix_input,
            value_input,
            check_on: true,
        }
    }

    fn section(&self, title: &str, body: gpui::Div) -> gpui::Div {
        panel()
            .v_flex()
            .flex_none()
            .child(panel_header(title))
            .child(body.p_3())
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let buttons = div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(button("b-pri", LedgerButton::Primary, "开始监控", cx))
                    .child(button("b-sec", LedgerButton::Secondary, "识别截图", cx))
                    .child(button("b-quiet", LedgerButton::Quiet, "+ 添加数值", cx))
                    .child(button("b-del", LedgerButton::Destructive, "删除方案", cx))
                    .child(button("b-dis", LedgerButton::Primary, "开始监控", cx).disabled(true))
                    .child(hotkey_chips(&["Ctrl", "⇧", "F10"])),
            )
            .child(
                div()
                    .text_size(fs(FS_10_5))
                    .text_color(c(TEXT_META))
                    .child("悬停/按下验证 hover 只改底色;最右为 disabled 态;焦点环 Tab 可达。"),
            );

        let inputs = div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(LABEL_COL))
                            .flex_none()
                            .text_size(fs(FS_11_5))
                            .text_color(c(TEXT_META))
                            .child("条件名称"),
                    )
                    .child(div().flex_1().child(Input::new(&self.name_input)))
                    .child(div().w(px(64.)).child(Input::new(&self.value_input)))
                    .child(segmented(&["任意", "全部", "指定条数"], 2, 26.)),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(LABEL_COL))
                            .flex_none()
                            .text_size(fs(FS_11_5))
                            .text_color(c(TEXT_META))
                            .child("完整词缀模板"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .border_l_2()
                            .border_color(c(ACCENT))
                            .child(Input::new(&self.affix_input)),
                    )
                    .child(
                        Checkbox::new("chk")
                            .label("持续显示画面提示")
                            .checked(self.check_on)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.check_on = *checked;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .pl(px(LABEL_COL + 8.))
                    .font_family(FONT_MONO)
                    .text_size(fs(FS_10_5))
                    .text_color(c(TEXT_META))
                    .child("归一化 <PCT> increased spell critical hit chance · 找到 1 个数值"),
            );

        let tree = div()
            .v_flex()
            .child(tree_row(TreeRowSpec {
                depth: 0,
                state: TreeState::Active,
                expander: Some(true),
                label: "流放之路二",
                trailing: "活动",
                trailing_color: Some(ACCENT_TEXT),
            }))
            .child(tree_row(TreeRowSpec {
                depth: 1,
                state: TreeState::Default,
                expander: Some(true),
                label: "可接受结果 1",
                trailing: "≥2/2",
                trailing_color: None,
            }))
            .child(tree_row(TreeRowSpec {
                depth: 2,
                state: TreeState::Selected,
                expander: None,
                label: "法术暴击率",
                trailing: "1 值",
                trailing_color: None,
            }))
            .child(tree_row(TreeRowSpec {
                depth: 2,
                state: TreeState::Hover,
                expander: None,
                label: "暴击伤害加成(hover 示意)",
                trailing: "1 值",
                trailing_color: None,
            }))
            .child(tree_row(TreeRowSpec {
                depth: 2,
                state: TreeState::Warning,
                expander: None,
                label: "攻速(缺模板)",
                trailing: "待补",
                trailing_color: Some(WARN),
            }))
            .child(tree_row(TreeRowSpec {
                depth: 2,
                state: TreeState::Disabled,
                expander: None,
                label: "已停用条件",
                trailing: "off",
                trailing_color: Some(TEXT_DISABLED),
            }));

        let ocr_states: &[(StatusKind, &str, &str)] = &[
            (StatusKind::Idle, "idle", "未监控 · 复用上次结果"),
            (
                StatusKind::Idle,
                "unchanged",
                "画面未变化 · 跳过识别  预检 3.1 ms",
            ),
            (
                StatusKind::Monitoring,
                "scanning",
                "逐带识别 band 02 / 05 · 蓝字掩膜 100/14/72",
            ),
            (
                StatusKind::Monitoring,
                "recheck",
                "原色局部复核 2 行 · 4/27",
            ),
            (
                StatusKind::Monitoring,
                "no-blue",
                "未检测到蓝字 · 继续等待(不自动停止)",
            ),
            (
                StatusKind::Warning,
                "fallback",
                "系统无中文 OCR · 已切内置 PP-OCRv5,较慢",
            ),
            (StatusKind::Hit, "hit", "严格命中 · 已接管输入 · 等待确认"),
            (StatusKind::Idle, "cancelled", "已停止 · 下一轮需重新启动"),
        ];
        let mut ocr_list = div().v_flex();
        for (i, (kind, code, text)) in ocr_states.iter().enumerate() {
            let mut row = div()
                .h(px(H_INPUT))
                .flex_none()
                .h_flex()
                .items_center()
                .gap(px(9.))
                .px_1();
            if i + 1 < ocr_states.len() {
                row = row.border_b_1().border_color(c(HAIRLINE_SOFT));
            }
            if *code == "fallback" {
                row = row.bg(c(WARN_WASH));
            } else if *code == "hit" {
                row = row.bg(c(DANGER_WASH));
            }
            let code_color = match kind {
                StatusKind::Warning => WARN,
                StatusKind::Hit => DANGER,
                _ => TEXT_META,
            };
            row = row
                .child(
                    div()
                        .w(px(76.))
                        .flex_none()
                        .font_family(FONT_MONO)
                        .text_size(fs(FS_9_5))
                        .text_color(c(code_color))
                        .child(*code),
                )
                .child(
                    div()
                        .size(px(6.))
                        .flex_none()
                        .rounded_full()
                        .bg(c(kind.dot())),
                )
                .child(
                    div()
                        .text_size(fs(FS_12))
                        .text_color(c(match kind {
                            StatusKind::Hit => DANGER_TEXT,
                            StatusKind::Warning => WARN_TEXT,
                            StatusKind::Monitoring => TEXT_PRIMARY,
                            _ => TEXT_SECONDARY,
                        }))
                        .child(*text),
                );
            ocr_list = ocr_list.child(row);
        }

        let run_panel = div()
            .v_flex()
            .gap_2()
            .child(status_line(StatusKind::Monitoring, "正在监控词缀", "00:12"))
            .child(
                div()
                    .v_flex()
                    .child(metric_row("判定 p50 / p95", "31.0 / 47.9 ms", false))
                    .child(metric_row("变化预检 p50", "3.1 ms", false))
                    .child(metric_row("局部复核", "4 / 27", false))
                    .child(metric_row("本轮命中", "0", true)),
            )
            .child(log_pane(&[
                LogLine::Meta("band 00 · 32 ms".into()),
                LogLine::Text("+24 生命".into()),
                LogLine::Meta("band 01 · 29 ms".into()),
                LogLine::Match("7% 法術暴擊率".into()),
                LogLine::Hit("规则命中 · 可接受结果 1 · 2/2 条".into()),
                LogLine::Meta("共 5 行 · 全帧 1 次 · 复核 1 次".into()),
            ]));

        let bands = div()
            .v_flex()
            .gap_2()
            .child(warning_band(
                "注意",
                "催化剂、品质或特殊效果会改变屏幕显示值。数值条件只比较屏幕上实际显示的值。",
            ))
            .child(error_band("数值范围的最小值不能大于最大值"));

        div()
            .v_flex()
            .size_full()
            .bg(c(CANVAS))
            .text_color(c(TEXT_PRIMARY))
            .font_family(FONT_UI)
            .text_size(fs(FS_12))
            .child(
                div()
                    .h(px(H_TITLEBAR))
                    .flex_none()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px(px(10.))
                    .bg(c(RAIL))
                    .border_b_1()
                    .border_color(c(HAIRLINE))
                    .child(div().size(px(9.)).bg(c(ACCENT)))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_11_5))
                            .text_color(c(TEXT_SECONDARY))
                            .child("LEDGER · 组件画廊 · 对照设计规范逐节检查"),
                    ),
            )
            .child(
                div()
                    .id("gallery-scroll")
                    .flex_1()
                    .min_h_0()
                    .v_flex()
                    .gap_3()
                    .p_3()
                    .overflow_y_scroll()
                    .child(self.section("05 · 按钮四类(实际可交互)", buttons))
                    .child(self.section("06 · 输入 · 勾选 · 分段 · chip", inputs))
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .items_start()
                            .flex_none()
                            .child(self.section("07 · 树行六态", tree).w(px(360.)))
                            .child(self.section("08 · OCR 八态", ocr_list).flex_1()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .items_start()
                            .flex_none()
                            .child(self.section("右栏 · 状态/指标/日志", run_panel).w(px(400.)))
                            .child(self.section("警告与错误(就地显示)", bands).flex_1()),
                    ),
            )
            .child(status_bar(
                &[
                    StatusSegment {
                        text: "正在监控词缀",
                        color: Some(ACCENT_TEXT),
                    },
                    StatusSegment {
                        text: "区域 1134×956 @ 0,58",
                        color: None,
                    },
                    StatusSegment {
                        text: "OCR 繁中 · zh-TW",
                        color: None,
                    },
                    StatusSegment {
                        text: "规则 2 方案 / 3 词缀",
                        color: None,
                    },
                    StatusSegment {
                        text: "p50 31.0 ms",
                        color: None,
                    },
                ],
                Some("Ctrl⇧F10 开始 · F11 框选 · F12 停止"),
            ))
    }
}

fn main() {
    Application::new().run(move |cx: &mut App| {
        gpui_component::init(cx);
        theme::apply_ledger_theme(cx);
        let bounds = Bounds::centered(None, size(px(1180.), px(840.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("POE Alarm · Ledger 组件画廊".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| Gallery::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
