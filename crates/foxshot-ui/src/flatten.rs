//! Flattening: rasterises an [`AnnotationDocument`]'s marks over a **copy**
//! of its frame, producing the flattened image for save, copy and the
//! editor's own on-screen composite. Pure CPU pixel work — no GPU, no I/O —
//! so it is unit-testable, and what the editor shows is byte-identical to
//! what save or copy produces.
//!
//! The source frame is never touched: [`flatten`] clones its bytes first and
//! every draw helper writes only into the clone.

use crate::digits::{GLYPH_HEIGHT, GLYPH_WIDTH, glyph, has_glyph};
use foxshot_core::annotation::{AnnotationDocument, Mark, MarkKind};
use foxshot_core::frame::Frame;
use foxshot_core::geometry::{Point, Rect, Size};

/// True when this slice draws `kind` for real. Everything else renders as
/// its bounding outline, so no mark is ever silently invisible — and the
/// editor's tool rail can flag the tool as not-yet-drawable.
pub fn is_drawable(kind: &MarkKind) -> bool {
    matches!(
        kind,
        MarkKind::Rectangle
            | MarkKind::Arrow { .. }
            | MarkKind::Text { .. }
            | MarkKind::StepNumber { .. }
            | MarkKind::Blur { .. }
    )
}

/// Rasterises every mark of `doc`, in insertion (z) order, over a copy of
/// the document's frame and returns the result as a **new** frame. The
/// document's own frame is byte-identical afterwards — that is the
/// non-destructive guarantee the annotation model exists for.
pub fn flatten(doc: &AnnotationDocument) -> Frame {
    let frame = doc.frame();
    let mut pixels = frame.bytes().to_vec();
    for mark in doc.marks() {
        draw_mark(&mut pixels, frame.size(), mark);
    }
    Frame::from_rgba8(frame.size(), frame.scale(), pixels)
        .expect("the composite has exactly the frame's dimensions")
}

/// Draws one mark into `pixels` (RGBA8, row-major, `size` pixels).
///
/// Rectangle, Arrow, Text, StepNumber and Blur draw for real; every other
/// kind draws its bounding outline in the mark's ink so it stays visible.
pub(crate) fn draw_mark(pixels: &mut [u8], size: Size, mark: &Mark) {
    let ink = mark.ink.colour;
    match &mark.kind {
        MarkKind::Rectangle => {
            stroke_rect(pixels, size, mark.bounds, mark.ink.width.max(1) as i32, ink);
        }
        MarkKind::Arrow { from, to } => {
            draw_arrow(pixels, size, *from, *to, mark.ink.width.max(1) as i32, ink);
        }
        MarkKind::Text {
            content,
            size: points,
        } => {
            draw_text(pixels, size, mark.bounds.origin, content, *points, ink);
        }
        MarkKind::StepNumber { index } => {
            let radius = (mark.bounds.size.width.max(mark.bounds.size.height) as i32 / 2).max(8);
            let centre = mark.bounds.origin;
            fill_circle(pixels, size, centre, radius, ink);
            // The badge number sits inside the circle, in the frame's
            // lightest readable contrast: opaque white.
            draw_number_centered(
                pixels,
                size,
                centre,
                *index,
                radius,
                [0xFF, 0xFF, 0xFF, 0xFF],
            );
        }
        MarkKind::Blur { radius } => {
            box_blur(pixels, size, mark.bounds, *radius as i32);
        }
        _ => {
            // Not-yet-drawable kinds render as their bounding outline.
            stroke_rect(pixels, size, mark.bounds, 1, ink);
        }
    }
}

/// Blends `colour` (alpha-over) into the pixel at (`x`, `y`), clipped.
fn blend_px(pixels: &mut [u8], size: Size, x: i32, y: i32, colour: [u8; 4]) {
    if x < 0 || y < 0 || x >= size.width as i32 || y >= size.height as i32 {
        return;
    }
    let at = (y as usize * size.width as usize + x as usize) * 4;
    let alpha = u32::from(colour[3]);
    if alpha == 255 {
        pixels[at..at + 4].copy_from_slice(&colour);
        return;
    }
    for channel in 0..3 {
        let dst = u32::from(pixels[at + channel]);
        let src = u32::from(colour[channel]);
        pixels[at + channel] = ((src * alpha + dst * (255 - alpha)) / 255) as u8;
    }
    pixels[at + 3] = pixels[at + 3].max(colour[3]);
}

