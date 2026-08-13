use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use crate::Decimal;

pub const DEFAULT_MAXIMUM_PHYSICAL_LINE_SPAN: usize = 4;
pub const MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN: usize = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AffixTokenKind {
    Word,
    Number,
    Percent,
    NegativeNumber,
    NegativePercent,
}

impl AffixTokenKind {
    pub fn is_word(self) -> bool {
        self == Self::Word
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AffixToken {
    pub kind: AffixTokenKind,
    pub text: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CanonicalAffix {
    pub tokens: Vec<AffixToken>,
    pub text: String,
}

impl CanonicalAffix {
    pub fn has_words(&self) -> bool {
        self.tokens.iter().any(|token| token.kind.is_word())
    }
}

/// Repairs dot-like decimal separators only when they occur between digits.
/// Full-width ASCII is folded at the same time, matching the useful part of
/// the .NET implementation's compatibility normalization for POE tooltips.
pub fn normalize_numeric_presentation(value: &str) -> String {
    let source: Vec<char> = value.chars().map(fold_width).collect();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < source.len() {
        let current = source[index];
        output.push(current);
        if current.is_ascii_digit() {
            let mut separator = index + 1;
            while separator < source.len() && source[separator].is_whitespace() {
                separator += 1;
            }
            if separator < source.len() && is_decimal_separator(source[separator]) {
                let mut next_digit = separator + 1;
                while next_digit < source.len() && source[next_digit].is_whitespace() {
                    next_digit += 1;
                }
                if next_digit < source.len() && source[next_digit].is_ascii_digit() {
                    output.push('.');
                    index = next_digit;
                    continue;
                }
            }
        }
        index += 1;
    }
    output
}

/// Converts a PoEDB template or OCR candidate into its strict structural form.
/// Numeric magnitudes are discarded while sign and percent units remain.
pub fn canonicalize(text: &str) -> CanonicalAffix {
    if text.trim().is_empty() {
        return CanonicalAffix {
            tokens: Vec::new(),
            text: String::new(),
        };
    }

    let normalized = normalize_for_matching(text);
    let chars: Vec<char> = normalized.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        if current.is_whitespace() {
            index += 1;
            continue;
        }

        if is_han(current) {
            tokens.push(AffixToken {
                kind: AffixTokenKind::Word,
                text: current.to_string(),
            });
            index += 1;
            continue;
        }

        if is_word_start(&chars, index) {
            let mut end = index + 1;
            while end < chars.len() && is_non_han_word_char(chars[end]) {
                end += 1;
            }
            let word: String = chars[index..end].iter().collect();
            let word = word.trim_matches('\'');
            if !word.is_empty() {
                tokens.push(AffixToken {
                    kind: AffixTokenKind::Word,
                    text: word.to_owned(),
                });
            }
            index = end;
            continue;
        }

        if let Some(parsed) = parse_numeric(&chars, index) {
            tokens.push(AffixToken {
                kind: parsed.kind,
                text: token_text(parsed.kind).to_owned(),
            });
            index = parsed.end;
            continue;
        }

        // Styling punctuation never identifies a modifier.
        index += 1;
    }

    let text = tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    CanonicalAffix { tokens, text }
}

/// Extracts displayed values in the same order as canonical numeric slots.
/// Rolled forms such as `179(170-179)` yield 179; template ranges and `#`
/// placeholders yield `None`.
pub fn extract_values(text: &str) -> Vec<Option<Decimal>> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let normalized: String = normalize_numeric_presentation(text)
        .chars()
        .map(|character| if is_dash(character) { '-' } else { character })
        .collect();
    let chars: Vec<char> = normalized.chars().collect();
    let mut values = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        if current.is_whitespace() || is_han(current) {
            index += 1;
            continue;
        }

        if is_latin_word_start(&chars, index) {
            index += 1;
            while index < chars.len() && is_non_han_word_char(chars[index]) {
                index += 1;
            }
            continue;
        }

        if let Some(parsed) = parse_numeric(&chars, index) {
            values.push(parsed.observed);
            index = parsed.end;
            continue;
        }
        index += 1;
    }
    values
}

