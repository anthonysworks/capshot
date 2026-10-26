//! Floating thumbnail shown after a capture.
//!
//! A small GDI-painted popup in the corner of the screen: clicking opens the
//! saved file, dragging drops it into any application, right click dismisses
//! it. It never takes the keyboard focus and is excluded from captures.

// Win32 message parameters and structure sizes are packed integers.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use kadra_core::{BgraImage, Point, Rect};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, CreateSolidBrush, DIB_RGB_COLORS,
    DeleteObject, EndPaint, FillRect, HGDIOBJ, PAINTSTRUCT, SetDIBitsToDevice,
};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::DROPEFFECT_COPY;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{
    BHID_DataObject, IShellItem, SHCreateItemFromParsingName, SHDoDragDrop, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DROPSHADOW, CreateWindowExW, GetSystemMetrics, HWND_TOPMOST, IDC_HAND, KillTimer,
    LoadCursorW, MA_NOACTIVATE, RegisterClassExW, SM_CXDRAG, SM_CYDRAG, SW_HIDE, SW_SHOWNORMAL,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
    WDA_EXCLUDEFROMCAPTURE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_PAINT,
    WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WNDPROC, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{HSTRING, w};

/// Sent after `TrackMouseEvent(TME_LEAVE)` (defined in the Controls API set).
const WM_MOUSELEAVE: u32 = 0x02A3;

/// How long the preview stays when the mouse is not over it.
const VISIBLE_MS: u32 = 6000;
/// Remaining time once the mouse leaves it.
const AFTER_HOVER_MS: u32 = 2500;
const HIDE_TIMER: usize = 1;

const BACKGROUND: COLORREF = COLORREF(0x0020_2020);
/// Checkerboard behind transparent pixels (freeform captures).
const CHECKER: [u8; 2] = [0x2b, 0x38];

pub const CLASS_NAME: windows::core::PCWSTR = w!("KadraCapturePreview");

pub fn register_class(instance: windows::Win32::Foundation::HINSTANCE, wndproc: WNDPROC) -> bool {
    // SAFETY: the class structure is fully initialised.
    unsafe {
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DROPSHADOW,
            lpfnWndProc: wndproc,
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&class) != 0
    }
}

pub fn create_window(
    instance: windows::Win32::Foundation::HINSTANCE,
) -> windows::core::Result<HWND> {
    // SAFETY: the class was registered by `register_class`.
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("Kadra preview"),
            WS_POPUP,
            0,
            0,
            8,
            8,
            None,
            None,
            Some(instance),
            None,
        )?;
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&round).cast::<c_void>(),
            size_of_val(&round) as u32,
        );
        // Never appear in the next screenshot or recording.
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        Ok(hwnd)
    }
}

/// Flattens transparency over a checkerboard so the thumbnail can be painted
/// with plain GDI.
fn flatten(image: &mut BgraImage) {
    let width = image.width() as usize;
    for (i, px) in image
        .as_bytes_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
    {
        let (x, y) = (i % width, i / width);
        let bg = u32::from(CHECKER[(x / 8 + y / 8) % 2]);
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * a + bg * (255 - a) + 127) / 255) as u8;
        }
        px[3] = 255;
    }
}

#[derive(Debug)]
pub struct Preview {
    hwnd: HWND,
    thumbnail: Option<BgraImage>,
    capture_id: u64,
    file: Option<PathBuf>,
    hovered: bool,
    pressed_at: Option<Point>,
    padding: i32,
}

fn lparam_point(lparam: LPARAM) -> Point {
    Point::new(
        i32::from(lparam.0 as u32 as u16 as i16),
        i32::from((lparam.0 as u32 >> 16) as u16 as i16),
    )
}

