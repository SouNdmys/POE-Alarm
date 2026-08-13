use std::fmt;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use poe_alarm_alert_win::AlertServiceConfig;
use poe_alarm_core::LogicalAffixMatch;
use poe_alarm_monitoring::{
    EventSink, Monitor, MonitorDetection, MonitorEvent, MonitorPlan, PreparedFrame,
    RecognitionResult, SystemClock,
};
use poe_alarm_recognition::PaddleBackendConfig;
use poe_alarm_vision::{BlueMaskSettings, CaptureRegion, CapturedFrame, build_blue_mask};

use crate::backend::{CaptureAdapter, RecognizerAdapter, ScreenshotBackend};
use crate::protection::AlertPresentation;
use crate::{
    AlertLatchStatus, BackendError, BackendFactory, CompiledRuntimeSettings, DetectionSummary,
    NativeProtection, ProductionBackendFactory, ProtectionEvent, ProtectionService, RuntimeCommand,
    RuntimeEvent, RuntimeGeneration, RuntimeOperation, RuntimeRequestId, RuntimeState,
    ScreenshotEvaluation, ScreenshotReport, ScreenshotRequest, compile_settings,
};

const COMMAND_CAPACITY: usize = 32;
const ACTOR_POLL_INTERVAL: Duration = Duration::from_millis(4);
const SHUTDOWN_TARGET: Duration = Duration::from_secs(1);
pub(crate) const MAXIMUM_SCREENSHOT_PASSES: usize = 128;

type LiveMonitor = Monitor<CaptureAdapter, RecognizerAdapter, SystemClock, MonitorBridge>;

#[derive(Clone, Debug)]
pub struct ProductionRuntimeConfig {
    pub alert: AlertServiceConfig,
    /// `None` resolves the packaged OCR assets beside the executable.
    pub paddle: Option<PaddleBackendConfig>,
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
/// No method joins a worker or performs capture/OCR. Dropping the handle sends
/// a best-effort shutdown request and detaches the actor thread.
pub struct RuntimeHandle {
    commands: mpsc::SyncSender<RuntimeCommand>,
    events: mpsc::Receiver<RuntimeEvent>,
    latest_snapshot: Arc<std::sync::Mutex<Option<RuntimeEvent>>>,
}

impl RuntimeHandle {
    pub fn start_production(config: ProductionRuntimeConfig) -> io::Result<Self> {
        Self::spawn_initialized(move || {
            let factory: Arc<dyn BackendFactory> =
                Arc::new(ProductionBackendFactory::new(config.paddle));
            let protection: Arc<dyn ProtectionService> =
                Arc::new(NativeProtection::start(config.alert)?);
            Ok((factory, protection))
        })
    }

    pub fn spawn_with_components(
        factory: Arc<dyn BackendFactory>,
        protection: Arc<dyn ProtectionService>,
    ) -> io::Result<Self> {
        Self::spawn_initialized(move || Ok((factory, protection)))
    }

