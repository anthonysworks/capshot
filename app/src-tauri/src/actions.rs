//! User-facing actions, triggered from hotkeys, the tray or the command line.

use tauri::AppHandle;

use crate::capture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Capture,
    Record,
}

impl Action {
    /// Command-line flag that triggers the action (`kadra.exe --capture`).
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Option<Self> {
        args.iter().find_map(|a| match a.as_ref() {
            "--capture" => Some(Self::Capture),
            "--record" => Some(Self::Record),
            _ => None,
        })
    }
}

pub fn trigger(app: &AppHandle, action: Action) {
    match action {
        Action::Capture => capture::start(app),
        // Screen recording is milestone M2.
        Action::Record => tracing::info!("screen recording is not available yet"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_line_flags() {
        assert_eq!(
            Action::from_args(&["kadra.exe", "--capture"]),
            Some(Action::Capture)
        );
        assert_eq!(Action::from_args(&["--record"]), Some(Action::Record));
        assert_eq!(Action::from_args(&["kadra.exe"]), None);
    }
}
