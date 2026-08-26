use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use poe_alarm_core::RuleEvaluationResult;

use crate::{
    AffixSource, CancellationToken, EventSink, MonitorClock, MonitorDetection, MonitorEvent,
    MonitorPlan, MonitorSnapshot, MonitorState, RecognitionResult, ScanPace, SessionId, StartError,
    StopError, StructuredOcrSupport,
};

struct Resources<S, K> {
    source: S,
    clock: K,
}

struct Lifecycle {
    state: MonitorState,
    current: Option<Arc<SessionControl>>,
    stop_in_progress: bool,
    next_session: u64,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            state: MonitorState::Idle,
            current: None,
            stop_in_progress: false,
            next_session: 1,
        }
    }
}

#[derive(Default)]
struct SharedState {
    lifecycle: Mutex<Lifecycle>,
}

struct SessionControl {
    id: SessionId,
    cancellation: CancellationToken,
    guard_held: AtomicBool,
    guard_order: Mutex<()>,
}

impl SessionControl {
    fn new(id: SessionId) -> Self {
        Self {
            id,
            cancellation: CancellationToken::new(),
            guard_held: AtomicBool::new(false),
            guard_order: Mutex::new(()),
        }
    }

    fn request_guard<E: EventSink>(&self, shared: &SharedState, events: &E) -> bool {
        let _order = self
            .guard_order
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !is_running(shared, self.id, &self.cancellation) {
            return false;
        }
        self.guard_held.store(true, Ordering::Release);
        // Every progressive slice refreshes the external guard watchdog.
        events.emit(MonitorEvent::InputGuardRequested {
            session_id: self.id,
        });
        true
    }

    fn release_guard<E: EventSink>(&self, events: &E) {
        let _order = self
            .guard_order
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.guard_held.swap(false, Ordering::AcqRel) {
            events.emit(MonitorEvent::InputGuardReleased {
                session_id: self.id,
            });
        }
    }
}

/// Owns exactly one capture/OCR worker at a time.
///
/// `stop` invalidates the current session and requests OCR cancellation before it waits for the
/// worker. Consequently an OCR result that returns after stop began cannot claim MatchFound or
/// emit a detection.
pub struct Monitor<S, K, E>
where
    S: AffixSource,
    K: MonitorClock,
    E: EventSink,
{
    resources: Option<Resources<S, K>>,
    worker: Option<JoinHandle<Resources<S, K>>>,
    shared: Arc<SharedState>,
    events: Arc<E>,
}

impl<S, K, E> Monitor<S, K, E>
where
    S: AffixSource,
    K: MonitorClock,
    E: EventSink,
{
    pub fn new(source: S, clock: K, events: E) -> Self {
        Self {
            resources: Some(Resources { source, clock }),
            worker: None,
            shared: Arc::new(SharedState::default()),
            events: Arc::new(events),
        }
    }

    pub fn state(&self) -> MonitorState {
        self.shared
            .lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .state
    }

    pub fn current_session(&self) -> Option<SessionId> {
        self.shared
            .lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .current
            .as_ref()
            .map(|session| session.id)
    }

    pub fn start(&mut self, plan: MonitorPlan) -> Result<SessionId, StartError> {
        self.reap_finished()?;

        {
            let lifecycle = self
                .shared
                .lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if lifecycle.stop_in_progress {
                return Err(StartError::StopInProgress);
            }
            if self.worker.is_some() || lifecycle.state == MonitorState::Running {
                return Err(StartError::AlreadyRunning);
            }
        }

        if matches!(plan, MonitorPlan::Structured(_))
            && self
                .resources
                .as_ref()
                .expect("idle monitor retains its worker resources")
                .source
                .structured_support()
                == StructuredOcrSupport::Unsupported
        {
            return Err(StartError::StructuredOcrUnsupported);
        }

        let session = {
            let mut lifecycle = self
                .shared
                .lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let id = SessionId(lifecycle.next_session);
            lifecycle.next_session = lifecycle.next_session.saturating_add(1);
            let session = Arc::new(SessionControl::new(id));
            lifecycle.current = Some(Arc::clone(&session));
            lifecycle.state = MonitorState::Running;
            session
        };

        let resources = self
            .resources
            .take()
            .expect("idle monitor retains its worker resources");
        let shared = Arc::clone(&self.shared);
        let events = Arc::clone(&self.events);
        let session_id = session.id;
        self.worker = Some(thread::spawn(move || {
            run_worker(resources, plan, session, shared, events)
        }));
        Ok(session_id)
    }

    pub fn stop(&mut self) -> Result<(), StopError> {
        let session = {
            let mut lifecycle = self
                .shared
                .lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            lifecycle.stop_in_progress = true;
            let session = lifecycle.current.take();
            if let Some(session) = &session {
                session.cancellation.cancel();
            }
            session
        };

        // Releasing this output before join preserves the fail-open stop contract even when an
        // OCR adapter needs a short period to unwind cooperative cancellation.
        if let Some(session) = &session {
            session.release_guard(self.events.as_ref());
        }

        let join_result = self.worker.take().map(JoinHandle::join);
        let worker_panicked = match join_result {
            Some(Ok(resources)) => {
                self.resources = Some(resources);
                false
            }
            Some(Err(_)) => true,
            None => false,
        };

        let idle_session = session.as_ref().map_or(SessionId(0), |value| value.id);
        {
            let mut lifecycle = self
                .shared
                .lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            lifecycle.state = MonitorState::Idle;
            lifecycle.current = None;
            lifecycle.stop_in_progress = false;
        }
        self.events
            .emit(MonitorEvent::Snapshot(MonitorSnapshot::idle(idle_session)));

        if worker_panicked {
            Err(StopError)
        } else {
            Ok(())
        }
    }

    fn reap_finished(&mut self) -> Result<(), StartError> {
        let Some(worker) = self.worker.as_ref() else {
            return Ok(());
        };
        if !worker.is_finished() {
            return Err(StartError::AlreadyRunning);
        }
        let worker = self.worker.take().expect("worker was just observed");
        match worker.join() {
            Ok(resources) => {
                self.resources = Some(resources);
                Ok(())
            }
            Err(_) => Err(StartError::WorkerPanicked),
        }
    }
}

