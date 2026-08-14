//! Ledger v1 design tokens and gpui-component theme override.
//!
//! Single source of truth for every color, font size, spacing step and
//! control height in the GPUI frontend. Components must never introduce
//! values outside this module (Ledger hard constraint #2: colors only from
//! tokens; #new sizes forbidden).

// Phase 3 will consume the remaining tokens/components; keep the full set now.
#![allow(dead_code)]

use gpui::{App, Hsla, Pixels, Rgba, px};

use gpui_component::theme::{Theme, ThemeMode};

// ---------------------------------------------------------------------------
// Surfaces (章节 01)
// ---------------------------------------------------------------------------

/// canvas · 窗口底
pub const CANVAS: u32 = 0xF5F2EC;
/// panel · 内容面
pub const PANEL: u32 = 0xFBF8F2;
/// rail · 栏 / 表头 / 状态栏
pub const RAIL: u32 = 0xF1EDE4;
/// well · 输入位(唯一近白)
pub const WELL: u32 = 0xFFFDF9;
/// hover(只改底色)
pub const HOVER: u32 = 0xF1EDE4;
/// selected(配 2px 左边框)
pub const SELECTED: u32 = 0xEFE9DE;
/// pressed
pub const PRESSED: u32 = 0xE7E0D1;

/// hairline-soft · 行分隔
pub const HAIRLINE_SOFT: u32 = 0xE9E3D8;
/// hairline · 区块 / 输入
pub const HAIRLINE: u32 = 0xD8D0C2;
/// hairline-strong · 窗口 / 表外框
pub const HAIRLINE_STRONG: u32 = 0xCBC2B2;

// ---------------------------------------------------------------------------
// Text (章节 02 · 文字四级)
// ---------------------------------------------------------------------------

/// 主文字 · 值 · 活动项 (12.4:1)
pub const TEXT_PRIMARY: u32 = 0x1D1A15;
/// 次级 · 未选中项 · 正文 (6.6:1)
pub const TEXT_SECONDARY: u32 = 0x524C41;
/// 元数据 · label · 计数 · 日志前缀 (4.7:1)
pub const TEXT_META: u32 = 0x6F6759;
/// 禁用 · 占位 · 不可用 (2.6:1, 有意最低档)
pub const TEXT_DISABLED: u32 = 0xA79E8E;
/// 数值等宽文字用的深灰(设计稿数据色)
pub const TEXT_DATA: u32 = 0x33302A;

// ---------------------------------------------------------------------------
// Status (章节 04 · 三色互不重叠)
// ---------------------------------------------------------------------------

/// 墨青 · 正常运行(点 / Primary 底)
pub const ACCENT: u32 = 0x0E6A64;
/// 墨青文字
pub const ACCENT_TEXT: u32 = 0x0B534E;
/// 墨青底
pub const ACCENT_WASH: u32 = 0xE3EEEB;
/// 墨青线
pub const ACCENT_LINE: u32 = 0xA6C9C4;
/// Primary hover / pressed
pub const ACCENT_HOVER: u32 = 0x0B534E;
pub const ACCENT_PRESSED: u32 = 0x084642;
/// Primary 上的热键 chip 文字
pub const ACCENT_CHIP_TEXT: u32 = 0xEAF3F1;

/// 琥珀 · 需要注意
pub const WARN: u32 = 0x8A5A0B;
pub const WARN_WASH: u32 = 0xF6EEDD;
pub const WARN_TEXT: u32 = 0x6A4A0A;
pub const WARN_BAR: u32 = 0xA8801F;

/// 砖红 · 命中 / 错误
pub const DANGER: u32 = 0xA6382B;
pub const DANGER_TEXT: u32 = 0x8C2F24;
pub const DANGER_WASH: u32 = 0xF7E7E2;
pub const DANGER_LINE: u32 = 0xE0B4A9;

/// 中性状态点(ready / idle)
pub const NEUTRAL_DOT: u32 = 0xA79E8E;
/// disabled 圆点底
pub const DISABLED_DOT: u32 = 0xE7E0D1;

/// 文字反白(Primary/Destructive pressed 上的文字)= well
pub const ON_ACCENT: u32 = 0xFFFDF9;

// ---------------------------------------------------------------------------
// Typography (章节 02)
// ---------------------------------------------------------------------------

pub const FONT_UI: &str = "Microsoft YaHei UI";
/// 数据字族:Cascadia Mono 优先(Windows 自带),JetBrains Mono 兜底。
pub const FONT_MONO: &str = "Cascadia Mono";

