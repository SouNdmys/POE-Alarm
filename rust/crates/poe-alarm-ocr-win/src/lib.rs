//! Process-lifetime Windows OCR worker for POE Alarm.
//!
//! The adapter accepts the top-down BGRA contract from `poe-alarm-vision`,
//! selects an installed Windows OCR language deterministically, and reports
//! end-to-end adapter latency with the recognized physical lines. All WinRT
//! objects are private to one persistent MTA worker because rebuilding WinRT
//! apartments around the `windows` crate's cached activation factories can
//! invalidate native pointers on supported Windows versions.

#![forbid(unsafe_op_in_unsafe_fn)]

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use poe_alarm_vision::{CapturedFrame, CroppedMask};

#[cfg(not(windows))]
mod non_windows;
#[cfg(windows)]
mod windows_ocr;
mod worker;

pub use worker::{OcrPending, OcrWorker};

/// A stable process-lifetime Windows OCR engine lane. POE2 confirmation uses a distinct engine
/// object while sharing the same permanent MTA actor and COM apartment as the primary path.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum OcrLane {
    #[default]
    Default,
    Poe2Confirmation,
}

/// Preferred Windows OCR recognizer family.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum OcrLanguagePreference {
    English,
    TraditionalChinese,
    /// Requires an installed recognizer with this exact BCP-47 tag.
    Exact(String),
}

/// Owned, validated top-down BGRA8 input safe to queue to the OCR worker.
#[derive(Clone, Debug)]
pub struct OwnedBgraImage {
    width: usize,
    height: usize,
    stride: usize,
    pixels: Arc<[u8]>,
    format: OcrPixelFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OcrPixelFormat {
    Gray8,
    Bgra8,
}

impl OwnedBgraImage {
    pub fn new(
        width: usize,
        height: usize,
        stride: usize,
        pixels: Vec<u8>,
    ) -> Result<Self, OcrError> {
        validate_bgra_layout(width, height, stride, pixels.len())?;
        Ok(Self {
            width,
            height,
            stride,
            pixels: pixels.into(),
            format: OcrPixelFormat::Bgra8,
        })
    }

    /// Constructs a packed or row-padded Gray8 image. Blue-mask callers should prefer this path
    /// so Windows OCR does not receive a redundant four-channel expansion of grayscale data.
    pub fn gray8(
        width: usize,
        height: usize,
        stride: usize,
        pixels: Vec<u8>,
    ) -> Result<Self, OcrError> {
        validate_gray_layout(width, height, stride, pixels.len())?;
        Ok(Self {
            width,
            height,
            stride,
            pixels: pixels.into(),
            format: OcrPixelFormat::Gray8,
        })
    }

    pub fn from_frame(frame: &CapturedFrame) -> Self {
        Self {
            width: frame.width(),
            height: frame.height(),
            stride: frame.stride(),
            pixels: Arc::from(frame.bgra_pixels()),
            format: OcrPixelFormat::Bgra8,
        }
    }

    pub fn from_crop(crop: &CroppedMask) -> Self {
        Self {
            width: crop.width,
            height: crop.height,
            stride: crop.stride,
            pixels: Arc::from(crop.bgra_pixels.as_slice()),
            format: OcrPixelFormat::Bgra8,
        }
    }

    pub fn as_bgra(&self) -> BgraImage<'_> {
        assert_eq!(
            self.format,
            OcrPixelFormat::Bgra8,
            "Gray8 OCR images cannot be viewed as BGRA"
        );
        BgraImage {
            width: self.width,
            height: self.height,
            stride: self.stride,
            pixels: &self.pixels,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn format(&self) -> OcrPixelFormat {
        self.format
    }
}

impl OcrLanguagePreference {
    fn requested_label(&self) -> &str {
        match self {
            Self::English => "English",
            Self::TraditionalChinese => "Traditional Chinese",
            Self::Exact(tag) => tag,
        }
    }
}

/// Borrowed, top-down BGRA8 pixels. Padding at the end of each row is allowed.
#[derive(Clone, Copy, Debug)]
pub struct BgraImage<'a> {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub pixels: &'a [u8],
}

impl<'a> BgraImage<'a> {
    pub fn new(
        width: usize,
        height: usize,
        stride: usize,
        pixels: &'a [u8],
    ) -> Result<Self, OcrError> {
        validate_bgra_layout(width, height, stride, pixels.len())?;
        Ok(Self {
            width,
            height,
            stride,
            pixels,
        })
    }

