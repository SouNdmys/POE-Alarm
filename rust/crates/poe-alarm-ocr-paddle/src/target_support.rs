use std::collections::HashSet;
use std::time::Duration;

use poe_alarm_core::{AffixTokenKind, FullLineAffixMatcher, canonicalize};
use unicode_normalization::UnicodeNormalization;

use crate::CtcEmission;

const EVIDENCE_TIME_RADIUS: usize = 2;
const MAXIMUM_STRONG_MISMATCHES: usize = 2;
const MAXIMUM_STRONG_RANK: usize = 3;
const MINIMUM_STRONG_PROBABILITY: f32 = 0.01;
const MINIMUM_STRONG_PROBABILITY_RATIO: f32 = 0.012;

/// Why target-conditioned CTC evidence was accepted or rejected.
///
/// This is intentionally typed instead of being a free-form diagnostic string:
/// callers can audit a rejection without parsing UI text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CtcTargetSupportReason {
    CanonicalTokenCountDiffers {
        expected: usize,
        actual: usize,
    },
    TokenKindDiffers {
        token_index: usize,
        expected: AffixTokenKind,
        actual: AffixTokenKind,
    },
    NotPositionPreservingLexicalSubstitution {
        token_index: usize,
        expected: String,
        actual: String,
    },
    CanonicalTextMatches,
    LexicalEmissionCountDiffers {
        expected: usize,
        actual: usize,
    },
    TargetCharacterAbsent {
        expected: String,
    },
    AllSubstitutionsStronglySupported,
    WeakSubstitutionEvidence,
    SegmentedCanonicalTextDoesNotMatch,
}

impl std::fmt::Display for CtcTargetSupportReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CanonicalTokenCountDiffers { expected, actual } => write!(
                formatter,
                "canonical token count differs ({expected} vs {actual})"
            ),
            Self::TokenKindDiffers {
                token_index,
                expected,
                actual,
            } => write!(
                formatter,
                "token {token_index} kind differs ({expected:?} vs {actual:?})"
            ),
            Self::NotPositionPreservingLexicalSubstitution { token_index, .. } => write!(
                formatter,
                "token {token_index} is not a position-preserving lexical substitution"
            ),
            Self::CanonicalTextMatches => formatter.write_str("canonical text already matches"),
            Self::LexicalEmissionCountDiffers { expected, actual } => write!(
                formatter,
                "lexical emission count differs ({expected} vs {actual})"
            ),
            Self::TargetCharacterAbsent { expected } => write!(
                formatter,
                "target character '{expected}' is absent from the model dictionary"
            ),
            Self::AllSubstitutionsStronglySupported => {
                formatter.write_str("all substitutions have top-3 target evidence")
            }
            Self::WeakSubstitutionEvidence => {
                formatter.write_str("one or more substitutions lack strong target evidence")
            }
            Self::SegmentedCanonicalTextDoesNotMatch => {
                formatter.write_str("segmented text does not strictly match")
            }
        }
    }
}

/// Auditable evidence for one position-preserving lexical substitution.
#[derive(Clone, Debug, PartialEq)]
pub struct CtcMismatchSupport {
    pub character_ordinal: usize,
    pub expected: String,
    pub actual: String,
    pub expected_class: usize,
    pub greedy_time_step: usize,
    pub evidence_time_step: usize,
    pub rank: usize,
    pub probability: f32,
    pub top_probability: f32,
    pub probability_ratio: f32,
}

/// Strict structural compatibility plus target-conditioned CTC evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct CtcTargetSupport {
    pub shape_compatible: bool,
    pub strongly_supported: bool,
    pub mismatches: Vec<CtcMismatchSupport>,
    pub reason: CtcTargetSupportReason,
}

impl CtcTargetSupport {
    fn unsupported(reason: CtcTargetSupportReason) -> Self {
        Self {
            shape_compatible: false,
            strongly_supported: false,
            mismatches: Vec::new(),
            reason,
        }
    }

    pub(crate) fn segmented_strict_match(matches: bool) -> Self {
        if matches {
            Self {
                shape_compatible: true,
                strongly_supported: true,
                mismatches: Vec::new(),
                reason: CtcTargetSupportReason::CanonicalTextMatches,
            }
        } else {
            Self::unsupported(CtcTargetSupportReason::SegmentedCanonicalTextDoesNotMatch)
        }
    }
}

