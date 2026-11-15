//! Commands invoked from the web UI.
//!
//! Errors are returned as plain strings: they are shown to the user as-is.

// Tauri injects `State` and `AppHandle` by value.
#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;

use capshot_core::Config;
use serde::{Deserialize, Serialize};
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, Manager, State};

use crate::capture::CaptureService;
use crate::convert::Task;
use crate::editor::EditorFiles;
use crate::saving::Saving;

use crate::hotkeys;
use crate::state::AppState;
use crate::tray;
use crate::windows;

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Config {
    state.config().clone()
}

/// Validates, applies and persists a new configuration.
///
/// Hotkeys are applied first: if one cannot be registered, the previous
/// bindings are restored and nothing is saved.
#[tauri::command]
pub fn save_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: Config,
) -> Result<(), String> {
    config.validate().map_err(|e| e.to_string())?;

    let previous = state.config().clone();
    if previous.hotkeys != config.hotkeys
        && let Err(err) = hotkeys::apply(&app, &config.hotkeys)
    {
        if let Err(restore) = hotkeys::apply(&app, &previous.hotkeys) {
            tracing::warn!("cannot restore previous hotkeys: {restore}");
        }
        return Err(err.to_string());
    }

    if previous.startup != config.startup
        && let Ok(exe) = std::env::current_exe()
        && let Err(err) = crate::system::set_startup(config.startup, &exe)
    {
        tracing::warn!("cannot change starting with Windows: {err}");
    }

    config
        .save(&state.paths().config_file())
        .map_err(|e| e.to_string())?;
    *state.config() = config;
    tray::refresh(&app);
    // Open windows follow the change (theme, toolbar toggles).
    let _ = tauri::Emitter::emit(&app, crate::recording::CONFIG_EVENT, &*state.config());
    tracing::info!("settings saved");
    Ok(())
}

/// Whether Windows keeps the Print Screen key for its own screen capture.
#[tauri::command]
pub fn print_screen_taken() -> bool {
    crate::system::print_screen_taken()
}

/// Turns Windows' own Print Screen capture off, for this user.
#[tauri::command]
pub fn release_print_screen() -> Result<(), String> {
    crate::system::release_print_screen()
}

/// Opens the Windows page where the Print Screen key can be freed.
#[tauri::command]
pub fn open_keyboard_settings() {
    if let Err(err) = std::process::Command::new("explorer")
        .arg("ms-settings:easeofaccess-keyboard")
        .spawn()
    {
        tracing::warn!("cannot open the keyboard settings: {err}");
    }
}

/// Asks for a folder; returns it, or nothing when the dialog is closed.
#[tauri::command]
pub async fn pick_folder(app: AppHandle, title: String, start: String) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut dialog = app.dialog().file().set_title(title);
    if std::path::Path::new(&start).is_dir() {
        dialog = dialog.set_directory(&start);
    }
    dialog
        .blocking_pick_folder()?
        .into_path()
        .ok()
        .map(|p| p.display().to_string())
}

/// Opens the setup again from the settings.
#[tauri::command]
pub fn open_setup(app: AppHandle) {
    windows::show_setup(&app);
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    config_file: String,
    log_dir: String,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        config_file: state.paths().config_file().display().to_string(),
        log_dir: state.paths().log_dir().display().to_string(),
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Folder {
    Screenshots,
    Recordings,
    Logs,
}

/// Opens one of the application folders. Only known folders can be opened,
/// never an arbitrary path coming from the webview.
#[tauri::command]
pub fn open_folder(state: State<'_, AppState>, folder: Folder) {
    let dir = match folder {
        Folder::Screenshots => state.config().capture.output_dir.clone(),
        Folder::Recordings => state.config().recording.output_dir.clone(),
        Folder::Logs => state.paths().log_dir().to_path_buf(),
    };
    windows::open_in_explorer(&dir);
}

/// Converts a video file to GIF in the background; returns the job id whose
/// progress is reported with [`crate::convert::PROGRESS_EVENT`].
#[tauri::command]
pub fn convert_to_gif(app: AppHandle, path: String) -> u64 {
    crate::convert::to_gif(&app, PathBuf::from(path))
}

/// A path the web UI may access: only files opened in an editor.
fn editable(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
    let path = crate::editor::normalize(&PathBuf::from(path));
    if app.state::<EditorFiles>().is_allowed(&path) {
        Ok(path)
    } else {
        tracing::warn!("refusing access to {}", path.display());
        Err("this file was not opened in the editor".into())
    }
}

/// Path sent in the `x-path` header of a raw request (percent-encoded).
fn header_path(app: &AppHandle, request: &Request<'_>) -> Result<PathBuf, String> {
    let header = request
        .headers()
        .get("x-path")
        .and_then(|v| v.to_str().ok())
        .and_then(crate::editor::decode_component)
        .ok_or("missing path")?;
    editable(app, &header)
}

fn raw_body<'a>(request: &'a Request<'_>) -> Result<&'a [u8], String> {
    match request.body() {
        InvokeBody::Raw(bytes) => Ok(bytes),
        InvokeBody::Json(_) => Err("expected raw image bytes".into()),
    }
}

/// Asks for a picture, a sound or a video to bring into the editor; returns
/// its path (the editor may read it from then on), or nothing when the
/// dialog is closed.
#[tauri::command]
pub async fn editor_pick_media(app: AppHandle, kind: String) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (title, name, extensions): (&str, &str, &[&str]) = match kind.as_str() {
        "image" => (
            "Add a picture",
            "Pictures",
            &["png", "jpg", "jpeg", "webp", "bmp", "gif"],
        ),
        "audio" => (
            "Add music or a sound",
            "Sounds",
            &["mp3", "wav", "m4a", "aac", "flac", "ogg", "opus", "wma"],
        ),
        "video" => (
            "Add a video",
            "Videos",
            &["mp4", "mov", "mkv", "webm", "avi", "m4v"],
        ),
        _ => return Err("unknown kind of file".into()),
    };
    let Some(file) = app
        .dialog()
        .file()
        .set_title(title)
        .add_filter(name, extensions)
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    let (path, _) = crate::editor::allow_media(&app, &path).ok_or("this file cannot be used")?;
    Ok(Some(path.display().to_string()))
}

