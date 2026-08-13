use crate::{PlatformError, PointI, RectI};

/// Observable phase of the region drag model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionPhase {
    Ready,
    Dragging,
    Selected,
    Cancelled,
}

/// Result of a pointer/key transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionUpdate {
    Changed,
    Ignored,
    TooSmall,
    Selected(RectI),
    Cancelled,
}

/// Deterministic virtual-desktop region-selection state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionSelectionState {
    desktop: RectI,
    minimum_extent: i32,
    phase: SelectionPhase,
    origin: Option<PointI>,
    current: Option<PointI>,
    selected: Option<RectI>,
}

impl RegionSelectionState {
    pub const DEFAULT_MINIMUM_EXTENT: i32 = 24;

    #[must_use]
    pub fn new(desktop: RectI) -> Self {
        Self::with_minimum_extent(desktop, Self::DEFAULT_MINIMUM_EXTENT)
            .expect("the stable minimum extent is positive")
    }

    pub fn with_minimum_extent(desktop: RectI, minimum_extent: i32) -> Option<Self> {
        (minimum_extent > 0).then_some(Self {
            desktop,
            minimum_extent,
            phase: SelectionPhase::Ready,
            origin: None,
            current: None,
            selected: None,
        })
    }

    #[must_use]
    pub const fn desktop(&self) -> RectI {
        self.desktop
    }

    #[must_use]
    pub const fn phase(&self) -> SelectionPhase {
        self.phase
    }

    #[must_use]
    pub const fn selected_region(&self) -> Option<RectI> {
        self.selected
    }

    #[must_use]
    pub fn preview_region(&self) -> Option<RectI> {
        match (self.origin, self.current) {
            (Some(origin), Some(current)) => RectI::from_corners(origin, current),
            _ => None,
        }
    }

    pub fn pointer_down(&mut self, screen_point: PointI) -> SelectionUpdate {
        if matches!(
            self.phase,
            SelectionPhase::Selected | SelectionPhase::Cancelled
        ) {
            return SelectionUpdate::Ignored;
        }
        let point = self.desktop.clamp_endpoint(screen_point);
        self.phase = SelectionPhase::Dragging;
        self.origin = Some(point);
        self.current = Some(point);
        self.selected = None;
        SelectionUpdate::Changed
    }

    pub fn pointer_move(&mut self, screen_point: PointI) -> SelectionUpdate {
        if self.phase != SelectionPhase::Dragging {
            return SelectionUpdate::Ignored;
        }
        self.current = Some(self.desktop.clamp_endpoint(screen_point));
        SelectionUpdate::Changed
    }

    pub fn pointer_up(&mut self, screen_point: PointI) -> SelectionUpdate {
        if self.phase != SelectionPhase::Dragging {
            return SelectionUpdate::Ignored;
        }
        self.current = Some(self.desktop.clamp_endpoint(screen_point));
        let candidate = self.preview_region();
        self.origin = None;
        self.current = None;
        match candidate {
            Some(region)
                if region.width >= self.minimum_extent && region.height >= self.minimum_extent =>
            {
                self.phase = SelectionPhase::Selected;
                self.selected = Some(region);
                SelectionUpdate::Selected(region)
            }
            _ => {
                self.phase = SelectionPhase::Ready;
                self.selected = None;
                SelectionUpdate::TooSmall
            }
        }
    }

    pub fn cancel(&mut self) -> SelectionUpdate {
        if self.phase == SelectionPhase::Selected {
            return SelectionUpdate::Ignored;
        }
        self.phase = SelectionPhase::Cancelled;
        self.origin = None;
        self.current = None;
        self.selected = None;
        SelectionUpdate::Cancelled
    }

    pub fn reset(&mut self) {
        self.phase = SelectionPhase::Ready;
        self.origin = None;
        self.current = None;
        self.selected = None;
    }
}

/// Native overlay presentation options. Geometry remains physical desktop pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionOverlayConfig {
    pub minimum_extent: i32,
    /// Opacity of the dark full-desktop shade (0-255).
    pub shade_alpha: u8,
}

impl Default for SelectionOverlayConfig {
    fn default() -> Self {
        Self {
            minimum_extent: RegionSelectionState::DEFAULT_MINIMUM_EXTENT,
            shade_alpha: 56,
        }
    }
}

/// Modal native region-selection overlay. `Esc` or closing the window returns `None`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RegionSelectionOverlay;

impl RegionSelectionOverlay {
    pub fn select(config: SelectionOverlayConfig) -> Result<Option<RectI>, PlatformError> {
        if config.minimum_extent <= 0 {
            return Err(PlatformError::Thread {
                operation: "configure region selection overlay",
                detail: "minimum extent must be positive".to_owned(),
            });
        }
        #[cfg(windows)]
        {
            crate::win32::select_region(config)
        }
        #[cfg(not(windows))]
        {
            crate::non_windows::select_region(config)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop() -> RectI {
        RectI::new(-1920, -200, 4480, 1640).unwrap()
    }

    #[test]
    fn reverse_drag_returns_physical_virtual_desktop_region() {
        let mut state = RegionSelectionState::new(desktop());
        assert_eq!(
            state.pointer_down(PointI::new(400, 500)),
            SelectionUpdate::Changed
        );
        state.pointer_move(PointI::new(-220, -80));
        assert_eq!(
            state.pointer_up(PointI::new(-220, -80)),
            SelectionUpdate::Selected(RectI::new(-220, -80, 620, 580).unwrap())
        );
        assert_eq!(state.phase(), SelectionPhase::Selected);
    }

    #[test]
    fn undersized_drag_resets_for_another_attempt() {
        let mut state = RegionSelectionState::new(desktop());
        state.pointer_down(PointI::new(10, 10));
        assert_eq!(
            state.pointer_up(PointI::new(33, 34)),
            SelectionUpdate::TooSmall
        );
        assert_eq!(state.phase(), SelectionPhase::Ready);
        state.pointer_down(PointI::new(10, 10));
        assert!(matches!(
            state.pointer_up(PointI::new(34, 34)),
            SelectionUpdate::Selected(_)
        ));
    }

    #[test]
    fn endpoints_are_clamped_to_the_overlay_desktop() {
        let mut state = RegionSelectionState::new(desktop());
        state.pointer_down(PointI::new(i32::MIN, i32::MIN));
        assert_eq!(
            state.pointer_up(PointI::new(i32::MAX, i32::MAX)),
            SelectionUpdate::Selected(desktop())
        );
    }

    #[test]
    fn escape_clears_a_partial_drag() {
        let mut state = RegionSelectionState::new(desktop());
        state.pointer_down(PointI::new(0, 0));
        assert_eq!(state.cancel(), SelectionUpdate::Cancelled);
        assert_eq!(state.preview_region(), None);
        assert_eq!(state.selected_region(), None);
    }
}
