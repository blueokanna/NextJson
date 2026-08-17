//! SIMD / register-width accelerated byte scanning for the JSON hot paths.
//!
//! Two predicates are provided:
//!
//! - [`find_escape`] — the first byte that must be escaped in JSON text
//!   output: control bytes `< 0x20`, `"`, `\`, and (optionally) any byte
//!   `>= 0x80`.
//! - [`find_string_special`] — the first byte that terminates a JSON string
//!   body or is forbidden there: `"`, `\`, or a control byte `< 0x20`.
//! - [`skip_whitespace`] — advance past a run of JSON whitespace.
//!
//! # Acceleration strategy
//!
//! - **Portable fallback (always compiled):** SWAR (SIMD-within-a-register)
//!   on `u64` (32-bit targets) or `u128` (64-bit targets). This is plain
//!   arithmetic on integer registers — no `unsafe` — and is correct on every
//!   platform. The `u128` form processes 16 bytes per iteration and is used
//!   wherever the target can hold 128-bit integers natively.
//! - **`simd` feature (opt-in):** on `x86_64` the scan uses SSE2 (present on
//!   every x86-64 CPU) and, when the `std` feature is enabled, AVX2 after a
//!   runtime CPUID check (`is_x86_feature_detected!`). On `aarch64` it uses
//!   NEON (present on every AArch64 CPU). Every vector load is guarded by a
//!   length check, so the unsafe code cannot read out of bounds.
//!
//! The default build (`simd` off) contains no `unsafe` code; the crate-level
//! `#![deny(unsafe_code)]` stays in force for this module too (the allow
//! below is conditional on the `simd` feature). The unsafe SIMD
//! implementations live only under `cfg(feature = "simd")`.
#![cfg_attr(feature = "simd", allow(unsafe_code))]

/// Index of the first byte that must be escaped in JSON text output, or
/// `None` when the slice contains no such byte.
///
/// `escape_non_ascii` additionally treats every byte `>= 0x80` as requiring
/// an escape.
#[inline]
pub(crate) fn find_escape(bytes: &[u8], escape_non_ascii: bool) -> Option<usize> {
    imp::scan(bytes, escape_non_ascii)
}

/// Index of the first byte that is a JSON string terminator (`"`), escape
/// marker (`\`), or forbidden control byte (`< 0x20`), or `None` when the
/// slice contains no such byte.
#[inline]
pub(crate) fn find_string_special(bytes: &[u8]) -> Option<usize> {
    imp::scan(bytes, false)
}

/// Advance `pos` past a run of JSON whitespace (` `, `\t`, `\n`, `\r`).
///
/// Never advances past `input.len()`.
#[inline]
pub(crate) fn skip_whitespace(input: &[u8], pos: usize) -> usize {
    imp::skip_ws(input, pos)
}

// ---------------------------------------------------------------------------
// Predicate
// ---------------------------------------------------------------------------

/// Whether `byte` must be escaped in JSON text output.
#[inline]
fn escape_byte(byte: u8, check_non_ascii: bool) -> bool {
    byte < 0x20 || byte == b'"' || byte == b'\\' || (check_non_ascii && byte >= 0x80)
}

/// Whether `byte` is a JSON string terminator / escape marker / control byte.
#[cfg(test)]
#[inline]
fn string_special_byte(byte: u8) -> bool {
    byte < 0x20 || byte == b'"' || byte == b'\\'
}

/// Reference scalar scan. Correctness of every accelerated path is tested
/// against this function.
#[inline]
fn scan_scalar(bytes: &[u8], check_non_ascii: bool) -> Option<usize> {
    bytes.iter().position(|&b| escape_byte(b, check_non_ascii))
}

