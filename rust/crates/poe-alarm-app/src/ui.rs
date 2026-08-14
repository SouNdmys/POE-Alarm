//! Ledger v1 base components (章节 03–08).
//!
//! Everything here draws exclusively from `theme` tokens. Hover only changes
//! background, never size or position. Panels/inputs have 0 radius, buttons
//! and chips 2px.

// Phase 3 will consume the remaining tokens/components; keep the full set now.
#![allow(dead_code)]

use gpui::{AnyElement, App, Div, IntoElement, ParentElement, SharedString, Styled, div, px};
use gpui_component::{
    StyledExt,
    button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants},
};

use crate::theme::*;

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

/// Simulate the spec's `.14em` letter-spacing (gpui has no letter-spacing)
/// by interspersing U+2009 THIN SPACE between characters.
pub fn spaced(text: &str) -> SharedString {
    let mut out = String::with_capacity(text.len() * 2);
    let mut first = true;
    for ch in text.chars() {
        if !first {
            out.push('\u{2009}');
        }
        out.push(ch);
        first = false;
    }
    out.into()
}

/// 微标题:等宽 + 字距,仅用于区块标题。
/// (按用户反馈从 10px 提到 10.5px、meta 色提到次级色,保证可读性。)
pub fn micro_title(text: &str) -> Div {
    div()
        .font_family(FONT_MONO)
        .text_size(fs(FS_10_5))
        .text_color(c(TEXT_SECONDARY))
        .child(spaced(text))
}

/// 窄栏微标题(10px,用户反馈上调)。
pub fn micro_title_sm(text: &str) -> Div {
    div()
        .font_family(FONT_MONO)
        .text_size(fs(FS_10))
        .text_color(c(TEXT_META))
        .child(spaced(text))
}

/// 等宽数据文字。
pub fn mono(text: impl Into<SharedString>) -> Div {
    div().font_family(FONT_MONO).child(text.into())
}

// ---------------------------------------------------------------------------
// Section scaffolding(面板 · 栏头 · 发丝线)
// ---------------------------------------------------------------------------

/// 内容面板:panel 底 + strong 外框(0 圆角、无阴影)。
pub fn panel() -> Div {
    div().bg(c(PANEL)).border_1().border_color(c(HAIRLINE))
}

/// 区块头:rail 底 + 微标题,高度随内容(9px 12px padding)。
pub fn panel_header(title: &str) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap_3()
        .px_3()
        .py_2()
        .bg(c(RAIL))
        .border_b_1()
        .border_color(c(HAIRLINE))
        .child(micro_title(title))
}

/// 水平分隔线(soft)。
pub fn hairline_soft() -> Div {
    div().h(px(1.)).flex_none().bg(c(HAIRLINE_SOFT))
}

/// 章节小标题行:微标题 + 一条 soft 线拉满(设计稿"数值条件"样式)。
pub fn inline_section(title: &str) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap_2()
        .child(micro_title_sm(title))
        .child(div().flex_1().h(px(1.)).bg(c(HAIRLINE_SOFT)))
}

// ---------------------------------------------------------------------------
// Status(章节 04 · 08)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusKind {
    /// ready / idle · 中性灰,无底色
    Idle,
    /// 正在监控 · 墨青
    Monitoring,
    /// 已命中 · 砖红
    Hit,
    /// 需要注意 · 琥珀
    Warning,
    /// 错误 · 砖红
    Error,
    /// 只读 / 不可用
    Disabled,
}

impl StatusKind {
    pub fn dot(self) -> u32 {
        match self {
            StatusKind::Idle => NEUTRAL_DOT,
            StatusKind::Monitoring => ACCENT,
            StatusKind::Hit | StatusKind::Error => DANGER,
            StatusKind::Warning => WARN,
            StatusKind::Disabled => DISABLED_DOT,
        }
    }

