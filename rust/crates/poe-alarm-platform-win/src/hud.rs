use crate::{NativeWindowHandle, PlatformError, PointI, RectI, SizeI};

const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;

/// Whether a status HUD is passive or temporarily accepts drag interaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HudInteractionMode {
    #[default]
    Passive,
    Placement,
}

/// Whether Windows capture APIs should include the HUD.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CaptureAffinity {
    #[default]
    Include,
    Exclude,
}

impl CaptureAffinity {
    #[must_use]
    pub const fn raw_value(self) -> u32 {
        match self {
            Self::Include => 0x0000_0000, // WDA_NONE
            Self::Exclude => 0x0000_0011, // WDA_EXCLUDEFROMCAPTURE
        }
    }
}

/// Composable native policy for the always-on-top status HUD.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HudWindowPolicy {
    pub interaction: HudInteractionMode,
    pub capture_affinity: CaptureAffinity,
}

impl Default for HudWindowPolicy {
    fn default() -> Self {
        Self {
            interaction: HudInteractionMode::Passive,
            capture_affinity: CaptureAffinity::Include,
        }
    }
}

impl HudWindowPolicy {
    /// Applies only the HUD-owned extended-style bits, preserving all others.
    #[must_use]
    pub const fn compose_extended_style(self, existing: u32) -> u32 {
        let common = existing | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
        match self.interaction {
            HudInteractionMode::Passive => common | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            HudInteractionMode::Placement => common & !WS_EX_TRANSPARENT & !WS_EX_NOACTIVATE,
        }
    }

    /// `SetWindowPos` flags used when showing/reasserting topmost placement.
    #[must_use]
    pub const fn show_position_flags(self) -> u32 {
        match self.interaction {
            HudInteractionMode::Passive => SWP_NOACTIVATE | SWP_SHOWWINDOW,
            HudInteractionMode::Placement => SWP_SHOWWINDOW,
        }
    }

    #[must_use]
    pub const fn is_click_through(self) -> bool {
        matches!(self.interaction, HudInteractionMode::Passive)
    }
}

/// Capture-region-aware persisted HUD placement.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum HudPlacement {
    #[default]
    Automatic,
    Manual {
        relative_x: f64,
        relative_y: f64,
    },
}

impl HudPlacement {
    pub fn manual(relative_x: f64, relative_y: f64) -> Option<Self> {
        (relative_x.is_finite()
            && relative_y.is_finite()
            && (0.0..=1.0).contains(&relative_x)
            && (0.0..=1.0).contains(&relative_y))
        .then_some(Self::Manual {
            relative_x,
            relative_y,
        })
    }
}

/// Resolves physical HUD coordinates using the stable ROI-avoidance order.
#[must_use]
pub fn resolve_hud_position(
    work_area: RectI,
    hud_size: SizeI,
    placement: HudPlacement,
    anchor_region: Option<RectI>,
) -> PointI {
    let available_width = (work_area.width - hud_size.width).max(0);
    let available_height = (work_area.height - hud_size.height).max(0);
    if let HudPlacement::Manual {
        relative_x,
        relative_y,
    } = placement
    {
        return PointI::new(
            work_area.x + (f64::from(available_width) * relative_x).round() as i32,
            work_area.y + (f64::from(available_height) * relative_y).round() as i32,
        );
    }

    const GAP: i32 = 12;
    const EDGE_MARGIN: i32 = 14;
    let min_x = work_area.x.saturating_add(EDGE_MARGIN);
    let min_y = work_area.y.saturating_add(EDGE_MARGIN);
    let max_x = min_x.max(
        work_area
            .right()
            .saturating_sub(hud_size.width)
            .saturating_sub(EDGE_MARGIN),
    );
    let max_y = min_y.max(
        work_area
            .bottom()
            .saturating_sub(hud_size.height)
            .saturating_sub(EDGE_MARGIN),
    );

    if let Some(anchor) = anchor_region {
        let candidates = [
            PointI::new(
                anchor.right().saturating_add(GAP),
                anchor.y.clamp(min_y, max_y),
            ),
            PointI::new(
                anchor.x.saturating_sub(hud_size.width).saturating_sub(GAP),
                anchor.y.clamp(min_y, max_y),
            ),
            PointI::new(
                anchor.x.clamp(min_x, max_x),
                anchor.y.saturating_sub(hud_size.height).saturating_sub(GAP),
            ),
            PointI::new(
                anchor.x.clamp(min_x, max_x),
                anchor.bottom().saturating_add(GAP),
            ),
        ];
        for candidate in candidates {
            if let Some(rectangle) =
                RectI::new(candidate.x, candidate.y, hud_size.width, hud_size.height)
                && contains_rect(work_area, rectangle)
                && !rectangle.intersects(anchor)
            {
                return candidate;
            }
        }
    }

    let corners = [
        PointI::new(max_x, min_y),
        PointI::new(max_x, max_y),
        PointI::new(min_x, min_y),
        PointI::new(min_x, max_y),
    ];
    let Some(anchor) = anchor_region else {
        return corners[0];
    };
    corners
        .into_iter()
        .find(|corner| {
            RectI::new(corner.x, corner.y, hud_size.width, hud_size.height)
                .is_some_and(|rectangle| !rectangle.intersects(anchor))
        })
        .unwrap_or(corners[0])
}

const fn contains_rect(outer: RectI, inner: RectI) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

