use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use poe_alarm_core::{
    AcceptableResultGroup, AffixCondition, CompiledRuleSet, FullLineAffixMatcher,
    LogicalAffixMatch, NumericConstraint, ResultGroupMode, RuleSetDefinition,
};
use poe_alarm_monitoring::{
    CancellationToken, EventSink, FrameCapture, Monitor, MonitorClock, MonitorDetection,
    MonitorEvent, MonitorPlan, MonitorState, OcrRecognizer, PreparedFrame, RecognitionResult,
    ScanPace, StructuredOcrSupport,
};
use poe_alarm_vision::{BYTES_PER_PIXEL, CaptureRegion, CapturedFrame};

const WAIT_LIMIT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
struct TestError(&'static str);

impl fmt::Display for TestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

#[derive(Clone)]
enum CaptureStep {
    Frame(CapturedFrame),
    Fault(TestError),
}

struct FakeCapture {
    steps: Vec<CaptureStep>,
    next: usize,
    calls: Arc<AtomicUsize>,
}

impl FakeCapture {
    fn repeating(frames: Vec<CapturedFrame>) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                steps: frames.into_iter().map(CaptureStep::Frame).collect(),
                next: 0,
                calls: Arc::clone(&calls),
            },
            calls,
        )
    }

    fn fault(error: &'static str) -> Self {
        Self {
            steps: vec![CaptureStep::Fault(TestError(error))],
            next: 0,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl FrameCapture for FakeCapture {
    type Error = TestError;

    fn capture_into(
        &mut self,
        _region: CaptureRegion,
        destination: &mut CapturedFrame,
    ) -> Result<(), Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let index = self.next.min(self.steps.len().saturating_sub(1));
        self.next = self.next.saturating_add(1);
        match &self.steps[index] {
            CaptureStep::Frame(frame) => {
                *destination = frame.clone();
                Ok(())
            }
            CaptureStep::Fault(error) => Err(error.clone()),
        }
    }
}

enum OcrStep {
    Return(RecognitionResult),
    BlockUntilCancelled(RecognitionResult),
    Fault(TestError),
}

#[derive(Default)]
struct OcrTrace {
    quick_targets: Vec<String>,
    structured_targets: Vec<Vec<String>>,
    fingerprints: Vec<u64>,
    calls_started: usize,
}

#[derive(Default)]
struct SharedOcrTrace {
    trace: Mutex<OcrTrace>,
    changed: Condvar,
}

struct FakeOcr {
    support: StructuredOcrSupport,
    steps: VecDeque<OcrStep>,
    trace: Arc<SharedOcrTrace>,
}

impl FakeOcr {
    fn new(
        support: StructuredOcrSupport,
        steps: impl IntoIterator<Item = OcrStep>,
    ) -> (Self, Arc<SharedOcrTrace>) {
        let trace = Arc::new(SharedOcrTrace::default());
        (
            Self {
                support,
                steps: steps.into_iter().collect(),
                trace: Arc::clone(&trace),
            },
            trace,
        )
    }

    fn run_step(
        &mut self,
        prepared: PreparedFrame<'_>,
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, TestError> {
        {
            let mut trace = self
                .trace
                .trace
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            trace.fingerprints.push(prepared.semantic_fingerprint);
            trace.calls_started += 1;
            self.trace.changed.notify_all();
        }
        match self
            .steps
            .pop_front()
            .unwrap_or_else(|| OcrStep::Return(RecognitionResult::default()))
        {
            OcrStep::Return(result) => Ok(result),
            OcrStep::BlockUntilCancelled(result) => {
                cancellation.wait_cancelled();
                Ok(result)
            }
            OcrStep::Fault(error) => Err(error),
        }
    }
}

impl OcrRecognizer for FakeOcr {
    type Error = TestError;

    fn structured_support(&self) -> StructuredOcrSupport {
        self.support
    }

    fn recognize_quick(
        &mut self,
        prepared: PreparedFrame<'_>,
        target: &FullLineAffixMatcher,
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, Self::Error> {
        self.trace
            .trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .quick_targets
            .push(target.template().text.clone());
        self.run_step(prepared, cancellation)
    }

    fn recognize_structured(
        &mut self,
        prepared: PreparedFrame<'_>,
        targets: &[FullLineAffixMatcher],
        cancellation: &CancellationToken,
    ) -> Result<RecognitionResult, Self::Error> {
        self.trace
            .trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .structured_targets
            .push(
                targets
                    .iter()
                    .map(|target| target.template().text.clone())
                    .collect(),
            );
        self.run_step(prepared, cancellation)
    }
}

impl SharedOcrTrace {
    fn wait_for_calls(&self, expected: usize) {
        let deadline = Instant::now() + WAIT_LIMIT;
        let mut trace = self
            .trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while trace.calls_started < expected {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {expected} OCR calls"
            );
            let (next, timeout) = self
                .changed
                .wait_timeout(trace, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            trace = next;
            assert!(
                !timeout.timed_out() || trace.calls_started >= expected,
                "timed out waiting for {expected} OCR calls"
            );
        }
    }
}

#[derive(Default)]
struct EventTrace {
    events: Mutex<Vec<MonitorEvent>>,
    changed: Condvar,
}

#[derive(Clone, Default)]
struct RecordingEvents(Arc<EventTrace>);

impl EventSink for RecordingEvents {
    fn emit(&self, event: MonitorEvent) {
        self.0
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event);
        self.0.changed.notify_all();
    }
}

impl RecordingEvents {
    fn wait_until(&self, predicate: impl Fn(&[MonitorEvent]) -> bool) {
        let deadline = Instant::now() + WAIT_LIMIT;
        let mut events = self
            .0
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !predicate(&events) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "timed out waiting for monitor events");
            let (next, timeout) = self
                .0
                .changed
                .wait_timeout(events, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            events = next;
            assert!(
                !timeout.timed_out() || predicate(&events),
                "timed out waiting for monitor events"
            );
        }
    }

    fn snapshot(&self) -> Vec<MonitorEvent> {
        self.0
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[derive(Default)]
struct GateState {
    paces: Vec<ScanPace>,
    permits: usize,
}

#[derive(Default)]
struct SharedClock {
    state: Mutex<GateState>,
    changed: Condvar,
    ticks: AtomicU64,
}

struct GatedClock(Arc<SharedClock>);

impl GatedClock {
    fn new() -> (Self, Arc<SharedClock>) {
        let shared = Arc::new(SharedClock::default());
        (Self(Arc::clone(&shared)), shared)
    }
}

impl MonitorClock for GatedClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.ticks.fetch_add(1, Ordering::SeqCst))
    }

    fn pace(&mut self, pace: ScanPace, cancellation: &CancellationToken) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.paces.push(pace);
        self.0.changed.notify_all();
        while state.permits == 0 && !cancellation.is_cancelled() {
            let (next, _) = self
                .0
                .changed
                .wait_timeout(state, Duration::from_millis(5))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
        }
        if state.permits > 0 {
            state.permits -= 1;
        }
    }
}

