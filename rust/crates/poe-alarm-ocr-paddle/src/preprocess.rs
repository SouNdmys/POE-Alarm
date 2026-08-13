use crate::{ImageView, PaddleError, TextPolarity};

pub const MODEL_HEIGHT: usize = 48;
pub const MINIMUM_MODEL_WIDTH: usize = 320;
pub const MAXIMUM_MODEL_WIDTH: usize = 3_200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InkCrop {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Default)]
pub(crate) struct InferenceWorkspace {
    input: Vec<f32>,
}

impl InferenceWorkspace {
    pub fn prepare(&mut self, length: usize) -> &mut [f32] {
        if self.input.len() < length {
            self.input.resize(length, 0.0);
        }
        let active = &mut self.input[..length];
        active.fill(0.0);
        active
    }

    pub fn active(&self, length: usize) -> &[f32] {
        &self.input[..length]
    }

    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.input.len()
    }
}

pub(crate) fn find_ink_crop(image: ImageView<'_>, margin: usize) -> InkCrop {
    let mut minimum_x = image.width();
    let mut minimum_y = image.height();
    let mut maximum_x = None;
    let mut maximum_y = None;

    for y in image.logical_top()..=image.logical_bottom() {
        for x in 0..image.width() {
            if image.intensity(x, y) == 0 {
                continue;
            }
            minimum_x = minimum_x.min(x);
            minimum_y = minimum_y.min(y);
            maximum_x = Some(maximum_x.map_or(x, |current: usize| current.max(x)));
            maximum_y = Some(maximum_y.map_or(y, |current: usize| current.max(y)));
        }
    }

    let (Some(maximum_x), Some(maximum_y)) = (maximum_x, maximum_y) else {
        return InkCrop {
            x: 0,
            y: 0,
            width: image.width(),
            height: image.height(),
        };
    };

    let minimum_x = minimum_x.saturating_sub(margin);
    let minimum_y = minimum_y.saturating_sub(margin);
    let maximum_x = maximum_x.saturating_add(margin).min(image.width() - 1);
    let maximum_y = maximum_y.saturating_add(margin).min(image.height() - 1);
    InkCrop {
        x: minimum_x,
        y: minimum_y,
        width: maximum_x - minimum_x + 1,
        height: maximum_y - minimum_y + 1,
    }
}

pub(crate) fn calculate_resized_width(crop: InkCrop) -> usize {
    let scaled = (crop.width as f64 * (MODEL_HEIGHT as f64 / crop.height as f64)).ceil();
    (scaled as usize).clamp(1, MAXIMUM_MODEL_WIDTH)
}

pub(crate) fn calculate_tensor_width(resized_width: usize, right_padding: usize) -> usize {
    round_up(
        MINIMUM_MODEL_WIDTH.max(resized_width.saturating_add(right_padding)),
        8,
    )
    .clamp(8, MAXIMUM_MODEL_WIDTH)
}

pub(crate) fn fill_tensor(
    image: ImageView<'_>,
    crop: InkCrop,
    resized_width: usize,
    tensor_width: usize,
    output: &mut [f32],
    polarity: TextPolarity,
) -> Result<(), PaddleError> {
    let expected = 3_usize
        .checked_mul(MODEL_HEIGHT)
        .and_then(|value| value.checked_mul(tensor_width))
        .ok_or_else(|| PaddleError::InvalidImage("tensor dimensions overflow".to_owned()))?;
    if output.len() != expected {
        return Err(PaddleError::InvalidImage(format!(
            "tensor buffer has {} elements; expected {expected}",
            output.len()
        )));
    }
    if resized_width > tensor_width {
        return Err(PaddleError::InvalidImage(format!(
            "resized width {resized_width} exceeds tensor width {tensor_width}"
        )));
    }

    let plane_length = MODEL_HEIGHT * tensor_width;
    for target_y in 0..MODEL_HEIGHT {
        let source_y = ((target_y as f64 + 0.5) * crop.height as f64 / MODEL_HEIGHT as f64) - 0.5;
        let y0 = (source_y.floor() as isize).clamp(0, crop.height as isize - 1) as usize;
        let y1 = (y0 + 1).min(crop.height - 1);
        let y_fraction = (source_y - y0 as f64).clamp(0.0, 1.0);

        for target_x in 0..resized_width {
            let source_x =
                ((target_x as f64 + 0.5) * crop.width as f64 / resized_width as f64) - 0.5;
            let x0 = (source_x.floor() as isize).clamp(0, crop.width as isize - 1) as usize;
            let x1 = (x0 + 1).min(crop.width - 1);
            let x_fraction = (source_x - x0 as f64).clamp(0.0, 1.0);

            let top_left = f64::from(image.intensity(crop.x + x0, crop.y + y0));
            let top_right = f64::from(image.intensity(crop.x + x1, crop.y + y0));
            let bottom_left = f64::from(image.intensity(crop.x + x0, crop.y + y1));
            let bottom_right = f64::from(image.intensity(crop.x + x1, crop.y + y1));
            let top = top_left + (top_right - top_left) * x_fraction;
            let bottom = bottom_left + (bottom_right - bottom_left) * x_fraction;
            let mut intensity = top + (bottom - top) * y_fraction;
            if polarity == TextPolarity::DarkOnLight {
                intensity = 255.0 - intensity;
            }

            let normalized = (intensity / 127.5 - 1.0) as f32;
            let pixel = target_y * tensor_width + target_x;
            output[pixel] = normalized;
            output[plane_length + pixel] = normalized;
            output[plane_length * 2 + pixel] = normalized;
        }
    }
    Ok(())
}