/// One entry in a target set after canonical-key deduplication.
#[derive(Clone, Debug, PartialEq)]
pub struct CtcTargetEvaluation {
    pub source_template: String,
    pub canonical_target: String,
    pub support: CtcTargetSupport,
}

/// Target-conditioned results sharing one greedy decode and one ONNX inference.
#[derive(Clone, Debug, PartialEq)]
pub struct CtcBatchRecognition {
    pub recognition: crate::CtcRecognition,
    pub target_supports: Vec<CtcTargetEvaluation>,
    pub target_analysis_elapsed: Duration,
}

impl CtcBatchRecognition {
    pub fn support_for(&self, target: &FullLineAffixMatcher) -> Option<&CtcTargetSupport> {
        self.support_for_canonical(&target.template().text)
    }

    pub fn support_for_canonical(&self, canonical_target: &str) -> Option<&CtcTargetSupport> {
        self.target_supports
            .iter()
            .find(|entry| entry.canonical_target == canonical_target)
            .map(|entry| &entry.support)
    }
}

/// A single-target result used by the segmented recovery route.
#[derive(Clone, Debug, PartialEq)]
pub struct CtcTargetRecognition {
    pub recognition: crate::CtcRecognition,
    pub target_support: CtcTargetSupport,
}

#[derive(Clone, Debug)]
pub(crate) struct TargetSupportAnalyzer {
    // Every verifiable mismatch is one Unicode scalar. Indexing by `char`
    // avoids retaining a second copy of the 18k-entry dictionary strings.
    character_classes: Vec<(char, usize)>,
    allow_latin_target_support: bool,
}

impl TargetSupportAnalyzer {
    pub(crate) fn new(dictionary: &[String], allow_latin_target_support: bool) -> Self {
        let mut character_classes = Vec::with_capacity(dictionary.len());
        for (index, character) in dictionary.iter().enumerate() {
            let mut scalars = character.chars();
            if let Some(character) = scalars.next()
                && scalars.next().is_none()
            {
                character_classes.push((character, index + 1));
            }
        }
        // Sorting by class as the secondary key preserves the .NET dictionary's
        // first-class-wins rule when a character appears more than once.
        character_classes.sort_unstable_by_key(|&(character, class)| (character, class));
        character_classes.dedup_by_key(|entry| entry.0);
        Self {
            character_classes,
            allow_latin_target_support,
        }
    }

    pub(crate) fn retained_index_payload_bytes(&self) -> usize {
        self.character_classes.capacity() * std::mem::size_of::<(char, usize)>()
    }

    pub(crate) fn analyze_set(
        &self,
        targets: &[FullLineAffixMatcher],
        decoded_text: &str,
        emissions: &[CtcEmission],
        output: &[f32],
        time_steps: usize,
        class_count: usize,
    ) -> Vec<CtcTargetEvaluation> {
        let mut seen = HashSet::with_capacity(targets.len());
        let mut results = Vec::with_capacity(targets.len());
        for target in targets {
            let canonical_target = target.template().text.clone();
            if !seen.insert(canonical_target.clone()) {
                continue;
            }
            let support = self.analyze(
                target.source_template(),
                decoded_text,
                emissions,
                output,
                time_steps,
                class_count,
            );
            results.push(CtcTargetEvaluation {
                source_template: target.source_template().to_owned(),
                canonical_target,
                support,
            });
        }
        results
    }

