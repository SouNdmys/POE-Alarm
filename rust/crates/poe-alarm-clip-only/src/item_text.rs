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
    line.split_once(':').map(|(_, value)| value.trim())
}

/// True for lines the client only emits when advanced mod descriptions are on.
fn is_advanced_annotation(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('{') || trimmed.starts_with('(')
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
    let mut affix_lines = Vec::new();

    for section in &sections {
        let meaningful: Vec<&str> = section
            .iter()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .collect();
        if meaningful.is_empty() {
            continue;
        }

        for line in &meaningful {
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

        let first = compact(meaningful[0]);
        if starts_with_any(&first, HEADER_KEYS)
            || starts_with_any(&first, METADATA_KEYS)
            || starts_with_any(&first, TRAILER_KEYS)
        {
            continue;
        }

        // A section whose every line is a status flag carries no affixes.
        let all_flags = meaningful
            .iter()
            .all(|line| FLAG_LINES.contains(&compact(line).as_str()));
        if all_flags {
            continue;
        }

        for line in meaningful {
            if is_advanced_annotation(line) {
                continue;
            }
            if FLAG_LINES.contains(&compact(line).as_str()) {
                continue;
            }
            affix_lines.push(line.to_string());
        }
    }

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
}
