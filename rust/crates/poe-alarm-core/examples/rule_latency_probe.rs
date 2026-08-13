use std::hint::black_box;
use std::time::Instant;

use poe_alarm_core::{
    AcceptableResultGroup, AffixCondition, CompiledRuleSet, NumericConstraint, ResultGroupMode,
    RuleSetDefinition,
};

fn main() {
    let conditions = (0..20)
        .map(|index| {
            AffixCondition::new(
                format!("condition-{index}"),
                format!("+# to maximum TestStat{index}"),
                vec![NumericConstraint::at_least(index as f64)],
            )
        })
        .collect();
    let rules = CompiledRuleSet::compile(RuleSetDefinition {
        schema_version: 1,
        name: "20-condition latency probe".into(),
        groups: vec![AcceptableResultGroup {
            name: "twenty".into(),
            mode: ResultGroupMode::AtLeast,
            required_count: 6,
            conditions,
        }],
    })
    .expect("probe rule set must compile");
    let lines = (0..6)
        .map(|index| format!("+{} to maximum TestStat{index}", index + 100))
        .collect::<Vec<_>>();

    for _ in 0..1_000 {
        black_box(rules.evaluate(black_box(&lines)));
    }
    const ITERATIONS: u32 = 20_000;
    let started = Instant::now();
    let mut matches = 0;
    for _ in 0..ITERATIONS {
        matches += usize::from(black_box(rules.evaluate(black_box(&lines))).is_match);
    }
    let elapsed = started.elapsed();
    let micros = elapsed.as_secs_f64() * 1_000_000.0 / f64::from(ITERATIONS);
    assert_eq!(matches, ITERATIONS as usize);
    println!(
        "warm 20-condition rule evaluation: {micros:.3} us/evaluation ({ITERATIONS} iterations)"
    );
}