/// Reference scalar whitespace run-skip.
fn skip_ws_scalar(input: &[u8], mut pos: usize) -> usize {
    while pos < input.len() && matches!(input[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

// ---------------------------------------------------------------------------
// Portable SWAR (always available; the `simd` implementations fall back to
// this module's scalar tail, and the scan entry points below dispatch here
// when no SIMD implementation exists for the target).
// ---------------------------------------------------------------------------

#[cfg(not(any(
    all(feature = "simd", target_arch = "x86_64"),
    all(feature = "simd", target_arch = "aarch64")
)))]
mod portable {
    /// 8-byte SWAR chunk check (used on 32-bit targets).
    #[cfg(not(target_pointer_width = "64"))]
    #[inline]
    fn chunk_has_escape(chunk: u64, check_non_ascii: bool) -> bool {
        // Every mask is written as an inline literal (a local `const`
        // produced wrong results under some rustc versions in the 128-bit
        // variants; keep all variants uniform).
        if (chunk.wrapping_sub(0x2020_2020_2020_2020)) & !chunk & 0x8080_8080_8080_8080 != 0 {
            return true;
        }
        let quote = chunk ^ 0x2222_2222_2222_2222;
        if (quote.wrapping_sub(0x0101_0101_0101_0101)) & !quote & 0x8080_8080_8080_8080 != 0 {
            return true;
        }
        let backslash = chunk ^ 0x5C5C_5C5C_5C5C_5C5C;
        if (backslash.wrapping_sub(0x0101_0101_0101_0101)) & !backslash & 0x8080_8080_8080_8080 != 0
        {
            return true;
        }
        check_non_ascii && (chunk & 0x8080_8080_8080_8080) != 0
    }

    /// 16-byte SWAR chunk check (used on 64-bit targets).
    #[cfg(target_pointer_width = "64")]
    #[inline]
    fn chunk_has_escape(chunk: u128, check_non_ascii: bool) -> bool {
        // Bytes < 0x20: `(x - 0x2020..) & !x & 0x8080..` is the "hasless"
        // test; the high-bit mask is written inline to keep every 128-bit
        // constant a plain literal (a local `const` here historically
        // produced a wrong result under some rustc versions for the borrow
        // chain of `wrapping_sub` at the top byte).
        if (chunk.wrapping_sub(0x2020_2020_2020_2020_2020_2020_2020_2020))
            & !chunk
            & 0x8080_8080_8080_8080_8080_8080_8080_8080
            != 0
        {
            return true;
        }
        // Any byte == 0x22 (`"`).
        let quote = chunk ^ 0x2222_2222_2222_2222_2222_2222_2222_2222;
        if (quote.wrapping_sub(0x0101_0101_0101_0101_0101_0101_0101_0101))
            & !quote
            & 0x8080_8080_8080_8080_8080_8080_8080_8080
            != 0
        {
            return true;
        }
        // Any byte == 0x5C (`\`).
        let backslash = chunk ^ 0x5C5C_5C5C_5C5C_5C5C_5C5C_5C5C_5C5C_5C5C;
        if (backslash.wrapping_sub(0x0101_0101_0101_0101_0101_0101_0101_0101))
            & !backslash
            & 0x8080_8080_8080_8080_8080_8080_8080_8080
            != 0
        {
            return true;
        }
        // Any byte >= 0x80 (only when non-ASCII must be escaped).
        check_non_ascii && (chunk & 0x8080_8080_8080_8080_8080_8080_8080_8080) != 0
    }

    /// SWAR scan: finds the first escape byte. On a chunk hit the exact
    /// position is resolved by the scalar reference scan (correctness by
    /// construction; the chunk loop only prunes whole chunks).
    #[cfg(target_pointer_width = "64")]
    #[inline]
    pub(super) fn scan(bytes: &[u8], check_non_ascii: bool) -> Option<usize> {
        let len = bytes.len();
        let mut i = 0;
        while i + 16 <= len {
            let chunk = u128::from_le_bytes(bytes[i..i + 16].try_into().unwrap());
            if chunk_has_escape(chunk, check_non_ascii) {
                return super::scan_scalar(&bytes[i..], check_non_ascii).map(|off| i + off);
            }
            i += 16;
        }
        super::scan_scalar(&bytes[i..], check_non_ascii).map(|off| i + off)
    }

    /// 32-bit SWAR scan (8-byte chunks).
    #[cfg(not(target_pointer_width = "64"))]
    #[inline]
    pub(super) fn scan(bytes: &[u8], check_non_ascii: bool) -> Option<usize> {
        let len = bytes.len();
        let mut i = 0;
        while i + 8 <= len {
            let chunk = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
            if chunk_has_escape(chunk, check_non_ascii) {
                return super::scan_scalar(&bytes[i..], check_non_ascii).map(|off| i + off);
            }
            i += 8;
        }
        super::scan_scalar(&bytes[i..], check_non_ascii).map(|off| i + off)
    }

