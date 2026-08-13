use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, TryLockError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use poe_alarm_platform_win::PendingMouseInputGuard;

use crate::{
    AlertEvent, AlertFailure, AlertFailureKind, AlertId, AlertServiceConfig, AlertTrigger,
    AlertTriggerError, AlertTriggerStatus,
};

const COMMAND_QUEUE_CAPACITY: usize = 4;
const STARTUP_TIMEOUT: Duration = Duration::from_millis(750);
const RELEASE_DRAIN_LIFETIME: Duration = Duration::from_millis(800);

static SERVICE_OWNED: AtomicBool = AtomicBool::new(false);

pub(crate) struct ServiceOwnership;

impl ServiceOwnership {
    fn acquire() -> Result<Self, AlertFailure> {
        SERVICE_OWNED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| {
                AlertFailure::new(
                    AlertFailureKind::AlreadyInUse,
                    "a native blocking alert service already owns this process",
                )
            })
    }
}

impl Drop for ServiceOwnership {
    fn drop(&mut self) {
        SERVICE_OWNED.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimePhase {
    Idle,
    Presenting,
    Blocking,
    AcknowledgementDrain,
    Stopping,
    Stopped,
}

#[derive(Debug)]
struct RuntimeInner {
    phase: RuntimePhase,
    active_id: Option<AlertId>,
    pending_handoff: Option<Arc<GuardHandoff>>,
}

#[derive(Debug)]
pub(crate) struct RuntimeShared {
    inner: Mutex<RuntimeInner>,
    stop_requested: AtomicBool,
    native_thread_id: AtomicU64,
    presentation_timeout: Duration,
}

impl RuntimeShared {
    fn new(presentation_timeout: Duration) -> Self {
        Self {
            inner: Mutex::new(RuntimeInner {
                phase: RuntimePhase::Idle,
                active_id: None,
                pending_handoff: None,
            }),
            stop_requested: AtomicBool::new(false),
            native_thread_id: AtomicU64::new(0),
            presentation_timeout,
        }
    }

    fn try_reserve(
        &self,
        alert_id: AlertId,
        handoff: Arc<GuardHandoff>,
    ) -> Result<AlertTriggerStatus, AlertTriggerError> {
        if self.stop_requested.load(Ordering::Acquire) {
            return Err(AlertTriggerError::Stopping);
        }
        let mut inner = match self.inner.try_lock() {
            Ok(inner) => inner,
            Err(TryLockError::WouldBlock) => return Err(AlertTriggerError::CommandQueueBusy),
            Err(TryLockError::Poisoned(_)) => return Err(AlertTriggerError::WorkerUnavailable),
        };
        match inner.phase {
            RuntimePhase::Idle => {
                inner.phase = RuntimePhase::Presenting;
                inner.active_id = Some(alert_id);
                inner.pending_handoff = Some(handoff);
                Ok(AlertTriggerStatus::Accepted(alert_id))
            }
            RuntimePhase::Presenting
            | RuntimePhase::Blocking
            | RuntimePhase::AcknowledgementDrain => Ok(AlertTriggerStatus::AlreadyLatched(
                inner.active_id.expect("latched runtime has an active id"),
            )),
            RuntimePhase::Stopping | RuntimePhase::Stopped => Err(AlertTriggerError::Stopping),
        }
    }

    pub(crate) fn is_current(&self, phase: RuntimePhase, alert_id: AlertId) -> bool {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        !self.stop_requested.load(Ordering::Acquire)
            && inner.phase == phase
            && inner.active_id == Some(alert_id)
    }

    pub(crate) fn mark_blocking(&self, alert_id: AlertId, handoff: &Arc<GuardHandoff>) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if self.stop_requested.load(Ordering::Acquire)
            || inner.phase != RuntimePhase::Presenting
            || inner.active_id != Some(alert_id)
        {
            return false;
        }
        inner.phase = RuntimePhase::Blocking;
        if inner
            .pending_handoff
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(pending, handoff))
        {
            inner.pending_handoff = None;
        }
        true
    }