impl SharedClock {
    fn wait_for_paces(&self, expected: usize) {
        let deadline = Instant::now() + WAIT_LIMIT;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while state.paces.len() < expected {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {expected} paces"
            );
            let (next, timeout) = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
            assert!(
                !timeout.timed_out() || state.paces.len() >= expected,
                "timed out waiting for {expected} paces"
            );
        }
    }

    fn advance(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.permits += 1;
        self.changed.notify_all();
    }

    fn paces(&self) -> Vec<ScanPace> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .paces
            .clone()
    }
}

fn frame(blue: u8) -> CapturedFrame {
    let region = test_region();
    let mut pixels = vec![0; region.width as usize * region.height as usize * BYTES_PER_PIXEL];
    for alpha in pixels.iter_mut().skip(3).step_by(BYTES_PER_PIXEL) {
        *alpha = 255;
    }
    pixels[0] = blue;
    CapturedFrame::from_bgra(
        region,
        region.width as usize * BYTES_PER_PIXEL,
        pixels,
        SystemTime::UNIX_EPOCH,
    )
    .unwrap()
}

fn frame_with_blue_at(blue: u8, pixel_index: usize) -> CapturedFrame {
    let region = test_region();
    let mut pixels = vec![0; region.width as usize * region.height as usize * BYTES_PER_PIXEL];
    for alpha in pixels.iter_mut().skip(3).step_by(BYTES_PER_PIXEL) {
        *alpha = 255;
    }
    pixels[pixel_index * BYTES_PER_PIXEL] = blue;
    CapturedFrame::from_bgra(
        region,
        region.width as usize * BYTES_PER_PIXEL,
        pixels,
        SystemTime::UNIX_EPOCH,
    )
    .unwrap()
}

