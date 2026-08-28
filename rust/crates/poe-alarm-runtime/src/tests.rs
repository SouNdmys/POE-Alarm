//! Actor contracts: generation filtering, alert ownership, bounded shutdown.
//!
//! What is exercised here is the actor, not recognition — the monitor's own
//! loop has its contract test in `poe-alarm-monitoring`. The fake source exists
//! so those actor contracts can be driven without a running client: it can be
//! told to match, to block until cancelled, to ignore cancellation, or to fail
//! to construct at all.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use poe_alarm_core::{AcceptableResultGroup, AffixCondition, ResultGroupMode, RuleSetDefinition};
use poe_alarm_monitoring::{
    AffixSource, CancellationToken, MonitorPlan, RecognitionResult, StructuredOcrSupport,
};
use poe_alarm_settings::{AppSettings, RuleEditorMode};

use crate::{
    AffixSourceFactory, AlertLatchStatus, AlertPresentation, BackendError, BoxedAffixSource,
    ItemCheckRequest, ProtectionError, ProtectionEvent, ProtectionService, RuntimeEvent,
    RuntimeGeneration, RuntimeHandle, RuntimeOperation, RuntimeRequestId, RuntimeSendError,
    RuntimeState, SourceError,
};

#[derive(Clone)]
struct FakeSources {
    state: Arc<FakeSourceState>,
}

struct FakeSourceState {
    readings: Mutex<VecDeque<RecognitionResult>>,
    reads: AtomicUsize,
    sources_alive: AtomicUsize,
    sources_peak: AtomicUsize,
    sources_created: AtomicUsize,
    /// 0 unsupported, 2 confirmed strict batch, anything else strict batch.
    structured_support: AtomicUsize,
    block_until_cancelled: AtomicBool,
    block_ignoring_cancellation: AtomicBool,
    fail_create: AtomicBool,
}

impl FakeSources {
    fn new(readings: Vec<RecognitionResult>) -> Self {
        Self {
            state: Arc::new(FakeSourceState {
                readings: Mutex::new(readings.into()),
                reads: AtomicUsize::new(0),
                sources_alive: AtomicUsize::new(0),
                sources_peak: AtomicUsize::new(0),
                sources_created: AtomicUsize::new(0),
                structured_support: AtomicUsize::new(1),
                block_until_cancelled: AtomicBool::new(false),
                block_ignoring_cancellation: AtomicBool::new(false),
                fail_create: AtomicBool::new(false),
            }),
        }
    }
}

impl AffixSourceFactory for FakeSources {
    fn create(&self) -> Result<BoxedAffixSource, BackendError> {
        if self.state.fail_create.load(Ordering::Acquire) {
            return Err(BackendError("injected source failure".to_owned()));
        }
        self.state.sources_created.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(FakeSource::new(Arc::clone(&self.state))))
    }
}

struct FakeSource {
    state: Arc<FakeSourceState>,
}

impl FakeSource {
    fn new(state: Arc<FakeSourceState>) -> Self {
        let alive = state.sources_alive.fetch_add(1, Ordering::AcqRel) + 1;
        state.sources_peak.fetch_max(alive, Ordering::AcqRel);
        Self { state }
    }
}

impl Drop for FakeSource {
    fn drop(&mut self) {
        self.state.sources_alive.fetch_sub(1, Ordering::AcqRel);
    }
}

impl AffixSource for FakeSource {
    type Error = SourceError;

    fn structured_support(&self) -> StructuredOcrSupport {
        match self.state.structured_support.load(Ordering::Acquire) {
            0 => StructuredOcrSupport::Unsupported,
            2 => StructuredOcrSupport::ConfirmedStrictBatch,
            _ => StructuredOcrSupport::StrictBatch,
        }
    }

