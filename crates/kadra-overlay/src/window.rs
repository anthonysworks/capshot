//! The overlay window and its thread.
//!
//! Everything lives on one dedicated thread: the Win32 window, the Direct2D
//! device context and the capture sessions. The window is created once,
//! hidden, and reused for every capture, so a capture only has to grab the
//! screen and present one frame.

// Win32 message parameters and structure sizes are packed integers.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::Instant;

use kadra_capture::{Capturer, Gpu, GpuKind, Snapshot};
use kadra_core::{BgraImage, Point, RecordingFormat, Rect};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, TRUE, WPARAM};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAK, DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT, DXGI_SCALING_NONE, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1,
};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, HCURSOR, HTCLIENT,
    HWND_TOPMOST, IDC_ARROW, IDC_CROSS, LoadCursorW, MSG, PostMessageW, PostQuitMessage,
    RegisterClassExW, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SetCursor, SetCursorPos,
    SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage, WA_INACTIVE, WM_ACTIVATE,
    WM_APP, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WNDCLASSEXW,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{BOOL, Interface, w};

use crate::clipboard;
use crate::indicator::{self, Indicator};
use crate::layout::{Button, Magnifier, Toolbar};
use crate::preview::{self, Preview};
use crate::render::{Renderer, Scene, Screenshot};
use crate::selection::{Outcome, Selection, Targets};
use crate::{CaptureRequest, CaptureResult, Command, Purpose, ResultHandler};

/// A capture request is waiting in the channel.
pub const WM_KADRA_REQUEST: u32 = WM_APP + 1;
/// Prepare the next capture (sent to self once the overlay is closed).
const WM_KADRA_PREPARE: u32 = WM_APP + 2;
/// Shut the overlay thread down.
pub const WM_KADRA_QUIT: u32 = WM_APP + 3;

impl Targets for Snapshot {
    fn window_at(&self, p: Point) -> Option<Rect> {
        Snapshot::window_at(self, p).map(|w| w.rect)
    }

    fn monitor_at(&self, p: Point) -> Rect {
        Snapshot::monitor_at(self, p).rect
    }

    fn bounds(&self) -> Rect {
        Snapshot::bounds(self)
    }
}

/// GPU objects, rebuilt from scratch if the device is lost.
struct Engine {
    capturer: Capturer,
    renderer: Renderer,
    swap: Option<SwapChain>,
}

impl Engine {
    /// Screenshots run on WARP, the software Direct3D device: every pixel
    /// ends up in system memory anyway, and it saves the ~50 MB and ~150 ms
    /// a GPU driver costs, for the same end-to-end latency.
    /// `KADRA_GPU=hardware` uses the graphics card instead.
    fn new() -> windows::core::Result<Self> {
        let kind = if std::env::var("KADRA_GPU").is_ok_and(|v| v.eq_ignore_ascii_case("hardware")) {
            GpuKind::Hardware
        } else {
            GpuKind::Software
        };
        let gpu = Gpu::with_kind(kind)?;
        let renderer = Renderer::new(&gpu)?;
        if let Err(err) = renderer.warm_up() {
            tracing::warn!("overlay warm-up failed: {err}");
        }
        let capturer = Capturer::new(gpu);
        capturer.warm_up();
        Ok(Self {
            renderer,
            capturer,
            swap: None,
        })
    }
}

struct SwapChain {
    chain: IDXGISwapChain1,
    size: (u32, u32),
    target: Option<ID2D1Bitmap1>,
}

