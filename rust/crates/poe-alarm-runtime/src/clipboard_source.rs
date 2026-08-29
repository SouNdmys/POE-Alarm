//! Reads affix lines from copies the user makes, injecting nothing.
//!
//! This is the passive successor to the timed injector, which was itself the
//! successor to the OCR pipeline. GGG's developer documentation requires any
//! synthesized input that affects the game to be invoked manually and names
//! timers as a disallowed trigger — which is exactly what the injector was. So
//! monitoring no longer sends anything at all: the user presses `Ctrl+C`
//! themselves (the game's own copy feature), and this source only watches the
//! clipboard sequence number, reads the payload when it moves, and hands the
//! parsed item to the rules.
//!
//! The one synthesized input left anywhere in the app is the manual check
//! hotkey, which sends a single chord per press — one manual invocation, one
//! fixed function, one action, per those same rules.

use std::fmt;
use std::time::{Duration, Instant};

use poe_alarm_clipboard::{ParsedItem, parse};
use poe_alarm_monitoring::{
    AffixSource, CancellationToken, MonitorPlan, ReadFailure, RecognitionResult,
    StructuredOcrSupport,
};
use poe_alarm_platform_win::{ClipboardError, game_is_foreground, read_text, sequence_number};

/// How many times to re-try opening the clipboard before reporting it busy.
///
/// The copy that moved the sequence number may still hold the clipboard open
/// for a moment; each attempt waits a millisecond. Sixty is far beyond any
/// healthy handoff and short enough that a genuinely wedged clipboard turns
/// into a visible error rather than a silent stall.
const CLIPBOARD_OPEN_ATTEMPTS: u32 = 60;

/// Why a reading could not be taken.
#[derive(Debug)]
pub enum SourceError {
    /// The clipboard could not be read.
    Clipboard(ClipboardError),
    /// A source that is not the clipboard failed.
    Other(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clipboard(error) => write!(formatter, "clipboard read failed: {error}"),
            Self::Other(detail) => formatter.write_str(detail),
        }
    }
}

impl ReadFailure for SourceError {
    fn is_actionable(&self) -> bool {
        // The injector had one actionable failure: Windows discarding its
        // keystrokes when the game ran elevated. Nothing is injected any more,
        // so elevation is irrelevant to monitoring and no failure here is
        // something a privilege change could fix.
        false
    }
}

/// Affix source backed by copies the user makes in the game client.
pub struct ClipboardSource {
    /// The clipboard sequence number at the last poll. `None` until the first
    /// poll, so whatever the clipboard held before the session becomes the
    /// baseline rather than a detection.
    last_sequence: Option<u32>,
    /// The last payload judged, used to answer "has this item moved".
    last_payload: Option<String>,
    /// The last item judged, used to notice the cursor moving to another item.
    last_item: Option<ParsedItem>,
}

impl Default for ClipboardSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardSource {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_sequence: None,
            last_payload: None,
            last_item: None,
        }
    }

    /// A reading that carries nothing new, so the loop paces down.
    fn unchanged(elapsed: Duration) -> RecognitionResult {
        RecognitionResult {
            was_cached: true,
            recognition_elapsed: elapsed,
            ..RecognitionResult::default()
        }
    }
}

impl AffixSource for ClipboardSource {
    type Error = SourceError;

    fn structured_support(&self) -> StructuredOcrSupport {
        // One reading carries the whole item, so every target is judged
        // together by construction.
        StructuredOcrSupport::StrictBatch
    }

    fn read(
        &mut self,
        _plan: &MonitorPlan,
        _cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, Self::Error> {
        let started = Instant::now();

        // The foreground gate is a privacy boundary now, not an injection
        // guard: while the game is not the foreground window, the clipboard
        // belongs to whatever else the user is doing, and this source must
        // not read it — not even to compare. The sequence number is a global
        // counter, not content, so sampling it here is fine and keeps a copy
        // made while tabbed out from being misread as a roll later.
        if !game_is_foreground() {
            self.last_sequence = Some(sequence_number());
            return Ok(Self::unchanged(started.elapsed()));
        }

        // A pure counter read. Nothing is sent, nothing is opened; until the
        // user actually copies something, every poll ends here.
        let sequence = sequence_number();
        if self.last_sequence == Some(sequence) {
            return Ok(Self::unchanged(started.elapsed()));
        }

        let text = match read_text(CLIPBOARD_OPEN_ATTEMPTS) {
            Ok((text, _attempts)) => text,
            // The copier may still hold the clipboard open. The sequence
            // number is deliberately NOT recorded, so the change is retried
            // on the next poll instead of being swallowed.
            Err(ClipboardError::Busy { .. }) => {
                return Ok(Self::unchanged(started.elapsed()));
            }
            // Not text (or empty): the user copied something that is not an
            // item. Recorded as seen — waiting will not turn a bitmap into
            // item text.
            Err(ClipboardError::NoTextFormat { .. } | ClipboardError::EmptyText) => {
                self.last_sequence = Some(sequence);
                return Ok(Self::unchanged(started.elapsed()));
            }
            Err(error) => return Err(SourceError::Clipboard(error)),
        };
        self.last_sequence = Some(sequence);

        if self
            .last_payload
            .as_deref()
            .is_some_and(|previous| previous == text)
        {
            return Ok(Self::unchanged(started.elapsed()));
        }

        let Ok(item) = parse(&text) else {
            // Readable clipboard, unreadable item — the user copied something
            // that is not an item. Leave the baseline alone so the roll being
            // waited on is still pending.
            return Ok(Self::unchanged(started.elapsed()));
        };

        // Text changing is not proof the *same* item was rerolled: copying a
        // different item changes it too. Judging that would report the wrong
        // affixes and, worse, adopt them as the baseline, after which the roll
        // the user is waiting on never gets compared again.
        if let Some(previous) = &self.last_item
            && !previous.is_same_item_as(&item)
        {
            self.last_payload = Some(text);
            self.last_item = Some(item);
            return Ok(Self::unchanged(started.elapsed()));
        }

        let first_reading = self.last_payload.is_none();
        self.last_payload = Some(text);
        let (lines, physical_line_identities) = item.render();
        self.last_item = Some(item);

        Ok(RecognitionResult {
            lines,
            physical_line_identities,
            recognition_elapsed: started.elapsed(),
            // The first reading establishes what the item looked like before
            // anything happened; there is nothing to compare it against yet.
            was_cached: first_reading,
            ..RecognitionResult::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poe_alarm_platform_win::ClipboardError;

    /// The branch's core promise, enforced at the source level: monitoring
    /// never injects. The manual check hotkey is the only caller allowed to
    /// copy, and it lives in the app crate, not here.
    #[test]
    fn the_monitoring_source_never_injects() {
        let source = include_str!("clipboard_source.rs");
        for needle in [
            concat!("copy_hovered", "_item"),
            concat!("send_", "ctrl_c"),
            concat!("Send", "Input"),
        ] {
            assert!(
                !source.contains(needle),
                "monitoring must stay injection-free, but the source mentions {needle}"
            );
        }
    }

    #[test]
    fn no_passive_failure_asks_the_user_to_elevate() {
        // The injector's actionable failure was Windows discarding keystrokes
        // sent at an elevated game. Nothing is sent any more, so nothing may
        // route the user to the elevation dialog.
        assert!(!SourceError::Clipboard(ClipboardError::Busy { attempts: 60 }).is_actionable());
        assert!(!SourceError::Other("anything".to_owned()).is_actionable());
    }
}
