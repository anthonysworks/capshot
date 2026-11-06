//! Drawing a [`Composition`] over video frames on the GPU with Direct2D:
//! overlays, the recorded cursor (hidden, enlarged or replaced) and click
//! marks. The CPU version in [`crate::compose`] serves GIF exports.
//!
//! Everything is drawn in source pixels; a transform maps them onto the
//! output frame (cropped, and scaled onto a canvas), clipped to the video.

use kadra_capture::Gpu;
use kadra_core::{BgraImage, Rect};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_FACTORY_OPTIONS,
    D2D1_FACTORY_TYPE_MULTI_THREADED, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext,
    ID2D1Factory1, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_TEXTURE2D_DESC, ID3D11Texture2D, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows::core::{Interface, Result};
use windows_numerics::{Matrix3x2, Vector2};

use crate::compose::{CLICK_MARK, ClickStyle, Composition, CursorStyle, decode_png};
use crate::convert::{Blitter, Space, drawable_texture};
use crate::cursor::CursorTrack;

fn props(options: D2D1_BITMAP_OPTIONS, opaque: bool) -> D2D1_BITMAP_PROPERTIES1 {
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
        colorContext: std::mem::ManuallyDrop::new(None),
    }
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

fn color([r, g, b, a]: [u8; 4]) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: f32::from(a) / 255.0,
    }
}

/// An image with its size, for drawing.
struct Sprite {
    bitmap: ID2D1Bitmap1,
    width: f32,
    height: f32,
}

struct Overlay {
    sprite: Sprite,
    x: f32,
    y: f32,
    start: f64,
    end: f64,
}

/// A pixelated area, see [`Painter::new`].
struct Pixelated {
    blit: Blitter,
    input: ID3D11VideoProcessorInputView,
    output: ID3D11VideoProcessorOutputView,
    bitmap: ID2D1Bitmap1,
    /// The area in the frame, and the small texture.
    from: Rect,
    small: Rect,
    /// The area in source pixels.
    to: D2D_RECT_F,
    start: f64,
    end: f64,
    _texture: ID3D11Texture2D,
}

