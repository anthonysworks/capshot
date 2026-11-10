//! The window shown after a capture: what can be done with it at once
//! (copy, save, edit, pin, read its text or QR code, pick a colour).
//!
//! The capture is not in the destination folder yet: a screenshot waits in
//! a temporary folder and a recording is a draft (see `saving`). *Save*
//! moves it there; closing the window without saving deletes it (the
//! clipboard keeps its copy). *Edit* hands it to the editor.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::editor::{EditorFiles, Kind};
use crate::saving::Saving;
use crate::state::AppState;

/// Captures shown in a quick-actions window, by path.
#[derive(Debug, Default)]
pub struct QuickFiles(Mutex<HashMap<PathBuf, Entry>>);

#[derive(Debug, Clone, Copy, Default)]
struct Entry {
    /// Moved to its destination: nothing to clean up.
    saved: bool,
    /// Now open in the editor, which owns it.
    edited: bool,
    /// Left in the temporary folder after an action (cleaned at start-up),
    /// so the confirmation thumbnail can still open it.
    kept: bool,
}

impl QuickFiles {
    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, Entry>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.lock().contains_key(path)
    }
}

/// Where screenshots wait until they are saved.
pub fn pending_dir() -> PathBuf {
    std::env::temp_dir().join("Capshot").join("captures")
}

/// Opens the quick-actions window for a capture that was just taken.
pub fn open(app: &AppHandle, path: &Path) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let Some(kind) = crate::editor::kind(path) else {
        return;
    };
    let path = crate::editor::normalize(path);
    app.state::<EditorFiles>().allow(&path);
    app.state::<QuickFiles>()
        .lock()
        .insert(path.clone(), Entry::default());
    if kind == Kind::Video
        && let Err(err) = app.asset_protocol_scope().allow_file(&path)
    {
        tracing::warn!("cannot play {}: {err}", path.display());
    }
    let label = format!("quick-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    // A small toolbar with the preview, at the bottom right of the screen.
    let (width, height) = if kind == Kind::Video {
        (540.0, 196.0)
    } else {
        (540.0, 252.0)
    };
    let (x, y) = corner(app, width, height);
    let url = format!(
        "index.html?quick={}",
        crate::editor::encode_component(&path.to_string_lossy())
    );
    let result = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title(if kind == Kind::Video {
            "Capshot · Recording"
        } else {
            "Capshot · Screenshot"
        })
        .inner_size(width, height)
        .position(x, y)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .skip_taskbar(true)
        .shadow(true)
        .always_on_top(true)
        .build();
    match result {
        Ok(window) => {
            let _ = window.set_focus();
            let app = app.clone();
            window.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    closed(&app, &path);
                }
            });
        }
        Err(err) => {
            tracing::error!("cannot open the capture window: {err}");
            closed(app, &path);
        }
    }
}

/// Bottom right of the primary screen (logical pixels), above the taskbar.
fn corner(app: &AppHandle, width: f64, height: f64) -> (f64, f64) {
    let Ok(Some(monitor)) = app.primary_monitor() else {
        return (100.0, 100.0);
    };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let (screen_w, screen_h) = (
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    );
    (
        (screen_w - width - 24.0).max(0.0),
        (screen_h - height - 72.0).max(0.0),
    )
}