    pub fn from_frame(frame: &'a CapturedFrame) -> Self {
        Self {
            width: frame.width(),
            height: frame.height(),
            stride: frame.stride(),
            pixels: frame.bgra_pixels(),
        }
    }

    pub fn from_crop(crop: &'a CroppedMask) -> Self {
        Self {
            width: crop.width,
            height: crop.height,
            stride: crop.stride,
            pixels: &crop.bgra_pixels,
        }
    }

    pub fn packed_stride(self) -> usize {
        self.width * 4
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecognizedLine {
    pub text: String,
    /// Aggregate Windows OCR word bounds. Keeping only the aggregate avoids retaining and
    /// allocating one Rust object for every word when all downstream layout logic needs is the
    /// physical line rectangle.
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OcrRecognition {
    pub language_tag: String,
    /// Includes stride packing, WinRT bitmap creation, OCR, and result materialization.
    pub elapsed: Duration,
    pub lines: Vec<RecognizedLine>,
}

impl OcrRecognition {
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OcrCapabilities {
    pub available_language_tags: Vec<String>,
    pub maximum_image_dimension: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OcrError {
    PlatformUnavailable,
    EmptyImage,
    DimensionsTooLarge,
    StrideTooSmall {
        minimum: usize,
        actual: usize,
    },
    BufferLength {
        expected: usize,
        actual: usize,
    },
    ImageExceedsOcrLimit {
        width: usize,
        height: usize,
        maximum: u32,
    },
    LanguageUnavailable {
        requested: String,
        available: Vec<String>,
    },
    WorkerStart {
        message: String,
    },
    WorkerUnavailable,
    WorkerQueueFull,
    Cancelled,
    WinRt {
        operation: &'static str,
        hresult: i32,
        message: String,
    },
}

impl fmt::Display for OcrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlatformUnavailable => {
                formatter.write_str("Windows.Media.Ocr is only available on Windows")
            }
            Self::EmptyImage => formatter.write_str("OCR image dimensions must be positive"),
            Self::DimensionsTooLarge => formatter.write_str("OCR image dimensions overflow"),
            Self::StrideTooSmall { minimum, actual } => write!(
                formatter,
                "BGRA stride is too small: expected at least {minimum}, got {actual}"
            ),
            Self::BufferLength { expected, actual } => write!(
                formatter,
                "BGRA buffer length mismatch: expected {expected}, got {actual}"
            ),
            Self::ImageExceedsOcrLimit {
                width,
                height,
                maximum,
            } => write!(
                formatter,
                "OCR image {width}x{height} exceeds Windows OCR limit {maximum}"
            ),
            Self::LanguageUnavailable {
                requested,
                available,
            } => write!(
                formatter,
                "no installed Windows OCR recognizer matches {requested}; available: {}",
                if available.is_empty() {
                    "none".to_owned()
                } else {
                    available.join(", ")
                }
            ),
            Self::WorkerStart { message } => {
                write!(formatter, "could not start Windows OCR worker: {message}")
            }
            Self::WorkerUnavailable => {
                formatter.write_str("the process-lifetime Windows OCR worker is unavailable")
            }
            Self::WorkerQueueFull => {
                formatter.write_str("the bounded Windows OCR worker queue is full")
            }
            Self::Cancelled => formatter.write_str("Windows OCR request was cancelled"),
            Self::WinRt {
                operation,
                hresult,
                message,
            } => write!(
                formatter,
                "{operation} failed with HRESULT 0x{:08X}: {message}",
                *hresult as u32
            ),
        }
    }
}

impl std::error::Error for OcrError {}

fn validate_bgra_layout(
    width: usize,
    height: usize,
    stride: usize,
    buffer_length: usize,
) -> Result<(), OcrError> {
    if width == 0 || height == 0 {
        return Err(OcrError::EmptyImage);
    }
    let minimum_stride = width.checked_mul(4).ok_or(OcrError::DimensionsTooLarge)?;
    if stride < minimum_stride {
        return Err(OcrError::StrideTooSmall {
            minimum: minimum_stride,
            actual: stride,
        });
    }
    let expected = stride
        .checked_mul(height)
        .ok_or(OcrError::DimensionsTooLarge)?;
    if buffer_length != expected {
        return Err(OcrError::BufferLength {
            expected,
            actual: buffer_length,
        });
    }
    i32::try_from(width).map_err(|_| OcrError::DimensionsTooLarge)?;
    i32::try_from(height).map_err(|_| OcrError::DimensionsTooLarge)?;
    u32::try_from(
        minimum_stride
            .checked_mul(height)
            .ok_or(OcrError::DimensionsTooLarge)?,
    )
    .map_err(|_| OcrError::DimensionsTooLarge)?;
    Ok(())
}

fn validate_gray_layout(
    width: usize,
    height: usize,
    stride: usize,
    buffer_length: usize,
) -> Result<(), OcrError> {
    if width == 0 || height == 0 {
        return Err(OcrError::EmptyImage);
    }
    if stride < width {
        return Err(OcrError::StrideTooSmall {
            minimum: width,
            actual: stride,
        });
    }
    let expected = stride
        .checked_mul(height)
        .ok_or(OcrError::DimensionsTooLarge)?;
    if buffer_length != expected {
        return Err(OcrError::BufferLength {
            expected,
            actual: buffer_length,
        });
    }
    i32::try_from(width).map_err(|_| OcrError::DimensionsTooLarge)?;
    i32::try_from(height).map_err(|_| OcrError::DimensionsTooLarge)?;
    u32::try_from(
        width
            .checked_mul(height)
            .ok_or(OcrError::DimensionsTooLarge)?,
    )
    .map_err(|_| OcrError::DimensionsTooLarge)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_contract_accepts_padding_and_rejects_invalid_layouts() {
        let padded = vec![0; 24];
        let image = BgraImage::new(2, 2, 12, &padded).unwrap();
        assert_eq!(image.packed_stride(), 8);

        assert!(matches!(
            BgraImage::new(2, 2, 7, &[0; 14]),
            Err(OcrError::StrideTooSmall { .. })
        ));
        assert!(matches!(
            BgraImage::new(2, 2, 8, &[0; 15]),
            Err(OcrError::BufferLength { .. })
        ));
    }

    #[test]
    fn owned_bgra_clone_reuses_the_same_pixel_allocation() {
        let image = OwnedBgraImage::new(2, 2, 8, vec![0; 16]).unwrap();
        let clone = image.clone();
        assert!(std::ptr::eq(
            image.pixels().as_ptr(),
            clone.pixels().as_ptr()
        ));
    }

    #[test]
    fn owned_gray8_accepts_padding_and_preserves_format() {
        let image = OwnedBgraImage::gray8(2, 2, 3, vec![0; 6]).unwrap();
        assert_eq!(image.format(), OcrPixelFormat::Gray8);
        assert_eq!(image.stride(), 3);
        assert!(matches!(
            OwnedBgraImage::gray8(2, 2, 1, vec![0; 2]),
            Err(OcrError::StrideTooSmall { .. })
        ));
    }

    #[test]
    fn recognition_text_preserves_physical_line_boundaries() {
        let result = OcrRecognition {
            language_tag: "en-US".to_owned(),
            elapsed: Duration::ZERO,
            lines: vec![
                RecognizedLine {
                    text: "first".to_owned(),
                    left: 0.0,
                    top: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
                RecognizedLine {
                    text: "second".to_owned(),
                    left: 0.0,
                    top: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
            ],
        };
        assert_eq!(result.text(), "first\nsecond");
    }
}
