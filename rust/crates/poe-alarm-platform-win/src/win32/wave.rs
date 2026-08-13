use windows::Win32::Media::Audio::{
    PlaySoundW, SND_ASYNC, SND_FLAGS, SND_LOOP, SND_MEMORY, SND_NODEFAULT,
};
use windows::core::PCWSTR;

use crate::PlatformError;

pub(crate) fn play_wave(bytes: &[u8]) -> Result<(), PlatformError> {
    if bytes.is_empty() {
        return Err(PlatformError::Win32 {
            operation: "PlaySoundW",
            code: 0,
            message: "validated WAV memory was unexpectedly empty".to_owned(),
        });
    }
    let flags = SND_FLAGS(SND_ASYNC.0 | SND_NODEFAULT.0 | SND_MEMORY.0 | SND_LOOP.0);
    // SAFETY: ValidatedWave retains this heap allocation until stop completes.
    // WinMM interprets the PCWSTR-typed pointer as RIFF bytes with SND_MEMORY.
    let accepted = unsafe { PlaySoundW(PCWSTR(bytes.as_ptr().cast::<u16>()), None, flags) };
    if accepted.as_bool() {
        Ok(())
    } else {
        Err(PlatformError::Win32 {
            operation: "PlaySoundW",
            code: 0,
            message: "WinMM rejected the validated PCM WAV".to_owned(),
        })
    }
}

pub(crate) fn stop_wave() -> Result<(), PlatformError> {
    // SAFETY: A null sound pointer and zero flags is WinMM's documented stop operation.
    let stopped = unsafe { PlaySoundW(PCWSTR::null(), None, SND_FLAGS(0)) };
    if stopped.as_bool() {
        Ok(())
    } else {
        Err(PlatformError::Win32 {
            operation: "PlaySoundW(stop)",
            code: 0,
            message: "WinMM did not accept the stop request".to_owned(),
        })
    }
}
