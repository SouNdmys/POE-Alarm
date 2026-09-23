//! Offline replay of complete production ClipboardSource implementations.
//! Only platform functions and Instant::now are substituted by a virtual OS;
//! no real clipboard, hooks, input, sleeps or game are used.
#![allow(dead_code)]

include!(concat!(env!("OUT_DIR"), "/sources.rs"));

use std::collections::BTreeMap;
use std::time::Duration;

use poe_alarm_core::FullLineAffixMatcher;
use poe_alarm_monitoring::{AffixSource, CancellationToken, MonitorPlan};

mod fake_platform {
    use std::cell::RefCell;
    use std::fmt;
    use std::time::{Duration, Instant};

    #[derive(Clone, Copy, Debug, Default)]
    pub struct KeyMethod;
    pub struct ClickObserver;
    pub struct TimerResolutionGuard;
    #[derive(Debug)]
    pub enum ClipboardError {
        Timeout { waited: Duration },
        Busy { attempts: u32 },
        NoTextFormat { formats: Vec<String> },
        EmptyText,
    }
    impl ClipboardError {
        pub fn is_transient(&self) -> bool {
            true
        }
    }
    impl fmt::Display for ClipboardError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{self:?}")
        }
    }
    pub struct CopyOutcome {
        pub text: String,
        pub sequence_number: u32,
        pub client_round_trip: Duration,
        pub read_time: Duration,
        pub open_attempts: u32,
    }

    #[derive(Clone)]
    struct Response {
        at: u64,
        text: String,
    }
    pub struct World {
        origin: Instant,
        pub ms: u64,
        pub lag: u64,
        pub rtt: u64,
        pub click_at: Vec<u64>,
        pub clicks: u64,
        pub sequence: u32,
        text: String,
        responses: Vec<Response>,
        pub copy_at: Vec<u64>,
        pub no_start_item: bool,
        pub suppress_initial_baseline: bool,
        pub first_click_before_start_answer: bool,
        lag_jitter: Vec<i64>,
        rtt_jitter: Vec<i64>,
        wake_jitter: u64,
    }
    impl Default for World {
        fn default() -> Self {
            Self {
                origin: Instant::now(),
                ms: 0,
                lag: 30,
                rtt: 3,
                click_at: Vec::new(),
                clicks: 0,
                sequence: 1,
                text: item(0),
                responses: Vec::new(),
                copy_at: Vec::new(),
                no_start_item: false,
                suppress_initial_baseline: false,
                first_click_before_start_answer: false,
                lag_jitter: Vec::new(),
                rtt_jitter: Vec::new(),
                wake_jitter: 0,
            }
        }
    }
    thread_local! { static WORLD: RefCell<World> = RefCell::new(World::default()); }
    pub fn setup(lag: u64, rtt: u64, cadence: u64, count: usize, phase: u64) {
        WORLD.with(|world| {
            *world.borrow_mut() = World {
                lag,
                rtt,
                click_at: (0..count)
                    .map(|index| 200 + phase + index as u64 * cadence)
                    .collect(),
                ..Default::default()
            }
        });
    }
    pub fn set_clicks(times: &[u64]) {
        WORLD.with(|w| w.borrow_mut().click_at = times.to_vec());
    }
    pub fn configure_baseline(suppress: bool, read_after_click: bool) {
        WORLD.with(|w| {
            let mut w = w.borrow_mut();
            w.suppress_initial_baseline = suppress;
            w.first_click_before_start_answer = read_after_click;
        });
    }
    pub fn configure_jitter(lag: &[i64], rtt: &[i64], wake: u64) {
        WORLD.with(|w| {
            let mut w = w.borrow_mut();
            w.lag_jitter = lag.to_vec();
            w.rtt_jitter = rtt.to_vec();
            w.wake_jitter = wake;
        });
    }
    pub fn wake_jitter() -> u64 {
        WORLD.with(|w| w.borrow().wake_jitter)
    }
    fn displayed_roll(w: &World, at: u64) -> u64 {
        let mut last_ready = 0;
        let mut roll = 0;
        for (index, &click) in w.click_at.iter().enumerate() {
            let jitter = if w.lag_jitter.is_empty() {
                0
            } else {
                w.lag_jitter[index % w.lag_jitter.len()]
            };
            let lag = (w.lag as i64 + jitter).max(0) as u64;
            // Preserve server execution order even if network delays vary.
            last_ready = last_ready.max(click + lag);
            if last_ready <= at {
                roll = index as u64 + 1;
            } else {
                break;
            }
        }
        roll
    }
    fn item(roll: u64) -> String {
        format!(
            "Item Class: Rings\nRarity: Magic\nHealthy Gold Ring\nGold Ring\n--------\nItem Level: 83\n--------\n+{} to maximum Life",
            1000 + roll
        )
    }
    fn advance_world(w: &mut World, to: u64) {
        w.ms = to;
        w.clicks = w.click_at.iter().take_while(|&&at| at <= to).count() as u64;
        w.responses.sort_by_key(|response| response.at);
        while w
            .responses
            .first()
            .is_some_and(|response| response.at <= to)
        {
            let response = w.responses.remove(0);
            w.text = response.text;
            w.sequence += 1;
        }
    }
    pub fn advance(to: u64) {
        WORLD.with(|w| advance_world(&mut w.borrow_mut(), to));
    }
    pub fn now_ms() -> u64 {
        WORLD.with(|w| w.borrow().ms)
    }
    pub fn now() -> Instant {
        WORLD.with(|w| {
            let w = w.borrow();
            w.origin + Duration::from_millis(w.ms)
        })
    }
    pub fn observed_clicks() -> u64 {
        WORLD.with(|w| w.borrow().clicks)
    }
    pub fn sequence_number() -> u32 {
        WORLD.with(|w| w.borrow().sequence)
    }
    pub fn game_is_foreground() -> bool {
        true
    }
    pub fn game_process_outranks_us() -> bool {
        false
    }
    pub fn request_fine_timer_resolution() -> TimerResolutionGuard {
        TimerResolutionGuard
    }
    pub fn start_click_observer() -> Result<ClickObserver, String> {
        Ok(ClickObserver)
    }
    pub fn read_text(_: u32) -> Result<(String, u32), ClipboardError> {
        WORLD.with(|w| Ok((w.borrow().text.clone(), 1)))
    }
    pub fn copy_hovered_item(
        timeout: Duration,
        _: KeyMethod,
    ) -> Result<CopyOutcome, ClipboardError> {
        WORLD.with(|world| {
            let mut w = world.borrow_mut();
            let sent = w.ms;
            let old_sequence = w.sequence;
            let deadline = sent + timeout.as_millis() as u64;
            w.copy_at.push(sent);
            let baseline_suppressed = w.suppress_initial_baseline && w.clicks == 0;
            if !baseline_suppressed {
                // Snapshot the latest server-applied roll when the client handles
                // the copy chord. A pending answer can arrive after our deadline.
                let jitter = if w.rtt_jitter.is_empty() {
                    0
                } else {
                    w.rtt_jitter[(w.copy_at.len() - 1) % w.rtt_jitter.len()]
                };
                let rtt = (w.rtt as i64 + jitter).max(1) as u64;
                let read_at = if w.first_click_before_start_answer && sent == 0 {
                    sent + rtt
                } else {
                    sent
                };
                let roll = displayed_roll(&w, read_at);
                let at = sent + rtt;
                w.responses.push(Response {
                    at,
                    text: item(roll),
                });
            }
            let answer_at = w
                .responses
                .iter()
                .map(|response| response.at)
                .filter(|&at| at <= deadline)
                .min();
            let returned_at = answer_at.unwrap_or(deadline);
            advance_world(&mut w, returned_at);
            if w.sequence == old_sequence {
                Err(ClipboardError::Timeout { waited: timeout })
            } else {
                Ok(CopyOutcome {
                    text: w.text.clone(),
                    sequence_number: w.sequence,
                    client_round_trip: Duration::from_millis(returned_at - sent),
                    read_time: Duration::ZERO,
                    open_attempts: 1,
                })
            }
        })
    }
    pub fn copy_times() -> Vec<u64> {
        WORLD.with(|w| w.borrow().copy_at.clone())
    }
    pub fn click_times() -> Vec<u64> {
        WORLD.with(|w| w.borrow().click_at.clone())
    }
}

