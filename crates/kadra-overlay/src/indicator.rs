//! On-screen indicator while recording: a red frame around the recorded
//! area and a small pill with a countdown, then the elapsed time, a
//! pause/resume button and a stop button.
//!
//! The pill is drawn off screen with antialiased shapes (GDI's own are
//! jagged), then copied in one go. When the recording starts it widens
//! smoothly to make room for the pause button.
//!
//! Every window is excluded from capture, so none of it appears in the
//! video, and the frame lets mouse clicks through.

// Win32 message parameters, structure sizes and pixel maths are packed or
// rounded integers.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names,
    clippy::too_many_arguments
)]

use std::ffi::c_void;
use std::time::{Duration, Instant};

use kadra_core::{Point, Rect};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CLEARTYPE_QUALITY,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DIB_RGB_COLORS, DT_CENTER,
    DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint, FONT_CHARSET,
    FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FW_BOLD, FW_SEMIBOLD, GdiFlush, HDC, HGDIOBJ,
    InvalidateRect, PAINTSTRUCT, SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, HWND_TOPMOST, IDC_ARROW, KillTimer, LWA_ALPHA, LoadCursorW, MA_NOACTIVATE,
    RegisterClassExW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes,
    SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE,
    WINDOW_EX_STYLE, WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_PAINT, WM_TIMER,
    WNDCLASSEXW, WNDPROC, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const PILL_CLASS: PCWSTR = w!("KadraRecordingPill");
const FRAME_CLASS: PCWSTR = w!("KadraRecordingFrame");
const TICK_TIMER: usize = 1;

/// Kadra's dark palette (`docs/UI_DESIGN.md`), as RGB.
type Rgb = [u8; 3];
const SURFACE: Rgb = [0x1c, 0x1c, 0x1f];
const CONTROL: Rgb = [0x2a, 0x2a, 0x2f];
const ACCENT: Rgb = [0x25, 0x63, 0xeb];
const RECORD: Rgb = [0xe5, 0x48, 0x4d];
const AMBER: Rgb = [0xff, 0xb5, 0x47];
const TEXT: Rgb = [0xf5, 0xf5, 0xf7];
const MUTED: Rgb = [0x9a, 0x9a, 0xa3];
/// The frame around the recorded area, as a GDI colour (0x00BBGGRR).
const FRAME_COLOR: COLORREF = COLORREF(0x004d_48e5);

/// Pill sizes, in pixels at 100% scale.
const PILL_HEIGHT: f32 = 48.0;
const PAD: f32 = 6.0;
const GAP: f32 = 6.0;
const STOP_WIDTH: f32 = 88.0;
/// Room for the status (countdown or time) on the left.
const STATUS_WIDTH: f32 = 132.0;
/// How long the pill takes to widen for the pause button.
const OPEN_MS: f32 = 260.0;

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
    /// Counting down (or, once `until` has passed, getting the recorder
    /// ready).
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
            hbrBackground: CreateSolidBrush(FRAME_COLOR),
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

/// Smooth start and end.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[derive(Debug)]
pub struct Indicator {
    pill: HWND,
    /// Top, bottom, left, right edges of the frame.
    edges: [HWND; 4],
    phase: Phase,
    /// When the pause button started to slide in (recording started).
    opening: Option<Instant>,
    /// Centre and top of the pill, in desktop pixels: it widens around it.
    centre: (i32, i32),
    /// The pill must stay inside these (desktop pixels).
    work: Rect,
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
            opening: None,
            centre: (0, 0),
            work: Rect::default(),
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

    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    /// How far the pause button has slid in, 0 to 1.
    fn open(&self) -> f32 {
        match (self.phase, self.opening) {
            (Phase::Running { .. }, Some(since)) => {
                ease(since.elapsed().as_secs_f32() * 1000.0 / OPEN_MS)
            }
            (Phase::Running { .. }, None) => 1.0,
            _ => 0.0,
        }
    }

    /// Width of the pill with the pause button `open` (0 to 1).
    fn width(&self, open: f32) -> i32 {
        let side = PILL_HEIGHT - 2.0 * PAD;
        self.px(PAD + STATUS_WIDTH + side + GAP + (side + GAP) * open + STOP_WIDTH + PAD)
    }

