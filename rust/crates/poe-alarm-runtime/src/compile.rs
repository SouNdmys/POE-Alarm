use std::fmt;

use poe_alarm_core::{
    CompiledRuleSet, FullLineAffixMatcher, matching::MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN,
};
use poe_alarm_monitoring::MonitorPlan;
use poe_alarm_settings::{AppSettings, CURRENT_SCHEMA_VERSION, RuleEditorMode};

use crate::{AlertCopy, CompiledUiBindings};

#[derive(Clone, Debug)]
pub struct CompiledRuntimeSettings {
    pub plan: MonitorPlan,
    pub ui: CompiledUiBindings,
    pub alert_copy: AlertCopy,
    /// Reserved for a future explicitly selected protection policy. The released fast mode must
    /// never suppress crafting clicks before a real match has been confirmed.
    pub input_guard_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsValidationError {
    pub fields: Vec<SettingsFieldError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsFieldError {
    pub field: &'static str,
    pub detail: String,
}

impl SettingsValidationError {
    fn one(field: &'static str, detail: impl Into<String>) -> Self {
        Self {
            fields: vec![SettingsFieldError {
                field,
                detail: detail.into(),
            }],
        }
    }
}

impl fmt::Display for SettingsValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let joined = self
            .fields
            .iter()
            .map(|error| format!("{}: {}", error.field, error.detail))
            .collect::<Vec<_>>()
            .join("; ");
        formatter.write_str(&joined)
    }
}

impl std::error::Error for SettingsValidationError {}