#[derive(Debug, Default)]
struct Outcome {
    reads: BTreeMap<u64, u64>,
    copies: Vec<u64>,
    clicks: Vec<u64>,
}

fn run<S: AffixSource>(mut source: S, end: u64, _deadline_pacing: bool) -> Outcome {
    let plan = MonitorPlan::Quick(FullLineAffixMatcher::new("+# to maximum Life").unwrap());
    let cancellation = CancellationToken::default();
    let mut outcome = Outcome::default();
    while fake_platform::now_ms() <= end {
        let result = source
            .read(&plan, &cancellation)
            .unwrap_or_else(|_| panic!("source failed"));
        let matched = match &plan {
            MonitorPlan::Quick(matcher) => matcher.find_match(&result.lines).is_some(),
            _ => false,
        };
        if !result.was_cached && matched {
            for line in &result.lines {
                if let Some(value) = line
                    .strip_prefix('+')
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    if value > 1000 {
                        outcome
                            .reads
                            .entry(value - 1000)
                            .or_insert(fake_platform::now_ms());
                    }
                }
            }
        }
        let wait = Duration::from_millis(10);
        fake_platform::advance(
            fake_platform::now_ms()
                + (wait.as_millis() as u64).max(1)
                + fake_platform::wake_jitter(),
        );
    }
    outcome.copies = fake_platform::copy_times();
    outcome.clicks = fake_platform::click_times();
    outcome
}

