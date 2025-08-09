use core::num::NonZeroU64;

use super::{
    encode_dict_size, CRC32, HEADER_SIZE, LZIP_MAGIC, LZIP_VERSION, MAX_DICT_SIZE, MIN_DICT_SIZE,
    TRAILER_SIZE,
};
use crate::{
    enc::{LZMAOptions, LZMAWriter},
    error_invalid_data, ByteWriter, Result, Write,
};

/// Options for LZIP compression.
#[derive(Default, Debug, Clone)]
pub struct LZIPOptions {
    /// LZMA compression options (will be overridden partially to use LZMA-302eos defaults).
    pub lzma_options: LZMAOptions,
    /// The maximal size of a member. If not set, the whole data will be written in one member.
    pub member_size: Option<NonZeroU64>,
}

impl LZIPOptions {
    /// Create options with specific preset.
    pub fn with_preset(preset: u32) -> Self {
        Self {
            lzma_options: LZMAOptions::with_preset(preset),
            member_size: None,
        }
    }

    /// Set the maximum member size (None means a single member, which is the default).
    pub fn set_block_size(&mut self, member_size: Option<NonZeroU64>) {
        self.member_size = member_size;
    }
}

struct CountingWriter<W> {
    inner: W,
    bytes_written: u64,
}

impl<W> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            bytes_written: 0,
        }
    }

    fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let bytes_written = self.inner.write(buf)?;
        self.bytes_written += bytes_written as u64;
        Ok(bytes_written)
    }

    fn flush(&mut self) -> Result<()> {
        self.inner.flush()
    }
}

/// A single-threaded LZIP compressor.
pub struct LZIPWriter<W: Write> {
    inner: Option<W>,
    lzma_writer: Option<LZMAWriter<CountingWriter<W>>>,
    options: LZIPOptions,
    header_written: bool,
    finished: bool,
    crc_digest: crc::Digest<'static, u32, crc::Table<16>>,
    uncompressed_size: u64,
    member_start_pos: u64,
    current_member_uncompressed_size: u64,
}

impl<W: Write> LZIPWriter<W> {
    /// Create a new LZIP writer with the given options.
    pub fn new(inner: W, options: LZIPOptions) -> Result<Self> {
        let mut options = options;

        // Overwrite with LZMA-302eos defaults.
        options.lzma_options.lc = 3;
        options.lzma_options.lp = 0;
        options.lzma_options.pb = 2;
        options.lzma_options.dict_size = options
            .lzma_options
            .dict_size
            .clamp(MIN_DICT_SIZE, MAX_DICT_SIZE);

        Ok(Self {
            inner: Some(inner),
            lzma_writer: None,
            options,
            header_written: false,
            finished: false,
            crc_digest: CRC32.digest(),
            uncompressed_size: 0,
            member_start_pos: 0,
            current_member_uncompressed_size: 0,
        })
    }

    /// Consume the writer and return the inner writer.
    pub fn into_inner(mut self) -> W {
        if let Some(lzma_writer) = self.lzma_writer.take() {
            return lzma_writer.into_inner().into_inner();
        }

        self.inner.take().expect("inner writer not set")
    }

    /// Check if we should finish the current member and start a new one.
    fn should_finish_member(&self) -> bool {
        if let Some(member_size) = self.options.member_size {
            self.current_member_uncompressed_size >= member_size.get()
        } else {
            false
        }
    }

    /// Start a new LZIP member.
    fn start_new_member(&mut self) -> Result<()> {
        let mut writer = self.inner.take().expect("inner writer not set");

        self.member_start_pos = 0;

        writer.write_all(&LZIP_MAGIC)?;
        writer.write_all(&[LZIP_VERSION])?;

        let dict_size_byte = encode_dict_size(self.options.lzma_options.dict_size)?;
        writer.write_all(&[dict_size_byte])?;

        let counting_writer = CountingWriter::new(writer);

        let lzma_writer =
            LZMAWriter::new_no_header(counting_writer, &self.options.lzma_options, true)?;

        self.lzma_writer = Some(lzma_writer);
        self.header_written = true;
        self.current_member_uncompressed_size = 0;
        self.crc_digest = CRC32.digest();
        self.uncompressed_size = 0;

        Ok(())
    }

    fn write_header(&mut self) -> Result<()> {
        if self.header_written {
            return Ok(());
        }

        self.start_new_member()
    }

    /// Finish the current member by writing its trailer.
    fn finish_current_member(&mut self) -> Result<()> {
        let lzma_writer = self.lzma_writer.take().expect("lzma writer not set");

        let counting_writer = lzma_writer.finish()?;
        let compressed_size = counting_writer.bytes_written();
        let mut writer = counting_writer.into_inner();

        // Calculate member size: header + compressed data + trailer.
        let member_size = HEADER_SIZE as u64 + compressed_size + TRAILER_SIZE as u64;

        let crc_digest = core::mem::replace(&mut self.crc_digest, CRC32.digest());
        let computed_crc = crc_digest.finalize();
        writer.write_u32(computed_crc)?;
        writer.write_u64(self.uncompressed_size)?;
        writer.write_u64(member_size)?;

        self.inner = Some(writer);
        self.header_written = false;

        Ok(())
    }

