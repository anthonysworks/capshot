//! Logging setup: daily rotating files plus stderr in debug builds.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Number of daily log files kept on disk.
const MAX_LOG_FILES: usize = 7;

/// Keeps the background log writer alive. Logs written after this guard is
/// dropped may be lost, so hold it for the whole lifetime of the process.
#[must_use = "dropping the guard stops the log writer"]
#[derive(Debug)]
pub struct LogGuard {
    _file: Option<WorkerGuard>,
}

/// Installs the global tracing subscriber.
///
/// The filter can be overridden with the `KADRA_LOG` environment variable
/// (same syntax as `RUST_LOG`). If the log directory cannot be created, file
/// logging is skipped and only stderr is used.
pub fn init(log_dir: &Path) -> LogGuard {
    let default_level = if cfg!(debug_assertions) {
        "debug"
    } else {
        "info"
    };
    let filter = EnvFilter::try_from_env("KADRA_LOG")
        .unwrap_or_else(|_| EnvFilter::new(format!("{default_level},tao=warn,wry=warn")));

    let appender = std::fs::create_dir_all(log_dir).ok().and_then(|()| {
        Builder::new()
            .rotation(Rotation::DAILY)
            .filename_prefix("kadra")
            .filename_suffix("log")
            .max_log_files(MAX_LOG_FILES)
            .build(log_dir)
            .ok()
    });

    let (file_layer, guard) = match appender {
        Some(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = fmt::layer().with_ansi(false).with_writer(writer);
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    let stderr_layer = cfg!(debug_assertions).then(|| fmt::layer().with_writer(std::io::stderr));

    // `try_init` fails only if a subscriber is already installed, which is
    // harmless (tests, embedding).
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();

    LogGuard { _file: guard }
}
