use alloc::rc::Rc;
use core::{cell::Cell, marker::PhantomData};

use sha2::Digest;

use super::{BlockHeader, CheckType, FilterType, StreamHeader, CRC32, CRC64};
use crate::{
    error_invalid_data,
    filter::{bcj::BCJReader, delta::DeltaReader},
    LZMA2Reader, Read, Result,
};

/// Trait for readers that can be "peeled" to extract their inner reader
trait PeelableRead: Read {
    fn peel(self: Box<Self>) -> Box<dyn PeelableRead>;
    fn is_base_reader(&self) -> bool {
        false
    }
    fn as_any(&self) -> &dyn std::any::Any;
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>;
}

/// Marker for the base reader
struct BaseReader<R> {
    inner: R,
    compressed_bytes_read: Rc<Cell<u64>>,
}

impl<R> BaseReader<R> {
    fn new(inner: R, compressed_bytes_read: Rc<Cell<u64>>) -> Self {
        Self {
            inner,
            compressed_bytes_read,
        }
    }

    fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for BaseReader<R> {
    #[inline(always)]
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        self.compressed_bytes_read
            .set(self.compressed_bytes_read.get() + bytes_read as u64);
        Ok(bytes_read)
    }
}

impl<R: Read + 'static> PeelableRead for BaseReader<R> {
    #[inline(always)]
    fn peel(self: Box<Self>) -> Box<dyn PeelableRead> {
        self
    }

    #[inline(always)]
    fn is_base_reader(&self) -> bool {
        true
    }

    #[inline(always)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[inline(always)]
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

impl<R: PeelableRead + 'static> PeelableRead for BoundedReader<R> {
    #[inline(always)]
    fn peel(self: Box<Self>) -> Box<dyn PeelableRead> {
        Box::new(self.inner)
    }

    #[inline(always)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[inline(always)]
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

impl<R: PeelableRead + 'static> PeelableRead for DeltaReader<R> {
    #[inline(always)]
    fn peel(self: Box<Self>) -> Box<dyn PeelableRead> {
        Box::new(self.into_inner())
    }

    #[inline(always)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[inline(always)]
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

impl<R: PeelableRead + 'static> PeelableRead for BCJReader<R> {
    #[inline(always)]
    fn peel(self: Box<Self>) -> Box<dyn PeelableRead> {
        Box::new(self.into_inner())
    }

    #[inline(always)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[inline(always)]
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

impl<R: PeelableRead + 'static> PeelableRead for LZMA2Reader<R> {
    #[inline(always)]
    fn peel(self: Box<Self>) -> Box<dyn PeelableRead> {
        Box::new(self.into_inner())
    }

    #[inline(always)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[inline(always)]
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

impl PeelableRead for Box<dyn PeelableRead> {
    #[inline(always)]
    fn peel(self: Box<Self>) -> Box<dyn PeelableRead> {
        // Remove one layer of boxing
        *self
    }

    #[inline(always)]
    fn is_base_reader(&self) -> bool {
        (**self).is_base_reader()
    }

    #[inline(always)]
    fn as_any(&self) -> &dyn std::any::Any {
        (**self).as_any()
    }

    #[inline(always)]
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        (*self).into_any()
    }
}

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

/// A wrapper around a reader that calculates checksums while reading
struct BlockReader<R> {
    inner: R,
    remaining_bytes: u64,
    checksum_calculator: ChecksumCalculator,
    finished: bool,
}

impl<R> BlockReader<R> {
    fn new(inner: R, block_size: Option<u64>, check_type: CheckType) -> Self {
        Self {
            inner,
            remaining_bytes: block_size.unwrap_or(u64::MAX),
            checksum_calculator: ChecksumCalculator::new(check_type),
            finished: false,
        }
    }

    fn verify_checksum(&self, expected: &[u8]) -> bool {
        self.checksum_calculator.verify(expected)
    }

    fn is_finished(&self) -> bool {
        self.finished
    }
}

