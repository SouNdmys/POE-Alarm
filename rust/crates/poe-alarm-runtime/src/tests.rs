use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use poe_alarm_core::{
    AcceptableResultGroup, AffixCondition, AssistedModifierObservation, FullLineAffixMatcher,
    ResultGroupMode, RuleSetDefinition, canonicalize,
};
use poe_alarm_monitoring::{
    CancellationToken, PreparedFrame, RecognitionResult, StructuredOcrSupport,
};
use poe_alarm_recognition::RecognitionProfile;
use poe_alarm_settings::{AppSettings, GameProfileSettings, RuleEditorMode, ScreenRegion};
use poe_alarm_vision::{CaptureRegion, CapturedFrame};

use crate::{
    AlertLatchStatus, AlertPresentation, BackendError, BackendFactory, CaptureBackend,
    ProtectionError, ProtectionEvent, ProtectionService, RecognizerBackend, RuntimeEvent,
    RuntimeGeneration, RuntimeHandle, RuntimeOperation, RuntimeRequestId, RuntimeSendError,
    RuntimeState, ScreenshotRequest,
};

#[derive(Clone)]
struct FakeFactory {
    state: Arc<FakeBackendState>,
}

struct FakeBackendState {
    frames: Mutex<VecDeque<CapturedFrame>>,
    last_frame: Mutex<CapturedFrame>,
    live_results: Mutex<VecDeque<RecognitionResult>>,
    screenshot_results: Mutex<VecDeque<RecognitionResult>>,
    live_calls: AtomicUsize,
    screenshot_calls: AtomicUsize,
    structured_calls: AtomicUsize,
    structured_support: AtomicUsize,
    block_live_until_cancelled: AtomicBool,
    block_live_ignoring_cancellation: AtomicBool,
    block_screenshot_until_cancelled: AtomicBool,
    block_screenshot_ignoring_cancellation: AtomicBool,
    screenshot_pass_delay_millis: AtomicUsize,
    recognizers_alive: AtomicUsize,
    recognizers_peak: AtomicUsize,
    decoded: Mutex<CapturedFrame>,
    profiles: Mutex<Vec<RecognitionProfile>>,
}

impl FakeFactory {
    fn new(frames: Vec<CapturedFrame>, results: Vec<RecognitionResult>) -> Self {
        let first = frames.first().cloned().unwrap_or_else(|| frame(1, 64, 64));
        Self {
            state: Arc::new(FakeBackendState {
                frames: Mutex::new(frames.into()),
                last_frame: Mutex::new(first.clone()),
                live_results: Mutex::new(results.clone().into()),
                screenshot_results: Mutex::new(results.into()),
                live_calls: AtomicUsize::new(0),
                screenshot_calls: AtomicUsize::new(0),
                structured_calls: AtomicUsize::new(0),
                structured_support: AtomicUsize::new(1),
                block_live_until_cancelled: AtomicBool::new(false),
                block_live_ignoring_cancellation: AtomicBool::new(false),
                block_screenshot_until_cancelled: AtomicBool::new(false),
                block_screenshot_ignoring_cancellation: AtomicBool::new(false),
                screenshot_pass_delay_millis: AtomicUsize::new(0),
                recognizers_alive: AtomicUsize::new(0),
                recognizers_peak: AtomicUsize::new(0),
                decoded: Mutex::new(first),
                profiles: Mutex::new(Vec::new()),
            }),
        }
    }
}

impl BackendFactory for FakeFactory {
    fn create_capture(&self) -> Result<Box<dyn CaptureBackend>, BackendError> {
        Ok(Box::new(FakeCapture {
            state: Arc::clone(&self.state),
        }))
    }

    fn create_recognizer(
        &self,
        profile: RecognitionProfile,
    ) -> Result<Box<dyn RecognizerBackend>, BackendError> {
        self.state.profiles.lock().unwrap().push(profile);
        let alive = self.state.recognizers_alive.fetch_add(1, Ordering::AcqRel) + 1;
        self.state
            .recognizers_peak
            .fetch_max(alive, Ordering::AcqRel);
        Ok(Box::new(FakeRecognizer {
            state: Arc::clone(&self.state),
        }))
    }

    fn decode_screenshot(&self, _path: &Path) -> Result<CapturedFrame, BackendError> {
        Ok(self.state.decoded.lock().unwrap().clone())
    }
}

struct FakeCapture {
    state: Arc<FakeBackendState>,
}

