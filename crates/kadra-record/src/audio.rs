//! System sound (WASAPI loopback) and microphone capture, mixed into one
//! 48 kHz stereo track.
//!
//! Every packet carries the performance-counter time of its first sample.
//! Packets are placed on the recording timeline by that timestamp rather
//! than by arrival order, which keeps audio in sync with video and copes
//! with loopback capture sending nothing at all while the system is silent.

// Frame counts and sample positions stay far below the limits of these
// conversions (2^32 frames at 48 kHz is more than a day).
#![allow(clippy::cast_possible_truncation)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, EDataFlow,
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::core::Result;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// `WAVE_FORMAT_IEEE_FLOAT`.
const FORMAT_FLOAT: u16 = 3;
/// Packets within this many frames of the expected position are appended
/// contiguously: small timestamp jitter must not create clicks.
const JITTER_FRAMES: u64 = SAMPLE_RATE as u64 / 100;
/// Audio is mixed this far behind real time so late packets still fit.
const MIX_DELAY_FRAMES: u64 = SAMPLE_RATE as u64 / 10;
/// Mixing period.
const MIX_PERIOD: Duration = Duration::from_millis(20);

/// Current performance-counter time in 100 ns units, the clock used by
/// WASAPI packet timestamps.
pub fn now_ticks() -> i64 {
    let (mut counter, mut frequency) = (0i64, 0i64);
    // SAFETY: valid out pointers.
    unsafe {
        let _ = QueryPerformanceCounter(&mut counter);
        let _ = QueryPerformanceFrequency(&mut frequency);
    }
    #[allow(clippy::cast_possible_truncation)]
    let ticks = (i128::from(counter) * 10_000_000 / i128::from(frequency.max(1))) as i64;
    ticks
}

/// Samples of one source, positioned on the recording timeline (in frames
/// of `CHANNELS` interleaved samples).
#[derive(Debug, Default)]
pub struct Track {
    start: u64,
    samples: VecDeque<f32>,
}

impl Track {
    fn end(&self) -> u64 {
        self.start + (self.samples.len() / CHANNELS) as u64
    }

    /// Adds a packet whose first frame belongs at timeline frame `at`.
    pub fn push(&mut self, at: u64, samples: &[f32]) {
        if self.samples.is_empty() {
            self.start = at;
        } else if at > self.end() + JITTER_FRAMES {
            // A gap (e.g. loopback during silence): fill it with silence.
            let gap = (at - self.end()) as usize * CHANNELS;
            self.samples.extend(std::iter::repeat_n(0.0, gap));
        }
        self.samples.extend(samples);
    }

    /// Writes frames `[from, to)` into `out` (added to what is there),
    /// with silence where the track has no data, and forgets them.
    pub fn mix_into(&mut self, from: u64, to: u64, out: &mut [f32]) {
        debug_assert_eq!(out.len(), (to - from) as usize * CHANNELS);
        // Drop what is too old to be used.
        if self.start < from {
            let stale = ((from - self.start) as usize * CHANNELS).min(self.samples.len());
            self.samples.drain(..stale);
            self.start = from;
        }
        if self.samples.is_empty() {
            return;
        }
        let offset = (self.start.saturating_sub(from) as usize * CHANNELS).min(out.len());
        let count = (out.len() - offset).min(self.samples.len());
        for (dst, src) in out[offset..offset + count]
            .iter_mut()
            .zip(self.samples.drain(..count))
        {
            *dst += src;
        }
        self.start += (count / CHANNELS) as u64;
    }
}

/// One WASAPI capture stream converted by Windows to 48 kHz stereo float.
struct Source {
    name: &'static str,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    track: Track,
}

impl Source {
    fn open(enumerator: &IMMDeviceEnumerator, flow: EDataFlow, name: &'static str) -> Result<Self> {
        let format = WAVEFORMATEX {
            wFormatTag: FORMAT_FLOAT,
            nChannels: CHANNELS as u16,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * 4 * CHANNELS as u32,
            nBlockAlign: 4 * CHANNELS as u16,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        let mut flags =
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        if flow == eRender {
            flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
        }
        // SAFETY: COM calls with a valid format; one second of buffering.
        unsafe {
            let device = enumerator.GetDefaultAudioEndpoint(flow, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                10_000_000,
                0,
                &format,
                None,
            )?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Self {
                name,
                client,
                capture,
                track: Track::default(),
            })
        }
    }

