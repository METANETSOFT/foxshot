//! A tiny 3×5 bitmap glyph set for the dimension readout and editor text.
//!
//! Pulling in a font stack for one `WIDTH x HEIGHT` label would dwarf the
//! rest of this crate, so the readout draws its own glyphs. The set covers
//! the ten digits, a space, the lowercase alphabet (uppercase maps onto it)
//! and a handful of punctuation — everything the selector readout, the tool
//! rail and simple text marks need. Anything outside the set reports
//! [`has_glyph`] == false so callers can draw a fallback box instead of
//! silently dropping the character.

/// Glyph width in cells.
pub(crate) const GLYPH_WIDTH: u32 = 3;
/// Glyph height in cells.
pub(crate) const GLYPH_HEIGHT: u32 = 5;

/// True when `ch` has a real glyph (space counts: its glyph is blank).
pub(crate) fn has_glyph(ch: char) -> bool {
    ch == ' '
        || ch.is_ascii_digit()
        || ch.is_ascii_alphabetic()
        || matches!(
            ch,
            '.' | ','
                | '-'
                | '_'
                | ':'
                | ';'
                | '!'
                | '?'
                | '\''
                | '('
                | ')'
                | '/'
                | '+'
                | '='
                | '@'
                | '#'
                | '<'
                | '>'
                | '|'
        )
}

/// The 3×5 bitmap of `ch`, one byte per row, bit 2 the leftmost cell.
/// Uppercase letters reuse the lowercase shapes; unknown characters map to
/// blank (which is also the space glyph) — callers that need to know the
/// difference ask [`has_glyph`] first.
pub(crate) fn glyph(ch: char) -> [u8; GLYPH_HEIGHT as usize] {
    match ch.to_ascii_lowercase() {
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
        'a' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'b' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'c' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'd' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'e' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'f' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'g' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'h' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'i' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'j' => [0b011, 0b001, 0b001, 0b101, 0b010],
        'k' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'l' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'm' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'n' => [0b110, 0b101, 0b101, 0b101, 0b101],
        'o' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'p' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'q' => [0b010, 0b101, 0b101, 0b110, 0b011],
        'r' => [0b110, 0b101, 0b110, 0b101, 0b101],
        's' => [0b011, 0b100, 0b010, 0b001, 0b110],
        't' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'u' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'v' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'w' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'x' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        ',' => [0b000, 0b000, 0b000, 0b010, 0b100],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        '_' => [0b000, 0b000, 0b000, 0b000, 0b111],
        ':' => [0b000, 0b010, 0b000, 0b010, 0b000],
        ';' => [0b000, 0b010, 0b000, 0b010, 0b100],
        '!' => [0b010, 0b010, 0b010, 0b000, 0b010],
        '?' => [0b110, 0b001, 0b010, 0b000, 0b010],
        '\'' => [0b010, 0b010, 0b000, 0b000, 0b000],
        '(' => [0b001, 0b010, 0b010, 0b010, 0b001],
        ')' => [0b100, 0b010, 0b010, 0b010, 0b100],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        '=' => [0b000, 0b111, 0b000, 0b111, 0b000],
        '@' => [0b010, 0b101, 0b111, 0b100, 0b011],
        '#' => [0b101, 0b111, 0b101, 0b111, 0b101],
        '<' => [0b001, 0b010, 0b100, 0b010, 0b001],
        '>' => [0b100, 0b010, 0b001, 0b010, 0b100],
        '|' => [0b010, 0b010, 0b010, 0b010, 0b010],
        _ => [0, 0, 0, 0, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_have_ink_and_space_is_blank() {
        for ch in "0123456789".chars() {
            assert!(
                glyph(ch).iter().any(|row| *row != 0),
                "glyph '{ch}' is blank"
            );
        }
        assert_eq!(glyph(' '), [0; 5]);
        assert!(has_glyph(' '));
        assert_eq!(glyph('x').iter().filter(|row| **row != 0).count(), 5);
    }

    #[test]
    fn every_letter_has_a_glyph_and_uppercase_matches() {
        for ch in 'a'..='z' {
            assert!(has_glyph(ch), "'{ch}' reports no glyph");
            assert!(
                glyph(ch).iter().any(|row| *row != 0),
                "glyph '{ch}' is blank"
            );
            assert_eq!(glyph(ch), glyph(ch.to_ascii_uppercase()));
        }
    }

    #[test]
    fn unsupported_characters_are_reported() {
        for ch in ['€', '✓', '}', '~'] {
            assert!(!has_glyph(ch), "'{ch}' should report no glyph");
        }
    }
}
