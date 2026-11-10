//! The mouse cursor, recorded as data next to the video.
//!
//! Recordings made for editing keep a copy of the video *without* the
//! cursor, and this track: where the cursor was at each frame, what it
//! looked like, and where the user clicked. The editor can then hide the
//! cursor, enlarge it, replace it, or mark the clicks, after the fact.

use std::collections::HashMap;
use std::path::Path;

use capshot_core::{BgraImage, Point};
use serde::{Deserialize, Serialize};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, HGDIOBJ, ReleaseDC,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, GetCursorInfo, GetIconInfo, HICON, ICONINFO,
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CursorSample {
    /// Recording time, in seconds (pauses excluded, like the video).
    pub t: f64,
    /// Position of the hot spot, in pixels of the recorded area.
    pub x: f32,
    pub y: f32,
    /// Index in [`CursorTrack::shapes`].
    pub shape: u32,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Click {
    pub t: f64,
    pub x: f32,
    pub y: f32,
    pub right: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorShape {
    pub hotspot: [i32; 2],
    /// PNG, base64.
    pub png: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CursorTrack {
    pub samples: Vec<CursorSample>,
    pub clicks: Vec<Click>,
    pub shapes: Vec<CursorShape>,
}

impl CursorTrack {
    pub fn load(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Cursor at time `t`: position (interpolated between samples) and
    /// shape, or `None` when hidden.
    #[allow(clippy::cast_possible_truncation)]
    pub fn at(&self, t: f64) -> Option<(f32, f32, u32)> {
        let i = self.samples.partition_point(|s| s.t <= t);
        let before = self.samples.get(i.checked_sub(1)?)?;
        if !before.visible {
            return None;
        }
        match self.samples.get(i) {
            Some(after) if after.visible && after.t > before.t => {
                let k = ((t - before.t) / (after.t - before.t)) as f32;
                Some((
                    before.x + (after.x - before.x) * k,
                    before.y + (after.y - before.y) * k,
                    before.shape,
                ))
            }
            _ => Some((before.x, before.y, before.shape)),
        }
    }
}

/// Samples the cursor while recording.
#[derive(Debug)]
pub struct CursorTracker {
    /// Desktop position of the recorded area.
    origin: Point,
    track: CursorTrack,
    shapes: HashMap<isize, u32>,
    buttons: [bool; 2],
}

impl CursorTracker {
    pub fn new(origin: Point) -> Self {
        Self {
            origin,
            track: CursorTrack::default(),
            shapes: HashMap::new(),
            buttons: [false; 2],
        }
    }

    /// Records the cursor and any click at recording time `t`.
    #[allow(clippy::cast_precision_loss)]
    pub fn sample(&mut self, t: f64) {
        let mut info = CURSORINFO {
            cbSize: u32::try_from(size_of::<CURSORINFO>()).unwrap_or(0),
            ..Default::default()
        };
        // SAFETY: correctly sized out structure.
        if unsafe { GetCursorInfo(&mut info) }.is_err() {
            return;
        }
        let x = (info.ptScreenPos.x - self.origin.x) as f32;
        let y = (info.ptScreenPos.y - self.origin.y) as f32;
        let visible = info.flags.0 & CURSOR_SHOWING.0 != 0 && !info.hCursor.is_invalid();
        let shape = if visible {
            self.shape(info.hCursor.0 as isize)
        } else {
            None
        };
        self.track.samples.push(CursorSample {
            t,
            x,
            y,
            shape: shape.unwrap_or(0),
            visible: shape.is_some(),
        });

        for (i, key) in [VK_LBUTTON, VK_RBUTTON].into_iter().enumerate() {
            // SAFETY: plain query. The low bit says the button was pressed
            // since the last call, which catches clicks between samples.
            let state = unsafe { GetAsyncKeyState(i32::from(key.0)) };
            let down = state < 0;
            if (down || state & 1 != 0) && !self.buttons[i] {
                self.track.clicks.push(Click {
                    t,
                    x,
                    y,
                    right: i == 1,
                });
            }
            self.buttons[i] = down;
        }
    }

    fn shape(&mut self, handle: isize) -> Option<u32> {
        if let Some(&id) = self.shapes.get(&handle) {
            return Some(id);
        }
        let (image, hotspot) = cursor_image(handle)?;
        let png = capshot_encode::encode(&image, capshot_core::ImageFormat::Png).ok()?;
        let id = u32::try_from(self.track.shapes.len()).ok()?;
        self.track.shapes.push(CursorShape {
            hotspot,
            png: capshot_core::base64::encode(&png),
        });
        self.shapes.insert(handle, id);
        Some(id)
    }

    pub fn finish(self) -> CursorTrack {
        self.track
    }
}

/// Reads a bitmap as top-down BGRA.
fn read_bitmap(bitmap: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    // SAFETY: GDI queries with correctly sized structures; the screen DC is
    // released before returning.
    unsafe {
        let mut info = BITMAP::default();
        let size = i32::try_from(size_of::<BITMAP>()).ok()?;
        if GetObjectW(HGDIOBJ(bitmap.0), size, Some((&raw mut info).cast())) == 0 {
            return None;
        }
        let (w, h) = (
            u32::try_from(info.bmWidth).ok()?,
            u32::try_from(info.bmHeight).ok()?,
        );
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).ok()?,
                biWidth: info.bmWidth,
                biHeight: -info.bmHeight,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        let dc = GetDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            h,
            Some(pixels.as_mut_ptr().cast()),
            &mut header,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        (lines > 0).then_some((w, h, pixels))
    }
}

/// The cursor's image (with transparency) and hot spot.
#[allow(clippy::single_match_else)]
fn cursor_image(handle: isize) -> Option<(BgraImage, [i32; 2])> {
    let mut info = ICONINFO::default();
    // SAFETY: `handle` is the current cursor; the bitmaps it returns are
    // ours and deleted below.
    unsafe { GetIconInfo(HICON(handle as *mut _), &mut info).ok()? };
    let hotspot = [
        i32::try_from(info.xHotspot).unwrap_or(0),
        i32::try_from(info.yHotspot).unwrap_or(0),
    ];
    let mask = read_bitmap(info.hbmMask);
    let color = (!info.hbmColor.is_invalid())
        .then(|| read_bitmap(info.hbmColor))
        .flatten();
    // SAFETY: deleting the bitmaps GetIconInfo created for us.
    unsafe {
        let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        }
    }
    let (mw, mh, mask) = mask?;
    let image = match color {
        Some((w, h, mut pixels)) => {
            // Old-style colour cursors carry no alpha: take it from the mask.
            if pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
                for (i, p) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let transparent = mask.get(i * 4).is_some_and(|&m| m != 0);
                    p[3] = if transparent { 0 } else { 255 };
                }
            }
            BgraImage::from_raw(w, h, pixels)
        }
        None => {
            // Monochrome: the mask holds AND (top half) and XOR (bottom).
            let h = mh / 2;
            let half = (mw * h * 4) as usize;
            let mut pixels = vec![0u8; half];
            for (i, p) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let and = mask[i * 4] != 0;
                let xor = mask[half + i * 4] != 0;
                *p = match (and, xor) {
                    (false, false) => [0, 0, 0, 255],
                    (false, true) => [255, 255, 255, 255],
                    // "Invert the screen": drawn as a dark outline.
                    (true, true) => [0, 0, 0, 200],
                    (true, false) => [0, 0, 0, 0],
                };
            }
            BgraImage::from_raw(mw, h, pixels)
        }
    };
    Some((image, hotspot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_interpolated_and_hidden_cursors_skipped() {
        let sample = |t, x, visible| CursorSample {
            t,
            x,
            y: 10.0,
            shape: 0,
            visible,
        };
        let track = CursorTrack {
            samples: vec![
                sample(0.0, 0.0, true),
                sample(1.0, 100.0, true),
                sample(2.0, 100.0, false),
            ],
            ..CursorTrack::default()
        };
        assert_eq!(track.at(-1.0), None);
        assert_eq!(track.at(0.5), Some((50.0, 10.0, 0)));
        assert_eq!(track.at(1.5), Some((100.0, 10.0, 0)));
        assert_eq!(track.at(2.5), None);
    }

    #[test]
    fn the_current_cursor_can_be_read() {
        let mut tracker = CursorTracker::new(Point::new(0, 0));
        tracker.sample(0.0);
        let track = tracker.finish();
        assert_eq!(track.samples.len(), 1);
        // A desktop session always has a cursor shape to read.
        if track.samples[0].visible {
            assert_eq!(track.shapes.len(), 1);
            assert!(capshot_core::base64::decode(&track.shapes[0].png).is_some());
        }
    }
}
