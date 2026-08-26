//! Runs the parser over every item in the captured corpus.
//!
//! The corpus is real Ctrl+C output, stored byte-exact, covering POE1 and POE2
//! in Traditional Chinese and English. Rather than assert the exact affix lines
//! for fifty items — which would be transcription by another name — these
//! checks assert invariants that must hold for all of them, and let a
//! disagreement point at the item that broke.

use std::path::PathBuf;

use poe_alarm_clipboard::{ModFilter, ModKind, ParsedItem, parse};

const CORPUS_FILES: &[&str] = &[
    "poe1/zh-tw.txt",
    "poe1/en.txt",
    "poe2/zh-tw.txt",
    "poe2/en.txt",
];

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/语料")
        .canonicalize()
        .expect("corpus directory exists")
}

/// Splits a capture file into individual dumps.
///
/// A dump always opens with the item class line, which is the only marker that
/// survives every locale and both games; blank-line runs between items vary.
fn split_items(raw: &str) -> Vec<String> {
    const OPENERS: &[&str] = &["item class:", "物品種類:", "物品類別:"];
    let normalized = raw.replace("\r\n", "\n");
    let mut items: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in normalized.lines() {
        let lowered = line.trim().to_lowercase();
        if OPENERS.iter().any(|opener| lowered.starts_with(opener)) && !current.is_empty() {
            items.push(current.join("\n"));
            current.clear();
        }
        current.push(line);
    }
    if !current.is_empty() {
        items.push(current.join("\n"));
    }
    items
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

struct Case {
    label: String,
    text: String,
}

fn all_cases() -> Vec<Case> {
    let root = corpus_root();
    let mut cases = Vec::new();
    for file in CORPUS_FILES {
        let path = root.join(file);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for (index, text) in split_items(&raw).into_iter().enumerate() {
            cases.push(Case {
                label: format!("{file}#{index}"),
                text,
            });
        }
    }
    assert!(cases.len() >= 40, "corpus shrank unexpectedly");
    cases
}

/// Annotations the client wrote, classified the way the parser classifies them.
fn annotation_kinds(text: &str) -> Vec<ModKind> {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .map(poe_alarm_clipboard::classify_annotation)
        .collect()
}

fn parsed(case: &Case, filter: ModFilter) -> ParsedItem {
    parse(&case.text, filter)
        .unwrap_or_else(|error| panic!("{}: parse failed: {error}", case.label))
}

#[test]
fn every_item_in_the_corpus_parses() {
    for case in all_cases() {
        let item = parsed(&case, ModFilter::default());
        assert!(
            !item.groups.is_empty(),
            "{}: no modifiers survived the filter",
            case.label
        );
        assert!(
            item.item_class.is_some(),
            "{}: item class not recognized",
            case.label
        );
    }
}

#[test]
fn kept_group_count_matches_the_explicit_annotations() {
    // The client states how many prefixes and suffixes an item has. Anything
    // else the parser emits is something it invented, and anything missing is
    // something it dropped.
    let filter = ModFilter::default();
    for case in all_cases() {
        let annotations = annotation_kinds(&case.text);
        if annotations.is_empty() {
            continue;
        }
        let expected = annotations
            .iter()
            .filter(|kind| filter.accepts(**kind))
            .count();
        let item = parsed(&case, filter);
        assert_eq!(
            item.groups.len(),
            expected,
            "{}: expected {expected} explicit modifiers, got {}\nkinds: {:?}",
            case.label,
            item.groups.len(),
            annotations
        );
    }
}

#[test]
fn no_kept_line_is_decoration_or_carries_an_inline_marker() {
    const LEAKED: &[&str] = &[
        "(implicit)",
        "(enchant)",
        "(crafted)",
        "(fractured)",
        "(rune)",
        "(augmented)",
        "(unmet)",
    ];
    for case in all_cases() {
        for group in &parsed(&case, ModFilter::default()).groups {
            for line in &group.lines {
                let trimmed = line.trim_start();
                assert!(
                    !trimmed.starts_with('(') && !trimmed.starts_with('\u{ff08}'),
                    "{}: decoration leaked into a modifier: {line:?}",
                    case.label
                );
                let lowered = line.to_lowercase();
                for marker in LEAKED {
                    assert!(
                        !lowered.contains(marker),
                        "{}: {marker} leaked into a modifier: {line:?}",
                        case.label
                    );
                }
                assert!(
                    !line.trim().is_empty(),
                    "{}: empty line inside a modifier",
                    case.label
                );
            }
        }
    }
}

#[test]
fn rendered_lines_and_identities_stay_aligned() {
    for case in all_cases() {
        let item = parsed(&case, ModFilter::default());
        let (lines, identities) = item.render();
        let text_lines = lines.iter().filter(|line| !line.is_empty()).count();
        assert_eq!(
            identities.len(),
            text_lines,
            "{}: every non-blank line needs an identity",
            case.label
        );
        for identity in &identities {
            assert!(
                !lines[identity.line_index].is_empty(),
                "{}: identity {} points at a blank line",
                case.label,
                identity.line_index
            );
        }
        // Blank lines separate groups, so there is one fewer than the groups.
        let blanks = lines.iter().filter(|line| line.is_empty()).count();
        assert_eq!(
            blanks,
            item.groups.len().saturating_sub(1),
            "{}: wrong number of group separators",
            case.label
        );
    }
}

#[test]
fn annotated_items_report_authoritative_grouping() {
    for case in all_cases() {
        if annotation_kinds(&case.text).is_empty() {
            continue;
        }
        assert!(
            parsed(&case, ModFilter::default()).grouping_is_authoritative,
            "{}: annotations present but grouping was reported as inferred",
            case.label
        );
    }
}

#[test]
fn identity_is_stable_across_items_of_the_same_class_and_level() {
    let cases = all_cases();
    for case in &cases {
        let item = parsed(case, ModFilter::default());
        assert!(
            item.is_same_item_as(&item),
            "{}: an item must match itself",
            case.label
        );
    }
    // Two dumps differing in item level are different items.
    let mut differing = 0;
    for window in cases.windows(2) {
        let left = parsed(&window[0], ModFilter::default());
        let right = parsed(&window[1], ModFilter::default());
        if left.identity() != right.identity() && !left.is_same_item_as(&right) {
            differing += 1;
        }
    }
    assert!(
        differing > 0,
        "the corpus should contain items that differ in class or level"
    );
}

#[test]
fn corpus_exercises_multi_line_modifiers() {
    let mut hybrids = 0;
    let mut widest = 0;
    let mut total_groups = 0;
    for case in all_cases() {
        for group in &parsed(&case, ModFilter::default()).groups {
            total_groups += 1;
            if group.lines.len() > 1 {
                hybrids += 1;
                widest = widest.max(group.lines.len());
            }
        }
    }
    println!("groups={total_groups} hybrids={hybrids} widest={widest}");
    assert!(
        hybrids > 0,
        "the corpus must contain multi-line modifiers, or the grouping path is untested"
    );
}
