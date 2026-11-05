//! Audio for edited videos: tempo change without pitch change, and click
//! sounds.
//!
//! Speeding a recording up by dropping or repeating samples would make
//! voices high-pitched (or slow and deep). WSOLA (waveform-similarity
//! overlap-add) instead cuts the sound into short overlapping windows and
//! lays them out at the new pace, choosing each window where it lines up
//! best with the previous one, so the pitch stays natural.

use std::f32::consts::PI;

/// Changes the duration of interleaved 16-bit PCM by `1 / tempo` (tempo 2:
/// twice as fast) without changing its pitch.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::needless_range_loop
)]
pub fn stretch(pcm: &[i16], channels: usize, rate: u32, tempo: f64) -> Vec<i16> {
    let channels = channels.max(1);
    let frames = pcm.len() / channels;
    if (tempo - 1.0).abs() < 1e-3 || frames == 0 || tempo <= 0.0 {
        return pcm.to_vec();
    }
    // 40 ms windows, half overlapping; up to ±12 ms of search.
    let n = ((f64::from(rate) * 0.04) as usize).max(64) & !1;
    let hop = n / 2;
    let search = (f64::from(rate) * 0.012) as usize;
    let window: Vec<f32> = (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos())
        .collect();
    // Mono copy for the similarity search.
    let mono: Vec<f32> = pcm
        .chunks_exact(channels)
        .map(|f| f.iter().map(|&s| f32::from(s)).sum::<f32>() / channels as f32)
        .collect();
    let out_frames = (frames as f64 / tempo).round() as usize;
    let mut out = vec![0f32; (out_frames + n) * channels];
    let last_start = frames.saturating_sub(n);

    let mut previous: Option<usize> = None;
    let mut k = 0usize;
    loop {
        let out_pos = k * hop;
        if out_pos >= out_frames {
            break;
        }
        let nominal = ((out_pos as f64 * tempo) as usize).min(last_start);
        let pos = match previous {
            None => nominal,
            Some(prev) => {
                // Where the previous window would naturally continue.
                let target = (prev + hop).min(last_start);
                let from = nominal.saturating_sub(search);
                let to = (nominal + search).min(last_start);
                let mut best = nominal;
                let mut best_score = f32::MIN;
                // Coarse search on every 4th sample: plenty for speech and
                // music, and 16 times cheaper.
                let mut c = from;
                while c <= to {
                    let mut score = 0f32;
                    let mut i = 0;
                    while i < hop {
                        score += mono[c + i] * mono[target + i];
                        i += 4;
                    }
                    if score > best_score {
                        best_score = score;
                        best = c;
                    }
                    c += 2;
                }
                best
            }
        };
        for i in 0..n {
            let src = pos + i;
            if src >= frames {
                break;
            }
            let w = window[i];
            let dst = (out_pos + i) * channels;
            for ch in 0..channels {
                out[dst + ch] += w * f32::from(pcm[src * channels + ch]);
            }
        }
        previous = Some(pos);
        k += 1;
    }
    out.truncate(out_frames * channels);
    out.into_iter()
        .map(|s| s.round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}

/// A short, soft, round "tock" (like a finger tapping wood), as `rate` Hz
/// mono samples in -1..1: a low sine whose pitch drops from 480 to 240 Hz,
/// with a gentle attack and a quick fade. No high frequencies, so it never
/// sounds shrill.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn click_sound(rate: u32) -> Vec<f32> {
    let rate = rate.max(1) as f32;
    let length = (rate * 0.09) as usize;
    let mut phase = 0.0f32;
    (0..length)
        .map(|i| {
            let t = i as f32 / rate;
            let pitch = 240.0 + 240.0 * (-t * 60.0).exp();
            phase += 2.0 * PI * pitch / rate;
            let attack = (t / 0.003).min(1.0);
            let envelope = attack * (-t * 55.0).exp();
            // A touch of the octave gives body without brightness.
            envelope * (0.85 * phase.sin() + 0.15 * (2.0 * phase).sin())
        })
        .collect()
}

/// How loud clicks are mixed into the sound track.
pub const CLICK_VOLUME: f32 = 0.35;

/// Mixes `sound` into interleaved PCM at each time of `times` (seconds).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn mix_at(
    pcm: &mut [i16],
    channels: usize,
    rate: u32,
    sound: &[f32],
    times: &[f64],
    volume: f32,
) {
    let channels = channels.max(1);
    let frames = pcm.len() / channels;
    for &t in times {
        if t < 0.0 {
            continue;
        }
        let start = (t * f64::from(rate)) as usize;
        for (i, s) in sound.iter().enumerate() {
            let frame = start + i;
            if frame >= frames {
                break;
            }
            for ch in 0..channels {
                let at = frame * channels + ch;
                let mixed = f32::from(pcm[at]) + s * volume * 32767.0;
                pcm[at] = mixed.clamp(-32768.0, 32767.0) as i16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, seconds: f64, hz: f32) -> Vec<i16> {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        (0..(f64::from(rate) * seconds) as usize)
            .flat_map(|i| {
                let v = ((2.0 * PI * hz * i as f32 / rate as f32).sin() * 12000.0) as i16;
                [v, v]
            })
            .collect()
    }

    /// Zero crossings per second of the left channel: the pitch.
    #[allow(clippy::cast_precision_loss)]
    fn crossings(pcm: &[i16], rate: u32) -> f32 {
        let left: Vec<i16> = pcm.iter().step_by(2).copied().collect();
        let n = left.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        n as f32 / (left.len() as f32 / rate as f32)
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn duration_changes_but_pitch_does_not() {
        let rate = 48_000;
        let input = tone(rate, 2.0, 440.0);
        for tempo in [0.5, 1.5, 2.0] {
            let out = stretch(&input, 2, rate, tempo);
            let expected = input.len() as f64 / tempo;
            #[allow(clippy::cast_precision_loss)]
            let ratio = out.len() as f64 / expected;
            assert!(
                (ratio - 1.0).abs() < 0.01,
                "tempo {tempo}: length ratio {ratio}"
            );
            let pitch = crossings(&out[4000..out.len() - 4000], rate) / 2.0;
            assert!((pitch - 440.0).abs() < 15.0, "tempo {tempo}: {pitch} Hz");
        }
    }

    #[test]
    fn clicks_are_mixed_at_their_time() {
        let rate = 48_000;
        let mut pcm = vec![0i16; 48_000 * 2];
        mix_at(&mut pcm, 2, rate, &click_sound(rate), &[0.5], 0.8);
        let loud: Vec<usize> = pcm
            .iter()
            .enumerate()
            .filter(|(_, s)| s.unsigned_abs() > 1000)
            .map(|(i, _)| i / 2)
            .collect();
        let first = loud.first().copied().unwrap_or(0);
        assert!((24_000..24_100).contains(&first), "{first}");
    }
}
