//! Splits a dump into sections and turns the modifier section into groups.
//!
//! The client separates sections with a run of dashes and always lays them out
//! in the same order: header, properties, requirements, sockets, item level,
//! enchantments, implicits, explicits, flags, vendor note. Everything from the
//! item level onwards that holds modifiers is wanted — no kind is filtered out,
//! because POE1 has more modifier sources than can be enumerated and one this
//! parser fails to recognise must still be matchable.
//!
//! So the job is to reject the sections that hold no modifiers at all, without
//! enumerating what they contain: the property keys alone run to dozens and
//! differ per item class. Every rejection rule here is therefore structural —
//! a colon, a full stop, a dash run — rather than a word list that GGG can
//! invalidate next league.
//!
//! When `{ ... }` annotations are present that is trivial: the annotated
//! sections are the modifier sections, and nothing else needs identifying.
//! Without them the layout order is the only signal left, and the last
//! modifier-bearing section is the explicit one.

use crate::classify::{
    ModKind, annotation_name, annotation_tier, classify_annotation, is_annotation,
    strip_inline_markers,
};
use crate::{ItemTextError, ModGroup, ParsedItem};

const ITEM_CLASS_KEYS: &[&str] = &[
    "itemclass:",
    "物品種類:",
    "物品种类:",
    "物品類別:",
    "物品类别:",
];
const RARITY_KEYS: &[&str] = &["rarity:", "稀有度:"];
const ITEM_LEVEL_KEYS: &[&str] = &["itemlevel:", "物品等級:", "物品等级:"];
// The Traditional Chinese client says 已汙染; 已腐化 is kept as an alias.
const CORRUPTED_KEYS: &[&str] = &["corrupted", "已汙染", "已污染", "已腐化"];
const TRAILER_KEYS: &[&str] = &["note:", "備註:", "备注:", "出售", "sellprice"];

/// Standalone lines that are status flags rather than modifiers.
///
/// Getting this list wrong is not cosmetic. These lines sit in their own
/// section after the explicit modifiers, and a section that is not recognised
/// as flags is taken for the explicit one — which demotes the real modifiers to
/// implicits and drops them. Influence tags are the ones that bite: an Elder
/// pair of gloves used to lose every affix on it.
const FLAG_LINES: &[&str] = &[
    "corrupted",
    "unidentified",
    "mirrored",
    "split",
    "synthesiseditem",
    "fractureditem",
    "desecrated",
    // Influence tags, one line each and often two at a time.
    "shaperitem",
    "elderitem",
    "crusaderitem",
    "redeemeritem",
    "hunteritem",
    "warloriditem",
    "searingexarchitem",
    "eaterofworldsitem",
    "影響物品",
    "開創者物品",
    "尊爺物品",
    "已汙染",
    "已污染",
    "已腐化",
    "未鑑定",
    "未鉴定",
    "已複製",
    "已复制",
    "分裂的物品",
    "破裂之物",
];

/// Phrases that only ever appear in usage or flavour text.
const PROSE_MARKERS: &[&str] = &["右鍵點擊", "右键点击", "right click", "place into"];

/// True for the run of dashes the client uses between sections.
#[must_use]
pub fn is_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 4 && trimmed.chars().all(|character| character == '-')
}

