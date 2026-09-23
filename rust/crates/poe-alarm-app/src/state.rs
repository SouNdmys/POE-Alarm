//! 规则台视图状态(Ledger 硬约束:控件实体常驻,刷新不重建、不丢输入)。
//!
//! Phase 5 起只保留 1180×620 规则台一档;轻量查看由可拖动 HUD 浮窗承担。

use gpui::{Entity, SharedString};
use gpui_component::input::InputState;

use crate::ui::StatusKind;

/// 规则台窗口尺寸。
pub const WORKBENCH_SIZE: (f32, f32) = (1180., 620.);

/// 程序只有两种运行态:监控中、命中后停止;idle 为默认安静态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunPhase {
    Idle,
    Monitoring,
    Hit,
}

impl RunPhase {
    pub fn status_kind(self) -> StatusKind {
        match self {
            RunPhase::Idle => StatusKind::Idle,
            RunPhase::Monitoring => StatusKind::Monitoring,
            RunPhase::Hit => StatusKind::Hit,
        }
    }
}

/// 中央编辑区 tab(词缀条件 / 设置 / 使用说明)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorTab {
    Conditions,
    Settings,
    Help,
    Library,
    Import,
}

/// 树节点指向的设置对象。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeRef {
    Game,
    Group(usize),
    Condition(usize, usize),
}

/// 规则树节点(由 settings 结构化规则展开)。
pub struct RuleNode {
    pub node: NodeRef,
    pub depth: usize,
    pub label: SharedString,
    pub trailing: SharedString,
    pub expandable: bool,
    pub expanded: bool,
    pub warning: bool,
    pub disabled: bool,
}

/// 数值条件行:与模板的数值占位一一对应,默认"不限制"。
/// 下限/上限输入是独立 InputState,刷新时按槽位保留。
pub struct ValueRow {
    pub mode: poe_alarm_core::NumericConstraintMode,
    pub min: Entity<InputState>,
    pub max: Entity<InputState>,
}

/// 规则台的全部视图状态。
pub struct ViewState {
    pub run: RunPhase,
    pub editor_tab: EditorTab,

    /// 左树数据与选中项
    pub tree: Vec<RuleNode>,
    pub selected: usize,

    /// 编辑区输入(实体常驻,刷新不丢输入)
    pub name_input: Entity<InputState>,
    /// The condition name the app last derived from the template, while the
    /// name is still that derived value. `None` once the user has typed one.
    pub auto_name: Option<String>,
    pub template_input: Entity<InputState>,
    /// 粘贴物品文本的多行输入框。
    pub item_text_input: Entity<InputState>,
    pub value_rows: Vec<ValueRow>,
    pub library_search: Entity<InputState>,
    pub library_category: Entity<InputState>,
    pub library_name: Entity<InputState>,
    pub library_selected: Option<usize>,
    pub import_groups: Vec<(bool, String)>,
    pub import_grouping_exact: bool,

    /// 运行侧展示数据
    pub elapsed: SharedString,
    pub hit_count: u32,
}

impl ViewState {
    pub fn selected_label(&self) -> SharedString {
        self.tree
            .get(self.selected)
            .map(|n| n.label.clone())
            .unwrap_or_else(|| "—".into())
    }
}
