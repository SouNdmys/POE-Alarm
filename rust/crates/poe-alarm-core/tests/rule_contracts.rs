use poe_alarm_core::{
    AcceptableResultGroup, AffixCondition, CompiledRuleSet, Decimal, NumericConstraint,
    NumericConstraintMode, ResultGroupMode, RuleSetDefinition,
};

fn one_condition(template: &str, constraints: Vec<NumericConstraint>) -> CompiledRuleSet {
    CompiledRuleSet::compile(RuleSetDefinition {
        schema_version: 1,
        name: "numeric".into(),
        groups: vec![AcceptableResultGroup {
            name: "result".into(),
            mode: ResultGroupMode::All,
            required_count: 1,
            conditions: vec![AffixCondition::new("condition", template, constraints)],
        }],
    })
    .unwrap()
}

#[test]
fn every_numeric_constraint_mode_has_inclusive_boundaries() {
    let range = one_condition(
        "#% increased Physical Damage",
        vec![NumericConstraint::range(170.0, 179.0)],
    );
    assert!(
        range
            .evaluate(&["170% increased Physical Damage".into()])
            .is_match
    );
    assert!(
        range
            .evaluate(&["179% increased Physical Damage".into()])
            .is_match
    );
    assert!(
        !range
            .evaluate(&["180% increased Physical Damage".into()])
            .is_match
    );

    let minimum = one_condition(
        "#% increased Attack Speed",
        vec![NumericConstraint::at_least(27.0)],
    );
    assert!(
        minimum
            .evaluate(&["27% increased Attack Speed".into()])
            .is_match
    );
    assert!(
        !minimum
            .evaluate(&["26% increased Attack Speed".into()])
            .is_match
    );

    let maximum = one_condition(
        "Regenerate # Life per second",
        vec![NumericConstraint::at_most(0.8)],
    );
    assert!(
        maximum
            .evaluate(&["Regenerate 0.8 Life per second".into()])
            .is_match
    );
    assert!(
        !maximum
            .evaluate(&["Regenerate 0.81 Life per second".into()])
            .is_match
    );

    let exact = one_condition("+# to maximum Life", vec![NumericConstraint::exactly(70.0)]);
    assert!(exact.evaluate(&["+70 to maximum Life".into()]).is_match);
    assert!(!exact.evaluate(&["+71 to maximum Life".into()]).is_match);
}

#[test]
fn cross_zero_ranges_use_the_displayed_signed_roll_for_numeric_constraints() {
    let rules = one_condition(
        "Breaches in Map have (-10—20)% reduced Pack Size",
        vec![NumericConstraint::range(-10, 20)],
    );
    for value in [-11, -10, -1, 0, 1, 20, 21] {
        for roll in [value.to_string(), format!("{value}(-10-20)")] {
            let result =
                rules.evaluate(&[format!("Breaches in Map have {roll}% reduced Pack Size")]);
            assert_eq!(result.is_match, (-10..=20).contains(&value), "{roll}");
            assert_eq!(
                result.groups[0].conditions[0].numeric_slots[0].actual_value,
                Some(Decimal::from(value))
            );
        }
    }
    let positive = one_condition(
        "Breaches in Map have (-10—20)% reduced Pack Size",
        vec![NumericConstraint::at_least(10)],
    );
    assert!(
        positive
            .evaluate(&["Breaches in Map have 20% reduced Pack Size".into()])
            .is_match
    );
    assert!(
        !positive
            .evaluate(&["Breaches in Map have -10% reduced Pack Size".into()])
            .is_match
    );
}