fn test_region() -> CaptureRegion {
    CaptureRegion::new(10, 20, 2, 2).unwrap()
}

fn quick_plan() -> MonitorPlan {
    MonitorPlan::Quick(FullLineAffixMatcher::new("#% increased Attack Speed").unwrap())
}

fn hit_result() -> RecognitionResult {
    RecognitionResult {
        lines: vec!["25% increased Attack Speed".to_owned()],
        recognition_elapsed: Duration::from_millis(3),
        ..RecognitionResult::default()
    }
}

fn miss_result() -> RecognitionResult {
    RecognitionResult {
        lines: vec!["17% increased Cast Speed".to_owned()],
        recognition_elapsed: Duration::from_millis(2),
        ..RecognitionResult::default()
    }
}

fn structured_rules() -> CompiledRuleSet {
    CompiledRuleSet::compile(RuleSetDefinition {
        schema_version: 1,
        name: "batch".to_owned(),
        groups: vec![AcceptableResultGroup {
            name: "speed".to_owned(),
            mode: ResultGroupMode::Any,
            required_count: 1,
            conditions: vec![
                AffixCondition::new(
                    "first",
                    "#% increased Attack Speed",
                    vec![NumericConstraint::at_least(20)],
                ),
                AffixCondition::new(
                    "duplicate",
                    "#% increased Attack Speed",
                    vec![NumericConstraint::at_least(25)],
                ),
            ],
        }],
    })
    .unwrap()
}

#[test]
fn unchanged_accepted_frame_skips_ocr_and_uses_cached_pacing() {
    let (capture, capture_calls) = FakeCapture::repeating(vec![frame(130)]);
    let (ocr, ocr_trace) = FakeOcr::new(
        StructuredOcrSupport::StrictBatch,
        [OcrStep::Return(miss_result())],
    );
    let (clock, clock_trace) = GatedClock::new();
    let events = RecordingEvents::default();
    let mut monitor = Monitor::new(capture, ocr, clock, events.clone());

    monitor.start(quick_plan(), test_region()).unwrap();
    clock_trace.wait_for_paces(1);
    assert_eq!(clock_trace.paces(), vec![ScanPace::UncachedDelay]);
    clock_trace.advance();
    clock_trace.wait_for_paces(2);

    assert_eq!(ocr_trace.trace.lock().unwrap().calls_started, 1);
    assert!(capture_calls.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        clock_trace.paces(),
        vec![ScanPace::UncachedDelay, ScanPace::CachedDelay]
    );
    monitor.stop().unwrap();

    let trace = events.snapshot();
    assert!(trace.iter().any(|event| {
        matches!(
            event,
            MonitorEvent::Snapshot(snapshot)
                if snapshot.state == MonitorState::Running && snapshot.scan_count == 1
        )
    }));
    assert!(
        !trace
            .iter()
            .any(|event| matches!(event, MonitorEvent::Detection(_)))
    );
    assert_eq!(
        trace
            .iter()
            .filter(|event| matches!(event, MonitorEvent::InputGuardRequested { .. }))
            .count(),
        1
    );
    assert_eq!(
        trace
            .iter()
            .filter(|event| matches!(event, MonitorEvent::InputGuardReleased { .. }))
            .count(),
        1
    );
}

#[test]
fn changed_frame_reenters_ocr_and_rearms_guard() {
    let (capture, _) =
        FakeCapture::repeating(vec![frame_with_blue_at(180, 0), frame_with_blue_at(180, 1)]);
    let (ocr, trace) = FakeOcr::new(
        StructuredOcrSupport::StrictBatch,
        [
            OcrStep::Return(miss_result()),
            OcrStep::Return(miss_result()),
        ],
    );
    let (clock, clock_trace) = GatedClock::new();
    let events = RecordingEvents::default();
    let mut monitor = Monitor::new(capture, ocr, clock, events.clone());

    monitor.start(quick_plan(), test_region()).unwrap();
    clock_trace.wait_for_paces(1);
    clock_trace.advance();
    clock_trace.wait_for_paces(2);
    trace.wait_for_calls(2);
    assert_eq!(trace.trace.lock().unwrap().calls_started, 2);
    monitor.stop().unwrap();

    let trace = events.snapshot();
    assert_eq!(
        trace
            .iter()
            .filter(|event| matches!(event, MonitorEvent::InputGuardRequested { .. }))
            .count(),
        2
    );
    assert_eq!(
        trace
            .iter()
            .filter(|event| matches!(event, MonitorEvent::InputGuardReleased { .. }))
            .count(),
        2
    );
}