    pub fn text(self) -> u32 {
        match self {
            StatusKind::Idle => TEXT_SECONDARY,
            StatusKind::Monitoring => ACCENT_TEXT,
            StatusKind::Hit | StatusKind::Error => DANGER,
            StatusKind::Warning => WARN,
            StatusKind::Disabled => TEXT_DISABLED,
        }
    }
}

/// 状态点(7px 圆)。
pub fn status_dot(kind: StatusKind) -> Div {
    let base = div()
        .size(px(7.))
        .flex_none()
        .rounded_full()
        .bg(c(kind.dot()));
    if kind == StatusKind::Disabled {
        base.border_1().border_color(c(HAIRLINE))
    } else {
        base
    }
}

/// 状态行:点 + 文字(600 字重 13px)+ 右侧等宽计时。
pub fn status_line(kind: StatusKind, label: &str, elapsed: &str) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap(px(9.))
        .child(status_dot(kind))
        .child(
            div()
                .text_size(fs(FS_13))
                .font_semibold()
                .text_color(c(kind.text()))
                .child(SharedString::from(label.to_string())),
        )
        .child(
            div()
                .ml_auto()
                .font_family(FONT_MONO)
                .text_size(fs(FS_11_5))
                .text_color(c(TEXT_SECONDARY))
                .child(SharedString::from(elapsed.to_string())),
        )
}

/// 琥珀注意条:2px 左边框 + wash 底,就地显示,不弹窗。
pub fn warning_band(tag: &str, text: &str) -> Div {
    div()
        .h_flex()
        .gap(px(9.))
        .px(px(11.))
        .py_2()
        .bg(c(WARN_WASH))
        .border_l_2()
        .border_color(c(WARN_BAR))
        .child(
            div()
                .font_family(FONT_MONO)
                .text_size(fs(FS_10))
                .text_color(c(WARN))
                .child(SharedString::from(tag.to_string())),
        )
        .child(
            div()
                .text_size(fs(FS_11_5))
                .line_height(px(FS_11_5 * 1.55))
                .text_color(c(WARN_TEXT))
                .child(SharedString::from(text.to_string())),
        )
}

/// 校验错误行:红点 + 文案,底 DANGER_WASH。占位恒定一行高。
pub fn error_band(text: &str) -> Div {
    div()
        .h_flex()
        .items_center()
        .gap_2()
        .px(px(10.))
        .py(px(6.))
        .bg(c(DANGER_WASH))
        .border_t_1()
        .border_color(c(DANGER_LINE))
        .child(div().size(px(6.)).flex_none().rounded_full().bg(c(DANGER)))
        .child(
            div()
                .text_size(fs(FS_11_5))
                .text_color(c(DANGER_TEXT))
                .child(SharedString::from(text.to_string())),
        )
}

/// 命中 wash 行(日志里的"规则命中"高亮)。
pub fn accent_wash_line(text: &str) -> Div {
    div()
        .px_2()
        .py(px(5.))
        .bg(c(ACCENT_WASH))
        .border_l_2()
        .border_color(c(ACCENT))
        .font_family(FONT_MONO)
        .text_size(fs(FS_10_5))
        .text_color(c(ACCENT_TEXT))
        .child(SharedString::from(text.to_string()))
}

// ---------------------------------------------------------------------------
// Buttons(章节 05 · 四类)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LedgerButton {
    /// 墨青实底 h28,一屏只允许一个
    Primary,
    /// panel 底 + hairline 边 h28
    Secondary,
    /// 无边框 h26,accent 文字
    Quiet,
    /// 红边红字 h26,pressed 变实底
    Destructive,
}