    fn read(
        &mut self,
        _plan: &MonitorPlan,
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, Self::Error> {
        self.state.reads.fetch_add(1, Ordering::AcqRel);
        if self.state.block_until_cancelled.load(Ordering::Acquire) {
            cancellation.wait_cancelled();
        }
        while self
            .state
            .block_ignoring_cancellation
            .load(Ordering::Acquire)
        {
            thread::sleep(Duration::from_millis(1));
        }
        // Running out of scripted readings means the item stopped changing,
        // which is what a real source reports as an unchanged reading rather
        // than as an empty item.
        Ok(self
            .state
            .readings
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(RecognitionResult {
                was_cached: true,
                ..RecognitionResult::default()
            }))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SafetyCall {
    Begin(RuntimeGeneration),
    Prepare(RuntimeGeneration),
    Arm(RuntimeGeneration),
    Release(RuntimeGeneration),
    StopPending(RuntimeGeneration),
    Latch(RuntimeGeneration),
    Ack,
    Shutdown,
}

#[derive(Default)]
struct FakeProtection {
    calls: Mutex<Vec<SafetyCall>>,
    fail_latch: AtomicBool,
    /// When set, latches succeed but report the click-block hook as failed.
    unguarded_reason: Mutex<Option<String>>,
    events: Mutex<VecDeque<ProtectionEvent>>,
    active_generation: Mutex<Option<RuntimeGeneration>>,
}

impl ProtectionService for FakeProtection {
    fn ready_for_new_work(&self) -> bool {
        self.active_generation.lock().unwrap().is_none()
    }

    fn begin_session(&self, generation: RuntimeGeneration) -> Result<(), ProtectionError> {
        *self.active_generation.lock().unwrap() = Some(generation);
        self.calls
            .lock()
            .unwrap()
            .push(SafetyCall::Begin(generation));
        Ok(())
    }

    fn prepare_pending(&self, generation: RuntimeGeneration) -> Result<(), ProtectionError> {
        self.calls
            .lock()
            .unwrap()
            .push(SafetyCall::Prepare(generation));
        Ok(())
    }

    fn arm_pending(&self, generation: RuntimeGeneration) -> Result<(), ProtectionError> {
        self.calls.lock().unwrap().push(SafetyCall::Arm(generation));
        Ok(())
    }

    fn release_pending(&self, generation: RuntimeGeneration) {
        self.calls
            .lock()
            .unwrap()
            .push(SafetyCall::Release(generation));
    }

    fn stop_pending(&self, generation: RuntimeGeneration) {
        let mut active = self.active_generation.lock().unwrap();
        if *active == Some(generation) {
            *active = None;
        }
        drop(active);
        self.calls
            .lock()
            .unwrap()
            .push(SafetyCall::StopPending(generation));
    }

    fn latch_red_alert(
        &self,
        generation: RuntimeGeneration,
        _presentation: AlertPresentation,
    ) -> Result<AlertLatchStatus, ProtectionError> {
        self.calls
            .lock()
            .unwrap()
            .push(SafetyCall::Latch(generation));
        if *self.active_generation.lock().unwrap() != Some(generation) {
            return Err(ProtectionError("stale session".to_owned()));
        }
        if self.fail_latch.load(Ordering::Acquire) {
            Err(ProtectionError("injected alert failure".to_owned()))
        } else {
            Ok(AlertLatchStatus::Accepted {
                unguarded: self.unguarded_reason.lock().unwrap().clone(),
            })
        }
    }

    fn acknowledge(&self) -> Result<(), ProtectionError> {
        self.calls.lock().unwrap().push(SafetyCall::Ack);
        Ok(())
    }

    fn poll_events(&self) -> Vec<ProtectionEvent> {
        self.events.lock().unwrap().drain(..).collect()
    }

    fn shutdown_fail_open(&self) -> Result<(), ProtectionError> {
        self.calls.lock().unwrap().push(SafetyCall::Shutdown);
        Ok(())
    }
}

fn quick_settings() -> AppSettings {
    let mut settings = AppSettings::default();
    settings.profiles.poe1.selected_rules_mut().target_affix =
        "+#% to Critical Hit Chance".to_owned();
    settings
}

fn poe2_quick_settings() -> AppSettings {
    let mut settings = quick_settings();
    settings.selected_game_profile = poe_alarm_settings::GameProfile::Poe2;
    settings.profiles.poe2 = settings.profiles.poe1.clone();
    settings
}

fn structured_settings() -> AppSettings {
    let mut settings = quick_settings();
    settings.profiles.poe1.selected_rules_mut().rule_editor_mode = RuleEditorMode::Structured;
    settings
        .profiles
        .poe1
        .selected_rules_mut()
        .structured_rule_set = Some(RuleSetDefinition {
        schema_version: 1,
        name: "results".to_owned(),
        groups: vec![AcceptableResultGroup {
            name: "critical".to_owned(),
            conditions: vec![AffixCondition::new(
                "chance",
                "+#% to Critical Hit Chance",
                Vec::new(),
            )],
            ..AcceptableResultGroup::default()
        }],
    });
    settings
}

fn at_least_two_settings() -> AppSettings {
    let mut settings = quick_settings();
    settings.profiles.poe1.selected_rules_mut().rule_editor_mode = RuleEditorMode::Structured;
    settings
        .profiles
        .poe1
        .selected_rules_mut()
        .structured_rule_set = Some(RuleSetDefinition {
        schema_version: 1,
        name: "results".to_owned(),
        groups: vec![AcceptableResultGroup {
            name: "two critical modifiers".to_owned(),
            mode: ResultGroupMode::AtLeast,
            required_count: 2,
            conditions: vec![
                AffixCondition::new("chance", "+#% to Critical Hit Chance", Vec::new()),
                AffixCondition::new(
                    "multiplier",
                    "+#% to Critical Strike Multiplier",
                    Vec::new(),
                ),
            ],
        }],
    });
    settings
}

fn poe2_structured_settings() -> AppSettings {
    let mut settings = structured_settings();
    settings.selected_game_profile = poe_alarm_settings::GameProfile::Poe2;
    settings.profiles.poe2 = settings.profiles.poe1.clone();
    settings
}

fn result(lines: &[&str]) -> RecognitionResult {
    RecognitionResult {
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
        ..RecognitionResult::default()
    }
}

fn runtime(sources: &FakeSources, protection: &Arc<FakeProtection>) -> RuntimeHandle {
    RuntimeHandle::spawn_with_components(
        Arc::new(sources.clone()),
        Arc::clone(protection) as Arc<dyn ProtectionService>,
    )
    .unwrap()
}

fn wait_for(
    handle: &RuntimeHandle,
    timeout: Duration,
    predicate: impl Fn(&RuntimeEvent) -> bool,
) -> RuntimeEvent {
    let deadline = Instant::now() + timeout;
    loop {
        while let Some(event) = handle.try_next_event() {
            if predicate(&event) {
                return event;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for runtime event"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn wait_until(condition: impl Fn() -> bool, detail: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {detail}");
        thread::sleep(Duration::from_millis(2));
    }
}

fn shutdown(handle: &RuntimeHandle) {
    handle.shutdown().unwrap();
    let event = wait_for(handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::ShutdownComplete { .. })
    });
    assert!(matches!(
        event,
        RuntimeEvent::ShutdownComplete {
            within_one_second: true,
            ..
        }
    ));
}

fn retry_queue(mut submit: impl FnMut() -> Result<(), RuntimeSendError>) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match submit() {
            Ok(()) => return,
            Err(RuntimeSendError::QueueFull) => {
                assert!(Instant::now() < deadline, "runtime queue stayed full");
                thread::yield_now();
            }
            Err(RuntimeSendError::RuntimeStopped) => panic!("runtime stopped during stress input"),
        }
    }
}

