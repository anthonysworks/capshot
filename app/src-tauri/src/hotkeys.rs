//! Global keyboard shortcuts.

use std::sync::Mutex;

use anyhow::{Context, Result};
use capshot_core::config::Hotkeys;
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

use crate::actions::{self, Action};
use crate::keyhook;

/// Shortcuts currently registered, with the action each one triggers.
#[derive(Debug, Default)]
pub struct Registered(Mutex<Vec<(Shortcut, Action)>>);

pub fn plugin() -> TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            let action = app.try_state::<Registered>().and_then(|registered| {
                let registered = registered.0.lock().ok()?;
                registered
                    .iter()
                    .find(|(s, _)| s == shortcut)
                    .map(|(_, action)| action.clone())
            });
            if let Some(action) = action {
                actions::trigger(app, action);
            }
        })
        .build()
}

/// Parses an accelerator such as `Ctrl+Shift+S` or `PrintScreen`.
pub fn parse(accelerator: &str) -> Result<Shortcut> {
    accelerator
        .parse::<Shortcut>()
        .map_err(|err| anyhow::anyhow!("invalid shortcut \"{accelerator}\": {err}"))
}

/// Replaces every registered shortcut with the ones from `hotkeys`.
///
/// Both shortcuts are parsed before anything is unregistered, so an invalid
/// accelerator leaves the current bindings untouched.
pub fn apply(app: &AppHandle, hotkeys: &Hotkeys) -> Result<()> {
    let wanted = [
        (parse(&hotkeys.capture)?, Action::Capture),
        (parse(&hotkeys.record)?, Action::Record),
    ];

    // Like ShareX, switch off the Windows option that opens the Snipping Tool
    // on Print Screen as soon as Capshot uses the key, so users need not.
    let bare_print_screen = wanted
        .iter()
        .any(|(s, _)| s.key == Code::PrintScreen && s.mods.is_empty());
    if bare_print_screen && crate::system::print_screen_taken() {
        match crate::system::release_print_screen() {
            Ok(()) => tracing::info!("Windows' own Print Screen capture switched off"),
            Err(err) => tracing::warn!("cannot switch off Windows' Print Screen capture: {err}"),
        }
    }

    // Print Screen goes through the keyboard hook: Windows may still hold it.
    let (print_screen, wanted): (Vec<_>, Vec<_>) = wanted
        .into_iter()
        .partition(|(shortcut, _)| shortcut.key == Code::PrintScreen);
    let hooked = keyhook::set(
        app,
        print_screen
            .iter()
            .map(|(shortcut, action)| (shortcut.mods, action.clone()))
            .collect(),
    );
    // Without the hook they are registered like any other shortcut.
    let wanted: Vec<_> = if hooked {
        wanted
    } else {
        wanted.into_iter().chain(print_screen).collect()
    };

    let manager = app.global_shortcut();
    manager
        .unregister_all()
        .context("cannot release previous shortcuts")?;

    let registered = app.state::<Registered>();
    let mut registered = registered
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    registered.clear();

    let mut failures = Vec::new();
    for (shortcut, action) in wanted {
        match manager.register(shortcut) {
            Ok(()) => registered.push((shortcut, action)),
            Err(err) => failures.push(format!("{shortcut}: {err}")),
        }
    }

    if failures.is_empty() {
        tracing::info!(capture = %hotkeys.capture, record = %hotkeys.record, "hotkeys registered");
        Ok(())
    } else {
        anyhow::bail!(
            "some shortcuts are already in use by another application ({})",
            failures.join(", ")
        )
    }
}
