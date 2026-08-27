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

use poe_alarm_clipboard::{ParsedItem, parse};
use poe_alarm_monitoring::{
    AffixSource, CancellationToken, MonitorPlan, RecognitionResult, StructuredOcrSupport,
};
use poe_alarm_platform_win::{
    ClipboardError, KeyMethod, copy_hovered_item, foreground_process_outranks_us,
    game_is_foreground,
};

/// How long to wait for the client to answer one Ctrl+C.
///
/// A healthy round trip measures ~3ms with a p99 under 10ms, so anything
/// longer is the client declining to answer rather than answering slowly — and
/// it declines for the whole time a craft is in flight. Waiting it out blinds
/// the source for exactly the window the new roll appears in, which is why
/// this is short: measured detection lag tracked this deadline almost
/// one-for-one (600ms deadline gave a 632ms median, 25ms gives 27ms).
const COPY_DEADLINE: Duration = Duration::from_millis(25);

/// How many unanswered copies in a row mean something is structurally wrong.
///
/// A craft blocks the client for a fraction of a second — a handful of polls at
/// most. Windows refusing to deliver the keystroke at all looks identical from
/// here, except that it never stops. This is the line between the two, set far
/// enough out that no craft can reach it and near enough that a user notices
/// within seconds rather than after a league of empty scans.
const SILENT_FAILURE_STREAK: u32 = 60;

/// Why a reading could not be taken.
#[derive(Debug)]
pub enum SourceError {
    /// The client refused or ignored the request.
    Clipboard(ClipboardError),
    /// The client stopped answering the copy request and never resumed.
    ///
    /// Carries whether the elevation check agreed, but does not depend on it:
    /// that check is a heuristic, and gating the report on it is what let this
    /// failure stay silent through 2330 scans in the field. A monitor that has
    /// been talking to nothing for a minute has to say so whatever the cause.
    Unanswered { attempts: u32, outranked: bool },
    /// A source that is not the clipboard failed.
    Other(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clipboard(error) => write!(formatter, "clipboard capture failed: {error}"),
            Self::Unanswered {
                attempts,
                outranked: true,
            } => write!(
                formatter,
                "the game did not answer {attempts} copy requests in a row. It is running with                  higher privileges than POE Alarm, so Windows is discarding them. Restart POE                  Alarm as administrator: Settings -> Privileges.",
            ),
            Self::Unanswered { attempts, .. } => write!(
                formatter,
                "the game did not answer {attempts} copy requests in a row. The usual cause is                  the game running as administrator while POE Alarm is not, which makes Windows                  discard them: try Settings -> Privileges -> Restart as administrator.",
            ),
            Self::Other(detail) => formatter.write_str(detail),
        }
    }
}

/// Affix source backed by the client's own item text.
pub struct ClipboardSource {
    key_method: KeyMethod,
    /// The last payload judged, used to answer "has this item moved".
    last_payload: Option<String>,
    /// The last item judged, used to notice the cursor moving to another item.
    last_item: Option<ParsedItem>,
    /// Consecutive unanswered copies while the game held focus.
    unanswered: u32,
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
            key_method: KeyMethod::default(),
            last_payload: None,
            last_item: None,
            unanswered: 0,
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
            Err(error) if error.is_transient() => {
                self.unanswered = self.unanswered.saturating_add(1);
                // Silence that never ends is not a craft. The usual cause is
                // the game running elevated — an accelerator or a launcher
                // started it as administrator — after which Windows drops every
                // keystroke this process sends and monitoring runs forever
                // without ever alarming. Saying so beats looking healthy.
                if self.unanswered >= SILENT_FAILURE_STREAK {
                    return Err(SourceError::Unanswered {
                        attempts: self.unanswered,
                        outranked: foreground_process_outranks_us(),
                    });
                }
                return Ok(Self::unchanged(started.elapsed()));
            }
            Err(error) => return Err(SourceError::Clipboard(error)),
        };
        self.unanswered = 0;

        if self
            .last_payload
            .as_deref()
            .is_some_and(|previous| previous == outcome.text)
        {
            return Ok(Self::unchanged(started.elapsed()));
        }

        let Ok(item) = parse(&outcome.text) else {
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
