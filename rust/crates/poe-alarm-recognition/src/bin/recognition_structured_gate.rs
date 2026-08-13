#[cfg(windows)]
mod common;

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("recognition-structured-gate: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("recognition-structured-gate requires Windows.Media.Ocr and WIC");
    std::process::exit(2);
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct Manifest {
    #[serde(default)]
    roi: Option<[i64; 4]>,
    screenshots: Vec<Screenshot>,
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
struct Screenshot {
    image: String,
    #[serde(default)]
    roi: Option<[i64; 4]>,
    cases: Vec<Case>,
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Case {
    Template(String),
    Detailed { template: String },
}

#[cfg(windows)]
impl Case {
    fn template(&self) -> &str {
        match self {
            Self::Template(value) | Self::Detailed { template: value } => value,
        }
    }
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    use std::collections::HashSet;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use poe_alarm_core::FullLineAffixMatcher;
    use poe_alarm_recognition::ProductionRecognizer;
    use poe_alarm_vision::{CaptureRegion, WicScreenshotDecoder};

    fn manifest_roi(values: [i64; 4]) -> Result<CaptureRegion, String> {
        common::parse_roi(&format!(
            "{},{},{},{}",
            values[0], values[1], values[2], values[3]
        ))
    }

    fn find_image(
        image_root: &Path,
        manifest_path: &Path,
        image_name: &str,
    ) -> Result<PathBuf, String> {
        let section = manifest_path
            .parent()
            .and_then(Path::file_name)
            .map(PathBuf::from);
        section
            .into_iter()
            .map(|section| image_root.join(section).join(image_name))
            .chain(std::iter::once(image_root.join(image_name)))
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| format!("missing private screenshot {image_name}"))
    }

    fn csv_field(value: &str) -> String {
        format!("\"{}\"", value.replace('"', "\"\""))
    }

    let arguments = common::Arguments::collect();
    let manifest_path = Path::new(arguments.required("--manifest")?);
    let image_root = Path::new(arguments.required("--image-root")?);
    let output_path = Path::new(arguments.required("--csv")?);
    let profile = common::parse_profile(
        arguments.required("--game")?,
        arguments.required("--language")?,
    )?;
    let repeats = arguments
        .value("--repeats")
        .unwrap_or("5")
        .parse::<usize>()
        .map_err(|error| format!("invalid --repeats: {error}"))?;
    if repeats < 5 {
        return Err("--repeats must be at least 5".to_owned());
    }
    let paddle = common::paddle_configuration(&arguments)?
        .ok_or_else(|| "--onnx-runtime is required for this release gate".to_owned())?;
    let manifest: Manifest = serde_json::from_str(
        &fs::read_to_string(manifest_path)
            .map_err(|error| format!("could not read {}: {error}", manifest_path.display()))?,
    )
    .map_err(|error| format!("invalid manifest: {error}"))?;
    let all_templates = manifest
        .screenshots
        .iter()
        .flat_map(|screenshot| &screenshot.cases)
        .map(Case::template)
        .collect::<Vec<_>>();
    let selected = manifest
        .screenshots
        .iter()
        .filter(|screenshot| (4..=6).contains(&screenshot.cases.len()))
        .take(3)
        .collect::<Vec<_>>();
    if selected.len() < 3 {
        return Err("manifest needs three real tooltips with 4-6 expected modifiers".to_owned());
    }

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let mut output = std::io::BufWriter::new(
        fs::File::create(output_path)
            .map_err(|error| format!("could not create {}: {error}", output_path.display()))?,
    );
    writeln!(
        output,
        "image,condition_count,positive_count,round,wall_ms,confirmation_ran,positive_misses,false_alerts"
    )
    .map_err(|error| error.to_string())?;

    let decoder = WicScreenshotDecoder::new().map_err(|error| error.to_string())?;
    let mut failures = Vec::new();
    for screenshot in selected {
        let image = find_image(image_root, manifest_path, &screenshot.image)?;
        let roi = screenshot
            .roi
            .or(manifest.roi)
            .map(manifest_roi)
            .transpose()?;
        let frame = common::decode(&decoder, &image, roi)?;
        let actual = screenshot
            .cases
            .iter()
            .map(Case::template)
            .collect::<Vec<_>>();
        let actual_set = actual.iter().copied().collect::<HashSet<_>>();
        for condition_count in [2_usize, 4, 10, 20] {
            let positive_count = actual.len().min(condition_count).min(6);
            let mut templates = actual[..positive_count].to_vec();
            for candidate in &all_templates {
                if templates.len() == condition_count {
                    break;
                }
                if !actual_set.contains(candidate) && !templates.contains(candidate) {
                    templates.push(candidate);
                }
            }
            if templates.len() != condition_count {
                return Err(format!(
                    "could only assemble {} of {condition_count} conditions",
                    templates.len()
                ));
            }
            let targets = templates
                .iter()
                .map(|template| {
                    FullLineAffixMatcher::new(template).map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut recognizer = ProductionRecognizer::start(profile, Some(paddle.clone()))
                .map_err(|error| error.to_string())?;
            for round in 1..=repeats {
                let started = Instant::now();
                let sequence =
                    common::recognize_structured_sequence(&mut recognizer, &frame, &targets)?;
                let wall_ms = started.elapsed().as_secs_f64() * 1_000.0;
                let confirmation_ran = sequence.confirmation.is_some();
                let result = sequence.final_result_ref();
                let positive_misses = targets[..positive_count]
                    .iter()
                    .filter(|target| !common::strict_target_present(result, target))
                    .count();
                let false_alerts = targets[positive_count..]
                    .iter()
                    .filter(|target| common::strict_target_present(result, target))
                    .count();
                writeln!(
                    output,
                    "{},{condition_count},{positive_count},{round},{wall_ms:.6},{confirmation_ran},{positive_misses},{false_alerts}",
                    csv_field(&screenshot.image)
                )
                .map_err(|error| error.to_string())?;
                if positive_misses != 0 || false_alerts != 0 {
                    failures.push(format!(
                        "{} conditions={condition_count} round={round}: misses={positive_misses}, false={false_alerts}",
                        screenshot.image
                    ));
                }
            }
        }
    }
    if !failures.is_empty() {
        return Err(format!(
            "structured production gate failed:\n{}",
            failures.join("\n")
        ));
    }
    println!(
        "structured production gate passed: 3 real tooltips x 4 condition counts x {repeats} rounds"
    );
    Ok(())
}
