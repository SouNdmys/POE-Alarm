use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use poe_alarm_core::{FullLineAffixMatcher, canonicalize};
use serde::Deserialize;

const POEDB_EXPECTED_CASES: usize = 163;
const POEDB_EXPECTED_UNIQUE_TEMPLATES: usize = 153;
const POEDB_EXPECTED_TEMPLATE_PAIRS: usize = 13_203;
const POEDB_EXPECTED_EXACT_DUPLICATE_PAIRS: usize = 11;
const POEDB_EXPECTED_SIMILARITY_DIRECTED: usize = 472;

const POE2DB_EXPECTED_CASES: usize = 167;
const POE2DB_EXPECTED_POSITIVES: usize = 334;
const POE2DB_EXPECTED_MISSING_LINE_NEGATIVES: usize = 562;
const POE2DB_EXPECTED_NUMERIC_TEMPLATES: usize = 334;
const POE2DB_EXPECTED_RANGE_POSITIVES: usize = 298;
const POE2DB_EXPECTED_PHYSICAL_WRAPS: usize = 2;
const POE2_MAXIMUM_PHYSICAL_LINE_SPAN: usize = 8;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PoedbCorpus {
    cases: Vec<PoedbCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PoedbCase {
    id: String,
    template: String,
    similarity_group: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Poe2Corpus {
    cases: Vec<Poe2Case>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Poe2Case {
    id: String,
    display_lines: BTreeMap<String, Vec<String>>,
    numeric_templates: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    physical_line_variants: BTreeMap<String, Vec<Vec<String>>>,
}

#[test]
fn poedb_traditional_full_corpus_preserves_strict_match_contract() {
    let path = repository_root().join("tests/screenshots/poedb-traditional-affix-corpus.json");
    let corpus: PoedbCorpus = read_json(&path);
    assert_eq!(corpus.cases.len(), POEDB_EXPECTED_CASES);

    let mut positive_failures = Vec::new();
    for item in &corpus.cases {
        let matcher = matcher(&item.template, 4);
        let rendered = materialize_numeric_slots(&item.template);
        let lines = physical_lines(&rendered);
        if matcher.find_match(&lines).is_none() {
            positive_failures.push(item.id.clone());
        }
    }
    assert!(
        positive_failures.is_empty(),
        "PoEDB positives failed: {positive_failures:?}"
    );

    let mut canonical_groups: BTreeMap<String, Vec<&PoedbCase>> = BTreeMap::new();
    for item in &corpus.cases {
        canonical_groups
            .entry(canonicalize(&item.template).text)
            .or_default()
            .push(item);
    }
    assert_eq!(canonical_groups.len(), POEDB_EXPECTED_UNIQUE_TEMPLATES);

    let unique_cases: Vec<&PoedbCase> = canonical_groups
        .values()
        .filter_map(|group| group.first().copied())
        .collect();
    let mut missing_line_checks = 0_usize;
    let mut missing_line_false_matches = Vec::new();
    for item in unique_cases {
        let authored_lines = physical_lines(&item.template);
        if authored_lines.len() <= 1 {
            continue;
        }
        let target = matcher(&item.template, 4);
        for omitted in 0..authored_lines.len() {
            missing_line_checks += 1;
            let incomplete = authored_lines
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != omitted)
                .map(|(_, line)| materialize_numeric_slots(line))
                .collect::<Vec<_>>();
            if target.find_match(&incomplete).is_some() {
                missing_line_false_matches.push(format!("{} line {omitted}", item.id));
            }
        }
    }
    assert_eq!(missing_line_checks, 58);
    assert!(
        missing_line_false_matches.is_empty(),
        "PoEDB missing-line false matches: {missing_line_false_matches:?}"
    );

    let mut similarity_groups: BTreeMap<&str, Vec<&PoedbCase>> = BTreeMap::new();
    for item in &corpus.cases {
        if let Some(group) = item.similarity_group.as_deref()
            && !group.trim().is_empty()
        {
            similarity_groups.entry(group).or_default().push(item);
        }
    }
    let mut similarity_directed = 0_usize;
    let mut similarity_false_matches = Vec::new();
    for group in similarity_groups.values() {
        for source in group {
            let rendered = materialize_numeric_slots(&source.template);
            let source_canonical = canonicalize(&source.template).text;
            for target in group {
                let target_canonical = canonicalize(&target.template).text;
                if source.id == target.id || source_canonical == target_canonical {
                    continue;
                }
                similarity_directed += 1;
                if matcher(&target.template, 4).is_match(&rendered) {
                    similarity_false_matches.push(format!("{} -> {}", source.id, target.id));
                }
            }
        }
    }
    assert_eq!(similarity_directed, POEDB_EXPECTED_SIMILARITY_DIRECTED);
    assert!(
        similarity_false_matches.is_empty(),
        "PoEDB labelled near-neighbours collided: {similarity_false_matches:?}"
    );

    let mut template_pairs = 0_usize;
    let mut exact_duplicate_pairs = 0_usize;
    let mut pair_collisions = Vec::new();
    for left_index in 0..corpus.cases.len() {
        let left = &corpus.cases[left_index];
        let left_matcher = matcher(&left.template, 4);
        for right in &corpus.cases[left_index + 1..] {
            template_pairs += 1;
            let right_matcher = matcher(&right.template, 4);
            if left_matcher.template().text == right_matcher.template().text {
                exact_duplicate_pairs += 1;
                continue;
            }
            if left_matcher.is_match(&materialize_numeric_slots(&right.template))
                || right_matcher.is_match(&materialize_numeric_slots(&left.template))
            {
                pair_collisions.push(format!("{} <=> {}", left.id, right.id));
            }
        }
    }
    assert_eq!(template_pairs, POEDB_EXPECTED_TEMPLATE_PAIRS);
    assert_eq!(exact_duplicate_pairs, POEDB_EXPECTED_EXACT_DUPLICATE_PAIRS);
    assert!(
        pair_collisions.is_empty(),
        "PoEDB canonical template collisions: {pair_collisions:?}"
    );
}

#[test]
fn poe2db_bilingual_full_corpus_preserves_all_hard_invariants() {
    let path = repository_root().join("tests/screenshots/poe2/poe2db-affix-corpus.json");
    let corpus: Poe2Corpus = read_json(&path);
    assert_eq!(corpus.cases.len(), POE2DB_EXPECTED_CASES);

    let mut positives = 0_usize;
    let mut positive_failures = Vec::new();
    let mut missing_line_negatives = 0_usize;
    let mut missing_line_false_matches = Vec::new();
    let mut numeric_templates = 0_usize;
    let mut numeric_template_failures = Vec::new();
    let mut range_positives = 0_usize;
    let mut range_failures = Vec::new();
    let mut physical_wraps = 0_usize;
    let mut physical_wrap_failures = Vec::new();

    for item in &corpus.cases {
        for locale in ["en-US", "zh-TW"] {
            let display = item
                .display_lines
                .get(locale)
                .unwrap_or_else(|| panic!("{}/{locale} has no display lines", item.id));
            let numeric = item
                .numeric_templates
                .get(locale)
                .unwrap_or_else(|| panic!("{}/{locale} has no numeric templates", item.id));
            assert_eq!(
                display.len(),
                numeric.len(),
                "{}/{locale} display/template line count",
                item.id
            );
            assert!(
                (1..=POE2_MAXIMUM_PHYSICAL_LINE_SPAN).contains(&display.len()),
                "{}/{locale} exceeds the eight-line production span",
                item.id
            );

            let display_template = display.join(" ");
            let display_matcher = matcher(&display_template, POE2_MAXIMUM_PHYSICAL_LINE_SPAN);
            positives += 1;
            if display_matcher.find_match(display).is_none() {
                positive_failures.push(format!("{}/{locale}", item.id));
            }

            for omitted in 0..display.len() {
                missing_line_negatives += 1;
                let incomplete = display
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != omitted)
                    .map(|(_, line)| line.clone())
                    .collect::<Vec<_>>();
                if display_matcher.find_match(&incomplete).is_some() {
                    missing_line_false_matches.push(format!("{}/{locale} line {omitted}", item.id));
                }
            }

            numeric_templates += 1;
            let numeric_matcher = matcher(&numeric.join(" "), POE2_MAXIMUM_PHYSICAL_LINE_SPAN);
            if numeric_matcher.find_match(display).is_none() {
                numeric_template_failures.push(format!("{}/{locale}", item.id));
            }

            let materialized = display
                .iter()
                .map(|line| materialize_parenthesized_ranges(line))
                .collect::<Vec<_>>();
            if materialized != *display {
                range_positives += 1;
                if display_matcher.find_match(&materialized).is_none() {
                    range_failures.push(format!("{}/{locale}", item.id));
                }
            }

            if let Some(variants) = item.physical_line_variants.get(locale) {
                for variant in variants {
                    physical_wraps += 1;
                    assert!(
                        (1..=POE2_MAXIMUM_PHYSICAL_LINE_SPAN).contains(&variant.len()),
                        "{}/{locale} invalid physical wrap length",
                        item.id
                    );
                    if display_matcher.find_match(variant).is_none() {
                        physical_wrap_failures.push(format!("{}/{locale}", item.id));
                    }
                }
            }
        }
    }

    assert_eq!(positives, POE2DB_EXPECTED_POSITIVES);
    assert_eq!(
        missing_line_negatives,
        POE2DB_EXPECTED_MISSING_LINE_NEGATIVES
    );
    assert_eq!(numeric_templates, POE2DB_EXPECTED_NUMERIC_TEMPLATES);
    assert_eq!(range_positives, POE2DB_EXPECTED_RANGE_POSITIVES);
    assert_eq!(physical_wraps, POE2DB_EXPECTED_PHYSICAL_WRAPS);
    assert!(
        positive_failures.is_empty(),
        "PoE2DB positives failed: {positive_failures:?}"
    );
    assert!(
        missing_line_false_matches.is_empty(),
        "PoE2DB missing-line false matches: {missing_line_false_matches:?}"
    );
    assert!(
        numeric_template_failures.is_empty(),
        "PoE2DB numeric templates failed: {numeric_template_failures:?}"
    );
    assert!(
        range_failures.is_empty(),
        "PoE2DB concrete roll ranges failed: {range_failures:?}"
    );
    assert!(
        physical_wrap_failures.is_empty(),
        "PoE2DB physical wraps failed: {physical_wrap_failures:?}"
    );
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate must be under rust/crates")
        .to_path_buf()
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    let json = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&json)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn matcher(template: &str, maximum_line_span: usize) -> FullLineAffixMatcher {
    FullLineAffixMatcher::with_maximum_line_span(template, maximum_line_span)
        .unwrap_or_else(|error| panic!("failed to compile {template:?}: {error}"))
}

fn physical_lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .split('\n')
        .map(str::to_owned)
        .collect()
}

