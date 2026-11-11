//! Screen recording workflow: select an area, count down, record (with
//! pause/resume), stop, preview.
//!
//! The recording itself runs in a separate process (see
//! [`crate::recorder_host`]).

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use capshot_core::{AfterCapture, RecordingFormat, Rect};
use tauri::{AppHandle, Manager};

use crate::capture::CaptureService;
use crate::recorder_host::{Event, Host, HostOptions, take_thumbnail};
use crate::state::AppState;
use crate::tray;

#[derive(Debug, Default)]
enum Phase {
    #[default]
    Idle,
    /// Area selected; the recorder is starting or counting down.
    Starting { cancelled: bool },
    Recording {
        host: Host,
        path: PathBuf,
        paused: bool,
    },
}

/// The recording in progress, if any.
#[derive(Debug, Default)]
pub struct RecorderState {
    phase: Mutex<Phase>,
}

impl RecorderState {
    fn phase(&self) -> MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// True from the area selection until the file is closed.
    pub fn is_active(&self) -> bool {
        !matches!(*self.phase(), Phase::Idle)
    }

    pub fn is_recording(&self) -> bool {
        matches!(*self.phase(), Phase::Recording { .. })
    }

    pub fn is_paused(&self) -> bool {
        matches!(*self.phase(), Phase::Recording { paused: true, .. })
    }
}

fn service(app: &AppHandle) -> Option<tauri::State<'_, CaptureService>> {
    app.try_state::<CaptureService>()
}

/// Starts selecting an area to record, or stops the current recording.
pub fn toggle(app: &AppHandle) {
    if app.state::<RecorderState>().is_active() {
        stop(app);
    } else {
        crate::capture::start(app, capshot_overlay::Purpose::Recording);
    }
}

/// What the user chose on the toolbar, remembered for next time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choices {
    pub format: RecordingFormat,
    /// Frame rate for `format`.
    pub fps: u32,
    pub countdown_secs: u32,
    pub system_audio: bool,
    pub microphone: bool,
}

/// Called once the user has picked the area (desktop coordinates).
pub fn start(app: &AppHandle, region: Rect, choices: Choices) {
    tracing::info!(?region, "recording area selected");
    {
        let state = app.state::<RecorderState>();
        let mut phase = state.phase();
        if !matches!(*phase, Phase::Idle) {
            return;
        }
        *phase = Phase::Starting { cancelled: false };
    }

    remember(app, choices);
    let gif = choices.format == RecordingFormat::Gif;
    let (options, countdown) = {
        let app_state = app.state::<AppState>();
        let config = app_state.config();
        let dir = config.recording.output_dir.clone();
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::error!("cannot create {}: {err}", dir.display());
        }
        let stem = capshot_encode::file_stem(capshot_encode::now());
        let countdown = Duration::from_secs(u64::from(choices.countdown_secs));
        let extension = if gif { "gif" } else { "mp4" };
        // Recordings that open in the editor or the quick actions are drafts
        // until saved.
        let draft = !gif
            && matches!(
                config.capture.after_capture,
                AfterCapture::Editor | AfterCapture::Actions
            );
        let output = if gif {
            capshot_encode::unique_path(&dir, &stem, extension)
        } else {
            crate::saving::recording_path(app, &dir, &stem, draft)
        };
        let options = HostOptions {
            region: [region.left, region.top, region.right, region.bottom],
            fps: choices.fps,
            codec: config.recording.codec,
            quality: config.recording.quality,
            capture_cursor: config.recording.capture_cursor,
            // A GIF has no sound.
            system_audio: choices.system_audio && !gif,
            microphone: choices.microphone && !gif,
            output,
            // Everything is prepared first (during the countdown), then
            // recording starts once the start chime has played, so the
            // video never catches it.
            start_paused: true,
            gif: gif.then(|| config.gif.clone()),
            clean_output: None,
            cursor_track: None,
        };
        let mut options = options;
        if !gif && config.recording.editable_cursor {
            let (clean, track) = crate::editor::edit_files(app_state.paths(), &options.output);
            crate::editor::prune_edit_files(app_state.paths());
            options.clean_output = Some(clean);
            options.cursor_track = Some(track);
        }
        (options, countdown)
    };

    if let Some(service) = service(app) {
        service.show_recording_indicator(region, countdown);
    }
    tray::refresh(app);

    // Starting the recorder process and its GPU encoder takes a moment:
    // never on the overlay thread.
    let app = app.clone();
    std::thread::spawn(move || begin(&app, &options, countdown));
}

/// Saves the toggles chosen on the toolbar as the new defaults.
fn remember(app: &AppHandle, choices: Choices) {
    let app_state = app.state::<AppState>();
    let mut config = app_state.config();
    let current = Choices {
        format: config.recording.format,
        fps: match choices.format {
            RecordingFormat::Mp4 => config.recording.fps,
            RecordingFormat::Gif => config.gif.fps,
        },
        countdown_secs: config.recording.countdown_secs,
        system_audio: config.recording.system_audio,
        microphone: config.recording.microphone,
    };
    if current == choices {
        return;
    }
    config.recording.format = choices.format;
    match choices.format {
        RecordingFormat::Mp4 => config.recording.fps = choices.fps,
        RecordingFormat::Gif => config.gif.fps = choices.fps,
    }
    config.recording.countdown_secs = choices.countdown_secs;
    config.recording.system_audio = choices.system_audio;
    config.recording.microphone = choices.microphone;
    if let Err(err) = config.save(&app_state.paths().config_file()) {
        tracing::warn!("cannot save the recording choices: {err}");
    }
    // Keep an open settings window in sync.
    let _ = tauri::Emitter::emit(app, CONFIG_EVENT, &*config);
}

