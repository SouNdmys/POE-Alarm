use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ort::logging::LogLevel;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{TensorElementType, TensorRef, ValueType};
use poe_alarm_core::FullLineAffixMatcher;

use crate::assets::EXPECTED_CLASS_COUNT;
use crate::preprocess::{
    InferenceWorkspace, MODEL_HEIGHT, calculate_resized_width, calculate_tensor_width, fill_tensor,
    find_ink_crop, split_crop,
};
use crate::target_support::{CtcBatchRecognition, CtcTargetRecognition, TargetSupportAnalyzer};
use crate::{ImageView, PaddleAssets, PaddleError};

const EXPECTED_INPUT_SHAPE: [i64; 4] = [-1, 3, 48, -1];
const EXPECTED_OUTPUT_SHAPE: [i64; 3] = [-1, -1, EXPECTED_CLASS_COUNT as i64];

/// Whether nonzero mask pixels represent foreground or background text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextPolarity {
    BrightOnDark,
    DarkOnLight,
}

/// Runtime and preprocessing settings equivalent to .NET `PaddleCtcSession`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaddleSessionConfig {
    pub threads: usize,
    pub polarity: TextPolarity,
    pub ink_margin: usize,
    pub right_padding: usize,
    pub maximum_segment_width: Option<usize>,
    pub allow_latin_target_support: bool,
}

impl Default for PaddleSessionConfig {
    fn default() -> Self {
        Self {
            threads: 2,
            polarity: TextPolarity::BrightOnDark,
            ink_margin: 2,
            right_padding: 0,
            maximum_segment_width: None,
            allow_latin_target_support: false,
        }
    }
}

impl PaddleSessionConfig {
    pub fn validate(&self) -> Result<(), PaddleError> {
        if !(1..=16).contains(&self.threads) {
            return Err(PaddleError::InvalidConfiguration(format!(
                "threads must be in 1..=16; got {}",
                self.threads
            )));
        }
        if self.ink_margin > 8 {
            return Err(PaddleError::InvalidConfiguration(format!(
                "ink margin must be in 0..=8; got {}",
                self.ink_margin
            )));
        }
        if self.right_padding > 128 {
            return Err(PaddleError::InvalidConfiguration(format!(
                "right padding must be in 0..=128; got {}",
                self.right_padding
            )));
        }
        if let Some(width) = self.maximum_segment_width
            && !(320..=1_600).contains(&width)
        {
            return Err(PaddleError::InvalidConfiguration(format!(
                "maximum segment width must be in 320..=1600; got {width}"
            )));
        }
        Ok(())
    }
}

