use std::fmt;
use std::time::SystemTime;

/// POE Alarm capture buffers use top-down BGRA pixels.
pub const BYTES_PER_PIXEL: usize = 4;

/// A desktop-space capture rectangle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CaptureRegion {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl CaptureRegion {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Result<Self, FrameError> {
        if width == 0 || height == 0 {
            return Err(FrameError::EmptyRegion);
        }

        let width_offset = i32::try_from(width - 1).map_err(|_| FrameError::DimensionsTooLarge)?;
        let height_offset =
            i32::try_from(height - 1).map_err(|_| FrameError::DimensionsTooLarge)?;
        x.checked_add(width_offset)
            .ok_or(FrameError::DimensionsTooLarge)?;
        y.checked_add(height_offset)
            .ok_or(FrameError::DimensionsTooLarge)?;

        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub(crate) const fn empty() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }

    pub fn subregion(self, rect: PixelRect) -> Result<Self, FrameError> {
        rect.validate_within(self.width as usize, self.height as usize)?;
        let x_offset = i32::try_from(rect.x).map_err(|_| FrameError::DimensionsTooLarge)?;
        let y_offset = i32::try_from(rect.y).map_err(|_| FrameError::DimensionsTooLarge)?;
        Self::new(
            self.x
                .checked_add(x_offset)
                .ok_or(FrameError::DimensionsTooLarge)?,
            self.y
                .checked_add(y_offset)
                .ok_or(FrameError::DimensionsTooLarge)?,
            u32::try_from(rect.width).map_err(|_| FrameError::DimensionsTooLarge)?,
            u32::try_from(rect.height).map_err(|_| FrameError::DimensionsTooLarge)?,
        )
    }
}

/// A half-open rectangle relative to a captured frame or mask.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PixelRect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl PixelRect {
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Result<Self, FrameError> {
        if width == 0 || height == 0 {
            return Err(FrameError::EmptyRegion);
        }
        x.checked_add(width)
            .and_then(|_| y.checked_add(height))
            .ok_or(FrameError::DimensionsTooLarge)?;
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn right_exclusive(self) -> usize {
        self.x + self.width
    }

    pub fn bottom_exclusive(self) -> usize {
        self.y + self.height
    }

    pub fn right_inclusive(self) -> usize {
        self.right_exclusive() - 1
    }

    pub fn bottom_inclusive(self) -> usize {
        self.bottom_exclusive() - 1
    }

    pub fn validate_within(self, width: usize, height: usize) -> Result<(), FrameError> {
        if self.width == 0 || self.height == 0 {
            return Err(FrameError::EmptyRegion);
        }
        let right = self
            .x
            .checked_add(self.width)
            .ok_or(FrameError::DimensionsTooLarge)?;
        let bottom = self
            .y
            .checked_add(self.height)
            .ok_or(FrameError::DimensionsTooLarge)?;
        if right > width || bottom > height {
            return Err(FrameError::RegionOutOfBounds {
                frame_width: width,
                frame_height: height,
                rect: self,
            });
        }
        Ok(())
    }
}

/// One top-down BGRA desktop capture.
///
/// `CapturedFrame::default()` is an intentionally empty destination for
/// [`crate::ScreenCapture::capture_into`]. Normal consumers should construct a
/// validated frame with [`CapturedFrame::from_bgra`].
#[derive(Clone, Debug)]
pub struct CapturedFrame {
    region: CaptureRegion,
    stride: usize,
    bgra_pixels: Vec<u8>,
    captured_at: SystemTime,
}

impl CapturedFrame {
    pub fn from_bgra(
        region: CaptureRegion,
        stride: usize,
        bgra_pixels: Vec<u8>,
        captured_at: SystemTime,
    ) -> Result<Self, FrameError> {
        validate_layout(region, stride, bgra_pixels.len())?;
        Ok(Self {
            region,
            stride,
            bgra_pixels,
            captured_at,
        })
    }

    pub fn region(&self) -> CaptureRegion {
        self.region
    }

    pub fn width(&self) -> usize {
        self.region.width as usize
    }

    pub fn height(&self) -> usize {
        self.region.height as usize
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn bgra_pixels(&self) -> &[u8] {
        &self.bgra_pixels
    }

    pub fn captured_at(&self) -> SystemTime {
        self.captured_at
    }

    pub(crate) fn prepare_for_capture(
        &mut self,
        region: CaptureRegion,
        captured_at: SystemTime,
    ) -> Result<&mut [u8], FrameError> {
        let stride = (region.width as usize)
            .checked_mul(BYTES_PER_PIXEL)
            .ok_or(FrameError::DimensionsTooLarge)?;
        let length = stride
            .checked_mul(region.height as usize)
            .ok_or(FrameError::DimensionsTooLarge)?;
        self.region = region;
        self.stride = stride;
        self.captured_at = captured_at;
        self.bgra_pixels.resize(length, 0);
        Ok(&mut self.bgra_pixels)
    }
}

impl Default for CapturedFrame {
    fn default() -> Self {
        Self {
            region: CaptureRegion::empty(),
            stride: 0,
            bgra_pixels: Vec::new(),
            captured_at: SystemTime::UNIX_EPOCH,
        }
    }
}

fn validate_layout(
    region: CaptureRegion,
    stride: usize,
    buffer_length: usize,
) -> Result<(), FrameError> {
    if region.width == 0 || region.height == 0 {
        return Err(FrameError::EmptyRegion);
    }
    let minimum_stride = (region.width as usize)
        .checked_mul(BYTES_PER_PIXEL)
        .ok_or(FrameError::DimensionsTooLarge)?;
    if stride < minimum_stride {
        return Err(FrameError::StrideTooSmall {
            minimum: minimum_stride,
            actual: stride,
        });
    }
    let expected = stride
        .checked_mul(region.height as usize)
        .ok_or(FrameError::DimensionsTooLarge)?;
    if buffer_length != expected {
        return Err(FrameError::BufferLength {
            expected,
            actual: buffer_length,
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    EmptyRegion,
    DimensionsTooLarge,
    StrideTooSmall {
        minimum: usize,
        actual: usize,
    },
    BufferLength {
        expected: usize,
        actual: usize,
    },
    RegionOutOfBounds {
        frame_width: usize,
        frame_height: usize,
        rect: PixelRect,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRegion => formatter.write_str("capture region must have a positive size"),
            Self::DimensionsTooLarge => formatter.write_str("capture dimensions overflow"),
            Self::StrideTooSmall { minimum, actual } => write!(
                formatter,
                "BGRA stride is too small: expected at least {minimum}, got {actual}"
            ),
            Self::BufferLength { expected, actual } => write!(
                formatter,
                "BGRA buffer length mismatch: expected {expected}, got {actual}"
            ),
            Self::RegionOutOfBounds {
                frame_width,
                frame_height,
                rect,
            } => write!(
                formatter,
                "pixel rectangle {rect:?} is outside {frame_width}x{frame_height}"
            ),
        }
    }
}

impl std::error::Error for FrameError {}
