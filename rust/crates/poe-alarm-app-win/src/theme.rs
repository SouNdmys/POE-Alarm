//! Platform-neutral visual tokens for the native application shell.
//!
//! The Win32 renderer owns brushes and fonts. This module only describes
//! semantic colours, interaction states, and DPI-scaled dimensions, which
//! keeps the visual language testable without creating a window.

/// The logical DPI used by all theme dimensions.
pub const BASE_DPI: u32 = 96;

/// An opaque sRGB colour.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// Returns the packed value expected by the Win32 `COLORREF` constructor.
    ///
    /// Keeping this as a plain `u32` avoids making the theme depend on Windows.
    pub const fn colorref(self) -> u32 {
        u32::from_le_bytes([self.red, self.green, self.blue, 0])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    pub canvas: Rgb,
    pub header: Rgb,
    pub card: Rgb,
    pub card_raised: Rgb,
    pub input: Rgb,
    pub hover: Rgb,
    pub pressed: Rgb,
    pub divider: Rgb,
    pub border: Rgb,
    pub border_emphasis: Rgb,
    pub text_primary: Rgb,
    pub text_secondary: Rgb,
    pub text_muted: Rgb,
    pub text_disabled: Rgb,
    pub accent: Rgb,
    pub accent_hover: Rgb,
    pub accent_pressed: Rgb,
    pub on_accent: Rgb,
    pub focus: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub danger: Rgb,
    pub info: Rgb,
}

