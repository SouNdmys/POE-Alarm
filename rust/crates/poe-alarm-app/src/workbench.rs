//! Workbench 1180×620(章节 09):
//! 标题栏 30 | 左规则树 218 | 中编辑区(tab 30 + 内容) | 右运行栏 318 | 状态栏 24。

use gpui::{Context, Div, SharedString, Window, div, prelude::*, px};
use gpui_component::{StyledExt, input::Input};

use crate::shell::AppShell;
use crate::state::*;
use crate::theme::*;
use crate::ui::*;

impl AppShell {
    pub fn render_workbench(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // 数值行跟随模板占位数(粘贴/清空模板后行数即时对齐)。
        self.ensure_value_rows(window, cx);
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

        let primary_label = self.t().primary_label(self.s.run);
        let game_crumb = match self.backend.as_ref().map(|b| b.settings.selected_game_profile) {
            Some(poe_alarm_settings::GameProfile::Poe1) => "POE1",
            _ => "POE2",
        };
        div()
            .h(px(H_TITLEBAR + 6.))
            .flex_none()
            .h_flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .bg(c(RAIL))
            .border_b_1()
            .border_color(c(HAIRLINE))
            .child(div().size(px(9.)).flex_none().bg(c(ACCENT)))
            .child(crumb(game_crumb, false))
            .child(sep())
            .child(crumb(self.s.selected_label().as_ref(), true))
            .child(
                div()
                    .ml_auto()
                    .h_flex()
                    .items_center()
                    .gap(px(14.))
                    .child(self.profile_switcher(cx))
                    .child(
                        button("wb-primary", LedgerButton::Primary, primary_label, cx)
                            .h(px(H_CHIP))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_run(cx))),
                    ),
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
                trailing_color: (node.depth == 0).then_some(ACCENT_TEXT),
            });
            list = list.child(
                div()
                    .id(("tree-row", ix))
                    .hover(|s| s.bg(c(HOVER)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_tree_node(ix, window, cx);
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
                    .child(micro_title_sm(self.t().rules_title)),
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_group(window, cx);
                            }))
                            .child(div().text_color(c(ACCENT)).child("+"))
                            .child(self.t().add_result),
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_condition(window, cx);
                            }))
                            .child(div().text_color(c(ACCENT)).child("+"))
                            .child(self.t().add_condition),
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
                EditorTab::Alerts => self.wb_tab_alerts(cx),
                EditorTab::Help => self.wb_tab_help(cx),
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

        let text = self.t();
        div()
            .h(px(H_TITLEBAR))
            .flex_none()
            .h_flex()
            .bg(c(RAIL))
            .border_b_1()
            .border_color(c(HAIRLINE))
            .child(tab(
                "tab-cond",
                text.tab_conditions,
                EditorTab::Conditions,
                self.s.editor_tab,
                cx,
            ))
            .child(tab(
                "tab-region",
                text.tab_region,
                EditorTab::Region,
                self.s.editor_tab,
                cx,
            ))
            .child(tab(
                "tab-alert",
                text.tab_alerts,
                EditorTab::Alerts,
                self.s.editor_tab,
                cx,
            ))
            .child(tab(
                "tab-help",
                text.tab_help,
                EditorTab::Help,
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
                        Some((kind, notice)) => div()
                            .text_color(c(kind.text()))
                            .whitespace_nowrap()
                            .child(notice.clone()),
                        None => div().text_color(c(TEXT_META)).child(text.unchanged),
                    }),
            )
    }

    /// 使用说明:上手流程、热键、规则语义、繁中 OCR 安装建议与作者信息。
    fn wb_tab_help(&mut self, cx: &mut Context<Self>) -> Div {
        const OCR_INSTALL_COMMAND: &str =
            r#"Add-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0""#;
        const OCR_VERIFY_COMMAND: &str =
            r#"Get-WindowsCapability -Online -Name "Language.OCR~~~zh-TW~0.0.1.0""#;
        const OCR_DOC_URL: &str =
            "https://learn.microsoft.com/zh-cn/windows-hardware/manufacture/desktop/add-language-packs-to-windows";
        const REPO_URL: &str = "https://github.com/SouNdmys/POE-Alarm";
        const CONTACT: &str = "soundmys1994@gmail.com";

        let section = |title: &'static str| {
            div()
                .v_flex()
                .gap(px(6.))
                .child(micro_title_sm(title))
                .child(div().h(px(1.)).bg(c(HAIRLINE_SOFT)))
        };
        let line = |text: &'static str| {
            div()
                .text_size(fs(FS_11_5))
                .text_color(c(TEXT_SECONDARY))
                .child(text)
        };
        let hotkey_row = |keys: &'static str, what: &'static str| {
            div()
                .h_flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(104.))
                        .flex_none()
                        .font_family(FONT_MONO)
                        .text_size(fs(FS_11_5))
                        .text_color(c(TEXT_PRIMARY))
                        .child(keys),
                )
                .child(
                    div()
                        .text_size(fs(FS_11_5))
                        .text_color(c(TEXT_SECONDARY))
                        .child(what),
                )
        };
        let mono_block = |text: &'static str| {
            div()
                .px(px(10.))
                .py(px(6.))
                .bg(c(WELL))
                .border_1()
                .border_color(c(HAIRLINE))
                .font_family(FONT_MONO)
                .text_size(fs(FS_11_5))
                .text_color(c(TEXT_PRIMARY))
                .child(text)
        };
        let link = |id: &'static str, label: &'static str, url: &'static str, cx: &mut Context<Self>| {
            div()
                .id(id)
                .text_size(fs(FS_11_5))
                .text_color(c(ACCENT_TEXT))
                .hover(|s| s.bg(c(HOVER)))
                .on_click(cx.listener(move |_, _, _, cx| cx.open_url(url)))
                .child(label)
        };

        let t = self.t();
        let steps = {
            let mut column = div().v_flex().gap(px(4.));
            for step in t.help_steps {
                column = column.child(line(step));
            }
            column
        };
        let rules = {
            let mut column = div().v_flex().gap(px(4.));
            for item in t.help_rules {
                column = column.child(line(item));
            }
            column
        };
        let hud = {
            let mut column = div().v_flex().gap(px(4.));
            for item in t.help_hud {
                column = column.child(line(item));
            }
            column
        };
        div().flex_1().min_h_0().child(
            div()
                .id("wb-help-scroll")
                .size_full()
                .overflow_y_scroll()
                .child(
                    div()
                        .v_flex()
                        .gap(px(14.))
                        .p_4()
                        .child(section(t.help_quick_start))
                        .child(steps)
                        .child(section(t.help_hotkeys))
                        .child(
                            div()
                                .v_flex()
                                .gap(px(3.))
                                .child(hotkey_row("Ctrl⇧F10", t.help_hotkey_start))
                                .child(hotkey_row("Ctrl⇧F11", t.help_hotkey_select))
                                .child(hotkey_row("Ctrl⇧F12", t.help_hotkey_stop)),
                        )
                        .child(section(t.help_rules_title))
                        .child(rules)
                        .child(section(t.help_hud_title))
                        .child(hud)
                        .child(section(t.help_ocr_title))
                        .child(
                            div()
                                .v_flex()
                                .gap(px(6.))
                                .child(line(t.help_ocr_intro))
                                .child(
                                    div()
                                        .h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(mono_block(OCR_INSTALL_COMMAND))
                                        .child(
                                            button(
                                                "help-copy-install",
                                                LedgerButton::Quiet,
                                                t.help_copy,
                                                cx,
                                            )
                                            .on_click(cx.listener(|_, _, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui::ClipboardItem::new_string(
                                                        OCR_INSTALL_COMMAND.to_owned(),
                                                    ),
                                                );
                                            })),
                                        ),
                                )
                                .child(line(t.help_ocr_verify))
                                .child(
                                    div()
                                        .h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(mono_block(OCR_VERIFY_COMMAND))
                                        .child(
                                            button(
                                                "help-copy-verify",
                                                LedgerButton::Quiet,
                                                t.help_copy,
                                                cx,
                                            )
                                            .on_click(cx.listener(|_, _, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui::ClipboardItem::new_string(
                                                        OCR_VERIFY_COMMAND.to_owned(),
                                                    ),
                                                );
                                            })),
                                        ),
                                )
                                .child(
                                    div()
                                        .h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(line(t.help_ocr_settings_path))
                                        .child(link("help-ms-doc", t.help_ocr_doc_link, OCR_DOC_URL, cx)),
                                ),
                        )
                        .child(section(t.help_about_title))
                        .child(
                            div()
                                .v_flex()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_size(fs(FS_11_5))
                                        .text_color(c(TEXT_SECONDARY))
                                        .child(SharedString::from(
                                            t.help_about_fmt
                                                .replacen("{}", env!("CARGO_PKG_VERSION"), 1),
                                        )),
                                )
                                .child(
                                    div()
                                        .h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(line(t.help_contact))
                                        .child(link(
                                            "help-mail",
                                            CONTACT,
                                            "mailto:soundmys1994@gmail.com",
                                            cx,
                                        ))
                                        .child(line(t.help_homepage))
                                        .child(link(
                                            "help-repo",
                                            "github.com/SouNdmys/POE-Alarm",
                                            REPO_URL,
                                            cx,
                                        )),
                                )
                                .child(line(t.help_notice)),
                        ),
                ),
        )
    }

    /// 提醒与显示:浮窗显隐、录屏可见性、提示音、界面语言、热键。改动即保存。
    fn wb_tab_alerts(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t();
        let (keep_hud, allow_capture, sound, hotkey, english_ui) = match &self.backend {
            Some(b) => (
                b.settings.keep_hud_visible,
                b.settings.allow_overlay_capture,
                b.sound_label().unwrap_or_else(|| t.sound_builtin.to_owned()),
                b.hotkey_label(),
                b.settings.ui_language.starts_with("en"),
            ),
            None => (true, true, "—".to_owned(), "—".to_owned(), false),
        };
        let label = |caption: &'static str| {
            div()
                .w(px(LABEL_COL))
                .flex_none()
                .text_size(fs(FS_11_5))
                .text_color(c(TEXT_META))
                .whitespace_nowrap()
                .child(caption)
        };
        // 两态分段开关(与比较方式分段同一形制)。
        let toggle = |id_on: &'static str,
                      id_off: &'static str,
                      on_label: &'static str,
                      off_label: &'static str,
                      on: bool,
                      cx: &mut Context<Self>,
                      set: fn(&mut Self, bool, &mut Context<Self>)| {
            let cell = |id: &'static str,
                        text: &'static str,
                        active: bool,
                        value: bool,
                        first: bool,
                        cx: &mut Context<Self>| {
                let mut cell = div()
                    .id(id)
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .text_size(fs(FS_11_5))
                    .whitespace_nowrap()
                    .on_click(cx.listener(move |this, _, _, cx| set(this, value, cx)));
                if !first {
                    cell = cell.border_l_1().border_color(c(HAIRLINE));
                }
                cell = if active {
                    cell.bg(c(ACCENT_WASH)).text_color(c(ACCENT_TEXT))
                } else {
                    cell.bg(c(PANEL))
                        .text_color(c(TEXT_SECONDARY))
                        .hover(|s| s.bg(c(HOVER)))
                };
                cell.child(text)
            };
            div()
                .h_flex()
                .flex_none()
                .border_1()
                .border_color(c(HAIRLINE))
                .child(cell(id_on, on_label, on, true, true, cx))
                .child(cell(id_off, off_label, !on, false, false, cx))
        };
        let row = |label_el: Div, control: Div, hint: &'static str| {
            div()
                .h_flex()
                .items_center()
                .gap(px(10.))
                .child(label_el)
                .child(control)
                .child(
                    div()
                        .text_size(fs(FS_10_5))
                        .text_color(c(TEXT_META))
                        .child(hint),
                )
        };
        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .gap(px(12.))
            .p_4()
            .child(row(
                label(t.hud_row),
                toggle(
                    "al-hud-on",
                    "al-hud-off",
                    t.hud_keep,
                    t.hud_hide,
                    keep_hud,
                    cx,
                    |this, v, cx| this.set_keep_hud_visible(v, cx),
                ),
                t.hud_row_hint,
            ))
            .child(row(
                label(t.capture_row),
                toggle(
                    "al-cap-on",
                    "al-cap-off",
                    t.capture_visible,
                    t.capture_hidden,
                    allow_capture,
                    cx,
                    |this, v, cx| this.set_allow_overlay_capture(v, cx),
                ),
                t.capture_row_hint,
            ))
            .child(row(
                label(t.ui_language_row),
                toggle(
                    "al-ui-zh",
                    "al-ui-en",
                    t.ui_language_zh,
                    t.ui_language_en,
                    !english_ui,
                    cx,
                    |this, v, cx| this.set_ui_language(if v { "zh-CN" } else { "en" }, cx),
                ),
                t.ui_language_hint,
            ))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap(px(10.))
                    .child(label(t.sound_row))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_12))
                            .text_color(c(TEXT_PRIMARY))
                            .whitespace_nowrap()
                            .child(SharedString::from(sound)),
                    )
                    .child(
                        button("al-sound-pick", LedgerButton::Secondary, t.sound_pick, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.choose_alert_sound(cx))),
                    )
                    .child(
                        button("al-sound-reset", LedgerButton::Quiet, t.sound_reset, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.clear_alert_sound(cx))),
                    ),
            )
            .child(row(
                label(t.hotkey_row),
                div()
                    .font_family(FONT_MONO)
                    .text_size(fs(FS_12))
                    .text_color(c(TEXT_PRIMARY))
                    .whitespace_nowrap()
                    .child(SharedString::from(hotkey)),
                t.hotkey_row_hint,
            ))
            .child(warning_band(t.alerts_note_tag, t.alerts_note))
    }

    fn wb_tab_region(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t();
        let (region, ocr) = match &self.backend {
            Some(b) => (
                b.region_label().unwrap_or_else(|| t.region_unset.to_owned()),
                b.ocr_language_label(),
            ),
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
                            .child(t.current_region),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_12))
                            .text_color(c(TEXT_PRIMARY))
                            .child(SharedString::from(region)),
                    )
                    .child(
                        button(
                            "wb-select-region",
                            LedgerButton::Secondary,
                            t.select_region_button,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.begin_region_selection(cx))),
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
                            .child(t.ocr_language_row),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_12))
                            .text_color(c(TEXT_PRIMARY))
                            .child(SharedString::from(ocr)),
                    ),
            )
            .child(warning_band(t.hint_tag, t.region_hint))
    }

    fn wb_tab_conditions(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.t();
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
                    .child(t.condition_name),
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
                    .child(t.group_rule),
            )
            .child({
                use poe_alarm_core::ResultGroupMode as G;
                let summary = self.selected_group_summary();
                let current = match summary.map(|(mode, _, _)| mode) {
                    Some(G::Any) => 0,
                    Some(G::All) => 1,
                    Some(G::AtLeast) => 2,
                    None => 0,
                };
                let mut row = div()
                    .h_flex()
                    .flex_none()
                    .border_1()
                    .border_color(c(HAIRLINE));
                for (i, (label, mode)) in [
                    (t.group_any, G::Any),
                    (t.group_all, G::All),
                    (t.group_at_least, G::AtLeast),
                ]
                .into_iter()
                .enumerate()
                {
                    let mut cell = div()
                        .id(("wb-gmode", i))
                        .h(px(24.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .text_size(fs(FS_11_5))
                        .whitespace_nowrap()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_group_mode(mode, window, cx);
                        }));
                    if i > 0 {
                        cell = cell.border_l_1().border_color(c(HAIRLINE));
                    }
                    cell = if i == current {
                        cell.bg(c(ACCENT_WASH)).text_color(c(ACCENT_TEXT))
                    } else {
                        cell.bg(c(PANEL))
                            .text_color(c(TEXT_SECONDARY))
                            .hover(|s| s.bg(c(HOVER)))
                    };
                    row = row.child(cell.child(label));
                }
                row
            })
            .child({
                // "指定条数"步进 + 真实条数;其他模式只显示共 N 条。
                let summary = self.selected_group_summary();
                let total = summary.map(|(_, _, n)| n).unwrap_or(0);
                let stepper = |id: &'static str,
                               label: &'static str,
                               delta: i64,
                               cx: &mut Context<Self>| {
                    div()
                        .id(id)
                        .w(px(20.))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(c(HAIRLINE))
                        .text_size(fs(FS_11_5))
                        .text_color(c(TEXT_SECONDARY))
                        .hover(|s| s.bg(c(HOVER)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.adjust_required_count(delta, window, cx);
                        }))
                        .child(label)
                };
                let mut wrap = div()
                    .flex_none()
                    .h_flex()
                    .items_center()
                    .gap_1()
                    .font_family(FONT_MONO)
                    .text_size(fs(FS_10))
                    .text_color(c(TEXT_META))
                    .whitespace_nowrap();
                if let Some((poe_alarm_core::ResultGroupMode::AtLeast, required, _)) = summary {
                    wrap = wrap
                        .child(t.hit_of)
                        .child(stepper("wb-req-dec", "−", -1, cx))
                        .child(
                            div()
                                .text_color(c(ACCENT_TEXT))
                                .child(SharedString::from(format!("{required}"))),
                        )
                        .child(stepper("wb-req-inc", "+", 1, cx));
                }
                wrap.child(SharedString::from(
                    t.total_conditions_fmt.replacen("{}", &total.to_string(), 1),
                ))
            });

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
                    .child(t.condition_template),
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

        // 数值条件表:72 / 比较方式(1fr 分段) / 110 / 110。
        // 行数由模板占位数决定(自动生成,默认不限制),无手动增删。
        use poe_alarm_core::NumericConstraintMode as M;
        let modes: [(&str, M); 5] = [
            (t.numeric_ignore, M::Ignore),
            (t.numeric_range, M::RangeInclusive),
            ("≥", M::AtLeast),
            ("≤", M::AtMost),
            ("=", M::Exactly),
        ];
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
                        .w(px(72.))
                        .flex_none()
                        .px(px(10.))
                        .py(px(6.))
                        .child(t.numeric_column),
                )
                .child(div().flex_1().px(px(10.)).py(px(6.)).child(t.comparison_column))
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .px(px(10.))
                        .py(px(6.))
                        .child(t.lower_column),
                )
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .px(px(10.))
                        .py(px(6.))
                        .child(t.upper_column),
                ),
        );
        if self.s.value_rows.is_empty() {
            table = table.child(
                div()
                    .h_flex()
                    .items_center()
                    .px(px(10.))
                    .h(px(H_ROW + 8.))
                    .bg(c(WELL))
                    .text_size(fs(FS_11_5))
                    .text_color(c(TEXT_META))
                    .child(t.no_numeric_slots),
            );
        }
        for (ix, row) in self.s.value_rows.iter().enumerate() {
            let is_last = ix + 1 == self.s.value_rows.len();
            let mode = row.mode;
            let uses_min = matches!(mode, M::RangeInclusive | M::AtLeast | M::Exactly);
            let uses_max = matches!(mode, M::RangeInclusive | M::AtMost);
            let mut r = div().h_flex().items_center().bg(c(WELL));
            if !is_last {
                r = r.border_b_1().border_color(c(HAIRLINE_SOFT));
            }
            // 比较方式:五档分段,点击即切换。
            let mut picker = div()
                .h_flex()
                .flex_none()
                .border_1()
                .border_color(c(HAIRLINE));
            for (i, (label, m)) in modes.into_iter().enumerate() {
                let mut cell = div()
                    .id(("wb-cmp", ix * modes.len() + i))
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .text_size(fs(FS_11_5))
                    .whitespace_nowrap()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_value_row_mode(ix, m, cx);
                    }));
                if i > 0 {
                    cell = cell.border_l_1().border_color(c(HAIRLINE));
                }
                cell = if m == mode {
                    cell.bg(c(ACCENT_WASH)).text_color(c(ACCENT_TEXT))
                } else {
                    cell.bg(c(PANEL))
                        .text_color(c(TEXT_SECONDARY))
                        .hover(|s| s.bg(c(HOVER)))
                };
                picker = picker.child(cell.child(label));
            }
            let bound_cell = |input: &gpui::Entity<gpui_component::input::InputState>,
                              used: bool| {
                let mut cell = div().w(px(110.)).flex_none().p(px(5.));
                if used {
                    cell = cell.child(Input::new(input));
                } else {
                    cell = cell.child(
                        div()
                            .h(px(H_ROW - 2.))
                            .flex()
                            .items_center()
                            .px(px(6.))
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_12))
                            .text_color(c(TEXT_DISABLED))
                            .child("—"),
                    );
                }
                cell
            };
            table = table.child(
                r.child(
                    div()
                        .w(px(72.))
                        .flex_none()
                        .px(px(10.))
                        .font_family(FONT_MONO)
                        .text_size(fs(FS_12))
                        .text_color(c(TEXT_META))
                        .whitespace_nowrap()
                        .child(SharedString::from(
                            t.numeric_slot_fmt.replacen("{}", &(ix + 1).to_string(), 1),
                        )),
                )
                .child(div().flex_1().px(px(10.)).py(px(4.)).child(picker))
                .child(bound_cell(&row.min, uses_min))
                .child(bound_cell(&row.max, uses_max)),
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
                    .child(micro_title_sm(t.numeric_rules_label))
                    .child(
                        div()
                            .text_size(fs(FS_10))
                            .text_color(c(TEXT_META))
                            .whitespace_nowrap()
                            .child(t.numeric_rules_hint),
                    )
                    .child(div().flex_1().h(px(1.)).bg(c(HAIRLINE_SOFT)))
                    .child(
                        button("wb-del-cond", LedgerButton::Destructive, t.delete_condition, cx)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.remove_selected_condition(window, cx)
                            })),
                    )
                    .child(
                        button("wb-del-group", LedgerButton::Destructive, t.delete_result, cx)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.remove_selected_group(window, cx)
                            })),
                    ),
            )
            .child(table)
            .child(warning_band(t.notice_tag, t.numeric_value_help))
    }

    // -- 右运行栏 318 -------------------------------------------------------

    fn wb_run_rail(&self, cx: &mut Context<Self>) -> Div {
        let t = self.t();
        let ocr_label = match &self.backend {
            Some(b) if b.ocr_language_label().starts_with("zh") => t.ocr_zh,
            _ => t.ocr_en,
        };
        let engine = t.engine_fmt.replacen("{}", ocr_label, 1);
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
                    .child(micro_title_sm(t.run_title))
                    .child(
                        div()
                            .ml_auto()
                            .font_family(FONT_MONO)
                            .text_size(fs(FS_9_5))
                            .text_color(c(TEXT_META))
                            .child(SharedString::from(engine)),
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
                    .child(micro_title_sm(t.screenshot_title))
                    .child(
                        div()
                            .id("wb-shot")
                            .ml_auto()
                            .text_size(fs(FS_11_5))
                            .text_color(c(ACCENT))
                            .hover(|s| s.bg(c(HOVER)))
                            .on_click(cx.listener(|this, _, _, cx| this.test_screenshot(cx)))
                            .child(t.screenshot_button),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.log_block(15)))
    }
}
