//! Turns a Path of Exile clipboard item dump into the affix lines the rule
//! engine expects.
//!
//! The client writes a plain-text block whose sections are separated by a run
//! of dashes. Section keys are localized and the simplified-Chinese client pads
//! two-character keys with spaces (`稀 有 度`), so every key comparison happens
//! on a whitespace-stripped copy of the line.

/// A parsed clipboard item dump.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedItem {
    /// Lines the rule engine should evaluate, in tooltip order.
    pub affix_lines: Vec<String>,
    /// Rarity value exactly as the client wrote it, when present.
    pub rarity: Option<String>,
    /// Item level value as written, when present.
    pub item_level: Option<String>,
    /// True when the dump carried a corruption marker.
    pub corrupted: bool,
}

/// Why a clipboard payload could not be read as an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemTextError {
    /// The payload held no dash separator, so it is not an item dump at all.
    NotAnItem,
    /// The payload looked like an item but carried no affix section.
    NoAffixSection,
}

impl std::fmt::Display for ItemTextError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnItem => formatter.write_str("clipboard payload is not a POE item dump"),
            Self::NoAffixSection => {
                formatter.write_str("item dump carried no recognizable affix section")
            }
        }
    }
}

impl std::error::Error for ItemTextError {}

/// Section keys that introduce a header block rather than affixes.
const HEADER_KEYS: &[&str] = &[
    "itemclass:",
    "rarity:",
    "物品類別:",
    "物品类别:",
    "物品種類:",
    "物品种类:",
    "稀有度:",
];

/// Section keys that introduce metadata blocks sitting between the header and
/// the affixes.
const METADATA_KEYS: &[&str] = &[
    "requirements:",
    "sockets:",
    "itemlevel:",
    "quality:",
    "armour:",
    "evasionrating:",
    "energyshield:",
    "stacksize:",
    "需求:",
    "插槽:",
    "物品等級:",
    "物品等级:",
    "品質:",
    "品质:",
    "護甲:",
    "护甲:",
    "閃避值:",
    "闪避值:",
    "能量護盾:",
    "能量护盾:",
    "堆疊數量:",
    "堆叠数量:",
];

/// Section keys that introduce trailing blocks after the affixes.
const TRAILER_KEYS: &[&str] = &["note:", "備註:", "备注:", "出售", "sellprice"];

/// Standalone lines that are status flags rather than affixes.
const FLAG_LINES: &[&str] = &[
    "corrupted",
    "unidentified",
    "mirrored",
    "split",
    "synthesiseditem",
    "已腐化",
    "未鑑定",
    "未鉴定",
    "已複製",
    "已复制",
];

const ITEM_LEVEL_KEYS: &[&str] = &["itemlevel:", "物品等級:", "物品等级:"];
const RARITY_KEYS: &[&str] = &["rarity:", "稀有度:"];
const CORRUPTED_KEYS: &[&str] = &["corrupted", "已腐化"];

/// Collapses whitespace and lowercases ASCII so localized padded keys compare
/// equal to their compact form.
fn compact(line: &str) -> String {
    line.chars()
        .filter(|character| !character.is_whitespace())
        // The Traditional Chinese client writes a full-width colon; folding it
        // to ASCII lets one key table serve every locale.
        .map(|character| if character == '\u{ff1a}' { ':' } else { character })
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 4 && trimmed.chars().all(|character| character == '-')
}

fn starts_with_any(compacted: &str, keys: &[&str]) -> bool {
    keys.iter().any(|key| compacted.starts_with(key))
}

fn value_after_key<'a>(line: &'a str, keys: &[&str]) -> Option<&'a str> {
    let compacted = compact(line);
    if !starts_with_any(&compacted, keys) {
        return None;
    }
    // Split on either colon: the key matched against a folded copy, but the
    // value has to come out of the original line.
    line.split_once([':', '\u{ff1a}'])
        .map(|(_, value)| value.trim())
}

/// A `{ Prefix "..." (Tier: 2) }` header, emitted only when advanced mod
/// descriptions are enabled. When present these mark the affixes exactly, which
/// beats every heuristic.
fn is_mod_annotation(line: &str) -> bool {
    line.trim().starts_with('{')
}

/// Lines that decorate an affix rather than being one.
fn is_decoration(line: &str) -> bool {
    let trimmed = line.trim();
    is_mod_annotation(trimmed) || trimmed.starts_with('(')
}

/// Flavour and usage text: one long sentence carrying no numbers.
fn is_prose(line: &str) -> bool {
    let trimmed = line.trim();
    const PROSE_MARKERS: &[&str] = &["右鍵點擊", "右键点击", "right click", "placed into"];
    let lowered = trimmed.to_lowercase();
    if PROSE_MARKERS.iter().any(|marker| lowered.contains(marker)) {
        return true;
    }
    trimmed.chars().count() > 24 && !trimmed.chars().any(|character| character.is_ascii_digit())
}

