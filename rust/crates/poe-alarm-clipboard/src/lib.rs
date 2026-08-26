//! Turns a Path of Exile clipboard item dump into rule-engine input.
//!
//! This replaces the OCR recognizer, and the difference is not only accuracy.
//! The OCR path only ever saw the region the user drew a box around, so that
//! box was doing the filtering: it excluded the header, the requirements, the
//! enchantments and the vendor note by simply not covering them. A clipboard
//! dump carries the whole item, so that implicit spatial filter is gone and
//! this module has to take over every job it was quietly doing.
//!
//! It also gains something the OCR path could never have. The tooltip merges
//! modifiers that grant the same stat — a bow carrying a 179% physical prefix
//! and a 79% physical hybrid prefix displays a single `258%` line, a number
//! belonging to no modifier on the item. The dump lists both separately, so a
//! rule asking for a tier-1 179% roll can no longer be satisfied by two lesser
//! modifiers adding up.
//!
//! # Grouping
//!
//! [`poe_alarm_core`] matches a modifier that renders across several lines by
//! joining consecutive non-empty lines, and a blank line is the only thing that
//! stops it. The OCR path produced those blanks from pixel gaps. Here the
//! client states the grouping outright:
//!
//! ```text
//! { 前綴 "獨裁者的"(階層：1)— 傷害,物理,攻擊 }
//! 增加 79(75-79)% 物理傷害
//! +186(175-200) 命中值
//! ```
//!
//! Both lines belong to one physical modifier. That is authoritative where a
//! pixel gap was a heuristic, so grouping follows the annotations when they are
//! present and falls back to one-line-per-modifier when they are not.

#![forbid(unsafe_op_in_unsafe_fn)]

mod classify;
mod sections;

pub use classify::{ModKind, classify_annotation, is_annotation};

use poe_alarm_core::PhysicalLineIdentity;

/// One physical modifier, with every line the client rendered for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModGroup {
    /// What kind of modifier this is, from the annotation or an inline marker.
    pub kind: ModKind,
    /// The modifier's lines, in tooltip order, decoration removed.
    pub lines: Vec<String>,
    /// Affix name from the annotation, such as `獨裁者的`.
    pub name: Option<String>,
    /// Tier from the annotation, where the client provided one.
    pub tier: Option<u32>,
}

/// A parsed clipboard item dump.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedItem {
    /// Modifier groups the caller asked to keep, in tooltip order.
    pub groups: Vec<ModGroup>,
    /// Rarity value exactly as the client wrote it.
    pub rarity: Option<String>,
    /// Item level value as written.
    pub item_level: Option<String>,
    /// Item class as written.
    pub item_class: Option<String>,
    /// True when the dump carried a corruption marker.
    pub corrupted: bool,
    /// True when the dump carried `{ ... }` annotations, so grouping is exact
    /// rather than assumed.
    pub grouping_is_authoritative: bool,
}

/// Why a clipboard payload could not be read as an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemTextError {
    /// The payload held no dash separator, so it is not an item dump at all.
    NotAnItem,
    /// The payload looked like an item but carried no modifier section.
    NoModifierSection,
}

impl std::fmt::Display for ItemTextError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnItem => formatter.write_str("clipboard payload is not a POE item dump"),
            Self::NoModifierSection => {
                formatter.write_str("item dump carried no recognizable modifier section")
            }
        }
    }
}

impl std::error::Error for ItemTextError {}

impl ParsedItem {
    /// Fields that survive any amount of crafting on one physical item.
    ///
    /// The name is not one of them — prefixes and suffixes rewrite it on every
    /// reroll — and neither is the level requirement, which rises with affix
    /// tier. Rarity is excluded so a regal or an alchemy still reads as the
    /// same item rather than as the cursor having moved.
    #[must_use]
    pub fn identity(&self) -> (Option<&str>, Option<&str>) {
        (self.item_class.as_deref(), self.item_level.as_deref())
    }

