//! Commands invoked from the web UI.
//!
//! Errors are returned as plain strings: they are shown to the user as-is.

// Tauri injects `State` and `AppHandle` by value.
#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;

use kadra_core::Config;
use serde::{Deserialize, Serialize};
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, Manager, State};

use crate::capture::CaptureService;
use crate::convert::Task;
use crate::editor::EditorFiles;

use crate::hotkeys;
use crate::state::AppState;
use crate::tray;
use crate::windows;

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Config {
    state.config().clone()
}

/// Validates, applies and persists a new configuration.
///
/// Hotkeys are applied first: if one cannot be registered, the previous
/// bindings are restored and nothing is saved.
#[tauri::command]
pub fn save_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: Config,
) -> Result<(), String> {
    config.validate().map_err(|e| e.to_string())?;

    let previous = state.config().clone();
    if previous.hotkeys != config.hotkeys
        && let Err(err) = hotkeys::apply(&app, &config.hotkeys)
    {
        if let Err(restore) = hotkeys::apply(&app, &previous.hotkeys) {
            tracing::warn!("cannot restore previous hotkeys: {restore}");
        }
        return Err(err.to_string());
    }

    config
        .save(&state.paths().config_file())
        .map_err(|e| e.to_string())?;
    *state.config() = config;
    tray::refresh(&app);
    tracing::info!("settings saved");
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    config_file: String,
    log_dir: String,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        config_file: state.paths().config_file().display().to_string(),
        log_dir: state.paths().log_dir().display().to_string(),
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Folder {
    Screenshots,
    Recordings,
    Logs,
}

/// Opens one of the application folders. Only known folders can be opened,
/// never an arbitrary path coming from the webview.
#[tauri::command]
pub fn open_folder(state: State<'_, AppState>, folder: Folder) {
    let dir = match folder {
        Folder::Screenshots => state.config().capture.output_dir.clone(),
        Folder::Recordings => state.config().recording.output_dir.clone(),
        Folder::Logs => state.paths().log_dir().to_path_buf(),
    };
    windows::open_in_explorer(&dir);
}

/// Converts a video file to GIF in the background; returns the job id whose
/// progress is reported with [`crate::convert::PROGRESS_EVENT`].
#[tauri::command]
pub fn convert_to_gif(app: AppHandle, path: String) -> u64 {
    crate::convert::to_gif(&app, PathBuf::from(path))
}

/// A path the web UI may access: only files opened in an editor.
fn editable(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if app.state::<EditorFiles>().is_allowed(&path) {
        Ok(path)
    } else {
        tracing::warn!("refusing access to {}", path.display());
        Err("this file was not opened in the editor".into())
    }
}

/// Path sent in the `x-path` header of a raw request (percent-encoded).
fn header_path(app: &AppHandle, request: &Request<'_>) -> Result<PathBuf, String> {
    let header = request
        .headers()
        .get("x-path")
        .and_then(|v| v.to_str().ok())
        .and_then(crate::editor::decode_component)
        .ok_or("missing path")?;
    editable(app, &header)
}

fn raw_body<'a>(request: &'a Request<'_>) -> Result<&'a [u8], String> {
    match request.body() {
        InvokeBody::Raw(bytes) => Ok(bytes),
        InvokeBody::Json(_) => Err("expected raw image bytes".into()),
    }
}

/// Reads a file opened in the editor, as raw bytes.
#[tauri::command]
pub fn editor_read(app: AppHandle, path: String) -> Result<Response, String> {
    let path = editable(&app, &path)?;
    std::fs::read(&path)
        .map(Response::new)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Saves an edited image over the original, in the original's format. The
/// body is the image encoded as PNG.
#[tauri::command]
pub fn editor_save(app: AppHandle, request: Request<'_>) -> Result<(), String> {
    let path = header_path(&app, &request)?;
    let image = kadra_encode::decode(raw_body(&request)?).map_err(|e| e.to_string())?;
    let format = kadra_encode::format_of(&path).ok_or("unsupported image format")?;
    let bytes = kadra_encode::encode(&image, format).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("kadra-tmp");
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("cannot save {}: {e}", path.display()))?;
    tracing::info!(path = %path.display(), "edited image saved");
    Ok(())
}

/// Copies an edited image (PNG body) to the clipboard.
#[tauri::command]
pub fn editor_copy(app: AppHandle, request: Request<'_>) -> Result<(), String> {
    let image = kadra_encode::decode(raw_body(&request)?).map_err(|e| e.to_string())?;
    let service = app
        .try_state::<CaptureService>()
        .ok_or("the clipboard is unavailable")?;
    service.copy_image(image);
    Ok(())
}

/// Exports part of a video opened in the editor; returns the job id.
#[tauri::command]
pub fn editor_export(app: AppHandle, path: String, task: Task) -> Result<u64, String> {
    let path = editable(&app, &path)?;
    Ok(crate::convert::export(&app, path, task))
}

/// Selects a file in Explorer. Only files Kadra created (listed in the
/// history) can be shown, never an arbitrary path coming from the webview.
#[tauri::command]
pub fn show_in_folder(state: State<'_, AppState>, path: String) {
    let path = std::path::PathBuf::from(path);
    let known = state.history().recent().any(|entry| entry.path == path);
    if !known {
        tracing::warn!("refusing to show {}", path.display());
        return;
    }
    let mut select = std::ffi::OsString::from("/select,");
    select.push(path.as_os_str());
    if let Err(err) = std::process::Command::new("explorer").arg(select).spawn() {
        tracing::warn!("cannot show {}: {err}", path.display());
    }
}
