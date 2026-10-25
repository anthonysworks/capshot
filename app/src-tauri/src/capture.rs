//! Screenshot workflow: overlay, clipboard and saving to disk.

use std::time::{Duration, Instant};

use kadra_core::BgraImage;
use kadra_overlay::{CaptureRequest, CaptureResult, Mode, Overlay};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

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

    pub fn capture(&self, mode: Mode, copy_to_clipboard: bool) {
        self.overlay.capture(CaptureRequest {
            mode,
            copy_to_clipboard,
            requested_at: Instant::now(),
        });
    }
}

/// Opens the overlay, after the configured delay.
pub fn start(app: &AppHandle) {
    let Some(service) = app.try_state::<CaptureService>() else {
        tracing::error!("capture is unavailable (overlay failed to start)");
        return;
    };
    let (delay, copy) = {
        let state = app.state::<AppState>();
        let config = state.config();
        (config.capture.delay_secs, config.capture.copy_to_clipboard)
    };
    if delay == 0 {
        service.capture(Mode::Region, copy);
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(u64::from(delay)));
        if let Some(service) = app.try_state::<CaptureService>() {
            service.capture(Mode::Region, copy);
        }
    });
}

/// Runs on the overlay thread: keep it short.
fn on_result(app: &AppHandle, result: CaptureResult) {
    match result {
        CaptureResult::Captured { image, copied } => {
            tracing::info!(
                width = image.width(),
                height = image.height(),
                copied,
                "screenshot taken"
            );
            let app = app.clone();
            std::thread::spawn(move || save(&app, &image));
        }
        CaptureResult::ColorCopied(hex) => tracing::info!(%hex, "colour copied"),
        CaptureResult::Cancelled => tracing::debug!("capture cancelled"),
        CaptureResult::Failed(err) => tracing::error!("capture failed: {err}"),
    }
}

fn save(app: &AppHandle, image: &BgraImage) {
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
            let _ = app.emit(SAVED_EVENT, path.display().to_string());
        }
        Err(err) => tracing::error!("cannot save the screenshot: {err}"),
    }
}
