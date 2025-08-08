use alloc::{boxed::Box, rc::Rc, vec, vec::Vec};
use core::cell::{Cell, RefCell};

use super::{
    count_multibyte_integer_size, count_multibyte_integer_size_for_value, encode_multibyte_integer,
    parse_multibyte_integer, parse_multibyte_integer_from_reader, CheckType, ChecksumCalculator,
    FilterType, IndexRecord, CRC32, XZ_FOOTER_MAGIC, XZ_MAGIC,
};
use crate::{
    error_invalid_data, error_invalid_input,
    filter::{bcj::BCJReader, delta::DeltaReader},
    ByteReader, LZMA2Reader, Read, Result,
};

/// XZ Index containing all block records and metadata.
#[derive(Debug)]
struct Index {
    number_of_records: u64,
    records: Vec<IndexRecord>,
}

impl Index {
    fn parse<R: Read>(reader: &mut R) -> Result<Self> {
        // sic! Index indicator is already parsed (0x00) in BlockHeader::parse.

        let number_of_records = parse_multibyte_integer_from_reader(reader)?;
        let mut records = Vec::with_capacity(number_of_records as usize);

        for _ in 0..number_of_records {
            let unpadded_size = parse_multibyte_integer_from_reader(reader)?;
            let uncompressed_size = parse_multibyte_integer_from_reader(reader)?;

            if unpadded_size == 0 {
                return Err(error_invalid_data("invalid index record unpadded size"));
            }

            records.push(IndexRecord {
                unpadded_size,
                uncompressed_size,
            });
        }

        // Skip index padding (0-3 null bytes to make multiple of 4).
        let mut bytes_read = 1;
        bytes_read += count_multibyte_integer_size_for_value(number_of_records);
        for record in &records {
            bytes_read += count_multibyte_integer_size_for_value(record.unpadded_size);
            bytes_read += count_multibyte_integer_size_for_value(record.uncompressed_size);
        }

        let padding_needed = (4 - (bytes_read % 4)) % 4;

        if padding_needed > 0 {
            let mut padding_buf = [0u8; 3];
            reader.read_exact(&mut padding_buf[..padding_needed])?;

            if !padding_buf[..padding_needed].iter().all(|&b| b == 0) {
                return Err(error_invalid_data("invalid index padding"));
            }
        }

        let expected_crc = reader.read_u32()?;

        // Calculate CRC32 over index data (excluding CRC32 itself).
        let mut crc = CRC32.digest();
        crc.update(&[0]);

        // Add number of records.
        let mut temp_buf = [0u8; 10];
        let size = encode_multibyte_integer(number_of_records, &mut temp_buf)?;
        crc.update(&temp_buf[..size]);

        // Add all records.
        for record in &records {
            let size = encode_multibyte_integer(record.unpadded_size, &mut temp_buf)?;
            crc.update(&temp_buf[..size]);
            let size = encode_multibyte_integer(record.uncompressed_size, &mut temp_buf)?;
            crc.update(&temp_buf[..size]);
        }

        // Add padding.
        match padding_needed {
            1 => crc.update(&[0]),
            2 => crc.update(&[0, 0]),
            3 => crc.update(&[0, 0, 0]),
            _ => {}
        }

        if expected_crc != crc.finalize() {
            return Err(error_invalid_data("index CRC32 mismatch"));
        }

        Ok(Index {
            number_of_records,
            records,
        })
    }
}

/// XZ stream footer,
#[derive(Debug)]
struct StreamFooter {
    pub backward_size: u32,
    pub stream_flags: [u8; 2],
}

impl StreamFooter {
    fn parse<R: Read>(reader: &mut R) -> Result<Self> {
        let expected_crc = reader.read_u32()?;

        let backward_size = reader.read_u32()?;

        let mut stream_flags = [0u8; 2];
        reader.read_exact(&mut stream_flags)?;

        // Verify CRC32 of backward size + stream flags.
        let mut crc = CRC32.digest();
        crc.update(&backward_size.to_le_bytes());
        crc.update(&stream_flags);

        if expected_crc != crc.finalize() {
            return Err(error_invalid_data("stream footer CRC32 mismatch"));
        }

        let mut footer_magic = [0u8; 2];
        reader.read_exact(&mut footer_magic)?;
        if footer_magic != XZ_FOOTER_MAGIC {
            return Err(error_invalid_data("invalid XZ footer magic bytes"));
        }

        Ok(StreamFooter {
            backward_size,
            stream_flags,
        })
    }
}

/// XZ stream header (12 bytes total)
#[derive(Debug)]
struct StreamHeader {
    check_type: CheckType,
}