    pub(crate) fn begin_acknowledgement(&self, alert_id: AlertId) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if inner.phase != RuntimePhase::Blocking || inner.active_id != Some(alert_id) {
            return false;
        }
        inner.phase = RuntimePhase::AcknowledgementDrain;
        true
    }

    pub(crate) fn finish_acknowledgement(&self, alert_id: AlertId) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if inner.phase != RuntimePhase::AcknowledgementDrain || inner.active_id != Some(alert_id) {
            return false;
        }
        inner.phase = RuntimePhase::Idle;
        inner.active_id = None;
        inner.pending_handoff = None;
        true
    }

    pub(crate) fn fail_pending(&self, alert_id: AlertId, handoff: &Arc<GuardHandoff>) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if inner.phase != RuntimePhase::Presenting
            || inner.active_id != Some(alert_id)
            || !inner
                .pending_handoff
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(pending, handoff))
        {
            return false;
        }
        inner.phase = RuntimePhase::Idle;
        inner.active_id = None;
        inner.pending_handoff = None;
        true
    }

    pub(crate) fn request_stop(&self) -> Option<Arc<GuardHandoff>> {
        self.stop_requested.store(true, Ordering::Release);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if inner.phase == RuntimePhase::Stopped {
            return None;
        }
        inner.phase = RuntimePhase::Stopping;
        inner.pending_handoff.take()
    }

    pub(crate) fn mark_stopped(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        inner.phase = RuntimePhase::Stopped;
        inner.active_id = None;
        inner.pending_handoff = None;
        self.native_thread_id.store(0, Ordering::Release);
    }

    pub(crate) fn native_thread_id(&self) -> u32 {
        self.native_thread_id.load(Ordering::Acquire) as u32
    }

    pub(crate) fn set_native_thread_id(&self, id: u32) {
        self.native_thread_id
            .store(u64::from(id), Ordering::Release);
    }

    pub(crate) const fn presentation_timeout(&self) -> Duration {
        self.presentation_timeout
    }

    pub(crate) fn stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
enum HandoffStage {
    Pending(Option<PendingMouseInputGuard>),
    Blocking,
    Failed,
}

#[derive(Debug)]
pub(crate) struct GuardHandoff {
    stage: Mutex<HandoffStage>,
}

impl GuardHandoff {
    fn new(guard: Option<PendingMouseInputGuard>) -> Self {
        Self {
            stage: Mutex::new(HandoffStage::Pending(guard)),
        }
    }

    /// Called only after native presentation verification. The mutex makes the
    /// timeout/release versus visible-overlay transfer decision atomic.
    pub(crate) fn transfer_to_visible_overlay(&self) -> bool {
        let mut stage = self
            .stage
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let HandoffStage::Pending(guard) = &mut *stage else {
            return false;
        };
        if let Some(mut guard) = guard.take() {
            guard.transfer_to_blocking_overlay();
        }
        *stage = HandoffStage::Blocking;
        true
    }

    /// Fails open while retaining an already-suppressed button pair long
    /// enough for the platform guard's bounded drain to complete.
    pub(crate) fn fail_open(&self) -> bool {
        let guard = {
            let mut stage = self
                .stage
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let HandoffStage::Pending(guard) = &mut *stage else {
                return false;
            };
            let guard = guard.take();
            *stage = HandoffStage::Failed;
            guard
        };
        if let Some(guard) = guard {
            release_guard_with_bounded_drain(guard);
        }
        true
    }
}

fn release_guard_with_bounded_drain(mut guard: PendingMouseInputGuard) {
    guard.release();
    // Keeping the owner alive lets a consumed Down/Up pair drain. If thread
    // creation fails, dropping the captured closure still invokes the guard's
    // fail-open destructor immediately.
    let _ = thread::Builder::new()
        .name("poe-alarm-alert-guard-drain".to_owned())
        .spawn(move || {
            thread::sleep(RELEASE_DRAIN_LIFETIME);
            drop(guard);
        });
}