impl<R: Read> Read for BlockReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.finished || self.remaining_bytes == 0 {
            return Ok(0);
        }

        let max_read = (buf.len() as u64).min(self.remaining_bytes) as usize;
        let bytes_read = self.inner.read(&mut buf[..max_read])?;

        if bytes_read == 0 {
            self.finished = true;
            return Ok(0);
        }

        self.checksum_calculator.update(&buf[..bytes_read]);
        self.remaining_bytes -= bytes_read as u64;

        if self.remaining_bytes == 0 {
            self.finished = true;
        }

        Ok(bytes_read)
    }
}

/// Handles checksum calculation for different XZ check types
enum ChecksumCalculator {
    None,
    Crc32(crc::Digest<'static, u32, crc::Table<16>>),
    Crc64(crc::Digest<'static, u64, crc::Table<16>>),
    Sha256(sha2::Sha256),
}

impl ChecksumCalculator {
    fn new(check_type: CheckType) -> Self {
        match check_type {
            CheckType::None => Self::None,
            CheckType::Crc32 => Self::Crc32(CRC32.digest()),
            CheckType::Crc64 => Self::Crc64(CRC64.digest()),
            CheckType::Sha256 => Self::Sha256(sha2::Sha256::new()),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            ChecksumCalculator::None => {}
            ChecksumCalculator::Crc32(crc) => {
                crc.update(data);
            }
            ChecksumCalculator::Crc64(crc) => {
                crc.update(data);
            }
            ChecksumCalculator::Sha256(sha) => {
                sha.update(data);
            }
        }
    }

    fn verify(&self, expected: &[u8]) -> bool {
        match self {
            ChecksumCalculator::None => true,
            ChecksumCalculator::Crc32(crc) => {
                if expected.len() != 4 {
                    return false;
                }

                let expected_crc =
                    u32::from_le_bytes([expected[0], expected[1], expected[2], expected[3]]);

                let final_crc = crc.clone().finalize();

                final_crc == expected_crc
            }
            ChecksumCalculator::Crc64(crc) => {
                if expected.len() != 8 {
                    return false;
                }

                let expected_crc = u64::from_le_bytes([
                    expected[0],
                    expected[1],
                    expected[2],
                    expected[3],
                    expected[4],
                    expected[5],
                    expected[6],
                    expected[7],
                ]);

                let final_crc = crc.clone().finalize();

                final_crc == expected_crc
            }
            ChecksumCalculator::Sha256(sha) => {
                if expected.len() != 32 {
                    return false;
                }

                let final_sha = sha.clone().finalize();

                &final_sha[..32] == expected
            }
        }
    }

    fn checksum_size(&self) -> usize {
        match self {
            ChecksumCalculator::None => 0,
            ChecksumCalculator::Crc32(_) => 4,
            ChecksumCalculator::Crc64(_) => 8,
            ChecksumCalculator::Sha256(_) => 32,
        }
    }
}

/// XZ format decoder that wraps LZMA2 blocks
pub struct XZReader<R> {
    stream_header: Option<StreamHeader>,
    current_reader: Option<Box<dyn PeelableRead>>,
    current_block_remaining: u64,
    current_checksum_calculator: Option<ChecksumCalculator>,
    compressed_bytes_read: Rc<Cell<u64>>,
    finished: bool,
    _marker: PhantomData<R>,
}

// TODO: 'static doesn't allow to use XZReader with borrowed data!
impl<R: Read + 'static> XZReader<R> {
    /// Create a new XZ reader
    pub fn new(inner: R) -> Self {
        let compressed_bytes_read = Rc::new(Cell::new(0));
        Self {
            stream_header: None,
            current_reader: Some(Box::new(BaseReader::new(
                inner,
                Rc::clone(&compressed_bytes_read),
            ))),
            current_block_remaining: 0,
            current_checksum_calculator: None,
            compressed_bytes_read,
            finished: false,
            _marker: PhantomData,
        }
    }

    /// Consume the XZReader and return the inner reader
    pub fn into_inner(mut self) -> R {
        match self.current_reader.take() {
            Some(mut current_reader) => {
                while !current_reader.is_base_reader() {
                    current_reader = current_reader.peel();
                }

                let base_reader_any = current_reader.into_any();

                match base_reader_any.downcast::<BaseReader<R>>() {
                    Ok(base_reader) => base_reader.into_inner(),
                    Err(_) => panic!("failed to downcast to BaseReader"),
                }
            }
            None => panic!("current_reader is None"),
        }
    }

    /// Initialize by parsing the stream header
    fn ensure_stream_header(&mut self) -> Result<()> {
        if self.stream_header.is_none() {
            let current_reader = self
                .current_reader
                .as_mut()
                .expect("current_reader not set");

            let header = StreamHeader::parse(current_reader)?;
            self.stream_header = Some(header);
        }
        Ok(())
    }

    /// Prepare the next block for reading
    fn prepare_next_block(&mut self) -> Result<bool> {
        let current_reader = self
            .current_reader
            .as_mut()
            .expect("current_reader not set");

        match BlockHeader::parse(current_reader)? {
            Some(block_header) => {
                self.current_block_remaining = block_header.uncompressed_size.unwrap_or(u64::MAX);

                let base_reader = self.current_reader.take().expect("current_reader not set");

                let mut chain_reader: Box<dyn PeelableRead> = match block_header.compressed_size {
                    Some(compressed_size) => {
                        Box::new(BoundedReader::new(base_reader, compressed_size))
                    }
                    None => base_reader,
                };

                for (filter, property) in block_header
                    .filters
                    .iter()
                    .copied()
                    .zip(block_header.properties)
                    .filter_map(|(filter, property)| filter.map(|filter| (filter, property)))
                {
                    chain_reader = match filter {
                        FilterType::Delta => {
                            let distance = property as usize;
                            Box::new(DeltaReader::new(chain_reader, distance))
                        }
                        FilterType::BcjX86 => {
                            let start_offset = property as usize;
                            Box::new(BCJReader::new_x86(chain_reader, start_offset))
                        }
                        FilterType::BcjPPC => {
                            let start_offset = property as usize;
                            Box::new(BCJReader::new_ppc(chain_reader, start_offset))
                        }
                        FilterType::BcjIA64 => {
                            todo!()
                        }
                        FilterType::BcjARM => {
                            let start_offset = property as usize;
                            Box::new(BCJReader::new_arm(chain_reader, start_offset))
                        }
                        FilterType::BcjARMThumb => {
                            let start_offset = property as usize;
                            Box::new(BCJReader::new_arm_thumb(chain_reader, start_offset))
                        }
                        FilterType::BcjSPARC => {
                            let start_offset = property as usize;
                            Box::new(BCJReader::new_sparc(chain_reader, start_offset))
                        }
                        FilterType::BcjARM64 => {
                            let start_offset = property as usize;
                            Box::new(BCJReader::new_arm64(chain_reader, start_offset))
                        }
                        FilterType::BcjRISCV => {
                            todo!()
                        }
                        FilterType::LZMA2 => {
                            let dict_size = property;
                            Box::new(LZMA2Reader::new(chain_reader, dict_size, None))
                        }
                    };
                }

                self.current_reader = Some(chain_reader);

                match self.stream_header.as_ref() {
                    Some(header) => {
                        self.current_checksum_calculator =
                            Some(ChecksumCalculator::new(header.check_type));
                    }
                    None => {
                        panic!("stream_header not set");
                    }
                }

                Ok(true)
            }
            None => {
                // End of blocks reached, index follows
                self.finished = true;
                Ok(false)
            }
        }
    }

    /// Consume padding bytes (null bytes) until 4-byte alignment
    fn consume_padding(&mut self) -> Result<()> {
        let padding_needed = match (4 - (self.compressed_bytes_read.get() % 4)) % 4 {
            0 => return Ok(()),
            n => n as usize,
        };

        let current_reader = self
            .current_reader
            .as_mut()
            .expect("current_reader not set");

        let mut padding_buf = [0u8; 3];

        let bytes_read = current_reader.read(&mut padding_buf[..padding_needed])?;

        if bytes_read != padding_needed {
            return Err(error_invalid_data("Incomplete XZ block padding"));
        }

        if !padding_buf[..bytes_read].iter().all(|&byte| byte == 0) {
            return Err(error_invalid_data("Invalid XZ block padding"));
        }

        Ok(())
    }

    /// Consume and verify the block checksum.
    fn verify_block_checksum(&mut self) -> Result<()> {
        if let Some(current_checksum_calculator) = self.current_checksum_calculator.as_mut() {
            let current_reader = self
                .current_reader
                .as_mut()
                .expect("current_reader not set");

            let checksum_size = current_checksum_calculator.checksum_size();

            let mut checksum = [0u8; 32];
            current_reader.read_exact(&mut checksum[..checksum_size])?;

            if !current_checksum_calculator.verify(&checksum[..checksum_size]) {
                return Err(error_invalid_data("invalid block checksum"));
            }
        }

        Ok(())
    }
}

impl<R: Read + 'static> Read for XZReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.finished {
            return Ok(0);
        }

        self.ensure_stream_header()?;

        loop {
            if self.current_block_remaining != 0 {
                let current_reader = self
                    .current_reader
                    .as_mut()
                    .expect("current_reader not set");

                let bytes_read = current_reader.read(buf)?;

                if bytes_read > 0 {
                    if let Some(ref mut calc) = self.current_checksum_calculator {
                        calc.update(&buf[..bytes_read]);
                    }

                    self.current_block_remaining = self
                        .current_block_remaining
                        .saturating_sub(bytes_read as u64);

                    return Ok(bytes_read);
                } else {
                    // Current block is exhausted - peel back to base reader
                    let mut current = self.current_reader.take().expect("current_reader not set");

                    while !current.is_base_reader() {
                        current = current.peel();
                    }

                    self.current_reader = Some(current);

                    self.consume_padding()?;
                    self.verify_block_checksum()?;

                    self.current_checksum_calculator = None;
                    self.current_block_remaining = 0;
                }
            } else {
                // TODO: We currently don't correctly handle reading multiple blocks.
                //       Check the specification what we are expected to do here.

                // No current block, prepare the next one
                if !self.prepare_next_block()? {
                    // No more blocks, we're done
                    return Ok(0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum_calculator_crc32() {
        let mut calc = ChecksumCalculator::new(CheckType::Crc32);
        calc.update(b"123456789");

        // CRC32 of "123456789" in little-endian format
        let expected = [0x26, 0x39, 0xF4, 0xCB];
        assert!(calc.verify(&expected));
    }

    #[test]
    fn test_checksum_calculator_crc64() {
        let mut calc = ChecksumCalculator::new(CheckType::Crc64);
        calc.update(b"123456789");

        // CRC64 of "123456789" in little-endian format
        let expected = [250, 57, 25, 223, 187, 201, 93, 153];
        assert!(calc.verify(&expected));
    }

    #[test]
    fn test_checksum_calculator_sha256() {
        let mut calc = ChecksumCalculator::new(CheckType::Sha256);
        calc.update(b"123456789");

        // SHA256 of "123456789"
        let expected = [
            21, 226, 176, 211, 195, 56, 145, 235, 176, 241, 239, 96, 158, 196, 25, 66, 12, 32, 227,
            32, 206, 148, 198, 95, 188, 140, 51, 18, 68, 142, 178, 37,
        ];
        assert!(calc.verify(&expected));
    }

    #[test]
    fn test_block_reader_limits() {
        let data = b"Hello, world! This is a test.";
        let mut reader = BlockReader::new(data.as_slice(), Some(5), CheckType::None);

        let mut buf = [0u8; 10];
        let bytes_read = reader.read(&mut buf).unwrap();

        assert_eq!(bytes_read, 5);
        assert_eq!(&buf[..5], b"Hello");
        assert!(reader.is_finished());
    }
}