    fn analyze(
        &self,
        target_template: &str,
        decoded_text: &str,
        emissions: &[CtcEmission],
        output: &[f32],
        time_steps: usize,
        class_count: usize,
    ) -> CtcTargetSupport {
        let expected = canonicalize(target_template);
        let actual = canonicalize(decoded_text);
        if expected.tokens.len() != actual.tokens.len() {
            return CtcTargetSupport::unsupported(
                CtcTargetSupportReason::CanonicalTokenCountDiffers {
                    expected: expected.tokens.len(),
                    actual: actual.tokens.len(),
                },
            );
        }

        let mut mismatches = Vec::new();
        let mut character_ordinal = 0;
        for (token_index, (expected_token, actual_token)) in
            expected.tokens.iter().zip(actual.tokens.iter()).enumerate()
        {
            if expected_token.kind != actual_token.kind {
                return CtcTargetSupport::unsupported(CtcTargetSupportReason::TokenKindDiffers {
                    token_index,
                    expected: expected_token.kind,
                    actual: actual_token.kind,
                });
            }
            if expected_token.kind != AffixTokenKind::Word {
                continue;
            }

            if expected_token.text != actual_token.text {
                if !can_verify_lexical_substitution(
                    &expected_token.text,
                    &actual_token.text,
                    self.allow_latin_target_support,
                ) {
                    return CtcTargetSupport::unsupported(
                        CtcTargetSupportReason::NotPositionPreservingLexicalSubstitution {
                            token_index,
                            expected: expected_token.text.clone(),
                            actual: actual_token.text.clone(),
                        },
                    );
                }

                for (character_index, (expected_character, actual_character)) in expected_token
                    .text
                    .chars()
                    .zip(actual_token.text.chars())
                    .enumerate()
                {
                    if expected_character == actual_character {
                        continue;
                    }
                    mismatches.push(PendingMismatch {
                        character_ordinal: character_ordinal + character_index,
                        expected: expected_character,
                        actual: actual_character,
                    });
                }
            }
            character_ordinal += expected_token.text.chars().count();
        }

        if mismatches.is_empty() {
            return CtcTargetSupport {
                shape_compatible: true,
                strongly_supported: true,
                mismatches: Vec::new(),
                reason: CtcTargetSupportReason::CanonicalTextMatches,
            };
        }

        let lexical_emissions = normalize_lexical_emissions(emissions);
        if lexical_emissions.len() != character_ordinal {
            return CtcTargetSupport::unsupported(
                CtcTargetSupportReason::LexicalEmissionCountDiffers {
                    expected: character_ordinal,
                    actual: lexical_emissions.len(),
                },
            );
        }

        let mut support = Vec::with_capacity(mismatches.len());
        for mismatch in mismatches {
            let Ok(class_index) = self
                .character_classes
                .binary_search_by_key(&mismatch.expected, |entry| entry.0)
            else {
                return CtcTargetSupport::unsupported(
                    CtcTargetSupportReason::TargetCharacterAbsent {
                        expected: mismatch.expected.to_string(),
                    },
                );
            };
            let expected_class = self.character_classes[class_index].1;
            if expected_class >= class_count {
                return CtcTargetSupport::unsupported(
                    CtcTargetSupportReason::TargetCharacterAbsent {
                        expected: mismatch.expected.to_string(),
                    },
                );
            }

            let center = lexical_emissions[mismatch.character_ordinal].time_step;
            let best = find_best_support(output, time_steps, class_count, expected_class, center);
            support.push(CtcMismatchSupport {
                character_ordinal: mismatch.character_ordinal,
                expected: mismatch.expected.to_string(),
                actual: mismatch.actual.to_string(),
                expected_class,
                greedy_time_step: center,
                evidence_time_step: best.time_step,
                rank: best.rank,
                probability: best.probability,
                top_probability: best.top_probability,
                probability_ratio: if best.top_probability <= 0.0 {
                    0.0
                } else {
                    best.probability / best.top_probability
                },
            });
        }

        let strongly_supported = support.len() <= MAXIMUM_STRONG_MISMATCHES
            && support.iter().all(|item| {
                item.rank <= MAXIMUM_STRONG_RANK
                    && item.probability >= MINIMUM_STRONG_PROBABILITY
                    && item.probability_ratio >= MINIMUM_STRONG_PROBABILITY_RATIO
            });
        CtcTargetSupport {
            shape_compatible: true,
            strongly_supported,
            mismatches: support,
            reason: if strongly_supported {
                CtcTargetSupportReason::AllSubstitutionsStronglySupported
            } else {
                CtcTargetSupportReason::WeakSubstitutionEvidence
            },
        }
    }
}

#[derive(Clone, Debug)]
struct PendingMismatch {
    character_ordinal: usize,
    expected: char,
    actual: char,
}

#[derive(Clone, Copy, Debug)]
struct LexicalEmission {
    time_step: usize,
}