#[derive(Debug)]
pub(crate) struct TriggerCommand {
    pub alert_id: AlertId,
    pub request: AlertTrigger,
    pub handoff: Arc<GuardHandoff>,
}

#[derive(Debug)]
pub(crate) enum WorkerCommand {
    Trigger(TriggerCommand),
}

/// Owns one process-local native red alert worker.
pub struct BlockingAlertService {
    command_sender: mpsc::SyncSender<WorkerCommand>,
    event_receiver: mpsc::Receiver<AlertEvent>,
    warning_receiver: mpsc::Receiver<AlertFailure>,
    event_sender: mpsc::Sender<AlertEvent>,
    runtime: Arc<RuntimeShared>,
    next_alert_id: AtomicU64,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for BlockingAlertService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BlockingAlertService")
            .field("thread_id", &self.runtime.native_thread_id())
            .finish_non_exhaustive()
    }
}

impl BlockingAlertService {
    /// Starts the dedicated owner thread. Call this once during application
    /// initialization; trigger, acknowledge, and stop are non-waiting methods.
    pub fn start(config: AlertServiceConfig) -> Result<Self, AlertFailure> {
        if config.presentation_timeout.is_zero() {
            return Err(AlertFailure::new(
                AlertFailureKind::InternalProtocol,
                "presentation timeout must be greater than zero",
            ));
        }
        let ownership = ServiceOwnership::acquire()?;
        let (command_sender, command_receiver) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        let (event_sender, event_receiver) = mpsc::channel();
        let (warning_sender, warning_receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let runtime = Arc::new(RuntimeShared::new(config.presentation_timeout));
        let thread = platform_spawn_worker(
            config,
            command_receiver,
            event_sender.clone(),
            warning_sender,
            Arc::clone(&runtime),
            ready_sender,
            ownership,
        )?;
        match ready_receiver.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                command_sender,
                event_receiver,
                warning_receiver,
                event_sender,
                runtime,
                next_alert_id: AtomicU64::new(1),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(error) => {
                let pending = runtime.request_stop();
                if let Some(pending) = pending {
                    pending.fail_open();
                }
                let _ = platform_wake_stop(runtime.native_thread_id());
                drop(thread);
                Err(AlertFailure::new(
                    AlertFailureKind::ThreadStart,
                    format!("alert worker did not become ready within 750 ms: {error}"),
                ))
            }
        }
    }