    /// SWAR whitespace run-skip (64-bit targets only; 32-bit targets fall
    /// straight through to the scalar loop).
    #[cfg(target_pointer_width = "64")]
    #[inline]
    pub(super) fn skip_ws(input: &[u8], mut pos: usize) -> usize {
        let len = input.len();
        while pos + 16 <= len {
            let chunk = u128::from_le_bytes(input[pos..pos + 16].try_into().unwrap());
            if !chunk_all_ws(chunk) {
                break;
            }
            pos += 16;
        }
        super::skip_ws_scalar(input, pos)
    }

    /// 32-bit whitespace run-skip: scalar only.
    #[cfg(not(target_pointer_width = "64"))]
    #[inline]
    pub(super) fn skip_ws(input: &[u8], pos: usize) -> usize {
        super::skip_ws_scalar(input, pos)
    }

    /// Whether every byte of a whitespace-shaped chunk is JSON whitespace.
    ///
    /// Each whitespace byte (0x20, 0x09, 0x0A, 0x0D) is detected with the
    /// "haszero" trick: `(x ^ splat(b)) - ONES & ~(x ^ splat(b)) & HIGH != 0`
    /// reports whether *any* byte equals `b`. A chunk is all-whitespace only
    /// when every one of its bytes is one of the four, which is true exactly
    /// when the OR of the four haszero masks equals the all-ones high mask
    /// (the four whitespace bytes are distinct, so at most one matches each
    /// byte position).
    #[cfg(target_pointer_width = "64")]
    #[inline]
    fn chunk_all_ws(chunk: u128) -> bool {
        fn haszero(x: u128, b: u128) -> u128 {
            // Inline-literal masks: a local `const` here produced wrong
            // results under some rustc versions (see chunk_has_escape).
            let x = x ^ b;
            (x.wrapping_sub(0x0101_0101_0101_0101_0101_0101_0101_0101))
                & !x
                & 0x8080_8080_8080_8080_8080_8080_8080_8080
        }
        let m = haszero(chunk, 0x2020_2020_2020_2020_2020_2020_2020_2020)
            | haszero(chunk, 0x0909_0909_0909_0909_0909_0909_0909_0909)
            | haszero(chunk, 0x0A0A_0A0A_0A0A_0A0A_0A0A_0A0A_0A0A_0A0A)
            | haszero(chunk, 0x0D0D_0D0D_0D0D_0D0D_0D0D_0D0D_0D0D_0D0D);
        m == 0x8080_8080_8080_8080_8080_8080_8080_8080
    }
}

// ---------------------------------------------------------------------------
// x86-64: SSE2 (baseline) + AVX2 (runtime-detected under `std`)
// ---------------------------------------------------------------------------

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
mod imp {
    use super::{scan_scalar, skip_ws_scalar};

