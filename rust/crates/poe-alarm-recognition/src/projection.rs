use std::collections::HashSet;

use poe_alarm_core::{PhysicalLineIdentity, canonicalize, normalize_numeric_presentation};
use poe_alarm_ocr_win::RecognizedLine;
use poe_alarm_vision::CropMetadata;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FloatRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl FloatRect {
    pub fn height(self) -> f32 {
        self.bottom - self.top
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

pub(crate) fn line_bounds(line: &RecognizedLine) -> Option<FloatRect> {
    if line.width <= 0.0 || line.height <= 0.0 {
        return None;
    }
    Some(FloatRect {
        left: line.left,
        top: line.top,
        right: line.left + line.width,
        bottom: line.top + line.height,
    })
}

pub(crate) fn normalize_chinese_numeric_dashes(text: &str) -> String {
    let normalized = normalize_numeric_presentation(text);
    let mut characters: Vec<char> = normalized.chars().collect();
    for index in 0..characters.len() {
        if characters[index] != '一' {
            continue;
        }
        let previous = (0..index)
            .rev()
            .find(|candidate| !characters[*candidate].is_whitespace());
        let next =
            (index + 1..characters.len()).find(|candidate| !characters[*candidate].is_whitespace());
        if previous.is_some_and(|value| characters[value].is_ascii_digit())
            && next.is_some_and(|value| characters[value].is_ascii_digit())
        {
            characters[index] = '-';
        }
    }
    characters.into_iter().collect()
}

pub(crate) fn has_layout_boundary(previous: FloatRect, current: FloatRect) -> bool {
    if current.top < previous.top {
        return true;
    }
    let vertical_gap = current.top - previous.bottom;
    let reference_height = previous.height().max(current.height());
    vertical_gap > 12.0_f32.max(reference_height * 0.9)
}

#[derive(Clone, Debug)]
pub(crate) struct ChineseProjection {
    pub lines: Vec<String>,
    pub identities: Vec<PhysicalLineIdentity>,
    pub detected: Vec<DetectedLine>,
}

#[derive(Clone, Debug)]
pub(crate) struct DetectedLine {
    pub text: String,
    pub bounds: FloatRect,
    pub logical_line_index: usize,
    pub physical_band_id: String,
}

pub(crate) fn project_chinese_lines(
    recognized: &[RecognizedLine],
    combined: &CropMetadata,
) -> ChineseProjection {
    let mut lines = Vec::with_capacity(recognized.len() * 2);
    let mut identities = Vec::with_capacity(recognized.len());
    let mut detected = Vec::with_capacity(recognized.len());
    let mut previous_bounds = None;
    for line in recognized {
        let text = normalize_chinese_numeric_dashes(line.text.trim());
        let Some(bounds) = line_bounds(line) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if previous_bounds.is_some_and(|previous| has_layout_boundary(previous, bounds)) {
            lines.push(String::new());
        }
        let line_index = lines.len();
        lines.push(text.clone());
        let top = bounds.top.floor() as i32;
        let bottom = bounds.bottom.ceil() as i32;
        let band_id = format!("winzh:{:016x}:{top}:{bottom}", combined.content_fingerprint);
        identities.push(PhysicalLineIdentity {
            line_index,
            physical_band_id: band_id.clone(),
        });
        detected.push(DetectedLine {
            text,
            bounds,
            logical_line_index: line_index,
            physical_band_id: band_id,
        });
        previous_bounds = Some(bounds);
    }
    ChineseProjection {
        lines,
        identities,
        detected,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct BandedView {
    pub metadata: CropMetadata,
    pub lines: Vec<String>,
}

pub(crate) fn project_banded_lines(
    views: &[BandedView],
    roi_width: usize,
    scale: usize,
    namespace: &str,
    structured: bool,
) -> (Vec<String>, Vec<PhysicalLineIdentity>) {
    let regular_canonical: HashSet<String> = views
        .iter()
        .filter(|view| !view.metadata.is_fallback)
        .flat_map(|view| &view.lines)
        .map(|line| canonicalize(line).text)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let mut lines = Vec::new();
    let mut identities = Vec::new();
    let mut previous_bottom = None;
    let mut previous_height = 0;
    let mut previous_width = 0;
    for view in views {
        let projected: Vec<&str> = view
            .lines
            .iter()
            .map(String::as_str)
            .filter(|line| !line.trim().is_empty())
            .filter(|line| {
                !structured
                    || !view.metadata.is_fallback
                    || !regular_canonical.contains(&canonicalize(line).text)
            })
            .collect();
        if projected.is_empty() {
            continue;
        }
        if view.metadata.is_fallback {
            add_boundary(&mut lines);
            for text in projected {
                let line_index = lines.len();
                lines.push(text.to_owned());
                identities.push(PhysicalLineIdentity {
                    line_index,
                    physical_band_id: band_id(namespace, &view.metadata),
                });
                add_boundary(&mut lines);
            }
            previous_bottom = None;
            previous_height = 0;
            previous_width = 0;
            continue;
        }
        let content = view.metadata.content_rect;
        if let Some(bottom) = previous_bottom {
            let gap = content.y.saturating_sub(bottom + 1);
            let close_enough = gap <= 12_usize.max(previous_height.max(content.height));
            let previous_filled = previous_width >= (roi_width as f64 * 0.70) as usize;
            if !close_enough || !previous_filled {
                add_boundary(&mut lines);
            }
        }
        let id = band_id(namespace, &view.metadata);
        for text in projected {
            let line_index = lines.len();
            lines.push(text.to_owned());
            identities.push(PhysicalLineIdentity {
                line_index,
                physical_band_id: id.clone(),
            });
        }
        previous_bottom = Some(content.bottom_inclusive());
        previous_height = content.height;
        previous_width = view.metadata.source_rect.width * scale;
    }
    (lines, identities)
}

fn add_boundary(lines: &mut Vec<String>) {
    if lines.last().is_some_and(|line| !line.is_empty()) {
        lines.push(String::new());
    }
}

pub(crate) fn band_id(namespace: &str, metadata: &CropMetadata) -> String {
    let content = metadata.content_rect;
    format!(
        "{namespace}:{:016x}:{}:{}:{}:{}",
        metadata.content_fingerprint,
        content.y,
        content.bottom_inclusive(),
        metadata.source_rect.x,
        metadata.source_rect.right_inclusive()
    )
}
