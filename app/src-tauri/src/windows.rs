//! Web-based windows, created on demand.
//!
//! No webview exists while Capshot sits in the tray: windows are built when
//! opened and destroyed when closed, which keeps idle memory low.

use std::path::Path;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const SETTINGS_LABEL: &str = "settings";

pub fn show_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let result =
        WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::App("index.html".into()))
            .title("Capshot Settings")
            .inner_size(680.0, 620.0)
            .min_inner_size(520.0, 420.0)
            .center()
            .build();

    if let Err(err) = result {
        tracing::error!("cannot open the settings window: {err}");
    }
}

/// Opens a folder in File Explorer, creating it first if needed.
pub fn open_in_explorer(dir: &Path) {
    if let Err(err) = std::fs::create_dir_all(dir) {
        tracing::warn!("cannot create {}: {err}", dir.display());
        return;
    }
    if let Err(err) = std::process::Command::new("explorer").arg(dir).spawn() {
        tracing::warn!("cannot open {}: {err}", dir.display());
    }
}

/// Opens the folder of `path` in File Explorer with the file selected.
pub fn select_in_explorer(path: &Path) {
    use std::os::windows::process::CommandExt;
    // Explorer parses its own command line: `/select,` must be followed by
    // the quoted path. Passed as a normal argument, the whole
    // `/select,C:\a b\c.mp4` gets quoted, which Explorer ignores (it opens
    // Documents instead). Paths cannot contain quotes on Windows.
    let line = format!("/select,\"{}\"", path.display());
    if let Err(err) = std::process::Command::new("explorer").raw_arg(line).spawn() {
        tracing::warn!("cannot show {}: {err}", path.display());
    }
}

/// Opens a file with its default application.
pub fn open_file(path: &Path) {
    if !path.exists() {
        tracing::warn!("{} no longer exists", path.display());
        return;
    }
    // Explorer hands the file to its registered application.
    if let Err(err) = std::process::Command::new("explorer").arg(path).spawn() {
        tracing::warn!("cannot open {}: {err}", path.display());
    }
}