pub fn compile_settings(
    settings: &AppSettings,
) -> Result<CompiledRuntimeSettings, SettingsValidationError> {
    if settings.schema_version > CURRENT_SCHEMA_VERSION {
        return Err(SettingsValidationError::one(
            "schema_version",
            format!(
                "settings schema {} is newer than supported schema {CURRENT_SCHEMA_VERSION}",
                settings.schema_version
            ),
        ));
    }

    let selected = settings.selected_profile();
    let selected_rules = selected.selected_rules();

    // Still validated even though the parser reads both locales: this value
    // also selects which rule set the profile uses.
    let language = selected.ocr_language.trim();
    if !language.eq_ignore_ascii_case("en") && !language.eq_ignore_ascii_case("zh-TW") {
        return Err(SettingsValidationError::one(
            "ocr_language",
            format!("unsupported affix language '{language}'"),
        ));
    }

    // How many physical lines one modifier may span: the supported maximum,
    // for both games. This used to fork per game (4 for POE1, 8 for POE2) on
    // the claim that "POE2 renders wider modifiers" — but that described
    // tooltip pixel wrapping, an OCR-era phenomenon. In clipboard text one
    // stat is always one line, both games share one layout engine, and the
    // corpus agrees: no modifier in either game comes near either number.
    //
    // The limit's only effect is capping how many lines one template may
    // span; blank-line barriers between groups already prevent joining two
    // modifiers regardless of this value. So a large value costs nothing,
    // while a small one fails silently — a template wider than the limit is
    // not a compile error, it just never matches. Maximum, one code path.
    let maximum_line_span = MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN;

    let plan = match selected_rules.rule_editor_mode {
        RuleEditorMode::Quick => MonitorPlan::Quick(
            FullLineAffixMatcher::with_maximum_line_span(
                &selected_rules.target_affix,
                maximum_line_span,
            )
            .map_err(|error| SettingsValidationError::one("target_affix", error.to_string()))?,
        ),
        RuleEditorMode::Structured => {
            let definition = selected_rules.structured_rule_set.clone().ok_or_else(|| {
                SettingsValidationError::one(
                    "structured_rule_set",
                    "add at least one acceptable result before monitoring",
                )
            })?;
            MonitorPlan::Structured(
                CompiledRuleSet::compile_with_maximum_line_span(definition, maximum_line_span)
                    .map_err(|error| {
                        SettingsValidationError::one("structured_rule_set", error.to_string())
                    })?,
            )
        }
    };

    Ok(CompiledRuntimeSettings {
        plan,
        ui: CompiledUiBindings::from_settings(settings),
        alert_copy: AlertCopy::for_ui_language(&settings.ui_language),
        input_guard_enabled: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use poe_alarm_core::{AcceptableResultGroup, AffixCondition, RuleSetDefinition};
    use poe_alarm_settings::GameProfile;

    fn valid_settings() -> AppSettings {
        let mut settings = AppSettings::default();
        settings.profiles.poe1.selected_rules_mut().target_affix =
            "+#% to Critical Hit Chance".to_owned();
        settings
    }

    #[test]
    fn a_profile_that_was_never_configured_beyond_its_rule_still_compiles() {
        // The OCR build refused to compile a profile with no capture region,
        // which after the clipboard migration meant a fresh install could
        // neither monitor nor check an item — the field was still required and
        // no longer read by anything.
        let settings = valid_settings();
        let compiled = compile_settings(&settings).unwrap();
        assert!(matches!(compiled.plan, MonitorPlan::Quick(_)));
        assert!(!compiled.input_guard_enabled);
    }

    #[test]
    fn fast_mode_never_suppresses_crafting_clicks_before_a_match() {
        let mut settings = valid_settings();
        settings.profiles.poe1.ocr_language = "zh-TW".to_owned();
        // 规则按语言隔离:繁中档需要自己的目标词缀才能编译 Quick 计划。
        settings.profiles.poe1.selected_rules_mut().target_affix = "+#% 暴擊率".to_owned();
        assert!(!compile_settings(&settings).unwrap().input_guard_enabled);

        settings.profiles.poe1.ocr_language = "en".to_owned();
        settings.selected_game_profile = GameProfile::Poe2;
        settings.profiles.poe2 = settings.profiles.poe1.clone();
        assert!(!compile_settings(&settings).unwrap().input_guard_enabled);
    }

    /// One layout engine, one span. The per-game fork this replaces encoded
    /// an OCR-era wrapping observation, and its failure mode was the silent
    /// kind: a POE1 template wider than 4 lines would simply never match.
    #[test]
    fn both_games_allow_the_full_eight_line_span() {
        for game in [GameProfile::Poe1, GameProfile::Poe2] {
            let mut settings = valid_settings();
            settings.selected_game_profile = game;
            settings.profiles.poe2 = settings.profiles.poe1.clone();

            let quick = compile_settings(&settings).unwrap();
            let MonitorPlan::Quick(matcher) = quick.plan else {
                panic!("expected quick plan");
            };
            assert_eq!(
                matcher.maximum_line_span(),
                MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN,
                "{game:?} must compile quick rules at the shared span"
            );
        }
    }

    #[test]
    fn poe2_quick_and_structured_rules_allow_eight_physical_lines() {
        let mut settings = valid_settings();
        settings.selected_game_profile = GameProfile::Poe2;
        settings.profiles.poe2 = settings.profiles.poe1.clone();

        let quick = compile_settings(&settings).unwrap();
        let MonitorPlan::Quick(matcher) = quick.plan else {
            panic!("expected quick plan");
        };
        assert_eq!(
            matcher.maximum_line_span(),
            MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN
        );

        settings.profiles.poe2.selected_rules_mut().rule_editor_mode = RuleEditorMode::Structured;
        settings
            .profiles
            .poe2
            .selected_rules_mut()
            .structured_rule_set = Some(RuleSetDefinition {
            schema_version: 1,
            name: "results".to_owned(),
            groups: vec![AcceptableResultGroup {
                name: "critical".to_owned(),
                conditions: vec![AffixCondition::new(
                    "chance",
                    "+#% to Critical Hit Chance",
                    Vec::new(),
                )],
                ..AcceptableResultGroup::default()
            }],
        });
        let structured = compile_settings(&settings).unwrap();
        let MonitorPlan::Structured(rules) = structured.plan else {
            panic!("expected structured plan");
        };
        assert_eq!(
            rules.targets()[0].maximum_line_span(),
            MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN
        );
    }

    #[test]
    fn structured_rules_are_compiled_once() {
        let mut settings = valid_settings();
        settings.profiles.poe1.selected_rules_mut().rule_editor_mode = RuleEditorMode::Structured;
        settings
            .profiles
            .poe1
            .selected_rules_mut()
            .structured_rule_set = Some(RuleSetDefinition {
            schema_version: 1,
            name: "results".to_owned(),
            groups: vec![AcceptableResultGroup {
                name: "critical".to_owned(),
                conditions: vec![AffixCondition::new(
                    "chance",
                    "+#% to Critical Hit Chance",
                    Vec::new(),
                )],
                ..AcceptableResultGroup::default()
            }],
        });
        assert!(matches!(
            compile_settings(&settings).unwrap().plan,
            MonitorPlan::Structured(_)
        ));
    }

    #[test]
    fn an_unknown_affix_language_is_an_explicit_error() {
        let mut settings = valid_settings();
        settings.profiles.poe1.ocr_language = "zh-CN".to_owned();
        assert_eq!(
            compile_settings(&settings).unwrap_err().fields[0].field,
            "ocr_language"
        );
    }
}
