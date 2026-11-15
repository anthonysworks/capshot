//! Trims a video and prints the time taken.
//!
//! ```text
//! cargo run --release -p capshot-record --example trim -- <in.mp4> <out.mp4> <start-s> <end-s> [mute|sound] [speed] [fps] [height] [music-file]
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use capshot_record::{TrimOptions, trim_video};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: trim <in> <out> <start> <end> [mute]";
    let input = PathBuf::from(args.get(1).ok_or(usage)?);
    let output = PathBuf::from(args.get(2).ok_or(usage)?);
    let seconds = |i: usize| -> Result<Duration, Box<dyn std::error::Error>> {
        Ok(Duration::from_secs_f64(args.get(i).ok_or(usage)?.parse()?))
    };
    let options = TrimOptions {
        segments: vec![capshot_record::Part {
            start: seconds(3)?,
            end: seconds(4)?,
            speed: args.get(6).and_then(|s| s.parse().ok()).unwrap_or(1.0),
        }],
        audio: args.get(5).is_none_or(|a| a != "mute"),
        quality: 18,
        crop: None,
        composition: capshot_record::compose::Composition::default(),
        track: None,
        fps: args.get(7).and_then(|s| s.parse().ok()),
        max_height: args.get(8).and_then(|s| s.parse().ok()),
        match_source: true,
        sound: capshot_record::SoundEdit::default(),
        music: args.get(9).map(|path| capshot_record::Music {
            path: PathBuf::from(path),
            start: 0.5,
            from: 0.0,
            length: None,
            volume: 1.0,
            fade_in: 0.5,
            fade_out: 0.5,
        }),
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
