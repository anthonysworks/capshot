//! Screen recording workflow: select an area, count down, record (with
//! pause/resume), stop, preview.
//!
//! The recording itself runs in a separate process (see
//! [`crate::recorder_host`]).

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use kadra_core::Rect;
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
    } else if let Some(service) = service(app) {
        service.select_recording_area();
    }
}

/// Called once the user has picked the area (desktop coordinates).
pub fn start(app: &AppHandle, region: Rect) {
    tracing::info!(?region, "recording area selected");
    {
        let state = app.state::<RecorderState>();
        let mut phase = state.phase();
        if !matches!(*phase, Phase::Idle) {
            return;
        }
        *phase = Phase::Starting { cancelled: false };
    }

    let (options, countdown) = {
        let app_state = app.state::<AppState>();
        let config = app_state.config();
        let dir = config.recording.output_dir.clone();
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::error!("cannot create {}: {err}", dir.display());
        }
        let stem = kadra_encode::file_stem(kadra_encode::now());
        let countdown = Duration::from_secs(u64::from(config.recording.countdown_secs));
        let options = HostOptions {
            region: [region.left, region.top, region.right, region.bottom],
            fps: config.recording.fps,
            codec: config.recording.codec,
            quality: config.recording.quality,
            capture_cursor: config.recording.capture_cursor,
            system_audio: config.recording.system_audio,
            microphone: config.recording.microphone,
            output: kadra_encode::unique_path(&dir, &stem, "mp4"),
            // Everything is prepared during the countdown, so recording
            // starts exactly when it reaches zero.
            start_paused: !countdown.is_zero(),
        };
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
        std::thread::sleep(Duration::from_millis(20));
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
    if options.start_paused {
        host.resume();
    }
    if let Some(service) = service(app) {
        service.recording_started();
    }
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
            if frames == 0 {
                let _ = std::fs::remove_file(&path);
                return;
            }
            #[allow(clippy::cast_precision_loss)]
            let seconds = duration_ms as f64 / 1000.0;
            tracing::info!(path = %path.display(), frames, dropped, seconds, "recording saved");
            crate::capture::remember(&app, &path, width, height);
            let preview = app.state::<AppState>().config().capture.show_preview;
            if preview && let (Some(image), Some(service)) = (image, service(&app)) {
                service.show_file_preview(image, path, region);
            }
        }
        Ok(other) => tracing::error!("unexpected recorder event {other:?}"),
        Err(err) => tracing::error!("recording failed: {err}"),
    });
}
