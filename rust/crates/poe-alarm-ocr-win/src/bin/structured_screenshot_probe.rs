#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Instant;

    use poe_alarm_core::{
        AcceptableResultGroup, AffixCondition, CompiledRuleSet, Decimal, NumericConstraint,
        ResultGroupMode, RuleSetDefinition,
    };
    use poe_alarm_ocr_win::{OcrLanguagePreference, OcrWorker, OwnedBgraImage};
    use poe_alarm_vision::{
        BgraCropBuffer, BlueMaskSettings, CaptureRegion, WicScreenshotDecoder, build_blue_mask,
        combined_crop_metadata,
    };

    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.len() != 7 && arguments.len() != 8 {
        return Err(
            "usage: structured-screenshot-probe IMAGE X Y WIDTH HEIGHT TEMPLATE MINIMUM [ITERATIONS]"
                .into(),
        );
    }
    let parse_i32 = |index: usize| -> Result<i32, Box<dyn std::error::Error>> {
        Ok(arguments[index].parse::<i32>()?)
    };
    let region = CaptureRegion::new(
        parse_i32(1)?,
        parse_i32(2)?,
        u32::try_from(parse_i32(3)?)?,
        u32::try_from(parse_i32(4)?)?,
    )?;
    let frame = WicScreenshotDecoder::new()?.decode(&arguments[0], Some(region))?;
    let mask_started = Instant::now();
    let mask = build_blue_mask(&frame, BlueMaskSettings::default());
    let metadata = combined_crop_metadata(&mask, 8)?;
    let mut crop = BgraCropBuffer::default();
    crop.prepare(&mask, &metadata)?;
    let preprocessing_elapsed = mask_started.elapsed();

    let input = OwnedBgraImage::new(
        crop.width(),
        crop.height(),
        crop.stride(),
        crop.bgra_pixels().to_vec(),
    )?;
    let iterations = arguments
        .get(7)
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(1);
    if iterations == 0 {
        return Err("iterations must be positive".into());
    }
    let worker = OcrWorker::start()?;
    let mut ocr_samples = Vec::with_capacity(iterations);
    let mut ocr = None;
    for _ in 0..iterations {
        let result = worker.recognize(OcrLanguagePreference::TraditionalChinese, input.clone())?;
        ocr_samples.push(result.elapsed);
        ocr = Some(result);
    }
    ocr_samples.sort_unstable();
    let ocr = ocr.expect("iterations are positive");
    let lines = create_logical_lines(&ocr.lines);
    let rules = CompiledRuleSet::compile(RuleSetDefinition {
        schema_version: 1,
        name: "screenshot numeric probe".to_owned(),
        groups: vec![AcceptableResultGroup {
            name: "result".to_owned(),
            mode: ResultGroupMode::Any,
            required_count: 1,
            conditions: vec![AffixCondition::new(
                "target",
                &arguments[5],
                vec![NumericConstraint::at_least(
                    arguments[6].parse::<Decimal>()?,
                )],
            )],
        }],
    })?;
    let evaluation = rules.evaluate(&lines);
    println!(
        "preprocess={:.3}ms ocr-last={:.3}ms language={} lines={} match={}",
        preprocessing_elapsed.as_secs_f64() * 1_000.0,
        ocr.elapsed.as_secs_f64() * 1_000.0,
        ocr.language_tag,
        lines.len(),
        evaluation.is_match,
    );
    if iterations > 1 {
        let p50 = ocr_samples[(iterations - 1) / 2].as_secs_f64() * 1_000.0;
        let p95 = ocr_samples[((iterations as f64 * 0.95).ceil() as usize - 1).min(iterations - 1)]
            .as_secs_f64()
            * 1_000.0;
        let mean = ocr_samples
            .iter()
            .map(|value| value.as_secs_f64())
            .sum::<f64>()
            * 1_000.0
            / iterations as f64;
        println!("ocr runs={iterations} mean={mean:.3}ms p50={p50:.3}ms p95={p95:.3}ms");
    }
    for (index, line) in lines.iter().enumerate() {
        println!("[{index}] {line}");
    }
    if let Some(condition) = evaluation.matched_group().and_then(|group| {
        group
            .conditions
            .iter()
            .find(|condition| condition.is_matched)
    }) {
        println!(
            "matched={} values={:?}",
            condition
                .observation
                .as_ref()
                .map(|value| value.original_text.as_str())
                .unwrap_or_default(),
            condition
                .observation
                .as_ref()
                .map(|value| value.numeric_values.as_slice())
                .unwrap_or_default(),
        );
    }
    if evaluation.is_match {
        Ok(())
    } else {
        Err("structured rule did not match".into())
    }
}

#[cfg(windows)]
fn create_logical_lines(lines: &[poe_alarm_ocr_win::RecognizedLine]) -> Vec<String> {
    let mut result = Vec::with_capacity(lines.len() * 2);
    let mut previous: Option<(f32, f32, f32)> = None;
    for line in lines {
        let text = line.text.trim();
        if text.is_empty() || line.width <= 0.0 || line.height <= 0.0 {
            continue;
        }
        let top = line.top;
        let bottom = line.top + line.height;
        let height = bottom - top;
        if let Some((previous_top, previous_bottom, previous_height)) = previous {
            let gap = top - previous_bottom;
            if top < previous_top || gap > 12.0_f32.max(previous_height.max(height) * 0.9) {
                result.push(String::new());
            }
        }
        result.push(normalize_chinese_numeric_dash(text));
        previous = Some((top, bottom, height));
    }
    result
}

#[cfg(windows)]
fn normalize_chinese_numeric_dash(text: &str) -> String {
    let mut characters = text.chars().collect::<Vec<_>>();
    for index in 0..characters.len() {
        if characters[index] != '一' {
            continue;
        }
        let previous = (0..index)
            .rev()
            .find(|candidate| !characters[*candidate].is_whitespace());
        let next =
            (index + 1..characters.len()).find(|candidate| !characters[*candidate].is_whitespace());
        if previous.is_some_and(|value| characters[value].is_ascii_digit())
            && next.is_some_and(|value| characters[value].is_ascii_digit())
        {
            characters[index] = '-';
        }
    }
    characters.into_iter().collect()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("structured screenshot OCR is only available on Windows");
}