impl<S, K, E> Drop for Monitor<S, K, E>
where
    S: AffixSource,
    K: MonitorClock,
    E: EventSink,
{
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.stop();
        }
    }
}

fn run_worker<S, K, E>(
    mut resources: Resources<S, K>,
    plan: MonitorPlan,
    session: Arc<SessionControl>,
    shared: Arc<SharedState>,
    events: Arc<E>,
) -> Resources<S, K>
where
    S: AffixSource,
    K: MonitorClock,
    E: EventSink,
{
    run_loop(&mut resources, &plan, &session, &shared, events.as_ref());
    // Match callbacks synchronously latch the alert before returning. This release therefore
    // relinquishes only the monitor's short pre-recognition request.
    session.release_guard(events.as_ref());
    resources
}

fn run_loop<S, K, E>(
    resources: &mut Resources<S, K>,
    plan: &MonitorPlan,
    session: &SessionControl,
    shared: &SharedState,
    events: &E,
) where
    S: AffixSource,
    K: MonitorClock,
    E: EventSink,
{
    let mut scan_count = 0_u64;

    while is_running(shared, session.id, &session.cancellation) {
        let read_started = resources.clock.now();

        // Armed before the reading rather than after, because with a source
        // that answers in milliseconds the reading *is* the decision point.
        if !session.request_guard(shared, events) {
            return;
        }

        let reading = match resources.source.read(plan, &session.cancellation) {
            Ok(reading) => reading,
            Err(error) => {
                fault(shared, session, events, scan_count, error.to_string());
                return;
            }
        };
        let read_elapsed = resources.clock.now().saturating_sub(read_started);

        // Stop linearizes by invalidating this session before it waits for a
        // reading. Never evaluate or publish evidence that returned after that
        // point.
        if !is_running(shared, session.id, &session.cancellation) {
            return;
        }
        scan_count = scan_count.saturating_add(1);

        // The source reports that the item has not moved since the previous
        // reading, so evaluation would reach the verdict it already reached.
        if reading.was_cached {
            session.release_guard(events);
            let snapshot = running_snapshot(
                session.id,
                scan_count,
                read_elapsed,
                reading.total_elapsed(),
                reading.lines,
                true,
            );
            emit_running(shared, session, events, MonitorEvent::Snapshot(snapshot));
            resources
                .clock
                .pace(ScanPace::CachedDelay, &session.cancellation);
            continue;
        }

        if let Some(detection) = evaluate(plan, &reading, session.id, scan_count, read_elapsed) {
            if !try_claim_match(shared, session) {
                return;
            }
            let snapshot = detection.snapshot().clone();
            events.emit(MonitorEvent::Snapshot(snapshot));
            events.emit(MonitorEvent::Detection(detection));
            return;
        }

        let pace = if reading.requires_rescan {
            ScanPace::ProgressiveYield
        } else {
            session.release_guard(events);
            ScanPace::UncachedDelay
        };
        let snapshot = running_snapshot(
            session.id,
            scan_count,
            read_elapsed,
            reading.total_elapsed(),
            reading.lines,
            reading.was_cached,
        );
        emit_running(shared, session, events, MonitorEvent::Snapshot(snapshot));
        resources.clock.pace(pace, &session.cancellation);
    }
}

