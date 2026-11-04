//! Frame compositing for edited videos: overlays drawn in the editor, the
//! recorded cursor (hidden, enlarged or replaced), click marks, cropping
//! and an output canvas with another aspect ratio.
//!
//! Frames are composited on the CPU in BGRA, then converted to NV12 for the
//! hardware encoder.

use kadra_core::{BgraImage, Point, Rect};
use serde::{Deserialize, Serialize};

use crate::cursor::CursorTrack;

/// An image drawn over the video between two times (seconds of the
/// source), at a position in pixels of the source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayImage {
    /// PNG, base64.
    pub png: String,
    pub x: i32,
    pub y: i32,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorImage {
    /// PNG, base64.
    pub png: String,
    pub hotspot: [i32; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorStyle {
    pub visible: bool,
    /// Size factor: 1 = as recorded.
    pub scale: f32,
    /// Replaces the recorded cursor.
    #[serde(default)]
    pub image: Option<CursorImage>,
    /// Translucent disc under the cursor, RGBA.
    #[serde(default)]
    pub halo: Option<[u8; 4]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClickStyle {
    /// Expanding ring where the user clicked.
    pub ring: bool,
    pub color: [u8; 3],
    /// Click sound mixed into the audio.
    pub sound: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanvasStyle {
    /// Width : height, e.g. `[9, 16]`.
    pub ratio: [u32; 2],
    /// One colour, or two for a diagonal gradient.
    pub colors: Vec<[u8; 3]>,
    /// Margin around the video, as a fraction of the canvas (0–0.3).
    pub padding: f32,
}

/// Everything drawn on the frames. Empty means "leave frames untouched".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    #[serde(default)]
    pub overlays: Vec<OverlayImage>,
    /// `None`: frames come from the original video, cursor included.
    #[serde(default)]
    pub cursor: Option<CursorStyle>,
    #[serde(default)]
    pub clicks: Option<ClickStyle>,
    #[serde(default)]
    pub canvas: Option<CanvasStyle>,
}

impl Composition {
    /// Whether frames have to be composited at all.
    pub fn is_empty(&self) -> bool {
        self.overlays.is_empty()
            && self.cursor.is_none()
            && self.clicks.as_ref().is_none_or(|c| !c.ring)
            && self.canvas.is_none()
    }

    /// Whether the cursor track (and the video without cursor) is needed.
    pub fn uses_track(&self) -> bool {
        self.cursor.is_some() || self.clicks.is_some()
    }

    pub fn wants_click_sound(&self) -> bool {
        self.clicks.as_ref().is_some_and(|c| c.sound)
    }
}

fn decode_png(base64: &str) -> Option<BgraImage> {
    kadra_encode::decode(&kadra_core::base64::decode(base64)?).ok()
}

/// Bilinear resize (up or down).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn resize(image: &BgraImage, width: u32, height: u32) -> BgraImage {
    let (w, h) = (image.width(), image.height());
    if (w, h) == (width, height) || w == 0 || h == 0 {
        return image.clone();
    }
    if width < w / 2 && height < h / 2 {
        // Strong reductions: area average looks better.
        return image.thumbnail(width, height);
    }
    let src = image.as_bytes();
    let stride = w as usize * 4;
    let mut out = vec![0u8; width as usize * height as usize * 4];
    let sx = w as f32 / width as f32;
    let sy = h as f32 / height as f32;
    for y in 0..height as usize {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (h - 1) as f32);
        let y0 = fy as usize;
        let y1 = (y0 + 1).min(h as usize - 1);
        let ky = fy - y0 as f32;
        for x in 0..width as usize {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (w - 1) as f32);
            let x0 = fx as usize;
            let x1 = (x0 + 1).min(w as usize - 1);
            let kx = fx - x0 as f32;
            for c in 0..4 {
                let p = |xx: usize, yy: usize| f32::from(src[yy * stride + xx * 4 + c]);
                let top = p(x0, y0) * (1.0 - kx) + p(x1, y0) * kx;
                let bottom = p(x0, y1) * (1.0 - kx) + p(x1, y1) * kx;
                out[(y * width as usize + x) * 4 + c] =
                    (top * (1.0 - ky) + bottom * ky).round() as u8;
            }
        }
    }
    BgraImage::from_raw(width, height, out)
}

