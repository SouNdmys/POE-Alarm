//! Workbench 1180×620(章节 09):
//! 标题栏 30 | 左规则树 218 | 中编辑区(tab 30 + 内容) | 右运行栏 318 | 状态栏 24。

use gpui::{Context, Div, SharedString, Window, div, prelude::*, px};
use gpui_component::{StyledExt, input::Input};

use crate::shell::AppShell;
use crate::state::*;
use crate::theme::*;
use crate::ui::*;

impl AppShell {
    pub fn render_workbench(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        div()
            .v_flex()
            .size_full()
            .child(self.wb_titlebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(self.wb_tree(cx))
                    .child(self.wb_editor(cx))
                    .child(self.wb_run_rail(cx)),
            )
            .child(self.shell_status_bar(false))
    }

    // -- 标题栏:面包屑 + 运行主操作 -----------------------------------------

    fn wb_titlebar(&self, cx: &mut Context<Self>) -> Div {
        let crumb = |text: &str, active: bool| {
            div()
                .font_family(FONT_MONO)
                .text_size(fs(FS_11_5))
                .text_color(c(if active { TEXT_PRIMARY } else { TEXT_SECONDARY }))
                .child(SharedString::from(text.to_string()))
        };
        let sep = || {
            div()
                .font_family(FONT_MONO)
                .text_size(fs(FS_11_5))
                .text_color(c(TEXT_DISABLED))
                .child("›")
        };

        let primary_label = self.s.run.primary_label();
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
            .child(crumb("POE2", false))
            .child(sep())
            .child(crumb("多词缀组合", false))
            .child(sep())
            .child(crumb("可接受结果 1", false))
            .child(sep())
            .child(crumb(self.s.selected_label().as_ref(), true))
            .child(
                div()
                    .ml_auto()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        button("wb-primary", LedgerButton::Primary, primary_label, cx)
                            .h(px(H_CHIP))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_run(cx))),
                    )
                    .child(self.tier_switcher(cx)),
            )
    }

    // -- 左规则树 218 -------------------------------------------------------

    fn wb_tree(&self, cx: &mut Context<Self>) -> Div {
        let mut list = div().v_flex().py(px(6.)).flex_1().min_h_0();
        for (ix, node) in self.s.tree.iter().enumerate() {
            let state = if node.disabled {
                TreeState::Disabled
            } else if node.warning {
                TreeState::Warning
            } else if ix == self.s.selected {
                if node.depth == 0 {
                    TreeState::Active
                } else {
                    TreeState::Selected
                }
            } else {
                TreeState::Default
            };
            let row = tree_row(TreeRowSpec {
                depth: node.depth,
                state,
                expander: node.expandable.then_some(node.expanded),
                label: node.label.as_ref(),
                trailing: node.trailing.as_ref(),
                trailing_color: if node.label.as_ref() == "流放之路二" {
                    Some(ACCENT_TEXT)
                } else {
                    None
                },
            });
            list = list.child(
                div()
                    .id(("tree-row", ix))
                    .hover(|s| s.bg(c(HOVER)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.s.selected = ix;
                        cx.notify();
                    }))
                    .child(row),
            );
        }

        div()
            .w(px(218.))
            .flex_none()
            .v_flex()
            .bg(c(RAIL))
            .border_r_1()
            .border_color(c(HAIRLINE))
            .child(
                div()
                    .h(px(H_BUTTON))
                    .flex_none()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px(px(11.))
                    .border_b_1()
                    .border_color(c(HAIRLINE_SOFT))
                    .child(micro_title_sm("规则"))
                    .child(
                        div()
                            .ml_auto()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_9_5))
                            .text_color(c(TEXT_META))
                            .child("2/8 · 3/32"),
                    ),
            )
            .child(list)
            .child(
                div()
                    .flex_none()
                    .h_flex()
                    .border_t_1()
                    .border_color(c(HAIRLINE_SOFT))
                    .child(
                        div()
                            .id("tree-add-plan")
                            .flex_1()
                            .h(px(H_BUTTON))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .text_size(fs(FS_11_5))
                            .text_color(c(TEXT_SECONDARY))
                            .border_r_1()
                            .border_color(c(HAIRLINE_SOFT))
                            .hover(|s| s.bg(c(PRESSED)))
                            .child(div().text_color(c(ACCENT)).child("+"))
                            .child("方案"),
                    )
                    .child(
                        div()
                            .id("tree-add-affix")
                            .flex_1()
                            .h(px(H_BUTTON))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .text_size(fs(FS_11_5))
                            .text_color(c(TEXT_SECONDARY))
                            .hover(|s| s.bg(c(PRESSED)))
                            .child(div().text_color(c(ACCENT)).child("+"))
                            .child("词缀"),
                    ),
            )
    }

    // -- 中编辑区 -----------------------------------------------------------

    fn wb_editor(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w_0()
            .v_flex()
            .bg(c(PANEL))
            .child(self.wb_editor_tabs(cx))
            .child(match self.s.editor_tab {
                EditorTab::Conditions => self.wb_tab_conditions(cx),
                EditorTab::Region => self.wb_tab_region(cx),
                EditorTab::Alerts => self.wb_tab_placeholder(
                    "提醒与显示",
                    "声音 / 浮窗 / 录屏可见性设置将在下一轮接入;当前沿用设置文件中的既有值",
                ),
            })
    }

    fn wb_editor_tabs(&self, cx: &mut Context<Self>) -> Div {
        let tab = |id: &'static str,
                   label: &'static str,
                   this_tab: EditorTab,
                   current: EditorTab,
                   cx: &mut Context<Self>| {
            let active = this_tab == current;
            let mut t = div()
                .id(id)
                .px(px(14.))
                .h_full()
                .flex()
                .items_center()
                .text_size(fs(FS_12))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.s.editor_tab = this_tab;
                    cx.notify();
                }));
            t = if active {
                t.bg(c(PANEL))
                    .text_color(c(TEXT_PRIMARY))
                    .border_b_2()
                    .border_color(c(ACCENT))
            } else {
                t.text_color(c(TEXT_META)).hover(|s| s.bg(c(HOVER)))
            };
            t.child(label)
        };

        div()
            .h(px(H_TITLEBAR))
            .flex_none()
            .h_flex()
            .bg(c(RAIL))
            .border_b_1()
            .border_color(c(HAIRLINE))
            .child(tab(
                "tab-cond",
                "词缀条件",
                EditorTab::Conditions,
                self.s.editor_tab,
                cx,
            ))
            .child(tab(
                "tab-region",
                "识别区域",
                EditorTab::Region,
                self.s.editor_tab,
                cx,
            ))
            .child(tab(
                "tab-alert",
                "提醒与显示",
                EditorTab::Alerts,
                self.s.editor_tab,
                cx,
            ))
            .child(
                div()
                    .ml_auto()
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .font_family(FONT_MONO)
                    .text_size(fs(FS_9_5))
                    .text_color(c(TEXT_META))
                    .child("settings.json")
                    .child(match &self.notice {
                        Some((kind, text)) => div()
                            .text_color(c(kind.text()))
                            .whitespace_nowrap()
                            .child(text.clone()),
                        None => div().text_color(c(TEXT_META)).child("未改动"),
                    }),
            )
    }

    fn wb_tab_placeholder(&self, title: &str, hint: &str) -> Div {
        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .gap_2()
            .p_4()
            .child(
                div()
                    .text_size(fs(FS_13))
                    .font_semibold()
                    .child(SharedString::from(title.to_string())),
            )
            .child(
                div()
                    .text_size(fs(FS_11_5))
                    .text_color(c(TEXT_META))
                    .child(SharedString::from(hint.to_string())),
            )
    }

    fn wb_tab_region(&mut self, cx: &mut Context<Self>) -> Div {
        let (region, ocr) = match &self.backend {
            Some(b) => (b.region_label(), b.ocr_language_label()),
            None => ("—".to_owned(), "—".to_owned()),
        };
        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .gap(px(10.))
            .p_4()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(LABEL_COL))
                            .flex_none()
                            .text_size(fs(FS_11_5))
                            .text_color(c(TEXT_META))
                            .child("当前区域"),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_12))
                            .text_color(c(TEXT_PRIMARY))
                            .child(SharedString::from(region)),
                    )
                    .child(
                        button("wb-select-region", LedgerButton::Secondary, "框选区域", cx)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.begin_region_selection(cx)),
                            ),
                    )
                    .child(hotkey_chips(&["Ctrl", "⇧", "F11"])),
            )
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(LABEL_COL))
                            .flex_none()
                            .text_size(fs(FS_11_5))
                            .text_color(c(TEXT_META))
                            .child("OCR 语言"),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_12))
                            .text_color(c(TEXT_PRIMARY))
                            .child(SharedString::from(ocr)),
                    ),
            )
            .child(warning_band(
                "提示",
                "框选时只圈装备提示框中的词缀区域;区域越小,识别越快。框选完成后自动保存。",
            ))
    }

    fn wb_tab_conditions(&mut self, cx: &mut Context<Self>) -> Div {
        let error = self.range_error(cx);

        // 条件名称 + 什么时候提醒
        let name_row = div()
            .h_flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .w(px(LABEL_COL))
                    .flex_none()
                    .text_size(fs(FS_11_5))
                    .text_color(c(TEXT_META))
                    .whitespace_nowrap()
                    .child("条件名称"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.s.name_input)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(fs(FS_11_5))
                    .text_color(c(TEXT_META))
                    .whitespace_nowrap()
                    .child("什么时候提醒"),
            )
            .child(segmented(&["任意", "全部", "指定条数"], 2, 26.))
            .child(
                div()
                    .flex_none()
                    .font_family(FONT_MONO)
                    .text_size(fs(FS_10))
                    .text_color(c(TEXT_META))
                    .whitespace_nowrap()
                    .child("/ 共 2 条"),
            );

        // 完整词缀模板(左 2px accent 竖条 = 「这是被匹配的原文」)
        let template_row = div()
            .h_flex()
            .items_start()
            .gap(px(10.))
            .child(
                div()
                    .w(px(LABEL_COL))
                    .flex_none()
                    .pt(px(6.))
                    .text_size(fs(FS_11_5))
                    .text_color(c(TEXT_META))
                    .child("完整词缀模板"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .border_l_2()
                    .border_color(c(ACCENT))
                    .child(Input::new(&self.s.template_input)),
            );

        let normalized = div()
            .pl(px(LABEL_COL + 10.))
            .font_family(FONT_MONO)
            .text_size(fs(FS_10_5))
            .text_color(c(TEXT_META))
            .child(self.normalized_preview(cx));

        // 数值条件表:104 / 1fr / 110 / 110 / 60
        let mut table = div().v_flex().border_1().border_color(c(HAIRLINE));
        table = table.child(
            div()
                .h_flex()
                .bg(c(RAIL))
                .border_b_1()
                .border_color(c(HAIRLINE))
                .font_family(FONT_MONO)
                .text_size(fs(FS_9_5))
                .text_color(c(TEXT_META))
                .child(
                    div()
                        .w(px(104.))
                        .flex_none()
                        .px(px(10.))
                        .py(px(6.))
                        .child("数值"),
                )
                .child(div().flex_1().px(px(10.)).py(px(6.)).child("比较方式"))
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .px(px(10.))
                        .py(px(6.))
                        .child("下限"),
                )
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .px(px(10.))
                        .py(px(6.))
                        .child("上限"),
                )
                .child(div().w(px(60.)).flex_none()),
        );
        for (ix, row) in self.s.value_rows.iter().enumerate() {
            let is_last = ix + 1 == self.s.value_rows.len();
            let mut r = div().h_flex().items_center().bg(c(WELL));
            if !is_last {
                r = r.border_b_1().border_color(c(HAIRLINE_SOFT));
            }
            table = table.child(
                r.child(
                    div()
                        .w(px(104.))
                        .flex_none()
                        .px(px(10.))
                        .font_family(FONT_MONO)
                        .text_size(fs(FS_12))
                        .text_color(c(TEXT_META))
                        .whitespace_nowrap()
                        .child(row.label.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .px(px(10.))
                        .h_flex()
                        .items_center()
                        .gap_1()
                        .text_size(fs(FS_12))
                        .child(row.comparison.clone())
                        .child(div().text_size(px(8.)).text_color(c(TEXT_META)).child("▾")),
                )
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .p(px(5.))
                        .child(Input::new(&row.min)),
                )
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .p(px(5.))
                        .child(Input::new(&row.max)),
                )
                .child(
                    div()
                        .w(px(60.))
                        .flex_none()
                        .px(px(10.))
                        .text_size(fs(FS_11_5))
                        .text_color(c(TEXT_META))
                        .child("删除"),
                ),
            );
        }
        // 错误行:占位始终保留一行高度,出现时不推挤下方内容。
        table = table.child(match error {
            Some(msg) => error_band(msg),
            None => div().h(px(H_ROW + 4.)).flex_none().bg(c(WELL)),
        });

        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .gap(px(10.))
            .p_4()
            .child(name_row)
            .child(template_row)
            .child(normalized)
            .child(
                div()
                    .mt(px(SP_8))
                    .h_flex()
                    .items_center()
                    .gap(px(10.))
                    .child(micro_title_sm("数值条件"))
                    .child(div().flex_1().h(px(1.)).bg(c(HAIRLINE_SOFT)))
                    .child(button("wb-add-value", LedgerButton::Quiet, "+ 添加数值", cx)),
            )
            .child(table)
            .child(warning_band(
                "注意",
                "催化剂、品质或特殊效果会改变屏幕显示值。数值条件只比较屏幕上实际显示的值,不推算基础值。",
            ))
    }

    // -- 右运行栏 318 -------------------------------------------------------

    fn wb_run_rail(&self, cx: &mut Context<Self>) -> Div {
        div()
            .w(px(318.))
            .flex_none()
            .v_flex()
            .bg(c(RAIL))
            .border_l_1()
            .border_color(c(HAIRLINE))
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
                            .child("繁中 · Windows OCR"),
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
            .child(
                div()
                    .h(px(H_BUTTON))
                    .flex_none()
                    .h_flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(c(HAIRLINE_SOFT))
                    .child(micro_title_sm("截图识别结果"))
                    .child(
                        div()
                            .ml_auto()
                            .text_size(fs(FS_11_5))
                            .text_color(c(ACCENT))
                            .child("识别截图"),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.log_block(14)))
            .child(
                div()
                    .flex_none()
                    .h_flex()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(c(HAIRLINE_SOFT))
                    .child(div().flex_1().child(
                        button("wb-save", LedgerButton::Secondary, "验证并保存", cx).w_full(),
                    ))
                    .child(button("wb-undo", LedgerButton::Secondary, "撤销", cx).w(px(74.))),
            )
    }
}
