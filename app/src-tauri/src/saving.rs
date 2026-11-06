//! Where edited videos are saved, and drafts.
//!
//! A recording that opens in the editor is a *draft*: it is recorded into
//! the drafts folder, not the destination folder. Only what the user
//! exports reaches the destination, so a recording and its edited version
//! never both take up space. Closing the editor without exporting saves
//! the recording as it is (nothing is ever lost silently); *Discard*
//! deletes it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::state::AppState;

#[derive(Debug, Clone)]
struct Draft {
    /// Size of the video, for the history.
    size: (u32, u32),
    /// Something was exported from it.
    exported: bool,
}

#[derive(Debug, Default)]
struct Inner {
    drafts: HashMap<PathBuf, Draft>,
    /// Destination folder chosen in the editor, per file.
    folders: HashMap<PathBuf, PathBuf>,
    /// Files the user picked in a save dialog: allowed as export targets.
    chosen: HashSet<PathBuf>,
}

/// Saving state of the files open in editors.
#[derive(Debug, Default)]
pub struct Saving(Mutex<Inner>);

impl Saving {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn add_draft(&self, path: &Path, size: (u32, u32)) {
        // Keyed like the editor names its files.
        self.lock().drafts.insert(
            crate::editor::normalize(path),
            Draft {
                size,
                exported: false,
            },
        );
    }

    pub fn is_draft(&self, path: &Path) -> bool {
        self.lock().drafts.contains_key(path)
    }

    fn mark_exported(&self, path: &Path) {
        if let Some(draft) = self.lock().drafts.get_mut(path) {
            draft.exported = true;
        }
    }

    fn take_draft(&self, path: &Path) -> Option<Draft> {
        self.lock().drafts.remove(path)
    }

    /// Takes a target picked in a save dialog (once).
    pub fn take_chosen(&self, path: &Path) -> bool {
        self.lock().chosen.remove(path)
    }
}

/// Where a new recording goes: the drafts folder when it will open in the
/// editor, named as it will be in `destination`.
pub fn recording_path(app: &AppHandle, destination: &Path, stem: &str, draft: bool) -> PathBuf {
    let final_path = kadra_encode::unique_path(destination, stem, "mp4");
    if !draft {
        return final_path;
    }
    let dir = app.state::<AppState>().paths().drafts_dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!("cannot create {}: {err}", dir.display());
        return final_path;
    }
    let name = final_path
        .file_stem()
        .map_or_else(|| stem.to_owned(), |s| s.to_string_lossy().into_owned());
    kadra_encode::unique_path(&dir, &name, "mp4")
}

/// The folder exports of `input` go to: chosen in the editor, else the
/// recording folder for drafts, else the folder of the file.
pub fn folder_for(app: &AppHandle, input: &Path) -> PathBuf {
    if let Some(folder) = app.state::<Saving>().lock().folders.get(input) {
        return folder.clone();
    }
    if app.state::<Saving>().is_draft(input) {
        return app
            .state::<AppState>()
            .config()
            .recording
            .output_dir
            .clone();
    }
    input
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map_or_else(|| "video".into(), |s| s.to_string_lossy().into_owned())
}

/// A new file for an export of `input`: drafts keep their name (they are
/// the recording), other files get " (edited)".
pub fn export_path(app: &AppHandle, input: &Path, extension: &str) -> PathBuf {
    let dir = folder_for(app, input);
    let _ = std::fs::create_dir_all(&dir);
    let stem = stem_of(input);
    let stem = if app.state::<Saving>().is_draft(input) {
        stem
    } else {
        format!("{stem} (edited)")
    };
    kadra_encode::unique_path(&dir, &stem, extension)
}

/// An export of `input` finished.
pub fn exported(app: &AppHandle, input: &Path) {
    app.state::<Saving>().mark_exported(input);
}

/// Asks for a destination folder for the exports of `input`.
pub fn choose_folder(app: &AppHandle, input: &Path) -> Option<PathBuf> {
    let current = folder_for(app, input);
    let folder = app
        .dialog()
        .file()
        .set_title("Save edited videos in")
        .set_directory(&current)
        .blocking_pick_folder()?
        .into_path()
        .ok()?;
    app.state::<Saving>()
        .lock()
        .folders
        .insert(input.to_path_buf(), folder.clone());
    Some(folder)
}