/// Parses a clipboard payload into affix lines plus the few header fields worth
/// surfacing in the lab output.
pub fn parse(payload: &str) -> Result<ParsedItem, ItemTextError> {
    let normalized = payload.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.lines().any(is_separator) {
        return Err(ItemTextError::NotAnItem);
    }

    let mut sections: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in normalized.lines() {
        if is_separator(line) {
            sections.push(std::mem::take(&mut current));
        } else {
            current.push(line);
        }
    }
    sections.push(current);

    let mut parsed = ParsedItem::default();

    let trimmed_sections: Vec<Vec<&str>> = sections
        .iter()
        .map(|section| {
            section
                .iter()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .collect()
        })
        .collect();

    for section in &trimmed_sections {
        for line in section {
            if let Some(value) = value_after_key(line, ITEM_LEVEL_KEYS) {
                parsed.item_level = Some(value.to_string());
            }
            if let Some(value) = value_after_key(line, RARITY_KEYS) {
                parsed.rarity = Some(value.to_string());
            }
            if starts_with_any(&compact(line), CORRUPTED_KEYS) {
                parsed.corrupted = true;
            }
        }
    }

    let is_affix = |line: &&&str| {
        !is_decoration(line) && !FLAG_LINES.contains(&compact(line).as_str())
    };

    // Preferred path: with advanced mod descriptions enabled the client labels
    // every affix itself, which is exact where any layout heuristic is a guess.
    let annotated: Vec<String> = trimmed_sections
        .iter()
        .filter(|section| section.iter().any(|line| is_mod_annotation(line)))
        .flat_map(|section| section.iter().filter(is_affix))
        .map(|line| (*line).to_string())
        .collect();

    let affix_lines = if annotated.is_empty() {
        // Fallback: affixes live in the sections after the item level.
        let start = trimmed_sections
            .iter()
            .position(|section| {
                section
                    .iter()
                    .any(|line| starts_with_any(&compact(line), ITEM_LEVEL_KEYS))
            })
            .map_or(1, |index| index + 1);
        trimmed_sections
            .iter()
            .skip(start)
            .filter(|section| {
                let Some(first) = section.first().map(|line| compact(line)) else {
                    return false;
                };
                !starts_with_any(&first, HEADER_KEYS)
                    && !starts_with_any(&first, METADATA_KEYS)
                    && !starts_with_any(&first, TRAILER_KEYS)
                    && !section
                        .iter()
                        .all(|line| FLAG_LINES.contains(&compact(line).as_str()))
            })
            .flat_map(|section| section.iter().filter(is_affix))
            .filter(|line| !is_prose(line))
            .map(|line| (*line).to_string())
            .collect()
    } else {
        annotated
    };

    if affix_lines.is_empty() {
        return Err(ItemTextError::NoAffixSection);
    }
    parsed.affix_lines = affix_lines;
    Ok(parsed)
}

