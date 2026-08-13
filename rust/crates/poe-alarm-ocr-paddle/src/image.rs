use crate::PaddleError;

/// Location of the intensity byte within each input pixel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    Gray8,
    /// Top-down BGRA8. The blue byte is read exactly like .NET `PreparedFrame`.
    Bgra8,
}

impl PixelFormat {
    fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Gray8 => 1,
            Self::Bgra8 => 4,
        }
    }
}

/// Bounds-checked borrowed grayscale or top-down BGRA mask input.
#[derive(Clone, Copy, Debug)]
pub struct ImageView<'a> {
    width: usize,
    height: usize,
    stride: usize,
    pixels: &'a [u8],
    format: PixelFormat,
    logical_top: usize,
    logical_bottom: usize,
}

impl<'a> ImageView<'a> {
    pub fn gray8(
        width: usize,
        height: usize,
        stride: usize,
        pixels: &'a [u8],
    ) -> Result<Self, PaddleError> {
        Self::new(width, height, stride, pixels, PixelFormat::Gray8)
    }

    pub fn bgra8(
        width: usize,
        height: usize,
        stride: usize,
        pixels: &'a [u8],
    ) -> Result<Self, PaddleError> {
        Self::new(width, height, stride, pixels, PixelFormat::Bgra8)
    }

    fn new(
        width: usize,
        height: usize,
        stride: usize,
        pixels: &'a [u8],
        format: PixelFormat,
    ) -> Result<Self, PaddleError> {
        if width == 0 || height == 0 {
            return Err(PaddleError::InvalidImage(
                "width and height must both be positive".to_owned(),
            ));
        }
        let minimum_stride = width
            .checked_mul(format.bytes_per_pixel())
            .ok_or_else(|| PaddleError::InvalidImage("image dimensions overflow".to_owned()))?;
        if stride < minimum_stride {
            return Err(PaddleError::InvalidImage(format!(
                "stride is {stride}; expected at least {minimum_stride}"
            )));
        }
        let expected_length = stride
            .checked_mul(height)
            .ok_or_else(|| PaddleError::InvalidImage("image dimensions overflow".to_owned()))?;
        if pixels.len() != expected_length {
            return Err(PaddleError::InvalidImage(format!(
                "buffer is {} bytes; expected exactly {expected_length}",
                pixels.len()
            )));
        }
        Ok(Self {
            width,
            height,
            stride,
            pixels,
            format,
            logical_top: 0,
            logical_bottom: height - 1,
        })
    }

    /// Restricts ink discovery to an inclusive logical content band.
    pub fn with_logical_content(
        mut self,
        top: usize,
        bottom_inclusive: usize,
    ) -> Result<Self, PaddleError> {
        if top > bottom_inclusive || bottom_inclusive >= self.height {
            return Err(PaddleError::InvalidImage(format!(
                "logical content bounds {top}..={bottom_inclusive} are outside height {}",
                self.height
            )));
        }
        self.logical_top = top;
        self.logical_bottom = bottom_inclusive;
        Ok(self)
    }

    pub fn width(self) -> usize {
        self.width
    }

    pub fn height(self) -> usize {
        self.height
    }

    pub fn stride(self) -> usize {
        self.stride
    }

    pub fn format(self) -> PixelFormat {
        self.format
    }

    pub fn pixels(self) -> &'a [u8] {
        self.pixels
    }

    pub(crate) fn logical_top(self) -> usize {
        self.logical_top
    }

    pub(crate) fn logical_bottom(self) -> usize {
        self.logical_bottom
    }

    pub(crate) fn intensity(self, x: usize, y: usize) -> u8 {
        let offset = y * self.stride + x * self.format.bytes_per_pixel();
        self.pixels[offset]
    }
}

/// Owned image payload intended for transfer to a process-lifetime OCR worker.
#[derive(Clone, Debug)]
pub struct OwnedImage {
    width: usize,
    height: usize,
    stride: usize,
    pixels: Vec<u8>,
    format: PixelFormat,
    logical_top: usize,
    logical_bottom: usize,
}

impl OwnedImage {
    pub fn gray8(
        width: usize,
        height: usize,
        stride: usize,
        pixels: Vec<u8>,
    ) -> Result<Self, PaddleError> {
        Self::new(width, height, stride, pixels, PixelFormat::Gray8)
    }

    pub fn bgra8(
        width: usize,
        height: usize,
        stride: usize,
        pixels: Vec<u8>,
    ) -> Result<Self, PaddleError> {
        Self::new(width, height, stride, pixels, PixelFormat::Bgra8)
    }

    fn new(
        width: usize,
        height: usize,
        stride: usize,
        pixels: Vec<u8>,
        format: PixelFormat,
    ) -> Result<Self, PaddleError> {
        ImageView::new(width, height, stride, &pixels, format)?;
        Ok(Self {
            width,
            height,
            stride,
            pixels,
            format,
            logical_top: 0,
            logical_bottom: height - 1,
        })
    }

    pub fn with_logical_content(
        mut self,
        top: usize,
        bottom_inclusive: usize,
    ) -> Result<Self, PaddleError> {
        self.as_view().with_logical_content(top, bottom_inclusive)?;
        self.logical_top = top;
        self.logical_bottom = bottom_inclusive;
        Ok(self)
    }

    pub fn as_view(&self) -> ImageView<'_> {
        ImageView {
            width: self.width,
            height: self.height,
            stride: self.stride,
            pixels: &self.pixels,
            format: self.format,
            logical_top: self.logical_top,
            logical_bottom: self.logical_bottom,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_contract_accepts_padding_and_rejects_short_buffers() {
        let pixels = [0_u8; 24];
        let image = ImageView::bgra8(2, 2, 12, &pixels).unwrap();
        assert_eq!(image.stride(), 12);
        assert!(ImageView::bgra8(2, 2, 7, &[0; 14]).is_err());
        assert!(ImageView::bgra8(2, 2, 8, &[0; 15]).is_err());
    }

    #[test]
    fn bgra_intensity_is_the_blue_byte() {
        let pixels = [17_u8, 99, 231, 255];
        let image = ImageView::bgra8(1, 1, 4, &pixels).unwrap();
        assert_eq!(image.intensity(0, 0), 17);
    }
}
