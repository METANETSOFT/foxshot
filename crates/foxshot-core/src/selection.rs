//! Interactive region selection: a pure state machine with no I/O, no
//! windowing and no rendering. The UI crate drives it with real input
//! events and draws whatever [`SelectionState::rect`] reports; nothing in
//! here knows a screen exists.
//!
//! Guarantees every operation upholds:
//! - the selection never leaves `bounds` (every result is clamped);
//! - a drag in any direction yields a normalised rect (never negative
//!   width or height);
//! - a click without a drag finishes as a cancel, not a 0×0 capture —
//!   the phase is `Settled` but [`SelectionState::rect`] is `None`.

use crate::geometry::{Point, Rect, Size};

/// Where the selection interaction currently is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionPhase {
    /// Nothing has started yet.
    Idle,
    /// A drag is in progress (`begin` called, `finish`/`cancel` not yet).
    Dragging,
    /// A drag finished (or a click came and went) — the rect, if any, is final.
    Settled,
    /// The interaction was cancelled; there is no selection.
    Cancelled,
}

/// One of the eight resize handles on a settled selection, or its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handle {
    /// Top-left corner.
    TopLeft,
    /// Top edge midpoint.
    Top,
    /// Top-right corner.
    TopRight,
    /// Right edge midpoint.
    Right,
    /// Bottom-right corner.
    BottomRight,
    /// Bottom edge midpoint.
    Bottom,
    /// Bottom-left corner.
    BottomLeft,
    /// Left edge midpoint.
    Left,
    /// The interior of the selection (a move, not a resize).
    Body,
}

/// The whole selection interaction, driven by the caller's input events.
#[derive(Debug, Clone)]
pub struct SelectionState {
    bounds: Rect,
    phase: SelectionPhase,
    anchor: Option<Point>,
    cursor: Point,
    rect: Option<Rect>,
    square_lock: bool,
}