/// Collapses whitespace, folds the full-width colon, and lowercases ASCII.
///
/// Section keys are written with an ASCII colon (`物品種類: 弓`) while
/// annotations use the full-width one (`(階層：1)`). Folding both lets one key
/// table serve every locale and both styles.
fn compact(line: &str) -> String {
    line.chars()
        .filter(|character| !character.is_whitespace())
        .map(|character| {
            if character == '\u{ff1a}' {
                ':'
            } else {
                character
            }
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn starts_with_any(compacted: &str, keys: &[&str]) -> bool {
    keys.iter().any(|key| compacted.starts_with(key))
}

fn value_after_key<'a>(line: &'a str, keys: &[&str]) -> Option<&'a str> {
    if !starts_with_any(&compact(line), keys) {
        return None;
    }
    line.split_once([':', '\u{ff1a}'])
        .map(|(_, value)| value.trim())
}

fn is_flag(line: &str) -> bool {
    FLAG_LINES.contains(&compact(line).as_str())
}

/// A parenthetical rider on the modifier above it, such as
/// `（ 近期內意指 4 秒內 ）`. Both bracket widths occur.
fn is_decoration(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('(') || trimmed.starts_with('\u{ff08}')
}

/// A `key: value` line, which is a property rather than a modifier.
///
/// Maps put `Monster Level: 83` in its own section after the item level, where
/// it would otherwise read as a modifier. Judged by shape rather than keyword —
/// property names run to dozens per item class, differ per locale, and a
/// keyword list would rot the moment GGG adds one. The colon alone is enough:
/// not one of the 340 annotated modifier lines in the corpus contains one, and
/// this is only ever applied where every line in a section agrees.
fn is_property(line: &str) -> bool {
    line.contains(':') || line.contains('\u{ff1a}')
}

/// A line that names how the item is used, which is never a modifier.
fn names_a_usage(line: &str) -> bool {
    let lowered = line.trim().to_lowercase();
    PROSE_MARKERS.iter().any(|marker| lowered.contains(marker))
}

/// Usage or flavour text, judged for a whole section at a time.
///
/// Only ever applied where every line in a section agrees, because the client
/// always gives usage text its own section.
///
/// There used to be a "long and carrying no numbers" guess here too. It ate
/// "Monsters Maim on Hit with Attacks" and "Area contains Labyrinth Hazards",
/// and then ate a map's one-line implicit section whole. Nothing here may rest
/// on a guess: an unrecognised section is taken for the explicit one, which
/// demotes the real modifiers and drops them, and a rule that silently stops
/// firing costs the user an item.
fn is_prose(line: &str) -> bool {
    let trimmed = line.trim();
    if names_a_usage(trimmed) {
        return true;
    }
    // The client never ends a modifier with a full stop and always ends usage
    // text with one. Checked against all 340 annotated modifier lines in the
    // four corpora: not one of them ends with a stop.
    trimmed.ends_with('。') || trimmed.ends_with('.') || trimmed.ends_with('!')
}

/// Sections, trimmed and stripped of empty lines, in document order.
fn split_sections(payload: &str) -> Vec<Vec<&str>> {
    let mut sections = Vec::new();
    let mut current = Vec::new();
    for line in payload.lines() {
        if is_separator(line) {
            sections.push(std::mem::take(&mut current));
        } else {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                current.push(trimmed);
            }
        }
    }
    sections.push(current);
    sections
}

pub fn parse(payload: &str) -> Result<ParsedItem, ItemTextError> {
    let normalized = payload.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.lines().any(is_separator) {
        return Err(ItemTextError::NotAnItem);
    }
    let sections = split_sections(&normalized);

    let mut parsed = ParsedItem::default();
    for section in &sections {
        for line in section {
            if let Some(value) = value_after_key(line, ITEM_LEVEL_KEYS) {
                parsed.item_level = Some(value.to_string());
            }
            if let Some(value) = value_after_key(line, RARITY_KEYS) {
                parsed.rarity = Some(value.to_string());
            }
            if let Some(value) = value_after_key(line, ITEM_CLASS_KEYS) {
                parsed.item_class = Some(value.to_string());
            }
            if starts_with_any(&compact(line), CORRUPTED_KEYS) {
                parsed.corrupted = true;
            }
        }
    }

    let candidates = modifier_sections(&sections);
    let annotated = candidates
        .iter()
        .any(|section| section.iter().any(|line| is_annotation(line)));
    parsed.grouping_is_authoritative = annotated;

    let last = candidates.len().saturating_sub(1);
    let groups: Vec<ModGroup> = candidates
        .iter()
        .enumerate()
        .flat_map(|(index, section)| {
            if section.iter().any(|line| is_annotation(line)) {
                return groups_from_annotations(section);
            }
            // A section carrying no annotation while others do cannot hold
            // explicit affixes — the client annotates every one of those.
            // This is a label, nothing more: no kind has gated matching
            // since the filter was removed.
            let positional = if annotated || index != last {
                ModKind::Implicit
            } else {
                ModKind::Explicit
            };
            groups_from_layout(section, positional)
        })
        .collect();

    parsed.groups = groups
        .into_iter()
        .filter(|group| !group.lines.is_empty())
        .collect();

    if parsed.groups.is_empty() {
        return Err(ItemTextError::NoModifierSection);
    }
    Ok(parsed)
}

