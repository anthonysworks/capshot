//! Screenshot workflow: overlay, clipboard and saving to disk. The overlay
//! also selects the area to record and shows the recording indicator.

use std::time::{Duration, Instant};

use kadra_core::{BgraImage, Rect};
use kadra_overlay::{CaptureRequest, CaptureResult, Mode, Overlay, Purpose};
use tauri::{AppHandle, Emitter, Manager};

use crate::recording;
use crate::state::AppState;
use crate::tray;

/// Owns the overlay thread.
#[derive(Debug)]
pub struct CaptureService {
    overlay: Overlay,
}

/// Sent to the web UI after a screenshot has been written to disk.
pub const SAVED_EVENT: &str = "capture-saved";

impl CaptureService {
    /// Starts the overlay thread (device creation happens here, not on the
    /// first hotkey press).
    pub fn start(app: &AppHandle) -> Result<Self, String> {
        let app = app.clone();
        let overlay = Overlay::spawn(move |result| on_result(&app, result))?;
        Ok(Self { overlay })
    }

    pub fn capture(&self, mode: Mode, options: Options) {
        self.overlay.capture(CaptureRequest {
            purpose: Purpose::Screenshot,
            mode,
            copy_to_clipboard: options.copy_to_clipboard,
            show_preview: options.show_preview,
            requested_at: Instant::now(),
        });
    }

    /// Opens the overlay to pick the area to record.
    pub fn select_recording_area(&self) {
        self.overlay.capture(CaptureRequest {
            purpose: Purpose::Recording,
            mode: Mode::Region,
            copy_to_clipboard: false,
            show_preview: false,
            requested_at: Instant::now(),
        });
    }

    pub fn show_recording_indicator(&self, region: Rect, countdown: Duration) {
        self.overlay.show_recording_indicator(region, countdown);
    }

    pub fn recording_started(&self) {
        self.overlay.recording_started();
    }

    pub fn recording_paused(&self, paused: bool) {
        self.overlay.recording_paused(paused);
    }

    pub fn show_file_preview(&self, image: BgraImage, path: std::path::PathBuf, region: Rect) {
        self.overlay.show_file_preview(image, path, region);
    }

    pub fn hide_recording_indicator(&self) {
        self.overlay.hide_recording_indicator();
    }
}

/// Capture settings read when the capture is requested.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    copy_to_clipboard: bool,
    show_preview: bool,
}

/// Opens the overlay, after the configured delay.
pub fn start(app: &AppHandle) {
    let Some(service) = app.try_state::<CaptureService>() else {
        tracing::error!("capture is unavailable (overlay failed to start)");
        return;
    };
    let (delay, options) = {
        let state = app.state::<AppState>();
        let config = state.config();
        (
            config.capture.delay_secs,
            Options {
                copy_to_clipboard: config.capture.copy_to_clipboard,
                show_preview: config.capture.show_preview,
            },
        )
    };
    if delay == 0 {
        service.capture(Mode::Region, options);
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(u64::from(delay)));
        if let Some(service) = app.try_state::<CaptureService>() {
            service.capture(Mode::Region, options);
        }
    });
}

/// Runs on the overlay thread: keep it short.
fn on_result(app: &AppHandle, result: CaptureResult) {
    match result {
        CaptureResult::Captured { id, image, copied } => {
            tracing::info!(
                width = image.width(),
                height = image.height(),
                copied,
                "screenshot taken"
            );
            let app = app.clone();
            std::thread::spawn(move || save(&app, id, &image));
        }
        CaptureResult::RegionSelected(region) => recording::start(app, region),
        CaptureResult::StopRecording => recording::stop(app),
        CaptureResult::TogglePause => recording::toggle_pause(app),
        CaptureResult::ColorCopied(hex) => tracing::info!(%hex, "colour copied"),
        CaptureResult::Cancelled => tracing::debug!("capture cancelled"),
        CaptureResult::Failed(err) => tracing::error!("capture failed: {err}"),
    }
}

fn save(app: &AppHandle, id: u64, image: &BgraImage) {
    let (enabled, format, dir) = {
        let state = app.state::<AppState>();
        let config = state.config();
        (
            config.capture.save_to_disk,
            config.capture.format,
            config.capture.output_dir.clone(),
        )
    };
    if !enabled {
        return;
    }
    let start = Instant::now();
    match kadra_encode::save_now(image, format, &dir) {
        Ok(path) => {
            tracing::info!(
                path = %path.display(),
                elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
                "screenshot saved"
            );
            if let Some(service) = app.try_state::<CaptureService>() {
                service.overlay.attach_file(id, path.clone());
            }
            remember(app, &path, image.width(), image.height());
            let _ = app.emit(SAVED_EVENT, path.display().to_string());
        }
        Err(err) => tracing::error!("cannot save the screenshot: {err}"),
    }
}

/// Adds a saved capture (image or video) to the history and the tray menu.
pub fn remember(app: &AppHandle, path: &std::path::Path, width: u32, height: u32) {
    let state = app.state::<AppState>();
    let taken_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    {
        let mut history = state.history();
        history.push(kadra_core::history::Entry {
            path: path.to_path_buf(),
            taken_at,
            width,
            height,
        });
        if let Err(err) = history.save(&state.paths().history_file()) {
            tracing::warn!("cannot save the capture history: {err}");
        }
    }
    tray::refresh(app);
}
