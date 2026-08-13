use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::PlatformError;

const MINIMUM_WAVE_BYTES: usize = 44;
const MAXIMUM_WAVE_BYTES: usize = 32 * 1024 * 1024;

static NEXT_PLAYER_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE_PLAYER_ID: AtomicU64 = AtomicU64::new(0);
static PLAYBACK_GATE: Mutex<()> = Mutex::new(());

/// Parsed standard PCM format accepted by the zero-dependency WinMM route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PcmWaveFormat {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub block_align: u16,
}

/// Stable validation categories suitable for localized UI messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaveValidationErrorKind {
    NoPath,
    NotFound,
    InvalidSize,
    Io,
    InvalidContainer,
    TruncatedContainer,
    TruncatedChunk,
    IncompleteFormat,
    UnsupportedFormat,
    MissingAudioData,
}

/// A local WAV validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WaveValidationError {
    pub kind: WaveValidationErrorKind,
    pub detail: String,
}

impl WaveValidationError {
    fn new(kind: WaveValidationErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for WaveValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for WaveValidationError {}

/// Fully validated local WAV bytes whose allocation remains stable for async playback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedWave {
    path: PathBuf,
    bytes: Box<[u8]>,
    format: PcmWaveFormat,
}

impl ValidatedWave {
    /// Reads and validates a local PCM WAV file using the .NET 1.0 limits.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WaveValidationError> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Err(WaveValidationError::new(
                WaveValidationErrorKind::NoPath,
                "no WAV file was selected",
            ));
        }

        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| {
                    WaveValidationError::new(WaveValidationErrorKind::Io, error.to_string())
                })?
                .join(path)
        };
        let metadata = fs::metadata(&absolute).map_err(|error| {
            let kind = if error.kind() == std::io::ErrorKind::NotFound {
                WaveValidationErrorKind::NotFound
            } else {
                WaveValidationErrorKind::Io
            };
            WaveValidationError::new(kind, format!("could not read WAV metadata: {error}"))
        })?;
        if !metadata.is_file() {
            return Err(WaveValidationError::new(
                WaveValidationErrorKind::NotFound,
                "the WAV path is not a regular file",
            ));
        }
        let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if !(MINIMUM_WAVE_BYTES..=MAXIMUM_WAVE_BYTES).contains(&length) {
            return Err(WaveValidationError::new(
                WaveValidationErrorKind::InvalidSize,
                "WAV size must be between 44 bytes and 32 MiB",
            ));
        }

        let bytes = fs::read(&absolute).map_err(|error| {
            WaveValidationError::new(
                WaveValidationErrorKind::Io,
                format!("could not read WAV data: {error}"),
            )
        })?;
        if !(MINIMUM_WAVE_BYTES..=MAXIMUM_WAVE_BYTES).contains(&bytes.len()) {
            return Err(WaveValidationError::new(
                WaveValidationErrorKind::InvalidSize,
                "WAV size changed while reading and is outside the 44 byte to 32 MiB limit",
            ));
        }
        let format = validate_pcm_wave(&bytes)?;
        Ok(Self {
            path: absolute,
            bytes: bytes.into_boxed_slice(),
            format,
        })
    }

    /// Builds validated playback data from memory. This is useful for a bundled
    /// fallback cue while retaining the exact production parser.
    pub fn from_bytes(
        logical_path: impl Into<PathBuf>,
        bytes: Vec<u8>,
    ) -> Result<Self, WaveValidationError> {
        if !(MINIMUM_WAVE_BYTES..=MAXIMUM_WAVE_BYTES).contains(&bytes.len()) {
            return Err(WaveValidationError::new(
                WaveValidationErrorKind::InvalidSize,
                "WAV size must be between 44 bytes and 32 MiB",
            ));
        }
        let format = validate_pcm_wave(&bytes)?;
        Ok(Self {
            path: logical_path.into(),
            bytes: bytes.into_boxed_slice(),
            format,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn format(&self) -> PcmWaveFormat {
        self.format
    }
}

/// Validates RIFF/WAVE PCM bytes with the same accepted interchange format as 1.0:
/// mono/stereo, 8-192 kHz, and 8/16/24/32 bits per sample.
pub fn validate_pcm_wave(wave: &[u8]) -> Result<PcmWaveFormat, WaveValidationError> {
    if wave.len() < MINIMUM_WAVE_BYTES
        || wave.get(0..4) != Some(b"RIFF")
        || wave.get(8..12) != Some(b"WAVE")
    {
        return Err(WaveValidationError::new(
            WaveValidationErrorKind::InvalidContainer,
            "the file is not a valid RIFF/WAVE stream",
        ));
    }

    let riff_size = read_u32(wave, 4).ok_or_else(|| {
        WaveValidationError::new(
            WaveValidationErrorKind::TruncatedContainer,
            "the RIFF header is truncated",
        )
    })?;
    if u64::from(riff_size) + 8 > wave.len() as u64 {
        return Err(WaveValidationError::new(
            WaveValidationErrorKind::TruncatedContainer,
            "the WAV length does not match its RIFF header",
        ));
    }

    let mut format = None;
    let mut has_audio_data = false;
    let mut offset = 12usize;
    while offset.checked_add(8).is_some_and(|end| end <= wave.len()) {
        let chunk_id = &wave[offset..offset + 4];
        let chunk_size = read_u32(wave, offset + 4).expect("bounded chunk header") as usize;
        let content_start = offset + 8;
        let content_end = content_start.checked_add(chunk_size).ok_or_else(|| {
            WaveValidationError::new(
                WaveValidationErrorKind::TruncatedChunk,
                "a WAV chunk size overflows the file boundary",
            )
        })?;
        if content_end > wave.len() {
            return Err(WaveValidationError::new(
                WaveValidationErrorKind::TruncatedChunk,
                "a WAV chunk extends beyond the file boundary",
            ));
        }

        if chunk_id == b"fmt " {
            if chunk_size < 16 {
                return Err(WaveValidationError::new(
                    WaveValidationErrorKind::IncompleteFormat,
                    "the WAV format chunk is incomplete",
                ));
            }
            let audio_format = read_u16(wave, content_start).expect("bounded fmt chunk");
            let channels = read_u16(wave, content_start + 2).expect("bounded fmt chunk");
            let sample_rate = read_u32(wave, content_start + 4).expect("bounded fmt chunk");
            let block_align = read_u16(wave, content_start + 12).expect("bounded fmt chunk");
            let bits_per_sample = read_u16(wave, content_start + 14).expect("bounded fmt chunk");
            let expected_block_align = u32::from(channels) * u32::from(bits_per_sample) / 8;
            let supported = audio_format == 1
                && matches!(channels, 1 | 2)
                && (8_000..=192_000).contains(&sample_rate)
                && matches!(bits_per_sample, 8 | 16 | 24 | 32)
                && u32::from(block_align) == expected_block_align;
            format = supported.then_some(PcmWaveFormat {
                channels,
                sample_rate,
                bits_per_sample,
                block_align,
            });
        } else if chunk_id == b"data" {
            // Preserve 1.0 behavior when malformed files contain multiple
            // data chunks: the final declared data chunk is authoritative.
            has_audio_data = chunk_size > 0;
        }

        let padding = chunk_size & 1;
        offset = content_end.checked_add(padding).ok_or_else(|| {
            WaveValidationError::new(
                WaveValidationErrorKind::TruncatedChunk,
                "a WAV chunk offset overflows the file boundary",
            )
        })?;
    }

    let format = format.ok_or_else(|| {
        WaveValidationError::new(
            WaveValidationErrorKind::UnsupportedFormat,
            "only standard mono/stereo PCM WAV at 8-32 bits is supported",
        )
    })?;
    if !has_audio_data {
        return Err(WaveValidationError::new(
            WaveValidationErrorKind::MissingAudioData,
            "the WAV file contains no audio data",
        ));
    }
    Ok(format)
}

/// Owns stable WAV memory for asynchronous, looping `PlaySoundW` playback.
#[derive(Debug)]
pub struct LoopingWavePlayer {
    id: u64,
    wave: ValidatedWave,
}

impl LoopingWavePlayer {
    #[must_use]
    pub fn new(wave: ValidatedWave) -> Self {
        let id = NEXT_PLAYER_ID.fetch_add(1, Ordering::Relaxed).max(1);
        Self { id, wave }
    }

    #[must_use]
    pub fn wave(&self) -> &ValidatedWave {
        &self.wave
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        ACTIVE_PLAYER_ID.load(Ordering::Acquire) == self.id
    }

    /// Submits asynchronous looping playback. Starting another player replaces
    /// the process's previous `PlaySoundW` sound, as WinMM specifies.
    pub fn start(&mut self) -> Result<(), PlatformError> {
        let _gate = PLAYBACK_GATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if self.is_playing() {
            return Ok(());
        }
        platform_play_wave(&self.wave.bytes)?;
        ACTIVE_PLAYER_ID.store(self.id, Ordering::Release);
        Ok(())
    }

    /// Stops this player if it still owns the process-wide WinMM playback slot.
    pub fn stop(&mut self) -> Result<(), PlatformError> {
        let _gate = PLAYBACK_GATE
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if ACTIVE_PLAYER_ID.load(Ordering::Acquire) != self.id {
            return Ok(());
        }
        platform_stop_wave()?;
        ACTIVE_PLAYER_ID.store(0, Ordering::Release);
        Ok(())
    }
}

impl Drop for LoopingWavePlayer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn platform_play_wave(bytes: &[u8]) -> Result<(), PlatformError> {
    #[cfg(windows)]
    {
        crate::win32::play_wave(bytes)
    }
    #[cfg(not(windows))]
    {
        crate::non_windows::play_wave(bytes)
    }
}

fn platform_stop_wave() -> Result<(), PlatformError> {
    #[cfg(windows)]
    {
        crate::win32::stop_wave()
    }
    #[cfg(not(windows))]
    {
        crate::non_windows::stop_wave()
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let value: [u8; 2] = bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_le_bytes(value))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let value: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm_wave(channels: u16, sample_rate: u32, bits: u16, samples: usize) -> Vec<u8> {
        let block_align = channels * bits / 8;
        let data_size = samples * usize::from(block_align);
        let mut wave = Vec::with_capacity(44 + data_size);
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&(36 + data_size as u32).to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&16u32.to_le_bytes());
        wave.extend_from_slice(&1u16.to_le_bytes());
        wave.extend_from_slice(&channels.to_le_bytes());
        wave.extend_from_slice(&sample_rate.to_le_bytes());
        wave.extend_from_slice(&(sample_rate * u32::from(block_align)).to_le_bytes());
        wave.extend_from_slice(&block_align.to_le_bytes());
        wave.extend_from_slice(&bits.to_le_bytes());
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&(data_size as u32).to_le_bytes());
        wave.resize(44 + data_size, 0);
        wave
    }

    #[test]
    fn accepts_each_stable_pcm_width() {
        for bits in [8, 16, 24, 32] {
            let parsed = validate_pcm_wave(&pcm_wave(2, 44_100, bits, 8)).unwrap();
            assert_eq!(parsed.bits_per_sample, bits);
            assert_eq!(parsed.channels, 2);
        }
    }

    #[test]
    fn accepts_unknown_and_odd_padded_chunks() {
        let base = pcm_wave(1, 8_000, 8, 8);
        let mut wave = Vec::new();
        wave.extend_from_slice(&base[..12]);
        wave.extend_from_slice(b"JUNK");
        wave.extend_from_slice(&1u32.to_le_bytes());
        wave.extend_from_slice(&[42, 0]);
        wave.extend_from_slice(&base[12..]);
        let riff_size = (wave.len() - 8) as u32;
        wave[4..8].copy_from_slice(&riff_size.to_le_bytes());
        assert!(validate_pcm_wave(&wave).is_ok());
    }

    #[test]
    fn rejects_float_pcm_and_empty_data() {
        let mut float = pcm_wave(1, 44_100, 32, 4);
        float[20..22].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(
            validate_pcm_wave(&float).unwrap_err().kind,
            WaveValidationErrorKind::UnsupportedFormat
        );

        let mut empty = pcm_wave(1, 44_100, 16, 1);
        empty.truncate(44);
        empty[4..8].copy_from_slice(&36u32.to_le_bytes());
        empty[40..44].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            validate_pcm_wave(&empty).unwrap_err().kind,
            WaveValidationErrorKind::MissingAudioData
        );
    }

    #[test]
    fn rejects_chunk_that_crosses_file_boundary() {
        let mut wave = pcm_wave(1, 44_100, 16, 4);
        wave[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            validate_pcm_wave(&wave).unwrap_err().kind,
            WaveValidationErrorKind::TruncatedChunk
        );
    }
}