impl CaptureBackend for FakeCapture {
    fn capture_into(
        &mut self,
        _region: CaptureRegion,
        destination: &mut CapturedFrame,
    ) -> Result<(), BackendError> {
        let next = self
            .state
            .frames
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| self.state.last_frame.lock().unwrap().clone());
        *self.state.last_frame.lock().unwrap() = next.clone();
        *destination = next;
        Ok(())
    }
}

struct FakeRecognizer {
    state: Arc<FakeBackendState>,
}

impl Drop for FakeRecognizer {
    fn drop(&mut self) {
        self.state.recognizers_alive.fetch_sub(1, Ordering::AcqRel);
    }
}

impl FakeRecognizer {
    fn next_live(&self) -> RecognitionResult {
        pop_or_default(&self.state.live_results)
    }

    fn next_screenshot(&self) -> RecognitionResult {
        pop_or_default(&self.state.screenshot_results)
    }
}

impl RecognizerBackend for FakeRecognizer {
    fn structured_support(&self) -> StructuredOcrSupport {
        match self.state.structured_support.load(Ordering::Acquire) {
            0 => StructuredOcrSupport::Unsupported,
            2 => StructuredOcrSupport::ConfirmedStrictBatch,
            _ => StructuredOcrSupport::StrictBatch,
        }
    }