#[derive(Clone, Debug)]
pub struct FullLineAffixMatcher {
    source_template: String,
    template: CanonicalAffix,
    maximum_line_span: usize,
}

impl FullLineAffixMatcher {
    pub fn new(template: impl AsRef<str>) -> Result<Self, MatchCompileError> {
        Self::with_maximum_line_span(template, DEFAULT_MAXIMUM_PHYSICAL_LINE_SPAN)
    }

    pub fn with_maximum_line_span(
        template: impl AsRef<str>,
        maximum_line_span: usize,
    ) -> Result<Self, MatchCompileError> {
        let source_template = template.as_ref().trim();
        if source_template.is_empty() {
            return Err(MatchCompileError::EmptyTemplate);
        }
        if !(1..=MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN).contains(&maximum_line_span) {
            return Err(MatchCompileError::InvalidLineSpan(maximum_line_span));
        }
        let canonical = canonicalize(source_template);
        if !canonical.has_words() {
            return Err(MatchCompileError::TemplateHasNoWords);
        }
        Ok(Self {
            source_template: source_template.to_owned(),
            template: canonical,
            maximum_line_span,
        })
    }

    pub fn source_template(&self) -> &str {
        &self.source_template
    }

    pub fn template(&self) -> &CanonicalAffix {
        &self.template
    }

    pub fn maximum_line_span(&self) -> usize {
        self.maximum_line_span
    }

    pub fn is_match(&self, candidate: &str) -> bool {
        self.is_canonical_match(&canonicalize(candidate))
    }

    pub fn is_canonical_match(&self, candidate: &CanonicalAffix) -> bool {
        let expected = &self.template.tokens;
        let actual = &candidate.tokens;
        let (mut expected_index, mut actual_index) = (0, 0);
        loop {
            let expected_start = expected_index;
            while expected_index < expected.len() && expected[expected_index].kind.is_word() {
                expected_index += 1;
            }
            let actual_start = actual_index;
            while actual_index < actual.len() && actual[actual_index].kind.is_word() {
                actual_index += 1;
            }

            if !word_runs_equivalent(
                expected,
                expected_start,
                expected_index,
                actual,
                actual_start,
                actual_index,
            ) {
                return false;
            }

            let expected_ended = expected_index == expected.len();
            let actual_ended = actual_index == actual.len();
            if expected_ended || actual_ended {
                return expected_ended && actual_ended;
            }
            if expected[expected_index].kind != actual[actual_index].kind {
                return false;
            }
            expected_index += 1;
            actual_index += 1;
        }
    }

    pub fn find_match(&self, physical_lines: &[String]) -> Option<LogicalAffixMatch> {
        self.find_match_with_span(physical_lines, self.maximum_line_span)
            .ok()
            .flatten()
    }

