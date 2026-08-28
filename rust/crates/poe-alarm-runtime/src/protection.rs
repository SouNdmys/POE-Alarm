use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use poe_alarm_alert_win::{
    AlertEvent, AlertId, AlertServiceConfig, AlertText, AlertTrigger, AlertTriggerStatus,
    BlockingAlertService,
};
use poe_alarm_platform_win::{PendingMouseInputGuard, RectI};

use crate::{AlertCopy, RuntimeGeneration};

const ALERT_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(750);
const ALERT_SHUTDOWN_POLL: Duration = Duration::from_millis(2);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlertPresentation {
    pub copy: AlertCopy,
    pub detail: String,
    /// Which monitor should host the alert, expressed as the game window.
    ///
    /// `None` means the game window could not be found, and the alert lands on
    /// the primary monitor — the same place it lands for anyone with one screen.
    pub anchor_region: Option<RectI>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlertLatchStatus {
    Accepted {
        /// `Some(reason)` when the click-block hook did not arm: the overlay
        /// still presents, but the next click is not swallowed. The reason is
        /// for the log; the alert swaps its notice line so the user can tell
        /// at a glance which kind of overshoot they are looking at.
        unguarded: Option<String>,
    },
    AlreadyLatched,
}

/// Environment variable that forces every latch onto the unguarded path.
///
/// The whole point of reporting a guard failure is that it is otherwise
/// indistinguishable from losing the timing race. This lever lets anyone see
/// the full reporting chain — log line and swapped alert notice — on a real
/// match, without having to genuinely break the hook. The reason string names
/// the variable so a forgotten setting cannot masquerade as a real failure.
pub const FORCE_UNGUARDED_OVERRIDE: &str = "POE_ALARM_FORCE_UNGUARDED";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtectionError(pub String);

impl fmt::Display for ProtectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ProtectionError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtectionEvent {
    Presented {
        generation: RuntimeGeneration,
    },
    Acknowledged {
        generation: RuntimeGeneration,
    },
    SoundFailed {
        generation: RuntimeGeneration,
        detail: String,
    },
    Failed {
        generation: Option<RuntimeGeneration>,
        detail: String,
    },
    Stopped,
}

/// Combined pending-input and red-alert boundary.
///
/// Combining these two operations is intentional: an implementation cannot
/// claim a detection without either atomically handing the pending guard to a
/// blocking alert or failing open and returning an error.
pub trait ProtectionService: Send + Sync + 'static {
    /// Returns whether no live generation or acknowledged-alert ownership is
    /// left behind. Runtime replacement work waits at this boundary instead of
    /// racing a previous red alert's acknowledgement drain.
    fn ready_for_new_work(&self) -> bool;

    /// Establishes the only generation allowed to latch a red alert. This is
    /// required even for profiles that do not use the short input guard.
    fn begin_session(&self, generation: RuntimeGeneration) -> Result<(), ProtectionError>;

    /// Installs the pass-through hook before monitoring starts so physical
    /// buttons already held down are observed before the first synchronous arm.
    fn prepare_pending(&self, generation: RuntimeGeneration) -> Result<(), ProtectionError>;

    fn arm_pending(&self, generation: RuntimeGeneration) -> Result<(), ProtectionError>;

    fn release_pending(&self, generation: RuntimeGeneration);

    /// Terminally unhooks pending-input tracking. Unlike `release_pending`,
    /// this is used only when a live session has ended.
    fn stop_pending(&self, generation: RuntimeGeneration);

    fn latch_red_alert(
        &self,
        generation: RuntimeGeneration,
        presentation: AlertPresentation,
    ) -> Result<AlertLatchStatus, ProtectionError>;

    fn acknowledge(&self) -> Result<(), ProtectionError>;

    fn poll_events(&self) -> Vec<ProtectionEvent>;

    /// Must synchronously fail open. Native alert-window teardown itself may
    /// continue on its dedicated message-loop thread.
    fn shutdown_fail_open(&self) -> Result<(), ProtectionError>;
}

pub struct NativeProtection {
    state: Mutex<NativeProtectionState>,
}

struct NativeProtectionState {
    guard: Option<GuardSlot>,
    bookkeeping: ProtectionBookkeeping,
    alert: BlockingAlertService,
}

#[derive(Default)]
struct ProtectionBookkeeping {
    active_generation: Option<RuntimeGeneration>,
    alert_generations: HashMap<AlertId, RuntimeGeneration>,
}