    pub(super) fn scan(bytes: &[u8], check_non_ascii: bool) -> Option<usize> {
        if bytes.len() < 32 {
            return scan_scalar(bytes, check_non_ascii);
        }
        #[cfg(feature = "std")]
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: `avx2` was confirmed present by CPUID.
            return unsafe { scan_avx2(bytes, check_non_ascii) };
        }
        // SAFETY: SSE2 is always present on x86-64.
        unsafe { scan_sse2(bytes, check_non_ascii) }
    }

    pub(super) fn skip_ws(input: &[u8], pos: usize) -> usize {
        if input.len().saturating_sub(pos) < 32 {
            return skip_ws_scalar(input, pos);
        }
        #[cfg(feature = "std")]
        if std::is_x86_feature_detected!("avx2") {
            // SAFETY: `avx2` was confirmed present by CPUID.
            return unsafe { skip_ws_avx2(input, pos) };
        }
        // SAFETY: SSE2 is always present on x86-64.
        unsafe { skip_ws_sse2(input, pos) }
    }

    #[inline]
    unsafe fn scan_sse2(bytes: &[u8], check_non_ascii: bool) -> Option<usize> {
        use core::arch::x86_64::*;
        let ptr = bytes.as_ptr();
        let len = bytes.len();
        let quote = _mm_set1_epi8(b'"' as i8);
        let backslash = _mm_set1_epi8(b'\\' as i8);
        let control = _mm_set1_epi8(0x1f);
        let high = _mm_set1_epi8(0x80_u8 as i8);
        let mut i = 0usize;
        while i + 16 <= len {
            // SAFETY: `i + 16 <= len` guarantees the load stays in bounds.
            let data = _mm_loadu_si128(ptr.add(i) as *const __m128i);
            // Bytes <= 0x1F: `min(data, 0x1F) == data`.
            let ctl = _mm_cmpeq_epi8(_mm_min_epu8(data, control), data);
            let q = _mm_cmpeq_epi8(data, quote);
            let bs = _mm_cmpeq_epi8(data, backslash);
            let mut mask = _mm_movemask_epi8(_mm_or_si128(_mm_or_si128(ctl, q), bs));
            if check_non_ascii {
                // movemask reads bit 7, so ANDing with 0x80 is a valid
                // "byte >= 0x80" mask.
                mask |= _mm_movemask_epi8(_mm_and_si128(data, high));
            }
            if mask != 0 {
                return Some(i + mask.trailing_zeros() as usize);
            }
            i += 16;
        }
        scan_scalar(&bytes[i..], check_non_ascii).map(|off| i + off)
    }

    #[cfg(feature = "std")]
    #[target_feature(enable = "avx2")]
    unsafe fn scan_avx2(bytes: &[u8], check_non_ascii: bool) -> Option<usize> {
        use core::arch::x86_64::*;
        let ptr = bytes.as_ptr();
        let len = bytes.len();
        let quote = _mm256_set1_epi8(b'"' as i8);
        let backslash = _mm256_set1_epi8(b'\\' as i8);
        let control = _mm256_set1_epi8(0x1f);
        let high = _mm256_set1_epi8(0x80_u8 as i8);
        let mut i = 0usize;
        while i + 32 <= len {
            // SAFETY: `i + 32 <= len` guarantees the load stays in bounds.
            let data = _mm256_loadu_si256(ptr.add(i) as *const __m256i);
            let ctl = _mm256_cmpeq_epi8(_mm256_min_epu8(data, control), data);
            let q = _mm256_cmpeq_epi8(data, quote);
            let bs = _mm256_cmpeq_epi8(data, backslash);
            let mut mask = _mm256_movemask_epi8(_mm256_or_si256(_mm256_or_si256(ctl, q), bs));
            if check_non_ascii {
                mask |= _mm256_movemask_epi8(_mm256_and_si256(data, high));
            }
            if mask != 0 {
                return Some(i + mask.trailing_zeros() as usize);
            }
            i += 32;
        }
        // AVX2-tail: SSE2 still covers chunks >= 16 bytes; shorter tails are
        // scalar. (This call is safe: SSE2 is a subset of AVX2.)
        scan_sse2(&bytes[i..], check_non_ascii).map(|off| i + off)
    }

    #[inline]
    unsafe fn skip_ws_sse2(input: &[u8], mut pos: usize) -> usize {
        use core::arch::x86_64::*;
        let ptr = input.as_ptr();
        let len = input.len();
        let space = _mm_set1_epi8(b' ' as i8);
        let tab = _mm_set1_epi8(b'\t' as i8);
        let lf = _mm_set1_epi8(b'\n' as i8);
        let cr = _mm_set1_epi8(b'\r' as i8);
        while pos + 16 <= len {
            // SAFETY: `pos + 16 <= len` guarantees the load stays in bounds.
            let data = _mm_loadu_si128(ptr.add(pos) as *const __m128i);
            let any = _mm_or_si128(
                _mm_or_si128(_mm_cmpeq_epi8(data, space), _mm_cmpeq_epi8(data, tab)),
                _mm_or_si128(_mm_cmpeq_epi8(data, lf), _mm_cmpeq_epi8(data, cr)),
            );
            let mask = _mm_movemask_epi8(any);
            if mask != 0xFFFF {
                return skip_ws_scalar(input, pos + mask.trailing_ones() as usize);
            }
            pos += 16;
        }
        skip_ws_scalar(input, pos)
    }

    #[cfg(feature = "std")]
    #[target_feature(enable = "avx2")]
    unsafe fn skip_ws_avx2(input: &[u8], mut pos: usize) -> usize {
        use core::arch::x86_64::*;
        let ptr = input.as_ptr();
        let len = input.len();
        let space = _mm256_set1_epi8(b' ' as i8);
        let tab = _mm256_set1_epi8(b'\t' as i8);
        let lf = _mm256_set1_epi8(b'\n' as i8);
        let cr = _mm256_set1_epi8(b'\r' as i8);
        while pos + 32 <= len {
            // SAFETY: `pos + 32 <= len` guarantees the load stays in bounds.
            let data = _mm256_loadu_si256(ptr.add(pos) as *const __m256i);
            let any = _mm256_or_si256(
                _mm256_or_si256(_mm256_cmpeq_epi8(data, space), _mm256_cmpeq_epi8(data, tab)),
                _mm256_or_si256(_mm256_cmpeq_epi8(data, lf), _mm256_cmpeq_epi8(data, cr)),
            );
            let mask = _mm256_movemask_epi8(any);
            if mask != -1 {
                return skip_ws_scalar(input, pos + mask.trailing_ones() as usize);
            }
            pos += 32;
        }
        skip_ws_sse2(input, pos)
    }
}

