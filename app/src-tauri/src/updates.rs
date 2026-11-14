//! Updates: Capshot looks for a new release in the background, downloads it
//! quietly and offers to install it from the tray menu. Nothing is installed
//! while the user is busy: installing restarts the app.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

/// First check, a little after startup so launching stays instant.
const FIRST_CHECK: Duration = Duration::from_secs(45);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// A downloaded update, waiting for the user to restart.
struct Ready {
    update: Update,
    bytes: Vec<u8>,
}

#[derive(Default)]
pub struct Updates {
    ready: Mutex<Option<Ready>>,
    /// Serializes checks: the timer and the settings button may overlap.
    busy: AtomicBool,
}

impl Updates {
    /// Version of the downloaded update, if any.
    pub fn pending_version(&self) -> Option<String> {
        self.ready
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|r| r.update.version.clone())
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    UpToDate,
    Ready { version: String },
    Failed { message: String },
}

/// Looks for an update and downloads it. Safe to call at any time.
pub async fn check(app: &AppHandle) -> Status {
    let updates = app.state::<Updates>();
    if let Some(version) = updates.pending_version() {
        return Status::Ready { version };
    }
    if updates.busy.swap(true, Ordering::AcqRel) {
        return Status::Failed {
            message: "A check is already running.".into(),
        };
    }
    let outcome = fetch(app).await;
    updates.busy.store(false, Ordering::Release);
    match outcome {
        Ok(Some(version)) => {
            crate::tray::refresh(app);
            Status::Ready { version }
        }
        Ok(None) => Status::UpToDate,
        Err(err) => {
            tracing::warn!("update check failed: {err}");
            Status::Failed {
                message: err.to_string(),
            }
        }
    }
}

async fn fetch(app: &AppHandle) -> tauri_plugin_updater::Result<Option<String>> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(None);
    };
    tracing::info!(version = %update.version, "downloading the update");
    let bytes = update.download(|_, _| {}, || {}).await?;
    let version = update.version.clone();
    *app.state::<Updates>()
        .ready
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Ready { update, bytes });
    Ok(Some(version))
}

/// Installs the downloaded update; the installer restarts Capshot.
pub fn install(app: &AppHandle) -> Result<(), String> {
    if app.state::<crate::recording::RecorderState>().is_active() {
        return Err("Finish the recording first.".into());
    }
    let ready = app
        .state::<Updates>()
        .ready
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .ok_or("No update downloaded.")?;
    ready.update.install(ready.bytes).map_err(|e| e.to_string())
}

/// Background checks, unless the user turned them off.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK);
        loop {
            if app.state::<AppState>().config().auto_update {
                tauri::async_runtime::block_on(check(&app));
            }
            std::thread::sleep(CHECK_EVERY);
        }
    });
}
