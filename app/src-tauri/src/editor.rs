//! Editor windows: annotate an image, trim a video.
//!
//! Each file opens in its own window (`editor-N`), built from the same web
//! UI as the settings with `?edit=<path>`. The webview can only read, save
//! or export files that were opened this way: paths coming from the web UI
//! are checked against that list.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Files opened in an editor during this session.
#[derive(Debug, Default)]
pub struct EditorFiles(Mutex<HashSet<PathBuf>>);

impl EditorFiles {
    fn files(&self) -> MutexGuard<'_, HashSet<PathBuf>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn allow(&self, path: &Path) {
        self.files().insert(path.to_path_buf());
    }

    pub fn is_allowed(&self, path: &Path) -> bool {
        self.files().contains(path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
}

pub fn kind(path: &Path) -> Option<Kind> {
    if kadra_encode::format_of(path).is_some() {
        Some(Kind::Image)
    } else if crate::convert::is_video(path) {
        Some(Kind::Video)
    } else {
        None
    }
}

/// Percent-encodes everything but unreserved URL characters.
fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Decodes `%XX` sequences (paths sent by the web UI in headers).
pub fn decode_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Opens `path` in a new editor window.
pub fn open(app: &AppHandle, path: &Path) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let Some(kind) = kind(path) else {
        tracing::warn!("cannot edit {}", path.display());
        return;
    };
    if !path.is_file() {
        tracing::warn!("{} no longer exists", path.display());
        return;
    }
    let path = std::fs::canonicalize(path).map_or_else(|_| path.to_path_buf(), strip_unc);
    app.state::<EditorFiles>().allow(&path);
    if kind == Kind::Video
        && let Err(err) = app.asset_protocol_scope().allow_file(&path)
    {
        tracing::warn!("cannot play {}: {err}", path.display());
    }

    let label = format!("editor-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let url = format!(
        "index.html?edit={}",
        encode_component(&path.to_string_lossy())
    );
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let result = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title(format!("{name} — Kadra"))
        .inner_size(1100.0, 760.0)
        .min_inner_size(640.0, 480.0)
        .center()
        .build();
    match result {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(err) => tracing::error!("cannot open the editor: {err}"),
    }
}

/// `canonicalize` returns `\\?\C:\...`; the web UI and the history use the
/// plain form.
fn strip_unc(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(plain) if !plain.starts_with("UNC") => PathBuf::from(plain),
        _ => path,
    }
}

/// Opens a capture (preview or recent capture clicked): in the editor when
/// it can edit it, otherwise with the default application.
pub fn open_capture(app: &AppHandle, path: &Path) {
    if kind(path).is_some() {
        open(app, path);
    } else {
        crate::windows::open_file(path);
    }
}

/// Where the editing material of `video` lives: the copy without cursor
/// and the cursor track.
pub fn edit_files(paths: &kadra_core::Paths, video: &Path) -> (PathBuf, PathBuf) {
    let name = video
        .file_name()
        .map_or_else(|| "video".into(), |n| n.to_string_lossy().into_owned());
    let dir = paths.edit_dir();
    (
        dir.join(format!("{name}.clean.mp4")),
        dir.join(format!("{name}.cursor.json")),
    )
}

/// Editing material that exists for `video`.
pub fn existing_edit_files(paths: &kadra_core::Paths, video: &Path) -> Option<(PathBuf, PathBuf)> {
    let (clean, track) = edit_files(paths, video);
    (clean.is_file() && track.is_file()).then_some((clean, track))
}

/// Keeps the material of the most recent recordings only: it doubles the
/// disk space a recording takes.
pub fn prune_edit_files(paths: &kadra_core::Paths) {
    const KEEP: usize = 30;
    let dir = paths.edit_dir();
    if let Err(err) = std::fs::create_dir_all(dir) {
        tracing::warn!("cannot create {}: {err}", dir.display());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut clean: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".clean.mp4"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    clean.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, path) in clean.into_iter().skip(KEEP) {
        let track = PathBuf::from(path.to_string_lossy().replace(".clean.mp4", ".cursor.json"));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(track);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_survive_url_encoding() {
        let path = r"C:\Users\Zoë\Vidéos\Kadra 2026-11-12 01-05-58 (1).mp4";
        let encoded = encode_component(path);
        assert!(encoded.is_ascii() && !encoded.contains(' '));
        assert_eq!(decode_component(&encoded).as_deref(), Some(path));
        assert_eq!(decode_component("%4"), None);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind(Path::new("a.PNG")), Some(Kind::Image));
        assert_eq!(kind(Path::new("a.mp4")), Some(Kind::Video));
        assert_eq!(kind(Path::new("a.gif")), None);
        assert_eq!(
            strip_unc(PathBuf::from(r"\\?\C:\a\b.png")),
            PathBuf::from(r"C:\a\b.png")
        );
    }
}