impl ProtectionBookkeeping {
    fn begin_session(&mut self, generation: RuntimeGeneration) -> Result<(), ProtectionError> {
        if self.active_generation.is_some() {
            return Err(ProtectionError(
                "a previous protection session is still active".to_owned(),
            ));
        }
        if !self.alert_generations.is_empty() {
            return Err(ProtectionError(
                "acknowledge the previous red alert before starting again".to_owned(),
            ));
        }
        self.active_generation = Some(generation);
        Ok(())
    }

    fn stop_session(&mut self, generation: RuntimeGeneration) {
        if self.active_generation == Some(generation) {
            self.active_generation = None;
        }
    }

    fn is_active(&self, generation: RuntimeGeneration) -> bool {
        self.active_generation == Some(generation)
    }

    fn insert_alert(&mut self, alert_id: AlertId, generation: RuntimeGeneration) {
        self.alert_generations.insert(alert_id, generation);
    }

    /// Translates native alert events without changing the live-session lease.
    /// Only `stop_session` may clear that lease; an empty UI event poll must be
    /// observationally inert.
    fn poll_alert_events(
        &mut self,
        mut next_event: impl FnMut() -> Option<AlertEvent>,
    ) -> Vec<ProtectionEvent> {
        let mut output = Vec::new();
        while let Some(event) = next_event() {
            let mapped = match event {
                AlertEvent::Presented { alert_id } => self
                    .alert_generations
                    .get(&alert_id)
                    .copied()
                    .map(|generation| ProtectionEvent::Presented { generation }),
                AlertEvent::Acknowledged { alert_id } => self
                    .alert_generations
                    .remove(&alert_id)
                    .map(|generation| ProtectionEvent::Acknowledged { generation }),
                AlertEvent::SoundFailed { alert_id, failure } => self
                    .alert_generations
                    .get(&alert_id)
                    .copied()
                    .map(|generation| ProtectionEvent::SoundFailed {
                        generation,
                        detail: failure.to_string(),
                    }),
                AlertEvent::Failed { alert_id, failure } => self
                    .alert_generations
                    .remove(&alert_id)
                    .map(|generation| ProtectionEvent::Failed {
                        generation: Some(generation),
                        detail: failure.to_string(),
                    }),
                AlertEvent::Stopped => Some(ProtectionEvent::Stopped),
            };
            if let Some(mapped) = mapped {
                output.push(mapped);
            }
        }
        output
    }
}

/// Arms a hook-level click block for the gap between a match and a shield.
///
/// A match with a slower lock is still a match, so the caller proceeds either
/// way — but a failure here means the alert will NOT stop the next click, and
/// for a long time that was invisible: the error was discarded, and the only
/// symptom was the user clicking past a winning roll with no way to tell a
/// dead hook from a lost timing race. The reason now travels with the status
/// so the log can say which one it was.
fn armed_guard_for_latch() -> Result<PendingMouseInputGuard, String> {
    if let Some(reason) =
        forced_unguarded_reason(std::env::var_os(FORCE_UNGUARDED_OVERRIDE).as_deref())
    {
        return Err(reason);
    }
    let mut guard = PendingMouseInputGuard::new();
    if let Err(error) = guard.prepare() {
        return Err(format!("the click-block hook could not prepare: {error}"));
    }
    if let Err(error) = guard.arm() {
        guard.release();
        return Err(format!("the click-block hook could not arm: {error}"));
    }
    Ok(guard)
}

/// The override decision, taken on the raw environment value so it is testable
/// without process-global environment mutation (this crate forbids the unsafe
/// `set_var`). Empty means unset: `POE_ALARM_FORCE_UNGUARDED=` clears it.
fn forced_unguarded_reason(value: Option<&std::ffi::OsStr>) -> Option<String> {
    value.filter(|value| !value.is_empty()).map(|_| {
        format!(
            "forced by {FORCE_UNGUARDED_OVERRIDE} (diagnostic override; unset it for real runs)"
        )
    })
}

struct GuardSlot {
    generation: RuntimeGeneration,
    guard: PendingMouseInputGuard,
}

