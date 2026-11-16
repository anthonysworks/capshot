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

/// Converts interleaved PCM to another channel count and sample rate
/// (linear interpolation: enough for a music bed under a recording).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn remix(
    pcm: &[i16],
    from_channels: usize,
    from_rate: u32,
    channels: usize,
    rate: u32,
) -> Vec<i16> {
    let (from_channels, channels) = (from_channels.max(1), channels.max(1));
    let frames = pcm.len() / from_channels;
    if frames == 0 {
        return Vec::new();
    }
    // One sample of output channel `c` for source frame `f`.
    let channel = |f: usize, c: usize| -> f32 {
        let frame = &pcm[f * from_channels..(f + 1) * from_channels];
        if channels == 1 {
            frame.iter().map(|&s| f32::from(s)).sum::<f32>() / from_channels as f32
        } else {
            f32::from(frame[c.min(from_channels - 1)])
        }
    };
    let ratio = f64::from(from_rate.max(1)) / f64::from(rate.max(1));
    let out_frames = if from_rate == rate {
        frames
    } else {
        (frames as f64 / ratio) as usize
    };
    let mut out = Vec::with_capacity(out_frames * channels);
    for o in 0..out_frames {
        let position = o as f64 * ratio;
        let a = (position as usize).min(frames - 1);
        let b = (a + 1).min(frames - 1);
        let t = (position - a as f64) as f32;
        for c in 0..channels {
            let v = channel(a, c) * (1.0 - t) + channel(b, c) * t;
            out.push(v.round().clamp(-32768.0, 32767.0) as i16);
        }
    }
    out
}

/// Where a music track sits on the result and how loud it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bed {
    /// Output time the music starts at, in seconds.
    pub start: f64,
    /// How long it plays at most, in seconds.
    pub length: f64,
    pub volume: f32,
    pub fade_in: f64,
    pub fade_out: f64,
}

/// Mixes `music` (interleaved, same rate and channels as `pcm`, starting at
/// the point of the file the music is played from) into `pcm`, which begins
/// at output time `pcm_start` seconds.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn mix_bed(
    pcm: &mut [i16],
    channels: usize,
    rate: u32,
    pcm_start: f64,
    music: &[i16],
    bed: &Bed,
) {
    let channels = channels.max(1);
    let music_frames = music.len() / channels;
    let length = bed.length.min(music_frames as f64 / f64::from(rate.max(1)));
    for (frame, samples) in pcm.chunks_mut(channels).enumerate() {
        let rel = pcm_start + frame as f64 / f64::from(rate.max(1)) - bed.start;
        if rel < 0.0 {
            continue;
        }
        if rel >= length {
            break;
        }
        let at = (rel * f64::from(rate)) as usize;
        if at >= music_frames {
            break;
        }
        let mut gain = f64::from(bed.volume);
        if bed.fade_in > 0.0 {
            gain *= (rel / bed.fade_in).clamp(0.0, 1.0);
        }
        if bed.fade_out > 0.0 {
            gain *= ((length - rel) / bed.fade_out).clamp(0.0, 1.0);
        }
        for (c, s) in samples.iter_mut().enumerate() {
            let mixed = f32::from(*s) + f32::from(music[at * channels + c]) * gain as f32;
            *s = mixed.round().clamp(-32768.0, 32767.0) as i16;
        }
    }
}

/// Mixes `chunk` (interleaved, same rate and channels as `pcm`) into `pcm`
/// starting `at` seconds from its beginning, scaled by `volume`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn mix_chunk(pcm: &mut [i16], channels: usize, rate: u32, chunk: &[i16], at: f64, volume: f32) {
    let channels = channels.max(1);
    let first = (at.max(0.0) * f64::from(rate)) as usize * channels;
    for (i, s) in chunk.iter().enumerate() {
        let Some(out) = pcm.get_mut(first + i) else {
            break;
        };
        let mixed = f32::from(*out) + f32::from(*s) * volume;
        *out = mixed.round().clamp(-32768.0, 32767.0) as i16;
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
    fn remix_changes_channels_and_rate() {
        let mono: Vec<i16> = (0..480).map(|i| i * 10).collect();
        let stereo = remix(&mono, 1, 48_000, 2, 48_000);
        assert_eq!(stereo.len(), 960);
        assert_eq!(stereo[10], stereo[11]);
        let half = remix(&stereo, 2, 48_000, 2, 24_000);
        assert_eq!(half.len(), 480);
        let down = remix(&stereo, 2, 48_000, 1, 48_000);
        assert_eq!(down.len(), 480);
        assert_eq!(down[7], mono[7]);
    }

    #[test]
    fn music_starts_ends_and_fades_where_asked() {
        let rate = 1000;
        let music = vec![1000i16; 4000 * 2];
        let mut pcm = vec![0i16; 4000 * 2];
        let bed = Bed {
            start: 1.0,
            length: 2.0,
            volume: 1.0,
            fade_in: 1.0,
            fade_out: 0.0,
        };
        mix_bed(&mut pcm, 2, rate, 0.0, &music, &bed);
        assert_eq!(pcm[2 * 500], 0, "silent before it starts");
        assert!(
            pcm[2 * 1500] > 400 && pcm[2 * 1500] < 600,
            "{}",
            pcm[2 * 1500]
        );
        assert_eq!(pcm[2 * 2500], 1000, "full level after the fade");
        assert_eq!(pcm[2 * 3500], 0, "silent after its length");
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
