use std::time::SystemTime;

use poe_alarm_vision::{
    BandDetectionSettings, BgraCropBuffer, BlueMaskSettings, CaptureRegion, CapturedFrame,
    FrameError, PhysicalBandDetector, build_blue_mask, combined_crop_metadata, crop_mask_bgra,
    fnv1a64,
};

#[test]
fn default_mask_uses_proven_blue_thresholds_and_dominance_strength() {
    let frame = frame_from_pixels(
        5,
        1,
        20,
        &[
            [104, 40, 40, 255],   // blue below 105
            [120, 103, 100, 255], // dominance below 18
            [180, 80, 155, 255],  // warm channels differ by more than 72
            [150, 100, 90, 255],  // valid: dominance 50 -> 222
            [255, 0, 0, 255],     // valid and clamped to 255
        ],
    );
    let mask = build_blue_mask(&frame, BlueMaskSettings::default());
    assert_eq!(mask.intensities(), &[0, 0, 0, 222, 255]);
}

#[test]
fn combined_crop_tightly_contains_all_blue_ink_with_requested_margin() {
    let width = 100;
    let height = 60;
    let stride = width * 4;
    let mut pixels = vec![0; stride * height];
    paint_blue_rect(&mut pixels, stride, 20, 12, 61, 18);
    paint_blue_rect(&mut pixels, stride, 30, 35, 81, 42);
    let frame = CapturedFrame::from_bgra(
        CaptureRegion::new(100, 200, width as u32, height as u32).unwrap(),
        stride,
        pixels,
        SystemTime::now(),
    )
    .unwrap();
    let mask = build_blue_mask(&frame, BlueMaskSettings::default());
    let metadata = combined_crop_metadata(&mask, 8).unwrap();
    assert_eq!(metadata.source_rect.x, 12);
    assert_eq!(metadata.source_rect.y, 4);
    assert_eq!(metadata.source_rect.width, 77);
    assert_eq!(metadata.source_rect.height, 46);
    assert_eq!(metadata.desktop_region.x, 112);
    assert_eq!(metadata.desktop_region.y, 204);
    assert_eq!(metadata.logical_content_top, 8);
    assert_eq!(metadata.logical_content_bottom, 37);

    let mut first = BgraCropBuffer::default();
    first.prepare(&mask, &metadata).unwrap();
    let pointer = first.bgra_pixels().as_ptr();
    first.prepare(&mask, &metadata).unwrap();
    assert_eq!(first.bgra_pixels().as_ptr(), pointer);
    assert_eq!(first.width(), metadata.source_rect.width);
    assert_eq!(first.height(), metadata.source_rect.height);
}

#[test]
fn semantic_fingerprint_ignores_background_but_includes_roi_dimensions() {
    let dark = frame_from_pixels(2, 2, 8, &[[0, 0, 0, 255]; 4]);
    let noisy = frame_from_pixels(2, 2, 8, &[[80, 70, 60, 255]; 4]);
    assert_eq!(
        build_blue_mask(&dark, BlueMaskSettings::default()).fingerprint(),
        build_blue_mask(&noisy, BlueMaskSettings::default()).fingerprint()
    );

    let same_pixel_count_different_shape = frame_from_pixels(1, 4, 4, &[[0, 0, 0, 255]; 4]);
    assert_ne!(
        build_blue_mask(&dark, BlueMaskSettings::default()).fingerprint(),
        build_blue_mask(
            &same_pixel_count_different_shape,
            BlueMaskSettings::default()
        )
        .fingerprint()
    );
}

#[test]
fn standard_fnv64_has_known_value() {
    assert_eq!(fnv1a64(b"hello"), 0xa430_d846_80aa_bd0b);
}

