/// A point in physical virtual-desktop pixels.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PointI {
    pub x: i32,
    pub y: i32,
}

impl PointI {
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// A strictly positive size in physical pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SizeI {
    pub width: i32,
    pub height: i32,
}

impl SizeI {
    #[must_use]
    pub const fn new(width: i32, height: i32) -> Option<Self> {
        if width > 0 && height > 0 {
            Some(Self { width, height })
        } else {
            None
        }
    }
}

/// A non-overflowing rectangle in physical virtual-desktop pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl RectI {
    #[must_use]
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        if x.checked_add(width).is_none() || y.checked_add(height).is_none() {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }

    #[must_use]
    pub fn from_corners(first: PointI, second: PointI) -> Option<Self> {
        let left = i64::from(first.x.min(second.x));
        let top = i64::from(first.y.min(second.y));
        let width = (i64::from(first.x) - i64::from(second.x)).abs();
        let height = (i64::from(first.y) - i64::from(second.y)).abs();
        let left = i32::try_from(left).ok()?;
        let top = i32::try_from(top).ok()?;
        let width = i32::try_from(width).ok()?;
        let height = i32::try_from(height).ok()?;
        Self::new(left, top, width, height)
    }

    #[must_use]
    pub const fn right(self) -> i32 {
        // Construction proves this addition cannot overflow.
        self.x + self.width
    }

    #[must_use]
    pub const fn bottom(self) -> i32 {
        // Construction proves this addition cannot overflow.
        self.y + self.height
    }

    #[must_use]
    pub const fn size(self) -> SizeI {
        SizeI {
            width: self.width,
            height: self.height,
        }
    }

    #[must_use]
    pub const fn contains(self, point: PointI) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }

    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }

    /// Clamps a drag endpoint to the inclusive desktop boundary.
    #[must_use]
    pub fn clamp_endpoint(self, point: PointI) -> PointI {
        PointI {
            x: point.x.clamp(self.x, self.right()),
            y: point.y.clamp(self.y, self.bottom()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_corners_preserve_negative_desktop_coordinates() {
        assert_eq!(
            RectI::from_corners(PointI::new(200, 140), PointI::new(-300, -60)),
            RectI::new(-300, -60, 500, 200)
        );
    }

    #[test]
    fn overflowing_rectangles_are_rejected() {
        assert_eq!(RectI::new(i32::MAX, 0, 1, 1), None);
        assert_eq!(RectI::new(0, i32::MAX, 1, 1), None);
        assert_eq!(
            RectI::from_corners(PointI::new(i32::MIN, 0), PointI::new(i32::MAX, 1)),
            None
        );
    }
}
