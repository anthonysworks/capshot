//! Notification area icon and its menu.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::actions::{self, Action};
use crate::state::AppState;
use crate::windows;

const CAPTURE: &str = "capture";
const RECORD: &str = "record";
const SCREENSHOTS: &str = "screenshots";
const SETTINGS: &str = "settings";
const QUIT: &str = "quit";

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let hotkeys = app.state::<AppState>().config().hotkeys.clone();

    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, CAPTURE, "Capture", true, Some(&hotkeys.capture))?,
            &MenuItem::with_id(app, RECORD, "Record video", true, Some(&hotkeys.record))?,
            &PredefinedMenuItem::separator(app)?,
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
    )?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Kadra")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            CAPTURE => actions::trigger(app, Action::Capture),
            RECORD => actions::trigger(app, Action::Record),
            SCREENSHOTS => {
                let dir = app.state::<AppState>().config().capture.output_dir.clone();
                windows::open_in_explorer(&dir);
            }
            SETTINGS => windows::show_settings(app),
            QUIT => app.exit(0),
            _ => {}
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
