//! Web-based windows, created on demand.
//!
//! No webview exists while Kadra sits in the tray: windows are built when
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
            .title("Kadra Settings")
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