/// Each annotation opens a group; the lines beneath it belong to that modifier.
fn groups_from_annotations(section: &[&str]) -> Vec<ModGroup> {
    let mut groups: Vec<ModGroup> = Vec::new();
    for line in section {
        if is_annotation(line) {
            groups.push(ModGroup {
                kind: classify_annotation(line),
                lines: Vec::new(),
                name: annotation_name(line),
                tier: annotation_tier(line),
            });
            continue;
        }
        // Lines before the first annotation belong to no modifier.
        let Some(group) = groups.last_mut() else {
            continue;
        };
        if is_decoration(line) || is_flag(line) {
            continue;
        }
        let (text, inline_kind) = strip_inline_markers(line);
        if text.is_empty() {
            continue;
        }
        // An inline marker outranks the annotation: only the line itself can
        // say it is an enchantment sitting inside an annotated block.
        if let Some(kind) = inline_kind
            && kind != group.kind
            && group.lines.is_empty()
        {
            group.kind = kind;
        }
        group.lines.push(text);
    }
    groups
}

/// Sections that could hold modifiers: everything after the item level, minus
/// the vendor note, the status flags, and the flavour text.
///
/// The property and requirement blocks sit before the item level, so this one
/// positional cut removes them without needing to know a single property key —
/// and there are dozens of those, differing per item class.
fn modifier_sections<'a>(sections: &'a [Vec<&'a str>]) -> Vec<&'a Vec<&'a str>> {
    let start = sections
        .iter()
        .position(|section| {
            section
                .iter()
                .any(|line| starts_with_any(&compact(line), ITEM_LEVEL_KEYS))
        })
        .map_or(0, |index| index + 1);

    sections
        .iter()
        .skip(start)
        .filter(|section| {
            let Some(first) = section.first() else {
                return false;
            };
            !starts_with_any(&compact(first), TRAILER_KEYS)
                && !section.iter().all(|line| is_flag(line))
                && !section.iter().all(|line| is_prose(line))
                && !section.iter().all(|line| is_property(line))
        })
        .collect()
}

