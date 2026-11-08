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
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CreateFontW, CreateSolidBrush, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, DeleteObject, DrawTextW, Ellipse, EndPaint, FONT_CHARSET, FONT_CLIP_PRECISION,
    FONT_OUTPUT_PRECISION, FW_SEMIBOLD, FillRect, GetStockObject, HDC, HGDIOBJ, InvalidateRect,
    NULL_PEN, PAINTSTRUCT, Polygon, RoundRect, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
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
const RED: COLORREF = COLORREF(0x0023_11e8); // #E81123
const AMBER: COLORREF = COLORREF(0x0000_b0ff); // #FFB000
const PANEL: COLORREF = COLORREF(0x001f_1c1c); // #1C1C1F
const WHITE: COLORREF = COLORREF(0x00ff_ffff);
const BUTTON: COLORREF = COLORREF(0x0034_302e); // #2E3034
/// Size of the pill, in pixels at 100% scale.
const PILL_WIDTH: f32 = 300.0;
const PILL_HEIGHT: f32 = 50.0;

/// What a click on the indicator asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Stop,
    TogglePause,
    /// Stop and delete the recording.
    Discard,
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

/// Where the pill goes: under the recorded area, else above it, else (the
/// area fills the screen) at the top of the screen, inside the area.
fn pill_position(region: Rect, work: Rect, (w, h): (i32, i32), gap: i32) -> (i32, i32) {
    let x =
        (region.left + (region.width() - w) / 2).clamp(work.left, (work.right - w).max(work.left));
    let y = if region.bottom + gap + h <= work.bottom {
        region.bottom + gap
    } else if region.top - gap - h >= work.top {
        region.top - gap - h
    } else {
        region.top.max(work.top) + gap
    };
    (x, y)
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
    discard_button: Rect,
    scale: f32,
}