#[test]
fn requires_rescan_keeps_fingerprint_pending_and_refreshes_guard() {
    let (capture, capture_calls) = FakeCapture::repeating(vec![frame(180)]);
    let mut progressive = miss_result();
    progressive.requires_rescan = true;
    let (ocr, trace) = FakeOcr::new(
        StructuredOcrSupport::StrictBatch,
        [OcrStep::Return(progressive), OcrStep::Return(miss_result())],
    );
    let (clock, clock_trace) = GatedClock::new();
    let events = RecordingEvents::default();
    let mut monitor = Monitor::new(capture, ocr, clock, events.clone());

    monitor.start(quick_plan(), test_region()).unwrap();
    clock_trace.wait_for_paces(1);
    assert_eq!(clock_trace.paces(), vec![ScanPace::ProgressiveYield]);
    let first_events = events.snapshot();
    assert_eq!(
        first_events
            .iter()
            .filter(|event| matches!(event, MonitorEvent::InputGuardRequested { .. }))
            .count(),
        1
    );
    assert!(
        !first_events
            .iter()
            .any(|event| matches!(event, MonitorEvent::InputGuardReleased { .. }))
    );

    clock_trace.advance();
    clock_trace.wait_for_paces(2);
    trace.wait_for_calls(2);
    assert!(capture_calls.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        clock_trace.paces(),
        vec![ScanPace::ProgressiveYield, ScanPace::UncachedDelay]
    );
    monitor.stop().unwrap();

    let final_events = events.snapshot();
    assert_eq!(
        final_events
            .iter()
            .filter(|event| matches!(event, MonitorEvent::InputGuardRequested { .. }))
            .count(),
        2
    );
    assert_eq!(
        final_events
            .iter()
            .filter(|event| matches!(event, MonitorEvent::InputGuardReleased { .. }))
            .count(),
        1
    );
}

#[test]
fn quick_and_structured_use_distinct_single_request_routes() {
    let (capture, _) = FakeCapture::repeating(vec![frame(160)]);
    let (ocr, quick_trace) = FakeOcr::new(
        StructuredOcrSupport::StrictBatch,
        [OcrStep::Return(hit_result())],
    );
    let (clock, _) = GatedClock::new();
    let quick_events = RecordingEvents::default();
    let mut quick = Monitor::new(capture, ocr, clock, quick_events.clone());
    quick.start(quick_plan(), test_region()).unwrap();
    quick_events.wait_until(|events| {
        events
            .iter()
            .any(|event| matches!(event, MonitorEvent::Detection(_)))
    });
    quick.stop().unwrap();
    let quick_trace = quick_trace.trace.lock().unwrap();
    assert_eq!(quick_trace.quick_targets.len(), 1);
    assert!(quick_trace.structured_targets.is_empty());

    let rules = structured_rules();
    assert_eq!(
        rules.targets().len(),
        1,
        "numeric variants must deduplicate"
    );
    let (capture, _) = FakeCapture::repeating(vec![frame(160)]);
    let (ocr, structured_trace) = FakeOcr::new(
        StructuredOcrSupport::StrictBatch,
        [OcrStep::Return(hit_result())],
    );
    let (clock, _) = GatedClock::new();
    let structured_events = RecordingEvents::default();
    let mut structured = Monitor::new(capture, ocr, clock, structured_events.clone());
    structured
        .start(MonitorPlan::Structured(rules), test_region())
        .unwrap();
    structured_events.wait_until(|events| {
        events
            .iter()
            .any(|event| matches!(event, MonitorEvent::Detection(_)))
    });
    structured.stop().unwrap();
    let structured_trace = structured_trace.trace.lock().unwrap();
    assert!(structured_trace.quick_targets.is_empty());
    assert_eq!(structured_trace.structured_targets.len(), 1);
    assert_eq!(structured_trace.structured_targets[0].len(), 1);
}

