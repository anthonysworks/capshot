//! Commands invoked from the web UI.
//!
//! Errors are returned as plain strings: they are shown to the user as-is.

// Tauri injects `State` and `AppHandle` by value.
#![allow(clippy::needless_pass_by_value)]

use kadra_core::Config;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::hotkeys;
use crate::state::AppState;
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