    fn recognize_quick_live(
        &mut self,
        _prepared: PreparedFrame<'_>,
        _target: &FullLineAffixMatcher,
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, BackendError> {
        self.state.live_calls.fetch_add(1, Ordering::AcqRel);
        if self
            .state
            .block_live_until_cancelled
            .load(Ordering::Acquire)
        {
            cancellation.wait_cancelled();
        }
        while self
            .state
            .block_live_ignoring_cancellation
            .load(Ordering::Acquire)
        {
            thread::sleep(Duration::from_millis(1));
        }
        Ok(self.next_live())
    }

    fn recognize_structured_live(
        &mut self,
        _prepared: PreparedFrame<'_>,
        _targets: &[FullLineAffixMatcher],
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, BackendError> {
        self.state.structured_calls.fetch_add(1, Ordering::AcqRel);
        self.recognize_quick_live(_prepared, _targets.first().unwrap(), cancellation)
    }

    fn recognize_quick_screenshot(
        &mut self,
        _prepared: PreparedFrame<'_>,
        _target: &FullLineAffixMatcher,
        cancelled: &AtomicBool,
    ) -> Result<RecognitionResult, BackendError> {
        self.state.screenshot_calls.fetch_add(1, Ordering::AcqRel);
        while self
            .state
            .block_screenshot_until_cancelled
            .load(Ordering::Acquire)
            && !cancelled.load(Ordering::Acquire)
        {
            thread::sleep(Duration::from_millis(1));
        }
        while self
            .state
            .block_screenshot_ignoring_cancellation
            .load(Ordering::Acquire)
        {
            thread::sleep(Duration::from_millis(1));
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(BackendError("cancelled".to_owned()));
        }
        let delay = self
            .state
            .screenshot_pass_delay_millis
            .load(Ordering::Acquire);
        if delay > 0 {
            thread::sleep(Duration::from_millis(delay as u64));
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(BackendError("cancelled".to_owned()));
        }
        Ok(self.next_screenshot())
    }

    fn recognize_structured_screenshot(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancelled: &AtomicBool,
    ) -> Result<RecognitionResult, BackendError> {
        self.state.structured_calls.fetch_add(1, Ordering::AcqRel);
        self.recognize_quick_screenshot(prepared, targets.first().unwrap(), cancelled)
    }
}

fn pop_or_default(queue: &Mutex<VecDeque<RecognitionResult>>) -> RecognitionResult {
    queue.lock().unwrap().pop_front().unwrap_or_default()
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
    fail_prepare: AtomicBool,
    fail_latch: AtomicBool,
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
        if self.fail_prepare.load(Ordering::Acquire) {
            Err(ProtectionError("injected prepare failure".to_owned()))
        } else {
            Ok(())
        }
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
            Ok(AlertLatchStatus::Accepted)
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

fn frame(seed: u8, width: u32, height: u32) -> CapturedFrame {
    let region = CaptureRegion::new(0, 0, width, height).unwrap();
    let stride = width as usize * 4;
    let mut pixels = vec![0_u8; stride * height as usize];
    let offset = usize::from(seed) % (width as usize * height as usize);
    pixels[offset * 4] = 220;
    pixels[offset * 4 + 1] = 80;
    pixels[offset * 4 + 2] = 70;
    CapturedFrame::from_bgra(region, stride, pixels, SystemTime::now()).unwrap()
}

fn quick_settings() -> AppSettings {
    let mut settings = AppSettings::default();
    settings.profiles.poe1 = GameProfileSettings {
        target_affix: "+#% to Critical Hit Chance".to_owned(),
        capture_region: Some(ScreenRegion::new(0, 0, 64, 64)),
        ..GameProfileSettings::default()
    };
    settings
}

fn protected_quick_settings() -> AppSettings {
    let mut settings = quick_settings();
    settings.selected_game_profile = poe_alarm_settings::GameProfile::Poe2;
    settings.profiles.poe2 = settings.profiles.poe1.clone();
    settings
}

fn structured_settings() -> AppSettings {
    let mut settings = quick_settings();
    settings.profiles.poe1.rule_editor_mode = RuleEditorMode::Structured;
    settings.profiles.poe1.structured_rule_set = Some(RuleSetDefinition {
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
    settings.profiles.poe1.rule_editor_mode = RuleEditorMode::Structured;
    settings.profiles.poe1.structured_rule_set = Some(RuleSetDefinition {
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

fn protected_structured_settings() -> AppSettings {
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

fn runtime(factory: &FakeFactory, protection: &Arc<FakeProtection>) -> RuntimeHandle {
    RuntimeHandle::spawn_with_components(
        Arc::new(factory.clone()),
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
                        | RuntimeEvent::ScreenshotCompleted(_)
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

#[test]
fn unchanged_fingerprint_skips_ocr() {
    let same = frame(1, 64, 64);
    let factory = FakeFactory::new(vec![same.clone(), same], vec![result(&["not it"])]);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle.start(quick_settings()).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::MonitorSnapshot { snapshot, .. }
                if snapshot.scan_count >= 2 && snapshot.ocr_was_cached
        )
    });
    assert_eq!(factory.state.live_calls.load(Ordering::Acquire), 1);
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
fn changed_frame_match_latches_alert_before_guard_release() {
    let factory = FakeFactory::new(
        vec![frame(1, 64, 64)],
        vec![result(&["+3.73% to Critical Hit Chance"])],
    );
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle.start(protected_quick_settings()).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::MatchFound { .. })
    });
    let calls = protection.calls.lock().unwrap().clone();
    let prepare = calls
        .iter()
        .position(|call| matches!(call, SafetyCall::Prepare(_)))
        .unwrap();
    let arm = calls
        .iter()
        .position(|call| matches!(call, SafetyCall::Arm(_)))
        .unwrap();
    let latch = calls
        .iter()
        .position(|call| matches!(call, SafetyCall::Latch(_)))
        .unwrap();
    let release = calls
        .iter()
        .rposition(|call| matches!(call, SafetyCall::Release(_)))
        .unwrap();
    assert!(
        prepare < arm && arm < latch && latch < release,
        "calls: {calls:?}"
    );
    shutdown(&handle);
}

#[test]
fn stop_cancels_blocked_live_ocr_without_late_match() {
    let factory = FakeFactory::new(
        vec![frame(1, 64, 64)],
        vec![result(&["+3.73% to Critical Hit Chance"])],
    );
    factory
        .state
        .block_live_until_cancelled
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle.start(protected_quick_settings()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while factory.state.live_calls.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    handle.stop().unwrap();
    let stop_deadline = Instant::now() + Duration::from_secs(2);
    while !protection
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| matches!(call, SafetyCall::StopPending(_)))
    {
        assert!(Instant::now() < stop_deadline);
        thread::sleep(Duration::from_millis(2));
    }
    let calls = protection.calls.lock().unwrap().clone();
    let arm = calls
        .iter()
        .rposition(|call| matches!(call, SafetyCall::Arm(_)))
        .expect("changed frame arms the pending hook");
    let terminal_stop = calls
        .iter()
        .rposition(|call| matches!(call, SafetyCall::StopPending(_)))
        .expect("runtime stop terminally removes the pending hook");
    assert!(arm < terminal_stop, "calls: {calls:?}");
    thread::sleep(Duration::from_millis(30));
    while let Some(event) = handle.try_next_event() {
        assert!(!matches!(event, RuntimeEvent::MatchFound { .. }));
    }
    shutdown(&handle);
}

#[test]
fn screenshot_cancel_suppresses_completion_and_close_is_bounded() {
    let factory = FakeFactory::new(vec![frame(1, 64, 64)], vec![result(&["not it"])]);
    factory
        .state
        .block_screenshot_until_cancelled
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    let request_id = RuntimeRequestId(44);
    handle
        .test_screenshot(ScreenshotRequest::new(
            request_id,
            quick_settings(),
            "unused.png",
        ))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while factory.state.screenshot_calls.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    handle.cancel_screenshot(request_id).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::ScreenshotCancelled { request_id: id } if *id == request_id
        )
    });
    shutdown(&handle);
    thread::sleep(Duration::from_millis(20));
    while let Some(event) = handle.try_next_event() {
        assert!(!matches!(event, RuntimeEvent::ScreenshotCompleted(_)));
    }
}

#[test]
fn screenshot_progressive_recognition_can_converge_after_more_than_three_passes() {
    let mut results = (0..5)
        .map(|_| RecognitionResult {
            requires_rescan: true,
            ..result(&["partial"])
        })
        .collect::<Vec<_>>();
    results.push(result(&["+3.73% to Critical Hit Chance"]));
    let factory = FakeFactory::new(vec![frame(1, 64, 64)], results);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle
        .test_screenshot(ScreenshotRequest::new(
            RuntimeRequestId(45),
            quick_settings(),
            "unused.png",
        ))
        .unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::ScreenshotCompleted(_))
    });
    let RuntimeEvent::ScreenshotCompleted(report) = event else {
        unreachable!()
    };
    assert!(report.evaluation.is_match);
    assert_eq!(factory.state.screenshot_calls.load(Ordering::Acquire), 6);
    shutdown(&handle);
}

