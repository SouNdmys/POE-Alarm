use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::PlatformError;

const PHYSICAL_SHIFT: u32 = 0;
const SUPPRESSED_SHIFT: u32 = 8;
const MODE_SHIFT: u32 = 16;
const BUTTON_MASK: u32 = MouseButtons::ALL.bits() as u32;
const MODE_MASK: u32 = 0x3;

/// Mouse button represented by the stable pending-input guard.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

impl MouseButton {
    const fn bit(self) -> u8 {
        match self {
            Self::Left => MouseButtons::LEFT.bits(),
            Self::Right => MouseButtons::RIGHT.bits(),
            Self::Middle => MouseButtons::MIDDLE.bits(),
            Self::X1 => MouseButtons::X1.bits(),
            Self::X2 => MouseButtons::X2.bits(),
        }
    }
}

/// Compact physical/suppressed button mask.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct MouseButtons(u8);

impl MouseButtons {
    pub const NONE: Self = Self(0);
    pub const LEFT: Self = Self(1 << 0);
    pub const RIGHT: Self = Self(1 << 1);
    pub const MIDDLE: Self = Self(1 << 2);
    pub const X1: Self = Self(1 << 3);
    pub const X2: Self = Self(1 << 4);
    pub const ALL: Self =
        Self(Self::LEFT.0 | Self::RIGHT.0 | Self::MIDDLE.0 | Self::X1.0 | Self::X2.0);

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, button: MouseButton) -> bool {
        self.0 & button.bit() != 0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self((self.0 | other.0) & Self::ALL.0)
    }
}

impl std::fmt::Debug for MouseButtons {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut set = formatter.debug_set();
        for button in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::X1,
            MouseButton::X2,
        ] {
            if self.contains(button) {
                set.entry(&button);
            }
        }
        set.finish()
    }
}

impl std::ops::BitOr for MouseButtons {
    type Output = Self;

    fn bitor(self, right: Self) -> Self::Output {
        self.union(right)
    }
}

/// Input categories decoded by the minimal low-level hook callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseInput {
    ButtonDown(MouseButton),
    ButtonUp(MouseButton),
    VerticalWheel,
    HorizontalWheel,
}

/// Stable pending-guard mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum GuardMode {
    #[default]
    Released = 0,
    WaitingForExistingRelease = 1,
    Guarding = 2,
    Draining = 3,
}

/// Atomic snapshot used for diagnostics and deterministic assertions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuardSnapshot {
    pub mode: GuardMode,
    pub physical_buttons: MouseButtons,
    pub suppressed_buttons: MouseButtons,
}

/// Per-message decision made without allocating or taking a lock.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GuardDecision {
    pub suppress: bool,
    pub became_released: bool,
}

/// Result of stopping acceptance of new clicks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardRelease {
    Released,
    Draining,
}

/// Lock-free packed state used directly by `WH_MOUSE_LL`.
///
/// The machine never injects, queues, or replays input. A button-down consumed
/// while guarding records its bit so the matching button-up is consumed even
/// after `release` switches the machine to draining.
pub struct MouseGuardStateMachine {
    packed: AtomicU32,
}

