//! Clipboard-only affix recognition. No pixels are read at any point.
//!
//! The previous experiment used a screen fingerprint to answer "did the orb
//! land yet", which dragged in a capture region, a blue mask, and a tolerance
//! threshold — three things to configure and three things to get wrong. All of
//! that existed to save one clipboard round trip.
//!
//! This version deletes it. The clipboard text answers both questions by
//! itself: if the text differs from the text before the click, the roll landed,
//! and the text *is* the affix list. There is no region to select, no mask to
//! tune, no threshold to calibrate, and nothing to recalibrate when the scene
//! behind the tooltip changes.
//!
//! The cost is one or more extra Ctrl+C round trips while the server is still
//! resolving the craft. Whether that lands inside the OCR pipeline's budget is
//! exactly what the binary measures.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod item_text;
pub mod stats;

#[cfg(windows)]
pub mod clipboard;

use std::fmt;

use poe_alarm_core::{CompiledRuleSet, RuleEvaluationResult};
use poe_alarm_settings::SettingsStore;

pub use item_text::{ItemTextError, ParsedItem};
pub use stats::{LatencySamples, format_millis};

/// Everything the lab needs from the user's real configuration.
///
/// Notably absent: a capture region. This pipeline never looks at the screen,
/// so the one piece of setup that had to be re-done whenever the UI moved is
/// simply not part of it.
pub struct LabProfile {
    /// Rules compiled from the user's saved rule set.
    pub rules: CompiledRuleSet,
    /// Affix language the profile is configured for.
    pub ocr_language: String,
    /// Which game profile the settings were loaded from.
    pub game_profile: String,
    /// Where the settings came from.
    pub settings_path: String,
}

/// Why the lab could not build a profile from saved settings.
#[derive(Debug)]
pub enum ProfileError {
    /// The settings file could not be located or opened.
    Settings(std::io::Error),
    /// The profile has no structured rule set saved.
    MissingRuleSet,
    /// The saved rule set does not compile.
    InvalidRuleSet(String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Settings(error) => write!(formatter, "could not read settings: {error}"),
            Self::MissingRuleSet => formatter
                .write_str("no structured rule set saved — build one in the app's rule editor"),
            Self::InvalidRuleSet(message) => {
                write!(formatter, "saved rule set does not compile: {message}")
            }
        }
    }
}

impl std::error::Error for ProfileError {}

impl LabProfile {
    /// Loads the release settings file and compiles the selected profile.
    pub fn load_release() -> Result<Self, ProfileError> {
        let mut store = SettingsStore::release_default().map_err(ProfileError::Settings)?;
        let settings_path = store.path().display().to_string();
        let settings = store.load();
        let profile = settings.selected_profile();
        let definition = profile
            .selected_rules()
            .structured_rule_set
            .clone()
            .ok_or(ProfileError::MissingRuleSet)?;
        let rules = CompiledRuleSet::compile(definition)
            .map_err(|error| ProfileError::InvalidRuleSet(error.to_string()))?;
        Ok(Self {
            rules,
            ocr_language: profile.ocr_language.clone(),
            game_profile: format!("{:?}", settings.selected_game_profile),
            settings_path,
        })
    }
}

/// Outcome of evaluating one clipboard payload.
#[derive(Debug)]
pub enum Verdict {
    /// The payload parsed and the rules matched.
    Hit {
        item: ParsedItem,
        evaluation: Box<RuleEvaluationResult>,
    },
    /// The payload parsed and the rules did not match.
    Miss {
        item: ParsedItem,
        evaluation: Box<RuleEvaluationResult>,
    },
    /// The payload could not be read as an item.
    Unreadable(ItemTextError),
}

impl Verdict {
    #[must_use]
    pub fn is_hit(&self) -> bool {
        matches!(self, Self::Hit { .. })
    }

    /// True when the lab could not form an opinion, which must fail closed.
    #[must_use]
    pub fn is_undecided(&self) -> bool {
        matches!(self, Self::Unreadable(_))
    }

    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Hit { .. } => "HIT",
            Self::Miss { .. } => "miss",
            Self::Unreadable(_) => "??",
        }
    }

    #[must_use]
    pub fn affix_count(&self) -> usize {
        match self {
            Self::Hit { item, .. } | Self::Miss { item, .. } => item.affix_lines.len(),
            Self::Unreadable(_) => 0,
        }
    }
}

