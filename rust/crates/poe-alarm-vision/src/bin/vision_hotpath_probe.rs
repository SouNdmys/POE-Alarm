use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use poe_alarm_vision::{
    BandDetection, BandDetectionSettings, BgraCropBuffer, BlueMaskSettings, BlueTextMask,
    CaptureRegion, CapturedFrame, PhysicalBandDetector, build_blue_mask_into,
};

fn main() {
    let options = Options::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    let frame = synthetic_tooltip(options.width, options.height);
    let mut mask = BlueTextMask::default();
    let mut detector = PhysicalBandDetector::new();
    let mut crop_buffers = Vec::new();

    for _ in 0..options.warmup {
        build_blue_mask_into(&frame, BlueMaskSettings::default(), &mut mask);
        let detection = detector
            .detect(&mask, BandDetectionSettings::default())
            .expect("synthetic mask is valid");
        prepare_crops(&mask, &detection, &mut crop_buffers);
        black_box(detection);
    }

    let mut mask_samples = Vec::with_capacity(options.iterations);
    let mut full_samples = Vec::with_capacity(options.iterations);
    let mut observed_bands = 0;
    for _ in 0..options.iterations {
        let mask_started = Instant::now();
        build_blue_mask_into(&frame, BlueMaskSettings::default(), &mut mask);
        let mask_elapsed = mask_started.elapsed().as_nanos() as u64;
        let detection = detector
            .detect(&mask, BandDetectionSettings::default())
            .expect("synthetic mask is valid");
        prepare_crops(&mask, &detection, &mut crop_buffers);
        let full_elapsed = mask_started.elapsed().as_nanos() as u64;
        observed_bands = detection.bands.len();
        black_box(mask.fingerprint());
        black_box(detection);
        mask_samples.push(mask_elapsed);
        full_samples.push(full_elapsed);
    }
    if let Some(path) = &options.csv_path {
        write_csv(
            path,
            &mask_samples,
            &full_samples,
            frame.width(),
            frame.height(),
            mask.fingerprint(),
        )
        .unwrap_or_else(|error| {
            eprintln!("could not write {}: {error}", path.display());
            std::process::exit(1);
        });
    }
    mask_samples.sort_unstable();
    full_samples.sort_unstable();
    let pixels = frame.width() * frame.height();
    println!(
        "vision hot path: {} iterations, {}x{}, {observed_bands} prepared bands, fingerprint={:016X}",
        options.iterations,
        frame.width(),
        frame.height(),
        mask.fingerprint(),
    );
    print_samples("mask+fingerprint", &mask_samples, pixels);
    print_samples("mask+fingerprint+bands+BGRA crops", &full_samples, pixels);
}

fn write_csv(
    path: &std::path::Path,
    mask_samples: &[u64],
    full_samples: &[u64],
    width: usize,
    height: usize,
    fingerprint: u64,
) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut output = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(
        output,
        "implementation,iteration,metric,nanoseconds,width,height,fingerprint"
    )?;
    for (iteration, (mask, full)) in mask_samples.iter().zip(full_samples).enumerate() {
        writeln!(
            output,
            "rust,{iteration},mask+fingerprint,{mask},{width},{height},{fingerprint:016X}"
        )?;
        writeln!(
            output,
            "rust,{iteration},mask+fingerprint+bands+BGRA crops,{full},{width},{height},{fingerprint:016X}"
        )?;
    }
    Ok(())
}

fn prepare_crops(
    mask: &BlueTextMask,
    detection: &BandDetection,
    buffers: &mut Vec<BgraCropBuffer>,
) {
    let required = detection.bands.len() + usize::from(detection.fallback_crop.is_some());
    buffers.resize_with(required, BgraCropBuffer::default);
    for (slot, band) in detection.bands.iter().enumerate() {
        buffers[slot]
            .prepare(mask, &band.crop)
            .expect("detected crop is valid");
        black_box(buffers[slot].bgra_pixels());
    }
    if let Some(fallback) = &detection.fallback_crop {
        buffers[required - 1]
            .prepare(mask, fallback)
            .expect("fallback crop is valid");
        black_box(buffers[required - 1].bgra_pixels());
    }
}

fn print_samples(label: &str, samples: &[u64], pixels: usize) {
    let percentile = |ratio: f64| {
        let index = ((samples.len() - 1) as f64 * ratio).round() as usize;
        samples[index] as f64
    };
    let mean = samples.iter().map(|value| *value as f64).sum::<f64>() / samples.len() as f64;
    println!(
        "{label}: mean={:.3} us p50={:.3} us p95={:.3} us p99={:.3} us ({:.3} ns/pixel mean)",
        mean / 1_000.0,
        percentile(0.50) / 1_000.0,
        percentile(0.95) / 1_000.0,
        percentile(0.99) / 1_000.0,
        mean / pixels as f64,
    );
}

#[derive(Clone)]
struct Options {
    width: usize,
    height: usize,
    iterations: usize,
    warmup: usize,
    csv_path: Option<PathBuf>,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            width: 720,
            height: 520,
            iterations: 500,
            warmup: 20,
            csv_path: None,
        };
        let mut arguments = arguments.peekable();
        while let Some(argument) = arguments.next() {
            if argument == "--csv" {
                options.csv_path = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--csv requires a path".to_owned())?,
                ));
                continue;
            }
            let target = match argument.as_str() {
                "--width" => &mut options.width,
                "--height" => &mut options.height,
                "--iterations" => &mut options.iterations,
                "--warmup" => &mut options.warmup,
                _ => return Err(format!("unknown argument: {argument}")),
            };
            let value = arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a positive integer"))?;
            *target = value
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| format!("{argument} requires a positive integer"))?;
        }
        if options.width < 640 || options.height < 400 {
            return Err("width must be at least 640 and height at least 400".to_owned());
        }
        Ok(options)
    }
}

fn synthetic_tooltip(width: usize, height: usize) -> CapturedFrame {
    let stride = width * 4;
    let mut pixels = vec![0_u8; stride * height];
    for y in 0..height {
        for x in 0..width {
            let pixel = y * stride + x * 4;
            pixels[pixel] = ((x * 5 + y * 3) % 96) as u8;
            pixels[pixel + 1] = ((x * 2 + y * 7) % 80) as u8;
            pixels[pixel + 2] = ((x * 3 + y * 2) % 80) as u8;
            pixels[pixel + 3] = 255;
        }
    }
    for line in 0..8 {
        let top = 70 + line * 42;
        for y in top..top + 18 {
            for x in (80..620).step_by(5) {
                let pixel = y * stride + x * 4;
                pixels[pixel] = 210;
                pixels[pixel + 1] = 125;
                pixels[pixel + 2] = 110;
            }
        }
    }
    CapturedFrame::from_bgra(
        CaptureRegion::new(100, 80, width as u32, height as u32).expect("valid dimensions"),
        stride,
        pixels,
        SystemTime::now(),
    )
    .expect("valid synthetic frame")
}