/// The window closed: a capture neither saved nor edited is deleted.
fn closed(app: &AppHandle, path: &Path) {
    let Some(entry) = app.state::<QuickFiles>().lock().remove(path) else {
        return;
    };
    if entry.saved || entry.edited || entry.kept {
        return;
    }
    match crate::editor::kind(path) {
        Some(Kind::Video) => crate::saving::editor_closed(app, path),
        _ => {
            if path.starts_with(pending_dir()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

fn mark(app: &AppHandle, path: &Path, change: impl FnOnce(&mut Entry)) {
    if let Some(entry) = app.state::<QuickFiles>().lock().get_mut(path) {
        change(entry);
    }
}

/// What the window shows about a capture.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    video: bool,
    /// Already in its destination.
    saved: bool,
    /// Folder Save puts it in.
    folder: String,
    bytes: u64,
}

pub fn info(app: &AppHandle, path: &Path) -> Info {
    let video = crate::editor::kind(path) == Some(Kind::Video);
    let saved = !(path.starts_with(pending_dir()) || app.state::<Saving>().is_draft(path));
    Info {
        video,
        saved,
        folder: destination(app, path, video).display().to_string(),
        bytes: std::fs::metadata(path).map_or(0, |m| m.len()),
    }
}

fn destination(app: &AppHandle, path: &Path, video: bool) -> PathBuf {
    if video {
        crate::saving::folder_for(app, path)
    } else {
        app.state::<AppState>().config().capture.output_dir.clone()
    }
}

/// Moves the capture to its folder (or to `to`, picked in the save
/// dialog); returns where it went.
pub fn save(app: &AppHandle, path: &Path, to: Option<PathBuf>) -> Result<PathBuf, String> {
    let video = crate::editor::kind(path) == Some(Kind::Video);
    let target = if let Some(to) = to {
        to
    } else {
        let dir = destination(app, path, video);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let stem = path
            .file_stem()
            .map_or_else(|| "Capshot".into(), |s| s.to_string_lossy().into_owned());
        let extension = path
            .extension()
            .map_or_else(|| "png".into(), |e| e.to_string_lossy().into_owned());
        capshot_encode::unique_path(&dir, &stem, &extension)
    };
    if video {
        crate::saving::keep_draft(app, path, &target).map_err(|e| e.to_string())?;
    } else {
        std::fs::copy(path, &target)
            .map_err(|e| format!("cannot save {}: {e}", target.display()))?;
        let _ = std::fs::remove_file(path);
        let (width, height) = image_size(&target);
        crate::capture::remember(app, &target, width, height);
    }
    tracing::info!(path = %target.display(), "capture saved");
    let target = crate::editor::normalize(&target);
    app.state::<EditorFiles>().allow(&target);
    if video && let Err(err) = app.asset_protocol_scope().allow_file(&target) {
        tracing::warn!("cannot play {}: {err}", target.display());
    }
    // The window now shows the saved file.
    let files = app.state::<QuickFiles>();
    let mut files = files.lock();
    files.remove(path);
    files.insert(
        target.clone(),
        Entry {
            saved: true,
            ..Entry::default()
        },
    );
    Ok(target)
}

fn image_size(path: &Path) -> (u32, u32) {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| capshot_encode::decode(&bytes).ok())
        .map_or((0, 0), |image| (image.width(), image.height()))
}

/// Hands the capture to the editor. A screenshot is saved first: the
/// editor saves over the file it opened.
pub fn edit(app: &AppHandle, path: &Path) -> Result<(), String> {
    let video = crate::editor::kind(path) == Some(Kind::Video);
    let path = if !video && path.starts_with(pending_dir()) {
        save(app, path, None)?
    } else {
        path.to_path_buf()
    };
    mark(app, &path, |entry| entry.edited = true);
    crate::editor::open(app, &path);
    Ok(())
}

/// Puts the capture on the clipboard: the picture, or the video as a file
/// (a draft is saved first, as in the editor).
pub fn copy(app: &AppHandle, path: &Path) -> Result<(), String> {
    let service = app
        .try_state::<crate::capture::CaptureService>()
        .ok_or("the clipboard is unavailable")?;
    if crate::editor::kind(path) == Some(Kind::Video) {
        let file = if app.state::<Saving>().is_draft(path) {
            crate::saving::save_copy(app, path).map_err(|e| e.to_string())?
        } else {
            path.to_path_buf()
        };
        service.copy_files(vec![file]);
    } else {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        service.copy_image(capshot_encode::decode(&bytes).map_err(|e| e.to_string())?);
    }
    Ok(())
}

/// An action of the window was done: the window goes away and, for a
/// screenshot, the thumbnail in the corner of the screen confirms it with
/// `label` (clicking it opens the editor).
pub fn done(app: &AppHandle, window: &tauri::Window, path: &Path, label: Option<String>) {
    if crate::editor::kind(path) != Some(Kind::Video)
        && let Some(label) = label
        && let Some(service) = app.try_state::<crate::capture::CaptureService>()
        && let Ok(bytes) = std::fs::read(path)
        && let Ok(image) = capshot_encode::decode(&bytes)
    {
        mark(app, path, |entry| entry.kept = true);
        // The screen the window is on.
        let region = match (window.outer_position(), window.outer_size()) {
            (Ok(p), Ok(s)) => capshot_core::Rect::from_size(
                p.x,
                p.y,
                i32::try_from(s.width).unwrap_or(1),
                i32::try_from(s.height).unwrap_or(1),
            ),
            _ => capshot_core::Rect::from_size(0, 0, 1, 1),
        };
        service.show_file_preview(image, path.to_path_buf(), region, Some(label));
    }
    let _ = window.destroy();
}

/// Shows the screenshot in a small window that stays on top.
pub fn pin(app: &AppHandle, path: &Path, scale: f64) -> Result<(), String> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let (width, height) = image_size(path);
    if width == 0 {
        return Err("cannot read the image".into());
    }
    // A pinned file outlives the capture window: keep a copy of its own.
    let dir = pending_dir().join("pinned");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let pinned = capshot_encode::unique_path(&dir, "pin", "png");
    std::fs::copy(path, &pinned).map_err(|e| e.to_string())?;
    let pinned = crate::editor::normalize(&pinned);
    app.state::<EditorFiles>().allow(&pinned);
    let scale = scale.max(0.5);
    let (w, h) = (f64::from(width) / scale, f64::from(height) / scale);
    // At most 70% of a 1080p screen, keeping the proportions.
    let fit = (1300.0 / w).min(760.0 / h).min(1.0);
    let label = format!("pin-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let url = format!(
        "index.html?pin={}",
        crate::editor::encode_component(&pinned.to_string_lossy())
    );
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("Capshot · Pinned")
        .inner_size(w * fit, h * fit)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(true)
        .center()
        .build()
        .map_err(|e| e.to_string())?;
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = std::fs::remove_file(&pinned);
        }
    });
    Ok(())
}

