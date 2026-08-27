//! Native half of the clipboard capture path.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HGLOBAL, HWND};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_ELEVATION,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenElevation, TokenIntegrityLevel,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardFormatNameW,
    GetClipboardSequenceNumber, OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, SendInput, VIRTUAL_KEY, VK_C, VK_CONTROL, VK_MENU,
    VK_SHIFT,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_SHOWNORMAL,
};
use windows::core::HSTRING;

use crate::clipboard::{
    ClipboardError, CopyOutcome, ElevateError, HeldModifiers, KeyMethod, SYNTHETIC_INPUT_SIGNATURE,
    describes_game,
};
use crate::geometry::RectI;

/// `CF_UNICODETEXT`. Spelled out so the crate need not take the OLE feature.
const CF_UNICODETEXT: u32 = 13;

/// How long the tight spin runs before the wait falls back to sleeping. Keeps
/// the common case sub-millisecond without pinning a core through the tail.
const SPIN_WINDOW: Duration = Duration::from_millis(4);

/// Set 1 scan codes for the keys involved.
const SCAN_LCONTROL: u16 = 0x1D;
const SCAN_C: u16 = 0x2E;
const SCAN_LSHIFT: u16 = 0x2A;
const SCAN_LALT: u16 = 0x38;

pub(crate) fn clipboard_sequence_number() -> u32 {
    // SAFETY: no arguments, no output buffer.
    unsafe { GetClipboardSequenceNumber() }
}

/// Sends a synthetic Ctrl+C to the foreground window.
///
/// Modifiers the user is physically holding are lifted for the duration and
/// pressed back afterwards. Continuous crafting holds Shift, and without this
/// the client receives Ctrl+Shift+C and copies nothing.
fn send_ctrl_c(method: KeyMethod, held: HeldModifiers) -> Result<(), ClipboardError> {
    let key = |vk: VIRTUAL_KEY, scan: u16, up: bool| {
        let mut flags = if up {
            KEYEVENTF_KEYUP
        } else {
            KEYBD_EVENT_FLAGS(0)
        };
        if method == KeyMethod::ScanCode {
            flags |= KEYEVENTF_SCANCODE;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: if method == KeyMethod::ScanCode {
                        VIRTUAL_KEY(0)
                    } else {
                        vk
                    },
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: SYNTHETIC_INPUT_SIGNATURE,
                },
            },
        }
    };

    let mut inputs = Vec::with_capacity(8);
    if held.shift {
        inputs.push(key(VK_SHIFT, SCAN_LSHIFT, true));
    }
    if held.alt {
        inputs.push(key(VK_MENU, SCAN_LALT, true));
    }
    inputs.extend([
        key(VK_CONTROL, SCAN_LCONTROL, false),
        key(VK_C, SCAN_C, false),
        key(VK_C, SCAN_C, true),
        key(VK_CONTROL, SCAN_LCONTROL, true),
    ]);
    if held.alt {
        inputs.push(key(VK_MENU, SCAN_LALT, false));
    }
    if held.shift {
        inputs.push(key(VK_SHIFT, SCAN_LSHIFT, false));
    }

    let expected = inputs.len() as u32;
    // SAFETY: `inputs` is a live slice of correctly sized INPUT records.
    let delivered = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if delivered == expected {
        Ok(())
    } else {
        Err(ClipboardError::InputRejected {
            delivered,
            expected,
        })
    }
}

