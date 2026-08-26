use std::f64::consts::PI;

use crate::wave::{ValidatedWave, WaveValidationError};

const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;
const DURATION_SECONDS: f64 = 2.15;

/// Generates the same original, self-contained cue shipped by the .NET 1.0 release.
/// No game audio or network asset is used.
#[must_use]
pub fn built_in_alert_wave() -> Result<ValidatedWave, WaveValidationError> {
    ValidatedWave::from_bytes("built-in-alert.wav", built_in_alert_wave_bytes())
}

fn built_in_alert_wave_bytes() -> Vec<u8> {
    let sample_count = (f64::from(SAMPLE_RATE) * DURATION_SECONDS) as usize;
    let pcm_byte_count = sample_count * usize::from(BITS_PER_SAMPLE / 8);
    let mut wave = Vec::with_capacity(44 + pcm_byte_count);
    wave.extend_from_slice(b"RIFF");
    wave.extend_from_slice(&(36 + pcm_byte_count as u32).to_le_bytes());
    wave.extend_from_slice(b"WAVEfmt ");
    wave.extend_from_slice(&16_u32.to_le_bytes());
    wave.extend_from_slice(&1_u16.to_le_bytes());
    wave.extend_from_slice(&CHANNELS.to_le_bytes());
    wave.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;
    wave.extend_from_slice(&(SAMPLE_RATE * u32::from(block_align)).to_le_bytes());
    wave.extend_from_slice(&block_align.to_le_bytes());
    wave.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    wave.extend_from_slice(b"data");
    wave.extend_from_slice(&(pcm_byte_count as u32).to_le_bytes());
    for index in 0..sample_count {
        let elapsed = index as f64 / f64::from(SAMPLE_RATE);
        wave.extend_from_slice(&rare_drop_sample(elapsed).to_le_bytes());
    }
    wave
}

fn rare_drop_sample(elapsed: f64) -> i16 {
    let value = impact(elapsed)
        + bell(elapsed, 0.030, 659.25, 0.55, 0.34)
        + bell(elapsed, 0.115, 987.77, 0.64, 0.30)
        + bell(elapsed, 0.225, 1_318.51, 0.72, 0.27)
        + bell(elapsed, 0.365, 1_975.53, 0.78, 0.20)
        + bell(elapsed, 0.515, 2_637.02, 0.62, 0.12);
    (f64::tanh(value * 1.12) * 0.72 * f64::from(i16::MAX)).round() as i16
}

fn impact(elapsed: f64) -> f64 {
    if !(0.0..=0.28).contains(&elapsed) {
        return 0.0;
    }
    let attack = f64::min(1.0, elapsed / 0.006);
    let decay = f64::exp(-elapsed * 15.0);
    let phase = 2.0 * PI * ((126.0 * elapsed) - (82.0 * elapsed * elapsed));
    f64::sin(phase) * attack * decay * 0.42
}

fn bell(elapsed: f64, start: f64, frequency: f64, decay_seconds: f64, gain: f64) -> f64 {
    let position = elapsed - start;
    if !(0.0..=1.15).contains(&position) {
        return 0.0;
    }
    let attack = f64::min(1.0, position / 0.0045);
    let decay = f64::exp(-position / decay_seconds);
    let fundamental = f64::sin(2.0 * PI * frequency * position);
    let overtone = f64::sin(2.0 * PI * frequency * 2.006 * position) * 0.31;
    let shimmer = f64::sin(2.0 * PI * frequency * 3.973 * position) * 0.10;
    (fundamental + overtone + shimmer) * attack * decay * gain
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn built_in_cue_is_fixed_valid_non_silent_pcm() {
        let wave = built_in_alert_wave().unwrap();
        assert_eq!(wave.bytes().len(), 189_674);
        assert_eq!(wave.format().channels, 1);
        assert_eq!(wave.format().sample_rate, 44_100);
        assert_eq!(wave.format().bits_per_sample, 16);
        assert!(
            wave.bytes()[44..]
                .chunks_exact(2)
                .any(|sample| sample != [0, 0])
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(wave.bytes())),
            "857653c9b1a95fd539d6d8d05112fc8ec0f8870f2371d0b1b07bfe4b03570d24"
        );
    }
}