/// Reads the text in a screenshot with the OCR built into Windows, in the
/// languages of the user's profile.
pub fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    text_in(capshot_encode::decode(&bytes).map_err(|e| e.to_string())?)
}

/// Text in a picture (Windows OCR).
pub fn text_in(image: capshot_core::BgraImage) -> Result<String, String> {
    // WinRT needs an initialised thread; this one is ours alone.
    std::thread::spawn(move || ocr::recognize(&image))
        .join()
        .map_err(|_| "text recognition failed".to_owned())?
}

/// QR codes (and other 2D codes rqrr reads) in a screenshot.
pub fn read_codes(path: &Path) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let image = capshot_encode::decode(&bytes).map_err(|e| e.to_string())?;
    Ok(codes_in(&image))
}

/// Contents of the QR codes in a picture.
pub fn codes_in(image: &capshot_core::BgraImage) -> Vec<String> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let data = image.as_bytes();
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| {
        let i = (y * width + x) * 4;
        let (b, g, r) = (
            u32::from(data[i]),
            u32::from(data[i + 1]),
            u32::from(data[i + 2]),
        );
        #[allow(clippy::cast_possible_truncation)]
        let luma = ((r * 299 + g * 587 + b * 114) / 1000) as u8;
        luma
    });
    prepared
        .detect_grids()
        .into_iter()
        .filter_map(|grid| grid.decode().ok().map(|(_, content)| content))
        .collect()
}