    pub fn find_match_with_span(
        &self,
        physical_lines: &[String],
        maximum_line_span: usize,
    ) -> Result<Option<LogicalAffixMatch>, MatchCompileError> {
        if !(1..=MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN).contains(&maximum_line_span) {
            return Err(MatchCompileError::InvalidLineSpan(maximum_line_span));
        }
        for start in 0..physical_lines.len() {
            if physical_lines[start].trim().is_empty() {
                continue;
            }
            let mut candidate = String::new();
            let available = maximum_line_span.min(physical_lines.len() - start);
            for line_count in 1..=available {
                let line = physical_lines[start + line_count - 1].trim();
                if line.is_empty() {
                    break;
                }
                if !candidate.is_empty() {
                    candidate.push(' ');
                }
                candidate.push_str(line);
                let canonical = canonicalize(&candidate);
                if self.is_canonical_match(&canonical) {
                    return Ok(Some(LogicalAffixMatch {
                        start_line_index: start,
                        physical_line_count: line_count,
                        original_text: candidate,
                        canonical_text: canonical.text,
                    }));
                }
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalAffixMatch {
    pub start_line_index: usize,
    pub physical_line_count: usize,
    pub original_text: String,
    pub canonical_text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MatchCompileError {
    EmptyTemplate,
    TemplateHasNoWords,
    InvalidLineSpan(usize),
}

impl std::fmt::Display for MatchCompileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyTemplate => write!(formatter, "affix template cannot be empty"),
            Self::TemplateHasNoWords => write!(formatter, "affix template must contain text"),
            Self::InvalidLineSpan(value) => write!(
                formatter,
                "line span {value} is outside 1..={MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN}"
            ),
        }
    }
}

impl std::error::Error for MatchCompileError {}

#[derive(Clone, Copy, Debug)]
struct ParsedNumeric {
    end: usize,
    kind: AffixTokenKind,
    observed: Option<Decimal>,
}

fn parse_numeric(chars: &[char], start: usize) -> Option<ParsedNumeric> {
    if start >= chars.len() {
        return None;
    }

    // Parenthesized template range: (6-8), optionally followed by percent.
    if chars[start] == '(' {
        let mut cursor = skip_space(chars, start + 1);
        let (first, after_first, negative) = parse_signed_number(chars, cursor)?;
        let _ = first;
        cursor = skip_space(chars, after_first);
        if cursor >= chars.len() || chars[cursor] != '-' {
            return None;
        }
        cursor = skip_space(chars, cursor + 1);
        let (_, after_second, _) = parse_signed_number(chars, cursor)?;
        cursor = skip_space(chars, after_second);
        if cursor >= chars.len() || chars[cursor] != ')' {
            return None;
        }
        return Some(finish_numeric(chars, cursor + 1, negative, None));
    }

    // Placeholder, with optional sign and whitespace.
    let mut placeholder = start;
    let placeholder_negative = chars[placeholder] == '-';
    if matches!(chars[placeholder], '+' | '-') {
        placeholder = skip_space(chars, placeholder + 1);
    }
    if placeholder < chars.len() && chars[placeholder] == '#' {
        return Some(finish_numeric(
            chars,
            placeholder + 1,
            placeholder_negative,
            None,
        ));
    }

    let (first, after_first, negative) = parse_signed_number(chars, start)?;
    let cursor = skip_space(chars, after_first);

    // Rolled value followed by its tier range: 179(170-179).
    if cursor < chars.len() && chars[cursor] == '(' {
        let mut range = skip_space(chars, cursor + 1);
        let (_, after_minimum, _) = parse_signed_number(chars, range)?;
        range = skip_space(chars, after_minimum);
        if range >= chars.len() || chars[range] != '-' {
            return Some(finish_numeric(chars, after_first, negative, Some(first)));
        }
        range = skip_space(chars, range + 1);
        let (_, after_maximum, _) = parse_signed_number(chars, range)?;
        range = skip_space(chars, after_maximum);
        if range < chars.len() && chars[range] == ')' {
            return Some(finish_numeric(chars, range + 1, negative, Some(first)));
        }
    }

    // Plain template range: 6-8. Its bounds are not an observed roll.
    if cursor < chars.len() && chars[cursor] == '-' {
        let second_start = skip_space(chars, cursor + 1);
        if let Some((_, after_second, _)) = parse_signed_number(chars, second_start) {
            return Some(finish_numeric(chars, after_second, negative, None));
        }
    }

    Some(finish_numeric(chars, after_first, negative, Some(first)))
}

fn finish_numeric(
    chars: &[char],
    expression_end: usize,
    negative: bool,
    observed: Option<Decimal>,
) -> ParsedNumeric {
    let mut end = skip_space(chars, expression_end);
    let percent = end < chars.len() && chars[end] == '%';
    if percent {
        end += 1;
    }
    let kind = match (negative, percent) {
        (true, true) => AffixTokenKind::NegativePercent,
        (true, false) => AffixTokenKind::NegativeNumber,
        (false, true) => AffixTokenKind::Percent,
        (false, false) => AffixTokenKind::Number,
    };
    ParsedNumeric {
        end,
        kind,
        observed,
    }
}

fn parse_signed_number(chars: &[char], start: usize) -> Option<(Decimal, usize, bool)> {
    if start >= chars.len() {
        return None;
    }
    let mut cursor = start;
    let mut negative = false;
    if matches!(chars[cursor], '+' | '-') {
        negative = chars[cursor] == '-';
        cursor += 1;
    }
    let digit_start = cursor;
    while cursor < chars.len() && chars[cursor].is_ascii_digit() {
        cursor += 1;
    }
    if cursor == digit_start {
        return None;
    }
    if cursor + 1 < chars.len()
        && matches!(chars[cursor], '.' | ',')
        && chars[cursor + 1].is_ascii_digit()
    {
        cursor += 1;
        while cursor < chars.len() && chars[cursor].is_ascii_digit() {
            cursor += 1;
        }
    }
    let source: String = chars[start..cursor].iter().collect();
    let value = Decimal::from_str(&source.replace(',', ".")).ok()?;
    Some((value, cursor, negative))
}

fn skip_space(chars: &[char], mut index: usize) -> usize {
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    index
}

fn token_text(kind: AffixTokenKind) -> &'static str {
    match kind {
        AffixTokenKind::Word => "",
        AffixTokenKind::Number => "<NUM>",
        AffixTokenKind::Percent => "<PCT>",
        AffixTokenKind::NegativeNumber => "<NEG_NUM>",
        AffixTokenKind::NegativePercent => "<NEG_PCT>",
    }
}

fn normalize_for_matching(value: &str) -> String {
    let source: Vec<char> = normalize_numeric_presentation(value).chars().collect();
    let mut output = String::with_capacity(value.len());
    for (index, character) in source.iter().copied().enumerate() {
        if is_dash(character) {
            let joins_letters = index > 0
                && index + 1 < source.len()
                && source[index - 1].is_alphabetic()
                && source[index + 1].is_alphabetic();
            if !joins_letters {
                output.push('-');
            }
            continue;
        }
        match character {
            '\u{2018}' | '\u{2019}' | '\u{201B}' | '\u{02BC}' | '\u{FF07}' => output.push('\''),
            '\u{00A0}' | '\u{2007}' | '\u{202F}' => output.push(' '),
            _ => output.extend(character.to_lowercase()),
        }
    }
    output
}

fn fold_width(character: char) -> char {
    match character {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(character as u32 - 0xFEE0).unwrap(),
        _ => character,
    }
}

fn is_decimal_separator(character: char) -> bool {
    matches!(
        character,
        '.' | ',' | '\u{00B7}' | '\u{2022}' | '\u{2027}' | '\u{2219}' | '\u{22C5}' | '\u{3002}'
    )
}

fn is_dash(character: char) -> bool {
    matches!(
        character,
        '-' | '\u{2010}'
            | '\u{2011}'
            | '\u{2012}'
            | '\u{2013}'
            | '\u{2014}'
            | '\u{2015}'
            | '\u{2212}'
    )
}

fn is_han(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
    )
}

