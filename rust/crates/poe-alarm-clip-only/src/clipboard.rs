//! Win32 clipboard round trip: ask the client to dump the hovered item, then
//! read the text back.
//!
//! The round trip is timed against `GetClipboardSequenceNumber`, which bumps on
//! every `SetClipboardData` regardless of payload. That matters: an alteration
//! roll can produce byte-identical text, and a content comparison would report
//! "the client never answered" when it actually did. The sequence number also
//! costs no clipboard ownership, so polling it never contends with the clipboard
//! managers and IMEs that would otherwise make `OpenClipboard` fail.

#![cfg(windows)]

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYBD_EVENT_FLAGS, SendInput, VK_C, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowTextW};

/// `CF_UNICODETEXT`. Spelled out here so the crate does not need the OLE feature.
const CF_UNICODETEXT: u32 = 13;

/// Stamped into `dwExtraInfo` so a hook can tell our synthetic keys apart from
/// the user's.
pub const SYNTHETIC_INPUT_SIGNATURE: usize = 0x504F_454C;

/// How long the tight spin runs before the wait falls back to sleeping. Keeps
/// the common case sub-millisecond without pinning a core through the tail.
const SPIN_WINDOW: Duration = Duration::from_millis(4);

/// Failure modes of a clipboard round trip. Every one of these is detectable,
/// which is the whole point: a caller can fail closed instead of guessing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardError {
    /// The client never touched the clipboard inside the deadline.
    Timeout { waited: Duration },
    /// `OpenClipboard` kept losing to another owner.
    Busy { attempts: u32 },
    /// The clipboard changed but held no Unicode text.
    NoText,
    /// `SendInput` did not deliver the whole key sequence.
    InputRejected { delivered: u32, expected: u32 },
    /// A Win32 call failed outright.
    Os {
        operation: &'static str,
        code: i32,
    },
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout { waited } => write!(
                formatter,
                "client did not write the clipboard within {:.1}ms",
                waited.as_secs_f64() * 1_000.0
            ),
            Self::Busy { attempts } => {
                write!(formatter, "clipboard stayed locked across {attempts} attempts")
            }
            Self::NoText => formatter.write_str("clipboard changed but carried no Unicode text"),
            Self::InputRejected {
                delivered,
                expected,
            } => write!(
                formatter,
                "SendInput delivered {delivered} of {expected} events"
            ),
            Self::Os { operation, code } => {
                write!(formatter, "{operation} failed with error {code}")
            }
        }
    }
}

impl std::error::Error for ClipboardError {}

/// A completed round trip.
#[derive(Clone, Debug)]
pub struct CopyOutcome {
    /// Raw clipboard payload.
    pub text: String,
    /// `SendInput` returning to the sequence number changing.
    pub client_round_trip: Duration,
    /// Time spent opening and draining the clipboard afterwards.
    pub read_time: Duration,
    /// How many `OpenClipboard` attempts the read needed.
    pub open_attempts: u32,
}

impl CopyOutcome {
    #[must_use]
    pub fn total(&self) -> Duration {
        self.client_round_trip + self.read_time
    }
}

/// Current clipboard sequence number.
#[must_use]
pub fn sequence_number() -> u32 {
    // SAFETY: no arguments, no output buffer.
    unsafe { GetClipboardSequenceNumber() }
}

/// Sends a synthetic Ctrl+C to the foreground window.
pub fn send_ctrl_c() -> Result<(), ClipboardError> {
    let key = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: SYNTHETIC_INPUT_SIGNATURE,
            },
        },
    };
    let inputs = [
        key(VK_CONTROL, false),
        key(VK_C, false),
        key(VK_C, true),
        key(VK_CONTROL, true),
    ];
    let expected = inputs.len() as u32;
    // SAFETY: `inputs` is a live slice of correctly sized INPUT records.
    let delivered = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if delivered != expected {
        return Err(ClipboardError::InputRejected {
            delivered,
            expected,
        });
    }
    Ok(())
}