/// Draws `src` (straight alpha) onto `dst` with its top-left at `at`.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
pub fn blend(dst: &mut BgraImage, src: &BgraImage, at: Point) {
    let (dw, dh) = (dst.width() as i32, dst.height() as i32);
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let x0 = at.x.max(0);
    let y0 = at.y.max(0);
    let x1 = (at.x + sw).min(dw);
    let y1 = (at.y + sh).min(dh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let dstride = dw as usize * 4;
    let sstride = sw as usize * 4;
    let s = src.as_bytes();
    let d = dst.as_bytes_mut();
    for y in y0..y1 {
        for x in x0..x1 {
            let si = (y - at.y) as usize * sstride + (x - at.x) as usize * 4;
            let di = y as usize * dstride + x as usize * 4;
            let a = u32::from(s[si + 3]);
            if a == 0 {
                continue;
            }
            for c in 0..3 {
                let v = (u32::from(s[si + c]) * a + u32::from(d[di + c]) * (255 - a) + 127) / 255;
                d[di + c] = u8::try_from(v).unwrap_or(255);
            }
            d[di + 3] = 255;
        }
    }
}

/// Draws an anti-aliased disc (`thickness` 0) or ring of `rgba`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names,
    clippy::similar_names
)]
fn circle(dst: &mut BgraImage, cx: f32, cy: f32, radius: f32, thickness: f32, rgba: [u8; 4]) {
    let (w, h) = (dst.width() as i32, dst.height() as i32);
    let reach = radius + thickness + 1.0;
    let x0 = ((cx - reach).floor() as i32).max(0);
    let y0 = ((cy - reach).floor() as i32).max(0);
    let x1 = ((cx + reach).ceil() as i32).min(w);
    let y1 = ((cy + reach).ceil() as i32).min(h);
    let stride = w as usize * 4;
    let d = dst.as_bytes_mut();
    for y in y0..y1 {
        for x in x0..x1 {
            let dist = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let coverage = if thickness <= 0.0 {
                (radius - dist + 0.5).clamp(0.0, 1.0)
            } else {
                let half = thickness / 2.0;
                (half - (dist - radius).abs() + 0.5).clamp(0.0, 1.0)
            };
            if coverage <= 0.0 {
                continue;
            }
            let a = coverage * f32::from(rgba[3]) / 255.0;
            let i = y as usize * stride + x as usize * 4;
            // BGRA order.
            for (c, value) in [rgba[2], rgba[1], rgba[0]].into_iter().enumerate() {
                d[i + c] = (f32::from(d[i + c]) * (1.0 - a) + f32::from(value) * a).round() as u8;
            }
        }
    }
}

/// How long a click mark lasts, in seconds.
const CLICK_MARK: f64 = 0.45;

#[derive(Debug)]
struct Overlay {
    image: BgraImage,
    at: Point,
    start: f64,
    end: f64,
}

