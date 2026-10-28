//! On-screen indicator while recording: a red frame around the recorded
//! area and a small pill with a countdown, then the elapsed time, a
//! pause/resume button and a stop button.
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
use std::time::{Duration, Instant};

use kadra_core::{Point, Rect};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CreateFontW, CreateSolidBrush, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, DeleteObject, DrawTextW, Ellipse, EndPaint, FONT_CHARSET, FONT_CLIP_PRECISION,
    FONT_OUTPUT_PRECISION, FW_SEMIBOLD, FillRect, GetStockObject, HDC, HGDIOBJ, InvalidateRect,
    NULL_PEN, PAINTSTRUCT, Polygon, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, HWND_TOPMOST, IDC_ARROW, KillTimer, LWA_ALPHA, LoadCursorW, MA_NOACTIVATE,
    RegisterClassExW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes,
    SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE,
    WINDOW_EX_STYLE, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_PAINT, WM_TIMER, WNDCLASSEXW, WNDPROC,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const PILL_CLASS: PCWSTR = w!("KadraRecordingPill");
const FRAME_CLASS: PCWSTR = w!("KadraRecordingFrame");
const TICK_TIMER: usize = 1;

// GDI colours are 0x00BBGGRR.
const RED: COLORREF = COLORREF(0x004d_48e5); // #E5484D
const AMBER: COLORREF = COLORREF(0x0000_b0ff); // #FFB000
const PANEL: COLORREF = COLORREF(0x0020_2020);
const WHITE: COLORREF = COLORREF(0x00ff_ffff);
const BUTTON: COLORREF = COLORREF(0x0040_4040);

/// What a click on the indicator asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Stop,
    TogglePause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Hidden,
    Countdown {
        until: Instant,
    },
    Running {
        started: Instant,
        paused_total: Duration,
        paused_since: Option<Instant>,
    },
}