fn soak_iterations(variable: &str, default: usize) -> usize {
    match std::env::var(variable) {
        Ok(value) => {
            let parsed = value
                .parse::<usize>()
                .unwrap_or_else(|error| panic!("{variable} must be a positive integer: {error}"));
            assert!(parsed > 0, "{variable} must be greater than zero");
            parsed
        }
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => panic!("could not read {variable}: {error}"),
    }
}

fn wait_for_without_late_result(
    handle: &RuntimeHandle,
    timeout: Duration,
    predicate: impl Fn(&RuntimeEvent) -> bool,
) -> RuntimeEvent {
    let deadline = Instant::now() + timeout;
    loop {
        while let Some(event) = handle.try_next_event() {
            assert!(
                !matches!(
                    event,
                    RuntimeEvent::MatchFound { .. }
                        | RuntimeEvent::ItemCheckCompleted(_)
                        | RuntimeEvent::Fault { .. }
                ),
                "cancelled work published a late terminal event: {event:?}"
            );
            if predicate(&event) {
                return event;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for runtime soak event"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

/// One rare jewel whose two critical modifiers come from a single prefix.
///
/// The client says so itself: both lines sit under one `{ Prefix Modifier }`
/// annotation, which is its authoritative statement that one physical modifier
/// produced them.
const ONE_HYBRID_PREFIX: &str = concat!(
    "Item Class: Jewels\r\n",
    "Rarity: Rare\r\n",
    "Rage Bliss\r\n",
    "Cobalt Jewel\r\n",
    "--------\r\n",
    "Item Level: 84\r\n",
    "--------\r\n",
    "{ Prefix Modifier \"Deadly\" (Tier: 1) — Critical }\r\n",
    "+3.7(3.0-4.0)% to Critical Hit Chance\r\n",
    "+25(20-30)% to Critical Strike Multiplier\r\n",
);

/// The same two lines, but rolled as two independent modifiers.
const TWO_SEPARATE_AFFIXES: &str = concat!(
    "Item Class: Jewels\r\n",
    "Rarity: Rare\r\n",
    "Rage Bliss\r\n",
    "Cobalt Jewel\r\n",
    "--------\r\n",
    "Item Level: 84\r\n",
    "--------\r\n",
    "{ Prefix Modifier \"Deadly\" (Tier: 1) — Critical }\r\n",
    "+3.7(3.0-4.0)% to Critical Hit Chance\r\n",
    "{ Suffix Modifier \"of Menace\" (Tier: 1) — Critical }\r\n",
    "+25(20-30)% to Critical Strike Multiplier\r\n",
);

#[test]
fn cached_readings_keep_the_session_running_without_a_pending_guard() {
    let sources = FakeSources::new(vec![result(&["not it"])]);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(quick_settings()).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::MonitorSnapshot { snapshot, .. }
                if snapshot.scan_count >= 2 && snapshot.ocr_was_cached
        )
    });
    let calls = protection.calls.lock().unwrap().clone();
    assert!(
        !calls.iter().any(|call| matches!(
            call,
            SafetyCall::Prepare(_) | SafetyCall::Arm(_) | SafetyCall::Release(_)
        )),
        "POE1 English must retain the 1.0 no-short-guard behavior: {calls:?}"
    );
    shutdown(&handle);
}

#[test]
fn fast_match_latches_alert_without_a_pending_mouse_guard() {
    let sources = FakeSources::new(vec![result(&["+3.73% to Critical Hit Chance"])]);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(poe2_quick_settings()).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::MatchFound { .. })
    });
    let calls = protection.calls.lock().unwrap().clone();
    let latch = calls
        .iter()
        .position(|call| matches!(call, SafetyCall::Latch(_)))
        .unwrap();
    let begin = calls
        .iter()
        .position(|call| matches!(call, SafetyCall::Begin(_)))
        .unwrap();
    assert!(begin < latch, "calls: {calls:?}");
    assert!(
        !calls.iter().any(|call| matches!(
            call,
            SafetyCall::Prepare(_) | SafetyCall::Arm(_) | SafetyCall::Release(_)
        )),
        "fast mode must not touch the pending mouse guard: {calls:?}"
    );
    shutdown(&handle);
}

