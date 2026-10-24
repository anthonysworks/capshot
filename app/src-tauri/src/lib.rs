//! Kadra desktop application: tray icon, global hotkeys and the windows built
//! with Tauri. Capture and encoding live in the `kadra-*` crates.

mod actions;
mod commands;
mod hotkeys;
mod state;
mod tray;
mod windows;

use kadra_core::{Config, Paths, logging};
use tauri::{Manager, RunEvent};

use crate::state::AppState;

/// Entry point called from `main`.
pub fn run() {
    let Some(paths) = Paths::resolve() else {
        // Without a user profile there is nowhere to store settings; this
        // only happens in broken service/sandbox environments.
        eprintln!("kadra: cannot locate the user profile directories");
        std::process::exit(1);
    };

    let _log_guard = logging::init(paths.log_dir());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting Kadra");

    let config = Config::load_or_init(&paths.config_file()).unwrap_or_else(|err| {
        tracing::error!("{err}; using default settings for this session");
        Config::default()
    });

    let app = tauri::Builder::default()
        // Must be registered first so a second launch exits before doing any work.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            windows::show_settings(app);
        }))
        .plugin(hotkeys::plugin())
        .manage(AppState::new(paths, config))
        .manage(hotkeys::Registered::default())
        .setup(|app| {
            tray::create(app.handle())?;
            let state = app.state::<AppState>();
            let hotkeys = state.config().hotkeys.clone();
            if let Err(err) = hotkeys::apply(app.handle(), &hotkeys) {
                tracing::warn!("{err}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::app_info,
            commands::open_folder,
        ])
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(err) => {
            tracing::error!("failed to start: {err}");
            std::process::exit(1);
        }
    };

    app.run(|_app, event| {
        // Kadra lives in the tray: closing the last window must not quit.
        // An explicit `exit` (tray "Quit") carries an exit code and goes through.
        if let RunEvent::ExitRequested { api, code, .. } = event
            && code.is_none()
        {
            api.prevent_exit();
        }
    });
}