impl std::fmt::Debug for MouseGuardStateMachine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MouseGuardStateMachine")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl Default for MouseGuardStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl MouseGuardStateMachine {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            packed: AtomicU32::new(0),
        }
    }

    pub fn initialize_released(&self, physical_buttons: MouseButtons) {
        self.packed.store(
            pack(physical_buttons.bits(), 0, GuardMode::Released),
            Ordering::Release,
        );
    }

    #[must_use]
    pub fn snapshot(&self) -> GuardSnapshot {
        decode(self.packed.load(Ordering::Acquire))
    }

    pub fn arm(&self) {
        self.update(|mut state| {
            if matches!(
                state.mode,
                GuardMode::WaitingForExistingRelease | GuardMode::Guarding
            ) {
                return state;
            }
            let unsuppressed_physical =
                state.physical_buttons.bits() & !state.suppressed_buttons.bits();
            state.mode = if unsuppressed_physical != 0 {
                GuardMode::WaitingForExistingRelease
            } else {
                GuardMode::Guarding
            };
            state
        });
    }

    /// Stops accepting new pairs but retains any already-suppressed releases.
    pub fn release(&self) -> GuardRelease {
        let state = self.update(|mut state| {
            state.mode = if state.suppressed_buttons == MouseButtons::NONE {
                GuardMode::Released
            } else {
                GuardMode::Draining
            };
            state
        });
        match state.mode {
            GuardMode::Released => GuardRelease::Released,
            GuardMode::Draining => GuardRelease::Draining,
            _ => unreachable!("release always chooses released or draining"),
        }
    }

    /// Independent fail-open transition. No synthetic release is generated.
    pub fn force_release(&self) {
        self.update(|mut state| {
            state.mode = GuardMode::Released;
            state.suppressed_buttons = MouseButtons::NONE;
            state
        });
    }

    #[must_use]
    pub fn process(&self, input: MouseInput) -> GuardDecision {
        match input {
            MouseInput::ButtonDown(button) => self.process_button(button, true),
            MouseInput::ButtonUp(button) => self.process_button(button, false),
            MouseInput::VerticalWheel | MouseInput::HorizontalWheel => GuardDecision {
                suppress: self.snapshot().mode == GuardMode::Guarding,
                became_released: false,
            },
        }
    }

    fn process_button(&self, button: MouseButton, is_down: bool) -> GuardDecision {
        let bit = button.bit();
        let mut decision = GuardDecision::default();
        self.update(|mut state| {
            let mut physical = state.physical_buttons.bits();
            let mut suppressed = state.suppressed_buttons.bits();
            decision = GuardDecision::default();
            if is_down {
                physical |= bit;
                if state.mode == GuardMode::Guarding || suppressed & bit != 0 {
                    suppressed |= bit;
                    decision.suppress = true;
                }
            } else {
                physical &= !bit;
                if suppressed & bit != 0 {
                    suppressed &= !bit;
                    decision.suppress = true;
                }
                if state.mode == GuardMode::WaitingForExistingRelease && physical & !suppressed == 0
                {
                    state.mode = GuardMode::Guarding;
                } else if state.mode == GuardMode::Draining && suppressed == 0 {
                    state.mode = GuardMode::Released;
                    decision.became_released = true;
                }
            }
            state.physical_buttons = MouseButtons(physical & MouseButtons::ALL.bits());
            state.suppressed_buttons = MouseButtons(suppressed & MouseButtons::ALL.bits());
            state
        });
        decision
    }

    fn update(&self, mut transition: impl FnMut(GuardSnapshot) -> GuardSnapshot) -> GuardSnapshot {
        let mut current = self.packed.load(Ordering::Acquire);
        loop {
            let next_state = transition(decode(current));
            let next = pack(
                next_state.physical_buttons.bits(),
                next_state.suppressed_buttons.bits(),
                next_state.mode,
            );
            match self.packed.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return next_state,
                Err(observed) => current = observed,
            }
        }
    }
}

fn pack(physical: u8, suppressed: u8, mode: GuardMode) -> u32 {
    ((u32::from(physical) & BUTTON_MASK) << PHYSICAL_SHIFT)
        | ((u32::from(suppressed) & BUTTON_MASK) << SUPPRESSED_SHIFT)
        | (((mode as u32) & MODE_MASK) << MODE_SHIFT)
}

fn decode(packed: u32) -> GuardSnapshot {
    let physical = ((packed >> PHYSICAL_SHIFT) & BUTTON_MASK) as u8;
    let suppressed = ((packed >> SUPPRESSED_SHIFT) & BUTTON_MASK) as u8;
    let mode = match (packed >> MODE_SHIFT) & MODE_MASK {
        0 => GuardMode::Released,
        1 => GuardMode::WaitingForExistingRelease,
        2 => GuardMode::Guarding,
        3 => GuardMode::Draining,
        _ => unreachable!("mode is masked to two bits"),
    };
    GuardSnapshot {
        mode,
        physical_buttons: MouseButtons(physical),
        suppressed_buttons: MouseButtons(suppressed),
    }
}

/// Native pass-through hook owner and 750 ms fail-open lifecycle.
#[derive(Debug)]
pub struct PendingMouseInputGuard {
    #[cfg(windows)]
    native: crate::win32::NativePendingMouseGuard,
    #[cfg(not(windows))]
    native: crate::non_windows::NativePendingMouseGuard,
}

impl Default for PendingMouseInputGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingMouseInputGuard {
    /// Maximum active OCR-pending interval before the guard stops accepting
    /// new input. A consumed button pair may then drain for one further bound.
    pub const FAIL_OPEN_TIMEOUT: Duration = Duration::from_millis(750);

    #[must_use]
    pub fn new() -> Self {
        Self {
            native: platform_new_guard(),
        }
    }

    /// Pre-installs the pass-through hook and initializes physical button state.
    pub fn prepare(&mut self) -> Result<(), PlatformError> {
        self.native.prepare()
    }

    /// Arms synchronously from the continuously tracked physical state.
    pub fn arm(&mut self) -> Result<(), PlatformError> {
        self.native.arm()
    }

    /// Stops new suppression, drains a consumed pair, and fails open at 750 ms.
    pub fn release(&mut self) {
        self.native.release();
    }