/// Asks where to save one export of `input`.
pub fn choose_file(app: &AppHandle, input: &Path, extension: &str) -> Option<PathBuf> {
    let suggested = export_path(app, input, extension);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save as")
        .add_filter(extension.to_uppercase(), &[extension]);
    if let Some(dir) = suggested.parent() {
        dialog = dialog.set_directory(dir);
    }
    if let Some(name) = suggested.file_name() {
        dialog = dialog.set_file_name(name.to_string_lossy());
    }
    let mut path = dialog.blocking_save_file()?.into_path().ok()?;
    if path
        .extension()
        .is_none_or(|e| !e.eq_ignore_ascii_case(extension))
    {
        path.set_extension(extension);
    }
    app.state::<Saving>().lock().chosen.insert(path.clone());
    Some(path)
}

/// A copy of a draft in its destination, for the clipboard (the draft
/// itself goes away when the editor closes).
pub fn save_copy(app: &AppHandle, input: &Path) -> std::io::Result<PathBuf> {
    let target = export_path(app, input, "mp4");
    std::fs::copy(input, &target)?;
    remember(app, input, &target);
    exported(app, input);
    Ok(target)
}

fn remember(app: &AppHandle, draft: &Path, target: &Path) {
    let size = app
        .state::<Saving>()
        .lock()
        .drafts
        .get(draft)
        .map_or((0, 0), |d| d.size);
    crate::capture::remember(app, target, size.0, size.1);
}

/// Moves a file, across drives too. Retries for a moment: the closing
/// window may still hold the video open.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last = None;
    for _ in 0..20 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err) => last = Some(err),
        }
        // Another drive: copy, then delete.
        if std::fs::copy(from, to).is_ok() {
            let _ = std::fs::remove_file(from);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("cannot move")))
}

/// Deletes a file, retrying while the closing window still holds it.
fn delete_file(path: &Path) {
    for _ in 0..20 {
        if std::fs::remove_file(path).is_ok() || !path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    tracing::warn!("cannot delete {}", path.display());
}

/// The editor of `input` closed: an exported draft is deleted, any other
/// draft is saved as recorded.
pub fn editor_closed(app: &AppHandle, input: &Path) {
    let Some(draft) = app.state::<Saving>().take_draft(input) else {
        return;
    };
    let app = app.clone();
    let input = input.to_path_buf();
    let folder = folder_for(&app, &input);
    std::thread::spawn(move || {
        let paths = app.state::<AppState>().paths().clone();
        let (clean, track) = crate::editor::edit_files(&paths, &input);
        if draft.exported {
            delete_file(&input);
            delete_file(&clean);
            delete_file(&track);
            tracing::info!(path = %input.display(), "draft discarded after export");
            return;
        }
        let _ = std::fs::create_dir_all(&folder);
        let target = kadra_encode::unique_path(&folder, &stem_of(&input), "mp4");
        match move_file(&input, &target) {
            Ok(()) => {
                tracing::info!(path = %target.display(), "recording saved as recorded");
                crate::capture::remember(&app, &target, draft.size.0, draft.size.1);
                // The editing material follows the file's name.
                let (new_clean, new_track) = crate::editor::edit_files(&paths, &target);
                let _ = std::fs::rename(&clean, new_clean);
                let _ = std::fs::rename(&track, new_track);
            }
            Err(err) => tracing::error!("cannot save {}: {err}", input.display()),
        }
    });
}

/// Deletes a draft and its editing material (the user discarded it).
pub fn discard(app: &AppHandle, input: &Path) -> bool {
    if app.state::<Saving>().take_draft(input).is_none() {
        return false;
    }
    let paths = app.state::<AppState>().paths().clone();
    let input = input.to_path_buf();
    std::thread::spawn(move || {
        let (clean, track) = crate::editor::edit_files(&paths, &input);
        delete_file(&input);
        delete_file(&clean);
        delete_file(&track);
        tracing::info!(path = %input.display(), "draft discarded");
    });
    true
}

/// Drafts left by a previous session (Kadra quit with an editor open) are
/// saved to the recording folder: never lose a recording.
pub fn recover(app: &AppHandle) {
    let state = app.state::<AppState>();
    let dir = state.paths().drafts_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let folder = state.config().recording.output_dir.clone();
    let paths = state.paths().clone();
    let left: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| crate::convert::is_video(p))
        .collect();
    if left.is_empty() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(&folder);
        for draft in left {
            let target = kadra_encode::unique_path(&folder, &stem_of(&draft), "mp4");
            if move_file(&draft, &target).is_ok() {
                tracing::info!(path = %target.display(), "recovered a draft");
                crate::capture::remember(&app, &target, 0, 0);
                let (clean, track) = crate::editor::edit_files(&paths, &draft);
                let (new_clean, new_track) = crate::editor::edit_files(&paths, &target);
                let _ = std::fs::rename(clean, new_clean);
                let _ = std::fs::rename(track, new_track);
            }
        }
    });
}
