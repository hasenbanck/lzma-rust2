use alloc::{boxed::Box, rc::Rc, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    num::NonZeroU64,
};

use super::{
    add_padding, write_xz_block_header, write_xz_index, write_xz_stream_footer,
    write_xz_stream_header, CheckType, ChecksumCalculator, FilterConfig, FilterType, IndexRecord,
};
use crate::{
    enc::{LZMA2Writer, LZMAOptions},
    error_invalid_data, error_invalid_input,
    filter::{bcj::BCJWriter, delta::DeltaWriter},
    LZMA2Options, Result, Write,
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
    /// LZMA compression options.
    pub lzma_options: LZMAOptions,
    /// Checksum type to use.
    pub check_type: CheckType,
    /// Maximum uncompressed size for each block (None = single block).
    /// Will get clamped to be at least the dict size to not waste memory.
    pub block_size: Option<NonZeroU64>,
    /// Pre-filter to use (at most 3).
    pub filters: Vec<FilterConfig>,
}

impl Default for XZOptions {
    fn default() -> Self {
        Self {
            lzma_options: LZMAOptions::default(),
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
            lzma_options: LZMAOptions::with_preset(preset),
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
    current_block_header_size: u64,
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

        if let Some(block_size) = options.block_size.as_mut() {
            *block_size =
                NonZeroU64::new(block_size.get().max(options.lzma_options.dict_size as u64))
                    .expect("block size is zero");
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
            current_block_header_size: 0,
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

    fn write_stream_header(&mut self) -> Result<()> {
        if self.header_written {
            return Ok(());
        }

        write_xz_stream_header(&mut *self.writer, self.options.check_type)?;

        self.header_written = true;

        Ok(())
    }

    fn prepare_next_block(&mut self) -> Result<()> {
        self.writer = Box::new(SharedWriter {
            inner: Rc::clone(&self.original_writer),
            compressed_bytes_written: Rc::clone(&self.compressed_bytes_written),
        });

        self.current_block_header_size = write_xz_block_header(
            &mut *self.writer,
            &self.options.filters,
            self.options.lzma_options.dict_size,
        )?;

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
                    let options = LZMA2Options {
                        lzma_options: self.options.lzma_options.clone(),
                        ..Default::default()
                    };
                    Box::new(LZMA2Writer::new(chain_writer, options))
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

        add_padding(&mut *self.writer, padding_needed as usize)?;

        self.write_block_checksum()?;

        let unpadded_size = self.current_block_header_size
            + block_compressed_size
            + self.options.check_type.checksum_size();

        self.index_records.push(IndexRecord {
            unpadded_size,
            uncompressed_size: self.block_uncompressed_size,
        });

        self.block_uncompressed_size = 0;

        Ok(())
    }

    fn get_block_header_size(&self, _compressed_size: u64, _uncompressed_size: u64) -> u64 {
        // Block header: size_byte(1) + flags(1) + filter_id(1) + props_size(1)
        // + dict_prop(1) + padding + crc32(4)
        let base_size: u64 = 9;
        base_size.div_ceil(4) * 4
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
        calculator.finalize_to_bytes()
    }

    /// Finish writing the XZ stream and return the inner writer.
    pub fn finish(mut self) -> Result<W> {
        if self.finished {
            return Ok(self.into_inner());
        }

        self.write_stream_header()?;
        self.finish_current_block()?;

        write_xz_index(&mut *self.writer, &self.index_records)?;

        write_xz_stream_footer(
            &mut *self.writer,
            &self.index_records,
            self.options.check_type,
        )?;

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

            let max_write_size = match self.options.block_size {
                Some(block_size) => {
                    let remaining_capacity = block_size
                        .get()
                        .saturating_sub(self.block_uncompressed_size);
                    remaining.len().min(remaining_capacity as usize)
                }
                None => remaining.len(),
            };

            if max_write_size == 0 {
                // Block is full, finish it and continue.
                continue;
            }

            let chunk_to_write = &remaining[..max_write_size];
            let written = self.writer.write(chunk_to_write)?;

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
