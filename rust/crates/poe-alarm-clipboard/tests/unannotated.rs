//! What changes when the client is not annotating modifiers.
//!
//! "Advanced Mod Descriptions" is the in-game setting that makes Ctrl+C label
//! each modifier — `{ 前綴 "名字"(階層：1) }` — and say which lines belong to
//! the same one. It is the client stating, authoritatively, facts that cannot
//! be recovered from the words alone.
//!
//! The guarantee that matters is directional. A false alarm costs a glance; a
//! missed roll costs the item. So the load-bearing test here is
//! `no_tracked_modifier_is_ever_lost_without_annotations`: whatever else
//! degrades, the lines a rule could match on must never shrink.

use std::collections::BTreeSet;

use poe_alarm_clipboard::{ParsedItem, parse};

const HYBRID_BOW: &str =
    include_str!("../../../../tests/fixtures/clipboard-items/poe1-tw-rare-bow-hybrid.txt");

const CORPORA: &[(&str, &str)] = &[
    (
        "poe1-en",
        include_str!("../../../../tests/corpus/poe1/en.txt"),
    ),
    (
        "poe1-zh-tw",
        include_str!("../../../../tests/corpus/poe1/zh-tw.txt"),
    ),
    (
        "poe2-en",
        include_str!("../../../../tests/corpus/poe2/en.txt"),
    ),
    (
        "poe2-zh-tw",
        include_str!("../../../../tests/corpus/poe2/zh-tw.txt"),
    ),
];

/// The same dump with every `{ ... }` line removed, which is what the client
/// writes when Advanced Mod Descriptions is off.
fn without_annotations(dump: &str) -> String {
    dump.lines()
        .filter(|line| !line.trim_start().starts_with('{'))
        .collect::<Vec<_>>()
        .join("\r\n")
}

fn lines_of(item: &ParsedItem) -> BTreeSet<String> {
    item.groups
        .iter()
        .flat_map(|group| group.lines.iter().cloned())
        .collect()
}

/// Splits a corpus file into its individual item dumps.
///
/// The client always opens with the item class, so a line starting one is the
/// only reliable separator; blank runs appear inside dumps too.
fn items(corpus: &str) -> Vec<String> {
    let mut dumps = Vec::new();
    let mut current = String::new();
    for line in corpus.lines() {
        let starts_item = line.starts_with("Item Class:")
            || line.starts_with("物品種類:")
            || line.starts_with("物品類別:");
        if starts_item && !current.trim().is_empty() {
            dumps.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push_str("\r\n");
    }
    if !current.trim().is_empty() {
        dumps.push(current);
    }
    dumps
}

/// The invariant the app is allowed to rely on: turning the setting off can
/// add noise, never remove signal.
#[test]
fn no_tracked_modifier_is_ever_lost_without_annotations() {
    let mut compared = 0_usize;
    for (name, corpus) in CORPORA {
        for (index, dump) in items(corpus).into_iter().enumerate() {
            let Ok(annotated) = parse(&dump) else {
                continue;
            };
            let Ok(flat) = parse(&without_annotations(&dump)) else {
                panic!("{name} item {index}: the flat dump must still parse");
            };
            let annotated_lines = lines_of(&annotated);
            let flat_lines = lines_of(&flat);
            let missing: Vec<_> = annotated_lines.difference(&flat_lines).collect();
            assert!(
                missing.is_empty(),
                "{name} item {index}: turning off annotations dropped {missing:?}"
            );
            compared += 1;
        }
    }
    assert!(
        compared >= 40,
        "the corpus should cover the real item classes, compared only {compared}"
    );
}

#[test]
fn annotations_keep_a_hybrid_modifier_whole() {
    let item = parse(HYBRID_BOW).expect("fixture parses");
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
    let annotated = parse(HYBRID_BOW).expect("fixture parses");
    let flat = parse(&without_annotations(HYBRID_BOW)).expect("parses");

    assert!(
        annotated.groups.iter().any(|group| group.lines.len() > 1),
        "the annotated dump has a hybrid"
    );
    assert!(
        flat.groups.iter().all(|group| group.lines.len() == 1),
        "without the client's grouping every line looks like its own modifier"
    );
    // Both lines survive, so nothing is missed. What is lost is the knowledge
    // that one physical modifier produced them, which is what stops a rule
    // asking for two modifiers being satisfied by one.
    assert!(flat.groups.len() > annotated.groups.len());
}
