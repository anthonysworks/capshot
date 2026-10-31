//! Video to GIF conversion.
//!
//! Like recording, conversion runs in a child process
//! (`kadra.exe --gif-host`): decoders and encoders allocate a lot, and all
//! of it is returned to the system when the process ends. The child reads
//! one JSON request on stdin and reports progress as JSON lines on stdout.

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use kadra_core::{GifSettings, Rect};
use kadra_record::{GifOptions, video_to_gif};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::capture::CaptureService;
use crate::recorder_host::{take_thumbnail, write_thumbnail};
use crate::state::AppState;

/// Command-line flag selecting converter host mode.
pub const FLAG: &str = "--gif-host";
/// Sent to the web UI while converting, see [`Progress`].
pub const PROGRESS_EVENT: &str = "gif-progress";
/// `CREATE_NO_WINDOW`: no console flashes on screen.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Video files offered for conversion.
pub const VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mov", "m4v", "wmv", "avi", "mkv"];

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    gif: GifSettings,
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
        frames: u64,
        bytes: u64,
        thumbnail: Option<PathBuf>,
    },
    Error {
        message: String,
    },
}

/// Conversion state reported to the web UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
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

/// Entry point of the host process. Returns the process exit code.
pub fn run_host() -> i32 {
    let mut line = String::new();
    let request = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .ok()
        .and_then(|_| serde_json::from_str::<Request>(&line).ok());
    let Some(request) = request else {
        send(&Event::Error {
            message: "invalid conversion request".into(),
        });
        return 2;
    };
    tracing::info!(input = %request.input.display(), "converting to GIF");
    let gif = &request.gif;
    let mut last = -1.0f32;
    let result = video_to_gif(
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
        |done| {
            // About a hundred updates at most.
            if done - last >= 0.01 {
                last = done;
                send(&Event::Progress { done });
            }
            true
        },
    );
    match result {
        Ok(conversion) => {
            let s = conversion.summary;
            tracing::info!(frames = s.frames, bytes = s.bytes, "GIF written");
            send(&Event::Finished {
                width: s.width,
                height: s.height,
                frames: s.frames,
                bytes: s.bytes,
                thumbnail: conversion.thumbnail.as_ref().and_then(write_thumbnail),
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

/// Whether `path` looks like a video Kadra can convert.
pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

/// GIF written next to the video, never overwriting a file.
fn output_for(input: &Path) -> PathBuf {
    let dir = input.parent().unwrap_or(Path::new("."));
    let stem = input
        .file_stem()
        .map_or_else(|| "video".into(), |s| s.to_string_lossy().into_owned());
    kadra_encode::unique_path(dir, &stem, "gif")
}

/// Converts `input` in the background, then adds the GIF to the history and
/// shows its preview.
pub fn to_gif(app: &AppHandle, input: PathBuf) {
    let app = app.clone();
    std::thread::spawn(move || {
        let label = input.display().to_string();
        let report = |progress: Progress| {
            let _ = app.emit(PROGRESS_EVENT, progress);
        };
        let failed = |error: String| {
            tracing::error!("GIF conversion of {label} failed: {error}");
            report(Progress {
                input: label.clone(),
                done: 0.0,
                output: None,
                bytes: None,
                error: Some(error),
            });
        };
        if !input.is_file() || !is_video(&input) {
            return failed("not a video file".into());
        }
        let request = Request {
            output: output_for(&input),
            gif: app.state::<AppState>().config().gif.clone(),
            input: input.clone(),
        };
        let result = convert_in_child(&request, |done| {
            report(Progress {
                input: label.clone(),
                done,
                output: None,
                bytes: None,
                error: None,
            });
        });
        match result {
            Ok(Event::Finished {
                width,
                height,
                bytes,
                thumbnail,
                ..
            }) => {
                tracing::info!(path = %request.output.display(), bytes, "GIF saved");
                crate::capture::remember(&app, &request.output, width, height);
                report(Progress {
                    input: label.clone(),
                    done: 1.0,
                    output: Some(request.output.display().to_string()),
                    bytes: Some(bytes),
                    error: None,
                });
                let image = thumbnail.as_deref().and_then(take_thumbnail);
                if let (Some(image), Some(service)) = (image, app.try_state::<CaptureService>()) {
                    // Primary monitor.
                    service.show_file_preview(image, request.output.clone(), Rect::new(0, 0, 1, 1));
                }
            }
            Ok(_) => failed("unexpected converter reply".into()),
            Err(err) => failed(err),
        }
    });
}

/// Runs the converter process and waits for its final event.
fn convert_in_child(request: &Request, mut progress: impl FnMut(f32)) -> Result<Event, String> {
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
    fn events_are_tagged() {
        let json = serde_json::to_string(&Event::Progress { done: 0.5 }).unwrap();
        assert_eq!(json, r#"{"event":"progress","done":0.5}"#);
    }
}
