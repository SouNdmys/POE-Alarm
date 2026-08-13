#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use poe_alarm_vision::{
        BandDetectionSettings, BlueMaskSettings, CaptureRegion, PhysicalBandDetector,
        WicScreenshotDecoder, build_blue_mask,
    };

    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let path = arguments
        .first()
        .ok_or("usage: screenshot-decode-probe IMAGE")?;
    let crop = if arguments.len() == 5 {
        let parse = |index: usize| -> Result<i32, Box<dyn std::error::Error>> {
            Ok(arguments[index].to_string_lossy().parse::<i32>()?)
        };
        Some(CaptureRegion::new(
            parse(1)?,
            parse(2)?,
            u32::try_from(parse(3)?)?,
            u32::try_from(parse(4)?)?,
        )?)
    } else if arguments.len() == 1 {
        None
    } else {
        return Err("usage: screenshot-decode-probe IMAGE [X Y WIDTH HEIGHT]".into());
    };
    let frame = WicScreenshotDecoder::new()?.decode(path, crop)?;
    let mask = build_blue_mask(&frame, BlueMaskSettings::default());
    let bands = PhysicalBandDetector::new().detect(&mask, BandDetectionSettings::default())?;
    let prepared_count = bands.bands.len() + usize::from(bands.fallback_crop.is_some());
    println!(
        "decoded {}x{}, fingerprint={:016X}, physical bands={}, fallback={}, prepared crops={prepared_count}",
        frame.width(),
        frame.height(),
        mask.fingerprint(),
        bands.bands.len(),
        bands.fallback_crop.is_some(),
    );
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("screenshot decoding is only available on Windows");
}
