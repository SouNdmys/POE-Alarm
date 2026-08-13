use crate::CapturedFrame;

pub const FNV1A_64_OFFSET_BASIS: u64 = 14_695_981_039_346_656_037;
pub const FNV1A_64_PRIME: u64 = 1_099_511_628_211;

/// Controls the grayscale value passed to a later OCR stage.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BlueMaskIntensityMode {
    /// Preserve anti-aliased edges using `72 + 3 * blue_dominance`.
    #[default]
    Dominance,
    BoostedDominance,
    BlueChannel,
    Binary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlueMaskSettings {
    pub minimum_blue: u8,
    pub minimum_blue_dominance: u8,
    pub maximum_warm_channel_difference: u8,
    pub intensity_mode: BlueMaskIntensityMode,
}

impl Default for BlueMaskSettings {
    fn default() -> Self {
        Self {
            minimum_blue: 105,
            minimum_blue_dominance: 18,
            maximum_warm_channel_difference: 72,
            intensity_mode: BlueMaskIntensityMode::Dominance,
        }
    }
}

/// One byte per source pixel; zero is background and non-zero is blue glyph ink.
#[derive(Clone, Debug, Default)]
pub struct BlueTextMask {
    width: usize,
    height: usize,
    intensities: Vec<u8>,
    x_fingerprint_terms: Vec<usize>,
    source_region: Option<crate::CaptureRegion>,
    fingerprint: u64,
}

impl BlueTextMask {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.width
    }

    pub fn intensities(&self) -> &[u8] {
        &self.intensities
    }

    pub fn source_region(&self) -> Option<crate::CaptureRegion> {
        self.source_region
    }

    /// A semantic FNV-1a-style hash of blue glyphs plus source ROI dimensions.
    /// The screen-space ROI origin is deliberately excluded.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    pub fn intensity_at(&self, x: usize, y: usize) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(self.intensities[y * self.width + x])
    }
}

pub fn build_blue_mask(frame: &CapturedFrame, settings: BlueMaskSettings) -> BlueTextMask {
    let mut output = BlueTextMask::default();
    build_blue_mask_into(frame, settings, &mut output);
    output
}

/// Builds a mask while retaining `output`'s allocation between scans.
pub fn build_blue_mask_into(
    frame: &CapturedFrame,
    settings: BlueMaskSettings,
    output: &mut BlueTextMask,
) {
    let width = frame.width();
    let height = frame.height();
    let pixel_count = width
        .checked_mul(height)
        .expect("validated capture dimensions must fit memory");
    output.width = width;
    output.height = height;
    output.source_region = Some(frame.region());
    output.intensities.resize(pixel_count, 0);
    output.intensities.fill(0);
    if output.x_fingerprint_terms.len() != width {
        output.x_fingerprint_terms.clear();
        output
            .x_fingerprint_terms
            .extend((0..width).map(|x| x * 397));
    }

    let mut fingerprint = FNV1A_64_OFFSET_BASIS;
    let source = frame.bgra_pixels();
    for y in 0..height {
        let source_row = y * frame.stride();
        let mask_row = y * width;
        let source_pixels = source[source_row..source_row + width * crate::BYTES_PER_PIXEL]
            .chunks_exact(crate::BYTES_PER_PIXEL);
        let mask_pixels = &mut output.intensities[mask_row..mask_row + width];
        for (x, (pixel, mask_pixel)) in source_pixels.zip(mask_pixels).enumerate() {
            let blue = pixel[0];
            let green = pixel[1];
            let red = pixel[2];
            let warm_max = red.max(green);
            let dominance = i16::from(blue) - i16::from(warm_max);
            let warm_difference = red.abs_diff(green);
            if blue < settings.minimum_blue
                || dominance < i16::from(settings.minimum_blue_dominance)
                || warm_difference > settings.maximum_warm_channel_difference
            {
                continue;
            }

            let intensity = match settings.intensity_mode {
                BlueMaskIntensityMode::Dominance => (72 + i32::from(dominance) * 3).clamp(0, 255),
                BlueMaskIntensityMode::BoostedDominance => {
                    (96 + i32::from(dominance) * 4).clamp(0, 255)
                }
                BlueMaskIntensityMode::BlueChannel => i32::from(blue),
                BlueMaskIntensityMode::Binary => 255,
            } as u8;

            *mask_pixel = intensity;
            fingerprint ^= (output.x_fingerprint_terms[x] ^ y ^ usize::from(intensity >> 5)) as u64;
            fingerprint = fingerprint.wrapping_mul(FNV1A_64_PRIME);
        }
    }

    // Match the established POE Alarm fingerprint contract: glyph semantics first,
    // then logical ROI width and height. ROI position is irrelevant to line wrapping.
    fingerprint ^= width as u64;
    fingerprint = fingerprint.wrapping_mul(FNV1A_64_PRIME);
    fingerprint ^= height as u64;
    fingerprint = fingerprint.wrapping_mul(FNV1A_64_PRIME);
    output.fingerprint = fingerprint;
}

/// Standard FNV-1a 64-bit hash, useful for fixtures and cross-language probes.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV1A_64_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV1A_64_PRIME);
    }
    hash
}