/// Reads a file opened in the editor, as raw bytes.
#[tauri::command]
pub fn editor_read(app: AppHandle, path: String) -> Result<Response, String> {
    let path = editable(&app, &path)?;
    std::fs::read(&path)
        .map(Response::new)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Saves an edited image over the original, in the original's format. The
/// body is the image encoded as PNG.
#[tauri::command]
pub fn editor_save(app: AppHandle, request: Request<'_>) -> Result<(), String> {
    let path = header_path(&app, &request)?;
    let image = capshot_encode::decode(raw_body(&request)?).map_err(|e| e.to_string())?;
    let format = capshot_encode::format_of(&path).ok_or("unsupported image format")?;
    let bytes = capshot_encode::encode(&image, format).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("capshot-tmp");
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("cannot save {}: {e}", path.display()))?;
    tracing::info!(path = %path.display(), "edited image saved");
    Ok(())
}

/// Saves an edited image (PNG body) as a new file next to the original,
/// `<name> (edited).<ext>`; returns its path. The copy joins the history
/// and can itself be edited.
#[tauri::command]
pub fn editor_save_copy(app: AppHandle, request: Request<'_>) -> Result<String, String> {
    let original = header_path(&app, &request)?;
    let image = capshot_encode::decode(raw_body(&request)?).map_err(|e| e.to_string())?;
    let format = capshot_encode::format_of(&original).ok_or("unsupported image format")?;
    let bytes = capshot_encode::encode(&image, format).map_err(|e| e.to_string())?;
    let dir = original.parent().ok_or("no folder")?;
    let stem = original
        .file_stem()
        .map_or_else(|| "capture".into(), |s| s.to_string_lossy().into_owned());
    let extension = original
        .extension()
        .map_or_else(|| "png".into(), |e| e.to_string_lossy().into_owned());
    let path = capshot_encode::unique_path(dir, &format!("{stem} (edited)"), &extension);
    std::fs::write(&path, bytes).map_err(|e| format!("cannot save {}: {e}", path.display()))?;
    tracing::info!(path = %path.display(), "edited copy saved");
    app.state::<EditorFiles>().allow(&path);
    crate::capture::remember(&app, &path, image.width(), image.height());
    Ok(path.display().to_string())
}

/// Copies an edited image (PNG body) to the clipboard.
#[tauri::command]
pub fn editor_copy(app: AppHandle, request: Request<'_>) -> Result<(), String> {
    let image = capshot_encode::decode(raw_body(&request)?).map_err(|e| e.to_string())?;
    let service = app
        .try_state::<CaptureService>()
        .ok_or("the clipboard is unavailable")?;
    service.copy_image(image);
    Ok(())
}

/// Exports part of a video opened in the editor; returns the job id. With
/// `copy`, the result goes to the clipboard once written.
#[tauri::command]
pub fn editor_export(
    app: AppHandle,
    path: String,
    task: Task,
    copy: Option<bool>,
    output: Option<String>,
) -> Result<u64, String> {
    let path = editable(&app, &path)?;
    // The music must be a file brought into the editor.
    let mut task = task;
    if let Task::Mp4 {
        music: Some(music), ..
    } = &mut task
    {
        music.path = editable(&app, &music.path.to_string_lossy())?;
    }
    // Only a file the user picked in the save dialog.
    let output = match output.map(PathBuf::from) {
        Some(output) if app.state::<Saving>().take_chosen(&output) => Some(output),
        Some(_) => return Err("this destination was not chosen in the editor".into()),
        None => None,
    };
    Ok(crate::convert::export(
        &app,
        path,
        task,
        copy.unwrap_or(false),
        output,
    ))
}