/// A composition ready to apply to frames of a given source.
#[derive(Debug)]
pub struct Composer {
    overlays: Vec<Overlay>,
    cursor: Option<CursorStyle>,
    /// Cursor images (recorded shapes, or the replacement), already scaled,
    /// with their hot spots.
    shapes: Vec<(BgraImage, [i32; 2])>,
    replacement: Option<(BgraImage, [i32; 2])>,
    track: Option<CursorTrack>,
    clicks: Option<ClickStyle>,
    crop: Rect,
    /// Canvas background and where the video goes on it.
    canvas: Option<(BgraImage, Rect)>,
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap
)]
impl Composer {
    /// `frame` is the source size, `crop` the (even) area kept.
    pub fn new(
        composition: &Composition,
        track: Option<CursorTrack>,
        frame: (u32, u32),
        crop: Option<Rect>,
    ) -> Self {
        let crop = crop.unwrap_or(Rect::new(0, 0, frame.0 as i32, frame.1 as i32));
        let scale = composition
            .cursor
            .as_ref()
            .map_or(1.0, |c| c.scale.clamp(0.25, 6.0));
        let scaled = |image: BgraImage, hotspot: [i32; 2]| {
            let w = ((image.width() as f32 * scale).round() as u32).max(1);
            let h = ((image.height() as f32 * scale).round() as u32).max(1);
            let hot = [
                (hotspot[0] as f32 * scale).round() as i32,
                (hotspot[1] as f32 * scale).round() as i32,
            ];
            (resize(&image, w, h), hot)
        };
        let shapes = track.as_ref().map_or_else(Vec::new, |t| {
            t.shapes
                .iter()
                .map(|s| {
                    let image = decode_png(&s.png).unwrap_or_else(|| BgraImage::zeroed(1, 1));
                    scaled(image, s.hotspot)
                })
                .collect()
        });
        let replacement = composition
            .cursor
            .as_ref()
            .and_then(|c| c.image.as_ref())
            .and_then(|i| Some(scaled(decode_png(&i.png)?, i.hotspot)));
        let overlays = composition
            .overlays
            .iter()
            .filter_map(|o| {
                Some(Overlay {
                    image: decode_png(&o.png)?,
                    at: Point::new(o.x, o.y),
                    start: o.start,
                    end: o.end,
                })
            })
            .collect();
        let canvas = composition
            .canvas
            .as_ref()
            .map(|c| Self::canvas(c, crop.width() as u32, crop.height() as u32));
        Self {
            overlays,
            cursor: composition.cursor.clone(),
            shapes,
            replacement,
            track,
            clicks: composition.clicks.clone(),
            crop,
            canvas,
        }
    }

    /// Canvas of the requested aspect ratio around a `w` × `h` video, and
    /// the area the video occupies (scaled to fit inside the margin).
    fn canvas(style: &CanvasStyle, w: u32, h: u32) -> (BgraImage, Rect) {
        let [rw, rh] = [style.ratio[0].max(1), style.ratio[1].max(1)];
        let ratio = rw as f32 / rh as f32;
        let pad = style.padding.clamp(0.0, 0.3);
        // The canvas keeps the video at its resolution on its larger side.
        let (mut cw, mut ch) = if w as f32 / h as f32 > ratio {
            (w as f32, w as f32 / ratio)
        } else {
            (h as f32 * ratio, h as f32)
        };
        cw /= 1.0 - 2.0 * pad;
        ch /= 1.0 - 2.0 * pad;
        // No larger than the source on its long side (a 1080p recording in
        // 9:16 gives 1080 × 1920), and never beyond 4K.
        let limit = (w.max(h) as f32).min(4096.0) / cw.max(ch);
        if limit < 1.0 {
            cw *= limit;
            ch *= limit;
        }
        let (cw, ch) = ((cw as u32 + 1) & !1, (ch as u32 + 1) & !1);
        let inner_w = cw as f32 * (1.0 - 2.0 * pad);
        let inner_h = ch as f32 * (1.0 - 2.0 * pad);
        let fit = (inner_w / w as f32).min(inner_h / h as f32);
        let (vw, vh) = ((w as f32 * fit) as i32, (h as f32 * fit) as i32);
        let area = Rect::from_size((cw as i32 - vw) / 2, (ch as i32 - vh) / 2, vw, vh);

        let first = style.colors.first().copied().unwrap_or([24, 24, 27]);
        let last = style.colors.last().copied().unwrap_or(first);
        let mut pixels = Vec::with_capacity(cw as usize * ch as usize * 4);
        let span = (cw + ch).max(1) as f32;
        for y in 0..ch {
            for x in 0..cw {
                let k = (x + y) as f32 / span;
                let mix =
                    |a: u8, b: u8| (f32::from(a) * (1.0 - k) + f32::from(b) * k).round() as u8;
                pixels.extend([
                    mix(first[2], last[2]),
                    mix(first[1], last[1]),
                    mix(first[0], last[0]),
                    255,
                ]);
            }
        }
        (BgraImage::from_raw(cw, ch, pixels), area)
    }