#[derive(Clone, Copy, Debug)]
struct BestClassSupport {
    time_step: usize,
    rank: usize,
    probability: f32,
    top_probability: f32,
}

fn find_best_support(
    output: &[f32],
    time_steps: usize,
    class_count: usize,
    expected_class: usize,
    center: usize,
) -> BestClassSupport {
    let mut best = BestClassSupport {
        time_step: center,
        rank: usize::MAX,
        probability: 0.0,
        top_probability: 0.0,
    };
    let start = center.saturating_sub(EVIDENCE_TIME_RADIUS);
    let end = center
        .saturating_add(EVIDENCE_TIME_RADIUS)
        .min(time_steps.saturating_sub(1));
    for time in start..=end {
        let step = &output[time * class_count..(time + 1) * class_count];
        let expected_probability = step[expected_class];
        let mut top_probability = 0.0_f32;
        let mut rank = 1;
        for &probability in step.iter().skip(1) {
            top_probability = top_probability.max(probability);
            if probability > expected_probability {
                rank += 1;
            }
        }
        if expected_probability > best.probability {
            best = BestClassSupport {
                time_step: time,
                rank,
                probability: expected_probability,
                top_probability,
            };
        }
    }
    best
}

fn normalize_lexical_emissions(emissions: &[CtcEmission]) -> Vec<LexicalEmission> {
    emissions
        .iter()
        .flat_map(|emission| {
            emission
                .text
                .nfkc()
                .flat_map(char::to_lowercase)
                .filter(|character| character.is_alphabetic())
                .map(|_| LexicalEmission {
                    time_step: emission.time_step,
                })
        })
        .collect()
}

fn is_single_han(text: &str) -> bool {
    let mut characters = text.chars();
    let Some(character) = characters.next() else {
        return false;
    };
    characters.next().is_none()
        && matches!(
            character,
            '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
        )
}