impl Phase {
    fn elapsed(&self) -> Duration {
        match *self {
            Self::Running {
                started,
                paused_total,
                paused_since,
            } => paused_since
                .unwrap_or_else(Instant::now)
                .saturating_duration_since(started)
                .saturating_sub(paused_total),
            _ => Duration::ZERO,
        }
    }
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
            WINDOW_EX_STYLE((WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE).0 | ex_style),
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

fn win_rect(r: Rect) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// Fills `r` with a solid colour.
fn fill(hdc: HDC, r: &RECT, color: COLORREF) {
    // SAFETY: the brush is created and deleted here.
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(hdc, r, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

/// Draws a filled shape with no outline.
fn with_brush(hdc: HDC, color: COLORREF, draw: impl FnOnce()) {
    // SAFETY: the selected objects are restored and the brush deleted.
    unsafe {
        let brush = CreateSolidBrush(color);
        let previous_brush = SelectObject(hdc, HGDIOBJ(brush.0));
        let previous_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
        draw();
        SelectObject(hdc, previous_pen);
        SelectObject(hdc, previous_brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

#[derive(Debug)]
pub struct Indicator {
    pill: HWND,
    /// Top, bottom, left, right edges of the frame.
    edges: [HWND; 4],
    phase: Phase,
    pause_button: Rect,
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
            phase: Phase::Hidden,
            pause_button: Rect::default(),
            stop_button: Rect::default(),
            scale: 1.0,
        })
    }

    pub fn owns(&self, hwnd: HWND) -> bool {
        hwnd == self.pill || self.edges.contains(&hwnd)
    }

    fn repaint(&self) {
        // SAFETY: our own window.
        let _ = unsafe { InvalidateRect(Some(self.pill), None, false) };
    }

    fn running_now() -> Phase {
        Phase::Running {
            started: Instant::now(),
            paused_total: Duration::ZERO,
            paused_since: None,
        }
    }

    /// Shows the frame around `region` and the pill next to it, both in
    /// desktop coordinates; `monitor` keeps the pill on screen. With a
    /// non-zero `countdown`, the pill counts down until [`Self::running`].
    #[allow(clippy::many_single_char_names)]
    pub fn show(&mut self, region: Rect, monitor: Rect, scale: f32, countdown: Duration) {
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

        let (w, h) = (px(164.0), px(36.0));
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
        self.pause_button = self.stop_button.offset(-(side + px(4.0)), 0);
        self.scale = scale;
        self.phase = if countdown.is_zero() {
            Self::running_now()
        } else {
            Phase::Countdown {
                until: Instant::now() + countdown,
            }
        };

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
            SetTimer(Some(self.pill), TICK_TIMER, 200, None);
        }
        self.repaint();
    }

    /// The recording has actually started: the timer starts from zero.
    pub fn running(&mut self) {
        if self.phase != Phase::Hidden {
            self.phase = Self::running_now();
            self.repaint();
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        if let Phase::Running {
            paused_total,
            paused_since,
            ..
        } = &mut self.phase
        {
            match (paused, *paused_since) {
                (true, None) => *paused_since = Some(Instant::now()),
                (false, Some(since)) => {
                    *paused_total += since.elapsed();
                    *paused_since = None;
                }
                _ => {}
            }
        }
        self.repaint();
    }

    pub fn hide(&mut self) {
        self.phase = Phase::Hidden;
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
                self.repaint();
                Some((LRESULT(0), None))
            }
            WM_LBUTTONUP if hwnd == self.pill => {
                let p = Point::new(
                    i32::from(lparam.0 as u32 as u16 as i16),
                    i32::from((lparam.0 as u32 >> 16) as u16 as i16),
                );
                let request = if self.stop_button.contains(p) {
                    Some(Request::Stop)
                } else if self.pause_button.contains(p)
                    && matches!(self.phase, Phase::Running { .. })
                {
                    Some(Request::TogglePause)
                } else {
                    None
                };
                Some((LRESULT(0), request))
            }
            _ => None,
        }
    }

    fn paint(&self) {
        let px = |v: f32| (v * self.scale).round() as i32;
        let height = px(36.0);
        let (running, paused, blink_on) = match self.phase {
            Phase::Running {
                started,
                paused_since,
                ..
            } => (
                true,
                paused_since.is_some(),
                started.elapsed().as_millis() % 1000 < 600,
            ),
            _ => (false, false, false),
        };
        let label = if let Phase::Countdown { until } = self.phase {
            let left = until.saturating_duration_since(Instant::now());
            format!("Starting in {}", left.as_secs() + 1)
        } else {
            let s = self.phase.elapsed().as_secs();
            format!("{:02}:{:02}", s / 60, s % 60)
        };

        let mut ps = PAINTSTRUCT::default();
        // SAFETY: standard paint cycle; every GDI object created here is
        // deselected and deleted before returning.
        unsafe {
            let hdc = BeginPaint(self.pill, &mut ps);
            fill(hdc, &ps.rcPaint, PANEL);

            // Status dot: blinking red while recording, amber when paused.
            let dot_color = if paused {
                Some(AMBER)
            } else if running && blink_on {
                Some(RED)
            } else {
                None
            };
            if let Some(color) = dot_color {
                let dot = px(10.0);
                let (x, y) = (px(14.0), (height - dot) / 2);
                with_brush(hdc, color, || {
                    let _ = Ellipse(hdc, x, y, x + dot, y + dot);
                });
            }

            let font = CreateFontW(
                -px(14.0),
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                FONT_CHARSET(0),
                FONT_OUTPUT_PRECISION(0),
                FONT_CLIP_PRECISION(0),
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI Variable Text"),
            );
            let previous_font = SelectObject(hdc, HGDIOBJ(font.0));
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, WHITE);
            let text_right = if running {
                self.pause_button.left
            } else {
                self.stop_button.left
            } - px(4.0);
            let mut area = win_rect(Rect::new(px(28.0), 0, text_right, height));
            let mut text: Vec<u16> = label.encode_utf16().collect();
            DrawTextW(
                hdc,
                &mut text,
                &mut area,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(hdc, previous_font);
            let _ = DeleteObject(HGDIOBJ(font.0));

            if running {
                self.paint_pause_button(hdc, paused);
            }
            // Stop: red square on a dark button.
            let b = self.stop_button;
            fill(hdc, &win_rect(b), BUTTON);
            let inset = b.width() / 3;
            let square = Rect::new(
                b.left + inset,
                b.top + inset,
                b.right - inset,
                b.bottom - inset,
            );
            fill(hdc, &win_rect(square), RED);

            let _ = EndPaint(self.pill, &ps);
        }
    }

    /// Two bars while recording, a "play" triangle while paused.
    fn paint_pause_button(&self, hdc: HDC, paused: bool) {
        let b = self.pause_button;
        fill(hdc, &win_rect(b), BUTTON);
        let inset = b.width() / 3;
        let inner = Rect::new(
            b.left + inset,
            b.top + inset,
            b.right - inset,
            b.bottom - inset,
        );
        if paused {
            let points = [
                POINT {
                    x: inner.left,
                    y: inner.top,
                },
                POINT {
                    x: inner.right,
                    y: inner.top + inner.height() / 2,
                },
                POINT {
                    x: inner.left,
                    y: inner.bottom,
                },
            ];
            with_brush(hdc, WHITE, || {
                // SAFETY: drawing on the paint DC with a valid point list.
                let _ = unsafe { Polygon(hdc, &points) };
            });
        } else {
            let bar = (inner.width() / 3).max(1);
            let left = Rect::new(inner.left, inner.top, inner.left + bar, inner.bottom);
            let right = Rect::new(inner.right - bar, inner.top, inner.right, inner.bottom);
            fill(hdc, &win_rect(left), WHITE);
            fill(hdc, &win_rect(right), WHITE);
        }
    }
}