#[test]
fn stop_cancels_a_blocked_reading_without_a_late_match() {
    let sources = FakeSources::new(vec![result(&["+3.73% to Critical Hit Chance"])]);
    sources
        .state
        .block_until_cancelled
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(poe2_quick_settings()).unwrap();
    wait_until(
        || sources.state.reads.load(Ordering::Acquire) > 0,
        "the source to be entered",
    );
    handle.stop().unwrap();
    wait_until(
        || {
            protection
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| matches!(call, SafetyCall::StopPending(_)))
        },
        "alert ownership to be closed",
    );
    let calls = protection.calls.lock().unwrap().clone();
    let terminal_stop = calls
        .iter()
        .rposition(|call| matches!(call, SafetyCall::StopPending(_)))
        .expect("runtime stop still closes the alert ownership session");
    assert!(
        !calls[..terminal_stop].iter().any(|call| matches!(
            call,
            SafetyCall::Prepare(_) | SafetyCall::Arm(_) | SafetyCall::Release(_)
        )),
        "stopping fast mode must not reveal a hidden pending guard: {calls:?}"
    );
    thread::sleep(Duration::from_millis(30));
    while let Some(event) = handle.try_next_event() {
        assert!(!matches!(event, RuntimeEvent::MatchFound { .. }));
    }
    shutdown(&handle);
}

