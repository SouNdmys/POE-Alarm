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
    game_process_outranks_us,
};

/// How long to wait for the client to answer one Ctrl+C.
///
/// A healthy round trip measures ~3ms with a p99 under 10ms, so anything
/// longer is the client declining to answer rather than answering slowly — and
/// it declines for the whole time a craft is in flight. Waiting it out blinds
/// the source for exactly the window the new roll appears in, which is why
/// this is short: measured detection lag tracked this deadline almost
/// one-for-one (600ms deadline gave a 632ms median, 25ms gave 27ms). The
/// mechanism behind that ratio is the attempt that straddles the moment the
/// client resumes: it burns its whole deadline before the next attempt can
/// succeed instantly, so every spare millisecond here is a millisecond of
/// detection lag. 12ms still clears the p99 of a healthy answer.
const COPY_DEADLINE: Duration = Duration::from_millis(12);

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

/// How fast the cursor may drift and still count as resting, in pixels per
/// second.
///
/// A speed, not a per-poll distance. The distance a cursor covers between two
/// polls depends on how often they happen, so a fixed pixel budget silently
/// loosens the moment polling speeds up — raising the timer resolution alone
/// would have turned slow travel into "resting" and started injecting through
/// it. Expressed as a speed the threshold means the same thing at any poll
/// rate.
///
/// 200px/s is a hand trembling on one spot, not a hand crossing an inventory.
const CURSOR_REST_SPEED: f64 = 200.0;

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
    /// Where the cursor was at the previous poll, and when, for the travel
    /// gate. The instant is what makes the threshold a speed rather than a
    /// per-poll distance.
    last_cursor: Option<(PointI, Instant)>,
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

        // Nothing is injected while the cursor is travelling: a travelling
        // cursor is never over the item the user means, so the client would
        // not answer. There is deliberately no mid-click gate any more. It
        // existed to keep the Shift lift away from the user's presses; with
        // the lift gone the chord touches nothing of theirs, and pausing for
        // the whole of a macro's hold time was costing the probe exactly the
        // window it needed — at an 80ms cadence with a 40ms hold, half of
        // every cycle went dark.
        let resting = match cursor_position() {
            Some(position) => {
                let moved = self.last_cursor.is_some_and(|(last, at)| {
                    let seconds = started.saturating_duration_since(at).as_secs_f64();
                    if seconds <= 0.0 {
                        return false;
                    }
                    let dx = f64::from(position.x - last.x);
                    let dy = f64::from(position.y - last.y);
                    dx.hypot(dy) / seconds > CURSOR_REST_SPEED
                });
                self.last_cursor = Some((position, started));
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

    /// The travel gate must mean the same thing at any poll rate.
    ///
    /// It used to be a per-poll pixel budget, which quietly loosened whenever
    /// polling sped up: raising the timer resolution alone would have turned
    /// slow travel into "resting" and started injecting through it. This walks
    /// one cursor speed past the threshold at two very different poll rates and
    /// requires the same verdict from both.
    #[test]
    fn the_travel_gate_does_not_drift_with_the_poll_rate() {
        fn is_travelling(pixels_per_second: f64, poll: Duration) -> bool {
            let seconds = poll.as_secs_f64();
            let step = pixels_per_second * seconds;
            step / seconds > CURSOR_REST_SPEED
        }

        let slow_poll = Duration::from_millis(16);
        let fast_poll = Duration::from_millis(4);

        for speed in [50.0, 150.0, 199.0] {
            assert!(!is_travelling(speed, slow_poll), "{speed} resting at 16ms");
            assert!(!is_travelling(speed, fast_poll), "{speed} resting at 4ms");
        }
        for speed in [201.0, 400.0, 2000.0] {
            assert!(is_travelling(speed, slow_poll), "{speed} moving at 16ms");
            assert!(is_travelling(speed, fast_poll), "{speed} moving at 4ms");
        }
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