impl StreamHeader {
    /// Parse stream header from reader
    fn parse<R: Read>(reader: &mut R) -> Result<Self> {
        let mut magic = [0u8; 6];
        reader.read_exact(&mut magic)?;
        if magic != XZ_MAGIC {
            return Err(error_invalid_data("invalid XZ magic bytes"));
        }

        Self::parse_flags_and_crc(reader)
    }

    /// Parse stream flags and CRC32 after magic bytes have been read.
    fn parse_flags_and_crc<R: Read>(reader: &mut R) -> Result<Self> {
        let mut flags = [0u8; 2];
        reader.read_exact(&mut flags)?;

        if flags[0] != 0 {
            return Err(error_invalid_data("invalid XZ stream flags"));
        }

        let check_type = CheckType::from_byte(flags[1])?;

        let expected_crc = reader.read_u32()?;

        if expected_crc != CRC32.checksum(&flags) {
            return Err(error_invalid_data("XZ stream header CRC32 mismatch"));
        }

        Ok(StreamHeader { check_type })
    }
}

/// XZ block header information
#[derive(Debug)]
struct BlockHeader {
    compressed_size: Option<u64>,
    uncompressed_size: Option<u64>,
    filters: [Option<FilterType>; 4],
    properties: [u32; 4],
}