    /// Places the buttons for a pill `width` wide, right to left: Stop,
    /// Pause (sliding in), Delete.
    fn layout(&mut self, width: i32, open: f32) {
        let (pad, gap) = (self.px(PAD), self.px(GAP));
        let side = self.px(PILL_HEIGHT) - 2 * pad;
        let stop_width = self.px(STOP_WIDTH);
        self.stop_button = Rect::from_size(width - pad - stop_width, pad, stop_width, side);
        let slot = ((side + gap) as f32 * open).round() as i32;
        self.pause_button = Rect::from_size(self.stop_button.left - gap - side, pad, side, side);
        self.discard_button =
            Rect::from_size(self.stop_button.left - slot - gap - side, pad, side, side);
    }

    /// Moves and sizes the pill for its current width, around its centre.
    fn place(&mut self) {
        let open = self.open();
        let (w, h) = (self.width(open), self.px(PILL_HEIGHT));
        self.layout(w, open);
        let x = (self.centre.0 - w / 2)
            .clamp(self.work.left, (self.work.right - w).max(self.work.left));
        // SAFETY: our own window.
        unsafe {
            let _ = SetWindowPos(
                self.pill,
                Some(HWND_TOPMOST),
                x,
                self.centre.1,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        self.repaint();
    }

    /// Shows the frame around `region` and the pill next to it, both in
    /// desktop coordinates; the pill stays inside `work` (the monitor
    /// without the taskbar, which would cover it). It counts down for
    /// `countdown` (then shows that the recording is starting) until
    /// [`Self::running`].
    pub fn show(&mut self, region: Rect, work: Rect, scale: f32, countdown: Duration) {
        self.scale = scale;
        let t = self.px(2.0).max(1);
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

        // Positioned for its widest, so that widening never pushes it off
        // the screen or over the area.
        let (w, h) = (self.width(1.0), self.px(PILL_HEIGHT));
        let (x, y) = pill_position(region, work, (w, h), self.px(8.0));
        self.centre = (x + w / 2, y);
        self.work = work;
        self.opening = None;
        self.phase = Phase::Countdown {
            until: Instant::now() + countdown,
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
            SetTimer(Some(self.pill), TICK_TIMER, 33, None);
        }
        self.place();
    }

    /// The recording has actually started: the timer starts from zero and
    /// the pill widens for the pause button.
    pub fn running(&mut self) {
        if self.phase != Phase::Hidden {
            self.phase = Phase::Running {
                started: Instant::now(),
                paused_total: Duration::ZERO,
                paused_since: None,
            };
            self.opening = Some(Instant::now());
            self.place();
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
        self.opening = None;
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
            // Painted whole, off screen: no flicker.
            WM_ERASEBKGND if hwnd == self.pill => Some((LRESULT(1), None)),
            WM_PAINT if hwnd == self.pill => {
                self.paint();
                Some((LRESULT(0), None))
            }
            WM_TIMER if hwnd == self.pill => {
                if let Some(since) = self.opening {
                    if since.elapsed().as_secs_f32() * 1000.0 >= OPEN_MS {
                        self.opening = None;
                    }
                    self.place();
                } else {
                    self.repaint();
                }
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
                    && self.opening.is_none()
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
            let mut client = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(self.pill, &mut client);
            self.draw(hdc, &client);
            let _ = EndPaint(self.pill, &ps);
        }
    }

    /// Draws the whole pill (`area` is its client rectangle) into `hdc`.
    #[allow(clippy::too_many_lines)]
    fn draw(&self, hdc: HDC, area: &RECT) {
        let (w, h) = (area.right - area.left, area.bottom - area.top);
        if w <= 0 || h <= 0 {
            return;
        }
        let Some(mut canvas) = Canvas::new(hdc, w, h) else {
            return;
        };
        let s = self.scale;
        let open = self.open();
        canvas.fill(SURFACE);

        let status_left = PAD * s + 6.0 * s;
        let mid = h as f32 / 2.0;
        let mut text: Option<(String, i32, Rgb, bool)> = None;
        match self.phase {
            Phase::Countdown { until } => {
                let left = until.saturating_duration_since(Instant::now());
                let r = 15.0 * s;
                let (cx, cy) = (status_left + r, mid);
                if left.is_zero() {
                    // Getting the recorder ready: a still red dot.
                    canvas.circle(cx, cy, 6.0 * s, RECORD, 1.0);
                    text = Some(("Starting…".into(), (cx + r + 9.0 * s) as i32, MUTED, false));
                } else {
                    // The number of seconds left in an accent disc, ringed
                    // by the time left in this second.
                    let secs = left.as_secs() + 1;
                    let frac = (left.as_secs_f32() - left.as_secs() as f32).clamp(0.0, 1.0);
                    canvas.circle(cx, cy, r, ACCENT, 1.0);
                    canvas.ring(cx, cy, r - 2.2 * s, 2.0 * s, frac, [255, 255, 255], 0.9);
                    canvas.text_at(
                        &secs.to_string(),
                        Rect::new(
                            (cx - r) as i32,
                            (cy - r) as i32,
                            (cx + r) as i32,
                            (cy + r) as i32,
                        ),
                        (14.0 * s) as i32,
                        TEXT,
                        true,
                        true,
                    );
                    text = Some(("Get ready".into(), (cx + r + 9.0 * s) as i32, MUTED, false));
                }
            }
            Phase::Running {
                started,
                paused_since,
                ..
            } => {
                let (cx, cy) = (status_left + 6.0 * s, mid);
                if paused_since.is_some() {
                    canvas.circle(cx, cy, 5.5 * s, AMBER, 1.0);
                } else {
                    // A soft pulse rather than a blink.
                    let t = started.elapsed().as_secs_f32();
                    let pulse = 0.5 + 0.5 * (t * std::f32::consts::TAU / 1.6).cos();
                    canvas.circle(cx, cy, 5.5 * s + 4.0 * s * pulse, RECORD, 0.22 * pulse);
                    canvas.circle(cx, cy, 5.5 * s, RECORD, 1.0);
                }
                let secs = self.phase.elapsed().as_secs();
                let label = format!("{:02}:{:02}", secs / 60, secs % 60);
                text = Some((label, (cx + 15.0 * s) as i32, TEXT, true));
            }
            Phase::Hidden => {}
        }

        // Buttons: Delete, Pause (sliding in), Stop.
        let radius = 10.0 * s;
        let d = self.discard_button;
        canvas.rounded(d, radius, CONTROL, 1.0);
        canvas.trash(d, TEXT);
        if open > 0.0 {
            let p = self.pause_button;
            canvas.rounded(p, radius, CONTROL, open);
            canvas.pause_icon(
                p,
                matches!(
                    self.phase,
                    Phase::Running {
                        paused_since: Some(_),
                        ..
                    }
                ),
                TEXT,
                open,
            );
        }
        let b = self.stop_button;
        canvas.rounded(b, radius, RECORD, 1.0);
        let icon = 11.0 * s;
        let icon_left = b.left as f32 + 15.0 * s;
        let top = b.top as f32 + (b.height() as f32 - icon) / 2.0;
        canvas.rounded_f(icon_left, top, icon, icon, 3.0 * s, [255, 255, 255], 1.0);

        // Text goes on last, with GDI, over the finished picture.
        canvas.flush();
        if let Some((label, x, color, big)) = text {
            let size = if big { 17.0 } else { 14.0 };
            canvas.text_at(
                &label,
                Rect::new(x, 0, d.left - self.px(4.0), h),
                (size * s) as i32,
                color,
                big,
                false,
            );
        }
        canvas.text_at(
            "Stop",
            Rect::new(
                (icon_left + icon + 7.0 * s) as i32,
                b.top,
                b.right - self.px(10.0),
                b.bottom,
            ),
            (14.0 * s) as i32,
            [255, 255, 255],
            true,
            false,
        );
        canvas.present(hdc);
    }
}

/// An off-screen 32-bit picture: antialiased shapes are computed into its
/// pixels, text is drawn with GDI, then it is copied to the window.
struct Canvas {
    dc: HDC,
    bitmap: HGDIOBJ,
    previous: HGDIOBJ,
    bits: *mut u8,
    w: i32,
    h: i32,
}

impl Canvas {
    fn new(target: HDC, w: i32, h: i32) -> Option<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid DC and bitmap description; every object is released
        // in `Drop`.
        unsafe {
            let dc = CreateCompatibleDC(Some(target));
            if dc.is_invalid() {
                return None;
            }
            let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            else {
                let _ = DeleteDC(dc);
                return None;
            };
            if bits.is_null() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(dc);
                return None;
            }
            let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
            Some(Self {
                dc,
                bitmap: HGDIOBJ(bitmap.0),
                previous,
                bits: bits.cast(),
                w,
                h,
            })
        }
    }

    fn pixels(&mut self) -> &mut [u8] {
        // SAFETY: the DIB section holds w × h × 4 bytes and lives as long as
        // `self`; GDI is flushed before the pixels are touched.
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) }
    }

    fn fill(&mut self, [r, g, b]: Rgb) {
        for p in self.pixels().as_chunks_mut::<4>().0 {
            *p = [b, g, r, 255];
        }
    }

    /// Paints `color` where the signed distance `sdf` (negative inside) is
    /// below zero, with a one-pixel soft edge, within the given box.
    fn shade(
        &mut self,
        bounds: (f32, f32, f32, f32),
        color: Rgb,
        alpha: f32,
        sdf: impl Fn(f32, f32) -> f32,
    ) {
        let (w, h) = (self.w, self.h);
        let x0 = (bounds.0.floor() as i32 - 1).max(0);
        let y0 = (bounds.1.floor() as i32 - 1).max(0);
        let x1 = (bounds.2.ceil() as i32 + 1).min(w);
        let y1 = (bounds.3.ceil() as i32 + 1).min(h);
        let pixels = self.pixels();
        for y in y0..y1 {
            for x in x0..x1 {
                let d = sdf(x as f32 + 0.5, y as f32 + 0.5);
                let a = (0.5 - d).clamp(0.0, 1.0) * alpha;
                if a <= 0.0 {
                    continue;
                }
                let i = ((y * w + x) * 4) as usize;
                for (c, v) in [color[2], color[1], color[0]].into_iter().enumerate() {
                    let old = f32::from(pixels[i + c]);
                    pixels[i + c] = (old + (f32::from(v) - old) * a).round() as u8;
                }
            }
        }
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Rgb, alpha: f32) {
        self.shade((cx - r, cy - r, cx + r, cy + r), color, alpha, |x, y| {
            (x - cx).hypot(y - cy) - r
        });
    }

    /// An arc of a ring, clockwise from the top, `share` of the way round,
    /// with round ends.
    fn ring(&mut self, cx: f32, cy: f32, r: f32, thick: f32, share: f32, color: Rgb, alpha: f32) {
        if share <= 0.0 {
            return;
        }
        let sweep = share * std::f32::consts::TAU;
        let end = (cx + r * sweep.sin(), cy - r * sweep.cos());
        let start = (cx, cy - r);
        let half = thick / 2.0;
        self.shade(
            (
                cx - r - thick,
                cy - r - thick,
                cx + r + thick,
                cy + r + thick,
            ),
            color,
            alpha,
            |x, y| {
                // Angle clockwise from the top, 0 to 2π.
                let mut a = (x - cx).atan2(-(y - cy));
                if a < 0.0 {
                    a += std::f32::consts::TAU;
                }
                if a <= sweep {
                    ((x - cx).hypot(y - cy) - r).abs() - half
                } else {
                    let to_start = (x - start.0).hypot(y - start.1);
                    let to_end = (x - end.0).hypot(y - end.1);
                    to_start.min(to_end) - half
                }
            },
        );
    }

    fn rounded_f(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgb, alpha: f32) {
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let (hw, hh) = (w / 2.0, h / 2.0);
        let r = radius.min(hw).min(hh);
        self.shade((x, y, x + w, y + h), color, alpha, |px, py| {
            let qx = (px - cx).abs() - (hw - r);
            let qy = (py - cy).abs() - (hh - r);
            qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
        });
    }

    fn rounded(&mut self, b: Rect, radius: f32, color: Rgb, alpha: f32) {
        self.rounded_f(
            b.left as f32,
            b.top as f32,
            b.width() as f32,
            b.height() as f32,
            radius,
            color,
            alpha,
        );
    }

    /// Two bars while recording, a "play" triangle while paused.
    fn pause_icon(&mut self, b: Rect, paused: bool, color: Rgb, alpha: f32) {
        let size = b.width() as f32 * 0.36;
        let (cx, cy) = (
            b.left as f32 + b.width() as f32 / 2.0,
            b.top as f32 + b.height() as f32 / 2.0,
        );
        if paused {
            // A triangle pointing right, nudged to look centred.
            let (l, t, bt) = (cx - size * 0.4, cy - size / 2.0, cy + size / 2.0);
            let r = cx + size * 0.55;
            let edge = |ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32| {
                let (dx, dy) = (bx - ax, by - ay);
                let len = dx.hypot(dy);
                ((px - ax) * dy - (py - ay) * dx) / len
            };
            self.shade((l, t, r, bt), color, alpha, |x, y| {
                edge(l, t, r, cy, x, y)
                    .max(edge(r, cy, l, bt, x, y))
                    .max(edge(l, bt, l, t, x, y))
            });
        } else {
            let bar = size * 0.32;
            let radius = bar * 0.4;
            self.rounded_f(
                cx - size / 2.0 + bar * 0.15,
                cy - size / 2.0,
                bar,
                size,
                radius,
                color,
                alpha,
            );
            self.rounded_f(
                cx + size / 2.0 - bar * 1.15,
                cy - size / 2.0,
                bar,
                size,
                radius,
                color,
                alpha,
            );
        }
    }

    /// A small bin: lid with a handle, and a rounded body.
    fn trash(&mut self, b: Rect, color: Rgb) {
        let size = b.width() as f32 * 0.42;
        let (cx, cy) = (
            b.left as f32 + b.width() as f32 / 2.0,
            b.top as f32 + b.height() as f32 / 2.0,
        );
        let top = cy - size / 2.0;
        let unit = size / 9.0;
        self.rounded_f(
            cx - 1.8 * unit,
            top,
            3.6 * unit,
            1.6 * unit,
            0.6 * unit,
            color,
            1.0,
        );
        self.rounded_f(
            cx - size / 2.0,
            top + 1.2 * unit,
            size,
            1.3 * unit,
            0.6 * unit,
            color,
            1.0,
        );
        let body_top = top + 3.0 * unit;
        let body_w = size - 2.0 * unit;
        self.rounded_f(
            cx - body_w / 2.0,
            body_top,
            body_w,
            size - 3.0 * unit,
            1.2 * unit,
            color,
            1.0,
        );
        // Grooves.
        for dx in [-1.4 * unit, 1.4 * unit] {
            self.rounded_f(
                cx + dx - 0.4 * unit,
                body_top + 1.3 * unit,
                0.8 * unit,
                size - 5.6 * unit,
                0.4 * unit,
                CONTROL,
                1.0,
            );
        }
    }

    /// Lets GDI and the pixel maths take turns on the bitmap.
    #[allow(clippy::unused_self)]
    fn flush(&self) {
        // SAFETY: plain GDI call.
        let _ = unsafe { GdiFlush() };
    }

    /// Single-line text in Segoe UI Variable, left-aligned or centred.
    fn text_at(
        &mut self,
        label: &str,
        area: Rect,
        size: i32,
        [r, g, b]: Rgb,
        bold: bool,
        centred: bool,
    ) {
        self.flush();
        // SAFETY: the font is selected, used and deleted here.
        unsafe {
            let font = CreateFontW(
                -size,
                0,
                0,
                0,
                if bold { FW_BOLD.0 } else { FW_SEMIBOLD.0 } as i32,
                0,
                0,
                0,
                FONT_CHARSET(0),
                FONT_OUTPUT_PRECISION(0),
                FONT_CLIP_PRECISION(0),
                CLEARTYPE_QUALITY,
                0,
                if bold {
                    w!("Segoe UI Variable Display")
                } else {
                    w!("Segoe UI Variable Text")
                },
            );
            let previous_font = SelectObject(self.dc, HGDIOBJ(font.0));
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(
                self.dc,
                COLORREF(u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16),
            );
            let mut rect = win_rect(area);
            let mut text: Vec<u16> = label.encode_utf16().collect();
            let align = if centred { DT_CENTER } else { DT_LEFT };
            DrawTextW(
                self.dc,
                &mut text,
                &mut rect,
                align | DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(self.dc, previous_font);
            let _ = DeleteObject(HGDIOBJ(font.0));
        }
        self.flush();
    }

    fn present(&self, target: HDC) {
        // SAFETY: both DCs are valid for the copy.
        unsafe {
            let _ = BitBlt(target, 0, 0, self.w, self.h, Some(self.dc), 0, 0, SRCCOPY);
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: restores and releases what `new` created.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
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