    /// Queues one latched alert without waiting for presentation. The optional
    /// pending guard is atomically transferred only after the HWND is verified.
    pub fn trigger(
        &self,
        request: AlertTrigger,
        pending_guard: Option<PendingMouseInputGuard>,
    ) -> Result<AlertTriggerStatus, AlertTriggerError> {
        let alert_id = AlertId(self.next_alert_id.fetch_add(1, Ordering::Relaxed).max(1));
        let handoff = Arc::new(GuardHandoff::new(pending_guard));
        let status = match self.runtime.try_reserve(alert_id, Arc::clone(&handoff)) {
            Ok(status) => status,
            Err(error) => {
                handoff.fail_open();
                return Err(error);
            }
        };
        let AlertTriggerStatus::Accepted(_) = status else {
            handoff.fail_open();
            return Ok(status);
        };

        if let Err(error) = self.schedule_timeout(alert_id, Arc::clone(&handoff)) {
            self.fail_queued_trigger(alert_id, &handoff, error);
            return Err(AlertTriggerError::WorkerUnavailable);
        }

        let command = WorkerCommand::Trigger(TriggerCommand {
            alert_id,
            request,
            handoff: Arc::clone(&handoff),
        });
        match self.command_sender.try_send(command) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                self.fail_queued_trigger(
                    alert_id,
                    &handoff,
                    AlertFailure::new(
                        AlertFailureKind::Dispatch,
                        "the bounded alert command queue is full",
                    ),
                );
                return Err(AlertTriggerError::CommandQueueBusy);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.fail_queued_trigger(
                    alert_id,
                    &handoff,
                    AlertFailure::new(
                        AlertFailureKind::Dispatch,
                        "the alert worker command channel is disconnected",
                    ),
                );
                return Err(AlertTriggerError::WorkerUnavailable);
            }
        }
        if let Err(error) = platform_wake_commands(self.runtime.native_thread_id()) {
            self.fail_queued_trigger(alert_id, &handoff, error);
            return Err(AlertTriggerError::WorkerUnavailable);
        }
        Ok(status)
    }

    /// Requests acknowledgement using the same 300 ms/all-buttons-up drain as
    /// the native button and Ctrl+Shift+F12. This method never waits for the overlay.
    pub fn acknowledge(&self) -> Result<(), AlertTriggerError> {
        if self.runtime.stop_requested.load(Ordering::Acquire) {
            return Err(AlertTriggerError::Stopping);
        }
        platform_wake_acknowledge(self.runtime.native_thread_id())
            .map_err(|_| AlertTriggerError::WorkerUnavailable)
    }

    /// Requests cleanup and return from the native message loop. It does not
    /// join the worker and is safe to invoke from a UI event handler.
    pub fn stop(&self) -> Result<(), AlertTriggerError> {
        let pending = self.runtime.request_stop();
        if let Some(pending) = pending {
            pending.fail_open();
        }
        let thread_id = self.runtime.native_thread_id();
        if thread_id == 0 {
            return Ok(());
        }
        platform_wake_stop(thread_id).map_err(|_| AlertTriggerError::WorkerUnavailable)
    }

    /// Returns one already-delivered event without waiting.
    pub fn try_next_event(&self) -> Option<AlertEvent> {
        self.event_receiver.try_recv().ok()
    }

    /// Returns one non-fatal platform compatibility warning without waiting.
    /// A warning never means that the visible alert or mouse-input shield was
    /// abandoned; fatal presentation failures are delivered as [`AlertEvent::Failed`].
    pub fn try_next_warning(&self) -> Option<AlertFailure> {
        self.warning_receiver.try_recv().ok()
    }

    #[cfg(test)]
    pub(crate) fn diagnostic_snapshot(&self) -> String {
        let inner = self
            .runtime
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        format!(
            "phase={:?}, active_id={:?}, stop_requested={}, native_thread_id={}, worker_finished={}",
            inner.phase,
            inner.active_id,
            self.runtime.stop_requested(),
            self.runtime.native_thread_id(),
            self.thread.as_ref().is_none_or(JoinHandle::is_finished),
        )
    }

    fn schedule_timeout(
        &self,
        alert_id: AlertId,
        handoff: Arc<GuardHandoff>,
    ) -> Result<(), AlertFailure> {
        let runtime = Arc::clone(&self.runtime);
        let sender = self.event_sender.clone();
        // Config is immutable on the worker. The platform publishes the
        // effective timeout for the watchdog before signaling startup.
        let timeout = runtime.presentation_timeout();
        thread::Builder::new()
            .name("poe-alarm-alert-handoff-timeout".to_owned())
            .spawn(move || {
                thread::sleep(timeout);
                if !handoff.fail_open() {
                    return;
                }
                if runtime.fail_pending(alert_id, &handoff) {
                    let _ = sender.send(AlertEvent::Failed {
                        alert_id,
                        failure: AlertFailure::new(
                            AlertFailureKind::PresentationTimeout,
                            format!(
                                "blocking alert was not verified within {} ms",
                                timeout.as_millis()
                            ),
                        ),
                    });
                }
                let _ = platform_wake_commands(runtime.native_thread_id());
            })
            .map(|_| ())
            .map_err(|error| {
                AlertFailure::new(
                    AlertFailureKind::ThreadStart,
                    format!("could not start alert handoff watchdog: {error}"),
                )
            })
    }

    fn fail_queued_trigger(
        &self,
        alert_id: AlertId,
        handoff: &Arc<GuardHandoff>,
        failure: AlertFailure,
    ) {
        handoff.fail_open();
        if self.runtime.fail_pending(alert_id, handoff) {
            let _ = self
                .event_sender
                .send(AlertEvent::Failed { alert_id, failure });
        }
    }
}