    /// True when `other` describes the same physical item.
    ///
    /// Unknown identity on either side counts as a match: refusing to evaluate
    /// over missing metadata would drop real rolls, and the caller still fails
    /// closed on anything it cannot read at all.
    #[must_use]
    pub fn is_same_item_as(&self, other: &Self) -> bool {
        let agrees = |left: Option<&str>, right: Option<&str>| match (left, right) {
            (Some(left), Some(right)) => left == right,
            _ => true,
        };
        let (left_class, left_level) = self.identity();
        let (right_class, right_level) = other.identity();
        agrees(left_class, right_class) && agrees(left_level, right_level)
    }

    /// Lines to hand to the rule engine, with a blank line between groups.
    ///
    /// The blank lines are load-bearing: they are the engine's only barrier
    /// against joining two independent modifiers into one candidate for a
    /// hybrid template. This mirrors what the OCR projection did from pixel
    /// gaps.
    #[must_use]
    pub fn physical_lines(&self) -> Vec<String> {
        self.render().0
    }

    /// Band identities aligned to [`ParsedItem::physical_lines`].
    ///
    /// Every line of one modifier shares a band, which is how the engine knows
    /// a single physical modifier may satisfy at most one condition.
    #[must_use]
    pub fn line_identities(&self) -> Vec<PhysicalLineIdentity> {
        self.render().1
    }

    /// Both halves at once, for callers that need them consistent.
    #[must_use]
    pub fn render(&self) -> (Vec<String>, Vec<PhysicalLineIdentity>) {
        let mut lines = Vec::new();
        let mut identities = Vec::new();
        for (group_index, group) in self.groups.iter().enumerate() {
            if lines.last().is_some_and(|line: &String| !line.is_empty()) {
                lines.push(String::new());
            }
            let band = format!("clip:{group_index}");
            for text in &group.lines {
                identities.push(PhysicalLineIdentity {
                    line_index: lines.len(),
                    physical_band_id: band.clone(),
                });
                lines.push(text.clone());
            }
        }
        (lines, identities)
    }
}

/// Parses a clipboard payload into every modifier the client listed.
///
/// Nothing is filtered by kind, and that is deliberate. Whether a line is a
/// prefix, an implicit or bench-crafted is a label the client happens to
/// provide; making matching depend on it would mean a new crafting source
/// GGG ships tomorrow could silently stop an alarm from firing. A false alarm
/// costs a glance. A missed roll costs the item.
///
/// [`ModGroup::kind`] survives for display only.
pub fn parse(payload: &str) -> Result<ParsedItem, ItemTextError> {
    sections::parse(payload)
}

/// Cheap guard used before timing a copy: does this payload even look like an
/// item the client just wrote?
#[must_use]
pub fn looks_like_item(payload: &str) -> bool {
    payload.lines().any(sections::is_separator)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(kind: ModKind, lines: &[&str]) -> ModGroup {
        ModGroup {
            kind,
            lines: lines.iter().map(|line| (*line).to_string()).collect(),
            name: None,
            tier: None,
        }
    }

    #[test]
    fn render_separates_groups_with_a_blank_line() {
        let item = ParsedItem {
            groups: vec![
                group(ModKind::Prefix, &["增加 179% 物理傷害"]),
                group(ModKind::Prefix, &["增加 79% 物理傷害", "+186 命中值"]),
            ],
            ..ParsedItem::default()
        };
        let (lines, identities) = item.render();
        assert_eq!(
            lines,
            vec![
                "增加 179% 物理傷害".to_string(),
                String::new(),
                "增加 79% 物理傷害".to_string(),
                "+186 命中值".to_string(),
            ]
        );
        // Identities index into the array that carries the blanks.
        assert_eq!(identities[0].line_index, 0);
        assert_eq!(identities[1].line_index, 2);
        assert_eq!(identities[2].line_index, 3);
        // The hybrid's two lines share a band; the standalone prefix does not.
        assert_eq!(
            identities[1].physical_band_id,
            identities[2].physical_band_id
        );
        assert_ne!(
            identities[0].physical_band_id,
            identities[1].physical_band_id
        );
    }

    #[test]
    fn render_of_an_empty_item_is_empty() {
        let (lines, identities) = ParsedItem::default().render();
        assert!(lines.is_empty());
        assert!(identities.is_empty());
    }
}
