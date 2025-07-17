use std::{io::Write, num::NonZeroU32};

use super::{
    encoder::{EncodeMode, LZMAEncoder, LZMAEncoderModes},
    lz::MFType,
    range_enc::{RangeEncoder, RangeEncoderBuffer},
};
use crate::enc::encoder::LZMAEncoderTrait;

#[derive(Debug, Clone)]
pub struct LZMA2Options {
    /// Option size of one stream. Used to allow multi-threaded compression & decompression.
    /// Essentially the encoded file is cut into `stream_size` slices that can be decoded & encoded
    /// in parallel. As a rule of thumb: The more streams a file has the less effective the
    /// compression ratio will be.
    ///
    /// If not set the data is compressed as one stream.
    pub stream_size: Option<NonZeroU32>,
    pub dict_size: u32,
    pub lc: u32,
    pub lp: u32,
    pub pb: u32,
    pub mode: EncodeMode,
    pub nice_len: u32,
    pub mf: MFType,
    pub depth_limit: i32,
    pub preset_dict: Option<Vec<u8>>,
}

impl Default for LZMA2Options {
    fn default() -> Self {
        Self::with_preset(6)
    }
}

impl LZMA2Options {
    pub const LC_DEFAULT: u32 = 3;

    pub const LP_DEFAULT: u32 = 0;

    pub const PB_DEFAULT: u32 = 2;

    pub const NICE_LEN_MAX: u32 = 273;

    pub const NICE_LEN_MIN: u32 = 8;

    pub const DICT_SIZE_DEFAULT: u32 = 8 << 20;

    const PRESET_TO_DICT_SIZE: &'static [u32] = &[
        1 << 18,
        1 << 20,
        1 << 21,
        1 << 22,
        1 << 22,
        1 << 23,
        1 << 23,
        1 << 24,
        1 << 25,
        1 << 26,
    ];

    const PRESET_TO_DEPTH_LIMIT: &'static [i32] = &[4, 8, 24, 48];

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stream_size: Option<NonZeroU32>,
        dict_size: u32,
        lc: u32,
        lp: u32,
        pb: u32,
        mode: EncodeMode,
        nice_len: u32,
        mf: MFType,
        depth_limit: i32,
    ) -> Self {
        Self {
            stream_size,
            dict_size,
            lc,
            lp,
            pb,
            mode,
            nice_len,
            mf,
            depth_limit,
            preset_dict: None,
        }
    }

    /// preset: [0..9]
    #[inline]
    pub fn with_preset(preset: u32) -> Self {
        let mut opt = Self {
            stream_size: None,
            dict_size: Default::default(),
            lc: Default::default(),
            lp: Default::default(),
            pb: Default::default(),
            mode: EncodeMode::Normal,
            nice_len: Default::default(),
            mf: Default::default(),
            depth_limit: Default::default(),
            preset_dict: Default::default(),
        };
        opt.set_preset(preset);
        opt
    }

    /// preset: [0..9]
    pub fn set_preset(&mut self, preset: u32) {
        let preset = preset.min(9);

        self.lc = Self::LC_DEFAULT;
        self.lp = Self::LP_DEFAULT;
        self.pb = Self::PB_DEFAULT;
        self.dict_size = Self::PRESET_TO_DICT_SIZE[preset as usize];
        if preset <= 3 {
            self.mode = EncodeMode::Fast;
            self.mf = MFType::HC4;
            self.nice_len = if preset <= 1 { 128 } else { Self::NICE_LEN_MAX };
            self.depth_limit = Self::PRESET_TO_DEPTH_LIMIT[preset as usize];
        } else {
            self.mode = EncodeMode::Normal;
            self.mf = MFType::BT4;
            self.nice_len = if preset == 4 {
                16
            } else if preset == 5 {
                32
            } else {
                64
            };
            self.depth_limit = 0;
        }
    }

    pub fn get_memory_usage(&self) -> u32 {
        let dict_size = self.dict_size;
        let extra_size_before = get_extra_size_before(dict_size);
        70 + LZMAEncoder::get_mem_usage(self.mode, dict_size, extra_size_before, self.mf)
    }

    #[inline(always)]
    pub fn get_props(&self) -> u8 {
        ((self.pb * 5 + self.lp) * 9 + self.lc) as u8
    }
}

const COMPRESSED_SIZE_MAX: u32 = 64 << 10;

pub fn get_extra_size_before(dict_size: u32) -> u32 {
    COMPRESSED_SIZE_MAX.saturating_sub(dict_size)
}

/// A single-threaded LZMA2 encoder.
///
/// # Examples
/// ```
/// use std::io::Write;
///
/// use lzma_rust2::{LZMA2Options, LZMA2Writer};
///
/// let mut writer = LZMA2Writer::new(Vec::new(), &LZMA2Options::default());
/// writer.write_all(b"some very long data...").unwrap();
/// let compressed_data = writer.finish().unwrap();
/// ```
pub struct LZMA2Writer<W: Write> {
    inner: W,
    rc: RangeEncoder<RangeEncoderBuffer>,
    lzma: LZMAEncoder,
    mode: LZMAEncoderModes,
    props: u8,
    dict_reset_needed: bool,
    state_reset_needed: bool,
    props_needed: bool,
    pending_size: u32,
    stream_size: Option<NonZeroU32>,
    current_stream_size: u32,
}

