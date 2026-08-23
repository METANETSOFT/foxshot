//! A tiny 3×5 bitmap glyph set for the dimension readout.
//!
//! Pulling in a font stack for one `WIDTH x HEIGHT` label would dwarf the
//! rest of this crate, so the readout draws its own glyphs: the ten
//! digits, a space and a lowercase `x` are all it ever needs.

/// Glyph width in cells.
pub(crate) const GLYPH_WIDTH: u32 = 3;
/// Glyph height in cells.
pub(crate) const GLYPH_HEIGHT: u32 = 5;

/// The 3×5 bitmap of `ch`, one byte per row, bit 2 the leftmost cell.
/// Unknown characters map to blank (which is also the space glyph).
pub(crate) fn glyph(ch: char) -> [u8; GLYPH_HEIGHT as usize] {
    match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b011, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'x' => [0b000, 0b101, 0b010, 0b101, 0b000],
        _ => [0, 0, 0, 0, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_have_ink_and_space_is_blank() {
        for ch in "0123456789".chars() {
            assert!(glyph(ch).iter().any(|row| *row != 0), "glyph '{ch}' is blank");
        }
        assert_eq!(glyph(' '), [0; 5]);
        assert_eq!(glyph('x').iter().filter(|row| **row != 0).count(), 3);
    }
}
