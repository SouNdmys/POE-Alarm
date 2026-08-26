//! Clipboard-only recognition loop, measured against the OCR pipeline's budget.
//!
//! Per click:
//!
//! 1. pass the click, immediately enter DECIDING (nothing else gets through)
//! 2. Ctrl+C, read the text, compare it to the text from before the click
//! 3. same text -> the craft has not resolved yet, ask again
//! 4. different text -> that is the new roll; run the user's rules on it
//! 5. hit -> stay locked and alert; miss -> release; anything else -> stay
//!    locked and say why
//!
//! Step 2 is the whole change detector. There is no capture region, no mask,
//! no fingerprint, and no tolerance to tune, so there is nothing to
//! recalibrate when the scene behind the tooltip changes.
//!
//! The question this binary exists to answer is step 3's cost: how many extra
//! round trips the server makes us spend, and whether the total still lands
//! inside the budget the OCR path already meets.

#[cfg(not(windows))]
fn main() {
    eprintln!("clip-only-lab only runs on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
mod app {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey, VK_F9, VK_F12,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, MSG, MSLLHOOKSTRUCT,
        PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL,
        WM_HOTKEY, WM_QUIT,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL,
        WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN,
    };

    use poe_alarm_clip_only::clipboard::{
        self, ClipboardError, KeyMethod, SYNTHETIC_INPUT_SIGNATURE, game_is_foreground,
    };
    use poe_alarm_clip_only::stats::{LatencySamples, format_millis};
    use poe_alarm_clip_only::{LabProfile, Verdict, describes_same_roll, evaluate_payload};

    const STATE_IDLE: u8 = 0;
    const STATE_DECIDING: u8 = 1;
    const STATE_LOCKED: u8 = 2;

    const HOTKEY_RELEASE: i32 = 0x5041;
    const HOTKEY_QUIT: i32 = 0x5042;
    const IDLE_TICK: Duration = Duration::from_millis(40);

    static STATE: AtomicU8 = AtomicU8::new(STATE_IDLE);
    static PASS_MATCHING_UP: AtomicBool = AtomicBool::new(false);
    static GAME_FOREGROUND: AtomicBool = AtomicBool::new(false);
    static SWALLOWED: AtomicU64 = AtomicU64::new(0);
    static RUNNING: AtomicBool = AtomicBool::new(true);
    static CLICK_TX: OnceLock<SyncSender<Instant>> = OnceLock::new();
    static HOOK_EVENTS: AtomicU64 = AtomicU64::new(0);
    static LBUTTON_DOWNS: AtomicU64 = AtomicU64::new(0);
    static CLICKS_DROPPED: AtomicU64 = AtomicU64::new(0);
    /// True while the current lock came from a failure rather than a hit. Only
    /// these auto-release: a diagnostic run must not wedge the mouse after one
    /// bad copy, but a real hit must stay locked until the user looks at it.
    static ERROR_LOCKED: AtomicBool = AtomicBool::new(false);
    /// Thread running the message pump, so the worker can end the session.
    static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
    /// Pass every click through untouched. Blocking costs the user roughly
    /// three clicks per applied orb at spam speed, which contaminates any
    /// attempt to measure how long the craft itself takes.
    static OBSERVE_ONLY: AtomicBool = AtomicBool::new(false);

    /// Per-message-type counts. "Moves arrive but presses do not" is a real
    /// enough outcome that it needs to be visible rather than inferred: swapped
    /// mouse buttons, for instance, deliver a physical left click to the hook
    /// as WM_RBUTTONDOWN.
    static MOVES: AtomicU64 = AtomicU64::new(0);
    static L_UPS: AtomicU64 = AtomicU64::new(0);
    static R_DOWNS: AtomicU64 = AtomicU64::new(0);
    static R_UPS: AtomicU64 = AtomicU64::new(0);
    static M_DOWNS: AtomicU64 = AtomicU64::new(0);
    static X_DOWNS: AtomicU64 = AtomicU64::new(0);
    static WHEELS: AtomicU64 = AtomicU64::new(0);
    static OTHER_MESSAGES: AtomicU64 = AtomicU64::new(0);
    /// Events Windows marked as injected by software rather than hardware.
    static INJECTED: AtomicU64 = AtomicU64::new(0);

    /// `LLMHF_INJECTED`.
    const LLMHF_INJECTED: u32 = 0x0000_0001;

    pub struct Options {
        /// Latency the OCR pipeline already meets, used as the pass mark.
        pub budget_ms: u64,
        /// Per-copy deadline. A healthy round trip is ~3ms and p99 is under
        /// 10ms, so anything longer is the client declining to answer rather
        /// than answering slowly — and it declines for the whole time a craft
        /// is in flight. The moment it answers again is the moment the new
        /// roll exists, so this deadline is what sets detection lag: measured
        /// staleness tracked it almost exactly (60ms deadline -> 81ms p50).
        pub copy_timeout_ms: u64,
        /// Overall deadline from the click.
        pub deadline_ms: u64,
        /// First pause between copies while waiting for the craft to resolve.
        /// Grows geometrically up to `poll_gap_max_ms`: a tight loop sends the
        /// client ~80 copies a second, each one also toggling the held Shift,
        /// which is both wasteful and a plausible way to disturb the game.
        pub poll_gap_ms: u64,
        /// Ceiling for the backoff.
        pub poll_gap_max_ms: u64,
        /// Delay before the very first copy, to skip a doomed early attempt.
        pub first_delay_ms: u64,
        /// How Ctrl+C is synthesized.
        pub key_method: KeyMethod,
        /// How long an error lock holds before releasing itself. Hit locks are
        /// never auto-released; this only stops a diagnostic run from wedging
        /// the mouse after a single failure.
        pub unlock_after_ms: u64,
        /// Standalone clipboard check: copy this many times and report.
        pub test_copy: Option<usize>,
        /// Floor on how soon a text change can plausibly be caused by the
        /// click. Applying currency is server-authoritative, so a change seen
        /// sooner than this is left over from an earlier craft; adopting it
        /// would report every roll one behind.
        pub min_craft_ms: u64,
        /// Require two consecutive identical reads before judging, so an
        /// intermediate render is never mistaken for the final roll.
        pub settle: bool,
        /// Measure how long the craft actually takes, without blocking a
        /// single click and with two copies per click instead of eight.
        pub observe: bool,
        /// Block every click until a verdict exists. Measured at ~2.9 presses
        /// per applied orb, so it is off by default; recognition accuracy and
        /// interception policy are independent choices and conflating them was
        /// what made this feel unusable.
        pub block_first: bool,
        /// How long after a click to keep polling for a new roll.
        pub active_window_ms: u64,
        /// Gap between polls while crafting is active.
        pub watch_interval_ms: u64,
    }

    impl Default for Options {
        fn default() -> Self {
            Self {
                budget_ms: 120,
                copy_timeout_ms: 25,
                deadline_ms: 1_500,
                poll_gap_ms: 12,
                poll_gap_max_ms: 60,
                first_delay_ms: 0,
                key_method: KeyMethod::VirtualKey,
                unlock_after_ms: 4_000,
                test_copy: None,
                min_craft_ms: 25,
                settle: true,
                observe: false,
                block_first: false,
                active_window_ms: 1_200,
                watch_interval_ms: 40,
            }
        }
    }

    /// Low-level mouse hook. Reads cached atomics and returns immediately;
    /// Windows drops hooks whose callback stalls, so no clipboard work here.
    unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code < 0 {
            // SAFETY: forwarding the chain with the parameters we were handed.
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        HOOK_EVENTS.fetch_add(1, Ordering::Relaxed);
        let message = wparam.0 as u32;
        match message {
            WM_MOUSEMOVE => &MOVES,
            WM_LBUTTONDOWN => &LBUTTON_DOWNS,
            WM_LBUTTONUP => &L_UPS,
            WM_RBUTTONDOWN => &R_DOWNS,
            WM_RBUTTONUP => &R_UPS,
            WM_MBUTTONDOWN => &M_DOWNS,
            WM_XBUTTONDOWN => &X_DOWNS,
            WM_MOUSEWHEEL => &WHEELS,
            _ => &OTHER_MESSAGES,
        }
        .fetch_add(1, Ordering::Relaxed);

        // SAFETY: for WH_MOUSE_LL with code >= 0, lparam points at a live
        // MSLLHOOKSTRUCT owned by the system for this call.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if info.flags & LLMHF_INJECTED != 0 {
            INJECTED.fetch_add(1, Ordering::Relaxed);
        }

        if !GAME_FOREGROUND.load(Ordering::Acquire) {
            STATE.store(STATE_IDLE, Ordering::Release);
            // SAFETY: as above.
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }

        if info.dwExtraInfo == SYNTHETIC_INPUT_SIGNATURE {
            // SAFETY: as above.
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }

        match wparam.0 as u32 {
            WM_LBUTTONDOWN if OBSERVE_ONLY.load(Ordering::Acquire)
                && STATE.load(Ordering::Acquire) != STATE_LOCKED =>
            {
                if let Some(sender) = CLICK_TX.get() {
                    let _ = sender.try_send(Instant::now());
                }
                // SAFETY: as above.
                unsafe { CallNextHookEx(None, code, wparam, lparam) }
            }
            WM_LBUTTONDOWN => {
                if STATE
                    .compare_exchange(
                        STATE_IDLE,
                        STATE_DECIDING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    PASS_MATCHING_UP.store(true, Ordering::Release);
                    match CLICK_TX.get().map(|sender| sender.try_send(Instant::now())) {
                        Some(Ok(())) => {}
                        _ => {
                            // Nobody will decide, so holding DECIDING would
                            // swallow every later click.
                            CLICKS_DROPPED.fetch_add(1, Ordering::Relaxed);
                            STATE.store(STATE_IDLE, Ordering::Release);
                        }
                    }
                    // SAFETY: as above.
                    return unsafe { CallNextHookEx(None, code, wparam, lparam) };
                }
                SWALLOWED.fetch_add(1, Ordering::Relaxed);
                LRESULT(1)
            }
            WM_LBUTTONUP => {
                if PASS_MATCHING_UP.swap(false, Ordering::AcqRel)
                    || STATE.load(Ordering::Acquire) == STATE_IDLE
                {
                    // SAFETY: as above.
                    return unsafe { CallNextHookEx(None, code, wparam, lparam) };
                }
                LRESULT(1)
            }
            // SAFETY: as above.
            _ => unsafe { CallNextHookEx(None, code, wparam, lparam) },
        }
    }

    struct Totals {
        first_copy: LatencySamples,
        decision: LatencySamples,
        copies_per_roll: Vec<u32>,
        first_change: LatencySamples,
        stale_discarded: u64,
        rolls: u64,
        hits: u64,
        fail_closed: u64,
    }

    impl Totals {
        fn new() -> Self {
            Self {
                first_copy: LatencySamples::with_capacity(512),
                decision: LatencySamples::with_capacity(512),
                copies_per_roll: Vec::with_capacity(512),
                first_change: LatencySamples::with_capacity(512),
                stale_discarded: 0,
                rolls: 0,
                hits: 0,
                fail_closed: 0,
            }
        }
    }

    /// One-line summary of what the hook actually received, by message type.
    fn input_breakdown() -> String {
        format!(
            "moves {}  L(down {} up {})  R(down {} up {})  M {}  X {}  wheel {}  other {}  injected {}",
            MOVES.load(Ordering::Relaxed),
            LBUTTON_DOWNS.load(Ordering::Relaxed),
            L_UPS.load(Ordering::Relaxed),
            R_DOWNS.load(Ordering::Relaxed),
            R_UPS.load(Ordering::Relaxed),
            M_DOWNS.load(Ordering::Relaxed),
            X_DOWNS.load(Ordering::Relaxed),
            WHEELS.load(Ordering::Relaxed),
            OTHER_MESSAGES.load(Ordering::Relaxed),
            INJECTED.load(Ordering::Relaxed),
        )
    }

    fn lock(reason: &str, recoverable: bool) {
        ERROR_LOCKED.store(recoverable, Ordering::Release);
        STATE.store(STATE_LOCKED, Ordering::Release);
        println!("        LOCKED — {reason}");
        println!("        press Ctrl+Shift+F12 to release");
    }

    fn release() {
        ERROR_LOCKED.store(false, Ordering::Release);
        STATE.store(STATE_IDLE, Ordering::Release);
    }

    /// Sleeps for the current gap and widens it for the next attempt.
    fn back_off(gap: &mut u64, options: &Options) {
        if *gap > 0 {
            std::thread::sleep(Duration::from_millis(*gap));
        }
        *gap = (*gap * 3 / 2).clamp(1, options.poll_gap_max_ms.max(1));
    }

    /// One click: poll the clipboard until the item text changes, then judge.
    fn handle_roll(
        profile: &LabProfile,
        baseline: &mut Option<String>,
        totals: &mut Totals,
        click_at: Instant,
        options: &Options,
    ) {
        totals.rolls += 1;
        let index = totals.rolls;
        let deadline = Duration::from_millis(options.deadline_ms);
        let copy_timeout = Duration::from_millis(options.copy_timeout_ms);
        let mut copies = 0_u32;
        let mut first_copy: Option<Duration> = None;
        let mut last_error: Option<ClipboardError> = None;
        let mut gap = options.poll_gap_ms;
        let mut candidate: Option<String> = None;
        let mut first_change_at: Option<Duration> = None;
        let min_craft = Duration::from_millis(options.min_craft_ms);

        if options.first_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(options.first_delay_ms));
        }

        loop {
            if click_at.elapsed() >= deadline {
                totals.fail_closed += 1;
                let detail = match &last_error {
                    Some(error) => format!("last error: {error}"),
                    None => "the text never changed".to_string(),
                };
                lock(
                    &format!(
                        "gave up after {copies} copies in {}ms — {detail}",
                        options.deadline_ms
                    ),
                    true,
                );
                if matches!(
                    last_error,
                    Some(ClipboardError::NoTextFormat { .. }) | Some(ClipboardError::EmptyText)
                ) {
                    println!(
                        "        The client answered but had nothing to copy. Ctrl+C copies"
                    );
                    println!(
                        "        whatever the cursor is over; holding a currency orb, or"
                    );
                    println!(
                        "        drifting off the item, both produce exactly this."
                    );
                }
                return;
            }

            copies += 1;
            let outcome = match clipboard::copy_hovered_item(copy_timeout, options.key_method) {
                Ok(outcome) => outcome,
                // A missing payload usually means the client had nothing to
                // give yet, which the next copy can fix. Only give up on it
                // when the whole deadline expires.
                Err(error) if error.is_transient() => {
                    last_error = Some(error);
                    back_off(&mut gap, options);
                    continue;
                }
                Err(error) => {
                    totals.fail_closed += 1;
                    lock(&format!("clipboard round trip failed: {error}"), true);
                    return;
                }
            };
            if first_copy.is_none() {
                first_copy = Some(outcome.total());
            }

            // Borrow ends before the branch that reassigns the baseline.
            let unchanged = baseline
                .as_deref()
                .map(|previous| describes_same_roll(previous, &outcome.text));

            match unchanged {
                // First click of the session: this read establishes what the
                // item looked like before the orb landed.
                None => *baseline = Some(outcome.text),
                // The server has not resolved the craft yet. Ask again.
                Some(true) => back_off(&mut gap, options),
                Some(false) => {
                    let seen_at = click_at.elapsed();
                    if first_change_at.is_none() {
                        first_change_at = Some(seen_at);
                        totals.first_change.push(seen_at);
                    }

                    // Too early to have been caused by this click, so it is a
                    // leftover from the previous craft. Take it as the new
                    // baseline and keep waiting for the real one.
                    if seen_at < min_craft {
                        totals.stale_discarded += 1;
                        *baseline = Some(outcome.text);
                        candidate = None;
                        back_off(&mut gap, options);
                        continue;
                    }

                    // Require the text to stop moving before judging it.
                    if options.settle && candidate.as_deref() != Some(outcome.text.as_str()) {
                        candidate = Some(outcome.text);
                        back_off(&mut gap, options);
                        continue;
                    }

                    let verdict = evaluate_payload(&profile.rules, &outcome.text);
                    let decided = click_at.elapsed();
                    totals.decision.push(decided);
                    totals.copies_per_roll.push(copies);
                    if let Some(first) = first_copy {
                        totals.first_copy.push(first);
                    }
                    *baseline = Some(outcome.text);

                    let modifiers = outcome.suppressed_modifiers;
                    println!(
                        "  #{index:<4} copies {copies:<3} first {:<8} verdict {:<8} {:<5} ({} lines){}",
                        format_millis(first_copy.unwrap_or_default()),
                        format_millis(decided),
                        verdict.label(),
                        verdict.affix_count(),
                        if modifiers.any() {
                            format!("  [held {modifiers}]")
                        } else {
                            String::new()
                        }
                    );

                    match verdict {
                        Verdict::Hit { item, evaluation } => {
                            totals.hits += 1;
                            println!("\x07");
                            println!("  ==========================================================");
                            println!("   HIT on roll #{index}");
                            if let Some(group) = evaluation.matched_group() {
                                println!("   matched group: {}", group.name);
                            }
                            for line in &item.affix_lines {
                                println!("     {line}");
                            }
                            println!("  ==========================================================");
                            lock("target affix reached", false);
                        }
                        Verdict::Miss { .. } => {
                            if copies > 12 {
                                println!(
                                    "        note: {copies} copies for one roll — the craft took a while"
                                );
                            }
                            release();
                        }
                        Verdict::Unreadable(error) => {
                            totals.fail_closed += 1;
                            lock(&format!("clipboard payload unreadable: {error}"), true);
                        }
                    }
                    return;
                }
            }
        }
    }

    /// Delays probed, in milliseconds. Rotating through them builds the whole
    /// curve in one session instead of forcing a run per delay.
    const OBSERVE_DELAYS: [u64; 8] = [40, 70, 100, 150, 220, 320, 460, 650];

    /// Measures how long the client actually takes to show a new roll.
    ///
    /// Blocks nothing and spends exactly two copies per click: one immediately,
    /// which reliably still shows the pre-craft item, and one after a delay.
    /// Comparing those two is self-contained per click, so a drifting baseline
    /// cannot confound it the way the polling loop's could.
    fn observe(profile: &LabProfile, clicks: &Receiver<Instant>, options: &Options) {
        let copy_timeout = Duration::from_millis(options.copy_timeout_ms);
        let mut attempts = [0_u32; OBSERVE_DELAYS.len()];
        let mut changed = [0_u32; OBSERVE_DELAYS.len()];
        let mut failures = 0_u64;
        let mut index = 0_usize;
        let mut armed = false;
        let mut observed = 0_u64;

        while RUNNING.load(Ordering::Acquire) {
            match clicks.recv_timeout(IDLE_TICK) {
                Ok(_) => {
                    let slot = index % OBSERVE_DELAYS.len();
                    index += 1;
                    let delay = OBSERVE_DELAYS[slot];

                    let before =
                        match clipboard::copy_hovered_item(copy_timeout, options.key_method) {
                            Ok(outcome) => outcome.text,
                            Err(_) => {
                                failures += 1;
                                continue;
                            }
                        };
                    std::thread::sleep(Duration::from_millis(delay));
                    let after =
                        match clipboard::copy_hovered_item(copy_timeout, options.key_method) {
                            Ok(outcome) => outcome.text,
                            Err(_) => {
                                failures += 1;
                                continue;
                            }
                        };

                    attempts[slot] += 1;
                    observed += 1;
                    let moved = !describes_same_roll(&before, &after);
                    if moved {
                        changed[slot] += 1;
                    }
                    println!(
                        "  observed #{observed:<4} waited {delay:>4}ms  {}",
                        if moved { "CHANGED" } else { "unchanged" }
                    );
                }
                Err(_) => {
                    let foreground = game_is_foreground();
                    GAME_FOREGROUND.store(foreground, Ordering::Release);
                    if foreground != armed {
                        armed = foreground;
                        println!(
                            "  [{}] {}",
                            if armed { "armed" } else { "idle" },
                            if armed {
                                "game focused — every click passes through untouched"
                            } else {
                                "game lost focus"
                            }
                        );
                    }
                }
            }
        }

        println!();
        println!("=== how long the craft actually takes ===");
        println!("  clicks observed           {observed}");
        println!("  copy failures             {failures}");
        println!();
        println!("  delay    changed / tried    share");
        let mut resolved_by = None;
        for (slot, delay) in OBSERVE_DELAYS.iter().enumerate() {
            let tried = attempts[slot];
            if tried == 0 {
                println!("  {delay:>4}ms   no samples");
                continue;
            }
            let share = f64::from(changed[slot]) / f64::from(tried);
            let bar = "#".repeat((share * 30.0).round() as usize);
            println!(
                "  {delay:>4}ms   {:>3} / {:<3}          {:>5.1}% {bar}",
                changed[slot], tried, share * 100.0
            );
            if share >= 0.9 && resolved_by.is_none() {
                resolved_by = Some(*delay);
            }
        }
        println!();
        match resolved_by {
            Some(delay) => {
                println!("  By {delay}ms, 90% of crafts had resolved. That is the real floor:");
                println!("  no recognizer of any kind can decide sooner, OCR included.");
                if delay > 150 {
                    println!();
                    println!("  It is well past the {}ms the OCR path is credited with, which", options.budget_ms);
                    println!("  means that number is measuring something other than 'the new");
                    println!("  affixes were readable'. Worth checking before comparing further.");
                }
            }
            None => {
                println!("  No delay reached 90%. Either the sample is too small, or the craft");
                println!("  regularly takes longer than {}ms.", OBSERVE_DELAYS[OBSERVE_DELAYS.len() - 1]);
            }
        }
        let _ = profile;
    }

    /// Watches for a new roll without ever holding a click back.
    ///
    /// This mirrors the policy the OCR build already ships and the user has
    /// already lived with: clicks flow, and only a confirmed hit locks. The
    /// clipboard replaces the recognizer, nothing else. Polling only runs while
    /// crafting is actually happening, so the clipboard is left alone the rest
    /// of the time.
    fn watch(profile: &LabProfile, clicks: &Receiver<Instant>, options: &Options) {
        let copy_timeout = Duration::from_millis(options.copy_timeout_ms);
        let active_window = Duration::from_millis(options.active_window_ms);
        let interval = Duration::from_millis(options.watch_interval_ms);

        let mut last_click: Option<Instant> = None;
        let mut last_text: Option<String> = None;
        let mut armed = false;
        let mut rolls_seen = 0_u64;
        let mut hits = 0_u64;
        let mut failures = 0_u64;
        let mut detect_latency = LatencySamples::with_capacity(512);
        // The honest detection metric. `click -> detected` shrinks with the
        // user's click rate and says nothing about the recognizer; the interval
        // between the poll that missed a roll and the poll that caught it
        // bounds how stale the alarm can be, and depends on nothing else.
        let mut staleness = LatencySamples::with_capacity(512);
        let mut poll_gap = LatencySamples::with_capacity(2048);
        let mut last_poll_at: Option<Instant> = None;
        let mut timeouts = 0_u64;
        let mut empties = 0_u64;
        let mut busy = 0_u64;

        while RUNNING.load(Ordering::Acquire) {
            // Drain whatever clicks arrived; only the most recent matters.
            while let Ok(at) = clicks.try_recv() {
                last_click = Some(at);
            }

            let foreground = game_is_foreground();
            GAME_FOREGROUND.store(foreground, Ordering::Release);
            if foreground != armed {
                armed = foreground;
                println!(
                    "  [{}] {}",
                    if armed { "armed" } else { "idle" },
                    if armed {
                        "game focused — clicks pass freely, only a hit will lock"
                    } else {
                        "game lost focus"
                    }
                );
                if !armed {
                    last_text = None;
                    last_poll_at = None;
                    if STATE.load(Ordering::Acquire) != STATE_IDLE {
                        release();
                    }
                }
            }

            let crafting = armed
                && STATE.load(Ordering::Acquire) != STATE_LOCKED
                && last_click.is_some_and(|at| at.elapsed() < active_window);
            if !crafting {
                // Not polling, so the next gap would span the whole pause and
                // report as detection lag it never caused.
                last_poll_at = None;
                if let Ok(at) = clicks.recv_timeout(IDLE_TICK) {
                    last_click = Some(at);
                }
                continue;
            }

            let outcome = match clipboard::copy_hovered_item(copy_timeout, options.key_method) {
                Ok(outcome) => outcome,
                Err(error) if error.is_transient() => {
                    failures += 1;
                    match error {
                        ClipboardError::Timeout { .. } => timeouts += 1,
                        ClipboardError::Busy { .. } => busy += 1,
                        _ => empties += 1,
                    }
                    // A refusal is not a reason to stop watching. Retry
                    // promptly rather than idling the full interval, so a
                    // stretch of silence does not become a blind spot.
                    std::thread::sleep(Duration::from_millis(4));
                    continue;
                }
                Err(error) => {
                    println!("  [error] {error}");
                    failures += 1;
                    std::thread::sleep(interval);
                    continue;
                }
            };
            let gap = last_poll_at.map(|at| at.elapsed());
            last_poll_at = Some(Instant::now());
            if let Some(gap) = gap {
                poll_gap.push(gap);
            }

            let unchanged = last_text
                .as_deref()
                .is_some_and(|previous| describes_same_roll(previous, &outcome.text));
            if unchanged {
                std::thread::sleep(interval);
                continue;
            }

            let first_read = last_text.is_none();
            last_text = Some(outcome.text.clone());
            if first_read {
                // Nothing to compare against yet; this only establishes state.
                continue;
            }

            rolls_seen += 1;
            let since_click = last_click.map(|at| at.elapsed()).unwrap_or_default();
            detect_latency.push(since_click);
            if let Some(gap) = gap {
                staleness.push(gap);
            }
            let verdict = evaluate_payload(&profile.rules, &outcome.text);
            println!(
                "  roll {rolls_seen:<4} seen {:<9} {:<5} ({} lines)",
                format_millis(since_click),
                verdict.label(),
                verdict.affix_count()
            );

            if let Verdict::Hit { item, evaluation } = verdict {
                hits += 1;
                println!("");
                println!("  ==========================================================");
                println!("   HIT — THIS IS THE ALARM FIRING. Clicks are blocked now.");
                if let Some(group) = evaluation.matched_group() {
                    println!("   matched group: {}", group.name);
                }
                for line in &item.affix_lines {
                    println!("     {line}");
                }
                println!("   Press Ctrl+Shift+F12 to release.");
                println!("  ==========================================================");
                lock("target affix reached", false);
            }
            std::thread::sleep(interval);
        }

        println!();
        println!("=== session summary (detect-then-block) ===");
        println!("  rolls seen                {rolls_seen}");
        println!("  hits                      {hits}");
        println!("  copy failures             {failures}");
        println!(
            "  clicks swallowed          {}  (only a hit blocks anything)",
            SWALLOWED.load(Ordering::Relaxed)
        );
        println!();
        println!("  {}", input_breakdown());
        println!();
        println!("  failures by kind          timeout {timeouts}  empty {empties}  busy {busy}");
        println!(
            "  per-copy timeout          {}ms  (a refusal costs this much blindness)",
            options.copy_timeout_ms
        );
        println!();
        println!("{}", staleness.summary("detection staleness"));
        println!("{}", poll_gap.summary("gap between polls"));
        println!("{}", detect_latency.summary("click -> detected (see note)"));
        println!();
        println!("  READ THIS ONE: 'detection staleness' is the window between the poll");
        println!("  that missed the new roll and the poll that caught it. The roll became");
        println!("  visible somewhere inside it, so the alarm is at worst that late. It is");
        println!("  the only number here that depends on this tool rather than on you.");
        println!();
        println!(
            "  It is dominated by --watch-interval-ms (currently {}ms) plus the ~3ms",
            options.watch_interval_ms
        );
        println!("  round trip, so it is a dial, not a limit. Halve the interval to halve it.");
        println!();
        println!("  'click -> detected' is measured from your most recent click, so it");
        println!("  shrinks as you click faster and proves nothing about speed. Ignore it.");
    }

    fn worker(profile: LabProfile, clicks: Receiver<Instant>, options: Options) {
        if options.observe {
            OBSERVE_ONLY.store(true, Ordering::Release);
            observe(&profile, &clicks, &options);
            return;
        }
        if !options.block_first {
            OBSERVE_ONLY.store(true, Ordering::Release);
            watch(&profile, &clicks, &options);
            return;
        }
        let mut totals = Totals::new();
        let mut baseline: Option<String> = None;
        let mut armed = false;
        let mut last_heartbeat = Instant::now();
        let mut last_events = 0_u64;
        let mut error_lock_at: Option<Instant> = None;
        let mut blind_heartbeats = 0_u32;

        while RUNNING.load(Ordering::Acquire) {
            match clicks.recv_timeout(IDLE_TICK) {
                Ok(click_at) => {
                    handle_roll(&profile, &mut baseline, &mut totals, click_at, &options);
                    error_lock_at = ERROR_LOCKED
                        .load(Ordering::Acquire)
                        .then(Instant::now);
                }
                Err(_) => {
                    let foreground = game_is_foreground();
                    GAME_FOREGROUND.store(foreground, Ordering::Release);
                    if foreground != armed {
                        armed = foreground;
                        if armed {
                            println!("  [armed] game focused — clicks are now being watched");
                        } else {
                            let (title, class) = clipboard::foreground_window_description();
                            println!(
                                "  [idle] game lost focus (now \"{title}\" / {class}) — not intercepting"
                            );
                            // The next click starts a fresh comparison; the item
                            // under the cursor may well be a different one.
                            baseline = None;
                        }
                    }
                    if armed && last_heartbeat.elapsed() >= Duration::from_secs(3) {
                        let events = HOOK_EVENTS.load(Ordering::Relaxed);
                        if events == last_events {
                            blind_heartbeats += 1;
                            println!(
                                "  [watching] hook saw 0 mouse events in 3s ({blind_heartbeats}/3)."
                            );
                            if blind_heartbeats >= 3 {
                                println!();
                                println!("  ============================================================");
                                println!("   GIVING UP — the hook is installed but receives nothing");
                                println!("   while the game is focused.");
                                println!();
                                match clipboard::process_is_elevated() {
                                    Some(false) => {
                                        println!("   This process is NOT elevated and Path of Exile almost");
                                        println!("   certainly is. Windows skips a low-integrity hook for");
                                        println!("   input aimed at an elevated window, so no click can");
                                        println!("   ever reach us.");
                                        println!();
                                        println!("   Fix: close this window. Press Win, type terminal,");
                                        println!("   right-click it, choose Run as administrator, then:");
                                        println!(r"     cd <repo>");
                                        println!(r"     rust\target\release\clip-only-lab.exe");
                                        println!("   The title bar must read \"Administrator:\".");
                                    }
                                    _ => {
                                        println!("   This process IS elevated, so elevation is not the");
                                        println!("   cause. Check for mouse software or an overlay taking");
                                        println!("   input below the hook layer.");
                                    }
                                }
                                println!("  ============================================================");
                                RUNNING.store(false, Ordering::Release);
                                let thread = HOOK_THREAD_ID.load(Ordering::Acquire);
                                if thread != 0 {
                                    // SAFETY: posting WM_QUIT to our own pump thread.
                                    let _ = unsafe {
                                        PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0))
                                    };
                                }
                            }
                        } else {
                            blind_heartbeats = 0;
                            println!("  [watching] {}  state {}", input_breakdown(), match STATE
                                .load(Ordering::Acquire)
                            {
                                STATE_DECIDING => "deciding",
                                STATE_LOCKED => "LOCKED",
                                _ => "idle",
                            });
                        }
                        last_events = events;
                        last_heartbeat = Instant::now();
                    }
                    if !foreground && STATE.load(Ordering::Acquire) != STATE_IDLE {
                        release();
                        error_lock_at = None;
                    } else if ERROR_LOCKED.load(Ordering::Acquire)
                        && error_lock_at.is_some_and(|at: Instant| {
                            at.elapsed() >= Duration::from_millis(options.unlock_after_ms)
                        })
                    {
                        release();
                        error_lock_at = None;
                        println!("  [auto-released] the failure lock expired — clicks pass again");
                    }
                }
            }
        }

        report(&totals, &options);
    }

    fn report(totals: &Totals, options: &Options) {
        println!();
        println!("=== session summary ===");
        println!("  rolls decided             {}", totals.rolls);
        println!("  hits                      {}", totals.hits);
        println!("  fail-closed locks         {}", totals.fail_closed);
        let swallowed = SWALLOWED.load(Ordering::Relaxed);
        println!("  clicks swallowed          {swallowed}");
        if totals.rolls > 0 {
            let per_orb = (swallowed + totals.rolls) as f64 / totals.rolls as f64;
            println!(
                "  clicks spent per orb      {per_orb:.1}  (blocking ate {swallowed} of {} presses)",
                swallowed + totals.rolls
            );
            if per_orb > 1.5 {
                println!(
                    "  ^ this is the felt cost of block-first at your click rate, not a bug"
                );
            }
        }
        println!();
        println!("=== what the mouse hook actually received ===");
        println!("  {}", input_breakdown());
        let left_downs = LBUTTON_DOWNS.load(Ordering::Relaxed);
        let right_downs = R_DOWNS.load(Ordering::Relaxed);
        if left_downs == 0 && MOVES.load(Ordering::Relaxed) > 100 {
            println!();
            println!("  The hook saw plenty of movement and zero left presses. Your left");
            println!("  button is not reaching this hook at all, so nothing downstream can");
            println!("  work. Likely causes, in order:");
            println!("    - mouse buttons are swapped in Windows, so a physical left click");
            if right_downs > 0 {
                println!("      arrives as a RIGHT press — and {right_downs} right presses were seen,");
                println!("      which fits. Check Settings > Bluetooth & devices > Mouse.");
            } else {
                println!("      would arrive as a right press (none were seen either)");
            }
            println!("    - mouse software rebinding the button below the hook layer");
            println!("    - you were crafting while the game was not the foreground window");
        }
        println!();
        println!("{}", totals.first_copy.summary("first Ctrl+C round trip"));
        println!("{}", totals.first_change.summary("click -> text first moved"));
        println!("{}", totals.decision.summary("click -> verdict"));
        println!(
            "  stale changes discarded   {} (seen sooner than {}ms, so left over from an earlier craft)",
            totals.stale_discarded, options.min_craft_ms
        );

        if !totals.copies_per_roll.is_empty() {
            let total: u32 = totals.copies_per_roll.iter().sum();
            let mean = f64::from(total) / totals.copies_per_roll.len() as f64;
            let mut counts = std::collections::BTreeMap::new();
            for copies in &totals.copies_per_roll {
                *counts.entry(*copies).or_insert(0_u64) += 1;
            }
            println!();
            println!("  Ctrl+C copies per roll (mean {mean:.2}):");
            for (copies, hits) in counts {
                let share = hits as f64 / totals.copies_per_roll.len() as f64 * 100.0;
                let bar = "#".repeat((share / 2.5).round() as usize);
                println!("    {copies:>2} copies  {hits:>5} {share:>5.1}% {bar}");
            }
        }

        if totals.decision.is_empty() {
            println!();
            println!("  No rolls were decided, so there is nothing to compare against OCR.");
            return;
        }

        println!();
        println!("  click -> verdict distribution:");
        println!("{}", totals.decision.histogram(&[40, 60, 80, 100, 120, 200]));
        println!();
        for budget in [60_u64, 80, 100, options.budget_ms, 150] {
            let share = totals
                .decision
                .fraction_within(Duration::from_millis(budget))
                * 100.0;
            println!("  decided within {budget:>3}ms: {share:>5.1}%");
        }

        let p95 = totals.decision.percentile(0.95).unwrap_or_default();
        let budget = Duration::from_millis(options.budget_ms);
        let within = totals.decision.fraction_within(budget);
        println!();
        println!("=== verdict ===");
        println!(
            "  Your OCR path closes the loop at about {}ms.",
            options.budget_ms
        );
        if totals.fail_closed > totals.rolls / 20 {
            println!("  But {} of {} rolls failed closed. Fix that before reading the", totals.fail_closed, totals.rolls);
            println!("  latency numbers — a fast pipeline that keeps giving up is not faster.");
        } else if within >= 0.99 {
            println!(
                "  This path decided {:.1}% of rolls inside that, p95 {}. It is at least",
                within * 100.0,
                format_millis(p95)
            );
            println!("  as fast, with no region, no mask, and no OCR. Drop OCR.");
        } else if within >= 0.90 {
            println!(
                "  This path decided {:.1}% inside that, p95 {}. Close, and block-first",
                within * 100.0,
                format_millis(p95)
            );
            println!("  turns the misses into a brief stutter rather than a lost roll.");
        } else {
            println!(
                "  This path only decided {:.1}% inside that, p95 {}. The extra copies",
                within * 100.0,
                format_millis(p95)
            );
            println!("  are costing too much — try --first-delay-ms to skip the doomed first");
            println!("  copy, or keep OCR.");
        }
    }

    fn pump(hook: HHOOK) {
        let mut message = MSG::default();
        loop {
            // SAFETY: standard message loop over this thread's queue.
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if result.0 <= 0 {
                break;
            }
            if message.message == WM_HOTKEY {
                match message.wParam.0 as i32 {
                    HOTKEY_RELEASE => {
                        let previous = STATE.swap(STATE_IDLE, Ordering::AcqRel);
                        println!(
                            "  [released] was {} — monitoring resumed",
                            match previous {
                                STATE_DECIDING => "deciding (a verdict was still pending)",
                                STATE_LOCKED => "LOCKED",
                                _ => "already idle (nothing was blocking your clicks)",
                            }
                        );
                    }
                    HOTKEY_QUIT => {
                        println!("  [quit] shutting down");
                        RUNNING.store(false, Ordering::Release);
                        break;
                    }
                    _ => {}
                }
                continue;
            }
            // SAFETY: message was filled by GetMessageW.
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        // SAFETY: hook came from SetWindowsHookExW and is removed exactly once.
        let _ = unsafe { UnhookWindowsHookEx(hook) };
    }

    fn parse_options(mut arguments: impl Iterator<Item = String>) -> Result<Options, String> {
        let mut options = Options::default();
        while let Some(argument) = arguments.next() {
            let mut value = || {
                arguments
                    .next()
                    .ok_or_else(|| "flag needs a value".to_string())
                    .and_then(|raw| {
                        raw.parse::<u64>()
                            .map_err(|_| format!("expected a number, got {raw}"))
                    })
            };
            match argument.as_str() {
                "--budget-ms" => options.budget_ms = value()?,
                "--copy-timeout-ms" => options.copy_timeout_ms = value()?,
                "--deadline-ms" => options.deadline_ms = value()?,
                "--poll-gap-ms" => options.poll_gap_ms = value()?,
                "--poll-gap-max-ms" => options.poll_gap_max_ms = value()?,
                "--first-delay-ms" => options.first_delay_ms = value()?,
                "--unlock-after-ms" => options.unlock_after_ms = value()?,
                "--test-copy" => options.test_copy = Some(value()? as usize),
                "--scancode" => options.key_method = KeyMethod::ScanCode,
                "--min-craft-ms" => options.min_craft_ms = value()?,
                "--no-settle" => options.settle = false,
                "--observe" => options.observe = true,
                "--block-first" => options.block_first = true,
                "--active-window-ms" => options.active_window_ms = value()?,
                "--watch-interval-ms" => options.watch_interval_ms = value()?,
                "--help" | "-h" => {
                    println!(
                        "usage: clip-only-lab [--test-copy N] [--scancode] [--budget-ms N] [--first-delay-ms N] [--poll-gap-ms N] [--copy-timeout-ms N] [--deadline-ms N] [--unlock-after-ms N]"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(options)
    }

    /// Standalone check of the one mechanism everything else depends on: does
    /// Ctrl+C over a hovered item actually populate the clipboard?
    ///
    /// No hook, no clicks, no rules. If this fails there is no point looking at
    /// anything downstream.
    fn test_copy(options: &Options, samples: usize) {
        println!("  clipboard check — {samples} copies, {} keys", options.key_method);
        println!();
        println!("  Switch to the game and hover the item you are rolling.");
        println!("  Reproduce real crafting conditions: hold Shift with the orb on the");
        println!("  cursor, exactly as when you spam. Held modifiers are lifted around");
        println!("  each Ctrl+C and restored afterwards, and this checks that works.");
        println!();
        for remaining in (1..=5).rev() {
            println!("  starting in {remaining}...");
            std::thread::sleep(Duration::from_secs(1));
        }
        if !game_is_foreground() {
            let (title, class) = clipboard::foreground_window_description();
            println!();
            println!("  The foreground window is \"{title}\" ({class}), not the game.");
            println!("  Nothing was sent. Switch to the game and run this again.");
            return;
        }

        let timeout = Duration::from_millis(options.copy_timeout_ms);
        let mut latencies = LatencySamples::with_capacity(samples);
        let mut failures: Vec<String> = Vec::new();
        let mut first_text: Option<String> = None;
        let mut with_modifiers = 0_usize;

        for index in 0..samples {
            if !game_is_foreground() {
                failures.push(format!("sample {index}: game lost focus"));
                break;
            }
            match clipboard::copy_hovered_item(timeout, options.key_method) {
                Ok(outcome) => {
                    if first_text.is_none() {
                        first_text = Some(outcome.text.clone());
                    }
                    if outcome.suppressed_modifiers.any() {
                        with_modifiers += 1;
                    }
                    latencies.push(outcome.total());
                }
                Err(error) => {
                    if failures.len() < 8 {
                        failures.push(format!("sample {index}: {error}"));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(120));
        }

        println!();
        println!("=== clipboard check ===");
        println!("  succeeded  {}/{samples}", latencies.len());
        println!("  failed     {}", failures.len());
        println!(
            "  copies that had to lift a held modifier  {with_modifiers}"
        );
        println!("{}", latencies.summary("round trip"));
        for failure in &failures {
            println!("    {failure}");
        }

        if let Some(text) = &first_text {
            println!();
            println!("  first payload (confirm this is the item you meant):");
            for line in text.lines().take(14) {
                println!("    | {line}");
            }
            let parsed = poe_alarm_clip_only::item_text::parse(text);
            println!();
            match parsed {
                Ok(item) => {
                    println!("  parsed {} affix lines:", item.affix_lines.len());
                    for line in &item.affix_lines {
                        println!("    {line}");
                    }
                }
                Err(error) => println!("  NOTE: could not parse that payload: {error}"),
            }
        }

        println!();
        if latencies.is_empty() {
            println!("=== verdict === the client never answered a single Ctrl+C.");
            println!("  Work through these in order:");
            println!("    1. Was the cursor resting on an item with its tooltip visible?");
            println!("       Ctrl+C copies whatever is under the pointer and nothing else.");
            if options.key_method == KeyMethod::VirtualKey {
                println!("    2. The client may ignore virtual-key synthetic input. Retry with:");
                println!("         clip-only-lab.exe --test-copy {samples} --scancode");
            } else {
                println!("    2. Scan codes did not work either, so the client is likely");
                println!("       ignoring synthetic keys altogether. Press Ctrl+C by hand over");
                println!("       the item — if that fills the clipboard but this does not, the");
                println!("       clipboard path cannot be driven and this whole approach dies.");
            }
            println!("    3. Confirm Ctrl+C works manually: hover the item, press it, paste");
            println!("       into Notepad. If nothing pastes, the client has it disabled.");
        } else if failures.is_empty() {
            println!("=== verdict === Ctrl+C works reliably. Run the full loop.");
        } else {
            println!(
                "=== verdict === Ctrl+C works but {} of {samples} attempts failed.",
                failures.len()
            );
            println!("  Usually that means the cursor drifted off the item.");
        }
    }

    pub fn run() {
        let options = parse_options(std::env::args().skip(1)).unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2);
        });
        let profile = match LabProfile::load_release() {
            Ok(profile) => profile,
            Err(error) => {
                eprintln!("could not build a lab profile: {error}");
                std::process::exit(2);
            }
        };

        println!("POE Alarm — clipboard-only recognition lab");
        println!("  settings   {}", profile.settings_path);
        println!("  game       {}", profile.game_profile);
        println!("  language   {}", profile.ocr_language);
        println!("  groups     {}", profile.rules.definition().groups.len());
        println!("  budget     {}ms (what your OCR path already meets)", options.budget_ms);
        println!();
        println!("  No capture region is used. Nothing on screen is read.");
        println!();
        println!(
            "  policy     {}",
            if options.observe {
                "observe only — nothing is blocked, nothing is judged"
            } else if options.block_first {
                "block-first — every click waits for a verdict (~2.9 presses per orb)"
            } else {
                "detect-then-block — clicks flow freely, only a hit locks"
            }
        );
        println!();
        println!("  Hover the item you are rolling and craft normally.");
        println!("  Ctrl+Shift+F12 releases a lock. Ctrl+Shift+F9 quits and reports.");
        println!();
        match clipboard::process_is_elevated() {
            Some(true) => println!("  elevation  this process IS running as Administrator."),
            Some(false) => {
                println!("  elevation  this process is NOT running as Administrator.");
                println!();
                println!("  >> If Path of Exile runs elevated, the mouse hook will install and");
                println!("  >> then receive nothing while the game is focused — which looks");
                println!("  >> exactly like you never clicked. If the [watching] line below");
                println!("  >> reports 0 events while you are in the game, that is this.");
                println!("  >> Fix: close this, right-click Terminal, Run as administrator.");
            }
            None => println!("  elevation  could not be determined"),
        }
        println!();
        println!("  NOTE: every roll overwrites your clipboard. That is inherent here.");
        println!();

        if let Some(samples) = options.test_copy {
            test_copy(&options, samples.max(1));
            return;
        }

        let (sender, receiver) = sync_channel::<Instant>(64);
        let _ = CLICK_TX.set(sender);

        let worker_handle = std::thread::Builder::new()
            .name("clip-only-worker".to_string())
            .spawn(move || worker(profile, receiver, options))
            .expect("worker thread spawns");

        // SAFETY: this executable's module stays loaded for the process
        // lifetime, which outlives the hook.
        let module = unsafe { GetModuleHandleW(None) }
            .map(|handle| HINSTANCE(handle.0))
            .unwrap_or_default();
        // SAFETY: mouse_proc has the required signature.
        let hook =
            match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(module), 0) } {
                Ok(hook) => hook,
                Err(error) => {
                    eprintln!("SetWindowsHookExW failed: {error}");
                    RUNNING.store(false, Ordering::Release);
                    let _ = worker_handle.join();
                    std::process::exit(1);
                }
            };

        let modifiers = MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT;
        // SAFETY: a null window posts WM_HOTKEY to this thread's queue.
        unsafe {
            if RegisterHotKey(None, HOTKEY_RELEASE, modifiers, u32::from(VK_F12.0)).is_err() {
                eprintln!("  warning: Ctrl+Shift+F12 is already taken");
            }
            if RegisterHotKey(None, HOTKEY_QUIT, modifiers, u32::from(VK_F9.0)).is_err() {
                eprintln!("  warning: Ctrl+Shift+F9 is already taken");
            }
        }

        HOOK_THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
        println!("  armed — go craft.");
        println!();
        pump(hook);

        // SAFETY: ids were registered on this thread.
        unsafe {
            let _ = UnregisterHotKey(None, HOTKEY_RELEASE);
            let _ = UnregisterHotKey(None, HOTKEY_QUIT);
        }
        RUNNING.store(false, Ordering::Release);
        let _ = worker_handle.join();
    }
}

#[cfg(windows)]
fn main() {
    app::run();
}
