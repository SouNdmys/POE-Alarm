#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use poe_alarm_core::FullLineAffixMatcher;
use poe_alarm_recognition::{
    GameVersion, PaddleBackendConfig, ProductionRecognizer, RecognitionLanguage, RecognitionProfile,
};
use poe_alarm_vision::{
    BlueMaskSettings, CaptureRegion, CapturedFrame, WicScreenshotDecoder, build_blue_mask,
};

pub struct Arguments {
    values: Vec<String>,
}

impl Arguments {
    pub fn collect() -> Self {
        Self {
            values: std::env::args().skip(1).collect(),
        }
    }

    pub fn has(&self, name: &str) -> bool {
        self.values.iter().any(|value| value == name)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .position(|value| value == name)
            .and_then(|index| self.values.get(index + 1))
            .map(String::as_str)
    }

    pub fn required(&self, name: &str) -> Result<&str, String> {
        self.value(name)
            .ok_or_else(|| format!("missing required option {name}"))
    }
}

pub fn parse_profile(game: &str, language: &str) -> Result<RecognitionProfile, String> {
    let game = match game.to_ascii_lowercase().as_str() {
        "poe1" | "1" => GameVersion::Poe1,
        "poe2" | "2" => GameVersion::Poe2,
        value => return Err(format!("unknown game '{value}'; expected poe1 or poe2")),
    };
    let language = match language.to_ascii_lowercase().as_str() {
        "en" | "en-us" | "english" => RecognitionLanguage::English,
        "zh" | "zh-tw" | "traditional" | "traditional-chinese" => {
            RecognitionLanguage::TraditionalChinese
        }
        value => {
            return Err(format!("unknown language '{value}'; expected en or zh-TW"));
        }
    };
    Ok(RecognitionProfile { game, language })
}

pub fn parse_roi(value: &str) -> Result<CaptureRegion, String> {
    let numbers = value
        .split(',')
        .map(str::trim)
        .map(str::parse::<i64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("invalid --roi '{value}': {error}"))?;
    if numbers.len() != 4 {
        return Err("--roi must be x,y,width,height".to_owned());
    }
    let x = i32::try_from(numbers[0]).map_err(|_| "ROI x is outside i32".to_owned())?;
    let y = i32::try_from(numbers[1]).map_err(|_| "ROI y is outside i32".to_owned())?;
    let width = u32::try_from(numbers[2]).map_err(|_| "ROI width must be positive".to_owned())?;
    let height = u32::try_from(numbers[3]).map_err(|_| "ROI height must be positive".to_owned())?;
    CaptureRegion::new(x, y, width, height).map_err(|error| error.to_string())
}

pub fn decode(
    decoder: &WicScreenshotDecoder,
    image: &Path,
    roi: Option<CaptureRegion>,
) -> Result<CapturedFrame, String> {
    decoder
        .decode(image, roi)
        .map_err(|error| error.to_string())
}

pub fn paddle_configuration(arguments: &Arguments) -> Result<Option<PaddleBackendConfig>, String> {
    let Some(runtime) = arguments.value("--onnx-runtime") else {
        return Ok(None);
    };
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let asset_root = source_root.join("assets/ocr");
    let model = arguments
        .value("--model")
        .map(PathBuf::from)
        .unwrap_or_else(|| asset_root.join("PP-OCRv5_mobile_rec.onnx"));
    let dictionary = arguments
        .value("--dictionary")
        .map(PathBuf::from)
        .unwrap_or_else(|| asset_root.join("ppocrv5_dict.txt"));
    let mut config = PaddleBackendConfig::new(runtime, model, dictionary);
    if let Some(threads) = arguments.value("--paddle-threads") {
        config.threads = threads
            .parse()
            .map_err(|error| format!("invalid --paddle-threads: {error}"))?;
    }
    Ok(Some(config))
}

pub fn recognize_structured(
    recognizer: &mut ProductionRecognizer,
    frame: &CapturedFrame,
    targets: &[FullLineAffixMatcher],
) -> Result<poe_alarm_monitoring::RecognitionResult, String> {
    Ok(recognize_structured_sequence(recognizer, frame, targets)?.final_result())
}

pub struct RecognitionSequence {
    pub primary: poe_alarm_monitoring::RecognitionResult,
    pub confirmation: Option<poe_alarm_monitoring::RecognitionResult>,
    pub primary_wall: Duration,
    pub confirmation_wall: Duration,
}

impl RecognitionSequence {
    pub fn final_result(self) -> poe_alarm_monitoring::RecognitionResult {
        self.confirmation.unwrap_or(self.primary)
    }

    pub fn final_result_ref(&self) -> &poe_alarm_monitoring::RecognitionResult {
        self.confirmation.as_ref().unwrap_or(&self.primary)
    }

    pub fn total_wall(&self) -> Duration {
        self.primary_wall.saturating_add(self.confirmation_wall)
    }
}

pub fn recognize_structured_sequence(
    recognizer: &mut ProductionRecognizer,
    frame: &CapturedFrame,
    targets: &[FullLineAffixMatcher],
) -> Result<RecognitionSequence, String> {
    let primary_started = Instant::now();
    let mask = build_blue_mask(frame, BlueMaskSettings::default());
    let primary = recognizer
        .recognize_structured_prepared(frame, &mask, targets)
        .map_err(|error| error.to_string())?;
    let primary_wall = primary_started.elapsed();
    let (confirmation, confirmation_wall) = if primary.requires_rescan {
        let confirmation_started = Instant::now();
        let confirmation = recognizer
            .recognize_structured_prepared(frame, &mask, targets)
            .map_err(|error| error.to_string())?;
        (Some(confirmation), confirmation_started.elapsed())
    } else {
        (None, Duration::ZERO)
    };
    Ok(RecognitionSequence {
        primary,
        confirmation,
        primary_wall,
        confirmation_wall,
    })
}

pub fn recognize_quick(
    recognizer: &mut ProductionRecognizer,
    frame: &CapturedFrame,
    target: &FullLineAffixMatcher,
) -> Result<poe_alarm_monitoring::RecognitionResult, String> {
    Ok(recognize_quick_sequence(recognizer, frame, target)?.final_result())
}

pub fn recognize_quick_sequence(
    recognizer: &mut ProductionRecognizer,
    frame: &CapturedFrame,
    target: &FullLineAffixMatcher,
) -> Result<RecognitionSequence, String> {
    let primary_started = Instant::now();
    let mask = build_blue_mask(frame, BlueMaskSettings::default());
    let primary = recognizer
        .recognize_quick_prepared(frame, &mask, target)
        .map_err(|error| error.to_string())?;
    let primary_wall = primary_started.elapsed();
    let (confirmation, confirmation_wall) = if primary.requires_rescan {
        let confirmation_started = Instant::now();
        let confirmation = recognizer
            .recognize_quick_prepared(frame, &mask, target)
            .map_err(|error| error.to_string())?;
        (Some(confirmation), confirmation_started.elapsed())
    } else {
        (None, Duration::ZERO)
    };
    Ok(RecognitionSequence {
        primary,
        confirmation,
        primary_wall,
        confirmation_wall,
    })
}

pub fn strict_target_present(
    result: &poe_alarm_monitoring::RecognitionResult,
    target: &FullLineAffixMatcher,
) -> bool {
    target.find_match(&result.lines).is_some()
        || result
            .target_assisted_match
            .as_ref()
            .is_some_and(|matched| matched.canonical_text == target.template().text)
        || result
            .assisted_observations
            .iter()
            .any(|observation| observation.canonical_target == target.template().text)
}
