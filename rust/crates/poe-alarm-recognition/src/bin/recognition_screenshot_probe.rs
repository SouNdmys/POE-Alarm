#[cfg(windows)]
mod common;

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("recognition-screenshot-probe: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("recognition-screenshot-probe requires Windows.Media.Ocr and WIC");
    std::process::exit(2);
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    use std::path::Path;
    use std::str::FromStr;

    use poe_alarm_core::{
        AcceptableResultGroup, AffixCondition, CompiledRuleSet, Decimal, FullLineAffixMatcher,
        NumericConstraint, ResultGroupMode, RuleSetDefinition,
    };
    use poe_alarm_recognition::ProductionRecognizer;
    use poe_alarm_vision::WicScreenshotDecoder;

    let arguments = common::Arguments::collect();
    if arguments.has("--help") {
        println!(
            "usage: recognition-screenshot-probe --image PATH [--roi x,y,w,h] \\\n+             --game poe1|poe2 --language en|zh-TW --template TEXT [--minimum VALUE] \\\n+             [--onnx-runtime PATH --model PATH --dictionary PATH]"
        );
        return Ok(());
    }
    let image = Path::new(arguments.required("--image")?);
    let profile = common::parse_profile(
        arguments.required("--game")?,
        arguments.required("--language")?,
    )?;
    let template = arguments.required("--template")?;
    let roi = arguments
        .value("--roi")
        .map(common::parse_roi)
        .transpose()?;
    let decoder = WicScreenshotDecoder::new().map_err(|error| error.to_string())?;
    let frame = common::decode(&decoder, image, roi)?;
    let target = FullLineAffixMatcher::new(template).map_err(|error| error.to_string())?;
    let mut recognizer =
        ProductionRecognizer::start(profile, common::paddle_configuration(&arguments)?)
            .map_err(|error| error.to_string())?;
    let result = common::recognize_structured(&mut recognizer, &frame, &[target.clone()])?;
    let lexical_evidence = target.find_match(&result.lines);
    let lexical_match = common::strict_target_present(&result, &target);
    let rule_match = if let Some(minimum) = arguments.value("--minimum") {
        let minimum = Decimal::from_str(minimum).map_err(|error| error.to_string())?;
        let rules = CompiledRuleSet::compile(RuleSetDefinition {
            schema_version: 1,
            name: "screenshot-probe".to_owned(),
            groups: vec![AcceptableResultGroup {
                name: "expected".to_owned(),
                mode: ResultGroupMode::All,
                required_count: 1,
                conditions: vec![AffixCondition::new(
                    "target",
                    template,
                    vec![NumericConstraint::at_least(minimum)],
                )],
            }],
        })
        .map_err(|error| error.to_string())?;
        Some(
            rules
                .evaluate_with_identity(
                    &result.lines,
                    &result.assisted_observations,
                    &result.physical_line_identities,
                )
                .is_match,
        )
    } else {
        None
    };
    let expected_rule_match = arguments
        .value("--expect-rule-match")
        .map(|value| match value {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err("--expect-rule-match must be true or false".to_owned()),
        })
        .transpose()?
        .unwrap_or(true);

    println!("image={}x{}", frame.width(), frame.height());
    println!(
        "preprocess_ms={:.3} ocr_ms={:.3} cached={} rescan={}",
        result.preprocessing_elapsed.as_secs_f64() * 1_000.0,
        result.recognition_elapsed.as_secs_f64() * 1_000.0,
        result.was_cached,
        result.requires_rescan
    );
    for (index, line) in result.lines.iter().enumerate() {
        println!("line[{index}]={line}");
    }
    if let Some(evidence) = lexical_evidence {
        println!(
            "strict_start_line={} strict_physical_lines={} strict_original_text={}",
            evidence.start_line_index, evidence.physical_line_count, evidence.original_text
        );
    }
    println!("lexical_match={lexical_match}");
    if let Some(rule_match) = rule_match {
        println!("numeric_rule_match={rule_match}");
    }
    if !lexical_match {
        return Err("target text was not accepted".to_owned());
    }
    if rule_match.is_some_and(|actual| actual != expected_rule_match) {
        return Err(format!(
            "numeric rule result was {rule_match:?}, expected {expected_rule_match}"
        ));
    }
    Ok(())
}