// ---------------------------------------------------------------------------
// AArch64: NEON (baseline on every AArch64 CPU)
// ---------------------------------------------------------------------------

#[cfg(all(feature = "simd", target_arch = "aarch64"))]
mod imp {
    use super::{scan_scalar, skip_ws_scalar};

    pub(super) fn scan(bytes: &[u8], check_non_ascii: bool) -> Option<usize> {
        if bytes.len() < 32 {
            return scan_scalar(bytes, check_non_ascii);
        }
        // SAFETY: NEON is always present on AArch64.
        unsafe { scan_neon(bytes, check_non_ascii) }
    }

    pub(super) fn skip_ws(input: &[u8], pos: usize) -> usize {
        if input.len().saturating_sub(pos) < 32 {
            return skip_ws_scalar(input, pos);
        }
        // SAFETY: NEON is always present on AArch64.
        unsafe { skip_ws_neon(input, pos) }
    }

    /// NEON scan. A horizontal-max test reports "any match" in the 16-byte
    /// chunk; the exact position is then resolved by the scalar reference
    /// scan over that chunk. This is correct by construction (the scalar
    /// scan uses the identical predicate), at a small per-hit cost that is
    /// irrelevant because hits stop the scan immediately.
    #[inline]
    unsafe fn scan_neon(bytes: &[u8], check_non_ascii: bool) -> Option<usize> {
        use core::arch::aarch64::*;
        let ptr = bytes.as_ptr();
        let len = bytes.len();
        let quote = vdupq_n_u8(b'"');
        let backslash = vdupq_n_u8(b'\\');
        let control = vdupq_n_u8(0x1F);
        let high_gt = vdupq_n_u8(0x7F);
        let mut i = 0usize;
        while i + 16 <= len {
            // SAFETY: `i + 16 <= len` guarantees the load stays in bounds.
            let data = vld1q_u8(ptr.add(i));
            // Bytes <= 0x1F: `min(data, 0x1F) == data`.
            let ctl = vceqq_u8(vminq_u8(data, control), data);
            let mut combined = vorrq_u8(
                vorrq_u8(ctl, vceqq_u8(data, quote)),
                vceqq_u8(data, backslash),
            );
            if check_non_ascii {
                // Unsigned `>` on 0x7F yields 0xFF for every byte >= 0x80.
                combined = vorrq_u8(combined, vcgtq_u8(data, high_gt));
            }
            // Horizontal maximum: 0xFF iff at least one byte matched.
            if vmaxvq_u8(combined) == 0xFF {
                // The scalar reference uses the identical predicate, so a
                // horizontal-max hit guarantees a scalar hit in this chunk.
                // Failing here would mean the SIMD compare diverged from the
                // predicate, which must surface loudly rather than corrupt
                // output.
                return Some(
                    i + scan_scalar(&bytes[i..i + 16], check_non_ascii)
                        .expect("NEON match implies scalar match"),
                );
            }
            i += 16;
        }
        scan_scalar(&bytes[i..], check_non_ascii).map(|off| i + off)
    }