/// HUD 卡片内容(Ledger 两态:冷灰未监控 / 墨青监控中)。
#[derive(Clone, Debug, Default)]
pub struct HudContent {
    pub monitoring: bool,
    pub status_text: String,
    pub elapsed: String,
    pub target: String,
}

/// Native HUD construction parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HudWindowConfig {
    pub bounds: RectI,
    pub policy: HudWindowPolicy,
    pub visible: bool,
}

/// Native topmost HUD shell with safe style and display-affinity transitions.
#[derive(Debug)]
pub struct HudWindow {
    #[cfg(windows)]
    native: crate::win32::NativeHudWindow,
    #[cfg(not(windows))]
    native: crate::non_windows::NativeHudWindow,
    policy: HudWindowPolicy,
}

impl HudWindow {
    pub fn create(config: HudWindowConfig) -> Result<Self, PlatformError> {
        let native = platform_create_hud(config)?;
        Ok(Self {
            native,
            policy: config.policy,
        })
    }

    #[must_use]
    pub const fn policy(&self) -> HudWindowPolicy {
        self.policy
    }

    #[must_use]
    pub fn window_handle(&self) -> NativeWindowHandle {
        self.native.window_handle()
    }

    pub fn set_interaction_mode(
        &mut self,
        interaction: HudInteractionMode,
    ) -> Result<(), PlatformError> {
        let mut policy = self.policy;
        policy.interaction = interaction;
        self.native.apply_policy(policy)?;
        self.policy = policy;
        Ok(())
    }

    pub fn set_capture_affinity(&mut self, affinity: CaptureAffinity) -> Result<(), PlatformError> {
        self.native.set_capture_affinity(affinity)?;
        self.policy.capture_affinity = affinity;
        Ok(())
    }

    pub fn set_bounds(&mut self, bounds: RectI) -> Result<(), PlatformError> {
        self.native.set_bounds(bounds, self.policy)
    }

    pub fn set_content(&mut self, content: HudContent) -> Result<(), PlatformError> {
        self.native.set_content(content)
    }

    pub fn show(&mut self) -> Result<(), PlatformError> {
        self.native.show(self.policy)
    }

    pub fn hide(&mut self) -> Result<(), PlatformError> {
        self.native.hide()
    }

    /// 取走最近一次用户拖动结束后的窗口左上角(屏幕坐标)。
    pub fn take_user_move(&mut self) -> Option<PointI> {
        self.native.take_user_move().map(|(x, y)| PointI::new(x, y))
    }
}

fn platform_create_hud(config: HudWindowConfig) -> Result<PlatformHudWindow, PlatformError> {
    #[cfg(windows)]
    {
        crate::win32::NativeHudWindow::create(config)
    }
    #[cfg(not(windows))]
    {
        crate::non_windows::NativeHudWindow::create(config)
    }
}

#[cfg(windows)]
type PlatformHudWindow = crate::win32::NativeHudWindow;
#[cfg(not(windows))]
type PlatformHudWindow = crate::non_windows::NativeHudWindow;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passive_policy_is_topmost_no_activate_and_click_through() {
        let policy = HudWindowPolicy::default();
        let style = policy.compose_extended_style(0x0008_0000);
        assert_eq!(style & WS_EX_TOPMOST, WS_EX_TOPMOST);
        assert_eq!(style & WS_EX_TOOLWINDOW, WS_EX_TOOLWINDOW);
        assert_eq!(style & WS_EX_NOACTIVATE, WS_EX_NOACTIVATE);
        assert_eq!(style & WS_EX_TRANSPARENT, WS_EX_TRANSPARENT);
        assert_eq!(style & 0x0008_0000, 0x0008_0000);
        assert_eq!(
            policy.show_position_flags(),
            SWP_NOACTIVATE | SWP_SHOWWINDOW
        );
    }

    #[test]
    fn placement_mode_removes_only_passive_bits() {
        let policy = HudWindowPolicy {
            interaction: HudInteractionMode::Placement,
            ..HudWindowPolicy::default()
        };
        let style = policy.compose_extended_style(WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | 0x400);
        assert_eq!(style & WS_EX_TRANSPARENT, 0);
        assert_eq!(style & WS_EX_NOACTIVATE, 0);
        assert_eq!(style & WS_EX_TOPMOST, WS_EX_TOPMOST);
        assert_eq!(style & WS_EX_TOOLWINDOW, WS_EX_TOOLWINDOW);
        assert_eq!(style & 0x400, 0x400);
    }

    #[test]
    fn automatic_position_prefers_right_of_capture_region() {
        let work = RectI::new(0, 0, 1920, 1040).unwrap();
        let anchor = RectI::new(500, 200, 400, 500).unwrap();
        let size = SizeI::new(336, 150).unwrap();
        assert_eq!(
            resolve_hud_position(work, size, HudPlacement::Automatic, Some(anchor)),
            PointI::new(912, 200)
        );
    }

    #[test]
    fn manual_position_uses_normalized_available_work_area() {
        let work = RectI::new(-1920, 0, 1920, 1040).unwrap();
        let size = SizeI::new(320, 140).unwrap();
        let placement = HudPlacement::manual(0.25, 1.0).unwrap();
        assert_eq!(
            resolve_hud_position(work, size, placement, None),
            PointI::new(-1520, 900)
        );
        assert_eq!(HudPlacement::manual(f64::NAN, 0.0), None);
    }
}
