use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use poe_alarm_ocr_win::{OcrLanguagePreference, OcrRecognition, OcrWorker, OwnedBgraImage};
#[cfg(windows)]
use poe_alarm_vision::{CaptureRegion, GdiScreenCapture, ScreenCapture};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("winrt-ocr-probe: {message}");
            eprintln!("Run with --help for usage.");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let Some(arguments) = Arguments::parse()? else {
        print_usage();
        return Ok(());
    };
    let worker = OcrWorker::start().map_err(|error| error.to_string())?;
    let capabilities = worker.capabilities().map_err(|error| error.to_string())?;
    println!(
        "Windows OCR maximum image dimension: {}",
        capabilities.maximum_image_dimension
    );
    println!(
        "Installed OCR languages: {}",
        if capabilities.available_language_tags.is_empty() {
            "none".to_owned()
        } else {
            capabilities.available_language_tags.join(", ")
        }
    );
    if arguments.list_only {
        return Ok(());
    }

    let input = arguments.load_input()?;
    println!(
        "Input: {} ({}x{}, stride {})",
        input.description, input.width, input.height, input.stride
    );
    let image = OwnedBgraImage::new(input.width, input.height, input.stride, input.pixels)
        .map_err(|error| error.to_string())?;

    let mut timings = Vec::with_capacity(arguments.iterations);
    let mut last_result = None;
    for _ in 0..arguments.iterations {
        let result = worker
            .recognize(arguments.language.clone(), image.clone())
            .map_err(|error| error.to_string())?;
        timings.push(result.elapsed);
        last_result = Some(result);
    }
    let result = last_result.expect("iterations are validated as positive");
    print_result(&result, &mut timings);
    Ok(())
}

