use alloc::{boxed::Box, rc::Rc, vec::Vec};
use core::cell::{Cell, RefCell};
use std::num::NonZeroU64;

use sha2::Digest;

use super::{
    count_multibyte_integer_size_for_value, encode_multibyte_integer, CheckType,
    ChecksumCalculator, FilterConfig, FilterType, IndexRecord, CRC32, XZ_FOOTER_MAGIC, XZ_MAGIC,
};
use crate::{
    enc::{LZMA2Writer, LZMAOptions},
    error_invalid_data, error_invalid_input,
    filter::{bcj::BCJWriter, delta::DeltaWriter},
    Result, Write,
};

trait FinishableWriter: Write {
    fn finish(self: Box<Self>) -> Result<()>;
}

impl<W: Write> FinishableWriter for LZMA2Writer<W> {
    fn finish(self: Box<Self>) -> Result<()> {
        (*self).finish()?;
        Ok(())
    }
}

impl<W: FinishableWriter> FinishableWriter for DeltaWriter<W> {
    fn finish(self: Box<Self>) -> Result<()> {
        let inner = (*self).into_inner();
        Box::new(inner).finish()
    }
}

impl<W: FinishableWriter> FinishableWriter for BCJWriter<W> {
    fn finish(self: Box<Self>) -> Result<()> {
        let inner = (*self).into_inner();
        Box::new(inner).finish()
    }
}

struct SharedWriter<W> {
    inner: Rc<RefCell<W>>,
    compressed_bytes_written: Rc<Cell<u64>>,
}

impl<W> SharedWriter<W> {
    fn new(inner: W, compressed_bytes_written: Rc<Cell<u64>>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(inner)),
            compressed_bytes_written,
        }
    }
}

impl<W: Write> Write for SharedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let mut writer = self.inner.borrow_mut();
        let bytes_written = writer.write(buf)?;
        self.compressed_bytes_written
            .set(self.compressed_bytes_written.get() + bytes_written as u64);
        Ok(bytes_written)
    }

    fn flush(&mut self) -> Result<()> {
        let mut writer = self.inner.borrow_mut();
        writer.flush()
    }
}

impl<W: Write> FinishableWriter for SharedWriter<W> {
    fn finish(mut self: Box<Self>) -> Result<()> {
        (*self).flush()
    }
}

impl<'writer> FinishableWriter for Box<dyn FinishableWriter + 'writer> {
    fn finish(self: Box<Self>) -> Result<()> {
        (*self).finish()
    }
}

struct DummyWriter;

impl Write for DummyWriter {
    fn write(&mut self, _buf: &[u8]) -> Result<usize> {
        unimplemented!()
    }

    fn flush(&mut self) -> Result<()> {
        unimplemented!()
    }
}

impl FinishableWriter for DummyWriter {
    fn finish(self: Box<Self>) -> Result<()> {
        unimplemented!()
    }
}

/// Configuration options for XZ compression.
#[derive(Debug, Clone)]
pub struct XZOptions {
    /// LZMA2 compression options.
    pub lzma2_options: LZMAOptions,
    /// Checksum type to use.
    pub check_type: CheckType,
    /// Maximum uncompressed size for each block (None = single block).
    pub block_size: Option<NonZeroU64>,
    /// Pre-filter to use (at most 3).
    pub filters: Vec<FilterConfig>,
}

impl Default for XZOptions {
    fn default() -> Self {
        Self {
            lzma2_options: LZMAOptions::default(),
            check_type: CheckType::Crc32,
            block_size: None,
            filters: Vec::new(),
        }
    }
}

impl XZOptions {
    /// Create options with specific preset and checksum type.
    pub fn with_preset(preset: u32) -> Self {
        Self {
            lzma2_options: LZMAOptions::with_preset(preset),
            check_type: CheckType::Crc64,
            block_size: None,
            filters: Vec::new(),
        }
    }

    /// Set the checksum type to use (Default is CRC64).
    pub fn set_check_sum_type(&mut self, check_type: CheckType) {
        self.check_type = check_type;
    }

    /// Set the maximum block size (None means a single block, which is the default).
    pub fn set_block_size(&mut self, block_size: Option<NonZeroU64>) {
        self.block_size = block_size;
    }

