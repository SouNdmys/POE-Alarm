//! AppShell:三档窗口的宿主实体(Phase 4:已接真实运行时桥)。
//!
//! 持有唯一的 ViewState + Backend,处理 Ctrl⇧1/2/3 切档、监控启停、
//! 运行时事件轮询与树/tab 交互。切档只改布局与窗口尺寸,所有 Entity 保留。

use std::time::{Duration, Instant};

use gpui::{
    Context, Div, FocusHandle, KeyDownEvent, SharedString, Window, div, prelude::*, px, size,
};
use gpui_component::{StyledExt, input::InputState};
use poe_alarm_settings::GameProfile;

use crate::backend::{Backend, BridgeEvent, BridgeState, PlatformEvent};
use crate::state::*;
use crate::theme::*;
use crate::ui::*;

gpui::actions!(poe_alarm, [SwitchWorkbench, SwitchInstrument, SwitchDock]);

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
    pub scan_count: u64,
    pub capture_ms: f64,
    pub ocr_ms: f64,
    pub ocr_cached: bool,
}

impl AppShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = match Backend::new() {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("backend init failed: {e}");
                None
            }
        };
        let initial_template = backend
            .as_ref()
            .map(|b| b.settings.selected_rules().target_affix.clone())
            .unwrap_or_default();

        let name_input = cx.new(|cx| InputState::new(window, cx).default_value("单条目标"));
        let template_input = cx.new(|cx| {
            let s = InputState::new(window, cx)
                .placeholder("粘贴完整词缀,例如:若近期有造成暴擊,增加 (6—8)% 攻擊速度");
            if initial_template.is_empty() {
                s
            } else {
                s.default_value(initial_template)
            }
        });
        fn mk(
            window: &mut Window,
            cx: &mut Context<AppShell>,
            v: &str,
        ) -> gpui::Entity<InputState> {
            let v = v.to_string();
            cx.new(|cx| InputState::new(window, cx).default_value(v))
        }
        let command_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("粘贴词缀后点「设为目标」"));
        let value_rows = vec![ValueRow {
            label: "1 · 百分比".into(),
            comparison: "在范围内".into(),
            min: mk(window, cx, "3.1"),
            max: mk(window, cx, "3.8"),
        }];

        let tree = Self::tree_from_settings(backend.as_ref());
        let notice = match &backend {
            Some(b) if b.read_only => Some((
                StatusKind::Warning,
                "设置文件由更新版本写入,当前会话只读".into(),
            )),
            None => Some((StatusKind::Error, "运行时后端初始化失败,详见控制台".into())),
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

        Self {
            s: ViewState {
                tier: Tier::Workbench,
                run: RunPhase::Idle,
                editor_tab: EditorTab::Conditions,
                target_mode: TargetMode::Single,
                tree,
                selected: 0,
                name_input,
                template_input,
                command_input,
                value_rows,
                elapsed: "--:--".into(),
                hit_count: 0,
            },
            backend,
            focus_handle: cx.focus_handle(),
            log: vec![(LogKind::Meta, "就绪 · 等待开始监控".into())],
            notice,
            monitor_since: None,
            scan_count: 0,
            capture_ms: 0.0,
            ocr_ms: 0.0,
            ocr_cached: false,
        }
    }

    /// 从设置里的结构化规则集合成树显示(display-only;结构化编辑 Phase 4b)。
    fn tree_from_settings(backend: Option<&Backend>) -> Vec<RuleNode> {
        let mut tree = Vec::new();
        let Some(backend) = backend else {
            return tree;
        };
        let settings = &backend.settings;
        let game_label = match settings.selected_game_profile {
            GameProfile::Poe1 => "流放之路一",
            GameProfile::Poe2 => "流放之路二",
        };
        tree.push(RuleNode {
            depth: 0,
            label: game_label.into(),
            trailing: "活动".into(),
            expandable: true,
            expanded: true,
            warning: false,
            disabled: false,
        });
        let rules = settings.selected_rules();
        match &rules.structured_rule_set {
            Some(set) if !set.groups.is_empty() => {
                for group in &set.groups {
                    let mode = match group.mode {
                        poe_alarm_core::ResultGroupMode::Any => "任意".to_owned(),
                        poe_alarm_core::ResultGroupMode::All => "全部".to_owned(),
                        poe_alarm_core::ResultGroupMode::AtLeast => {
                            format!("≥{}/{}", group.required_count, group.conditions.len())
                        }
                    };
                    let group_name = if group.name.trim().is_empty() {
                        "可接受结果".to_owned()
                    } else {
                        group.name.clone()
                    };
                    tree.push(RuleNode {
                        depth: 1,
                        label: group_name.into(),
                        trailing: mode.into(),
                        expandable: true,
                        expanded: true,
                        warning: false,
                        disabled: false,
                    });
                    for cond in &group.conditions {
                        let label = if cond.name.trim().is_empty() {
                            cond.template.clone()
                        } else {
                            cond.name.clone()
                        };
                        let missing = cond.template.trim().is_empty();
                        tree.push(RuleNode {
                            depth: 2,
                            label: label.into(),
                            trailing: if missing {
                                "待补".into()
                            } else {
                                format!("{} 值", cond.numeric_constraints.len()).into()
                            },
                            expandable: false,
                            expanded: false,
                            warning: missing,
                            disabled: false,
                        });
                    }
                }
            }
            _ => {
                let target = rules.target_affix.trim();
                tree.push(RuleNode {
                    depth: 1,
                    label: if target.is_empty() {
                        "尚未设置目标词缀".into()
                    } else {
                        target.to_owned().into()
                    },
                    trailing: "单条".into(),
                    expandable: false,
                    expanded: false,
                    warning: target.is_empty(),
                    disabled: false,
                });
            }
        }
        tree
    }

    // -- runtime event loop -------------------------------------------------

    fn tick(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut reset_runtime = false;
        let platform_events = match &mut self.backend {
            Some(backend) => backend.poll_platform(),
            None => Vec::new(),
        };
        for event in platform_events {
            changed = true;
            match event {
                PlatformEvent::HotKeyStart => {
                    if self.s.run == RunPhase::Idle {
                        self.toggle_run(cx);
                    }
                }
                PlatformEvent::HotKeySelectRegion => self.begin_region_selection(cx),
                PlatformEvent::HotKeyStopOrAcknowledge => {
                    if self.s.run != RunPhase::Idle {
                        self.toggle_run(cx);
                    }
                }
                PlatformEvent::RegionSelected(region) => {
                    if let Some(backend) = &mut self.backend {
                        backend.set_region(region);
                        let label = backend.region_label();
                        match backend.save() {
                            Ok(()) => {
                                self.notice = Some((
                                    StatusKind::Monitoring,
                                    format!("区域已保存 · {label}").into(),
                                ));
                                self.push_log(LogKind::Meta, format!("识别区域已更新 · {label}"));
                            }
                            Err(e) => {
                                self.notice =
                                    Some((StatusKind::Error, format!("区域保存失败:{e}").into()));
                            }
                        }
                    }
                }
                PlatformEvent::RegionSelectionCancelled => {
                    self.push_log(LogKind::Meta, "框选已取消".to_owned());
                }
                PlatformEvent::RegionSelectionFailed => {
                    self.notice = Some((StatusKind::Error, "框选失败,详见控制台".into()));
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
                    let group = matched_group.unwrap_or_else(|| "目标".to_owned());
                    self.push_log(LogKind::Hit, format!("规则命中 · {group} · {detail}"));
                }
                BridgeEvent::AlertPresented => {
                    self.s.run = RunPhase::Hit;
                    self.push_log(
                        LogKind::Hit,
                        "红色拦截窗已接管输入 · Ctrl⇧F12 解除".to_owned(),
                    );
                }
                BridgeEvent::AlertAcknowledged => {
                    self.apply_runtime_state(BridgeState::Idle);
                    self.push_log(LogKind::Meta, "已确认 · 下一轮需重新启动".to_owned());
                }
                BridgeEvent::Fault(detail) => {
                    self.notice = Some((StatusKind::Error, detail.clone().into()));
                    self.push_log(LogKind::Meta, format!("错误:{detail}"));
                    self.apply_runtime_state(BridgeState::Idle);
                    reset_runtime = true;
                }
                BridgeEvent::SoundFallback => {
                    self.notice = Some((
                        StatusKind::Warning,
                        "自定义提示音无效,已回退内置音效".into(),
                    ));
                }
            }
        }
        if reset_runtime && let Some(backend) = &mut self.backend {
            backend.reset_runtime();
            self.push_log(LogKind::Meta, "运行时已重建,可重新开始监控".to_owned());
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
            cx.notify();
        }
    }

    fn apply_runtime_state(&mut self, state: BridgeState) {
        let next = match state {
            BridgeState::Starting | BridgeState::Monitoring | BridgeState::TestingScreenshot => {
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
        }
        if next != RunPhase::Monitoring {
            self.monitor_since = None;
            if next == RunPhase::Idle {
                self.s.elapsed = "--:--".into();
            }
        }
        self.s.run = next;
    }

    fn push_log(&mut self, kind: LogKind, text: String) {
        self.log.push((kind, text.into()));
        if self.log.len() > LOG_CAP {
            let excess = self.log.len() - LOG_CAP;
            self.log.drain(..excess);
        }
    }

    // -- state transitions --------------------------------------------------

    pub fn switch_tier(&mut self, tier: Tier, window: &mut Window, cx: &mut Context<Self>) {
        if self.s.tier != tier {
            self.s.tier = tier;
            let (w, h) = tier.size();
            window.resize(size(px(w), px(h)));
            cx.notify();
        }
    }

    /// 主操作:开始监控 / 停止监控 / 解除鼠标锁定。
    pub fn toggle_run(&mut self, cx: &mut Context<Self>) {
        // 先把单条模板写回设置,保证 runtime 拿到的是屏幕上的内容。
        let template = self.s.template_input.read(cx).value().to_string();
        let Some(backend) = &mut self.backend else {
            self.notice = Some((StatusKind::Error, "后端未初始化".into()));
            cx.notify();
            return;
        };
        let starting = self.s.run == RunPhase::Idle;
        let result = match self.s.run {
            RunPhase::Idle => {
                if self.s.target_mode == TargetMode::Single {
                    backend.apply_single_target(&template);
                }
                backend.start_monitoring()
            }
            RunPhase::Monitoring => backend.stop_monitoring(),
            RunPhase::Hit => backend.acknowledge_alert(),
        };
        if starting && result.is_ok() {
            self.push_log(LogKind::Meta, "启动监控…".to_owned());
        }
        if let Err(e) = result {
            self.notice = Some((StatusKind::Error, e.clone().into()));
            self.push_log(LogKind::Meta, format!("错误:{e}"));
        }
        cx.notify();
    }

    /// Dock 命令行:把粘贴的词缀设为当前单条目标并保存。
    pub fn apply_command_as_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.s.command_input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.s.template_input.update(cx, |input, cx| {
            input.set_value(text.clone(), window, cx);
        });
        self.s.target_mode = TargetMode::Single;
        if let Some(backend) = &mut self.backend {
            backend.apply_single_target(&text);
            self.notice = Some(match backend.save() {
                Ok(()) => (StatusKind::Monitoring, "已设为目标并保存".into()),
                Err(e) => (StatusKind::Error, format!("保存失败:{e}").into()),
            });
            self.s.tree = Self::tree_from_settings(self.backend.as_ref());
        }
        cx.notify();
    }

    /// 触发框选(热键 Ctrl⇧F11 或界面按钮)。
    pub fn begin_region_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(backend) = &mut self.backend {
            if let Err(e) = backend.begin_region_selection() {
                self.notice = Some((StatusKind::Error, e.into()));
            } else {
                self.push_log(LogKind::Meta, "框选中:拖拽圈出词缀区域,Esc 取消".to_owned());
            }
            cx.notify();
        }
    }

    /// 验证并保存设置。
    pub fn save_settings(&mut self, cx: &mut Context<Self>) {
        if self.range_error(cx).is_some() {
            self.notice = Some((StatusKind::Error, "数值范围未通过校验,未保存".into()));
            cx.notify();
            return;
        }
        let template = self.s.template_input.read(cx).value().to_string();
        let Some(backend) = &mut self.backend else {
            return;
        };
        if self.s.target_mode == TargetMode::Single {
            backend.apply_single_target(&template);
        }
        let path = backend.settings_path();
        self.notice = Some(match backend.save() {
            Ok(()) => {
                self.push_log(LogKind::Meta, format!("设置已保存 · {path}"));
                (StatusKind::Monitoring, "已保存".into())
            }
            Err(e) => {
                self.push_log(LogKind::Meta, format!("保存失败:{e}"));
                (StatusKind::Error, format!("保存失败:{e}").into())
            }
        });
        self.s.tree = Self::tree_from_settings(self.backend.as_ref());
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        if ks.modifiers.control && ks.modifiers.shift {
            match ks.key.as_str() {
                "1" | "!" => self.switch_tier(Tier::Workbench, window, cx),
                "2" | "@" => self.switch_tier(Tier::Instrument, window, cx),
                "3" | "#" => self.switch_tier(Tier::Dock, window, cx),
                _ => {}
            }
        }
    }

    // -- shared chrome ------------------------------------------------------

    /// 校验数值范围;返回错误文案(空间稳定:错误行占位恒定)。
    pub fn range_error(&self, cx: &Context<Self>) -> Option<&'static str> {
        for row in &self.s.value_rows {
            let min = row.min.read(cx).value().parse::<f64>().ok();
            let max = row.max.read(cx).value().parse::<f64>().ok();
            if let (Some(a), Some(b)) = (min, max)
                && a > b
            {
                return Some("数值范围的最小值不能大于最大值");
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

    /// 底部状态栏(三档共用同一套文案;状态点+文字+计时坐标恒定)。
    pub fn shell_status_bar(&self, compact: bool) -> Div {
        let kind_color = match self.s.run {
            RunPhase::Idle => TEXT_SECONDARY,
            RunPhase::Monitoring => ACCENT_TEXT,
            RunPhase::Hit => DANGER_TEXT,
        };
        let ocr = format!("判定 {}", self.ocr_ms_text());
        let scans = format!("扫描 {}", self.scan_count);
        if compact {
            status_bar(
                &[
                    StatusSegment {
                        text: self.s.run.status_text(),
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
                Some("F10 · F11 · F12"),
            )
        } else {
            let notice_text = self
                .notice
                .as_ref()
                .map(|(_, t)| t.to_string())
                .unwrap_or_else(|| "Ctrl⇧F10 开始 · F11 框选 · F12 停止".to_owned());
            status_bar(
                &[
                    StatusSegment {
                        text: self.s.run.status_text(),
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
        let template = self.s.template_input.read(cx).value();
        let trimmed = template.trim();
        if trimmed.is_empty() {
            return "归一化 — · 粘贴完整词缀后在此预览".into();
        }
        let canonical = poe_alarm_core::canonicalize(trimmed);
        let values = poe_alarm_core::extract_values(trimmed).len();
        format!(
            "归一化 {} · {} 个数值占位 · 只比屏幕显示值",
            canonical.text, values
        )
        .into()
    }

    /// 标题栏右侧的三档切换块(点击直达;快捷键 Ctrl⇧1/2/3)。
    pub fn tier_switcher(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .h_flex()
            .flex_none()
            .border_1()
            .border_color(c(HAIRLINE));
        for (i, (label, tier)) in [
            ("1 规则台", Tier::Workbench),
            ("2 仪表", Tier::Instrument),
            ("3 停靠", Tier::Dock),
        ]
        .into_iter()
        .enumerate()
        {
            let mut cell = div()
                .id(("tier-chip", i))
                .h(px(H_CHIP - 2.))
                .px(px(8.))
                .flex()
                .items_center()
                .font_family(FONT_MONO)
                .text_size(fs(FS_10))
                .whitespace_nowrap()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.switch_tier(tier, window, cx);
                }));
            if i > 0 {
                cell = cell.border_l_1().border_color(c(HAIRLINE));
            }
            cell = if self.s.tier == tier {
                cell.bg(c(ACCENT_WASH)).text_color(c(ACCENT_TEXT))
            } else {
                cell.bg(c(PANEL))
                    .text_color(c(TEXT_SECONDARY))
                    .hover(|s| s.bg(c(HOVER)))
            };
            row = row.child(cell.child(label));
        }
        row
    }

    /// 运行状态块(右栏 / Dock 共用;状态点呼吸是三处动效之一)。
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
                    .child(SharedString::from(self.s.run.status_text())),
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
                "判定耗时",
                &dash(&format!("{:.1} ms", self.ocr_ms)),
                false,
            ))
            .child(metric_row(
                "截屏耗时",
                &dash(&format!("{:.1} ms", self.capture_ms)),
                false,
            ))
            .child(metric_row(
                "扫描轮数",
                &dash(&self.scan_count.to_string()),
                false,
            ))
            .child(metric_row("本轮命中", &self.s.hit_count.to_string(), true))
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
        let body = match self.s.tier {
            Tier::Workbench => self.render_workbench(window, cx),
            Tier::Instrument => self.render_instrument(window, cx),
            Tier::Dock => self.render_dock(window, cx),
        };
        div()
            .id("app-shell")
            .track_focus(&self.focus_handle)
            .v_flex()
            .size_full()
            .bg(c(CANVAS))
            .text_color(c(TEXT_PRIMARY))
            .font_family(FONT_UI)
            .text_size(fs(FS_12))
            .on_key_down(cx.listener(|this, event, window, cx| this.on_key(event, window, cx)))
            .on_action(cx.listener(|this, _: &SwitchWorkbench, window, cx| {
                this.switch_tier(Tier::Workbench, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchInstrument, window, cx| {
                this.switch_tier(Tier::Instrument, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SwitchDock, window, cx| {
                this.switch_tier(Tier::Dock, window, cx);
            }))
            .child(body)
    }
}
