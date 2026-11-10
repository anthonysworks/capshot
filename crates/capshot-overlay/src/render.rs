//! Direct2D drawing of the overlay.
//!
//! The renderer only draws a [`Scene`] into whatever target is set on its
//! device context: the window swap chain at run time, or an off-screen
//! bitmap in tests and examples.

use std::cell::RefCell;
use std::collections::HashMap;

use capshot_capture::Gpu;
use capshot_core::{BgraImage, Point, Rect};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED,
    D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_CPU_READ,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_FACTORY_OPTIONS, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_MAP_OPTIONS_READ, D2D1_ROUNDED_RECT,
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1BitmapBrush1, ID2D1Brush, ID2D1Device,
    ID2D1DeviceContext, ID2D1Factory1, ID2D1PathGeometry1, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::core::{HSTRING, Interface, Result};
use windows_numerics::Vector2;

use crate::layout::{
    Action, ActionBar, Button, MAGNIFIER_CELLS, Magnifier, Toolbar, size_label, size_label_above,
};
use crate::selection::{Mode, Purpose, Selection, Targets};
use capshot_core::RecordingFormat;

const fn rgba(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

/// Blue accent: screenshots, the active tool.
const ACCENT: D2D1_COLOR_F = rgba(0x25, 0x63, 0xeb, 1.0);
const ACCENT_HOVER: D2D1_COLOR_F = rgba(0x3b, 0x76, 0xf6, 1.0);
/// The close button of the action bar.
const CLOSE: D2D1_COLOR_F = rgba(0xef, 0x44, 0x44, 1.0);
const RECORD: D2D1_COLOR_F = rgba(0xe8, 0x11, 0x23, 1.0);
const DIM: D2D1_COLOR_F = rgba(0, 0, 0, 0.5);
const WHITE: D2D1_COLOR_F = rgba(255, 255, 255, 1.0);
const BLACK: D2D1_COLOR_F = rgba(0, 0, 0, 1.0);
/// Toolbar colours of a theme (see `docs/UI_DESIGN.md`).
struct Palette {
    panel: D2D1_COLOR_F,
    border: D2D1_COLOR_F,
    hover: D2D1_COLOR_F,
    /// Background of a toggle that is on.
    toggle_on: D2D1_COLOR_F,
    separator: D2D1_COLOR_F,
    text: D2D1_COLOR_F,
    /// Label of a text toggle that is off.
    dimmed: D2D1_COLOR_F,
    /// Glyph of a toggle that is off.
    muted: D2D1_COLOR_F,
}

const DARK: Palette = Palette {
    panel: rgba(0x0c, 0x0d, 0x11, 0.97),
    border: rgba(255, 255, 255, 0.2),
    hover: rgba(255, 255, 255, 0.1),
    toggle_on: rgba(255, 255, 255, 0.16),
    separator: rgba(255, 255, 255, 0.18),
    text: WHITE,
    dimmed: rgba(255, 255, 255, 0.6),
    muted: rgba(0xff, 0x5a, 0x5f, 1.0),
};

const LIGHT: Palette = Palette {
    panel: rgba(0xf7, 0xf7, 0xf7, 0.98),
    border: rgba(0, 0, 0, 0.25),
    hover: rgba(0, 0, 0, 0.07),
    toggle_on: rgba(0, 0, 0, 0.12),
    separator: rgba(0, 0, 0, 0.18),
    text: rgba(0x10, 0x10, 0x10, 1.0),
    dimmed: rgba(0x10, 0x10, 0x10, 0.55),
    muted: rgba(0xc5, 0x0f, 0x1f, 1.0),
};

const fn palette(light: bool) -> &'static Palette {
    if light { &LIGHT } else { &DARK }
}

const TEXT_FONT: &str = "Segoe UI Variable Text";
const ICON_FONT: &str = "Segoe Fluent Icons";