fn materialize_numeric_slots(template: &str) -> String {
    const VALUES: [&str; 4] = ["13", "27", "8", "41"];
    let mut output = String::with_capacity(template.len() + 16);
    let mut slot = 0_usize;
    for character in template.chars() {
        if character == '#' {
            output.push_str(VALUES[slot % VALUES.len()]);
            slot += 1;
        } else {
            output.push(character);
        }
    }
    output
}

fn materialize_parenthesized_ranges(line: &str) -> String {
    let characters: Vec<char> = line.chars().collect();
    let mut output = String::with_capacity(line.len());
    let mut index = 0_usize;
    while index < characters.len() {
        if characters[index] != '(' {
            output.push(characters[index]);
            index += 1;
            continue;
        }
        let Some(relative_end) = characters[index + 1..]
            .iter()
            .position(|character| *character == ')')
        else {
            output.push(characters[index]);
            index += 1;
            continue;
        };
        let end = index + 1 + relative_end;
        let content = characters[index + 1..end].iter().collect::<String>();
        let Some(minimum) = numeric_range_minimum(&content) else {
            output.extend(characters[index..=end].iter());
            index = end + 1;
            continue;
        };
        output.push_str(minimum);
        index = end + 1;
    }
    output
}

fn numeric_range_minimum(content: &str) -> Option<&str> {
    for (byte_index, character) in content.char_indices() {
        if !matches!(character, '—' | '–' | '-') {
            continue;
        }
        if byte_index == 0 {
            continue;
        }
        let left = content[..byte_index].trim();
        let right = content[byte_index + character.len_utf8()..].trim();
        if is_decimal_literal(left) && is_decimal_literal(right) {
            return Some(left);
        }
    }
    None
}

fn is_decimal_literal(value: &str) -> bool {
    let value = value.trim();
    let value = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    let mut parts = value.split(['.', ',']);
    let Some(integer) = parts.next() else {
        return false;
    };
    if integer.is_empty() || !integer.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    match (parts.next(), parts.next()) {
        (None, None) => true,
        (Some(fraction), None) => {
            !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit())
        }
        _ => false,
    }
}
