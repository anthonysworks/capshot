//! Visible top-level windows, used for click-to-capture-window.

use std::ffi::c_void;

use capshot_core::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowLongPtrW, GetWindowTextLengthW,
    GetWindowTextW, IsIconic, IsWindowVisible, WS_EX_TRANSPARENT,
};
use windows::core::BOOL;

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub hwnd: HWND,
    /// Visible bounds, without the invisible resize borders.
    pub rect: Rect,
    pub title: String,
}

/// Lists visible windows from front to back, in desktop coordinates.
pub fn enumerate(exclude: Option<HWND>) -> Vec<WindowInfo> {
    unsafe extern "system" fn callback(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<HWND>` passed below.
        let handles = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
        handles.push(hwnd);
        TRUE
    }

    let mut handles: Vec<HWND> = Vec::with_capacity(256);
    // SAFETY: the callback only touches `handles`, which outlives the call.
    let _ = unsafe {
        EnumWindows(
            Some(callback),
            LPARAM(std::ptr::from_mut(&mut handles) as isize),
        )
    };

    handles
        .into_iter()
        .filter(|&h| Some(h) != exclude)
        .filter_map(describe)
        .collect()
}

#[allow(clippy::cast_possible_truncation)]
fn describe(hwnd: HWND) -> Option<WindowInfo> {
    // SAFETY: all calls below accept any window handle, valid or stale.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }

        // Click-through overlays (game/GPU overlays, screen dimmers…).
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TRANSPARENT.0 != 0 {
            return None;
        }

        // Windows on other virtual desktops or suspended UWP apps.
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            std::ptr::from_mut(&mut cloaked).cast::<c_void>(),
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
        {
            return None;
        }

        let mut bounds = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            std::ptr::from_mut(&mut bounds).cast::<c_void>(),
            size_of::<RECT>() as u32,
        )
        .ok()?;
        let rect = Rect::new(bounds.left, bounds.top, bounds.right, bounds.bottom);
        if rect.width() < 2 || rect.height() < 2 {
            return None;
        }

        // The desktop is handled as "whole monitor" by the overlay.
        let mut class = [0u16; 32];
        let len = usize::try_from(GetClassNameW(hwnd, &mut class)).unwrap_or(0);
        let class = String::from_utf16_lossy(&class[..len]);
        if class == "Progman" || class == "WorkerW" {
            return None;
        }

        let title_len = usize::try_from(GetWindowTextLengthW(hwnd)).unwrap_or(0);
        let mut title = vec![0u16; title_len + 1];
        let copied = usize::try_from(GetWindowTextW(hwnd, &mut title)).unwrap_or(0);

        Some(WindowInfo {
            hwnd,
            rect,
            title: String::from_utf16_lossy(&title[..copied]),
        })
    }
}
