//! Reads affix lines by asking the client for the item under the cursor.
//!
//! This is what replaced the OCR pipeline. Where that captured a frame, masked
//! it for blue text, split it into bands and recognised each one, this presses
//! Ctrl+C and parses the reply. The client hands over the item exactly, so
//! there is no recognition step left to be wrong about — and no capture region
//! to select, no model to ship, and nothing to recalibrate when the scene
//! behind the tooltip changes.

use std::fmt;
use std::time::{Duration, Instant};

use poe_alarm_clipboard::{ModFilter, ParsedItem, parse};
use poe_alarm_monitoring::{
    AffixSource, CancellationToken, MonitorPlan, RecognitionResult, StructuredOcrSupport,
};
use poe_alarm_platform_win::{ClipboardError, KeyMethod, copy_hovered_item, game_is_foreground};

/// How long to wait for the client to answer one Ctrl+C.
///
/// A healthy round trip measures ~3ms with a p99 under 10ms, so anything
/// longer is the client declining to answer rather than answering slowly — and
/// it declines for the whole time a craft is in flight. Waiting it out blinds
/// the source for exactly the window the new roll appears in, which is why
/// this is short: measured detection lag tracked this deadline almost
/// one-for-one (600ms deadline gave a 632ms median, 25ms gives 27ms).
const COPY_DEADLINE: Duration = Duration::from_millis(25);

/// Why a reading could not be taken.
#[derive(Debug)]
pub enum SourceError {
    /// The client refused or ignored the request.
    Clipboard(ClipboardError),
    /// A source that is not the clipboard failed.
    Other(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clipboard(error) => write!(formatter, "clipboard capture failed: {error}"),
            Self::Other(detail) => formatter.write_str(detail),
        }
    }
}

/// Affix source backed by the client's own item text.
pub struct ClipboardSource {
    filter: ModFilter,
    key_method: KeyMethod,
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
            filter: ModFilter::default(),
            key_method: KeyMethod::default(),
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

        // Injected input only reaches a focused window, and copying while the
        // user is elsewhere would clobber their clipboard for nothing.
        if !game_is_foreground() {
            return Ok(Self::unchanged(started.elapsed()));
        }

        let outcome = match copy_hovered_item(COPY_DEADLINE, self.key_method) {
            Ok(outcome) => outcome,
            // The client goes quiet for the duration of a craft, so a missing
            // payload means it had nothing to give yet. Treating that as a
            // fault would end the session every time the user crafted.
            Err(error) if error.is_transient() => return Ok(Self::unchanged(started.elapsed())),
            Err(error) => return Err(SourceError::Clipboard(error)),
        };

        if self
            .last_payload
            .as_deref()
            .is_some_and(|previous| previous == outcome.text)
        {
            return Ok(Self::unchanged(started.elapsed()));
        }

        let Ok(item) = parse(&outcome.text, self.filter) else {
            // Readable clipboard, unreadable item — the cursor is over
            // something that is not an item. Leave the baseline alone so the
            // roll being waited on is still pending.
            return Ok(Self::unchanged(started.elapsed()));
        };

        // Text changing is not proof the *same* item was rerolled: a drifting
        // cursor changes it too. Judging that would report the wrong affixes
        // and, worse, adopt them as the baseline, after which the roll the user
        // is waiting on never gets compared again.
        if let Some(previous) = &self.last_item
            && !previous.is_same_item_as(&item)
        {
            self.last_payload = Some(outcome.text);
            self.last_item = Some(item);
            return Ok(Self::unchanged(started.elapsed()));
        }

        let first_reading = self.last_payload.is_none();
        self.last_payload = Some(outcome.text);
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
