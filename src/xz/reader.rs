use alloc::{boxed::Box, rc::Rc};
use core::cell::{Cell, RefCell};

use super::{
    create_filter_chain, BlockHeader, ChecksumCalculator, Index, StreamFooter, StreamHeader,
    XZ_MAGIC,
};
use crate::{error_invalid_data, Read, Result};

struct BoundedReader<R> {
    inner: R,
    position: u64,
    limit: u64,
}

impl<R> BoundedReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            position: 0,
            limit,
        }
    }

    fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.position >= self.limit {
            return Ok(0);
        }

        let left = (self.limit - self.position).min(buf.len() as u64) as usize;
        let read_size = self.inner.read(&mut buf[..left])?;
        self.position += read_size as u64;
        Ok(read_size)
    }
}

struct SharedReader<R> {
    inner: Rc<RefCell<R>>,
    compressed_bytes_read: Rc<Cell<u64>>,
}

/// A single-threaded XZ decompressor.
pub struct XZReader<'reader, R> {
    reader: Box<dyn Read + 'reader>,
    stream_header: Option<StreamHeader>,
    checksum_calculator: Option<ChecksumCalculator>,
    finished: bool,
    allow_multiple_streams: bool,
    blocks_processed: u64,
    compressed_bytes_read: Rc<Cell<u64>>,
    original_reader: Rc<RefCell<R>>,
}

impl<R> SharedReader<R> {
    fn new(inner: R, compressed_bytes_read: Rc<Cell<u64>>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(inner)),
            compressed_bytes_read,
        }
    }
}

impl<R: Read> Read for SharedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut reader = self.inner.borrow_mut();
        let bytes_read = reader.read(buf)?;
        self.compressed_bytes_read
            .set(self.compressed_bytes_read.get() + bytes_read as u64);
        Ok(bytes_read)
    }
}

impl<'reader, R: Read + 'reader> XZReader<'reader, R> {
    /// Create a new [`XZReader`].
    pub fn new(inner: R, allow_multiple_streams: bool) -> Self {
        let compressed_bytes_read = Rc::new(Cell::new(0));
        let original_reader = Rc::new(RefCell::new(inner));
        let reader = Box::new(SharedReader {
            inner: Rc::clone(&original_reader),
            compressed_bytes_read: Rc::clone(&compressed_bytes_read),
        });

        Self {
            reader,
            stream_header: None,
            checksum_calculator: None,
            finished: false,
            allow_multiple_streams,
            blocks_processed: 0,
            compressed_bytes_read,
            original_reader,
        }
    }

    /// Consume the XZReader and return the inner reader.
    pub fn into_inner(self) -> R {
        let Self {
            reader,
            original_reader,
            ..
        } = self;

        drop(reader);

        match Rc::try_unwrap(original_reader) {
            Ok(refcell) => refcell.into_inner(),
            Err(_) => {
                panic!("failed to unwrap original reader - other references exists");
            }
        }
    }
}

