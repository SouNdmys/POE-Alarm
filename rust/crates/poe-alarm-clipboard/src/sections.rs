//! Splits a dump into sections and turns the modifier section into groups.
//!
//! The client separates sections with a run of dashes and always lays them out
//! in the same order: header, properties, requirements, sockets, item level,
//! enchantments, implicits, explicits, flags, vendor note. Only two of those
//! are wanted, so the job is to find them without enumerating the rest — the
//! property keys alone run to dozens and differ per item class.
//!
//! When `{ ... }` annotations are present that is trivial: the annotated
//! sections are the modifier sections, and nothing else needs identifying.
//! Without them the layout order is the only signal left, and the last
//! modifier-bearing section is the explicit one.

use crate::classify::{
    ModKind, annotation_name, annotation_tier, classify_annotation, is_annotation,
    strip_inline_markers,
};
use crate::{ItemTextError, ModFilter, ModGroup, ParsedItem};

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

/// A line that names how the item is used, which is never a modifier.
fn names_a_usage(line: &str) -> bool {
    let lowered = line.trim().to_lowercase();
    PROSE_MARKERS.iter().any(|marker| lowered.contains(marker))
}

/// Usage or flavour text, judged for a whole section at a time.
///
/// Only ever applied where every line in a section agrees, because the client
/// always gives usage text its own section — and because the last half of this
/// is a guess. Plenty of real modifiers are long sentences with no numbers in
/// them: "Monsters Maim on Hit with Attacks", "Area contains Labyrinth Hazards".
///
/// Getting this wrong is not a cosmetic miss. An unrecognised trailing section
/// is taken for the explicit one, which demotes the real modifiers to implicits
/// and drops them — a quiver whose only trailer was 持弓時才可裝備。 lost all
/// six of its affixes.
fn is_prose(line: &str) -> bool {
    let trimmed = line.trim();
    if names_a_usage(trimmed) {
        return true;
    }
    // The client never ends a modifier with a full stop and always ends usage
    // text with one. Checked against all 340 annotated modifier lines in the
    // four corpora: not one of them ends with a stop.
    if trimmed.ends_with('。') || trimmed.ends_with('.') || trimmed.ends_with('!') {
        return true;
    }
    trimmed.chars().count() > 24 && !trimmed.chars().any(|character| character.is_ascii_digit())
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

pub fn parse(payload: &str, filter: ModFilter) -> Result<ParsedItem, ItemTextError> {
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
            // Left as Explicit it would pollute the default filter.
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
        .filter(|group| filter.accepts(group.kind) && !group.lines.is_empty())
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
        let item = parse(BOW, ModFilter::default()).expect("parses");
        assert_eq!(item.item_class.as_deref(), Some("弓"));
        assert_eq!(item.rarity.as_deref(), Some("稀有"));
        assert_eq!(item.item_level.as_deref(), Some("90"));
        assert!(!item.corrupted);
        assert!(item.grouping_is_authoritative);
    }

    #[test]
    fn bow_keeps_only_explicit_modifiers() {
        let item = parse(BOW, ModFilter::default()).expect("parses");
        let kinds: Vec<ModKind> = item.groups.iter().map(|group| group.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ModKind::Prefix,
                ModKind::Prefix,
                ModKind::Prefix,
                ModKind::Suffix,
                ModKind::Suffix,
            ],
            "the three enchantment lines and the crafted suffix must be dropped"
        );
    }

    #[test]
    fn bow_groups_the_hybrid_prefix_as_one_modifier() {
        let item = parse(BOW, ModFilter::default()).expect("parses");
        let hybrid = &item.groups[2];
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
        let item = parse(BOW, ModFilter::default()).expect("parses");
        assert_eq!(item.groups[1].lines, vec!["增加 179(170-179)% 物理傷害"]);
        assert_eq!(item.groups[2].lines[0], "增加 79(75-79)% 物理傷害");
    }

    #[test]
    fn bow_renders_a_blank_line_between_modifiers() {
        let item = parse(BOW, ModFilter::default()).expect("parses");
        let (lines, identities) = item.render();
        // Five modifiers, six lines of text, four separators between them.
        assert_eq!(identities.len(), 6);
        assert_eq!(lines.iter().filter(|line| line.is_empty()).count(), 4);
        // The hybrid's two lines are adjacent and share a band.
        let hybrid: Vec<&str> = identities
            .iter()
            .filter(|identity| identity.physical_band_id == "clip:2")
            .map(|identity| lines[identity.line_index].as_str())
            .collect();
        assert_eq!(
            hybrid,
            vec!["增加 79(75-79)% 物理傷害", "+186(175-200) 命中值"]
        );
    }

    #[test]
    fn crafted_modifiers_return_when_the_filter_asks_for_them() {
        let filter = ModFilter {
            crafted: true,
            ..ModFilter::default()
        };
        let item = parse(BOW, filter).expect("parses");
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
    fn enchantments_return_when_the_filter_asks_for_them() {
        let filter = ModFilter {
            enchant: true,
            ..ModFilter::default()
        };
        let item = parse(BOW, filter).expect("parses");
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
        assert_eq!(
            parse("just some text", ModFilter::default()),
            Err(ItemTextError::NotAnItem)
        );
    }
}