impl Drop for BlockingAlertService {
    fn drop(&mut self) {
        let _ = self.stop();
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.join();
        }
        // Dropping a live JoinHandle detaches it; runtime shutdown remains on
        // its dedicated thread instead of blocking the UI destructor.
    }
}

#[cfg(windows)]
fn platform_spawn_worker(
    config: AlertServiceConfig,
    commands: mpsc::Receiver<WorkerCommand>,
    events: mpsc::Sender<AlertEvent>,
    warnings: mpsc::Sender<AlertFailure>,
    runtime: Arc<RuntimeShared>,
    ready: mpsc::SyncSender<Result<(), AlertFailure>>,
    ownership: ServiceOwnership,
) -> Result<JoinHandle<()>, AlertFailure> {
    crate::win32::spawn_worker(
        config, commands, events, warnings, runtime, ready, ownership,
    )
}

#[cfg(not(windows))]
fn platform_spawn_worker(
    config: AlertServiceConfig,
    commands: mpsc::Receiver<WorkerCommand>,
    events: mpsc::Sender<AlertEvent>,
    warnings: mpsc::Sender<AlertFailure>,
    runtime: Arc<RuntimeShared>,
    ready: mpsc::SyncSender<Result<(), AlertFailure>>,
    ownership: ServiceOwnership,
) -> Result<JoinHandle<()>, AlertFailure> {
    crate::non_windows::spawn_worker(
        config, commands, events, warnings, runtime, ready, ownership,
    )
}

fn platform_wake_commands(thread_id: u32) -> Result<(), AlertFailure> {
    #[cfg(windows)]
    {
        crate::win32::wake_commands(thread_id)
    }
    #[cfg(not(windows))]
    {
        crate::non_windows::wake_commands(thread_id)
    }
}

fn platform_wake_acknowledge(thread_id: u32) -> Result<(), AlertFailure> {
    #[cfg(windows)]
    {
        crate::win32::wake_acknowledge(thread_id)
    }
    #[cfg(not(windows))]
    {
        crate::non_windows::wake_acknowledge(thread_id)
    }
}

fn platform_wake_stop(thread_id: u32) -> Result<(), AlertFailure> {
    #[cfg(windows)]
    {
        crate::win32::wake_stop(thread_id)
    }
    #[cfg(not(windows))]
    {
        crate::non_windows::wake_stop(thread_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ObservedHandoffStage {
        Pending,
        Blocking,
        Failed,
    }

    impl GuardHandoff {
        fn observed_stage(&self) -> ObservedHandoffStage {
            match &*self
                .stage
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
            {
                HandoffStage::Pending(_) => ObservedHandoffStage::Pending,
                HandoffStage::Blocking => ObservedHandoffStage::Blocking,
                HandoffStage::Failed => ObservedHandoffStage::Failed,
            }
        }
    }

    #[test]
    fn failed_handoff_is_fail_open_and_cannot_transfer_later() {
        let handoff = GuardHandoff::new(None);
        assert_eq!(handoff.observed_stage(), ObservedHandoffStage::Pending);
        assert!(handoff.fail_open());
        assert_eq!(handoff.observed_stage(), ObservedHandoffStage::Failed);
        assert!(!handoff.transfer_to_visible_overlay());
    }

    #[test]
    fn successful_handoff_cannot_be_failed_after_transfer() {
        let handoff = GuardHandoff::new(None);
        assert!(handoff.transfer_to_visible_overlay());
        assert_eq!(handoff.observed_stage(), ObservedHandoffStage::Blocking);
        assert!(!handoff.fail_open());
    }
}