#[test]
fn saved_unchecked_rules_do_not_consume_monitoring_budgets() {
    use poe_alarm_core::rules::{MAXIMUM_SAVED_CONDITIONS, MAXIMUM_SAVED_GROUPS};

    let inactive = AffixCondition {
        enabled: false,
        ..Default::default()
    };
    let mut definition = RuleSetDefinition {
        groups: (0..MAXIMUM_SAVED_GROUPS)
            .map(|_| AcceptableResultGroup {
                conditions: vec![inactive.clone(); MAXIMUM_SAVED_CONDITIONS / MAXIMUM_SAVED_GROUPS],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let last_group = definition.groups.len() - 1;
    let last_condition = definition.groups[last_group].conditions.len() - 1;
    definition.groups[last_group].conditions[last_condition] = AffixCondition::new(
        "life",
        "+# to maximum Life",
        vec![NumericConstraint::at_least(70)],
    );
    let rules = CompiledRuleSet::compile(definition.clone()).unwrap();
    assert_eq!(rules.definition(), &definition);
    assert_eq!(rules.targets().len(), 1);
    let result = rules.evaluate(&["+70 to maximum Life".into()]);
    assert_eq!(result.matched_group_index, Some(last_group));
    assert_eq!(
        result.matched_group().unwrap().conditions[0].condition_index,
        last_condition
    );
    assert!(!rules.evaluate(&["+69 to maximum Life".into()]).is_match);

    let mut too_many_conditions = definition.clone();
    too_many_conditions.groups[0]
        .conditions
        .push(inactive.clone());
    assert!(
        CompiledRuleSet::compile(too_many_conditions)
            .unwrap_err()
            .to_string()
            .contains("save at most 1024 conditions")
    );
    definition.groups.push(AcceptableResultGroup {
        conditions: vec![inactive],
        ..Default::default()
    });
    assert!(
        CompiledRuleSet::compile(definition)
            .unwrap_err()
            .to_string()
            .contains("save at most 128 acceptable results")
    );
}

#[test]
fn active_rule_budgets_still_reject_excess_enabled_conditions_or_groups() {
    use poe_alarm_core::rules::{MAXIMUM_CONDITIONS, MAXIMUM_GROUPS};

    let mut definition = RuleSetDefinition {
        groups: vec![AcceptableResultGroup {
            conditions: (0..MAXIMUM_CONDITIONS)
                .map(|index| {
                    AffixCondition::new(format!("life {index}"), "+# to maximum Life", vec![])
                })
                .collect(),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(CompiledRuleSet::compile(definition.clone()).is_ok());
    definition.groups[0].conditions.push(AffixCondition::new(
        "one too many",
        "+# to maximum Mana",
        vec![],
    ));
    assert!(
        CompiledRuleSet::compile(definition)
            .unwrap_err()
            .to_string()
            .contains("at most 32 enabled conditions")
    );

    let mut definition = RuleSetDefinition {
        groups: (0..MAXIMUM_GROUPS)
            .map(|_| AcceptableResultGroup {
                conditions: vec![AffixCondition::new("life", "+# to maximum Life", vec![])],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    assert!(CompiledRuleSet::compile(definition.clone()).is_ok());
    definition.groups.push(definition.groups[0].clone());
    assert!(
        CompiledRuleSet::compile(definition)
            .unwrap_err()
            .to_string()
            .contains("at most 8 acceptable results at once")
    );
}

#[test]
fn rolled_ranges_expose_only_displayed_rolls_in_slot_order() {
    let rules = one_condition(
        "Adds # to # Physical Damage",
        vec![
            NumericConstraint::at_least(37.0),
            NumericConstraint::at_least(63.0),
        ],
    );
    let result = rules.evaluate(&["Adds 55(37-55) to 94(63-94) Physical Damage".into()]);
    assert!(result.is_match);
    assert_eq!(
        result.groups[0].conditions[0]
            .observation
            .as_ref()
            .unwrap()
            .numeric_values,
        vec![Some(Decimal::from(55)), Some(Decimal::from(94))]
    );
}

#[test]
fn any_acceptable_result_can_stop_and_first_match_is_reported() {
    let rules = CompiledRuleSet::compile(RuleSetDefinition {
        schema_version: 1,
        name: "or".into(),
        groups: vec![
            AcceptableResultGroup {
                name: "physical".into(),
                mode: ResultGroupMode::All,
                required_count: 1,
                conditions: vec![AffixCondition::new(
                    "physical",
                    "#% increased Physical Damage",
                    vec![NumericConstraint::at_least(170.0)],
                )],
            },
            AcceptableResultGroup {
                name: "life".into(),
                mode: ResultGroupMode::Any,
                required_count: 1,
                conditions: vec![AffixCondition::new(
                    "life",
                    "+# to maximum Life",
                    vec![NumericConstraint::at_least(70.0)],
                )],
            },
        ],
    })
    .unwrap();
    assert_eq!(
        rules
            .evaluate(&[
                "178% increased Physical Damage".into(),
                "+75 to maximum Life".into(),
            ])
            .matched_group_id(),
        Some("physical")
    );
    assert_eq!(
        rules
            .evaluate(&["+75 to maximum Life".into()])
            .matched_group_id(),
        Some("life")
    );
}

#[test]
fn existing_rule_json_schema_deserializes_without_translation() {
    let json = r##"{
      "schemaVersion": 1,
      "name": "saved profile",
      "groups": [{
        "name": "two of two",
        "mode": "AtLeast",
        "requiredCount": 2,
        "conditions": [
          {"name":"speed","template":"#% increased Attack Speed","numericConstraints":[{"mode":"AtLeast","minimum":20.5}]},
          {"name":"life","template":"+# to maximum Life","numericConstraints":[{"mode":"Ignore"}]}
        ]
      }]
    }"##;
    let definition: RuleSetDefinition = serde_json::from_str(json).unwrap();
    assert!(
        definition.groups[0]
            .conditions
            .iter()
            .all(|condition| condition.enabled)
    );
    assert_eq!(definition.groups[0].mode, ResultGroupMode::AtLeast);
    assert_eq!(
        definition.groups[0].conditions[0].numeric_constraints[0].mode,
        NumericConstraintMode::AtLeast
    );
    let serialized = serde_json::to_string(&definition).unwrap();
    assert!(serialized.contains("\"minimum\":20.5"));
    assert!(!serialized.contains("\"minimum\":\"20.5\""));
    assert!(
        CompiledRuleSet::compile(definition)
            .unwrap()
            .evaluate(&[
                "21% increased Attack Speed".into(),
                "+70 to maximum Life".into(),
            ])
            .is_match
    );
}

#[test]
fn invalid_thresholds_and_duplicate_names_are_rejected() {
    let duplicate = RuleSetDefinition {
        schema_version: 1,
        name: "invalid".into(),
        groups: vec![AcceptableResultGroup {
            name: "group".into(),
            mode: ResultGroupMode::AtLeast,
            required_count: 3,
            conditions: vec![
                AffixCondition::new("same", "+# to maximum Life", vec![]),
                AffixCondition::new("same", "+# to maximum Mana", vec![]),
            ],
        }],
    };
    let error = CompiledRuleSet::compile(duplicate).unwrap_err();
    assert!(
        error
            .errors
            .iter()
            .any(|item| item.contains("requires between"))
    );
    assert!(
        error
            .errors
            .iter()
            .any(|item| item.contains("duplicate condition"))
    );
}

#[test]
fn unchecked_affixes_are_saved_but_excluded_from_every_match_mode() {
    let mut speed = AffixCondition::new("speed", "#% increased Attack Speed", vec![]);
    speed.enabled = false;
    for mode in [
        ResultGroupMode::Any,
        ResultGroupMode::All,
        ResultGroupMode::AtLeast,
    ] {
        let definition = RuleSetDefinition {
            groups: vec![AcceptableResultGroup {
                mode,
                required_count: 2,
                conditions: vec![
                    speed.clone(),
                    AffixCondition::new(
                        "life",
                        "+# to maximum Life",
                        vec![NumericConstraint::at_least(70)],
                    ),
                    AffixCondition::new("mana", "+# to maximum Mana", vec![]),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let rules = CompiledRuleSet::compile(definition.clone()).unwrap();
        assert_eq!(rules.definition(), &definition);
        assert_eq!(rules.targets().len(), 2);
        assert!(
            !rules
                .evaluate(&["30% increased Attack Speed".into()])
                .is_match
        );
        assert_eq!(
            rules.evaluate(&["+70 to maximum Life".into()]).is_match,
            mode == ResultGroupMode::Any
        );
        let result = rules.evaluate(&["+70 to maximum Life".into(), "+40 to maximum Mana".into()]);
        assert!(result.is_match);
        assert_eq!(result.groups[0].matched_count, 2);
        assert_eq!(
            result.groups[0]
                .conditions
                .iter()
                .map(|condition| condition.condition_index)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        // Rechecking restores the condition, including its effect on All.
        let mut restored = definition;
        restored.groups[0].conditions[0].enabled = true;
        let restored = CompiledRuleSet::compile(restored).unwrap();
        assert_eq!(restored.targets().len(), 3);
        assert_eq!(
            restored
                .evaluate(&["+70 to maximum Life".into(), "+40 to maximum Mana".into()])
                .is_match,
            mode != ResultGroupMode::All
        );
    }
}

#[test]
fn fully_unchecked_groups_never_match_and_preserve_result_indices() {
    for mode in [
        ResultGroupMode::Any,
        ResultGroupMode::All,
        ResultGroupMode::AtLeast,
    ] {
        let rules = CompiledRuleSet::compile(RuleSetDefinition {
            groups: vec![
                AcceptableResultGroup {
                    mode,
                    // An inactive group's threshold and incomplete template are ignored.
                    required_count: 0,
                    conditions: vec![AffixCondition {
                        enabled: false,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                AcceptableResultGroup {
                    conditions: vec![AffixCondition::new("life", "+# to maximum Life", vec![])],
                    ..Default::default()
                },
            ],
            ..Default::default()
        })
        .unwrap();
        assert!(!rules.evaluate(&[]).is_match);
        let result = rules.evaluate(&["+70 to maximum Life".into()]);
        assert!(!result.groups[0].is_match);
        assert!(result.groups[0].conditions.is_empty());
        assert_eq!(result.matched_group_index, Some(1));
        assert_eq!(result.matched_group().unwrap().group_index, 1);
    }
}

#[test]
fn unchecking_cannot_silently_reduce_a_required_count_or_enable_empty_monitoring() {
    let mut definition = RuleSetDefinition {
        groups: vec![AcceptableResultGroup {
            mode: ResultGroupMode::AtLeast,
            required_count: 2,
            conditions: vec![
                AffixCondition::new("life", "+# to maximum Life", vec![]),
                AffixCondition::new("mana", "+# to maximum Mana", vec![]),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(CompiledRuleSet::compile(definition.clone()).is_ok());
    definition.groups[0].conditions[0].enabled = false;
    assert!(
        CompiledRuleSet::compile(definition.clone())
            .unwrap_err()
            .to_string()
            .contains("requires between 1 and 1 enabled conditions")
    );
    definition.groups[0].conditions[1].enabled = false;
    assert!(
        CompiledRuleSet::compile(definition)
            .unwrap_err()
            .to_string()
            .contains("at least one enabled condition")
    );
}

#[test]
fn unchecked_invalid_constraints_do_not_block_monitoring_and_are_restored_when_rechecked() {
    let mut definition = RuleSetDefinition {
        groups: vec![AcceptableResultGroup {
            conditions: vec![
                AffixCondition::new("life", "+# to maximum Life", vec![]),
                AffixCondition {
                    enabled: false,
                    // Duplicated name and invalid range can be saved for later.
                    name: "life".into(),
                    template: "#% increased Attack Speed".into(),
                    numeric_constraints: vec![NumericConstraint::range(30, 20)],
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let serialized = serde_json::to_string(&definition).unwrap();
    let reloaded: RuleSetDefinition = serde_json::from_str(&serialized).unwrap();
    assert_eq!(reloaded, definition);
    assert!(
        CompiledRuleSet::compile(reloaded)
            .unwrap()
            .evaluate(&["+70 to maximum Life".into()])
            .is_match
    );
    definition.groups[0].conditions[1].enabled = true;
    assert!(CompiledRuleSet::compile(definition).is_err());
    assert!(AffixCondition::default().enabled);
}

#[test]
fn decimal_boundaries_never_use_binary_rounding() {
    let at_least = one_condition(
        "#% increased Critical Strike Chance",
        vec![NumericConstraint::at_least(
            "3.1".parse::<Decimal>().unwrap(),
        )],
    );
    assert!(
        at_least
            .evaluate(&["3.10% increased Critical Strike Chance".into()])
            .is_match
    );
    assert!(
        !at_least
            .evaluate(&["3.099999% increased Critical Strike Chance".into()])
            .is_match
    );

    let exact = one_condition(
        "#% increased Critical Strike Chance",
        vec![NumericConstraint::exactly(
            "3.10".parse::<Decimal>().unwrap(),
        )],
    );
    assert!(
        exact
            .evaluate(&["3.1% increased Critical Strike Chance".into()])
            .is_match
    );

    let range = one_condition(
        "#% increased Critical Strike Chance",
        vec![NumericConstraint::range(
            "3.1".parse::<Decimal>().unwrap(),
            "3.8".parse::<Decimal>().unwrap(),
        )],
    );
    assert!(
        range
            .evaluate(&["3.1% increased Critical Strike Chance".into()])
            .is_match
    );
    assert!(
        range
            .evaluate(&["3.8% increased Critical Strike Chance".into()])
            .is_match
    );

    let negative = one_condition(
        "-# to Mana Cost",
        vec![NumericConstraint::at_least(
            "-3.1".parse::<Decimal>().unwrap(),
        )],
    );
    assert!(negative.evaluate(&["-3.1 to Mana Cost".into()]).is_match);
    assert!(
        !negative
            .evaluate(&["-3.100001 to Mana Cost".into()])
            .is_match
    );
}
