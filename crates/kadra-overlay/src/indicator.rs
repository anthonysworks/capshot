//! On-screen indicator while recording: a red frame around the recorded
//! area and a small pill with the elapsed time and a stop button.
//!
//! Every window is excluded from capture, so none of it appears in the
//! video, and the frame lets mouse clicks through.

// Win32 message parameters and structure sizes are packed integers.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::ffi::c_void;
use std::time::Instant;

use kadra_core::{Point, Rect};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CreateFontW, CreateSolidBrush, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, DeleteObject, DrawTextW, Ellipse, EndPaint, FW_SEMIBOLD, FillRect, HGDIOBJ,
    InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, HWND_TOPMOST, IDC_ARROW, KillTimer, LWA_ALPHA, LoadCursorW, MA_NOACTIVATE,
    RegisterClassExW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes,
    SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE,
    WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_PAINT, WM_TIMER, WNDCLASSEXW, WNDPROC, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const PILL_CLASS: PCWSTR = w!("KadraRecordingPill");
const FRAME_CLASS: PCWSTR = w!("KadraRecordingFrame");
const TICK_TIMER: usize = 1;

const RED: COLORREF = COLORREF(0x004d_48e5); // #E5484D (BGR)
const PANEL: COLORREF = COLORREF(0x0020_2020);
const WHITE: COLORREF = COLORREF(0x00ff_ffff);
const STOP_HOVERLESS: COLORREF = COLORREF(0x0040_4040);

/// What a click on the indicator asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Stop,
}

pub fn register_classes(instance: HINSTANCE, wndproc: WNDPROC) -> bool {
    // SAFETY: the class structures are fully initialised.
    unsafe {
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let pill = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: wndproc,
            hInstance: instance,
            hCursor: cursor,
            lpszClassName: PILL_CLASS,
            ..Default::default()
        };
        let frame = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: wndproc,
            hInstance: instance,
            lpszClassName: FRAME_CLASS,
            hbrBackground: CreateSolidBrush(RED),
            ..Default::default()
        };
        RegisterClassExW(&pill) != 0 && RegisterClassExW(&frame) != 0
    }
}

fn create(instance: HINSTANCE, class: PCWSTR, ex_style: u32) -> windows::core::Result<HWND> {
    // SAFETY: the classes were registered by `register_classes`.
    unsafe {
        let hwnd = CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(
                (WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE).0 | ex_style,
            ),
            class,
            w!("Kadra recording"),
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
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        Ok(hwnd)
    }
}

#[derive(Debug)]
pub struct Indicator {
    pill: HWND,
    /// Top, bottom, left, right edges of the frame.
    edges: [HWND; 4],
    started: Option<Instant>,
    stop_button: Rect,
    scale: f32,
}

