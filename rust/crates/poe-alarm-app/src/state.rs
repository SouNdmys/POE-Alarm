//! 三档窗口共用的视图状态(Ledger 硬约束 3:三档共用一份视图状态,
//! 切档不重建控件、不丢输入)。
//!
//! Phase 3 阶段这里是纯前端 mock;Phase 4 接 poe-alarm-runtime/settings 时,
//! 字段会替换为真实运行时快照,结构保持不变。

use gpui::{Entity, SharedString};
use gpui_component::input::InputState;

use crate::ui::StatusKind;

/// 三档窗口(章节 09/10):同一坐标系的三档,Ctrl⇧1/2/3 直达。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    /// 1180×620 规则台
    Workbench,
    /// 720×560 单目标仪表
    Instrument,
    /// 400×620 游戏内长期停靠
    Dock,
}

impl Tier {
    pub fn size(self) -> (f32, f32) {
        match self {
            Tier::Workbench => (1180., 620.),
            Tier::Instrument => (720., 560.),
            Tier::Dock => (400., 620.),
        }
    }
}

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

    pub fn status_text(self) -> &'static str {
        match self {
            RunPhase::Idle => "可以开始",
            RunPhase::Monitoring => "正在监控词缀",
            RunPhase::Hit => "已命中,请确认",
        }
    }

    pub fn primary_label(self) -> &'static str {
        match self {
            RunPhase::Idle => "开始监控",
            RunPhase::Monitoring => "停止监控",
            RunPhase::Hit => "解除鼠标锁定",
        }
    }
}

/// 中央编辑区 tab(词缀条件 / 识别区域 / 提醒与显示)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorTab {
    Conditions,
    Region,
    Alerts,
}

/// 命中方式(Instrument 顶部分段)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetMode {
    Single,
    Multi,
}

/// 规则树节点(mock;Phase 4 由 settings 载入)。
pub struct RuleNode {
    pub depth: usize,
    pub label: SharedString,
    pub trailing: SharedString,
    pub expandable: bool,
    pub expanded: bool,
    pub warning: bool,
    pub disabled: bool,
}

/// 数值条件行:下限/上限输入是独立 InputState,跨档保留。
pub struct ValueRow {
    pub label: SharedString,
    pub comparison: SharedString,
    pub min: Entity<InputState>,
    pub max: Entity<InputState>,
}

/// 三档共享的全部视图状态。
pub struct ViewState {
    pub tier: Tier,
    pub run: RunPhase,
    pub editor_tab: EditorTab,
    pub target_mode: TargetMode,

    /// 左树数据与选中项
    pub tree: Vec<RuleNode>,
    pub selected: usize,

    /// 编辑区输入(实体跨档共享,切档不丢输入)
    pub name_input: Entity<InputState>,
    pub template_input: Entity<InputState>,
    /// Dock 命令行(粘贴词缀)
    pub command_input: Entity<InputState>,
    pub value_rows: Vec<ValueRow>,

    /// 运行侧展示数据(mock)
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