impl SwapChain {
    fn new(gpu: &Gpu, hwnd: HWND, size: (u32, u32)) -> windows::core::Result<Self> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.0,
            Height: size.1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_NONE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
            ..Default::default()
        };
        // SAFETY: the factory is the one that created the device's adapter.
        unsafe {
            let dxgi: IDXGIDevice = gpu.device().cast()?;
            let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
            let chain = factory.CreateSwapChainForHwnd(gpu.device(), hwnd, &desc, None, None)?;
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            Ok(Self {
                chain,
                size,
                target: None,
            })
        }
    }

    /// Resizes the buffers (target must be unbound first).
    fn resize(&mut self, renderer: &Renderer, size: (u32, u32)) -> windows::core::Result<()> {
        if self.size == size {
            return Ok(());
        }
        self.unbind(renderer);
        // SAFETY: no outstanding references to the back buffers remain.
        unsafe {
            self.chain.ResizeBuffers(
                0,
                size.0,
                size.1,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        self.size = size;
        Ok(())
    }

    fn bind(&mut self, renderer: &Renderer) -> windows::core::Result<()> {
        if self.target.is_none() {
            // SAFETY: buffer 0 is the current back buffer of a flip chain.
            let target = unsafe {
                let surface: IDXGISurface = self.chain.GetBuffer(0)?;
                renderer
                    .context()
                    .CreateBitmapFromDxgiSurface(&surface, Some(&Renderer::target_props()))?
            };
            self.target = Some(target);
        }
        // SAFETY: the bitmap belongs to this device context.
        unsafe {
            if let Some(target) = &self.target {
                renderer.context().SetTarget(target);
            }
        }
        Ok(())
    }

    fn unbind(&mut self, renderer: &Renderer) {
        // SAFETY: clearing the target is always valid.
        unsafe { renderer.context().SetTarget(None) };
        self.target = None;
    }

    fn present(&self) -> windows::core::Result<()> {
        // SAFETY: plain present call.
        unsafe { self.chain.Present(1, DXGI_PRESENT(0)).ok() }
    }
}

/// A capture in progress.
struct Session {
    snapshot: Snapshot,
    shot: Screenshot,
    selection: Selection,
    toolbar: Toolbar,
    toolbar_hover: Option<Button>,
    /// Monitor the toolbar sits on and its scale, to rebuild it.
    toolbar_monitor: (Rect, f32),
    /// Switchable from the toolbar, unlike the rest of the request.
    purpose: Purpose,
    format: RecordingFormat,
    system_audio: bool,
    microphone: bool,
    request: CaptureRequest,
}

impl Session {
    fn scene(&self) -> Scene<'_> {
        let cursor = self.selection.cursor();
        let monitor = self.snapshot.monitor_at(cursor);
        let over_toolbar = !self.selection.is_active() && self.toolbar.contains(cursor);
        Scene {
            selection: &self.selection,
            hover: (!over_toolbar)
                .then(|| self.selection.hover_rect(&self.snapshot))
                .flatten(),
            toolbar: (!self.selection.is_active()).then_some(&self.toolbar),
            toolbar_hover: self.toolbar_hover,
            magnifier: (!over_toolbar)
                .then(|| Magnifier::new(cursor, monitor.rect, monitor.scale())),
            pixel: self.snapshot.image.pixel(cursor),
            scale: monitor.scale(),
            bounds: self.snapshot.bounds(),
            purpose: self.purpose,
            format: self.format,
            system_audio: self.system_audio,
            microphone: self.microphone,
        }
    }

    fn set_purpose(&mut self, purpose: Purpose) {
        if self.purpose == purpose {
            return;
        }
        self.purpose = purpose;
        self.rebuild_toolbar();
        let mode = purpose.sanitize(self.selection.mode());
        self.selection.set_mode(mode);
    }

    fn toggle_gif(&mut self) {
        self.format = match self.format {
            RecordingFormat::Mp4 => RecordingFormat::Gif,
            RecordingFormat::Gif => RecordingFormat::Mp4,
        };
        self.rebuild_toolbar();
    }

    fn rebuild_toolbar(&mut self) {
        let (monitor, scale) = self.toolbar_monitor;
        self.toolbar = Toolbar::new(monitor, scale, self.purpose, self.format);
        self.toolbar_hover = None;
    }

    /// Switches to `mode`, and to taking a screenshot if recording does
    /// not offer it (e.g. the colour picker).
    fn set_mode(&mut self, mode: crate::Mode) {
        if !self.purpose.modes().contains(&mode) {
            self.set_purpose(Purpose::Screenshot);
        }
        self.selection.set_mode(mode);
    }

    fn crop(&self, outcome: Outcome) -> BgraImage {
        match outcome {
            Outcome::Region(rect) => self.snapshot.image.crop(rect),
            Outcome::Pick(_) => BgraImage::zeroed(0, 0),
            Outcome::Freeform(points) => {
                let Some(bounds) = Rect::bounding(&points) else {
                    return BgraImage::zeroed(0, 0);
                };
                let mut image = self.snapshot.image.crop(bounds);
                let local: Vec<Point> = points
                    .iter()
                    .map(|p| p.offset(-bounds.left, -bounds.top))
                    .collect();
                image.mask_outside_polygon(&local);
                image
            }
        }
    }
}

