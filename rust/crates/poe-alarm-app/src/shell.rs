//! AppShell:规则台窗口的宿主实体(Phase 5:单一 1180×620 规则台)。
//!
//! 持有唯一的 ViewState + Backend,处理监控启停、运行时事件轮询与树/编辑器交互。
//! 规则统一走结构化规则集(单条目标 = 一方案一词缀);数值条件行与模板的
//! 数值占位一一对应,默认"不限制"。

use std::time::{Duration, Instant};

use gpui::{Context, Div, FocusHandle, SharedString, Window, div, prelude::*, px};
use gpui_component::{StyledExt, input::InputState};
use poe_alarm_core::{NumericConstraint, NumericConstraintMode, ResultGroupMode};
use poe_alarm_settings::GameProfile;

use crate::backend::{Backend, BridgeEvent, BridgeState, PlatformEvent};
use crate::i18n;
use crate::state::*;
use crate::theme::*;
use crate::ui::*;

/// 日志条目(持久存储;渲染时映射为 ui::LogLine)。
#[derive(Clone)]
pub enum LogKind {
    Meta,
    Text,
    Match,
    Hit,
}

const LOG_CAP: usize = 60;

pub struct AppShell {
    pub s: ViewState,
    pub backend: Option<Backend>,
    pub focus_handle: FocusHandle,
    pub log: Vec<(LogKind, SharedString)>,
    /// 就地通知(状态栏右侧/编辑区顶部),不弹窗。
    pub notice: Option<(StatusKind, SharedString)>,
    monitor_since: Option<Instant>,
    /// BitBlt 类一次性故障后的自动重启预算(成功进入监控后复位)。
    auto_restart_budget: u8,
    /// 延迟启动时刻:热键去抖(按键/IME 弹窗散场)与故障重建后的冷却。
    pending_start_at: Option<Instant>,
    /// 提示音/录屏可见性在监控中被改过:回到待机后重建运行时以生效。
    pending_runtime_reset: bool,
    /// HUD 交互态缓存(仅未监控时可拖动;避免每 tick 重复发命令)。
    hud_interactive: Option<bool>,
    pub scan_count: u64,
    pub capture_ms: f64,
    pub ocr_ms: f64,
    pub ocr_cached: bool,
}

