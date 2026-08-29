//! Reads affix lines via one copy per user click, plus copies the user makes.
//!
//! GGG's developer documentation requires synthesized input that affects the
//! game to be invoked manually by the user, and names timers among the
//! disallowed triggers. The old monitoring was a free-running timer sending
//! `Ctrl+C` twenty times a second; this source replaces it with a copy that is
//! *invoked by the user's own click*: a pass-through hook counts left clicks
//! (suppressing and synthesizing nothing), and after each click — once the
//! server has had time to answer — exactly one `Ctrl+C` chord is sent to read
//! the result. One manual invocation, one fixed function, one action. Copies
//! the user makes by hand are honored too, through the clipboard sequence
//! number, so pressing `Ctrl+C` yourself always works.
//!
//! What remains grey and is stated rather than argued: the invoking press is
//! the crafting click itself, doing double duty. Whether that satisfies
//! "invoked manually" is GGG's call; a timer it is not.

use std::fmt;
use std::time::{Duration, Instant};

use poe_alarm_clipboard::{ParsedItem, parse};
use poe_alarm_monitoring::{
    AffixSource, CancellationToken, MonitorPlan, ReadFailure, RecognitionResult,
    StructuredOcrSupport,
};
use poe_alarm_platform_win::{
    ClickObserver, ClipboardError, KeyMethod, copy_hovered_item, game_is_foreground,
    game_process_outranks_us, observed_clicks, read_text, sequence_number, start_click_observer,
};

/// How long to wait for the client to answer one Ctrl+C.
///
/// A healthy round trip measures ~3ms with a p99 under 10ms; longer means the
/// client is declining to answer (a craft in flight does that), and waiting it
/// out only delays the retry.
const COPY_DEADLINE: Duration = Duration::from_millis(12);

/// How long after a click before the copy is sent.
///
/// The new affixes exist only after a server round trip, so a copy fired too
/// early reads the old item. 80ms clears a ~60ms in-game latency with margin;
/// `POE_ALARM_COPY_DELAY_MS` overrides it for other connections.
const DEFAULT_COPY_DELAY: Duration = Duration::from_millis(80);

/// Environment variable overriding the click-to-copy delay, in milliseconds.
pub const COPY_DELAY_OVERRIDE: &str = "POE_ALARM_COPY_DELAY_MS";

/// Gap between re-copies when the first copy still shows the old text.
const RECOPY_GAP: Duration = Duration::from_millis(50);

/// Most chords one click may cost, counting the first copy and re-copies.
///
/// Bounded and small on purpose: this is the entire injection budget of one
/// click, and it is spent only when the client answers with stale text or not
/// at all. Three attempts spaced by `RECOPY_GAP` cover a round trip of ~180ms.
const MAX_COPIES_PER_CLICK: u32 = 3;

/// How many times to re-try opening the clipboard for a user-made copy.
const CLIPBOARD_OPEN_ATTEMPTS: u32 = 60;

/// Why a reading could not be taken.
#[derive(Debug)]
pub enum SourceError {
    /// The clipboard could not be read.
    Clipboard(ClipboardError),
    /// The game runs with higher privileges than this process.
    ///
    /// Both halves of the click-invoked copy die against that wall: Windows
    /// neither delivers our chord to an elevated window nor lets our
    /// unelevated hook observe clicks headed to one. Detected up front so the
    /// user hears about it immediately instead of after a silent session.
    Outranked,
    /// A source that is not the clipboard failed.
    Other(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clipboard(error) => write!(formatter, "clipboard read failed: {error}"),
            Self::Outranked => formatter.write_str(
                "the game is running with higher privileges than POE Alarm, so Windows hides \
                 your clicks from it and discards its copies. Restart POE Alarm as \
                 administrator: Settings -> Privileges.",
            ),
            Self::Other(detail) => formatter.write_str(detail),
        }
    }
}

impl ReadFailure for SourceError {
    fn is_actionable(&self) -> bool {
        // Outranked is the one failure the user can fix, and the fix is
        // exactly the elevation flow the UI already offers.
        matches!(self, Self::Outranked)
    }
}

/// Reads the copy-delay override once, ignoring anything unparseable.
fn copy_delay_override(value: Option<&std::ffi::OsStr>) -> Option<Duration> {
    let raw = value?.to_str()?;
    let millis: u64 = raw.trim().parse().ok()?;
    (1..=1000)
        .contains(&millis)
        .then(|| Duration::from_millis(millis))
}

