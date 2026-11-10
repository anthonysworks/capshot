//! Streaming GIF encoder.
//!
//! ```text
//! push(frame, time) ─▶ downscale ─▶ diff with the previous frame
//!     ─▶ crop to the changed area, unchanged pixels transparent
//!     ─▶ workers: palette quantisation (imagequant), in parallel
//!     ─▶ writer: back in order, delays from timestamps ─▶ .gif
//! ```
//!
//! Screen recordings change little from one frame to the next: identical
//! frames only lengthen the previous one, and a changed frame only stores
//! the rectangle that changed. Each frame gets its own 256-colour palette,
//! so quality does not degrade over long recordings, and memory stays
//! bounded whatever the duration.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use capshot_core::{BgraImage, Rect};
use imagequant::RGBA;

/// GIF delays are in hundredths of a second; browsers treat anything
/// shorter than 2 as 10, so 50 FPS is the useful maximum.
pub const MAX_FPS: u32 = 50;

#[derive(Debug, thiserror::Error)]
pub enum GifError {
    #[error("cannot write the GIF: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot encode the GIF: {0}")]
    Encoding(#[from] gif::EncodingError),
    #[error("palette quantisation failed: {0}")]
    Quantize(#[from] imagequant::Error),
    #[error("the image is too large for a GIF ({0}×{1})")]
    TooLarge(u32, u32),
    #[error("the GIF encoder stopped unexpectedly")]
    Stopped,
}

pub type Result<T> = std::result::Result<T, GifError>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GifOptions {
    /// Frames wider than this are scaled down (0 = keep the size).
    pub max_width: u32,
    /// Palette quality, 1–100.
    pub quality: u8,
    /// Floyd–Steinberg dithering: smoother gradients, larger files.
    pub dither: bool,
    /// Loop forever instead of playing once.
    pub repeat: bool,
    /// Per-channel difference below which a pixel counts as unchanged.
    /// 0 for screen captures (exact); a few units for decoded video, whose
    /// compression noise changes every pixel slightly.
    pub tolerance: u8,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            max_width: 0,
            quality: 90,
            dither: true,
            repeat: true,
            tolerance: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GifSummary {
    pub width: u32,
    pub height: u32,
    /// Frames written (identical frames are merged).
    pub frames: u64,
    pub bytes: u64,
}

/// Output size for a `width` × `height` source.
pub fn output_size(width: u32, height: u32, options: &GifOptions) -> (u32, u32) {
    if options.max_width == 0 || width <= options.max_width {
        return (width, height);
    }
    let h = u64::from(height) * u64::from(options.max_width) / u64::from(width);
    (
        options.max_width,
        u32::try_from(h).unwrap_or(u32::MAX).max(1),
    )
}

/// A frame ready for quantisation.
struct Job {
    index: u64,
    time: Duration,
    left: u16,
    top: u16,
    width: u16,
    height: u16,
    /// Straight RGBA; alpha 0 marks pixels that did not change.
    pixels: Vec<RGBA>,
}

/// A frame ready to be written.
struct Quantized {
    index: u64,
    time: Duration,
    left: u16,
    top: u16,
    width: u16,
    height: u16,
    palette: Vec<u8>,
    transparent: Option<u8>,
    indices: Vec<u8>,
}

enum Message {
    Frame(Quantized),
    Failed(GifError),
    /// No frame follows; the last one lasts until this time.
    End(Duration),
}

/// Writes a GIF from frames pushed in time order.
#[derive(Debug)]
pub struct GifWriter {
    options: GifOptions,
    size: (u32, u32),
    jobs: Option<SyncSender<Job>>,
    workers: Vec<JoinHandle<()>>,
    results: Sender<Message>,
    writer: Option<JoinHandle<Result<GifSummary>>>,
    /// What the viewer sees after the frames sent so far.
    shown: Option<BgraImage>,
    next_index: u64,
}

impl GifWriter {
    /// Creates `path` for frames of `width` × `height` (before scaling).
    pub fn create(path: &Path, width: u32, height: u32, options: GifOptions) -> Result<Self> {
        let size = output_size(width, height, &options);
        let (w, h) = match (u16::try_from(size.0), u16::try_from(size.1)) {
            (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
            _ => return Err(GifError::TooLarge(size.0, size.1)),
        };
        let mut encoder = gif::Encoder::new(BufWriter::new(File::create(path)?), w, h, &[])?;
        if options.repeat {
            encoder.set_repeat(gif::Repeat::Infinite)?;
        }

        let threads = std::thread::available_parallelism()
            .map_or(2, std::num::NonZero::get)
            .saturating_sub(1)
            .clamp(1, 6);
        let (jobs, jobs_rx) = mpsc::sync_channel::<Job>(threads * 2);
        let jobs_rx = Arc::new(Mutex::new(jobs_rx));
        let (results, results_rx) = mpsc::channel();

        let mut workers = Vec::with_capacity(threads);
        for i in 0..threads {
            let jobs_rx = Arc::clone(&jobs_rx);
            let results = results.clone();
            let worker = std::thread::Builder::new()
                .name(format!("capshot-gif-{i}"))
                .spawn(move || quantize_loop(&jobs_rx, &results, options))?;
            workers.push(worker);
        }
        let file_size = size;
        let writer = std::thread::Builder::new()
            .name("capshot-gif-writer".into())
            .spawn(move || write_loop(encoder, &results_rx, file_size))?;

        Ok(Self {
            options,
            size,
            jobs: Some(jobs),
            workers,
            results,
            writer: Some(writer),
            shown: None,
            next_index: 0,
        })
    }

    /// Size of the frames written.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Adds a frame shown from `time` (relative to the start) until the
    /// next one. Blocks when the workers fall behind.
    pub fn push(&mut self, frame: &BgraImage, time: Duration) -> Result<()> {
        let scaled;
        let frame = if (frame.width(), frame.height()) == self.size {
            frame
        } else {
            scaled = frame.thumbnail(self.size.0, self.size.1);
            &scaled
        };
        if (frame.width(), frame.height()) != self.size {
            // Should not happen: the source changed size.
            tracing::warn!("GIF frame of unexpected size dropped");
            return Ok(());
        }

        let tolerance = self.options.tolerance;
        let job = match &mut self.shown {
            None => {
                self.shown = Some(frame.clone());
                Some(full_frame(frame))
            }
            Some(shown) => changed_area(shown, frame, tolerance)
                .map(|area| delta_frame(shown, frame, area, tolerance)),
        };
        let Some((area, pixels)) = job else {
            // Identical frame: the previous one simply lasts longer.
            return Ok(());
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let job = Job {
            index: self.next_index,
            time,
            left: area.left as u16,
            top: area.top as u16,
            width: area.width() as u16,
            height: area.height() as u16,
            pixels,
        };
        self.next_index += 1;
        self.jobs
            .as_ref()
            .ok_or(GifError::Stopped)?
            .send(job)
            .map_err(|_| GifError::Stopped)?;
        Ok(())
    }

    /// Writes the remaining frames and closes the file; the last frame
    /// lasts until `end`.
    pub fn finish(mut self, end: Duration) -> Result<GifSummary> {
        self.jobs = None;
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        let _ = self.results.send(Message::End(end));
        let writer = self.writer.take().ok_or(GifError::Stopped)?;
        // `self.results` is the last sender: the writer stops on `End`.
        writer.join().map_err(|_| GifError::Stopped)?
    }

    pub fn options(&self) -> &GifOptions {
        &self.options
    }
}

impl Drop for GifWriter {
    fn drop(&mut self) {
        self.jobs = None;
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = self.results.send(Message::End(Duration::ZERO));
            let _ = writer.join();
        }
    }
}

fn rgba(px: [u8; 4]) -> RGBA {
    RGBA::new(px[2], px[1], px[0], 255)
}

fn full_frame(frame: &BgraImage) -> (Rect, Vec<RGBA>) {
    let pixels = frame
        .as_bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&px| rgba(px))
        .collect();
    (frame.bounds(), pixels)
}

/// Whether two BGRA pixels count as equal: exactly, or within
/// `tolerance` on every channel (decoded video carries compression noise
/// that would otherwise make every frame differ everywhere).
#[inline]
fn same(p: [u8; 4], q: [u8; 4], tolerance: u8) -> bool {
    if tolerance == 0 {
        return p == q;
    }
    p[..3]
        .iter()
        .zip(&q[..3])
        .all(|(&x, &y)| x.abs_diff(y) <= tolerance)
}

/// Bounding box of the pixels that differ from what is shown, if any.
fn changed_area(shown: &BgraImage, frame: &BgraImage, tolerance: u8) -> Option<Rect> {
    let width = frame.width() as usize;
    let rows_a = shown.as_bytes().chunks_exact(width * 4);
    let rows_b = frame.as_bytes().chunks_exact(width * 4);
    let (mut left, mut right) = (usize::MAX, 0);
    let (mut top, mut bottom) = (usize::MAX, 0);
    for (y, (a, b)) in rows_a.zip(rows_b).enumerate() {
        if a == b {
            continue;
        }
        let a = a.as_chunks::<4>().0;
        let b = b.as_chunks::<4>().0;
        let differs = |(p, q): (&[u8; 4], &[u8; 4])| !same(*p, *q, tolerance);
        let Some(first) = a.iter().zip(b).position(differs) else {
            continue;
        };
        let last = a.iter().zip(b).rposition(differs).unwrap_or(first);
        left = left.min(first);
        right = right.max(last + 1);
        top = top.min(y);
        bottom = y + 1;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    (top != usize::MAX).then(|| Rect::new(left as i32, top as i32, right as i32, bottom as i32))
}

/// The changed area of `frame`, with pixels equal to what is shown made
/// transparent so that the previous frame shows through (and compresses
/// to almost nothing). `shown` is updated to what the viewer will see, so
/// that small differences never accumulate into drift.
fn delta_frame(
    shown: &mut BgraImage,
    frame: &BgraImage,
    area: Rect,
    tolerance: u8,
) -> (Rect, Vec<RGBA>) {
    #[allow(clippy::cast_sign_loss)]
    let (left, top, right, bottom) = (
        area.left as usize,
        area.top as usize,
        area.right as usize,
        area.bottom as usize,
    );
    let stride = frame.width() as usize * 4;
    let mut pixels = Vec::with_capacity((right - left) * (bottom - top));
    for y in top..bottom {
        let span = y * stride + left * 4..y * stride + right * 4;
        let before = shown.as_bytes_mut()[span.clone()].as_chunks_mut::<4>().0;
        let after = frame.as_bytes()[span].as_chunks::<4>().0;
        pixels.extend(before.iter_mut().zip(after).map(|(old, &new)| {
            if same(*old, new, tolerance) {
                RGBA::new(0, 0, 0, 0)
            } else {
                *old = new;
                rgba(new)
            }
        }));
    }
    (area, pixels)
}

fn quantize_loop(jobs: &Mutex<Receiver<Job>>, results: &Sender<Message>, options: GifOptions) {
    loop {
        let job = {
            let jobs = jobs.lock().unwrap_or_else(PoisonError::into_inner);
            jobs.recv()
        };
        let Ok(job) = job else {
            return;
        };
        let message = match quantize(job, options) {
            Ok(frame) => Message::Frame(frame),
            Err(err) => Message::Failed(err),
        };
        if results.send(message).is_err() {
            return;
        }
    }
}

fn quantize(job: Job, options: GifOptions) -> Result<Quantized> {
    let mut attr = imagequant::new();
    attr.set_quality(0, options.quality.clamp(1, 100))?;
    // Speed 6 keeps a 1080p frame well under 100 ms on one core with no
    // visible loss for screen content.
    attr.set_speed(6)?;
    let mut image = attr.new_image(
        job.pixels,
        usize::from(job.width),
        usize::from(job.height),
        0.0,
    )?;
    let mut result = attr.quantize(&mut image)?;
    result.set_dithering_level(if options.dither { 1.0 } else { 0.0 })?;
    let (palette, indices) = result.remapped(&mut image)?;

    let transparent = palette
        .iter()
        .position(|c| c.a == 0)
        .and_then(|i| u8::try_from(i).ok());
    let palette = palette.iter().flat_map(|c| [c.r, c.g, c.b]).collect();
    Ok(Quantized {
        index: job.index,
        time: job.time,
        left: job.left,
        top: job.top,
        width: job.width,
        height: job.height,
        palette,
        transparent,
        indices,
    })
}

/// Position of `time` on the GIF timeline, in hundredths of a second.
fn centis(time: Duration) -> u64 {
    u64::try_from((time.as_millis() + 5) / 10).unwrap_or(u64::MAX)
}

fn write_loop(
    mut encoder: gif::Encoder<BufWriter<File>>,
    results: &Receiver<Message>,
    size: (u32, u32),
) -> Result<GifSummary> {
    let mut waiting = BTreeMap::new();
    let mut next = 0u64;
    let mut pending: Option<Quantized> = None;
    let mut written = 0u64;
    let mut failure = None;

    let mut write = |frame: &Quantized, until: Duration| -> Result<()> {
        // Delays are derived from absolute positions so that rounding never
        // accumulates into drift.
        let delay = centis(until).saturating_sub(centis(frame.time)).max(2);
        let out = gif::Frame {
            delay: u16::try_from(delay).unwrap_or(u16::MAX),
            dispose: gif::DisposalMethod::Keep,
            transparent: frame.transparent,
            left: frame.left,
            top: frame.top,
            width: frame.width,
            height: frame.height,
            palette: Some(frame.palette.clone()),
            buffer: std::borrow::Cow::Borrowed(&frame.indices),
            ..gif::Frame::default()
        };
        encoder.write_frame(&out)?;
        written += 1;
        Ok(())
    };

    let end = loop {
        match results.recv() {
            Ok(Message::Frame(frame)) => {
                waiting.insert(frame.index, frame);
                while let Some(frame) = waiting.remove(&next) {
                    next += 1;
                    if let Some(previous) = pending.take()
                        && failure.is_none()
                        && let Err(err) = write(&previous, frame.time)
                    {
                        failure = Some(err);
                    }
                    pending = Some(frame);
                }
            }
            Ok(Message::Failed(err)) => {
                failure.get_or_insert(err);
            }
            Ok(Message::End(end)) => break end,
            Err(_) => break Duration::ZERO,
        }
    };
    if let Some(err) = failure {
        return Err(err);
    }
    if let Some(last) = pending.take() {
        let end = end.max(last.time);
        write(&last, end)?;
    }
    drop(write);
    let file = encoder
        .into_inner()
        .map_err(|e| GifError::Io(std::io::Error::other(e.to_string())))?;
    let file = file
        .into_inner()
        .map_err(|e| GifError::Io(e.into_error()))?;
    let bytes = file.metadata().map_or(0, |m| m.len());
    Ok(GifSummary {
        width: size.0,
        height: size.1,
        frames: written,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> BgraImage {
        BgraImage::from_raw(w, h, bgra.repeat((w * h) as usize))
    }

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    struct Decoded {
        frames: Vec<(u16, Rect, Option<u8>)>,
        last: Vec<u8>,
    }

    fn decode(path: &Path) -> Decoded {
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(File::open(path).unwrap()).unwrap();
        let (w, h) = (decoder.width() as usize, decoder.height() as usize);
        let mut canvas = vec![0u8; w * h * 4];
        let mut frames = Vec::new();
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            let (fl, ft) = (usize::from(frame.left), usize::from(frame.top));
            let fw = usize::from(frame.width);
            for (i, px) in frame.buffer.as_chunks::<4>().0.iter().enumerate() {
                if px[3] != 0 {
                    let (x, y) = (fl + i % fw, ft + i / fw);
                    canvas[(y * w + x) * 4..][..4].copy_from_slice(px);
                }
            }
            frames.push((
                frame.delay,
                Rect::from_size(
                    i32::from(frame.left),
                    i32::from(frame.top),
                    i32::from(frame.width),
                    i32::from(frame.height),
                ),
                frame.transparent,
            ));
        }
        Decoded {
            frames,
            last: canvas,
        }
    }

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "capshot-gif-test-{}-{name}.gif",
            std::process::id()
        ))
    }

    #[test]
    fn identical_frames_are_merged_and_timed() {
        let path = temp("merge");
        let a = solid(8, 8, [255, 0, 0, 255]);
        let b = solid(8, 8, [0, 0, 255, 255]);
        let mut gif = GifWriter::create(&path, 8, 8, GifOptions::default()).unwrap();
        gif.push(&a, ms(0)).unwrap();
        gif.push(&a, ms(100)).unwrap();
        gif.push(&b, ms(200)).unwrap();
        let summary = gif.finish(ms(300)).unwrap();
        assert_eq!(summary.frames, 2);
        let decoded = decode(&path);
        let delays: Vec<u16> = decoded.frames.iter().map(|f| f.0).collect();
        assert_eq!(delays, [20, 10]);
        // Blue in BGRA is red in RGBA order once decoded.
        assert_eq!(&decoded.last[..4], &[255, 0, 0, 255]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn later_frames_only_store_what_changed() {
        let path = temp("delta");
        let a = solid(16, 16, [10, 20, 30, 255]);
        let mut b = a.clone();
        // Two pixels change: (3, 2) and (5, 4).
        for (x, y) in [(3usize, 2usize), (5, 4)] {
            b.as_bytes_mut()[(y * 16 + x) * 4..][..4].copy_from_slice(&[200, 100, 50, 255]);
        }
        let mut gif = GifWriter::create(&path, 16, 16, GifOptions::default()).unwrap();
        gif.push(&a, ms(0)).unwrap();
        gif.push(&b, ms(67)).unwrap();
        gif.finish(ms(133)).unwrap();

        let decoded = decode(&path);
        assert_eq!(decoded.frames.len(), 2);
        assert_eq!(decoded.frames[0].1, Rect::new(0, 0, 16, 16));
        assert_eq!(decoded.frames[1].1, Rect::new(3, 2, 6, 5));
        assert!(
            decoded.frames[1].2.is_some(),
            "unchanged pixels are transparent"
        );
        // Delays follow absolute positions: 0 → 7 → 13 hundredths.
        assert_eq!(decoded.frames[0].0, 7);
        assert_eq!(decoded.frames[1].0, 6);
        // The composed result equals the last frame.
        let expected: Vec<u8> = b
            .as_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect();
        assert_eq!(decoded.last, expected);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn frames_are_scaled_down_to_the_maximum_width() {
        let path = temp("scale");
        let options = GifOptions {
            max_width: 50,
            ..GifOptions::default()
        };
        assert_eq!(output_size(200, 100, &options), (50, 25));
        assert_eq!(output_size(40, 100, &options), (40, 100));
        let mut gif = GifWriter::create(&path, 200, 100, options).unwrap();
        gif.push(&solid(200, 100, [0, 255, 0, 255]), ms(0)).unwrap();
        let summary = gif.finish(ms(500)).unwrap();
        assert_eq!((summary.width, summary.height), (50, 25));
        assert!(summary.bytes > 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tolerance_ignores_noise_without_drift() {
        let path = temp("noise");
        let options = GifOptions {
            tolerance: 4,
            ..GifOptions::default()
        };
        let mut gif = GifWriter::create(&path, 8, 8, options).unwrap();
        // Slowly brightening by 1 per frame: each step is within the
        // tolerance, but the drift must not exceed it.
        for i in 0..10u8 {
            gif.push(
                &solid(8, 8, [100 + i, 100 + i, 100 + i, 255]),
                ms(u64::from(i) * 100),
            )
            .unwrap();
        }
        let summary = gif.finish(ms(1000)).unwrap();
        // 100 shown, then updated at 105 (diff 5 > 4); 109 stays within 4.
        assert_eq!(summary.frames, 2);
        let decoded = decode(&path);
        assert!(decoded.last[0].abs_diff(109) <= 4, "{}", decoded.last[0]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn many_frames_stay_in_order() {
        let path = temp("order");
        let mut gif = GifWriter::create(&path, 32, 32, GifOptions::default()).unwrap();
        let mut frame = solid(32, 32, [0, 0, 0, 255]);
        for i in 0..40u8 {
            let at = usize::from(i) * 4;
            frame.as_bytes_mut()[at..at + 4].copy_from_slice(&[i * 6, 255 - i * 6, 128, 255]);
            gif.push(&frame, ms(u64::from(i) * 50)).unwrap();
        }
        let summary = gif.finish(ms(2000)).unwrap();
        assert_eq!(summary.frames, 40);
        let decoded = decode(&path);
        assert!(decoded.frames.iter().all(|f| f.0 == 5));
        let _ = std::fs::remove_file(&path);
    }
}