struct State {
    hwnd: HWND,
    engine: Option<Engine>,
    session: Option<Session>,
    commands: Receiver<Command>,
    handler: ResultHandler,
    preview: Preview,
    indicator: Indicator,
    next_capture_id: u64,
    cross: HCURSOR,
    arrow: HCURSOR,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn lparam_point(lparam: LPARAM) -> Point {
    #[allow(clippy::cast_possible_truncation)]
    let (x, y) = (
        lparam.0 as u32 as u16 as i16,
        (lparam.0 as u32 >> 16) as u16 as i16,
    );
    Point::new(i32::from(x), i32::from(y))
}

fn cloak(hwnd: HWND, cloaked: bool) {
    let value = BOOL::from(cloaked);
    // SAFETY: `value` outlives the call and has the documented size.
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            std::ptr::from_ref(&value).cast::<c_void>(),
            size_of::<BOOL>() as u32,
        )
    };
}

/// Brings the overlay to the foreground so it receives the keyboard.
///
/// Windows only lets the process that received the last input take the
/// foreground. That is the case for hotkeys and tray clicks; for other
/// triggers (command line, automation) the input queues are attached to the
/// current foreground thread for the duration of the switch.
fn take_focus(hwnd: HWND) {
    // SAFETY: plain window-management calls; the input queues are always
    // detached again.
    unsafe {
        if SetForegroundWindow(hwnd).as_bool() {
            let _ = SetFocus(Some(hwnd));
            return;
        }
        let foreground = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let current = GetCurrentThreadId();
        if foreground != 0
            && foreground != current
            && AttachThreadInput(current, foreground, true).as_bool()
        {
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            let _ = AttachThreadInput(current, foreground, false);
        }
    }
}

/// Monitor containing the centre of `region` (desktop coordinates): its
/// area, its work area (without the taskbar) and its UI scale.
fn monitor_of(region: Rect) -> (Rect, Rect, f32) {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    let centre = windows::Win32::Foundation::POINT {
        x: region.left + region.width() / 2,
        y: region.top + region.height() / 2,
    };
    // SAFETY: plain queries with correctly sized structures.
    unsafe {
        let monitor = MonitorFromPoint(centre, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dpi, mut unused) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut unused);
        let to_rect =
            |r: windows::Win32::Foundation::RECT| Rect::new(r.left, r.top, r.right, r.bottom);
        #[allow(clippy::cast_precision_loss)]
        let scale = dpi as f32 / 96.0;
        (to_rect(info.rcMonitor), to_rect(info.rcWork), scale)
    }
}

fn shift_down() -> bool {
    // SAFETY: plain query.
    unsafe { GetKeyState(i32::from(VK_SHIFT.0)) < 0 }
}

