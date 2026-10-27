//! Screen recording.
//!
//! ```text
//! Windows.Graphics.Capture ─▶ latest frame (GPU) ─▶ pacer @ N FPS
//!     ─▶ video processor: crop + BGRA→NV12 (GPU) ─▶ hardware encoder ─▶ MP4
//! ```
//!
//! Frames never leave the GPU. The pacer emits frames at a constant rate
//! (repeating the last one when the screen does not change) so the result is
//! a constant-frame-rate video that every editor handles well.

mod convert;
mod encoder;
mod pacer;
mod source;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use kadra_capture::{Gpu, GpuKind};
use kadra_core::{Point, Rect, VideoCodec};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};

pub use encoder::VideoSettings;

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("Windows API call failed: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("the area to record is empty")]
    EmptyRegion,
    #[error("the recording thread stopped unexpectedly")]
    ThreadLost,
}

pub type Result<T> = std::result::Result<T, RecordError>;

#[derive(Debug, Clone)]
pub struct RecordOptions {
    /// Area to record, in desktop coordinates. It is clipped to the monitor
    /// containing its centre.
    pub region: Rect,
    pub fps: u32,
    pub codec: VideoCodec,
    /// 0 (best) to 51 (smallest).
    pub quality: u8,
    pub capture_cursor: bool,
    pub output: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RecordingStats {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Codec actually used (see [`codec_fallbacks`]).
    pub codec: VideoCodec,
    /// Frames written to the file.
    pub frames: u64,
    /// Frames skipped because the pacer fell behind.
    pub dropped: u64,
    /// Distinct frames delivered by the system (screen changes).
    pub captured: u64,
    pub duration: Duration,
}

/// A recording in progress. Dropping it stops the recording.
#[derive(Debug)]
pub struct Recording {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<RecordingStats>>>,
    started: Instant,
    size: (u32, u32),
}

/// Monitor containing the centre of `region`, with its desktop rectangle.
fn monitor_for(region: Rect) -> (HMONITOR, Rect) {
    let centre = Point::new(
        region.left + region.width() / 2,
        region.top + region.height() / 2,
    );
    // SAFETY: plain queries with a correctly sized structure.
    unsafe {
        let monitor = MonitorFromPoint(
            windows::Win32::Foundation::POINT {
                x: centre.x,
                y: centre.y,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        let mut info = MONITORINFO {
            #[allow(clippy::cast_possible_truncation)]
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let r = info.rcMonitor;
        (monitor, Rect::new(r.left, r.top, r.right, r.bottom))
    }
}

/// Clips `region` to `monitor` and makes its size even (required by NV12),
/// returning it relative to the monitor.
pub fn encoded_area(region: Rect, monitor: Rect) -> Option<Rect> {
    let r = region
        .intersect(monitor)
        .offset(-monitor.left, -monitor.top);
    let (w, h) = (r.width() & !1, r.height() & !1);
    (w >= 2 && h >= 2).then(|| Rect::from_size(r.left, r.top, w, h))
}

impl Recording {
    /// Starts recording. Returns once the capture and the encoder are ready,
    /// so configuration errors are reported here.
    pub fn start(options: RecordOptions) -> Result<Self> {
        let (monitor, monitor_rect) = monitor_for(options.region);
        let area = encoded_area(options.region, monitor_rect).ok_or(RecordError::EmptyRegion)?;

        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread_stop = Arc::clone(&stop);
        let monitor_raw = monitor.0 as isize;
        let thread = std::thread::Builder::new()
            .name("kadra-recorder".into())
            .spawn(move || {
                let monitor = HMONITOR(monitor_raw as *mut _);
                run(&options, monitor, area, &thread_stop, &ready_tx)
            })
            .map_err(|_| RecordError::ThreadLost)?;

        match ready_rx.recv() {
            Ok(Ok(size)) => Ok(Self {
                stop,
                thread: Some(thread),
                started: Instant::now(),
                size,
            }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => match thread.join() {
                Ok(Err(err)) => Err(err),
                _ => Err(RecordError::ThreadLost),
            },
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Encoded frame size.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Stops recording and finalises the file.
    pub fn stop(mut self) -> Result<RecordingStats> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread
            .take()
            .ok_or(RecordError::ThreadLost)?
            .join()
            .map_err(|_| RecordError::ThreadLost)?
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Codecs to try, best first: hardware AV1 needs a recent GPU (RTX 40,
/// Radeon RX 7000, Arc), HEVC most GPUs since 2016, H.264 works everywhere.
pub fn codec_fallbacks(requested: VideoCodec) -> &'static [VideoCodec] {
    match requested {
        VideoCodec::Av1 => &[VideoCodec::Av1, VideoCodec::Hevc, VideoCodec::H264],
        VideoCodec::Hevc => &[VideoCodec::Hevc, VideoCodec::H264],
        VideoCodec::H264 => &[VideoCodec::H264],
    }
}

/// Opens the encoder with the requested codec, falling back to an older one
/// when the GPU has no hardware encoder for it.
fn open_encoder(
    gpu: &Gpu,
    options: &RecordOptions,
    size: (u32, u32),
    fps: u32,
) -> Result<(encoder::Encoder, VideoSettings)> {
    let mut last_error = None;
    for &codec in codec_fallbacks(options.codec) {
        let settings = VideoSettings {
            width: size.0,
            height: size.1,
            fps,
            codec,
            quality: options.quality,
        };
        match encoder::Encoder::new(gpu, &options.output, &settings) {
            Ok(encoder) => return Ok((encoder, settings)),
            Err(err) => {
                tracing::warn!(?codec, "no usable encoder: {err}");
                // The failed attempt may have left an empty file behind.
                let _ = std::fs::remove_file(&options.output);
                last_error = Some(err);
            }
        }
    }
    Err(last_error.map_or(RecordError::ThreadLost, RecordError::Windows))
}

/// Recording thread body.
fn run(
    options: &RecordOptions,
    monitor: HMONITOR,
    area: Rect,
    stop: &AtomicBool,
    ready: &mpsc::SyncSender<Result<(u32, u32)>>,
) -> Result<RecordingStats> {
    #[allow(clippy::cast_sign_loss)]
    let size = (area.width() as u32, area.height() as u32);
    let setup = || -> Result<_> {
        // SAFETY: initialises the Windows Runtime for this thread.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        let gpu = Gpu::with_kind(GpuKind::Hardware)?;
        let fps = options.fps.clamp(1, 240);
        let source = source::Source::start(&gpu, monitor, fps, options.capture_cursor)?;
        let converter = source.with_latest(|latest| {
            convert::Converter::new(&gpu, latest, source.size(), area, size, fps)
        })?;
        let (encoder, settings) = open_encoder(&gpu, options, size, fps)?;
        tracing::info!(
            width = size.0,
            height = size.1,
            fps,
            codec = ?settings.codec,
            bitrate = settings.bitrate(),
            "recording started"
        );
        Ok((gpu, source, converter, encoder, fps, settings.codec))
    };

    let (_gpu, source, converter, encoder, fps, codec) = match setup() {
        Ok(parts) => parts,
        Err(err) => {
            // The caller receives the actual error through `ready`.
            let _ = ready.send(Err(err));
            return Err(RecordError::ThreadLost);
        }
    };
    let _ = ready.send(Ok(size));

    // Give the system a moment to deliver the first frame, so the video
    // does not start with a black frame.
    let deadline = Instant::now() + Duration::from_millis(250);
    while source.frames() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }

    let pace = pacer::Pacer::new(fps)?;
    let frame_ticks = encoder::TICKS_PER_SECOND / i64::from(fps);
    let mut frames = 0u64;
    let started = Instant::now();
    let mut result = Ok(());

    while !stop.load(Ordering::Relaxed) {
        let index = match pace.wait_next() {
            pacer::Tick::Frame(index) => index,
            pacer::Tick::Late { index, skipped } => {
                tracing::debug!(skipped, "recording fell behind");
                index
            }
        };
        let frame = source
            .with_latest(|_| converter.convert())
            .and_then(|texture| {
                #[allow(clippy::cast_possible_wrap)]
                encoder.write_video(&texture, index as i64 * frame_ticks, frame_ticks)
            });
        if let Err(err) = frame {
            result = Err(err);
            break;
        }
        frames += 1;
    }

    let finished = encoder.finish();
    let stats = RecordingStats {
        path: options.output.clone(),
        width: size.0,
        height: size.1,
        codec,
        frames,
        dropped: pace.skipped(),
        captured: source.frames(),
        duration: started.elapsed(),
    };
    tracing::info!(
        frames = stats.frames,
        dropped = stats.dropped,
        captured = stats.captured,
        seconds = stats.duration.as_secs_f64(),
        "recording finished"
    );
    result.and(finished)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_area_is_clipped_relative_and_even() {
        let monitor = Rect::new(1920, 0, 3840, 1080);
        let area = encoded_area(Rect::new(1900, 101, 2121, 400), monitor).unwrap();
        assert_eq!(area, Rect::new(0, 101, 200, 399));
        assert!(encoded_area(Rect::new(0, 0, 10, 10), monitor).is_none());
        assert!(encoded_area(Rect::new(1920, 0, 1921, 500), monitor).is_none());
    }
}