/// Toolbar glyphs (Segoe Fluent Icons); `on` is the state of a toggle.
pub fn icon(button: Button, on: bool) -> char {
    match button {
        Button::Purpose(Purpose::Screenshot) => '\u{E722}',
        Button::Purpose(Purpose::Recording) => '\u{E714}',
        Button::Mode(Mode::Region) => '\u{F407}',
        Button::Mode(Mode::Freeform) => '\u{F408}',
        Button::Mode(Mode::Screen) => '\u{E7F4}',
        Button::Mode(Mode::Picker) => '\u{EF3C}',
        // Drawn as text instead.
        Button::Gif | Button::Fps => ' ',
        Button::Countdown => '\u{E916}',
        Button::SystemAudio if on => '\u{E767}',
        Button::SystemAudio => '\u{E74F}',
        Button::Microphone if on => '\u{E720}',
        Button::Microphone => '\u{EC54}',
        Button::Close => '\u{E711}',
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Font {
    /// Toolbar glyphs, centred.
    Icon,
    /// Centred labels.
    Label,
    /// Left-aligned readouts.
    Readout,
}

/// Everything that changes from one frame to the next.
#[derive(Debug)]
pub struct Scene<'a> {
    pub selection: &'a Selection,
    /// Window or monitor highlighted under the cursor.
    pub hover: Option<Rect>,
    /// Hidden while dragging so it never covers the selection.
    pub toolbar: Option<&'a Toolbar>,
    pub toolbar_hover: Option<Button>,
    pub magnifier: Option<Magnifier>,
    /// Captured pixel under the cursor, `[b, g, r, a]`.
    pub pixel: Option<[u8; 4]>,
    /// UI scale of the monitor under the cursor.
    pub scale: f32,
    pub bounds: Rect,
    /// Recording is drawn in red instead of blue.
    pub purpose: Purpose,
    /// Recording toggles.
    pub format: RecordingFormat,
    /// Frame rate of the current format and countdown, in seconds.
    pub fps: u32,
    pub countdown: u32,
    pub system_audio: bool,
    pub microphone: bool,
    /// Light theme for the toolbar and labels.
    pub light: bool,
    /// A screenshot selection waiting for an action.
    pub pending: Option<Pending<'a>>,
}

/// A selection kept on screen with the bar of actions under it.
#[derive(Debug, Clone, Copy)]
pub struct Pending<'a> {
    pub rect: Rect,
    /// Freeform selections are drawn along their path (no handles).
    pub path: Option<&'a [Point]>,
    pub bar: &'a ActionBar,
    pub hover: Option<Action>,
}

/// Glyphs of the action bar (Segoe Fluent Icons); Text is drawn as a "T".
pub const fn action_icon(action: Action) -> char {
    match action {
        Action::Copy => '\u{E8C8}',
        Action::Save => '\u{E896}',
        Action::SaveAs => '\u{E838}',
        Action::Edit => '\u{E70F}',
        Action::Pin => '\u{E718}',
        Action::Text => 'T',
        Action::Scan => '\u{ED14}',
        Action::Close => '\u{E711}',
    }
}

/// The frozen desktop, uploaded to the GPU once per capture.
#[derive(Debug)]
pub struct Screenshot {
    bitmap: ID2D1Bitmap1,
    brush: ID2D1BitmapBrush1,
    bounds: Rect,
}

#[derive(Debug)]
pub struct Renderer {
    factory: ID2D1Factory1,
    device: ID2D1Device,
    dc: ID2D1DeviceContext,
    dwrite: IDWriteFactory,
    brush: ID2D1SolidColorBrush,
    formats: RefCell<HashMap<(Font, u32), IDWriteTextFormat>>,
}

#[allow(clippy::cast_precision_loss)]
fn rect_f(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left as f32,
        top: r.top as f32,
        right: r.right as f32,
        bottom: r.bottom as f32,
    }
}

/// Rectangle of a stroke of width `w` drawn just outside `r`.
#[allow(clippy::cast_precision_loss)]
fn outline(r: Rect, w: f32) -> D2D_RECT_F {
    let h = w / 2.0;
    D2D_RECT_F {
        left: r.left as f32 - h,
        top: r.top as f32 - h,
        right: r.right as f32 + h,
        bottom: r.bottom as f32 + h,
    }
}

