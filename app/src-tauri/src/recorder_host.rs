//! Out-of-process recorder.
//!
//! Recording runs in a child process (`capshot.exe --recorder-host`) driven
//! through JSON lines on stdin/stdout:
//!
//! - when the recording ends, Windows reclaims everything the GPU driver
//!   and the hardware encoder allocated (~80 MB that would otherwise stay
//!   in the main process);
//! - a driver crash cannot take the tray application down;
//! - if the main process exits, stdin closes and the file is finalised.

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use capshot_core::{BgraImage, GifSettings, Rect, VideoCodec};
use capshot_record::{GifOptions, RecordOptions, Recording};
use serde::{Deserialize, Serialize};

/// Command-line flag selecting host mode.
pub const FLAG: &str = "--recorder-host";
/// `CREATE_NO_WINDOW`: no console flashes on screen.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Opening the GPU device and the encoder takes well under a second.
const START_TIMEOUT: Duration = Duration::from_secs(15);
/// Finalising a long file can take a moment.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);

/// Options sent by the main process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct HostOptions {
    /// `[left, top, right, bottom]`, desktop coordinates.
    pub region: [i32; 4],
    pub fps: u32,
    pub codec: VideoCodec,
    pub quality: u8,
    pub capture_cursor: bool,
    pub system_audio: bool,
    pub microphone: bool,
    pub output: PathBuf,
    pub start_paused: bool,
    /// Record a GIF with these settings instead of a video.
    pub gif: Option<GifSettings>,
    /// Editing material: the video without cursor and the cursor track.
    #[serde(default)]
    pub clean_output: Option<PathBuf>,
    #[serde(default)]
    pub cursor_track: Option<PathBuf>,
}

/// Events sent back by the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ready {
        region: [i32; 4],
    },
    Finished {
        width: u32,
        height: u32,
        frames: u64,
        dropped: u64,
        duration_ms: u64,
        /// Raw thumbnail file (see [`write_thumbnail`]).
        thumbnail: Option<PathBuf>,
    },
    Error {
        message: String,
    },
}

fn rect(r: [i32; 4]) -> Rect {
    Rect::new(r[0], r[1], r[2], r[3])
}

fn array(r: Rect) -> [i32; 4] {
    [r.left, r.top, r.right, r.bottom]
}

/// Thumbnail exchange format: width and height (u32 little endian), then
/// BGRA pixels.
pub fn write_thumbnail(image: &BgraImage) -> Option<PathBuf> {
    let path = std::env::temp_dir().join(format!("capshot-thumb-{}.bgra", std::process::id()));
    let mut bytes = Vec::with_capacity(8 + image.as_bytes().len());
    bytes.extend(image.width().to_le_bytes());
    bytes.extend(image.height().to_le_bytes());
    bytes.extend(image.as_bytes());
    std::fs::write(&path, bytes).ok().map(|()| path)
}

/// Reads and deletes a thumbnail written by the host.
pub fn take_thumbnail(path: &Path) -> Option<BgraImage> {
    let bytes = std::fs::read(path).ok();
    let _ = std::fs::remove_file(path);
    let bytes = bytes?;
    let width = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?);
    let height = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
    let pixels = bytes.get(8..)?;
    (pixels.len() == width as usize * height as usize * 4)
        .then(|| BgraImage::from_raw(width, height, pixels.to_vec()))
}

