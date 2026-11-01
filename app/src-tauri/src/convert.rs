//! Video jobs: conversion to GIF and trimming.
//!
//! Like recording, they run in a child process (`kadra.exe --gif-host`):
//! decoders and encoders allocate a lot, and all of it is returned to the
//! system when the process ends. The child reads one JSON request on stdin
//! and reports progress as JSON lines on stdout.

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use kadra_core::{GifSettings, Rect};
use kadra_record::{GifOptions, TrimOptions, trim_video, video_to_gif};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::capture::CaptureService;
use crate::recorder_host::{take_thumbnail, write_thumbnail};
use crate::state::AppState;

/// Command-line flag selecting the job host mode.
pub const FLAG: &str = "--gif-host";
/// Sent to the web UI while a job runs, see [`Progress`].
pub const PROGRESS_EVENT: &str = "video-job";
/// `CREATE_NO_WINDOW`: no console flashes on screen.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Video files Kadra can read.
pub const VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mov", "m4v", "wmv", "avi", "mkv"];
/// Quality of edited videos (0 best – 51 smallest): edits should not lose
/// visible detail.
const EDIT_QUALITY: u8 = 18;

/// What to do with the input video. Times are in seconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Task {
    Gif {
        gif: GifSettings,
        range: Option<(f64, f64)>,
    },
    Mp4 {
        start: f64,
        end: Option<f64>,
        audio: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    task: Task,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    Progress {
        done: f32,
    },
    Finished {
        width: u32,
        height: u32,
        thumbnail: Option<PathBuf>,
    },
    Error {
        message: String,
    },
}

/// Job state reported to the web UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub job: u64,
    pub input: String,
    /// 0–1.
    pub done: f32,
    /// Set once finished.
    pub output: Option<String>,
    pub bytes: Option<u64>,
    pub error: Option<String>,
}