impl Indicator {
    pub fn new(instance: HINSTANCE) -> windows::core::Result<Self> {
        let pill = create(instance, PILL_CLASS, 0)?;
        let round = DWMWCP_ROUND;
        // SAFETY: valid attribute value for our window.
        unsafe {
            let _ = DwmSetWindowAttribute(
                pill,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                std::ptr::from_ref(&round).cast::<c_void>(),
                size_of_val(&round) as u32,
            );
        }
        let edge = || {
            let hwnd = create(instance, FRAME_CLASS, (WS_EX_LAYERED | WS_EX_TRANSPARENT).0)?;
            // SAFETY: layered window owned by this thread.
            unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 230, LWA_ALPHA)? };
            Ok::<_, windows::core::Error>(hwnd)
        };
        Ok(Self {
            pill,
            edges: [edge()?, edge()?, edge()?, edge()?],
            started: None,
            stop_button: Rect::default(),
            scale: 1.0,
        })
    }

    pub fn owns(&self, hwnd: HWND) -> bool {
        hwnd == self.pill || self.edges.contains(&hwnd)
    }

    /// Shows the frame around `region` and the pill next to it, both in
    /// desktop coordinates; `monitor` keeps the pill on screen.
    #[allow(clippy::many_single_char_names)]
    pub fn show(&mut self, region: Rect, monitor: Rect, scale: f32) {
        let px = |v: f32| (v * scale).round() as i32;
        let t = px(2.0).max(1);
        let edges = [
            Rect::new(
                region.left - t,
                region.top - t,
                region.right + t,
                region.top,
            ),
            Rect::new(
                region.left - t,
                region.bottom,
                region.right + t,
                region.bottom + t,
            ),
            Rect::new(region.left - t, region.top, region.left, region.bottom),
            Rect::new(region.right, region.top, region.right + t, region.bottom),
        ];

        let (w, h) = (px(132.0), px(36.0));
        let gap = px(8.0);
        let x = (region.left + (region.width() - w) / 2).clamp(monitor.left, monitor.right - w);
        let y = if region.bottom + gap + h <= monitor.bottom {
            region.bottom + gap
        } else if region.top - gap - h >= monitor.top {
            region.top - gap - h
        } else {
            region.bottom - gap - h
        };
        let side = h - px(10.0);
        self.stop_button = Rect::from_size(w - px(5.0) - side, px(5.0), side, side);
        self.scale = scale;
        self.started = Some(Instant::now());

        // SAFETY: windows owned by this thread.
        unsafe {
            for (hwnd, r) in self.edges.iter().zip(edges) {
                let _ = SetWindowPos(
                    *hwnd,
                    Some(HWND_TOPMOST),
                    r.left,
                    r.top,
                    r.width(),
                    r.height(),
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
            let _ = SetWindowPos(
                self.pill,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            SetTimer(Some(self.pill), TICK_TIMER, 250, None);
        }
    }

    pub fn hide(&mut self) {
        self.started = None;
        // SAFETY: windows owned by this thread.
        unsafe {
            let _ = KillTimer(Some(self.pill), TICK_TIMER);
            let _ = ShowWindow(self.pill, SW_HIDE);
            for hwnd in self.edges {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    /// Handles a message for one of the indicator windows.
    pub fn handle(
        &mut self,
        hwnd: HWND,
        msg: u32,
        lparam: LPARAM,
    ) -> Option<(LRESULT, Option<Request>)> {
        match msg {
            WM_MOUSEACTIVATE => Some((LRESULT(MA_NOACTIVATE as isize), None)),
            WM_PAINT if hwnd == self.pill => {
                self.paint();
                Some((LRESULT(0), None))
            }
            WM_TIMER if hwnd == self.pill => {
                // SAFETY: our own window.
                let _ = unsafe { InvalidateRect(Some(self.pill), None, false) };
                Some((LRESULT(0), None))
            }
            WM_LBUTTONUP if hwnd == self.pill => {
                let p = Point::new(
                    i32::from(lparam.0 as u32 as u16 as i16),
                    i32::from((lparam.0 as u32 >> 16) as u16 as i16),
                );
                let request = self.stop_button.contains(p).then_some(Request::Stop);
                Some((LRESULT(0), request))
            }
            _ => None,
        }
    }

    fn paint(&self) {
        let elapsed = self.started.map_or(0, |s| s.elapsed().as_secs());
        let text: Vec<u16> = format!("{:02}:{:02}", elapsed / 60, elapsed % 60)
            .encode_utf16()
            .collect();
        let px = |v: f32| (v * self.scale).round() as i32;
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: standard paint cycle; every GDI object created here is
        // deselected and deleted before returning.
        unsafe {
            let hdc = BeginPaint(self.pill, &mut ps);
            let panel = CreateSolidBrush(PANEL);
            FillRect(hdc, &ps.rcPaint, panel);
            let _ = DeleteObject(HGDIOBJ(panel.0));

            // Recording dot (blinks every half second).
            let height = px(36.0);
            let dot = px(10.0);
            let on = self
                .started
                .is_none_or(|s| s.elapsed().as_millis() % 1000 < 600);
            if on {
                let red = CreateSolidBrush(RED);
                let previous = SelectObject(hdc, HGDIOBJ(red.0));
                let x = px(14.0);
                let y = (height - dot) / 2;
                let _ = Ellipse(hdc, x, y, x + dot, y + dot);
                SelectObject(hdc, previous);
                let _ = DeleteObject(HGDIOBJ(red.0));
            }

            // Elapsed time.
            let font = CreateFontW(
                -px(14.0),
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                windows::Win32::Graphics::Gdi::FONT_CHARSET(0),
                windows::Win32::Graphics::Gdi::FONT_OUTPUT_PRECISION(0),
                windows::Win32::Graphics::Gdi::FONT_CLIP_PRECISION(0),
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI Variable Text"),
            );
            let previous = SelectObject(hdc, HGDIOBJ(font.0));
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, WHITE);
            let mut area = RECT {
                left: px(30.0),
                top: 0,
                right: self.stop_button.left - px(4.0),
                bottom: height,
            };
            let mut text = text;
            DrawTextW(
                hdc,
                &mut text,
                &mut area,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(hdc, previous);
            let _ = DeleteObject(HGDIOBJ(font.0));

            // Stop button: rounded-looking square with a red square inside.
            let b = self.stop_button;
            let button = CreateSolidBrush(STOP_HOVERLESS);
            let r = RECT {
                left: b.left,
                top: b.top,
                right: b.right,
                bottom: b.bottom,
            };
            FillRect(hdc, &r, button);
            let _ = DeleteObject(HGDIOBJ(button.0));
            let inset = b.width() / 3;
            let red = CreateSolidBrush(RED);
            let square = RECT {
                left: b.left + inset,
                top: b.top + inset,
                right: b.right - inset,
                bottom: b.bottom - inset,
            };
            FillRect(hdc, &square, red);
            let _ = DeleteObject(HGDIOBJ(red.0));

            let _ = EndPaint(self.pill, &ps);
        }
    }
}
