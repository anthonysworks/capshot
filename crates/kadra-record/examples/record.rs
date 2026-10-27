//! Records the primary monitor, then checks the file and prints statistics.
//!
//! ```text
//! cargo run --release -p kadra-record --example record -- <out.mp4> [seconds] [fps] [h264|hevc|av1]
//! ```

// A measurement tool: lossy numeric conversions are fine here.
#![allow(
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::unchecked_time_subtraction
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kadra_core::{Rect, VideoCodec};
use kadra_record::{RecordOptions, Recording};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Media::MediaFoundation::{
    IMFSourceReader, MF_PD_DURATION, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READER_MEDIASOURCE, MF_SOURCE_READERF_ENDOFSTREAM, MF_VERSION,
    MFCreateSourceReaderFromURL, MFSTARTUP_FULL, MFStartup,
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

    let cpu_before = cpu_time();
    let start = Instant::now();
    let recording = Recording::start(RecordOptions {
        region: Rect::new(0, 0, w, h),
        fps,
        codec,
        quality: 23,
        capture_cursor: true,
        output: output.clone(),
    })?;
    println!(
        "started in {:.0} ms, {:?}",
        start.elapsed().as_secs_f64() * 1e3,
        recording.size()
    );
    std::thread::sleep(Duration::from_secs(seconds));
    let stats = recording.stop()?;
    let wall = start.elapsed();
    let cpu = cpu_time() - cpu_before;

    let size = std::fs::metadata(&output)?.len();
    #[allow(clippy::cast_precision_loss)]
    let mbit = size as f64 * 8.0 / 1e6 / stats.duration.as_secs_f64();
    println!("codec: {:?}", stats.codec);
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
    let (duration, frames) = probe(&output)?;
    println!("file: {duration:.2} s, {frames} video frames");
    Ok(())
}