#[test]
fn hit_wins_even_when_recognizer_requests_progressive_rescan() {
    let (capture, _) = FakeCapture::repeating(vec![frame(150)]);
    let mut result = hit_result();
    result.requires_rescan = true;
    let (ocr, _) = FakeOcr::new(StructuredOcrSupport::StrictBatch, [OcrStep::Return(result)]);
    let (clock, clock_trace) = GatedClock::new();
    let events = RecordingEvents::default();
    let mut monitor = Monitor::new(capture, ocr, clock, events.clone());

    monitor.start(quick_plan(), test_region()).unwrap();
    events.wait_until(|events| {
        events
            .iter()
            .any(|event| matches!(event, MonitorEvent::Detection(_)))
    });
    assert_eq!(monitor.state(), MonitorState::MatchFound);
    assert!(clock_trace.paces().is_empty());

    let trace = events.snapshot();
    let match_snapshot = trace
        .iter()
        .position(|event| {
            matches!(
                event,
                MonitorEvent::Snapshot(snapshot) if snapshot.state == MonitorState::MatchFound
            )
        })
        .unwrap();
    let detection = trace
        .iter()
        .position(|event| matches!(event, MonitorEvent::Detection(_)))
        .unwrap();
    let release = trace
        .iter()
        .position(|event| matches!(event, MonitorEvent::InputGuardReleased { .. }))
        .unwrap();
    assert!(match_snapshot < detection && detection < release);
    monitor.stop().unwrap();
}

#[test]
fn wrong_quick_assistance_is_rejected_but_matching_assistance_preserves_text() {
    let wrong = LogicalAffixMatch {
        start_line_index: 0,
        physical_line_count: 1,
        original_text: "assisted wrong".to_owned(),
        canonical_text: "# % increased cast speed".to_owned(),
    };
    let matching = LogicalAffixMatch {
        start_line_index: 0,
        physical_line_count: 1,
        original_text: "25% INCREASED ATTACK SPEED".to_owned(),
        canonical_text: FullLineAffixMatcher::new("#% increased Attack Speed")
            .unwrap()
            .template()
            .text
            .clone(),
    };
    let (capture, _) =
        FakeCapture::repeating(vec![frame_with_blue_at(180, 0), frame_with_blue_at(180, 1)]);
    let (ocr, _) = FakeOcr::new(
        StructuredOcrSupport::StrictBatch,
        [
            OcrStep::Return(RecognitionResult {
                target_assisted_match: Some(wrong),
                ..miss_result()
            }),
            OcrStep::Return(RecognitionResult {
                lines: Vec::new(),
                target_assisted_match: Some(matching),
                ..RecognitionResult::default()
            }),
        ],
    );
    let (clock, clock_trace) = GatedClock::new();
    let events = RecordingEvents::default();
    let mut monitor = Monitor::new(capture, ocr, clock, events.clone());

    monitor.start(quick_plan(), test_region()).unwrap();
    clock_trace.wait_for_paces(1);
    clock_trace.advance();
    events.wait_until(|events| {
        events
            .iter()
            .any(|event| matches!(event, MonitorEvent::Detection(_)))
    });
    let detected_text = events.snapshot().into_iter().find_map(|event| match event {
        MonitorEvent::Detection(MonitorDetection::Quick { matched, .. }) => {
            Some(matched.original_text)
        }
        _ => None,
    });
    assert_eq!(detected_text.as_deref(), Some("25% INCREASED ATTACK SPEED"));
    monitor.stop().unwrap();
}

#[test]
fn stop_during_ocr_cancels_without_late_hit_or_hang() {
    let (capture, _) = FakeCapture::repeating(vec![frame(170)]);
    let (ocr, trace) = FakeOcr::new(
        StructuredOcrSupport::StrictBatch,
        [OcrStep::BlockUntilCancelled(hit_result())],
    );
    let (clock, _) = GatedClock::new();
    let events = RecordingEvents::default();
    let mut monitor = Monitor::new(capture, ocr, clock, events.clone());

    monitor.start(quick_plan(), test_region()).unwrap();
    trace.wait_for_calls(1);
    let started = Instant::now();
    monitor.stop().unwrap();
    assert!(started.elapsed() < Duration::from_millis(250));
    assert_eq!(monitor.state(), MonitorState::Idle);

    let trace = events.snapshot();
    assert!(
        !trace
            .iter()
            .any(|event| matches!(event, MonitorEvent::Detection(_)))
    );
    assert!(!trace.iter().any(|event| {
        matches!(
            event,
            MonitorEvent::Snapshot(snapshot) if snapshot.state == MonitorState::MatchFound
        )
    }));
    assert!(
        trace
            .iter()
            .any(|event| matches!(event, MonitorEvent::InputGuardReleased { .. }))
    );
}