impl BlockHeader {
    fn parse<R: Read>(reader: &mut R) -> Result<Option<Self>> {
        let header_size_encoded = reader.read_u8()?;

        if header_size_encoded == 0 {
            // If header size is 0, this indicates end of blocks (index follows).
            return Ok(None);
        }

        let header_size = (header_size_encoded as usize + 1) * 4;
        if !(8..=1024).contains(&header_size) {
            return Err(error_invalid_data("invalid XZ block header size"));
        }

        // -1 because we already read the size byte.
        let mut header_data = vec![0u8; header_size - 1];
        reader.read_exact(&mut header_data)?;

        let block_flags = header_data[0];
        let num_filters = ((block_flags & 0x03) + 1) as usize;
        let has_compressed_size = (block_flags & 0x40) != 0;
        let has_uncompressed_size = (block_flags & 0x80) != 0;

        let mut offset = 1;
        let mut compressed_size = None;
        let mut uncompressed_size = None;

        // Parse optional compressed size.
        if has_compressed_size {
            if offset + 8 > header_data.len() {
                return Err(error_invalid_data(
                    "XZ block header too short for compressed size",
                ));
            }
            compressed_size = Some(parse_multibyte_integer(&header_data[offset..])?);
            offset += count_multibyte_integer_size(&header_data[offset..]);
        }

        if has_uncompressed_size {
            if offset >= header_data.len() {
                return Err(error_invalid_data(
                    "XZ block header too short for uncompressed size",
                ));
            }
            uncompressed_size = Some(parse_multibyte_integer(&header_data[offset..])?);
            offset += count_multibyte_integer_size(&header_data[offset..]);
        }

        let mut filters = [None; 4];
        let mut properties = [0; 4];

        for i in 0..num_filters {
            if offset >= header_data.len() {
                return Err(error_invalid_data("XZ block header too short for filters"));
            }

            let filter_type =
                FilterType::try_from(parse_multibyte_integer(&header_data[offset..])?)
                    .map_err(|_| error_invalid_input("unsupported filter type found"))?;

            offset += count_multibyte_integer_size(&header_data[offset..]);

            let property = match filter_type {
                FilterType::Delta => {
                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for Delta properties",
                        ));
                    }

                    let props_size = parse_multibyte_integer(&header_data[offset..])?;
                    offset += count_multibyte_integer_size(&header_data[offset..]);

                    if props_size != 1 {
                        return Err(error_invalid_data("invalid Delta properties size"));
                    }

                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for Delta properties",
                        ));
                    }

                    let distance_prop = header_data[offset];
                    offset += 1;

                    // Distance is encoded as byte value + 1, range [1, 256].
                    (distance_prop as u32) + 1
                }
                FilterType::BcjX86
                | FilterType::BcjPPC
                | FilterType::BcjIA64
                | FilterType::BcjARM
                | FilterType::BcjARMThumb
                | FilterType::BcjSPARC
                | FilterType::BcjARM64
                | FilterType::BcjRISCV => {
                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for BCJ properties",
                        ));
                    }

                    let props_size = parse_multibyte_integer(&header_data[offset..])?;
                    offset += count_multibyte_integer_size(&header_data[offset..]);

                    match props_size {
                        0 => {
                            // No start offset specified, use default (0).
                            0
                        }
                        4 => {
                            // 4-byte start offset specified.
                            if offset + 4 > header_data.len() {
                                return Err(error_invalid_data(
                                    "XZ block header too short for BCJ start offset",
                                ));
                            }

                            let start_offset_value = u32::from_le_bytes([
                                header_data[offset],
                                header_data[offset + 1],
                                header_data[offset + 2],
                                header_data[offset + 3],
                            ]);
                            offset += 4;

                            // Validate alignment based on filter type.
                            let bcj_alignment = match filter_type {
                                FilterType::BcjX86 => 1,
                                FilterType::BcjPPC => 4,
                                FilterType::BcjIA64 => 16,
                                FilterType::BcjARM => 4,
                                FilterType::BcjARMThumb => 2,
                                FilterType::BcjSPARC => 4,
                                FilterType::BcjARM64 => 4,
                                FilterType::BcjRISCV => 2,
                                _ => unreachable!(),
                            };

                            if start_offset_value % bcj_alignment != 0 {
                                return Err(error_invalid_data(
                                    "BCJ start offset not aligned to filter requirements",
                                ));
                            }

                            start_offset_value
                        }
                        _ => {
                            return Err(error_invalid_data("invalid BCJ properties size"));
                        }
                    }
                }
                FilterType::LZMA2 => {
                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for LZMA2 properties",
                        ));
                    }

                    let props_size = parse_multibyte_integer(&header_data[offset..])?;
                    offset += count_multibyte_integer_size(&header_data[offset..]);

                    if props_size != 1 {
                        return Err(error_invalid_data("invalid LZMA2 properties size"));
                    }

                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for LZMA2 properties",
                        ));
                    }

                    let dict_size_prop = header_data[offset];
                    offset += 1;

                    if dict_size_prop > 40 {
                        return Err(error_invalid_data("invalid LZMA2 dictionary size"));
                    }

                    if dict_size_prop == 40 {
                        0xFFFFFFFF
                    } else {
                        let base = 2 | ((dict_size_prop & 1) as u32);
                        base << (dict_size_prop / 2 + 11)
                    }
                }
            };

            filters[i] = Some(filter_type);
            properties[i] = property;
        }

        if filters.iter().filter_map(|x| *x).next_back() != Some(FilterType::LZMA2) {
            return Err(error_invalid_input(
                "XZ block's last filter must be a LZMA2 filter",
            ));
        }

        // Header must be padded so that the total header size matches the declared size.
        // We need to pad until: 1 (size byte) + offset + 4 (CRC32) == header_size
        let expected_offset = header_size - 1 - 4; // header_size - size_byte - crc32_size
        while offset < expected_offset {
            if offset >= header_data.len() || header_data[offset] != 0 {
                return Err(error_invalid_data("invalid XZ block header padding"));
            }
            offset += 1;
        }

        // Last 4 bytes should be CRC32 of the header (excluding the CRC32 itself).
        if offset + 4 != header_data.len() {
            return Err(error_invalid_data("invalid XZ block header CRC32 position"));
        }

        let expected_crc = u32::from_le_bytes([
            header_data[offset],
            header_data[offset + 1],
            header_data[offset + 2],
            header_data[offset + 3],
        ]);

        // Calculate CRC32 of header size byte + header data (excluding CRC32).
        let mut crc = CRC32.digest();
        crc.update(&[header_size_encoded]);
        crc.update(&header_data[..offset]);

        if expected_crc != crc.finalize() {
            return Err(error_invalid_data("XZ block header CRC32 mismatch"));
        }

        Ok(Some(BlockHeader {
            compressed_size,
            uncompressed_size,
            filters,
            properties,
        }))
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
                let mut chain_reader: Box<dyn Read + 'reader> =
                    core::mem::replace(&mut self.reader, Box::new(DUMMY));

                for (filter, property) in block_header
                    .filters
                    .iter()
                    .copied()
                    .zip(block_header.properties)
                    .filter_map(|(filter, property)| filter.map(|filter| (filter, property)))
                    .rev()
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
                            let start_offset = property as usize;
                            Box::new(BCJReader::new_ia64(chain_reader, start_offset))
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
                            let start_offset = property as usize;
                            Box::new(BCJReader::new_riscv(chain_reader, start_offset))
                        }
                        FilterType::LZMA2 => {
                            let dict_size = property;
                            Box::new(LZMA2Reader::new(chain_reader, dict_size, None))
                        }
                    };
                }

                self.reader = chain_reader;

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

            let stream_header = StreamHeader::parse_flags_and_crc(&mut self.reader)?;

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