#[test]
fn alert_failure_emits_explicit_fault_without_a_pending_guard() {
    let sources = FakeSources::new(vec![result(&["+3.73% to Critical Hit Chance"])]);
    let protection = Arc::new(FakeProtection::default());
    protection.fail_latch.store(true, Ordering::Release);
    let handle = runtime(&sources, &protection);
    handle.start(poe2_quick_settings()).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::Fault {
                operation: RuntimeOperation::Alert,
                ..
            }
        )
    });
    let calls = protection.calls.lock().unwrap().clone();
    assert!(!calls.iter().any(|call| matches!(
        call,
        SafetyCall::Prepare(_) | SafetyCall::Arm(_) | SafetyCall::Release(_)
    )));
    shutdown(&handle);
}

#[test]
fn a_source_that_cannot_be_created_faults_without_opening_a_session() {
    let sources = FakeSources::new(Vec::new());
    sources.state.fail_create.store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(quick_settings()).unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::Fault {
                operation: RuntimeOperation::Start,
                ..
            }
        )
    });
    let RuntimeEvent::Fault { detail, .. } = event else {
        unreachable!()
    };
    assert!(detail.contains("injected source failure"), "{detail}");
    let calls = protection.calls.lock().unwrap().clone();
    assert!(
        !calls
            .iter()
            .any(|call| matches!(call, SafetyCall::Begin(_) | SafetyCall::Prepare(_))),
        "a source that never started must not open an alert session: {calls:?}"
    );
    shutdown(&handle);
}

#[test]
fn sound_failure_is_non_fatal_while_red_alert_remains_latched() {
    let sources = FakeSources::new(vec![result(&["+3.73% to Critical Hit Chance"])]);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(quick_settings()).unwrap();
    let match_event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::MatchFound { .. })
    });
    let RuntimeEvent::MatchFound { generation, .. } = match_event else {
        unreachable!()
    };
    protection
        .events
        .lock()
        .unwrap()
        .push_back(ProtectionEvent::SoundFailed {
            generation,
            detail: "injected sound failure".to_owned(),
        });
    let sound_event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::AlertSoundFailed { .. })
    });
    assert!(matches!(
        sound_event,
        RuntimeEvent::AlertSoundFailed {
            generation: current,
            ref detail,
        } if current == generation && detail.contains("sound failure")
    ));
    assert!(
        !protection
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| matches!(call, SafetyCall::StopPending(_))),
        "audio failure must not tear down the still-safe red alert"
    );
    shutdown(&handle);
}

#[test]
fn poe2_fast_monitoring_never_prepares_the_dormant_guard() {
    let sources = FakeSources::new(vec![result(&["not it"])]);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(poe2_quick_settings()).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::StateChanged {
                state: RuntimeState::Monitoring,
                ..
            }
        )
    });
    assert!(
        !protection.calls.lock().unwrap().iter().any(|call| matches!(
            call,
            SafetyCall::Prepare(_) | SafetyCall::Arm(_) | SafetyCall::Release(_)
        )),
        "POE2 fast monitoring must not prepare the dormant pending guard"
    );
    shutdown(&handle);
}