#[test]
fn permanently_progressive_screenshot_emits_typed_non_convergence_fault() {
    let results = (0..crate::actor::MAXIMUM_SCREENSHOT_PASSES)
        .map(|_| RecognitionResult {
            requires_rescan: true,
            ..result(&["partial"])
        })
        .collect();
    let factory = FakeFactory::new(vec![frame(1, 64, 64)], results);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle
        .test_screenshot(ScreenshotRequest::new(
            RuntimeRequestId(46),
            quick_settings(),
            "unused.png",
        ))
        .unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::Fault {
                operation: RuntimeOperation::Screenshot,
                detail,
                ..
            } if detail.contains("did not converge after 128 passes")
        )
    });
    assert!(matches!(
        event,
        RuntimeEvent::Fault {
            operation: RuntimeOperation::Screenshot,
            ..
        }
    ));
    assert_eq!(
        factory.state.screenshot_calls.load(Ordering::Acquire),
        crate::actor::MAXIMUM_SCREENSHOT_PASSES
    );
    shutdown(&handle);
}

#[test]
fn progressive_screenshot_cancellation_wins_before_convergence_or_fault() {
    let results = (0..crate::actor::MAXIMUM_SCREENSHOT_PASSES)
        .map(|_| RecognitionResult {
            requires_rescan: true,
            ..result(&["partial"])
        })
        .collect();
    let factory = FakeFactory::new(vec![frame(1, 64, 64)], results);
    factory
        .state
        .screenshot_pass_delay_millis
        .store(2, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    let request_id = RuntimeRequestId(47);
    handle
        .test_screenshot(ScreenshotRequest::new(
            request_id,
            quick_settings(),
            "unused.png",
        ))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while factory.state.screenshot_calls.load(Ordering::Acquire) <= 3 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    handle.cancel_screenshot(request_id).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::ScreenshotCancelled { request_id: id } if *id == request_id
        )
    });
    thread::sleep(Duration::from_millis(20));
    while let Some(event) = handle.try_next_event() {
        assert!(!matches!(
            event,
            RuntimeEvent::ScreenshotCompleted(_) | RuntimeEvent::Fault { .. }
        ));
    }
    shutdown(&handle);
}

#[test]
fn structured_screenshot_uses_one_batch_and_preserves_strict_evaluation() {
    let factory = FakeFactory::new(
        vec![frame(1, 64, 64)],
        vec![result(&["+3.73% to Critical Hit Chance"])],
    );
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle
        .test_screenshot(ScreenshotRequest::new(
            RuntimeRequestId(7),
            structured_settings(),
            "unused.png",
        ))
        .unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::ScreenshotCompleted(_))
    });
    let RuntimeEvent::ScreenshotCompleted(report) = event else {
        unreachable!()
    };
    assert!(report.evaluation.is_match);
    assert_eq!(report.evaluation.matched_group.as_deref(), Some("critical"));
    assert_eq!(factory.state.structured_calls.load(Ordering::Acquire), 1);
    assert_eq!(factory.state.screenshot_calls.load(Ordering::Acquire), 1);
    shutdown(&handle);
}

