#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let self_test = std::env::args().any(|argument| argument == "--self-test");
    let result = if self_test {
        poe_alarm_app_win::run_self_test()
    } else {
        poe_alarm_app_win::run()
    };
    if let Err(error) = result {
        show_startup_error(&error);
        std::process::exit(1);
    }
}

fn show_startup_error(error: &str) {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        use windows::core::PCWSTR;

        let message = format!("POE Alarm could not start.\r\n\r\n{error}");
        let message = message
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let title = "POE Alarm"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    eprintln!("POE Alarm could not start: {error}");
}
