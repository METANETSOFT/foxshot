//! The annotation model: marks that live above a captured frame.
//!
//! The rule this module exists to enforce: **a mark never writes into a
//! [`Frame`]**. The frame is captured once and is immutable; marks live in a
//! layer above it, carry their own geometry and ink, and can be added,
//! removed, undone and redone without the frame's bytes ever changing. The
//! editor renders the frame plus the marks; flattening, when it happens,
//! produces a *new* image and still leaves the source frame untouched.

use crate::frame::Frame;
use crate::geometry::{Point, Rect};
use std::collections::BTreeMap;

/// Unique identifier of a mark inside an [`AnnotationDocument`].
///
/// Ids are handed out monotonically and are never reused within a document,
/// so an undo history entry can refer to a mark without ambiguity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MarkId(pub u32);

/// The shape and payload of an annotation mark.
///
/// Each variant carries only the data it genuinely needs; everything else
/// (bounding box, colour, stroke width) lives on [`Mark`] itself. Variants
/// without payloads are pure area marks: their [`Mark::bounds`] fully
/// describes where they act.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkKind {
    /// A rectangle outline.
    Rectangle,
    /// An ellipse outline inscribed in the mark bounds.
    Ellipse,
    /// A straight line between two points.
    Line {
        /// Start point.
        from: Point,
        /// End point.
        to: Point,
    },
    /// A straight line with an arrowhead at `to`.
    Arrow {
        /// Start point (tail).
        from: Point,
        /// End point (arrowhead).
        to: Point,
    },
    /// A hand-drawn stroke following the given points.
    Freehand {
        /// Polyline vertices in drawing order.
        points: Vec<Point>,
    },
    /// A text label.
    Text {
        /// The text content.
        content: String,
        /// Font size in points.
        size: u16,
    },
    /// A speech balloon whose tail points at `tail`.
    SpeechBalloon {
        /// The text inside the balloon.
        content: String,
        /// The point the balloon's tail aims at.
        tail: Point,
    },
    /// A numbered step badge. Numbers are assigned by the document in
    /// insertion order (1, 2, 3, …) and renumber after a removal, so the
    /// `index` passed to [`AnnotationDocument::add`] is always overwritten.
    StepNumber {
        /// The 1-based step number shown in the badge.
        index: u32,
    },
    /// A translucent highlight wash over an area.
    Highlight,
    /// A gaussian blur over an area.
    Blur {
        /// Blur radius in pixels.
        radius: u8,
    },
    /// A mosaic/pixelation over an area.
    Pixelate {
        /// Edge length of one mosaic block in pixels.
        block: u8,
    },
    /// A loupe magnifying an area.
    Magnify {
        /// Magnification factor (1.0 = no change).
        factor: f32,
    },
    /// Dims everything except the marked area.
    Spotlight {
        /// Opacity of the dimming veil (0–255).
        dim: u8,
    },
    /// An eraser that removes whatever lies under an area.
    SmartEraser,
    /// A crop selection.
    Crop,
    /// An embedded image (e.g. a pasted sticker or logo).
    Image {
        /// Human-readable label of the embedded image.
        label: String,
    },
    /// A marker (highlighter pen) stroke following the given points.
    Marker {
        /// Polyline vertices in drawing order.
        points: Vec<Point>,
    },
}

impl MarkKind {
    /// A short human-readable label used in the editor's side list.
    pub fn label(&self) -> &'static str {
        match self {
            MarkKind::Rectangle => "Rectangle",
            MarkKind::Ellipse => "Ellipse",
            MarkKind::Line { .. } => "Line",
            MarkKind::Arrow { .. } => "Arrow",
            MarkKind::Freehand { .. } => "Freehand",
            MarkKind::Text { .. } => "Text",
            MarkKind::SpeechBalloon { .. } => "Speech balloon",
            MarkKind::StepNumber { .. } => "Step",
            MarkKind::Highlight => "Highlight",
            MarkKind::Blur { .. } => "Blur",
            MarkKind::Pixelate { .. } => "Pixelate",
            MarkKind::Magnify { .. } => "Magnify",
            MarkKind::Spotlight { .. } => "Spotlight",
            MarkKind::SmartEraser => "Smart eraser",
            MarkKind::Crop => "Crop",
            MarkKind::Image { .. } => "Image",
            MarkKind::Marker { .. } => "Marker",
        }
    }
}

