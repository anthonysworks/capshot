//! Screen capture for Kadra.
//!
//! [`Capturer::snapshot`] freezes the whole virtual desktop in a single CPU
//! image together with the monitor layout and the visible windows, which is
//! everything the selection overlay needs.
//!
//! Capture uses Windows.Graphics.Capture (no cursor, no yellow border) and
//! falls back to GDI if it is unavailable or fails.
//!
//! The process must be per-monitor DPI aware (v2) so that every coordinate is
//! in physical pixels; Tauri enables this at start-up.

mod gdi;
mod gpu;
mod monitors;
mod wgc;
mod windows_list;

use std::cell::RefCell;
use std::time::Instant;

use kadra_core::{BgraImage, Point, Rect};
use windows::Win32::Foundation::HWND;

pub use gpu::Gpu;
pub use monitors::Monitor;
pub use windows_list::WindowInfo;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("Windows API call failed: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("no monitor found")]
    NoMonitor,
    #[error("timed out waiting for a frame from {0}")]
    Timeout(String),
}

pub type Result<T> = std::result::Result<T, CaptureError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    GraphicsCapture,
    Gdi,
}

/// A frozen picture of the desktop.
///
/// All rectangles are relative to the image, whose top-left corner is the
/// top-left corner of the virtual screen (`origin` in desktop coordinates).
#[derive(Debug)]
pub struct Snapshot {
    pub origin: Point,
    pub image: BgraImage,
    pub monitors: Vec<Monitor>,
    /// Visible top-level windows, front to back.
    pub windows: Vec<WindowInfo>,
    pub backend: Backend,
}

impl Snapshot {
    pub fn bounds(&self) -> Rect {
        self.image.bounds()
    }

    /// Monitor containing `p`, or the primary monitor.
    pub fn monitor_at(&self, p: Point) -> &Monitor {
        self.monitors
            .iter()
            .find(|m| m.rect.contains(p))
            .or_else(|| self.monitors.iter().find(|m| m.primary))
            .unwrap_or(&self.monitors[0])
    }

    /// Front-most window under `p`.
    pub fn window_at(&self, p: Point) -> Option<&WindowInfo> {
        self.windows.iter().find(|w| w.rect.contains(p))
    }
}

/// Takes screenshots. Create it once and reuse it from the same thread:
/// device and session creation are the expensive parts.
#[derive(Debug)]
pub struct Capturer {
    gpu: Gpu,
    wgc: bool,
    armed: RefCell<Option<wgc::Armed>>,
}

impl Capturer {
    /// Must be called on a thread where the Windows Runtime is initialised.
    pub fn new(gpu: Gpu) -> Self {
        // `KADRA_CAPTURE_BACKEND=gdi` forces the fallback, for troubleshooting.
        let forced_gdi =
            std::env::var("KADRA_CAPTURE_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("gdi"));
        let wgc = !forced_gdi && wgc::is_supported();
        if wgc {
            wgc::request_borderless_access();
        } else {
            tracing::warn!("Windows.Graphics.Capture unavailable, using GDI");
        }
        let capturer = Self {
            gpu,
            wgc,
            armed: RefCell::new(None),
        };
        capturer.prepare();
        capturer
    }

    /// Prepares the next capture so that [`Self::snapshot`] returns as fast
    /// as possible. Call it when idle, e.g. right after showing a snapshot.
    pub fn prepare(&self) {
        if !self.wgc {
            return;
        }
        let armed = monitors::enumerate().and_then(|m| wgc::Armed::new(&self.gpu, &m));
        match armed {
            Ok(armed) => *self.armed.borrow_mut() = Some(armed),
            Err(err) => tracing::warn!("cannot prepare capture sessions: {err}"),
        }
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// Captures every monitor. `exclude` is left out of the window list
    /// (typically the overlay itself).
    pub fn snapshot(&self, exclude: Option<HWND>) -> Result<Snapshot> {
        let start = Instant::now();
        let desktop_monitors = monitors::enumerate()?;
        let virtual_rect = desktop_monitors
            .iter()
            .fold(desktop_monitors[0].rect, |r, m| r.union(m.rect));
        let origin = Point::new(virtual_rect.left, virtual_rect.top);

        // Areas of the virtual screen not covered by a monitor stay transparent.
        #[allow(clippy::cast_sign_loss)]
        let mut image =
            BgraImage::zeroed(virtual_rect.width() as u32, virtual_rect.height() as u32);

        let backend = if self.wgc {
            let armed = self
                .armed
                .take()
                .filter(|a| a.matches(&desktop_monitors))
                .map_or_else(|| wgc::Armed::new(&self.gpu, &desktop_monitors), Ok);
            match armed.and_then(|a| a.capture(&self.gpu, origin, &mut image)) {
                Ok(()) => Backend::GraphicsCapture,
                Err(err) => {
                    tracing::warn!("Windows.Graphics.Capture failed ({err}), using GDI");
                    gdi::capture(virtual_rect, &mut image)?;
                    Backend::Gdi
                }
            }
        } else {
            gdi::capture(virtual_rect, &mut image)?;
            Backend::Gdi
        };

        let to_local = |r: Rect| r.offset(-origin.x, -origin.y);
        let monitors = desktop_monitors
            .into_iter()
            .map(|m| Monitor {
                rect: to_local(m.rect),
                work: to_local(m.work),
                ..m
            })
            .collect();
        let windows = windows_list::enumerate(exclude)
            .into_iter()
            .map(|w| WindowInfo {
                rect: to_local(w.rect).intersect(image.bounds()),
                ..w
            })
            .filter(|w| !w.rect.is_empty())
            .collect();

        tracing::debug!(
            ?backend,
            width = image.width(),
            height = image.height(),
            elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
            "desktop captured"
        );

        Ok(Snapshot {
            origin,
            image,
            monitors,
            windows,
            backend,
        })
    }
}
