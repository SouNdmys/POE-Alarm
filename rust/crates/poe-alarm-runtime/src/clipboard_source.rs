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
    AffixSource, CancellationToken, MonitorPlan, ReadFailure, RecognitionResult,
    StructuredOcrSupport,
};
use poe_alarm_platform_win::{
    ClipboardError, KeyMethod, PointI, copy_hovered_item, cursor_position, game_is_foreground,
    game_process_outranks_us, primary_button_down,
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
///
/// Crossing it proves nothing by itself: a cursor resting over empty ground
/// produces exactly this silence, because the client only answers while an
/// item is hovered. The streak earns a report only when the integrity
/// comparison confirms the game outranks this process.
const SILENT_FAILURE_STREAK: u32 = 60;

/// How far the cursor may drift between polls and still count as resting.
///
/// A hand trembles by a pixel or two while spam-clicking one spot; travelling
/// to an item covers tens of pixels per poll. Nothing meaningful lives between.
const CURSOR_REST_TOLERANCE: i32 = 3;

/// Why a reading could not be taken.
#[derive(Debug)]
pub enum SourceError {
    /// The client refused or ignored the request.
    Clipboard(ClipboardError),
    /// The client never answered while the game outranks this process.
    ///
    /// Only produced when the integrity comparison agrees, because unanswered
    /// alone is the ordinary state of a cursor resting over nothing. The
    /// comparison errs toward reporting — every failure to read a token counts
    /// as outranked — so the silent-forever failure this once had cannot come
    /// back through it; only a confident same-level-or-lower answer suppresses
    /// the report, and elevating genuinely cannot help then.
    Unanswered { attempts: u32 },
    /// A source that is not the clipboard failed.
    Other(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clipboard(error) => write!(formatter, "clipboard capture failed: {error}"),
            Self::Unanswered { attempts } => write!(
                formatter,
                "the game did not answer {attempts} copy requests in a row. It is running with \
                 higher privileges than POE Alarm, so Windows is discarding them. Restart POE \
                 Alarm as administrator: Settings -> Privileges."
            ),
            Self::Other(detail) => formatter.write_str(detail),
        }
    }
}

impl ReadFailure for SourceError {
    fn is_actionable(&self) -> bool {
        // Sixty unanswered copies is never a hiccup, and the fix is always
        // something only the user can do. Deliberately not conditioned on the
        // privilege check: that is a guess about the cause, and the whole
        // reason this exists is that a wrong guess left the failure silent.
        matches!(self, Self::Unanswered { .. })
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
    /// Where the cursor was at the previous poll, for the travel gate.
    last_cursor: Option<PointI>,
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
            last_cursor: None,
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

        // Nothing is injected mid-click or while the cursor is travelling: the
        // client only answers over a hovered item, so those injections would be
        // no-ops for this monitor and real keystrokes to the game.
        if primary_button_down() {
            return Ok(Self::unchanged(started.elapsed()));
        }
        let resting = match cursor_position() {
            Some(position) => {
                let moved = self.last_cursor.is_some_and(|last| {
                    (position.x - last.x).abs() > CURSOR_REST_TOLERANCE
                        || (position.y - last.y).abs() > CURSOR_REST_TOLERANCE
                });
                self.last_cursor = Some(position);
                !moved
            }
            None => true,
        };
        if !resting {
            return Ok(Self::unchanged(started.elapsed()));
        }

        // No pacing beyond the monitor's own. The injected chord leaves the
        // user's modifiers alone, so a copy has no visible effect and there is
        // nothing to ration. Three generations of click-gating, chase windows
        // and keepalives existed to ration a Shift flicker that a misdiagnosis
        // had made necessary; with the lift gone, free running is both the
        // simplest and the fastest this has ever been.

        let outcome = match copy_hovered_item(COPY_DEADLINE, self.key_method) {
            Ok(outcome) => outcome,
            // The client goes quiet for the duration of a craft, so a missing
            // payload means it had nothing to give yet. Treating that as a
            // fault would end the session every time the user crafted.
            Err(error) if error.is_transient() => {
                self.unanswered = self.unanswered.saturating_add(1);
                // Silence that never ends is not a craft — but it is also the
                // ordinary state of a cursor resting over empty ground, so the
                // streak alone must not interrupt anyone. The integrity
                // comparison decides, and it errs toward reporting: only a
                // confident answer that the game does not outrank us quietly
                // forgives the streak, and elevating could not have helped
                // then anyway.
                if self.unanswered >= SILENT_FAILURE_STREAK {
                    if game_process_outranks_us() {
                        return Err(SourceError::Unanswered {
                            attempts: self.unanswered,
                        });
                    }
                    self.unanswered = 0;
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

#[cfg(test)]
mod tests {
    use super::*;
    use poe_alarm_platform_win::ClipboardError;
    use std::time::Duration;

    #[test]
    fn an_unanswered_streak_is_actionable() {
        // By the time this error exists, the integrity comparison has already
        // confirmed the mismatch — the read path cannot construct the variant
        // any other way — so it is always the user's to fix.
        assert!(
            SourceError::Unanswered {
                attempts: SILENT_FAILURE_STREAK,
            }
            .is_actionable()
        );
    }

    #[test]
    fn an_ordinary_clipboard_hiccup_is_not_actionable() {
        // A craft in flight looks exactly like this and needs no dialog.
        assert!(
            !SourceError::Clipboard(ClipboardError::Timeout {
                waited: Duration::from_millis(25)
            })
            .is_actionable()
        );
    }
}
