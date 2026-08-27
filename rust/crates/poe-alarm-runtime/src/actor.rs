use std::fmt;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use poe_alarm_alert_win::AlertServiceConfig;
use poe_alarm_core::LogicalAffixMatch;
use poe_alarm_monitoring::{
    EventSink, Monitor, MonitorDetection, MonitorEvent, MonitorPlan, RecognitionResult, SystemClock,
};
use poe_alarm_platform_win::game_window_rect;

use crate::backend::{AffixSourceFactory, DynamicSource, ProductionSourceFactory};
use crate::protection::AlertPresentation;
use crate::{
    AlertLatchStatus, BackendError, CompiledRuntimeSettings, DetectionSummary, ItemCheckEvaluation,
    ItemCheckReport, ItemCheckRequest, NativeProtection, ProtectionEvent, ProtectionService,
    RuntimeCommand, RuntimeEvent, RuntimeGeneration, RuntimeOperation, RuntimeRequestId,
    RuntimeState, compile_settings,
};

const COMMAND_CAPACITY: usize = 32;
const ACTOR_POLL_INTERVAL: Duration = Duration::from_millis(4);
const SHUTDOWN_TARGET: Duration = Duration::from_secs(1);

type LiveMonitor = Monitor<DynamicSource, SystemClock, MonitorBridge>;

#[derive(Clone, Debug)]
pub struct ProductionRuntimeConfig {
    pub alert: AlertServiceConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeSendError {
    QueueFull,
    RuntimeStopped,
}

impl fmt::Display for RuntimeSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::QueueFull => "the runtime command queue is full",
            Self::RuntimeStopped => "the runtime is no longer running",
        })
    }
}

impl std::error::Error for RuntimeSendError {}

/// UI-facing non-blocking runtime endpoint.
///
/// No method joins a worker or reads the clipboard. Dropping the handle sends
/// a best-effort shutdown request and detaches the actor thread.
pub struct RuntimeHandle {
    commands: mpsc::SyncSender<RuntimeCommand>,
    events: mpsc::Receiver<RuntimeEvent>,
    latest_snapshot: Arc<std::sync::Mutex<Option<RuntimeEvent>>>,
}

impl RuntimeHandle {
    pub fn start_production(config: ProductionRuntimeConfig) -> io::Result<Self> {
        Self::spawn_initialized(move || {
            let sources: Arc<dyn AffixSourceFactory> = Arc::new(ProductionSourceFactory);
            let protection: Arc<dyn ProtectionService> =
                Arc::new(NativeProtection::start(config.alert)?);
            Ok((sources, protection))
        })
    }

    pub fn spawn_with_components(
        sources: Arc<dyn AffixSourceFactory>,
        protection: Arc<dyn ProtectionService>,
    ) -> io::Result<Self> {
        Self::spawn_initialized(move || Ok((sources, protection)))
    }

