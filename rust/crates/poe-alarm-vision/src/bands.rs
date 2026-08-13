use std::fmt;

use crate::{
    BYTES_PER_PIXEL, BlueTextMask, CaptureRegion, FNV1A_64_OFFSET_BASIS, FNV1A_64_PRIME,
    FrameError, PixelRect,
};

/// Defaults mirror the proven .NET POE blue-affix preprocessing path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BandDetectionSettings {
    pub minimum_ink_per_row: usize,
    pub tolerated_blank_rows: usize,
    pub minimum_band_height: usize,
    /// Inclusive ink span (`right - left`) required for a text-like row.
    pub minimum_horizontal_span: usize,
    pub horizontal_margin: usize,
    pub vertical_margin: usize,
    pub oversized_band_height: usize,
    pub maximum_oversized_band_height: usize,
}

impl Default for BandDetectionSettings {
    fn default() -> Self {
        Self {
            minimum_ink_per_row: 3,
            tolerated_blank_rows: 2,
            minimum_band_height: 5,
            minimum_horizontal_span: 24,
            horizontal_margin: 10,
            vertical_margin: 14,
            oversized_band_height: 48,
            maximum_oversized_band_height: 96,
        }
    }
}

impl BandDetectionSettings {
    fn validate(self) -> Result<(), BandDetectionError> {
        if self.minimum_ink_per_row == 0 {
            return Err(BandDetectionError::InvalidSettings(
                "minimum_ink_per_row must be positive",
            ));
        }
        if self.minimum_band_height == 0 {
            return Err(BandDetectionError::InvalidSettings(
                "minimum_band_height must be positive",
            ));
        }
        if self.maximum_oversized_band_height < self.oversized_band_height {
            return Err(BandDetectionError::InvalidSettings(
                "maximum oversized height must not be smaller than oversized height",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PhysicalBandIdentity {
    pub content_fingerprint: u64,
    pub source_top: usize,
    pub source_bottom: usize,
    pub source_left: usize,
    pub source_right: usize,
}

impl fmt::Display for PhysicalBandIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "vision:{:016x}:{}:{}:{}:{}",
            self.content_fingerprint,
            self.source_top,
            self.source_bottom,
            self.source_left,
            self.source_right
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CropMetadata {
    /// Half-open crop relative to the captured ROI.
    pub source_rect: PixelRect,
    /// The same crop in desktop coordinates.
    pub desktop_region: CaptureRegion,
    /// Tight horizontal ink bounds and logical row bounds, relative to the ROI.
    pub content_rect: PixelRect,
    /// Inclusive logical row bounds relative to the crop.
    pub logical_content_top: usize,
    pub logical_content_bottom: usize,
    pub content_fingerprint: u64,
    pub is_fallback: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalLineBand {
    pub index: usize,
    pub identity: PhysicalBandIdentity,
    pub ink_pixels: usize,
    pub crop: CropMetadata,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BandDetection {
    pub bands: Vec<PhysicalLineBand>,
    /// Present when connected blue UI art produced an oversized candidate.
    pub fallback_crop: Option<CropMetadata>,
}

#[derive(Default)]
pub struct PhysicalBandDetector {
    row_ink: Vec<usize>,
    candidates: Vec<(usize, usize)>,
}

impl PhysicalBandDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Detects physical glyph rows while reusing row-projection scratch buffers.
    pub fn detect(
        &mut self,
        mask: &BlueTextMask,
        settings: BandDetectionSettings,
    ) -> Result<BandDetection, BandDetectionError> {
        settings.validate()?;
        let source_region = mask.source_region().ok_or(BandDetectionError::EmptyMask)?;
        let width = mask.width();
        let height = mask.height();
        if width == 0 || height == 0 {
            return Err(BandDetectionError::EmptyMask);
        }

        self.row_ink.resize(height, 0);
        self.row_ink.fill(0);
        for (y, row) in mask.intensities().chunks_exact(width).enumerate() {
            self.row_ink[y] = row.iter().filter(|intensity| **intensity != 0).count();
        }

        self.candidates.clear();
        let mut start = None;
        let mut last_ink = 0;
        for (y, ink) in self.row_ink.iter().copied().enumerate() {
            if ink >= settings.minimum_ink_per_row {
                start.get_or_insert(y);
                last_ink = y;
                continue;
            }
            if let Some(first) = start
                && y - last_ink > settings.tolerated_blank_rows
            {
                if last_ink - first + 1 >= settings.minimum_band_height {
                    self.candidates.push((first, last_ink));
                }
                start = None;
            }
        }
        if let Some(first) = start
            && last_ink - first + 1 >= settings.minimum_band_height
        {
            self.candidates.push((first, last_ink));
        }

        let mut result = BandDetection::default();
        let mut fallback_required = false;
        for &(top, bottom) in &self.candidates {
            let band_height = bottom - top + 1;
            if band_height > settings.oversized_band_height {
                fallback_required = true;
                if band_height > settings.maximum_oversized_band_height {
                    continue;
                }
            }

            let mut left = width;
            let mut right = None;
            let mut ink_pixels = 0;
            for y in top..=bottom {
                let row = &mask.intensities()[y * width..(y + 1) * width];
                for (x, intensity) in row.iter().copied().enumerate() {
                    if intensity == 0 {
                        continue;
                    }
                    left = left.min(x);
                    right = Some(right.map_or(x, |current: usize| current.max(x)));
                    ink_pixels += 1;
                }
            }
            let Some(right) = right else { continue };
            if right - left < settings.minimum_horizontal_span {
                continue;
            }

            let crop_left = left.saturating_sub(settings.horizontal_margin);
            let crop_right = right
                .saturating_add(settings.horizontal_margin)
                .min(width - 1);
            let crop_top = top.saturating_sub(settings.vertical_margin);
            let crop_bottom = bottom
                .saturating_add(settings.vertical_margin)
                .min(height - 1);
            let source_rect = PixelRect::new(
                crop_left,
                crop_top,
                crop_right - crop_left + 1,
                crop_bottom - crop_top + 1,
            )?;
            let content_rect = PixelRect::new(left, top, right - left + 1, band_height)?;
            let content_fingerprint = crop_fingerprint(mask, source_rect);
            let crop = CropMetadata {
                source_rect,
                desktop_region: source_region.subregion(source_rect)?,
                content_rect,
                logical_content_top: top - crop_top,
                logical_content_bottom: bottom - crop_top,
                content_fingerprint,
                is_fallback: false,
            };
            let identity = PhysicalBandIdentity {
                content_fingerprint,
                source_top: top,
                source_bottom: bottom,
                source_left: crop_left,
                source_right: crop_right,
            };
            result.bands.push(PhysicalLineBand {
                index: result.bands.len(),
                identity,
                ink_pixels,
                crop,
            });
        }

        if fallback_required {
            let source_rect = PixelRect::new(0, 0, width, height)?;
            result.fallback_crop = Some(CropMetadata {
                source_rect,
                desktop_region: source_region,
                content_rect: source_rect,
                logical_content_top: 0,
                logical_content_bottom: height - 1,
                content_fingerprint: crop_fingerprint(mask, source_rect),
                is_fallback: true,
            });
        }
        Ok(result)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CroppedMask {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bgra_pixels: Vec<u8>,
    pub metadata: CropMetadata,
}

/// Reusable BGRA storage for one OCR crop.
///
/// Monitoring code normally keeps one buffer per detected band so steady-state scans do not
/// allocate. The crop metadata remains owned by the detector and can be passed to the OCR layer
/// alongside [`bgra_pixels`](Self::bgra_pixels).
#[derive(Clone, Debug, Default)]
pub struct BgraCropBuffer {
    width: usize,
    height: usize,
    stride: usize,
    bgra_pixels: Vec<u8>,
}

/// Metadata for one tight crop containing every blue glyph in a mask.
///
/// Traditional-Chinese Windows OCR performs its own multi-line layout analysis, so it consumes
/// this combined crop rather than one image per physical row.
pub fn combined_crop_metadata(
    mask: &BlueTextMask,
    margin: usize,
) -> Result<CropMetadata, BandDetectionError> {
    let source_region = mask.source_region().ok_or(BandDetectionError::EmptyMask)?;
    let width = mask.width();
    let height = mask.height();
    if width == 0 || height == 0 {
        return Err(BandDetectionError::EmptyMask);
    }
    let mut minimum_x = width;
    let mut minimum_y = height;
    let mut maximum_x = None;
    let mut maximum_y = None;
    for (index, intensity) in mask.intensities().iter().copied().enumerate() {
        if intensity == 0 {
            continue;
        }
        let x = index % width;
        let y = index / width;
        minimum_x = minimum_x.min(x);
        minimum_y = minimum_y.min(y);
        maximum_x = Some(maximum_x.map_or(x, |current: usize| current.max(x)));
        maximum_y = Some(maximum_y.map_or(y, |current: usize| current.max(y)));
    }
    let (Some(maximum_x), Some(maximum_y)) = (maximum_x, maximum_y) else {
        return PixelRect::new(0, 0, 1, 1)
            .and_then(|source_rect| {
                Ok(CropMetadata {
                    source_rect,
                    desktop_region: source_region.subregion(source_rect)?,
                    content_rect: source_rect,
                    logical_content_top: 0,
                    logical_content_bottom: 0,
                    content_fingerprint: crop_fingerprint(mask, source_rect),
                    is_fallback: false,
                })
            })
            .map_err(BandDetectionError::from);
    };
    let left = minimum_x.saturating_sub(margin);
    let top = minimum_y.saturating_sub(margin);
    let right = maximum_x.saturating_add(margin).min(width - 1);
    let bottom = maximum_y.saturating_add(margin).min(height - 1);
    let source_rect = PixelRect::new(left, top, right - left + 1, bottom - top + 1)?;
    let content_rect = PixelRect::new(
        minimum_x,
        minimum_y,
        maximum_x - minimum_x + 1,
        maximum_y - minimum_y + 1,
    )?;
    Ok(CropMetadata {
        source_rect,
        desktop_region: source_region.subregion(source_rect)?,
        content_rect,
        logical_content_top: minimum_y - top,
        logical_content_bottom: maximum_y - top,
        content_fingerprint: crop_fingerprint(mask, source_rect),
        is_fallback: false,
    })
}

impl BgraCropBuffer {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn bgra_pixels(&self) -> &[u8] {
        &self.bgra_pixels
    }

    /// Rebuilds this crop while retaining the largest allocation seen by this buffer.
    pub fn prepare(
        &mut self,
        mask: &BlueTextMask,
        metadata: &CropMetadata,
    ) -> Result<(), BandDetectionError> {
        metadata
            .source_rect
            .validate_within(mask.width(), mask.height())?;
        self.width = metadata.source_rect.width;
        self.height = metadata.source_rect.height;
        self.stride = self
            .width
            .checked_mul(BYTES_PER_PIXEL)
            .ok_or(FrameError::DimensionsTooLarge)?;
        let required_length = self
            .stride
            .checked_mul(self.height)
            .ok_or(FrameError::DimensionsTooLarge)?;
        self.bgra_pixels.resize(required_length, 0);

        for output_y in 0..self.height {
            let source_y = metadata.source_rect.y + output_y;
            for output_x in 0..self.width {
                let source_x = metadata.source_rect.x + output_x;
                let intensity = mask.intensities()[source_y * mask.width() + source_x];
                let pixel = output_y * self.stride + output_x * BYTES_PER_PIXEL;
                self.bgra_pixels[pixel] = intensity;
                self.bgra_pixels[pixel + 1] = intensity;
                self.bgra_pixels[pixel + 2] = intensity;
                self.bgra_pixels[pixel + 3] = 255;
            }
        }
        Ok(())
    }
}

/// Materializes an OCR-friendly grayscale BGRA crop from metadata returned by detection.
pub fn crop_mask_bgra(
    mask: &BlueTextMask,
    metadata: &CropMetadata,
) -> Result<CroppedMask, BandDetectionError> {
    let mut buffer = BgraCropBuffer::default();
    buffer.prepare(mask, metadata)?;
    Ok(CroppedMask {
        width: buffer.width,
        height: buffer.height,
        stride: buffer.stride,
        bgra_pixels: buffer.bgra_pixels,
        metadata: metadata.clone(),
    })
}

fn crop_fingerprint(mask: &BlueTextMask, rect: PixelRect) -> u64 {
    let mut fingerprint = FNV1A_64_OFFSET_BASIS;
    fingerprint ^= rect.width as u64;
    fingerprint = fingerprint.wrapping_mul(FNV1A_64_PRIME);
    fingerprint ^= rect.height as u64;
    fingerprint = fingerprint.wrapping_mul(FNV1A_64_PRIME);

    for output_y in 0..rect.height {
        let source_y = rect.y + output_y;
        for output_x in 0..rect.width {
            let source_x = rect.x + output_x;
            let intensity = mask.intensities()[source_y * mask.width() + source_x];
            if intensity == 0 {
                continue;
            }
            fingerprint ^= (output_y * rect.width + output_x + 1) as u64;
            fingerprint = fingerprint.wrapping_mul(FNV1A_64_PRIME);
            fingerprint ^= u64::from(intensity >> 4);
            fingerprint = fingerprint.wrapping_mul(FNV1A_64_PRIME);
        }
    }
    fingerprint
}

#[derive(Debug)]
pub enum BandDetectionError {
    EmptyMask,
    InvalidSettings(&'static str),
    Frame(FrameError),
}

impl fmt::Display for BandDetectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMask => formatter.write_str("blue-text mask is empty"),
            Self::InvalidSettings(message) => formatter.write_str(message),
            Self::Frame(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for BandDetectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Frame(error) => Some(error),
            _ => None,
        }
    }
}

impl From<FrameError> for BandDetectionError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}
