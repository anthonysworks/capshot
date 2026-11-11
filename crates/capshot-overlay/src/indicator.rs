//! On-screen indicator while recording: a red frame around the recorded
//! area and a floating bar: a grip to move it, then a chip with the stop
//! button and the elapsed time (a countdown first), and restart, pause and
//! delete buttons.
//!
//! The bar is a layered window drawn off screen with antialiased shapes
//! (GDI's own are jagged) and per-pixel transparency, so it can be a true
//! pill with a soft shadow, in the light or the dark theme. When the
//! recording starts it widens smoothly to make room for restart and pause.
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

use capshot_core::{Point, Rect};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CLEARTYPE_QUALITY, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush,
    DIB_RGB_COLORS, DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject,
    DrawTextW, FONT_CHARSET, FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FW_BOLD, FW_SEMIBOLD,
    GdiFlush, GetDC, HDC, HGDIOBJ, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetWindowRect, HTCAPTION, HTCLIENT, HWND_TOPMOST, IDC_ARROW, KillTimer,
    LWA_ALPHA, LoadCursorW, MA_NOACTIVATE, RegisterClassExW, SW_HIDE, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetTimer, SetWindowDisplayAffinity, SetWindowPos,
    ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE,
    WM_EXITSIZEMOVE, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCHITTEST, WM_TIMER,
    WNDCLASSEXW, WNDPROC, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const PILL_CLASS: PCWSTR = w!("CapshotRecordingPill");
const FRAME_CLASS: PCWSTR = w!("CapshotRecordingFrame");
const TICK_TIMER: usize = 1;
/// Sent after `TrackMouseEvent(TME_LEAVE)` (defined in the Controls API set).
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Capshot's palettes (`docs/UI_DESIGN.md`), as RGB.
type Rgb = [u8; 3];
const ACCENT: Rgb = [0x25, 0x63, 0xeb];
const RECORD: Rgb = [0xe5, 0x48, 0x4d];
const AMBER: Rgb = [0xff, 0xb5, 0x47];
const WHITE: Rgb = [0xff, 0xff, 0xff];
/// The frame around the recorded area, as a GDI colour (0x00BBGGRR).
const FRAME_COLOR: COLORREF = COLORREF(0x004d_48e5);

/// Colours of the bar in one theme.
struct Palette {
    panel: Rgb,
    /// The chip holding the stop button and the time.
    chip: Rgb,
    hover: Rgb,
    /// Button glyphs, quieter than the time.
    icon: Rgb,
    text: Rgb,
    muted: Rgb,
    /// Hairline around the bar, and the shadow's strength.
    border: (Rgb, f32),
    shadow: f32,
}

const LIGHT: Palette = Palette {
    panel: [0xff, 0xff, 0xff],
    chip: [0xf0, 0xf0, 0xf3],
    hover: [0xea, 0xea, 0xee],
    icon: [0x6b, 0x6b, 0x74],
    text: [0x1c, 0x1c, 0x1f],
    muted: [0x6e, 0x6e, 0x78],
    border: ([0x00, 0x00, 0x00], 0.08),
    shadow: 0.16,
};

const DARK: Palette = Palette {
    panel: [0x1c, 0x1c, 0x1f],
    chip: [0x2a, 0x2a, 0x2f],
    hover: [0x34, 0x34, 0x3a],
    icon: [0xb4, 0xb4, 0xbd],
    text: [0xf5, 0xf5, 0xf7],
    muted: [0x9a, 0x9a, 0xa3],
    border: ([0xff, 0xff, 0xff], 0.1),
    shadow: 0.4,
};

/// Bar sizes, in pixels at 100% scale.
const PILL_HEIGHT: f32 = 48.0;
const PAD: f32 = 6.0;
const GAP: f32 = 2.0;
/// Room around the bar for its shadow.
const SHADOW: f32 = 10.0;
const GRIP_WIDTH: f32 = 22.0;
/// The chip: the stop button and the time (or the countdown).
const CHIP_WIDTH: f32 = 118.0;
/// Square icon buttons.
const BUTTON: f32 = 36.0;
/// How long the bar takes to widen for restart and pause.
const OPEN_MS: f32 = 260.0;