/// 构造 Ledger 风格按钮(基于 gpui-component Button:保留焦点环/键盘/禁用逻辑,
/// 颜色与几何全部覆盖为 token)。
pub fn button(id: &'static str, kind: LedgerButton, label: &str, cx: &App) -> Button {
    use gpui_component::ActiveTheme as _;
    let _ = cx.theme();
    let (variant, height) = match kind {
        LedgerButton::Primary => (
            ButtonCustomVariant::new(cx)
                .color(hsla_of(ACCENT))
                .foreground(hsla_of(ON_ACCENT))
                .border(hsla_of(ACCENT))
                .hover(hsla_of(ACCENT_HOVER))
                .active(hsla_of(ACCENT_PRESSED)),
            H_BUTTON,
        ),
        LedgerButton::Secondary => (
            ButtonCustomVariant::new(cx)
                .color(hsla_of(PANEL))
                .foreground(hsla_of(TEXT_PRIMARY))
                .border(hsla_of(HAIRLINE))
                .hover(hsla_of(HOVER))
                .active(hsla_of(PRESSED)),
            H_BUTTON,
        ),
        LedgerButton::Quiet => (
            ButtonCustomVariant::new(cx)
                .color(gpui::transparent_black())
                .foreground(hsla_of(ACCENT_TEXT))
                .border(gpui::transparent_black())
                .hover(hsla_of(HOVER))
                .active(hsla_of(PRESSED)),
            H_INPUT,
        ),
        LedgerButton::Destructive => (
            ButtonCustomVariant::new(cx)
                .color(hsla_of(PANEL))
                .foreground(hsla_of(DANGER))
                .border(hsla_of(DANGER_LINE))
                .hover(hsla_of(DANGER_WASH))
                .active(hsla_of(DANGER)),
            H_INPUT,
        ),
    };
    let btn = Button::new(id)
        .custom(variant)
        .rounded(ButtonRounded::Size(px(RADIUS_BUTTON)))
        .label(SharedString::from(label.to_string()))
        .h(px(height));
    // 用户反馈:Primary 底色深,12px 常规字重不够清晰 → 12.5 + 600。
    if kind == LedgerButton::Primary {
        btn.text_size(fs(FS_12_5)).font_semibold()
    } else {
        btn.text_size(fs(FS_12))
    }
}

/// 呼吸状态点:监控中 1.6s 透明度 1 → 0.45(设计允许的三处动效之一)。
pub fn breathing_dot(id: &'static str, kind: StatusKind) -> gpui::AnyElement {
    use gpui::{Animation, AnimationExt as _};
    use std::time::Duration;
    if kind == StatusKind::Monitoring {
        status_dot(kind)
            .with_animation(
                id,
                Animation::new(Duration::from_millis(1600))
                    .repeat()
                    .with_easing(gpui::pulsating_between(0.45, 1.0)),
                |dot, delta| dot.opacity(delta),
            )
            .into_any_element()
    } else {
        status_dot(kind).into_any_element()
    }
}

/// 热键 chip:等宽 10px,1px 边框,2px 圆角。
pub fn hotkey_chip(key: &str) -> Div {
    div()
        .flex_none()
        .border_1()
        .border_color(c(HAIRLINE))
        .bg(c(PANEL))
        .rounded(px(RADIUS_BUTTON))
        .px(px(6.))
        .py(px(3.))
        .font_family(FONT_MONO)
        .text_size(fs(FS_10))
        .text_color(c(TEXT_SECONDARY))
        .child(SharedString::from(key.to_string()))
}

/// 一组热键 chip。
pub fn hotkey_chips(keys: &[&str]) -> Div {
    let mut row = div().h_flex().gap_1();
    for k in keys {
        row = row.child(hotkey_chip(k));
    }
    row
}

// ---------------------------------------------------------------------------
// Segmented control(章节 06 ·「任意 / 全部 / 指定条数」)
// ---------------------------------------------------------------------------

