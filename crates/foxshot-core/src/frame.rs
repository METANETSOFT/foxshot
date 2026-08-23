//! A captured screen frame: raw RGBA8 pixels plus the metadata needed to
//! interpret them.

use crate::error::{Error, Result};
use crate::geometry::{Rect, Scale, Size};

/// Number of bytes per pixel (red, green, blue, alpha).
pub const BYTES_PER_PIXEL: usize = 4;

/// A captured frame of RGBA8 pixels, 4 bytes per pixel, row-major.
///
/// A frame is **immutable once captured**: annotation and every other
/// downstream stage render into their own buffers and never write back into
/// a `Frame`. Methods that derive new imagery, such as [`Frame::crop`],
/// always return a new `Frame` and leave the source byte-for-byte intact.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    size: Size,
    scale: Scale,
    pixels: Vec<u8>,
}

impl Frame {
    /// Creates a frame filled with a single RGBA colour.
    pub fn new_filled(size: Size, scale: Scale, colour: [u8; 4]) -> Frame {
        let mut pixels = vec![0u8; size.width as usize * size.height as usize * BYTES_PER_PIXEL];
        for chunk in pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
            chunk.copy_from_slice(&colour);
        }
        Frame { size, scale, pixels }
    }

    /// Creates a frame from an existing RGBA8 buffer, validating its length.
    pub fn from_rgba8(size: Size, scale: Scale, pixels: Vec<u8>) -> Result<Frame> {
        let expected = size.width as usize * size.height as usize * BYTES_PER_PIXEL;
        if pixels.len() != expected {
            return Err(Error::InvalidPixelBuffer { expected, got: pixels.len() });
        }
        Ok(Frame { size, scale, pixels })
    }

    /// Frame size in physical pixels.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Scale factor the frame was captured at.
    pub fn scale(&self) -> Scale {
        self.scale
    }

    /// Raw RGBA8 bytes, row-major, 4 bytes per pixel.
    pub fn bytes(&self) -> &[u8] {
        &self.pixels
    }

    /// Total number of pixels in the frame.
    pub fn pixel_count(&self) -> usize {
        self.size.width as usize * self.size.height as usize
    }

    /// Extracts a sub-rectangle as a **new** frame.
    ///
    /// The source frame is never modified — this is the non-destructive
    /// guarantee the annotation pipeline relies on. Returns
    /// [`Error::RectOutOfBounds`] when `rect` does not fit entirely inside
    /// the frame.
    pub fn crop(&self, rect: Rect) -> Result<Frame> {
        let bounds = Rect::from_xywh(0, 0, self.size.width, self.size.height);
        if rect.is_empty() || rect.intersection(&bounds) != Some(rect) {
            return Err(Error::RectOutOfBounds { requested: rect, bounds });
        }
        let width = self.size.width as usize;
        let row_bytes = rect.size.width as usize * BYTES_PER_PIXEL;
        let mut out = Vec::with_capacity(row_bytes * rect.size.height as usize);
        for row in rect.top()..rect.bottom() {
            let start = (row as usize * width + rect.left() as usize) * BYTES_PER_PIXEL;
            out.extend_from_slice(&self.pixels[start..start + row_bytes]);
        }
        Ok(Frame { size: rect.size, scale: self.scale, pixels: out })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient_frame(width: u32, height: u32) -> Frame {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[x as u8, y as u8, 0xAB, 0xFF]);
            }
        }
        Frame::from_rgba8(Size { width, height }, Scale::new(1.0), pixels).unwrap()
    }

    #[test]
    fn crop_returns_size_and_never_mutates_source() {
        let frame = gradient_frame(8, 8);
        let before = frame.bytes().to_vec();
        let cropped = frame.crop(Rect::from_xywh(2, 2, 4, 4)).unwrap();
        assert_eq!(cropped.size(), Size { width: 4, height: 4 });
        let mut expected = Vec::new();
        for y in 2..6usize {
            let start = (y * 8 + 2) * 4;
            expected.extend_from_slice(&before[start..start + 16]);
        }
        assert_eq!(cropped.bytes(), expected.as_slice());
        // Non-destructive guarantee: source bytes are byte-identical afterwards.
        assert_eq!(frame.bytes(), before.as_slice());
    }

    #[test]
    fn from_rgba8_rejects_wrong_length() {
        let err = Frame::from_rgba8(Size { width: 4, height: 4 }, Scale::new(1.0), vec![0u8; 10])
            .unwrap_err();
        assert_eq!(err, Error::InvalidPixelBuffer { expected: 64, got: 10 });
    }

    #[test]
    fn crop_out_of_frame_is_rect_out_of_bounds() {
        let frame = gradient_frame(8, 8);
        let err = frame.crop(Rect::from_xywh(6, 6, 8, 8)).unwrap_err();
        match err {
            Error::RectOutOfBounds { requested, bounds } => {
                assert_eq!(requested, Rect::from_xywh(6, 6, 8, 8));
                assert_eq!(bounds, Rect::from_xywh(0, 0, 8, 8));
            }
            other => panic!("expected RectOutOfBounds, got {other:?}"),
        }
    }

    #[test]
    fn new_filled_paints_every_pixel() {
        let frame = Frame::new_filled(Size { width: 3, height: 2 }, Scale::new(1.0), [1, 2, 3, 4]);
        assert_eq!(frame.pixel_count(), 6);
        assert!(frame.bytes().chunks_exact(4).all(|c| c == [1, 2, 3, 4]));
    }
}
