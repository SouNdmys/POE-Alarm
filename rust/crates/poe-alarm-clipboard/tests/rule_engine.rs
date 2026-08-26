//! End-to-end: a real clipboard dump through the parser and into the rule
//! engine, checking the two failure modes that motivated the whole exercise.
//!
//! The tooltip merges modifiers granting the same stat, so a bow carrying a
//! 179% physical prefix alongside a 79% physical hybrid displays `258%` — a
//! number no modifier on the item actually rolled. OCR could only ever see
//! that merged figure. These tests pin down that the clipboard path does not.

use poe_alarm_clipboard::{ModFilter, parse};
use poe_alarm_core::rules::CURRENT_SCHEMA_VERSION;
use poe_alarm_core::{
    AcceptableResultGroup, AffixCondition, CompiledRuleSet, NumericConstraint, ResultGroupMode,
    RuleEvaluationResult, RuleSetDefinition,
};

const BOW: &str = include_str!("../../../../tests/fixtures/clipboard-items/poe1-tw-rare-bow-hybrid.txt");

fn rules(conditions: Vec<AffixCondition>) -> CompiledRuleSet {
    CompiledRuleSet::compile(RuleSetDefinition {
        schema_version: CURRENT_SCHEMA_VERSION,
        name: "test".to_string(),
        groups: vec![AcceptableResultGroup {
            name: "group".to_string(),
            mode: ResultGroupMode::All,
            required_count: conditions.len(),
            conditions,
        }],
    })
    .expect("rule set compiles")
}

/// Parses the bow and evaluates `conditions` the way production will: with the
/// band identities, so one physical modifier can satisfy at most one condition.
fn evaluate(conditions: Vec<AffixCondition>) -> RuleEvaluationResult {
    let item = parse(BOW, ModFilter::default()).expect("bow parses");
    let (lines, identities) = item.render();
    rules(conditions).evaluate_with_identity(&lines, &[], &identities)
}

fn physical(constraints: Vec<NumericConstraint>) -> AffixCondition {
    AffixCondition::new("physical", "增加 (170—179)% 物理傷害", constraints)
}

#[test]
fn a_poedb_template_matches_the_rolled_value_with_its_tier_range() {
    // The dump writes `增加 179(170-179)% 物理傷害`; the user's rule is the
    // PoEDB template. They have to canonicalize to the same thing.
    let result = evaluate(vec![physical(vec![NumericConstraint::ignored()])]);
    assert!(result.is_match);
}

#[test]
fn a_numeric_floor_reads_the_rolled_value_not_the_range_bounds() {
    let result = evaluate(vec![physical(vec![NumericConstraint::at_least(179.0)])]);
    assert!(result.is_match, "179 is the rolled value and meets the floor");

    let result = evaluate(vec![physical(vec![NumericConstraint::at_least(180.0)])]);
    assert!(!result.is_match, "nothing on the bow rolled above 179");
}

#[test]
fn merged_tooltip_values_cannot_satisfy_a_rule() {
    // This is the regression the clipboard path exists to prevent. The tooltip
    // shows 179 + 79 = 258% physical damage, so an OCR read of that display
    // would satisfy a 200% floor even though neither modifier is close.
    let result = evaluate(vec![physical(vec![NumericConstraint::at_least(200.0)])]);
    assert!(
        !result.is_match,
        "the two physical prefixes must stay separate; 179 and 79 do not add up"
    );
}

#[test]
fn a_hybrid_template_matches_the_two_lines_of_one_modifier() {
    // `增加 79(75-79)% 物理傷害` and `+186(175-200) 命中值` are one physical
    // modifier, and the engine joins them because they sit together with no
    // blank line between.
    let result = evaluate(vec![AffixCondition::new(
        "hybrid",
        "增加 (75—79)% 物理傷害\n+(175—200) 命中值",
        vec![
            NumericConstraint::ignored(),
            NumericConstraint::ignored(),
        ],
    )]);
    assert!(result.is_match);
}

#[test]
fn two_separate_modifiers_are_never_joined_into_one() {
    // Without the blank line the parser writes between groups, the engine would
    // join `增加 179% 物理傷害` with the `增加 79% 物理傷害` beneath it and match
    // this template against two unrelated prefixes.
    let result = evaluate(vec![AffixCondition::new(
        "bogus-hybrid",
        "增加 (170—179)% 物理傷害\n增加 (75—79)% 物理傷害",
        vec![
            NumericConstraint::ignored(),
            NumericConstraint::ignored(),
        ],
    )]);
    assert!(
        !result.is_match,
        "adjacent independent modifiers must not combine into a hybrid candidate"
    );
}

#[test]
fn one_modifier_cannot_satisfy_two_conditions() {
    // Both conditions canonicalize to `增加 #% 物理傷害`. The bow has two such
    // prefixes, so both should be satisfied — by different modifiers.
    let result = evaluate(vec![
        AffixCondition::new("high", "增加 #% 物理傷害", vec![NumericConstraint::at_least(170.0)]),
        AffixCondition::new("low", "增加 #% 物理傷害", vec![NumericConstraint::at_least(75.0)]),
    ]);
    assert!(result.is_match);

    // Only one modifier clears 170, so two conditions demanding it cannot both
    // be met from a single physical modifier.
    let result = evaluate(vec![
        AffixCondition::new("high", "增加 #% 物理傷害", vec![NumericConstraint::at_least(170.0)]),
        AffixCondition::new("also-high", "增加 #% 物理傷害", vec![NumericConstraint::at_least(170.0)]),
    ]);
    assert!(!result.is_match);
}

#[test]
fn excluded_modifier_kinds_cannot_trigger_a_rule() {
    // `增加 25(21-25)% 暴擊率` belongs to the crafted suffix, which the default
    // filter drops, and `增加 200% 能力值需求` is an enchantment.
    let result = evaluate(vec![AffixCondition::new(
        "crafted-crit",
        "增加 (21—25)% 暴擊率",
        vec![NumericConstraint::ignored()],
    )]);
    assert!(!result.is_match, "crafted suffix must not be visible");

    let result = evaluate(vec![AffixCondition::new(
        "enchant",
        "增加 #% 能力值需求",
        vec![NumericConstraint::ignored()],
    )]);
    assert!(!result.is_match, "enchantment must not be visible");
}
