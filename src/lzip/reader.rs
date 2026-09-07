use alloc::vec::Vec;

use super::{HEADER_SIZE, LzipHeader, LzipTrailer, TRAILER_SIZE};
use crate::{
    CountingReader, LzmaReader, Read, Result, crc::Crc32, error_invalid_data, error_invalid_input,
};

/// The input, with the bytes the last member's LZMA reader read ahead of its
/// stream put back in front. Those hold the trailer, and then the next member.
struct Prefixed<R> {
    pending: Vec<u8>,
    pos: usize,
    inner: R,
}

impl<R> Prefixed<R> {
    fn new(inner: R) -> Self {
        Self {
            pending: Vec::new(),
            pos: 0,
            inner,
        }
    }

    /// Puts `bytes` in front of whatever is still pending.
    fn put_back(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let mut pending = bytes;
        pending.extend_from_slice(&self.pending[self.pos..]);
        self.pending = pending;
        self.pos = 0;
    }
}

impl<R: Read> Read for Prefixed<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.pos < self.pending.len() {
            let n = (self.pending.len() - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
            self.pos += n;
            if self.pos == self.pending.len() {
                self.pending.clear();
                self.pos = 0;
            }
            return Ok(n);
        }
        self.inner.read(buf)
    }
}

/// A single-threaded LZIP decompressor.
pub struct LzipReader<R> {
    inner: Option<Prefixed<R>>,
    lzma_reader: Option<LzmaReader<CountingReader<Prefixed<R>>>>,
    current_header: Option<LzipHeader>,
    finished: bool,
    trailer_buf: Vec<u8>,
    crc_digest: Option<Crc32>,
    data_size: u64,
}

impl<R> LzipReader<R> {
    /// Consume the LzipReader and return the inner reader.
    ///
    /// Bytes read ahead of the current position in the LZIP stream are dropped.
    pub fn into_inner(mut self) -> R {
        if let Some(lzma_reader) = self.lzma_reader.take() {
            return lzma_reader.into_inner().inner.inner;
        }

        self.inner.take().expect("inner reader not set").inner
    }

    /// Returns a reference to the inner reader.
    pub fn inner(&self) -> &R {
        self.lzma_reader
            .as_ref()
            .map(|reader| &reader.inner().inner().inner)
            .unwrap_or_else(|| &self.inner.as_ref().expect("inner reader not set").inner)
    }

    /// Returns a mutable reference to the inner reader.
    pub fn inner_mut(&mut self) -> &mut R {
        self.lzma_reader
            .as_mut()
            .map(|reader| &mut reader.inner_mut().inner_mut().inner)
            .unwrap_or_else(|| &mut self.inner.as_mut().expect("inner reader not set").inner)
    }
}

impl<R: Read> LzipReader<R> {
    /// Create a new LZIP reader.
    pub fn new(inner: R) -> Self {
        Self {
            inner: Some(Prefixed::new(inner)),
            lzma_reader: None,
            current_header: None,
            finished: false,
            trailer_buf: Vec::with_capacity(TRAILER_SIZE),
            crc_digest: None,
            data_size: 0,
        }
    }

    /// Start processing the next LZIP member.
    /// Returns Ok(true) if a new member was started, Ok(false) if EOF was reached.
    fn start_next_member(&mut self) -> Result<bool> {
        let mut reader = self.inner.take().expect("inner reader not set");

        let header = match LzipHeader::parse(&mut reader) {
            Ok(header) => header,
            Err(_) => {
                // If header parsing fails, we've probably reached EOF:
                // Put the reader back and indicate we're done:
                self.inner = Some(reader);
                return Ok(false);
            }
        };

        if header.version != 1 {
            return Err(error_invalid_input("unsupported LZIP version"));
        }

        let counting_reader = CountingReader::new(reader);

        // Create LZMA reader with LZMA-302eos properties:
        // - lc=3 (literal context bits)
        // - lp=0 (literal position bits)
        // - pb=2 (position bits)
        // - Unlimited uncompressed size (we'll use trailer to verify)
        let lzma_reader =
            LzmaReader::new(counting_reader, u64::MAX, 3, 0, 2, header.dict_size, None)?;

        self.current_header = Some(header);
        self.lzma_reader = Some(lzma_reader);
        self.trailer_buf.clear();
        self.crc_digest = Some(Crc32::new());
        self.data_size = 0;

        Ok(true)
    }

    fn finish_current_member(&mut self) -> Result<()> {
        let lzma_reader = self.lzma_reader.take().expect("lzma reader not set");

        // What the LZMA reader read ahead of its stream is the trailer and
        // whatever follows. It goes back in front of the input.
        let (counting_reader, leftover) = lzma_reader.into_parts();
        let compressed_bytes = counting_reader.bytes_read() - leftover.len() as u64;

        let mut inner_reader = counting_reader.inner;
        inner_reader.put_back(leftover);

        // The reader goes back in place first, so that a bad trailer leaves
        // `into_inner` something to return.
        let inner_reader = self.inner.insert(inner_reader);
        let trailer = LzipTrailer::parse(inner_reader)?;

        let computed_crc = self.crc_digest.take().expect("no CRC digest").finalize();

        if computed_crc != trailer.crc32 {
            return Err(error_invalid_data("LZIP CRC32 mismatch"));
        }

        if self.data_size != trailer.data_size {
            return Err(error_invalid_data("LZIP data size mismatch"));
        }

        let actual_member_size = HEADER_SIZE as u64 + compressed_bytes + TRAILER_SIZE as u64;
        if actual_member_size != trailer.member_size {
            return Err(error_invalid_data("LZIP member size mismatch"));
        }

        Ok(())
    }
}

impl<R: Read> Read for LzipReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            // If we have an active LZMA reader, try to read from it.
            if let Some(ref mut lzma_reader) = self.lzma_reader {
                match lzma_reader.read(buf) {
                    Ok(0) => {
                        // Current member is finished, verify trailer.
                        self.finish_current_member()?;

                        if !self.start_next_member()? {
                            // No more members, we're done.
                            self.finished = true;
                            return Ok(0);
                        }

                        // Continue to read from the new member.
                        continue;
                    }
                    Ok(bytes_read) => {
                        // Update CRC with the decompressed data
                        if let Some(ref mut crc_digest) = self.crc_digest {
                            crc_digest.update(&buf[..bytes_read]);
                            self.data_size += bytes_read as u64;
                        }
                        return Ok(bytes_read);
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            } else if self.finished {
                // Already finished, return EOF.
                return Ok(0);
            } else {
                // No active LZMA reader, start the first/next member.
                if !self.start_next_member()? {
                    // No members found, we're done.
                    self.finished = true;
                    return Ok(0);
                }
            }
        }
    }
}