/// Static names and shapes read from the loaded official model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelContract {
    pub input_name: String,
    pub output_name: String,
    pub input_shape: Vec<i64>,
    pub output_shape: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CtcEmission {
    pub text: String,
    pub time_step: usize,
    pub probability: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CtcRecognition {
    pub text: String,
    pub mean_confidence: f64,
    pub emissions: Vec<CtcEmission>,
    pub tensor_width: usize,
    pub time_steps: usize,
    pub class_count: usize,
    pub segment_count: usize,
    pub tensor_preparation_elapsed: Duration,
    pub inference_elapsed: Duration,
    pub decode_elapsed: Duration,
}

impl CtcRecognition {
    pub fn total_elapsed(&self) -> Duration {
        self.tensor_preparation_elapsed + self.inference_elapsed + self.decode_elapsed
    }
}

/// A serial, allocation-reusing session designed to live on one OCR worker thread.
///
/// `ort` 2.x deliberately requires `&mut Session` for inference. Keeping this
/// wrapper owned by one worker makes the native ownership rule explicit and
/// retains the largest input tensor for reuse without a global lock.
pub struct PaddleCtcSession {
    session: Session,
    dictionary: Arc<[String]>,
    contract: ModelContract,
    config: PaddleSessionConfig,
    workspace: InferenceWorkspace,
    target_support_analyzer: TargetSupportAnalyzer,
    session_creation_elapsed: Duration,
}

impl PaddleCtcSession {
    pub fn from_assets(
        assets: &PaddleAssets,
        config: PaddleSessionConfig,
    ) -> Result<Self, PaddleError> {
        config.validate()?;
        let started = Instant::now();
        let session = Session::builder()?
            .with_intra_threads(config.threads)?
            .with_inter_threads(1)?
            .with_parallel_execution(false)?
            .with_optimization_level(GraphOptimizationLevel::All)?
            .with_memory_pattern(true)?
            .with_config_entry("session.intra_op.allow_spinning", "0")?
            .with_log_level(LogLevel::Error)?
            .commit_from_memory(assets.model())?;
        let session_creation_elapsed = started.elapsed();
        let contract = validate_model_contract(&session)?;
        let dictionary: Arc<[String]> = Arc::from(assets.dictionary());
        let target_support_analyzer =
            TargetSupportAnalyzer::new(&dictionary, config.allow_latin_target_support);
        Ok(Self {
            session,
            dictionary,
            contract,
            config,
            workspace: InferenceWorkspace::default(),
            target_support_analyzer,
            session_creation_elapsed,
        })
    }

    pub fn contract(&self) -> &ModelContract {
        &self.contract
    }

    pub fn dictionary_size(&self) -> usize {
        self.dictionary.len()
    }

    pub fn session_creation_elapsed(&self) -> Duration {
        self.session_creation_elapsed
    }

    pub fn config(&self) -> &PaddleSessionConfig {
        &self.config
    }

    /// Capacity retained by the compact character-to-class index used only for
    /// target-conditioned CTC analysis. ONNX output tensors are not included
    /// because they are borrowed for a call and never copied or retained.
    pub fn target_support_index_bytes(&self) -> usize {
        self.target_support_analyzer.retained_index_payload_bytes()
    }

    pub fn recognize(&mut self, image: ImageView<'_>) -> Result<CtcRecognition, PaddleError> {
        self.recognize_with_segment_width(image, self.config.maximum_segment_width)
    }

    /// Runs one ONNX inference and evaluates a canonically deduplicated target set
    /// against the shared output tensor. The tensor is borrowed from ONNX Runtime
    /// only for this call and is never retained or copied into the session.
    pub fn recognize_batch(
        &mut self,
        image: ImageView<'_>,
        targets: &[FullLineAffixMatcher],
    ) -> Result<CtcBatchRecognition, PaddleError> {
        let crop = find_ink_crop(image, self.config.ink_margin);
        self.recognize_crop_batch(image, crop, targets)
    }

    pub fn recognize_target(
        &mut self,
        image: ImageView<'_>,
        target: &FullLineAffixMatcher,
    ) -> Result<CtcTargetRecognition, PaddleError> {
        self.recognize_target_with_segment_width(image, target, self.config.maximum_segment_width)
    }

    /// Mirrors the .NET segmented target route: segmented text can only assist
    /// when the combined transcript strictly matches the compiled target.
    pub fn recognize_target_with_segment_width(
        &mut self,
        image: ImageView<'_>,
        target: &FullLineAffixMatcher,
        maximum_segment_width: Option<usize>,
    ) -> Result<CtcTargetRecognition, PaddleError> {
        validate_segment_width(maximum_segment_width)?;
        let crop = find_ink_crop(image, self.config.ink_margin);
        let natural_width = calculate_resized_width(crop);
        if let Some(maximum_width) = maximum_segment_width
            && natural_width > maximum_width
        {
            let segments = split_crop(image, crop, natural_width, maximum_width);
            let mut results = Vec::with_capacity(segments.len());
            for segment in segments {
                results.push(self.recognize_crop(image, segment)?);
            }
            let recognition = combine_segment_results(results);
            let target_support =
                crate::CtcTargetSupport::segmented_strict_match(target.is_match(&recognition.text));
            return Ok(CtcTargetRecognition {
                recognition,
                target_support,
            });
        }

        let batch = self.recognize_crop_batch(image, crop, std::slice::from_ref(target))?;
        let target_support = batch
            .target_supports
            .into_iter()
            .next()
            .expect("one compiled target always produces one target evaluation")
            .support;
        Ok(CtcTargetRecognition {
            recognition: batch.recognition,
            target_support,
        })
    }

    pub fn recognize_with_segment_width(
        &mut self,
        image: ImageView<'_>,
        maximum_segment_width: Option<usize>,
    ) -> Result<CtcRecognition, PaddleError> {
        validate_segment_width(maximum_segment_width)?;

        let crop = find_ink_crop(image, self.config.ink_margin);
        let natural_width = calculate_resized_width(crop);
        if let Some(maximum_width) = maximum_segment_width
            && natural_width > maximum_width
        {
            let segments = split_crop(image, crop, natural_width, maximum_width);
            let mut results = Vec::with_capacity(segments.len());
            for segment in segments {
                results.push(self.recognize_crop(image, segment)?);
            }
            return Ok(combine_segment_results(results));
        }
        self.recognize_crop(image, crop)
    }

    fn recognize_crop(
        &mut self,
        image: ImageView<'_>,
        crop: crate::preprocess::InkCrop,
    ) -> Result<CtcRecognition, PaddleError> {
        Ok(self.recognize_crop_batch(image, crop, &[])?.recognition)
    }

    fn recognize_crop_batch(
        &mut self,
        image: ImageView<'_>,
        crop: crate::preprocess::InkCrop,
        targets: &[FullLineAffixMatcher],
    ) -> Result<CtcBatchRecognition, PaddleError> {
        let preparation_started = Instant::now();
        let resized_width = calculate_resized_width(crop);
        let tensor_width = calculate_tensor_width(resized_width, self.config.right_padding);
        let input_length = 3_usize
            .checked_mul(MODEL_HEIGHT)
            .and_then(|value| value.checked_mul(tensor_width))
            .ok_or_else(|| PaddleError::InvalidImage("tensor dimensions overflow".to_owned()))?;
        let input = self.workspace.prepare(input_length);
        fill_tensor(
            image,
            crop,
            resized_width,
            tensor_width,
            input,
            self.config.polarity,
        )?;
        let tensor_preparation_elapsed = preparation_started.elapsed();

        let inference_started = Instant::now();
        let tensor = TensorRef::from_array_view((
            [1_usize, 3, MODEL_HEIGHT, tensor_width],
            self.workspace.active(input_length),
        ))?;
        let outputs = self.session.run(ort::inputs![tensor])?;
        let inference_elapsed = inference_started.elapsed();

        let decode_started = Instant::now();
        let (shape, output) = outputs[0].try_extract_tensor::<f32>()?;
        if shape.len() != 3 || shape[0] != 1 {
            return Err(PaddleError::ModelContract(format!(
                "expected runtime output [1,time,classes], received {shape:?}"
            )));
        }
        let time_steps = usize::try_from(shape[1]).map_err(|_| {
            PaddleError::ModelContract(format!("invalid runtime time dimension {}", shape[1]))
        })?;
        let class_count = usize::try_from(shape[2]).map_err(|_| {
            PaddleError::ModelContract(format!("invalid runtime class dimension {}", shape[2]))
        })?;
        if class_count != EXPECTED_CLASS_COUNT {
            return Err(PaddleError::ModelContract(format!(
                "runtime class count is {class_count}; expected {EXPECTED_CLASS_COUNT}"
            )));
        }
        let expected_values = time_steps.checked_mul(class_count).ok_or_else(|| {
            PaddleError::ModelContract("runtime output dimensions overflow".to_owned())
        })?;
        if output.len() != expected_values {
            return Err(PaddleError::ModelContract(format!(
                "runtime output has {} values; expected {expected_values}",
                output.len()
            )));
        }
        let decoded = decode_greedy(output, time_steps, class_count, &self.dictionary)?;
        let target_analysis_started = Instant::now();
        let target_supports = self.target_support_analyzer.analyze_set(
            targets,
            &decoded.text,
            &decoded.emissions,
            output,
            time_steps,
            class_count,
        );
        let target_analysis_elapsed = target_analysis_started.elapsed();
        let decode_elapsed = decode_started.elapsed();
        Ok(CtcBatchRecognition {
            recognition: CtcRecognition {
                text: decoded.text,
                mean_confidence: decoded.mean_confidence,
                emissions: decoded.emissions,
                tensor_width,
                time_steps,
                class_count,
                segment_count: 1,
                tensor_preparation_elapsed,
                inference_elapsed,
                decode_elapsed,
            },
            target_supports,
            target_analysis_elapsed,
        })
    }
}

fn validate_segment_width(maximum_segment_width: Option<usize>) -> Result<(), PaddleError> {
    if let Some(width) = maximum_segment_width
        && !(320..=1_600).contains(&width)
    {
        return Err(PaddleError::InvalidConfiguration(format!(
            "maximum segment width must be in 320..=1600; got {width}"
        )));
    }
    Ok(())
}

/// Loads the requested native ONNX Runtime DLL/shared library without network access.
///
/// Call this before constructing any `ort` object. Applications that configure
/// `ort` themselves can skip this helper.
pub fn initialize_onnx_runtime(path: impl AsRef<Path>) -> Result<(), PaddleError> {
    let builder = ort::init_from(path.as_ref())
        .map_err(|error| PaddleError::RuntimeLoad(error.to_string()))?;
    builder
        .with_name("poe-alarm-paddle")
        .with_telemetry(false)
        .commit();
    Ok(())
}

fn validate_model_contract(session: &Session) -> Result<ModelContract, PaddleError> {
    if session.inputs().len() != 1 || session.outputs().is_empty() {
        return Err(PaddleError::ModelContract(format!(
            "expected one input and at least one output; found {} input(s) and {} output(s)",
            session.inputs().len(),
            session.outputs().len()
        )));
    }
    let input = &session.inputs()[0];
    let output = &session.outputs()[0];
    let input_shape = tensor_shape(input.dtype(), "input", TensorElementType::Float32)?;
    let output_shape = tensor_shape(output.dtype(), "output", TensorElementType::Float32)?;
    if input_shape != EXPECTED_INPUT_SHAPE {
        return Err(PaddleError::ModelContract(format!(
            "input shape is {input_shape:?}; expected {EXPECTED_INPUT_SHAPE:?}"
        )));
    }
    if output_shape != EXPECTED_OUTPUT_SHAPE {
        return Err(PaddleError::ModelContract(format!(
            "output shape is {output_shape:?}; expected {EXPECTED_OUTPUT_SHAPE:?}"
        )));
    }
    Ok(ModelContract {
        input_name: input.name().to_owned(),
        output_name: output.name().to_owned(),
        input_shape,
        output_shape,
    })
}

fn tensor_shape(
    value_type: &ValueType,
    label: &str,
    expected_type: TensorElementType,
) -> Result<Vec<i64>, PaddleError> {
    if value_type.tensor_type() != Some(expected_type) {
        return Err(PaddleError::ModelContract(format!(
            "{label} type is {value_type}; expected Tensor<{expected_type}>"
        )));
    }
    value_type
        .tensor_shape()
        .map(|shape| shape.iter().copied().collect())
        .ok_or_else(|| PaddleError::ModelContract(format!("{label} is not a tensor")))
}

struct DecodedText {
    text: String,
    mean_confidence: f64,
    emissions: Vec<CtcEmission>,
}

fn decode_greedy(
    output: &[f32],
    time_steps: usize,
    class_count: usize,
    dictionary: &[String],
) -> Result<DecodedText, PaddleError> {
    if class_count == 0 || output.len() != time_steps.saturating_mul(class_count) {
        return Err(PaddleError::ModelContract(format!(
            "cannot decode {} values as {time_steps}x{class_count} CTC output",
            output.len()
        )));
    }
    let mut text = String::new();
    let mut emissions = Vec::new();
    let mut confidence_total = 0.0_f64;
    let mut previous_class = usize::MAX;
    for time in 0..time_steps {
        let step = &output[time * class_count..(time + 1) * class_count];
        let mut best_class = 0;
        let mut best_score = step[0];
        for (class_index, &score) in step.iter().enumerate().skip(1) {
            if score > best_score {
                best_class = class_index;
                best_score = score;
            }
        }
        if best_class != 0 && best_class != previous_class {
            let dictionary_index = best_class - 1;
            let emitted = if dictionary_index < dictionary.len() {
                dictionary[dictionary_index].as_str()
            } else if dictionary_index == dictionary.len() {
                // Paddle may append use_space_char after loading a dictionary
                // which already contains its own final space entry.
                " "
            } else {
                "\u{fffd}"
            };
            text.push_str(emitted);
            emissions.push(CtcEmission {
                text: emitted.to_owned(),
                time_step: time,
                probability: best_score,
            });
            confidence_total += f64::from(best_score);
        }
        previous_class = best_class;
    }
    let mean_confidence = if emissions.is_empty() {
        0.0
    } else {
        confidence_total / emissions.len() as f64
    };
    Ok(DecodedText {
        text,
        mean_confidence,
        emissions,
    })
}

fn combine_segment_results(results: Vec<CtcRecognition>) -> CtcRecognition {
    let segment_count = results.len();
    let text = results
        .iter()
        .map(|result| result.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mean_confidence = if results.is_empty() {
        0.0
    } else {
        results
            .iter()
            .map(|result| result.mean_confidence)
            .sum::<f64>()
            / results.len() as f64
    };
    let mut time_offset = 0;
    let mut emissions = Vec::new();
    for result in &results {
        emissions.extend(result.emissions.iter().cloned().map(|mut emission| {
            emission.time_step += time_offset;
            emission
        }));
        time_offset += result.time_steps;
    }
    CtcRecognition {
        text,
        mean_confidence,
        emissions,
        tensor_width: results.iter().map(|result| result.tensor_width).sum(),
        time_steps: results.iter().map(|result| result.time_steps).sum(),
        class_count: results.first().map_or(0, |result| result.class_count),
        segment_count,
        tensor_preparation_elapsed: sum_durations(
            results
                .iter()
                .map(|result| result.tensor_preparation_elapsed),
        ),
        inference_elapsed: sum_durations(results.iter().map(|result| result.inference_elapsed)),
        decode_elapsed: sum_durations(results.iter().map(|result| result.decode_elapsed)),
    }
}

fn sum_durations(values: impl Iterator<Item = Duration>) -> Duration {
    values.fold(Duration::ZERO, |total, value| total + value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_uses_dotnet_bounds() {
        let mut config = PaddleSessionConfig::default();
        assert!(config.validate().is_ok());
        config.threads = 0;
        assert!(config.validate().is_err());
        config.threads = 2;
        config.maximum_segment_width = Some(319);
        assert!(config.validate().is_err());
        config.maximum_segment_width = Some(1_600);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn greedy_decode_collapses_repeats_and_blanks() {
        let dictionary = vec!["A".to_owned(), "B".to_owned()];
        #[rustfmt::skip]
        let output = [
            0.1, 0.8, 0.1, 0.0,
            0.1, 0.7, 0.2, 0.0,
            0.9, 0.05, 0.05, 0.0,
            0.1, 0.2, 0.6, 0.1,
            0.1, 0.1, 0.2, 0.6,
        ];
        let decoded = decode_greedy(&output, 5, 4, &dictionary).unwrap();
        assert_eq!(decoded.text, "AB ");
        assert_eq!(decoded.emissions.len(), 3);
        assert!((decoded.mean_confidence - (0.8 + 0.6 + 0.6) / 3.0).abs() < 1e-6);
    }

    #[test]
    fn greedy_ties_keep_the_first_class() {
        let dictionary = vec!["first".to_owned(), "second".to_owned()];
        let decoded = decode_greedy(&[0.1, 0.8, 0.8], 1, 3, &dictionary).unwrap();
        assert_eq!(decoded.text, "first");
    }

    #[test]
    fn segmented_confidence_is_an_unweighted_segment_average() {
        let make = |text: &str, confidence: f64, width: usize| CtcRecognition {
            text: text.to_owned(),
            mean_confidence: confidence,
            emissions: Vec::new(),
            tensor_width: width,
            time_steps: width / 4,
            class_count: EXPECTED_CLASS_COUNT,
            segment_count: 1,
            tensor_preparation_elapsed: Duration::from_millis(1),
            inference_elapsed: Duration::from_millis(2),
            decode_elapsed: Duration::from_millis(3),
        };
        let combined =
            combine_segment_results(vec![make(" one ", 0.2, 320), make("two", 0.8, 640)]);
        assert_eq!(combined.text, "one two");
        assert_eq!(combined.mean_confidence, 0.5);
        assert_eq!(combined.tensor_width, 960);
        assert_eq!(combined.segment_count, 2);
        assert_eq!(combined.total_elapsed(), Duration::from_millis(12));
    }

    #[test]
    fn maximum_model_width_constant_matches_dotnet() {
        assert_eq!(crate::MAXIMUM_MODEL_WIDTH, 3_200);
    }
}
