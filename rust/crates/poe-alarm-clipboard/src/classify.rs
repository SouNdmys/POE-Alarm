//! Works out what kind of modifier a line or annotation describes.
//!
//! Two independent signals carry this, and the client provides whichever it
//! feels like. A `{ ... }` annotation names the kind outright; failing that,
//! some lines carry an inline marker such as ` (enchant)`. Both are handled so
//! that a dump missing one still classifies correctly.

/// What kind of modifier a group holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ModKind {
    /// A rolled prefix.
    Prefix,
    /// A rolled suffix.
    Suffix,
    /// Rolled, but the client did not say which half. This is what the
    /// fallback path produces when no annotation is present.
    #[default]
    Explicit,
    /// A rolled affix locked by a fracturing orb. Still an explicit affix.
    Fractured,
    /// Bench-crafted.
    Crafted,
    /// Innate to the base item.
    Implicit,
    /// From the labyrinth, an altar, or similar.
    Enchant,
}

/// Words the client uses inside `{ ... }`, most specific first.
///
/// Order matters. A crafted suffix is annotated `{ 已大師工藝 後綴 "工藝之" }`,
/// which names both, and calling it crafted is the more useful answer.
const ANNOTATION_MARKERS: &[(&str, ModKind)] = &[
    ("已大師工藝", ModKind::Crafted),
    ("工藝", ModKind::Crafted),
    ("crafted", ModKind::Crafted),
    ("分裂", ModKind::Fractured),
    ("fractured", ModKind::Fractured),
    ("固有", ModKind::Implicit),
    ("implicit", ModKind::Implicit),
    ("附魔", ModKind::Enchant),
    ("enchant", ModKind::Enchant),
    ("前綴", ModKind::Prefix),
    ("前缀", ModKind::Prefix),
    ("prefix", ModKind::Prefix),
    ("後綴", ModKind::Suffix),
    ("后缀", ModKind::Suffix),
    ("suffix", ModKind::Suffix),
];

/// Markers the client appends to a line when it is not annotating separately.
const INLINE_MARKERS: &[(&str, ModKind)] = &[
    ("(enchant)", ModKind::Enchant),
    ("(implicit)", ModKind::Implicit),
    ("(crafted)", ModKind::Crafted),
    ("(fractured)", ModKind::Fractured),
    ("(rune)", ModKind::Enchant),
    ("(scourge)", ModKind::Enchant),
    ("(veiled)", ModKind::Crafted),
];

/// Markers that describe a *property* rather than a modifier, and which must be
/// stripped without changing how the line is classified.
const NOISE_MARKERS: &[&str] = &["(augmented)", "(unmet)"];

/// True for a `{ ... }` annotation line.
#[must_use]
pub fn is_annotation(line: &str) -> bool {
    line.trim_start().starts_with('{')
}

/// Classifies a `{ ... }` annotation.
///
/// Only the text before the affix name is considered. The name itself can
/// contain a marker — `"工藝之"` embeds 工藝 — and reading it would misclassify
/// an ordinary suffix as crafted.
#[must_use]
pub fn classify_annotation(annotation: &str) -> ModKind {
    let head = annotation
        .split_once('"')
        .map_or(annotation, |(before, _)| before)
        .to_lowercase();
    ANNOTATION_MARKERS
        .iter()
        .find(|(marker, _)| head.contains(marker))
        .map_or(ModKind::Explicit, |(_, kind)| *kind)
}