pub(crate) fn copy_hovered_item(
    timeout: Duration,
    method: KeyMethod,
) -> Result<CopyOutcome, ClipboardError> {
    let baseline = clipboard_sequence_number();
    let held = held_modifiers();
    let started = Instant::now();
    send_ctrl_c(method, held)?;

    loop {
        if clipboard_sequence_number() != baseline {
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
    let (text, open_attempts) = read_clipboard_text(60).map_err(|error| match error {
        ClipboardError::NoTextFormat { .. } => ClipboardError::NoTextFormat {
            formats: available_formats(),
        },
        other => other,
    })?;
    Ok(CopyOutcome {
        text,
        client_round_trip,
        read_time: read_started.elapsed(),
        open_attempts,
        suppressed_modifiers: held,
    })
}

pub(crate) fn read_clipboard_text(max_attempts: u32) -> Result<(String, u32), ClipboardError> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        // SAFETY: a default window handle asks for the current task to own it.
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
    let Ok(handle) = (unsafe { GetClipboardData(CF_UNICODETEXT) }) else {
        return Err(ClipboardError::NoTextFormat {
            formats: Vec::new(),
        });
    };
    if handle.0.is_null() {
        return Err(ClipboardError::NoTextFormat {
            formats: Vec::new(),
        });
    }
    let global = HGLOBAL(handle.0);
    // SAFETY: `global` came from the clipboard and stays valid until we close.
    let pointer = unsafe { GlobalLock(global) } as *const u16;
    if pointer.is_null() {
        return Err(ClipboardError::Os {
            operation: "GlobalLock",
            code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        });
    }
    // SAFETY: the block is at least `GlobalSize` bytes long.
    let capacity_units = unsafe { GlobalSize(global) } / size_of::<u16>();
    let mut length = 0;
    // SAFETY: bounded by the reported size, so the scan cannot run off the end
    // even if the producer omitted the terminator.
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
        Err(ClipboardError::EmptyText)
    } else {
        Ok(text)
    }
}

/// Names of the formats on the clipboard, for diagnosing a client that answered
/// with something other than text.
fn available_formats() -> Vec<String> {
    let mut names = Vec::new();
    // SAFETY: enumeration requires the clipboard open; a failure leaves the
    // list empty rather than reporting anything false.
    if unsafe { OpenClipboard(Some(HWND::default())) }.is_err() {
        return names;
    }
    let mut format = 0_u32;
    loop {
        // SAFETY: the clipboard is open on this thread.
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 || names.len() >= 12 {
            break;
        }
        let mut buffer = [0_u16; 128];
        // SAFETY: the buffer length travels with the slice.
        let written = unsafe { GetClipboardFormatNameW(format, &mut buffer) };
        names.push(if written > 0 {
            String::from_utf16_lossy(&buffer[..written as usize])
        } else {
            match format {
                1 => "CF_TEXT".to_string(),
                7 => "CF_OEMTEXT".to_string(),
                13 => "CF_UNICODETEXT".to_string(),
                16 => "CF_LOCALE".to_string(),
                other => format!("#{other}"),
            }
        });
    }
    // SAFETY: the clipboard is open on this thread.
    let _ = unsafe { CloseClipboard() };
    names
}

pub(crate) fn held_modifiers() -> HeldModifiers {
    // SAFETY: takes a virtual key code and returns a bitfield.
    let down =
        |vk: VIRTUAL_KEY| (unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0;
    HeldModifiers {
        control: down(VK_CONTROL),
        shift: down(VK_SHIFT),
        alt: down(VK_MENU),
    }
}

pub(crate) fn foreground_window_description() -> (String, String) {
    // SAFETY: returns a borrowed handle valid for the call below.
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return (String::new(), String::new());
    }
    window_description(window)
}

fn window_description(window: HWND) -> (String, String) {
    let mut title = [0_u16; 256];
    let mut class = [0_u16; 256];
    // SAFETY: both buffers are live and their lengths travel with the slices.
    let title_length = unsafe { GetWindowTextW(window, &mut title) }.max(0) as usize;
    // SAFETY: as above.
    let class_length = unsafe { GetClassNameW(window, &mut class) }.max(0) as usize;
    (
        String::from_utf16_lossy(&title[..title_length]),
        String::from_utf16_lossy(&class[..class_length]),
    )
}

/// Where the game window is, if it can be found.
///
/// Only ever used to answer "which monitor" — the red alert centres its card on
/// the monitor holding this rectangle. `None` is an ordinary answer, not a
/// failure: every caller already treats it as "use the primary monitor", which
/// is exactly what a user with one screen sees either way.
pub(crate) fn game_window_rect() -> Option<RectI> {
    game_window().and_then(window_rect)
}