    /// Size of the frames produced.
    pub fn output_size(&self) -> (u32, u32) {
        match &self.canvas {
            Some((background, _)) => (background.width(), background.height()),
            None => (self.crop.width() as u32, self.crop.height() as u32),
        }
    }

    /// Click times (source seconds), for the click sound.
    pub fn click_times(&self) -> Vec<f64> {
        self.track
            .as_ref()
            .map_or_else(Vec::new, |t| t.clicks.iter().map(|c| c.t).collect())
    }

    /// Composites one source frame shown at source time `t`.
    #[allow(clippy::many_single_char_names)]
    pub fn render(&self, mut frame: BgraImage, t: f64) -> BgraImage {
        for o in &self.overlays {
            if t >= o.start && t < o.end {
                blend(&mut frame, &o.image, o.at);
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
                circle(
                    &mut frame,
                    c.x,
                    c.y,
                    (6.0 + 26.0 * eased) * scale,
                    3.0 * scale,
                    [r, g, b, alpha],
                );
                circle(
                    &mut frame,
                    c.x,
                    c.y,
                    5.0 * scale * (1.0 - p),
                    0.0,
                    [r, g, b, alpha / 2],
                );
            }
        }
        if let (Some(style), Some(track)) = (&self.cursor, &self.track)
            && style.visible
            && let Some((x, y, shape)) = track.at(t)
        {
            if let Some([r, g, b, a]) = style.halo {
                circle(
                    &mut frame,
                    x,
                    y,
                    22.0 * style.scale.max(1.0),
                    0.0,
                    [r, g, b, a],
                );
            }
            let image = self
                .replacement
                .as_ref()
                .or_else(|| self.shapes.get(shape as usize));
            if let Some((image, hot)) = image {
                let at = Point::new(x.round() as i32 - hot[0], y.round() as i32 - hot[1]);
                blend(&mut frame, image, at);
            }
        }
        let full = Rect::new(0, 0, frame.width() as i32, frame.height() as i32);
        let frame = if self.crop == full {
            frame
        } else {
            frame.crop(self.crop)
        };
        match &self.canvas {
            None => frame,
            Some((background, area)) => {
                let mut out = background.clone();
                let video = resize(&frame, area.width() as u32, area.height() as u32);
                blend(&mut out, &video, Point::new(area.left, area.top));
                out
            }
        }
    }
}

