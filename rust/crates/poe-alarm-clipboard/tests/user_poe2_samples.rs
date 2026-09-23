//! User-supplied POE2 captures from 2026-09-23. Expected rolls and groupings
//! below are read independently from the supplied text, never inferred from
//! the parser under test. No clipboard, game or other native IO is performed.
use poe_alarm_clipboard::{ParsedItem, parse};
use poe_alarm_core::{
    AcceptableResultGroup, AffixCondition, CompiledRuleSet, Decimal, NumericConstraint as N,
    ResultGroupMode as Mode, RuleEvaluationResult, RuleSetDefinition,
};

const CAPTURE: &str =
    include_str!("../../../../tests/fixtures/clipboard-items/poe2-en-user-20260923.txt");

fn captures() -> Vec<String> {
    CAPTURE
        .split("Item Class: ")
        .skip(1)
        .map(|body| format!("Item Class: {body}"))
        .collect()
}

fn item(index: usize) -> ParsedItem {
    parse(&captures()[index]).expect("real item parses")
}

fn condition(template: &str, constraints: Vec<N>) -> AffixCondition {
    AffixCondition::new(template, template, constraints)
}

fn evaluate(
    index: usize,
    mode: Mode,
    required_count: usize,
    conditions: Vec<AffixCondition>,
) -> RuleEvaluationResult {
    let rule = CompiledRuleSet::compile_with_maximum_line_span(
        RuleSetDefinition {
            schema_version: poe_alarm_core::rules::CURRENT_SCHEMA_VERSION,
            name: "user POE2".into(),
            groups: vec![AcceptableResultGroup {
                name: "target".into(),
                mode,
                required_count,
                conditions,
            }],
        },
        8,
    )
    .expect("valid POE2 rule");
    let (lines, identities) = item(index).render();
    rule.evaluate_with_identity(&lines, &[], &identities)
}

fn matches(index: usize, template: &str, constraints: Vec<N>) -> bool {
    evaluate(index, Mode::All, 1, vec![condition(template, constraints)]).is_match
}

#[test]
fn five_items_have_the_expected_complete_modifier_groups() {
    assert_eq!(captures().len(), 5);
    // Groups / physical modifier lines, counted from the original captures.
    for (index, class, groups, lines) in [
        (0, "Gloves", 7, 7),
        (1, "Helmets", 9, 10),
        (2, "Boots", 8, 9),
        (3, "Quarterstaves", 7, 8),
        (4, "Crossbows", 9, 10),
    ] {
        let parsed = item(index);
        assert_eq!(parsed.item_class.as_deref(), Some(class));
        assert!(parsed.grouping_is_authoritative);
        assert_eq!(parsed.groups.len(), groups, "{class}: {:?}", parsed.groups);
        assert_eq!(
            parsed.groups.iter().map(|g| g.lines.len()).sum::<usize>(),
            lines,
            "{class}"
        );
    }
}

#[test]
fn sanctified_is_item_status_and_cannot_be_imported_or_matched_as_a_modifier() {
    assert!(
        !item(3)
            .groups
            .iter()
            .flat_map(|g| &g.lines)
            .any(|line| line == "Sanctified")
    );
    assert!(!matches(3, "Sanctified", vec![]));
}

#[test]
fn properties_annotations_and_statuses_never_become_rule_evidence() {
    for index in 0..5 {
        for template in [
            "Quality: +#%",
            "Item Level: #",
            "Energy Shield: #",
            "Physical Damage: #-#",
            "Corrupted",
            "Fractured Item",
        ] {
            assert!(
                !matches(index, template, vec![]),
                "item {index}: {template}"
            );
        }
        let parsed = item(index);
        assert!(
            parsed
                .groups
                .iter()
                .flat_map(|g| &g.lines)
                .all(|line| !line.starts_with('{') && !line.contains("(rune)"))
        );
    }
}