    /// Moves every pending packet into the track.
    fn drain(&mut self, origin: i64) -> Result<()> {
        // SAFETY: buffers returned by GetBuffer are read before ReleaseBuffer.
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let (mut frames, mut flags, mut ticks) = (0u32, 0u32, 0u64);
                self.capture.GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    None,
                    Some(&mut ticks),
                )?;
                #[allow(clippy::cast_possible_wrap)]
                let relative = ticks as i64 - origin;
                if relative >= 0 && frames > 0 {
                    #[allow(clippy::cast_sign_loss)]
                    let at = (relative as u128 * u128::from(SAMPLE_RATE) / 10_000_000) as u64;
                    let len = frames as usize * CHANNELS;
                    #[allow(clippy::cast_possible_wrap)]
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                        self.track.push(at, &vec![0.0; len]);
                    } else {
                        // WASAPI buffers are allocated with at least 16-byte
                        // alignment and hold the float format requested.
                        #[allow(clippy::cast_ptr_alignment)]
                        let samples = std::slice::from_raw_parts(data.cast::<f32>(), len);
                        self.track.push(at, samples);
                    }
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
        Ok(())
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: stopping a started client.
        let _ = unsafe { self.client.Stop() };
    }
}

/// Converts mixed float samples to 16-bit PCM for the AAC encoder.
#[allow(clippy::cast_possible_truncation)]
pub fn to_pcm16(samples: &[f32], out: &mut Vec<i16>) {
    out.clear();
    out.extend(
        samples
            .iter()
            .map(|s| (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16),
    );
}

/// Receives mixed PCM: samples, timeline position and duration (100 ns).
pub type AudioSink = Box<dyn FnMut(&[i16], i64, i64) -> Result<()> + Send>;

/// Captures and mixes the selected sources on a dedicated thread.
pub struct AudioCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioCapture {
    /// Starts capturing. `origin` is the timeline zero (see [`now_ticks`]).
    /// Missing devices are skipped with a warning.
    pub fn start(system: bool, microphone: bool, origin: i64, mut sink: AudioSink) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("kadra-audio".into())
            .spawn(move || {
                // SAFETY: balanced with CoUninitialize below.
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                run(system, microphone, origin, &thread_stop, &mut sink);
                // SAFETY: matches CoInitializeEx above.
                unsafe { CoUninitialize() };
            })
            .ok();
        Self { stop, thread }
    }

    /// Mixes what remains up to now and stops.
    pub fn stop(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.finish();
    }
}

fn open_sources(system: bool, microphone: bool) -> Vec<Source> {
    // SAFETY: COM is initialised on this thread.
    let enumerator: Result<IMMDeviceEnumerator> =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) };
    let enumerator = match enumerator {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("audio devices unavailable: {err}");
            return Vec::new();
        }
    };
    let wanted = [
        (system, eRender, "system audio"),
        (microphone, eCapture, "microphone"),
    ];
    wanted
        .into_iter()
        .filter(|(on, _, _)| *on)
        .filter_map(
            |(_, flow, name)| match Source::open(&enumerator, flow, name) {
                Ok(source) => Some(source),
                Err(err) => {
                    tracing::warn!("cannot capture {name}: {err}");
                    None
                }
            },
        )
        .collect()
}