/// Pulls the affix name out of an annotation, such as `獨裁者的`.
#[must_use]
pub fn annotation_name(annotation: &str) -> Option<String> {
    let (_, after) = annotation.split_once('"')?;
    let (name, _) = after.split_once('"')?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Pulls the tier out of an annotation, from `(階層：1)` or `(Tier: 1)`.
#[must_use]
pub fn annotation_tier(annotation: &str) -> Option<u32> {
    const TIER_KEYS: &[&str] = &["階層", "阶层", "tier"];
    let lowered = annotation.to_lowercase();
    let key_end = TIER_KEYS
        .iter()
        .find_map(|key| lowered.find(key).map(|at| at + key.len()))?;
    annotation[key_end..]
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

/// Strips inline markers from a line, reporting any kind they implied.
///
/// Property noise such as `(augmented)` is removed without implying a kind,
/// since it decorates a property line rather than a modifier.
#[must_use]
pub fn strip_inline_markers(line: &str) -> (String, Option<ModKind>) {
    let mut text = line.trim().to_string();
    let mut kind = None;
    let mut changed = true;
    // A line can carry more than one, as in `敏捷: 636 (augmented) (unmet)`.
    while changed {
        changed = false;
        let lowered = text.to_lowercase();
        for (marker, marker_kind) in INLINE_MARKERS {
            if lowered.ends_with(marker) {
                text.truncate(text.len() - marker.len());
                text = text.trim_end().to_string();
                kind.get_or_insert(*marker_kind);
                changed = true;
                break;
            }
        }
        if changed {
            continue;
        }
        for marker in NOISE_MARKERS {
            if lowered.ends_with(marker) {
                text.truncate(text.len() - marker.len());
                text = text.trim_end().to_string();
                changed = true;
                break;
            }
        }
    }
    (text, kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_traditional_annotations() {
        assert_eq!(
            classify_annotation("{ 前綴 \"迸出的\"(階層：1)— 傷害,物理,攻擊 }"),
            ModKind::Prefix
        );
        assert_eq!(
            classify_annotation("{ 後綴 \"喝采之\"(階層：1)— 攻擊,速度 }"),
            ModKind::Suffix
        );
    }

    #[test]
    fn an_affix_name_never_drives_classification() {
        // The name embeds 工藝, but the modifier is a plain crafted suffix and
        // reading the name would have called an ordinary suffix crafted.
        assert_eq!(
            classify_annotation("{ 已大師工藝 後綴 \"工藝之\"— 攻擊,暴擊,能力 }"),
            ModKind::Crafted
        );
    }

    #[test]
    fn classifies_english_annotations() {
        assert_eq!(
            classify_annotation("{ Prefix Modifier \"Athlete's\" (Tier: 2) }"),
            ModKind::Prefix
        );
        assert_eq!(
            classify_annotation("{ Implicit Modifier }"),
            ModKind::Implicit
        );
    }

    #[test]
    fn unknown_annotations_read_as_plain_explicit() {
        assert_eq!(classify_annotation("{ something else }"), ModKind::Explicit);
    }

    #[test]
    fn extracts_name_and_tier() {
        let annotation = "{ 前綴 \"獨裁者的\"(階層：1)— 傷害,物理,攻擊  — 增加 15% }";
        assert_eq!(annotation_name(annotation).as_deref(), Some("獨裁者的"));
        assert_eq!(annotation_tier(annotation), Some(1));
        assert_eq!(
            annotation_tier("{ Prefix Modifier \"Athlete's\" (Tier: 12) }"),
            Some(12)
        );
    }

    #[test]
    fn tier_is_absent_when_the_client_omits_it() {
        // Master-crafted annotations carry no tier.
        assert_eq!(
            annotation_tier("{ 已大師工藝 後綴 \"工藝之\"— 攻擊,暴擊,能力 }"),
            None
        );
    }

    #[test]
    fn strips_inline_marker_and_reports_the_kind() {
        let (text, kind) = strip_inline_markers("增加 200% 能力值需求 (enchant)");
        assert_eq!(text, "增加 200% 能力值需求");
        assert_eq!(kind, Some(ModKind::Enchant));
    }

    #[test]
    fn strips_stacked_property_noise_without_implying_a_kind() {
        let (text, kind) = strip_inline_markers("敏捷: 636 (augmented) (unmet)");
        assert_eq!(text, "敏捷: 636");
        assert_eq!(kind, None);
    }

    #[test]
    fn leaves_an_unmarked_line_alone() {
        let (text, kind) = strip_inline_markers("增加 179(170-179)% 物理傷害");
        assert_eq!(text, "增加 179(170-179)% 物理傷害");
        assert_eq!(kind, None);
    }
}