/// Fills `rect` with `colour`, clipped to the image.
fn fill_rect(pixels: &mut [u8], size: Size, rect: Rect, colour: [u8; 4]) {
    let Some(rect) = rect.intersection(&Rect::from_xywh(0, 0, size.width, size.height)) else {
        return;
    };
    for y in rect.top()..rect.bottom() {
        for x in rect.left()..rect.right() {
            blend_px(pixels, size, x, y, colour);
        }
    }
}

/// Draws the outline of `rect` with a stroke of `width` pixels, growing
/// inward from the rect's edge so a 1px stroke lands exactly on the edge.
fn stroke_rect(pixels: &mut [u8], size: Size, rect: Rect, width: i32, colour: [u8; 4]) {
    if rect.is_empty() {
        return;
    }
    let w = width.max(1);
    let (l, t, r, b) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    fill_rect(
        pixels,
        size,
        Rect::from_xywh(l, t, (r - l) as u32, w as u32),
        colour,
    );
    fill_rect(
        pixels,
        size,
        Rect::from_xywh(l, b - w, (r - l) as u32, w as u32),
        colour,
    );
    let inner_h = (b - t - 2 * w).max(0) as u32;
    fill_rect(
        pixels,
        size,
        Rect::from_xywh(l, t + w, w as u32, inner_h),
        colour,
    );
    fill_rect(
        pixels,
        size,
        Rect::from_xywh(r - w, t + w, w as u32, inner_h),
        colour,
    );
}

/// Draws a thick line by stamping a disc of radius `width / 2` every half
/// pixel along the segment.
fn draw_line(pixels: &mut [u8], size: Size, from: Point, to: Point, width: i32, colour: [u8; 4]) {
    let radius = (width / 2).max(1);
    let length = ((to.x - from.x).pow(2) + (to.y - from.y).pow(2)) as f64;
    let steps = (length.sqrt() * 2.0).ceil() as i32 + 1;
    for step in 0..=steps {
        let t = step as f64 / steps as f64;
        let x = from.x as f64 + (to.x - from.x) as f64 * t;
        let y = from.y as f64 + (to.y - from.y) as f64 * t;
        fill_circle(
            pixels,
            size,
            Point {
                x: x.round() as i32,
                y: y.round() as i32,
            },
            radius,
            colour,
        );
    }
}

/// Fills the triangle through `a`, `b`, `c` by scanning its bounding box.
fn fill_triangle(pixels: &mut [u8], size: Size, a: Point, b: Point, c: Point, colour: [u8; 4]) {
    /// Twice the signed area of the triangle p1–p2–p3.
    fn area(p1: Point, p2: Point, p3: Point) -> i64 {
        (p2.x as i64 - p1.x as i64) * (p3.y as i64 - p1.y as i64)
            - (p3.x as i64 - p1.x as i64) * (p2.y as i64 - p1.y as i64)
    }
    let left = a.x.min(b.x).min(c.x);
    let right = a.x.max(b.x).max(c.x);
    let top = a.y.min(b.y).min(c.y);
    let bottom = a.y.max(b.y).max(c.y);
    let total = area(a, b, c);
    if total == 0 {
        return;
    }
    for y in top..=bottom {
        for x in left..=right {
            let p = Point { x, y };
            let (w1, w2, w3) = (area(b, c, p), area(c, a, p), area(a, b, p));
            if (w1 >= 0 && w2 >= 0 && w3 >= 0) || (w1 <= 0 && w2 <= 0 && w3 <= 0) {
                blend_px(pixels, size, x, y, colour);
            }
        }
    }
}