/// Parses a clipboard payload and runs it through the compiled rules.
#[must_use]
pub fn evaluate_payload(rules: &CompiledRuleSet, payload: &str) -> Verdict {
    match item_text::parse(payload) {
        Ok(item) => evaluate_item(rules, item),
        Err(error) => Verdict::Unreadable(error),
    }
}

/// Runs an already-parsed item through the compiled rules, so a caller that
/// needed the parse for other reasons does not pay for it twice.
#[must_use]
pub fn evaluate_item(rules: &CompiledRuleSet, item: ParsedItem) -> Verdict {
    let evaluation = Box::new(rules.evaluate(&item.affix_lines));
    if evaluation.is_match {
        Verdict::Hit { item, evaluation }
    } else {
        Verdict::Miss { item, evaluation }
    }
}

/// Whether two clipboard payloads describe the same item state.
///
/// This is the entire change detector. It replaces the capture region, the
/// blue mask, the fingerprint, and the tolerance threshold, and unlike all of
/// them it cannot be thrown off by anything drawn behind the tooltip.
/// Compares line by line, ignoring trailing whitespace and blank lines, so a
/// cosmetic difference never reads as a reroll. Allocation-free: both sides are
/// borrowed straight out of the payloads.
#[must_use]
pub fn describes_same_roll(previous: &str, current: &str) -> bool {
    meaningful_lines(previous).eq(meaningful_lines(current))
}

fn meaningful_lines(payload: &str) -> impl Iterator<Item = &str> {
    payload
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use poe_alarm_core::rules::CURRENT_SCHEMA_VERSION;
    use poe_alarm_core::{
        AcceptableResultGroup, AffixCondition, NumericConstraint, ResultGroupMode, RuleSetDefinition,
    };

    fn life_rule() -> CompiledRuleSet {
        let condition = AffixCondition::new(
            "life",
            "+# to maximum Life",
            vec![NumericConstraint::at_least(40.0)],
        );
        let group = AcceptableResultGroup {
            name: "life".to_string(),
            mode: ResultGroupMode::All,
            required_count: 1,
            conditions: vec![condition],
        };
        CompiledRuleSet::compile(RuleSetDefinition {
            schema_version: CURRENT_SCHEMA_VERSION,
            name: "lab".to_string(),
            groups: vec![group],
        })
        .expect("rule compiles")
    }

    const ROLLED_HIGH: &str = concat!(
        "Rarity: Magic\r\n",
        "Coral Ring\r\n",
        "--------\r\n",
        "Item Level: 62\r\n",
        "--------\r\n",
        "+42 to maximum Life\r\n",
    );

    const ROLLED_LOW: &str = concat!(
        "Rarity: Magic\r\n",
        "Coral Ring\r\n",
        "--------\r\n",
        "Item Level: 62\r\n",
        "--------\r\n",
        "+12 to maximum Life\r\n",
    );

    #[test]
    fn matching_payload_is_a_hit() {
        assert!(evaluate_payload(&life_rule(), ROLLED_HIGH).is_hit());
    }

    #[test]
    fn below_threshold_payload_is_a_miss() {
        let verdict = evaluate_payload(&life_rule(), ROLLED_LOW);
        assert!(!verdict.is_hit());
        assert!(!verdict.is_undecided());
    }

    #[test]
    fn garbage_payload_is_undecided_so_callers_fail_closed() {
        assert!(evaluate_payload(&life_rule(), "not an item").is_undecided());
    }

    #[test]
    fn identical_payloads_are_the_same_roll() {
        assert!(describes_same_roll(ROLLED_HIGH, ROLLED_HIGH));
    }

    #[test]
    fn a_changed_value_is_a_new_roll() {
        assert!(!describes_same_roll(ROLLED_HIGH, ROLLED_LOW));
    }

    #[test]
    fn cosmetic_line_ending_differences_are_not_a_new_roll() {
        let unix = ROLLED_HIGH.replace("\r\n", "\n");
        assert!(describes_same_roll(ROLLED_HIGH, &unix));
    }

    #[test]
    fn trailing_blank_lines_are_not_a_new_roll() {
        let padded = format!("{ROLLED_HIGH}\r\n\r\n");
        assert!(describes_same_roll(ROLLED_HIGH, &padded));
    }
}
