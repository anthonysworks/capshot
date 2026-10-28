//! Kadra desktop application: tray icon, global hotkeys and the windows built
//! with Tauri. Capture and encoding live in the `kadra-*` crates.

mod actions;
mod capture;
mod commands;
mod hotkeys;
mod recording;
mod state;
mod tray;
mod windows;

use kadra_core::{Config, Paths, logging};
use tauri::{Manager, RunEvent};

use crate::actions::Action;
use crate::capture::CaptureService;
use crate::state::AppState;

/// Entry point called from `main`.
pub fn run() {
    let Some(paths) = Paths::resolve() else {
        // Without a user profile there is nowhere to store settings; this
        // only happens in broken service/sandbox environments.
        eprintln!("kadra: cannot locate the user profile directories");
        std::process::exit(1);
    };

    // Tauri ends the process with `exit`, which skips destructors: the guard
    // is dropped explicitly on `RunEvent::Exit` so buffered lines are written.
    let mut log_guard = Some(logging::init(paths.log_dir()));
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting Kadra");

    let config = Config::load_or_init(&paths.config_file()).unwrap_or_else(|err| {
        tracing::error!("{err}; using default settings for this session");
        Config::default()
    });

    // A second launch such as `kadra.exe --capture` was started by the user
    // and may take the foreground; hand that right to the running instance
    // so its overlay gets keyboard focus.
    let args: Vec<String> = std::env::args().collect();
    if Action::from_args(&args).is_some() {
        // SAFETY: plain call, no pointers involved.
        let _ = unsafe {
            ::windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                ::windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
            )
        };
    }

    let app = tauri::Builder::default()
        // Must be registered first so a second launch exits before doing any work.
        // `kadra.exe --capture` triggers a capture in the running instance,
        // any other launch opens the settings.
        .plugin(tauri_plugin_single_instance::init(
            |app, args, _cwd| match Action::from_args(&args) {
                Some(action) => actions::trigger(app, action),
                None => windows::show_settings(app),
            },
        ))
        .plugin(hotkeys::plugin())
        .manage(AppState::new(paths, config))
        .manage(hotkeys::Registered::default())
        .manage(recording::RecorderState::default())
        .setup(move |app| {
            tray::create(app.handle())?;
            let state = app.state::<AppState>();
            let hotkeys = state.config().hotkeys.clone();
            if let Err(err) = hotkeys::apply(app.handle(), &hotkeys) {
                tracing::warn!("{err}");
            }
            match CaptureService::start(app.handle()) {
                Ok(service) => {
                    app.manage(service);
                }
                Err(err) => tracing::error!("cannot start the capture overlay: {err}"),
            }
            if let Some(action) = Action::from_args(&args) {
                actions::trigger(app.handle(), action);
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

    app.run(move |_app, event| match event {
        // Kadra lives in the tray: closing the last window must not quit.
        // An explicit `exit` (tray "Quit") carries an exit code and goes through.
        RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
        RunEvent::Exit => {
            tracing::info!("exiting");
            drop(log_guard.take());
        }
        _ => {}
    });
}