#[test]
fn stop_during_structured_batch_cancels_without_late_hit() {
    let (capture, _) = FakeCapture::repeating(vec![frame(170)]);
    let (ocr, trace) = FakeOcr::new(
        StructuredOcrSupport::StrictBatch,
        [OcrStep::BlockUntilCancelled(hit_result())],
    );
    let (clock, _) = GatedClock::new();
    let events = RecordingEvents::default();
    let mut monitor = Monitor::new(capture, ocr, clock, events.clone());

    monitor
        .start(MonitorPlan::Structured(structured_rules()), test_region())
        .unwrap();
    trace.wait_for_calls(1);
    monitor.stop().unwrap();

    assert_eq!(monitor.state(), MonitorState::Idle);
    assert!(
        !events
            .snapshot()
            .iter()
            .any(|event| matches!(event, MonitorEvent::Detection(_)))
    );
    let trace = trace.trace.lock().unwrap();
    assert!(trace.quick_targets.is_empty());
    assert_eq!(trace.structured_targets.len(), 1);
}

#[test]
fn ocr_fault_transitions_to_faulted_and_releases_guard() {
    let (capture, _) = FakeCapture::repeating(vec![frame(190)]);
    let (ocr, _) = FakeOcr::new(
        StructuredOcrSupport::StrictBatch,
        [OcrStep::Fault(TestError("engine unavailable"))],
    );
    let (clock, _) = GatedClock::new();
    let events = RecordingEvents::default();
    let mut monitor = Monitor::new(capture, ocr, clock, events.clone());

    monitor.start(quick_plan(), test_region()).unwrap();
    events.wait_until(|events| {
        events.iter().any(|event| {
            matches!(
                event,
                MonitorEvent::Snapshot(snapshot) if snapshot.state == MonitorState::Faulted
            )
        }) && events
            .iter()
            .any(|event| matches!(event, MonitorEvent::InputGuardReleased { .. }))
    });
    assert_eq!(monitor.state(), MonitorState::Faulted);
    let fault = events.snapshot().into_iter().find_map(|event| match event {
        MonitorEvent::Snapshot(snapshot) if snapshot.state == MonitorState::Faulted => {
            snapshot.detail
        }
        _ => None,
    });
    assert_eq!(fault.as_deref(), Some("engine unavailable"));
    monitor.stop().unwrap();
    assert_eq!(monitor.state(), MonitorState::Idle);
}

#[test]
fn capture_fault_transitions_to_faulted_without_arming_guard() {
    let capture = FakeCapture::fault("desktop unavailable");
    let (ocr, _) = FakeOcr::new(StructuredOcrSupport::StrictBatch, []);
    let (clock, _) = GatedClock::new();
    let events = RecordingEvents::default();
    let mut monitor = Monitor::new(capture, ocr, clock, events.clone());

    monitor.start(quick_plan(), test_region()).unwrap();
    events.wait_until(|events| {
        events.iter().any(|event| {
            matches!(
                event,
                MonitorEvent::Snapshot(snapshot) if snapshot.state == MonitorState::Faulted
            )
        })
    });
    assert_eq!(monitor.state(), MonitorState::Faulted);
    assert!(
        !events
            .snapshot()
            .iter()
            .any(|event| matches!(event, MonitorEvent::InputGuardRequested { .. }))
    );
    monitor.stop().unwrap();
}

#[test]
fn unsupported_structured_start_is_rejected_without_state_change() {
    let (capture, _) = FakeCapture::repeating(vec![frame(130)]);
    let (ocr, trace) = FakeOcr::new(StructuredOcrSupport::Unsupported, []);
    let (clock, _) = GatedClock::new();
    let events = RecordingEvents::default();
    let mut monitor = Monitor::new(capture, ocr, clock, events);

    let error = monitor
        .start(MonitorPlan::Structured(structured_rules()), test_region())
        .unwrap_err();
    assert_eq!(
        error,
        poe_alarm_monitoring::StartError::StructuredOcrUnsupported
    );
    assert_eq!(monitor.state(), MonitorState::Idle);
    assert_eq!(trace.trace.lock().unwrap().calls_started, 0);
}