fn is_non_han_word_char(character: char) -> bool {
    !is_han(character) && (character.is_alphanumeric() || character == '\'')
}

fn is_word_start(chars: &[char], index: usize) -> bool {
    if chars[index].is_alphabetic() {
        return true;
    }
    if !chars[index].is_ascii_digit() {
        return false;
    }
    let mut cursor = index + 1;
    while cursor < chars.len() && is_non_han_word_char(chars[cursor]) {
        if chars[cursor].is_alphabetic() && !is_han(chars[cursor]) {
            return true;
        }
        cursor += 1;
    }
    false
}

fn is_latin_word_start(chars: &[char], index: usize) -> bool {
    if chars[index].is_alphabetic() && !is_han(chars[index]) {
        return true;
    }
    if !chars[index].is_ascii_digit() {
        return false;
    }
    let mut cursor = index + 1;
    while cursor < chars.len() && is_non_han_word_char(chars[cursor]) {
        if chars[cursor].is_alphabetic() && !is_han(chars[cursor]) {
            return true;
        }
        cursor += 1;
    }
    false
}

fn word_runs_equivalent(
    expected: &[AffixToken],
    expected_start: usize,
    expected_end: usize,
    actual: &[AffixToken],
    actual_start: usize,
    actual_end: usize,
) -> bool {
    let expected_count = expected_end - expected_start;
    let actual_count = actual_end - actual_start;
    if expected_count == actual_count
        && (0..expected_count).all(|offset| {
            words_equivalent(
                &expected[expected_start + offset].text,
                &actual[actual_start + offset].text,
            )
        })
    {
        return true;
    }
    if expected_count < 2 || actual_count == 0 || actual_count >= expected_count {
        return false;
    }
    merged_runs_equivalent(
        &expected[expected_start..expected_end],
        &actual[actual_start..actual_end],
    )
}

