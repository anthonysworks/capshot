//! Audio for edited videos: tempo change without pitch change, and click
//! sounds.
//!
//! Speeding a recording up by dropping or repeating samples would make
//! voices high-pitched (or slow and deep). WSOLA (waveform-similarity
//! overlap-add) instead cuts the sound into short overlapping windows and
//! lays them out at the new pace, choosing each window where it lines up
//! best with the previous one, so the pitch stays natural.

use std::f32::consts::PI;

use crate::compose::ClickVoice;

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

/// The click sound `voice` as `rate` Hz mono samples in -1..1, with a peak
/// of one. The preview in the editor makes the same samples (`clickSamples` in
/// `stage.ts`).
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn click_sound(rate: u32, voice: ClickVoice) -> Vec<f32> {
    let rate = rate.max(1) as f32;
    let seconds = match voice {
        ClickVoice::Soft => 0.14,
        ClickVoice::Pop => 0.08,
        ClickVoice::Click => 0.1,
    };
    let length = (rate * seconds) as usize;
    let mut phase = 0.0f32;
    let mut seed: u32 = 0x2545_f491;
    let mut low = 0.0f32;
    let mut out: Vec<f32> = (0..length)
        .map(|i| {
            let t = i as f32 / rate;
            match voice {
                ClickVoice::Soft => {
                    // A fingertip on a table: low, round, no edge.
                    let pitch = 150.0 + 110.0 * (-t * 35.0).exp();
                    phase += 2.0 * PI * pitch / rate;
                    let attack = if t < 0.008 {
                        0.5 - 0.5 * (PI * t / 0.008).cos()
                    } else {
                        1.0
                    };
                    attack * (-t * 32.0).exp() * (phase.sin() + 0.06 * (2.0 * phase).sin())
                }
                ClickVoice::Pop => {
                    let pitch = 320.0 + 520.0 * (1.0 - (-t * 80.0).exp());
                    phase += 2.0 * PI * pitch / rate;
                    (t / 0.002).min(1.0) * (-t * 60.0).exp() * phase.sin()
                }
                ClickVoice::Click => {
                    // Press, then release: two short, dull snaps.
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let noise = (f64::from(seed) / 4_294_967_296.0 * 2.0 - 1.0) as f32;
                    low += 0.3 * (noise - low);
                    let press = (-t * 700.0).exp();
                    let r = t - 0.055;
                    let release = if r >= 0.0 {
                        0.55 * (-r * 900.0).exp()
                    } else {
                        0.0
                    };
                    phase += 2.0 * PI * 1900.0 / rate;
                    (press + release) * (0.8 * low * 3.0 + 0.2 * phase.sin())
                }
            }
        })
        .collect();
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > 0.0 {
        for v in &mut out {
            *v /= peak;
        }
    }
    out
}

/// How loud a click sound is mixed into the sound track.
pub const fn click_volume(voice: ClickVoice) -> f32 {
    match voice {
        ClickVoice::Soft | ClickVoice::Click => 0.3,
        ClickVoice::Pop => 0.25,
    }
}

/// Multiplies interleaved PCM by `gain(t)`, `t` in seconds from its start.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub fn apply_gain(pcm: &mut [i16], channels: usize, rate: u32, gain: impl Fn(f64) -> f32) {
    let channels = channels.max(1);
    for (frame, samples) in pcm.chunks_mut(channels).enumerate() {
        let g = gain(frame as f64 / f64::from(rate.max(1)));
        if (g - 1.0).abs() < f32::EPSILON {
            continue;
        }
        for s in samples {
            *s = (f32::from(*s) * g).round().clamp(-32768.0, 32767.0) as i16;
        }
    }
}

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
        mix_at(
            &mut pcm,
            2,
            rate,
            &click_sound(rate, ClickVoice::Soft),
            &[0.5],
            0.8,
        );
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