    /// Transfers safety to an already-visible blocking overlay and unhooks now.
    pub fn transfer_to_blocking_overlay(&mut self) {
        self.native.force_release_and_stop();
    }

    #[must_use]
    pub fn is_installed(&self) -> bool {
        self.native.is_installed()
    }

    #[must_use]
    pub fn snapshot(&self) -> GuardSnapshot {
        self.native.snapshot()
    }
}

#[cfg(windows)]
fn platform_new_guard() -> crate::win32::NativePendingMouseGuard {
    crate::win32::NativePendingMouseGuard::new()
}

#[cfg(not(windows))]
fn platform_new_guard() -> crate::non_windows::NativePendingMouseGuard {
    crate::non_windows::NativePendingMouseGuard::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_button_is_preserved_before_guard_takes_over() {
        let machine = MouseGuardStateMachine::new();
        machine.initialize_released(MouseButtons::LEFT);
        machine.arm();
        assert_eq!(
            machine.snapshot().mode,
            GuardMode::WaitingForExistingRelease
        );
        assert_eq!(
            machine.process(MouseInput::ButtonUp(MouseButton::Left)),
            GuardDecision {
                suppress: false,
                became_released: false,
            }
        );
        assert_eq!(machine.snapshot().mode, GuardMode::Guarding);
    }

    #[test]
    fn every_new_button_pair_is_suppressed_and_drained() {
        for button in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::X1,
            MouseButton::X2,
        ] {
            let machine = MouseGuardStateMachine::new();
            machine.arm();
            assert!(machine.process(MouseInput::ButtonDown(button)).suppress);
            assert_eq!(machine.release(), GuardRelease::Draining);
            assert_eq!(machine.snapshot().mode, GuardMode::Draining);
            assert_eq!(
                machine.process(MouseInput::ButtonUp(button)),
                GuardDecision {
                    suppress: true,
                    became_released: true,
                }
            );
            assert_eq!(machine.snapshot().mode, GuardMode::Released);
        }
    }

    #[test]
    fn simultaneous_buttons_keep_independent_pairing() {
        let machine = MouseGuardStateMachine::new();
        machine.arm();
        let _ = machine.process(MouseInput::ButtonDown(MouseButton::Left));
        let _ = machine.process(MouseInput::ButtonDown(MouseButton::Right));
        assert_eq!(machine.release(), GuardRelease::Draining);
        assert!(
            machine
                .process(MouseInput::ButtonUp(MouseButton::Left))
                .suppress
        );
        assert_eq!(machine.snapshot().mode, GuardMode::Draining);
        assert_eq!(
            machine.process(MouseInput::ButtonUp(MouseButton::Right)),
            GuardDecision {
                suppress: true,
                became_released: true,
            }
        );
    }

    #[test]
    fn wheel_is_only_consumed_while_accepting_new_input() {
        let machine = MouseGuardStateMachine::new();
        assert!(!machine.process(MouseInput::VerticalWheel).suppress);
        machine.arm();
        assert!(machine.process(MouseInput::VerticalWheel).suppress);
        assert!(machine.process(MouseInput::HorizontalWheel).suppress);
        let _ = machine.process(MouseInput::ButtonDown(MouseButton::Left));
        machine.release();
        assert!(!machine.process(MouseInput::VerticalWheel).suppress);
    }

    #[test]
    fn force_release_drops_bookkeeping_without_replay() {
        let machine = MouseGuardStateMachine::new();
        machine.arm();
        let _ = machine.process(MouseInput::ButtonDown(MouseButton::Middle));
        machine.release();
        machine.force_release();
        assert_eq!(
            machine.snapshot(),
            GuardSnapshot {
                mode: GuardMode::Released,
                physical_buttons: MouseButtons::MIDDLE,
                suppressed_buttons: MouseButtons::NONE,
            }
        );
        assert!(
            !machine
                .process(MouseInput::ButtonUp(MouseButton::Middle))
                .suppress
        );
    }

    #[test]
    fn repeated_down_does_not_lose_matching_release() {
        let machine = MouseGuardStateMachine::new();
        machine.arm();
        assert!(
            machine
                .process(MouseInput::ButtonDown(MouseButton::Left))
                .suppress
        );
        assert!(
            machine
                .process(MouseInput::ButtonDown(MouseButton::Left))
                .suppress
        );
        machine.release();
        assert_eq!(
            machine.process(MouseInput::ButtonUp(MouseButton::Left)),
            GuardDecision {
                suppress: true,
                became_released: true,
            }
        );
    }

    #[test]
    fn fail_open_contract_is_seven_hundred_fifty_milliseconds() {
        assert_eq!(
            PendingMouseInputGuard::FAIL_OPEN_TIMEOUT,
            Duration::from_millis(750)
        );
    }
}