#[test]
fn monitor_start_failure_does_not_prepare_the_dormant_guard() {
    let sources = FakeSources::new(vec![result(&["not it"])]);
    sources.state.structured_support.store(0, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(poe2_structured_settings()).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::Fault {
                operation: RuntimeOperation::Start,
                ..
            }
        )
    });
    let calls = protection.calls.lock().unwrap().clone();
    let stop = calls
        .iter()
        .position(|call| matches!(call, SafetyCall::StopPending(_)))
        .unwrap();
    assert!(
        !calls[..stop].iter().any(|call| matches!(
            call,
            SafetyCall::Prepare(_) | SafetyCall::Arm(_) | SafetyCall::Release(_)
        )),
        "calls: {calls:?}"
    );
    assert_eq!(sources.state.reads.load(Ordering::Acquire), 0);
    shutdown(&handle);
}

#[test]
fn one_hybrid_modifier_cannot_satisfy_two_conditions() {
    let sources = FakeSources::new(Vec::new());
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle
        .check_item(ItemCheckRequest::new(
            RuntimeRequestId(70),
            at_least_two_settings(),
            ONE_HYBRID_PREFIX,
        ))
        .unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::ItemCheckCompleted(_))
    });
    let RuntimeEvent::ItemCheckCompleted(report) = event else {
        unreachable!()
    };
    assert_eq!(report.request_id, RuntimeRequestId(70));
    assert_eq!(
        report.modifier_count, 1,
        "the client annotated both lines as one prefix: {:?}",
        report.lines
    );
    assert!(
        !report.evaluation.is_match,
        "one physical modifier must not satisfy two conditions: {:?}",
        report.lines
    );
    assert_eq!(report.evaluation.matched_group, None);
    shutdown(&handle);
}

#[test]
fn two_separate_modifiers_do_satisfy_two_conditions() {
    let sources = FakeSources::new(Vec::new());
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle
        .check_item(ItemCheckRequest::new(
            RuntimeRequestId(71),
            at_least_two_settings(),
            TWO_SEPARATE_AFFIXES,
        ))
        .unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::ItemCheckCompleted(_))
    });
    let RuntimeEvent::ItemCheckCompleted(report) = event else {
        unreachable!()
    };
    assert_eq!(report.modifier_count, 2);
    assert!(
        report.evaluation.is_match,
        "two independent modifiers must match: {:?}",
        report.lines
    );
    assert_eq!(
        report.evaluation.matched_group.as_deref(),
        Some("two critical modifiers")
    );
    // A blank entry between the groups is what stops the engine joining them.
    assert!(
        report.lines.iter().any(|line| line.is_empty()),
        "grouping must survive into the report: {:?}",
        report.lines
    );
    shutdown(&handle);
}

#[test]
fn text_that_is_not_an_item_faults_rather_than_reporting_a_miss() {
    let sources = FakeSources::new(Vec::new());
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle
        .check_item(ItemCheckRequest::new(
            RuntimeRequestId(72),
            structured_settings(),
            "whatever happened to be on the clipboard",
        ))
        .unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::Fault {
                operation: RuntimeOperation::ItemCheck,
                ..
            }
        )
    });
    shutdown(&handle);
}

#[test]
fn an_item_check_queued_behind_retiring_work_can_be_cancelled() {
    let sources = FakeSources::new(vec![result(&["not it"])]);
    sources
        .state
        .block_ignoring_cancellation
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(quick_settings()).unwrap();
    wait_until(
        || sources.state.reads.load(Ordering::Acquire) > 0,
        "the source to be entered",
    );

    let request_id = RuntimeRequestId(73);
    handle
        .check_item(ItemCheckRequest::new(
            request_id,
            structured_settings(),
            TWO_SEPARATE_AFFIXES,
        ))
        .unwrap();
    handle.cancel_item_check(request_id).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::ItemCheckCancelled { request_id: id } if *id == request_id
        )
    });

    sources
        .state
        .block_ignoring_cancellation
        .store(false, Ordering::Release);
    thread::sleep(Duration::from_millis(30));
    while let Some(event) = handle.try_next_event() {
        assert!(!matches!(event, RuntimeEvent::ItemCheckCompleted(_)));
    }
    shutdown(&handle);
}