/// One group per line, for a section the client did not annotate.
///
/// `positional` is the caller's read of what the section holds; an inline
/// marker on the line itself outranks it.
fn groups_from_layout(section: &[&str], positional: ModKind) -> Vec<ModGroup> {
    section
        .iter()
        .filter_map(|line| {
            if is_decoration(line) || is_flag(line) || names_a_usage(line) {
                return None;
            }
            let (text, inline_kind) = strip_inline_markers(line);
            (!text.is_empty()).then(|| ModGroup {
                kind: inline_kind.unwrap_or(positional),
                lines: vec![text],
                name: None,
                tier: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    const BOW: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/fixtures/clipboard-items/poe1-tw-rare-bow-hybrid.txt"
    ));

    #[test]
    fn separator_needs_a_run_of_dashes() {
        assert!(is_separator("--------"));
        assert!(!is_separator("---"));
        assert!(!is_separator("增加 179% 物理傷害"));
    }

    #[test]
    fn compact_folds_the_full_width_colon() {
        assert_eq!(compact("物品等級: 90"), "物品等級:90");
        assert_eq!(compact("物品等級：90"), "物品等級:90");
        assert_eq!(compact("Item Level: 90"), "itemlevel:90");
    }

    #[test]
    fn decoration_covers_both_bracket_widths() {
        assert!(is_decoration("(implicit)"));
        assert!(is_decoration("（ 近期內意指 4 秒內 ）"));
        assert!(!is_decoration("增加 179(170-179)% 物理傷害"));
    }

    #[test]
    fn bow_metadata_reads_from_ascii_colon_keys() {
        let item = parse(BOW).expect("parses");
        assert_eq!(item.item_class.as_deref(), Some("弓"));
        assert_eq!(item.rarity.as_deref(), Some("稀有"));
        assert_eq!(item.item_level.as_deref(), Some("90"));
        assert!(!item.corrupted);
        assert!(item.grouping_is_authoritative);
    }

    #[test]
    fn bow_keeps_every_modifier_the_client_annotated() {
        // Nothing is filtered by kind. POE1 has more modifier sources than can
        // be enumerated, and one this parser fails to recognise must still be
        // matchable — a rule that silently stops firing costs the user an item.
        let item = parse(BOW).expect("parses");
        let kinds: Vec<ModKind> = item.groups.iter().map(|group| group.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ModKind::Enchant,
                ModKind::Enchant,
                ModKind::Prefix,
                ModKind::Prefix,
                ModKind::Prefix,
                ModKind::Suffix,
                ModKind::Suffix,
                ModKind::Crafted,
            ]
        );
    }

    fn group_starting_with<'a>(item: &'a ParsedItem, prefix: &str) -> &'a ModGroup {
        item.groups
            .iter()
            .find(|group| group.lines[0].starts_with(prefix))
            .unwrap_or_else(|| panic!("no modifier starts with {prefix}"))
    }

    #[test]
    fn bow_groups_the_hybrid_prefix_as_one_modifier() {
        let item = parse(BOW).expect("parses");
        let hybrid = group_starting_with(&item, "增加 79");
        assert_eq!(
            hybrid.lines,
            vec![
                "增加 79(75-79)% 物理傷害".to_string(),
                "+186(175-200) 命中值".to_string(),
            ]
        );
        assert_eq!(hybrid.name.as_deref(), Some("獨裁者的"));
        assert_eq!(hybrid.tier, Some(1));
    }

    #[test]
    fn bow_lists_both_physical_prefixes_separately() {
        // The tooltip merges these into a single 258% line, which belongs to no
        // modifier on the item. Keeping them apart is the whole point.
        let item = parse(BOW).expect("parses");
        assert_eq!(
            group_starting_with(&item, "增加 179").lines,
            vec!["增加 179(170-179)% 物理傷害"]
        );
        assert_eq!(
            group_starting_with(&item, "增加 79").lines[0],
            "增加 79(75-79)% 物理傷害"
        );
    }

    #[test]
    fn bow_renders_a_blank_line_between_modifiers() {
        let item = parse(BOW).expect("parses");
        let (lines, identities) = item.render();
        // Eight modifiers, ten lines of text — the hybrid prefix and the
        // crafted suffix carry two each — and seven separators between them.
        assert_eq!(identities.len(), 10);
        assert_eq!(lines.iter().filter(|line| line.is_empty()).count(), 7);
        // The hybrid's two lines are adjacent and share a band.
        let hybrid_band = &identities[4].physical_band_id;
        assert_eq!(&identities[5].physical_band_id, hybrid_band);
        assert_ne!(&identities[3].physical_band_id, hybrid_band);
    }

    #[test]
    fn a_crafted_modifier_still_reaches_the_rules() {
        let item = parse(BOW).expect("parses");
        let crafted = item
            .groups
            .iter()
            .find(|group| group.kind == ModKind::Crafted)
            .expect("crafted suffix is present");
        assert_eq!(
            crafted.lines,
            vec![
                "+21(20-24) 力量和智慧".to_string(),
                "增加 25(21-25)% 暴擊率".to_string(),
            ]
        );
    }

    #[test]
    fn enchantments_still_reach_the_rules() {
        let item = parse(BOW).expect("parses");
        let enchants: Vec<&ModGroup> = item
            .groups
            .iter()
            .filter(|group| group.kind == ModKind::Enchant)
            .collect();
        // Two enchantment lines survive; the third is a parenthetical rider.
        assert_eq!(enchants.len(), 2);
        assert_eq!(enchants[0].lines, vec!["增加 15% 變動物理詞綴的大小"]);
    }

    #[test]
    fn rejects_a_payload_that_is_not_an_item() {
        assert_eq!(parse("just some text"), Err(ItemTextError::NotAnItem));
    }
}
