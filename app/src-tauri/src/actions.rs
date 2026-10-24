//! User-facing actions, triggered from hotkeys or the tray menu.

use tauri::AppHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Capture,
    Record,
}

pub fn trigger(_app: &AppHandle, action: Action) {
    // The capture overlay and the recording pipeline are implemented in the
    // upcoming `kadra-capture` / `kadra-overlay` crates (milestones M1/M2).
    tracing::info!(?action, "action requested");
}
