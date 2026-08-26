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
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey, VK_F9, VK_F12,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, MSG, MSLLHOOKSTRUCT,
        SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL, WM_HOTKEY,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL,
        WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN,
    };

    use poe_alarm_clip_only::clipboard::{
        self, KeyMethod, SYNTHETIC_INPUT_SIGNATURE, game_is_foreground,
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
        /// Per-copy deadline.
        pub copy_timeout_ms: u64,
        /// Overall deadline from the click.
        pub deadline_ms: u64,
        /// Pause between copies while waiting for the craft to resolve.
        pub poll_gap_ms: u64,
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
    }

    impl Default for Options {
        fn default() -> Self {
            Self {
                budget_ms: 120,
                copy_timeout_ms: 600,
                deadline_ms: 1_500,
                poll_gap_ms: 0,
                first_delay_ms: 0,
                key_method: KeyMethod::VirtualKey,
                unlock_after_ms: 4_000,
                test_copy: None,
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

        if options.first_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(options.first_delay_ms));
        }

        loop {
            if click_at.elapsed() >= deadline {
                totals.fail_closed += 1;
                lock(
                    &format!(
                        "item text never changed within {}ms across {copies} copies",
                        options.deadline_ms
                    ),
                    true,
                );
                return;
            }

            copies += 1;
            let outcome = match clipboard::copy_hovered_item(copy_timeout, options.key_method) {
                Ok(outcome) => outcome,
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
                Some(true) => {
                    if options.poll_gap_ms > 0 {
                        std::thread::sleep(Duration::from_millis(options.poll_gap_ms));
                    }
                }
                Some(false) => {
                    let verdict = evaluate_payload(&profile.rules, &outcome.text);
                    let decided = click_at.elapsed();
                    totals.decision.push(decided);
                    totals.copies_per_roll.push(copies);
                    if let Some(first) = first_copy {
                        totals.first_copy.push(first);
                    }
                    *baseline = Some(outcome.text);

                    println!(
                        "  #{index:<4} copies {copies:<3} first {:<8} verdict {:<8} {:<5} ({} lines)",
                        format_millis(first_copy.unwrap_or_default()),
                        format_millis(decided),
                        verdict.label(),
                        verdict.affix_count()
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
                        Verdict::Miss { .. } => release(),
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

    fn worker(profile: LabProfile, clicks: Receiver<Instant>, options: Options) {
        let mut totals = Totals::new();
        let mut baseline: Option<String> = None;
        let mut armed = false;
        let mut last_heartbeat = Instant::now();
        let mut last_events = 0_u64;
        let mut error_lock_at: Option<Instant> = None;

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
                            println!(
                                "  [watching] hook saw 0 mouse events in 3s. Move the mouse; if this"
                            );
                            println!(
                                "             keeps printing, run this terminal as Administrator."
                            );
                        } else {
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
        println!(
            "  clicks swallowed          {}",
            SWALLOWED.load(Ordering::Relaxed)
        );
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
        println!("{}", totals.decision.summary("click -> verdict"));

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
                "--first-delay-ms" => options.first_delay_ms = value()?,
                "--unlock-after-ms" => options.unlock_after_ms = value()?,
                "--test-copy" => options.test_copy = Some(value()? as usize),
                "--scancode" => options.key_method = KeyMethod::ScanCode,
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
        println!("  Do not hold a currency orb on the cursor for this test.");
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
