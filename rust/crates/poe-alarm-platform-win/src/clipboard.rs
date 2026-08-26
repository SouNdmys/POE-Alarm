//! Asks the client to dump the hovered item, then reads it back.
//!
//! Path of Exile writes the full text of whatever the cursor rests on to the
//! clipboard when it receives Ctrl+C. That is a supported client feature — the
//! price-check tools have used it for years — and it hands over the item
//! exactly, with no recognition step to be wrong about.
//!
//! Two details govern everything here.
//!
//! **The round trip is timed against the clipboard sequence number.** It bumps
//! on every `SetClipboardData` regardless of payload, so an alteration that
//! rerolls into byte-identical text still registers as an answer, where a
//! content comparison would report the client as silent. Reading the sequence
//! number takes no clipboard ownership either, so polling it never contends
//! with the clipboard managers and IMEs that make `OpenClipboard` fail.
//!
//! **Held modifiers have to be lifted.** Continuous currency application
//! requires Shift to be held — without it the orb leaves the cursor after one
//! use — so the one moment worth copying is the one moment Shift is down, and
//! the client would receive Ctrl+Shift+C.

use std::time::Duration;

/// Stamped into `dwExtraInfo` so a mouse or keyboard hook can tell our own
/// synthetic keys apart from the user's and decline to act on them.
pub const SYNTHETIC_INPUT_SIGNATURE: usize = 0x504F_454C;

/// How the synthetic Ctrl+C is delivered.
///
/// Some clients read the keyboard through raw input and honour only hardware
/// scan codes. Path of Exile accepts virtual keys, but the choice is kept so a
/// client that does not can still be driven.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyMethod {
    /// Virtual key codes.
    #[default]
    VirtualKey,
    /// Hardware scan codes with `KEYEVENTF_SCANCODE`.
    ScanCode,
}

impl std::fmt::Display for KeyMethod {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VirtualKey => formatter.write_str("virtual-key"),
            Self::ScanCode => formatter.write_str("scan-code"),
        }
    }
}

/// Failure modes of a clipboard round trip.
///
/// Every one of these is detectable, which is the point: a caller can fail
/// closed on a known failure rather than act on a guess. That is the property
/// OCR could not offer — a misread returns a confident wrong answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardError {
    /// The client never touched the clipboard inside the deadline.
    Timeout { waited: Duration },
    /// `OpenClipboard` kept losing to another owner.
    Busy { attempts: u32 },
    /// The clipboard changed but offered no Unicode text format at all.
    NoTextFormat { formats: Vec<String> },
    /// A Unicode text format existed but was empty.
    EmptyText,
    /// `SendInput` did not deliver the whole key sequence.
    InputRejected { delivered: u32, expected: u32 },
    /// A Win32 call failed outright.
    Os { operation: &'static str, code: i32 },
    /// This build has no clipboard support.
    Unsupported,
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
                write!(
                    formatter,
                    "clipboard stayed locked across {attempts} attempts"
                )
            }
            Self::NoTextFormat { formats } => {
                if formats.is_empty() {
                    formatter.write_str("clipboard changed but offered no formats at all")
                } else {
                    write!(
                        formatter,
                        "clipboard changed but offered no Unicode text; formats present: {}",
                        formats.join(", ")
                    )
                }
            }
            Self::EmptyText => {
                formatter.write_str("clipboard carried Unicode text, but it was empty")
            }
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
            Self::Unsupported => formatter.write_str("clipboard capture is Windows-only"),
        }
    }
}

impl std::error::Error for ClipboardError {}

impl ClipboardError {
    /// True when retrying inside the same deadline is worthwhile.
    ///
    /// The client stops answering Ctrl+C for as long as a craft is in flight,
    /// so a missing payload usually means it had nothing to give *yet* — a
    /// state the very next copy can leave.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Timeout { .. } | Self::Busy { .. } | Self::NoTextFormat { .. } | Self::EmptyText
        )
    }
}

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
    /// Modifiers the user was physically holding, which had to be lifted.
    pub suppressed_modifiers: HeldModifiers,
}

impl CopyOutcome {
    #[must_use]
    pub fn total(&self) -> Duration {
        self.client_round_trip + self.read_time
    }
}

/// Modifier keys the user is physically holding.
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

/// Why a relaunch-as-administrator attempt did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElevateError {
    /// The user dismissed the UAC prompt.
    Declined,
    /// The executable path could not be determined.
    NoExecutablePath,
    /// ShellExecute refused for some other reason.
    Failed(i32),
    /// This build cannot elevate.
    Unsupported,
}

impl std::fmt::Display for ElevateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declined => formatter.write_str("the elevation prompt was declined"),
            Self::NoExecutablePath => formatter.write_str("could not locate this executable"),
            Self::Failed(code) => write!(formatter, "ShellExecute failed with code {code}"),
            Self::Unsupported => formatter.write_str("elevation is Windows-only"),
        }
    }
}

impl std::error::Error for ElevateError {}

/// True when a window title and class describe a Path of Exile client.
///
/// The window class is the primary signal. Title matching is deliberately
/// exact: a browser tab reading "Path of Exile - Wikipedia" must not arm
/// anything, and a prefix match would let it.
#[must_use]
pub fn describes_game(title: &str, class: &str) -> bool {
    let class_lower = class.to_lowercase();
    if class_lower.contains("poewindowclass") || class_lower.contains("pathofexile") {
        return true;
    }
    matches!(title.trim(), "Path of Exile" | "Path of Exile 2")
}

