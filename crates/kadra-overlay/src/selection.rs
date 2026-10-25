//! Selection logic, independent of any windowing or rendering code.

use kadra_core::{Point, Rect};

/// Below this distance (in pixels) a press-release is a click, not a drag.
const DRAG_THRESHOLD: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Drag a rectangle, or click to take the window under the cursor.
    #[default]
    Region,
    /// Draw any shape; the outside becomes transparent.
    Freeform,
    /// Click a monitor.
    Screen,
}

/// What the user picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Region(Rect),
    Freeform(Vec<Point>),
}

/// Answers "what is under this point" for click-to-select.
pub trait Targets {
    /// Front-most window under `p`, clipped to the capture.
    fn window_at(&self, p: Point) -> Option<Rect>;
    /// Monitor containing `p` (or the closest one).
    fn monitor_at(&self, p: Point) -> Rect;
    /// The whole capture.
    fn bounds(&self) -> Rect;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Idle,
    Dragging { anchor: Point },
    Drawing { points: Vec<Point> },
}

#[derive(Debug, Clone)]
pub struct Selection {
    mode: Mode,
    state: State,
    cursor: Point,
}

impl Selection {
    pub fn new(mode: Mode, cursor: Point) -> Self {
        Self {
            mode,
            state: State::Idle,
            cursor,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn cursor(&self) -> Point {
        self.cursor
    }

    /// Switches mode, abandoning any selection in progress.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.state = State::Idle;
    }

    /// True while a button is held down to drag or draw.
    pub fn is_active(&self) -> bool {
        self.state != State::Idle
    }

    /// Abandons the selection in progress. Returns false if there was none.
    pub fn abort(&mut self) -> bool {
        let was_active = self.is_active();
        self.state = State::Idle;
        was_active
    }

    pub fn press(&mut self, p: Point, targets: &impl Targets) {
        let p = targets.bounds().clamp(p);
        self.cursor = p;
        self.state = match self.mode {
            Mode::Region | Mode::Screen => State::Dragging { anchor: p },
            Mode::Freeform => State::Drawing { points: vec![p] },
        };
    }

    pub fn moved(&mut self, p: Point, targets: &impl Targets) {
        let p = targets.bounds().clamp(p);
        self.cursor = p;
        if let State::Drawing { points } = &mut self.state
            && points.last() != Some(&p)
        {
            points.push(p);
        }
    }

    /// Ends a press. Returns the selection, or `None` if it was too small to
    /// mean anything (the overlay then simply stays open).
    pub fn release(&mut self, p: Point, targets: &impl Targets) -> Option<Outcome> {
        self.moved(p, targets);
        let p = self.cursor;
        let state = std::mem::replace(&mut self.state, State::Idle);
        match (self.mode, state) {
            (Mode::Region, State::Dragging { anchor }) => {
                if anchor.distance_max(p) < DRAG_THRESHOLD {
                    Some(Outcome::Region(Self::target_at(p, targets)))
                } else {
                    Some(Outcome::Region(Rect::from_corners(anchor, p)))
                }
            }
            (Mode::Screen, State::Dragging { .. }) => Some(Outcome::Region(targets.monitor_at(p))),
            (Mode::Freeform, State::Drawing { points }) => {
                let bounds = Rect::bounding(&points)?;
                let big_enough = points.len() >= 3
                    && bounds.width() >= DRAG_THRESHOLD
                    && bounds.height() >= DRAG_THRESHOLD;
                big_enough.then_some(Outcome::Freeform(points))
            }
            _ => None,
        }
    }

    /// Selects what is under the cursor without a mouse click (Enter key).
    pub fn confirm_hover(&self, targets: &impl Targets) -> Option<Outcome> {
        match self.mode {
            Mode::Region => Some(Outcome::Region(Self::target_at(self.cursor, targets))),
            Mode::Screen => Some(Outcome::Region(targets.monitor_at(self.cursor))),
            Mode::Freeform => None,
        }
    }

    fn target_at(p: Point, targets: &impl Targets) -> Rect {
        targets
            .window_at(p)
            .unwrap_or_else(|| targets.monitor_at(p))
    }

    /// Rectangle being dragged, if any.
    pub fn dragged_rect(&self) -> Option<Rect> {
        match (&self.state, self.mode) {
            (State::Dragging { anchor }, Mode::Region)
                if anchor.distance_max(self.cursor) >= DRAG_THRESHOLD =>
            {
                Some(Rect::from_corners(*anchor, self.cursor))
            }
            _ => None,
        }
    }

    /// Freeform path being drawn, if any.
    pub fn drawn_path(&self) -> Option<&[Point]> {
        match &self.state {
            State::Drawing { points } => Some(points),
            _ => None,
        }
    }

    /// Area highlighted under the cursor when nothing is being dragged.
    pub fn hover_rect(&self, targets: &impl Targets) -> Option<Rect> {
        match (&self.state, self.mode) {
            (State::Idle, Mode::Region) => Some(Self::target_at(self.cursor, targets)),
            (State::Idle | State::Dragging { .. }, Mode::Screen) => {
                Some(targets.monitor_at(self.cursor))
            }
            (State::Dragging { anchor }, Mode::Region)
                if anchor.distance_max(self.cursor) < DRAG_THRESHOLD =>
            {
                Some(Self::target_at(self.cursor, targets))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two monitors side by side, one window on the left one.
    struct Fake;

    const LEFT: Rect = Rect::new(0, 0, 100, 100);
    const RIGHT: Rect = Rect::new(100, 0, 200, 100);
    const WINDOW: Rect = Rect::new(10, 10, 50, 50);

    impl Targets for Fake {
        fn window_at(&self, p: Point) -> Option<Rect> {
            WINDOW.contains(p).then_some(WINDOW)
        }
        fn monitor_at(&self, p: Point) -> Rect {
            if RIGHT.contains(p) { RIGHT } else { LEFT }
        }
        fn bounds(&self) -> Rect {
            Rect::new(0, 0, 200, 100)
        }
    }

    fn click(sel: &mut Selection, p: Point) -> Option<Outcome> {
        sel.press(p, &Fake);
        sel.release(p, &Fake)
    }

    #[test]
    fn click_selects_window_under_cursor() {
        let mut sel = Selection::new(Mode::Region, Point::default());
        assert_eq!(
            click(&mut sel, Point::new(20, 20)),
            Some(Outcome::Region(WINDOW))
        );
    }

    #[test]
    fn click_outside_windows_selects_monitor() {
        let mut sel = Selection::new(Mode::Region, Point::default());
        assert_eq!(
            click(&mut sel, Point::new(150, 80)),
            Some(Outcome::Region(RIGHT))
        );
    }

    #[test]
    fn drag_selects_rectangle_inclusive() {
        let mut sel = Selection::new(Mode::Region, Point::default());
        sel.press(Point::new(60, 70), &Fake);
        sel.moved(Point::new(30, 20), &Fake);
        assert_eq!(sel.dragged_rect(), Some(Rect::new(30, 20, 61, 71)));
        assert_eq!(
            sel.release(Point::new(30, 20), &Fake),
            Some(Outcome::Region(Rect::new(30, 20, 61, 71)))
        );
        assert!(!sel.is_active());
    }

    #[test]
    fn drag_is_clamped_to_bounds() {
        let mut sel = Selection::new(Mode::Region, Point::default());
        sel.press(Point::new(190, 90), &Fake);
        let out = sel.release(Point::new(500, 500), &Fake);
        assert_eq!(out, Some(Outcome::Region(Rect::new(190, 90, 200, 100))));
    }

    #[test]
    fn small_drag_counts_as_click() {
        let mut sel = Selection::new(Mode::Region, Point::default());
        sel.press(Point::new(20, 20), &Fake);
        sel.moved(Point::new(22, 21), &Fake);
        assert_eq!(sel.dragged_rect(), None);
        assert_eq!(sel.hover_rect(&Fake), Some(WINDOW));
        assert_eq!(
            sel.release(Point::new(22, 21), &Fake),
            Some(Outcome::Region(WINDOW))
        );
    }

    #[test]
    fn screen_mode_selects_monitor() {
        let mut sel = Selection::new(Mode::Screen, Point::new(150, 50));
        assert_eq!(sel.hover_rect(&Fake), Some(RIGHT));
        assert_eq!(
            click(&mut sel, Point::new(20, 20)),
            Some(Outcome::Region(LEFT))
        );
    }

    #[test]
    fn freeform_returns_path() {
        let mut sel = Selection::new(Mode::Freeform, Point::default());
        sel.press(Point::new(10, 10), &Fake);
        sel.moved(Point::new(40, 10), &Fake);
        sel.moved(Point::new(40, 10), &Fake); // duplicates are ignored
        sel.moved(Point::new(40, 40), &Fake);
        assert_eq!(sel.drawn_path().map(<[Point]>::len), Some(3));
        let out = sel.release(Point::new(10, 40), &Fake);
        assert_eq!(
            out,
            Some(Outcome::Freeform(vec![
                Point::new(10, 10),
                Point::new(40, 10),
                Point::new(40, 40),
                Point::new(10, 40),
            ]))
        );
    }

    #[test]
    fn tiny_freeform_is_ignored() {
        let mut sel = Selection::new(Mode::Freeform, Point::default());
        assert_eq!(click(&mut sel, Point::new(10, 10)), None);
        assert!(!sel.is_active());
    }

    #[test]
    fn mode_change_and_abort_reset_state() {
        let mut sel = Selection::new(Mode::Region, Point::default());
        sel.press(Point::new(10, 10), &Fake);
        assert!(sel.abort());
        assert!(!sel.abort());
        sel.press(Point::new(10, 10), &Fake);
        sel.set_mode(Mode::Freeform);
        assert!(!sel.is_active());
        assert_eq!(sel.confirm_hover(&Fake), None);
    }

    #[test]
    fn enter_confirms_hovered_target() {
        let mut sel = Selection::new(Mode::Region, Point::new(20, 20));
        assert_eq!(sel.confirm_hover(&Fake), Some(Outcome::Region(WINDOW)));
        sel.set_mode(Mode::Screen);
        assert_eq!(sel.confirm_hover(&Fake), Some(Outcome::Region(LEFT)));
    }
}
