//! Overlay geometry: turns the current [`SelectionState`] into coloured
//! quads (selection border, eight handles, dimension readout). Pure
//! vertex construction — no GPU types in here, so it is unit-testable.

use crate::digits::{GLYPH_HEIGHT, GLYPH_WIDTH, glyph, has_glyph};
use foxshot_core::{Rect, SelectionState};

/// A coloured quad in surface pixels: x, y, width, height, RGBA.
pub(crate) type Quads = Vec<(i32, i32, i32, i32, [f32; 4])>;

/// Coloured-quad overlay shader, shared by the selector and the editor.
pub(crate) const WGSL: &str = r"
struct Viewport {
    size: vec2<f32>,
};
@group(0) @binding(0) var<uniform> viewport: Viewport;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let ndc = vec2<f32>(
        in.pos.x / viewport.size.x * 2.0 - 1.0,
        1.0 - in.pos.y / viewport.size.y * 2.0,
    );
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
";

/// The FoxShot action colour, #FF6A3D, as linear RGBA floats.
pub(crate) const ACTION: [f32; 4] = [1.0, 0x6A as f32 / 255.0, 0x3D as f32 / 255.0, 1.0];
/// Readout text colour.
pub(crate) const TEXT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// Readout background (dimmed black, alpha-blended).
pub(crate) const TEXT_BG: [f32; 4] = [0.0, 0.0, 0.0, 0.6];

/// One overlay vertex: position in surface pixels, RGBA colour.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct OverlayVertex {
    /// Position in surface pixels (origin top-left).
    pub pos: [f32; 2],
    /// RGBA colour.
    pub color: [f32; 4],
}

/// Width of the selection border, in pixels.
const BORDER: i32 = 1;
/// Side of a handle square, in pixels.
const HANDLE: i32 = 7;
/// Readout cell size in pixels (each bitmap cell becomes a square quad).
const CELL: i32 = 2;

/// Maximum overlay vertices ever emitted (bounds the GPU vertex buffer).
pub(crate) const MAX_VERTICES: usize = 4096;

/// Builds the overlay quad list for the current selection: a 1px border
/// in the action colour, eight handles and the live `WIDTH x HEIGHT`
/// readout. Returns at most [`MAX_VERTICES`] vertices (6 per quad).
pub(crate) fn build(selection: &SelectionState) -> Vec<OverlayVertex> {
    let mut quads: Quads = Vec::new();
    if let Some(rect) = selection.rect().filter(|r| !r.is_empty()) {
        push_border(&mut quads, rect);
        push_handles(&mut quads, rect);
        push_readout(&mut quads, rect, selection.bounds());
    }
    to_vertices(&quads)
}

/// Turns a quad list into two triangles (six vertices) per quad, capped at
/// [`MAX_VERTICES`] vertices.
pub(crate) fn to_vertices(quads: &Quads) -> Vec<OverlayVertex> {
    let mut vertices = Vec::with_capacity(quads.len() * 6);
    for &(x, y, w, h, color) in quads {
        push_quad(&mut vertices, x, y, w, h, color);
        if vertices.len() > MAX_VERTICES - 6 {
            break;
        }
    }
    vertices
}

/// Pushes the bitmap-glyph cells of `text`, top-left at (`x`, `y`), each
/// cell `cell` pixels square. Characters without a glyph get a small box
/// outline so nothing is dropped silently. Returns the drawn pixel width.
pub(crate) fn push_text(
    quads: &mut Quads,
    x: i32,
    y: i32,
    text: &str,
    cell: i32,
    color: [f32; 4],
) -> i32 {
    let advance = (GLYPH_WIDTH as i32 + 1) * cell;
    let mut pen = x;
    for ch in text.chars() {
        if has_glyph(ch) {
            let bitmap = glyph(ch);
            for (row, bits) in bitmap.iter().enumerate() {
                for col in 0..GLYPH_WIDTH as i32 {
                    if bits & (0b100 >> col) != 0 {
                        quads.push((pen + col * cell, y + row as i32 * cell, cell, cell, color));
                    }
                }
            }
        } else {
            // Fallback box outline for glyph-less characters.
            let (w, h) = (GLYPH_WIDTH as i32 * cell, GLYPH_HEIGHT as i32 * cell);
            quads.push((pen, y, w, cell, color));
            quads.push((pen, y + h - cell, w, cell, color));
            quads.push((pen, y + cell, cell, h - 2 * cell, color));
            quads.push((pen + w - cell, y + cell, cell, h - 2 * cell, color));
        }
        pen += advance;
    }
    pen - x - cell
}

