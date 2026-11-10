use std::sync::{Mutex, MutexGuard, PoisonError};

use capshot_core::{Config, History, Paths};

/// State shared by commands, hotkeys and the tray.
#[derive(Debug)]
pub struct AppState {
    paths: Paths,
    config: Mutex<Config>,
    history: Mutex<History>,
}

impl AppState {
    pub fn new(paths: Paths, config: Config) -> Self {
        let mut history = History::load(&paths.history_file());
        history.prune_missing();
        Self {
            paths,
            config: Mutex::new(config),
            history: Mutex::new(history),
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

    /// Recent captures (same poisoning rule as [`Self::config`]).
    pub fn history(&self) -> MutexGuard<'_, History> {
        self.history.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