/// Draws an arrow: the shaft line plus a solid triangular head at `to`.
fn draw_arrow(pixels: &mut [u8], size: Size, from: Point, to: Point, width: i32, colour: [u8; 4]) {
    draw_line(pixels, size, from, to, width, colour);
    let dx = f64::from(to.x - from.x);
    let dy = f64::from(to.y - from.y);
    let length = dx.hypot(dy);
    if length < 1.0 {
        return;
    }
    let (ux, uy) = (dx / length, dy / length);
    let head_len = f64::from(width.max(2)) * 4.0;
    let head_half = f64::from(width.max(2)) * 2.5;
    // Base centre sits `head_len` back from the tip.
    let base = Point {
        x: (f64::from(to.x) - ux * head_len).round() as i32,
        y: (f64::from(to.y) - uy * head_len).round() as i32,
    };
    let (px, py) = (-uy, ux); // unit perpendicular
    let b1 = Point {
        x: (f64::from(base.x) + px * head_half).round() as i32,
        y: (f64::from(base.y) + py * head_half).round() as i32,
    };
    let b2 = Point {
        x: (f64::from(base.x) - px * head_half).round() as i32,
        y: (f64::from(base.y) - py * head_half).round() as i32,
    };
    fill_triangle(pixels, size, to, b1, b2, colour);
}

/// Fills a circle of `radius` around `centre`.
fn fill_circle(pixels: &mut [u8], size: Size, centre: Point, radius: i32, colour: [u8; 4]) {
    let r2 = radius * radius;
    for y in (centre.y - radius)..=(centre.y + radius) {
        for x in (centre.x - radius)..=(centre.x + radius) {
            let (dx, dy) = (x - centre.x, y - centre.y);
            if dx * dx + dy * dy <= r2 {
                blend_px(pixels, size, x, y, colour);
            }
        }
    }
}

/// The cell size (pixels per bitmap cell) for text of `points` points:
/// one cell per 5 points, floor of 1, so 14pt text draws 2px cells (a
/// 10px-tall glyph) and 24pt draws 4px cells.
fn text_cell(points: u16) -> i32 {
    (i32::from(points) / 5).max(1)
}

/// The pixel width one glyph advances the pen at this cell size.
fn text_advance(cell: i32) -> i32 {
    (GLYPH_WIDTH as i32 + 1) * cell
}

/// The pixel width of `content` rendered at `points` points.
pub(crate) fn text_width(content: &str, points: u16) -> i32 {
    let advance = text_advance(text_cell(points));
    (content.chars().count() as i32 * advance - text_cell(points)).max(0)
}

/// Draws `content` with the bitmap glyph set, top-left at `origin`.
/// Characters without a glyph draw a small box instead of being skipped.
fn draw_text(
    pixels: &mut [u8],
    size: Size,
    origin: Point,
    content: &str,
    points: u16,
    colour: [u8; 4],
) {
    let cell = text_cell(points);
    let advance = text_advance(cell);
    let mut pen = origin.x;
    for ch in content.chars() {
        if has_glyph(ch) {
            let bitmap = glyph(ch);
            for (row, bits) in bitmap.iter().enumerate() {
                for col in 0..GLYPH_WIDTH as i32 {
                    if bits & (0b100 >> col) != 0 {
                        fill_rect(
                            pixels,
                            size,
                            Rect::from_xywh(
                                pen + col * cell,
                                origin.y + row as i32 * cell,
                                cell as u32,
                                cell as u32,
                            ),
                            colour,
                        );
                    }
                }
            }
        } else {
            // No glyph: draw a small box so the character is not silently lost.
            stroke_rect(
                pixels,
                size,
                Rect::from_xywh(
                    pen,
                    origin.y,
                    (GLYPH_WIDTH as i32 * cell) as u32,
                    (GLYPH_HEIGHT as i32 * cell) as u32,
                ),
                cell,
                colour,
            );
        }
        pen += advance;
    }
}

/// Draws `number` centred on `centre`, sized to fit inside a badge of the
/// given radius.
fn draw_number_centered(
    pixels: &mut [u8],
    size: Size,
    centre: Point,
    number: u32,
    radius: i32,
    colour: [u8; 4],
) {
    let text = number.to_string();
    let cell = ((radius * 2) / (GLYPH_HEIGHT as i32 + 2)).max(1);
    let width = text_width(&text, (cell * 5) as u16);
    let height = GLYPH_HEIGHT as i32 * cell;
    let origin = Point {
        x: centre.x - width / 2,
        y: centre.y - height / 2,
    };
    draw_text(pixels, size, origin, &text, (cell * 5) as u16, colour);
}