fn metrics(outcome: &Outcome) -> (usize, usize, u64) {
    let mut timely = 0;
    let mut max_latency = 0;
    for (&roll, &read_at) in &outcome.reads {
        let clicked_at = outcome.clicks[roll as usize - 1];
        let next_click = outcome
            .clicks
            .get(roll as usize)
            .copied()
            .unwrap_or(u64::MAX);
        if read_at < next_click {
            timely += 1;
        }
        max_latency = max_latency.max(read_at - clicked_at);
    }
    (outcome.reads.len(), timely, max_latency)
}

fn main() {
    println!(
        "Production-source A/B offline replay. Columns: observed/30, before next click, maximum click-to-evidence ms, copy count."
    );
    println!(
        "Assumptions: text becomes available after given lag; copy snapshots at send; extended mode includes fixed periodic jitter; classification/parser are production code. No input interception is simulated."
    );
    if std::env::var_os("SCHEDULER_EXTENDED").is_some() {
        for cadence in [100, 120, 150] {
            for lag in [30, 60, 90, 120, 135, 150, 180] {
                for rtt in [3, 8, 15] {
                    for phase in [0, 5] {
                        for jitter in [false, true] {
                            let prepare = || {
                                fake_platform::setup(lag, rtt, cadence, 30, phase);
                                if jitter {
                                    fake_platform::configure_jitter(
                                        &[0, 15, -10, 30, 0, -15],
                                        &[0, 7, 0, -2, 12, 0],
                                        1,
                                    );
                                }
                            };
                            let end = 200 + phase + cadence * 29 + 1000;
                            prepare();
                            let old = run(baseline::ClipboardSource::new(), end, false);
                            prepare();
                            let new = run(candidate::ClipboardSource::new(), end, true);
                            println!(
                                "cadence={cadence} lag={lag:3} rtt={rtt:2} phase={phase} jitter={jitter} baseline={:?}, copies={} candidate={:?}, copies={}",
                                metrics(&old),
                                old.copies.len(),
                                metrics(&new),
                                new.copies.len()
                            );
                        }
                    }
                }
            }
        }
        return;
    }
    for cadence in [100, 120, 150] {
        for lag in [30, 60, 90, 120] {
            for rtt in [3, 8, 15] {
                for phase in [0, 5] {
                    fake_platform::setup(lag, rtt, cadence, 30, phase);
                    let old = run(
                        baseline::ClipboardSource::new(),
                        200 + phase + cadence * 29 + 1000,
                        false,
                    );
                    fake_platform::setup(lag, rtt, cadence, 30, phase);
                    let new = run(
                        candidate::ClipboardSource::new(),
                        200 + phase + cadence * 29 + 1000,
                        true,
                    );
                    println!(
                        "cadence={cadence} lag={lag:3} rtt={rtt:2} phase={phase} baseline={:?}, copies={} candidate={:?}, copies={}",
                        metrics(&old),
                        old.copies.len(),
                        metrics(&new),
                        new.copies.len()
                    );
                }
            }
        }
    }
    for (name, lag, rtt, clicks, suppress, post) in [
        (
            "overlap while previous late copy is pending",
            30,
            15,
            vec![200, 292],
            false,
            false,
        ),
        (
            "first click inside baseline answer",
            0,
            8,
            vec![4],
            false,
            true,
        ),
        (
            "first click after no startup baseline",
            30,
            15,
            vec![200],
            true,
            false,
        ),
    ] {
        fake_platform::setup(lag, rtt, 150, 0, 0);
        fake_platform::set_clicks(&clicks);
        fake_platform::configure_baseline(suppress, post);
        let old = run(baseline::ClipboardSource::new(), 1000, false);
        fake_platform::setup(lag, rtt, 150, 0, 0);
        fake_platform::set_clicks(&clicks);
        fake_platform::configure_baseline(suppress, post);
        let new = run(candidate::ClipboardSource::new(), 1000, true);
        println!(
            "special={name}: baseline reads={:?}, copies={:?}; candidate reads={:?}, copies={:?}",
            old.reads, old.copies, new.reads, new.copies
        );
    }
}