impl Preview {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            thumbnail: None,
            capture_id: 0,
            file: None,
            hovered: false,
            pressed_at: None,
            padding: 6,
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Shows `image` in the bottom-right corner of `work_area` (desktop
    /// coordinates).
    pub fn show(&mut self, capture_id: u64, image: &BgraImage, work_area: Rect, scale: f32) {
        let px = |v: f32| (v * scale).round() as i32;
        let mut thumbnail = image.thumbnail(px(300.0) as u32, px(190.0) as u32);
        flatten(&mut thumbnail);

        self.padding = px(6.0);
        let width = thumbnail.width() as i32 + 2 * self.padding;
        let height = thumbnail.height() as i32 + 2 * self.padding;
        let margin = px(16.0);
        let x = work_area.right - margin - width;
        let y = work_area.bottom - margin - height;

        self.thumbnail = Some(thumbnail);
        self.capture_id = capture_id;
        self.file = None;
        self.hovered = false;
        self.pressed_at = None;

        // SAFETY: our own window, on its thread.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            SetTimer(Some(self.hwnd), HIDE_TIMER, VISIBLE_MS, None);
        }
    }

    /// Associates the saved file with the capture currently shown.
    pub fn attach_file(&mut self, capture_id: u64, path: PathBuf) {
        if self.thumbnail.is_some() && capture_id == self.capture_id {
            self.file = Some(path);
        }
    }

    pub fn hide(&mut self) {
        if self.thumbnail.take().is_none() {
            return;
        }
        self.file = None;
        self.pressed_at = None;
        // SAFETY: our own window, on its thread.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), HIDE_TIMER);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    pub fn handle(&mut self, msg: u32, _wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_PAINT => self.paint(),
            WM_MOUSEACTIVATE => return Some(LRESULT(MA_NOACTIVATE as isize)),
            WM_MOUSEMOVE => self.on_move(lparam_point(lparam)),
            WM_MOUSELEAVE => {
                self.hovered = false;
                self.pressed_at = None;
                // SAFETY: our own window.
                unsafe { SetTimer(Some(self.hwnd), HIDE_TIMER, AFTER_HOVER_MS, None) };
            }
            WM_LBUTTONDOWN => self.pressed_at = Some(lparam_point(lparam)),
            WM_LBUTTONUP => {
                if self.pressed_at.take().is_some() {
                    if let Some(file) = &self.file {
                        open(file);
                    }
                    self.hide();
                }
            }
            // Timeout or right click.
            WM_TIMER | WM_RBUTTONUP => self.hide(),
            _ => return None,
        }
        Some(LRESULT(0))
    }

    fn on_move(&mut self, p: Point) {
        if !self.hovered {
            self.hovered = true;
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: valid structure for our own window.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), HIDE_TIMER);
                let _ = TrackMouseEvent(&mut track);
            }
        }
        let Some(start) = self.pressed_at else {
            return;
        };
        // SAFETY: plain metrics queries.
        let (dx, dy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
        if (p.x - start.x).abs() > dx || (p.y - start.y).abs() > dy {
            self.pressed_at = None;
            if let Some(file) = self.file.clone() {
                match drag(self.hwnd, &file) {
                    Ok(()) => self.hide(),
                    Err(err) => tracing::warn!("drag and drop failed: {err}"),
                }
            }
        }
    }

    fn paint(&self) {
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: standard paint cycle on our own window; GDI objects are
        // released before returning.
        unsafe {
            let hdc = BeginPaint(self.hwnd, &mut ps);
            let brush = CreateSolidBrush(BACKGROUND);
            FillRect(hdc, &ps.rcPaint, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
            if let Some(thumb) = &self.thumbnail {
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: thumb.width() as i32,
                        biHeight: -(thumb.height() as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                SetDIBitsToDevice(
                    hdc,
                    self.padding,
                    self.padding,
                    thumb.width(),
                    thumb.height(),
                    0,
                    0,
                    0,
                    thumb.height(),
                    thumb.as_bytes().as_ptr().cast(),
                    &info,
                    DIB_RGB_COLORS,
                );
            }
            let _ = EndPaint(self.hwnd, &ps);
        }
    }
}

/// Opens a file with its default application.
fn open(file: &Path) {
    // SAFETY: valid strings for the duration of the call.
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(file.as_os_str()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

/// Starts a shell drag and drop of `file` (with the standard drag image).
fn drag(hwnd: HWND, file: &Path) -> windows::core::Result<()> {
    // SAFETY: COM calls on the window's STA thread with live objects.
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(file.as_os_str()), None)?;
        let data: IDataObject = item.BindToHandler(None, &BHID_DataObject)?;
        SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY)?;
    }
    Ok(())
}
