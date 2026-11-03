//! Converts a video to GIF and prints the time taken.
//!
//! ```text
//! cargo run --release -p kadra-record --example to_gif -- <in.mp4> <out.gif> [fps] [max-width]
//! ```

#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::path::PathBuf;
use std::time::Instant;

use kadra_record::{GifOptions, video_to_gif};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let input = PathBuf::from(
        args.get(1)
            .ok_or("usage: to_gif <in> <out> [fps] [width]")?,
    );
    let output = PathBuf::from(args.get(2).ok_or("missing output")?);
    let fps = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(15);
    let max_width = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let start = Instant::now();
    let mut last = 0;
    let summary = &video_to_gif(
        &input,
        &output,
        fps,
        GifOptions {
            max_width,
            ..GifOptions::default()
        },
        &kadra_record::VideoEdit::whole(),
        |done| {
            let percent = (done * 100.0) as u32;
            if percent / 25 != last / 25 {
                println!("{percent} %");
            }
            last = percent;
            true
        },
    )?
    .summary;
    println!(
        "{}x{}, {} frames, {} KB in {:.2} s",
        summary.width,
        summary.height,
        summary.frames,
        summary.bytes / 1000,
        start.elapsed().as_secs_f64()
    );
    Ok(())
}