impl State {
    fn handle(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_KADRA_REQUEST => {
                // Only the latest pending capture request matters.
                let mut capture = None;
                for command in self.commands.try_iter().collect::<Vec<_>>() {
                    match command {
                        Command::Capture(request) => capture = Some(request),
                        Command::AttachFile { capture_id, path } => {
                            self.preview.attach_file(capture_id, path);
                        }
                        Command::CopyImage(image) => {
                            if let Err(err) = clipboard::set_image(self.hwnd, &image) {
                                tracing::warn!("cannot copy to the clipboard: {err}");
                            }
                        }
                        Command::ShowIndicator { region, countdown } => {
                            let (monitor, _, scale) = monitor_of(region);
                            self.indicator.show(region, monitor, scale, countdown);
                        }
                        Command::IndicatorRunning => self.indicator.running(),
                        Command::IndicatorPaused(paused) => self.indicator.set_paused(paused),
                        Command::ShowFile {
                            image,
                            path,
                            region,
                        } => {
                            let (_, work, scale) = monitor_of(region);
                            self.next_capture_id += 1;
                            let id = self.next_capture_id;
                            self.preview.show(id, &image, work, scale);
                            self.preview.attach_file(id, path);
                        }
                        Command::HideIndicator => self.indicator.hide(),
                    }
                }
                if let Some(request) = capture {
                    self.start(request);
                }
            }
            WM_KADRA_PREPARE => {
                if self.session.is_none()
                    && let Some(engine) = &self.engine
                {
                    engine.capturer.prepare();
                }
            }
            WM_KADRA_QUIT => {
                // SAFETY: our own window, on its own thread.
                let _ = unsafe { DestroyWindow(self.hwnd) };
            }
            WM_DESTROY => {
                // SAFETY: called on the window thread.
                unsafe { PostQuitMessage(0) };
            }
            // Right click cancels. The window is never destroyed by the user:
            // it is reused for every capture.
            WM_CLOSE | WM_RBUTTONUP => self.finish(None),
            // The window deliberately spans monitors with different DPIs.
            WM_DPICHANGED => {}
            WM_DISPLAYCHANGE => {
                let _ = self.post(WM_KADRA_PREPARE);
                return None;
            }
            WM_PAINT => {
                // SAFETY: validating our own window.
                let _ = unsafe { ValidateRect(Some(self.hwnd), None) };
                self.render();
            }
            WM_SETCURSOR if self.session.is_some() => {
                #[allow(clippy::cast_possible_truncation)]
                if (lparam.0 & 0xFFFF) as u32 != HTCLIENT {
                    return None;
                }
                let over_toolbar = self.session.as_ref().is_some_and(|s| {
                    !s.selection.is_active() && s.toolbar.contains(s.selection.cursor())
                });
                // SAFETY: shared system cursors.
                unsafe { SetCursor(Some(if over_toolbar { self.arrow } else { self.cross })) };
                return Some(LRESULT(TRUE.0 as isize));
            }
            WM_MOUSEMOVE => self.on_move(lparam_point(lparam)),
            WM_LBUTTONDOWN => self.on_press(lparam_point(lparam)),
            WM_LBUTTONUP => self.on_release(lparam_point(lparam)),
            WM_KEYDOWN => self.on_key(wparam),
            WM_ACTIVATE => {
                #[allow(clippy::cast_possible_truncation)]
                if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                    self.finish(None);
                }
                return None;
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }

    fn post(&self, msg: u32) -> windows::core::Result<()> {
        // SAFETY: posting to our own window.
        unsafe { PostMessageW(Some(self.hwnd), msg, WPARAM(0), LPARAM(0)) }
    }

    fn invalidate(&self) {
        // SAFETY: our own window.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    fn start(&mut self, request: CaptureRequest) {
        if self.session.is_some() {
            return;
        }
        self.preview.hide();
        match self.try_start(request) {
            Ok(latency) => tracing::info!(latency_ms = latency, "capture overlay shown"),
            Err(err) => {
                tracing::error!("cannot start capture: {err}");
                self.reset_engine();
                (self.handler)(CaptureResult::Failed(err.to_string()));
            }
        }
    }