/// 字号阶:10 · 10.5 · 11.5 · 12 · 12.5 · 13 · 15 · 20
pub const FS_10: f32 = 10.0;
pub const FS_10_5: f32 = 10.5;
pub const FS_11_5: f32 = 11.5;
pub const FS_12: f32 = 12.0;
pub const FS_12_5: f32 = 12.5;
pub const FS_13: f32 = 13.0;
pub const FS_15: f32 = 15.0;
pub const FS_20: f32 = 20.0;
/// 微标题专用 9.5/10(设计稿在窄栏用 9.5)
pub const FS_9_5: f32 = 9.5;

// ---------------------------------------------------------------------------
// Spacing / sizes (章节 03)
// ---------------------------------------------------------------------------

/// 间距阶(基数 4):4 6 8 10 12 16 18 24
pub const SP_4: f32 = 4.0;
pub const SP_6: f32 = 6.0;
pub const SP_8: f32 = 8.0;
pub const SP_10: f32 = 10.0;
pub const SP_12: f32 = 12.0;
pub const SP_16: f32 = 16.0;
pub const SP_18: f32 = 18.0;
pub const SP_24: f32 = 24.0;

/// label 列固定宽
pub const LABEL_COL: f32 = 96.0;

/// 控件高度八档(不得新增)
pub const H_CHIP: f32 = 22.0; // 标题栏 chip · tab 项
pub const H_ROW: f32 = 24.0; // 表格行 · 指标行 · 状态栏
pub const H_INPUT: f32 = 26.0; // 输入 · 下拉 · 树节点 · 小按钮
pub const H_BUTTON: f32 = 28.0; // 次级按钮 · 栏底动作
pub const H_TITLEBAR: f32 = 30.0; // 标题栏 · tab 条
pub const H_DOCK_ROW: f32 = 32.0; // Dock 摘要行
pub const H_DOCK_PRIMARY: f32 = 40.0; // Dock 主操作
pub const H_CONFIRM: f32 = 46.0; // 命中锁定卡确认键(唯一大控件)

/// 圆角:面板/表格/输入 = 0,按钮/chip = 2
pub const RADIUS_BUTTON: f32 = 2.0;

/// 树缩进:11 / 24 / 40
pub const TREE_INDENT: [f32; 3] = [11.0, 24.0, 40.0];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Hex → Hsla(gpui-component 主题需要 Hsla)。
pub fn hsla_of(hex: u32) -> Hsla {
    gpui::rgb(hex).into()
}

/// Convenience:hex → gpui color for direct styling.
pub fn c(hex: u32) -> Rgba {
    gpui::rgb(hex)
}

pub fn fs(v: f32) -> Pixels {
    px(v)
}

// ---------------------------------------------------------------------------
// gpui-component theme override
// ---------------------------------------------------------------------------

