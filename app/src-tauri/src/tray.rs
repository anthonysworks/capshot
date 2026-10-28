//! Notification area icon and its menu.

use std::path::Path;

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::actions::{self, Action};
use crate::state::AppState;
use crate::windows;

const TRAY_ID: &str = "main";
const CAPTURE: &str = "capture";
const RECORD: &str = "record";
const PAUSE: &str = "pause";
const SCREENSHOTS: &str = "screenshots";
const SETTINGS: &str = "settings";
const QUIT: &str = "quit";
/// Prefix of the "recent capture" items, followed by their index.
const RECENT: &str = "recent:";
/// Recent captures listed in the menu.
const RECENT_IN_MENU: usize = 8;

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let state = app.state::<AppState>();
    let hotkeys = state.config().hotkeys.clone();
    let recorder = app.state::<crate::recording::RecorderState>();
    let (active, recording, paused) = (
        recorder.is_active(),
        recorder.is_recording(),
        recorder.is_paused(),
    );
    let record_label = if active {
        "Stop recording"
    } else {
        "Record video"
    };
    let pause_label = if paused {
        "Resume recording"
    } else {
        "Pause recording"
    };
    let recent: Vec<String> = state
        .history()
        .recent()
        .take(RECENT_IN_MENU)
        .map(|e| file_name(&e.path))
        .collect();

    let recent_items = recent
        .iter()
        .enumerate()
        .map(|(i, name)| MenuItem::with_id(app, format!("{RECENT}{i}"), name, true, None::<&str>))
        .collect::<tauri::Result<Vec<_>>>()?;
    let empty = MenuItem::with_id(app, "recent-empty", "No captures yet", false, None::<&str>)?;
    let recent_refs: Vec<&dyn IsMenuItem<Wry>> = if recent_items.is_empty() {
        vec![&empty]
    } else {
        recent_items
            .iter()
            .map(|i| i as &dyn IsMenuItem<Wry>)
            .collect()
    };
    let recent_menu = Submenu::with_items(app, "Recent captures", true, &recent_refs)?;

    Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, CAPTURE, "Capture", true, Some(&hotkeys.capture))?,
            &MenuItem::with_id(app, RECORD, record_label, true, Some(&hotkeys.record))?,
            &MenuItem::with_id(app, PAUSE, pause_label, recording, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &recent_menu,
            &MenuItem::with_id(
                app,
                SCREENSHOTS,
                "Open screenshots folder",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(app, SETTINGS, "Settings", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, QUIT, "Quit Kadra", true, None::<&str>)?,
        ],
    )
}

/// Rebuilds the menu, e.g. after a capture or a hotkey change.
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    match build_menu(app) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(err) => tracing::warn!("cannot rebuild the tray menu: {err}"),
    }
}

fn open_recent(app: &AppHandle, index: usize) {
    let path = app
        .state::<AppState>()
        .history()
        .recent()
        .nth(index)
        .map(|e| e.path.clone());
    if let Some(path) = path {
        windows::open_file(&path);
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Kadra")
        .menu(&build_menu(app)?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            CAPTURE => actions::trigger(app, Action::Capture),
            RECORD => actions::trigger(app, Action::Record),
            PAUSE => crate::recording::toggle_pause(app),
            SCREENSHOTS => {
                let dir = app.state::<AppState>().config().capture.output_dir.clone();
                windows::open_in_explorer(&dir);
            }
            SETTINGS => windows::show_settings(app),
            QUIT => app.exit(0),
            id => {
                if let Some(index) = id.strip_prefix(RECENT).and_then(|i| i.parse().ok()) {
                    open_recent(app, index);
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            // Left click captures straight away, like the hotkey.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                actions::trigger(tray.app_handle(), Action::Capture);
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}
