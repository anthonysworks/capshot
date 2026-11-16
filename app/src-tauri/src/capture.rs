//! Screenshot workflow: overlay, clipboard and saving to disk. The overlay
//! also selects the area to record and shows the recording indicator.

use std::time::{Duration, Instant};

use capshot_core::{AfterCapture, BgraImage, Rect};
use capshot_overlay::{Action, CaptureRequest, CaptureResult, Mode, Overlay, Purpose};
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

    /// Opens the overlay; the toolbar can still switch between taking a
    /// screenshot and recording.
    pub fn open(&self, purpose: Purpose, options: Options) {
        self.overlay.capture(CaptureRequest {
            purpose,
            mode: Mode::Region,
            copy_to_clipboard: options.copy_to_clipboard,
            show_preview: options.after == AfterCapture::Preview,
            actions: options.after == AfterCapture::Actions,
            format: options.format,
            fps: options.fps,
            gif_fps: options.gif_fps,
            countdown_secs: options.countdown_secs,
            system_audio: options.system_audio,
            microphone: options.microphone,
            light: options.light,
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

    pub fn show_file_preview(
        &self,
        image: BgraImage,
        path: std::path::PathBuf,
        region: Rect,
        label: Option<String>,
    ) {
        self.overlay.show_file_preview(image, path, region, label);
    }

    pub fn copy_files(&self, files: Vec<std::path::PathBuf>) {
        self.overlay.copy_files(files);
    }

    pub fn hide_recording_indicator(&self) {
        self.overlay.hide_recording_indicator();
    }

    pub fn copy_image(&self, image: BgraImage) {
        self.overlay.copy_image(image);
    }

    pub fn copy_text(&self, text: String) {
        self.overlay.copy_text(text);
    }
}

/// Settings read when the overlay is requested.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Options {
    copy_to_clipboard: bool,
    after: AfterCapture,
    format: capshot_core::RecordingFormat,
    fps: u32,
    gif_fps: u32,
    countdown_secs: u32,
    system_audio: bool,
    microphone: bool,
    light: bool,
}

/// Opens the overlay for `purpose` (after the configured delay for a
/// screenshot; recordings have their own countdown).
pub fn start(app: &AppHandle, purpose: Purpose) {
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
                after: config.capture.after_capture,
                format: config.recording.format,
                fps: config.recording.fps,
                gif_fps: config.gif.fps,
                countdown_secs: config.recording.countdown_secs,
                system_audio: config.recording.system_audio,
                microphone: config.recording.microphone,
                light: match config.appearance.theme {
                    capshot_core::Theme::Light => true,
                    capshot_core::Theme::Dark => false,
                    capshot_core::Theme::System => capshot_overlay::system_uses_light_theme(),
                },
            },
        )
    };
    if delay == 0 || purpose == Purpose::Recording {
        service.open(purpose, options);
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(u64::from(delay)));
        if let Some(service) = app.try_state::<CaptureService>() {
            service.open(purpose, options);
        }
    });
}

/// Runs on the overlay thread: keep it short.
fn on_result(app: &AppHandle, result: CaptureResult) {
    match result {
        CaptureResult::Captured {
            id,
            image,
            copied,
            action,
            area,
        } => {
            tracing::info!(
                width = image.width(),
                height = image.height(),
                copied,
                ?action,
                "screenshot taken"
            );
            let app = app.clone();
            match action {
                Some(action) => std::thread::spawn(move || run_action(&app, action, &image, area)),
                None => std::thread::spawn(move || save(&app, id, &image)),
            };
        }
        CaptureResult::RegionSelected {
            region,
            format,
            fps,
            countdown_secs,
            system_audio,
            microphone,
        } => recording::start(
            app,
            region,
            recording::Choices {
                format,
                fps,
                countdown_secs,
                system_audio,
                microphone,
            },
        ),
        CaptureResult::OpenFile(path) => crate::editor::open_capture(app, &path),
        CaptureResult::StopRecording => recording::stop(app),
        CaptureResult::DiscardRecording => recording::discard(app),
        CaptureResult::TogglePause => recording::toggle_pause(app),
        CaptureResult::ColorCopied(hex) => tracing::info!(%hex, "colour copied"),
        CaptureResult::Cancelled => tracing::info!("capture cancelled"),
        CaptureResult::Failed(err) => tracing::error!("capture failed: {err}"),
    }
}