fn print_result(result: &OcrRecognition, timings: &mut [Duration]) {
    timings.sort_unstable();
    let total_seconds = timings.iter().map(Duration::as_secs_f64).sum::<f64>();
    let mean_ms = total_seconds * 1_000.0 / timings.len() as f64;
    let p50 = timings[(timings.len() - 1) / 2].as_secs_f64() * 1_000.0;
    let p95_index = ((timings.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(timings.len() - 1);
    let p95 = timings[p95_index].as_secs_f64() * 1_000.0;
    println!(
        "Completed {} | {} run(s) | mean {mean_ms:.3} ms | p50 {p50:.3} ms | p95 {p95:.3} ms | {} line(s)",
        result.language_tag,
        timings.len(),
        result.lines.len()
    );
    for line in &result.lines {
        println!("{}", line.text);
    }
}

struct Arguments {
    language: OcrLanguagePreference,
    input: InputArgument,
    stride: Option<usize>,
    iterations: usize,
    list_only: bool,
}

enum InputArgument {
    Constructed,
    Raw {
        path: PathBuf,
        width: usize,
        height: usize,
    },
    Region {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
}

impl Arguments {
    fn parse() -> Result<Option<Self>, String> {
        let values = std::env::args().skip(1).collect::<Vec<_>>();
        let mut language = OcrLanguagePreference::English;
        let mut input = InputArgument::Constructed;
        let mut stride = None;
        let mut iterations = 1;
        let mut list_only = false;
        let mut index = 0;
        while index < values.len() {
            match values[index].as_str() {
                "--help" | "-h" => return Ok(None),
                "--list-languages" => list_only = true,
                "--language" => {
                    let value = take(&values, index + 1, "--language value")?;
                    language = match value.to_ascii_lowercase().as_str() {
                        "en" | "english" => OcrLanguagePreference::English,
                        "zh" | "zh-hant" | "traditional-chinese" => {
                            OcrLanguagePreference::TraditionalChinese
                        }
                        _ => OcrLanguagePreference::Exact(value.to_owned()),
                    };
                    index += 1;
                }
                "--bgra" => {
                    let path = PathBuf::from(take(&values, index + 1, "--bgra path")?);
                    let width = parse(take(&values, index + 2, "--bgra width")?, "width")?;
                    let height = parse(take(&values, index + 3, "--bgra height")?, "height")?;
                    input = InputArgument::Raw {
                        path,
                        width,
                        height,
                    };
                    index += 3;
                }
                "--stride" => {
                    stride = Some(parse(
                        take(&values, index + 1, "--stride bytes")?,
                        "stride",
                    )?);
                    index += 1;
                }
                "--region" => {
                    let x = parse(take(&values, index + 1, "--region x")?, "x")?;
                    let y = parse(take(&values, index + 2, "--region y")?, "y")?;
                    let width = parse(take(&values, index + 3, "--region width")?, "width")?;
                    let height = parse(take(&values, index + 4, "--region height")?, "height")?;
                    input = InputArgument::Region {
                        x,
                        y,
                        width,
                        height,
                    };
                    index += 4;
                }
                "--iterations" => {
                    iterations = parse(
                        take(&values, index + 1, "--iterations count")?,
                        "iterations",
                    )?;
                    if iterations == 0 {
                        return Err("iterations must be positive".to_owned());
                    }
                    index += 1;
                }
                unknown => return Err(format!("unknown argument {unknown}")),
            }
            index += 1;
        }
        Ok(Some(Self {
            language,
            input,
            stride,
            iterations,
            list_only,
        }))
    }

    fn load_input(&self) -> Result<InputPixels, String> {
        match &self.input {
            InputArgument::Constructed => {
                let width = 320;
                let height = 96;
                Ok(InputPixels {
                    width,
                    height,
                    stride: width * 4,
                    pixels: vec![255_u8; width * height * 4],
                    description: "constructed white BGRA buffer".to_owned(),
                })
            }
            InputArgument::Raw {
                path,
                width,
                height,
            } => {
                let pixels = std::fs::read(path)
                    .map_err(|error| format!("read {}: {error}", path.display()))?;
                Ok(InputPixels {
                    width: *width,
                    height: *height,
                    stride: self.stride.unwrap_or(width.saturating_mul(4)),
                    pixels,
                    description: format!("raw BGRA file {}", path.display()),
                })
            }
            InputArgument::Region {
                x,
                y,
                width,
                height,
            } => capture_region(*x, *y, *width, *height),
        }
    }
}

struct InputPixels {
    width: usize,
    height: usize,
    stride: usize,
    pixels: Vec<u8>,
    description: String,
}

#[cfg(windows)]
fn capture_region(x: i32, y: i32, width: u32, height: u32) -> Result<InputPixels, String> {
    let region = CaptureRegion::new(x, y, width, height).map_err(|error| error.to_string())?;
    let frame = GdiScreenCapture::new()
        .capture(region)
        .map_err(|error| error.to_string())?;
    Ok(InputPixels {
        width: frame.width(),
        height: frame.height(),
        stride: frame.stride(),
        pixels: frame.bgra_pixels().to_vec(),
        description: format!("desktop region {x},{y} {width}x{height}"),
    })
}

#[cfg(not(windows))]
fn capture_region(_x: i32, _y: i32, _width: u32, _height: u32) -> Result<InputPixels, String> {
    Err("desktop region capture is only available on Windows".to_owned())
}

fn take<'a>(values: &'a [String], index: usize, expected: &str) -> Result<&'a str, String> {
    values
        .get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("missing {expected}"))
}

fn parse<T>(value: &str, name: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|error| format!("invalid {name} {value}: {error}"))
}

fn print_usage() {
    println!(
        "Usage:\n  winrt-ocr-probe [--language en|zh-hant|TAG] [--iterations N]\n  winrt-ocr-probe --bgra FILE WIDTH HEIGHT [--stride BYTES] [--language ...]\n  winrt-ocr-probe --region X Y WIDTH HEIGHT [--language ...]\n  winrt-ocr-probe --list-languages\n\nRaw input must be top-down BGRA8. With no input, the probe runs real OCR on a constructed white buffer."
    );
}