/// Sent to the web UI when the configuration changes outside of it.
pub const CONFIG_EVENT: &str = "config-changed";

/// Recorder start-up, countdown and hand-over to the recording phase.
fn begin(app: &AppHandle, options: &HostOptions, countdown: Duration) {
    let started = Instant::now();
    let host = Host::start(options);
    let state = app.state::<RecorderState>();

    let mut host = match host {
        Ok(host) => host,
        Err(err) => {
            tracing::error!("cannot start recording: {err}");
            *state.phase() = Phase::Idle;
            if let Some(service) = service(app) {
                service.hide_recording_indicator();
            }
            tray::refresh(app);
            return;
        }
    };

    // Wait for the end of the countdown, unless the user cancels.
    let cancelled = || matches!(*state.phase(), Phase::Starting { cancelled: true });
    while started.elapsed() < countdown && !cancelled() {
        std::thread::sleep(Duration::from_millis(10));
    }
    // The indicator turns to recording with the chime; the recorder (ready
    // and paused) resumes as soon as the chime is over.
    if !cancelled() {
        if let Some(service) = service(app) {
            service.recording_started();
        }
        crate::sounds::recording_starts();
    }

    let mut phase = state.phase();
    if matches!(*phase, Phase::Starting { cancelled: true }) {
        *phase = Phase::Idle;
        drop(phase);
        tracing::info!("recording cancelled during the countdown");
        let _ = host.stop();
        let _ = std::fs::remove_file(&options.output);
        tray::refresh(app);
        return;
    }
    host.resume();
    tracing::info!(region = ?host.region(), "recording");
    *phase = Phase::Recording {
        host,
        path: options.output.clone(),
        paused: false,
    };
    drop(phase);
    tray::refresh(app);
}

/// Pauses or resumes the current recording.
pub fn toggle_pause(app: &AppHandle) {
    let state = app.state::<RecorderState>();
    let paused = {
        let mut phase = state.phase();
        let Phase::Recording { host, paused, .. } = &mut *phase else {
            return;
        };
        if *paused {
            host.resume();
        } else {
            host.pause();
        }
        *paused = !*paused;
        *paused
    };
    if let Some(service) = service(app) {
        service.recording_paused(paused);
    }
    tray::refresh(app);
}

/// Stops (or cancels) the recording, then saves and previews the video.
pub fn stop(app: &AppHandle) {
    finish(app, true);
}

/// Stops the recording and deletes it.
pub fn discard(app: &AppHandle) {
    finish(app, false);
}

fn finish(app: &AppHandle, keep: bool) {
    let state = app.state::<RecorderState>();
    let taken = {
        let mut phase = state.phase();
        match std::mem::take(&mut *phase) {
            Phase::Starting { .. } => {
                // The start-up thread cleans up when it sees this.
                *phase = Phase::Starting { cancelled: true };
                None
            }
            Phase::Recording { host, path, .. } => Some((host, path)),
            Phase::Idle => None,
        }
    };
    if let Some(service) = service(app) {
        service.hide_recording_indicator();
    }
    let Some((host, path)) = taken else {
        return;
    };
    tray::refresh(app);

    let region = host.region();
    let app = app.clone();
    std::thread::spawn(move || match host.stop() {
        Ok(Event::Finished {
            width,
            height,
            frames,
            dropped,
            duration_ms,
            thumbnail,
        }) => {
            let image = thumbnail.as_deref().and_then(take_thumbnail);
            if frames == 0 || !keep {
                let _ = std::fs::remove_file(&path);
                let state = app.state::<AppState>();
                let (clean, track) = crate::editor::edit_files(state.paths(), &path);
                let _ = std::fs::remove_file(clean);
                let _ = std::fs::remove_file(track);
                if !keep {
                    tracing::info!("recording discarded");
                }
                return;
            }
            #[allow(clippy::cast_precision_loss)]
            let seconds = duration_ms as f64 / 1000.0;
            tracing::info!(path = %path.display(), frames, dropped, seconds, "recording saved");
            let (after, copy) = {
                let state = app.state::<AppState>();
                let config = state.config();
                (
                    config.capture.after_capture,
                    config.capture.copy_to_clipboard,
                )
            };
            let drafts = app.state::<AppState>().paths().drafts_dir();
            if path.starts_with(&drafts) {
                // Saved to its folder when exported or when the editor closes.
                app.state::<crate::saving::Saving>()
                    .add_draft(&path, (width, height));
                if after == AfterCapture::Actions {
                    crate::quick::open(&app, &path);
                } else {
                    crate::editor::open(&app, &path);
                }
                return;
            }
            crate::capture::remember(&app, &path, width, height);
            if after == AfterCapture::Editor {
                crate::editor::open(&app, &path);
                return;
            }
            let Some(service) = service(&app) else {
                return;
            };
            // Pasted as a file: into Explorer, a chat, a mail.
            if copy {
                service.copy_files(vec![path.clone()]);
            }
            if matches!(after, AfterCapture::Preview | AfterCapture::Actions)
                && let Some(image) = image
            {
                let label = copy.then(|| "Copied to the clipboard".to_owned());
                service.show_file_preview(image, path, region, label);
            }
        }
        Ok(other) => tracing::error!("unexpected recorder event {other:?}"),
        Err(err) => tracing::error!("recording failed: {err}"),
    });
}