    #[inline]
    unsafe fn skip_ws_neon(input: &[u8], mut pos: usize) -> usize {
        use core::arch::aarch64::*;
        let ptr = input.as_ptr();
        let len = input.len();
        let space = vdupq_n_u8(b' ');
        let tab = vdupq_n_u8(b'\t');
        let lf = vdupq_n_u8(b'\n');
        let cr = vdupq_n_u8(b'\r');
        while pos + 16 <= len {
            // SAFETY: `pos + 16 <= len` guarantees the load stays in bounds.
            let data = vld1q_u8(ptr.add(pos));
            let any = vorrq_u8(
                vorrq_u8(vceqq_u8(data, space), vceqq_u8(data, tab)),
                vorrq_u8(vceqq_u8(data, lf), vceqq_u8(data, cr)),
            );
            // Count consecutive whitespace bytes from the start of the chunk.
            if vmaxvq_u8(any) != 0xFF {
                // Find the first non-whitespace byte within this chunk.
                return skip_ws_scalar(input, pos);
            }
            pos += 16;
        }
        skip_ws_scalar(input, pos)
    }
}

// ---------------------------------------------------------------------------
// Other targets: portable SWAR only
// ---------------------------------------------------------------------------

#[cfg(not(any(
    all(feature = "simd", target_arch = "x86_64"),
    all(feature = "simd", target_arch = "aarch64")
)))]
mod imp {
    pub(super) use super::portable::{scan, skip_ws};
}