#[test]
fn stale_session_cannot_publish_detection_after_replacement() {
    let sources = FakeSources::new(vec![
        result(&["+3.73% to Critical Hit Chance"]),
        result(&["not it"]),
    ]);
    sources
        .state
        .block_until_cancelled
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(quick_settings()).unwrap();
    wait_until(
        || sources.state.reads.load(Ordering::Acquire) > 0,
        "the source to be entered",
    );
    sources
        .state
        .block_until_cancelled
        .store(false, Ordering::Release);
    handle.start(quick_settings()).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::StateChanged {
                generation: Some(RuntimeGeneration(2)),
                state: RuntimeState::Monitoring,
            }
        )
    });
    thread::sleep(Duration::from_millis(30));
    while let Some(event) = handle.try_next_event() {
        assert!(!matches!(
            event,
            RuntimeEvent::MatchFound {
                generation: RuntimeGeneration(1),
                ..
            }
        ));
    }
    shutdown(&handle);
}

#[test]
fn shutdown_is_bounded_when_a_reading_ignores_cancellation_in_flight() {
    let sources = FakeSources::new(vec![result(&["+3.73% to Critical Hit Chance"])]);
    sources
        .state
        .block_ignoring_cancellation
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(poe2_quick_settings()).unwrap();
    wait_until(
        || sources.state.reads.load(Ordering::Acquire) > 0,
        "the source to be entered",
    );

    let shutdown_started = Instant::now();
    handle.shutdown().unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::ShutdownComplete { .. })
    });
    assert!(shutdown_started.elapsed() < Duration::from_secs(1));
    assert!(matches!(
        event,
        RuntimeEvent::ShutdownComplete {
            within_one_second: true,
            ..
        }
    ));
    assert!(
        protection
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| matches!(call, SafetyCall::StopPending(_))),
        "input protection must be terminally removed before the detached drain"
    );

    sources
        .state
        .block_ignoring_cancellation
        .store(false, Ordering::Release);
    thread::sleep(Duration::from_millis(30));
    while let Some(event) = handle.try_next_event() {
        assert!(!matches!(event, RuntimeEvent::MatchFound { .. }));
    }
}

#[test]
#[ignore = "release-gate soak; set POE_ALARM_RUNTIME_START_STOP_SOAK_CYCLES to override 1000"]
fn one_thousand_start_stop_cycles_keep_runtime_resources_bounded() {
    let cycles = soak_iterations("POE_ALARM_RUNTIME_START_STOP_SOAK_CYCLES", 1_000);
    let matching = result(&["+3.73% to Critical Hit Chance"]);
    let sources = FakeSources::new(vec![matching; cycles]);
    sources
        .state
        .block_until_cancelled
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);

    for cycle in 0..cycles {
        let generation = RuntimeGeneration(cycle as u64 + 1);
        retry_queue(|| handle.start(quick_settings()));
        wait_for_without_late_result(&handle, Duration::from_secs(2), |event| {
            matches!(
                event,
                RuntimeEvent::StateChanged {
                    generation: Some(current),
                    state: RuntimeState::Monitoring,
                } if *current == generation
            )
        });
        wait_until(
            || sources.state.reads.load(Ordering::Acquire) > cycle,
            "the cycle to enter the source",
        );

        retry_queue(|| handle.stop());
        wait_for_without_late_result(&handle, Duration::from_secs(2), |event| {
            matches!(
                event,
                RuntimeEvent::StateChanged {
                    generation: None,
                    state: RuntimeState::Idle,
                }
            )
        });
        wait_until(
            || sources.state.sources_alive.load(Ordering::Acquire) == 0,
            "the cycle to release its source",
        );
    }

    assert_eq!(
        sources.state.sources_created.load(Ordering::Acquire),
        cycles
    );
    assert_eq!(sources.state.sources_alive.load(Ordering::Acquire), 0);
    assert_eq!(sources.state.sources_peak.load(Ordering::Acquire), 1);
    assert_eq!(*protection.active_generation.lock().unwrap(), None);

    thread::sleep(Duration::from_millis(30));
    while let Some(event) = handle.try_next_event() {
        assert!(
            !matches!(
                event,
                RuntimeEvent::MatchFound { .. }
                    | RuntimeEvent::ItemCheckCompleted(_)
                    | RuntimeEvent::Fault { .. }
            ),
            "cancelled live work published after the soak: {event:?}"
        );
    }

    let shutdown_started = Instant::now();
    shutdown(&handle);
    assert!(
        shutdown_started.elapsed() < Duration::from_secs(1),
        "runtime shutdown exceeded one second after {cycles} cycles"
    );
}