    fn spawn_initialized<F>(initialize: F) -> io::Result<Self>
    where
        F: FnOnce() -> Result<
                (Arc<dyn AffixSourceFactory>, Arc<dyn ProtectionService>),
                crate::ProtectionError,
            > + Send
            + 'static,
    {
        let (command_sender, command_receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (event_sender, event_receiver) = mpsc::channel();
        let latest_snapshot = Arc::new(std::sync::Mutex::new(None));
        let actor_snapshot = Arc::clone(&latest_snapshot);
        thread::Builder::new()
            .name("poe-alarm-runtime".to_owned())
            .spawn(move || match initialize() {
                Ok((sources, protection)) => {
                    let actor = RuntimeActor::new(
                        command_receiver,
                        event_sender.clone(),
                        actor_snapshot,
                        sources,
                        protection,
                    );
                    let _ = event_sender.send(RuntimeEvent::Ready);
                    actor.run();
                }
                Err(error) => {
                    let _ = event_sender.send(RuntimeEvent::Fault {
                        generation: None,
                        operation: RuntimeOperation::Startup,
                        detail: error.to_string(),
                        actionable: false,
                    });
                }
            })?;
        Ok(Self {
            commands: command_sender,
            events: event_receiver,
            latest_snapshot,
        })
    }

    pub fn try_send(&self, command: RuntimeCommand) -> Result<(), RuntimeSendError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => RuntimeSendError::QueueFull,
                mpsc::TrySendError::Disconnected(_) => RuntimeSendError::RuntimeStopped,
            })
    }

    pub fn start(&self, settings: poe_alarm_settings::AppSettings) -> Result<(), RuntimeSendError> {
        self.try_send(RuntimeCommand::Start { settings })
    }

    pub fn stop(&self) -> Result<(), RuntimeSendError> {
        self.try_send(RuntimeCommand::Stop)
    }

    pub fn check_item(&self, request: ItemCheckRequest) -> Result<(), RuntimeSendError> {
        self.try_send(RuntimeCommand::CheckItem(request))
    }

    pub fn cancel_item_check(&self, request_id: RuntimeRequestId) -> Result<(), RuntimeSendError> {
        self.try_send(RuntimeCommand::CancelItemCheck { request_id })
    }

    pub fn acknowledge_alert(&self) -> Result<(), RuntimeSendError> {
        self.try_send(RuntimeCommand::AlertAck)
    }

    pub fn shutdown(&self) -> Result<(), RuntimeSendError> {
        self.try_send(RuntimeCommand::Shutdown)
    }

    #[must_use]
    pub fn try_next_event(&self) -> Option<RuntimeEvent> {
        self.events.try_recv().ok().or_else(|| {
            self.latest_snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
        })
    }
}

impl Drop for RuntimeHandle {
    fn drop(&mut self) {
        let _ = self.commands.try_send(RuntimeCommand::Shutdown);
    }
}

struct RuntimeActor {
    commands: mpsc::Receiver<RuntimeCommand>,
    events: mpsc::Sender<RuntimeEvent>,
    internal_sender: mpsc::Sender<InternalEvent>,
    internal_events: mpsc::Receiver<InternalEvent>,
    monitor_snapshot:
        Arc<std::sync::Mutex<Option<(RuntimeGeneration, poe_alarm_monitoring::MonitorSnapshot)>>>,
    public_snapshot: Arc<std::sync::Mutex<Option<RuntimeEvent>>>,
    sources: Arc<dyn AffixSourceFactory>,
    protection: Arc<dyn ProtectionService>,
    live: Option<LiveSession>,
    retired: Option<RetiredWork>,
    pending_intent: Option<RuntimeIntent>,
    active_generation: Option<RuntimeGeneration>,
    next_generation: u64,
    state: RuntimeState,
}

struct LiveSession {
    generation: RuntimeGeneration,
    valid: Arc<AtomicBool>,
    monitor: LiveMonitor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetiredWork {
    Live {
        generation: RuntimeGeneration,
    },
    /// The OS refused the one cleanup thread. The monitor is deliberately
    /// leaked until process exit and replacement work remains disabled so the
    /// leak cannot repeat without bound.
    CleanupBlocked,
}

enum RuntimeIntent {
    Live(poe_alarm_settings::AppSettings),
    ItemCheck(ItemCheckRequest),
}

impl RuntimeActor {
    fn new(
        commands: mpsc::Receiver<RuntimeCommand>,
        events: mpsc::Sender<RuntimeEvent>,
        public_snapshot: Arc<std::sync::Mutex<Option<RuntimeEvent>>>,
        sources: Arc<dyn AffixSourceFactory>,
        protection: Arc<dyn ProtectionService>,
    ) -> Self {
        let (internal_sender, internal_events) = mpsc::channel();
        Self {
            commands,
            events,
            internal_sender,
            internal_events,
            monitor_snapshot: Arc::new(std::sync::Mutex::new(None)),
            public_snapshot,
            sources,
            protection,
            live: None,
            retired: None,
            pending_intent: None,
            active_generation: None,
            next_generation: 1,
            state: RuntimeState::Idle,
        }
    }

