//! Continuous monitor capture into a GPU texture.
//!
//! Windows.Graphics.Capture only delivers a frame when the screen changes.
//! Each new frame is copied (on the GPU) into `latest`, which the pacer reads
//! at a constant rate: an unchanged screen simply repeats the last frame.
//!
//! No extra lock is needed: the copy and the conversion are both commands
//! on the immediate context, which `ID3D11Multithread` protection
//! serialises, so the converter always reads a complete frame. Holding a
//! lock of our own across D3D calls on two threads could deadlock with
//! that internal lock.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use capshot_capture::Gpu;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, ID3D11Texture2D,
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
    frames: AtomicU64,
}

// SAFETY: D3D11 textures are free-threaded objects; access to the device
// context is serialised by `ID3D11Multithread` protection.
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

    /// Copies `area` of the latest frame to system memory.
    pub fn read_area(&self, gpu: &Gpu, area: capshot_core::Rect) -> Result<capshot_core::BgraImage> {
        AreaReader::new(gpu, area)?.read(self, gpu)
    }

    /// The texture holding the most recent frame.
    pub fn latest(&self) -> &ID3D11Texture2D {
        &self.shared.latest
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// Reads one area of the latest frame to system memory, reusing its
/// staging texture from one frame to the next.
pub struct AreaReader {
    staging: ID3D11Texture2D,
    area: capshot_core::Rect,
}

impl AreaReader {
    pub fn new(gpu: &Gpu, area: capshot_core::Rect) -> Result<Self> {
        #[allow(clippy::cast_sign_loss)]
        let desc = D3D11_TEXTURE2D_DESC {
            Width: area.width() as u32,
            Height: area.height() as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        // SAFETY: valid description and out pointer.
        unsafe {
            gpu.device()
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
        }
        Ok(Self {
            staging: staging.ok_or_else(windows::core::Error::empty)?,
            area,
        })
    }

    pub fn read(&self, source: &Source, gpu: &Gpu) -> Result<capshot_core::BgraImage> {
        let area = self.area;
        #[allow(clippy::cast_sign_loss)]
        let (w, h) = (area.width() as u32, area.height() as u32);
        #[allow(clippy::cast_sign_loss)]
        let region = D3D11_BOX {
            left: area.left as u32,
            top: area.top as u32,
            front: 0,
            right: area.right as u32,
            bottom: area.bottom as u32,
            back: 1,
        };
        // SAFETY: the staging texture matches the copied box; the mapping
        // is released before returning.
        unsafe {
            let context = gpu.context();
            context.CopySubresourceRegion(
                &self.staging,
                0,
                0,
                0,
                0,
                &source.shared.latest,
                0,
                Some(&region),
            );
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let data = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), pitch * h as usize);
            let mut image = capshot_core::BgraImage::new(w, h);
            image.blit(data, pitch, w, h, capshot_core::Point::new(0, 0));
            context.Unmap(&self.staging, 0);
            // Captured frames carry no meaningful alpha.
            for px in image.as_bytes_mut().as_chunks_mut::<4>().0 {
                px[3] = 255;
            }
            Ok(image)
        }
    }
}