#[test]
fn detects_physical_rows_and_preserves_source_crop_coordinates() {
    let width = 120;
    let height = 60;
    let stride = width * 4;
    let mut pixels = vec![0; stride * height];
    paint_blue_rect(&mut pixels, stride, 20, 10, 51, 15);
    paint_blue_rect(&mut pixels, stride, 40, 30, 81, 36);
    let frame = CapturedFrame::from_bgra(
        CaptureRegion::new(100, 200, width as u32, height as u32).unwrap(),
        stride,
        pixels,
        SystemTime::now(),
    )
    .unwrap();
    let mask = build_blue_mask(&frame, BlueMaskSettings::default());
    let mut detector = PhysicalBandDetector::new();
    let result = detector
        .detect(&mask, BandDetectionSettings::default())
        .unwrap();

    assert_eq!(result.bands.len(), 2);
    let first = &result.bands[0];
    assert_eq!(first.identity.source_top, 10);
    assert_eq!(first.identity.source_bottom, 14);
    assert_eq!(first.identity.source_left, 10);
    assert_eq!(first.identity.source_right, 60);
    assert_eq!(first.crop.source_rect.x, 10);
    assert_eq!(first.crop.source_rect.y, 0);
    assert_eq!(first.crop.source_rect.width, 51);
    assert_eq!(first.crop.source_rect.height, 29);
    assert_eq!(first.crop.desktop_region.x, 110);
    assert_eq!(first.crop.desktop_region.y, 200);
    assert_eq!(first.crop.content_rect.x, 20);
    assert_eq!(first.crop.content_rect.y, 10);
    assert_eq!(first.crop.logical_content_top, 10);
    assert_eq!(first.crop.logical_content_bottom, 14);
    assert_eq!(first.identity.to_string().split(':').count(), 6);

    let crop = crop_mask_bgra(&mask, &first.crop).unwrap();
    assert_eq!(crop.stride, crop.width * 4);
    let ink_pixel = first.crop.logical_content_top * crop.stride + 10 * 4;
    assert!(crop.bgra_pixels[ink_pixel] > 0);
    assert_eq!(crop.bgra_pixels[ink_pixel], crop.bgra_pixels[ink_pixel + 1]);
    assert_eq!(crop.bgra_pixels[ink_pixel + 3], 255);
}

#[test]
fn two_blank_projection_rows_are_kept_in_one_physical_band() {
    let width = 60;
    let height = 30;
    let stride = width * 4;
    let mut pixels = vec![0; stride * height];
    paint_blue_rect(&mut pixels, stride, 10, 5, 41, 8);
    paint_blue_rect(&mut pixels, stride, 10, 10, 41, 13);
    let frame = CapturedFrame::from_bgra(
        CaptureRegion::new(0, 0, width as u32, height as u32).unwrap(),
        stride,
        pixels,
        SystemTime::now(),
    )
    .unwrap();
    let mask = build_blue_mask(&frame, BlueMaskSettings::default());
    let result = PhysicalBandDetector::new()
        .detect(&mask, BandDetectionSettings::default())
        .unwrap();
    assert_eq!(result.bands.len(), 1);
    assert_eq!(result.bands[0].identity.source_top, 5);
    assert_eq!(result.bands[0].identity.source_bottom, 12);
}

#[test]
fn oversized_connected_ink_requests_full_roi_fallback() {
    let width = 80;
    let height = 70;
    let stride = width * 4;
    let mut pixels = vec![0; stride * height];
    paint_blue_rect(&mut pixels, stride, 15, 5, 66, 59);
    let frame = CapturedFrame::from_bgra(
        CaptureRegion::new(20, 30, width as u32, height as u32).unwrap(),
        stride,
        pixels,
        SystemTime::now(),
    )
    .unwrap();
    let mask = build_blue_mask(&frame, BlueMaskSettings::default());
    let result = PhysicalBandDetector::new()
        .detect(&mask, BandDetectionSettings::default())
        .unwrap();
    let fallback = result.fallback_crop.expect("fallback is required");
    assert!(fallback.is_fallback);
    assert_eq!(fallback.source_rect.width, width);
    assert_eq!(fallback.source_rect.height, height);
    assert_eq!(fallback.desktop_region, frame.region());
}

#[test]
fn captured_frame_rejects_short_stride_and_bad_buffer_length() {
    let region = CaptureRegion::new(0, 0, 2, 2).unwrap();
    let short_stride = CapturedFrame::from_bgra(region, 7, vec![0; 14], SystemTime::now());
    assert!(matches!(
        short_stride,
        Err(FrameError::StrideTooSmall { .. })
    ));

    let wrong_length = CapturedFrame::from_bgra(region, 8, vec![0; 15], SystemTime::now());
    assert!(matches!(wrong_length, Err(FrameError::BufferLength { .. })));
}

fn frame_from_pixels(
    width: usize,
    height: usize,
    stride: usize,
    pixels: &[[u8; 4]],
) -> CapturedFrame {
    assert_eq!(pixels.len(), width * height);
    let mut buffer = vec![0; stride * height];
    for y in 0..height {
        for x in 0..width {
            let destination = y * stride + x * 4;
            buffer[destination..destination + 4].copy_from_slice(&pixels[y * width + x]);
        }
    }
    CapturedFrame::from_bgra(
        CaptureRegion::new(0, 0, width as u32, height as u32).unwrap(),
        stride,
        buffer,
        SystemTime::now(),
    )
    .unwrap()
}

fn paint_blue_rect(
    pixels: &mut [u8],
    stride: usize,
    left: usize,
    top: usize,
    right_exclusive: usize,
    bottom_exclusive: usize,
) {
    for y in top..bottom_exclusive {
        for x in left..right_exclusive {
            let pixel = y * stride + x * 4;
            pixels[pixel..pixel + 4].copy_from_slice(&[180, 100, 90, 255]);
        }
    }
}