fn send(event: &Event) {
    if let Ok(line) = serde_json::to_string(event) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

fn seconds(s: f64) -> Duration {
    Duration::try_from_secs_f64(s.max(0.0)).unwrap_or(Duration::ZERO)
}

/// Entry point of the host process. Returns the process exit code.
pub fn run_host() -> i32 {
    let mut line = String::new();
    let request = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .ok()
        // Shells may prefix piped text with a byte order mark.
        .and_then(|_| serde_json::from_str::<Request>(line.trim_start_matches('\u{feff}')).ok());
    let Some(request) = request else {
        send(&Event::Error {
            message: "invalid job request".into(),
        });
        return 2;
    };
    tracing::info!(input = %request.input.display(), task = ?request.task, "video job");
    let mut last = -1.0f32;
    let mut progress = |done: f32| {
        // About a hundred updates at most.
        if done - last >= 0.01 || done >= 1.0 {
            last = done;
            send(&Event::Progress { done });
        }
        true
    };
    let result = match &request.task {
        Task::Gif { gif, range } => video_to_gif(
            &request.input,
            &request.output,
            gif.fps,
            GifOptions {
                max_width: gif.max_width,
                quality: gif.quality,
                dither: gif.dither,
                repeat: gif.repeat,
                tolerance: 0,
            },
            range.map(|(a, b)| (seconds(a), seconds(b))),
            &mut progress,
        )
        .map(|c| (c.summary.width, c.summary.height, c.thumbnail)),
        Task::Mp4 { start, end, audio } => trim_video(
            &request.input,
            &request.output,
            &TrimOptions {
                start: seconds(*start),
                end: end.map(seconds),
                audio: *audio,
                quality: EDIT_QUALITY,
            },
            &mut progress,
        )
        .map(|stats| (stats.width, stats.height, None)),
    };
    match result {
        Ok((width, height, thumbnail)) => {
            send(&Event::Finished {
                width,
                height,
                thumbnail: thumbnail.as_ref().and_then(write_thumbnail),
            });
            0
        }
        Err(err) => {
            let _ = std::fs::remove_file(&request.output);
            send(&Event::Error {
                message: err.to_string(),
            });
            1
        }
    }
}

/// Whether `path` looks like a video Kadra can read.
pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

/// Output written next to the input, never overwriting a file.
fn output_for(input: &Path, suffix: &str, extension: &str) -> PathBuf {
    let dir = input.parent().unwrap_or(Path::new("."));
    let stem = input
        .file_stem()
        .map_or_else(|| "video".into(), |s| s.to_string_lossy().into_owned());
    kadra_encode::unique_path(dir, &format!("{stem}{suffix}"), extension)
}

/// Converts a whole video to GIF with the GIF settings.
pub fn to_gif(app: &AppHandle, input: PathBuf) -> u64 {
    let gif = app.state::<AppState>().config().gif.clone();
    let output = output_for(&input, "", "gif");
    start(app, input, output, Task::Gif { gif, range: None })
}

/// Exports part of a video edited in the editor.
pub fn export(app: &AppHandle, input: PathBuf, task: Task) -> u64 {
    let extension = match task {
        Task::Gif { .. } => "gif",
        Task::Mp4 { .. } => "mp4",
    };
    let output = output_for(&input, " (edited)", extension);
    start(app, input, output, task)
}

/// Runs a job in the background; returns its id. Progress is reported
/// with [`PROGRESS_EVENT`]; the result is added to the history and
/// previewed.
fn start(app: &AppHandle, input: PathBuf, output: PathBuf, task: Task) -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let job = NEXT.fetch_add(1, Ordering::Relaxed);
    let app = app.clone();
    std::thread::spawn(move || {
        let label = input.display().to_string();
        let report = |done: f32, output: Option<String>, bytes, error| {
            let _ = app.emit(
                PROGRESS_EVENT,
                Progress {
                    job,
                    input: label.clone(),
                    done,
                    output,
                    bytes,
                    error,
                },
            );
        };
        if !input.is_file() || !is_video(&input) {
            report(0.0, None, None, Some("not a video file".into()));
            return;
        }
        let request = Request {
            input: input.clone(),
            output,
            task,
        };
        match run_child(&request, |done| report(done, None, None, None)) {
            Ok(Event::Finished {
                width,
                height,
                thumbnail,
            }) => {
                let bytes = std::fs::metadata(&request.output).map_or(0, |m| m.len());
                tracing::info!(path = %request.output.display(), bytes, "video job done");
                crate::capture::remember(&app, &request.output, width, height);
                report(
                    1.0,
                    Some(request.output.display().to_string()),
                    Some(bytes),
                    None,
                );
                let image = thumbnail.as_deref().and_then(take_thumbnail);
                if let (Some(image), Some(service)) = (image, app.try_state::<CaptureService>()) {
                    // Primary monitor.
                    service.show_file_preview(image, request.output.clone(), Rect::new(0, 0, 1, 1));
                }
            }
            Ok(_) => report(0.0, None, None, Some("unexpected reply".into())),
            Err(err) => {
                tracing::error!("video job on {label} failed: {err}");
                report(0.0, None, None, Some(err));
            }
        }
    });
    job
}

/// Runs the job process and waits for its final event.
fn run_child(request: &Request, mut progress: impl FnMut(f32)) -> Result<Event, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .arg(FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("cannot start the converter: {e}"))?;
    let line = serde_json::to_string(request).map_err(|e| e.to_string())?;
    {
        let mut stdin = child.stdin.take().ok_or("no converter input")?;
        writeln!(stdin, "{line}").map_err(|e| e.to_string())?;
    }
    let stdout = child.stdout.take().ok_or("no converter output")?;
    let mut last = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        match serde_json::from_str::<Event>(&line) {
            Ok(Event::Progress { done }) => progress(done),
            Ok(event) => last = Some(event),
            Err(err) => tracing::warn!("bad converter event {line:?}: {err}"),
        }
    }
    let _ = child.wait();
    match last {
        Some(Event::Error { message }) => Err(message),
        Some(event) => Ok(event),
        None => Err("the converter stopped unexpectedly".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_videos() {
        assert!(is_video(Path::new(r"C:\v\clip.MP4")));
        assert!(is_video(Path::new("a.mkv")));
        assert!(!is_video(Path::new("a.gif")));
        assert!(!is_video(Path::new("mp4")));
    }

    #[test]
    fn tasks_are_tagged() {
        let json = serde_json::to_string(&Task::Mp4 {
            start: 1.5,
            end: None,
            audio: false,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"kind":"mp4","start":1.5,"end":null,"audio":false}"#
        );
        let json = serde_json::to_string(&Event::Progress { done: 0.5 }).unwrap();
        assert_eq!(json, r#"{"event":"progress","done":0.5}"#);
    }
}