/// The visual style of a mark: an RGBA colour plus a stroke width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ink {
    /// RGBA colour, one byte per channel.
    pub colour: [u8; 4],
    /// Stroke width in logical points.
    pub width: u16,
}

impl Ink {
    /// Creates an ink from a colour and a stroke width.
    pub fn new(colour: [u8; 4], width: u16) -> Self {
        Self { colour, width }
    }
}

impl Default for Ink {
    /// The FoxShot default ink: opaque red, 3 points wide.
    fn default() -> Self {
        Self { colour: [0xE0, 0x1B, 0x24, 0xFF], width: 3 }
    }
}

/// A single annotation: what it is, where it acts, and how it is painted.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    /// Unique id within the owning document.
    pub id: MarkId,
    /// Shape and payload.
    pub kind: MarkKind,
    /// Bounding box in logical points.
    pub bounds: Rect,
    /// Colour and stroke width.
    pub ink: Ink,
}

impl Mark {
    /// The mark's bounding box.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// True when `point` hits the mark, tested honestly per kind:
    ///
    /// - area kinds (rectangle, text, blur, …) test their bounding box;
    /// - `Line` and `Arrow` test the distance to their segment against the
    ///   stroke width;
    /// - `Freehand` and `Marker` test the distance to any of their segments;
    /// - `StepNumber` tests a circle around its origin.
    pub fn hit_test(&self, point: Point) -> bool {
        /// Half the stroke width, with a floor so hairline strokes stay
        /// clickable at all.
        fn stroke_tolerance(ink: Ink) -> f64 {
            (f64::from(ink.width) / 2.0).max(1.0)
        }

        match &self.kind {
            MarkKind::Line { from, to } | MarkKind::Arrow { from, to } => {
                distance_to_segment(point, *from, *to) <= stroke_tolerance(self.ink)
            }
            MarkKind::Freehand { points } | MarkKind::Marker { points } => {
                let tolerance = stroke_tolerance(self.ink);
                match points.as_slice() {
                    [] => false,
                    [single] => distance_to_segment(point, *single, *single) <= tolerance,
                    _ => points
                        .windows(2)
                        .any(|seg| distance_to_segment(point, seg[0], seg[1]) <= tolerance),
                }
            }
            MarkKind::StepNumber { .. } => {
                let centre = self.bounds.origin;
                let radius =
                    (self.bounds.size.width.max(self.bounds.size.height) as f64 / 2.0).max(1.0);
                let dx = f64::from(point.x - centre.x);
                let dy = f64::from(point.y - centre.y);
                dx.hypot(dy) <= radius
            }
            _ => self.bounds.contains_point(point),
        }
    }
}

/// Shortest distance from `p` to the segment `a`–`b`.
fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let (px, py) = (f64::from(p.x), f64::from(p.y));
    let (ax, ay) = (f64::from(a.x), f64::from(a.y));
    let (bx, by) = (f64::from(b.x), f64::from(b.y));
    let (dx, dy) = (bx - ax, by - ay);
    let length_sq = dx.mul_add(dx, dy * dy);
    if length_sq == 0.0 {
        return (px - ax).hypot(py - ay);
    }
    // Project p onto the line, clamped to the segment.
    let t = (((px - ax) * dx + (py - ay) * dy) / length_sq).clamp(0.0, 1.0);
    let closest_x = ax + t * dx;
    let closest_y = ay + t * dy;
    (px - closest_x).hypot(py - closest_y)
}