#[test]
fn actual_rolls_are_used_even_outside_printed_ranges_or_with_fixed_parentheses() {
    for (index, template, actual) in [
        (0, "#% increased Attack Speed", 15),
        (0, "+# to Level of all Melee Skills", 2),
        (1, "+# to maximum Energy Shield", 72),
        (3, "#% increased Attack Speed", 22), // 22(23-25), not 23 or 25.
        (3, "+# to Level of all Attack Skills", 4), // +4(3), not 3.
        (4, "#% increased Attack Speed", 16),
        (4, "+# to Level of all Attack Skills", 3),
    ] {
        for (constraint, expected) in [
            (N::exactly(actual), true),
            (N::at_least(actual), true),
            (N::at_least(actual + 1), false),
            (N::at_most(actual), true),
            (N::at_most(actual - 1), false),
            (N::range(actual, actual), true),
        ] {
            assert_eq!(
                matches(index, template, vec![constraint.clone()]),
                expected,
                "item {index}: {template}, {constraint:?}"
            );
        }
    }
}

#[test]
fn all_damage_slots_and_decimal_thresholds_use_exact_actual_values() {
    assert!(matches(
        0,
        "Adds # to # Cold damage to Attacks",
        vec![N::exactly(24), N::exactly(35)]
    ));
    assert!(!matches(
        0,
        "Adds # to # Cold damage to Attacks",
        vec![N::ignored(), N::at_least(36)]
    ));
    assert!(matches(
        3,
        "Adds # to # Physical Damage",
        vec![N::exactly(64), N::exactly(109)]
    ));
    assert!(!matches(
        3,
        "Adds # to # Physical Damage",
        vec![N::at_most(55), N::ignored()]
    ));
    for (index, template, exact, higher) in [
        (2, "# Life Regeneration per second", "16.4", "16.41"),
        (3, "+#% to Critical Hit Chance", "4.78", "4.79"),
        (4, "+#% to Critical Hit Chance", "4.68", "4.69"),
    ] {
        let exact: Decimal = exact.parse().unwrap();
        let higher: Decimal = higher.parse().unwrap();
        assert!(matches(index, template, vec![N::exactly(exact)]));
        assert!(!matches(index, template, vec![N::at_least(higher)]));
    }
}

fn attack_conditions() -> Vec<AffixCondition> {
    vec![
        condition("#% increased Attack Speed", vec![N::at_least(15)]),
        condition(
            "+#% to Critical Hit Chance",
            vec![N::at_least("4.6".parse::<Decimal>().unwrap())],
        ),
        condition("+# to Level of all Attack Skills", vec![N::at_least(4)]),
    ]
}

#[test]
fn any_all_and_chosen_count_combinations_distinguish_all_five_real_items() {
    // Gloves satisfy only speed; staff all three; crossbow speed + crit.
    for (index, matched) in [(0, 1), (1, 0), (2, 0), (3, 3), (4, 2)] {
        for (mode, required, expected) in [
            (Mode::Any, 1, matched >= 1),
            (Mode::All, 3, matched == 3),
            (Mode::AtLeast, 2, matched >= 2),
            (Mode::AtLeast, 3, matched >= 3),
        ] {
            let result = evaluate(index, mode, required, attack_conditions());
            assert_eq!(
                result.is_match, expected,
                "item {index}, {mode:?}, count {required}"
            );
            assert_eq!(result.groups[0].matched_count, matched, "item {index}");
        }
    }
}