fn send(event: &Event) {
    if let Ok(line) = serde_json::to_string(event) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// Entry point of the host process. Returns the process exit code.
pub fn run() -> i32 {
    let mut lines = std::io::stdin().lock().lines();
    let options = lines
        .next()
        .and_then(Result::ok)
        .and_then(|line| serde_json::from_str::<HostOptions>(&line).ok());
    let Some(options) = options else {
        send(&Event::Error {
            message: "invalid recorder options".into(),
        });
        return 2;
    };

    let recording = match Recording::start(RecordOptions {
        region: rect(options.region),
        fps: options.fps,
        codec: options.codec,
        quality: options.quality,
        capture_cursor: options.capture_cursor,
        system_audio: options.system_audio,
        microphone: options.microphone,
        output: options.output.clone(),
        start_paused: options.start_paused,
        gif: options.gif.as_ref().map(|gif| GifOptions {
            max_width: gif.max_width,
            quality: gif.quality,
            dither: gif.dither,
            repeat: gif.repeat,
            tolerance: 0,
        }),
        clean_output: options.clean_output.clone(),
        cursor_track: options.cursor_track.clone(),
    }) {
        Ok(recording) => recording,
        Err(err) => {
            send(&Event::Error {
                message: err.to_string(),
            });
            return 1;
        }
    };
    send(&Event::Ready {
        region: array(recording.region()),
    });

    // Commands until "stop" or until the main process goes away.
    for line in lines {
        match line.as_deref().map(str::trim) {
            Ok("pause") => recording.pause(),
            Ok("resume") => recording.resume(),
            Ok("stop") | Err(_) => break,
            Ok(other) => tracing::warn!("unknown recorder command {other:?}"),
        }
    }

    match recording.stop() {
        Ok(stats) => {
            #[allow(clippy::cast_possible_truncation)]
            let duration_ms = stats.duration.as_millis() as u64;
            send(&Event::Finished {
                width: stats.width,
                height: stats.height,
                frames: stats.frames,
                dropped: stats.dropped,
                duration_ms,
                thumbnail: stats.thumbnail.as_ref().and_then(write_thumbnail),
            });
            0
        }
        Err(err) => {
            send(&Event::Error {
                message: err.to_string(),
            });
            1
        }
    }
}

/// Main-process side: a running recorder process.
#[derive(Debug)]
pub struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    events: mpsc::Receiver<Event>,
    region: Rect,
}

impl Host {
    /// Starts a recorder process and waits until it is recording (or ready,
    /// when started paused).
    pub fn start(options: &HostOptions) -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = Command::new(exe)
            .arg(FLAG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("cannot start the recorder: {e}"))?;

        let mut stdin = child.stdin.take().ok_or("no recorder input")?;
        let stdout = child.stdout.take().ok_or("no recorder output")?;
        let line = serde_json::to_string(options).map_err(|e| e.to_string())?;
        writeln!(stdin, "{line}").map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;

        let (tx, events) = mpsc::channel();
        std::thread::Builder::new()
            .name("capshot-recorder-events".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    match serde_json::from_str::<Event>(&line) {
                        Ok(event) => {
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                        Err(err) => tracing::warn!("bad recorder event {line:?}: {err}"),
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        let fail = |mut child: Child, message: String| {
            let _ = child.kill();
            let _ = child.wait();
            Err(message)
        };
        match events.recv_timeout(START_TIMEOUT) {
            Ok(Event::Ready { region }) => Ok(Self {
                child,
                stdin: Some(stdin),
                events,
                region: rect(region),
            }),
            Ok(Event::Error { message }) => fail(child, message),
            Ok(other) => fail(child, format!("unexpected recorder event {other:?}")),
            Err(_) => fail(child, "the recorder did not start".into()),
        }
    }

    /// Area actually recorded (desktop coordinates).
    pub fn region(&self) -> Rect {
        self.region
    }

    fn command(&mut self, command: &str) {
        if let Some(stdin) = self.stdin.as_mut()
            && (writeln!(stdin, "{command}").is_err() || stdin.flush().is_err())
        {
            tracing::warn!("the recorder process is gone");
        }
    }

    pub fn pause(&mut self) {
        self.command("pause");
    }

    pub fn resume(&mut self) {
        self.command("resume");
    }

    /// Stops and waits for the file to be finalised.
    pub fn stop(mut self) -> Result<Event, String> {
        self.command("stop");
        // Closing stdin also stops a recorder that missed the command.
        self.stdin = None;
        let result = match self.events.recv_timeout(STOP_TIMEOUT) {
            Ok(event @ Event::Finished { .. }) => Ok(event),
            Ok(Event::Error { message }) => Err(message),
            Ok(other) => Err(format!("unexpected recorder event {other:?}")),
            Err(_) => Err("the recorder did not finish in time".into()),
        };
        if result.is_err() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_as_tagged_json() {
        let event = Event::Ready {
            region: [1, 2, 3, 4],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, r#"{"event":"ready","region":[1,2,3,4]}"#);
        assert!(matches!(
            serde_json::from_str::<Event>(&json).unwrap(),
            Event::Ready {
                region: [1, 2, 3, 4]
            }
        ));
    }

    #[test]
    fn thumbnails_survive_the_trip() {
        let image = BgraImage::from_raw(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let path = write_thumbnail(&image).unwrap();
        assert_eq!(take_thumbnail(&path), Some(image));
        assert!(!path.exists());
    }
}