/// One readable row in the editor's side list, produced by a mark.
///
/// The note starts empty and is user-editable via
/// [`AnnotationDocument::set_note`]; everything else is derived from the
/// mark itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The mark this row belongs to.
    pub mark: MarkId,
    /// 1-based row number in insertion order.
    pub index: u32,
    /// Human-readable label of the mark kind.
    pub kind_label: &'static str,
    /// Representative location of the mark (its bounds origin).
    pub at: Point,
    /// Free-form user note; empty until the user types one.
    pub note: String,
}

/// The inverse of a user action, stored on the undo/redo stacks.
///
/// Every entry carries everything needed to re-derive its own inverse when
/// applied, so undo and redo are exact mirrors of each other.
#[derive(Debug, Clone)]
enum Edit {
    /// Insert this mark back at `position`, restoring its note.
    Insert {
        /// The mark to re-insert.
        mark: Mark,
        /// The note the mark had when it was removed, if any.
        note: Option<String>,
        /// Index into the mark list where the mark used to sit.
        position: usize,
    },
    /// Delete the mark with this id (and its note).
    Delete {
        /// Id of the mark to remove.
        id: MarkId,
    },
}

/// A captured frame plus the ordered layer of marks above it.
///
/// The document owns the frame and never mutates it — marks, undo history
/// and notes all live beside it. `document.frame().bytes()` is byte-identical
/// to the captured frame no matter what annotation history happened on top.
#[derive(Debug)]
pub struct AnnotationDocument {
    frame: Frame,
    marks: Vec<Mark>,
    notes: BTreeMap<MarkId, String>,
    undo_stack: Vec<Edit>,
    redo_stack: Vec<Edit>,
    next_id: u32,
    dirty: bool,
}

impl AnnotationDocument {
    /// Creates a document over a freshly captured frame.
    pub fn new(frame: Frame) -> Self {
        Self {
            frame,
            marks: Vec::new(),
            notes: BTreeMap::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            next_id: 1,
            dirty: false,
        }
    }

    /// The underlying frame. Always byte-identical to what was captured.
    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// All marks in insertion (z) order; the last one is topmost.
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    /// The side-list rows derived from the current marks, in insertion order.
    pub fn findings(&self) -> Vec<Finding> {
        self.marks
            .iter()
            .enumerate()
            .map(|(i, mark)| Finding {
                mark: mark.id,
                index: i as u32 + 1,
                kind_label: mark.kind.label(),
                at: mark.bounds.origin,
                note: self.notes.get(&mark.id).cloned().unwrap_or_default(),
            })
            .collect()
    }

    /// Adds a mark and returns its id.
    ///
    /// [`MarkKind::StepNumber`] marks ignore the index they were constructed
    /// with and are numbered by the document (1, 2, 3, … in insertion order).
    pub fn add(&mut self, mut kind: MarkKind, bounds: Rect, ink: Ink) -> MarkId {
        if let MarkKind::StepNumber { index } = &mut kind {
            *index = self.next_step_index();
        }
        let id = MarkId(self.next_id);
        self.next_id += 1;
        let mark = Mark { id, kind, bounds, ink };
        self.marks.push(mark);
        self.undo_stack.push(Edit::Delete { id });
        self.redo_stack.clear();
        self.dirty = true;
        id
    }

    /// Removes a mark. Returns false when no mark has that id.
    pub fn remove(&mut self, id: MarkId) -> bool {
        let Some(position) = self.marks.iter().position(|m| m.id == id) else {
            return false;
        };
        let mark = self.marks.remove(position);
        let note = self.notes.remove(&id);
        self.undo_stack.push(Edit::Insert { mark, note, position });
        self.redo_stack.clear();
        self.renumber_steps();
        self.dirty = true;
        true
    }

    /// The id of the topmost mark hit by `point`, if any.
    ///
    /// Marks added later sit higher in the z-order, so the list is scanned
    /// from the end.
    pub fn mark_at(&self, point: Point) -> Option<MarkId> {
        self.marks.iter().rev().find(|m| m.hit_test(point)).map(|m| m.id)
    }