/// Apply the Ledger token set on top of gpui-component's light theme so every
/// stock component (Input, Dropdown, Checkbox, Switch, scrollbars, ...)
/// renders in Ledger colors. Call once at startup, before any window opens.
pub fn apply_ledger_theme(cx: &mut App) {
    let theme = Theme::global_mut(cx);

    theme.mode = ThemeMode::Light;
    theme.font_family = FONT_UI.into();
    theme.font_size = px(FS_12);
    theme.mono_font_family = FONT_MONO.into();
    theme.mono_font_size = px(FS_10_5);
    // 面板/输入 0 圆角;按钮/chip 的 2px 由组件层自己画。
    theme.radius = px(0.);
    theme.radius_lg = px(0.);
    theme.shadow = false;
    theme.tile_shadow = false;
    theme.tile_radius = px(0.);

    let colors = &mut theme.colors;

    // Surfaces
    colors.background = hsla_of(CANVAS);
    colors.foreground = hsla_of(TEXT_PRIMARY);
    colors.popover = hsla_of(WELL);
    colors.popover_foreground = hsla_of(TEXT_PRIMARY);
    colors.title_bar = hsla_of(RAIL);
    colors.title_bar_border = hsla_of(HAIRLINE);
    colors.window_border = hsla_of(HAIRLINE_STRONG);
    colors.tiles = hsla_of(CANVAS);
    colors.overlay = gpui::hsla(0., 0., 0., 0.06);

    // Muted / secondary text
    colors.muted = hsla_of(RAIL);
    colors.muted_foreground = hsla_of(TEXT_META);
    colors.secondary = hsla_of(PANEL);
    colors.secondary_foreground = hsla_of(TEXT_PRIMARY);
    colors.secondary_hover = hsla_of(HOVER);
    colors.secondary_active = hsla_of(PRESSED);

    // Borders / inputs
    colors.border = hsla_of(HAIRLINE);
    colors.input = hsla_of(HAIRLINE);
    colors.ring = hsla_of(ACCENT);
    colors.caret = hsla_of(ACCENT);
    colors.selection = hsla_of(ACCENT_WASH);

    // Primary(墨青)
    colors.primary = hsla_of(ACCENT);
    colors.primary_foreground = hsla_of(ON_ACCENT);
    colors.primary_hover = hsla_of(ACCENT_HOVER);
    colors.primary_active = hsla_of(ACCENT_PRESSED);
    colors.accent = hsla_of(SELECTED);
    colors.accent_foreground = hsla_of(TEXT_PRIMARY);

    // Danger(砖红)
    colors.danger = hsla_of(DANGER);
    colors.danger_foreground = hsla_of(ON_ACCENT);
    colors.danger_hover = hsla_of(DANGER_TEXT);
    colors.danger_active = hsla_of(DANGER_TEXT);

    // Warning(琥珀)
    colors.warning = hsla_of(WARN);
    colors.warning_foreground = hsla_of(ON_ACCENT);
    colors.warning_hover = hsla_of(WARN_TEXT);
    colors.warning_active = hsla_of(WARN_TEXT);

    // Success 映射到墨青(设计中"正常"就是墨青,不引入第四色)
    colors.success = hsla_of(ACCENT);
    colors.success_foreground = hsla_of(ON_ACCENT);
    colors.success_hover = hsla_of(ACCENT_HOVER);
    colors.success_active = hsla_of(ACCENT_PRESSED);

    // Info 同样收敛到墨青体系
    colors.info = hsla_of(ACCENT_WASH);
    colors.info_foreground = hsla_of(ACCENT_TEXT);
    colors.info_hover = hsla_of(ACCENT_LINE);
    colors.info_active = hsla_of(ACCENT_LINE);

    // List / tree
    colors.list = hsla_of(PANEL);
    colors.list_active = hsla_of(SELECTED);
    colors.list_active_border = hsla_of(ACCENT);
    colors.list_even = hsla_of(PANEL);
    colors.list_head = hsla_of(RAIL);
    colors.list_hover = hsla_of(HOVER);

    // Table(数值条件表)
    colors.table = hsla_of(WELL);
    colors.table_active = hsla_of(SELECTED);
    colors.table_active_border = hsla_of(ACCENT);
    colors.table_even = hsla_of(WELL);
    colors.table_head = hsla_of(RAIL);
    colors.table_head_foreground = hsla_of(TEXT_META);
    colors.table_hover = hsla_of(HOVER);
    colors.table_row_border = hsla_of(HAIRLINE_SOFT);

    // Tabs
    colors.tab = gpui::transparent_black();
    colors.tab_active = hsla_of(PANEL);
    colors.tab_active_foreground = hsla_of(TEXT_PRIMARY);
    colors.tab_bar = hsla_of(RAIL);
    colors.tab_bar_segmented = hsla_of(RAIL);
    colors.tab_foreground = hsla_of(TEXT_META);

    // Sidebar(左规则树栏)
    colors.sidebar = hsla_of(RAIL);
    colors.sidebar_accent = hsla_of(SELECTED);
    colors.sidebar_accent_foreground = hsla_of(TEXT_PRIMARY);
    colors.sidebar_border = hsla_of(HAIRLINE);
    colors.sidebar_foreground = hsla_of(TEXT_SECONDARY);
    colors.sidebar_primary = hsla_of(ACCENT);
    colors.sidebar_primary_foreground = hsla_of(ON_ACCENT);

    // Scrollbar:低存在感
    colors.scrollbar = gpui::transparent_black();
    colors.scrollbar_thumb = hsla_of(HAIRLINE);
    colors.scrollbar_thumb_hover = hsla_of(HAIRLINE_STRONG);

    // Misc
    colors.link = hsla_of(ACCENT_TEXT);
    colors.link_hover = hsla_of(ACCENT);
    colors.link_active = hsla_of(ACCENT_PRESSED);
    colors.drag_border = hsla_of(ACCENT);
    colors.drop_target = hsla_of(ACCENT_WASH);
    colors.progress_bar = hsla_of(ACCENT);
    colors.skeleton = hsla_of(RAIL);
    colors.switch = hsla_of(HAIRLINE);
    colors.switch_thumb = hsla_of(WELL);
    colors.slider_bar = hsla_of(ACCENT);
    colors.slider_thumb = hsla_of(WELL);
    colors.accordion = hsla_of(PANEL);
    colors.accordion_hover = hsla_of(HOVER);
    colors.group_box = hsla_of(PANEL);
    colors.group_box_foreground = hsla_of(TEXT_PRIMARY);
    colors.description_list_label = hsla_of(RAIL);
    colors.description_list_label_foreground = hsla_of(TEXT_META);
}
