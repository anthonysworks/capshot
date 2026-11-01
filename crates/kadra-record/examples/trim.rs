//! Trims a video and prints the time taken.
//!
//! ```text
//! cargo run --release -p kadra-record --example trim -- <in.mp4> <out.mp4> <start-s> <end-s> [mute]
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kadra_record::{TrimOptions, trim_video};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: trim <in> <out> <start> <end> [mute]";
    let input = PathBuf::from(args.get(1).ok_or(usage)?);
    let output = PathBuf::from(args.get(2).ok_or(usage)?);
    let seconds = |i: usize| -> Result<Duration, Box<dyn std::error::Error>> {
        Ok(Duration::from_secs_f64(args.get(i).ok_or(usage)?.parse()?))
    };
    let options = TrimOptions {
        start: seconds(3)?,
        end: Some(seconds(4)?),
        audio: args.get(5).is_none_or(|a| a != "mute"),
        quality: 18,
    };
    let started = Instant::now();
    let stats = trim_video(&input, &output, &options, |_| true)?;
    println!(
        "{}x{}, {} frames, {:.2} s of video in {:.2} s, {} KB",
        stats.width,
        stats.height,
        stats.frames,
        stats.duration.as_secs_f64(),
        started.elapsed().as_secs_f64(),
        std::fs::metadata(&output)?.len() / 1000
    );
    Ok(())
}