/// The game's top-level window, wherever it is.
fn game_window() -> Option<HWND> {
    // The overwhelmingly common case is that the player is looking at the game.
    // SAFETY: returns a borrowed handle checked below.
    let foreground = unsafe { GetForegroundWindow() };
    if !foreground.is_invalid() {
        let (title, class) = window_description(foreground);
        if describes_game(&title, &class) {
            return Some(foreground);
        }
    }
    enumerate_game_window()
}

/// The window rectangle, refusing the two shapes that would silently mislead.
fn window_rect(window: HWND) -> Option<RectI> {
    // A minimized window reports roughly (-32000, -32000)-(-31840, -31972).
    // That has positive area, so it would sail through RectI::new and then
    // resolve to the primary monitor — an answer that looks deliberate and is
    // not. Refusing it hands the caller an honest "unknown" instead.
    // SAFETY: takes a borrowed handle.
    if unsafe { IsIconic(window) }.as_bool() {
        return None;
    }
    let mut rect = windows::Win32::Foundation::RECT::default();
    // SAFETY: `rect` is live for the call.
    unsafe { GetWindowRect(window, &raw mut rect) }.ok()?;
    RectI::new(
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    )
}

fn enumerate_game_window() -> Option<HWND> {
    let mut found: Option<HWND> = None;
    // SAFETY: the callback below only writes through this pointer while
    // EnumWindows is running, and EnumWindows is synchronous.
    let _ = unsafe {
        EnumWindows(
            Some(visit_window),
            windows::Win32::Foundation::LPARAM((&raw mut found) as isize),
        )
    };
    found
}

unsafe extern "system" fn visit_window(
    window: HWND,
    state: windows::Win32::Foundation::LPARAM,
) -> windows::core::BOOL {
    // SAFETY: `state` is the &mut Option<HWND> handed to EnumWindows above.
    let found = unsafe { &mut *(state.0 as *mut Option<HWND>) };
    // SAFETY: EnumWindows hands out live top-level handles.
    if !unsafe { IsWindowVisible(window) }.as_bool() {
        return true.into();
    }
    let (title, class) = window_description(window);
    if describes_game(&title, &class) {
        *found = Some(window);
        return false.into();
    }
    true.into()
}

pub(crate) fn process_is_elevated() -> Option<bool> {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess yields a pseudo-handle needing no close;
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

/// Whether the game is running at a higher integrity level than this process.
///
/// Asks about the game window, not the foreground window. Those are the same
/// thing only while the user is actually playing: by the time a monitoring
/// fault reaches the UI they have alt-tabbed back, the foreground window is
/// ours, reading our own token succeeds, and the answer comes back "no
/// mismatch" for a process that has been failing for a minute. That is why the
/// privilege prompt fired on the hotkey path and never on the monitoring one.
/// The mandatory integrity level of a process, as its RID.
///
/// Medium is 0x2000 and High is 0x3000, so a plain comparison orders them.
fn integrity_level(process: HANDLE) -> Option<u32> {
    let mut token = HANDLE::default();
    // SAFETY: `token` receives an owned handle, closed below.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;

    let mut needed = 0_u32;
    // SAFETY: asking for the required size; failure is expected here.
    let _ = unsafe { GetTokenInformation(token, TokenIntegrityLevel, None, 0, &raw mut needed) };
    let mut buffer = vec![0_u8; needed as usize];
    // SAFETY: the buffer is `needed` bytes as the call just reported.
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &raw mut needed,
        )
    };
    // SAFETY: token came from OpenProcessToken.
    let _ = unsafe { CloseHandle(token) };
    queried.ok()?;

    // SAFETY: on success the buffer holds a TOKEN_MANDATORY_LABEL whose Sid
    // points inside it, and the RID is the last subauthority.
    unsafe {
        let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(label.Label.Sid, u32::from(count) - 1))
    }
}