pub(crate) fn split_crop(
    image: ImageView<'_>,
    crop: InkCrop,
    natural_width: usize,
    maximum_segment_width: usize,
) -> Vec<InkCrop> {
    let segment_count = natural_width.div_ceil(maximum_segment_width).max(2);
    let mut boundaries = Vec::with_capacity(segment_count + 1);
    boundaries.push(crop.x);
    let crop_end = crop.x + crop.width;
    let minimum_source_width = (crop.height / 2).max(4);

    for index in 1..segment_count {
        let ideal_offset =
            (crop.width as f64 * (index as f64 / segment_count as f64)).round_ties_even() as usize;
        let ideal = crop.x + ideal_offset;
        let minimum = boundaries[index - 1].saturating_add(minimum_source_width);
        let remaining_segments = segment_count - index;
        let maximum = crop_end.saturating_sub(remaining_segments * minimum_source_width);
        boundaries.push(find_quiet_column(image, crop, ideal, minimum, maximum));
    }
    boundaries.push(crop_end);

    boundaries
        .windows(2)
        .map(|pair| InkCrop {
            x: pair[0],
            y: crop.y,
            width: pair[1].saturating_sub(pair[0]).max(1),
            height: crop.height,
        })
        .collect()
}

fn find_quiet_column(
    image: ImageView<'_>,
    crop: InkCrop,
    ideal: usize,
    minimum: usize,
    maximum: usize,
) -> usize {
    if minimum >= maximum {
        return ideal.clamp(crop.x + 1, crop.x + crop.width - 1);
    }

    let radius = crop.height.max(6);
    let start = minimum.max(ideal.saturating_sub(radius));
    let end = maximum.min(ideal.saturating_add(radius));
    let mut best_column = ideal.clamp(start, end);
    let mut best_ink = usize::MAX;
    let mut best_distance = usize::MAX;
    for x in start..=end {
        let ink = (crop.y..crop.y + crop.height)
            .filter(|&y| image.intensity(x, y) != 0)
            .count();
        let distance = x.abs_diff(ideal);
        if ink < best_ink || (ink == best_ink && distance < best_distance) {
            best_column = x;
            best_ink = ink;
            best_distance = distance;
        }
    }
    best_column
}

fn round_up(value: usize, multiple: usize) -> usize {
    value.div_ceil(multiple) * multiple
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_crop_obeys_logical_bounds_and_margin() {
        let mut pixels = vec![0_u8; 8 * 6];
        pixels[1] = 200;
        pixels[3 * 8 + 4] = 200;
        let image = ImageView::gray8(8, 6, 8, &pixels)
            .unwrap()
            .with_logical_content(2, 5)
            .unwrap();
        assert_eq!(
            find_ink_crop(image, 1),
            InkCrop {
                x: 3,
                y: 2,
                width: 3,
                height: 3
            }
        );
    }

    #[test]
    fn no_ink_uses_the_entire_frame_like_dotnet() {
        let pixels = [0_u8; 35];
        let image = ImageView::gray8(7, 5, 7, &pixels)
            .unwrap()
            .with_logical_content(2, 3)
            .unwrap();
        assert_eq!(
            find_ink_crop(image, 8),
            InkCrop {
                x: 0,
                y: 0,
                width: 7,
                height: 5
            }
        );
    }

    #[test]
    fn tensor_is_chw_normalized_and_padding_is_neutral() {
        let pixels = [0_u8, 255];
        let image = ImageView::gray8(2, 1, 2, &pixels).unwrap();
        let crop = InkCrop {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        let mut output = vec![0.0; 3 * MODEL_HEIGHT * MINIMUM_MODEL_WIDTH];
        fill_tensor(
            image,
            crop,
            2,
            MINIMUM_MODEL_WIDTH,
            &mut output,
            TextPolarity::BrightOnDark,
        )
        .unwrap();
        let plane = MODEL_HEIGHT * MINIMUM_MODEL_WIDTH;
        for offset in [0, plane, plane * 2] {
            assert_eq!(output[offset], -1.0);
            assert_eq!(output[offset + 1], 1.0);
            assert_eq!(output[offset + 2], 0.0);
        }
    }

    #[test]
    fn dark_on_light_inverts_intensity() {
        let pixels = [0_u8];
        let image = ImageView::gray8(1, 1, 1, &pixels).unwrap();
        let crop = InkCrop {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let mut output = vec![0.0; 3 * MODEL_HEIGHT * MINIMUM_MODEL_WIDTH];
        fill_tensor(
            image,
            crop,
            1,
            MINIMUM_MODEL_WIDTH,
            &mut output,
            TextPolarity::DarkOnLight,
        )
        .unwrap();
        assert_eq!(output[0], 1.0);
    }

    #[test]
    fn workspace_keeps_its_largest_allocation_and_clears_active_data() {
        let mut workspace = InferenceWorkspace::default();
        workspace.prepare(12).fill(3.0);
        workspace.prepare(4);
        assert_eq!(workspace.capacity(), 12);
        assert_eq!(workspace.active(4), &[0.0; 4]);
    }

    #[test]
    fn segmentation_selects_a_quiet_column_near_the_ideal_split() {
        let mut pixels = vec![1_u8; 40 * 10];
        for y in 0..10 {
            pixels[y * 40 + 19] = 0;
        }
        let image = ImageView::gray8(40, 10, 40, &pixels).unwrap();
        let crop = InkCrop {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        let segments = split_crop(image, crop, 400, 320);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].width, 19);
        assert_eq!(segments[1].x, 19);
    }
}