/// Where the start-invoked baseline copy stands.
///
/// The first reading is never evaluated (an item that already matches must
/// not lock the screen the moment monitoring starts), so without a baseline
/// the first roll would be swallowed as one. Instead of asking the user to
/// copy by hand, the start-monitoring press itself invokes one bounded copy
/// attempt — the same shape as a price checker's hotkey: one press of yours,
/// one copy, and the budget is spent whether it succeeds or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Baseline {
    /// Not yet attempted; schedule one bounded copy when the gates allow.
    Pending,
    /// The attempt is using the shared copy machinery right now.
    Running,
    /// Attempted (or superseded); no further unprompted copies, ever.
    Done,
}

/// What one ingested clipboard payload turned out to be.
enum Ingested {
    /// New text for the same item: real evidence, evaluate it.
    Evidence(RecognitionResult),
    /// Identical to the last payload — the roll has not landed yet.
    SameText,
    /// Anything else that ends the attempt: baseline, other item, not an item.
    Settled(RecognitionResult),
}

/// Affix source: one copy per user click, plus the user's own copies.
pub struct ClipboardSource {
    key_method: KeyMethod,
    /// Pass-through click counter, held for its lifetime: dropping the source
    /// unhooks it. `None` when the observer failed to start.
    _observer: Option<ClickObserver>,
    /// Why the observer is `None`, reported once.
    observer_failure: Option<String>,
    copy_delay: Duration,
    /// Clicks already answered with a copy (or given up on).
    clicks_handled: u64,
    /// When the newest unanswered click was observed.
    click_seen_at: Option<Instant>,
    /// Chords spent on the current click.
    copies_this_click: u32,
    /// Earliest instant the next copy may fire.
    next_copy_at: Option<Instant>,
    /// The clipboard sequence number at the last poll.
    last_sequence: Option<u32>,
    /// The last payload judged, used to answer "has this item moved".
    last_payload: Option<String>,
    /// The last item judged, used to notice the cursor moving to another item.
    last_item: Option<ParsedItem>,
    /// Clipboard changes skipped while the game was unfocused.
    ignored_while_unfocused: u64,
    /// The start-invoked baseline copy's progress.
    baseline: Baseline,
}