impl AppShell {
    /// 当前界面语言的文案目录。
    pub fn t(&self) -> &'static i18n::Text {
        let english = self
            .backend
            .as_ref()
            .map(|b| b.settings.ui_language.starts_with("en"))
            .unwrap_or(false);
        if english { &i18n::EN } else { &i18n::ZH }
    }

    /// 切换界面语言(即时生效,树标签整体刷新,选中与输入不动)。
    pub fn set_ui_language(&mut self, language: &str, cx: &mut Context<Self>) {
        if let Some(backend) = &mut self.backend {
            backend.settings.ui_language =
                poe_alarm_settings::normalize_ui_language(language).to_owned();
        }
        self.persist();
        let node = self.selected_node();
        self.s.tree = Self::tree_from_settings(self.backend.as_ref());
        if let Some(ix) = self.s.tree.iter().position(|n| n.node == node) {
            self.s.selected = ix;
        }
        cx.notify();
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut backend = match Backend::new() {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("backend init failed: {e}");
                None
            }
        };
        if let Some(backend) = &mut backend {
            Self::ensure_structured_rules(backend);
        }

        let text = Self::text_for(backend.as_ref());
        let name_input = cx.new(|cx| InputState::new(window, cx));
        let template_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(text.template_placeholder));
        let item_text_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(10)
                .placeholder(text.item_text_placeholder)
        });

        let tree = Self::tree_from_settings(backend.as_ref());
        let selected = tree
            .iter()
            .position(|n| matches!(n.node, NodeRef::Condition(..)))
            .unwrap_or(0);
        let notice = match &backend {
            Some(b) if b.read_only => Some((StatusKind::Warning, text.notice_read_only.into())),
            None => Some((StatusKind::Error, text.notice_backend_failed.into())),
            _ => None,
        };

        // 运行时事件轮询(120ms;监控中同时驱动计时器刷新)。
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                if this
                    .update(cx, |this: &mut AppShell, cx| this.tick(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let mut shell = Self {
            s: ViewState {
                run: RunPhase::Idle,
                editor_tab: EditorTab::Conditions,
                tree,
                selected,
                name_input,
                template_input,
                item_text_input,
                value_rows: Vec::new(),
                elapsed: "--:--".into(),
                hit_count: 0,
            },
            backend,
            focus_handle: cx.focus_handle(),
            log: vec![(LogKind::Meta, text.log_ready.into())],
            notice,
            monitor_since: None,
            auto_restart_budget: 2,
            pending_start_at: None,
            pending_runtime_reset: false,
            hud_interactive: None,
            scan_count: 0,
            capture_ms: 0.0,
            ocr_ms: 0.0,
            ocr_cached: false,
        };
        shell.sync_editor_from_selection(window, cx);
        shell
    }

    /// 保证当前配置有结构化规则集(单条目标迁移为一方案一词缀)。
    fn ensure_structured_rules(backend: &mut Backend) {
        let rules = backend.settings.selected_rules_mut();
        let empty = rules
            .structured_rule_set
            .as_ref()
            .map(|set| set.groups.is_empty())
            .unwrap_or(true);
        if empty {
            let template = rules.target_affix.trim().to_owned();
            let set = rules
                .structured_rule_set
                .get_or_insert_with(Default::default);
            set.groups.push(poe_alarm_core::AcceptableResultGroup {
                name: "可接受结果 1".to_owned(),
                mode: ResultGroupMode::Any,
                required_count: 1,
                conditions: vec![poe_alarm_core::AffixCondition {
                    name: String::new(),
                    template,
                    numeric_constraints: Vec::new(),
                }],
            });
        }
    }

    /// 当前后端对应的文案目录(构造期 self 尚不存在时用)。
    fn text_for(backend: Option<&Backend>) -> &'static i18n::Text {
        let english = backend
            .map(|b| b.settings.ui_language.starts_with("en"))
            .unwrap_or(false);
        if english { &i18n::EN } else { &i18n::ZH }
    }

    /// 从设置里的结构化规则集合成树显示。
    fn tree_from_settings(backend: Option<&Backend>) -> Vec<RuleNode> {
        let text = Self::text_for(backend);
        let mut tree = Vec::new();
        let Some(backend) = backend else {
            return tree;
        };
        let settings = &backend.settings;
        let game_label = match settings.selected_game_profile {
            GameProfile::Poe1 => text.game_poe1,
            GameProfile::Poe2 => text.game_poe2,
        };
        tree.push(RuleNode {
            node: NodeRef::Game,
            depth: 0,
            label: game_label.into(),
            trailing: text.tree_active.into(),
            expandable: true,
            expanded: true,
            warning: false,
            disabled: false,
        });
        let rules = settings.selected_rules();
        if let Some(set) = &rules.structured_rule_set {
            for (g_ix, group) in set.groups.iter().enumerate() {
                let mode = match group.mode {
                    ResultGroupMode::Any => text.tree_any.to_owned(),
                    ResultGroupMode::All => text.tree_all.to_owned(),
                    ResultGroupMode::AtLeast => {
                        format!("≥{}/{}", group.required_count, group.conditions.len())
                    }
                };
                let group_name = if group.name.trim().is_empty() {
                    format!("{} {}", text.default_result_name, g_ix + 1)
                } else {
                    group.name.clone()
                };
                let group_empty = group.conditions.is_empty();
                tree.push(RuleNode {
                    node: NodeRef::Group(g_ix),
                    depth: 1,
                    label: group_name.into(),
                    trailing: if group_empty {
                        text.tree_empty_group.into()
                    } else {
                        mode.into()
                    },
                    expandable: true,
                    expanded: true,
                    warning: group_empty,
                    disabled: false,
                });
                for (c_ix, cond) in group.conditions.iter().enumerate() {
                    let missing = cond.template.trim().is_empty();
                    let label = if !cond.name.trim().is_empty() {
                        cond.name.clone()
                    } else if missing {
                        text.tree_new_condition.to_owned()
                    } else {
                        cond.template.clone()
                    };
                    tree.push(RuleNode {
                        node: NodeRef::Condition(g_ix, c_ix),
                        depth: 2,
                        label: label.into(),
                        trailing: if missing { text.tree_pending } else { "" }.into(),
                        expandable: false,
                        expanded: false,
                        warning: missing,
                        disabled: false,
                    });
                }
            }
        }
        tree
    }

    // -- runtime event loop -------------------------------------------------

    fn tick(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut reset_runtime = false;
        let mut auto_restart = false;
        let platform_events = match &mut self.backend {
            Some(backend) => backend.poll_platform(),
            None => Vec::new(),
        };
        for event in platform_events {
            changed = true;
            match event {
                PlatformEvent::HotKeyStart => {
                    if self.s.run == RunPhase::Idle && self.pending_start_at.is_none() {
                        // 去抖 350ms:等热键按键抬起、IME 切换弹窗散场后再首帧截屏。
                        self.pending_start_at = Some(Instant::now() + Duration::from_millis(350));
                        self.push_log(LogKind::Meta, self.t().log_hotkey_start.to_owned());
                    }
                }
                PlatformEvent::ItemUnderCursor(item_text) => {
                    let origin = self.t().log_check_under_cursor;
                    self.run_item_check(item_text, origin, cx);
                }
                PlatformEvent::NoItemUnderCursor => {
                    self.notice = Some((
                        StatusKind::Warning,
                        self.t().notice_no_item_under_cursor.into(),
                    ));
                }
                PlatformEvent::HotKeysUnavailable => {
                    let detail = self.t().notice_hotkeys_unavailable;
                    self.notice = Some((StatusKind::Error, detail.into()));
                    self.push_log(LogKind::Meta, detail.to_owned());
                }
                PlatformEvent::GameNotFocused => {
                    self.notice =
                        Some((StatusKind::Warning, self.t().notice_game_not_focused.into()));
                }
                PlatformEvent::CopyRefused { outranked } => {
                    let text = self.t();
                    let detail = if outranked {
                        text.notice_copy_refused_elevated
                    } else {
                        text.notice_copy_refused
                    };
                    self.notice = Some((StatusKind::Error, detail.into()));
                    self.push_log(LogKind::Meta, detail.to_owned());
                }
                PlatformEvent::HudMoved(rx, ry) => {
                    if let Some(backend) = &mut self.backend {
                        backend.settings.hud_placement = poe_alarm_settings::HudPlacement {
                            monitor_device_name: None,
                            relative_x: Some(rx),
                            relative_y: Some(ry),
                        };
                        if backend.save().is_ok() {
                            self.push_log(LogKind::Meta, self.t().log_hud_moved.to_owned());
                        }
                    }
                }
            }
        }
        let events = match &mut self.backend {
            Some(backend) => backend.poll(),
            None => Vec::new(),
        };
        for event in events {
            changed = true;
            match event {
                BridgeEvent::State(state) => self.apply_runtime_state(state),
                BridgeEvent::Snapshot {
                    scan_count,
                    capture_ms,
                    ocr_ms,
                    cached,
                    lines,
                    detail,
                } => {
                    self.scan_count = scan_count;
                    self.capture_ms = capture_ms;
                    self.ocr_ms = ocr_ms;
                    self.ocr_cached = cached;
                    if let Some(detail) = detail {
                        self.push_log(LogKind::Meta, detail);
                    }
                    for line in lines.into_iter().take(4) {
                        self.push_log(LogKind::Text, line);
                    }
                }
                BridgeEvent::MatchFound {
                    detail,
                    lines,
                    matched_group,
                } => {
                    self.s.hit_count += 1;
                    for line in lines.into_iter().take(4) {
                        self.push_log(LogKind::Match, line);
                    }
                    let group =
                        matched_group.unwrap_or_else(|| self.t().log_target_fallback.to_owned());
                    self.push_log(
                        LogKind::Hit,
                        format!("{} · {group} · {detail}", self.t().log_rule_hit_prefix),
                    );
                }
                BridgeEvent::ItemCheckReport {
                    lines,
                    matched,
                    detail,
                } => {
                    for line in lines.into_iter().take(12) {
                        self.push_log(LogKind::Text, line);
                    }
                    if matched {
                        self.push_log(
                            LogKind::Hit,
                            format!("{} · {detail}", self.t().log_check_hit_prefix),
                        );
                    } else {
                        self.push_log(LogKind::Meta, self.t().log_check_miss.to_owned());
                    }
                }
                BridgeEvent::AlertPresented => {
                    self.s.run = RunPhase::Hit;
                    self.push_log(LogKind::Hit, self.t().log_alert_presented.to_owned());
                }
                BridgeEvent::AlertAcknowledged => {
                    self.apply_runtime_state(BridgeState::Idle);
                    self.push_log(LogKind::Meta, self.t().log_acknowledged.to_owned());
                }
                BridgeEvent::Fault(detail) => {
                    self.notice = Some((StatusKind::Error, detail.clone().into()));
                    self.push_log(
                        LogKind::Meta,
                        format!("{}:{detail}", self.t().log_error_prefix),
                    );
                    self.apply_runtime_state(BridgeState::Idle);
                    reset_runtime = true;
                    if detail.contains("BitBlt") && self.auto_restart_budget > 0 {
                        self.auto_restart_budget -= 1;
                        auto_restart = true;
                    }
                }
                BridgeEvent::SoundFallback => {
                    self.notice =
                        Some((StatusKind::Warning, self.t().notice_sound_fallback.into()));
                }
            }
        }
        if reset_runtime && let Some(backend) = &mut self.backend {
            backend.reset_runtime();
            self.push_log(LogKind::Meta, self.t().log_runtime_rebuilt.to_owned());
        }
        if auto_restart {
            // 重建后的警报服务线程需要时间释放进程单例,延迟 700ms 再启动。
            self.pending_start_at = Some(Instant::now() + Duration::from_millis(700));
            self.push_log(LogKind::Meta, self.t().log_auto_restart.to_owned());
        }
        if let Some(at) = self.pending_start_at
            && Instant::now() >= at
        {
            self.pending_start_at = None;
            if self.s.run == RunPhase::Idle {
                self.toggle_run(cx);
            }
            changed = true;
        }
        // 编辑器 → 树标签实时同步:粘贴模板后左树与面包屑立即刷新,不等切换选中。
        if let NodeRef::Condition(..) = self.selected_node() {
            let text = self.t();
            let name = self.s.name_input.read(cx).value().trim().to_string();
            let template = self.s.template_input.read(cx).value().trim().to_string();
            let missing = template.is_empty();
            let label = if !name.is_empty() {
                name
            } else if missing {
                text.tree_new_condition.to_owned()
            } else {
                template
            };
            let selected = self.s.selected;
            if let Some(row) = self.s.tree.get_mut(selected)
                && row.label.as_ref() != label
            {
                row.label = label.into();
                row.trailing = if missing { text.tree_pending } else { "" }.into();
                row.warning = missing;
                changed = true;
            }
        }
        // 监控计时(状态点+文字+计时坐标恒定,只更新文本)
        if let Some(since) = self.monitor_since {
            let total = since.elapsed().as_secs();
            let text = format!("{:02}:{:02}", total / 60, total % 60);
            if self.s.elapsed.as_ref() != text {
                self.s.elapsed = text.into();
                changed = true;
            }
        }
        if changed {
            let target = self.s.template_input.read(cx).value().trim().to_string();
            if let Some(backend) = &self.backend {
                backend.hud_update(
                    self.s.run == RunPhase::Monitoring,
                    self.t().status_text(self.s.run),
                    self.s.elapsed.as_ref(),
                    &target,
                );
                // 命中弹窗期间 HUD 让位隐藏;其余时刻跟随"持续显示"设置。
                backend.hud_set_visible(
                    backend.settings.keep_hud_visible && self.s.run != RunPhase::Hit,
                );
            }
            cx.notify();
        }
        // 仅未监控时允许拖动浮窗;监控中保持点击穿透。
        // 不依赖 changed:启动后哪怕没有任何事件也要下发一次 Placement,
        // 否则浮窗保持点击穿透、无法拖动。
        if let Some(backend) = &self.backend {
            let interactive = self.s.run == RunPhase::Idle;
            if self.hud_interactive != Some(interactive) {
                backend.hud_set_interactive(interactive);
                self.hud_interactive = Some(interactive);
            }
        }
    }

    fn apply_runtime_state(&mut self, state: BridgeState) {
        let next = match state {
            BridgeState::Starting | BridgeState::Monitoring | BridgeState::CheckingItem => {
                RunPhase::Monitoring
            }
            BridgeState::MatchFound => RunPhase::Hit,
            BridgeState::Idle
            | BridgeState::Faulted
            | BridgeState::ShuttingDown
            | BridgeState::Stopped => RunPhase::Idle,
        };
        if next == RunPhase::Monitoring && self.monitor_since.is_none() {
            self.monitor_since = Some(Instant::now());
            self.auto_restart_budget = 2;
            // 成功进入监控后清掉遗留的错误提示,避免旧故障一直挂在栏上。
            if matches!(self.notice, Some((StatusKind::Error, _))) {
                self.notice = None;
            }
        }
        if next != RunPhase::Monitoring {
            self.monitor_since = None;
            if next == RunPhase::Idle {
                self.s.elapsed = "--:--".into();
                // 监控期间改过提示音/录屏可见性:此刻重建,下次启动生效。
                if self.pending_runtime_reset
                    && let Some(backend) = &mut self.backend
                {
                    self.pending_runtime_reset = false;
                    backend.reset_runtime();
                }
            }
        }
        self.s.run = next;
    }

    /// 提示音或红窗相关设置变化后调用:待机立即重建运行时,监控中挂起到停止。
    fn invalidate_runtime(&mut self) {
        if self.s.run == RunPhase::Idle {
            if let Some(backend) = &mut self.backend {
                backend.reset_runtime();
            }
        } else {
            self.pending_runtime_reset = true;
        }
    }

    fn push_log(&mut self, kind: LogKind, text: String) {
        self.log.push((kind, text.into()));
        if self.log.len() > LOG_CAP {
            let excess = self.log.len() - LOG_CAP;
            self.log.drain(..excess);
        }
    }

    // -- state transitions --------------------------------------------------

    /// 主操作:开始监控 / 停止监控 / 解除鼠标锁定。
    pub fn toggle_run(&mut self, cx: &mut Context<Self>) {
        // 先把编辑内容写回设置,保证 runtime 拿到的是屏幕上的内容。
        if self.s.run == RunPhase::Idle {
            self.apply_editor_to_selection(cx);
        }
        let text = self.t();
        let Some(backend) = &mut self.backend else {
            self.notice = Some((StatusKind::Error, text.notice_backend_missing.into()));
            cx.notify();
            return;
        };
        let starting = self.s.run == RunPhase::Idle;
        let result = match self.s.run {
            RunPhase::Idle => {
                backend.settings.selected_rules_mut().rule_editor_mode =
                    poe_alarm_settings::RuleEditorMode::Structured;
                // 启动前自动落盘;只读会话保存失败不阻塞启动。
                if let Err(e) = backend.save() {
                    eprintln!("settings save before start failed: {e}");
                }
                backend.start_monitoring()
            }
            RunPhase::Monitoring => backend.stop_monitoring(),
            RunPhase::Hit => backend.acknowledge_alert(),
        };
        if starting && result.is_ok() {
            self.push_log(LogKind::Meta, text.log_starting.to_owned());
        }
        if let Err(e) = result {
            self.notice = Some((StatusKind::Error, e.clone().into()));
            self.push_log(LogKind::Meta, format!("{}:{e}", text.log_error_prefix));
        }
        cx.notify();
    }

    // ---- 结构化多词缀编辑(树 ↔ 编辑器) ----------------------------------

    fn selected_node(&self) -> NodeRef {
        self.s
            .tree
            .get(self.s.selected)
            .map(|n| n.node)
            .unwrap_or(NodeRef::Game)
    }

    /// 当前选中所属组(供分段控件与"共 N 条"展示)。
    pub fn selected_group_summary(&self) -> Option<(ResultGroupMode, usize, usize)> {
        let g = match self.selected_node() {
            NodeRef::Group(g) | NodeRef::Condition(g, _) => g,
            NodeRef::Game => return None,
        };
        self.backend
            .as_ref()?
            .settings
            .selected_rules()
            .structured_rule_set
            .as_ref()?
            .groups
            .get(g)
            .map(|group| (group.mode, group.required_count, group.conditions.len()))
    }

    /// 模板文本的数值占位数(与归一化预览一致)。
    fn slot_count(template: &str) -> usize {
        let trimmed = template.trim();
        if trimmed.is_empty() {
            return 0;
        }
        poe_alarm_core::extract_values(trimmed).len()
    }

    /// 数值行与模板占位数对齐(渲染前调用;保留已有行的模式与输入)。
    pub fn ensure_value_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let want = Self::slot_count(&self.s.template_input.read(cx).value());
        let have = self.s.value_rows.len();
        if want == have {
            return;
        }
        if want < have {
            self.s.value_rows.truncate(want);
        } else {
            for _ in have..want {
                self.s.value_rows.push(ValueRow {
                    mode: NumericConstraintMode::Ignore,
                    min: cx.new(|cx| InputState::new(window, cx)),
                    max: cx.new(|cx| InputState::new(window, cx)),
                });
            }
        }
    }

    /// 把选中条件的数据载入编辑器(name/template/数值行)。
    pub fn sync_editor_from_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let NodeRef::Condition(g, c) = self.selected_node() else {
            return;
        };
        let Some(backend) = &self.backend else {
            return;
        };
        let Some(cond) = backend
            .settings
            .selected_rules()
            .structured_rule_set
            .as_ref()
            .and_then(|set| set.groups.get(g))
            .and_then(|group| group.conditions.get(c))
        else {
            return;
        };
        let name = cond.name.clone();
        let template = cond.template.clone();
        let slots = Self::slot_count(&template).max(cond.numeric_constraints.len());
        let rows: Vec<(NumericConstraintMode, String, String)> = (0..slots)
            .map(|ix| {
                let nc = cond
                    .numeric_constraints
                    .get(ix)
                    .cloned()
                    .unwrap_or_default();
                let min = match nc.mode {
                    NumericConstraintMode::Exactly => nc.expected,
                    _ => nc.minimum,
                }
                .map(|d| d.to_string())
                .unwrap_or_default();
                let max = nc.maximum.map(|d| d.to_string()).unwrap_or_default();
                (nc.mode, min, max)
            })
            .collect();
        self.s.name_input.update(cx, |input, cx| {
            input.set_value(name, window, cx);
        });
        self.s.template_input.update(cx, |input, cx| {
            input.set_value(template, window, cx);
        });
        self.s.value_rows = rows
            .into_iter()
            .map(|(mode, min, max)| ValueRow {
                mode,
                min: cx.new(|cx| InputState::new(window, cx).default_value(min)),
                max: cx.new(|cx| InputState::new(window, cx).default_value(max)),
            })
            .collect();
    }

    /// 静默落盘(自动保存,无手动按钮);失败才提示。
    fn persist(&mut self) {
        let text = self.t();
        if let Some(backend) = &mut self.backend
            && let Err(e) = backend.save()
        {
            self.notice = Some((
                StatusKind::Error,
                format!("{}:{e}", text.notice_save_failed_prefix).into(),
            ));
        }
    }

    /// 把编辑器内容写回选中的条件,返回是否有实际改动。约束按占位数落盘;
    /// "范围"缺一边时宽松降级(只有下限→至少,只有上限→至多,全空→不限制),
    /// 保证不会因为空输入卡住启动。
    pub fn apply_editor_to_selection(&mut self, cx: &mut Context<Self>) -> bool {
        use NumericConstraintMode as M;
        let NodeRef::Condition(g, c) = self.selected_node() else {
            return false;
        };
        let name = self.s.name_input.read(cx).value().trim().to_string();
        let template = self.s.template_input.read(cx).value().trim().to_string();
        let slots = Self::slot_count(&template);
        let parse = |entity: &gpui::Entity<InputState>| {
            entity
                .read(cx)
                .value()
                .trim()
                .parse::<poe_alarm_core::Decimal>()
                .ok()
        };
        let constraints: Vec<NumericConstraint> = (0..slots)
            .map(|ix| match self.s.value_rows.get(ix) {
                None => NumericConstraint::default(),
                Some(row) => {
                    let min = parse(&row.min);
                    let max = parse(&row.max);
                    match row.mode {
                        M::Ignore => NumericConstraint::default(),
                        M::AtLeast => NumericConstraint {
                            mode: if min.is_some() { M::AtLeast } else { M::Ignore },
                            minimum: min,
                            ..Default::default()
                        },
                        M::AtMost => NumericConstraint {
                            mode: if max.is_some() { M::AtMost } else { M::Ignore },
                            maximum: max,
                            ..Default::default()
                        },
                        M::Exactly => NumericConstraint {
                            mode: if min.is_some() { M::Exactly } else { M::Ignore },
                            expected: min,
                            ..Default::default()
                        },
                        M::RangeInclusive => match (min, max) {
                            (Some(a), Some(b)) => NumericConstraint {
                                mode: M::RangeInclusive,
                                minimum: Some(a.min(b)),
                                maximum: Some(a.max(b)),
                                ..Default::default()
                            },
                            (Some(a), None) => NumericConstraint {
                                mode: M::AtLeast,
                                minimum: Some(a),
                                ..Default::default()
                            },
                            (None, Some(b)) => NumericConstraint {
                                mode: M::AtMost,
                                maximum: Some(b),
                                ..Default::default()
                            },
                            (None, None) => NumericConstraint::default(),
                        },
                    }
                }
            })
            .collect();
        let Some(backend) = &mut self.backend else {
            return false;
        };
        let Some(cond) = backend
            .settings
            .selected_rules_mut()
            .structured_rule_set
            .as_mut()
            .and_then(|set| set.groups.get_mut(g))
            .and_then(|group| group.conditions.get_mut(c))
        else {
            return false;
        };
        let modified = cond.name != name
            || cond.template != template
            || cond.numeric_constraints != constraints;
        cond.name = name;
        cond.template = template;
        cond.numeric_constraints = constraints;
        modified
    }

    /// 树点选:先落盘当前编辑(有改动即自动保存),再切换选中并载入。
    pub fn select_tree_node(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.apply_editor_to_selection(cx) {
            self.persist();
        }
        self.s.selected = ix;
        self.refresh_tree_keep_selection(window, cx);
        self.sync_editor_from_selection(window, cx);
        cx.notify();
    }

    /// 结构化操作:+方案 / +词缀 / 删除词缀 / 删除方案 / 组模式与条数。
    pub fn add_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_editor_to_selection(cx);
        let Some(backend) = &mut self.backend else {
            return;
        };
        let rules = backend.settings.selected_rules_mut();
        let set = rules
            .structured_rule_set
            .get_or_insert_with(Default::default);
        let n = set.groups.len() + 1;
        set.groups.push(poe_alarm_core::AcceptableResultGroup {
            name: format!("可接受结果 {n}"),
            mode: ResultGroupMode::Any,
            required_count: 1,
            conditions: vec![poe_alarm_core::AffixCondition::default()],
        });
        self.persist();
        self.refresh_tree_select_last(window, cx);
    }

    pub fn add_condition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_editor_to_selection(cx);
        let group_ix = match self.selected_node() {
            NodeRef::Group(g) | NodeRef::Condition(g, _) => g,
            NodeRef::Game => 0,
        };
        let Some(backend) = &mut self.backend else {
            return;
        };
        let rules = backend.settings.selected_rules_mut();
        let set = rules
            .structured_rule_set
            .get_or_insert_with(Default::default);
        if set.groups.is_empty() {
            set.groups.push(poe_alarm_core::AcceptableResultGroup {
                name: "可接受结果 1".to_owned(),
                mode: ResultGroupMode::Any,
                required_count: 1,
                conditions: Vec::new(),
            });
        }
        let g = group_ix.min(set.groups.len() - 1);
        set.groups[g]
            .conditions
            .push(poe_alarm_core::AffixCondition::default());
        // 选中刚加进的词缀(树顺序:该组的最后一个条件)。
        let target = NodeRef::Condition(g, set.groups[g].conditions.len() - 1);
        self.persist();
        self.s.tree = Self::tree_from_settings(self.backend.as_ref());
        self.s.selected = self
            .s
            .tree
            .iter()
            .position(|n| n.node == target)
            .unwrap_or(0);
        self.sync_editor_from_selection(window, cx);
        cx.notify();
    }

    /// 删除词缀:只删当前词缀,组保留(空组会以"空"标出并阻止启动)。
    pub fn remove_selected_condition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let NodeRef::Condition(g, c) = self.selected_node() else {
            return;
        };
        if let Some(backend) = &mut self.backend
            && let Some(set) = backend
                .settings
                .selected_rules_mut()
                .structured_rule_set
                .as_mut()
            && let Some(group) = set.groups.get_mut(g)
            && c < group.conditions.len()
        {
            group.conditions.remove(c);
        }
        // 选中同组余下的邻近词缀,否则回到组节点。
        let fallback = NodeRef::Group(g);
        let target = NodeRef::Condition(g, c.saturating_sub(1));
        self.persist();
        self.s.tree = Self::tree_from_settings(self.backend.as_ref());
        self.s.selected = self
            .s
            .tree
            .iter()
            .position(|n| n.node == target)
            .or_else(|| self.s.tree.iter().position(|n| n.node == fallback))
            .unwrap_or(0);
        self.sync_editor_from_selection(window, cx);
        cx.notify();
    }

    /// 删除方案:删除选中方案及其全部词缀。
    pub fn remove_selected_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let g = match self.selected_node() {
            NodeRef::Group(g) | NodeRef::Condition(g, _) => g,
            NodeRef::Game => return,
        };
        if let Some(backend) = &mut self.backend
            && let Some(set) = backend
                .settings
                .selected_rules_mut()
                .structured_rule_set
                .as_mut()
            && g < set.groups.len()
        {
            set.groups.remove(g);
        }
        self.persist();
        self.s.tree = Self::tree_from_settings(self.backend.as_ref());
        let target = self
            .s
            .tree
            .iter()
            .position(|n| matches!(n.node, NodeRef::Condition(..)))
            .or_else(|| {
                self.s
                    .tree
                    .iter()
                    .position(|n| matches!(n.node, NodeRef::Group(_)))
            });
        self.s.selected = target.unwrap_or(0);
        self.sync_editor_from_selection(window, cx);
        cx.notify();
    }

    pub fn set_group_mode(
        &mut self,
        mode: ResultGroupMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let group_ix = match self.selected_node() {
            NodeRef::Group(g) | NodeRef::Condition(g, _) => g,
            NodeRef::Game => return,
        };
        if let Some(backend) = &mut self.backend
            && let Some(group) = backend
                .settings
                .selected_rules_mut()
                .structured_rule_set
                .as_mut()
                .and_then(|set| set.groups.get_mut(group_ix))
        {
            group.mode = mode;
            if mode == ResultGroupMode::AtLeast {
                group.required_count = group.required_count.clamp(1, group.conditions.len().max(1));
            }
        }
        self.persist();
        self.refresh_tree_keep_selection(window, cx);
    }

    /// "指定条数"步进(±1,夹在 1..=词缀数)。
    pub fn adjust_required_count(
        &mut self,
        delta: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let group_ix = match self.selected_node() {
            NodeRef::Group(g) | NodeRef::Condition(g, _) => g,
            NodeRef::Game => return,
        };
        if let Some(backend) = &mut self.backend
            && let Some(group) = backend
                .settings
                .selected_rules_mut()
                .structured_rule_set
                .as_mut()
                .and_then(|set| set.groups.get_mut(group_ix))
        {
            let max = group.conditions.len().max(1) as i64;
            let next = (group.required_count as i64 + delta).clamp(1, max);
            group.required_count = next as usize;
        }
        self.persist();
        self.refresh_tree_keep_selection(window, cx);
    }

    /// 数值行的比较方式切换,随手写回并自动保存。
    pub fn set_value_row_mode(
        &mut self,
        ix: usize,
        mode: NumericConstraintMode,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.s.value_rows.get_mut(ix) {
            row.mode = mode;
            if self.apply_editor_to_selection(cx) {
                self.persist();
            }
            cx.notify();
        }
    }

    fn refresh_tree_select_last(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.s.tree = Self::tree_from_settings(self.backend.as_ref());
        self.s.selected = self.s.tree.len().saturating_sub(1);
        self.sync_editor_from_selection(window, cx);
        cx.notify();
    }

    fn refresh_tree_keep_selection(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let node = self.selected_node();
        self.s.tree = Self::tree_from_settings(self.backend.as_ref());
        if let Some(ix) = self.s.tree.iter().position(|n| n.node == node) {
            self.s.selected = ix;
        }
        cx.notify();
    }

    /// Restarts the app elevated, so injected keystrokes reach an elevated game.
    ///
    /// Windows refuses input sent from a lower-integrity process to a
    /// higher-integrity window, and a launcher that starts the game as
    /// administrator puts it out of reach. There is no way to ask for that
    /// privilege without restarting, and no reason to ask for it up front:
    /// most people never need it, and a UAC prompt on every launch is a worse
    /// tax than a button nobody presses.
    ///
    /// Quitting is the last thing this does, and only once the elevated process
    /// exists. Exiting on "the request was submitted" tore down the UAC prompt
    /// along with the process that owned it, so the app closed and nothing
    /// else happened.
    pub fn relaunch_elevated(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            use poe_alarm_platform_win::ElevateError;

            let text = self.t();
            match poe_alarm_platform_win::relaunch_elevated(&[]) {
                // The elevated copy is starting; this one steps aside so two
                // instances never contend for the same global hotkeys.
                Ok(()) => cx.quit(),
                Err(ElevateError::Declined) => {
                    self.notice = Some((StatusKind::Warning, text.notice_elevate_declined.into()));
                }
                Err(_) => {
                    self.notice = Some((StatusKind::Error, text.notice_elevate_failed.into()));
                }
            }
            cx.notify();
        }
        #[cfg(not(windows))]
        {
            let _ = cx;
        }
    }

    /// 测试规则:拿粘贴框里的物品文本跑一次判定。
    ///
    /// 不再弹文件选择框。要检查的东西本来就在剪贴板里,让用户去存一个
    /// 文件再选回来,中间那两步纯粹是 OCR 时代留下的。
    pub fn check_item(&mut self, cx: &mut Context<Self>) {
        let item_text = self.s.item_text_input.read(cx).value().trim().to_string();
        let text = self.t();
        if item_text.is_empty() {
            self.notice = Some((StatusKind::Warning, text.notice_item_text_empty.into()));
            cx.notify();
            return;
        }
        self.run_item_check(item_text, text.log_check_prefix, cx);
    }

    /// Runs one rule check over item text, whatever produced it.
    ///
    /// `origin` names the source in the log so a hotkey check and a pasted one
    /// stay tellable apart.
    fn run_item_check(&mut self, item_text: String, origin: &'static str, cx: &mut Context<Self>) {
        let text = self.t();
        if let Some(backend) = &mut self.backend {
            match backend.check_item(item_text) {
                Ok(()) => self.push_log(LogKind::Meta, origin.to_owned()),
                Err(e) => {
                    self.notice = Some((StatusKind::Error, e.clone().into()));
                    self.push_log(
                        LogKind::Meta,
                        format!("{}:{e}", text.log_check_failed_prefix),
                    );
                }
            }
            cx.notify();
        }
    }

    /// 切换游戏或词缀语言:写设置、保存并整体刷新(模板/树)。
    pub fn switch_profile(
        &mut self,
        game: Option<poe_alarm_settings::GameProfile>,
        ocr_language: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_editor_to_selection(cx);
        let Some(backend) = &mut self.backend else {
            return;
        };
        if let Some(game) = game {
            backend.set_game(game);
        }
        if let Some(language) = ocr_language {
            backend.set_ocr_language(language);
        }
        Self::ensure_structured_rules(backend);
        let text = Self::text_for(Some(backend));
        self.notice = Some(match backend.save() {
            Ok(()) => (StatusKind::Monitoring, text.notice_profile_switched.into()),
            Err(e) => (
                StatusKind::Error,
                format!("{}:{e}", text.notice_profile_switch_failed_prefix).into(),
            ),
        });
        self.s.tree = Self::tree_from_settings(self.backend.as_ref());
        self.s.selected = self
            .s
            .tree
            .iter()
            .position(|n| matches!(n.node, NodeRef::Condition(..)))
            .unwrap_or(0);
        self.sync_editor_from_selection(window, cx);
        cx.notify();
    }

    // ---- 提醒与显示设置(改动即保存) --------------------------------------

    pub fn set_keep_hud_visible(&mut self, keep: bool, cx: &mut Context<Self>) {
        if let Some(backend) = &mut self.backend {
            backend.settings.keep_hud_visible = keep;
        }
        if let Some(backend) = &self.backend {
            backend.hud_set_visible(keep && self.s.run != RunPhase::Hit);
        }
        self.persist();
        cx.notify();
    }

    /// 切换启动热键(三选一,即时生效并保存)。
    pub fn set_start_hotkey(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(backend) = &mut self.backend {
            backend.set_start_hotkey_index(index);
        }
        self.persist();
        cx.notify();
    }

    pub fn set_allow_overlay_capture(&mut self, allow: bool, cx: &mut Context<Self>) {
        if let Some(backend) = &mut self.backend {
            backend.settings.allow_overlay_capture = allow;
            backend.hud_set_capture(allow);
        }
        self.invalidate_runtime();
        self.persist();
        self.push_log(LogKind::Meta, self.t().log_capture_updated.to_owned());
        cx.notify();
    }

    /// 选择自定义提示音(WAV);选中后立即保存并在下次启动生效。
    pub fn choose_alert_sound(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = receiver.await
                && let Some(path) = paths.pop()
            {
                let _ = this.update(cx, |this: &mut AppShell, cx| {
                    if let Some(backend) = &mut this.backend {
                        backend.settings.custom_alert_sound_path = Some(path.display().to_string());
                    }
                    this.invalidate_runtime();
                    this.persist();
                    let message = this.t().log_sound_changed_fmt.replacen(
                        "{}",
                        &path.display().to_string(),
                        1,
                    );
                    this.push_log(LogKind::Meta, message);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn clear_alert_sound(&mut self, cx: &mut Context<Self>) {
        if let Some(backend) = &mut self.backend {
            backend.settings.custom_alert_sound_path = None;
        }
        self.invalidate_runtime();
        self.persist();
        self.push_log(LogKind::Meta, self.t().log_sound_reset.to_owned());
        cx.notify();
    }

    // -- shared chrome ------------------------------------------------------

    /// 校验数值范围;返回错误文案(空间稳定:错误行占位恒定)。
    pub fn range_error(&self, cx: &Context<Self>) -> Option<&'static str> {
        for row in &self.s.value_rows {
            if row.mode != NumericConstraintMode::RangeInclusive {
                continue;
            }
            let min = row.min.read(cx).value().parse::<f64>().ok();
            let max = row.max.read(cx).value().parse::<f64>().ok();
            if let (Some(a), Some(b)) = (min, max)
                && a > b
            {
                return Some(self.t().range_error);
            }
        }
        None
    }

    pub fn ocr_ms_text(&self) -> String {
        if self.scan_count == 0 {
            "—".to_owned()
        } else {
            format!("{:.1} ms", self.ocr_ms)
        }
    }

    /// 底部状态栏(状态点+文字+计时坐标恒定)。
    pub fn shell_status_bar(&self, compact: bool) -> Div {
        let text = self.t();
        let kind_color = match self.s.run {
            RunPhase::Idle => TEXT_SECONDARY,
            RunPhase::Monitoring => ACCENT_TEXT,
            RunPhase::Hit => DANGER_TEXT,
        };
        let ocr = format!("{} {}", text.bar_ocr_prefix, self.ocr_ms_text());
        let scans = format!("{} {}", text.bar_scan_prefix, self.scan_count);
        if compact {
            status_bar(
                &[
                    StatusSegment {
                        text: text.status_text(self.s.run),
                        color: Some(kind_color),
                    },
                    StatusSegment {
                        text: &scans,
                        color: None,
                    },
                    StatusSegment {
                        text: &ocr,
                        color: None,
                    },
                ],
                Some("F10 · F12"),
            )
        } else {
            let notice_text = self
                .notice
                .as_ref()
                .map(|(_, t)| t.to_string())
                .unwrap_or_else(|| text.bar_hint.to_owned());
            status_bar(
                &[
                    StatusSegment {
                        text: text.status_text(self.s.run),
                        color: Some(kind_color),
                    },
                    StatusSegment {
                        text: &scans,
                        color: None,
                    },
                    StatusSegment {
                        text: &ocr,
                        color: None,
                    },
                ],
                Some(&notice_text),
            )
        }
    }

    /// 真实归一化预览:用 core 的 canonicalize 展示模板归一化结果与数值占位数。
    pub fn normalized_preview(&self, cx: &Context<Self>) -> SharedString {
        let text = self.t();
        let template = self.s.template_input.read(cx).value();
        let trimmed = template.trim();
        if trimmed.is_empty() {
            return text.normalized_empty.into();
        }
        let canonical = poe_alarm_core::canonicalize(trimmed);
        let values = poe_alarm_core::extract_values(trimmed).len();
        text.normalized_fmt
            .replacen("{}", &canonical.text, 1)
            .replacen("{}", &values.to_string(), 1)
            .into()
    }

    /// 标题栏的游戏/OCR 语言切换(带说明标签,一眼可见)。
    pub fn profile_switcher(&self, cx: &mut Context<Self>) -> Div {
        use poe_alarm_settings::GameProfile;
        let text = self.t();
        let (game, ocr) = match &self.backend {
            Some(b) => (b.settings.selected_game_profile, b.ocr_language_label()),
            None => (GameProfile::Poe2, String::new()),
        };
        let traditional = ocr.starts_with("zh");
        let chip = |id: &'static str,
                    label: &'static str,
                    active: bool,
                    cx: &mut Context<Self>,
                    action: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            let mut cell = div()
                .id(id)
                .h(px(H_CHIP))
                .px(px(10.))
                .flex()
                .items_center()
                .text_size(fs(FS_11_5))
                .whitespace_nowrap()
                .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)));
            cell = if active {
                cell.bg(c(ACCENT_WASH)).text_color(c(ACCENT_TEXT))
            } else {
                cell.bg(c(PANEL))
                    .text_color(c(TEXT_SECONDARY))
                    .hover(|s| s.bg(c(HOVER)))
            };
            cell.child(label)
        };
        let caption = |label: &'static str| {
            div()
                .text_size(fs(FS_10))
                .text_color(c(TEXT_META))
                .whitespace_nowrap()
                .child(label)
        };
        let seg = |cells: Div| cells.h_flex().border_1().border_color(c(HAIRLINE));
        div()
            .h_flex()
            .items_center()
            .gap_2()
            .child(caption(text.game_label))
            .child(
                seg(div())
                    .child(chip(
                        "pf-poe1",
                        "POE 1",
                        game == GameProfile::Poe1,
                        cx,
                        |t, w, cx| {
                            t.switch_profile(
                                Some(poe_alarm_settings::GameProfile::Poe1),
                                None,
                                w,
                                cx,
                            )
                        },
                    ))
                    .child(
                        chip(
                            "pf-poe2",
                            "POE 2",
                            game == GameProfile::Poe2,
                            cx,
                            |t, w, cx| {
                                t.switch_profile(
                                    Some(poe_alarm_settings::GameProfile::Poe2),
                                    None,
                                    w,
                                    cx,
                                )
                            },
                        )
                        .border_l_1()
                        .border_color(c(HAIRLINE)),
                    ),
            )
            .child(caption(text.ocr_language_label))
            .child(
                seg(div())
                    .child(chip("pf-zh", text.ocr_zh, traditional, cx, |t, w, cx| {
                        t.switch_profile(None, Some("zh-TW"), w, cx)
                    }))
                    .child(
                        chip("pf-en", text.ocr_en, !traditional, cx, |t, w, cx| {
                            t.switch_profile(None, Some("en"), w, cx)
                        })
                        .border_l_1()
                        .border_color(c(HAIRLINE)),
                    ),
            )
    }

    /// 运行状态块(右栏;状态点呼吸是三处动效之一)。
    pub fn run_status_block(&self) -> Div {
        let kind = self.s.run.status_kind();
        div()
            .h_flex()
            .items_center()
            .gap(px(9.))
            .child(breathing_dot("run-dot", kind))
            .child(
                div()
                    .text_size(fs(FS_13))
                    .font_semibold()
                    .text_color(c(kind.text()))
                    .child(SharedString::from(self.t().status_text(self.s.run))),
            )
            .child(
                div()
                    .ml_auto()
                    .font_family(FONT_MONO)
                    .text_size(fs(FS_11_5))
                    .text_color(c(TEXT_SECONDARY))
                    .child(self.s.elapsed.clone()),
            )
    }

    /// 指标行组(真实运行数据)。
    pub fn metrics_block(&self) -> Div {
        let text = self.t();
        let dash = |v: &str| {
            if self.scan_count == 0 {
                "—".to_owned()
            } else {
                v.to_owned()
            }
        };
        div()
            .v_flex()
            .child(metric_row(
                text.metric_ocr,
                &dash(&format!("{:.1} ms", self.ocr_ms)),
                false,
            ))
            .child(metric_row(
                text.metric_capture,
                &dash(&format!("{:.1} ms", self.capture_ms)),
                false,
            ))
            .child(metric_row(
                text.metric_scans,
                &dash(&self.scan_count.to_string()),
                false,
            ))
            .child(metric_row(
                text.metric_hits,
                &self.s.hit_count.to_string(),
                true,
            ))
    }

    /// 日志面板(真实事件流;最新在下)。
    pub fn log_block(&self, max: usize) -> Div {
        let start = self.log.len().saturating_sub(max);
        let lines: Vec<LogLine> = self.log[start..]
            .iter()
            .map(|(kind, text)| match kind {
                LogKind::Meta => LogLine::Meta(text.clone()),
                LogKind::Text => LogLine::Text(text.clone()),
                LogKind::Match => LogLine::Match(text.clone()),
                LogKind::Hit => LogLine::Hit(text.clone()),
            })
            .collect();
        log_pane(&lines)
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self.render_workbench(window, cx);
        div()
            .id("app-shell")
            .track_focus(&self.focus_handle)
            .v_flex()
            .size_full()
            .bg(c(CANVAS))
            .text_color(c(TEXT_PRIMARY))
            .font_family(FONT_UI)
            .text_size(fs(FS_12))
            .child(body)
    }
}