    /// Prepend a filter to the chain. You can prepend at most 3 additional filter.
    pub fn prepend_pre_filter(&mut self, filter_type: FilterType, property: u32) {
        self.filters.insert(
            0,
            FilterConfig {
                filter_type,
                property,
            },
        );
    }
}

/// A single-threaded XZ compressor.
pub struct XZWriter<'writer, W: Write> {
    writer: Box<dyn FinishableWriter + 'writer>,
    options: XZOptions,
    index_records: Vec<IndexRecord>,
    block_uncompressed_size: u64,
    checksum_calculator: ChecksumCalculator,
    header_written: bool,
    finished: bool,
    total_uncompressed_pos: u64,
    current_block_start_pos: u64,
    compressed_bytes_written: Rc<Cell<u64>>,
    original_writer: Rc<RefCell<W>>,
}

impl<'writer, W: Write + 'writer> XZWriter<'writer, W> {
    /// Create a new XZ writer with the given options.
    pub fn new(inner: W, options: XZOptions) -> Result<Self> {
        let mut options = options;

        if options.filters.len() > 3 {
            return Err(error_invalid_input(
                "XZ allows only at most 3 pre-filters plus LZMA2",
            ));
        }

        // Last filter is always LZMA2.
        options.filters.push(FilterConfig {
            filter_type: FilterType::LZMA2,
            property: 0,
        });

        let checksum_calculator = ChecksumCalculator::new(options.check_type);
        let compressed_bytes_written = Rc::new(Cell::new(0));
        let original_writer = Rc::new(RefCell::new(inner));

        let writer = Box::new(SharedWriter {
            inner: Rc::clone(&original_writer),
            compressed_bytes_written: Rc::clone(&compressed_bytes_written),
        });

        Ok(Self {
            writer,
            compressed_bytes_written,
            original_writer,
            options,
            index_records: Vec::new(),
            block_uncompressed_size: 0,
            checksum_calculator,
            header_written: false,
            finished: false,
            total_uncompressed_pos: 0,
            current_block_start_pos: 0,
        })
    }

    /// Consume the XZWriter and return the inner writer.
    pub fn into_inner(self) -> W {
        let Self {
            writer,
            original_writer,
            ..
        } = self;

        drop(writer);

        match Rc::try_unwrap(original_writer) {
            Ok(refcell) => refcell.into_inner(),
            Err(_) => {
                panic!("failed to unwrap original writer - other references exist");
            }
        }
    }

    /// Write the XZ stream header
    fn write_stream_header(&mut self) -> Result<()> {
        if self.header_written {
            return Ok(());
        }

        self.writer.write_all(&XZ_MAGIC)?;

        let stream_flags = [0u8, self.options.check_type as u8];
        self.writer.write_all(&stream_flags)?;

        let crc = CRC32.checksum(&stream_flags);
        self.writer.write_all(&crc.to_le_bytes())?;

        self.header_written = true;

        Ok(())
    }

    fn prepare_next_block(&mut self) -> Result<()> {
        self.writer = Box::new(SharedWriter {
            inner: Rc::clone(&self.original_writer),
            compressed_bytes_written: Rc::clone(&self.compressed_bytes_written),
        });

        self.write_block_header()?;

        self.current_block_start_pos = self.compressed_bytes_written.get();

        let mut chain_writer: Box<dyn FinishableWriter + 'writer> =
            core::mem::replace(&mut self.writer, Box::new(DummyWriter));

        for filter_config in self.options.filters.iter().rev() {
            chain_writer = match filter_config.filter_type {
                FilterType::Delta => {
                    let distance = filter_config.property as usize;
                    Box::new(DeltaWriter::new(chain_writer, distance))
                }
                FilterType::BcjX86 => {
                    let start_offset = filter_config.property as usize;
                    Box::new(BCJWriter::new_x86(chain_writer, start_offset))
                }
                FilterType::BcjPPC => {
                    let start_offset = filter_config.property as usize;
                    Box::new(BCJWriter::new_ppc(chain_writer, start_offset))
                }
                FilterType::BcjIA64 => {
                    let start_offset = filter_config.property as usize;
                    Box::new(BCJWriter::new_ia64(chain_writer, start_offset))
                }
                FilterType::BcjARM => {
                    let start_offset = filter_config.property as usize;
                    Box::new(BCJWriter::new_arm(chain_writer, start_offset))
                }
                FilterType::BcjARMThumb => {
                    let start_offset = filter_config.property as usize;
                    Box::new(BCJWriter::new_arm_thumb(chain_writer, start_offset))
                }
                FilterType::BcjSPARC => {
                    let start_offset = filter_config.property as usize;
                    Box::new(BCJWriter::new_sparc(chain_writer, start_offset))
                }
                FilterType::BcjARM64 => {
                    let start_offset = filter_config.property as usize;
                    Box::new(BCJWriter::new_arm64(chain_writer, start_offset))
                }
                FilterType::BcjRISCV => {
                    let start_offset = filter_config.property as usize;
                    Box::new(BCJWriter::new_riscv(chain_writer, start_offset))
                }
                FilterType::LZMA2 => {
                    Box::new(LZMA2Writer::new(chain_writer, &self.options.lzma2_options))
                }
            };
        }

        self.writer = chain_writer;
        self.block_uncompressed_size = 0;

        Ok(())
    }

    fn should_finish_block(&self) -> bool {
        if let Some(block_size) = self.options.block_size {
            self.block_uncompressed_size >= block_size.get()
        } else {
            false
        }
    }

    fn add_padding(&mut self, padding_needed: usize) -> Result<()> {
        match padding_needed {
            1 => self.writer.write_all(&[0]),
            2 => self.writer.write_all(&[0, 0]),
            3 => self.writer.write_all(&[0, 0, 0]),
            _ => Ok(()),
        }
    }

    fn finish_current_block(&mut self) -> Result<()> {
        // Unwrap the filter chain and finish writing the block.
        let writer = core::mem::replace(
            &mut self.writer,
            Box::new(SharedWriter {
                inner: Rc::clone(&self.original_writer),
                compressed_bytes_written: Rc::clone(&self.compressed_bytes_written),
            }),
        );
        writer.finish()?;

        // self.writer is now the inner writer.
        let block_compressed_size =
            self.compressed_bytes_written.get() - self.current_block_start_pos;

        let data_size = block_compressed_size;
        let padding_needed = (4 - (data_size % 4)) % 4;

        self.add_padding(padding_needed as usize)?;
        self.write_block_checksum()?;

        let unpadded_size = block_compressed_size + self.get_checksum_size();
        self.index_records.push(IndexRecord {
            unpadded_size,
            uncompressed_size: self.block_uncompressed_size,
        });

        self.block_uncompressed_size = 0;

        Ok(())
    }

    fn write_block_header(&mut self) -> Result<()> {
        let mut header_data = Vec::new();

        let num_filters = self.options.filters.len();

        if num_filters > 4 {
            return Err(error_invalid_input("too many filters in chain (maximum 4)"));
        }

        // Block flags: no compressed size, no uncompressed size, filter count
        let block_flags = (num_filters - 1) as u8; // -1 because 0 means 1 filter, 3 means 4 filters
        header_data.push(block_flags);

        let mut temp_buf = [0u8; 10];

        for filter_config in &self.options.filters {
            // Write filter ID.
            let filter_id = match filter_config.filter_type {
                FilterType::Delta => 0x03,
                FilterType::BcjX86 => 0x04,
                FilterType::BcjPPC => 0x05,
                FilterType::BcjIA64 => 0x06,
                FilterType::BcjARM => 0x07,
                FilterType::BcjARMThumb => 0x08,
                FilterType::BcjSPARC => 0x09,
                FilterType::BcjARM64 => 0x0A,
                FilterType::BcjRISCV => 0x0B,
                FilterType::LZMA2 => 0x21,
            };
            let size = encode_multibyte_integer(filter_id, &mut temp_buf)?;
            header_data.extend_from_slice(&temp_buf[..size]);

            // Write filter properties.
            match filter_config.filter_type {
                FilterType::Delta => {
                    // Properties size (1 byte)
                    let size = encode_multibyte_integer(1, &mut temp_buf)?;
                    header_data.extend_from_slice(&temp_buf[..size]);
                    // Distance property (encoded as distance - 1)
                    let distance_prop = (filter_config.property - 1) as u8;
                    header_data.push(distance_prop);
                }
                FilterType::BcjX86
                | FilterType::BcjPPC
                | FilterType::BcjIA64
                | FilterType::BcjARM
                | FilterType::BcjARMThumb
                | FilterType::BcjSPARC
                | FilterType::BcjARM64
                | FilterType::BcjRISCV => {
                    if filter_config.property == 0 {
                        // No start offset.
                        let size = encode_multibyte_integer(0, &mut temp_buf)?;
                        header_data.extend_from_slice(&temp_buf[..size]);
                    } else {
                        // 4-byte start offset.
                        let size = encode_multibyte_integer(4, &mut temp_buf)?;
                        header_data.extend_from_slice(&temp_buf[..size]);
                        header_data.extend_from_slice(&filter_config.property.to_le_bytes());
                    }
                }
                FilterType::LZMA2 => {
                    let size = encode_multibyte_integer(1, &mut temp_buf)?;
                    header_data.extend_from_slice(&temp_buf[..size]);

                    let dict_size = self.options.lzma2_options.dict_size;
                    let dict_size_prop = self.encode_lzma2_dict_size(dict_size)?;
                    header_data.push(dict_size_prop);
                }
            }
        }

        // Calculate header size (including size byte and CRC32, rounded up to multiple of 4)
        let total_size_needed = 1 + header_data.len() + 4;
        let header_size = total_size_needed.div_ceil(4) * 4;
        let header_size_encoded = ((header_size / 4) - 1) as u8;

        self.writer.write_all(&[header_size_encoded])?;
        self.writer.write_all(&header_data)?;

        let padding_needed = header_size - 1 - header_data.len() - 4;
        self.add_padding(padding_needed)?;

        // Calculate and write CRC32 of header size byte + header data + padding
        let mut crc = CRC32.digest();
        crc.update(&[header_size_encoded]);
        crc.update(&header_data);

        match padding_needed {
            1 => crc.update(&[0]),
            2 => crc.update(&[0, 0]),
            3 => crc.update(&[0, 0, 0]),
            _ => {}
        }

        self.writer.write_all(&crc.finalize().to_le_bytes())?;

        Ok(())
    }

    fn get_block_header_size(&self, _compressed_size: u64, _uncompressed_size: u64) -> u64 {
        // Block header: size_byte(1) + flags(1) + filter_id(1) + props_size(1)
        // + dict_prop(1) + padding + crc32(4)
        let base_size: u64 = 9;
        base_size.div_ceil(4) * 4
    }

    fn get_checksum_size(&self) -> u64 {
        match self.options.check_type {
            CheckType::None => 0,
            CheckType::Crc32 => 4,
            CheckType::Crc64 => 8,
            CheckType::Sha256 => 32,
        }
    }

    fn encode_lzma2_dict_size(&self, dict_size: u32) -> Result<u8> {
        if dict_size < 4096 {
            return Err(error_invalid_input("LZMA2 dictionary size too small"));
        }

        if dict_size == 0xFFFFFFFF {
            return Ok(40);
        }

        // Find the appropriate property value.
        for prop in 0u8..40 {
            let base = 2 | ((prop & 1) as u32);
            let size = base << (prop / 2 + 11);

            if size >= dict_size {
                return Ok(prop);
            }
        }

        Err(error_invalid_input("LZMA2 dictionary size too large"))
    }

    fn write_block_checksum(&mut self) -> Result<()> {
        let checksum = self.take_checksum();
        self.writer.write_all(&checksum)?;

        // Reset checksum calculator for next block.
        self.checksum_calculator = ChecksumCalculator::new(self.options.check_type);

        Ok(())
    }

    fn take_checksum(&mut self) -> Vec<u8> {
        let calculator = core::mem::replace(
            &mut self.checksum_calculator,
            ChecksumCalculator::new(self.options.check_type),
        );

        match calculator {
            ChecksumCalculator::None => Vec::new(),
            ChecksumCalculator::Crc32(crc) => crc.finalize().to_le_bytes().to_vec(),
            ChecksumCalculator::Crc64(crc) => crc.finalize().to_le_bytes().to_vec(),
            ChecksumCalculator::Sha256(sha) => sha.finalize().to_vec(),
        }
    }

    fn write_index(&mut self) -> Result<()> {
        // Index indicator (0x00).
        self.writer.write_all(&[0x00])?;

        let mut index_data = Vec::new();

        let mut temp_buf = [0u8; 10];
        let size = encode_multibyte_integer(self.index_records.len() as u64, &mut temp_buf)?;
        index_data.extend_from_slice(&temp_buf[..size]);

        for record in &self.index_records {
            let size = encode_multibyte_integer(record.unpadded_size, &mut temp_buf)?;
            index_data.extend_from_slice(&temp_buf[..size]);

            let size = encode_multibyte_integer(record.uncompressed_size, &mut temp_buf)?;
            index_data.extend_from_slice(&temp_buf[..size]);
        }

        self.writer.write_all(&index_data)?;

        let bytes_written = 1 + index_data.len(); // indicator + index data
        let padding_needed = (4 - (bytes_written % 4)) % 4;
        self.add_padding(padding_needed)?;

        let mut crc = CRC32.digest();
        crc.update(&[0x00]);
        crc.update(&index_data);

        match padding_needed {
            1 => crc.update(&[0]),
            2 => crc.update(&[0, 0]),
            3 => crc.update(&[0, 0, 0]),
            _ => {}
        }

        self.writer.write_all(&crc.finalize().to_le_bytes())?;

        Ok(())
    }

    fn write_stream_footer(&mut self) -> Result<()> {
        // Calculate backward size (index size in 4-byte blocks).
        let mut index_size = 1; // indicator
        index_size += count_multibyte_integer_size_for_value(self.index_records.len() as u64);

        for record in &self.index_records {
            index_size += count_multibyte_integer_size_for_value(record.unpadded_size);
            index_size += count_multibyte_integer_size_for_value(record.uncompressed_size);
        }

        let padding_needed = (4 - (index_size % 4)) % 4;
        index_size += padding_needed;
        index_size += 4; // CRC32

        let backward_size = ((index_size / 4) - 1) as u32;

        // Stream flags (same as header).
        let stream_flags = [0u8, self.options.check_type as u8];

        // Calculate CRC32 of backward size + stream flags
        let mut crc = CRC32.digest();
        crc.update(&backward_size.to_le_bytes());
        crc.update(&stream_flags);

        self.writer.write_all(&crc.finalize().to_le_bytes())?;
        self.writer.write_all(&backward_size.to_le_bytes())?;
        self.writer.write_all(&stream_flags)?;
        self.writer.write_all(&XZ_FOOTER_MAGIC)?;

        Ok(())
    }

    /// Finish writing the XZ stream and return the inner writer.
    pub fn finish(mut self) -> Result<W> {
        if self.finished {
            return Ok(self.into_inner());
        }

        self.write_stream_header()?;
        self.finish_current_block()?;
        self.write_index()?;
        self.write_stream_footer()?;

        Ok(self.into_inner())
    }
}

impl<'writer, W: Write + 'writer> Write for XZWriter<'writer, W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        if self.finished {
            return Err(error_invalid_data("XZWriter already finished"));
        }

        self.write_stream_header()?;

        let mut total_written = 0;
        let mut remaining = buf;

        while !remaining.is_empty() {
            // Check if we need to start a new block.
            if self.should_finish_block() {
                self.finish_current_block()?;
            }

            // Check if we need to prepare the next block (either first block or after finishing one).
            if self.block_uncompressed_size == 0 {
                self.prepare_next_block()?;
            }

            let written = self.writer.write(remaining)?;

            self.checksum_calculator.update(&remaining[..written]);

            remaining = &remaining[written..];
            total_written += written;
            self.block_uncompressed_size += written as u64;
            self.total_uncompressed_pos += written as u64;
        }

        Ok(total_written)
    }

    fn flush(&mut self) -> Result<()> {
        self.writer.flush()
    }
}