fn can_verify_lexical_substitution(expected: &str, actual: &str, allow_latin: bool) -> bool {
    if is_single_han(expected) && is_single_han(actual) {
        return true;
    }
    if !allow_latin {
        return false;
    }
    let expected_characters = expected.chars().collect::<Vec<_>>();
    let actual_characters = actual.chars().collect::<Vec<_>>();
    expected_characters.len() >= 4
        && expected_characters.len() == actual_characters.len()
        && expected_characters
            .iter()
            .zip(actual_characters.iter())
            .filter(|(expected, actual)| expected != actual)
            .count()
            <= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(template: &str) -> FullLineAffixMatcher {
        FullLineAffixMatcher::new(template).unwrap()
    }

    fn dictionary() -> Vec<String> {
        ["暴", "擊", "擎", "率", "命", "中", "快", "速", "傷", "害"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn emissions(text: &str, steps: &[usize]) -> Vec<CtcEmission> {
        text.chars()
            .zip(steps.iter().copied())
            .map(|(character, time_step)| CtcEmission {
                text: character.to_string(),
                time_step,
                probability: 0.8,
            })
            .collect()
    }

    fn output(time_steps: usize, class_count: usize) -> Vec<f32> {
        vec![0.0; time_steps * class_count]
    }

    #[test]
    fn strong_han_support_reports_rank_probability_and_time() {
        let dictionary = dictionary();
        let analyzer = TargetSupportAnalyzer::new(&dictionary, false);
        let class_count = dictionary.len() + 1;
        let mut logits = output(8, class_count);
        let center = 3;
        logits[(center + 1) * class_count + 2] = 0.30; // expected 擊, dictionary class 2
        logits[(center + 1) * class_count + 3] = 0.80; // greedy 擎
        logits[(center + 1) * class_count + 5] = 0.40; // one extra class above target
        let support = analyzer.analyze(
            "暴擊率",
            "暴擎率",
            &emissions("暴擎率", &[0, center, 6]),
            &logits,
            8,
            class_count,
        );
        assert!(support.shape_compatible);
        assert!(support.strongly_supported);
        assert_eq!(
            support.reason,
            CtcTargetSupportReason::AllSubstitutionsStronglySupported
        );
        assert_eq!(support.mismatches.len(), 1);
        let mismatch = &support.mismatches[0];
        assert_eq!(mismatch.character_ordinal, 1);
        assert_eq!(mismatch.expected, "擊");
        assert_eq!(mismatch.actual, "擎");
        assert_eq!(mismatch.greedy_time_step, center);
        assert_eq!(mismatch.evidence_time_step, center + 1);
        assert_eq!(mismatch.rank, 3);
        assert_eq!(mismatch.probability, 0.30);
        assert_eq!(mismatch.top_probability, 0.80);
        assert!((mismatch.probability_ratio - 0.375).abs() < f32::EPSILON);
    }

    #[test]
    fn weak_probability_and_wrong_neighbor_are_not_strong_evidence() {
        let dictionary = dictionary();
        let analyzer = TargetSupportAnalyzer::new(&dictionary, false);
        let class_count = dictionary.len() + 1;
        let mut logits = output(5, class_count);
        logits[2 * class_count + 2] = 0.009;
        logits[2 * class_count + 3] = 0.90;
        let support = analyzer.analyze(
            "暴擊率",
            "暴擎率",
            &emissions("暴擎率", &[0, 2, 4]),
            &logits,
            5,
            class_count,
        );
        assert!(support.shape_compatible);
        assert!(!support.strongly_supported);
        assert_eq!(
            support.reason,
            CtcTargetSupportReason::WeakSubstitutionEvidence
        );
    }

    #[test]
    fn token_kind_mismatch_is_rejected_before_ctc_evidence() {
        let dictionary = dictionary();
        let analyzer = TargetSupportAnalyzer::new(&dictionary, false);
        let support = analyzer.analyze(
            "+3.1%暴擊率",
            "+3.1暴擊率",
            &emissions("暴擊率", &[0, 1, 2]),
            &output(3, dictionary.len() + 1),
            3,
            dictionary.len() + 1,
        );
        assert!(!support.shape_compatible);
        assert!(!support.strongly_supported);
        assert!(matches!(
            support.reason,
            CtcTargetSupportReason::TokenKindDiffers { .. }
        ));
    }

    #[test]
    fn batch_deduplicates_canonical_targets_before_analysis() {
        let dictionary = dictionary();
        let analyzer = TargetSupportAnalyzer::new(&dictionary, false);
        let targets = [
            matcher("+3.1%暴擊率"),
            matcher("+4.0%暴擊率"),
            matcher("+3.1%暴擎率"),
        ];
        let class_count = dictionary.len() + 1;
        let results = analyzer.analyze_set(
            &targets,
            "+3.7%暴擎率",
            &emissions("暴擎率", &[0, 2, 4]),
            &output(5, class_count),
            5,
            class_count,
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].source_template, "+3.1%暴擊率");
        assert_eq!(results[1].source_template, "+3.1%暴擎率");
    }

    #[test]
    fn latin_substitution_requires_explicit_opt_in() {
        let dictionary = ["c", "r", "i", "t", "l", "a"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let target = "critical";
        let actual = "critlcal";
        let emissions = emissions(actual, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let class_count = dictionary.len() + 1;
        let logits = output(8, class_count);
        let disabled = TargetSupportAnalyzer::new(&dictionary, false).analyze(
            target,
            actual,
            &emissions,
            &logits,
            8,
            class_count,
        );
        assert!(!disabled.shape_compatible);
        let enabled = TargetSupportAnalyzer::new(&dictionary, true).analyze(
            target,
            actual,
            &emissions,
            &logits,
            8,
            class_count,
        );
        assert!(enabled.shape_compatible);
        assert!(!enabled.strongly_supported);
    }

    #[test]
    fn more_than_two_supported_substitutions_remain_weak() {
        let dictionary = dictionary();
        let analyzer = TargetSupportAnalyzer::new(&dictionary, false);
        let class_count = dictionary.len() + 1;
        let mut logits = output(6, class_count);
        for (time, expected_class) in [(0, 5), (2, 6), (4, 7)] {
            logits[time * class_count + expected_class] = 0.5;
        }
        let support = analyzer.analyze(
            "命中快",
            "傷害速",
            &emissions("傷害速", &[0, 2, 4]),
            &logits,
            6,
            class_count,
        );
        assert!(support.shape_compatible);
        assert_eq!(support.mismatches.len(), 3);
        assert!(!support.strongly_supported);
    }
}
