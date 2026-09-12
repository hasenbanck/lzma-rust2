use alloc::vec::Vec;

use super::{HEADER_SIZE, LzipHeader, LzipTrailer, TRAILER_SIZE, read_some};
use crate::{
    CountingReader, LzmaReader, Read, Result, StickyError, crc::Crc32, error_invalid_data,
};

/// A single-threaded LZIP decompressor.
///
/// Reads from a source that blocks until it has data. An error from the source
/// ends the decoding: the reader reports that error again on every later call.
pub struct LzipReader<R> {
    inner: Option<MemberInput<R>>,
    lzma_reader: Option<LzmaReader<CountingReader<MemberInput<R>>>>,
    current_header: Option<LzipHeader>,
    finished: bool,
    failure: Option<StickyError>,
    trailer_buf: Vec<u8>,
    crc_digest: Option<Crc32>,
    data_size: u64,
}

/// Input recovered from a member's LZMA reader must precede the next source read.
struct MemberInput<R> {
    reader: R,
    pending: Vec<u8>,
    position: usize,
}

impl<R> MemberInput<R> {
    fn restore(&mut self, mut bytes: Vec<u8>) {
        bytes.extend_from_slice(&self.pending[self.position..]);
        self.pending = bytes;
        self.position = 0;
    }

    /// The source together with `bytes` and everything still held back here.
    fn into_parts(mut self, bytes: Vec<u8>) -> (R, Vec<u8>) {
        self.restore(bytes);
        (self.reader, self.pending)
    }
}

impl<R: Read> Read for MemberInput<R> {
    fn read(&mut self, output: &mut [u8]) -> Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let count = output.len().min(self.pending.len() - self.position);
        if count == 0 {
            return self.reader.read(output);
        }
        output[..count].copy_from_slice(&self.pending[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

impl<R> LzipReader<R> {
    /// Consume the LzipReader and return the inner reader.
    ///
    /// Discards any buffered input. Use [`Self::into_parts`] to recover it.
    pub fn into_inner(self) -> R {
        self.into_parts().0
    }

    /// Returns the underlying reader and the bytes buffered but not consumed.
    /// Read the returned bytes before continuing with the underlying reader.
    ///
    /// The reader reads ahead, so after the last member these are the bytes
    /// that follow the LZIP stream.
    pub fn into_parts(mut self) -> (R, Vec<u8>) {
        if let Some(lzma_reader) = self.lzma_reader.take() {
            let (counting_reader, unused) = lzma_reader.into_parts();
            return counting_reader.inner.into_parts(unused);
        }

        let input = self.inner.take().expect("inner reader not set");
        input.into_parts(Vec::new())
    }

    /// Returns a reference to the inner reader.
    ///
    /// The reader reads ahead, so it can already sit past the member being decoded.
    pub fn inner(&self) -> &R {
        self.lzma_reader
            .as_ref()
            .map(|reader| &reader.inner().inner().reader)
            .unwrap_or_else(|| &self.inner.as_ref().expect("inner reader not set").reader)
    }

    /// Returns a mutable reference to the inner reader.
    ///
    /// The reader reads ahead, so it can already sit past the member being
    /// decoded. Reading from the returned reader takes bytes the decoder still
    /// needs.
    pub fn inner_mut(&mut self) -> &mut R {
        self.lzma_reader
            .as_mut()
            .map(|reader| &mut reader.inner_mut().inner_mut().reader)
            .unwrap_or_else(|| &mut self.inner.as_mut().expect("inner reader not set").reader)
    }
}

impl<R: Read> LzipReader<R> {
    /// Create a new LZIP reader.
    pub fn new(inner: R) -> Self {
        Self {
            inner: Some(MemberInput {
                reader: inner,
                pending: Vec::new(),
                position: 0,
            }),
            lzma_reader: None,
            current_header: None,
            finished: false,
            failure: None,
            trailer_buf: Vec::with_capacity(TRAILER_SIZE),
            crc_digest: None,
            data_size: 0,
        }
    }

    /// Start processing the next LZIP member.
    /// Returns Ok(true) if a new member was started, Ok(false) if EOF was reached.
    fn start_next_member(&mut self) -> Result<bool> {
        let mut reader = self.inner.take().expect("inner reader not set");

        let mut header_bytes = [0u8; HEADER_SIZE];
        let filled = match read_some(&mut reader, &mut header_bytes) {
            Ok(filled) => filled,
            Err(error) => {
                self.inner = Some(reader);
                return Err(error);
            }
        };

        if filled == 0 {
            // The source ended on a member boundary, so the file is whole.
            self.inner = Some(reader);
            return Ok(false);
        }

        let header = match LzipHeader::parse(&header_bytes[..filled]) {
            Ok(header) => header,
            Err(error) => {
                // Hand the bytes back, so that the caller can reach whatever
                // stands where a member was meant to be.
                reader.restore(header_bytes[..filled].to_vec());
                self.inner = Some(reader);
                return Err(error);
            }
        };

        let counting_reader = CountingReader::new(reader);

        // Create LZMA reader with LZMA-302eos properties:
        // - lc=3 (literal context bits)
        // - lp=0 (literal position bits)
        // - pb=2 (position bits)
        // - Unlimited uncompressed size (we'll use trailer to verify)
        let lzma_reader = match LzmaReader::new_recover(
            counting_reader,
            u64::MAX,
            3,
            0,
            2,
            header.dict_size,
            None,
        ) {
            Ok(lzma_reader) => lzma_reader,
            Err((counting_reader, unused, error)) => {
                // A member that cannot start still leaves the source usable,
                // so keep it rather than let the caller find it gone.
                let mut input = counting_reader.inner;
                input.restore(unused);
                self.inner = Some(input);
                return Err(error);
            }
        };

        self.current_header = Some(header);
        self.lzma_reader = Some(lzma_reader);
        self.trailer_buf.clear();
        self.crc_digest = Some(Crc32::new());
        self.data_size = 0;

        Ok(true)
    }

    fn finish_current_member(&mut self) -> Result<()> {
        let lzma_reader = self.lzma_reader.take().expect("lzma reader not set");

        let (counting_reader, unused) = lzma_reader.into_parts();
        let compressed_bytes = counting_reader.bytes_read() - unused.len() as u64;

        let mut input = counting_reader.inner;
        input.restore(unused);
        // Preserve ownership even when parsing a truncated trailer fails.
        self.inner = Some(input);
        let trailer = LzipTrailer::parse(self.inner.as_mut().expect("inner reader not set"))?;

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
        if let Some(failure) = self.failure.as_ref() {
            return Err(failure.report());
        }

        match self.read_decode(buf) {
            Ok(count) => Ok(count),
            Err(error) => {
                // A rejected member leaves the source part way through the
                // file. Starting a member from there fails to parse a header,
                // which a later read would otherwise take for a clean end.
                let failure = StickyError::new(error);
                let reported = failure.report();
                self.failure = Some(failure);
                Err(reported)
            }
        }
    }
}

impl<R: Read> LzipReader<R> {
    fn read_decode(&mut self, buf: &mut [u8]) -> Result<usize> {
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