/// Puts a file opened in the editor on the clipboard (pasted as a file).
/// A draft is saved first: it disappears when the editor closes.
#[tauri::command]
pub fn editor_copy_file(app: AppHandle, path: String) -> Result<(), String> {
    let path = editable(&app, &path)?;
    let service = app
        .try_state::<CaptureService>()
        .ok_or("the clipboard is unavailable")?;
    let file = if app.state::<Saving>().is_draft(&path) {
        crate::saving::save_copy(&app, &path).map_err(|e| e.to_string())?
    } else {
        path
    };
    service.copy_files(vec![file]);
    Ok(())
}

/// What the video editor can use besides the video: the copy without
/// cursor (playable) and the cursor track, when the recording kept them;
/// and where exports go.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSources {
    clean: Option<String>,
    track: Option<serde_json::Value>,
    /// Folder exports are saved in.
    folder: String,
    /// The recording is not saved yet: only exports will be.
    draft: bool,
    /// Size of the video file, in bytes.
    bytes: u64,
}

#[tauri::command]
pub fn editor_video_sources(app: AppHandle, path: String) -> Result<VideoSources, String> {
    let path = editable(&app, &path)?;
    let mut sources = VideoSources {
        clean: None,
        track: None,
        folder: crate::saving::folder_for(&app, &path).display().to_string(),
        draft: app.state::<Saving>().is_draft(&path),
        bytes: std::fs::metadata(&path).map_or(0, |m| m.len()),
    };
    let Some((clean, track)) =
        crate::editor::existing_edit_files(app.state::<AppState>().paths(), &path)
    else {
        return Ok(sources);
    };
    if let Err(err) = app.asset_protocol_scope().allow_file(&clean) {
        tracing::warn!("cannot play {}: {err}", clean.display());
    }
    sources.track = std::fs::read(&track)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    sources.clean = Some(clean.display().to_string());
    Ok(sources)
}

/// Asks for the folder exports of `path` go to; returns it.
#[tauri::command]
pub async fn editor_choose_folder(app: AppHandle, path: String) -> Result<Option<String>, String> {
    let path = editable(&app, &path)?;
    Ok(crate::saving::choose_folder(&app, &path).map(|p| p.display().to_string()))
}

/// Asks where to save one export of `path`; returns the file picked.
#[tauri::command]
pub async fn editor_save_as(
    app: AppHandle,
    path: String,
    extension: String,
) -> Result<Option<String>, String> {
    let path = editable(&app, &path)?;
    let extension = if extension == "gif" { "gif" } else { "mp4" };
    Ok(crate::saving::choose_file(&app, &path, extension).map(|p| p.display().to_string()))
}

/// Deletes a recording that was never saved, and closes its editor.
#[tauri::command]
pub fn editor_discard(app: AppHandle, window: tauri::Window, path: String) -> Result<(), String> {
    let path = editable(&app, &path)?;
    if !crate::saving::discard(&app, &path) {
        return Err("only an unsaved recording can be discarded".into());
    }
    let _ = window.destroy();
    Ok(())
}

/// Selects a file in Explorer. Only files Capshot created (listed in the
/// history) can be shown, never an arbitrary path coming from the webview.
#[tauri::command]
pub fn show_in_folder(state: State<'_, AppState>, path: String) {
    let path = std::path::PathBuf::from(path);
    let known = state.history().recent().any(|entry| entry.path == path);
    if !known {
        tracing::warn!("refusing to show {}", path.display());
        return;
    }
    crate::windows::select_in_explorer(&path);
}

/// Asks where to save an image opened in the editor; returns the file
/// picked (then written by `editor_save_to`).
#[tauri::command]
pub async fn editor_image_save_as(app: AppHandle, path: String) -> Result<Option<String>, String> {
    let path = editable(&app, &path)?;
    let extension = path
        .extension()
        .map_or_else(|| "png".to_owned(), |e| e.to_string_lossy().to_lowercase());
    Ok(crate::saving::choose_file(&app, &path, &extension).map(|p| p.display().to_string()))
}