    fn run(mut self) {
        self.publish_state(None, RuntimeState::Idle);
        loop {
            self.drain_internal_events();
            self.drain_protection_events();
            match self.commands.recv_timeout(ACTOR_POLL_INTERVAL) {
                Ok(command) => {
                    if self.handle_command_batch(command) {
                        self.shutdown();
                        return;
                    }
                    self.try_launch_pending();
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.shutdown();
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => self.try_launch_pending(),
            }
        }
    }

    /// Coalesces a burst before standing up a fresh monitor. Shutdown therefore
    /// has priority over a queued replacement and cannot get stuck behind a
    /// session that is about to be replaced anyway.
    fn handle_command_batch(&mut self, first: RuntimeCommand) -> bool {
        let mut next = Some(first);
        loop {
            let command = next.take().expect("the command batch always has a head");
            if matches!(command, RuntimeCommand::Shutdown) {
                return true;
            }
            self.handle_command(command);
            match self.commands.try_recv() {
                Ok(command) => next = Some(command),
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => return true,
            }
        }
    }

    fn handle_command(&mut self, command: RuntimeCommand) {
        match command {
            RuntimeCommand::Start { settings } => {
                self.replace_intent(RuntimeIntent::Live(settings));
                self.retire_active_work(true);
            }
            RuntimeCommand::Stop => self.stop_all(true),
            RuntimeCommand::CheckItem(request) => {
                self.replace_intent(RuntimeIntent::ItemCheck(request));
                self.retire_active_work(true);
            }
            RuntimeCommand::CancelItemCheck { request_id } => {
                self.cancel_pending_item_check(request_id, true);
            }
            RuntimeCommand::AlertAck => {
                if let Err(error) = self.protection.acknowledge() {
                    self.fault(
                        self.active_generation,
                        RuntimeOperation::Alert,
                        error.to_string(),
                    );
                }
            }
            RuntimeCommand::Shutdown => unreachable!("shutdown is handled by the receive loop"),
        }
    }

    fn allocate_generation(&mut self) -> RuntimeGeneration {
        self.clear_snapshots();
        let generation = RuntimeGeneration(self.next_generation.max(1));
        self.next_generation = self.next_generation.saturating_add(1).max(1);
        generation
    }

    fn launch_live(&mut self, settings: poe_alarm_settings::AppSettings) {
        debug_assert!(self.live.is_none());
        debug_assert!(self.retired.is_none());
        let generation = self.allocate_generation();
        self.active_generation = Some(generation);
        self.publish_state(Some(generation), RuntimeState::Starting);
        let compiled = match compile_settings(&settings) {
            Ok(compiled) => compiled,
            Err(error) => {
                self.fault(Some(generation), RuntimeOperation::Start, error.to_string());
                return;
            }
        };
        self.publish_compiled(generation, &compiled);

        let source = match self.sources.create() {
            Ok(source) => DynamicSource(source),
            Err(error) => {
                self.fault(Some(generation), RuntimeOperation::Start, error.to_string());
                return;
            }
        };
        if let Err(error) = self.protection.begin_session(generation) {
            self.fault(Some(generation), RuntimeOperation::Start, error.to_string());
            return;
        }
        if compiled.input_guard_enabled
            && let Err(error) = self.protection.prepare_pending(generation)
        {
            self.protection.stop_pending(generation);
            self.fault(
                Some(generation),
                RuntimeOperation::Start,
                format!("could not prepare input protection: {error}"),
            );
            return;
        }
        let valid = Arc::new(AtomicBool::new(true));
        let bridge = MonitorBridge {
            generation,
            alert_copy: compiled.alert_copy.clone(),
            input_guard_enabled: compiled.input_guard_enabled,
            protection: Arc::clone(&self.protection),
            events: self.internal_sender.clone(),
            latest_snapshot: Arc::clone(&self.monitor_snapshot),
            session_valid: Arc::clone(&valid),
            safety_failed: AtomicBool::new(false),
        };
        let mut monitor = Monitor::new(source, SystemClock::default(), bridge);
        match monitor.start(compiled.plan) {
            Ok(_) => {
                self.live = Some(LiveSession {
                    generation,
                    valid,
                    monitor,
                });
                self.publish_state(Some(generation), RuntimeState::Monitoring);
            }
            Err(error) => {
                self.protection.stop_pending(generation);
                self.fault(Some(generation), RuntimeOperation::Start, error.to_string());
            }
        }
    }

    /// Runs one offline rule check against item text the user supplied.
    ///
    /// The OCR build handed this to a bounded worker thread that owned WIC's
    /// COM apartment and a cached ONNX session, because decoding a PNG and
    /// recognising it took seconds and had to stay cancellable. A parse and an
    /// evaluation take microseconds, so the whole apparatus — worker,
    /// cancellation flag, in-flight task tracking, retirement — is gone and the
    /// answer is produced before this function returns.
    fn launch_item_check(&mut self, request: ItemCheckRequest) {
        debug_assert!(self.live.is_none());
        debug_assert!(self.retired.is_none());
        let generation = self.allocate_generation();
        self.active_generation = Some(generation);
        self.publish_state(Some(generation), RuntimeState::CheckingItem);
        let compiled = match compile_settings(&request.settings) {
            Ok(compiled) => compiled,
            Err(error) => {
                self.fault(
                    Some(generation),
                    RuntimeOperation::ItemCheck,
                    error.to_string(),
                );
                return;
            }
        };
        self.publish_compiled(generation, &compiled);
        match check_item_text(generation, request.request_id, &request.text, &compiled) {
            Ok(report) => {
                let _ = self.events.send(RuntimeEvent::ItemCheckCompleted(report));
                self.active_generation = None;
                self.publish_state(None, RuntimeState::Idle);
            }
            Err(error) => {
                self.active_generation = None;
                self.fault(
                    Some(generation),
                    RuntimeOperation::ItemCheck,
                    error.to_string(),
                );
            }
        }
    }

    fn stop_all(&mut self, acknowledge_alert: bool) {
        self.cancel_pending_intent(true);
        if self.live.is_none() && acknowledge_alert && !self.protection.ready_for_new_work() {
            let _ = self.protection.acknowledge();
        }
        let stopped = self.stop_live(acknowledge_alert);
        self.active_generation = None;
        self.clear_snapshots();
        if stopped {
            self.publish_state(None, RuntimeState::Idle);
        }
    }

    fn replace_intent(&mut self, intent: RuntimeIntent) {
        if let Some(RuntimeIntent::ItemCheck(request)) = self.pending_intent.replace(intent) {
            let _ = self.events.send(RuntimeEvent::ItemCheckCancelled {
                request_id: request.request_id,
            });
        }
    }

    fn cancel_pending_intent(&mut self, publish: bool) {
        if let Some(RuntimeIntent::ItemCheck(request)) = self.pending_intent.take()
            && publish
        {
            let _ = self.events.send(RuntimeEvent::ItemCheckCancelled {
                request_id: request.request_id,
            });
        }
    }

    fn retire_active_work(&mut self, acknowledge_alert: bool) {
        if self.live.is_some() {
            let _ = self.stop_live(acknowledge_alert);
        }
    }

    fn try_launch_pending(&mut self) {
        if self.live.is_some() || self.retired.is_some() || !self.protection.ready_for_new_work() {
            return;
        }
        match self.pending_intent.take() {
            Some(RuntimeIntent::Live(settings)) => self.launch_live(settings),
            Some(RuntimeIntent::ItemCheck(request)) => self.launch_item_check(request),
            None => {}
        }
    }

    fn stop_live(&mut self, acknowledge_alert: bool) -> bool {
        let Some(live) = self.live.take() else {
            return true;
        };
        if self.active_generation == Some(live.generation) {
            self.active_generation = None;
        }
        live.valid.store(false, Ordering::Release);
        // Invalidates alert ownership for all profiles and terminally removes
        // the short guard when this profile uses one.
        self.protection.stop_pending(live.generation);
        if acknowledge_alert {
            let _ = self.protection.acknowledge();
        }
        if self.retired.is_some() {
            std::mem::forget(live.monitor);
            self.retired = Some(RetiredWork::CleanupBlocked);
            self.fault(
                Some(live.generation),
                RuntimeOperation::Stop,
                "runtime cleanup capacity was already occupied; replacement work is disabled until process exit"
                    .to_owned(),
            );
            return false;
        }
        self.spawn_monitor_drain(live.generation, live.monitor, RuntimeOperation::Stop)
    }

    /// Cancels an item-text check that is still queued behind retiring work.
    ///
    /// Nothing is ever in flight to cancel: the check itself completes inside
    /// `launch_item_check`. What can be cancelled is the intent waiting for a
    /// live session to finish draining.
    fn cancel_pending_item_check(&mut self, expected_request: RuntimeRequestId, publish: bool) {
        let Some(RuntimeIntent::ItemCheck(request)) = self.pending_intent.as_ref() else {
            return;
        };
        if request.request_id != expected_request {
            return;
        }
        self.pending_intent = None;
        if publish {
            let _ = self.events.send(RuntimeEvent::ItemCheckCancelled {
                request_id: expected_request,
            });
        }
    }

    fn shutdown(&mut self) {
        let started = Instant::now();
        self.publish_state(self.active_generation, RuntimeState::ShuttingDown);
        self.cancel_pending_intent(false);
        self.drain_live_for_shutdown();
        self.active_generation = None;
        self.clear_snapshots();
        if let Err(error) = self.protection.shutdown_fail_open() {
            self.fault(None, RuntimeOperation::Shutdown, error.to_string());
        }
        self.publish_state(None, RuntimeState::Stopped);
        let elapsed = started.elapsed();
        let _ = self.events.send(RuntimeEvent::ShutdownComplete {
            elapsed,
            within_one_second: elapsed <= SHUTDOWN_TARGET,
        });
    }

    /// Invalidates input and event ownership synchronously, then lets a
    /// possibly in-flight clipboard round trip finish outside the actor. The
    /// source observes cancellation between reads but cannot interrupt one the
    /// client has not answered yet.
    fn drain_live_for_shutdown(&mut self) {
        let Some(live) = self.live.take() else {
            return;
        };
        if self.active_generation == Some(live.generation) {
            self.active_generation = None;
        }
        live.valid.store(false, Ordering::Release);
        self.protection.stop_pending(live.generation);

        let _ = self.spawn_monitor_drain(live.generation, live.monitor, RuntimeOperation::Shutdown);
    }

    fn spawn_monitor_drain(
        &mut self,
        generation: RuntimeGeneration,
        monitor: LiveMonitor,
        operation: RuntimeOperation,
    ) -> bool {
        // Keep ownership recoverable until spawn succeeds. If the operating
        // system cannot create a cleanup thread, leaking the monitor is safer
        // than running its blocking Drop on the actor and missing the bounded
        // fail-open shutdown contract. Process exit reclaims native resources.
        let holder = Arc::new(std::sync::Mutex::new(Some(monitor)));
        let worker_holder = Arc::clone(&holder);
        let sender = self.internal_sender.clone();
        match thread::Builder::new()
            .name("poe-alarm-monitor-drain".to_owned())
            .spawn(move || {
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    let monitor = worker_holder
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                    if let Some(mut monitor) = monitor {
                        let _ = monitor.stop();
                    }
                }));
                let _ = sender.send(InternalEvent::LiveDrainFinished { generation });
            }) {
            Ok(_) => {
                self.retired = Some(RetiredWork::Live { generation });
                true
            }
            Err(error) => {
                std::mem::forget(holder);
                self.retired = Some(RetiredWork::CleanupBlocked);
                let _ = self.events.send(RuntimeEvent::Fault {
                    generation: Some(generation),
                    operation,
                    detail: format!(
                        "could not start the monitor cleanup worker; resources will be reclaimed at process exit: {error}"
                    ),
                    actionable: false,
                });
                false
            }
        }
    }

    fn drain_internal_events(&mut self) {
        self.drain_monitor_snapshot();
        while let Ok(event) = self.internal_events.try_recv() {
            match event {
                InternalEvent::LiveDrainFinished { generation }
                    if self.retired == Some(RetiredWork::Live { generation }) =>
                {
                    self.retired = None;
                }
                InternalEvent::Monitor { generation, event }
                    if self.active_generation == Some(generation) =>
                {
                    match event {
                        BridgeMonitorEvent::Detection(detection) => {
                            self.publish_state(Some(generation), RuntimeState::MatchFound);
                            let _ = self.events.send(RuntimeEvent::MatchFound {
                                generation,
                                detection,
                            });
                        }
                        BridgeMonitorEvent::Fault(detail) => {
                            let _ = self.stop_live(false);
                            self.fault(Some(generation), RuntimeOperation::Alert, detail);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn drain_monitor_snapshot(&mut self) {
        let pending = self
            .monitor_snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some((generation, snapshot)) = pending else {
            return;
        };
        if self.active_generation != Some(generation) {
            return;
        }
        let state = RuntimeEvent::monitor_state(&snapshot);
        self.state = state;
        if state == RuntimeState::Faulted {
            let _ = self.stop_live(false);
            self.fault_with(
                Some(generation),
                RuntimeOperation::Monitoring,
                snapshot
                    .detail
                    .clone()
                    .unwrap_or_else(|| "monitoring failed".to_owned()),
                snapshot.detail_is_actionable,
            );
        }
        *self
            .public_snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(RuntimeEvent::MonitorSnapshot {
                generation,
                snapshot,
            });
    }

    fn drain_protection_events(&mut self) {
        for event in self.protection.poll_events() {
            match event {
                ProtectionEvent::Presented { generation }
                    if self.active_generation == Some(generation) =>
                {
                    let _ = self
                        .events
                        .send(RuntimeEvent::AlertPresented { generation });
                }
                ProtectionEvent::Acknowledged { generation } => {
                    if self.active_generation == Some(generation)
                        || self.active_generation.is_none()
                    {
                        if self.active_generation == Some(generation) {
                            let _ = self.stop_live(false);
                            self.publish_state(None, RuntimeState::Idle);
                        }
                        let _ = self
                            .events
                            .send(RuntimeEvent::AlertAcknowledged { generation });
                    }
                }
                ProtectionEvent::SoundFailed { generation, detail }
                    if self.active_generation == Some(generation) =>
                {
                    let _ = self
                        .events
                        .send(RuntimeEvent::AlertSoundFailed { generation, detail });
                }
                ProtectionEvent::Failed { generation, detail } => {
                    if generation.is_none() || generation == self.active_generation {
                        if generation.is_some() {
                            let _ = self.stop_live(false);
                        }
                        self.fault(generation, RuntimeOperation::Alert, detail);
                    }
                }
                ProtectionEvent::Stopped
                | ProtectionEvent::Presented { .. }
                | ProtectionEvent::SoundFailed { .. } => {}
            }
        }
    }

    fn publish_compiled(&self, generation: RuntimeGeneration, compiled: &CompiledRuntimeSettings) {
        let _ = self.events.send(RuntimeEvent::SettingsCompiled {
            generation,
            ui: compiled.ui.clone(),
        });
    }

    fn clear_snapshots(&self) {
        self.monitor_snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        self.public_snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }

    fn publish_state(&mut self, generation: Option<RuntimeGeneration>, state: RuntimeState) {
        self.state = state;
        let _ = self
            .events
            .send(RuntimeEvent::StateChanged { generation, state });
    }

    fn fault(
        &mut self,
        generation: Option<RuntimeGeneration>,
        operation: RuntimeOperation,
        detail: String,
    ) {
        self.fault_with(generation, operation, detail, false);
    }

    fn fault_with(
        &mut self,
        generation: Option<RuntimeGeneration>,
        operation: RuntimeOperation,
        detail: String,
        actionable: bool,
    ) {
        self.state = RuntimeState::Faulted;
        let _ = self.events.send(RuntimeEvent::Fault {
            generation,
            operation,
            detail,
            actionable,
        });
    }
}

enum InternalEvent {
    Monitor {
        generation: RuntimeGeneration,
        event: BridgeMonitorEvent,
    },
    LiveDrainFinished {
        generation: RuntimeGeneration,
    },
}

enum BridgeMonitorEvent {
    Detection(DetectionSummary),
    Fault(String),
}

struct MonitorBridge {
    generation: RuntimeGeneration,
    alert_copy: crate::AlertCopy,
    input_guard_enabled: bool,
    protection: Arc<dyn ProtectionService>,
    events: mpsc::Sender<InternalEvent>,
    latest_snapshot:
        Arc<std::sync::Mutex<Option<(RuntimeGeneration, poe_alarm_monitoring::MonitorSnapshot)>>>,
    session_valid: Arc<AtomicBool>,
    safety_failed: AtomicBool,
}

impl MonitorBridge {
    fn send(&self, event: BridgeMonitorEvent) {
        let _ = self.events.send(InternalEvent::Monitor {
            generation: self.generation,
            event,
        });
    }
}

impl EventSink for MonitorBridge {
    fn emit(&self, event: MonitorEvent) {
        match event {
            MonitorEvent::InputGuardRequested { .. } => {
                if !self.input_guard_enabled || !self.session_valid.load(Ordering::Acquire) {
                    return;
                }
                if let Err(error) = self.protection.arm_pending(self.generation) {
                    self.safety_failed.store(true, Ordering::Release);
                    self.protection.release_pending(self.generation);
                    self.send(BridgeMonitorEvent::Fault(format!(
                        "could not protect pending input: {error}"
                    )));
                }
            }
            MonitorEvent::InputGuardReleased { .. } => {
                if self.input_guard_enabled && self.session_valid.load(Ordering::Acquire) {
                    self.protection.release_pending(self.generation);
                }
            }
            MonitorEvent::Snapshot(snapshot) => {
                if !self.session_valid.load(Ordering::Acquire) {
                    return;
                }
                *self
                    .latest_snapshot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some((self.generation, snapshot));
            }
            MonitorEvent::Detection(detection) => {
                if !self.session_valid.load(Ordering::Acquire)
                    || self.safety_failed.load(Ordering::Acquire)
                {
                    if self.input_guard_enabled {
                        self.protection.release_pending(self.generation);
                    }
                    return;
                }
                let summary = detection_summary(&detection);
                let alert_detail = if summary.detail.trim().is_empty() {
                    self.alert_copy.title.clone()
                } else {
                    summary.detail.clone()
                };
                let presentation = AlertPresentation {
                    copy: self.alert_copy.clone(),
                    detail: alert_detail,
                    // Asked here rather than at compile time: a rectangle
                    // captured when the user pressed Start can be minutes stale
                    // by the time an item rolls, and the client may not even
                    // have been up. This runs immediately before a fullscreen
                    // window is painted, so one lookup costs nothing.
                    anchor_region: game_window_rect(),
                };
                match self
                    .protection
                    .latch_red_alert(self.generation, presentation)
                {
                    Ok(AlertLatchStatus::Accepted | AlertLatchStatus::AlreadyLatched) => {
                        // Ownership is latched synchronously before this callback returns.
                        self.send(BridgeMonitorEvent::Detection(summary));
                    }
                    Err(error) => {
                        if self.input_guard_enabled {
                            self.protection.release_pending(self.generation);
                        }
                        self.send(BridgeMonitorEvent::Fault(format!(
                            "red alert could not be latched; input was released: {error}"
                        )));
                    }
                }
            }
        }
    }
}

fn detection_summary(detection: &MonitorDetection) -> DetectionSummary {
    match detection {
        MonitorDetection::Quick { matched, snapshot } => DetectionSummary {
            detail: matched.original_text.clone(),
            lines: snapshot.last_lines.clone(),
            matched_group: None,
        },
        MonitorDetection::Structured {
            evaluation,
            detected_text,
            snapshot,
        } => DetectionSummary {
            detail: detected_text.clone(),
            lines: snapshot.last_lines.clone(),
            matched_group: evaluation.matched_group_id().map(str::to_owned),
        },
    }
}

/// Parses item text and runs the compiled plan over it.
///
/// The grouping the parser recovered is carried through as physical line
/// identities, so a check here reaches the same verdict as the live monitor
/// would on the same item — including refusing to let one modifier satisfy two
/// conditions.
fn check_item_text(
    generation: RuntimeGeneration,
    request_id: RuntimeRequestId,
    text: &str,
    compiled: &CompiledRuntimeSettings,
) -> Result<ItemCheckReport, BackendError> {
    let parse_started = Instant::now();
    let item = poe_alarm_clipboard::parse(text).map_err(|error| BackendError(error.to_string()))?;
    let modifier_count = item.groups.len();
    let (lines, physical_line_identities) = item.render();
    let parse_elapsed = parse_started.elapsed();

    let result = RecognitionResult {
        lines,
        physical_line_identities,
        ..RecognitionResult::default()
    };
    let evaluation_started = Instant::now();
    let evaluation = evaluate_item(&compiled.plan, &result);
    let evaluation_elapsed = evaluation_started.elapsed();

    Ok(ItemCheckReport {
        request_id,
        generation,
        lines: result.lines,
        modifier_count,
        parse_elapsed,
        evaluation_elapsed,
        evaluation,
    })
}

fn evaluate_item(plan: &MonitorPlan, result: &RecognitionResult) -> ItemCheckEvaluation {
    match plan {
        MonitorPlan::Quick(target) => {
            let matched = target.find_match(&result.lines).or_else(|| {
                result
                    .target_assisted_match
                    .as_ref()
                    .filter(|candidate| candidate.canonical_text == target.template().text)
                    .cloned()
            });
            quick_evaluation(matched)
        }
        MonitorPlan::Structured(rules) => {
            let evaluation = rules.evaluate_with_identity(
                &result.lines,
                &result.assisted_observations,
                &result.physical_line_identities,
            );
            let group = evaluation.matched_group();
            ItemCheckEvaluation {
                is_match: evaluation.is_match,
                detail: group.map(|matched| {
                    let observed = matched
                        .conditions
                        .iter()
                        .filter(|condition| condition.is_matched)
                        .filter_map(|condition| condition.observation.as_ref())
                        .map(|observation| observation.original_text.as_str())
                        .collect::<Vec<_>>()
                        .join(" + ");
                    if observed.is_empty() {
                        matched.name.clone()
                    } else {
                        format!("{}: {observed}", matched.name)
                    }
                }),
                matched_group: evaluation.matched_group_id().map(str::to_owned),
            }
        }
    }
}

fn quick_evaluation(matched: Option<LogicalAffixMatch>) -> ItemCheckEvaluation {
    ItemCheckEvaluation {
        is_match: matched.is_some(),
        detail: matched.map(|value| value.original_text),
        matched_group: None,
    }
}
