//! Counts the user's physical left clicks without touching them.
//!
//! The click-invoked copy needs to know *that* the user clicked, nothing more.
//! This hook classifies exactly one message kind, increments a counter, and
//! passes everything through unconditionally — it never suppresses, never
//! synthesizes, and does no other work on the hook thread. Injected clicks are
//! counted too, deliberately: the user's own macro software injects its clicks,
//! and those are precisely the ones the copy must follow.
//!
//! One observer per process. The counter is a process-wide static because the
//! hook procedure has no other channel; the singleton flag keeps two sessions
//! from double-counting into it.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL,
    WM_LBUTTONDOWN, WM_QUIT,
};

use crate::PlatformError;

static OBSERVER_ACTIVE: AtomicBool = AtomicBool::new(false);
static CLICKS: AtomicU64 = AtomicU64::new(0);
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);

/// A running click counter; dropping it unhooks and stops the thread.
pub(crate) struct NativeClickObserver {
    thread: Option<JoinHandle<()>>,
}

pub(crate) fn observed_clicks() -> u64 {
    CLICKS.load(Ordering::Acquire)
}

pub(crate) fn start_click_observer() -> Result<NativeClickObserver, PlatformError> {
    if OBSERVER_ACTIVE.swap(true, Ordering::AcqRel) {
        return Err(PlatformError::AlreadyInUse {
            capability: "click observer",
        });
    }

    let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("poe-alarm-click-observer".to_owned())
        .spawn(move || {
            // Create the message queue before publishing the thread id.
            let mut queue_probe = MSG::default();
            // SAFETY: PeekMessage with PM_NOREMOVE only creates the queue.
            let _ = unsafe { PeekMessageW(&mut queue_probe, None, 0, 0, PM_NOREMOVE) };
            // SAFETY: no preconditions.
            HOOK_THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::Release);

            // SAFETY: the procedure is a valid extern "system" fn for the
            // lifetime of the hook, which this thread owns and unhooks.
            let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(observe_proc), None, 0) };
            let hook = match hook {
                Ok(hook) => {
                    let _ = ready_sender.send(Ok(()));
                    hook
                }
                Err(error) => {
                    let _ = ready_sender.send(Err(super::error_from_windows(
                        "SetWindowsHookExW(WH_MOUSE_LL, observer)",
                        error,
                    )));
                    OBSERVER_ACTIVE.store(false, Ordering::Release);
                    return;
                }
            };

            let mut message = MSG::default();
            // SAFETY: standard message loop owned by this thread.
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
                // SAFETY: message came from this thread's queue.
                let _ = unsafe { TranslateMessage(&message) };
                // SAFETY: as above.
                unsafe { DispatchMessageW(&message) };
            }

            // SAFETY: this thread installed the hook.
            let _ = unsafe { UnhookWindowsHookEx(hook) };
            HOOK_THREAD_ID.store(0, Ordering::Release);
            OBSERVER_ACTIVE.store(false, Ordering::Release);
        })
        .map_err(|error| PlatformError::Thread {
            operation: "spawn click observer",
            detail: error.to_string(),
        })?;

    match ready_receiver.recv() {
        Ok(Ok(())) => Ok(NativeClickObserver {
            thread: Some(thread),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            OBSERVER_ACTIVE.store(false, Ordering::Release);
            Err(PlatformError::Thread {
                operation: "start click observer",
                detail: "observer thread exited before reporting readiness".to_owned(),
            })
        }
    }
}

impl Drop for NativeClickObserver {
    fn drop(&mut self) {
        let thread_id = HOOK_THREAD_ID.load(Ordering::Acquire);
        if thread_id != 0 {
            // SAFETY: the id was published by a live thread with a queue.
            let _ = unsafe { PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        if let Some(thread) = self.thread.take() {
            // The loop exits on WM_QUIT and the hook procedure never blocks,
            // so this join is bounded in practice.
            let _ = thread.join();
        }
    }
}

/// The entire hook procedure: classify one message kind, count, pass through.
unsafe extern "system" fn observe_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && wparam.0 as u32 == WM_LBUTTONDOWN {
        CLICKS.fetch_add(1, Ordering::AcqRel);
    }
    // SAFETY: forwarding with the original arguments is the documented pattern.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_observer_is_a_process_singleton_and_releases_on_drop() {
        let first = start_click_observer().expect("first observer starts");
        assert!(matches!(
            start_click_observer(),
            Err(PlatformError::AlreadyInUse { .. })
        ));
        drop(first);
        // Drop joins the thread, so the slot must be free again immediately.
        let second = start_click_observer().expect("observer restarts after drop");
        drop(second);
    }
}