mod ocr {
    use capshot_core::BgraImage;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};

    pub fn recognize(image: &BgraImage) -> Result<String, String> {
        // SAFETY: initialises WinRT on this thread, undone below.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| e.to_string())?;
        let result = run(image).map_err(|e| e.message());
        // SAFETY: pairs with the RoInitialize above.
        unsafe { RoUninitialize() };
        result.and_then(|text| {
            if text.is_none() {
                Err("no OCR language is installed in Windows (Settings › Time & language › Language)".into())
            } else {
                Ok(text.unwrap_or_default())
            }
        })
    }

    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    fn run(image: &BgraImage) -> windows::core::Result<Option<String>> {
        let engine = match OcrEngine::TryCreateFromUserProfileLanguages() {
            Ok(engine) => engine,
            Err(_) => {
                match OcrEngine::TryCreateFromLanguage(&Language::CreateLanguage(&"en-US".into())?)
                {
                    Ok(engine) => engine,
                    Err(_) => return Ok(None),
                }
            }
        };
        let max = OcrEngine::MaxImageDimension()?;
        // Screen text is small and often light on dark: the engine reads
        // dark text on a light background, a few times larger, best. The
        // other polarity is tried when the first finds nothing.
        let prepared = prepare(image, max);
        let text = read(&engine, &prepared)?;
        if !text.trim().is_empty() {
            return Ok(Some(text));
        }
        let mut inverted = prepared;
        invert(&mut inverted);
        let other = read(&engine, &inverted)?;
        Ok(Some(other))
    }

    #[allow(clippy::cast_possible_wrap)]
    fn read(engine: &OcrEngine, image: &BgraImage) -> windows::core::Result<String> {
        let buffer = CryptographicBuffer::CreateFromByteArray(image.as_bytes())?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            image.width() as i32,
            image.height() as i32,
            BitmapAlphaMode::Ignore,
        )?;
        let result = engine.RecognizeAsync(&bitmap)?.join()?;
        let mut lines = Vec::new();
        for line in result.Lines()? {
            lines.push(line.Text()?.to_string());
        }
        Ok(lines.join("\n"))
    }

    /// Grey levels on an opaque background (screen grabs can carry a zero
    /// alpha, and freeform selections are transparent outside), dark text
    /// on light, with a margin, scaled up so small text is a readable size.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn prepare(image: &BgraImage, max: u32) -> BgraImage {
        let (w, h) = (image.width() as usize, image.height() as usize);
        let src = image.as_bytes();
        let mut grey = vec![255u8; w * h];
        // A grab whose alpha is zero everywhere carries no transparency.
        let masked = src.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
        let mut sum = 0u64;
        for (i, g) in grey.iter_mut().enumerate() {
            let p = &src[i * 4..i * 4 + 4];
            let luma =
                (u32::from(p[2]) * 299 + u32::from(p[1]) * 587 + u32::from(p[0]) * 114) / 1000;
            // Transparent pixels (outside a freeform shape) count as background.
            *g = if masked && p[3] == 0 { 255 } else { luma as u8 };
            sum += u64::from(*g);
        }
        let dark = !grey.is_empty() && sum / (grey.len() as u64) < 128;
        if dark {
            for g in &mut grey {
                *g = 255 - *g;
            }
        }
        // About 3× for a few lines of UI text, less for large pictures,
        // never past what the engine accepts.
        let pad = 12usize;
        let longest = (w.max(h) + 2 * pad) as f64;
        let scale = (2400.0 / longest)
            .clamp(1.0, 3.0)
            .min(f64::from(max) / longest);
        let (pw, ph) = (w + 2 * pad, h + 2 * pad);
        let (ow, oh) = (
            ((pw as f64 * scale) as usize).max(1),
            ((ph as f64 * scale) as usize).max(1),
        );
        let at = |x: f64, y: f64| -> f64 {
            // Bilinear sample of the padded picture (margin is white).
            let get = |xi: isize, yi: isize| -> f64 {
                let (sx, sy) = (xi - pad.cast_signed(), yi - pad.cast_signed());
                if sx < 0 || sy < 0 || sx >= w.cast_signed() || sy >= h.cast_signed() {
                    255.0
                } else {
                    f64::from(grey[sy as usize * w + sx as usize])
                }
            };
            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = (x - x0, y - y0);
            let (xi, yi) = (x0 as isize, y0 as isize);
            let top = get(xi, yi) * (1.0 - fx) + get(xi + 1, yi) * fx;
            let bottom = get(xi, yi + 1) * (1.0 - fx) + get(xi + 1, yi + 1) * fx;
            top * (1.0 - fy) + bottom * fy
        };
        let mut out = Vec::with_capacity(ow * oh * 4);
        for y in 0..oh {
            let sy = ((y as f64 + 0.5) / scale - 0.5).max(0.0);
            for x in 0..ow {
                let sx = ((x as f64 + 0.5) / scale - 0.5).max(0.0);
                let v = at(sx, sy).round().clamp(0.0, 255.0) as u8;
                out.extend([v, v, v, 255]);
            }
        }
        BgraImage::from_raw(ow as u32, oh as u32, out)
    }

    fn invert(image: &mut BgraImage) {
        for p in image.as_bytes_mut().as_chunks_mut::<4>().0 {
            p[0] = 255 - p[0];
            p[1] = 255 - p[1];
            p[2] = 255 - p[2];
        }
    }
}

/// Asks where to save the capture, starting in its folder.
pub fn choose(app: &AppHandle, path: &Path) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;
    let video = crate::editor::kind(path) == Some(Kind::Video);
    let extension = path
        .extension()
        .map_or_else(|| "png".to_owned(), |e| e.to_string_lossy().to_lowercase());
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save as")
        .add_filter(extension.to_uppercase(), &[extension.as_str()])
        .set_directory(destination(app, path, video));
    if let Some(name) = path.file_name() {
        dialog = dialog.set_file_name(name.to_string_lossy());
    }
    let mut chosen = dialog.blocking_save_file()?.into_path().ok()?;
    if chosen
        .extension()
        .is_none_or(|e| !e.eq_ignore_ascii_case(&extension))
    {
        chosen.set_extension(&extension);
    }
    Some(chosen)
}

/// Screenshots left waiting by an earlier session (quick actions, pins,
/// confirmation thumbnails) are deleted at start-up.
pub fn clean_pending() {
    let dir = pending_dir();
    for path in [dir.clone(), dir.join("pinned")] {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            if entry.path().is_file() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}
