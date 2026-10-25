//! Placement of the overlay widgets (toolbar, magnifier, size label), in
//! physical pixels relative to the capture.

use kadra_core::{Point, Rect};

use crate::selection::Mode;

#[allow(clippy::cast_possible_truncation)]
fn px(value: f32, scale: f32) -> i32 {
    (value * scale).round() as i32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Mode(Mode),
    Close,
}

pub const BUTTONS: [Button; 4] = [
    Button::Mode(Mode::Region),
    Button::Mode(Mode::Freeform),
    Button::Mode(Mode::Screen),
    Button::Close,
];

/// Floating toolbar centred at the top of a monitor.
#[derive(Debug, Clone)]
pub struct Toolbar {
    pub frame: Rect,
    pub buttons: [(Button, Rect); 4],
    pub scale: f32,
}

impl Toolbar {
    pub fn new(monitor: Rect, scale: f32) -> Self {
        let button = px(40.0, scale);
        let pad = px(4.0, scale);
        let separator = px(12.0, scale);
        let width = pad * 2 + button * 4 + pad * 2 + separator;
        let height = button + pad * 2;
        let left = monitor.left + (monitor.width() - width) / 2;
        let top = monitor.top + px(16.0, scale);
        let frame = Rect::from_size(left, top, width, height);

        let mut x = left + pad;
        let buttons = BUTTONS.map(|b| {
            if b == Button::Close {
                x += separator - pad;
            }
            let r = Rect::from_size(x, top + pad, button, button);
            x += button + pad;
            (b, r)
        });
        Self {
            frame,
            buttons,
            scale,
        }
    }

    pub fn hit(&self, p: Point) -> Option<Button> {
        self.buttons
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|&(b, _)| b)
    }

    pub fn contains(&self, p: Point) -> bool {
        self.frame.contains(p)
    }
}

/// Number of captured pixels shown across the magnifier (odd, so one is
/// centred under the cursor).
pub const MAGNIFIER_CELLS: i32 = 15;

/// Zoomed view of the pixels around the cursor, with a colour readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Magnifier {
    /// Where the zoomed pixels are drawn.
    pub frame: Rect,
    /// Readout below (or above) the zoom.
    pub info: Rect,
    /// Captured pixels shown, centred on the cursor (may exceed the capture).
    pub source: Rect,
    /// Size of one magnified pixel.
    pub cell: i32,
}

impl Magnifier {
    pub fn new(cursor: Point, monitor: Rect, scale: f32) -> Self {
        let cell = px(8.0, scale).max(2);
        let size = cell * MAGNIFIER_CELLS;
        let info_height = px(36.0, scale);
        let gap = px(4.0, scale);
        let offset = px(20.0, scale);
        let total_height = size + gap + info_height;

        let x = if cursor.x + offset + size <= monitor.right {
            cursor.x + offset
        } else {
            cursor.x - offset - size
        };
        let y = if cursor.y + offset + total_height <= monitor.bottom {
            cursor.y + offset
        } else {
            cursor.y - offset - total_height
        };
        // Keep it on the monitor even on tiny screens.
        let x = x.clamp(monitor.left, (monitor.right - size).max(monitor.left));
        let y = y.clamp(
            monitor.top,
            (monitor.bottom - total_height).max(monitor.top),
        );

        let half = MAGNIFIER_CELLS / 2;
        Self {
            frame: Rect::from_size(x, y, size, size),
            info: Rect::from_size(x, y + size + gap, size, info_height),
            source: Rect::from_size(
                cursor.x - half,
                cursor.y - half,
                MAGNIFIER_CELLS,
                MAGNIFIER_CELLS,
            ),
            cell,
        }
    }
}

/// Position of the "width × height" label: just below the selection, or
/// inside it when there is no room below.
pub fn size_label(selection: Rect, bounds: Rect, label_size: (i32, i32), scale: f32) -> Rect {
    let (w, h) = label_size;
    let gap = px(6.0, scale);
    let y = if selection.bottom + gap + h <= bounds.bottom {
        selection.bottom + gap
    } else {
        selection.bottom - gap - h
    };
    let x = selection
        .left
        .clamp(bounds.left, (bounds.right - w).max(bounds.left));
    Rect::from_size(x, y.max(bounds.top), w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn toolbar_is_centred_and_hit_tested() {
        let bar = Toolbar::new(MONITOR, 1.0);
        assert_eq!(bar.frame.left + bar.frame.width() / 2, 960);
        assert_eq!(bar.frame.top, 16);
        for (button, rect) in bar.buttons {
            let centre = Point::new(rect.left + rect.width() / 2, rect.top + rect.height() / 2);
            assert_eq!(bar.hit(centre), Some(button));
            assert!(bar.frame.contains(Point::new(rect.left, rect.top)));
            assert!(
                bar.frame
                    .contains(Point::new(rect.right - 1, rect.bottom - 1))
            );
        }
        assert_eq!(bar.hit(Point::new(5, 500)), None);
    }

    #[test]
    fn toolbar_scales_with_dpi() {
        let normal = Toolbar::new(MONITOR, 1.0);
        let large = Toolbar::new(MONITOR, 1.5);
        assert_eq!(large.frame.height(), normal.frame.height() * 3 / 2);
    }

    #[test]
    fn magnifier_prefers_bottom_right() {
        let m = Magnifier::new(Point::new(100, 100), MONITOR, 1.0);
        assert_eq!((m.frame.left, m.frame.top), (120, 120));
        assert_eq!(m.frame.width(), 15 * 8);
        assert_eq!(m.source, Rect::new(93, 93, 108, 108));
    }

    #[test]
    fn magnifier_flips_near_edges() {
        let m = Magnifier::new(Point::new(1910, 1070), MONITOR, 1.0);
        assert!(m.frame.right <= 1910);
        assert!(m.info.bottom <= 1070);
        assert!(MONITOR.contains(Point::new(m.frame.left, m.frame.top)));
    }

    #[test]
    fn size_label_moves_inside_at_bottom_edge() {
        let below = size_label(Rect::new(10, 10, 200, 200), MONITOR, (80, 20), 1.0);
        assert_eq!(below.top, 206);
        let inside = size_label(Rect::new(10, 10, 200, 1080), MONITOR, (80, 20), 1.0);
        assert!(inside.bottom <= 1080);
    }
}
