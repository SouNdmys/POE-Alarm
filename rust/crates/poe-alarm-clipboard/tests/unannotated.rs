//! What is lost when the client is not annotating modifiers.
//!
//! "Advanced Mod Descriptions" is the in-game setting that makes Ctrl+C label
//! each modifier — `{ 前綴 "名字"(階層：1) }` — and say which lines belong to
//! the same one. It is the client stating, authoritatively, facts that cannot
//! be recovered from the text alone.
//!
//! These tests pin down exactly what is lost without it, because that is what
//! the app has to tell users, and "it works a bit worse" would be too vague to
//! act on. Both losses produce false positives, not misses.

use poe_alarm_clipboard::{ModFilter, parse};

const ANNOTATED: &str =
    include_str!("../../../../tests/fixtures/clipboard-items/poe1-tw-rare-bow-hybrid.txt");

/// The same dump with every `{ ... }` line removed, which is what the client
/// writes when Advanced Mod Descriptions is off.
fn without_annotations() -> String {
    ANNOTATED
        .lines()
        .filter(|line| !line.trim_start().starts_with('{'))
        .collect::<Vec<_>>()
        .join("\r\n")
}

fn flatten(item: &poe_alarm_clipboard::ParsedItem) -> Vec<String> {
    item.groups
        .iter()
        .flat_map(|group| group.lines.iter().cloned())
        .collect()
}

#[test]
fn annotations_keep_a_hybrid_modifier_whole() {
    let item = parse(ANNOTATED, ModFilter::default()).expect("fixture parses");
    let hybrid = item
        .groups
        .iter()
        .find(|group| group.lines.len() > 1)
        .expect("this bow carries a hybrid prefix");
    assert_eq!(hybrid.lines.len(), 2, "{:?}", hybrid.lines);
    assert!(hybrid.lines[0].contains("物理傷害"));
    assert!(hybrid.lines[1].contains("命中值"));
}

#[test]
fn without_annotations_a_hybrid_splits_into_two_modifiers() {
    let annotated = parse(ANNOTATED, ModFilter::default()).expect("fixture parses");
    let flat = parse(&without_annotations(), ModFilter::default()).expect("flat dump parses");

    assert!(
        annotated.groups.iter().any(|group| group.lines.len() > 1),
        "the annotated dump has a hybrid"
    );
    assert!(
        flat.groups.iter().all(|group| group.lines.len() == 1),
        "without the client's grouping every line looks like its own modifier"
    );
    // This is the damage: a rule asking for two separate modifiers can now be
    // satisfied by the single physical prefix that produced both lines.
    assert!(flat.groups.len() > annotated.groups.len());
}

#[test]
fn without_annotations_a_bench_crafted_modifier_is_read_as_rolled() {
    let annotated = flatten(&parse(ANNOTATED, ModFilter::default()).expect("fixture parses"));
    let flat = flatten(&parse(&without_annotations(), ModFilter::default()).expect("parses"));

    // The bow's crafted suffix — { 已大師工藝 後綴 "工藝之" } — contributes these
    // two lines. Excluded by default when the client says it is crafted.
    for crafted in ["+21(20-24) 力量和智慧", "增加 25(21-25)% 暴擊率"] {
        assert!(
            !annotated.contains(&crafted.to_owned()),
            "a crafted line must not reach the rules: {crafted}"
        );
        assert!(
            flat.contains(&crafted.to_owned()),
            "without annotations the crafted line is indistinguishable from a roll: {crafted}"
        );
    }

    // Which means the alarm can fire on a modifier the player crafted onto the
    // item themselves — the one kind of false positive that never self-corrects.
    assert_eq!(annotated.len(), 6);
    assert_eq!(flat.len(), 8);
}