impl<'reader, R: Read + 'reader> XZReader<'reader, R> {
    fn ensure_stream_header(&mut self) -> Result<()> {
        if self.stream_header.is_none() {
            let header = StreamHeader::parse(&mut self.reader)?;
            self.stream_header = Some(header);
        }
        Ok(())
    }

    fn prepare_next_block(&mut self) -> Result<bool> {
        match BlockHeader::parse(&mut self.reader)? {
            Some(block_header) => {
                static DUMMY: &[u8] = &[];
                let base_reader: Box<dyn Read + 'reader> =
                    core::mem::replace(&mut self.reader, Box::new(DUMMY));

                self.reader = create_filter_chain(
                    base_reader,
                    &block_header.filters,
                    &block_header.properties,
                );

                match self.stream_header.as_ref() {
                    Some(header) => {
                        self.checksum_calculator = Some(ChecksumCalculator::new(header.check_type));
                    }
                    None => {
                        panic!("stream_header not set");
                    }
                }

                self.blocks_processed += 1;

                Ok(true)
            }
            None => {
                // End of blocks reached, index follows.
                self.parse_index_and_footer()?;

                if self.allow_multiple_streams && self.try_start_next_stream()? {
                    return self.prepare_next_block();
                }

                self.finished = true;
                Ok(false)
            }
        }
    }

    fn consume_padding(&mut self) -> Result<()> {
        let padding_needed = match (4 - (self.compressed_bytes_read.get() % 4)) % 4 {
            0 => return Ok(()),
            n => n as usize,
        };

        let mut padding_buf = [0u8; 3];

        let bytes_read = self.reader.read(&mut padding_buf[..padding_needed])?;

        if bytes_read != padding_needed {
            return Err(error_invalid_data("incomplete XZ block padding"));
        }

        if !padding_buf[..bytes_read].iter().all(|&byte| byte == 0) {
            return Err(error_invalid_data("invalid XZ block padding"));
        }

        Ok(())
    }

    fn verify_block_checksum(&mut self) -> Result<()> {
        let checksum_calculator = self
            .checksum_calculator
            .take()
            .expect("checksum_calculator not set");

        match checksum_calculator {
            ChecksumCalculator::None => { /* Nothing to check */ }
            ChecksumCalculator::Crc32(_) => {
                let mut checksum = [0u8; 4];
                self.reader.read_exact(&mut checksum)?;

                if !checksum_calculator.verify(&checksum) {
                    return Err(error_invalid_data("invalid block checksum"));
                }
            }
            ChecksumCalculator::Crc64(_) => {
                let mut checksum = [0u8; 8];
                self.reader.read_exact(&mut checksum)?;

                if !checksum_calculator.verify(&checksum) {
                    return Err(error_invalid_data("invalid block checksum"));
                }
            }
            ChecksumCalculator::Sha256(_) => {
                let mut checksum = [0u8; 32];
                self.reader.read_exact(&mut checksum)?;

                if !checksum_calculator.verify(&checksum) {
                    return Err(error_invalid_data("invalid block checksum"));
                }
            }
        }

        Ok(())
    }

    /// Look for the start of the next stream by reading bytes one at a time
    /// and checking for the XZ magic sequence, allowing for stream padding.
    fn try_start_next_stream(&mut self) -> Result<bool> {
        let mut padding_bytes = 0;
        let mut buffer = [0u8; 6];

        loop {
            let mut byte_buffer = [0u8; 1];
            let read = self.reader.read(&mut byte_buffer)?;
            if read == 0 {
                // EOF reached, no more streams.
                return Ok(false);
            }

            let byte = byte_buffer[0];

            if byte == 0 {
                // Potential stream padding.
                padding_bytes += 1;
                continue;
            }

            // Non-zero byte found - check if it starts XZ magic.
            if byte == XZ_MAGIC[0] {
                return Err(error_invalid_data("invalid data after stream"));
            }

            buffer[0] = byte;
            let mut buffer_pos = 1;

            // Read the rest of the magic bytes.
            while buffer_pos < 6 {
                match self.reader.read(&mut byte_buffer)? {
                    0 => {
                        return Err(error_invalid_data("incomplete XZ magic bytes"));
                    }
                    1 => {
                        buffer[buffer_pos] = byte_buffer[0];
                        buffer_pos += 1;
                    }
                    _ => unreachable!(),
                }
            }

            if buffer != XZ_MAGIC {
                return Err(error_invalid_data("invalid data after stream padding"));
            }

            if padding_bytes % 4 != 0 {
                return Err(error_invalid_data("stream padding size not multiple of 4"));
            }

            let stream_header = StreamHeader::parse_stream_header_flags_and_crc(&mut self.reader)?;

            // Reset state for new stream.
            self.stream_header = Some(stream_header);
            self.blocks_processed = 0;

            return Ok(true);
        }
    }

    fn parse_index_and_footer(&mut self) -> Result<()> {
        let index = Index::parse(&mut self.reader)?;

        if index.number_of_records != self.blocks_processed {
            return Err(error_invalid_data(
                "number of blocks processed doesn't match index records",
            ));
        }

        let stream_footer = StreamFooter::parse(&mut self.reader)?;

        let header = self.stream_header.as_ref().expect("stream_header not set");

        let header_flags = [0, header.check_type as u8];
        if stream_footer.stream_flags != header_flags {
            return Err(error_invalid_data(
                "stream header and footer flags mismatch",
            ));
        }

        Ok(())
    }
}

impl<'reader, R: Read + 'reader> Read for XZReader<'reader, R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.finished {
            return Ok(0);
        }

        self.ensure_stream_header()?;

        loop {
            if self.checksum_calculator.is_some() {
                let bytes_read = self.reader.read(buf)?;

                if bytes_read > 0 {
                    if let Some(ref mut calc) = self.checksum_calculator {
                        calc.update(&buf[..bytes_read]);
                    }

                    return Ok(bytes_read);
                } else {
                    // Current block is finished.
                    let shared_reader = Box::new(SharedReader {
                        inner: Rc::clone(&self.original_reader),
                        compressed_bytes_read: Rc::clone(&self.compressed_bytes_read),
                    });

                    self.reader = shared_reader;

                    self.consume_padding()?;
                    self.verify_block_checksum()?;
                }
            } else {
                // No current block, prepare the next one.
                if !self.prepare_next_block()? {
                    // No more blocks, we're done.
                    return Ok(0);
                }
            }
        }
    }
}