/// The four 1px border edges of `rect` in the action colour.
fn push_border(quads: &mut Vec<(i32, i32, i32, i32, [f32; 4])>, rect: Rect) {
    let (l, t, r, b) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    quads.push((l, t, r - l, BORDER, ACTION)); // top
    quads.push((l, b - BORDER, r - l, BORDER, ACTION)); // bottom
    quads.push((l, t + BORDER, BORDER, b - t - 2 * BORDER, ACTION)); // left
    quads.push((r - BORDER, t + BORDER, BORDER, b - t - 2 * BORDER, ACTION)); // right
}

/// The eight handles: four corners plus four edge midpoints.
fn push_handles(quads: &mut Vec<(i32, i32, i32, i32, [f32; 4])>, rect: Rect) {
    let (l, t, r, b) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    let cx = l + (r - l) / 2;
    let cy = t + (b - t) / 2;
    for (x, y) in [(l, t), (cx, t), (r, t), (r, cy), (r, b), (cx, b), (l, b), (l, cy)] {
        quads.push((x - HANDLE / 2, y - HANDLE / 2, HANDLE, HANDLE, ACTION));
    }
}

/// The `WIDTH x HEIGHT` readout, above the selection when there is room,
/// otherwise just inside its top-left corner.
fn push_readout(quads: &mut Quads, rect: Rect, bounds: Rect) {
    let text = format!("{} x {}", rect.size.width, rect.size.height);
    let advance = (GLYPH_WIDTH as i32 + 1) * CELL;
    let text_w = text.len() as i32 * advance - CELL;
    let text_h = GLYPH_HEIGHT as i32 * CELL;
    let pad = 4;
    let mut x = rect.left();
    if x + text_w + 2 * pad > bounds.right() {
        x = (bounds.right() - text_w - 2 * pad).max(bounds.left());
    }
    let above = rect.top() - text_h - 2 * pad >= bounds.top();
    let y = if above { rect.top() - text_h - 2 * pad } else { rect.top() + pad };
    quads.push((x - pad, y - pad, text_w + 2 * pad, text_h + 2 * pad, TEXT_BG));
    push_text(quads, x, y, &text, CELL, TEXT);
}

/// Emits two triangles (six vertices) for one pixel-space quad.
fn push_quad(
    vertices: &mut Vec<OverlayVertex>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: [f32; 4],
) {
    let (x0, y0) = (x as f32, y as f32);
    let (x1, y1) = ((x + w) as f32, (y + h) as f32);
    let corners = [
        [x0, y0],
        [x1, y0],
        [x0, y1],
        [x0, y1],
        [x1, y0],
        [x1, y1],
    ];
    vertices.extend(corners.iter().map(|pos| OverlayVertex { pos: *pos, color }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxshot_core::Point;

    #[test]
    fn empty_selection_draws_nothing() {
        let selection = SelectionState::new(Rect::from_xywh(0, 0, 800, 600));
        assert!(build(&selection).is_empty());
    }

    #[test]
    fn settled_selection_draws_border_handles_and_readout() {
        let mut selection = SelectionState::new(Rect::from_xywh(0, 0, 800, 600));
        selection.begin(Point { x: 100, y: 100 });
        selection.drag_to(Point { x: 300, y: 250 });
        selection.finish();
        let vertices = build(&selection);
        // 4 border quads + 8 handles + 1 readout background + digit cells,
        // six vertices per quad, and always within the buffer cap.
        assert!(vertices.len() >= 13 * 6);
        assert_eq!(vertices.len() % 6, 0);
        assert!(vertices.len() <= MAX_VERTICES);
        // Everything drawn sits inside the surface.
        assert!(vertices.iter().all(|v| v.pos[0] >= 0.0
            && v.pos[0] <= 800.0
            && v.pos[1] >= 0.0
            && v.pos[1] <= 600.0));
        // The action colour is in there (border + handles).
        assert!(vertices.iter().any(|v| v.color == ACTION));
    }
}
