//! Shared primitives for the binary codecs.
//!
//! These helpers keep the per-format modules small and focused. All of them
//! operate on `&mut Vec<u8>` / `&[u8]` directly and are allocation-friendly.

use alloc::vec::Vec;

use crate::error::{Error, Result};

/// Maximum number of unvalidated container entries reserved up front.
///
/// Valid larger containers grow normally as entries are decoded. Keeping the
/// initial reservation bounded prevents a forged length plus unrelated
/// trailing bytes from amplifying into a much larger `Vec<Value>` allocation.
pub(crate) const MAX_CONTAINER_PREALLOC: usize = 4_096;

/// Unsigned LEB128 (varint) length used by Postcard sequence headers.
pub(crate) fn write_varint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            buf.push(byte);
            break;
        }
        buf.push(byte | 0x80);
    }
}

/// Read an unsigned LEB128 varint from `input` starting at `pos`.
///
/// Returns `(value, new_pos)`. Rejects overlong encodings (more than 10
/// bytes) and values that overflow `u64`.
pub(crate) fn read_varint(input: &[u8], pos: usize) -> Result<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    let mut i = pos;
    loop {
        let byte = *input
            .get(i)
            .ok_or_else(|| Error::custom("postcard: truncated varint"))?;
        i += 1;
        let payload = (byte & 0x7F) as u64;
        if shift >= 64 || (shift == 63 && payload > 1) {
            return Err(Error::custom("postcard: varint overflow"));
        }
        value |= payload << shift;
        if byte & 0x80 == 0 {
            if shift > 0 && payload == 0 {
                return Err(Error::custom("postcard: non-canonical overlong varint"));
            }
            return Ok((value, i));
        }
        shift += 7;
        if shift >= 70 {
            return Err(Error::custom("postcard: varint too long"));
        }
    }
}

/// Patch a length-prefixed container header in place.
///
/// `start` points at the single placeholder byte written by `begin_*`; the
/// payload follows immediately. `header` replaces the placeholder, growing
/// the buffer when the real header is wider (memmove semantics for the
/// payload, so overlapping copy is safe).
pub(crate) fn patch_prefix(buf: &mut Vec<u8>, start: usize, header: &[u8]) {
    debug_assert!(start < buf.len(), "prefix start out of range");
    let extra = header.len().saturating_sub(1);
    if extra > 0 {
        let len = buf.len();
        buf.resize(len + extra, 0);
        buf.copy_within(start + 1..len, start + 1 + extra);
    }
    buf[start..start + header.len()].copy_from_slice(header);
}

/// Read a fixed little-endian integer from `input`.
pub(crate) fn read_le<const N: usize>(input: &[u8], pos: usize) -> Result<[u8; N]> {
    let end = pos
        .checked_add(N)
        .ok_or_else(|| Error::custom("offset overflow"))?;
    let slice = input
        .get(pos..end)
        .ok_or_else(|| Error::custom("truncated integer"))?;
    let mut out = [0u8; N];
    out.copy_from_slice(slice);
    Ok(out)
}

/// Read a fixed big-endian integer from `input`.
pub(crate) fn read_be<const N: usize>(input: &[u8], pos: usize) -> Result<[u8; N]> {
    let end = pos
        .checked_add(N)
        .ok_or_else(|| Error::custom("offset overflow"))?;
    let slice = input
        .get(pos..end)
        .ok_or_else(|| Error::custom("truncated integer"))?;
    let mut out = [0u8; N];
    out.copy_from_slice(slice);
    Ok(out)
}

/// A small cursor over a byte slice with positional errors.
pub(crate) struct Cursor<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Cursor { input, pos: 0 }
    }

    #[inline]
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Bytes remaining from the current cursor position.
    #[inline]
    pub(crate) fn remaining_len(&self) -> usize {
        self.input.len().saturating_sub(self.pos)
    }

    #[inline]
    pub(crate) fn peek(&self) -> Result<u8> {
        self.input
            .get(self.pos)
            .copied()
            .ok_or_else(|| Error::custom("unexpected end of input"))
    }

    #[inline]
    pub(crate) fn byte(&mut self) -> Result<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Ok(b)
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| Error::custom("offset overflow"))?;
        let slice = self
            .input
            .get(self.pos..end)
            .ok_or_else(|| Error::custom("truncated byte string"))?;
        self.pos = end;
        Ok(slice)
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        self.bytes(n)
    }

    pub(crate) fn le_u32(&mut self) -> Result<u32> {
        let b = read_le::<4>(self.input, self.pos)?;
        self.pos += 4;
        Ok(u32::from_le_bytes(b))
    }

    pub(crate) fn be_u16(&mut self) -> Result<u16> {
        let b = read_be::<2>(self.input, self.pos)?;
        self.pos += 2;
        Ok(u16::from_be_bytes(b))
    }

    pub(crate) fn be_u32(&mut self) -> Result<u32> {
        let b = read_be::<4>(self.input, self.pos)?;
        self.pos += 4;
        Ok(u32::from_be_bytes(b))
    }

    pub(crate) fn be_u64(&mut self) -> Result<u64> {
        let b = read_be::<8>(self.input, self.pos)?;
        self.pos += 8;
        Ok(u64::from_be_bytes(b))
    }

    /// Read until `byte` inclusive; returns the slice up to and including it.
    pub(crate) fn until_inclusive(&mut self, byte: u8) -> Result<&'a [u8]> {
        let tail = &self.input[self.pos..];
        let found = tail
            .iter()
            .position(|b| *b == byte)
            .ok_or_else(|| Error::custom("missing terminator byte"))?;
        let end = self.pos + found + 1;
        let slice = &self.input[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Whether the cursor is exactly at the end of input.
    pub(crate) fn at_end(&self) -> bool {
        self.pos >= self.input.len()
    }

    /// Rewind the cursor by up to `n` bytes (for lookahead re-reads).
    pub(crate) fn rewind(&mut self, n: usize) {
        self.pos = self.pos.saturating_sub(n);
    }

    /// Seek the cursor to an absolute offset (used by backtracking).
    pub(crate) fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// The full input slice.
    pub(crate) fn input(&self) -> &'a [u8] {
        self.input
    }
}