/// 分段控件(静态渲染;交互接线在 Phase 3 由调用方包 on_click)。
pub fn segmented(items: &[&str], selected: usize, height: f32) -> Div {
    let mut row = div()
        .h_flex()
        .flex_none()
        .border_1()
        .border_color(c(HAIRLINE));
    for (i, item) in items.iter().enumerate() {
        let mut cell = div()
            .h(px(height - 2.))
            .flex_none()
            .px(px(10.))
            .flex()
            .items_center()
            .text_size(fs(FS_11_5))
            .whitespace_nowrap();
        if i > 0 {
            cell = cell.border_l_1().border_color(c(HAIRLINE));
        }
        cell = if i == selected {
            cell.bg(c(ACCENT_WASH)).text_color(c(ACCENT_TEXT))
        } else {
            cell.bg(c(PANEL)).text_color(c(TEXT_SECONDARY))
        };
        row = row.child(cell.child(SharedString::from(item.to_string())));
    }
    row
}

// ---------------------------------------------------------------------------
// Tree rows(章节 07 · 六态)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TreeState {
    Active,
    Selected,
    Hover,
    Default,
    Warning,
    Disabled,
}

pub struct TreeRowSpec<'a> {
    /// 0/1/2 → 缩进 11/24/40
    pub depth: usize,
    pub state: TreeState,
    /// Some(expanded) 渲染 ▾/▸;None 渲染 4px 方点
    pub expander: Option<bool>,
    pub label: &'a str,
    /// 右侧等宽计数
    pub trailing: &'a str,
    /// 计数颜色覆盖(默认 meta)
    pub trailing_color: Option<u32>,
}

/// 树行:行高 26,右侧计数等宽右对齐。
pub fn tree_row(spec: TreeRowSpec) -> Div {
    let indent = TREE_INDENT[spec.depth.min(2)];
    let mut row = div()
        .h(px(H_INPUT))
        .flex_none()
        .h_flex()
        .items_center()
        .gap(px(7.))
        .pr(px(11.))
        .pl(px(indent));

    row = match spec.state {
        TreeState::Active => row.bg(c(PANEL)).border_l_2().border_color(c(ACCENT)),
        TreeState::Selected => row.bg(c(SELECTED)).border_l_2().border_color(c(ACCENT)),
        TreeState::Hover => row.bg(c(HOVER)),
        _ => row,
    };

    let (label_color, weight_semibold) = match spec.state {
        TreeState::Active => (TEXT_PRIMARY, true),
        TreeState::Selected => (TEXT_PRIMARY, false),
        TreeState::Disabled => (TEXT_DISABLED, false),
        TreeState::Warning => (TEXT_SECONDARY, false),
        _ => (TEXT_SECONDARY, false),
    };

    // leading marker
    row = match spec.expander {
        Some(expanded) => row.child(
            div()
                .text_size(px(8.))
                .text_color(c(TEXT_META))
                .child(if expanded { "▾" } else { "▸" }),
        ),
        None => {
            let dot_color = match spec.state {
                TreeState::Active | TreeState::Selected => ACCENT,
                TreeState::Warning => WARN,
                TreeState::Disabled => DISABLED_DOT,
                _ => NEUTRAL_DOT,
            };
            row.child(div().size(px(4.)).flex_none().bg(c(dot_color)))
        }
    };

    let mut label = div()
        .text_size(fs(FS_12))
        .text_color(c(label_color))
        .whitespace_nowrap()
        .overflow_hidden()
        .child(SharedString::from(spec.label.to_string()));
    if weight_semibold {
        label = label.font_semibold();
    }
    row = row.child(label);

    row.child(
        div()
            .ml_auto()
            .font_family(FONT_MONO)
            .text_size(fs(FS_9_5))
            .text_color(c(spec.trailing_color.unwrap_or(TEXT_META)))
            .whitespace_nowrap()
            .child(SharedString::from(spec.trailing.to_string())),
    )
}

// ---------------------------------------------------------------------------
// Metric / summary rows(右栏 24px 指标行 · Dock 32px 摘要行)
// ---------------------------------------------------------------------------