pub struct Painter {
    pixels: Vec<Pixelated>,
    dc: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    overlays: Vec<Overlay>,
    /// Recorded cursor shapes and their hot spots.
    shapes: Vec<Option<(Sprite, [f32; 2])>>,
    replacement: Option<(Sprite, [f32; 2])>,
    cursor: Option<CursorStyle>,
    clicks: Option<ClickStyle>,
    track: Option<CursorTrack>,
    /// Source pixels → output pixels.
    transform: Matrix3x2,
    /// The video, in source pixels.
    clip: D2D_RECT_F,
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
impl Painter {
    /// Draws into `target` (a BGRA render texture), where the `crop` area of
    /// the source is shown at `area`.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        gpu: &Gpu,
        target: &ID3D11Texture2D,
        composition: &Composition,
        track: Option<CursorTrack>,
        crop: Rect,
        area: Rect,
    ) -> Result<Self> {
        // SAFETY: COM calls on live interfaces with valid descriptions.
        unsafe {
            let factory: ID2D1Factory1 = D2D1CreateFactory(
                D2D1_FACTORY_TYPE_MULTI_THREADED,
                Some(&D2D1_FACTORY_OPTIONS::default()),
            )?;
            let device = factory.CreateDevice(&gpu.device().cast::<IDXGIDevice>()?)?;
            let dc = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let surface: IDXGISurface = target.cast()?;
            let bitmap = dc.CreateBitmapFromDxgiSurface(
                &surface,
                Some(&props(
                    D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    true,
                )),
            )?;
            dc.SetTarget(&bitmap);
            let brush = dc.CreateSolidColorBrush(&color([255, 255, 255, 255]), None)?;

            let sprite = |image: &BgraImage| -> Result<Sprite> {
                let premultiplied = premultiply(image);
                let bitmap = dc.CreateBitmap(
                    D2D_SIZE_U {
                        width: image.width(),
                        height: image.height(),
                    },
                    Some(premultiplied.as_ptr().cast()),
                    image.width() * 4,
                    &props(D2D1_BITMAP_OPTIONS_NONE, false),
                )?;
                Ok(Sprite {
                    bitmap,
                    width: image.width() as f32,
                    height: image.height() as f32,
                })
            };
            let hot = |h: [i32; 2]| [h[0] as f32, h[1] as f32];
            let mut overlays = Vec::new();
            for o in &composition.overlays {
                if let Some(image) = decode_png(&o.png) {
                    overlays.push(Overlay {
                        sprite: sprite(&image)?,
                        x: o.x as f32,
                        y: o.y as f32,
                        start: o.start,
                        end: o.end,
                    });
                }
            }
            let shapes = match &track {
                Some(t) if composition.cursor.is_some() => t
                    .shapes
                    .iter()
                    .map(|s| {
                        decode_png(&s.png)
                            .and_then(|image| sprite(&image).ok())
                            .map(|sp| (sp, hot(s.hotspot)))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let replacement = match composition.cursor.as_ref().and_then(|c| c.image.as_ref()) {
                Some(image) => decode_png(&image.png)
                    .map(|i| sprite(&i))
                    .transpose()?
                    .map(|sp| (sp, hot(image.hotspot))),
                None => None,
            };

            let sx = area.width() as f32 / crop.width().max(1) as f32;
            let sy = area.height() as f32 / crop.height().max(1) as f32;
            let transform = Matrix3x2 {
                M11: sx,
                M12: 0.0,
                M21: 0.0,
                M22: sy,
                M31: area.left as f32 - crop.left as f32 * sx,
                M32: area.top as f32 - crop.top as f32 * sy,
            };

            // Each pixelated area: the video processor shrinks it into a
            // small texture, drawn back enlarged without smoothing.
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            target.GetDesc(&mut desc);
            let frame = (desc.Width, desc.Height);
            let frame_rect = Rect::new(0, 0, frame.0 as i32, frame.1 as i32);
            let mut pixels = Vec::new();
            for p in &composition.pixelate {
                let r = p.rect();
                let to_frame = Rect::new(
                    (area.left as f32 + (r.left - crop.left) as f32 * sx).floor() as i32,
                    (area.top as f32 + (r.top - crop.top) as f32 * sy).floor() as i32,
                    (area.left as f32 + (r.right - crop.left) as f32 * sx).ceil() as i32,
                    (area.top as f32 + (r.bottom - crop.top) as f32 * sy).ceil() as i32,
                )
                .intersect(frame_rect);
                if to_frame.width() < 2 || to_frame.height() < 2 {
                    continue;
                }
                let block = (p.block.max(2) as f32 * sx).max(2.0);
                let small = (
                    ((to_frame.width() as f32 / block).ceil() as u32).max(1),
                    ((to_frame.height() as f32 / block).ceil() as u32).max(1),
                );
                let texture = drawable_texture(gpu, small)?;
                let blit = Blitter::new(gpu, frame, small, 30, (Space::Rgb, Space::Rgb))?;
                let input = blit.input(target, 0)?;
                let output = blit.output(&texture)?;
                let bitmap = dc.CreateBitmapFromDxgiSurface(
                    &texture.cast::<IDXGISurface>()?,
                    Some(&props(D2D1_BITMAP_OPTIONS_NONE, true)),
                )?;
                // Drawn in source pixels, like everything else.
                let source = D2D_RECT_F {
                    left: crop.left as f32 + (to_frame.left as f32 - area.left as f32) / sx,
                    top: crop.top as f32 + (to_frame.top as f32 - area.top as f32) / sy,
                    right: crop.left as f32 + (to_frame.right as f32 - area.left as f32) / sx,
                    bottom: crop.top as f32 + (to_frame.bottom as f32 - area.top as f32) / sy,
                };
                pixels.push(Pixelated {
                    blit,
                    input,
                    output,
                    bitmap,
                    from: to_frame,
                    small: Rect::new(0, 0, small.0 as i32, small.1 as i32),
                    to: source,
                    start: p.start,
                    end: p.end,
                    _texture: texture,
                });
            }
            Ok(Self {
                pixels,
                dc,
                brush,
                overlays,
                shapes,
                replacement,
                cursor: composition.cursor.clone(),
                clicks: composition.clicks.clone(),
                track,
                transform,
                clip: rect_f(crop),
            })
        }
    }

    fn draw(&self, sprite: &Sprite, x: f32, y: f32, scale: f32) {
        let dest = D2D_RECT_F {
            left: x,
            top: y,
            right: x + sprite.width * scale,
            bottom: y + sprite.height * scale,
        };
        // SAFETY: inside BeginDraw/EndDraw with live resources.
        unsafe {
            self.dc.DrawBitmap(
                &sprite.bitmap,
                Some(&dest),
                1.0,
                D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                None,
                None,
            );
        }
    }

    fn circle(&self, x: f32, y: f32, radius: f32, stroke: f32, rgba: [u8; 4]) {
        let ellipse = D2D1_ELLIPSE {
            point: Vector2 { X: x, Y: y },
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: inside BeginDraw/EndDraw with live resources.
        unsafe {
            self.brush.SetColor(&color(rgba));
            if stroke > 0.0 {
                self.dc.DrawEllipse(&ellipse, &self.brush, stroke, None);
            } else {
                self.dc.FillEllipse(&ellipse, &self.brush);
            }
        }
    }

    /// Draws everything visible at source time `t` over the target.
    #[allow(clippy::cast_sign_loss, clippy::many_single_char_names)]
    pub fn paint(&self, t: f64) -> Result<()> {
        let pixels: Vec<&Pixelated> = self
            .pixels
            .iter()
            .filter(|p| t >= p.start && t < p.end)
            .collect();
        for p in &pixels {
            p.blit.blit(&p.input, p.from, &p.output, p.small)?;
        }
        // SAFETY: Direct2D drawing on the target set in `new`.
        unsafe {
            self.dc.BeginDraw();
            self.dc.SetTransform(&self.transform);
            self.dc
                .PushAxisAlignedClip(&self.clip, D2D1_ANTIALIAS_MODE_ALIASED);
        }
        for p in &pixels {
            // SAFETY: inside BeginDraw/EndDraw with live resources.
            unsafe {
                self.dc.DrawBitmap(
                    &p.bitmap,
                    Some(&p.to),
                    1.0,
                    D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                    None,
                );
            }
        }
        for o in &self.overlays {
            if t >= o.start && t < o.end {
                self.draw(&o.sprite, o.x, o.y, 1.0);
            }
        }
        if let (Some(clicks), Some(track)) = (&self.clicks, &self.track)
            && clicks.ring
        {
            let scale = self.cursor.as_ref().map_or(1.0, |c| c.scale.max(1.0));
            for c in track
                .clicks
                .iter()
                .filter(|c| c.t <= t && t - c.t < CLICK_MARK)
            {
                let p = ((t - c.t) / CLICK_MARK) as f32;
                let eased = 1.0 - (1.0 - p).powi(3);
                let [r, g, b] = clicks.color;
                let alpha = ((1.0 - p) * 230.0) as u8;
                self.circle(
                    c.x,
                    c.y,
                    (6.0 + 26.0 * eased) * scale,
                    3.0 * scale,
                    [r, g, b, alpha],
                );
                self.circle(c.x, c.y, 5.0 * scale * (1.0 - p), 0.0, [r, g, b, alpha / 2]);
            }
        }
        if let (Some(style), Some(track)) = (&self.cursor, &self.track)
            && style.visible
            && let Some((x, y, shape)) = track.at(t)
        {
            let scale = style.scale.clamp(0.25, 6.0);
            if let Some(halo) = style.halo {
                self.circle(x, y, 22.0 * scale.max(1.0), 0.0, halo);
            }
            let sprite = self
                .replacement
                .as_ref()
                .or_else(|| self.shapes.get(shape as usize).and_then(Option::as_ref));
            if let Some((sprite, hot)) = sprite {
                self.draw(sprite, x - hot[0] * scale, y - hot[1] * scale, scale);
            }
        }
        // SAFETY: closes the drawing opened above.
        unsafe {
            self.dc.PopAxisAlignedClip();
            self.dc.EndDraw(None, None)
        }
    }
}

/// Straight-alpha BGRA → premultiplied, as Direct2D bitmaps expect.
#[allow(clippy::cast_possible_truncation)]
fn premultiply(image: &BgraImage) -> Vec<u8> {
    let mut out = image.as_bytes().to_vec();
    for px in out.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
    out
}
