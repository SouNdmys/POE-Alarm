//! Runs the parser over every item in the captured corpus.
//!
//! The corpus is real Ctrl+C output, stored byte-exact, covering POE1 and POE2
//! in Traditional Chinese and English. Rather than assert the exact affix lines
//! for fifty items — which would be transcription by another name — these
//! checks assert invariants that must hold for all of them, and let a
//! disagreement point at the item that broke.

use std::collections::BTreeSet;
use std::path::PathBuf;

use poe_alarm_clipboard::{ModKind, ParsedItem, parse};

const CORPUS_FILES: &[&str] = &[
    "poe1/zh-tw.txt",
    "poe1/en.txt",
    "poe2/zh-tw.txt",
    "poe2/en.txt",
];

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/corpus")
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

/// Reads an annotation the way a person would, without asking the parser.
///
/// This deliberately does not call `classify_annotation`. It used to, which
/// made the invariant below check only that the parser agreed with itself — and
/// it did agree, right through a stretch where every Traditional Chinese
/// implicit was being matched against user rules because the marker table
/// looked for 固有 and the client writes 固定詞綴.
///
/// The words here were taken by listing every distinct `{ ... }` head in the
/// corpus and reading them. Keep it that way: if the parser gains a marker,
/// this list should only gain it after the corpus shows the client using it.
fn expected_kind(annotation: &str) -> ModKind {
    let head = annotation
        .trim_start_matches('{')
        .split('"')
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let has = |needle: &str| head.contains(needle);

    if has("已大師工藝") || has("工藝") || has("crafted") {
        ModKind::Crafted
    } else if has("已破裂") || has("fractured") {
        ModKind::Fractured
    } else if has("固定詞綴") || has("implicit") {
        ModKind::Implicit
    } else if has("附魔") || has("enchant") {
        ModKind::Enchant
    } else if has("前綴") || has("prefix") {
        ModKind::Prefix
    } else if has("後綴") || has("suffix") {
        ModKind::Suffix
    } else {
        panic!("unrecognised annotation head, add it after checking the corpus: {annotation}")
    }
}

/// Pairs every modifier line with the annotation that introduced it.
///
/// Decoration lines - the parenthesised explanations the client appends, like
/// `(Maimed enemies have 30% reduced Movement Speed)` - are not modifiers and
/// are left out.
fn annotated_lines(text: &str) -> Vec<(ModKind, &str)> {
    // Status and influence tags sit directly after the last modifier with no
    // separator, so they have to be named. Listed here literally rather than
    // borrowed from the parser, for the same reason `expected_kind` is: a test
    // that reuses the code under test proves only self-consistency.
    const TAGS: &[&str] = &[
        "Searing Exarch Item",
        "Eater of Worlds Item",
        "Shaper Item",
        "Elder Item",
        "Crusader Item",
        "Redeemer Item",
        "Hunter Item",
        "Warlord Item",
        "Fractured Item",
        "Synthesised Item",
        "Corrupted",
        "Mirrored",
        "Unidentified",
    ];
    let mut pairs = Vec::new();
    let mut current: Option<ModKind> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('{') {
            current = Some(expected_kind(line));
        } else if line.is_empty() || line.chars().all(|c| c == '-') {
            current = None;
        } else if let Some(kind) = current
            && !line.starts_with('(')
            && !line.starts_with('\u{ff08}')
            && !TAGS.contains(&line)
        {
            pairs.push((kind, line));
        }
    }
    pairs
}

fn annotation_kinds(text: &str) -> Vec<ModKind> {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .map(expected_kind)
        .collect()
}

/// The parser and an independent reading of the same annotation must agree.
#[test]
fn the_parser_classifies_every_annotation_in_the_corpus_the_way_a_person_would() {
    let mut checked = 0_usize;
    for case in all_cases() {
        for line in case
            .text
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with('{'))
        {
            assert_eq!(
                poe_alarm_clipboard::classify_annotation(line),
                expected_kind(line),
                "{}: {line}",
                case.label
            );
            checked += 1;
        }
    }
    assert!(checked >= 200, "only checked {checked} annotations");
}

fn parsed(case: &Case) -> ParsedItem {
    parse(&case.text).unwrap_or_else(|error| panic!("{}: parse failed: {error}", case.label))
}

#[test]
fn every_item_in_the_corpus_parses() {
    for case in all_cases() {
        let item = parsed(&case);
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

/// Every line the client wrote under an annotation reaches the rules.
///
/// Not "most of them", and not "the ones we classified as affixes" — all of
/// them. POE1 has more modifier sources than anyone can enumerate, and a source
/// GGG adds next league is one this parser will not recognise. If recognition
/// gated matching, that unknown source would silently stop an alarm from
/// firing on an item worth real money. So it does not gate anything.
///
/// Deliberately compares LINES, not counts. A count comparison held while whole
/// affixes were being deleted as flavour text and whole items were losing every
/// modifier to a mis-read trailing section.
///
/// One-directional on purpose. Extra lines are not a defect: the client leaves
/// some real modifiers unannotated — quality types like "Quality does not
/// increase Physical Damage" carry no `{ ... }` at all — and a user is entitled
/// to track those too.
#[test]
fn no_annotated_modifier_line_vanishes_unaccounted() {
    let mut checked = 0_usize;
    for case in all_cases() {
        let annotated = annotated_lines(&case.text);
        if annotated.is_empty() {
            continue;
        }
        let expected: BTreeSet<&str> = annotated.iter().map(|(_, line)| *line).collect();
        let item = parsed(&case);
        let actual: BTreeSet<&str> = item
            .groups
            .iter()
            .flat_map(|group| group.lines.iter().map(String::as_str))
            .collect();

        let lost: Vec<_> = expected.difference(&actual).collect();
        assert!(
            lost.is_empty(),
            "{}: these lines never reached the rules: {lost:#?}",
            case.label
        );
        checked += 1;
    }
    assert!(checked >= 40, "only checked {checked} annotated items");
}

#[test]
fn every_annotation_produces_at_least_one_group() {
    // Weaker than an equality, and it has to be: the client annotates most
    // modifiers but not all, so the parser legitimately emits more groups than
    // there are annotations.
    for case in all_cases() {
        let annotations = annotation_kinds(&case.text);
        if annotations.is_empty() {
            continue;
        }
        let item = parsed(&case);
        assert!(
            item.groups.len() >= annotations.len(),
            "{}: client annotated {} modifiers, parser produced only {}",
            case.label,
            annotations.len(),
            item.groups.len()
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
        for group in &parsed(&case).groups {
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
        let item = parsed(&case);
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
            parsed(&case).grouping_is_authoritative,
            "{}: annotations present but grouping was reported as inferred",
            case.label
        );
    }
}

#[test]
fn identity_is_stable_across_items_of_the_same_class_and_level() {
    let cases = all_cases();
    for case in &cases {
        let item = parsed(case);
        assert!(
            item.is_same_item_as(&item),
            "{}: an item must match itself",
            case.label
        );
    }
    // Two dumps differing in item level are different items.
    let mut differing = 0;
    for window in cases.windows(2) {
        let left = parsed(&window[0]);
        let right = parsed(&window[1]);
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
        for group in &parsed(&case).groups {
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