/// Current clipboard sequence number.
#[must_use]
pub fn sequence_number() -> u32 {
    #[cfg(windows)]
    {
        crate::win32::clipboard_sequence_number()
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Full round trip: Ctrl+C, wait for the client to answer, read the payload.
///
/// Nothing is written to the clipboard first. The sequence number already
/// reports whether the client responded, and skipping the clear saves an
/// open/close pair on the hot path.
pub fn copy_hovered_item(
    timeout: Duration,
    method: KeyMethod,
) -> Result<CopyOutcome, ClipboardError> {
    #[cfg(windows)]
    {
        crate::win32::copy_hovered_item(timeout, method)
    }
    #[cfg(not(windows))]
    {
        let _ = (timeout, method);
        Err(ClipboardError::Unsupported)
    }
}

/// Reads the clipboard as text, retrying while another process owns it.
pub fn read_text(max_attempts: u32) -> Result<(String, u32), ClipboardError> {
    #[cfg(windows)]
    {
        crate::win32::read_clipboard_text(max_attempts)
    }
    #[cfg(not(windows))]
    {
        let _ = max_attempts;
        Err(ClipboardError::Unsupported)
    }
}

/// Reads the physical modifier state.
#[must_use]
pub fn held_modifiers() -> HeldModifiers {
    #[cfg(windows)]
    {
        crate::win32::held_modifiers()
    }
    #[cfg(not(windows))]
    {
        HeldModifiers::default()
    }
}

/// Title and class of the foreground window.
#[must_use]
pub fn foreground_window_description() -> (String, String) {
    #[cfg(windows)]
    {
        crate::win32::foreground_window_description()
    }
    #[cfg(not(windows))]
    {
        (String::new(), String::new())
    }
}

/// True when the foreground window is a Path of Exile client.
#[must_use]
pub fn game_is_foreground() -> bool {
    let (title, class) = foreground_window_description();
    describes_game(&title, &class)
}

/// Whether this process is running elevated, or `None` if it cannot be told.
#[must_use]
pub fn process_is_elevated() -> Option<bool> {
    #[cfg(windows)]
    {
        crate::win32::process_is_elevated()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Whether the foreground process outranks this one.
///
/// A medium-integrity process is refused the token of an elevated one, so
/// failing to read the foreground process's token while reading our own
/// identifies the case exactly. That case matters because UIPI then discards
/// any input this process injects, and the client never sees the Ctrl+C —
/// which otherwise presents as an unexplained wall of timeouts.
#[must_use]
pub fn foreground_process_outranks_us() -> bool {
    #[cfg(windows)]
    {
        crate::win32::foreground_process_outranks_us()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Relaunches this executable elevated, carrying the current arguments over.
///
/// Deliberately not a manifest `requireAdministrator`: that puts a UAC prompt
/// in front of every user, including the majority whose client is not elevated
/// and who therefore need no privileges at all. Elevation is asked for only
/// once [`foreground_process_outranks_us`] says it is necessary.
///
/// The elevated process gets a fresh console; the caller is expected to exit.
pub fn relaunch_elevated(skip_arguments: &[&str]) -> Result<(), ElevateError> {
    #[cfg(windows)]
    {
        crate::win32::relaunch_elevated(skip_arguments)
    }
    #[cfg(not(windows))]
    {
        let _ = skip_arguments;
        Err(ElevateError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_both_clients_by_class() {
        assert!(describes_game("Path of Exile", "POEWindowClass"));
        assert!(describes_game("Path of Exile 2", "POEWindowClass"));
        assert!(describes_game("流亡黯道", "POEWindowClass"));
    }

    #[test]
    fn recognizes_a_client_by_exact_title_when_the_class_is_unknown() {
        assert!(describes_game("Path of Exile 2", "SomeFutureClass"));
    }

    #[test]
    fn a_browser_tab_named_after_the_game_is_not_the_game() {
        assert!(!describes_game(
            "Path of Exile - Wikipedia - Chrome",
            "Chrome_WidgetWin_1"
        ));
        assert!(!describes_game("Notepad", "Notepad"));
        assert!(!describes_game("", ""));
    }

    #[test]
    fn modifier_display_lists_held_keys() {
        assert_eq!(HeldModifiers::default().to_string(), "none");
        assert!(!HeldModifiers::default().any());
        let both = HeldModifiers {
            control: true,
            shift: true,
            alt: false,
        };
        assert_eq!(both.to_string(), "Ctrl+Shift");
        assert!(both.any());
    }

    #[test]
    fn a_missing_payload_is_worth_retrying_but_a_rejected_key_is_not() {
        assert!(
            ClipboardError::Timeout {
                waited: Duration::from_millis(25)
            }
            .is_transient()
        );
        assert!(ClipboardError::EmptyText.is_transient());
        assert!(
            ClipboardError::NoTextFormat {
                formats: Vec::new()
            }
            .is_transient()
        );
        assert!(
            !ClipboardError::InputRejected {
                delivered: 0,
                expected: 4
            }
            .is_transient()
        );
        assert!(
            !ClipboardError::Os {
                operation: "GlobalLock",
                code: 5
            }
            .is_transient()
        );
    }
}
