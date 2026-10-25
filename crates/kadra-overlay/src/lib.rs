//! Native selection overlay.
//!
//! The overlay runs on its own thread with a pre-created, hidden window.
//! [`Overlay::capture`] freezes the desktop, shows it full screen and lets the
//! user pick a region, a window, a monitor or a freeform shape. The result is
//! delivered to the handler given to [`Overlay::spawn`].

mod clipboard;
pub mod layout;
pub mod render;
pub mod selection;
mod window;

use std::sync::mpsc;
use std::time::Instant;

use kadra_core::BgraImage;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub use selection::Mode;

#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub mode: Mode,
    pub copy_to_clipboard: bool,
    /// When the user asked for the capture, to measure latency.
    pub requested_at: Instant,
}

#[derive(Debug)]
pub enum CaptureResult {
    Captured {
        image: BgraImage,
        copied: bool,
    },
    /// The colour under the cursor was copied as `#RRGGBB`.
    ColorCopied(String),
    Cancelled,
    Failed(String),
}

/// Receives capture results on the overlay thread. Keep it short: hand heavy
/// work (encoding, saving) to another thread.
pub type ResultHandler = Box<dyn FnMut(CaptureResult) + Send + 'static>;

/// Handle to the overlay thread.
#[derive(Debug)]
pub struct Overlay {
    requests: mpsc::Sender<CaptureRequest>,
    hwnd: isize,
}

impl Overlay {
    /// Starts the overlay thread and waits until it is ready.
    pub fn spawn(handler: impl FnMut(CaptureResult) + Send + 'static) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (requests, requests_rx) = mpsc::channel();
        let handler: ResultHandler = Box::new(handler);
        std::thread::Builder::new()
            .name("kadra-overlay".into())
            .spawn(move || window::run(ready_tx, requests_rx, handler))
            .map_err(|e| e.to_string())?;
        let hwnd = ready_rx
            .recv()
            .map_err(|_| "overlay thread exited during start-up".to_owned())??;
        Ok(Self { requests, hwnd })
    }

    fn post(&self, msg: u32) {
        // SAFETY: PostMessage is thread-safe; a stale handle only fails.
        let _ = unsafe { PostMessageW(Some(HWND(self.hwnd as *mut _)), msg, WPARAM(0), LPARAM(0)) };
    }

    /// Opens the overlay. Ignored while a capture is already in progress.
    pub fn capture(&self, request: CaptureRequest) {
        if self.requests.send(request).is_ok() {
            self.post(window::WM_KADRA_REQUEST);
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.post(window::WM_KADRA_QUIT);
    }
}
