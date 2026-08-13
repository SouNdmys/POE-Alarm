use std::time::Duration;

use poe_alarm_platform_win::MouseButtons;

use crate::AlertId;

pub(crate) const ACKNOWLEDGEMENT_MINIMUM: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LifecyclePhase {
    Idle,
    Presenting,
    Blocking,
    AcknowledgementDrain,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TriggerDecision {
    Accepted,
    Duplicate(AlertId),
    Stopped,
}

/// Single-thread protocol that makes the safety ordering explicit and testable.
#[derive(Debug)]
pub(crate) struct AlertLifecycle {
    phase: LifecyclePhase,
    active_id: Option<AlertId>,
    presentation_verified: bool,
    guard_transferred: bool,
}

impl Default for AlertLifecycle {
    fn default() -> Self {
        Self {
            phase: LifecyclePhase::Idle,
            active_id: None,
            presentation_verified: false,
            guard_transferred: false,
        }
    }
}

impl AlertLifecycle {
    pub(crate) fn begin_trigger(&mut self, alert_id: AlertId) -> TriggerDecision {
        match self.phase {
            LifecyclePhase::Idle => {
                self.phase = LifecyclePhase::Presenting;
                self.active_id = Some(alert_id);
                self.presentation_verified = false;
                self.guard_transferred = false;
                TriggerDecision::Accepted
            }
            LifecyclePhase::Stopped => TriggerDecision::Stopped,
            _ => TriggerDecision::Duplicate(self.active_id.expect("active phase has an id")),
        }
    }

    pub(crate) fn mark_presentation_verified(&mut self, alert_id: AlertId) -> bool {
        if self.phase != LifecyclePhase::Presenting || self.active_id != Some(alert_id) {
            return false;
        }
        self.presentation_verified = true;
        true
    }

    pub(crate) fn mark_guard_transferred(&mut self, alert_id: AlertId) -> bool {
        if !self.presentation_verified
            || self.phase != LifecyclePhase::Presenting
            || self.active_id != Some(alert_id)
        {
            return false;
        }
        self.guard_transferred = true;
        self.phase = LifecyclePhase::Blocking;
        true
    }

    pub(crate) fn begin_acknowledgement(&mut self, alert_id: AlertId) -> bool {
        if self.phase != LifecyclePhase::Blocking || self.active_id != Some(alert_id) {
            return false;
        }
        self.phase = LifecyclePhase::AcknowledgementDrain;
        true
    }

    pub(crate) fn acknowledgement_ready(
        &self,
        alert_id: AlertId,
        elapsed: Duration,
        physical_buttons: MouseButtons,
    ) -> bool {
        self.phase == LifecyclePhase::AcknowledgementDrain
            && self.active_id == Some(alert_id)
            && elapsed >= ACKNOWLEDGEMENT_MINIMUM
            && physical_buttons == MouseButtons::NONE
    }

    pub(crate) fn finish_acknowledgement(&mut self, alert_id: AlertId) -> bool {
        if self.phase != LifecyclePhase::AcknowledgementDrain || self.active_id != Some(alert_id) {
            return false;
        }
        self.reset_idle();
        true
    }

    pub(crate) fn fail(&mut self, alert_id: AlertId) -> bool {
        if self.active_id != Some(alert_id) || self.phase == LifecyclePhase::Stopped {
            return false;
        }
        self.reset_idle();
        true
    }

    pub(crate) fn stop(&mut self) {
        self.phase = LifecyclePhase::Stopped;
        self.active_id = None;
        self.presentation_verified = false;
        self.guard_transferred = false;
    }

    #[cfg(test)]
    pub(crate) const fn phase(&self) -> LifecyclePhase {
        self.phase
    }

    fn reset_idle(&mut self) {
        self.phase = LifecyclePhase::Idle;
        self.active_id = None;
        self.presentation_verified = false;
        self.guard_transferred = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_cannot_skip_visible_hit_test_verification() {
        let id = AlertId(7);
        let mut lifecycle = AlertLifecycle::default();
        assert_eq!(lifecycle.begin_trigger(id), TriggerDecision::Accepted);
        assert!(!lifecycle.mark_guard_transferred(id));
        assert!(lifecycle.mark_presentation_verified(id));
        assert!(lifecycle.mark_guard_transferred(id));
        assert_eq!(lifecycle.phase(), LifecyclePhase::Blocking);
    }

    #[test]
    fn acknowledgement_waits_300_ms_and_every_mouse_button() {
        let id = AlertId(9);
        let mut lifecycle = AlertLifecycle::default();
        lifecycle.begin_trigger(id);
        lifecycle.mark_presentation_verified(id);
        lifecycle.mark_guard_transferred(id);
        assert!(lifecycle.begin_acknowledgement(id));
        assert!(!lifecycle.acknowledgement_ready(
            id,
            Duration::from_millis(299),
            MouseButtons::NONE
        ));
        for button in [
            MouseButtons::LEFT,
            MouseButtons::RIGHT,
            MouseButtons::MIDDLE,
            MouseButtons::X1,
            MouseButtons::X2,
        ] {
            assert!(!lifecycle.acknowledgement_ready(id, Duration::from_secs(1), button));
        }
        assert!(lifecycle.acknowledgement_ready(
            id,
            Duration::from_millis(300),
            MouseButtons::NONE
        ));
        assert!(lifecycle.finish_acknowledgement(id));
        assert_eq!(lifecycle.phase(), LifecyclePhase::Idle);
    }

    #[test]
    fn failure_releases_latch_for_a_later_alert() {
        let mut lifecycle = AlertLifecycle::default();
        assert_eq!(
            lifecycle.begin_trigger(AlertId(1)),
            TriggerDecision::Accepted
        );
        assert!(lifecycle.fail(AlertId(1)));
        assert_eq!(
            lifecycle.begin_trigger(AlertId(2)),
            TriggerDecision::Accepted
        );
    }

    #[test]
    fn repeated_trigger_is_latched_to_original_id() {
        let mut lifecycle = AlertLifecycle::default();
        assert_eq!(
            lifecycle.begin_trigger(AlertId(21)),
            TriggerDecision::Accepted
        );
        assert_eq!(
            lifecycle.begin_trigger(AlertId(22)),
            TriggerDecision::Duplicate(AlertId(21))
        );
    }
}