impl Default for ClipboardSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardSource {
    #[must_use]
    pub fn new() -> Self {
        let (observer, observer_failure) = match start_click_observer() {
            Ok(observer) => (Some(observer), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            key_method: KeyMethod::default(),
            _observer: observer,
            observer_failure,
            copy_delay: copy_delay_override(std::env::var_os(COPY_DELAY_OVERRIDE).as_deref())
                .unwrap_or(DEFAULT_COPY_DELAY),
            clicks_handled: observed_clicks(),
            click_seen_at: None,
            copies_this_click: 0,
            next_copy_at: None,
            last_sequence: None,
            last_payload: None,
            last_item: None,
            ignored_while_unfocused: 0,
            baseline: Baseline::Pending,
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

    /// An unchanged reading that still says why, so the log can tell "nothing
    /// happened" apart from "something happened and was digested silently".
    /// Never echoes clipboard content — only the classification.
    fn noted(elapsed: Duration, note: String) -> RecognitionResult {
        RecognitionResult {
            lines: vec![note],
            was_cached: true,
            recognition_elapsed: elapsed,
            ..RecognitionResult::default()
        }
    }

    /// Shared judgement for one clipboard payload, however it arrived.
    fn ingest(&mut self, text: String, started: Instant) -> Ingested {
        if self
            .last_payload
            .as_deref()
            .is_some_and(|previous| previous == text)
        {
            return Ingested::SameText;
        }

        let Ok(item) = parse(&text) else {
            // Readable clipboard, unreadable item. The content is deliberately
            // not echoed: the clipboard may hold anything.
            return Ingested::Settled(Self::noted(
                started.elapsed(),
                "剪贴板内容不是物品文本 (not an item)".to_owned(),
            ));
        };

        // Text changing is not proof the *same* item was rerolled: copying a
        // different item changes it too. Judging that would report the wrong
        // affixes and adopt them as the baseline.
        if let Some(previous) = &self.last_item
            && !previous.is_same_item_as(&item)
        {
            self.last_payload = Some(text);
            self.last_item = Some(item);
            return Ingested::Settled(Self::noted(
                started.elapsed(),
                "换了物品,重新建立基准 (different item; rebaselined)".to_owned(),
            ));
        }

        let first_reading = self.last_payload.is_none();
        self.last_payload = Some(text);
        let (lines, physical_line_identities) = item.render();
        self.last_item = Some(item);

        let result = RecognitionResult {
            lines,
            physical_line_identities,
            recognition_elapsed: started.elapsed(),
            // The first reading establishes what the item looked like before
            // anything happened; there is nothing to compare it against yet.
            was_cached: first_reading,
            ..RecognitionResult::default()
        };
        if first_reading {
            Ingested::Settled(result)
        } else {
            Ingested::Evidence(result)
        }
    }

    /// Finishes the current click, win or lose.
    fn settle_click(&mut self) {
        self.clicks_handled = observed_clicks();
        self.click_seen_at = None;
        self.copies_this_click = 0;
        self.next_copy_at = None;
        if self.baseline == Baseline::Running {
            self.baseline = Baseline::Done;
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

        // Privacy boundary: while the game is not the foreground window the
        // clipboard belongs to whatever else the user is doing, and this
        // source must not read it. The sequence number is a global counter,
        // not content; recording it keeps a copy made while tabbed out from
        // being misread as a roll later. Clicks made elsewhere are likewise
        // written off.
        if !game_is_foreground() {
            let sequence = sequence_number();
            if self.last_sequence.is_some_and(|last| last != sequence) {
                self.ignored_while_unfocused = self.ignored_while_unfocused.saturating_add(1);
            }
            self.last_sequence = Some(sequence);
            self.settle_click();
            return Ok(Self::unchanged(started.elapsed()));
        }

        // Both halves of the click-invoked copy die against an elevated game:
        // Windows hides clicks from our unelevated hook and discards our
        // chord. Reported immediately — the old sixty-poll streak took
        // seconds to notice what this can see at once.
        if game_process_outranks_us() {
            return Err(SourceError::Outranked);
        }

        if let Some(failure) = self.observer_failure.take() {
            return Err(SourceError::Other(format!(
                "the click observer could not start, so click-invoked copies are unavailable: \
                 {failure}"
            )));
        }

        if self.ignored_while_unfocused > 0 {
            let ignored = self.ignored_while_unfocused;
            self.ignored_while_unfocused = 0;
            return Ok(Self::noted(
                started.elapsed(),
                format!("游戏后台期间剪贴板有 {ignored} 次变化,已忽略 (ignored while unfocused)"),
            ));
        }

        // The clipboard as it stood before this session is nobody's business
        // and no evidence: the very first poll records the sequence number
        // without reading a byte of content.
        if self.last_sequence.is_none() {
            self.last_sequence = Some(sequence_number());
        }

        // The user's own copies first: a hand-pressed Ctrl+C is always
        // honored, click or no click.
        let sequence = sequence_number();
        if self.last_sequence != Some(sequence) {
            match read_text(CLIPBOARD_OPEN_ATTEMPTS) {
                Ok((text, _attempts)) => {
                    self.last_sequence = Some(sequence);
                    match self.ingest(text, started) {
                        Ingested::Evidence(result) | Ingested::Settled(result) => {
                            self.settle_click();
                            return Ok(result);
                        }
                        Ingested::SameText => {
                            // An old payload re-copied says nothing; fall
                            // through to the click logic below.
                        }
                    }
                }
                // The copier may still hold the clipboard open; the sequence
                // number is deliberately not recorded, so this change is
                // retried on the next poll instead of being swallowed.
                Err(ClipboardError::Busy { .. }) => {
                    return Ok(Self::unchanged(started.elapsed()));
                }
                Err(ClipboardError::NoTextFormat { .. } | ClipboardError::EmptyText) => {
                    self.last_sequence = Some(sequence);
                    return Ok(Self::noted(
                        started.elapsed(),
                        "剪贴板内容不是文本 (not text)".to_owned(),
                    ));
                }
                Err(error) => return Err(SourceError::Clipboard(error)),
            }
        }

        // The click-invoked copy. A new click restarts the delay window; a
        // burst faster than the delay coalesces into one copy after the last
        // click, which is the only roll that still exists anyway.
        let clicks = observed_clicks();
        if clicks != self.clicks_handled {
            self.clicks_handled = clicks;
            self.click_seen_at = Some(started);
            self.copies_this_click = 0;
            self.next_copy_at = Some(started + self.copy_delay);
        }

        // The start-invoked baseline: scheduled exactly once, immediately,
        // through the same bounded machinery a click uses. A click arriving
        // first simply takes the slot — its copy doubles as the baseline.
        if self.baseline == Baseline::Pending && self.next_copy_at.is_none() {
            self.baseline = Baseline::Running;
            self.copies_this_click = 0;
            self.next_copy_at = Some(started);
        }

        let Some(due) = self.next_copy_at else {
            return Ok(Self::unchanged(started.elapsed()));
        };
        if started < due {
            return Ok(Self::unchanged(started.elapsed()));
        }

        self.copies_this_click += 1;
        let outcome = match copy_hovered_item(COPY_DEADLINE, self.key_method) {
            Ok(outcome) => outcome,
            Err(error) if error.is_transient() => {
                // The client goes quiet while a craft is in flight. Retry on
                // the re-copy schedule until the click's budget is spent.
                if self.copies_this_click >= MAX_COPIES_PER_CLICK {
                    let baselining = self.baseline == Baseline::Running;
                    self.settle_click();
                    let note = if baselining {
                        "未能建立基准(开始监控时请悬停在物品上);首次点击的复制将作为基准 \
                         (no baseline; the first click's copy will become it)"
                    } else {
                        "客户端未应答,本轮放弃 (client did not answer; giving up this click)"
                    };
                    return Ok(Self::noted(started.elapsed(), note.to_owned()));
                }
                self.next_copy_at = Some(started + RECOPY_GAP);
                return Ok(Self::unchanged(started.elapsed()));
            }
            Err(error) => return Err(SourceError::Clipboard(error)),
        };
        self.last_sequence = Some(sequence_number());

        match self.ingest(outcome.text, started) {
            Ingested::Evidence(result) | Ingested::Settled(result) => {
                self.settle_click();
                Ok(result)
            }
            Ingested::SameText => {
                // The copy answered with the pre-click text: the roll has not
                // arrived yet. Re-copy on a spaced schedule until the budget
                // for this click is spent.
                if self.copies_this_click >= MAX_COPIES_PER_CLICK {
                    self.settle_click();
                    return Ok(Self::noted(
                        started.elapsed(),
                        format!(
                            "复制 {MAX_COPIES_PER_CLICK} 次仍是旧词缀,本轮放弃 \
                             (still the old text after {MAX_COPIES_PER_CLICK} copies; \
                             raise {COPY_DELAY_OVERRIDE})"
                        ),
                    ));
                }
                self.next_copy_at = Some(started + RECOPY_GAP);
                Ok(Self::unchanged(started.elapsed()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poe_alarm_platform_win::ClipboardError;
    use std::time::Duration;

    #[test]
    fn outranked_is_the_only_actionable_failure() {
        // Elevation is the one thing the user can fix, and the UI's offer to
        // relaunch elevated is exactly that fix.
        assert!(SourceError::Outranked.is_actionable());
        assert!(
            !SourceError::Clipboard(ClipboardError::Timeout {
                waited: Duration::from_millis(12)
            })
            .is_actionable()
        );
        assert!(!SourceError::Other("anything".to_owned()).is_actionable());
    }

    #[test]
    fn the_copy_delay_override_parses_and_bounds() {
        assert_eq!(
            copy_delay_override(Some(std::ffi::OsStr::new("120"))),
            Some(Duration::from_millis(120))
        );
        assert_eq!(copy_delay_override(Some(std::ffi::OsStr::new("0"))), None);
        assert_eq!(
            copy_delay_override(Some(std::ffi::OsStr::new("1001"))),
            None
        );
        assert_eq!(
            copy_delay_override(Some(std::ffi::OsStr::new("fast"))),
            None
        );
        assert_eq!(copy_delay_override(None), None);
    }

    /// The injection budget of one click is bounded by construction; this
    /// pins the constant so a future edit cannot silently unbound it.
    #[test]
    fn one_click_costs_at_most_three_chords() {
        assert_eq!(MAX_COPIES_PER_CLICK, 3);
    }
}
