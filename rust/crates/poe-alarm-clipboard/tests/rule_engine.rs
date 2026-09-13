//! End-to-end: a real clipboard dump through the parser and into the rule
//! engine, checking the two failure modes that motivated the whole exercise.
//!
//! The tooltip merges modifiers granting the same stat, so a bow carrying a
//! 179% physical prefix alongside a 79% physical hybrid displays `258%` — a
//! number no modifier on the item actually rolled. OCR could only ever see
//! that merged figure. These tests pin down that the clipboard path does not.

use poe_alarm_clipboard::parse;
use poe_alarm_core::rules::CURRENT_SCHEMA_VERSION;
use poe_alarm_core::{
    AcceptableResultGroup, AffixCondition, CompiledRuleSet, NumericConstraint, ResultGroupMode,
    RuleEvaluationResult, RuleSetDefinition,
};

const BOW: &str =
    include_str!("../../../../tests/fixtures/clipboard-items/poe1-tw-rare-bow-hybrid.txt");
const SPEAR: &str =
    include_str!("../../../../tests/fixtures/clipboard-items/poe2-tw-rare-spear-unscalable.txt");
const JEWEL: &str = include_str!(
    "../../../../tests/fixtures/clipboard-items/poe2-en-rare-jewel-quality-crafted.txt"
);
const BOOTS: &str =
    include_str!("../../../../tests/fixtures/clipboard-items/poe2-en-rare-boots-hybrid-speed.txt");

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
    let item = parse(BOW).expect("bow parses");
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
    assert!(
        result.is_match,
        "179 is the rolled value and meets the floor"
    );

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
        vec![NumericConstraint::ignored(), NumericConstraint::ignored()],
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
        vec![NumericConstraint::ignored(), NumericConstraint::ignored()],
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
        AffixCondition::new(
            "high",
            "增加 #% 物理傷害",
            vec![NumericConstraint::at_least(170.0)],
        ),
        AffixCondition::new(
            "low",
            "增加 #% 物理傷害",
            vec![NumericConstraint::at_least(75.0)],
        ),
    ]);
    assert!(result.is_match);

    // Only one modifier clears 170, so two conditions demanding it cannot both
    // be met from a single physical modifier.
    let result = evaluate(vec![
        AffixCondition::new(
            "high",
            "增加 #% 物理傷害",
            vec![NumericConstraint::at_least(170.0)],
        ),
        AffixCondition::new(
            "also-high",
            "增加 #% 物理傷害",
            vec![NumericConstraint::at_least(170.0)],
        ),
    ]);
    assert!(!result.is_match);
}

#[test]
fn a_modifier_matches_whatever_the_client_says_produced_it() {
    // `增加 25(21-25)% 暴擊率` belongs to a bench-crafted suffix and
    // `增加 200% 能力值需求` to an enchantment. Both reach the rules.
    //
    // These used to be excluded, on the theory that a crafting bench is not
    // something you roll for. The theory is fine and the mechanism was not:
    // POE1 has more modifier sources than can be enumerated, exclusion depends
    // on recognising the word the client used for each one, and a word this
    // parser has not seen would silently stop an alarm from firing on an item
    // worth real money. A false alarm costs a glance. Nothing else here is
    // allowed to cost more than that.
    let result = evaluate(vec![AffixCondition::new(
        "crafted-crit",
        "增加 (21—25)% 暴擊率",
        vec![NumericConstraint::ignored()],
    )]);
    assert!(result.is_match, "a crafted suffix is still a modifier");

    let result = evaluate(vec![AffixCondition::new(
        "enchant",
        "增加 #% 能力值需求",
        vec![NumericConstraint::ignored()],
    )]);
    assert!(result.is_match, "an enchantment is still a modifier");
}

/// The spear's Thrud's prefix ends in ` — 無法變動的值`, the client's value
/// annotation. The field failure this pins down: the tail's tokens made the
/// strict matcher refuse the line, so the rule tracking the modifier never
/// fired on a real item.
#[test]
fn a_value_annotation_tail_does_not_cost_the_user_the_hit() {
    let item = parse(SPEAR).expect("spear parses");
    let (lines, identities) = item.render();
    let condition =
        |constraints| AffixCondition::new("thrud", "增加(25—30)%變動速度詞綴的大小", constraints);

    let result = rules(vec![condition(vec![NumericConstraint::ignored()])]).evaluate_with_identity(
        &lines,
        &[],
        &identities,
    );
    assert!(result.is_match, "the tail must not block the match");

    // The rolled value still reads as 30, untouched by the strip.
    let floored = rules(vec![condition(vec![NumericConstraint::at_least(30.0)])])
        .evaluate_with_identity(&lines, &[], &identities);
    assert!(floored.is_match, "the rolled 30 meets a floor of 30");
    let exceeded = rules(vec![condition(vec![NumericConstraint::at_least(31.0)])])
        .evaluate_with_identity(&lines, &[], &identities);
    assert!(
        !exceeded.is_match,
        "the rolled 30 must not pass a floor of 31"
    );
}

