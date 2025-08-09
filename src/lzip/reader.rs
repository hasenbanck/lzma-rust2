use alloc::vec::Vec;

use super::{LZIPHeader, LZIPTrailer, CRC32, HEADER_SIZE, TRAILER_SIZE};
use crate::{error_invalid_data, error_invalid_input, LZMAReader, Read, Result};

struct CountingReader<R> {
    inner: R,
    bytes_read: u64,
}

impl<R> CountingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            bytes_read: 0,
        }
    }

    fn bytes_read(&self) -> u64 {
        self.bytes_read
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        self.bytes_read += bytes_read as u64;
        Ok(bytes_read)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        self.inner.read_exact(buf)?;
        self.bytes_read += buf.len() as u64;
        Ok(())
    }
}

/// A single-threaded LZIP decompressor.
pub struct LZIPReader<R> {
    inner: Option<R>,
    lzma_reader: Option<LZMAReader<CountingReader<R>>>,
    current_header: Option<LZIPHeader>,
    finished: bool,
    trailer_buf: Vec<u8>,
    crc_digest: Option<crc::Digest<'static, u32, crc::Table<16>>>,
    data_size: u64,
}

impl<R> LZIPReader<R> {
    /// Consume the LZIPReader and return the inner reader.
    pub fn into_inner(mut self) -> R {
        if let Some(lzma_reader) = self.lzma_reader.take() {
            return lzma_reader.into_inner().inner;
        }

        self.inner.take().expect("inner reader not set")
    }
}

impl<R: Read> LZIPReader<R> {
    /// Create a new LZIP reader.
    pub fn new(inner: R) -> Result<Self> {
        Ok(Self {
            inner: Some(inner),
            lzma_reader: None,
            current_header: None,
            finished: false,
            trailer_buf: Vec::with_capacity(TRAILER_SIZE),
            crc_digest: None,
            data_size: 0,
        })
    }

    /// Start processing the next LZIP member.
    /// Returns Ok(true) if a new member was started, Ok(false) if EOF was reached.
    fn start_next_member(&mut self) -> Result<bool> {
        let mut reader = self.inner.take().expect("inner reader not set");

        let header = match LZIPHeader::parse(&mut reader) {
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
            LZMAReader::new(counting_reader, u64::MAX, 3, 0, 2, header.dict_size, None)?;

        self.current_header = Some(header);
        self.lzma_reader = Some(lzma_reader);
        self.trailer_buf.clear();
        self.crc_digest = Some(CRC32.digest());
        self.data_size = 0;

        Ok(true)
    }

    fn finish_current_member(&mut self) -> Result<()> {
        let lzma_reader = self.lzma_reader.take().expect("lzma reader not set");

        let counting_reader = lzma_reader.into_inner();
        let compressed_bytes = counting_reader.bytes_read();

        let mut inner_reader = counting_reader.inner;
        let trailer = LZIPTrailer::parse(&mut inner_reader)?;

        let computed_crc = self.crc_digest.take().expect("no CRC digest").finalize();

        if computed_crc != trailer.crc32 {
            self.inner = Some(inner_reader);
            return Err(error_invalid_data("LZIP CRC32 mismatch"));
        }

        if self.data_size != trailer.data_size {
            self.inner = Some(inner_reader);
            return Err(error_invalid_data("LZIP data size mismatch"));
        }

        let actual_member_size = HEADER_SIZE as u64 + compressed_bytes + TRAILER_SIZE as u64;
        if actual_member_size != trailer.member_size {
            self.inner = Some(inner_reader);
            return Err(error_invalid_data("LZIP member size mismatch"));
        }

        // Store the reader for potential next member.
        self.inner = Some(inner_reader);

        Ok(())
    }
}

impl<R: Read> Read for LZIPReader<R> {
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