/// Runs what was picked on the bar under the selection. A thumbnail in the
/// corner of the screen confirms it (clicking it opens the editor).
// One arm per action of the bar.
#[allow(clippy::too_many_lines)]
fn run_action(app: &AppHandle, action: Action, image: &BgraImage, area: Rect) {
    let (format, dir, keep) = {
        let state = app.state::<AppState>();
        let config = state.config();
        (
            config.capture.format,
            config.capture.output_dir.clone(),
            config.capture.save_to_disk,
        )
    };
    let service = app.try_state::<CaptureService>();
    let confirm = |path: &std::path::Path, label: &str| {
        if let Some(service) = &service {
            service.show_file_preview(
                image.clone(),
                path.to_path_buf(),
                area,
                Some(label.to_owned()),
            );
        }
    };
    let temporary = || {
        capshot_encode::save_now(image, format, &crate::quick::pending_dir())
            .map_err(|e| e.to_string())
    };
    let result: Result<(), String> = (|| {
        match action {
            Action::Copy => confirm(&temporary()?, "Copied to the clipboard"),
            Action::Save => {
                let path =
                    capshot_encode::save_now(image, format, &dir).map_err(|e| e.to_string())?;
                remember(app, &path, image.width(), image.height());
                let _ = app.emit(SAVED_EVENT, path.display().to_string());
                confirm(&path, "Saved");
            }
            Action::SaveAs => {
                let temp = temporary()?;
                let chosen = crate::quick::choose(app, &temp);
                let _ = std::fs::remove_file(&temp);
                if let Some(to) = chosen {
                    let format = capshot_encode::format_of(&to).unwrap_or(format);
                    let bytes = capshot_encode::encode(image, format).map_err(|e| e.to_string())?;
                    std::fs::write(&to, bytes).map_err(|e| e.to_string())?;
                    remember(app, &to, image.width(), image.height());
                    confirm(&to, "Saved");
                }
            }
            Action::Edit => {
                // The editor saves over the file it opens.
                let dir = if keep {
                    dir
                } else {
                    std::env::temp_dir().join("Capshot")
                };
                let path =
                    capshot_encode::save_now(image, format, &dir).map_err(|e| e.to_string())?;
                if keep {
                    remember(app, &path, image.width(), image.height());
                    let _ = app.emit(SAVED_EVENT, path.display().to_string());
                }
                crate::editor::open(app, &path);
            }
            Action::Pin => {
                let temp = temporary()?;
                #[allow(clippy::cast_lossless)]
                let scale = app
                    .monitor_from_point(f64::from(area.left), f64::from(area.top))
                    .ok()
                    .flatten()
                    .map_or(1.0, |m| m.scale_factor());
                let pinned = crate::quick::pin(app, &temp, scale);
                let _ = std::fs::remove_file(&temp);
                pinned?;
            }
            Action::Text => {
                let text = crate::quick::text_in(image.clone())?;
                let path = temporary()?;
                if text.trim().is_empty() {
                    confirm(&path, "No text found");
                } else {
                    if let Some(service) = &service {
                        service.copy_text(text);
                    }
                    confirm(&path, "Text copied");
                }
            }
            Action::Scan => {
                let codes = crate::quick::codes_in(image);
                let path = temporary()?;
                if codes.is_empty() {
                    confirm(&path, "No QR code found");
                } else {
                    if let Some(service) = &service {
                        service.copy_text(codes.join("\n"));
                    }
                    confirm(&path, "QR code copied");
                }
            }
            Action::Close => {}
        }
        Ok(())
    })();
    if let Err(err) = result {
        tracing::error!("screenshot action {action:?} failed: {err}");
    }
}

fn save(app: &AppHandle, id: u64, image: &BgraImage) {
    let (enabled, format, dir, after) = {
        let state = app.state::<AppState>();
        let config = state.config();
        (
            config.capture.save_to_disk,
            config.capture.format,
            config.capture.output_dir.clone(),
            config.capture.after_capture,
        )
    };
    let edit = after == AfterCapture::Editor;
    // The quick actions save it only when asked: it waits in a temporary
    // folder. The editor works on a file: without saving, use a temporary one.
    let quick = after == AfterCapture::Actions;
    let enabled = enabled && !quick;
    let dir = match (enabled, edit || quick) {
        (true, _) => dir,
        (false, true) if quick => crate::quick::pending_dir(),
        (false, true) => std::env::temp_dir().join("Capshot"),
        (false, false) => return,
    };
    let start = Instant::now();
    match capshot_encode::save_now(image, format, &dir) {
        Ok(path) => {
            tracing::info!(
                path = %path.display(),
                elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
                "screenshot saved"
            );
            if let Some(service) = app.try_state::<CaptureService>() {
                service.overlay.attach_file(id, path.clone());
            }
            if enabled {
                remember(app, &path, image.width(), image.height());
                let _ = app.emit(SAVED_EVENT, path.display().to_string());
            }
            if edit {
                crate::editor::open(app, &path);
            } else if quick {
                crate::quick::open(app, &path);
            }
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
        history.push(capshot_core::history::Entry {
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