impl NativeProtection {
    pub fn start(config: AlertServiceConfig) -> Result<Self, ProtectionError> {
        let alert = BlockingAlertService::start(config)
            .map_err(|error| ProtectionError(error.to_string()))?;
        Ok(Self {
            state: Mutex::new(NativeProtectionState {
                guard: None,
                bookkeeping: ProtectionBookkeeping::default(),
                alert,
            }),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, NativeProtectionState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ProtectionService for NativeProtection {
    fn ready_for_new_work(&self) -> bool {
        let state = self.state();
        state.bookkeeping.active_generation.is_none()
            && state.bookkeeping.alert_generations.is_empty()
    }

    fn begin_session(&self, generation: RuntimeGeneration) -> Result<(), ProtectionError> {
        self.state().bookkeeping.begin_session(generation)
    }

    fn prepare_pending(&self, generation: RuntimeGeneration) -> Result<(), ProtectionError> {
        let mut state = self.state();
        if state
            .guard
            .as_ref()
            .is_some_and(|slot| slot.generation != generation)
            && let Some(mut stale) = state.guard.take()
        {
            stale.guard.transfer_to_blocking_overlay();
        }
        if state.guard.is_none() {
            state.guard = Some(GuardSlot {
                generation,
                guard: PendingMouseInputGuard::new(),
            });
        }
        let result = state
            .guard
            .as_mut()
            .expect("guard was initialized")
            .guard
            .prepare()
            .map_err(|error| ProtectionError(error.to_string()));
        if result.is_err()
            && let Some(mut slot) = state.guard.take()
        {
            slot.guard.transfer_to_blocking_overlay();
        }
        result
    }

    fn arm_pending(&self, generation: RuntimeGeneration) -> Result<(), ProtectionError> {
        let mut state = self.state();
        let Some(slot) = state
            .guard
            .as_mut()
            .filter(|slot| slot.generation == generation)
        else {
            return Err(ProtectionError(
                "pending input guard was not prepared before monitoring".to_owned(),
            ));
        };
        slot.guard
            .arm()
            .map_err(|error| ProtectionError(error.to_string()))
    }

    fn release_pending(&self, generation: RuntimeGeneration) {
        if let Some(slot) = self
            .state()
            .guard
            .as_mut()
            .filter(|slot| slot.generation == generation)
        {
            slot.guard.release();
        }
    }

    fn stop_pending(&self, generation: RuntimeGeneration) {
        let mut state = self.state();
        state.bookkeeping.stop_session(generation);
        if state
            .guard
            .as_ref()
            .is_some_and(|slot| slot.generation == generation)
            && let Some(mut slot) = state.guard.take()
        {
            slot.guard.transfer_to_blocking_overlay();
        }
    }

    fn latch_red_alert(
        &self,
        generation: RuntimeGeneration,
        presentation: AlertPresentation,
    ) -> Result<AlertLatchStatus, ProtectionError> {
        let mut state = self.state();
        if !state.bookkeeping.is_active(generation) {
            return Err(ProtectionError(
                "the detection belongs to a stopped runtime session".to_owned(),
            ));
        }
        // The guard arms before the text is even validated: the hook arms in
        // microseconds and every step in front of it is part of the race the
        // user's next click is running.
        let (pending_guard, unguarded) = if state
            .guard
            .as_ref()
            .is_some_and(|slot| slot.generation == generation)
        {
            (state.guard.take().map(|slot| slot.guard), None)
        } else {
            // Fast mode arms one right here, at the instant of the match. The
            // shield window takes tens of milliseconds to present and verify,
            // and a crafting macro clicks faster than that: the winning roll
            // was being clicked past while the window was still on its way up.
            // The hook arms in microseconds on this thread, so the very next
            // click is swallowed however soon it comes. The alert service owns
            // the rest of the lifecycle it was already built for: transfer to
            // the overlay once it is verified, or fail open on a bounded
            // timeout so a stuck presentation can never wedge the mouse.
            match armed_guard_for_latch() {
                Ok(guard) => (Some(guard), None),
                Err(reason) => (None, Some(reason)),
            }
        };
        let notice = if unguarded.is_some() {
            presentation.copy.notice_unguarded
        } else {
            presentation.copy.notice
        };
        let text = match AlertText::new(
            presentation.copy.title,
            presentation.detail,
            presentation.copy.button,
            notice,
            presentation.copy.footer,
        ) {
            Ok(text) => text,
            Err(error) => {
                // A guard with no alert to hand it to must not stay armed.
                if let Some(mut guard) = pending_guard {
                    guard.release();
                }
                return Err(ProtectionError(error.to_string()));
            }
        };
        let mut trigger = AlertTrigger::new(text);
        trigger.anchor_region = presentation.anchor_region;
        match state.alert.trigger(trigger, pending_guard) {
            Ok(AlertTriggerStatus::Accepted(alert_id)) => {
                state.bookkeeping.insert_alert(alert_id, generation);
                Ok(AlertLatchStatus::Accepted { unguarded })
            }
            Ok(AlertTriggerStatus::AlreadyLatched(_)) => Ok(AlertLatchStatus::AlreadyLatched),
            Err(error) => Err(ProtectionError(error.to_string())),
        }
    }

    fn acknowledge(&self) -> Result<(), ProtectionError> {
        self.state()
            .alert
            .acknowledge()
            .map_err(|error| ProtectionError(error.to_string()))
    }

    fn poll_events(&self) -> Vec<ProtectionEvent> {
        let mut state = self.state();
        let NativeProtectionState {
            bookkeeping, alert, ..
        } = &mut *state;
        bookkeeping.poll_alert_events(|| alert.try_next_event())
    }

    fn shutdown_fail_open(&self) -> Result<(), ProtectionError> {
        let mut state = self.state();
        if let Some(mut slot) = state.guard.take() {
            slot.guard.transfer_to_blocking_overlay();
        }
        state
            .alert
            .stop()
            .map_err(|error| ProtectionError(error.to_string()))?;
        let deadline = Instant::now() + ALERT_SHUTDOWN_TIMEOUT;
        loop {
            if let Some(AlertEvent::Stopped) = state.alert.try_next_event() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(ProtectionError(
                    "red alert worker did not stop within 750 ms".to_owned(),
                ));
            }
            thread::sleep(ALERT_SHUTDOWN_POLL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_alert_polls_preserve_the_live_generation_lease() {
        let generation = RuntimeGeneration(17);
        let mut bookkeeping = ProtectionBookkeeping::default();
        bookkeeping.begin_session(generation).unwrap();

        for _ in 0..8 {
            assert!(bookkeeping.poll_alert_events(|| None).is_empty());
            assert!(bookkeeping.is_active(generation));
        }

        bookkeeping.stop_session(generation);
        assert!(!bookkeeping.is_active(generation));
    }

    /// The override sits in front of any native hook work, and its reason
    /// must name the variable — a forgotten setting has to read as what it is
    /// in the user's log instead of masquerading as a real hook failure.
    #[test]
    fn the_forced_unguarded_override_names_itself_and_ignores_empty_values() {
        let reason = forced_unguarded_reason(Some(std::ffi::OsStr::new("1")))
            .expect("a set override must force the failure path");
        assert!(reason.contains(FORCE_UNGUARDED_OVERRIDE), "{reason}");
        assert_eq!(
            forced_unguarded_reason(Some(std::ffi::OsStr::new(""))),
            None
        );
        assert_eq!(forced_unguarded_reason(None), None);
    }

    /// Manual production-boundary smoke test. It intentionally owns the real
    /// process-wide alert worker and can briefly present the native red shield,
    /// so the normal unit suite exercises the pure bookkeeping regression above.
    #[cfg(windows)]
    #[test]
    #[ignore = "opens the native blocking alert window"]
    fn native_empty_polls_still_allow_one_alert_to_be_accepted() {
        let protection = NativeProtection::start(AlertServiceConfig::new(test_wave())).unwrap();
        let generation = RuntimeGeneration(23);
        protection.begin_session(generation).unwrap();
        for _ in 0..8 {
            assert!(protection.poll_events().is_empty());
        }

        let status = protection
            .latch_red_alert(
                generation,
                AlertPresentation {
                    copy: AlertCopy {
                        title: "Target found".to_owned(),
                        button: "Acknowledge".to_owned(),
                        notice: "Further mouse clicks are blocked.".to_owned(),
                        notice_unguarded: "This alert did NOT block clicks.".to_owned(),
                        footer: "Or Ctrl F12".to_owned(),
                    },
                    detail: "Native protection smoke test".to_owned(),
                    anchor_region: RectI::new(0, 0, 64, 64),
                },
            )
            .unwrap();
        assert_eq!(status, AlertLatchStatus::Accepted { unguarded: None });
        protection.stop_pending(generation);
        let _ = protection.acknowledge();
        protection.shutdown_fail_open().unwrap();
    }

    #[cfg(windows)]
    fn test_wave() -> poe_alarm_platform_win::ValidatedWave {
        let channels = 1u16;
        let sample_rate = 8_000u32;
        let bits = 8u16;
        let samples = 80usize;
        let block_align = channels * bits / 8;
        let data_size = samples * usize::from(block_align);
        let mut wave = Vec::with_capacity(44 + data_size);
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&(36 + data_size as u32).to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&16u32.to_le_bytes());
        wave.extend_from_slice(&1u16.to_le_bytes());
        wave.extend_from_slice(&channels.to_le_bytes());
        wave.extend_from_slice(&sample_rate.to_le_bytes());
        wave.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        wave.extend_from_slice(&block_align.to_le_bytes());
        wave.extend_from_slice(&bits.to_le_bytes());
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&(data_size as u32).to_le_bytes());
        wave.resize(44 + data_size, 128);
        poe_alarm_platform_win::ValidatedWave::from_bytes("runtime-alert-test.wav", wave).unwrap()
    }
}
