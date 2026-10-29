//! Worst-case encoding speed: a desktop image that scrolls by a few pixels
//! every frame, so every frame changes almost everywhere.
//!
//! ```text
//! cargo run --release -p kadra-gif --example throughput -- <desktop.png> <out.gif> [frames]
//! ```

#![allow(clippy::cast_precision_loss)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kadra_core::BgraImage;
use kadra_gif::{GifOptions, GifWriter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let input = PathBuf::from(
        args.get(1)
            .ok_or("usage: throughput <in.png> <out.gif> [frames]")?,
    );
    let output = PathBuf::from(args.get(2).ok_or("missing output")?);
    let frames: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);

    let rgba = image::open(&input)?.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let desktop = BgraImage::from_raw(w, h, bgra);

    let options = GifOptions {
        max_width: std::env::var("KADRA_GIF_WIDTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        ..GifOptions::default()
    };
    let start = Instant::now();
    let mut gif = GifWriter::create(&output, w, h, options)?;
    let stride = w as usize * 4;
    for i in 0..frames {
        // Scroll up by 3 px per frame.
        let shift = (i as usize * 3) % h as usize;
        let src = desktop.as_bytes();
        let mut data = Vec::with_capacity(src.len());
        data.extend_from_slice(&src[shift * stride..]);
        data.extend_from_slice(&src[..shift * stride]);
        gif.push(
            &BgraImage::from_raw(w, h, data),
            Duration::from_secs(u64::from(i)) / 15,
        )?;
    }
    let summary = gif.finish(Duration::from_secs(u64::from(frames)) / 15)?;
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "{}x{}, {} frames in {elapsed:.2} s = {:.1} frames/s, {:.1} MB",
        summary.width,
        summary.height,
        summary.frames,
        f64::from(frames) / elapsed,
        summary.bytes as f64 / 1e6
    );
    Ok(())
}
