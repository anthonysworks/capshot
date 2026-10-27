//! Continuous monitor capture into a GPU texture.
//!
//! Windows.Graphics.Capture only delivers a frame when the screen changes.
//! Each new frame is copied (on the GPU) into `latest`, which the pacer reads
//! at a constant rate: an unchanged screen simply repeats the last frame.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use kadra_capture::Gpu;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface, Result};

/// Frame buffers in the capture pool: one being filled while another is
/// copied keeps capture from stalling.
const POOL_BUFFERS: i32 = 2;

pub struct Source {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    shared: Arc<Shared>,
    width: u32,
    height: u32,
}

struct Shared {
    latest: ID3D11Texture2D,
    /// Serialises copies into `latest` with reads by the pacer.
    lock: Mutex<()>,
    frames: AtomicU64,
}

// SAFETY: D3D11 textures are free-threaded objects; access to the device
// context is serialised by `ID3D11Multithread` protection and `lock`.
unsafe impl Send for Shared {}
// SAFETY: see above.
unsafe impl Sync for Shared {}

impl Source {
    pub fn start(gpu: &Gpu, monitor: HMONITOR, fps: u32, cursor: bool) -> Result<Self> {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        // SAFETY: `monitor` is a valid monitor handle.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor)? };
        let size = item.Size()?;
        #[allow(clippy::cast_sign_loss)]
        let (width, height) = (size.Width as u32, size.Height as u32);

        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            #[allow(clippy::cast_sign_loss)]
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut latest = None;
        // SAFETY: valid description and out pointer.
        unsafe {
            gpu.device()
                .CreateTexture2D(&desc, None, Some(&mut latest))?;
        }
        let shared = Arc::new(Shared {
            latest: latest.ok_or_else(windows::core::Error::empty)?,
            lock: Mutex::new(()),
            frames: AtomicU64::new(0),
        });

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            gpu.winrt_device(),
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            POOL_BUFFERS,
            size,
        )?;

        let context = gpu.context().clone();
        let target = Arc::clone(&shared);
        pool.FrameArrived(
            &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
                let Some(pool) = pool.as_ref() else {
                    return Ok(());
                };
                let frame = pool.TryGetNextFrame()?;
                let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
                // SAFETY: the surface is a texture of our device; the copy
                // is serialised with the pacer's reads.
                unsafe {
                    let texture: ID3D11Texture2D = access.GetInterface()?;
                    let _guard = target.lock.lock().unwrap_or_else(PoisonError::into_inner);
                    context.CopyResource(&target.latest, &texture);
                }
                target.frames.fetch_add(1, Ordering::Relaxed);
                frame.Close()
            }),
        )?;

        let session = pool.CreateCaptureSession(&item)?;
        session.SetIsCursorCaptureEnabled(cursor)?;
        let _ = session.SetIsBorderRequired(false);
        // Windows 11 24H2+: no need for more frames than we encode.
        if fps > 0 {
            let _ = session.SetMinUpdateInterval(TimeSpan {
                Duration: 10_000_000 / i64::from(fps),
            });
        }
        session.StartCapture()?;

        Ok(Self {
            pool,
            session,
            shared,
            width,
            height,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Frames delivered by the system so far.
    pub fn frames(&self) -> u64 {
        self.shared.frames.load(Ordering::Relaxed)
    }

    /// Runs `f` with the latest frame, preventing concurrent updates.
    pub fn with_latest<T>(&self, f: impl FnOnce(&ID3D11Texture2D) -> T) -> T {
        let _guard = self
            .shared
            .lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        f(&self.shared.latest)
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
