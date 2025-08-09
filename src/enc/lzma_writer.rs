use super::{
    encoder::{LZMAEncoder, LZMAEncoderModes},
    range_enc::RangeEncoder,
    LZMAOptions,
};
use crate::{error_invalid_input, error_unsupported, Write};

/// A single-threaded LZMA2 compressor.
pub struct LZMAWriter<W: Write> {
    rc: RangeEncoder<W>,
    lzma: LZMAEncoder,
    use_end_marker: bool,
    current_uncompressed_size: u64,
    expected_uncompressed_size: Option<u64>,
    props: u8,
    mode: LZMAEncoderModes,
}

impl<W: Write> LZMAWriter<W> {
    /// Creates a new LZMA writer with full control over formatting options.
    pub fn new(
        mut out: W,
        options: &LZMAOptions,
        use_header: bool,
        use_end_marker: bool,
        expected_uncompressed_size: Option<u64>,
    ) -> crate::Result<LZMAWriter<W>> {
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
        if let Some(preset_dict) = &options.preset_dict {
            if use_header {
                return Err(error_unsupported(
                    "Header is not supported with preset dict",
                ));
            }
            lzma.lz.set_preset_dict(options.dict_size, preset_dict);
        }

        let props = options.get_props();
        if use_header {
            out.write_all(&[props])?;
            let mut dict_size = options.dict_size;
            for _i in 0..4 {
                out.write_all(&[(dict_size & 0xFF) as u8])?;
                dict_size >>= 8;
            }
            let expected_compressed_size = expected_uncompressed_size.unwrap_or(u64::MAX);
            for i in 0..8 {
                out.write_all(&[((expected_compressed_size >> (i * 8)) & 0xFF) as u8])?;
            }
        }

        let rc = RangeEncoder::new(out);
        Ok(LZMAWriter {
            rc,
            lzma,
            use_end_marker,
            current_uncompressed_size: 0,
            expected_uncompressed_size,
            props,
            mode,
        })
    }

    /// Creates a new LZMA writer that includes a header with the specified input size.
    #[inline]
    pub fn new_use_header(
        out: W,
        options: &LZMAOptions,
        input_size: Option<u64>,
    ) -> crate::Result<Self> {
        Self::new(out, options, true, input_size.is_none(), input_size)
    }

    /// Creates a new LZMA writer without a header.
    #[inline]
    pub fn new_no_header(
        out: W,
        options: &LZMAOptions,
        use_end_marker: bool,
    ) -> crate::Result<Self> {
        Self::new(out, options, false, use_end_marker, None)
    }

    /// Returns the LZMA properties byte.
    #[inline]
    pub fn props(&self) -> u8 {
        self.props
    }

    /// Returns the number of uncompressed bytes written so far.
    #[inline]
    pub fn get_uncompressed_size(&self) -> u64 {
        self.current_uncompressed_size
    }

    /// Unwraps the writer, returning the underlying writer.
    pub fn into_inner(self) -> W {
        self.rc.into_inner()
    }

    /// Finishes the compression and returns the underlying writer.
    pub fn finish(mut self) -> crate::Result<W> {
        if let Some(exp) = self.expected_uncompressed_size {
            if exp != self.current_uncompressed_size {
                return Err(error_invalid_input(
                    "expected compressed size does not match actual compressed size",
                ));
            }
        }
        self.lzma.lz.set_finishing();
        self.lzma.encode_for_lzma1(&mut self.rc, &mut self.mode)?;
        if self.use_end_marker {
            self.lzma.encode_lzma1_end_marker(&mut self.rc)?;
        }
        self.rc.finish()?;

        let Self { rc, .. } = self;

        Ok(rc.into_inner())
    }
}

impl<W: Write> Write for LZMAWriter<W> {
    fn write(&mut self, buf: &[u8]) -> crate::Result<usize> {
        if let Some(exp) = self.expected_uncompressed_size {
            if exp < self.current_uncompressed_size + buf.len() as u64 {
                return Err(error_invalid_input(
                    "expected compressed size does not match actual compressed size",
                ));
            }
        }
        self.current_uncompressed_size += buf.len() as u64;
        let mut len = buf.len();
        let mut off = 0;
        while len > 0 {
            let used = self.lzma.lz.fill_window(&buf[off..]);
            off += used;
            len -= used;
            self.lzma.encode_for_lzma1(&mut self.rc, &mut self.mode)?;
        }

        Ok(off)
    }

    fn flush(&mut self) -> crate::Result<()> {
        Ok(())
    }
}
