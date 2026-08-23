//! Geometry primitives in logical (points) and physical (pixels) coordinates.

/// A 2D point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: i32,
    /// Vertical coordinate.
    pub y: i32,
}

/// A 2D size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Size {
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// An axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rect {
    /// Top-left corner.
    pub origin: Point,
    /// Extent of the rectangle.
    pub size: Size,
}

impl Rect {
    /// Creates a rectangle from an origin and a size.
    pub fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    /// Creates a rectangle from raw coordinates and dimensions.
    pub fn from_xywh(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }

    /// Left edge x-coordinate.
    pub fn left(&self) -> i32 {
        self.origin.x
    }

    /// Top edge y-coordinate.
    pub fn top(&self) -> i32 {
        self.origin.y
    }

    /// Right edge x-coordinate (exclusive).
    pub fn right(&self) -> i32 {
        self.origin.x + self.size.width as i32
    }

    /// Bottom edge y-coordinate (exclusive).
    pub fn bottom(&self) -> i32 {
        self.origin.y + self.size.height as i32
    }

    /// True when the rectangle has zero area.
    pub fn is_empty(&self) -> bool {
        self.size.width == 0 || self.size.height == 0
    }

    /// True when `point` lies inside the rectangle (edges inclusive on
    /// left/top, exclusive on right/bottom).
    pub fn contains_point(&self, point: Point) -> bool {
        point.x >= self.left()
            && point.x < self.right()
            && point.y >= self.top()
            && point.y < self.bottom()
    }

    /// The overlapping region of two rectangles, or `None` when they are
    /// disjoint (or the overlap is empty).
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let left = self.left().max(other.left());
        let top = self.top().max(other.top());
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= left || bottom <= top {
            return None;
        }
        Some(Rect::from_xywh(
            left,
            top,
            (right - left) as u32,
            (bottom - top) as u32,
        ))
    }

    /// The smallest rectangle containing both `self` and `other`.
    pub fn union(&self, other: &Rect) -> Rect {
        let left = self.left().min(other.left());
        let top = self.top().min(other.top());
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Rect::from_xywh(left, top, (right - left) as u32, (bottom - top) as u32)
    }

    /// The rectangle moved by the given offset.
    pub fn translated(&self, by: Point) -> Rect {
        Rect {
            origin: Point {
                x: self.origin.x + by.x,
                y: self.origin.y + by.y,
            },
            size: self.size,
        }
    }
}

/// A display scale factor (logical-to-physical ratio). Always strictly
/// positive: construction clamps to a sane minimum so a scale can never be
/// zero or negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale(f32);

impl Scale {
    /// Smallest scale factor ever allowed.
    const MIN: f32 = 0.01;

    /// Creates a scale factor, clamped to [`Scale::MIN`] so it can never be
    /// zero, negative, or NaN.
    pub fn new(factor: f32) -> Self {
        if factor.is_finite() && factor >= Self::MIN {
            Self(factor)
        } else {
            Self(Self::MIN)
        }
    }

    /// The raw multiplier.
    pub fn factor(&self) -> f32 {
        self.0
    }

    /// Converts a logical size to physical pixels.
    pub fn to_physical(&self, size: Size) -> Size {
        Size {
            width: (size.width as f32 * self.0).round() as u32,
            height: (size.height as f32 * self.0).round() as u32,
        }
    }

    /// Converts a physical size to logical points.
    pub fn to_logical(&self, size: Size) -> Size {
        Size {
            width: (size.width as f32 / self.0).round() as u32,
            height: (size.height as f32 / self.0).round() as u32,
        }
    }

    /// Converts a logical point to physical pixels.
    pub fn point_to_physical(&self, point: Point) -> Point {
        Point {
            x: (point.x as f32 * self.0).round() as i32,
            y: (point.y as f32 * self.0).round() as i32,
        }
    }

    /// Converts a logical rectangle to physical pixels.
    pub fn rect_to_physical(&self, rect: Rect) -> Rect {
        Rect {
            origin: self.point_to_physical(rect.origin),
            size: self.to_physical(rect.size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_intersection_overlap() {
        let a = Rect::from_xywh(0, 0, 100, 100);
        let b = Rect::from_xywh(50, 50, 100, 100);
        assert_eq!(a.intersection(&b), Some(Rect::from_xywh(50, 50, 50, 50)));
    }

    #[test]
    fn rect_intersection_disjoint() {
        let a = Rect::from_xywh(0, 0, 10, 10);
        let b = Rect::from_xywh(20, 20, 10, 10);
        assert_eq!(a.intersection(&b), None);
    }

    #[test]
    fn scale_physical_and_logical_roundtrip() {
        let scale = Scale::new(2.0);
        let logical = Size {
            width: 100,
            height: 50,
        };
        let physical = scale.to_physical(logical);
        assert_eq!(
            physical,
            Size {
                width: 200,
                height: 100
            }
        );
        assert_eq!(scale.to_logical(physical), logical);
    }

    #[test]
    fn scale_never_zero_or_negative() {
        assert!(Scale::new(0.0).factor() > 0.0);
        assert!(Scale::new(-3.0).factor() > 0.0);
        assert!(Scale::new(f32::NAN).factor() > 0.0);
    }

    #[test]
    fn rect_union_and_translate() {
        let a = Rect::from_xywh(0, 0, 10, 10);
        let b = Rect::from_xywh(20, 20, 10, 10);
        assert_eq!(a.union(&b), Rect::from_xywh(0, 0, 30, 30));
        assert_eq!(
            a.translated(Point { x: 5, y: -5 }),
            Rect::from_xywh(5, -5, 10, 10)
        );
    }

    #[test]
    fn rect_contains_point() {
        let r = Rect::from_xywh(10, 10, 10, 10);
        assert!(r.contains_point(Point { x: 10, y: 10 }));
        assert!(r.contains_point(Point { x: 19, y: 19 }));
        assert!(!r.contains_point(Point { x: 20, y: 20 }));
    }
}