/// A restrained graphite-and-warm-gold palette inspired by the game rather
/// than copying its highly decorated chrome.
pub const DARK_PALETTE: Palette = Palette {
    canvas: Rgb::new(17, 19, 22),
    header: Rgb::new(13, 15, 18),
    card: Rgb::new(27, 31, 36),
    card_raised: Rgb::new(34, 39, 45),
    input: Rgb::new(18, 21, 25),
    hover: Rgb::new(43, 49, 56),
    pressed: Rgb::new(22, 26, 31),
    divider: Rgb::new(48, 54, 62),
    border: Rgb::new(74, 81, 91),
    border_emphasis: Rgb::new(105, 113, 124),
    text_primary: Rgb::new(245, 241, 232),
    text_secondary: Rgb::new(202, 195, 181),
    text_muted: Rgb::new(166, 160, 150),
    text_disabled: Rgb::new(137, 139, 136),
    accent: Rgb::new(210, 169, 79),
    accent_hover: Rgb::new(228, 191, 105),
    accent_pressed: Rgb::new(174, 132, 54),
    on_accent: Rgb::new(22, 18, 11),
    focus: Rgb::new(241, 203, 120),
    success: Rgb::new(105, 210, 155),
    warning: Rgb::new(240, 196, 106),
    danger: Rgb::new(255, 140, 142),
    info: Rgb::new(121, 196, 242),
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusTone {
    Neutral,
    Success,
    Warning,
    Danger,
    Info,
}

impl Palette {
    pub const fn status_color(self, tone: StatusTone) -> Rgb {
        match tone {
            StatusTone::Neutral => self.text_secondary,
            StatusTone::Success => self.success,
            StatusTone::Warning => self.warning,
            StatusTone::Danger => self.danger,
            StatusTone::Info => self.info,
        }
    }

    pub const fn control_colors(self, role: ControlRole, state: ControlState) -> ControlColors {
        let disabled = ControlColors {
            background: self.pressed,
            border: self.divider,
            foreground: self.text_disabled,
            focus_ring: None,
        };
        if matches!(state, ControlState::Disabled) {
            return disabled;
        }

        let focus_ring = if matches!(state, ControlState::Focused) {
            Some(self.focus)
        } else {
            None
        };
        let (background, border, foreground) = match role {
            ControlRole::Primary => (
                match state {
                    ControlState::Hovered => self.accent_hover,
                    ControlState::Pressed => self.accent_pressed,
                    _ => self.accent,
                },
                self.accent,
                self.on_accent,
            ),
            ControlRole::Secondary => (
                match state {
                    ControlState::Hovered => self.hover,
                    ControlState::Pressed => self.pressed,
                    _ => self.card_raised,
                },
                self.border_emphasis,
                self.text_primary,
            ),
            ControlRole::Destructive => (
                match state {
                    ControlState::Hovered => Rgb::new(119, 52, 58),
                    ControlState::Pressed => Rgb::new(76, 35, 40),
                    _ => Rgb::new(96, 43, 48),
                },
                self.danger,
                self.text_primary,
            ),
            ControlRole::Input => (
                match state {
                    ControlState::Hovered => self.card_raised,
                    ControlState::Pressed => self.input,
                    _ => self.input,
                },
                if matches!(state, ControlState::Focused) {
                    self.focus
                } else {
                    self.border
                },
                self.text_primary,
            ),
            ControlRole::Quiet => (
                match state {
                    ControlState::Hovered => self.hover,
                    ControlState::Pressed => self.pressed,
                    _ => self.card,
                },
                if matches!(state, ControlState::Focused) {
                    self.focus
                } else {
                    self.card
                },
                self.text_secondary,
            ),
        };
        ControlColors {
            background,
            border,
            foreground,
            focus_ring,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlRole {
    Primary,
    Secondary,
    Destructive,
    Input,
    Quiet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlState {
    Normal,
    Hovered,
    Pressed,
    Disabled,
    Focused,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlColors {
    pub background: Rgb,
    pub border: Rgb,
    pub foreground: Rgb,
    pub focus_ring: Option<Rgb>,
}

/// Logical layout values. These deliberately remain compact enough for the
/// existing 1024x768 fit path while creating a consistent spacing rhythm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThemeMetrics {
    pub border: u32,
    pub focus_ring: u32,
    pub corner_radius: u32,
    pub space_xs: u32,
    pub space_sm: u32,
    pub space_md: u32,
    pub space_lg: u32,
    pub space_xl: u32,
    pub control_height: u32,
    pub button_height: u32,
    pub card_padding: u32,
    pub header_height: u32,
}

pub const LOGICAL_METRICS: ThemeMetrics = ThemeMetrics {
    border: 1,
    focus_ring: 2,
    corner_radius: 7,
    space_xs: 4,
    space_sm: 8,
    space_md: 12,
    space_lg: 18,
    space_xl: 24,
    control_height: 32,
    button_height: 36,
    card_padding: 18,
    header_height: 82,
};

impl ThemeMetrics {
    pub fn at_dpi(self, dpi: u32) -> Self {
        Self {
            border: scale_px(self.border, dpi).max(1),
            focus_ring: scale_px(self.focus_ring, dpi).max(1),
            corner_radius: scale_px(self.corner_radius, dpi),
            space_xs: scale_px(self.space_xs, dpi),
            space_sm: scale_px(self.space_sm, dpi),
            space_md: scale_px(self.space_md, dpi),
            space_lg: scale_px(self.space_lg, dpi),
            space_xl: scale_px(self.space_xl, dpi),
            control_height: scale_px(self.control_height, dpi),
            button_height: scale_px(self.button_height, dpi),
            card_padding: scale_px(self.card_padding, dpi),
            header_height: scale_px(self.header_height, dpi),
        }
    }
}

/// Scales a non-negative logical pixel value using Win32's nearest-pixel rule.
/// A zero DPI is treated as the standard 96 DPI rather than collapsing layout.
pub fn scale_px(logical_px: u32, dpi: u32) -> u32 {
    let dpi = if dpi == 0 { BASE_DPI } else { dpi };
    let scaled =
        (u64::from(logical_px) * u64::from(dpi) + u64::from(BASE_DPI / 2)) / u64::from(BASE_DPI);
    scaled.min(u64::from(u32::MAX)) as u32
}

/// WCAG relative luminance for an opaque sRGB colour.
pub fn relative_luminance(color: Rgb) -> f64 {
    fn linear(channel: u8) -> f64 {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linear(color.red) + 0.7152 * linear(color.green) + 0.0722 * linear(color.blue)
}

/// WCAG contrast ratio. Argument order does not matter.
pub fn contrast_ratio(first: Rgb, second: Rgb) -> f64 {
    let first = relative_luminance(first);
    let second = relative_luminance(second);
    let (lighter, darker) = if first >= second {
        (first, second)
    } else {
        (second, first)
    };
    (lighter + 0.05) / (darker + 0.05)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContrastTarget {
    NormalText,
    LargeText,
    UiComponent,
}

impl ContrastTarget {
    pub const fn minimum_ratio(self) -> f64 {
        match self {
            Self::NormalText => 4.5,
            Self::LargeText | Self::UiComponent => 3.0,
        }
    }
}

pub fn meets_contrast(foreground: Rgb, background: Rgb, target: ContrastTarget) -> bool {
    contrast_ratio(foreground, background) >= target.minimum_ratio()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_uses_windows_byte_order_without_windows_types() {
        assert_eq!(Rgb::new(0x12, 0x34, 0x56).colorref(), 0x0056_3412);
    }

    #[test]
    fn common_dpi_values_scale_deterministically() {
        assert_eq!(scale_px(100, 0), 100);
        assert_eq!(scale_px(100, 96), 100);
        assert_eq!(scale_px(100, 120), 125);
        assert_eq!(scale_px(100, 144), 150);
        assert_eq!(scale_px(100, 192), 200);
        assert_eq!(LOGICAL_METRICS.at_dpi(144).button_height, 54);
    }

    #[test]
    fn core_text_hierarchy_meets_normal_text_contrast() {
        for foreground in [
            DARK_PALETTE.text_primary,
            DARK_PALETTE.text_secondary,
            DARK_PALETTE.text_muted,
        ] {
            assert!(meets_contrast(
                foreground,
                DARK_PALETTE.card,
                ContrastTarget::NormalText
            ));
        }
    }

    #[test]
    fn primary_control_and_status_colours_remain_readable() {
        for state in [
            ControlState::Normal,
            ControlState::Hovered,
            ControlState::Pressed,
            ControlState::Focused,
        ] {
            let colors = DARK_PALETTE.control_colors(ControlRole::Primary, state);
            assert!(meets_contrast(
                colors.foreground,
                colors.background,
                ContrastTarget::NormalText
            ));
        }
        for tone in [
            StatusTone::Success,
            StatusTone::Warning,
            StatusTone::Danger,
            StatusTone::Info,
        ] {
            assert!(meets_contrast(
                DARK_PALETTE.status_color(tone),
                DARK_PALETTE.card,
                ContrastTarget::NormalText
            ));
        }
    }

    #[test]
    fn focused_controls_have_a_distinct_accessible_ring() {
        for role in [
            ControlRole::Primary,
            ControlRole::Secondary,
            ControlRole::Destructive,
            ControlRole::Input,
            ControlRole::Quiet,
        ] {
            let colors = DARK_PALETTE.control_colors(role, ControlState::Focused);
            assert_eq!(colors.focus_ring, Some(DARK_PALETTE.focus));
            assert!(meets_contrast(
                DARK_PALETTE.focus,
                DARK_PALETTE.canvas,
                ContrastTarget::UiComponent
            ));
        }
    }
}
