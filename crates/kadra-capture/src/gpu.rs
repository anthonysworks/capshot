//! Shared Direct3D 11 device.

use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext, ID3D11Multithread,
};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIDevice3};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::core::{Interface, Result};

/// A hardware Direct3D 11 device usable from Direct2D and from the Windows
/// Runtime capture APIs.
#[derive(Debug, Clone)]
pub struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
}

/// Which Direct3D implementation backs a [`Gpu`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuKind {
    /// The graphics card. Required for hardware video encoding, but the
    /// driver alone commits ~50 MB and takes ~150 ms to initialise.
    Hardware,
    /// WARP, the CPU implementation shipped with Windows: under 1 MB and a
    /// few milliseconds to create. Ideal for still screenshots, where every
    /// pixel ends up in system memory anyway.
    Software,
}

impl Gpu {
    /// Creates a hardware device.
    pub fn new() -> Result<Self> {
        Self::with_kind(GpuKind::Hardware)
    }

    pub fn with_kind(kind: GpuKind) -> Result<Self> {
        let levels = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let mut device = None;
        let mut context = None;
        // SAFETY: every out pointer refers to a live local.
        unsafe {
            D3D11CreateDevice(
                None,
                match kind {
                    GpuKind::Hardware => D3D_DRIVER_TYPE_HARDWARE,
                    GpuKind::Software => D3D_DRIVER_TYPE_WARP,
                },
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device: ID3D11Device = device.ok_or_else(windows::core::Error::empty)?;
        let context = context.ok_or_else(windows::core::Error::empty)?;

        // Windows.Graphics.Capture signals frames from its own threads.
        // SAFETY: plain COM calls on valid interfaces.
        unsafe {
            let _ = device
                .cast::<ID3D11Multithread>()?
                .SetMultithreadProtected(true);
        }

        let dxgi: IDXGIDevice = device.cast()?;
        // SAFETY: `dxgi` is a valid DXGI device.
        let winrt_device = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;

        Ok(Self {
            device,
            context,
            winrt_device,
        })
    }

    /// Asks the driver to release memory it keeps for reuse. Call when idle.
    pub fn trim(&self) {
        // SAFETY: plain calls on live interfaces.
        unsafe {
            self.context.ClearState();
            self.context.Flush();
            if let Ok(dxgi) = self.device.cast::<IDXGIDevice3>() {
                dxgi.Trim();
            }
        }
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    pub fn context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    pub fn winrt_device(&self) -> &IDirect3DDevice {
        &self.winrt_device
    }
}
