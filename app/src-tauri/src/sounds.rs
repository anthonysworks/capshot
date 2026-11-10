//! Short sounds played by Kadra itself.

use std::sync::OnceLock;

use std::time::Duration;

use windows::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT, SND_SYNC};
use windows::core::PCWSTR;

/// After the chime returns, what is still buffered in the audio device.
const DRAIN: Duration = Duration::from_millis(120);

/// A soft, round two-note chime: the recording starts. Returns once it has
/// been heard, so that the recording can begin right after it without
/// catching it.
pub fn recording_starts() {
    static CHIME: OnceLock<Vec<u8>> = OnceLock::new();
    let wav = CHIME.get_or_init(|| wav(&chime(48_000), 48_000));
    // SAFETY: the WAV data is static and the playback synchronous;
    // SND_MEMORY makes the "name" a pointer to it.
    let played = unsafe {
        PlaySoundW(
            PCWSTR(wav.as_ptr().cast()),
            None,
            SND_MEMORY | SND_SYNC | SND_NODEFAULT,
        )
    };
    if played.as_bool() {
        std::thread::sleep(DRAIN);
    }
}

/// Two gentle sine notes (G5 then C6), each fading out quickly.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn chime(rate: u32) -> Vec<i16> {
    let rate_f = rate as f32;
    let length = (rate_f * 0.3) as usize;
    let note = |t: f32, start: f32, hz: f32| {
        let t = t - start;
        if t < 0.0 {
            return 0.0;
        }
        let attack = (t / 0.012).min(1.0);
        let phase = 2.0 * std::f32::consts::PI * hz * t;
        attack * (-t * 14.0).exp() * (phase.sin() + 0.12 * (2.0 * phase).sin())
    };
    (0..length)
        .map(|i| {
            let t = i as f32 / rate_f;
            let v = 0.55 * note(t, 0.0, 783.99) + 0.5 * note(t, 0.08, 1046.5);
            (v * 0.22 * 32767.0).clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

/// 16-bit mono PCM wrapped in a WAV file.
#[allow(clippy::cast_possible_truncation)]
fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_is_a_valid_quiet_wav() {
        let samples = chime(48_000);
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(peak > 2000 && peak < 12_000, "peak {peak}");
        let file = wav(&samples, 48_000);
        assert_eq!(&file[..4], b"RIFF");
        assert_eq!(file.len(), 44 + samples.len() * 2);
    }
}