/// Whether the game runs at a higher integrity level than this process.
///
/// Compares the levels rather than asking whether the game's token can be
/// opened at all. The default mandatory policy is NO_WRITE_UP, not NO_READ_UP,
/// so a medium-integrity process usually *can* read an elevated process's
/// token — which made the old test answer "no mismatch" for a game that was
/// discarding every keystroke sent to it.
pub(crate) fn game_process_outranks_us() -> bool {
    let Some(window) = game_window() else {
        return false;
    };
    let mut pid = 0_u32;
    // SAFETY: `pid` is a live out-parameter.
    unsafe { GetWindowThreadProcessId(window, Some(&raw mut pid)) };
    if pid == 0 {
        return false;
    }
    // SAFETY: a refused open simply yields Err. Being unable to open the game
    // at all already means it is out of reach.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return true;
    };
    let game = integrity_level(process);
    // SAFETY: process came from OpenProcess.
    let _ = unsafe { CloseHandle(process) };

    // SAFETY: a pseudo-handle needing no close.
    let ours = integrity_level(unsafe { GetCurrentProcess() });
    match (game, ours) {
        (Some(game), Some(ours)) => game > ours,
        // Unreadable is itself a sign of being outranked, and saying so costs
        // the user a dialog they can decline.
        _ => true,
    }
}

pub(crate) fn confirm_relaunch_elevated(title: &str, body: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONWARNING, MB_SETFOREGROUND, MB_SYSTEMMODAL, MB_YESNO, MessageBoxW,
    };

    let title = HSTRING::from(title);
    let body = HSTRING::from(body);
    // SYSTEMMODAL and SETFOREGROUND because the user is looking at the game,
    // not at this application, which is the entire reason a status line did
    // not reach them.
    // SAFETY: both strings outlive the call; a null owner window is valid.
    let answer = unsafe {
        MessageBoxW(
            None,
            &body,
            &title,
            MB_YESNO | MB_ICONWARNING | MB_SYSTEMMODAL | MB_SETFOREGROUND,
        )
    };
    answer == IDYES
}

/// Starts an elevated copy of this executable and waits for it to exist.
///
/// Uses ShellExecuteExW rather than ShellExecuteW because the caller has to know
/// whether the new process actually started. ShellExecuteW returns success as
/// soon as it hands the request off, so a caller that exits on success kills its
/// own UAC prompt before the user can answer it — the app closed and nothing
/// happened. SEE_MASK_NOCLOSEPROCESS yields the process handle, and
/// SEE_MASK_NOASYNC keeps the call from returning before the shell has finished
/// with it, so an Ok here means there is something to hand over to.
pub(crate) fn relaunch_elevated(skip_arguments: &[&str]) -> Result<(), ElevateError> {
    const ERROR_CANCELLED: u32 = 1223;

    let executable = std::env::current_exe().map_err(|_| ElevateError::NoExecutablePath)?;
    let arguments = std::env::args()
        .skip(1)
        .filter(|argument| !skip_arguments.contains(&argument.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
    let directory = std::env::current_dir().unwrap_or_default();

    let operation = HSTRING::from("runas");
    let file = HSTRING::from(executable.as_os_str());
    let parameters = HSTRING::from(arguments);
    let working_directory = HSTRING::from(directory.as_os_str());

    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: windows::core::PCWSTR(operation.as_ptr()),
        lpFile: windows::core::PCWSTR(file.as_ptr()),
        lpDirectory: windows::core::PCWSTR(working_directory.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    if !parameters.is_empty() {
        info.lpParameters = windows::core::PCWSTR(parameters.as_ptr());
    }

    // SAFETY: every pointer above outlives the call, and cbSize matches.
    let launched = unsafe { ShellExecuteExW(&raw mut info) };
    if let Err(error) = launched {
        return Err(match error.code().0 as u32 & 0xFFFF {
            ERROR_CANCELLED => ElevateError::Declined,
            _ => ElevateError::Failed(error.code().0),
        });
    }
    if info.hProcess.is_invalid() {
        return Err(ElevateError::Failed(0));
    }
    // SAFETY: the handle came from ShellExecuteExW and is closed exactly once.
    let _ = unsafe { CloseHandle(info.hProcess) };
    Ok(())
}