impl<W: Write> LZMA2Writer<W> {
    pub fn new(inner: W, options: &LZMA2Options) -> Self {
        let dict_size = options.dict_size;
        let rc = RangeEncoder::new_buffer(COMPRESSED_SIZE_MAX as usize);
        let (mut lzma, mode) = LZMAEncoder::new(
            options.mode,
            options.lc,
            options.lp,
            options.pb,
            options.mf,
            options.depth_limit,
            options.dict_size,
            options.nice_len as usize,
        );

        let props = options.get_props();
        let mut dict_reset_needed = true;
        if let Some(preset_dict) = &options.preset_dict {
            lzma.lz.set_preset_dict(dict_size, preset_dict);
            dict_reset_needed = false;
        }
        Self {
            inner,
            rc,
            lzma,
            mode,
            props,
            dict_reset_needed,
            state_reset_needed: true,
            props_needed: true,
            pending_size: 0,
            stream_size: options.stream_size,
            current_stream_size: 0,
        }
    }

    fn prepare_for_encode(&mut self) -> std::io::Result<()> {
        if self.dict_reset_needed {
            // A full dictionary reset is needed. Reset everything.
            self.lzma.reset(true, &mut self.mode);
            self.dict_reset_needed = false;
            self.state_reset_needed = false;
        } else if self.state_reset_needed {
            // A state-only reset is needed (e.g., after uncompressed chunk).
            self.lzma.reset(false, &mut self.mode);
            self.state_reset_needed = false;
        }
        Ok(())
    }

    fn write_lzma(&mut self, uncompressed_size: u32, compressed_size: u32) -> std::io::Result<()> {
        let mut control: u8;
        let mut chunk_header = [0u8; 6];
        let mut header_len = 5;

        if self.props_needed {
            control = 0x80 | (3 << 5);
            chunk_header[5] = self.props;
            header_len = 6;
            self.props_needed = false;
        } else if self.state_reset_needed {
            control = 0x80 | (2 << 5);
            self.state_reset_needed = false;
        } else {
            control = 0x80;
        }
        control |= ((uncompressed_size - 1) >> 16) as u8;

        chunk_header[0] = control;
        chunk_header[1] = ((uncompressed_size - 1) >> 8) as u8;
        chunk_header[2] = (uncompressed_size - 1) as u8;
        chunk_header[3] = ((compressed_size - 1) >> 8) as u8;
        chunk_header[4] = (compressed_size - 1) as u8;

        self.inner.write_all(&chunk_header[..header_len])?;
        self.rc.write_to(&mut self.inner)?;

        Ok(())
    }

    fn write_uncompressed(&mut self, uncompressed_size: u32) -> std::io::Result<()> {
        let mut remaining = uncompressed_size;
        while remaining > 0 {
            let chunk_size = remaining.min(COMPRESSED_SIZE_MAX);
            let mut chunk_header = [0u8; 3];

            // The first uncompressed chunk after a reset signal needs to carry that signal.
            chunk_header[0] = if self.state_reset_needed { 0x01 } else { 0x02 };
            chunk_header[1] = ((chunk_size - 1) >> 8) as u8;
            chunk_header[2] = (chunk_size - 1) as u8;
            self.inner.write_all(&chunk_header)?;

            self.lzma.lz.copy_uncompressed(
                &mut self.inner,
                chunk_size as i32,
                chunk_size as usize,
            )?;
            remaining -= chunk_size;

            // Only the first chunk carries the reset signal. Subsequent ones are normal.
            self.state_reset_needed = false;
        }

        // After any uncompressed data, the next LZMA chunk must reset its state.
        // But the dictionary is preserved. This is a "soft" reset.
        self.state_reset_needed = true;
        Ok(())
    }

    fn write_chunk(&mut self) -> std::io::Result<()> {
        let compressed_size = self.rc.finish_buffer()?.unwrap_or_default() as u32;
        let uncompressed_size = self.lzma.data.uncompressed_size;

        debug_assert!(uncompressed_size > 0);

        if compressed_size > 0 && compressed_size + 2 < uncompressed_size {
            self.write_lzma(uncompressed_size, compressed_size)?;
        } else {
            // The uncompressed data is smaller. We need to reset the LZMA state
            // because we are interrupting it to write an uncompressed block.
            self.lzma.reset(false, &mut self.mode);
            self.write_uncompressed(uncompressed_size)?;
        }

        self.pending_size -= uncompressed_size;
        self.lzma.reset_uncompressed_size();
        self.rc.reset_buffer();

        // If stream_size is configured, check if we need to start a new stream.
        if let Some(stream_size) = self.stream_size {
            self.current_stream_size += uncompressed_size;

            if self.current_stream_size >= stream_size.get() {
                // The *next* chunk must start a new stream.
                // Signal that a full dictionary and props reset is needed.
                self.dict_reset_needed = true;
                self.props_needed = true;
                self.state_reset_needed = true;
                self.current_stream_size = 0;
            }
        }

        Ok(())
    }

    pub fn inner(&mut self) -> &mut W {
        &mut self.inner
    }

    pub fn finish(mut self) -> std::io::Result<W> {
        self.lzma.lz.set_finishing();

        while self.pending_size > 0 {
            self.prepare_for_encode()?;
            if self.lzma.encode_for_lzma2(&mut self.rc, &mut self.mode)? {
                self.write_chunk()?;
            }
        }

        self.inner.write_all(&[0x00])?;

        Ok(self.inner)
    }
}

impl<W: Write> Write for LZMA2Writer<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut len = buf.len();
        let mut off = 0;
        while len > 0 {
            self.prepare_for_encode()?;

            let used = self.lzma.lz.fill_window(&buf[off..(off + len)]);
            off += used;
            len -= used;
            self.pending_size += used as u32;

            if self.lzma.encode_for_lzma2(&mut self.rc, &mut self.mode)? {
                self.write_chunk()?;
            }
        }
        Ok(off)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.lzma.lz.set_flushing();

        while self.pending_size > 0 {
            self.prepare_for_encode()?;
            if self.lzma.encode_for_lzma2(&mut self.rc, &mut self.mode)? {
                self.write_chunk()?;
            }
        }

        self.inner.flush()
    }
}