/// 指标行:label(meta) + 右侧等宽值,h24,底部 soft 分隔。
pub fn metric_row(label: &str, value: &str, last: bool) -> Div {
    let mut row = div()
        .h(px(H_ROW))
        .flex_none()
        .h_flex()
        .items_center()
        .text_size(fs(FS_11_5));
    if !last {
        row = row.border_b_1().border_color(c(HAIRLINE_SOFT));
    }
    row.child(
        div()
            .text_color(c(TEXT_META))
            .child(SharedString::from(label.to_string())),
    )
    .child(
        div()
            .ml_auto()
            .font_family(FONT_MONO)
            .text_color(c(TEXT_PRIMARY))
            .child(SharedString::from(value.to_string())),
    )
}

// ---------------------------------------------------------------------------
// Status bar(24px)
// ---------------------------------------------------------------------------

pub struct StatusSegment<'a> {
    pub text: &'a str,
    pub color: Option<u32>,
}

/// 状态栏:rail 底,10px 等宽,段间 soft 竖线;最后一个右对齐段自动推到最右。
pub fn status_bar(segments: &[StatusSegment], trailing: Option<&str>) -> Div {
    let mut bar = div()
        .h(px(H_ROW))
        .flex_none()
        .h_flex()
        .items_center()
        .bg(c(RAIL))
        .border_t_1()
        .border_color(c(HAIRLINE))
        .font_family(FONT_MONO)
        .text_size(fs(FS_10))
        .text_color(c(TEXT_SECONDARY));
    for (i, seg) in segments.iter().enumerate() {
        let mut cell = div()
            .px(px(11.))
            .h_full()
            .flex()
            .items_center()
            .whitespace_nowrap();
        if i < segments.len() {
            cell = cell.border_r_1().border_color(c(HAIRLINE_SOFT));
        }
        if let Some(color) = seg.color {
            cell = cell.text_color(c(color));
        }
        bar = bar.child(cell.child(SharedString::from(seg.text.to_string())));
    }
    if let Some(t) = trailing {
        bar = bar.child(
            div()
                .ml_auto()
                .px(px(11.))
                .h_full()
                .flex()
                .items_center()
                .border_l_1()
                .border_color(c(HAIRLINE_SOFT))
                .text_color(c(TEXT_META))
                .whitespace_nowrap()
                .child(SharedString::from(t.to_string())),
        );
    }
    bar
}

// ---------------------------------------------------------------------------
// Log pane(右栏日志 · 10.5px 等宽 · 行高 1.5)
// ---------------------------------------------------------------------------

pub enum LogLine {
    /// 前缀行(meta 色):`band 01 · 29 ms`
    Meta(SharedString),
    /// 内容行(次级色,左缩进 8)
    Text(SharedString),
    /// 命中行(accent 色,左缩进 8)
    Match(SharedString),
    /// 命中 wash 块
    Hit(SharedString),
}

pub fn log_pane(lines: &[LogLine]) -> Div {
    let mut pane = div()
        .v_flex()
        .gap(px(5.))
        .p(px(9.))
        .px_3()
        .bg(c(PANEL))
        .font_family(FONT_MONO)
        .text_size(fs(FS_10_5))
        .line_height(px(FS_10_5 * 1.5));
    for line in lines {
        pane = match line {
            LogLine::Meta(t) => pane.child(div().text_color(c(TEXT_META)).child(t.clone())),
            LogLine::Text(t) => {
                pane.child(div().pl_2().text_color(c(TEXT_SECONDARY)).child(t.clone()))
            }
            LogLine::Match(t) => {
                pane.child(div().pl_2().text_color(c(ACCENT_TEXT)).child(t.clone()))
            }
            LogLine::Hit(t) => pane.child(accent_wash_line(t)),
        };
    }
    pane
}

/// helper: erase Div type for heterogeneous children lists.
pub fn el(d: Div) -> AnyElement {
    d.into_any_element()
}