// ---------------------------------------------------------------------------
// Tests: every accelerated path must agree with the scalar reference.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn scan_matches_scalar_reference() {
        let mut buf = Vec::new();
        // All single bytes, all two-byte pairs, plus structured patterns.
        for a in 0..=255u8 {
            buf.clear();
            buf.push(a);
            for (esc, got) in [
                (find_escape(&buf, false), scan_scalar(&buf, false)),
                (find_escape(&buf, true), scan_scalar(&buf, true)),
                (find_string_special(&buf), {
                    buf.iter().position(|&b| string_special_byte(b))
                }),
            ] {
                assert_eq!(got, esc, "single byte 0x{a:02X}");
            }
        }
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                buf.clear();
                buf.push(a);
                buf.push(b);
                assert_eq!(
                    find_escape(&buf, false),
                    scan_scalar(&buf, false),
                    "pair {a:02X},{b:02X}"
                );
                assert_eq!(
                    find_escape(&buf, true),
                    scan_scalar(&buf, true),
                    "pair non-ascii {a:02X},{b:02X}"
                );
                assert_eq!(
                    find_string_special(&buf),
                    buf.iter().position(|&x| string_special_byte(x)),
                    "pair special {a:02X},{b:02X}"
                );
            }
        }
    }

    #[test]
    fn scan_lengths_and_patterns() {
        // Every length 0..=80 with every interesting byte placed at the tail,
        // head, and middle, plus long all-clean buffers.
        let interesting = [
            0x00u8, 0x08, 0x09, 0x0A, 0x0D, 0x1F, 0x20, 0x21, 0x22, 0x5B, 0x5C, 0x5D, 0x7E, 0x7F,
            0x80, 0xC3, 0xE4, 0xFF,
        ];
        for len in 0..=80usize {
            let mut clean = vec![b'a'; len];
            assert_eq!(find_escape(&clean, false), scan_scalar(&clean, false));
            assert_eq!(find_escape(&clean, true), scan_scalar(&clean, true));
            assert_eq!(find_string_special(&clean), scan_scalar(&clean, false));
            if len > 0 {
                for &byte in &interesting {
                    for pos in [0usize, len / 2, len - 1] {
                        clean[pos] = byte;
                        assert_eq!(
                            find_escape(&clean, false),
                            scan_scalar(&clean, false),
                            "len {len} byte {byte:02X} at {pos}"
                        );
                        assert_eq!(
                            find_escape(&clean, true),
                            scan_scalar(&clean, true),
                            "len {len} byte {byte:02X} at {pos} (na)"
                        );
                        assert_eq!(
                            find_string_special(&clean),
                            scan_scalar(&clean, false),
                            "len {len} byte {byte:02X} at {pos} (special)"
                        );
                        clean[pos] = b'a';
                    }
                }
            }
        }
        // Large buffers: 1 MiB of clean bytes and buffers with a single
        // escape at a sweep of positions.
        let big = vec![b'x'; 1 << 20];
        assert_eq!(find_escape(&big, false), None);
        assert_eq!(find_escape(&big, true), None);
        assert_eq!(find_string_special(&big), None);
        let mut sweeps = vec![b'x'; 4096];
        for pos in [0usize, 1, 15, 16, 31, 32, 33, 63, 64, 100, 4095] {
            sweeps[pos] = b'"';
            let expect = Some(pos);
            assert_eq!(find_escape(&sweeps, false), expect, "sweep {pos}");
            assert_eq!(find_string_special(&sweeps), expect, "sweep special {pos}");
            sweeps[pos] = b'x';
        }
        // First match wins even when multiple escapes exist.
        let multi = b"aaa\"bbb\\ccc\x01ddd";
        assert_eq!(find_escape(multi, false), Some(3));
        assert_eq!(find_string_special(multi), Some(3));
        let non_ascii_first = [0xC3u8, 0xA9, b'x', b'y', b'z'];
        assert_eq!(find_escape(&non_ascii_first, false), None);
        assert_eq!(find_escape(&non_ascii_first, true), Some(0));
    }

    #[test]
    fn skip_whitespace_matches_scalar() {
        let mut buf = Vec::new();
        // Whitespace-only runs, mixed runs, embedded whitespace, none.
        for len in 0..=100usize {
            for kind in 0..8usize {
                buf.clear();
                for _ in 0..len {
                    buf.push(match (kind + len) % 4 {
                        0 => b' ',
                        1 => b'\t',
                        2 => b'\n',
                        _ => b'\r',
                    });
                }
                let scalar = skip_ws_scalar(&buf, 0);
                assert_eq!(
                    skip_whitespace(&buf, 0),
                    scalar,
                    "run len {len} kind {kind}"
                );
                assert_eq!(skip_whitespace(&buf, 5), skip_ws_scalar(&buf, 5), "offset");
            }
        }
        let cases: &[&[u8]] = &[
            b"",
            b"   ",
            b"\t\n\r ",
            b" \t\n\r x",
            b" \t\n\rx y",
            b"x",
            b"  x  ",
            b"          x ",
            b"  \n\r\t  xyz",
        ];
        for case in cases {
            for start in 0..=case.len() {
                assert_eq!(
                    skip_whitespace(case, start),
                    skip_ws_scalar(case, start),
                    "case {case:?} start {start}"
                );
            }
        }
        // Large whitespace run followed by content.
        let mut big = vec![b'\n'; 4096];
        big.extend_from_slice(b"content");
        assert_eq!(skip_whitespace(&big, 0), 4096);
        assert_eq!(skip_whitespace(&big, 4090), 4096);
        // All-whitespace input consumes to the end.
        let all = vec![b' '; 4096];
        assert_eq!(skip_whitespace(&all, 0), 4096);
    }

    #[test]
    fn no_panic_on_any_input() {
        // Fuzz-ish: random buffers (deterministic LCG) never panic and agree
        // with the reference on every prefix length.
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let mut rng = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        };
        for _ in 0..2000 {
            let len = (rng() as usize) % 80;
            let buf: Vec<u8> = (0..len).map(|_| rng()).collect();
            for end in 0..=len {
                let slice = &buf[..end];
                assert_eq!(find_escape(slice, false), scan_scalar(slice, false));
                assert_eq!(find_escape(slice, true), scan_scalar(slice, true));
                assert_eq!(find_string_special(slice), scan_scalar(slice, false));
                assert_eq!(skip_whitespace(slice, 0), skip_ws_scalar(slice, 0));
            }
        }
    }
}
