//! Integer pixel geometry.
//!
//! Coordinates are physical pixels. Rectangles are half-open: `right` and
//! `bottom` are excluded, so `width = right - left`.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Chebyshev distance: the larger of the horizontal and vertical offsets.
    pub fn distance_max(self, other: Self) -> i32 {
        (self.x - other.x).abs().max((self.y - other.y).abs())
    }

    #[must_use]
    pub fn offset(self, dx: i32, dy: i32) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const fn from_size(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self::new(x, y, x + width, y + height)
    }

    /// Smallest rectangle covering both corners, inclusive of both pixels.
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self::new(
            a.x.min(b.x),
            a.y.min(b.y),
            a.x.max(b.x) + 1,
            a.y.max(b.y) + 1,
        )
    }

    /// Bounding box of a set of points, inclusive of every pixel.
    pub fn bounding(points: &[Point]) -> Option<Self> {
        let first = *points.first()?;
        Some(
            points
                .iter()
                .fold(Self::from_corners(first, first), |r, &p| {
                    r.union(Self::from_corners(p, p))
                }),
        )
    }

    pub const fn width(&self) -> i32 {
        self.right - self.left
    }

    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub const fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub const fn contains(&self, p: Point) -> bool {
        p.x >= self.left && p.x < self.right && p.y >= self.top && p.y < self.bottom
    }

    #[must_use]
    pub fn intersect(&self, other: Self) -> Self {
        let r = Self::new(
            self.left.max(other.left),
            self.top.max(other.top),
            self.right.min(other.right),
            self.bottom.min(other.bottom),
        );
        if r.is_empty() { Self::default() } else { r }
    }

    #[must_use]
    pub fn union(&self, other: Self) -> Self {
        Self::new(
            self.left.min(other.left),
            self.top.min(other.top),
            self.right.max(other.right),
            self.bottom.max(other.bottom),
        )
    }

    #[must_use]
    pub const fn offset(&self, dx: i32, dy: i32) -> Self {
        Self::new(
            self.left + dx,
            self.top + dy,
            self.right + dx,
            self.bottom + dy,
        )
    }

    /// Moves `p` inside the rectangle.
    pub fn clamp(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.left, (self.right - 1).max(self.left)),
            p.y.clamp(self.top, (self.bottom - 1).max(self.top)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_inclusive() {
        let r = Rect::from_corners(Point::new(10, 20), Point::new(5, 25));
        assert_eq!(r, Rect::new(5, 20, 11, 26));
        assert_eq!((r.width(), r.height()), (6, 6));
    }

    #[test]
    fn intersection_and_union() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 20, 20);
        assert_eq!(a.intersect(b), Rect::new(5, 5, 10, 10));
        assert_eq!(a.union(b), Rect::new(0, 0, 20, 20));
        assert!(a.intersect(Rect::new(10, 0, 20, 10)).is_empty());
    }

    #[test]
    fn bounding_box() {
        let pts = [Point::new(3, 4), Point::new(-2, 8), Point::new(5, 1)];
        assert_eq!(Rect::bounding(&pts), Some(Rect::new(-2, 1, 6, 9)));
        assert_eq!(Rect::bounding(&[]), None);
    }

    #[test]
    fn clamp_stays_inside() {
        let r = Rect::new(0, 0, 100, 50);
        assert_eq!(r.clamp(Point::new(-5, 70)), Point::new(0, 49));
        assert!(r.contains(r.clamp(Point::new(1000, 1000))));
    }
}