/// Reads the clipboard as text, retrying while another process owns it.
pub fn read_text(max_attempts: u32) -> Result<(String, u32), ClipboardError> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        // SAFETY: passing None asks for the current task to own the clipboard.
        let opened = unsafe { OpenClipboard(Some(HWND::default())) };
        if opened.is_ok() {
            let result = read_open_clipboard();
            // SAFETY: the clipboard is open on this thread.
            let _ = unsafe { CloseClipboard() };
            return result.map(|text| (text, attempts));
        }
        if attempts >= max_attempts {
            return Err(ClipboardError::Busy { attempts });
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Reads `CF_UNICODETEXT` from an already-open clipboard.
fn read_open_clipboard() -> Result<String, ClipboardError> {
    // SAFETY: the clipboard is open; a missing format returns Err rather than
    // an invalid handle.
    let handle: HANDLE = match unsafe { GetClipboardData(CF_UNICODETEXT) } {
        Ok(handle) => handle,
        Err(_) => return Err(ClipboardError::NoText),
    };
    if handle.0.is_null() {
        return Err(ClipboardError::NoText);
    }
    let global = HGLOBAL(handle.0);
    // SAFETY: `global` came from the clipboard and stays valid until we close it.
    let pointer = unsafe { GlobalLock(global) } as *const u16;
    if pointer.is_null() {
        return Err(ClipboardError::Os {
            operation: "GlobalLock",
            code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        });
    }
    // SAFETY: the clipboard block is at least `GlobalSize` bytes long.
    let capacity_bytes = unsafe { GlobalSize(global) };
    let capacity_units = capacity_bytes / size_of::<u16>();
    let mut length = 0;
    // SAFETY: bounded by the reported block size, so the scan cannot run off
    // the end even if the producer forgot the terminator.
    while length < capacity_units && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` units were just proven readable.
    let units = unsafe { std::slice::from_raw_parts(pointer, length) };
    let text = String::from_utf16_lossy(units);
    // GlobalUnlock reports Err when the lock count reaches zero, which is the
    // normal outcome here, so the result is deliberately discarded.
    // SAFETY: matches the GlobalLock above.
    let _ = unsafe { GlobalUnlock(global) };
    if text.is_empty() {
        return Err(ClipboardError::NoText);
    }
    Ok(text)
}

/// Full round trip: Ctrl+C, wait for the client to answer, read the payload.
///
/// Nothing is written to the clipboard first. The sequence number already tells
/// us whether the client responded, and skipping the clear saves an
/// open/close pair on the hot path.
pub fn copy_hovered_item(timeout: Duration) -> Result<CopyOutcome, ClipboardError> {
    let baseline = sequence_number();
    let started = Instant::now();
    send_ctrl_c()?;

    loop {
        if sequence_number() != baseline {
            break;
        }
        let waited = started.elapsed();
        if waited >= timeout {
            return Err(ClipboardError::Timeout { waited });
        }
        if waited < SPIN_WINDOW {
            std::hint::spin_loop();
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let client_round_trip = started.elapsed();

    let read_started = Instant::now();
    let (text, open_attempts) = read_text(60)?;
    Ok(CopyOutcome {
        text,
        client_round_trip,
        read_time: read_started.elapsed(),
        open_attempts,
    })
}

/// Modifier keys the user is physically holding right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeldModifiers {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
}

impl HeldModifiers {
    #[must_use]
    pub fn any(self) -> bool {
        self.control || self.shift || self.alt
    }
}

impl std::fmt::Display for HeldModifiers {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names = Vec::new();
        if self.control {
            names.push("Ctrl");
        }
        if self.shift {
            names.push("Shift");
        }
        if self.alt {
            names.push("Alt");
        }
        if names.is_empty() {
            formatter.write_str("none")
        } else {
            formatter.write_str(&names.join("+"))
        }
    }
}

/// Reads the physical modifier state.
#[must_use]
pub fn held_modifiers() -> HeldModifiers {
    // SAFETY: GetAsyncKeyState takes a virtual key code and returns a bitfield.
    let down = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| {
        (unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0
    };
    HeldModifiers {
        control: down(VK_CONTROL),
        shift: down(VK_SHIFT),
        alt: down(VK_MENU),
    }
}

/// Title and class of the foreground window.
#[must_use]
pub fn foreground_window_description() -> (String, String) {
    // SAFETY: returns a borrowed handle that stays valid for the calls below.
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return (String::new(), String::new());
    }
    let mut title = [0_u16; 256];
    let mut class = [0_u16; 256];
    // SAFETY: both buffers are live and their lengths are passed by slice.
    let title_length = unsafe { GetWindowTextW(window, &mut title) }.max(0) as usize;
    // SAFETY: as above.
    let class_length = unsafe { GetClassNameW(window, &mut class) }.max(0) as usize;
    (
        String::from_utf16_lossy(&title[..title_length]),
        String::from_utf16_lossy(&class[..class_length]),
    )
}

/// Whether this process is running elevated.
///
/// A low-level mouse hook in a medium-integrity process never receives input
/// destined for an elevated window, so a mismatch here means the hook installs
/// successfully and then sees nothing while the game is focused. That failure
/// is otherwise indistinguishable from the user simply not clicking, so it is
/// worth reporting up front rather than inferring from a title bar.
#[must_use]
pub fn process_is_elevated() -> Option<bool> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing;
    // `token` receives an owned handle closed below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;

    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0_u32;
    // SAFETY: the buffer matches the size declared for TokenElevation.
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &raw mut returned,
        )
    };
    // SAFETY: `token` came from OpenProcessToken and is closed exactly once.
    let _ = unsafe { CloseHandle(token) };
    queried.ok()?;
    Some(elevation.TokenIsElevated != 0)
}

/// True when the foreground window looks like a Path of Exile client.
///
/// The lab never intercepts input unless this holds, so a stuck state machine
/// can never lock up the desktop.
#[must_use]
pub fn game_is_foreground() -> bool {
    let (title, class) = foreground_window_description();
    describes_game(&title, &class)
}

/// Pure predicate behind [`game_is_foreground`], split out so it is testable.
///
/// The window class is the primary signal. Title matching is deliberately
/// exact: a browser tab reading "Path of Exile - Wikipedia - Chrome" must not
/// arm interception, and a prefix match would let it.
#[must_use]
pub fn describes_game(title: &str, class: &str) -> bool {
    let class_lower = class.to_lowercase();
    if class_lower.contains("poewindowclass") || class_lower.contains("pathofexile") {
        return true;
    }
    matches!(title.trim(), "Path of Exile" | "Path of Exile 2")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_both_clients_by_title() {
        assert!(describes_game("Path of Exile", "POEWindowClass"));
        assert!(describes_game("Path of Exile 2", "POEWindowClass"));
    }

    #[test]
    fn recognizes_client_by_exact_title_when_class_is_unknown() {
        assert!(describes_game("Path of Exile 2", "SomeFutureClass"));
    }

    #[test]
    fn recognizes_client_by_class_when_title_is_localized() {
        assert!(describes_game("流亡黯道", "POEWindowClass"));
    }

    #[test]
    fn rejects_unrelated_windows() {
        assert!(!describes_game("Notepad", "Notepad"));
        assert!(!describes_game("", ""));
        assert!(!describes_game(
            "Path of Exile - Wikipedia - Chrome",
            "Chrome_WidgetWin_1"
        ));
    }

    #[test]
    fn modifier_display_lists_held_keys() {
        let none = HeldModifiers::default();
        assert_eq!(none.to_string(), "none");
        assert!(!none.any());
        let both = HeldModifiers {
            control: true,
            shift: true,
            alt: false,
        };
        assert_eq!(both.to_string(), "Ctrl+Shift");
        assert!(both.any());
    }
}