fn words_equivalent(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    let left = collapse_apostrophes_and_rn(expected);
    let right = collapse_apostrophes_and_rn(actual);
    if left == right {
        return true;
    }
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();
    if left_chars.len() < 4 || left_chars.len() != right_chars.len() {
        return false;
    }
    let mut confusions = 0;
    for (&left, &right) in left_chars.iter().zip(&right_chars) {
        if left == right {
            continue;
        }
        if !is_common_glyph_confusion(left, right) {
            return false;
        }
        confusions += 1;
        if confusions > 2 {
            return false;
        }
    }
    confusions > 0
}

#[derive(Clone, Copy)]
struct ExpectedCharacter {
    value: char,
    word_index: usize,
    word_length: usize,
}

#[derive(Clone, Copy)]
struct ActualCharacter {
    value: char,
    token_index: usize,
}

struct MergedMatchContext<'a> {
    expected: &'a [ExpectedCharacter],
    actual: &'a [ActualCharacter],
    boundaries: &'a HashSet<usize>,
    memo: HashMap<(usize, usize, bool, u8), bool>,
}

impl MergedMatchContext<'_> {
    fn matches(
        &mut self,
        expected_offset: usize,
        actual_offset: usize,
        skipped_digit: bool,
        confusions: u8,
    ) -> bool {
        if expected_offset == self.expected.len() || actual_offset == self.actual.len() {
            return expected_offset == self.expected.len() && actual_offset == self.actual.len();
        }
        let state = (expected_offset, actual_offset, skipped_digit, confusions);
        if let Some(cached) = self.memo.get(&state) {
            return *cached;
        }
        let expected_char = self.expected[expected_offset];
        let actual_char = self.actual[actual_offset];
        let next_confusions = if expected_offset + 1 < self.expected.len()
            && self.expected[expected_offset + 1].word_index == expected_char.word_index
        {
            confusions
        } else {
            0
        };

        let mut matched = expected_char.value == actual_char.value
            && self.matches(
                expected_offset + 1,
                actual_offset + 1,
                skipped_digit,
                next_confusions,
            );
        if !matched
            && expected_char.word_length >= 4
            && confusions < 2
            && is_common_glyph_confusion(expected_char.value, actual_char.value)
        {
            let after = if expected_offset + 1 < self.expected.len()
                && self.expected[expected_offset + 1].word_index == expected_char.word_index
            {
                confusions + 1
            } else {
                0
            };
            matched = self.matches(expected_offset + 1, actual_offset + 1, skipped_digit, after);
        }
        if !matched
            && !skipped_digit
            && self.boundaries.contains(&expected_offset)
            && is_embedded_isolated_digit(self.actual, actual_offset)
        {
            matched = self.matches(expected_offset, actual_offset + 1, true, confusions);
        }
        self.memo.insert(state, matched);
        matched
    }
}

