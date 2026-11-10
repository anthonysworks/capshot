//! Captures the desktop a few times and prints timings.
//!
//! ```text
//! cargo run --release -p capshot-capture --example snapshot
//! ```

use std::time::Instant;

use capshot_capture::{Capturer, Gpu, GpuKind};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: called once at start-up, before any window exists.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
        RoInitialize(RO_INIT_MULTITHREADED)?;
    }

    let start = Instant::now();
    // `snapshot warp` uses the software device.
    let kind = if std::env::args().any(|a| a == "warp") {
        GpuKind::Software
    } else {
        GpuKind::Hardware
    };
    let capturer = Capturer::new(Gpu::with_kind(kind)?);
    println!("init: {:.1} ms", start.elapsed().as_secs_f64() * 1000.0);

    for i in 0..5 {
        let start = Instant::now();
        let snap = capturer.snapshot(None)?;
        println!(
            "#{i}: {:.1} ms  {}x{}  {:?}  {} monitor(s)  {} window(s)",
            start.elapsed().as_secs_f64() * 1000.0,
            snap.image.width(),
            snap.image.height(),
            snap.backend,
            snap.monitors.len(),
            snap.windows.len(),
        );
        // What the overlay does once it is on screen.
        let start = Instant::now();
        capturer.prepare();
        println!(
            "    re-armed in {:.1} ms",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
    Ok(())
}
