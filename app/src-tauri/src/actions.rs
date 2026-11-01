//! User-facing actions, triggered from hotkeys, the tray or the command line.

use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::{capture, convert, recording};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Capture,
    Record,
    /// Convert a video file to GIF (`kadra.exe --to-gif video.mp4`).
    ToGif(PathBuf),
    /// Open a file in the editor (`kadra.exe --edit capture.png`).
    Edit(PathBuf),
}

impl Action {
    /// Command-line flag that triggers the action (`kadra.exe --capture`).
    /// Relative paths are resolved against `cwd`.
    pub fn from_args<S: AsRef<str>>(args: &[S], cwd: &Path) -> Option<Self> {
        let mut args = args.iter().map(AsRef::as_ref);
        while let Some(arg) = args.next() {
            match arg {
                "--capture" => return Some(Self::Capture),
                "--record" => return Some(Self::Record),
                "--to-gif" => return args.next().map(|path| Self::ToGif(cwd.join(path))),
                "--edit" => return args.next().map(|path| Self::Edit(cwd.join(path))),
                _ => {}
            }
        }
        None
    }
}

pub fn trigger(app: &AppHandle, action: Action) {
    match action {
        Action::Capture => capture::start(app, kadra_overlay::Purpose::Screenshot),
        // Starts selecting an area, or stops the current recording.
        Action::Record => recording::toggle(app),
        Action::ToGif(path) => {
            convert::to_gif(app, path);
        }
        Action::Edit(path) => crate::editor::open(app, &path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_line_flags() {
        let cwd = Path::new(r"C:\work");
        assert_eq!(
            Action::from_args(&["kadra.exe", "--capture"], cwd),
            Some(Action::Capture)
        );
        assert_eq!(Action::from_args(&["--record"], cwd), Some(Action::Record));
        assert_eq!(Action::from_args(&["kadra.exe"], cwd), None);
        assert_eq!(
            Action::from_args(&["kadra.exe", "--to-gif", "clip.mp4"], cwd),
            Some(Action::ToGif(PathBuf::from(r"C:\work\clip.mp4")))
        );
        assert_eq!(
            Action::from_args(&["--to-gif", r"D:\v.mp4"], cwd),
            Some(Action::ToGif(PathBuf::from(r"D:\v.mp4")))
        );
        assert_eq!(Action::from_args(&["--to-gif"], cwd), None);
        assert_eq!(
            Action::from_args(&["--edit", "a.png"], cwd),
            Some(Action::Edit(PathBuf::from(r"C:\work\a.png")))
        );
    }
}
