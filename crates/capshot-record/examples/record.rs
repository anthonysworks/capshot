//! Records the primary monitor, then checks the file and prints statistics.
//!
//! ```text
//! cargo run --release -p capshot-record --example record -- <out.mp4> [seconds] [fps] [h264|hevc|av1] [pause]
//! ```
//!
//! An output ending in `.gif` records a GIF (`CAPSHOT_GIF_WIDTH` limits its
//! width).

// A measurement tool: lossy numeric conversions are fine here.
#![allow(
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::unchecked_time_subtraction,
    clippy::cast_ptr_alignment
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use capshot_core::{Rect, VideoCodec};
use capshot_record::{GifOptions, RecordOptions, Recording};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Media::MediaFoundation::{
    IMFSourceReader, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_PD_DURATION,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READER_MEDIASOURCE, MF_SOURCE_READERF_ENDOFSTREAM, MF_VERSION, MFAudioFormat_PCM,
    MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Audio, MFSTARTUP_FULL, MFStartup,
};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
use windows::core::HSTRING;

fn cpu_time() -> Duration {
    let (mut c, mut e, mut k, mut u) = Default::default();
    // SAFETY: valid out pointers for the current process.
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
    Duration::from_nanos((ticks(k) + ticks(u)) * 100)
}

/// Reads the file back: duration and number of encoded video frames.
fn probe(path: &Path) -> windows::core::Result<(f64, u64)> {
    // SAFETY: Media Foundation calls with valid arguments.
    unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let reader: IMFSourceReader =
            MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
        let duration = reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)?;
        let ticks = u64::try_from(&duration).unwrap_or(0);
        let mut frames = 0;
        loop {
            let mut flags = 0;
            let mut sample = None;
            reader.ReadSample(
                MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                0,
                None,
                Some(&mut flags),
                None,
                Some(&mut sample),
            )?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            if sample.is_some() {
                frames += 1;
            }
        }
        Ok((ticks as f64 / 1e7, frames))
    }
}

/// Decodes the audio track and returns its length and the time of the first
/// sample louder than -30 dBFS, to check audio/video synchronisation.
fn probe_audio(path: &Path) -> windows::core::Result<(f64, Option<f64>)> {
    // SAFETY: Media Foundation calls with valid arguments; buffers are
    // read while locked.
    unsafe {
        let reader: IMFSourceReader =
            MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
        let pcm = MFCreateMediaType()?;
        pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        reader.SetCurrentMediaType(stream, None, &pcm)?;
        let (mut end, mut onset) = (0.0, None);
        loop {
            let (mut flags, mut time) = (0, 0i64);
            let mut sample = None;
            reader.ReadSample(
                stream,
                0,
                None,
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(sample) = sample else { continue };
            let buffer = sample.ConvertToContiguousBuffer()?;
            let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
            buffer.Lock(&mut data, None, Some(&mut len))?;
            let samples = std::slice::from_raw_parts(data.cast::<i16>(), len as usize / 2);
            if onset.is_none()
                && let Some(i) = samples.iter().position(|s| s.unsigned_abs() > 1036)
            {
                onset = Some(time as f64 / 1e7 + (i / 2) as f64 / 48_000.0);
            }
            buffer.Unlock()?;
            end = (time + sample.GetSampleDuration().unwrap_or(0)) as f64 / 1e7;
        }
        Ok((end, onset))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let output = PathBuf::from(
        args.get(1)
            .ok_or("usage: record <out.mp4> [s] [fps] [codec]")?,
    );
    let seconds: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let fps: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);
    let codec = match args.get(4).map(String::as_str) {
        Some("hevc") => VideoCodec::Hevc,
        Some("av1") => VideoCodec::Av1,
        _ => VideoCodec::H264,
    };

    // SAFETY: called once at start-up.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)? };
    // SAFETY: plain metrics queries.
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };

    let gif = output
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"))
        .then(|| GifOptions {
            max_width: std::env::var("CAPSHOT_GIF_WIDTH")
                .ok()
                .and_then(|w| w.parse().ok())
                .unwrap_or(0),
            ..GifOptions::default()
        });
    let cpu_before = cpu_time();
    let start = Instant::now();
    let recording = Recording::start(RecordOptions {
        region: Rect::new(0, 0, w, h),
        fps,
        codec,
        quality: 23,
        capture_cursor: true,
        system_audio: gif.is_none(),
        microphone: std::env::var("CAPSHOT_MIC").is_ok(),
        start_paused: false,
        output: output.clone(),
        gif,
        clean_output: std::env::var_os("CAPSHOT_CLEAN").map(PathBuf::from),
        cursor_track: std::env::var_os("CAPSHOT_TRACK").map(PathBuf::from),
    })?;
    println!(
        "started in {:.0} ms, {:?}",
        start.elapsed().as_secs_f64() * 1e3,
        recording.size()
    );
    if args.get(5).is_some_and(|a| a == "pause") {
        // Record half, pause for as long, record the other half.
        let half = Duration::from_secs(seconds) / 2;
        std::thread::sleep(half);
        recording.pause();
        std::thread::sleep(half);
        recording.resume();
        std::thread::sleep(half);
    } else {
        std::thread::sleep(Duration::from_secs(seconds));
    }
    let stats = recording.stop()?;
    let wall = start.elapsed();
    let cpu = cpu_time() - cpu_before;

    let size = std::fs::metadata(&output)?.len();
    #[allow(clippy::cast_precision_loss)]
    let mbit = size as f64 * 8.0 / 1e6 / stats.duration.as_secs_f64();
    println!("codec: {:?}", stats.codec);
    if let Some(thumb) = &stats.thumbnail {
        println!("thumbnail: {}x{}", thumb.width(), thumb.height());
    }
    println!(
        "{} frames ({} dropped, {} screen updates) in {:.2} s, {:.1} MB, {mbit:.1} Mbit/s",
        stats.frames,
        stats.dropped,
        stats.captured,
        stats.duration.as_secs_f64(),
        size as f64 / 1e6
    );
    println!(
        "CPU: {:.1} % of one core",
        cpu.as_secs_f64() / wall.as_secs_f64() * 100.0
    );
    if gif.is_some() {
        return Ok(());
    }
    let (duration, frames) = probe(&output)?;
    println!("file: {duration:.2} s, {frames} video frames");
    match probe_audio(&output) {
        Ok((length, onset)) => println!("audio: {length:.2} s, first sound at {onset:?} s"),
        Err(err) => println!("audio: none ({err})"),
    }
    Ok(())
}