/// BGRA → NV12 (BT.709, limited range), the encoder's input format.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]
pub fn to_nv12(image: &BgraImage) -> Vec<u8> {
    let (w, h) = (image.width() as usize & !1, image.height() as usize & !1);
    let stride = image.width() as usize * 4;
    let src = image.as_bytes();
    let mut out = vec![0u8; w * h * 3 / 2];
    let (luma, chroma) = out.split_at_mut(w * h);
    for y in 0..h {
        let row = &src[y * stride..];
        for x in 0..w {
            let (b, g, r) = (
                i32::from(row[x * 4]),
                i32::from(row[x * 4 + 1]),
                i32::from(row[x * 4 + 2]),
            );
            luma[y * w + x] = ((47 * r + 157 * g + 16 * b + 128) / 256 + 16).clamp(0, 255) as u8;
        }
    }
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let mut sum = [0i32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (y + dy) * stride + (x + dx) * 4;
                sum[0] += i32::from(src[i]);
                sum[1] += i32::from(src[i + 1]);
                sum[2] += i32::from(src[i + 2]);
            }
            let (b, g, r) = (sum[0] / 4, sum[1] / 4, sum[2] / 4);
            let u = (-26 * r - 87 * g + 112 * b + 128) / 256 + 128;
            let v = (112 * r - 102 * g - 10 * b + 128) / 256 + 128;
            let at = (y / 2) * w + x;
            chroma[at] = u.clamp(0, 255) as u8;
            chroma[at + 1] = v.clamp(0, 255) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::{Click, CursorSample, CursorShape};

    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> BgraImage {
        BgraImage::from_raw(w, h, bgra.repeat((w * h) as usize))
    }

    fn png(image: &BgraImage) -> String {
        kadra_core::base64::encode(
            &kadra_encode::encode(image, kadra_core::ImageFormat::Png).unwrap(),
        )
    }

    #[test]
    fn empty_compositions_are_detected() {
        assert!(Composition::default().is_empty());
        let clicks = Composition {
            clicks: Some(ClickStyle {
                ring: false,
                color: [0, 0, 0],
                sound: true,
            }),
            ..Composition::default()
        };
        assert!(clicks.is_empty() && clicks.wants_click_sound() && clicks.uses_track());
    }

    #[test]
    fn overlays_follow_their_time_range() {
        let composition = Composition {
            overlays: vec![OverlayImage {
                png: png(&solid(2, 2, [0, 0, 255, 255])),
                x: 1,
                y: 1,
                start: 1.0,
                end: 2.0,
            }],
            ..Composition::default()
        };
        let composer = Composer::new(&composition, None, (4, 4), None);
        let black = solid(4, 4, [0, 0, 0, 255]);
        assert_eq!(
            composer.render(black.clone(), 0.5).pixel(Point::new(1, 1)),
            Some([0, 0, 0, 255])
        );
        assert_eq!(
            composer.render(black, 1.5).pixel(Point::new(1, 1)),
            Some([0, 0, 255, 255])
        );
    }

    #[test]
    fn the_cursor_is_drawn_scaled_at_its_hot_spot() {
        let track = CursorTrack {
            samples: vec![CursorSample {
                t: 0.0,
                x: 10.0,
                y: 10.0,
                shape: 0,
                visible: true,
            }],
            clicks: vec![Click {
                t: 0.0,
                x: 10.0,
                y: 10.0,
                right: false,
            }],
            shapes: vec![CursorShape {
                hotspot: [0, 0],
                png: png(&solid(2, 2, [255, 255, 255, 255])),
            }],
        };
        let composition = Composition {
            cursor: Some(CursorStyle {
                visible: true,
                scale: 2.0,
                image: None,
                halo: None,
            }),
            ..Composition::default()
        };
        let composer = Composer::new(&composition, Some(track.clone()), (32, 32), None);
        let out = composer.render(solid(32, 32, [0, 0, 0, 255]), 0.1);
        assert_eq!(out.pixel(Point::new(13, 13)), Some([255, 255, 255, 255]));
        assert_eq!(out.pixel(Point::new(14, 14)), Some([0, 0, 0, 255]));
        assert_eq!(composer.click_times(), vec![0.0]);

        let hidden = Composition {
            cursor: Some(CursorStyle {
                visible: false,
                scale: 1.0,
                image: None,
                halo: None,
            }),
            ..Composition::default()
        };
        let composer = Composer::new(&hidden, Some(track), (32, 32), None);
        let out = composer.render(solid(32, 32, [0, 0, 0, 255]), 0.1);
        assert_eq!(out.pixel(Point::new(10, 10)), Some([0, 0, 0, 255]));
    }

    #[test]
    fn canvas_has_the_ratio_and_holds_the_video() {
        let composition = Composition {
            canvas: Some(CanvasStyle {
                ratio: [9, 16],
                colors: vec![[255, 0, 0]],
                padding: 0.0,
            }),
            ..Composition::default()
        };
        let composer = Composer::new(&composition, None, (160, 90), None);
        let (w, h) = composer.output_size();
        // The canvas is no larger than the source on its long side.
        assert_eq!((w, h), (90, 160));
        let out = composer.render(solid(160, 90, [0, 255, 0, 255]), 0.0);
        // Background above the video, video in the middle.
        assert_eq!(out.pixel(Point::new(45, 5)), Some([0, 0, 255, 255]));
        assert_eq!(out.pixel(Point::new(45, 80)), Some([0, 255, 0, 255]));
    }

    #[test]
    fn nv12_matches_known_values() {
        let white = to_nv12(&solid(2, 2, [255, 255, 255, 255]));
        assert_eq!(white, vec![235, 235, 235, 235, 128, 128]);
        let black = to_nv12(&solid(2, 2, [0, 0, 0, 255]));
        assert_eq!(black, vec![16, 16, 16, 16, 128, 128]);
    }
}