/// Cheap guard used before timing a sample: does this payload even look like an
/// item the client just wrote?
pub fn looks_like_item(payload: &str) -> bool {
    payload.lines().any(is_separator)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENGLISH_MAGIC_RING: &str = concat!(
        "Item Class: Rings\r\n",
        "Rarity: Magic\r\n",
        "Serpent's Coral Ring of the Lizard\r\n",
        "--------\r\n",
        "Requirements:\r\n",
        "Level: 20\r\n",
        "--------\r\n",
        "Item Level: 62\r\n",
        "--------\r\n",
        "+25 to maximum Life\r\n",
        "+11% to Chaos Resistance\r\n",
    );

    const SIMPLIFIED_PADDED: &str = concat!(
        "物品类别: 戒指\r\n",
        "稀 有 度: 魔法\r\n",
        "蛇形 珊瑚戒指 之 蜥蜴\r\n",
        "--------\r\n",
        "需求:\r\n",
        "等级: 20\r\n",
        "--------\r\n",
        "物品等级: 62\r\n",
        "--------\r\n",
        "+25 最大生命\r\n",
        "+11% 混沌抗性\r\n",
        "--------\r\n",
        "出售此物品可得...\r\n",
    );

    #[test]
    fn parses_english_magic_item() {
        let parsed = parse(ENGLISH_MAGIC_RING).expect("dump parses");
        assert_eq!(
            parsed.affix_lines,
            vec![
                "+25 to maximum Life".to_string(),
                "+11% to Chaos Resistance".to_string(),
            ]
        );
        assert_eq!(parsed.rarity.as_deref(), Some("Magic"));
        assert_eq!(parsed.item_level.as_deref(), Some("62"));
        assert!(!parsed.corrupted);
    }

    #[test]
    fn handles_padded_simplified_keys_and_drops_vendor_block() {
        let parsed = parse(SIMPLIFIED_PADDED).expect("dump parses");
        assert_eq!(
            parsed.affix_lines,
            vec!["+25 最大生命".to_string(), "+11% 混沌抗性".to_string()]
        );
        assert_eq!(parsed.rarity.as_deref(), Some("魔法"));
        assert_eq!(parsed.item_level.as_deref(), Some("62"));
    }

    #[test]
    fn drops_advanced_mod_annotations() {
        let payload = concat!(
            "Rarity: Rare\r\n",
            "Doom Whorl\r\n",
            "--------\r\n",
            "Item Level: 84\r\n",
            "--------\r\n",
            "{ Prefix Modifier (Tier: 2) }\r\n",
            "+42 to maximum Life\r\n",
            "(implicit)\r\n",
            "+8% to Fire Resistance\r\n",
        );
        let parsed = parse(payload).expect("dump parses");
        assert_eq!(
            parsed.affix_lines,
            vec![
                "+42 to maximum Life".to_string(),
                "+8% to Fire Resistance".to_string(),
            ]
        );
    }

    #[test]
    fn records_corruption_without_emitting_it_as_an_affix() {
        let payload = concat!(
            "Rarity: Rare\r\n",
            "Doom Whorl\r\n",
            "--------\r\n",
            "Item Level: 84\r\n",
            "--------\r\n",
            "+42 to maximum Life\r\n",
            "--------\r\n",
            "Corrupted\r\n",
        );
        let parsed = parse(payload).expect("dump parses");
        assert_eq!(parsed.affix_lines, vec!["+42 to maximum Life".to_string()]);
        assert!(parsed.corrupted);
    }

    #[test]
    fn rejects_non_item_payloads() {
        assert_eq!(parse("just some text"), Err(ItemTextError::NotAnItem));
        assert!(!looks_like_item("just some text"));
        assert!(looks_like_item(ENGLISH_MAGIC_RING));
    }

    #[test]
    fn rejects_currency_without_affixes() {
        let payload = concat!(
            "Item Class: Stackable Currency\r\n",
            "Rarity: Currency\r\n",
            "Orb of Alteration\r\n",
            "--------\r\n",
            "Stack Size: 40/20\r\n",
        );
        assert_eq!(parse(payload), Err(ItemTextError::NoAffixSection));
    }

    /// Verbatim from a live zh-TW client, which is what exposed the
    /// full-width colon and the 物品種類 key in the first place.
    const TRADITIONAL_ABYSS_JEWEL: &str = concat!(
        "物品種類：深淵珠寶
",
        "稀有度：魔法
",
        "放電的殺戮之眼珠寶
",
        "--------
",
        "深淵
",
        "無形性：18%
",
        "--------
",
        "需求：
",
        "等級：56
",
        "--------
",
        "物品等級：83
",
        "--------
",
        "{ 前綴 \"放電的\"(階層: 2)— 傷害,元素,閃電,攻擊 }
",
        "錘和權杖攻擊附加 2(2-4) 至 42(40-43) 閃電攻擊
",
        "--------
",
        "放置到一個道具的深淵珠寶插槽或一個天賦樹的珠寶插槽中以產生效果。右鍵點擊以移出插槽。
",
    );

    #[test]
    fn traditional_client_yields_only_the_real_affix() {
        let parsed = parse(TRADITIONAL_ABYSS_JEWEL).expect("dump parses");
        assert_eq!(
            parsed.affix_lines,
            vec!["錘和權杖攻擊附加 2(2-4) 至 42(40-43) 閃電攻擊".to_string()]
        );
        assert_eq!(parsed.rarity.as_deref(), Some("魔法"));
        assert_eq!(parsed.item_level.as_deref(), Some("83"));
        assert!(!parsed.corrupted);
    }

    #[test]
    fn traditional_client_without_annotations_uses_the_positional_fallback() {
        let stripped: String = TRADITIONAL_ABYSS_JEWEL
            .lines()
            .filter(|line| !line.trim_start().starts_with('{'))
            .map(|line| format!("{line}
"))
            .collect();
        let parsed = parse(&stripped).expect("dump parses");
        assert_eq!(
            parsed.affix_lines,
            vec!["錘和權杖攻擊附加 2(2-4) 至 42(40-43) 閃電攻擊".to_string()],
            "the usage prose and the pre-item-level property block must stay out"
        );
    }

    #[test]
    fn usage_prose_is_never_an_affix() {
        assert!(is_prose("放置到一個道具的深淵珠寶插槽或一個天賦樹的珠寶插槽中以產生效果。右鍵點擊以移出插槽。"));
        assert!(is_prose("Place into an allocated Jewel Socket. Right click to remove."));
        assert!(!is_prose("錘和權杖攻擊附加 2(2-4) 至 42(40-43) 閃電攻擊"));
        assert!(!is_prose("+25 to maximum Life"));
    }

    #[test]
    fn full_width_colons_fold_to_ascii_for_key_matching() {
        assert_eq!(compact("物品等級：83"), "物品等級:83");
        assert_eq!(compact("Item Level: 83"), "itemlevel:83");
    }
}
