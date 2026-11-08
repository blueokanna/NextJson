//! Source-independent byte input for `no_std + alloc` decoders.

use crate::Result;

/// A synchronous byte source with no operating-system dependency.
///
/// Return the number of initialized bytes written to `out`, at most its length.
/// `Ok(0)` on a nonempty buffer signals EOF. Return [`crate::Error::interrupted`]
/// for a retryable interruption; other errors stop decoding. Implementations
/// are responsible for deadlines and for bounding retries on stalled devices.
pub trait Read {
    /// Read bytes into the supplied buffer.
    fn read(&mut self, out: &mut [u8]) -> Result<usize>;
}

impl Read for &[u8] {
    fn read(&mut self, out: &mut [u8]) -> Result<usize> {
        let len = self.len().min(out.len());
        let (head, tail) = self.split_at(len);
        out[..len].copy_from_slice(head);
        *self = tail;
        Ok(len)
    }
}

impl<R: Read + ?Sized> Read for &mut R {
    fn read(&mut self, out: &mut [u8]) -> Result<usize> {
        (**self).read(out)
    }
}
