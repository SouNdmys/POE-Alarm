//! Field harness for the clipboard recognition path.
//!
//! Everything of substance now lives in the production crates: parsing in
//! `poe-alarm-clipboard`, the Win32 capture in `poe-alarm-platform-win`. This
//! crate is the thin shell that drives them from a console so the behaviour can
//! be measured against a running client — which is the one thing a unit test
//! cannot do.
//!
//! Keeping it pointed at the production code is deliberate. A copy would let
//! the harness and the shipped path drift, and then a good field run would
//! prove nothing about what users actually get.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod stats;

/// The Win32 capture surface, under the name the harness has always used.
pub use poe_alarm_platform_win as clipboard;

use poe_alarm_clipboard::ParsedItem;
use poe_alarm_core::{CompiledRuleSet, RuleEvaluationResult};
use poe_alarm_settings::SettingsStore;
use std::fmt;

pub use poe_alarm_clipboard::{ItemTextError, ModGroup, ModKind, parse};
pub use stats::{LatencySamples, format_millis};

/// Everything the harness needs from the user's real configuration.
///
/// Notably absent: a capture region. This path never looks at the screen, so
/// the one piece of setup that had to be redone whenever the UI moved is not
/// part of it.
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

/// Why the harness could not build a profile from saved settings.
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
        item: Box<ParsedItem>,
        evaluation: Box<RuleEvaluationResult>,
    },
    /// The payload parsed and the rules did not match.
    Miss {
        item: Box<ParsedItem>,
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

    /// True when no opinion could be formed, which callers must fail closed on.
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

    /// Modifier lines the rules were run against.
    #[must_use]
    pub fn line_count(&self) -> usize {
        match self {
            Self::Hit { item, .. } | Self::Miss { item, .. } => {
                item.groups.iter().map(|group| group.lines.len()).sum()
            }
            Self::Unreadable(_) => 0,
        }
    }

    /// Modifiers the rules were run against, as distinct physical affixes.
    #[must_use]
    pub fn modifier_count(&self) -> usize {
        match self {
            Self::Hit { item, .. } | Self::Miss { item, .. } => item.groups.len(),
            Self::Unreadable(_) => 0,
        }
    }

    /// Every modifier line, flattened, for display.
    #[must_use]
    pub fn lines(&self) -> Vec<&str> {
        match self {
            Self::Hit { item, .. } | Self::Miss { item, .. } => item
                .groups
                .iter()
                .flat_map(|group| group.lines.iter().map(String::as_str))
                .collect(),
            Self::Unreadable(_) => Vec::new(),
        }
    }
}

/// Parses a clipboard payload and runs it through the compiled rules.
#[must_use]
pub fn evaluate_payload(rules: &CompiledRuleSet, payload: &str) -> Verdict {
    match parse(payload) {
        Ok(item) => evaluate_item(rules, item),
        Err(error) => Verdict::Unreadable(error),
    }
}

/// Runs an already-parsed item through the compiled rules.
///
/// Uses `evaluate_with_identity`, the same entry point the shipped OCR path
/// takes, so a modifier rendered across several lines counts once rather than
/// satisfying two conditions.
#[must_use]
pub fn evaluate_item(rules: &CompiledRuleSet, item: ParsedItem) -> Verdict {
    let (lines, identities) = item.render();
    let evaluation = Box::new(rules.evaluate_with_identity(&lines, &[], &identities));
    let item = Box::new(item);
    if evaluation.is_match {
        Verdict::Hit { item, evaluation }
    } else {
        Verdict::Miss { item, evaluation }
    }
}

/// Whether two clipboard payloads describe the same item state.
///
/// This is the change detector. It replaces the capture region, the blue mask,
/// the fingerprint and the tolerance threshold that the first experiment
/// needed, and unlike all of them nothing drawn behind the tooltip can disturb
/// it. Allocation-free: both sides are borrowed straight out of the payloads.
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
        AcceptableResultGroup, AffixCondition, NumericConstraint, ResultGroupMode,
        RuleSetDefinition,
    };

    fn life_rule() -> CompiledRuleSet {
        CompiledRuleSet::compile(RuleSetDefinition {
            schema_version: CURRENT_SCHEMA_VERSION,
            name: "lab".to_string(),
            groups: vec![AcceptableResultGroup {
                name: "life".to_string(),
                mode: ResultGroupMode::All,
                required_count: 1,
                conditions: vec![AffixCondition::new(
                    "life",
                    "+# to maximum Life",
                    vec![NumericConstraint::at_least(40.0)],
                )],
            }],
        })
        .expect("rule compiles")
    }

    const ROLLED_HIGH: &str = concat!(
        "Item Class: Rings\r\n",
        "Rarity: Magic\r\n",
        "Coral Ring\r\n",
        "--------\r\n",
        "Item Level: 62\r\n",
        "--------\r\n",
        "+42 to maximum Life\r\n",
    );

    const ROLLED_LOW: &str = concat!(
        "Item Class: Rings\r\n",
        "Rarity: Magic\r\n",
        "Coral Ring\r\n",
        "--------\r\n",
        "Item Level: 62\r\n",
        "--------\r\n",
        "+12 to maximum Life\r\n",
    );

    #[test]
    fn matching_payload_is_a_hit() {
        let verdict = evaluate_payload(&life_rule(), ROLLED_HIGH);
        assert!(verdict.is_hit(), "{:?}", verdict.lines());
        assert_eq!(verdict.modifier_count(), 1);
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
    fn cosmetic_differences_are_not_a_new_roll() {
        let unix = ROLLED_HIGH.replace("\r\n", "\n");
        assert!(describes_same_roll(ROLLED_HIGH, &unix));
        let padded = format!("{ROLLED_HIGH}\r\n\r\n");
        assert!(describes_same_roll(ROLLED_HIGH, &padded));
    }
}