/// A real two-pass separable box blur over `rect` (clamped to the image).
/// Horizontal then vertical, kernel 2·radius+1, samples clamped to the
/// region so the blur does not bleed outside pixels in.
fn box_blur(pixels: &mut [u8], size: Size, rect: Rect, radius: i32) {
    if radius <= 0 {
        return;
    }
    let Some(region) = rect.intersection(&Rect::from_xywh(0, 0, size.width, size.height)) else {
        return;
    };
    let (w, h) = (region.size.width as usize, region.size.height as usize);
    if w == 0 || h == 0 {
        return;
    }
    let stride = size.width as usize;
    let (l, t) = (region.left() as usize, region.top() as usize);
    let radius = radius as usize;
    let mut temp = vec![[0u8; 4]; w * h];
    // Horizontal pass: image -> temp.
    for row in 0..h {
        for col in 0..w {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            let lo = col.saturating_sub(radius);
            let hi = (col + radius).min(w - 1);
            for sample in lo..=hi {
                let at = ((t + row) * stride + l + sample) * 4;
                r += u32::from(pixels[at]);
                g += u32::from(pixels[at + 1]);
                b += u32::from(pixels[at + 2]);
                a += u32::from(pixels[at + 3]);
            }
            let n = (hi - lo + 1) as u32;
            temp[row * w + col] = [(r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8];
        }
    }
    // Vertical pass: temp -> image.
    for row in 0..h {
        for col in 0..w {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            let lo = row.saturating_sub(radius);
            let hi = (row + radius).min(h - 1);
            for sample in lo..=hi {
                let px = temp[sample * w + col];
                r += u32::from(px[0]);
                g += u32::from(px[1]);
                b += u32::from(px[2]);
                a += u32::from(px[3]);
            }
            let n = (hi - lo + 1) as u32;
            let at = ((t + row) * stride + l + col) * 4;
            pixels[at..at + 4].copy_from_slice(&[
                (r / n) as u8,
                (g / n) as u8,
                (b / n) as u8,
                (a / n) as u8,
            ]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxshot_core::Scale;
    use foxshot_core::annotation::Ink;

    const GREY: [u8; 4] = [128, 128, 128, 255];
    const ORANGE: [u8; 4] = [0xFF, 0x6A, 0x3D, 0xFF];

    fn grey_doc(width: u32, height: u32) -> AnnotationDocument {
        AnnotationDocument::new(Frame::new_filled(
            Size { width, height },
            Scale::new(1.0),
            GREY,
        ))
    }

    fn px(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * frame.size().width as usize + x as usize) * 4;
        frame.bytes()[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn flatten_leaves_the_document_frame_untouched() {
        let original = grey_doc(32, 24).frame().bytes().to_vec();
        let mut doc = grey_doc(32, 24);
        doc.add(
            MarkKind::Rectangle,
            Rect::from_xywh(4, 4, 20, 12),
            Ink::new(ORANGE, 2),
        );
        doc.add(
            MarkKind::Arrow {
                from: Point { x: 0, y: 0 },
                to: Point { x: 31, y: 23 },
            },
            Rect::from_xywh(0, 0, 32, 24),
            Ink::new(ORANGE, 2),
        );
        doc.add(
            MarkKind::Blur { radius: 4 },
            Rect::from_xywh(8, 8, 10, 10),
            Ink::default(),
        );
        let flattened = flatten(&doc);
        assert_ne!(flattened.bytes(), original.as_slice(), "marks must land");
        assert_eq!(flattened.size(), doc.frame().size());
        // The guarantee: the document's frame bytes did not change.
        assert_eq!(doc.frame().bytes(), original.as_slice());
    }

    #[test]
    fn rectangle_paints_its_edge_and_not_its_interior() {
        let mut doc = grey_doc(40, 30);
        doc.add(
            MarkKind::Rectangle,
            Rect::from_xywh(10, 10, 20, 10),
            Ink::new(ORANGE, 2),
        );
        let out = flatten(&doc);
        assert_eq!(px(&out, 10, 10), ORANGE, "top-left edge");
        assert_eq!(px(&out, 29, 19), ORANGE, "bottom-right edge");
        assert_eq!(px(&out, 20, 15), GREY, "interior stays grey");
        assert_eq!(px(&out, 5, 5), GREY, "outside stays grey");
    }

    #[test]
    fn arrow_paints_shaft_and_head() {
        let mut doc = grey_doc(60, 20);
        doc.add(
            MarkKind::Arrow {
                from: Point { x: 5, y: 10 },
                to: Point { x: 50, y: 10 },
            },
            Rect::from_xywh(5, 9, 45, 2),
            Ink::new(ORANGE, 2),
        );
        let out = flatten(&doc);
        assert_eq!(px(&out, 20, 10), ORANGE, "shaft");
        assert_eq!(px(&out, 49, 10), ORANGE, "head tip");
        assert_eq!(px(&out, 44, 7), ORANGE, "head wing (above shaft)");
        assert_eq!(px(&out, 5, 0), GREY, "far away stays grey");
    }

    #[test]
    fn step_number_paints_a_filled_circle_where_placed() {
        let mut doc = grey_doc(40, 40);
        doc.add(
            MarkKind::StepNumber { index: 0 },
            Rect::from_xywh(20, 20, 24, 24),
            Ink::new(ORANGE, 1),
        );
        let out = flatten(&doc);
        assert_eq!(
            px(&out, 20, 20),
            [0xFF, 0xFF, 0xFF, 0xFF],
            "digit 1 in the middle"
        );
        assert_eq!(px(&out, 27, 20), ORANGE, "circle right of the digit");
        assert_eq!(px(&out, 20, 32 + 1), GREY, "outside the circle stays grey");
    }

    #[test]
    fn text_paints_glyph_cells_and_boxes_unknowns() {
        let mut doc = grey_doc(60, 20);
        doc.add(
            MarkKind::Text {
                content: "a€".to_string(),
                size: 10,
            },
            Rect::from_xywh(2, 2, 20, 10),
            Ink::new(ORANGE, 1),
        );
        let out = flatten(&doc);
        // 'a' at cell 2: top row is 0b010 -> cell column 1 lights up.
        assert_eq!(px(&out, 2 + 2, 2), ORANGE, "glyph 'a' top cell");
        // '€' has no glyph: its box outline starts after one advance (8px).
        assert_eq!(px(&out, 2 + 8, 2), ORANGE, "fallback box top-left");
        assert_eq!(px(&out, 30, 15), GREY, "far away stays grey");
    }

    #[test]
    fn blur_softens_edges_instead_of_painting_a_veil() {
        // A sharp black/white edge: box blur must produce intermediate grey
        // values right at the edge — a translucent veil could not.
        let mut pixels = Vec::new();
        for _y in 0..20 {
            for x in 0..20 {
                let v = if x < 10 { 0 } else { 255 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let frame = Frame::from_rgba8(
            Size {
                width: 20,
                height: 20,
            },
            Scale::new(1.0),
            pixels,
        )
        .unwrap();
        let mut doc = AnnotationDocument::new(frame);
        doc.add(
            MarkKind::Blur { radius: 3 },
            Rect::from_xywh(0, 0, 20, 20),
            Ink::default(),
        );
        let out = flatten(&doc);
        let at_edge = px(&out, 9, 10)[0];
        assert!(at_edge > 10 && at_edge < 245, "edge softened to {at_edge}");
        assert_eq!(
            px(&out, 0, 10)[0],
            0,
            "deep black side is untouched (clamped kernel)"
        );
        assert_eq!(px(&out, 19, 10)[0], 255, "deep white side is untouched");
    }

    #[test]
    fn unsupported_kinds_render_as_their_outline() {
        let mut doc = grey_doc(30, 30);
        assert!(!is_drawable(&MarkKind::Ellipse));
        doc.add(
            MarkKind::Ellipse,
            Rect::from_xywh(5, 5, 10, 8),
            Ink::new(ORANGE, 3),
        );
        let out = flatten(&doc);
        assert_eq!(px(&out, 5, 5), ORANGE, "outline corner is painted");
        assert_eq!(px(&out, 10, 9), GREY, "interior is not");
    }
}