/// The jewel's tooltip shows suffix values scaled by quality and a crafted
/// "increased Effect of Suffixes" — the client declares the multiplier in the
/// annotation (`— 80% Increased`) and its crit suffix displays as 36, not 20.
/// Ctrl+C carries the base roll, and judgement deliberately uses only that:
/// rolled and displayed values map one-to-one per modifier, so a base-value
/// threshold loses nothing, while re-deriving the display would gamble on
/// unverified rounding at exactly the boundaries where money changes hands.
#[test]
fn judgement_reads_the_base_roll_not_the_scaled_display() {
    let item = parse(JEWEL).expect("jewel parses");
    let (lines, identities) = item.render();
    let crit = |constraints| {
        AffixCondition::new(
            "crit",
            "Minions have (10—20)% increased Critical Hit Chance",
            constraints,
        )
    };

    let rolled = rules(vec![crit(vec![NumericConstraint::at_least(20.0)])]).evaluate_with_identity(
        &lines,
        &[],
        &identities,
    );
    assert!(rolled.is_match, "the rolled 20 meets a floor of 20");
    let displayed = rules(vec![crit(vec![NumericConstraint::at_least(36.0)])])
        .evaluate_with_identity(&lines, &[], &identities);
    assert!(
        !displayed.is_match,
        "the tooltip's scaled 36 must not be what judgement compares"
    );
}

/// The crafted prefix carries the annotation tail and an empty affix name
/// (`{ Crafted Prefix Modifier "" }`); both survive parsing and the modifier
/// stays matchable.
#[test]
fn a_crafted_modifier_with_an_annotation_tail_still_matches() {
    let item = parse(JEWEL).expect("jewel parses");
    let (lines, identities) = item.render();
    let result = rules(vec![AffixCondition::new(
        "suffix-effect",
        "(40—60)% increased Effect of Suffixes",
        vec![NumericConstraint::at_least(60.0)],
    )])
    .evaluate_with_identity(&lines, &[], &identities);
    assert!(result.is_match);
}

fn boots_match(conditions: Vec<AffixCondition>) -> RuleEvaluationResult {
    let item = parse(BOOTS).expect("boots parse");
    let (lines, identities) = item.render();
    rules(conditions).evaluate_with_identity(&lines, &[], &identities)
}

const SPEED_HALF: &str = "(30—32)% increased Movement Speed";
const SLOW_HALF: &str = "(21—25)% reduced Slowing Potency of Debuffs on You";

/// Uhtred's is one modifier described on two lines. Written as one condition
/// with both lines — pasted with the newline PoE2DB puts between them — each
/// line's value is its own slot. The client writes the second line's range
/// high to low, `21(25-21)`, and it still reads as the rolled 21.
#[test]
fn a_hybrid_is_one_condition_with_one_slot_per_line() {
    let joined = format!("{SPEED_HALF}\n{SLOW_HALF}");
    let hit = boots_match(vec![AffixCondition::new(
        "uhtred",
        &joined,
        vec![
            NumericConstraint::at_least(31.0),
            NumericConstraint::at_least(21.0),
        ],
    )]);
    assert!(hit.is_match);
    let miss = boots_match(vec![AffixCondition::new(
        "uhtred",
        &joined,
        vec![
            NumericConstraint::at_least(31.0),
            NumericConstraint::at_least(22.0),
        ],
    )]);
    assert!(!miss.is_match, "the second slot reads the rolled 21");
}

/// Either line of a hybrid can be tracked on its own as a one-line template.
#[test]
fn each_half_of_a_hybrid_can_be_tracked_alone() {
    let speed = boots_match(vec![AffixCondition::new(
        "speed",
        SPEED_HALF,
        vec![NumericConstraint::at_least(32.0)],
    )]);
    assert!(speed.is_match);
    let slow = boots_match(vec![AffixCondition::new(
        "slow",
        SLOW_HALF,
        vec![NumericConstraint::at_least(21.0)],
    )]);
    assert!(
        slow.is_match,
        "an inverted tier range still yields the rolled value"
    );
}

/// Split into two conditions, a hybrid can never satisfy both: one physical
/// modifier satisfies at most one condition, by design — the same rule that
/// stops one line from being counted twice toward an "any N" group. The help
/// text steers users to the one-condition form above; this pins the reason.
#[test]
fn a_hybrid_split_into_two_conditions_cannot_satisfy_both() {
    let both = boots_match(vec![
        AffixCondition::new("speed", SPEED_HALF, vec![NumericConstraint::at_least(31.0)]),
        AffixCondition::new("slow", SLOW_HALF, vec![NumericConstraint::at_least(21.0)]),
    ]);
    assert!(!both.is_match);
    assert_eq!(both.groups[0].matched_count, 1);
}