impl Indicator {
    pub fn new(instance: HINSTANCE) -> windows::core::Result<Self> {
        let pill = create(instance, PILL_CLASS, 0)?;
        // Soft corners, like the rest of Kadra.
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
            // Windows 11 rounds and outlines popups: on 2-pixel strips this
            // turns the frame into a blotchy, grey-edged line. Keep it crisp.
            let square = DWMWCP_DONOTROUND;
            let no_border = DWMWA_COLOR_NONE;
            // SAFETY: layered window owned by this thread; the attribute
            // values outlive the calls.
            unsafe {
                SetLayeredWindowAttributes(hwnd, COLORREF(0), 230, LWA_ALPHA)?;
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    std::ptr::from_ref(&square).cast::<c_void>(),
                    size_of_val(&square) as u32,
                );
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_BORDER_COLOR,
                    std::ptr::from_ref(&no_border).cast::<c_void>(),
                    size_of_val(&no_border) as u32,
                );
            }
            Ok::<_, windows::core::Error>(hwnd)
        };
        Ok(Self {
            pill,
            edges: [edge()?, edge()?, edge()?, edge()?],
            phase: Phase::Hidden,
            pause_button: Rect::default(),
            stop_button: Rect::default(),
            discard_button: Rect::default(),
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
    /// desktop coordinates; the pill stays inside `work` (the monitor
    /// without the taskbar, which would cover it). With a
    /// non-zero `countdown`, the pill counts down until [`Self::running`].
    #[allow(clippy::many_single_char_names)]
    pub fn show(&mut self, region: Rect, work: Rect, scale: f32, countdown: Duration) {
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

        // Timer, then Delete and Pause, then a large Stop button.
        let (w, h) = (px(PILL_WIDTH), px(PILL_HEIGHT));
        let gap = px(8.0);
        let (x, y) = pill_position(region, work, (w, h), gap);
        let pad = px(7.0);
        let side = h - 2 * pad;
        let stop_width = px(98.0);
        self.stop_button = Rect::from_size(w - pad - stop_width, pad, stop_width, side);
        self.pause_button =
            Rect::from_size(self.stop_button.left - px(6.0) - side, pad, side, side);
        self.discard_button = self.pause_button.offset(-(side + px(6.0)), 0);
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
                } else if self.discard_button.contains(p) {
                    Some(Request::Discard)
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
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: standard paint cycle on our own window.
        unsafe {
            let hdc = BeginPaint(self.pill, &mut ps);
            self.draw(hdc, &ps.rcPaint);
            let _ = EndPaint(self.pill, &ps);
        }
    }

    fn draw(&self, hdc: HDC, area: &RECT) {
        let px = |v: f32| (v * self.scale).round() as i32;
        let height = px(PILL_HEIGHT);
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
            format!("Starts in {}", left.as_secs() + 1)
        } else {
            let s = self.phase.elapsed().as_secs();
            format!("{:02}:{:02}", s / 60, s % 60)
        };

        // SAFETY: every GDI object created here is deselected and deleted
        // before returning.
        unsafe {
            fill(hdc, area, PANEL);

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
                let (x, y) = (px(16.0), (height - dot) / 2);
                with_brush(hdc, color, || {
                    let _ = Ellipse(hdc, x, y, x + dot, y + dot);
                });
            }

            let text_right = self.discard_button.left - px(4.0);
            let area = Rect::new(px(32.0), 0, text_right, height);
            draw_text(hdc, &label, area, px(15.0), WHITE);

            // Delete, Pause (while recording), then Stop: the one that
            // matters, in red with its name.
            let radius = px(8.0);
            rounded(hdc, self.discard_button, radius, BUTTON);
            paint_trash(hdc, self.discard_button);
            if running {
                rounded(hdc, self.pause_button, radius, BUTTON);
                self.paint_pause_button(hdc, paused);
            }
            let b = self.stop_button;
            rounded(hdc, b, radius, RED);
            let icon = px(11.0);
            let icon_left = b.left + px(16.0);
            let top = b.top + (b.height() - icon) / 2;
            with_brush(hdc, WHITE, || {
                let _ = RoundRect(
                    hdc,
                    icon_left,
                    top,
                    icon_left + icon,
                    top + icon,
                    px(3.0),
                    px(3.0),
                );
            });
            let text = Rect::new(
                icon_left + icon + px(4.0),
                b.top,
                b.right - px(8.0),
                b.bottom,
            );
            draw_text(hdc, "Stop", text, px(15.0), WHITE);
        }
    }

    /// Two bars while recording, a "play" triangle while paused.
    fn paint_pause_button(&self, hdc: HDC, paused: bool) {
        let b = self.pause_button;
        let inset = b.width() * 7 / 20;
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

/// A filled rectangle with rounded corners.
fn rounded(hdc: HDC, r: Rect, radius: i32, color: COLORREF) {
    with_brush(hdc, color, || {
        // SAFETY: drawing on the paint DC.
        let _ = unsafe {
            RoundRect(
                hdc,
                r.left,
                r.top,
                r.right + 1,
                r.bottom + 1,
                radius,
                radius,
            )
        };
    });
}

/// A small bin: handle, lid, and a body narrowing towards the bottom.
fn paint_trash(hdc: HDC, b: Rect) {
    let size = b.width() * 9 / 20;
    let cx = b.left + b.width() / 2;
    let top = b.top + (b.height() - size) / 2;
    let unit = (size / 9).max(1);
    let handle = Rect::new(cx - 2 * unit, top, cx + 2 * unit, top + unit);
    let lid = Rect::new(cx - size / 2, top + unit, cx + size / 2, top + 2 * unit);
    fill(hdc, &win_rect(handle), WHITE);
    fill(hdc, &win_rect(lid), WHITE);
    let (body_top, bottom) = (top + 3 * unit, top + size);
    let (wide, narrow) = (size / 2 - unit, size / 2 - 2 * unit);
    let body = [
        POINT {
            x: cx - wide,
            y: body_top,
        },
        POINT {
            x: cx + wide,
            y: body_top,
        },
        POINT {
            x: cx + narrow,
            y: bottom,
        },
        POINT {
            x: cx - narrow,
            y: bottom,
        },
    ];
    with_brush(hdc, WHITE, || {
        // SAFETY: drawing on the paint DC with a valid point list.
        let _ = unsafe { Polygon(hdc, &body) };
    });
    // Grooves in the body.
    let groove = (unit * 2 / 3).max(1);
    for dx in [-(size / 6), size / 6] {
        let left = cx + dx - groove / 2;
        let g = Rect::new(left, body_top + unit, left + groove, bottom - unit);
        fill(hdc, &win_rect(g), BUTTON);
    }
}

/// Centred single-line text in Segoe UI Variable.
fn draw_text(hdc: HDC, label: &str, area: Rect, size: i32, color: COLORREF) {
    // SAFETY: the font is selected, used and deleted here.
    unsafe {
        let font = CreateFontW(
            -size,
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
        SetTextColor(hdc, color);
        let mut rect = win_rect(area);
        let mut text: Vec<u16> = label.encode_utf16().collect();
        DrawTextW(
            hdc,
            &mut text,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(hdc, previous_font);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect::new(0, 0, 1920, 1032);

    #[test]
    fn pill_goes_below_then_above_then_top() {
        let size = (164, 36);
        assert_eq!(
            pill_position(Rect::new(100, 100, 500, 400), WORK, size, 8),
            (218, 408)
        );
        // No room below (taskbar): above.
        assert_eq!(
            pill_position(Rect::new(100, 600, 500, 1030), WORK, size, 8).1,
            556
        );
        // Full screen: at the top, never under the taskbar.
        let (_, y) = pill_position(Rect::new(0, 0, 1920, 1080), WORK, size, 8);
        assert_eq!(y, 8);
    }
}