impl SelectionState {
    /// A fresh selection constrained to `bounds` (the display area).
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            phase: SelectionPhase::Idle,
            anchor: None,
            cursor: bounds.origin,
            rect: None,
            square_lock: false,
        }
    }

    /// Current phase of the interaction.
    pub fn phase(&self) -> SelectionPhase {
        self.phase
    }

    /// The current selection rectangle, clamped inside `bounds`.
    ///
    /// While dragging this is the live drag rect; after `finish` it is the
    /// final one. `None` when there is nothing to capture: before any drag,
    /// after a cancel, or after a click with no movement.
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    /// The display area this selection may never leave.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// True while Shift-style square locking is engaged.
    pub fn square_lock(&self) -> bool {
        self.square_lock
    }

    /// The last known cursor position (clamped into `bounds`).
    pub fn cursor(&self) -> Point {
        self.cursor
    }

    /// Enables or disables square locking. When engaged mid-drag the live
    /// rect is recomputed immediately.
    pub fn set_square_lock(&mut self, on: bool) {
        self.square_lock = on;
        if self.phase == SelectionPhase::Dragging {
            self.update_drag_rect();
        }
    }

    /// Starts a drag at `point` (left-button press). Clears any previous
    /// selection and moves to [`SelectionPhase::Dragging`].
    pub fn begin(&mut self, point: Point) {
        let point = self.clamp_point(point);
        self.anchor = Some(point);
        self.cursor = point;
        self.rect = None;
        self.phase = SelectionPhase::Dragging;
    }

    /// Moves the drag cursor to `point` (mouse motion). Outside a drag this
    /// only tracks the cursor position.
    pub fn drag_to(&mut self, point: Point) {
        self.cursor = self.clamp_point(point);
        if self.phase == SelectionPhase::Dragging {
            self.update_drag_rect();
        }
    }

    /// Ends the drag. A drag with zero area is a click, and a click is a
    /// cancel: the phase becomes `Settled` but [`Self::rect`] stays `None`.
    pub fn finish(&mut self) {
        if self.phase == SelectionPhase::Dragging {
            if self.rect.is_some_and(|r| r.is_empty()) {
                self.rect = None;
            }
            self.anchor = None;
            self.phase = SelectionPhase::Settled;
        }
    }

    /// Cancels the interaction: phase `Cancelled`, no rect, no anchor.
    pub fn cancel(&mut self) {
        self.phase = SelectionPhase::Cancelled;
        self.anchor = None;
        self.rect = None;
    }

    /// Moves a settled (or live) selection by whole pixels, clamped so the
    /// rect never leaves `bounds`. No-op when there is no selection.
    pub fn nudge(&mut self, dx: i32, dy: i32) {
        let Some(rect) = self.rect else { return };
        self.rect = Some(self.clamp_rect_position(rect, dx, dy));
    }

    /// Resizes the current selection by pulling `handle` by `(dx, dy)`,
    /// clamped to `bounds` and normalised so width/height never go
    /// negative. No-op when there is no selection or the handle is
    /// [`Handle::Body`] (use [`Self::nudge`] to move).
    pub fn resize(&mut self, handle: Handle, dx: i32, dy: i32) {
        let Some(rect) = self.rect else { return };
        if handle == Handle::Body {
            self.nudge(dx, dy);
            return;
        }
        let mut left = rect.left();
        let mut top = rect.top();
        let mut right = rect.right();
        let mut bottom = rect.bottom();
        // The moving edge clamps against the fixed edge and the bounds, so
        // the rect stays normalised (never inverted) and never leaves
        // `bounds`.
        match handle {
            Handle::TopLeft => {
                left = (left + dx).clamp(self.bounds.left(), right);
                top = (top + dy).clamp(self.bounds.top(), bottom);
            }
            Handle::Top => top = (top + dy).clamp(self.bounds.top(), bottom),
            Handle::TopRight => {
                right = (right + dx).clamp(left, self.bounds.right());
                top = (top + dy).clamp(self.bounds.top(), bottom);
            }
            Handle::Right => right = (right + dx).clamp(left, self.bounds.right()),
            Handle::BottomRight => {
                right = (right + dx).clamp(left, self.bounds.right());
                bottom = (bottom + dy).clamp(top, self.bounds.bottom());
            }
            Handle::Bottom => bottom = (bottom + dy).clamp(top, self.bounds.bottom()),
            Handle::BottomLeft => {
                left = (left + dx).clamp(self.bounds.left(), right);
                bottom = (bottom + dy).clamp(top, self.bounds.bottom());
            }
            Handle::Left => left = (left + dx).clamp(self.bounds.left(), right),
            Handle::Body => unreachable!("handled above"),
        }
        self.rect = Some(Rect::from_xywh(
            left,
            top,
            (right - left) as u32,
            (bottom - top) as u32,
        ));
    }

    /// Hit-tests the handles of the current selection. Returns the handle
    /// whose hot zone (a square of `grab_px` side centred on the handle)
    /// contains `point`; corners win over edges, and anything inside the
    /// rect but not on a handle is [`Handle::Body`]. `None` when the point
    /// is outside the selection or there is no selection.
    pub fn handle_at(&self, point: Point, grab_px: u32) -> Option<Handle> {
        let rect = self.rect?;
        if rect.is_empty() {
            return None;
        }
        let grab = grab_px as i32;
        let cx = rect.left() + rect.size.width as i32 / 2;
        let cy = rect.top() + rect.size.height as i32 / 2;
        let near = |a: i32, b: i32| (a - b).abs() <= grab;
        let on_left = near(point.x, rect.left());
        let on_right = near(point.x, rect.right());
        let on_top = near(point.y, rect.top());
        let on_bottom = near(point.y, rect.bottom());
        let on_cx = near(point.x, cx);
        let on_cy = near(point.y, cy);
        if on_left && on_top {
            return Some(Handle::TopLeft);
        }
        if on_right && on_top {
            return Some(Handle::TopRight);
        }
        if on_right && on_bottom {
            return Some(Handle::BottomRight);
        }
        if on_left && on_bottom {
            return Some(Handle::BottomLeft);
        }
        if on_cx && on_top {
            return Some(Handle::Top);
        }
        if on_cx && on_bottom {
            return Some(Handle::Bottom);
        }
        if on_left && on_cy {
            return Some(Handle::Left);
        }
        if on_right && on_cy {
            return Some(Handle::Right);
        }
        if rect.contains_point(point) {
            return Some(Handle::Body);
        }
        None
    }

    /// Snaps the selection to `candidate` when the cursor is inside it —
    /// used for window snapping: the caller offers the window rect under
    /// the cursor and the selection jumps to it (clamped to bounds).
    /// Returns true when the snap happened.
    pub fn snap_to(&mut self, candidate: &Rect) -> bool {
        if !candidate.contains_point(self.cursor) {
            return false;
        }
        if let Some(clamped) = candidate.intersection(&self.bounds) {
            self.rect = Some(clamped);
            if self.phase == SelectionPhase::Dragging {
                self.anchor = None;
                self.phase = SelectionPhase::Settled;
            }
            return true;
        }
        false
    }

    /// Recomputes the live drag rect from anchor and cursor, applying
    /// square lock and clamping into bounds.
    fn update_drag_rect(&mut self) {
        let Some(anchor) = self.anchor else { return };
        let cursor = self.cursor;
        let mut dx = cursor.x - anchor.x;
        let mut dy = cursor.y - anchor.y;
        if self.square_lock {
            // Square from the larger delta, keeping the drag's direction.
            // The side shrinks so the whole square still fits in bounds —
            // clamping the corners afterwards would break the squareness.
            let sx = dx.signum();
            let sy = dy.signum();
            let avail_x = if sx >= 0 {
                self.bounds.right() - anchor.x
            } else {
                anchor.x - self.bounds.left()
            };
            let avail_y = if sy >= 0 {
                self.bounds.bottom() - anchor.y
            } else {
                anchor.y - self.bounds.top()
            };
            let side = dx.abs().max(dy.abs()).min(avail_x).min(avail_y).max(0);
            dx = sx * side;
            dy = sy * side;
        }
        let mut left = anchor.x.min(anchor.x + dx);
        let mut top = anchor.y.min(anchor.y + dy);
        let mut right = anchor.x.max(anchor.x + dx);
        let mut bottom = anchor.y.max(anchor.y + dy);
        left = left.clamp(self.bounds.left(), self.bounds.right());
        top = top.clamp(self.bounds.top(), self.bounds.bottom());
        right = right.clamp(left, self.bounds.right());
        bottom = bottom.clamp(top, self.bounds.bottom());
        self.rect = Some(Rect::from_xywh(
            left,
            top,
            (right - left) as u32,
            (bottom - top) as u32,
        ));
    }

    /// A point pulled inside `bounds` (inclusive of the far edge, so a drag
    /// can reach the very last pixel).
    fn clamp_point(&self, point: Point) -> Point {
        Point {
            x: point.x.clamp(self.bounds.left(), self.bounds.right()),
            y: point.y.clamp(self.bounds.top(), self.bounds.bottom()),
        }
    }

    /// `rect` moved by `(dx, dy)`, clamped so the whole rect stays inside
    /// `bounds`.
    fn clamp_rect_position(&self, rect: Rect, dx: i32, dy: i32) -> Rect {
        let width = rect.size.width as i32;
        let height = rect.size.height as i32;
        let max_x = (self.bounds.right() - width).max(self.bounds.left());
        let max_y = (self.bounds.bottom() - height).max(self.bounds.top());
        Rect {
            origin: Point {
                x: (rect.left() + dx).clamp(self.bounds.left(), max_x),
                y: (rect.top() + dy).clamp(self.bounds.top(), max_y),
            },
            size: Size {
                width: rect.size.width,
                height: rect.size.height,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect::from_xywh(0, 0, 1920, 1080)
    }

    #[test]
    fn drag_bottom_right_then_finish_gives_rect() {
        let mut sel = SelectionState::new(bounds());
        sel.begin(Point { x: 100, y: 100 });
        sel.drag_to(Point { x: 500, y: 400 });
        sel.finish();
        assert_eq!(sel.phase(), SelectionPhase::Settled);
        assert_eq!(sel.rect(), Some(Rect::from_xywh(100, 100, 400, 300)));
    }

    #[test]
    fn drag_up_left_normalises() {
        let mut sel = SelectionState::new(bounds());
        sel.begin(Point { x: 500, y: 400 });
        sel.drag_to(Point { x: 100, y: 100 });
        sel.finish();
        assert_eq!(sel.rect(), Some(Rect::from_xywh(100, 100, 400, 300)));
    }

    #[test]
    fn drag_beyond_bounds_clamps_on_every_side() {
        let mut sel = SelectionState::new(bounds());
        sel.begin(Point { x: 10, y: 10 });
        sel.drag_to(Point { x: -500, y: -500 });
        assert_eq!(sel.rect(), Some(Rect::from_xywh(0, 0, 10, 10)));
        sel.drag_to(Point { x: 9999, y: 9999 });
        assert_eq!(sel.rect(), Some(Rect::from_xywh(10, 10, 1910, 1070)));
        sel.finish();
        assert_eq!(sel.rect(), Some(Rect::from_xywh(10, 10, 1910, 1070)));
    }

    #[test]
    fn click_without_movement_is_a_cancel_not_a_capture() {
        let mut sel = SelectionState::new(bounds());
        sel.begin(Point { x: 300, y: 300 });
        sel.finish();
        assert_eq!(sel.phase(), SelectionPhase::Settled);
        assert_eq!(sel.rect(), None);
    }

    #[test]
    fn square_lock_makes_width_equal_height() {
        let mut sel = SelectionState::new(bounds());
        sel.set_square_lock(true);
        sel.begin(Point { x: 100, y: 100 });
        sel.drag_to(Point { x: 460, y: 250 });
        let rect = sel.rect().unwrap();
        assert_eq!(rect.size.width, rect.size.height);
        assert_eq!(rect.size.width, 360);
        // Toggling mid-drag recomputes live.
        sel.set_square_lock(false);
        assert_eq!(sel.rect(), Some(Rect::from_xywh(100, 100, 360, 150)));
        // Square lock still clamps to bounds: the square shrinks to fit
        // rather than overflowing and breaking its squareness.
        sel.set_square_lock(true);
        sel.begin(Point { x: 1900, y: 1000 });
        sel.drag_to(Point { x: -10, y: -10 });
        let rect = sel.rect().unwrap();
        assert_eq!(rect.size.width, rect.size.height);
        assert_eq!(rect, Rect::from_xywh(900, 0, 1000, 1000));
        assert_eq!(rect.intersection(&bounds()), Some(rect));
    }

    #[test]
    fn nudge_moves_one_pixel_and_refuses_to_leave_bounds() {
        let mut sel = SelectionState::new(bounds());
        sel.begin(Point { x: 100, y: 100 });
        sel.drag_to(Point { x: 200, y: 200 });
        sel.finish();
        sel.nudge(1, 1);
        assert_eq!(sel.rect(), Some(Rect::from_xywh(101, 101, 100, 100)));
        sel.nudge(-1, -1);
        assert_eq!(sel.rect(), Some(Rect::from_xywh(100, 100, 100, 100)));
        // Slam it against the far corner: nudges past the edge do nothing.
        sel.nudge(10_000, 10_000);
        assert_eq!(sel.rect(), Some(Rect::from_xywh(1820, 980, 100, 100)));
        sel.nudge(1, 1);
        assert_eq!(sel.rect(), Some(Rect::from_xywh(1820, 980, 100, 100)));
        sel.nudge(-10_000, -10_000);
        assert_eq!(sel.rect(), Some(Rect::from_xywh(0, 0, 100, 100)));
    }

    #[test]
    fn handle_at_corners_body_and_outside() {
        let mut sel = SelectionState::new(bounds());
        sel.begin(Point { x: 100, y: 100 });
        sel.drag_to(Point { x: 300, y: 240 });
        sel.finish();
        assert_eq!(
            sel.handle_at(Point { x: 102, y: 99 }, 4),
            Some(Handle::TopLeft)
        );
        assert_eq!(
            sel.handle_at(Point { x: 200, y: 170 }, 4),
            Some(Handle::Body)
        );
        assert_eq!(
            sel.handle_at(Point { x: 200, y: 101 }, 4),
            Some(Handle::Top)
        );
        assert_eq!(
            sel.handle_at(Point { x: 299, y: 170 }, 4),
            Some(Handle::Right)
        );
        assert_eq!(sel.handle_at(Point { x: 900, y: 900 }, 4), None);
    }

    #[test]
    fn resize_right_changes_width_only_and_clamps() {
        let mut sel = SelectionState::new(bounds());
        sel.begin(Point { x: 100, y: 100 });
        sel.drag_to(Point { x: 300, y: 240 });
        sel.finish();
        sel.resize(Handle::Right, 50, 999);
        assert_eq!(sel.rect(), Some(Rect::from_xywh(100, 100, 250, 140)));
        sel.resize(Handle::Right, 10_000, 0);
        assert_eq!(sel.rect(), Some(Rect::from_xywh(100, 100, 1820, 140)));
        // Pulling the right edge past the left edge keeps the rect normalised.
        sel.resize(Handle::Right, -10_000, 0);
        let rect = sel.rect().unwrap();
        assert_eq!(rect.left(), 100);
        assert!(!rect.is_empty() || rect.size.width == 0);
        assert!(rect.right() >= rect.left());
    }

    #[test]
    fn cancel_leaves_cancelled_phase_and_no_rect() {
        let mut sel = SelectionState::new(bounds());
        sel.begin(Point { x: 100, y: 100 });
        sel.drag_to(Point { x: 300, y: 240 });
        sel.cancel();
        assert_eq!(sel.phase(), SelectionPhase::Cancelled);
        assert_eq!(sel.rect(), None);
    }
}
