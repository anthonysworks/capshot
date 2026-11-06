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
use kadra_record::compose::Composition;
use kadra_record::cursor::CursorTrack;
use kadra_record::{GifOptions, Part, TrimOptions, VideoEdit, trim_video, video_to_gif};
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

/// Edits made in the video editor. Times are in seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    /// Parts kept, in order.
    pub segments: Vec<(f64, f64)>,
    /// 2.0 plays twice as fast; multiplies the speed of each part.
    #[serde(default = "one")]
    pub speed: f64,
    /// Speed of each part, in order (missing: 1).
    #[serde(default)]
    pub speeds: Vec<f64>,
    /// `[x, y, width, height]` in video pixels.
    pub crop: Option<[i32; 4]>,
    /// Overlays, cursor and click effects, aspect ratio.
    #[serde(default)]
    pub composition: Composition,
}

fn one() -> f64 {
    1.0
}

impl Edit {
    fn segments(&self) -> Vec<Part> {
        self.segments
            .iter()
            .enumerate()
            .map(|(i, &(a, b))| Part {
                start: seconds(a),
                end: seconds(b),
                speed: self.speeds.get(i).copied().unwrap_or(1.0) * self.speed,
            })
            .collect()
    }

    fn crop(&self) -> Option<Rect> {
        self.crop.map(|[x, y, w, h]| Rect::from_size(x, y, w, h))
    }
}

/// What to do with the input video.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Task {
    Gif {
        gif: GifSettings,
        /// `None`: the whole video.
        edit: Option<Edit>,
    },
    Mp4 {
        edit: Edit,
        audio: bool,
        /// 0 (best) – 51 (smallest); `None`: as good as the recording.
        quality: Option<u8>,
        /// Frame rate; `None` keeps the recording's.
        #[serde(default)]
        fps: Option<u32>,
        /// Height in pixels (scaled down); `None` keeps the size.
        #[serde(default)]
        height: Option<u32>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    task: Task,
    /// The recording without cursor and its cursor track, when kept.
    #[serde(default)]
    clean: Option<PathBuf>,
    #[serde(default)]
    track: Option<PathBuf>,
}

impl Request {
    /// The video to read and the cursor track, given what the edit needs:
    /// cursor effects read the copy without cursor. Without editing
    /// material, cursor and click effects are dropped.
    fn sources(&self, composition: &Composition) -> (PathBuf, Composition, Option<CursorTrack>) {
        let mut composition = composition.clone();
        let track = if composition.uses_track() {
            self.track.as_deref().and_then(CursorTrack::load)
        } else {
            None
        };
        let clean = self.clean.as_ref().filter(|p| p.is_file());
        match (&track, clean) {
            (Some(_), Some(clean)) if composition.cursor.is_some() => {
                (clean.clone(), composition, track)
            }
            (Some(_), _) => {
                // Clicks can still be marked over the original cursor.
                composition.cursor = None;
                (self.input.clone(), composition, track)
            }
            (None, _) => {
                composition.cursor = None;
                composition.clicks = None;
                (self.input.clone(), composition, None)
            }
        }
    }
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
        Task::Gif { gif, edit } => {
            let (input, composition, track) = request.sources(
                &edit
                    .as_ref()
                    .map(|e| e.composition.clone())
                    .unwrap_or_default(),
            );
            video_to_gif(
                &input,
                &request.output,
                gif.fps,
                GifOptions {
                    max_width: gif.max_width,
                    quality: gif.quality,
                    dither: gif.dither,
                    repeat: gif.repeat,
                    tolerance: 0,
                },
                &edit.as_ref().map_or_else(VideoEdit::whole, |e| VideoEdit {
                    segments: e.segments(),
                    crop: e.crop(),
                    composition,
                    track,
                }),
                &mut progress,
            )
            .map(|c| (c.summary.width, c.summary.height, c.thumbnail))
        }
        Task::Mp4 {
            edit,
            audio,
            quality,
            fps,
            height,
        } => {
            let (input, composition, track) = request.sources(&edit.composition);
            trim_video(
                &input,
                &request.output,
                &TrimOptions {
                    segments: edit.segments(),
                    audio: *audio,
                    quality: quality.unwrap_or(EDIT_QUALITY).min(51),
                    crop: edit.crop(),
                    composition,
                    track,
                    fps: *fps,
                    max_height: *height,
                    match_source: quality.is_none(),
                },
                &mut progress,
            )
            .map(|stats| (stats.width, stats.height, None))
        }
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
    start(app, input, output, Task::Gif { gif, edit: None }, false)
}

/// File extension of what `task` makes.
pub fn extension(task: &Task) -> &'static str {
    match task {
        Task::Gif { .. } => "gif",
        Task::Mp4 { .. } => "mp4",
    }
}

/// Exports a video edited in the editor, to `output` (picked by the user)
/// or a new file in the export folder; with `copy`, the result is put on
/// the clipboard when done.
pub fn export(
    app: &AppHandle,
    input: PathBuf,
    task: Task,
    copy: bool,
    output: Option<PathBuf>,
) -> u64 {
    let output =
        output.unwrap_or_else(|| crate::saving::export_path(app, &input, extension(&task)));
    // Holds the name, so that two exports started together never share it.
    let _ = std::fs::File::create(&output);
    start(app, input, output, task, copy)
}

/// Runs a job in the background; returns its id. Progress is reported
/// with [`PROGRESS_EVENT`]; the result is added to the history and
/// previewed.
fn start(app: &AppHandle, input: PathBuf, output: PathBuf, task: Task, copy: bool) -> u64 {
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
        let (clean, track) =
            crate::editor::existing_edit_files(app.state::<AppState>().paths(), &input).unzip();
        let request = Request {
            input: input.clone(),
            output,
            task,
            clean,
            track,
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
                crate::saving::exported(&app, &input);
                if copy && let Some(service) = app.try_state::<CaptureService>() {
                    service.copy_files(vec![request.output.clone()]);
                }
                report(
                    1.0,
                    Some(request.output.display().to_string()),
                    Some(bytes),
                    None,
                );
                let image = thumbnail.as_deref().and_then(take_thumbnail);
                if let (Some(image), Some(service)) = (image, app.try_state::<CaptureService>()) {
                    // Primary monitor.
                    service.show_file_preview(
                        image,
                        request.output.clone(),
                        Rect::new(0, 0, 1, 1),
                        None,
                    );
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
        let task: Task = serde_json::from_str(
            r#"{"kind":"mp4","edit":{"segments":[[0,1.5],[3,4]],"speed":2,"crop":[10,20,300,200]},"audio":false,"quality":null}"#,
        )
        .unwrap();
        let Task::Mp4 { edit, .. } = task else {
            panic!("expected an MP4 task");
        };
        let parts = edit.segments();
        assert_eq!(parts.len(), 2);
        assert!((parts[1].speed - 2.0).abs() < 1e-9);
        assert_eq!(edit.crop(), Some(Rect::new(10, 20, 310, 220)));
        let json = serde_json::to_string(&Event::Progress { done: 0.5 }).unwrap();
        assert_eq!(json, r#"{"event":"progress","done":0.5}"#);
    }
}