    /// Sets (or replaces) the user note attached to a mark. Returns false
    /// when no mark has that id.
    pub fn set_note(&mut self, id: MarkId, note: String) -> bool {
        if !self.marks.iter().any(|m| m.id == id) {
            return false;
        }
        self.notes.insert(id, note);
        self.dirty = true;
        true
    }

    /// Undoes the most recent add or remove. Returns false when there is
    /// nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo_stack.pop() else {
            return false;
        };
        let inverse = self.apply(edit);
        self.redo_stack.push(inverse);
        self.dirty = true;
        true
    }

    /// Redoes the most recently undone add or remove. Returns false when
    /// there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo_stack.pop() else {
            return false;
        };
        let inverse = self.apply(edit);
        self.undo_stack.push(inverse);
        self.dirty = true;
        true
    }

    /// True when the document changed since it was created (or last saved).
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The number the next [`MarkKind::StepNumber`] mark will receive.
    pub fn next_step_index(&self) -> u32 {
        self.marks.iter().filter(|m| matches!(m.kind, MarkKind::StepNumber { .. })).count() as u32
            + 1
    }

    /// Applies an edit and returns its inverse, for the opposite stack.
    fn apply(&mut self, edit: Edit) -> Edit {
        let inverse = match edit {
            Edit::Insert { mark, note, position } => {
                let id = mark.id;
                let position = position.min(self.marks.len());
                self.marks.insert(position, mark);
                if let Some(note) = note {
                    self.notes.insert(id, note);
                }
                Edit::Delete { id }
            }
            Edit::Delete { id } => {
                // History entries are produced in strict alternation with the
                // opposite stack, so the mark is always present here.
                let position = self
                    .marks
                    .iter()
                    .position(|m| m.id == id)
                    .expect("history entries only reference existing marks");
                let mark = self.marks.remove(position);
                let note = self.notes.remove(&id);
                Edit::Insert { mark, note, position }
            }
        };
        self.renumber_steps();
        inverse
    }

    /// Renumbers all step badges to 1, 2, 3, … in insertion order.
    fn renumber_steps(&mut self) {
        let mut next = 1;
        for mark in &mut self.marks {
            if let MarkKind::StepNumber { index } = &mut mark.kind {
                *index = next;
                next += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Size;
    use crate::Scale;

    fn test_frame() -> Frame {
        Frame::new_filled(Size { width: 8, height: 8 }, Scale::new(1.0), [10, 20, 30, 255])
    }

    fn rect(x: i32, y: i32) -> Rect {
        Rect::from_xywh(x, y, 10, 10)
    }

    #[test]
    fn adding_three_marks_produces_three_findings_in_order() {
        let mut doc = AnnotationDocument::new(test_frame());
        doc.add(MarkKind::Rectangle, rect(0, 0), Ink::default());
        doc.add(MarkKind::Highlight, rect(20, 20), Ink::default());
        doc.add(MarkKind::Text { content: "hi".into(), size: 14 }, rect(40, 40), Ink::default());

        let findings = doc.findings();
        assert_eq!(findings.len(), 3);
        assert_eq!(findings[0].kind_label, "Rectangle");
        assert_eq!(findings[1].kind_label, "Highlight");
        assert_eq!(findings[2].kind_label, "Text");
        assert_eq!(findings[0].index, 1);
        assert_eq!(findings[2].index, 3);
        assert!(findings.iter().all(|f| f.note.is_empty()));
    }

    #[test]
    fn step_numbers_renumber_after_middle_removal() {
        let mut doc = AnnotationDocument::new(test_frame());
        doc.add(MarkKind::StepNumber { index: 0 }, rect(0, 0), Ink::default());
        let middle = doc.add(MarkKind::StepNumber { index: 0 }, rect(20, 0), Ink::default());
        doc.add(MarkKind::StepNumber { index: 0 }, rect(40, 0), Ink::default());

        let indices: Vec<u32> = doc
            .marks()
            .iter()
            .map(|m| match m.kind {
                MarkKind::StepNumber { index } => index,
                _ => panic!("only step marks were added"),
            })
            .collect();
        assert_eq!(indices, [1, 2, 3]);

        assert!(doc.remove(middle));
        let indices: Vec<u32> = doc
            .marks()
            .iter()
            .map(|m| match m.kind {
                MarkKind::StepNumber { index } => index,
                _ => panic!("only step marks exist"),
            })
            .collect();
        assert_eq!(indices, [1, 2]);
        assert_eq!(doc.next_step_index(), 3);
    }

    #[test]
    fn add_undo_removes_mark_redo_restores_it() {
        let mut doc = AnnotationDocument::new(test_frame());
        let id = doc.add(MarkKind::Ellipse, rect(1, 1), Ink::default());
        assert_eq!(doc.marks().len(), 1);

        assert!(doc.undo());
        assert!(doc.marks().is_empty());
        assert_eq!(doc.findings().len(), 0);

        assert!(doc.redo());
        assert_eq!(doc.marks().len(), 1);
        assert_eq!(doc.marks()[0].id, id);
        assert_eq!(doc.marks()[0].kind, MarkKind::Ellipse);

        // Stacks behave at their boundaries.
        let mut fresh = AnnotationDocument::new(test_frame());
        assert!(!fresh.undo());
        assert!(!fresh.redo());
    }

    #[test]
    fn frame_bytes_are_never_touched() {
        let frame = test_frame();
        let original = frame.bytes().to_vec();
        let mut doc = AnnotationDocument::new(frame);

        let a = doc.add(MarkKind::Rectangle, rect(0, 0), Ink::default());
        doc.add(MarkKind::Arrow { from: Point { x: 0, y: 0 }, to: Point { x: 7, y: 7 } },
            rect(0, 0), Ink::default());
        doc.set_note(a, "look here".into());
        assert!(doc.remove(a));
        assert!(doc.undo());
        assert!(doc.undo());
        assert!(doc.redo());

        // The non-destructive guarantee: after all that, the frame is intact.
        assert_eq!(doc.frame().bytes(), original.as_slice());
    }

    #[test]
    fn mark_at_returns_topmost_of_overlapping_marks() {
        let mut doc = AnnotationDocument::new(test_frame());
        let bottom = doc.add(MarkKind::Rectangle, rect(0, 0), Ink::default());
        let top = doc.add(MarkKind::Highlight, rect(0, 0), Ink::default());

        let hit = Point { x: 5, y: 5 };
        assert!(doc.marks().iter().all(|m| m.hit_test(hit)));
        assert_eq!(doc.mark_at(hit), Some(top));
        assert_ne!(doc.mark_at(hit), Some(bottom));
        assert_eq!(doc.mark_at(Point { x: 100, y: 100 }), None);
    }

    #[test]
    fn arrow_hit_test_near_and_far() {
        let mut doc = AnnotationDocument::new(test_frame());
        let ink = Ink::new([0, 0, 0, 255], 10);
        let id = doc.add(
            MarkKind::Arrow { from: Point { x: 0, y: 0 }, to: Point { x: 100, y: 0 } },
            Rect::from_xywh(0, 0, 100, 1),
            ink,
        );
        let mark = doc.marks().iter().find(|m| m.id == id).unwrap();

        assert!(mark.hit_test(Point { x: 50, y: 3 }), "3px from a 10px-wide stroke hits");
        assert!(mark.hit_test(Point { x: 0, y: 0 }), "the start point hits");
        assert!(!mark.hit_test(Point { x: 50, y: 50 }), "50px away misses");
        assert!(!mark.hit_test(Point { x: 150, y: 0 }), "past the end of the segment misses");
    }
}