fn run(system: bool, microphone: bool, origin: i64, stop: &AtomicBool, sink: &mut AudioSink) {
    let mut sources = open_sources(system, microphone);
    if sources.is_empty() {
        return;
    }
    tracing::info!(
        sources = ?sources.iter().map(|s| s.name).collect::<Vec<_>>(),
        "audio capture started"
    );

    let mut cursor = 0u64;
    let mut mixed = Vec::new();
    let mut pcm = Vec::new();
    loop {
        let stopping = stop.load(Ordering::Relaxed);
        if !stopping {
            std::thread::sleep(MIX_PERIOD);
        }
        for source in &mut sources {
            if let Err(err) = source.drain(origin) {
                tracing::warn!("{} capture failed: {err}", source.name);
            }
        }

        // While running, stay behind real time so late packets still fit;
        // when stopping, mix everything up to now.
        #[allow(clippy::cast_sign_loss)]
        let now =
            ((now_ticks() - origin).max(0) as u128 * u128::from(SAMPLE_RATE) / 10_000_000) as u64;
        let target = if stopping {
            now
        } else {
            now.saturating_sub(MIX_DELAY_FRAMES)
        };
        if target > cursor {
            mixed.clear();
            mixed.resize((target - cursor) as usize * CHANNELS, 0.0);
            for source in &mut sources {
                source.track.mix_into(cursor, target, &mut mixed);
            }
            to_pcm16(&mixed, &mut pcm);
            let time = ticks_at(cursor);
            let duration = ticks_at(target) - time;
            if let Err(err) = sink(&pcm, time, duration) {
                tracing::warn!("cannot encode audio: {err}");
                return;
            }
            cursor = target;
        }
        if stopping {
            return;
        }
    }
}

/// Timeline frame → 100 ns units.
#[allow(clippy::cast_possible_wrap)]
fn ticks_at(frame: u64) -> i64 {
    (u128::from(frame) * 10_000_000 / u128::from(SAMPLE_RATE)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(frames: usize, value: f32) -> Vec<f32> {
        vec![value; frames * CHANNELS]
    }

    #[test]
    fn contiguous_packets_mix_in_order() {
        let mut track = Track::default();
        track.push(0, &stereo(10, 0.5));
        track.push(10, &stereo(10, 0.25));
        let mut out = vec![0.0; 20 * CHANNELS];
        track.mix_into(0, 20, &mut out);
        assert_eq!(out[0], 0.5);
        assert_eq!(out[19 * CHANNELS], 0.25);
    }

    #[test]
    fn gaps_become_silence() {
        let mut track = Track::default();
        track.push(0, &stereo(10, 1.0));
        // Next packet 1 s later: loopback sent nothing in between.
        track.push(48_000, &stereo(10, 1.0));
        let mut out = vec![0.0; 48_010 * CHANNELS];
        track.mix_into(0, 48_010, &mut out);
        assert_eq!(out[5 * CHANNELS], 1.0);
        assert_eq!(out[1000 * CHANNELS], 0.0);
        assert_eq!(out[48_005 * CHANNELS], 1.0);
    }

    #[test]
    fn small_jitter_is_appended() {
        let mut track = Track::default();
        track.push(0, &stereo(100, 1.0));
        track.push(105, &stereo(100, 1.0));
        assert_eq!(track.end(), 200);
    }

    #[test]
    fn late_start_and_stale_data() {
        let mut track = Track::default();
        track.push(50, &stereo(10, 1.0));
        let mut out = vec![0.0; 100 * CHANNELS];
        track.mix_into(0, 100, &mut out);
        assert_eq!(out[49 * CHANNELS], 0.0);
        assert_eq!(out[50 * CHANNELS], 1.0);
        assert_eq!(out[60 * CHANNELS], 0.0);

        // Data older than the mixing cursor is discarded.
        let mut track = Track::default();
        track.push(0, &stereo(100, 1.0));
        let mut out = vec![0.0; 10 * CHANNELS];
        track.mix_into(95, 105, &mut out);
        assert_eq!(out[4 * CHANNELS], 1.0);
        assert_eq!(out[5 * CHANNELS], 0.0);
    }

    #[test]
    fn sources_are_summed_and_clamped() {
        let mut a = Track::default();
        let mut b = Track::default();
        a.push(0, &stereo(4, 0.75));
        b.push(0, &stereo(4, 0.75));
        let mut out = vec![0.0; 4 * CHANNELS];
        a.mix_into(0, 4, &mut out);
        b.mix_into(0, 4, &mut out);
        let mut pcm = Vec::new();
        to_pcm16(&out, &mut pcm);
        assert_eq!(pcm[0], i16::MAX);
    }

    #[test]
    fn timeline_conversion() {
        assert_eq!(ticks_at(48_000), 10_000_000);
        assert_eq!(ticks_at(480), 100_000);
    }
}
