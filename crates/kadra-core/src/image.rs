//! CPU image buffer in the native Windows pixel layout.

use crate::geometry::{Point, Rect};

/// Tightly packed 8-bit BGRA image (straight alpha, top-down rows).
#[derive(Clone, PartialEq, Eq)]
pub struct BgraImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl std::fmt::Debug for BgraImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BgraImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl BgraImage {
    /// Creates an opaque black image.
    pub fn new(width: u32, height: u32) -> Self {
        let mut data = vec![0; width as usize * height as usize * 4];
        for px in data.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Self {
            width,
            height,
            data,
        }
    }

    /// Creates a fully transparent image (cheap: zeroed memory).
    pub fn zeroed(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * 4],
        }
    }

    /// Wraps existing pixel data.
    ///
    /// # Panics
    /// If `data` is not exactly `width * height * 4` bytes long.
    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(
            data.len(),
            width as usize * height as usize * 4,
            "BGRA buffer size mismatch"
        );
        Self {
            width,
            height,
            data,
        }
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn stride(&self) -> usize {
        self.width as usize * 4
    }

    pub fn bounds(&self) -> Rect {
        #[allow(clippy::cast_possible_wrap)]
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }

    /// Returns the `[b, g, r, a]` pixel at `p`, or `None` outside the image.
    pub fn pixel(&self, p: Point) -> Option<[u8; 4]> {
        if !self.bounds().contains(p) {
            return None;
        }
        #[allow(clippy::cast_sign_loss)]
        let i = p.y as usize * self.stride() + p.x as usize * 4;
        self.data[i..i + 4].try_into().ok()
    }

    /// Copies `src` rows into this image with its top-left corner at `at`.
    /// Parts falling outside are clipped.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
    pub fn blit(
        &mut self,
        src: &[u8],
        src_stride: usize,
        src_width: u32,
        src_height: u32,
        at: Point,
    ) {
        let dest = Rect::from_size(at.x, at.y, src_width as i32, src_height as i32)
            .intersect(self.bounds());
        if dest.is_empty() {
            return;
        }
        let row_bytes = dest.width() as usize * 4;
        let src_x = (dest.left - at.x) as usize * 4;
        let stride = self.stride();
        for y in dest.top..dest.bottom {
            let src_row = (y - at.y) as usize * src_stride + src_x;
            let dst_row = y as usize * stride + dest.left as usize * 4;
            self.data[dst_row..dst_row + row_bytes]
                .copy_from_slice(&src[src_row..src_row + row_bytes]);
        }
    }

    /// Copies the part of the image inside `rect` (clipped to the image).
    #[must_use]
    #[allow(clippy::cast_sign_loss)]
    pub fn crop(&self, rect: Rect) -> Self {
        let r = rect.intersect(self.bounds());
        let (w, h) = (r.width().max(0) as u32, r.height().max(0) as u32);
        let row = w as usize * 4;
        let mut data = Vec::with_capacity(row * h as usize);
        for y in r.top..r.bottom {
            let start = y as usize * self.stride() + r.left as usize * 4;
            data.extend_from_slice(&self.data[start..start + row]);
        }
        Self::from_raw(w, h, data)
    }

    /// Makes every pixel outside `polygon` fully transparent.
    ///
    /// The polygon is given in image coordinates and filled with the even-odd
    /// rule, sampling each pixel at its centre.
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss
    )]
    pub fn mask_outside_polygon(&mut self, polygon: &[Point]) {
        let stride = self.stride();
        let width = self.width as usize;
        let mut crossings: Vec<f64> = Vec::new();
        for y in 0..self.height as usize {
            let cy = y as f64 + 0.5;
            crossings.clear();
            for (i, a) in polygon.iter().enumerate() {
                let b = polygon[(i + 1) % polygon.len()];
                let (ay, by) = (f64::from(a.y) + 0.5, f64::from(b.y) + 0.5);
                if (ay <= cy) != (by <= cy) {
                    let t = (cy - ay) / (by - ay);
                    let (ax, bx) = (f64::from(a.x) + 0.5, f64::from(b.x) + 0.5);
                    crossings.push(ax + t * (bx - ax));
                }
            }
            crossings.sort_by(f64::total_cmp);

            let row = &mut self.data[y * stride..(y + 1) * stride];
            let mut x = 0usize;
            for pair in crossings.as_chunks::<2>().0 {
                // Pixels whose centre lies in [start, end) are inside.
                let start = ((pair[0] - 0.5).ceil().max(0.0) as usize).min(width);
                let end = ((pair[1] - 0.5).ceil().max(0.0) as usize).min(width);
                clear_alpha(&mut row[x * 4..start.max(x) * 4]);
                x = end.max(x);
            }
            clear_alpha(&mut row[x * 4..]);
        }
    }
}

/// Transparent pixels are zeroed entirely so they are valid premultiplied too.
fn clear_alpha(pixels: &mut [u8]) {
    pixels.fill(0);
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> BgraImage {
        let mut img = BgraImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize * 4;
                img.data[i] = x as u8;
                img.data[i + 1] = y as u8;
            }
        }
        img
    }

    #[test]
    fn crop_copies_the_right_pixels() {
        let img = gradient(10, 8);
        let c = img.crop(Rect::new(2, 3, 5, 7));
        assert_eq!((c.width(), c.height()), (3, 4));
        assert_eq!(c.pixel(Point::new(0, 0)), Some([2, 3, 0, 255]));
        assert_eq!(c.pixel(Point::new(2, 3)), Some([4, 6, 0, 255]));
    }

    #[test]
    fn crop_is_clipped() {
        let img = gradient(10, 8);
        let c = img.crop(Rect::new(-5, -5, 3, 2));
        assert_eq!((c.width(), c.height()), (3, 2));
        assert_eq!(img.crop(Rect::new(20, 20, 30, 30)).as_bytes(), &[] as &[u8]);
    }

    #[test]
    fn blit_places_and_clips() {
        let mut dst = BgraImage::new(4, 4);
        let src = vec![9u8; 3 * 3 * 4];
        dst.blit(&src, 12, 3, 3, Point::new(2, 2));
        assert_eq!(dst.pixel(Point::new(2, 2)), Some([9; 4]));
        assert_eq!(dst.pixel(Point::new(3, 3)), Some([9; 4]));
        assert_eq!(dst.pixel(Point::new(1, 1)), Some([0, 0, 0, 255]));
    }

    #[test]
    fn polygon_mask_keeps_inside_only() {
        let mut img = BgraImage::new(10, 10);
        // Square covering pixels 2..=7 in both directions.
        let square = [
            Point::new(2, 2),
            Point::new(8, 2),
            Point::new(8, 8),
            Point::new(2, 8),
        ];
        img.mask_outside_polygon(&square);
        assert_eq!(img.pixel(Point::new(5, 5)).unwrap()[3], 255);
        assert_eq!(img.pixel(Point::new(2, 2)).unwrap()[3], 255);
        assert_eq!(img.pixel(Point::new(1, 5)).unwrap()[3], 0);
        assert_eq!(img.pixel(Point::new(9, 9)).unwrap()[3], 0);
        assert_eq!(img.pixel(Point::new(8, 5)).unwrap()[3], 0);
    }

    #[test]
    fn degenerate_polygon_clears_everything() {
        let mut img = BgraImage::new(4, 4);
        img.mask_outside_polygon(&[Point::new(1, 1)]);
        assert!(img.as_bytes().iter().all(|&b| b == 0));
    }
}