#[allow(clippy::cast_precision_loss)]
fn centre(p: Point) -> Vector2 {
    Vector2 {
        X: p.x as f32 + 0.5,
        Y: p.y as f32 + 0.5,
    }
}

fn bitmap_props(options: D2D1_BITMAP_OPTIONS, opaque: bool) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: if opaque {
                D2D1_ALPHA_MODE_IGNORE
            } else {
                D2D1_ALPHA_MODE_PREMULTIPLIED
            },
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: options,
        ..Default::default()
    }
}

#[allow(clippy::cast_sign_loss)]
fn size_u(r: Rect) -> D2D_SIZE_U {
    D2D_SIZE_U {
        width: r.width().max(1) as u32,
        height: r.height().max(1) as u32,
    }
}

impl Renderer {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let options = D2D1_FACTORY_OPTIONS::default();
        // SAFETY: plain factory/device creation with valid arguments.
        unsafe {
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&options))?;
            let dxgi: IDXGIDevice = gpu.device().cast()?;
            let device = factory.CreateDevice(&dxgi)?;
            let dc = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let brush = dc.CreateSolidColorBrush(&WHITE, None)?;
            let dwrite = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            Ok(Self {
                factory,
                device,
                dc,
                dwrite,
                brush,
                formats: RefCell::new(HashMap::new()),
            })
        }
    }

    /// Releases cached GPU resources (call when idle).
    pub fn trim(&self) {
        // SAFETY: plain call on a live device.
        unsafe { self.device.ClearResources(0) };
    }

    pub fn context(&self) -> &ID2D1DeviceContext {
        &self.dc
    }

    /// Largest bitmap the GPU accepts, in pixels per side.
    pub fn max_bitmap_size(&self) -> u32 {
        // SAFETY: simple getter.
        unsafe { self.dc.GetMaximumBitmapSize() }
    }

    pub fn load_screenshot(&self, image: &BgraImage) -> Result<Screenshot> {
        // SAFETY: the pointer and pitch describe `image`, which outlives the call.
        unsafe {
            #[allow(clippy::cast_possible_truncation)]
            let bitmap = self.dc.CreateBitmap(
                size_u(image.bounds()),
                Some(image.as_bytes().as_ptr().cast()),
                image.stride() as u32,
                &bitmap_props(D2D1_BITMAP_OPTIONS_NONE, true),
            )?;
            let brush = self.dc.CreateBitmapBrush(&bitmap, None, None)?;
            Ok(Screenshot {
                bitmap,
                brush,
                bounds: image.bounds(),
            })
        }
    }

    /// Creates a bitmap usable as a render target (swap chain back buffers
    /// use [`Self::target_props`] instead).
    pub fn target_props() -> D2D1_BITMAP_PROPERTIES1 {
        bitmap_props(
            D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            true,
        )
    }

    fn color(&self, c: D2D1_COLOR_F) -> ID2D1Brush {
        // SAFETY: plain setter on a live brush.
        unsafe { self.brush.SetColor(&c) };
        self.brush.clone().into()
    }

    fn format(&self, font: Font, size: f32) -> Result<IDWriteTextFormat> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let key = (font, (size * 10.0).round() as u32);
        if let Some(f) = self.formats.borrow().get(&key) {
            return Ok(f.clone());
        }
        let (family, weight): (&str, DWRITE_FONT_WEIGHT) = match font {
            Font::Icon => (ICON_FONT, DWRITE_FONT_WEIGHT_NORMAL),
            Font::Label | Font::Readout => (TEXT_FONT, DWRITE_FONT_WEIGHT_SEMI_BOLD),
        };
        let alignment = if font == Font::Readout {
            DWRITE_TEXT_ALIGNMENT_LEADING
        } else {
            DWRITE_TEXT_ALIGNMENT_CENTER
        };
        // SAFETY: valid strings and enum values.
        let format = unsafe {
            let f = self.dwrite.CreateTextFormat(
                &HSTRING::from(family),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                &HSTRING::from("en-us"),
            )?;
            f.SetTextAlignment(alignment)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            f
        };
        self.formats.borrow_mut().insert(key, format.clone());
        Ok(format)
    }

    /// Width of `text` laid out on one line.
    fn text_width(&self, text: &str, format: &IDWriteTextFormat) -> Result<f32> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut metrics = DWRITE_TEXT_METRICS::default();
        // SAFETY: `wide` and `metrics` outlive the calls.
        unsafe {
            let layout = self
                .dwrite
                .CreateTextLayout(&wide, format, 10_000.0, 10_000.0)?;
            layout.GetMetrics(&mut metrics)?;
        }
        Ok(metrics.widthIncludingTrailingWhitespace)
    }

    fn text(&self, text: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F, color: D2D1_COLOR_F) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let brush = self.color(color);
        // SAFETY: drawing inside BeginDraw/EndDraw with live resources.
        unsafe {
            self.dc.DrawText(
                &wide,
                format,
                &rect,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn path(&self, points: &[Point], closed: bool) -> Result<ID2D1PathGeometry1> {
        // SAFETY: the sink is closed before the geometry is used.
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            let mut iter = points.iter().copied().map(centre);
            if let Some(first) = iter.next() {
                let begin = if closed {
                    D2D1_FIGURE_BEGIN_FILLED
                } else {
                    D2D1_FIGURE_BEGIN_HOLLOW
                };
                sink.BeginFigure(first, begin);
                let rest: Vec<Vector2> = iter.collect();
                sink.AddLines(&rest);
                sink.EndFigure(if closed {
                    D2D1_FIGURE_END_CLOSED
                } else {
                    D2D1_FIGURE_END_OPEN
                });
            }
            sink.Close()?;
            Ok(geometry)
        }
    }

    /// Draws a full frame into the current target.
    pub fn draw(&self, scene: &Scene<'_>, shot: &Screenshot) -> Result<()> {
        let dc = &self.dc;
        // SAFETY: every resource used below belongs to this device context.
        unsafe {
            dc.BeginDraw();
            dc.Clear(Some(&BLACK));
            let full = rect_f(shot.bounds);
            dc.DrawBitmap(
                &shot.bitmap,
                Some(&full),
                1.0,
                D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(&full),
                None,
            );
            dc.FillRectangle(&full, &self.color(DIM));

            let border = (2.0 * scene.scale).max(1.0);
            let accent = if scene.purpose == Purpose::Recording {
                RECORD
            } else {
                ACCENT
            };
            let selection = scene.selection;
            if let Some(pending) = &scene.pending {
                self.draw_pending(pending, scene, shot, border)?;
            } else if let Some(path) = selection.drawn_path() {
                if path.len() >= 3 {
                    dc.FillGeometry(&self.path(path, true)?, &shot.brush, None);
                }
                dc.DrawGeometry(&self.path(path, false)?, &self.color(accent), border, None);
            } else if let Some(rect) = selection.dragged_rect() {
                dc.FillRectangle(&rect_f(rect), &shot.brush);
                dc.DrawRectangle(&outline(rect, border), &self.color(accent), border, None);
                self.draw_size_label(rect, scene)?;
            } else if let Some(rect) = scene.hover {
                dc.FillRectangle(&rect_f(rect), &shot.brush);
                dc.DrawRectangle(&outline(rect, border), &self.color(accent), border, None);
            }

            if let Some(toolbar) = scene.toolbar {
                self.draw_toolbar(toolbar, scene, accent)?;
            }
            if let Some(m) = scene.magnifier {
                self.draw_magnifier(&m, scene, shot)?;
            }

            dc.EndDraw(None, None)
        }
    }

    /// A selection waiting for an action: the picture inside, a thin
    /// outline with handles, its size above it and the bar of actions.
    fn draw_pending(
        &self,
        pending: &Pending<'_>,
        scene: &Scene<'_>,
        shot: &Screenshot,
        border: f32,
    ) -> Result<()> {
        let dc = &self.dc;
        let s = scene.scale;
        let line = (1.5 * s).max(1.0);
        // SAFETY: drawing inside BeginDraw/EndDraw.
        unsafe {
            if let Some(path) = pending.path {
                if path.len() >= 3 {
                    dc.FillGeometry(&self.path(path, true)?, &shot.brush, None);
                }
                dc.DrawGeometry(&self.path(path, true)?, &self.color(ACCENT), border, None);
            } else {
                let area = pending.rect;
                dc.FillRectangle(&rect_f(area), &shot.brush);
                dc.DrawRectangle(&outline(area, line), &self.color(ACCENT), line, None);
                let edges = rect_f(area);
                let (left, top, right, bottom) = (edges.left, edges.top, edges.right, edges.bottom);
                let (middle, centre) = (f32::midpoint(left, right), f32::midpoint(top, bottom));
                let radius = 5.0 * s;
                for (x, y) in [
                    (left, top),
                    (right, top),
                    (right, bottom),
                    (left, bottom),
                    (middle, top),
                    (right, centre),
                    (middle, bottom),
                    (left, centre),
                ] {
                    let dot = D2D1_ELLIPSE {
                        point: Vector2 { X: x, Y: y },
                        radiusX: radius,
                        radiusY: radius,
                    };
                    dc.FillEllipse(&dot, &self.color(WHITE));
                    dc.DrawEllipse(&dot, &self.color(ACCENT), line, None);
                }
            }
        }
        self.draw_size_above(pending.rect, scene)?;
        self.draw_action_bar(pending.bar, pending.hover, scene)
    }

    fn draw_size_above(&self, rect: Rect, scene: &Scene<'_>) -> Result<()> {
        let text = format!("{} × {}", rect.width(), rect.height());
        let format = self.format(Font::Label, 12.0 * scene.scale)?;
        let pad = 8.0 * scene.scale;
        #[allow(clippy::cast_possible_truncation)]
        let size = (
            (self.text_width(&text, &format)? + 2.0 * pad).ceil() as i32,
            (22.0 * scene.scale).round() as i32,
        );
        let label = size_label_above(rect, scene.bounds, size, scene.scale);
        let p = palette(scene.light);
        self.pill(label, p.panel, 6.0 * scene.scale);
        self.text(&text, &format, rect_f(label), p.text);
        Ok(())
    }

    fn draw_action_bar(
        &self,
        bar: &ActionBar,
        hover: Option<Action>,
        scene: &Scene<'_>,
    ) -> Result<()> {
        let s = bar.scale;
        let p = palette(scene.light);
        self.pill(bar.frame, p.panel, 12.0 * s);
        self.rounded_outline(bar.frame, p.border, 12.0 * s);
        // SAFETY: drawing inside BeginDraw/EndDraw.
        #[allow(clippy::cast_precision_loss)]
        unsafe {
            let x = bar.separator as f32 + 0.5;
            let inset = 12.0 * s;
            self.dc.DrawLine(
                Vector2 {
                    X: x,
                    Y: bar.frame.top as f32 + inset,
                },
                Vector2 {
                    X: x,
                    Y: bar.frame.bottom as f32 - inset,
                },
                &self.color(p.separator),
                1.0,
                None,
            );
        }
        let glyphs = self.format(Font::Icon, 17.0 * s)?;
        let label = self.format(Font::Label, 14.0 * s)?;
        for &(action, rect) in &bar.buttons {
            let hovered = hover == Some(action);
            if action == Action::Copy {
                let fill = if hovered { ACCENT_HOVER } else { ACCENT };
                self.pill(rect, fill, 8.0 * s);
                let mut icon = rect_f(rect);
                icon.right = icon.left + 32.0 * s;
                icon.left += 6.0 * s;
                self.text(&action_icon(action).to_string(), &glyphs, icon, WHITE);
                let mut text = rect_f(rect);
                text.left += 30.0 * s;
                self.text("Copy", &label, text, WHITE);
                continue;
            }
            if hovered {
                self.pill(rect, p.hover, 8.0 * s);
            }
            let colour = if action == Action::Close {
                CLOSE
            } else {
                p.text
            };
            if action == Action::Text {
                let big = self.format(Font::Label, 18.0 * s)?;
                self.text("T", &big, rect_f(rect), colour);
            } else {
                self.text(
                    &action_icon(action).to_string(),
                    &glyphs,
                    rect_f(rect),
                    colour,
                );
            }
        }
        Ok(())
    }

    fn draw_size_label(&self, rect: Rect, scene: &Scene<'_>) -> Result<()> {
        let text = format!("{} × {}", rect.width(), rect.height());
        let format = self.format(Font::Label, 12.0 * scene.scale)?;
        let pad = 8.0 * scene.scale;
        #[allow(clippy::cast_possible_truncation)]
        let size = (
            (self.text_width(&text, &format)? + 2.0 * pad).ceil() as i32,
            (22.0 * scene.scale).round() as i32,
        );
        let label = size_label(rect, scene.bounds, size, scene.scale);
        let p = palette(scene.light);
        self.pill(label, p.panel, 8.0 * scene.scale);
        self.rounded_outline(label, p.border, 8.0 * scene.scale);
        self.text(&text, &format, rect_f(label), p.text);
        Ok(())
    }

    fn pill(&self, r: Rect, color: D2D1_COLOR_F, radius: f32) {
        let rounded = D2D1_ROUNDED_RECT {
            rect: rect_f(r),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: drawing inside BeginDraw/EndDraw.
        unsafe { self.dc.FillRoundedRectangle(&rounded, &self.color(color)) };
    }

    /// Hairline outline of a rounded rectangle, inside `r`.
    fn rounded_outline(&self, r: Rect, color: D2D1_COLOR_F, radius: f32) {
        let rounded = D2D1_ROUNDED_RECT {
            rect: outline(r, 1.0),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: drawing inside BeginDraw/EndDraw.
        unsafe {
            self.dc
                .DrawRoundedRectangle(&rounded, &self.color(color), 1.0, None);
        }
    }

    /// Outline of a square, inside `r`.
    fn frame(&self, r: Rect, color: D2D1_COLOR_F, width: f32) {
        // SAFETY: drawing inside BeginDraw/EndDraw.
        unsafe {
            self.dc
                .DrawRectangle(&outline(r, width), &self.color(color), width, None);
        }
    }

    fn draw_toolbar(&self, bar: &Toolbar, scene: &Scene<'_>, accent: D2D1_COLOR_F) -> Result<()> {
        let s = bar.scale;
        let p = palette(scene.light);
        // Soft corners, like the rest of Capshot (docs/UI_DESIGN.md).
        self.pill(bar.frame, p.panel, 12.0 * s);
        self.rounded_outline(bar.frame, p.border, 12.0 * s);
        #[allow(clippy::cast_precision_loss)]
        for &x in &bar.separators {
            let x = x as f32 + 0.5;
            let inset = 10.0 * s;
            // SAFETY: drawing inside BeginDraw/EndDraw.
            unsafe {
                self.dc.DrawLine(
                    Vector2 {
                        X: x,
                        Y: bar.frame.top as f32 + inset,
                    },
                    Vector2 {
                        X: x,
                        Y: bar.frame.bottom as f32 - inset,
                    },
                    &self.color(p.separator),
                    1.0,
                    None,
                );
            }
        }
        let format = self.format(Font::Icon, 16.0 * s)?;
        for &(button, rect) in &bar.buttons {
            let toggle = match button {
                Button::Gif => Some(scene.format == RecordingFormat::Gif),
                Button::SystemAudio => Some(scene.system_audio),
                Button::Microphone => Some(scene.microphone),
                _ => None,
            };
            let active = match button {
                Button::Mode(mode) => mode == scene.selection.mode(),
                Button::Purpose(purpose) => purpose == scene.purpose,
                _ => false,
            };
            let hovered = scene.toolbar_hover == Some(button);
            if active {
                self.pill(rect, accent, 8.0 * s);
            } else if hovered {
                self.pill(rect, p.hover, 8.0 * s);
            } else if toggle == Some(true) {
                self.pill(rect, p.toggle_on, 8.0 * s);
            }
            if button == Button::Gif {
                let label = self.format(Font::Label, 11.0 * s)?;
                let colour = if toggle == Some(true) {
                    p.text
                } else {
                    p.dimmed
                };
                self.text("GIF", &label, rect_f(rect), colour);
                continue;
            }
            if button == Button::Fps {
                let label = self.format(Font::Label, 12.0 * s)?;
                self.text(&format!("{} fps", scene.fps), &label, rect_f(rect), p.text);
                continue;
            }
            if button == Button::Countdown {
                // Stopwatch glyph on the left, seconds on the right.
                let mut glyph = rect_f(rect);
                glyph.right = glyph.left + 22.0 * s;
                glyph.left += 4.0 * s;
                let colour = if scene.countdown == 0 {
                    p.dimmed
                } else {
                    p.text
                };
                self.text(
                    "\u{E916}",
                    &self.format(Font::Icon, 14.0 * s)?,
                    glyph,
                    colour,
                );
                let mut value = rect_f(rect);
                value.left += 20.0 * s;
                let label = self.format(Font::Label, 12.0 * s)?;
                self.text(&format!("{}s", scene.countdown), &label, value, colour);
                continue;
            }
            let colour = match (active, toggle) {
                (true, _) => WHITE,
                (_, Some(false)) => p.muted,
                _ => p.text,
            };
            let glyph = icon(button, toggle.unwrap_or(true)).to_string();
            self.text(&glyph, &format, rect_f(rect), colour);
        }
        Ok(())
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::many_single_char_names
    )]
    fn draw_magnifier(&self, m: &Magnifier, scene: &Scene<'_>, shot: &Screenshot) -> Result<()> {
        let dc = &self.dc;
        // Only the part of the source inside the capture can be sampled.
        let src = m.source.intersect(shot.bounds);
        // SAFETY: drawing inside BeginDraw/EndDraw.
        unsafe {
            dc.FillRectangle(&rect_f(m.frame), &self.color(BLACK));
            if !src.is_empty() {
                let dest = Rect::new(
                    m.frame.left + (src.left - m.source.left) * m.cell,
                    m.frame.top + (src.top - m.source.top) * m.cell,
                    m.frame.left + (src.right - m.source.left) * m.cell,
                    m.frame.top + (src.bottom - m.source.top) * m.cell,
                );
                dc.DrawBitmap(
                    &shot.bitmap,
                    Some(&rect_f(dest)),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    Some(&rect_f(src)),
                    None,
                );
            }
            let half = MAGNIFIER_CELLS / 2;
            let centre_cell = Rect::from_size(
                m.frame.left + half * m.cell,
                m.frame.top + half * m.cell,
                m.cell,
                m.cell,
            );
            dc.DrawRectangle(&outline(centre_cell, 2.0), &self.color(BLACK), 1.0, None);
            dc.DrawRectangle(&outline(centre_cell, 0.0), &self.color(WHITE), 1.0, None);
            dc.DrawRectangle(&outline(m.frame, 2.0), &self.color(WHITE), 2.0, None);
        }

        let p = palette(scene.light);
        self.pill(m.info, p.panel, 2.0 * scene.scale);
        self.frame(m.info, p.border, 1.0);
        let cursor = scene.selection.cursor();
        let pad = 8.0 * scene.scale;
        let mut text_left = m.info.left as f32 + pad;
        let text = match scene.pixel {
            Some([b, g, r, a]) if a > 0 => {
                // Colour swatch on the left of the readout.
                let side = m.info.height() - (8.0 * scene.scale) as i32;
                let swatch = Rect::from_size(
                    m.info.left + (6.0 * scene.scale) as i32,
                    m.info.top + (m.info.height() - side) / 2,
                    side,
                    side,
                );
                // SAFETY: drawing inside BeginDraw/EndDraw.
                unsafe {
                    self.dc
                        .FillRectangle(&rect_f(swatch), &self.color(rgba(r, g, b, 1.0)));
                    self.dc
                        .DrawRectangle(&rect_f(swatch), &self.color(p.border), 1.0, None);
                }
                text_left = swatch.right as f32 + pad * 0.75;
                format!("#{r:02X}{g:02X}{b:02X}")
            }
            _ => "—".to_owned(),
        };
        let format = self.format(Font::Readout, 11.0 * scene.scale)?;
        let mut info = rect_f(m.info);
        info.left = text_left;
        self.text(
            &format!("{text}\n{}, {}", cursor.x, cursor.y),
            &format,
            info,
            p.text,
        );
        Ok(())
    }

    /// Draws every kind of element once off-screen so that shaders, fonts
    /// and glyph caches are ready before the first real capture.
    pub fn warm_up(&self) -> Result<()> {
        struct Canvas(Rect);
        impl Targets for Canvas {
            fn window_at(&self, _: Point) -> Option<Rect> {
                None
            }
            fn monitor_at(&self, _: Point) -> Rect {
                self.0
            }
            fn bounds(&self) -> Rect {
                self.0
            }
        }

        let image = BgraImage::new(256, 256);
        let canvas = Canvas(image.bounds());
        let shot = self.load_screenshot(&image)?;
        let toolbar = Toolbar::new(canvas.0, 1.0, Purpose::Recording, RecordingFormat::Mp4);

        let mut drag = Selection::new(Mode::Region, Point::new(20, 20));
        drag.press(Point::new(20, 20), &canvas);
        drag.moved(Point::new(120, 120), &canvas);
        let mut free = Selection::new(Mode::Freeform, Point::new(20, 20));
        free.press(Point::new(20, 20), &canvas);
        free.moved(Point::new(120, 30), &canvas);
        free.moved(Point::new(60, 120), &canvas);
        let idle = Selection::new(Mode::Region, Point::new(100, 100));

        for selection in [&drag, &free, &idle] {
            let scene = Scene {
                selection,
                hover: selection.hover_rect(&canvas),
                toolbar: Some(&toolbar),
                toolbar_hover: Some(Button::Close),
                magnifier: Some(Magnifier::new(selection.cursor(), canvas.0, 1.0)),
                pixel: Some([10, 20, 30, 255]),
                scale: 1.0,
                bounds: canvas.0,
                purpose: if selection.mode() == Mode::Freeform {
                    Purpose::Screenshot
                } else {
                    Purpose::Recording
                },
                format: RecordingFormat::Mp4,
                fps: 60,
                countdown: 3,
                system_audio: true,
                microphone: selection.mode() == Mode::Freeform,
                light: selection.mode() == Mode::Freeform,
                pending: None,
            };
            self.render_to_image(&scene, &shot)?;
        }
        self.trim();
        Ok(())
    }

    /// Renders a scene into a CPU image (examples and visual checks).
    pub fn render_to_image(&self, scene: &Scene<'_>, shot: &Screenshot) -> Result<BgraImage> {
        let size = size_u(shot.bounds);
        // SAFETY: bitmaps are created, drawn and mapped on this context; the
        // mapping is released before returning.
        unsafe {
            let target = self.dc.CreateBitmap(size, None, 0, &Self::target_props())?;
            self.dc.SetTarget(&target);
            let drawn = self.draw(scene, shot);
            self.dc.SetTarget(None);
            drawn?;

            let cpu = self.dc.CreateBitmap(
                size,
                None,
                0,
                &bitmap_props(
                    D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    true,
                ),
            )?;
            cpu.CopyFromBitmap(None, &target, None)?;
            let mapped = cpu.Map(D2D1_MAP_OPTIONS_READ)?;
            let pitch = mapped.pitch as usize;
            let data = std::slice::from_raw_parts(mapped.bits, pitch * size.height as usize);
            let mut image = BgraImage::new(size.width, size.height);
            image.blit(data, pitch, size.width, size.height, Point::new(0, 0));
            cpu.Unmap()?;
            Ok(image)
        }
    }
}
