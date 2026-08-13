use poe_alarm_core::{
    AssistedModifierObservation, CompiledRuleSet, PhysicalLineIdentity, RuleSetDefinition,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Corpus {
    scenarios: Vec<Scenario>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Scenario {
    id: String,
    rule_set: RuleSetDefinition,
    cases: Vec<CorpusCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CorpusCase {
    id: String,
    expected_match: bool,
    expected_group: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    lines: Vec<String>,
    #[serde(default)]
    assisted_observations: Vec<CorpusAssistedObservation>,
    #[serde(default)]
    physical_lines: Vec<CorpusPhysicalLine>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CorpusAssistedObservation {
    physical_band_id: String,
    original_text: String,
    canonical_target: String,
    #[serde(default)]
    related_physical_band_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CorpusPhysicalLine {
    line_index: usize,
    physical_band_id: String,
}

#[test]
fn existing_mirror_tier_contract_is_preserved() {
    let corpus: Corpus = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/fixtures/mirror-tier-composite-rules.json"
    )))
    .expect("the shared mirror-tier fixture must deserialize");

    let mut evaluated = 0;
    let mut identity_contracts = 0;
    let mut failures = Vec::new();
    for scenario in corpus.scenarios {
        let rules = CompiledRuleSet::compile(scenario.rule_set)
            .unwrap_or_else(|error| panic!("{} failed to compile: {error}", scenario.id));
        for case in scenario.cases {
            evaluated += 1;
            let assisted: Vec<_> = case
                .assisted_observations
                .iter()
                .map(|observation| AssistedModifierObservation {
                    physical_band_id: observation.physical_band_id.clone(),
                    original_text: observation.original_text.clone(),
                    canonical_target: observation.canonical_target.clone(),
                    related_physical_band_ids: observation.related_physical_band_ids.clone(),
                })
                .collect();
            let identities: Vec<_> = case
                .physical_lines
                .iter()
                .map(|line| PhysicalLineIdentity {
                    line_index: line.line_index,
                    physical_band_id: line.physical_band_id.clone(),
                })
                .collect();
            let result = rules.evaluate_with_identity(&case.lines, &assisted, &identities);
            let group_matches = case
                .expected_group
                .as_deref()
                .is_none_or(|expected| result.matched_group_id() == Some(expected));
            if result.is_match != case.expected_match || !group_matches {
                failures.push(format!(
                    "{}/{} expected match={} group={:?}, got match={} group={:?}",
                    scenario.id,
                    case.id,
                    case.expected_match,
                    case.expected_group,
                    result.is_match,
                    result.matched_group_id()
                ));
            }

            if case.tags.iter().any(|tag| tag == "one-modifier-one-count") {
                identity_contracts += 1;
                let without_identity = rules.evaluate_with_identity(&case.lines, &assisted, &[]);
                if !without_identity.is_match || result.is_match {
                    failures.push(format!(
                        "{}/{} did not discriminate physical identity: without={}, with={}",
                        scenario.id, case.id, without_identity.is_match, result.is_match
                    ));
                }
            }
        }
    }

    assert_eq!(evaluated, 36, "fixture case count unexpectedly changed");
    assert!(identity_contracts > 0, "fixture lost its identity contract");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