#[test]
fn out_of_bounds_screenshot_crop_falls_back_to_full_image() {
    let factory = FakeFactory::new(vec![frame(1, 32, 32)], vec![result(&["not it"])]);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    let mut settings = quick_settings();
    settings.profiles.poe1.capture_region = Some(ScreenRegion::new(100, 100, 64, 64));
    handle
        .test_screenshot(ScreenshotRequest::new(
            RuntimeRequestId(8),
            settings,
            "unused.png",
        ))
        .unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::ScreenshotCompleted(_))
    });
    let RuntimeEvent::ScreenshotCompleted(report) = event else {
        unreachable!()
    };
    assert!(report.used_full_image_fallback);
    shutdown(&handle);
}

#[test]
fn alert_failure_releases_guard_and_emits_explicit_fault() {
    let factory = FakeFactory::new(
        vec![frame(1, 64, 64)],
        vec![result(&["+3.73% to Critical Hit Chance"])],
    );
    let protection = Arc::new(FakeProtection::default());
    protection.fail_latch.store(true, Ordering::Release);
    let handle = runtime(&factory, &protection);
    handle.start(protected_quick_settings()).unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::Fault {
                operation: RuntimeOperation::Alert,
                ..
            }
        )
    });
    assert!(matches!(event, RuntimeEvent::Fault { .. }));
    let calls = protection.calls.lock().unwrap().clone();
    assert!(
        calls
            .iter()
            .any(|call| matches!(call, SafetyCall::Release(_)))
    );
    shutdown(&handle);
}

#[test]
fn sound_failure_is_non_fatal_while_red_alert_remains_latched() {
    let factory = FakeFactory::new(
        vec![frame(1, 64, 64)],
        vec![result(&["+3.73% to Critical Hit Chance"])],
    );
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
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
fn input_guard_prepare_failure_prevents_monitor_start() {
    let factory = FakeFactory::new(vec![frame(1, 64, 64)], vec![result(&["not it"])]);
    let protection = Arc::new(FakeProtection::default());
    protection.fail_prepare.store(true, Ordering::Release);
    let handle = runtime(&factory, &protection);
    handle.start(protected_quick_settings()).unwrap();
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
    assert!(detail.contains("prepare input protection"));
    assert_eq!(factory.state.live_calls.load(Ordering::Acquire), 0);
    assert!(
        protection
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| matches!(call, SafetyCall::StopPending(_))),
        "a failed prepare must still terminally unhook"
    );
    shutdown(&handle);
}

#[test]
fn monitor_start_failure_after_prepare_terminally_unhooks() {
    let factory = FakeFactory::new(vec![frame(1, 64, 64)], vec![result(&["not it"])]);
    factory.state.structured_support.store(0, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle.start(protected_structured_settings()).unwrap();
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
    let prepare = calls
        .iter()
        .position(|call| matches!(call, SafetyCall::Prepare(_)))
        .unwrap();
    let stop = calls
        .iter()
        .position(|call| matches!(call, SafetyCall::StopPending(_)))
        .unwrap();
    assert!(prepare < stop, "calls: {calls:?}");
    assert_eq!(factory.state.live_calls.load(Ordering::Acquire), 0);
    shutdown(&handle);
}

#[test]
fn structured_screenshot_does_not_reuse_one_physical_band_twice() {
    let mut recognition = result(&[]);
    recognition.assisted_observations = vec![
        AssistedModifierObservation::new(
            "same-band",
            "+3.7% to Critical Hit Chance",
            canonicalize("+#% to Critical Hit Chance").text,
        ),
        AssistedModifierObservation::new(
            "same-band",
            "+25% to Critical Strike Multiplier",
            canonicalize("+#% to Critical Strike Multiplier").text,
        ),
    ];
    let factory = FakeFactory::new(vec![frame(1, 64, 64)], vec![recognition]);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle
        .test_screenshot(ScreenshotRequest::new(
            RuntimeRequestId(70),
            at_least_two_settings(),
            "unused.png",
        ))
        .unwrap();
    let event = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::ScreenshotCompleted(_))
    });
    let RuntimeEvent::ScreenshotCompleted(report) = event else {
        unreachable!()
    };
    assert!(!report.evaluation.is_match);
    assert_eq!(report.evaluation.matched_group, None);
    shutdown(&handle);
}