fn evaluate(
    plan: &MonitorPlan,
    recognition: &RecognitionResult,
    session_id: SessionId,
    scan_count: u64,
    capture_elapsed: Duration,
) -> Option<MonitorDetection> {
    match plan {
        MonitorPlan::Quick(target) => {
            let matched = target.find_match(&recognition.lines).or_else(|| {
                recognition
                    .target_assisted_match
                    .as_ref()
                    .filter(|candidate| candidate.canonical_text == target.template().text)
                    .cloned()
            })?;
            let snapshot = MonitorSnapshot {
                session_id,
                state: MonitorState::MatchFound,
                scan_count,
                capture_elapsed,
                ocr_elapsed: recognition.total_elapsed(),
                last_lines: recognition.lines.clone(),
                detail: Some(matched.original_text.clone()),
                ocr_was_cached: recognition.was_cached,
            };
            Some(MonitorDetection::Quick { matched, snapshot })
        }
        MonitorPlan::Structured(rules) => {
            let evaluation = rules.evaluate_with_identity(
                &recognition.lines,
                &recognition.assisted_observations,
                &recognition.physical_line_identities,
            );
            if !evaluation.is_match {
                return None;
            }
            let detected_text = structured_detail(&evaluation);
            let snapshot = MonitorSnapshot {
                session_id,
                state: MonitorState::MatchFound,
                scan_count,
                capture_elapsed,
                ocr_elapsed: recognition.total_elapsed(),
                last_lines: recognition.lines.clone(),
                detail: Some(detected_text.clone()),
                ocr_was_cached: recognition.was_cached,
            };
            Some(MonitorDetection::Structured {
                evaluation,
                detected_text,
                snapshot,
            })
        }
    }
}

fn structured_detail(evaluation: &RuleEvaluationResult) -> String {
    let Some(group) = evaluation.matched_group() else {
        return "Structured rule matched.".to_owned();
    };
    let mut seen = HashSet::new();
    let observations = group
        .conditions
        .iter()
        .filter(|condition| condition.is_matched)
        .filter_map(|condition| condition.observation.as_ref())
        .filter_map(|observation| {
            seen.insert(observation.original_text.clone())
                .then_some(observation.original_text.as_str())
        })
        .collect::<Vec<_>>();
    if observations.is_empty() {
        group.name.clone()
    } else {
        format!("{}: {}", group.name, observations.join(" + "))
    }
}

fn running_snapshot(
    session_id: SessionId,
    scan_count: u64,
    capture_elapsed: Duration,
    ocr_elapsed: Duration,
    last_lines: Vec<String>,
    ocr_was_cached: bool,
) -> MonitorSnapshot {
    MonitorSnapshot {
        session_id,
        state: MonitorState::Running,
        scan_count,
        capture_elapsed,
        ocr_elapsed,
        last_lines,
        detail: None,
        ocr_was_cached,
    }
}

fn is_running(
    shared: &SharedState,
    session_id: SessionId,
    cancellation: &CancellationToken,
) -> bool {
    if cancellation.is_cancelled() {
        return false;
    }
    let lifecycle = shared
        .lifecycle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    !lifecycle.stop_in_progress
        && lifecycle.state == MonitorState::Running
        && lifecycle
            .current
            .as_ref()
            .is_some_and(|current| current.id == session_id)
}

fn emit_running<E: EventSink>(
    shared: &SharedState,
    session: &SessionControl,
    events: &E,
    event: MonitorEvent,
) {
    if is_running(shared, session.id, &session.cancellation) {
        events.emit(event);
    }
}

fn try_claim_match(shared: &SharedState, session: &SessionControl) -> bool {
    let mut lifecycle = shared
        .lifecycle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if lifecycle.stop_in_progress
        || session.cancellation.is_cancelled()
        || lifecycle.state != MonitorState::Running
        || lifecycle
            .current
            .as_ref()
            .is_none_or(|current| current.id != session.id)
    {
        return false;
    }
    lifecycle.state = MonitorState::MatchFound;
    true
}

fn fault<E: EventSink>(
    shared: &SharedState,
    session: &SessionControl,
    events: &E,
    scan_count: u64,
    detail: String,
) {
    let claimed = {
        let mut lifecycle = shared
            .lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.stop_in_progress
            || session.cancellation.is_cancelled()
            || lifecycle.state != MonitorState::Running
            || lifecycle
                .current
                .as_ref()
                .is_none_or(|current| current.id != session.id)
        {
            false
        } else {
            lifecycle.state = MonitorState::Faulted;
            true
        }
    };
    if claimed {
        events.emit(MonitorEvent::Snapshot(MonitorSnapshot {
            session_id: session.id,
            state: MonitorState::Faulted,
            scan_count,
            capture_elapsed: Duration::ZERO,
            ocr_elapsed: Duration::ZERO,
            last_lines: Vec::new(),
            detail: Some(detail),
            ocr_was_cached: false,
        }));
    }
}