#[cfg(windows)]
#[test]
fn production_constructor_and_source_factory_are_compile_checked() {
    let _constructor: fn(crate::ProductionRuntimeConfig) -> std::io::Result<RuntimeHandle> =
        RuntimeHandle::start_production;
    let _factory: Arc<dyn AffixSourceFactory> = Arc::new(crate::ProductionSourceFactory);
}

/// The alert is the one screen a user reads under time pressure, and for a long
/// while two of its five lines were painted from literals in the window code
/// rather than from the caller-supplied copy — so an English user got English
/// chrome around Chinese instructions. Asserting on the alphabet rather than on
/// the wording catches the next line someone forgets to route through here,
/// which is what actually went wrong.
#[test]
fn the_english_alert_carries_no_chinese() {
    let copy = crate::AlertCopy::for_ui_language("en");
    for (field, value) in [
        ("title", &copy.title),
        ("button", &copy.button),
        ("notice", &copy.notice),
        ("notice_unguarded", &copy.notice_unguarded),
        ("footer", &copy.footer),
    ] {
        assert!(
            !value.is_empty(),
            "the English alert leaves {field} empty, and the window refuses to draw it"
        );
        if let Some(offender) = value.chars().find(|c| {
            // Typographic marks the footer legitimately uses are not letters.
            !c.is_ascii() && !matches!(c, '\u{b7}' | '\u{21e7}' | '\u{2014}' | '\u{2019}')
        }) {
            panic!("the English alert's {field} contains {offender:?}: {value}");
        }
    }
}

/// A guard failure used to be discarded inside the latch, which made a dead
/// hook indistinguishable from a lost timing race — the two need opposite
/// responses, and the user can only tell them apart if the reason survives
/// the trip. The actor is the sole courier between the latch status and the
/// UI event, so this pins that it copies the reason onto the detection
/// instead of flattening the status back to a bare "accepted".
#[test]
fn an_unguarded_latch_carries_its_reason_to_the_match_event() {
    let sources = FakeSources::new(vec![result(&["+3.73% to Critical Hit Chance"])]);
    let protection = Arc::new(FakeProtection::default());
    *protection.unguarded_reason.lock().unwrap() =
        Some("the click-block hook could not arm: injected".to_owned());
    let handle = runtime(&sources, &protection);
    handle.start(poe2_quick_settings()).unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::MatchFound { .. })
    });
    let RuntimeEvent::MatchFound { detection, .. } = event else {
        unreachable!()
    };
    assert_eq!(
        detection.unguarded.as_deref(),
        Some("the click-block hook could not arm: injected"),
        "the reason must reach the UI verbatim, or the log cannot name the failure"
    );
    shutdown(&handle);
}

/// The ordinary path must stay quiet: a guarded latch reports no reason, so
/// the log gains no line and the alert keeps its normal notice.
#[test]
fn a_guarded_latch_reports_no_unguarded_reason() {
    let sources = FakeSources::new(vec![result(&["+3.73% to Critical Hit Chance"])]);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&sources, &protection);
    handle.start(poe2_quick_settings()).unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::MatchFound { .. })
    });
    let RuntimeEvent::MatchFound { detection, .. } = event else {
        unreachable!()
    };
    assert_eq!(detection.unguarded, None);
    shutdown(&handle);
}