fn merged_runs_equivalent(expected: &[AffixToken], actual: &[AffixToken]) -> bool {
    let mut expected_characters = Vec::new();
    let mut expected_boundaries = HashSet::new();
    for (word_index, token) in expected.iter().enumerate() {
        let word = collapse_apostrophes_and_rn(&token.text);
        let word_length = word.chars().count();
        expected_characters.extend(word.chars().map(|value| ExpectedCharacter {
            value,
            word_index,
            word_length,
        }));
        if word_index + 1 < expected.len() {
            expected_boundaries.insert(expected_characters.len());
        }
    }

    let mut actual_characters = Vec::new();
    for (token_index, token) in actual.iter().enumerate() {
        let word = collapse_apostrophes_and_rn(&token.text);
        actual_characters.extend(
            word.chars()
                .map(|value| ActualCharacter { value, token_index }),
        );
    }

    MergedMatchContext {
        expected: &expected_characters,
        actual: &actual_characters,
        boundaries: &expected_boundaries,
        memo: HashMap::new(),
    }
    .matches(0, 0, false, 0)
}

fn is_embedded_isolated_digit(characters: &[ActualCharacter], index: usize) -> bool {
    if index == 0 || index + 1 >= characters.len() || !characters[index].value.is_ascii_digit() {
        return false;
    }
    let token = characters[index].token_index;
    characters[index - 1].token_index == token
        && characters[index + 1].token_index == token
        && characters[index - 1].value.is_alphabetic()
        && characters[index + 1].value.is_alphabetic()
}

fn collapse_apostrophes_and_rn(value: &str) -> String {
    value.replace('\'', "").replace("rn", "m")
}

fn is_common_glyph_confusion(left: char, right: char) -> bool {
    ["il1|!", "o0", "s5", "b8"]
        .iter()
        .any(|group| group.contains(left) && group.contains(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poedb_ranges_and_rolls_share_structure() {
        let matcher = FullLineAffixMatcher::new(
            "(6—8)% increased Attack Speed if you've dealt a Critical Strike Recently",
        )
        .unwrap();
        assert_eq!(
            matcher.template().text,
            "<PCT> increased attack speed if you've dealt a critical strike recently"
        );
        assert!(
            matcher.is_match(
                "8(6-8)% increased Attack Speed if you've dealt a Critical Strike Recently"
            )
        );
        assert!(
            !matcher.is_match("8% increased Cast Speed if you've dealt a Critical Strike Recently")
        );
    }

    #[test]
    fn traditional_decimal_glyph_is_one_slot() {
        let matcher = FullLineAffixMatcher::new("+(3.11—3.8)%暴擊率").unwrap();
        assert!(matcher.is_match("+ 3 · 73 % 暴 擊 率"));
        assert_eq!(
            extract_values("+ 3 · 73 % 暴 擊 率"),
            vec![Some(Decimal::from(3.73))]
        );
        assert!(!matcher.is_match("+ 3・73 % 暴 擊 率"));
    }

    #[test]
    fn merged_ocr_words_remain_semantically_strict() {
        let matcher =
            FullLineAffixMatcher::new("1 to (19-20) Added Lightning Damage with Dagger Attacks")
                .unwrap();
        assert!(matcher.is_match("1(1-2) TO 24(23-24) ADDED LIGHTNING DAMAGEWITH2DAGGERATTACKS"));
        assert!(!matcher.is_match("1(1-2) TO 24(23-24) ADDED LIGHTNING DAMAGEWITH2CLAWATTACKS"));
    }

    #[test]
    fn physical_line_join_respects_blank_boundaries() {
        let matcher = FullLineAffixMatcher::new("#% increased Attack Speed Recently").unwrap();
        let lines = vec![
            "8% increased".into(),
            "Attack".into(),
            "Speed".into(),
            "Recently".into(),
        ];
        let found = matcher.find_match(&lines).unwrap();
        assert_eq!(found.physical_line_count, 4);
        assert!(
            matcher
                .find_match(&[
                    "8% increased Attack".into(),
                    "".into(),
                    "Speed Recently".into()
                ])
                .is_none()
        );
    }
}
