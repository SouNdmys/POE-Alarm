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
