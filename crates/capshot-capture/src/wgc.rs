//! Windows.Graphics.Capture backend.
//!
//! Creating a capture item, frame pool and session costs tens of
//! milliseconds, while starting a session that already exists is nearly free
//! and DWM then delivers a frame on its next composition. Sessions are
//! therefore created ahead of time ("armed") and only started when a
//! screenshot is requested. An armed session that has not been started does
//! no work at all.

use std::sync::mpsc;
use std::time::Duration;

use capshot_core::{BgraImage, Point, Rect};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess,
    GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Texture2D,
};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{IInspectable, Interface};

use crate::gpu::Gpu;
use crate::monitors::Monitor;
use crate::{CaptureError, Result};

/// Longest wait for the first frame of a monitor. DWM normally delivers it
/// on the next composition, i.e. within one refresh interval.
const FRAME_TIMEOUT: Duration = Duration::from_millis(500);

pub fn is_supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

/// Asks once for permission to hide the yellow capture border. Unpackaged
/// desktop apps are granted it silently on Windows 11.
///
/// Runs on a short-lived multithreaded-apartment thread: blocking on an
/// asynchronous operation from a single-threaded apartment could deadlock.
pub fn request_borderless_access() {
    let worker = std::thread::spawn(|| {
        // SAFETY: initialises the Windows Runtime for this new thread only.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }?;
        GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)?.join()
    });
    match worker.join() {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => tracing::debug!("borderless capture not granted: {err}"),
        Err(_) => tracing::debug!("borderless capture request panicked"),
    }
}

/// A capture session ready to start.
struct ArmedSession {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    frames: mpsc::Receiver<Direct3D11CaptureFrame>,
    monitor: Monitor,
}

impl Drop for ArmedSession {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// One armed session per monitor, valid for a given display layout.
pub struct Armed {
    layout: Vec<Rect>,
    sessions: Vec<ArmedSession>,
}

impl std::fmt::Debug for Armed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Armed")
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}

fn layout_of(monitors: &[Monitor]) -> Vec<Rect> {
    monitors.iter().map(|m| m.rect).collect()
}

impl Armed {
    pub fn new(gpu: &Gpu, monitors: &[Monitor]) -> Result<Self> {
        Ok(Self {
            layout: layout_of(monitors),
            sessions: monitors
                .iter()
                .map(|m| arm(gpu, m))
                .collect::<Result<_>>()?,
        })
    }

    /// Whether these sessions still match the current monitors.
    pub fn matches(&self, monitors: &[Monitor]) -> bool {
        self.layout == layout_of(monitors)
    }

    /// Starts every session and copies one frame of each monitor into
    /// `image` (whose top-left is `origin`). Consumes the sessions: they
    /// cannot be started twice.
    pub fn capture(self, gpu: &Gpu, origin: Point, image: &mut BgraImage) -> Result<()> {
        // Start them all before waiting so monitors are captured in parallel.
        for s in &self.sessions {
            s.session.StartCapture()?;
        }
        for s in &self.sessions {
            let frame = s
                .frames
                .recv_timeout(FRAME_TIMEOUT)
                .map_err(|_| CaptureError::Timeout(s.monitor.name.clone()))?;
            let at = Point::new(
                s.monitor.rect.left - origin.x,
                s.monitor.rect.top - origin.y,
            );
            let copied = read_frame(gpu, &frame, at, image);
            let _ = frame.Close();
            copied?;
        }
        Ok(())
    }
}

fn arm(gpu: &Gpu, monitor: &Monitor) -> Result<ArmedSession> {
    let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    // SAFETY: `monitor.handle` comes from EnumDisplayMonitors.
    let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor.handle)? };

    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        gpu.winrt_device(),
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        1,
        item.Size()?,
    )?;

    let (tx, rx) = mpsc::sync_channel(1);
    pool.FrameArrived(
        &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
            if let Some(pool) = pool.as_ref()
                && let Ok(frame) = pool.TryGetNextFrame()
            {
                // Only the first frame matters; later ones are dropped.
                let _ = tx.try_send(frame);
            }
            Ok(())
        }),
    )?;

    let session = pool.CreateCaptureSession(&item)?;
    session.SetIsCursorCaptureEnabled(false)?;
    // Fails when borderless access was not granted; the border then only
    // flashes for the few milliseconds the capture lasts.
    let _ = session.SetIsBorderRequired(false);

    Ok(ArmedSession {
        pool,
        session,
        frames: rx,
        monitor: monitor.clone(),
    })
}

/// Copies a frame to the CPU and blits it into `image` at `at`.
fn read_frame(
    gpu: &Gpu,
    frame: &Direct3D11CaptureFrame,
    at: Point,
    image: &mut BgraImage,
) -> Result<()> {
    let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
    // SAFETY: the frame surface is a D3D11 texture created on `gpu`.
    let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };

    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid out pointer.
    unsafe { texture.GetDesc(&mut desc) };
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    #[allow(clippy::cast_sign_loss)]
    {
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    }
    desc.MiscFlags = 0;

    let mut staging = None;
    // SAFETY: `desc` describes a valid staging texture; out pointer is local.
    unsafe {
        gpu.device()
            .CreateTexture2D(&desc, None, Some(&mut staging))?;
    }
    let staging = staging.ok_or_else(windows::core::Error::empty)?;

    let context = gpu.context();
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: both textures share size and format; the mapping is released
    // before `staging` is dropped.
    unsafe {
        context.CopyResource(&staging, &texture);
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let len = mapped.RowPitch as usize * desc.Height as usize;
        let data = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), len);
        image.blit(data, mapped.RowPitch as usize, desc.Width, desc.Height, at);
        context.Unmap(&staging, 0);
    }
    Ok(())
}