#[test]
fn stale_session_cannot_publish_detection_after_replacement() {
    let factory = FakeFactory::new(
        vec![frame(1, 64, 64), frame(2, 64, 64)],
        vec![
            result(&["+3.73% to Critical Hit Chance"]),
            result(&["not it"]),
        ],
    );
    factory
        .state
        .block_live_until_cancelled
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle.start(quick_settings()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while factory.state.live_calls.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    factory
        .state
        .block_live_until_cancelled
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
fn shutdown_is_bounded_when_native_ocr_ignores_cancellation_in_flight() {
    let factory = FakeFactory::new(
        vec![frame(1, 64, 64)],
        vec![result(&["+3.73% to Critical Hit Chance"])],
    );
    factory
        .state
        .block_live_ignoring_cancellation
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle.start(protected_quick_settings()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while factory.state.live_calls.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }

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
        "input protection must be terminally removed before detached OCR drain"
    );

    factory
        .state
        .block_live_ignoring_cancellation
        .store(false, Ordering::Release);
    thread::sleep(Duration::from_millis(30));
    while let Some(event) = handle.try_next_event() {
        assert!(!matches!(event, RuntimeEvent::MatchFound { .. }));
    }
}

#[test]
fn one_hundred_live_replacements_coalesce_behind_one_retired_native_call() {
    let factory = FakeFactory::new(vec![frame(1, 64, 64)], vec![result(&["not a match"])]);
    factory
        .state
        .block_live_ignoring_cancellation
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle.start(quick_settings()).unwrap();
    let entered = Instant::now() + Duration::from_secs(2);
    while factory.state.live_calls.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < entered);
        thread::sleep(Duration::from_millis(1));
    }

    for replacement in 0..100 {
        retry_queue(|| handle.start(quick_settings()));
        if replacement != 99 {
            retry_queue(|| handle.stop());
        }
    }
    retry_queue(|| handle.acknowledge_alert());
    let processed = Instant::now() + Duration::from_secs(2);
    loop {
        let acknowledgements = protection
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| matches!(call, SafetyCall::Ack))
            .count();
        if acknowledgements >= 2 {
            break;
        }
        assert!(
            Instant::now() < processed,
            "replacement marker was not processed"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(factory.state.profiles.lock().unwrap().len(), 1);
    assert_eq!(factory.state.recognizers_alive.load(Ordering::Acquire), 1);
    assert_eq!(factory.state.recognizers_peak.load(Ordering::Acquire), 1);

    factory
        .state
        .block_live_ignoring_cancellation
        .store(false, Ordering::Release);
    let replacement = Instant::now() + Duration::from_secs(2);
    let mut replacement_started = false;
    while !replacement_started {
        while let Some(event) = handle.try_next_event() {
            assert!(!matches!(
                event,
                RuntimeEvent::MatchFound {
                    generation: RuntimeGeneration(1),
                    ..
                }
            ));
            replacement_started = matches!(
                event,
                RuntimeEvent::StateChanged {
                    generation: Some(RuntimeGeneration(2)),
                    state: RuntimeState::Monitoring,
                }
            );
        }
        assert!(
            Instant::now() < replacement,
            "latest live intent did not start"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(factory.state.profiles.lock().unwrap().len(), 2);
    assert_eq!(factory.state.recognizers_peak.load(Ordering::Acquire), 1);

    let shutdown_started = Instant::now();
    shutdown(&handle);
    assert!(shutdown_started.elapsed() < Duration::from_secs(1));
}

#[test]
fn one_hundred_screenshot_replacements_keep_only_the_latest_intent() {
    let factory = FakeFactory::new(vec![frame(1, 64, 64)], vec![result(&["not a match"])]);
    factory
        .state
        .block_screenshot_ignoring_cancellation
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle
        .test_screenshot(ScreenshotRequest::new(
            RuntimeRequestId(1),
            quick_settings(),
            "unused.png",
        ))
        .unwrap();
    let entered = Instant::now() + Duration::from_secs(2);
    while factory.state.screenshot_calls.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < entered);
        thread::sleep(Duration::from_millis(1));
    }

    for request in 2..=101 {
        retry_queue(|| {
            handle.test_screenshot(ScreenshotRequest::new(
                RuntimeRequestId(request),
                quick_settings(),
                "unused.png",
            ))
        });
    }
    retry_queue(|| handle.acknowledge_alert());
    let processed = Instant::now() + Duration::from_secs(2);
    while !protection
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| matches!(call, SafetyCall::Ack))
    {
        assert!(
            Instant::now() < processed,
            "replacement marker was not processed"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(factory.state.screenshot_calls.load(Ordering::Acquire), 1);
    assert_eq!(factory.state.profiles.lock().unwrap().len(), 1);
    assert_eq!(factory.state.recognizers_alive.load(Ordering::Acquire), 1);
    assert_eq!(factory.state.recognizers_peak.load(Ordering::Acquire), 1);

    factory
        .state
        .block_screenshot_ignoring_cancellation
        .store(false, Ordering::Release);
    let completed = Instant::now() + Duration::from_secs(2);
    let mut latest_completed = false;
    while !latest_completed {
        while let Some(event) = handle.try_next_event() {
            if let RuntimeEvent::ScreenshotCompleted(report) = event {
                assert_eq!(report.request_id, RuntimeRequestId(101));
                latest_completed = true;
            }
        }
        assert!(
            Instant::now() < completed,
            "latest screenshot intent did not finish"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(factory.state.screenshot_calls.load(Ordering::Acquire), 2);
    assert_eq!(factory.state.profiles.lock().unwrap().len(), 2);
    assert_eq!(factory.state.recognizers_peak.load(Ordering::Acquire), 1);

    let shutdown_started = Instant::now();
    shutdown(&handle);
    assert!(shutdown_started.elapsed() < Duration::from_secs(1));
}

#[test]
#[ignore = "release-gate soak; set POE_ALARM_RUNTIME_START_STOP_SOAK_CYCLES to override 1000"]
fn one_thousand_start_stop_cycles_keep_runtime_resources_bounded() {
    let cycles = soak_iterations("POE_ALARM_RUNTIME_START_STOP_SOAK_CYCLES", 1_000);
    let matching_result = result(&["+3.73% to Critical Hit Chance"]);
    let factory = FakeFactory::new(vec![frame(1, 64, 64)], vec![matching_result; cycles]);
    factory
        .state
        .block_live_until_cancelled
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);

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

        let entered = Instant::now() + Duration::from_secs(2);
        while factory.state.live_calls.load(Ordering::Acquire) < cycle + 1 {
            assert!(
                Instant::now() < entered,
                "cycle {cycle} never entered the recognizer"
            );
            thread::sleep(Duration::from_millis(1));
        }

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

        let released = Instant::now() + Duration::from_secs(2);
        while factory.state.recognizers_alive.load(Ordering::Acquire) != 0 {
            assert!(
                Instant::now() < released,
                "cycle {cycle} did not release its recognizer"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    assert_eq!(factory.state.live_calls.load(Ordering::Acquire), cycles);
    assert_eq!(factory.state.profiles.lock().unwrap().len(), cycles);
    assert_eq!(factory.state.recognizers_alive.load(Ordering::Acquire), 0);
    assert_eq!(factory.state.recognizers_peak.load(Ordering::Acquire), 1);
    assert_eq!(*protection.active_generation.lock().unwrap(), None);

    thread::sleep(Duration::from_millis(30));
    while let Some(event) = handle.try_next_event() {
        assert!(
            !matches!(
                event,
                RuntimeEvent::MatchFound { .. }
                    | RuntimeEvent::ScreenshotCompleted(_)
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

#[test]
#[ignore = "release-gate soak; set POE_ALARM_RUNTIME_SCREENSHOT_SOAK_CYCLES to override 500"]
fn five_hundred_screenshot_replacements_and_cancels_are_coalesced() {
    let replacements = soak_iterations("POE_ALARM_RUNTIME_SCREENSHOT_SOAK_CYCLES", 500);
    let final_request_id = replacements as u64 + 1;
    let factory = FakeFactory::new(
        vec![frame(1, 64, 64)],
        vec![result(&["+3.73% to Critical Hit Chance"]); 2],
    );
    factory
        .state
        .block_screenshot_ignoring_cancellation
        .store(true, Ordering::Release);
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle
        .test_screenshot(ScreenshotRequest::new(
            RuntimeRequestId(1),
            quick_settings(),
            "unused.png",
        ))
        .unwrap();

    let entered = Instant::now() + Duration::from_secs(2);
    while factory.state.screenshot_calls.load(Ordering::Acquire) == 0 {
        assert!(
            Instant::now() < entered,
            "initial screenshot never entered the recognizer"
        );
        thread::sleep(Duration::from_millis(1));
    }

    for request_id in 2..=final_request_id {
        retry_queue(|| {
            handle.test_screenshot(ScreenshotRequest::new(
                RuntimeRequestId(request_id),
                quick_settings(),
                "unused.png",
            ))
        });
        if request_id != final_request_id && request_id % 5 == 0 {
            retry_queue(|| handle.cancel_screenshot(RuntimeRequestId(request_id)));
        }
    }
    retry_queue(|| handle.acknowledge_alert());

    let processed = Instant::now() + Duration::from_secs(2);
    while !protection
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| matches!(call, SafetyCall::Ack))
    {
        assert!(
            Instant::now() < processed,
            "screenshot replacement batch was not processed"
        );
        thread::sleep(Duration::from_millis(1));
    }

    let mut cancellation_counts = vec![0_usize; final_request_id as usize + 1];
    while let Some(event) = handle.try_next_event() {
        match event {
            RuntimeEvent::ScreenshotCancelled { request_id } => {
                cancellation_counts[request_id.0 as usize] += 1;
            }
            RuntimeEvent::ScreenshotCompleted(_) | RuntimeEvent::MatchFound { .. } => {
                panic!("blocked screenshot work published a terminal result: {event:?}")
            }
            RuntimeEvent::Fault { .. } => panic!("screenshot soak faulted: {event:?}"),
            _ => {}
        }
    }
    for request_id in 1..final_request_id {
        assert_eq!(
            cancellation_counts[request_id as usize], 1,
            "request {request_id} did not publish exactly one cancellation"
        );
    }
    assert_eq!(cancellation_counts[final_request_id as usize], 0);
    assert_eq!(factory.state.screenshot_calls.load(Ordering::Acquire), 1);
    assert_eq!(factory.state.profiles.lock().unwrap().len(), 1);
    assert_eq!(factory.state.recognizers_alive.load(Ordering::Acquire), 1);
    assert_eq!(factory.state.recognizers_peak.load(Ordering::Acquire), 1);

    factory
        .state
        .block_screenshot_ignoring_cancellation
        .store(false, Ordering::Release);
    let completed = wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::ScreenshotCompleted(report)
                if report.request_id == RuntimeRequestId(final_request_id)
        )
    });
    let RuntimeEvent::ScreenshotCompleted(report) = completed else {
        unreachable!()
    };
    assert_eq!(report.request_id, RuntimeRequestId(final_request_id));
    assert_eq!(factory.state.screenshot_calls.load(Ordering::Acquire), 2);
    assert_eq!(factory.state.profiles.lock().unwrap().len(), 2);
    assert_eq!(factory.state.recognizers_alive.load(Ordering::Acquire), 0);
    assert_eq!(factory.state.recognizers_peak.load(Ordering::Acquire), 1);

    thread::sleep(Duration::from_millis(30));
    while let Some(event) = handle.try_next_event() {
        assert!(
            !matches!(
                event,
                RuntimeEvent::ScreenshotCompleted(_)
                    | RuntimeEvent::MatchFound { .. }
                    | RuntimeEvent::Fault { .. }
            ),
            "screenshot work published a late event after completion: {event:?}"
        );
    }

    let shutdown_started = Instant::now();
    shutdown(&handle);
    assert!(
        shutdown_started.elapsed() < Duration::from_secs(1),
        "runtime shutdown exceeded one second after {replacements} replacements"
    );
}

#[test]
fn profile_switch_builds_a_fresh_recognizer() {
    let factory = FakeFactory::new(
        vec![frame(1, 64, 64), frame(2, 64, 64)],
        vec![result(&["not it"]), result(&["not it"])],
    );
    let protection = Arc::new(FakeProtection::default());
    let handle = runtime(&factory, &protection);
    handle.start(quick_settings()).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(event, RuntimeEvent::MonitorSnapshot { .. })
    });
    let mut second = quick_settings();
    second.selected_game_profile = poe_alarm_settings::GameProfile::Poe2;
    second.profiles.poe2 = second.profiles.poe1.clone();
    handle.start(second).unwrap();
    wait_for(&handle, Duration::from_secs(2), |event| {
        matches!(
            event,
            RuntimeEvent::StateChanged {
                generation: Some(RuntimeGeneration(2)),
                state: RuntimeState::Monitoring,
            }
        )
    });
    let profiles = factory.state.profiles.lock().unwrap().clone();
    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0], RecognitionProfile::POE1_ENGLISH);
    assert_eq!(profiles[1], RecognitionProfile::POE2_ENGLISH);
    shutdown(&handle);
}

#[cfg(windows)]
#[test]
fn production_constructor_and_factory_are_compile_checked() {
    let _constructor: fn(crate::ProductionRuntimeConfig) -> std::io::Result<RuntimeHandle> =
        RuntimeHandle::start_production;
    let _factory = crate::ProductionBackendFactory::packaged();
}
