//! Native selection overlay.
//!
//! The overlay runs on its own thread with a pre-created, hidden window.
//! [`Overlay::capture`] freezes the desktop, shows it full screen and lets the
//! user pick a region, a window, a monitor or a freeform shape. The result is
//! delivered to the handler given to [`Overlay::spawn`].

mod clipboard;
mod indicator;
pub mod layout;
mod preview;
pub mod render;
pub mod selection;
mod window;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kadra_core::{BgraImage, RecordingFormat, Rect};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub use selection::{Mode, Purpose};

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct CaptureRequest {
    /// Initial purpose; the toolbar can switch it.
    pub purpose: Purpose,
    pub mode: Mode,
    pub copy_to_clipboard: bool,
    /// Show a thumbnail in the corner of the screen afterwards.
    pub show_preview: bool,
    /// Initial state of the recording toggles.
    pub format: RecordingFormat,
    pub system_audio: bool,
    pub microphone: bool,
    /// When the user asked for the capture, to measure latency.
    pub requested_at: Instant,
}

#[derive(Debug)]
pub enum CaptureResult {
    Captured {
        /// Identifies the capture in [`Overlay::attach_file`].
        id: u64,
        image: BgraImage,
        copied: bool,
    },
    /// An area to record was selected (desktop coordinates), with the
    /// audio sources chosen on the toolbar.
    RegionSelected {
        region: Rect,
        format: RecordingFormat,
        system_audio: bool,
        microphone: bool,
    },
    /// The stop button of the recording indicator was clicked.
    StopRecording,
    /// The pause/resume button of the recording indicator was clicked.
    TogglePause,
    /// The colour under the cursor was copied as `#RRGGBB`.
    ColorCopied(String),
    Cancelled,
    Failed(String),
}

/// Receives capture results on the overlay thread. Keep it short: hand heavy
/// work (encoding, saving) to another thread.
pub type ResultHandler = Box<dyn FnMut(CaptureResult) + Send + 'static>;

/// Messages sent to the overlay thread.
#[derive(Debug)]
enum Command {
    Capture(CaptureRequest),
    AttachFile {
        capture_id: u64,
        path: PathBuf,
    },
    ShowIndicator {
        region: Rect,
        countdown: Duration,
    },
    IndicatorRunning,
    IndicatorPaused(bool),
    HideIndicator,
    ShowFile {
        image: BgraImage,
        path: PathBuf,
        region: Rect,
    },
}

/// Handle to the overlay thread.
#[derive(Debug)]
pub struct Overlay {
    commands: mpsc::Sender<Command>,
    hwnd: isize,
}

impl Overlay {
    /// Starts the overlay thread and waits until it is ready.
    pub fn spawn(handler: impl FnMut(CaptureResult) + Send + 'static) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (commands, commands_rx) = mpsc::channel();
        let handler: ResultHandler = Box::new(handler);
        std::thread::Builder::new()
            .name("kadra-overlay".into())
            .spawn(move || window::run(ready_tx, commands_rx, handler))
            .map_err(|e| e.to_string())?;
        let hwnd = ready_rx
            .recv()
            .map_err(|_| "overlay thread exited during start-up".to_owned())??;
        Ok(Self { commands, hwnd })
    }

    fn post(&self, msg: u32) {
        // SAFETY: PostMessage is thread-safe; a stale handle only fails.
        let _ = unsafe { PostMessageW(Some(HWND(self.hwnd as *mut _)), msg, WPARAM(0), LPARAM(0)) };
    }

    /// Opens the overlay. Ignored while a capture is already in progress.
    pub fn capture(&self, request: CaptureRequest) {
        self.send(Command::Capture(request));
    }

    /// Tells the preview where capture `capture_id` was saved, so that it
    /// can be opened or dragged into another application.
    pub fn attach_file(&self, capture_id: u64, path: PathBuf) {
        self.send(Command::AttachFile { capture_id, path });
    }

    /// Shows the recording frame and pill around `region` (desktop
    /// coordinates), counting down for `countdown` first. They are
    /// excluded from capture.
    pub fn show_recording_indicator(&self, region: Rect, countdown: Duration) {
        self.send(Command::ShowIndicator { region, countdown });
    }

    /// The recording has actually started: the pill shows the timer.
    pub fn recording_started(&self) {
        self.send(Command::IndicatorRunning);
    }

    pub fn recording_paused(&self, paused: bool) {
        self.send(Command::IndicatorPaused(paused));
    }

    /// Shows the preview for a saved file (e.g. a video and its last
    /// frame) on the monitor of `region`.
    pub fn show_file_preview(&self, image: BgraImage, path: PathBuf, region: Rect) {
        self.send(Command::ShowFile {
            image,
            path,
            region,
        });
    }

    pub fn hide_recording_indicator(&self) {
        self.send(Command::HideIndicator);
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            self.post(window::WM_KADRA_REQUEST);
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.post(window::WM_KADRA_QUIT);
    }
}