    fn spawn_initialized<F>(initialize: F) -> io::Result<Self>
    where
        F: FnOnce() -> Result<
                (Arc<dyn BackendFactory>, Arc<dyn ProtectionService>),
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
                Ok((factory, protection)) => {
                    match RuntimeActor::new(
                        command_receiver,
                        event_sender.clone(),
                        actor_snapshot,
                        factory,
                        protection,
                    ) {
                        Ok(actor) => {
                            let _ = event_sender.send(RuntimeEvent::Ready);
                            actor.run();
                        }
                        Err(error) => {
                            let _ = event_sender.send(RuntimeEvent::Fault {
                                generation: None,
                                operation: RuntimeOperation::Startup,
                                detail: format!("could not start screenshot worker: {error}"),
                            });
                        }
                    }
                }
                Err(error) => {
                    let _ = event_sender.send(RuntimeEvent::Fault {
                        generation: None,
                        operation: RuntimeOperation::Startup,
                        detail: error.to_string(),
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

    pub fn test_screenshot(&self, request: ScreenshotRequest) -> Result<(), RuntimeSendError> {
        self.try_send(RuntimeCommand::Screenshot(request))
    }

    pub fn cancel_screenshot(&self, request_id: RuntimeRequestId) -> Result<(), RuntimeSendError> {
        self.try_send(RuntimeCommand::CancelScreenshot { request_id })
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
    factory: Arc<dyn BackendFactory>,
    screenshot_worker: ScreenshotWorker,
    protection: Arc<dyn ProtectionService>,
    live: Option<LiveSession>,
    screenshot: Option<ScreenshotTask>,
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

struct ScreenshotTask {
    request_id: RuntimeRequestId,
    generation: RuntimeGeneration,
    cancelled: Arc<AtomicBool>,
}

enum ScreenshotWorkerCommand {
    Run {
        generation: RuntimeGeneration,
        request_id: RuntimeRequestId,
        path: std::path::PathBuf,
        compiled: Box<CompiledRuntimeSettings>,
        cancelled: Arc<AtomicBool>,
    },
    ReleaseRecognizer {
        acknowledged: mpsc::SyncSender<()>,
    },
    Stop,
}

/// The only screenshot execution thread in a runtime process.
///
/// It owns WIC's COM apartment and the cached native recognizer, preventing
/// every UI screenshot test from creating and destroying an ONNX session,
/// thread stacks and COM/WIC state. The actor still owns cancellation,
/// generation filtering and latest-intent coalescing.
struct ScreenshotWorker {
    commands: mpsc::SyncSender<ScreenshotWorkerCommand>,
}

impl ScreenshotWorker {
    fn start(
        factory: Arc<dyn BackendFactory>,
        events: mpsc::Sender<InternalEvent>,
    ) -> io::Result<Self> {
        let (commands, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("poe-alarm-screenshot".to_owned())
            .spawn(move || {
                let mut backend: Option<Box<dyn ScreenshotBackend>> = None;
                while let Ok(command) = receiver.recv() {
                    match command {
                        ScreenshotWorkerCommand::Run {
                            generation,
                            request_id,
                            path,
                            compiled,
                            cancelled,
                        } => {
                            let result = catch_unwind(AssertUnwindSafe(|| {
                                if backend.is_none() {
                                    backend = Some(factory.create_screenshot_backend()?);
                                }
                                let backend = backend
                                    .as_deref_mut()
                                    .expect("the screenshot backend was initialized above");
                                run_screenshot(
                                    backend, generation, request_id, &path, &compiled, &cancelled,
                                )
                            }));
                            let result = match result {
                                Ok(result) => result,
                                Err(_) => {
                                    // Do not reuse native state across an unwind. A later request
                                    // can rebuild it on this same bounded worker.
                                    backend = None;
                                    Err(BackendError(
                                        "screenshot worker panicked during native recognition"
                                            .to_owned(),
                                    ))
                                }
                            };
                            let _ = events.send(InternalEvent::ScreenshotFinished {
                                generation,
                                request_id,
                                result,
                            });
                        }
                        ScreenshotWorkerCommand::ReleaseRecognizer { acknowledged } => {
                            if let Some(backend) = backend.as_mut() {
                                backend.release_recognizer();
                            }
                            let _ = acknowledged.send(());
                        }
                        ScreenshotWorkerCommand::Stop => break,
                    }
                }
            })?;
        Ok(Self { commands })
    }

    fn submit(&self, command: ScreenshotWorkerCommand) -> Result<(), BackendError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    BackendError("screenshot worker queue is unexpectedly full".to_owned())
                }
                mpsc::TrySendError::Disconnected(_) => {
                    BackendError("screenshot worker is no longer running".to_owned())
                }
            })
    }

    fn stop(&self) {
        let _ = self.commands.try_send(ScreenshotWorkerCommand::Stop);
    }

    fn release_recognizer(&self) -> Result<(), BackendError> {
        let (acknowledged, acknowledgement) = mpsc::sync_channel(1);
        self.commands
            .send(ScreenshotWorkerCommand::ReleaseRecognizer { acknowledged })
            .map_err(|_| BackendError("screenshot worker is no longer running".to_owned()))?;
        acknowledgement
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| BackendError("screenshot recognizer release timed out".to_owned()))
    }
}

impl Drop for ScreenshotWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetiredWork {
    Live {
        generation: RuntimeGeneration,
    },
    Screenshot {
        generation: RuntimeGeneration,
        request_id: RuntimeRequestId,
    },
    /// The OS refused the one cleanup thread. The monitor is deliberately
    /// leaked until process exit and replacement work remains disabled so the
    /// leak cannot repeat without bound.
    CleanupBlocked,
}

enum RuntimeIntent {
    Live(poe_alarm_settings::AppSettings),
    Screenshot(ScreenshotRequest),
}

impl RuntimeActor {
    fn new(
        commands: mpsc::Receiver<RuntimeCommand>,
        events: mpsc::Sender<RuntimeEvent>,
        public_snapshot: Arc<std::sync::Mutex<Option<RuntimeEvent>>>,
        factory: Arc<dyn BackendFactory>,
        protection: Arc<dyn ProtectionService>,
    ) -> io::Result<Self> {
        let (internal_sender, internal_events) = mpsc::channel();
        let screenshot_worker =
            ScreenshotWorker::start(Arc::clone(&factory), internal_sender.clone())?;
        Ok(Self {
            commands,
            events,
            internal_sender,
            internal_events,
            monitor_snapshot: Arc::new(std::sync::Mutex::new(None)),
            public_snapshot,
            factory,
            screenshot_worker,
            protection,
            live: None,
            screenshot: None,
            retired: None,
            pending_intent: None,
            active_generation: None,
            next_generation: 1,
            state: RuntimeState::Idle,
        })
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

    /// Coalesces a burst before allocating a fresh native recognizer. Shutdown
    /// therefore has priority over a queued replacement and cannot get stuck
    /// behind avoidable Paddle/WinRT initialization.
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
            RuntimeCommand::Screenshot(request) => {
                self.replace_intent(RuntimeIntent::Screenshot(request));
                self.retire_active_work(true);
            }
            RuntimeCommand::CancelScreenshot { request_id } => {
                self.cancel_screenshot_task(Some(request_id), true);
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
        debug_assert!(self.screenshot.is_none());
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

        // A live monitor owns its own recognizer. Drop the idle screenshot
        // cache so native model memory remains bounded to one active session.
        if let Err(error) = self.screenshot_worker.release_recognizer() {
            self.fault(Some(generation), RuntimeOperation::Start, error.to_string());
            return;
        }

        let capture = match self.factory.create_capture() {
            Ok(capture) => capture,
            Err(error) => {
                self.fault(Some(generation), RuntimeOperation::Start, error.to_string());
                return;
            }
        };
        let recognizer = match self.factory.create_recognizer(compiled.profile) {
            Ok(recognizer) => recognizer,
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
            region: compiled.region,
            alert_copy: compiled.alert_copy.clone(),
            input_guard_enabled: compiled.input_guard_enabled,
            protection: Arc::clone(&self.protection),
            events: self.internal_sender.clone(),
            latest_snapshot: Arc::clone(&self.monitor_snapshot),
            session_valid: Arc::clone(&valid),
            safety_failed: AtomicBool::new(false),
        };
        let mut monitor = Monitor::new(
            CaptureAdapter(capture),
            RecognizerAdapter(recognizer),
            SystemClock::default(),
            bridge,
        );
        match monitor.start(compiled.plan, compiled.region) {
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

    fn launch_screenshot(&mut self, request: ScreenshotRequest) {
        debug_assert!(self.live.is_none());
        debug_assert!(self.screenshot.is_none());
        debug_assert!(self.retired.is_none());
        let generation = self.allocate_generation();
        self.active_generation = Some(generation);
        self.publish_state(Some(generation), RuntimeState::TestingScreenshot);
        let compiled = match compile_settings(&request.settings) {
            Ok(compiled) => compiled,
            Err(error) => {
                self.fault(
                    Some(generation),
                    RuntimeOperation::Screenshot,
                    error.to_string(),
                );
                return;
            }
        };
        self.publish_compiled(generation, &compiled);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.screenshot = Some(ScreenshotTask {
            request_id: request.request_id,
            generation,
            cancelled: Arc::clone(&cancelled),
        });
        let request_id = request.request_id;
        let path = request.path;
        if let Err(error) = self.screenshot_worker.submit(ScreenshotWorkerCommand::Run {
            generation,
            request_id,
            path,
            compiled: Box::new(compiled),
            cancelled,
        }) {
            self.screenshot = None;
            self.fault(
                Some(generation),
                RuntimeOperation::Screenshot,
                format!("could not start screenshot worker: {error}"),
            );
        }
    }

    fn stop_all(&mut self, acknowledge_alert: bool) {
        self.cancel_pending_intent(true);
        self.cancel_screenshot_task(None, true);
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
        if let Some(RuntimeIntent::Screenshot(request)) = self.pending_intent.replace(intent) {
            let _ = self.events.send(RuntimeEvent::ScreenshotCancelled {
                request_id: request.request_id,
            });
        }
    }

    fn cancel_pending_intent(&mut self, publish: bool) {
        if let Some(RuntimeIntent::Screenshot(request)) = self.pending_intent.take()
            && publish
        {
            let _ = self.events.send(RuntimeEvent::ScreenshotCancelled {
                request_id: request.request_id,
            });
        }
    }

    fn retire_active_work(&mut self, acknowledge_alert: bool) {
        self.cancel_screenshot_task(None, true);
        if self.live.is_some() {
            let _ = self.stop_live(acknowledge_alert);
        }
    }

    fn try_launch_pending(&mut self) {
        if self.live.is_some()
            || self.screenshot.is_some()
            || self.retired.is_some()
            || !self.protection.ready_for_new_work()
        {
            return;
        }
        match self.pending_intent.take() {
            Some(RuntimeIntent::Live(settings)) => self.launch_live(settings),
            Some(RuntimeIntent::Screenshot(request)) => self.launch_screenshot(request),
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

    fn cancel_screenshot_task(
        &mut self,
        expected_request: Option<RuntimeRequestId>,
        publish: bool,
    ) {
        if let Some(RuntimeIntent::Screenshot(request)) = self.pending_intent.as_ref()
            && expected_request.is_some_and(|expected| expected == request.request_id)
        {
            let request_id = request.request_id;
            self.pending_intent = None;
            if publish {
                let _ = self
                    .events
                    .send(RuntimeEvent::ScreenshotCancelled { request_id });
            }
            return;
        }
        let Some(task) = self.screenshot.as_ref() else {
            return;
        };
        if expected_request.is_some_and(|expected| expected != task.request_id) {
            return;
        }
        let task = self.screenshot.take().expect("task was just observed");
        task.cancelled.store(true, Ordering::Release);
        if self.active_generation == Some(task.generation) {
            self.active_generation = None;
        }
        if self.retired.is_some() {
            self.retired = Some(RetiredWork::CleanupBlocked);
            self.fault(
                Some(task.generation),
                RuntimeOperation::Screenshot,
                "runtime cleanup capacity was already occupied; replacement work is disabled until process exit"
                    .to_owned(),
            );
        } else {
            self.retired = Some(RetiredWork::Screenshot {
                generation: task.generation,
                request_id: task.request_id,
            });
        }
        if publish {
            let _ = self.events.send(RuntimeEvent::ScreenshotCancelled {
                request_id: task.request_id,
            });
        }
    }

    fn shutdown(&mut self) {
        let started = Instant::now();
        self.publish_state(self.active_generation, RuntimeState::ShuttingDown);
        self.cancel_pending_intent(false);
        self.cancel_screenshot_task(None, false);
        self.drain_live_for_shutdown();
        self.screenshot_worker.stop();
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
    /// possibly in-flight native OCR call finish outside the actor. Paddle's
    /// public production boundary observes cancellation before/after ONNX but
    /// cannot interrupt an inference already inside the native runtime.
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
                        "could not start OCR cleanup worker; resources will be reclaimed at process exit: {error}"
                    ),
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
                InternalEvent::ScreenshotFinished {
                    generation,
                    request_id,
                    ..
                } if self.retired
                    == Some(RetiredWork::Screenshot {
                        generation,
                        request_id,
                    }) =>
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
                InternalEvent::ScreenshotFinished {
                    generation,
                    request_id,
                    result,
                } if self.active_generation == Some(generation)
                    && self.screenshot.as_ref().is_some_and(|task| {
                        task.generation == generation && task.request_id == request_id
                    }) =>
                {
                    self.screenshot = None;
                    match result {
                        Ok(report) => {
                            let _ = self.events.send(RuntimeEvent::ScreenshotCompleted(report));
                            self.active_generation = None;
                            self.publish_state(None, RuntimeState::Idle);
                        }
                        Err(error) => {
                            self.active_generation = None;
                            self.fault(
                                Some(generation),
                                RuntimeOperation::Screenshot,
                                error.to_string(),
                            );
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
            self.fault(
                Some(generation),
                RuntimeOperation::Monitoring,
                snapshot
                    .detail
                    .clone()
                    .unwrap_or_else(|| "monitoring failed".to_owned()),
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
            profile: compiled.profile,
            region: compiled.region,
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
        self.state = RuntimeState::Faulted;
        let _ = self.events.send(RuntimeEvent::Fault {
            generation,
            operation,
            detail,
        });
    }
}

enum InternalEvent {
    Monitor {
        generation: RuntimeGeneration,
        event: BridgeMonitorEvent,
    },
    ScreenshotFinished {
        generation: RuntimeGeneration,
        request_id: RuntimeRequestId,
        result: Result<ScreenshotReport, BackendError>,
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
    region: CaptureRegion,
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
                    anchor_region: self.region,
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

fn run_screenshot(
    backend: &mut dyn ScreenshotBackend,
    generation: RuntimeGeneration,
    request_id: RuntimeRequestId,
    path: &std::path::Path,
    compiled: &CompiledRuntimeSettings,
    cancelled: &AtomicBool,
) -> Result<ScreenshotReport, BackendError> {
    ensure_not_cancelled(cancelled)?;
    let load_started = Instant::now();
    let full_frame = backend.decode(path)?;
    let (frame, used_full_image_fallback) = crop_or_full(full_frame, compiled.region)?;
    let load_elapsed = load_started.elapsed();
    ensure_not_cancelled(cancelled)?;

    let mask_started = Instant::now();
    let blue_mask = build_blue_mask(&frame, BlueMaskSettings::default());
    let mask_elapsed = mask_started.elapsed();
    backend.begin_request();
    let mut preprocessing_elapsed = mask_elapsed;
    let mut recognition_elapsed = Duration::ZERO;
    let mut final_result = None;
    for _ in 0..MAXIMUM_SCREENSHOT_PASSES {
        ensure_not_cancelled(cancelled)?;
        let prepared = PreparedFrame {
            frame: &frame,
            blue_mask: &blue_mask,
            semantic_fingerprint: blue_mask.fingerprint(),
        };
        let result = match &compiled.plan {
            MonitorPlan::Quick(target) => {
                backend.recognize_quick(compiled.profile, prepared, target, cancelled)?
            }
            MonitorPlan::Structured(rules) => backend.recognize_structured(
                compiled.profile,
                prepared,
                rules.targets(),
                cancelled,
            )?,
        };
        preprocessing_elapsed = preprocessing_elapsed.saturating_add(result.preprocessing_elapsed);
        recognition_elapsed = recognition_elapsed.saturating_add(result.recognition_elapsed);
        let requires_rescan = result.requires_rescan;
        final_result = Some(result);
        if !requires_rescan {
            break;
        }
    }
    let result = final_result.expect("at least one screenshot recognition pass runs");
    if result.requires_rescan {
        return Err(BackendError(format!(
            "screenshot recognition did not converge after {MAXIMUM_SCREENSHOT_PASSES} passes"
        )));
    }
    ensure_not_cancelled(cancelled)?;
    let evaluation_started = Instant::now();
    let evaluation = evaluate_screenshot(&compiled.plan, &result);
    let evaluation_elapsed = evaluation_started.elapsed();
    ensure_not_cancelled(cancelled)?;

    Ok(ScreenshotReport {
        request_id,
        generation,
        profile: compiled.profile,
        requested_region: compiled.region,
        used_full_image_fallback,
        lines: result.lines,
        load_elapsed,
        preprocessing_elapsed,
        recognition_elapsed,
        evaluation_elapsed,
        evaluation,
    })
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<(), BackendError> {
    if cancelled.load(Ordering::Acquire) {
        Err(BackendError(
            "screenshot recognition was cancelled".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn crop_or_full(
    full: CapturedFrame,
    requested: CaptureRegion,
) -> Result<(CapturedFrame, bool), BackendError> {
    let valid = requested.x >= 0
        && requested.y >= 0
        && (requested.x as u32) < full.width() as u32
        && (requested.y as u32) < full.height() as u32
        && requested.width <= full.width() as u32 - requested.x as u32
        && requested.height <= full.height() as u32 - requested.y as u32;
    if !valid {
        return Ok((full, true));
    }
    if requested.x == 0
        && requested.y == 0
        && requested.width as usize == full.width()
        && requested.height as usize == full.height()
    {
        return Ok((full, false));
    }

    let stride = requested.width as usize * poe_alarm_vision::BYTES_PER_PIXEL;
    let mut pixels = vec![0_u8; stride * requested.height as usize];
    let source_stride = full.stride();
    let source_x = requested.x as usize * poe_alarm_vision::BYTES_PER_PIXEL;
    for row in 0..requested.height as usize {
        let source_start = (requested.y as usize + row) * source_stride + source_x;
        let destination_start = row * stride;
        pixels[destination_start..destination_start + stride]
            .copy_from_slice(&full.bgra_pixels()[source_start..source_start + stride]);
    }
    CapturedFrame::from_bgra(requested, stride, pixels, SystemTime::now())
        .map(|frame| (frame, false))
        .map_err(|error| BackendError(error.to_string()))
}

fn evaluate_screenshot(plan: &MonitorPlan, result: &RecognitionResult) -> ScreenshotEvaluation {
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
            ScreenshotEvaluation {
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

fn quick_evaluation(matched: Option<LogicalAffixMatch>) -> ScreenshotEvaluation {
    ScreenshotEvaluation {
        is_match: matched.is_some(),
        detail: matched.map(|value| value.original_text),
        matched_group: None,
    }
}