    /// Finish writing the LZIP stream and return the inner writer.
    pub fn finish(mut self) -> Result<W> {
        if self.finished {
            return Ok(self.into_inner());
        }

        if !self.header_written {
            self.write_header()?;
        }

        self.finish_current_member()?;
        self.finished = true;

        Ok(self.into_inner())
    }
}

impl<W: Write> Write for LZIPWriter<W> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        if self.finished {
            return Err(error_invalid_data("LZIP writer already finished"));
        }

        if buf.is_empty() {
            return Ok(0);
        }

        let mut total_written = 0;
        let mut remaining = buf;

        while !remaining.is_empty() {
            if self.should_finish_member() && self.header_written {
                self.finish_current_member()?;
            }

            if !self.header_written {
                self.start_new_member()?;
            }

            let lzma_writer = self.lzma_writer.as_mut().expect("lzma writer not set");

            let bytes_to_write = if let Some(member_size) = self.options.member_size {
                let remaining_in_member = member_size
                    .get()
                    .saturating_sub(self.current_member_uncompressed_size);
                (remaining.len() as u64).min(remaining_in_member) as usize
            } else {
                remaining.len()
            };

            if bytes_to_write == 0 {
                self.finish_current_member()?;
                continue;
            }

            let bytes_written = lzma_writer.write(&remaining[..bytes_to_write])?;

            if bytes_written > 0 {
                self.crc_digest.update(&remaining[..bytes_written]);
                self.uncompressed_size += bytes_written as u64;
                self.current_member_uncompressed_size += bytes_written as u64;
                total_written += bytes_written;
                remaining = &remaining[bytes_written..];
            } else {
                break;
            }
        }

        Ok(total_written)
    }

    fn flush(&mut self) -> Result<()> {
        if let Some(ref mut lzma_writer) = self.lzma_writer {
            lzma_writer.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LZIPReader, Read};

    #[test]
    fn test_counting_writer() {
        let mut writer = CountingWriter::new(Vec::new());

        assert_eq!(writer.bytes_written(), 0);

        writer.write_all(b"hello").unwrap();
        assert_eq!(writer.bytes_written(), 5);

        writer.write_all(b" world").unwrap();
        assert_eq!(writer.bytes_written(), 11);

        let inner = writer.into_inner();
        assert_eq!(inner, b"hello world");
    }

    #[test]
    fn test_lzip_writer_basic() {
        let data = b"Hello, LZIP world!";
        let mut writer = LZIPWriter::new(Vec::new(), LZIPOptions::default()).unwrap();

        writer.write_all(data).unwrap();
        let compressed = writer.finish().unwrap();

        assert_eq!(&compressed[0..4], b"LZIP");
        assert_eq!(compressed[4], 1); // Version

        assert!(compressed.len() > HEADER_SIZE + TRAILER_SIZE);
    }

    #[test]
    fn test_lzip_writer_multi_member() {
        let data = b"A".repeat(1000);

        let mut options = LZIPOptions::default();
        options.set_block_size(Some(NonZeroU64::new(100).unwrap()));

        let mut writer = LZIPWriter::new(Vec::new(), options).unwrap();
        writer.write_all(&data).unwrap();
        let compressed = writer.finish().unwrap();

        let mut lzip_count = 0;
        for window in compressed.windows(4) {
            if window == b"LZIP" {
                lzip_count += 1;
            }
        }

        // With 1000 bytes and 100 bytes per member, expect 10 members.
        assert_eq!(lzip_count, 10);

        let mut reader = LZIPReader::new(compressed.as_slice()).unwrap();
        let mut decompressed = Vec::new();
        reader.read_to_end(&mut decompressed).unwrap();

        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_lzip_writer_single_vs_multi_member() {
        let data = b"Hello world! ".repeat(50);

        let mut single_writer = LZIPWriter::new(Vec::new(), LZIPOptions::default()).unwrap();
        single_writer.write_all(&data).unwrap();
        let single_compressed = single_writer.finish().unwrap();

        let mut options = LZIPOptions::default();
        options.set_block_size(Some(NonZeroU64::new(100).unwrap()));
        let mut multi_writer = LZIPWriter::new(Vec::new(), options).unwrap();
        multi_writer.write_all(&data).unwrap();
        let multi_compressed = multi_writer.finish().unwrap();

        assert!(multi_compressed.len() > single_compressed.len());

        let mut single_reader = LZIPReader::new(single_compressed.as_slice()).unwrap();
        let mut single_decompressed = Vec::new();
        single_reader.read_to_end(&mut single_decompressed).unwrap();

        let mut multi_reader = LZIPReader::new(multi_compressed.as_slice()).unwrap();
        let mut multi_decompressed = Vec::new();
        multi_reader.read_to_end(&mut multi_decompressed).unwrap();

        assert_eq!(single_decompressed, data);
        assert_eq!(multi_decompressed, data);
        assert_eq!(single_decompressed, multi_decompressed);
    }
}
