use std::sync::{Mutex, MutexGuard, PoisonError};

use kadra_core::{Config, Paths};

/// State shared by commands, hotkeys and the tray.
#[derive(Debug)]
pub struct AppState {
    paths: Paths,
    config: Mutex<Config>,
}

impl AppState {
    pub fn new(paths: Paths, config: Config) -> Self {
        Self {
            paths,
            config: Mutex::new(config),
        }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Locks the configuration. A poisoned lock is recovered: the config is
    /// plain data and stays consistent even if a holder panicked.
    pub fn config(&self) -> MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