#[test]
fn one_real_hybrid_cannot_satisfy_two_separate_required_conditions() {
    for (index, first, first_value, second, second_value) in [
        (
            1,
            "#% increased Energy Shield",
            39,
            "+# to maximum Mana",
            37,
        ),
        (
            2,
            "+# to Evasion Rating",
            73,
            "+# to maximum Energy Shield",
            24,
        ),
        (
            3,
            "#% increased Physical Damage",
            68,
            "+# to Accuracy Rating",
            156,
        ),
        (
            4,
            "#% increased Physical Damage",
            57,
            "+# to Accuracy Rating",
            139,
        ),
    ] {
        assert!(matches(
            index,
            &format!("{first}\n{second}"),
            vec![N::exactly(first_value), N::exactly(second_value)]
        ));
        let split = || {
            vec![
                condition(first, vec![N::exactly(first_value)]),
                condition(second, vec![N::exactly(second_value)]),
            ]
        };
        assert!(!evaluate(index, Mode::All, 2, split()).is_match);
        assert!(!evaluate(index, Mode::AtLeast, 2, split()).is_match);
        assert!(evaluate(index, Mode::Any, 1, split()).is_match);
    }
}

#[test]
fn separate_modifiers_neither_merge_values_nor_form_a_false_hybrid() {
    assert!(matches(
        1,
        "#% increased Energy Shield",
        vec![N::exactly(95)]
    ));
    assert!(!matches(
        1,
        "#% increased Energy Shield",
        vec![N::at_least(96)]
    ));
    assert!(!matches(
        1,
        "#% increased Energy Shield\n+# to maximum Mana",
        vec![N::exactly(95), N::exactly(37)]
    ));
    assert!(
        evaluate(
            1,
            Mode::All,
            2,
            vec![
                condition("#% increased Energy Shield", vec![N::exactly(95)]),
                condition(
                    "#% increased Energy Shield\n+# to maximum Mana",
                    vec![N::exactly(39), N::exactly(37)]
                ),
            ]
        )
        .is_match
    );
    // 60 rune + 68 hybrid + 206 prefix must not become one 334% modifier.
    assert!(matches(
        3,
        "#% increased Physical Damage",
        vec![N::exactly(206)]
    ));
    assert!(!matches(
        3,
        "#% increased Physical Damage",
        vec![N::at_least(207)]
    ));
    assert!(!matches(
        3,
        "#% increased Physical Damage\n+# to Accuracy Rating",
        vec![N::exactly(206), N::exactly(156)]
    ));
}

#[test]
fn similar_words_and_different_damage_scopes_do_not_cross_match() {
    for template in [
        "#% increased Cast Speed",
        "+# to Level of all Attack Skills",
        "Adds # to # Fire damage to Attacks",
        "Adds # to # Physical Damage",
    ] {
        assert!(!matches(0, template, vec![]), "{template}");
    }
    assert!(!matches(
        3,
        "Adds # to # Physical Damage to Attacks",
        vec![]
    ));
    assert!(!matches(
        4,
        "Grenade Skills Fire an additional Projectile\n#% increased Physical Damage",
        vec![]
    ));
}

#[test]
fn enhancement_rune_implicit_and_unscalable_text_remain_usable() {
    for (index, template, constraints) in [
        (
            0,
            "Destroys all Augment Sockets on the item to create a Jewel Socket",
            vec![],
        ),
        (1, "Allocates Paragon", vec![]),
        (1, "+# to Maximum Power Charges", vec![N::exactly(2)]),
        (1, "Raven-Touched", vec![]),
        (2, "Your speed is unaffected by Slows", vec![]),
        (2, "Can roll Chronomancy modifiers", vec![]),
        (4, "Grenade Skills Fire an additional Projectile", vec![]),
        (
            4,
            "Gain #% of Damage as Extra Damage of all Elements",
            vec![N::exactly(5)],
        ),
    ] {
        assert!(matches(index, template, constraints), "{template}");
    }
}

#[test]
fn unchecked_conditions_do_not_contribute_to_or_block_real_item_combinations() {
    let mut conditions = attack_conditions();
    conditions[2].enabled = false;
    assert!(evaluate(4, Mode::All, 2, conditions.clone()).is_match);
    conditions[0].enabled = false;
    assert!(!evaluate(0, Mode::Any, 1, conditions).is_match);
}
