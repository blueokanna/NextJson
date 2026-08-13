//! Pure lexical helpers shared by every byte-source decoder.
//!
//! The in-memory byte decoder (`de::BytesReader`) and the incremental stream
//! decoder (`stream::StreamDecoder`) must apply identical byte-level rules:
//! line / column computation for diagnostics, hex-digit decoding for `\u`
//! escapes, and JSON simple-escape decoding. These functions are pure (no
//! decoder state), so they live here exactly once instead of drifting apart
//! in the two decoders. Format codecs with their own lexers (JSON5, TOML,
//! urlform) reuse the hex-digit rule too.

/// Compute the 1-based line / column of a byte offset.
pub(crate) fn line_col(input: &[u8], pos: usize) -> (u32, u32) {
    let pos = pos.min(input.len());
    let mut line = 1u32;
    let mut col = 1u32;
    for &b in &input[..pos] {
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Decode a single hex digit byte (`0-9`, `a-f`, `A-F`).
#[inline]
pub(crate) fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse exactly four hex digits starting at `start` (for `\uXXXX`).
pub(crate) fn parse_hex4(input: &[u8], start: usize) -> Option<u16> {
    if start + 4 > input.len() {
        return None;
    }
    let mut v: u16 = 0;
    for &b in &input[start..start + 4] {
        v = v * 16 + hex_digit(b)? as u16;
    }
    Some(v)
}

/// Decode a JSON simple escape character (`"`, `\`, `/`, `b`, `f`, `n`,
/// `r`, `t`).
///
/// Returns `None` for `u` and for anything else: `\u` needs surrounding
/// bytes (the caller reads the four hex digits itself), and every other
/// backslash is an invalid escape.
#[inline]
pub(crate) fn simple_escape(esc: u8) -> Option<char> {
    match esc {
        b'"' => Some('"'),
        b'\\' => Some('\\'),
        b'/' => Some('/'),
        b'b' => Some('\u{8}'),
        b'f' => Some('\u{c}'),
        b'n' => Some('\n'),
        b'r' => Some('\r'),
        b't' => Some('\t'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_is_one_based_and_clamped() {
        assert_eq!(line_col(b"", 0), (1, 1));
        assert_eq!(line_col(b"abc", 0), (1, 1));
        assert_eq!(line_col(b"abc", 3), (1, 4));
        // Position past the end clamps to the input length.
        assert_eq!(line_col(b"abc", 99), (1, 4));
        assert_eq!(line_col(b"a\nbc", 3), (2, 2));
        assert_eq!(line_col(b"a\n\nbc", 4), (3, 2));
    }

    #[test]
    fn hex_digit_covers_all_valid_inputs() {
        assert_eq!(hex_digit(b'0'), Some(0));
        assert_eq!(hex_digit(b'9'), Some(9));
        assert_eq!(hex_digit(b'a'), Some(10));
        assert_eq!(hex_digit(b'f'), Some(15));
        assert_eq!(hex_digit(b'A'), Some(10));
        assert_eq!(hex_digit(b'F'), Some(15));
        for b in [b'g', b'G', b' ', b'-', 0x00, 0x7F] {
            assert_eq!(hex_digit(b), None, "byte {b:#x}");
        }
    }

    #[test]
    fn parse_hex4_round_trips() {
        assert_eq!(parse_hex4(b"0000", 0), Some(0));
        assert_eq!(parse_hex4(b"abcd", 0), Some(0xABCD));
        assert_eq!(parse_hex4(b"10FF", 0), Some(0x10FF));
        assert_eq!(parse_hex4(b"12", 0), None);
        assert_eq!(parse_hex4(b"12G4", 0), None);
        // The cursor must point at the first digit.
        assert_eq!(parse_hex4(b"xx1a2b", 2), Some(0x1A2B));
    }

    #[test]
    fn simple_escape_maps_the_json_shorts() {
        let mut pairs = [
            (b'"', '"'),
            (b'\\', '\\'),
            (b'/', '/'),
            (b'b', '\u{8}'),
            (b'f', '\u{c}'),
            (b'n', '\n'),
            (b'r', '\r'),
            (b't', '\t'),
        ];
        for (byte, expected) in pairs.iter_mut() {
            assert_eq!(simple_escape(*byte), Some(*expected));
        }
        // `\u` and anything else are not simple escapes.
        assert_eq!(simple_escape(b'u'), None);
        assert_eq!(simple_escape(b'x'), None);
        assert_eq!(simple_escape(0x01), None);
    }
}
