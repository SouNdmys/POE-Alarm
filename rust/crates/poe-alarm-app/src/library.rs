//! Personal reusable rules and manual-test explanations; never used in the monitor loop.
use crate::{
    shell::AppShell,
    state::{EditorTab, NodeRef},
    theme::*,
    ui::*,
};
use gpui::{Context, Div, SharedString, Window, div, prelude::*, px};
use gpui_component::{Disableable, StyledExt, checkbox::Checkbox, input::Input};
use poe_alarm_core::{
    AcceptableResultGroup, AffixCondition, CompiledRuleSet, NumericConstraintMode as M,
};
use poe_alarm_settings::LibraryEntry;

impl AppShell {
    fn can_grow_saved_rules(
        &mut self,
        extra_groups: usize,
        extra_conditions: usize,
        replaced_conditions: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let rules = self
            .backend
            .as_ref()
            .and_then(|b| b.settings.selected_rules().structured_rule_set.as_ref());
        let group_count = rules.map_or(0, |rules| rules.groups.len());
        let condition_count = rules.map_or(0, |rules| {
            rules
                .groups
                .iter()
                .map(|group| group.conditions.len())
                .sum::<usize>()
        });
        if saved_rule_growth_fits(
            group_count,
            condition_count,
            extra_groups,
            extra_conditions,
            replaced_conditions,
        ) {
            return true;
        }
        self.notice=Some((StatusKind::Warning,self.word("当前规则最多保存 128 个方案、1024 条词缀；请先整理规则，收藏库内容会保留", "Current rules can store up to 128 plans and 1024 affixes; organize them first. Your library is retained").into()));
        cx.notify();
        false
    }
    pub fn undo_rule_deletion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.condition_selection_locked() || !self.validate_editor(cx) {
            return;
        }
        // A newer edit invalidates the snapshot; preserve it before Undo can
        // restore a whole rule set from before that edit.
        if self.apply_editor_to_selection(cx) {
            self.persist();
            cx.notify();
            return;
        }
        if let Some(rules) = self.undo_rules.take()
            && let Some(backend) = &mut self.backend
        {
            backend.settings.selected_rules_mut().structured_rule_set = Some(rules);
        }
        self.persist();
        self.refresh_tree_select_last(window, cx);
    }
    pub(crate) fn render_plan_summary(&self, cx: &mut Context<Self>) -> Div {
        let title = match self.selected_node() {
            NodeRef::Game => self.word(
                "选择左侧词缀进行编辑，或添加一个方案",
                "Select an affix to edit, or add a plan",
            ),
            _ => self.word("方案设置", "Plan settings"),
        };
        let mut panel = div()
            .flex_1()
            .v_flex()
            .p_4()
            .gap_3()
            .child(div().text_size(fs(FS_13)).child(title));
        if let Some((mode, required, enabled)) = self.selected_group_summary() {
            let mut modes = div().h_flex().gap_2();
            for (index, (label, value)) in [
                (
                    self.word("任意", "Any"),
                    poe_alarm_core::ResultGroupMode::Any,
                ),
                (
                    self.word("全部", "All"),
                    poe_alarm_core::ResultGroupMode::All,
                ),
                (
                    self.word("指定条数", "At least"),
                    poe_alarm_core::ResultGroupMode::AtLeast,
                ),
            ]
            .into_iter()
            .enumerate()
            {
                modes = modes.child(
                    button(
                        ("plan-mode", index),
                        if value == mode {
                            LedgerButton::Primary
                        } else {
                            LedgerButton::Secondary
                        },
                        label,
                        cx,
                    )
                    .disabled(self.condition_selection_locked())
                    .on_click(cx.listener(move |this, _, w, cx| this.set_group_mode(value, w, cx))),
                );
            }
            panel = panel
                .child(modes)
                .child(div().child(SharedString::from(format!(
                    "{} {enabled} · {} {required}",
                    self.word("已启用", "Enabled"),
                    self.word("指定条数", "Required")
                ))))
                .when(mode == poe_alarm_core::ResultGroupMode::AtLeast, |this| {
                    this.child(
                        div()
                            .h_flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                button("plan-required-less", LedgerButton::Secondary, "−", cx)
                                    .disabled(self.condition_selection_locked())
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.adjust_required_count(-1, w, cx)
                                    })),
                            )
                            .child(
                                button("plan-required-more", LedgerButton::Secondary, "+", cx)
                                    .disabled(self.condition_selection_locked())
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.adjust_required_count(1, w, cx)
                                    })),
                            ),
                    )
                });
            panel = panel.child(
                button(
                    "plan-remove",
                    LedgerButton::Destructive,
                    self.word("删除方案", "Delete plan"),
                    cx,
                )
                .disabled(self.condition_selection_locked())
                .on_click(cx.listener(|this, _, w, cx| this.remove_selected_group(w, cx))),
            );
        }
        panel.child(
            button(
                "plan-open-add",
                LedgerButton::Secondary,
                self.word("添加词缀", "Add affix"),
                cx,
            )
            .disabled(self.condition_selection_locked())
            .on_click(cx.listener(|this, _, _, cx| this.open_import(cx))),
        )
    }

    pub fn open_library(&mut self, cx: &mut Context<Self>) {
        if self.condition_selection_locked() || !self.validate_editor(cx) {
            return;
        }
        if !self.flush_editor(cx) {
            return;
        }
        self.s.editor_tab = EditorTab::Library;
        cx.notify();
    }

    pub fn open_import(&mut self, cx: &mut Context<Self>) {
        if self.condition_selection_locked() || !self.validate_editor(cx) {
            return;
        }
        if !self.flush_editor(cx) {
            return;
        }
        self.s.editor_tab = EditorTab::Import;
        self.parse_import_item(cx);
        cx.notify();
    }

    fn parse_import_item(&mut self, cx: &mut Context<Self>) {
        match poe_alarm_clipboard::parse(&self.s.item_text_input.read(cx).value()) {
            Ok(item) => {
                self.s.import_grouping_exact = item.grouping_is_authoritative;
                self.s.import_groups = item
                    .groups
                    .into_iter()
                    .map(|group| (false, group.lines.join("\n")))
                    .collect();
                self.notice = None;
            }
            Err(_) => {
                self.s.import_groups.clear();
                self.notice = Some((
                    StatusKind::Warning,
                    self.word(
                        "请先在右侧粘贴完整物品文本，再点“读取词缀”",
                        "Paste a complete item on the right, then choose Read modifiers",
                    )
                    .into(),
                ));
            }
        }
        cx.notify();
    }

    fn selected_library_group(&self) -> Option<AcceptableResultGroup> {
        let g = match self.selected_node() {
            NodeRef::Group(g) | NodeRef::Condition(g, _) => g,
            _ => return None,
        };
        self.backend
            .as_ref()?
            .settings
            .selected_rules()
            .structured_rule_set
            .as_ref()?
            .groups
            .get(g)
            .cloned()
    }

    fn save_to_library(&mut self, plan: bool, cx: &mut Context<Self>) {
        if self.condition_selection_locked() || !self.validate_editor(cx) {
            return;
        }
        if !self.flush_editor(cx) {
            return;
        }
        let Some(mut group) = self.selected_library_group() else {
            return;
        };
        if !plan {
            let NodeRef::Condition(_, index) = self.selected_node() else {
                return;
            };
            let Some(condition) = group.conditions.get(index).cloned() else {
                return;
            };
            group = AcceptableResultGroup {
                name: condition.name.clone(),
                conditions: vec![condition],
                ..Default::default()
            };
        }
        let custom_name = self.s.library_name.read(cx).value().trim().to_string();
        let name = if !custom_name.is_empty() {
            custom_name
        } else if group.name.trim().is_empty() {
            if plan {
                self.word("收藏方案", "Saved plan").to_owned()
            } else {
                group
                    .conditions
                    .first()
                    .map(|c| c.template.clone())
                    .unwrap_or_default()
            }
        } else {
            group.name.clone()
        };
        let category = self.s.library_category.read(cx).value().trim().to_string();
        if plan {
            group.name = name.clone();
        }
        let Some(backend) = &self.backend else {
            return;
        };
        if !library_growth_fits(
            &backend.settings.affix_library,
            std::slice::from_ref(&group),
        ) {
            self.notice = Some((
                StatusKind::Warning,
                self.word(
                    "收藏库最多保存 4096 条收藏、8192 条词缀；请先整理后再添加",
                    "The library holds up to 4096 entries and 8192 affixes; organize it before adding more",
                )
                .into(),
            ));
            cx.notify();
            return;
        }
        let entry = LibraryEntry {
            name,
            category,
            game: backend.settings.selected_game_profile,
            language: backend.settings.selected_profile().ocr_language.clone(),
            group,
            is_plan: plan,
        };
        if let Err(error) = validate_library_entries(std::slice::from_ref(&entry)) {
            self.notice = Some((
                StatusKind::Error,
                format!(
                    "{}: {error}",
                    self.word("词缀尚未填写完整", "Complete the rule before saving")
                )
                .into(),
            ));
            cx.notify();
            return;
        }
        if let Some(backend) = &mut self.backend {
            backend.settings.affix_library.push(entry);
        }
        if !self.persist() {
            cx.notify();
            return;
        }
        self.notice = Some((
            StatusKind::Idle,
            self.word(
                "已收藏；使用时会添加独立副本",
                "Saved; adding creates an independent copy",
            )
            .into(),
        ));
        cx.notify();
    }

    fn add_library_entry(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.condition_selection_locked() || !self.validate_editor(cx) {
            return;
        }
        if !self.flush_editor(cx) {
            return;
        }
        let Some(entry) = self
            .backend
            .as_ref()
            .and_then(|b| b.settings.affix_library.get(index))
            .cloned()
        else {
            return;
        };
        let Some(backend) = &self.backend else {
            return;
        };
        if entry.game != backend.settings.selected_game_profile
            || entry.language != backend.settings.selected_profile().ocr_language
        {
            self.notice = Some((
                StatusKind::Warning,
                self.word(
                    "请先切换到收藏标注的游戏和词缀语言",
                    "Switch to this entry's game and modifier language first",
                )
                .into(),
            ));
            cx.notify();
            return;
        }
        if entry.is_plan {
            if !self.can_grow_saved_rules(1, entry.group.conditions.len(), 0, cx) {
                return;
            }
            self.undo_rules = None;
            if let Some(backend) = &mut self.backend {
                let set = backend
                    .settings
                    .selected_rules_mut()
                    .structured_rule_set
                    .get_or_insert_with(Default::default);
                let mut group = entry.group;
                make_group_name_unique(&mut group, &set.groups);
                set.groups.push(group);
            }
            self.persist();
            self.s.editor_tab = EditorTab::Conditions;
            self.refresh_tree_select_last(window, cx);
        } else {
            self.insert_conditions(entry.group.conditions, window, cx);
        }
    }

    fn insert_conditions(
        &mut self,
        mut conditions: Vec<AffixCondition>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if conditions.is_empty() {
            return;
        }
        let group_index = match self.selected_node() {
            NodeRef::Group(g) | NodeRef::Condition(g, _) => g,
            _ => 0,
        };
        let groups = self
            .backend
            .as_ref()
            .and_then(|b| b.settings.selected_rules().structured_rule_set.as_ref())
            .map(|rules| rules.groups.as_slice())
            .unwrap_or(&[]);
        let extra_groups = usize::from(groups.is_empty());
        let replaced_conditions = groups
            .get(group_index.min(groups.len().saturating_sub(1)))
            .map_or(0, |group| usize::from(has_pristine_placeholder(group)));
        if !self.can_grow_saved_rules(extra_groups, conditions.len(), replaced_conditions, cx) {
            return;
        }
        self.undo_rules = None;
        let Some(backend) = &mut self.backend else {
            return;
        };
        let set = backend
            .settings
            .selected_rules_mut()
            .structured_rule_set
            .get_or_insert_with(Default::default);
        if set.groups.is_empty() {
            set.groups.push(AcceptableResultGroup::default());
        }
        let group_index = group_index.min(set.groups.len().saturating_sub(1));
        let group = &mut set.groups[group_index];
        // Only the untouched initial row is disposable. Named or disabled
        // drafts and additional blank rows belong to the user and survive.
        if has_pristine_placeholder(group) {
            group.conditions.clear();
        }
        for condition in &mut conditions {
            condition.enabled = true;
            make_condition_name_unique(condition, &group.conditions);
            group.conditions.push(condition.clone());
        }
        let target = NodeRef::Condition(group_index, group.conditions.len() - 1);
        self.persist();
        self.s.tree = Self::tree_from_settings(self.backend.as_ref());
        self.s.selected = self
            .s
            .tree
            .iter()
            .position(|node| node.node == target)
            .unwrap_or(0);
        self.s.editor_tab = EditorTab::Conditions;
        self.sync_editor_from_selection(window, cx);
        cx.notify();
    }

    fn add_import_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.condition_selection_locked() || !self.validate_editor(cx) {
            return;
        }
        if !self.flush_editor(cx) {
            return;
        }
        let conditions = self
            .s
            .import_groups
            .iter()
            .filter(|(selected, _)| *selected)
            .map(|(_, template)| AffixCondition {
                template: template.split_whitespace().collect::<Vec<_>>().join(" "),
                ..Default::default()
            })
            .collect();
        self.insert_conditions(conditions, window, cx);
    }

    pub(crate) fn build_check_details(&mut self, text: &str) {
        self.check_details.clear();
        self.show_check_details = false;
        let Some(definition) = self
            .backend
            .as_ref()
            .and_then(|b| b.settings.selected_rules().structured_rule_set.clone())
        else {
            return;
        };
        let Ok(item) = poe_alarm_clipboard::parse(text) else {
            return;
        };
        let span = if self.backend.as_ref().is_some_and(|b| {
            b.settings.selected_game_profile == poe_alarm_settings::GameProfile::Poe2
        }) {
            poe_alarm_core::matching::MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN
        } else {
            poe_alarm_core::matching::DEFAULT_MAXIMUM_PHYSICAL_LINE_SPAN
        };
        let Ok(rules) = CompiledRuleSet::compile_with_maximum_line_span(definition, span) else {
            return;
        };
        let (lines, identities) = item.render();
        let evaluation = rules.evaluate_with_identity(&lines, &[], &identities);
        for group in evaluation.groups {
            if group.conditions.is_empty() {
                continue;
            }
            self.check_details.push(format!(
                "{} {} · {}/{} · {}",
                self.word("方案", "Plan"),
                group.group_index + 1,
                group.matched_count,
                group.required_count,
                if group.is_match {
                    self.word("命中", "Matched")
                } else {
                    self.word("未命中", "Not matched")
                }
            ));
            for condition in group.conditions {
                let label = if condition.name.is_empty() {
                    condition.template
                } else {
                    condition.name
                };
                self.check_details.push(format!(
                    "{} {}",
                    if condition.is_matched { "✓" } else { "×" },
                    label
                ));
                if let Some(observation) = condition.observation {
                    self.check_details
                        .push(format!("  {}", observation.original_text));
                    let mut numeric_failed = false;
                    for slot in condition.numeric_slots {
                        if slot.constraint.mode == M::Ignore {
                            continue;
                        }
                        numeric_failed |= !slot.is_satisfied;
                        self.check_details.push(format!(
                            "  {} {}: {} / {} {}",
                            self.word("数值", "Value"),
                            slot.slot_index + 1,
                            slot.actual_value
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "?".into()),
                            constraint_description(&slot.constraint),
                            if slot.is_satisfied { "✓" } else { "×" }
                        ));
                    }
                    if !condition.is_matched && !numeric_failed {
                        self.check_details.push(
                            self.word(
                                "  同一条物理词缀不能重复满足多个条件",
                                "  One physical modifier cannot satisfy multiple conditions",
                            )
                            .to_owned(),
                        );
                    }
                } else {
                    self.check_details.push(
                        self.word(
                            "  未找到匹配的完整词缀文本",
                            "  No matching complete modifier text",
                        )
                        .to_owned(),
                    );
                }
            }
        }
    }

    pub(crate) fn render_import(&self, cx: &mut Context<Self>) -> Div {
        let mut list = div()
            .id("import-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .v_flex()
            .gap_2();
        for (index, (selected, template)) in self.s.import_groups.iter().enumerate() {
            list = list.child(
                div()
                    .h_flex()
                    .items_start()
                    .gap_2()
                    .p_2()
                    .border_1()
                    .border_color(c(HAIRLINE))
                    .child(
                        Checkbox::new(("import-mod", index))
                            .checked(*selected)
                            .disabled(self.condition_selection_locked())
                            .on_click(cx.listener(move |this, value, _, cx| {
                                if let Some(group) = this.s.import_groups.get_mut(index) {
                                    group.0 = *value;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .text_size(fs(FS_12))
                            .child(SharedString::from(template.clone())),
                    ),
            );
        }
        div().flex_1().min_h_0().v_flex().p_4().gap_3()
            .child(div().h_flex().flex_wrap().gap_2()
                .child(button("new-blank-affix", LedgerButton::Secondary, self.word("新建空白词缀", "New blank affix"), cx).on_click(cx.listener(|this,_,w,cx| {this.add_condition(w,cx);this.s.editor_tab=EditorTab::Conditions;})))
                .child(button("read-import-affixes", LedgerButton::Secondary, self.word("读取词缀", "Read modifiers"), cx).on_click(cx.listener(|this,_,_,cx| this.parse_import_item(cx))))
                .child(button("open-affix-library", LedgerButton::Secondary, self.word("从词缀库添加", "From library"), cx).on_click(cx.listener(|this,_,_,cx| this.open_library(cx)))))
            .child(warning_band(self.word("提示", "Note"), if self.s.import_grouping_exact {
                self.word("完整复合词缀会一起添加。添加后请在数值条件中设置要求，默认不限制数值。", "Complete hybrid groups stay together. Set numeric requirements after adding; values are unrestricted initially.")
            } else { self.word("普通复制文本可能未区分复合词缀。建议粘贴游戏高级物品描述；当前按每行导入，添加后请检查分组。", "Plain copied text may not identify hybrid groups. Prefer advanced item descriptions; review grouping before use.") }))
            .child(list)
            .child(button("add-selected-modifiers", LedgerButton::Primary, self.word("添加勾选词缀", "Add selected modifiers"), cx).disabled(self.condition_selection_locked() || !self.s.import_groups.iter().any(|(checked,_)| *checked)).on_click(cx.listener(|this,_,w,cx| this.add_import_selection(w,cx))))
    }

    pub(crate) fn render_library(&self, cx: &mut Context<Self>) -> Div {
        let query = self.s.library_search.read(cx).value().to_lowercase();
        let mut list = div()
            .id("personal-library-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .v_flex()
            .gap_2();
        let entries = self
            .backend
            .as_ref()
            .map(|b| b.settings.affix_library.as_slice())
            .unwrap_or(&[]);
        for (index, entry) in entries.iter().enumerate() {
            let searchable = format!(
                "{} {} {}",
                entry.name,
                entry.category,
                entry
                    .group
                    .conditions
                    .iter()
                    .map(|c| c.template.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .to_lowercase();
            if !query
                .split_whitespace()
                .all(|part| searchable.contains(part))
            {
                continue;
            }
            let game = match entry.game {
                poe_alarm_settings::GameProfile::Poe1 => "POE1",
                _ => "POE2",
            };
            let title = format!(
                "{} · {} · {} · {}",
                if entry.is_plan {
                    self.word("方案", "Plan")
                } else {
                    self.word("词缀", "Affix")
                },
                game,
                entry.language,
                if entry.category.is_empty() {
                    self.word("未分类", "Uncategorized")
                } else {
                    &entry.category
                }
            );
            list = list.child(
                div()
                    .v_flex()
                    .gap_1()
                    .p_2()
                    .border_1()
                    .border_color(c(HAIRLINE))
                    .child(
                        div()
                            .text_size(fs(FS_10))
                            .text_color(c(TEXT_META))
                            .child(SharedString::from(title)),
                    )
                    .child(
                        div()
                            .text_size(fs(FS_12))
                            .whitespace_normal()
                            .child(SharedString::from(entry.name.clone())),
                    )
                    .child(
                        div()
                            .h_flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                button(
                                    ("library-add", index),
                                    LedgerButton::Secondary,
                                    self.word("添加副本", "Add copy"),
                                    cx,
                                )
                                .disabled(self.condition_selection_locked())
                                .on_click(cx.listener(
                                    move |this, _, w, cx| this.add_library_entry(index, w, cx),
                                )),
                            )
                            .child(
                                button(
                                    ("library-category", index),
                                    LedgerButton::Quiet,
                                    self.word("编辑名称/分类", "Edit details"),
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if let Some(entry) = this
                                            .backend
                                            .as_ref()
                                            .and_then(|b| b.settings.affix_library.get(index))
                                            .cloned()
                                        {
                                            this.s.library_selected = Some(index);
                                            this.s.library_name.update(cx, |input, cx| {
                                                input.set_value(entry.name, window, cx)
                                            });
                                            this.s.library_category.update(cx, |input, cx| {
                                                input.set_value(entry.category, window, cx)
                                            });
                                        }
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                button(
                                    ("library-delete", index),
                                    LedgerButton::Quiet,
                                    self.word("删除收藏", "Remove"),
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if let Some(b) = &mut this.backend {
                                            if index < b.settings.affix_library.len() {
                                                this.undo_library = Some((
                                                    index,
                                                    b.settings.affix_library.remove(index),
                                                ));
                                            }
                                        }
                                        this.s.library_selected = None;
                                        this.persist();
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .p_4()
            .gap_2()
            .child(div().text_size(fs(FS_12)).child(self.word(
                "个人词缀库 · 收藏与当前监控规则分别保存",
                "Personal library · Independent from active monitoring rules",
            )))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(div().w(px(60.)).child(self.word("搜索", "Search")))
                    .child(Input::new(&self.s.library_search)),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(div().w(px(60.)).child(self.word("分类", "Category")))
                    .child(Input::new(&self.s.library_category)),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(div().w(px(60.)).child(self.word("名称", "Name")))
                    .child(Input::new(&self.s.library_name)),
            )
            .when(self.s.library_selected.is_some(), |this| {
                this.child(
                    button(
                        "update-library-entry",
                        LedgerButton::Primary,
                        self.word("保存选中收藏的名称与分类", "Save selected entry details"),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let name = this.s.library_name.read(cx).value().trim().to_string();
                        let category = this.s.library_category.read(cx).value().trim().to_string();
                        if name.len() > 1024 || category.len() > 256 {
                            this.notice = Some((
                                StatusKind::Error,
                                this.word(
                                    "名称或分类过长，请缩短后保存",
                                    "Name or category is too long; shorten it before saving",
                                )
                                .into(),
                            ));
                            cx.notify();
                            return;
                        }
                        if let Some(index) = this.s.library_selected
                            && let Some(entry) = this
                                .backend
                                .as_mut()
                                .and_then(|b| b.settings.affix_library.get_mut(index))
                        {
                            if !name.is_empty() {
                                entry.name = name;
                            }
                            entry.category = category;
                        }
                        if this.persist() {
                            this.s.library_selected = None;
                        }
                        cx.notify();
                    })),
                )
            })
            .child(
                div()
                    .h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        button(
                            "save-affix",
                            LedgerButton::Secondary,
                            self.word("收藏当前词缀", "Save affix"),
                            cx,
                        )
                        .disabled(
                            !matches!(self.selected_node(), NodeRef::Condition(..))
                                || self.condition_selection_locked(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.save_to_library(false, cx))),
                    )
                    .child(
                        button(
                            "save-plan",
                            LedgerButton::Secondary,
                            self.word("收藏当前方案", "Save plan"),
                            cx,
                        )
                        .disabled(
                            matches!(self.selected_node(), NodeRef::Game)
                                || self.condition_selection_locked(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.save_to_library(true, cx))),
                    )
                    .child(
                        button(
                            "import-library",
                            LedgerButton::Quiet,
                            self.word("导入", "Import"),
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.import_library_file(cx))),
                    )
                    .child(
                        button(
                            "export-library",
                            LedgerButton::Quiet,
                            self.word("导出", "Export"),
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.export_library_file(cx))),
                    ),
            )
            .when(self.undo_library.is_some(), |this| {
                this.child(
                    button(
                        "undo-library-delete",
                        LedgerButton::Quiet,
                        self.word("撤销删除收藏", "Undo removal"),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some((_, entry)) = &this.undo_library
                            && let Some(backend) = &this.backend
                            && !library_growth_fits(
                                &backend.settings.affix_library,
                                std::iter::once(&entry.group),
                            )
                        {
                            this.notice = Some((
                                StatusKind::Warning,
                                this.word(
                                    "收藏库容量不足，请先整理后再撤销删除",
                                    "The library is full; free space before restoring this entry",
                                )
                                .into(),
                            ));
                            cx.notify();
                            return;
                        }
                        if let Some((index, entry)) = this.undo_library.take()
                            && let Some(b) = &mut this.backend
                        {
                            b.settings
                                .affix_library
                                .insert(index.min(b.settings.affix_library.len()), entry);
                        }
                        this.persist();
                        cx.notify();
                    })),
                )
            })
            .child(list)
    }

    fn import_library_file(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.first()
            {
                let path = path.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { read_library_file(&path) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(entries) => {
                            let result = this.backend.as_mut().map(|b| {
                                merge_library_entries(&mut b.settings.affix_library, entries)
                            });
                            match result {
                                Some(Ok(added)) => {
                                    if this.persist()
                                        && !matches!(this.notice, Some((StatusKind::Error, _)))
                                    {
                                        this.notice = Some((
                                            StatusKind::Idle,
                                            format!(
                                                "{} {added}",
                                                this.word("已导入收藏", "Imported entries:")
                                            )
                                            .into(),
                                        ));
                                    }
                                }
                                Some(Err(error)) => {
                                    this.notice = Some((
                                        StatusKind::Error,
                                        format!(
                                            "{}: {error}",
                                            this.word("导入失败", "Import failed")
                                        )
                                        .into(),
                                    ))
                                }
                                None => {}
                            }
                        }
                        Err(error) => {
                            this.notice = Some((
                                StatusKind::Error,
                                format!("{}: {error}", this.word("导入失败", "Import failed"))
                                    .into(),
                            ))
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn export_library_file(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = &self.backend else {
            return;
        };
        let bytes = match serde_json::to_vec_pretty(&backend.settings.affix_library) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.notice = Some((
                    StatusKind::Error,
                    format!("{}: {error}", self.word("导出失败", "Export failed")).into(),
                ));
                cx.notify();
                return;
            }
        };
        let base = std::path::PathBuf::from(backend.settings_path())
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let receiver = cx.prompt_for_new_path(&base, Some("poe-affix-library.json"));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = receiver.await {
                let destination = path.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { std::fs::write(destination, bytes) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(()) if !matches!(this.notice, Some((StatusKind::Error, _))) => {
                            this.notice = Some((
                                StatusKind::Idle,
                                format!("{}: {}", this.word("已导出", "Exported"), path.display())
                                    .into(),
                            ))
                        }
                        Ok(()) => {}
                        Err(error) => {
                            this.notice = Some((StatusKind::Error, error.to_string().into()))
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

fn constraint_description(constraint: &poe_alarm_core::NumericConstraint) -> String {
    let show =
        |v: Option<poe_alarm_core::Decimal>| v.map(|v| v.to_string()).unwrap_or_else(|| "?".into());
    match constraint.mode {
        M::Ignore => "—".into(),
        M::AtLeast => format!("≥{}", show(constraint.minimum)),
        M::AtMost => format!("≤{}", show(constraint.maximum)),
        M::Exactly => format!("={}", show(constraint.expected)),
        M::RangeInclusive => format!("{}…{}", show(constraint.minimum), show(constraint.maximum)),
    }
}

fn has_pristine_placeholder(group: &AcceptableResultGroup) -> bool {
    group.conditions.len() == 1 && group.conditions[0] == AffixCondition::default()
}

fn saved_rule_growth_fits(
    groups: usize,
    conditions: usize,
    extra_groups: usize,
    extra_conditions: usize,
    replaced_conditions: usize,
) -> bool {
    groups.saturating_add(extra_groups) <= poe_alarm_core::rules::MAXIMUM_SAVED_GROUPS
        && conditions
            .saturating_sub(replaced_conditions)
            .saturating_add(extra_conditions)
            <= poe_alarm_core::rules::MAXIMUM_SAVED_CONDITIONS
}

fn library_growth_fits<'a>(
    existing: &[LibraryEntry],
    additional: impl IntoIterator<Item = &'a AcceptableResultGroup>,
) -> bool {
    let (extra_entries, extra_conditions) =
        additional
            .into_iter()
            .fold((0usize, 0usize), |(entries, conditions), group| {
                (
                    entries.saturating_add(1),
                    conditions.saturating_add(group.conditions.len()),
                )
            });
    existing.len().saturating_add(extra_entries) <= 4096
        && existing
            .iter()
            .map(|entry| entry.group.conditions.len())
            .sum::<usize>()
            .saturating_add(extra_conditions)
            <= 8192
}

fn merge_library_entries(
    existing: &mut Vec<LibraryEntry>,
    incoming: Vec<LibraryEntry>,
) -> Result<usize, &'static str> {
    let mut additional = Vec::new();
    for entry in incoming {
        if !existing.contains(&entry) && !additional.contains(&entry) {
            additional.push(entry);
        }
    }
    if !library_growth_fits(existing, additional.iter().map(|entry| &entry.group)) {
        return Err("Library limit: 4096 entries / 8192 affixes. Remove some before importing.");
    }
    let added = additional.len();
    existing.extend(additional);
    Ok(added)
}

fn read_library_file(path: &std::path::Path) -> Result<Vec<LibraryEntry>, String> {
    use std::io::Read;
    const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Library file exceeds 10 MiB".into());
    }
    let entries: Vec<LibraryEntry> =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    validate_library_entries(&entries)?;
    Ok(entries)
}

fn validate_library_entries(entries: &[LibraryEntry]) -> Result<(), String> {
    if entries.len() > 4096 {
        return Err("Library contains more than 4096 entries".into());
    }
    let mut total = 0usize;
    for entry in entries {
        total = total.saturating_add(entry.group.conditions.len());
        if total > 8192
            || entry.group.conditions.len() > poe_alarm_core::rules::MAXIMUM_SAVED_CONDITIONS
        {
            return Err("Library contains too many conditions".into());
        }
        if !matches!(entry.language.as_str(), "en" | "zh-TW")
            || entry.group.conditions.is_empty()
            || (!entry.is_plan && entry.group.conditions.len() != 1)
        {
            return Err(format!(
                "Invalid language or modifier count: {}",
                entry.name
            ));
        }
        if entry.name.len() > 1024 || entry.category.len() > 256 || entry.group.name.len() > 1024 {
            return Err("Library names are too long".into());
        }
        let span = match entry.game {
            poe_alarm_settings::GameProfile::Poe1 => {
                poe_alarm_core::matching::DEFAULT_MAXIMUM_PHYSICAL_LINE_SPAN
            }
            poe_alarm_settings::GameProfile::Poe2 => {
                poe_alarm_core::matching::MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN
            }
        };
        for condition in &entry.group.conditions {
            if condition.template.len() > 8192 {
                return Err("Modifier template is too long".into());
            }
            let mut condition = condition.clone();
            condition.enabled = true;
            let definition = poe_alarm_core::RuleSetDefinition {
                groups: vec![AcceptableResultGroup {
                    conditions: vec![condition],
                    ..Default::default()
                }],
                ..Default::default()
            };
            CompiledRuleSet::compile_with_maximum_line_span(definition, span)
                .map_err(|error| format!("{}: {error}", entry.name))?;
        }
        if entry.group.mode == poe_alarm_core::ResultGroupMode::AtLeast
            && (entry.group.required_count == 0
                || entry.group.required_count > entry.group.conditions.len())
        {
            return Err(format!("Invalid required condition count: {}", entry.name));
        }
    }
    Ok(())
}

fn make_condition_name_unique(condition: &mut AffixCondition, others: &[AffixCondition]) {
    if condition.name.is_empty() {
        return;
    }
    let original = condition.name.clone();
    let mut suffix = 2;
    while others
        .iter()
        .any(|other| other.name.eq_ignore_ascii_case(&condition.name))
    {
        condition.name = format!("{original} ({suffix})");
        suffix += 1;
    }
}
fn make_group_name_unique(group: &mut AcceptableResultGroup, others: &[AcceptableResultGroup]) {
    if group.name.is_empty() {
        return;
    }
    let original = group.name.clone();
    let mut suffix = 2;
    while others
        .iter()
        .any(|other| other.name.eq_ignore_ascii_case(&group.name))
    {
        group.name = format!("{original} ({suffix})");
        suffix += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry() -> LibraryEntry {
        LibraryEntry {
            name: "Life".into(),
            category: "General".into(),
            game: poe_alarm_settings::GameProfile::Poe2,
            language: "en".into(),
            group: AcceptableResultGroup {
                conditions: vec![AffixCondition::new(
                    "Life",
                    "+# to maximum Life",
                    vec![poe_alarm_core::NumericConstraint::at_least(70)],
                )],
                ..Default::default()
            },
            is_plan: false,
        }
    }
    #[test]
    fn library_copy_is_independent_and_preserves_numeric_requirements() {
        let saved = entry();
        let mut copy = saved.group.clone();
        copy.conditions[0].numeric_constraints[0] = poe_alarm_core::NumericConstraint::at_least(99);
        make_condition_name_unique(&mut copy.conditions[0], &saved.group.conditions);
        assert_eq!(
            saved.group.conditions[0].numeric_constraints[0].minimum,
            Some(70.into())
        );
        assert_eq!(copy.conditions[0].name, "Life (2)");
        assert_eq!(
            copy.conditions[0].numeric_constraints[0].minimum,
            Some(99.into())
        );
    }
    #[test]
    fn import_rejects_invalid_rules_and_languages_before_mutating_library() {
        let mut candidate = entry();
        assert!(validate_library_entries(&[candidate.clone()]).is_ok());
        candidate.language = "fr".into();
        assert!(validate_library_entries(&[candidate.clone()]).is_err());
        candidate.language = "en".into();
        candidate.group.conditions[0].numeric_constraints[0].minimum = None;
        assert!(validate_library_entries(&[candidate.clone()]).is_err());
        candidate = entry();
        candidate.group.conditions[0].template.clear();
        assert!(validate_library_entries(&[candidate]).is_err());
    }
    #[test]
    fn grouped_item_import_keeps_hybrid_lines_together() {
        let item=poe_alarm_clipboard::parse("Item Class: Bows\nRarity: Rare\nTest Bow\n--------\nItem Level: 83\n--------\n{ Prefix Modifier \"Test\" (Tier: 1) }\n79% increased Physical Damage\n+186 to Accuracy Rating\n--------").unwrap();
        assert!(item.grouping_is_authoritative);
        assert_eq!(item.groups.len(), 1);
        assert_eq!(item.groups[0].lines.len(), 2);
    }

    #[test]
    fn only_one_untouched_default_row_can_be_replaced_during_import() {
        let mut group = AcceptableResultGroup {
            conditions: vec![AffixCondition::default()],
            ..Default::default()
        };
        assert!(has_pristine_placeholder(&group));
        group.conditions[0].name = "A named draft".into();
        assert!(!has_pristine_placeholder(&group));
        group.conditions[0] = AffixCondition {
            enabled: false,
            ..Default::default()
        };
        assert!(!has_pristine_placeholder(&group));
        group.conditions[0] = AffixCondition {
            numeric_constraints: vec![poe_alarm_core::NumericConstraint::at_least(70)],
            ..Default::default()
        };
        assert!(!has_pristine_placeholder(&group));
        group.conditions = vec![AffixCondition::default(); 2];
        assert!(!has_pristine_placeholder(&group));
    }

    #[test]
    fn stored_rule_limits_include_all_plans_and_preserve_placeholder_replacement_space() {
        assert!(saved_rule_growth_fits(127, 1023, 1, 1, 0));
        assert!(!saved_rule_growth_fits(128, 1023, 1, 1, 0));
        assert!(!saved_rule_growth_fits(127, 1024, 0, 1, 0));
        assert!(saved_rule_growth_fits(128, 1024, 0, 1, 1));
        assert!(!saved_rule_growth_fits(128, 1024, 0, 2, 1));
    }

    #[test]
    fn import_deduplicates_before_capacity_check_and_rejects_atomically() {
        let mut existing: Vec<_> = (0..4096)
            .map(|index| {
                let mut item = entry();
                item.name = format!("Entry {index}");
                item
            })
            .collect();
        let duplicate = existing[0].clone();
        assert_eq!(merge_library_entries(&mut existing, vec![duplicate]), Ok(0));
        let previous = existing.clone();
        assert!(merge_library_entries(&mut existing, vec![entry()]).is_err());
        assert_eq!(existing, previous);
        let mut small = vec![entry()];
        let mut new_entry = entry();
        new_entry.name = "Other".into();
        assert_eq!(
            merge_library_entries(&mut small, vec![new_entry.clone(), new_entry]),
            Ok(1)
        );
        assert_eq!(small.len(), 2);
    }

    #[test]
    fn library_total_affix_budget_prevents_unimportable_exports() {
        let mut full_plan = entry();
        full_plan.is_plan = true;
        full_plan.group.conditions = vec![full_plan.group.conditions[0].clone(); 1024];
        let mut existing = vec![full_plan; 8];
        assert!(!library_growth_fits(
            &existing,
            std::iter::once(&entry().group)
        ));
        let snapshot = existing.clone();
        assert!(merge_library_entries(&mut existing, vec![entry()]).is_err());
        assert_eq!(existing, snapshot);
    }
}