/// What a click on the indicator asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Stop,
    TogglePause,
    /// Stop and delete the recording.
    Discard,
    /// Delete the recording and start again on the same area.
    Restart,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Stop,
    Restart,
    Pause,
    Discard,
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
            w!("Capshot recording"),
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

fn point(lparam: LPARAM) -> Point {
    Point::new(
        i32::from(lparam.0 as u32 as u16 as i16),
        i32::from((lparam.0 as u32 >> 16) as u16 as i16),
    )
}

#[derive(Debug)]
pub struct Indicator {
    pill: HWND,
    /// Top, bottom, left, right edges of the frame.
    edges: [HWND; 4],
    phase: Phase,
    /// When restart and pause started to slide in (recording started).
    opening: Option<Instant>,
    /// Centre and top of the window, in desktop pixels: it widens around it.
    centre: (i32, i32),
    /// The window must stay inside these (desktop pixels).
    work: Rect,
    /// Where the window is, in desktop pixels.
    at: Rect,
    grip: Rect,
    stop_button: Rect,
    restart_button: Rect,
    pause_button: Rect,
    discard_button: Rect,
    hover: Option<Part>,
    tracking: bool,
    light: bool,
    scale: f32,
}

impl Indicator {
    pub fn new(instance: HINSTANCE) -> windows::core::Result<Self> {
        let pill = create(instance, PILL_CLASS, WS_EX_LAYERED.0)?;
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
            at: Rect::default(),
            grip: Rect::default(),
            stop_button: Rect::default(),
            restart_button: Rect::default(),
            pause_button: Rect::default(),
            discard_button: Rect::default(),
            hover: None,
            tracking: false,
            light: false,
            scale: 1.0,
        })
    }

    pub fn owns(&self, hwnd: HWND) -> bool {
        hwnd == self.pill || self.edges.contains(&hwnd)
    }

    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    const fn palette(&self) -> &'static Palette {
        if self.light { &LIGHT } else { &DARK }
    }

    /// How far restart and pause have slid in, 0 to 1.
    fn open(&self) -> f32 {
        match (self.phase, self.opening) {
            (Phase::Running { .. }, Some(since)) => {
                ease(since.elapsed().as_secs_f32() * 1000.0 / OPEN_MS)
            }
            (Phase::Running { .. }, None) => 1.0,
            _ => 0.0,
        }
    }

    /// Width of the window (bar and shadow) with restart and pause `open`
    /// (0 to 1).
    fn width(&self, open: f32) -> i32 {
        let slots = 1.0 + 2.0 * open;
        self.px(2.0 * SHADOW + PAD + GRIP_WIDTH + CHIP_WIDTH + (GAP + BUTTON) * slots + PAD)
    }

    fn height(&self) -> i32 {
        self.px(PILL_HEIGHT + 2.0 * SHADOW)
    }

    /// The bar itself inside the window.
    fn bar(&self, width: i32) -> Rect {
        let m = self.px(SHADOW);
        Rect::new(m, m, width - m, m + self.px(PILL_HEIGHT))
    }

    /// Places the parts for a window `width` wide: grip, chip (with Stop),
    /// then Restart and Pause (sliding in) and Delete.
    fn layout(&mut self, width: i32, open: f32) {
        let bar = self.bar(width);
        let pad = self.px(PAD);
        let (top, side) = (bar.top + pad, bar.height() - 2 * pad);
        self.grip = Rect::from_size(bar.left + pad, top, self.px(GRIP_WIDTH), side);
        let chip = Rect::from_size(self.grip.right, top, self.px(CHIP_WIDTH), side);
        self.stop_button = Rect::from_size(chip.left, top, side, side);
        let button = self.px(BUTTON);
        let y = bar.top + (bar.height() - button) / 2;
        let step = self.px(GAP) + button;
        self.discard_button = Rect::from_size(bar.right - pad - button, y, button, button);
        let offset = |n: f32| ((step as f32) * n * open).round() as i32;
        self.pause_button =
            Rect::from_size(self.discard_button.left - offset(1.0), y, button, button);
        self.restart_button =
            Rect::from_size(self.discard_button.left - offset(2.0), y, button, button);
    }

    fn chip(&self) -> Rect {
        Rect::new(
            self.grip.right,
            self.grip.top,
            self.grip.right + self.px(CHIP_WIDTH),
            self.grip.bottom,
        )
    }

    /// Moves and sizes the window for its current width, around its centre.
    fn place(&mut self) {
        let open = self.open();
        let (w, h) = (self.width(open), self.height());
        self.layout(w, open);
        let x = (self.centre.0 - w / 2)
            .clamp(self.work.left, (self.work.right - w).max(self.work.left));
        self.at = Rect::from_size(x, self.centre.1, w, h);
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

    /// Shows the frame around `region` and the bar next to it, both in
    /// desktop coordinates; the bar stays inside `work` (the monitor
    /// without the taskbar, which would cover it). It counts down for
    /// `countdown` (then shows that the recording is starting) until
    /// [`Self::running`].
    pub fn show(&mut self, region: Rect, work: Rect, scale: f32, countdown: Duration, light: bool) {
        self.scale = scale;
        self.light = light;
        self.hover = None;
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
        let (w, h) = (self.width(1.0), self.height());
        let (x, y) = pill_position(region, work, (w, h), 0);
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
    /// the bar widens for restart and pause.
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

    fn part_at(&self, p: Point) -> Option<Part> {
        let running = matches!(self.phase, Phase::Running { .. }) && self.opening.is_none();
        if self.stop_button.contains(p) {
            Some(Part::Stop)
        } else if self.discard_button.contains(p) {
            Some(Part::Discard)
        } else if running && self.pause_button.contains(p) {
            Some(Part::Pause)
        } else if running && self.restart_button.contains(p) {
            Some(Part::Restart)
        } else {
            None
        }
    }

    /// Handles a message for one of the indicator windows.
    pub fn handle(
        &mut self,
        hwnd: HWND,
        msg: u32,
        lparam: LPARAM,
    ) -> Option<(LRESULT, Option<Request>)> {
        if hwnd != self.pill {
            return (msg == WM_MOUSEACTIVATE).then_some((LRESULT(MA_NOACTIVATE as isize), None));
        }
        match msg {
            WM_MOUSEACTIVATE => Some((LRESULT(MA_NOACTIVATE as isize), None)),
            // The grip moves the bar (the system drags it).
            WM_NCHITTEST => {
                let mut r = RECT::default();
                // SAFETY: our own window.
                let _ = unsafe { GetWindowRect(self.pill, &mut r) };
                let p = point(lparam);
                let local = Point::new(p.x - r.left, p.y - r.top);
                let hit = if self.grip.contains(local) {
                    HTCAPTION
                } else {
                    HTCLIENT
                };
                Some((LRESULT(hit as isize), None))
            }
            WM_EXITSIZEMOVE => {
                let mut r = RECT::default();
                // SAFETY: our own window.
                let _ = unsafe { GetWindowRect(self.pill, &mut r) };
                self.centre = (i32::midpoint(r.left, r.right), r.top);
                self.at = Rect::new(r.left, r.top, r.right, r.bottom);
                Some((LRESULT(0), None))
            }
            WM_TIMER => {
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
            WM_MOUSEMOVE => {
                if !self.tracking {
                    self.tracking = true;
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.pill,
                        dwHoverTime: 0,
                    };
                    // SAFETY: valid structure for our own window.
                    let _ = unsafe { TrackMouseEvent(&mut track) };
                }
                let hover = self.part_at(point(lparam));
                if hover != self.hover {
                    self.hover = hover;
                    self.repaint();
                }
                Some((LRESULT(0), None))
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                if self.hover.take().is_some() {
                    self.repaint();
                }
                Some((LRESULT(0), None))
            }
            WM_LBUTTONUP => {
                let request = match self.part_at(point(lparam)) {
                    Some(Part::Stop) => Some(Request::Stop),
                    Some(Part::Discard) => Some(Request::Discard),
                    Some(Part::Pause) => Some(Request::TogglePause),
                    Some(Part::Restart) => Some(Request::Restart),
                    None => None,
                };
                Some((LRESULT(0), request))
            }
            _ => None,
        }
    }

    /// Draws the bar and puts it on screen.
    fn repaint(&self) {
        if self.phase == Phase::Hidden {
            return;
        }
        let (w, h) = (self.at.width(), self.at.height());
        if w <= 0 || h <= 0 {
            return;
        }
        // SAFETY: the screen DC is released below.
        let screen = unsafe { GetDC(None) };
        if let Some(mut canvas) = Canvas::new(screen, w, h) {
            self.draw(&mut canvas);
            canvas.present_layered(self.pill, screen, Point::new(self.at.left, self.at.top));
        }
        // SAFETY: obtained above.
        unsafe {
            ReleaseDC(None, screen);
        }
    }

    /// Draws the whole window into `canvas`.
    #[allow(clippy::too_many_lines)]
    fn draw(&self, canvas: &mut Canvas) {
        let s = self.scale;
        let p = self.palette();
        let open = self.open();
        let bar = self.bar(canvas.w);
        let radius = bar.height() as f32 / 2.0;
        // The bar is drawn on its own colour everywhere (GDI text needs an
        // opaque background); its shape and shadow become the alpha last.
        canvas.fill(p.panel);

        // Grip: two columns of three dots.
        let g = self.grip;
        let (gx, gy) = (
            g.left as f32 + g.width() as f32 / 2.0,
            g.top as f32 + g.height() as f32 / 2.0,
        );
        for col in [-1.0, 1.0] {
            for row in [-1.0, 0.0, 1.0] {
                canvas.circle(gx + col * 3.0 * s, gy + row * 5.5 * s, 1.6 * s, p.icon, 0.8);
            }
        }

        // The chip: Stop (or the countdown) and the time.
        let chip = self.chip();
        canvas.rounded(chip, chip.height() as f32 / 2.0, p.chip, 1.0);
        let b = self.stop_button;
        let (cx, cy) = (
            b.left as f32 + b.width() as f32 / 2.0,
            b.top as f32 + b.height() as f32 / 2.0,
        );
        let mut text: Option<(String, Rgb, bool)> = None;
        match self.phase {
            Phase::Countdown { until } => {
                let left = until.saturating_duration_since(Instant::now());
                let r = 13.0 * s;
                if left.is_zero() {
                    // Getting the recorder ready: a still red dot.
                    canvas.circle(cx, cy, 6.0 * s, RECORD, 1.0);
                    text = Some(("Starting…".into(), p.muted, false));
                } else {
                    // The seconds left in an accent disc, ringed by the time
                    // left in this second.
                    let secs = left.as_secs() + 1;
                    let frac = (left.as_secs_f32() - left.as_secs() as f32).clamp(0.0, 1.0);
                    canvas.circle(cx, cy, r, ACCENT, 1.0);
                    canvas.ring(cx, cy, r - 2.2 * s, 2.0 * s, frac, WHITE, 0.9);
                    canvas.flush();
                    canvas.text_at(
                        &secs.to_string(),
                        Rect::new(
                            (cx - r) as i32,
                            (cy - r) as i32,
                            (cx + r) as i32,
                            (cy + r) as i32,
                        ),
                        (13.0 * s) as i32,
                        WHITE,
                        true,
                        true,
                    );
                    text = Some(("Get ready".into(), p.muted, false));
                }
            }
            Phase::Running {
                started,
                paused_since,
                ..
            } => {
                // Stop: a red disc with a white square; it pulses softly
                // while recording, and turns amber when paused.
                let r = 13.0 * s;
                let hovered = self.hover == Some(Part::Stop);
                if paused_since.is_some() {
                    canvas.circle(cx, cy, r, AMBER, 1.0);
                } else {
                    let t = started.elapsed().as_secs_f32();
                    let pulse = 0.5 + 0.5 * (t * std::f32::consts::TAU / 1.6).cos();
                    canvas.circle(cx, cy, r + 3.0 * s * pulse, RECORD, 0.2 * pulse);
                    canvas.circle(cx, cy, r, RECORD, 1.0);
                }
                if hovered {
                    canvas.circle(cx, cy, r, [0, 0, 0], 0.12);
                }
                let square = 9.0 * s;
                canvas.rounded_f(
                    cx - square / 2.0,
                    cy - square / 2.0,
                    square,
                    square,
                    2.0 * s,
                    WHITE,
                    1.0,
                );
                let secs = self.phase.elapsed().as_secs();
                text = Some((format!("{:02}:{:02}", secs / 60, secs % 60), p.text, true));
            }
            Phase::Hidden => {}
        }

        // Restart and Pause (sliding in), Delete.
        let radius_button = 10.0 * s;
        let paused = matches!(
            self.phase,
            Phase::Running {
                paused_since: Some(_),
                ..
            }
        );
        if open > 0.0 {
            // Uncovered from under Delete as the bar widens.
            for (part, rect) in [
                (Part::Restart, self.restart_button),
                (Part::Pause, self.pause_button),
            ] {
                if self.hover == Some(part) {
                    canvas.rounded(rect, radius_button, p.hover, open);
                }
                match part {
                    Part::Restart => canvas.restart_icon(rect, p.icon, open),
                    _ => canvas.pause_icon(rect, paused, p.icon, open),
                }
            }
        }
        let d = self.discard_button;
        // Covers the buttons sliding out from under it.
        canvas.rounded(d, radius_button, p.panel, 1.0);
        if self.hover == Some(Part::Discard) {
            canvas.rounded(d, radius_button, p.hover, 1.0);
        }
        let bin_back = if self.hover == Some(Part::Discard) {
            p.hover
        } else {
            p.panel
        };
        canvas.trash(d, p.icon, bin_back);

        // Text goes on last, with GDI, over the finished picture.
        canvas.flush();
        if let Some((label, color, big)) = text {
            let size = if big { 16.0 } else { 13.0 };
            canvas.text_at(
                &label,
                Rect::new(
                    b.right + self.px(8.0),
                    chip.top,
                    chip.right - self.px(8.0),
                    chip.bottom,
                ),
                (size * s) as i32,
                color,
                big,
                false,
            );
        }
        // The shape: a pill with a hairline and a soft shadow underneath.
        let (border, border_alpha) = p.border;
        canvas.finish(
            bar,
            radius,
            border,
            border_alpha,
            p.shadow,
            SHADOW * s * 0.8,
        );
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
    fn trash(&mut self, b: Rect, color: Rgb, back: Rgb) {
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
                back,
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

    /// A filled triangle.
    fn triangle(&mut self, a: (f32, f32), b: (f32, f32), c: (f32, f32), color: Rgb, alpha: f32) {
        // Inside must be negative: flip the edges if the centre is not.
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        let (b, c) = if area < 0.0 { (c, b) } else { (b, c) };
        let edge = |p: (f32, f32), q: (f32, f32), x: f32, y: f32| {
            let (dx, dy) = (q.0 - p.0, q.1 - p.1);
            ((x - p.0) * dy - (y - p.1) * dx) / dx.hypot(dy)
        };
        let bounds = (
            a.0.min(b.0).min(c.0),
            a.1.min(b.1).min(c.1),
            a.0.max(b.0).max(c.0),
            a.1.max(b.1).max(c.1),
        );
        self.shade(bounds, color, alpha, |x, y| {
            edge(a, b, x, y).max(edge(b, c, x, y)).max(edge(c, a, x, y))
        });
    }

    /// A circular arrow, going clockwise: start again.
    fn restart_icon(&mut self, b: Rect, color: Rgb, alpha: f32) {
        let size = b.width() as f32 * 0.42;
        let (cx, cy) = (
            b.left as f32 + b.width() as f32 / 2.0,
            b.top as f32 + b.height() as f32 / 2.0,
        );
        let r = size / 2.0 - size * 0.08;
        let thick = size * 0.15;
        let share = 0.76;
        self.ring(cx, cy, r, thick, share, color, alpha);
        let angle = share * std::f32::consts::TAU;
        let end = (cx + r * angle.sin(), cy - r * angle.cos());
        let along = (angle.cos(), angle.sin());
        let out = (angle.sin(), -angle.cos());
        let head = size * 0.36;
        self.triangle(
            (end.0 + along.0 * head * 0.7, end.1 + along.1 * head * 0.7),
            (end.0 + out.0 * head * 0.6, end.1 + out.1 * head * 0.6),
            (end.0 - out.0 * head * 0.6, end.1 - out.1 * head * 0.6),
            color,
            alpha,
        );
    }

    /// Turns the picture into the bar: opaque inside the rounded rectangle
    /// `bar` (with a hairline `border` along its edge), a soft shadow
    /// `blur` wide below it, transparent elsewhere; premultiplied, as
    /// layered windows expect.
    fn finish(
        &mut self,
        bar: Rect,
        radius: f32,
        border: Rgb,
        border_alpha: f32,
        shadow: f32,
        blur: f32,
    ) {
        let (cx, cy) = (
            (bar.left + bar.right) as f32 / 2.0,
            (bar.top + bar.bottom) as f32 / 2.0,
        );
        let (hw, hh) = (bar.width() as f32 / 2.0, bar.height() as f32 / 2.0);
        let r = radius.min(hw).min(hh);
        let sdf = |x: f32, y: f32| {
            let qx = (x - cx).abs() - (hw - r);
            let qy = (y - cy).abs() - (hh - r);
            qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
        };
        let drop = blur * 0.3;
        let w = self.w;
        let h = self.h;
        let pixels = self.pixels();
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = sdf(fx, fy);
                let inside = (0.5 - d).clamp(0.0, 1.0);
                let line = (1.0 - (d + 0.5).abs()).clamp(0.0, 1.0) * border_alpha;
                let ds = sdf(fx, fy - drop).max(0.0);
                let cast = shadow * (1.0 - (ds / blur.max(1.0))).clamp(0.0, 1.0).powi(2);
                let alpha = inside + (1.0 - inside) * cast;
                let i = ((y * w + x) * 4) as usize;
                for (c, v) in [border[2], border[1], border[0]].into_iter().enumerate() {
                    let base = f32::from(pixels[i + c]);
                    let lined = base + (f32::from(v) - base) * line;
                    // Premultiplied: the shadow is black.
                    pixels[i + c] = (lined * inside).round() as u8;
                }
                pixels[i + 3] = (alpha * 255.0).round() as u8;
            }
        }
    }

    /// Puts the picture on screen as `hwnd`'s content, at `at`.
    fn present_layered(&self, hwnd: HWND, screen: HDC, at: Point) {
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: both DCs are valid; the bitmap is premultiplied BGRA.
        unsafe {
            let _ = UpdateLayeredWindow(
                hwnd,
                Some(screen),
                Some(&POINT { x: at.x, y: at.y }),
                Some(&SIZE {
                    cx: self.w,
                    cy: self.h,
                }),
                Some(self.dc),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
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