    /// Captures the screen and shows the overlay. Returns the latency.
    fn try_start(&mut self, request: CaptureRequest) -> Result<f64, Box<dyn std::error::Error>> {
        if self.engine.is_none() {
            self.engine = Some(Engine::new()?);
        }
        let hwnd = self.hwnd;
        let engine = self.engine.as_mut().ok_or("no engine")?;

        let t0 = Instant::now();
        let snapshot = engine.capturer.snapshot(Some(hwnd))?;
        let t_capture = t0.elapsed();
        let max = engine.renderer.max_bitmap_size();
        if snapshot.image.width() > max || snapshot.image.height() > max {
            return Err(format!(
                "the desktop ({}×{}) exceeds the GPU texture limit ({max})",
                snapshot.image.width(),
                snapshot.image.height()
            )
            .into());
        }
        let shot = engine.renderer.load_screenshot(&snapshot.image)?;
        let t_upload = t0.elapsed();

        let mut cursor = windows::Win32::Foundation::POINT::default();
        // SAFETY: valid out pointer.
        unsafe { GetCursorPos(&mut cursor)? };
        let cursor = Point::new(cursor.x - snapshot.origin.x, cursor.y - snapshot.origin.y);
        let monitor = snapshot.monitor_at(cursor);
        let purpose = request.purpose;
        let toolbar = Toolbar::new(monitor.rect, monitor.scale(), purpose, request.format);
        let toolbar_monitor = (monitor.rect, monitor.scale());
        let size = (snapshot.image.width(), snapshot.image.height());
        let origin = snapshot.origin;

        // Resize and draw while cloaked so the old frame never flashes.
        cloak(hwnd, true);
        #[allow(clippy::cast_possible_wrap)]
        // SAFETY: our own window.
        unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                origin.x,
                origin.y,
                size.0 as i32,
                size.1 as i32,
                SWP_NOACTIVATE,
            )?;
        }

        match &mut engine.swap {
            Some(swap) => swap.resize(&engine.renderer, size)?,
            None => engine.swap = Some(SwapChain::new(engine.capturer.gpu(), hwnd, size)?),
        }

        self.session = Some(Session {
            selection: Selection::new(purpose.sanitize(request.mode), cursor),
            snapshot,
            shot,
            toolbar,
            toolbar_hover: None,
            toolbar_monitor,
            purpose,
            format: request.format,
            system_audio: request.system_audio,
            microphone: request.microphone,
            request,
        });

        // SAFETY: our own window, on its thread.
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let t_shown = t0.elapsed();
        self.render();
        let t_rendered = t0.elapsed();
        cloak(hwnd, false);
        take_focus(hwnd);
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
        tracing::debug!(
            capture_ms = ms(t_capture),
            upload_ms = ms(t_upload.saturating_sub(t_capture)),
            window_ms = ms(t_shown.saturating_sub(t_upload)),
            render_ms = ms(t_rendered.saturating_sub(t_shown)),
            focus_ms = ms(t0.elapsed().saturating_sub(t_rendered)),
            "overlay timings"
        );

        let latency = self.session.as_ref().map_or(0.0, |s| {
            s.request.requested_at.elapsed().as_secs_f64() * 1000.0
        });
        Ok(latency)
    }

    fn render(&mut self) {
        let (Some(engine), Some(session)) = (self.engine.as_mut(), self.session.as_ref()) else {
            return;
        };
        let Some(swap) = engine.swap.as_mut() else {
            return;
        };
        let result = swap
            .bind(&engine.renderer)
            .and_then(|()| engine.renderer.draw(&session.scene(), &session.shot))
            .and_then(|()| swap.present());
        if let Err(err) = result {
            tracing::error!("overlay rendering failed: {err}");
            self.finish(None);
            self.reset_engine();
        }
    }

    /// Drops every GPU object; they are recreated on the next capture.
    fn reset_engine(&mut self) {
        self.engine = None;
    }

    fn on_move(&mut self, p: Point) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.selection.moved(p, &session.snapshot);
        session.toolbar_hover = if session.selection.is_active() {
            None
        } else {
            session.toolbar.hit(p)
        };
        self.invalidate();
    }

    fn on_press(&mut self, p: Point) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.selection.is_active() && session.toolbar.contains(p) {
            match session.toolbar.hit(p) {
                Some(Button::Purpose(purpose)) => session.set_purpose(purpose),
                Some(Button::Mode(mode)) => session.set_mode(mode),
                Some(Button::Gif) => session.toggle_gif(),
                Some(Button::SystemAudio) => session.system_audio = !session.system_audio,
                Some(Button::Microphone) => session.microphone = !session.microphone,
                Some(Button::Close) => return self.finish(None),
                None => {}
            }
            self.invalidate();
            return;
        }
        session.selection.press(p, &session.snapshot);
        // SAFETY: our own window.
        unsafe { SetCapture(self.hwnd) };
        self.invalidate();
    }

    fn on_release(&mut self, p: Point) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.selection.is_active() {
            return;
        }
        // SAFETY: releasing our own capture.
        let _ = unsafe { ReleaseCapture() };
        let outcome = session.selection.release(p, &session.snapshot);
        match outcome {
            Some(outcome) => self.finish(Some(outcome)),
            None => self.invalidate(),
        }
    }

    fn on_key(&mut self, wparam: WPARAM) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        #[allow(clippy::cast_possible_truncation)]
        let key = wparam.0 as u16;
        match key {
            k if k == VK_ESCAPE.0 => {
                if session.selection.abort() {
                    // SAFETY: releasing our own capture.
                    let _ = unsafe { ReleaseCapture() };
                    self.invalidate();
                } else {
                    self.finish(None);
                }
            }
            k if k == VK_RETURN.0 => {
                if let Some(outcome) = session.selection.confirm_hover(&session.snapshot) {
                    self.finish(Some(outcome));
                }
            }
            0x52 /* R */ => self.set_mode(crate::Mode::Region),
            0x46 /* F */ => self.set_mode(crate::Mode::Freeform),
            0x53 /* S */ => self.set_mode(crate::Mode::Screen),
            0x49 /* I */ => self.set_mode(crate::Mode::Picker),
            0x43 /* C */ => {
                let cursor = session.selection.cursor();
                self.finish(Some(Outcome::Pick(cursor)));
            }
            k if k == 0x56 /* V */ || k == VK_TAB.0 => {
                let purpose = session.purpose.toggled();
                session.set_purpose(purpose);
                self.invalidate();
            }
            0x47 /* G */ => {
                if session.purpose == Purpose::Recording {
                    session.toggle_gif();
                    self.invalidate();
                }
            }
            0x41 /* A */ => {
                session.system_audio = !session.system_audio;
                self.invalidate();
            }
            0x4D /* M */ => {
                session.microphone = !session.microphone;
                self.invalidate();
            }
            k if [VK_LEFT.0, VK_RIGHT.0, VK_UP.0, VK_DOWN.0].contains(&k) => {
                // Pixel-precise positioning: 1 px, or 10 px with Shift.
                let step = if shift_down() { 10 } else { 1 };
                let (dx, dy) = match k {
                    k if k == VK_LEFT.0 => (-step, 0),
                    k if k == VK_RIGHT.0 => (step, 0),
                    k if k == VK_UP.0 => (0, -step),
                    _ => (0, step),
                };
                let mut pos = windows::Win32::Foundation::POINT::default();
                // SAFETY: valid out pointer; moving the cursor generates a
                // regular WM_MOUSEMOVE.
                unsafe {
                    if GetCursorPos(&mut pos).is_ok() {
                        let _ = SetCursorPos(pos.x + dx, pos.y + dy);
                    }
                }
            }
            _ => {}
        }
    }

    fn set_mode(&mut self, mode: crate::Mode) {
        if let Some(session) = self.session.as_mut() {
            session.set_mode(mode);
            self.invalidate();
        }
    }

    /// Copies the colour of pixel `at` as `#RRGGBB` and confirms it with a
    /// swatch in the corner of the screen.
    fn copy_colour(&mut self, session: &Session, at: Point) -> CaptureResult {
        let Some([b, g, r, a]) = session.snapshot.image.pixel(at) else {
            return CaptureResult::Cancelled;
        };
        if a == 0 {
            return CaptureResult::Cancelled;
        }
        let hex = format!("#{r:02X}{g:02X}{b:02X}");
        if let Err(err) = clipboard::set_text(self.hwnd, &hex) {
            return CaptureResult::Failed(err.to_string());
        }
        if session.request.show_preview {
            let monitor = session.snapshot.monitor_at(at);
            let origin = session.snapshot.origin;
            let work = monitor.work.offset(origin.x, origin.y);
            self.preview
                .show_colour(&hex, [r, g, b], work, monitor.scale());
        }
        CaptureResult::ColorCopied(hex)
    }

    /// Returns idle GPU memory to the system once a capture is over.
    fn trim(&self) {
        if let Some(engine) = &self.engine {
            engine.renderer.trim();
            engine.capturer.gpu().trim();
        }
    }

    /// Hides the overlay and frees the capture.
    fn close(&mut self) -> Option<Session> {
        let session = self.session.take()?;
        // SAFETY: our own window, on its thread.
        unsafe {
            let _ = ReleaseCapture();
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        // Release the big back buffers while idle; recreating the swap chain
        // costs about a millisecond.
        if let Some(engine) = self.engine.as_mut()
            && let Some(mut swap) = engine.swap.take()
        {
            swap.unbind(&engine.renderer);
        }
        Some(session)
    }

    fn finish(&mut self, outcome: Option<Outcome>) {
        let Some(session) = self.close() else {
            return;
        };
        let result = match outcome {
            None => CaptureResult::Cancelled,
            Some(Outcome::Pick(p)) => self.copy_colour(&session, p),
            // Recording: hand the area over, in desktop coordinates.
            Some(outcome) if session.purpose == Purpose::Recording => {
                let origin = session.snapshot.origin;
                let area = match outcome {
                    Outcome::Region(rect) => Some(rect),
                    Outcome::Freeform(points) => Rect::bounding(&points),
                    Outcome::Pick(_) => None,
                };
                area.filter(|r| !r.is_empty())
                    .map_or(CaptureResult::Cancelled, |r| {
                        CaptureResult::RegionSelected {
                            region: r.offset(origin.x, origin.y),
                            format: session.format,
                            system_audio: session.system_audio,
                            microphone: session.microphone,
                        }
                    })
            }
            Some(outcome) => {
                let image = session.crop(outcome);
                if image.width() == 0 || image.height() == 0 {
                    CaptureResult::Cancelled
                } else {
                    let copied = session.request.copy_to_clipboard
                        && match clipboard::set_image(self.hwnd, &image) {
                            Ok(()) => true,
                            Err(err) => {
                                tracing::warn!("cannot copy to the clipboard: {err}");
                                false
                            }
                        };
                    self.next_capture_id += 1;
                    let id = self.next_capture_id;
                    if session.request.show_preview {
                        let monitor = session.snapshot.monitor_at(session.selection.cursor());
                        let origin = session.snapshot.origin;
                        let work = monitor.work.offset(origin.x, origin.y);
                        self.preview.show(id, &image, work, monitor.scale());
                    }
                    CaptureResult::Captured { id, image, copied }
                }
            }
        };
        // Free the desktop copy (CPU and GPU) before handing the result over.
        drop(session);
        self.trim();
        (self.handler)(result);
        let _ = self.post(WM_KADRA_PREPARE);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Messages sent re-entrantly (e.g. by ShowWindow while the state is
    // borrowed) get the default behaviour.
    let handled = STATE.with(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        let state = state.as_mut()?;
        if hwnd == state.preview.hwnd() {
            let result = state.preview.handle(msg, wparam, lparam);
            if let Some(path) = state.preview.take_clicked() {
                (state.handler)(CaptureResult::OpenFile(path));
            }
            result
        } else if state.indicator.owns(hwnd) {
            let (result, request) = state.indicator.handle(hwnd, msg, lparam)?;
            match request {
                Some(indicator::Request::Stop) => (state.handler)(CaptureResult::StopRecording),
                Some(indicator::Request::TogglePause) => {
                    (state.handler)(CaptureResult::TogglePause);
                }
                None => {}
            }
            Some(result)
        } else {
            state.handle(msg, wparam, lparam)
        }
    });
    // SAFETY: forwarding the original arguments.
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}

/// Thread entry point. Reports the window handle (or an error) through
/// `ready`, then runs the message loop until [`WM_KADRA_QUIT`].
#[allow(clippy::needless_pass_by_value)]
pub fn run(
    ready: SyncSender<Result<isize, String>>,
    commands: Receiver<Command>,
    handler: ResultHandler,
) {
    let init = || -> windows::core::Result<(HWND, HWND, Indicator, Engine)> {
        // SAFETY: thread-level initialisation calls with valid arguments.
        unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // A single-threaded apartment with OLE: required by drag and drop
            // from the preview. The capture APIs used here are agile.
            OleInitialize(None)?;
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: w!("KadraCaptureOverlay"),
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 {
                return Err(windows::core::Error::from_thread());
            }
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
                w!("KadraCaptureOverlay"),
                w!("Kadra capture"),
                WS_POPUP,
                0,
                0,
                8,
                8,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let disabled = TRUE;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED,
                std::ptr::from_ref(&disabled).cast::<c_void>(),
                size_of::<BOOL>() as u32,
            );
            if !preview::register_class(instance.into(), Some(wndproc)) {
                return Err(windows::core::Error::from_thread());
            }
            let preview = preview::create_window(instance.into())?;
            if !indicator::register_classes(instance.into(), Some(wndproc)) {
                return Err(windows::core::Error::from_thread());
            }
            let indicator = Indicator::new(instance.into())?;
            Ok((hwnd, preview, indicator, Engine::new()?))
        }
    };

    let (hwnd, preview_hwnd, indicator, engine) = match init() {
        Ok(v) => v,
        Err(err) => {
            let _ = ready.send(Err(err.to_string()));
            return;
        }
    };

    // SAFETY: system cursors are shared and never freed.
    let (cross, arrow) = unsafe {
        (
            LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        )
    };
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            hwnd,
            engine: Some(engine),
            session: None,
            commands,
            handler,
            preview: Preview::new(preview_hwnd),
            indicator,
            next_capture_id: 0,
            cross,
            arrow,
        });
    });
    let _ = ready.send(Ok(hwnd.0 as isize));

    let mut msg = MSG::default();
    // SAFETY: standard message loop on the window thread.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    STATE.with(|s| s.borrow_mut().take());
}
