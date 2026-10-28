//! Screen recording workflow: select an area, record, stop.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use kadra_core::Rect;
use kadra_record::{RecordOptions, Recording};
use tauri::{AppHandle, Manager};

use crate::capture::CaptureService;
use crate::state::AppState;
use crate::tray;

#[derive(Debug)]
struct Active {
    recording: Recording,
    path: PathBuf,
}

/// The recording in progress, if any.
#[derive(Debug, Default)]
pub struct RecorderState {
    active: Mutex<Option<Active>>,
    /// Set between the area selection and the start of the recording.
    starting: Mutex<bool>,
}

impl RecorderState {
    fn active(&self) -> MutexGuard<'_, Option<Active>> {
        self.active.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn starting(&self) -> MutexGuard<'_, bool> {
        self.starting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn is_recording(&self) -> bool {
        self.active().is_some()
    }
}

/// Starts selecting an area to record, or stops the current recording.
pub fn toggle(app: &AppHandle) {
    let state = app.state::<RecorderState>();
    if state.is_recording() {
        stop(app);
    } else if !*state.starting()
        && let Some(service) = app.try_state::<CaptureService>()
    {
        service.select_recording_area();
    }
}

/// Called once the user has picked the area (desktop coordinates).
pub fn start(app: &AppHandle, region: Rect) {
    tracing::info!(?region, "recording area selected");
    let state = app.state::<RecorderState>();
    {
        let mut starting = state.starting();
        if *starting || state.is_recording() {
            return;
        }
        *starting = true;
    }

    let options = {
        let app_state = app.state::<AppState>();
        let config = app_state.config();
        let dir = config.recording.output_dir.clone();
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::error!("cannot create {}: {err}", dir.display());
        }
        let stem = kadra_encode::file_stem(kadra_encode::now());
        RecordOptions {
            region,
            fps: config.recording.fps,
            codec: config.recording.codec,
            quality: config.recording.quality,
            capture_cursor: config.recording.capture_cursor,
            system_audio: config.recording.system_audio,
            microphone: config.recording.microphone,
            output: kadra_encode::unique_path(&dir, &stem, "mp4"),
            start_paused: false,
        }
    };

    // Creating the GPU device and the encoder takes a few hundred
    // milliseconds: never on the overlay thread.
    let app = app.clone();
    std::thread::spawn(move || {
        let path = options.output.clone();
        tracing::debug!(path = %path.display(), "starting the recorder");
        let result = Recording::start(options);
        let state = app.state::<RecorderState>();
        match result {
            Ok(recording) => {
                if let Some(service) = app.try_state::<CaptureService>() {
                    service.show_recording_indicator(recording.region());
                }
                *state.active() = Some(Active { recording, path });
            }
            Err(err) => tracing::error!("cannot start recording: {err}"),
        }
        *state.starting() = false;
        tray::refresh(&app);
    });
}

/// Stops the recording and adds the video to the history.
pub fn stop(app: &AppHandle) {
    let Some(active) = app.state::<RecorderState>().active().take() else {
        return;
    };
    if let Some(service) = app.try_state::<CaptureService>() {
        service.hide_recording_indicator();
    }
    tray::refresh(app);

    let app = app.clone();
    std::thread::spawn(move || match active.recording.stop() {
        Ok(stats) => {
            tracing::info!(
                path = %active.path.display(),
                frames = stats.frames,
                dropped = stats.dropped,
                "recording saved"
            );
            crate::capture::remember(&app, &active.path, stats.width, stats.height);
        }
        Err(err) => tracing::error!("recording failed: {err}"),
    });
}
