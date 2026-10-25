//! Display enumeration.

use kadra_core::Rect;
use windows::Win32::Foundation::{LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::BOOL;

use crate::{CaptureError, Result};

#[derive(Debug, Clone)]
pub struct Monitor {
    pub handle: HMONITOR,
    /// Full monitor area.
    pub rect: Rect,
    /// Area not covered by the taskbar.
    pub work: Rect,
    /// Effective DPI (96 = 100 % scaling).
    pub dpi: u32,
    pub primary: bool,
    /// Device name such as `\\.\DISPLAY1`.
    pub name: String,
}

impl Monitor {
    /// UI scale factor for this monitor (1.0 at 96 DPI).
    #[allow(clippy::cast_precision_loss)]
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
}

fn to_rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

pub fn enumerate() -> Result<Vec<Monitor>> {
    unsafe extern "system" fn callback(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<HMONITOR>` passed below, alive for
        // the whole enumeration.
        let handles = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        handles.push(monitor);
        TRUE
    }

    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only touches `handles`, which outlives the call.
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(callback),
            LPARAM(std::ptr::from_mut(&mut handles) as isize),
        )
        .ok()?;
    }

    let monitors: Vec<Monitor> = handles.into_iter().filter_map(describe).collect();
    if monitors.is_empty() {
        return Err(CaptureError::NoMonitor);
    }
    Ok(monitors)
}

fn describe(handle: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            #[allow(clippy::cast_possible_truncation)]
            cbSize: size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `info` is a correctly sized MONITORINFOEXW.
    unsafe { GetMonitorInfoW(handle, std::ptr::from_mut(&mut info).cast()) }
        .ok()
        .ok()?;

    let (mut dpi_x, mut dpi_y) = (96, 96);
    // SAFETY: out pointers refer to locals.
    if unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_err() {
        dpi_x = 96;
    }

    let name_len = info
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szDevice.len());

    Some(Monitor {
        handle,
        rect: to_rect(info.monitorInfo.rcMonitor),
        work: to_rect(info.monitorInfo.rcWork),
        dpi: dpi_x,
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        name: String::from_utf16_lossy(&info.szDevice[..name_len]),
    })
}
