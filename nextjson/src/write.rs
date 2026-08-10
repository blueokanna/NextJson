//! Minimal no_std byte sink trait used by the encoder.

use crate::error::{Error, Result};

/// A minimal byte sink.
///
/// This is the library's own abstraction so the core stays `no_std` and
/// dependency-free. Implementations are provided for `alloc` buffers, and
/// (behind the `std` feature) a wrapper for any `std::io::Write` type.
pub trait Write {
    /// Write all bytes.
    fn write_all(&mut self, buf: &[u8]) -> Result<()>;
    /// Flush buffered output. Defaults to a no-op.
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Write for alloc::vec::Vec<u8> {
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.extend_from_slice(buf);
        Ok(())
    }
}

impl Write for alloc::string::String {
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        let s = core::str::from_utf8(buf)
            .map_err(|_| Error::custom("write: non-UTF-8 bytes into String"))?;
        self.push_str(s);
        Ok(())
    }
}

impl<W: Write + ?Sized> Write for &mut W {
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        (**self).write_all(buf)
    }
    fn flush(&mut self) -> Result<()> {
        (**self).flush()
    }
}

impl Write for &mut [u8] {
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        let this = core::mem::replace(self, &mut [][..]);
        if buf.len() > this.len() {
            *self = this;
            return Err(Error::custom("write: output buffer too small"));
        }
        let len = buf.len();
        let (head, tail) = this.split_at_mut(len);
        head.copy_from_slice(buf);
        *self = tail;
        Ok(())
    }
}

/// Adapter from `std::io::Write` to this library's `Write`.
#[cfg(feature = "std")]
pub struct StdWriter<W>(pub W);

#[cfg(feature = "std")]
impl<W: std::io::Write> Write for StdWriter<W> {
    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.0.write_all(buf).map_err(Error::io)
    }
    fn flush(&mut self) -> Result<()> {
        self.0.flush().map_err(Error::io)
    }
}