/// Writes an image (PNG body) to the file picked in the save dialog
/// (`x-output` header), in that file's format; returns its path.
#[tauri::command]
pub fn editor_save_to(app: AppHandle, request: Request<'_>) -> Result<String, String> {
    header_path(&app, &request)?;
    let output = request
        .headers()
        .get("x-output")
        .and_then(|v| v.to_str().ok())
        .and_then(crate::editor::decode_component)
        .map(PathBuf::from)
        .ok_or("missing destination")?;
    if !app.state::<Saving>().take_chosen(&output) {
        return Err("this destination was not chosen in the editor".into());
    }
    let image = capshot_encode::decode(raw_body(&request)?).map_err(|e| e.to_string())?;
    let format = capshot_encode::format_of(&output).unwrap_or(capshot_core::ImageFormat::Png);
    let bytes = capshot_encode::encode(&image, format).map_err(|e| e.to_string())?;
    std::fs::write(&output, bytes).map_err(|e| format!("cannot save {}: {e}", output.display()))?;
    tracing::info!(path = %output.display(), "image saved as");
    app.state::<EditorFiles>().allow(&output);
    crate::capture::remember(&app, &output, image.width(), image.height());
    Ok(output.display().to_string())
}

// ── Quick actions after a capture ──────────────────────────────────────

/// A capture shown in a quick-actions window.
fn quick_path(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
    let path = editable(app, path)?;
    if app.state::<crate::quick::QuickFiles>().contains(&path) {
        Ok(path)
    } else {
        Err("this capture is not open in a quick-actions window".into())
    }
}

#[tauri::command]
pub fn quick_info(app: AppHandle, path: String) -> Result<crate::quick::Info, String> {
    Ok(crate::quick::info(&app, &quick_path(&app, &path)?))
}

/// Saves the capture to its folder; returns its new path.
#[tauri::command]
pub fn quick_save(app: AppHandle, path: String) -> Result<String, String> {
    let path = quick_path(&app, &path)?;
    crate::quick::save(&app, &path, None).map(|p| p.display().to_string())
}

/// Saves the capture where the user picks; returns its new path, if any.
#[tauri::command]
pub async fn quick_save_as(app: AppHandle, path: String) -> Result<Option<String>, String> {
    let path = quick_path(&app, &path)?;
    let Some(to) = crate::quick::choose(&app, &path) else {
        return Ok(None);
    };
    crate::quick::save(&app, &path, Some(to)).map(|p| Some(p.display().to_string()))
}

#[tauri::command]
pub fn quick_copy(app: AppHandle, path: String) -> Result<(), String> {
    crate::quick::copy(&app, &quick_path(&app, &path)?)
}

/// Windows are built from async commands only: a synchronous command runs
/// on the main thread, where creating a webview window freezes it (blank
/// window).
/// Opens the capture in the editor and closes the quick-actions window.
#[tauri::command]
pub async fn quick_edit(app: AppHandle, window: tauri::Window, path: String) -> Result<(), String> {
    crate::quick::edit(&app, &quick_path(&app, &path)?)?;
    let _ = window.destroy();
    Ok(())
}

#[tauri::command]
pub async fn quick_pin(app: AppHandle, window: tauri::Window, path: String) -> Result<(), String> {
    let scale = window.scale_factor().unwrap_or(1.0);
    crate::quick::pin(&app, &quick_path(&app, &path)?, scale)
}

/// Closes the quick-actions window after an action, with a thumbnail
/// confirming it (`label`) for a screenshot.
#[tauri::command]
pub async fn quick_done(
    app: AppHandle,
    window: tauri::Window,
    path: String,
    label: Option<String>,
) -> Result<(), String> {
    crate::quick::done(&app, &window, &quick_path(&app, &path)?, label);
    Ok(())
}

/// Text in the screenshot (Windows OCR).
#[tauri::command]
pub async fn quick_text(app: AppHandle, path: String) -> Result<String, String> {
    crate::quick::read_text(&quick_path(&app, &path)?)
}

/// Contents of the QR codes in the screenshot.
#[tauri::command]
pub async fn quick_codes(app: AppHandle, path: String) -> Result<Vec<String>, String> {
    crate::quick::read_codes(&quick_path(&app, &path)?)
}

/// Deletes an unsaved recording and closes its window.
#[tauri::command]
pub fn quick_discard(app: AppHandle, window: tauri::Window, path: String) -> Result<(), String> {
    let path = quick_path(&app, &path)?;
    if crate::editor::kind(&path) == Some(crate::editor::Kind::Video) {
        crate::saving::discard(&app, &path);
    }
    let _ = window.destroy();
    Ok(())
}

/// Puts text on the clipboard (recognised text, a QR code, a colour).
#[tauri::command]
pub fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    let service = app
        .try_state::<CaptureService>()
        .ok_or("the clipboard is unavailable")?;
    service.copy_text(text);
    Ok(())
}

/// Looks for an update now (the settings button).
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> crate::updates::Status {
    crate::updates::check(&app).await
}

/// Installs the downloaded update and restarts.
#[tauri::command]
pub fn install_update(app: AppHandle) -> Result<(), String> {
    crate::updates::install(&app)
}
