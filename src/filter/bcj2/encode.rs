use alloc::{boxed::Box, vec};

use super::{BCJ2_NUM_STREAMS, BIT_MODEL_TOTAL};
use crate::{StickyError, Write, enc::range_enc::RangeEncoder, error_invalid_input};

const OUTPUT_BUF_SIZE: usize = 1 << 14;

/// Encoding options for the BCJ2 x86 branch converter.
#[derive(Debug, Clone)]
pub struct Bcj2Options {
    /// Expected input size, if known.
    ///
    /// Only branches targeting a byte within the expected input are converted.
    /// The writer rejects input longer than this size and checks the exact size
    /// when finished. None disables the target range check.
    pub uncompressed_size: Option<u64>,
    /// Limit for signed relative branch offsets, in bytes.
    ///
    /// Offsets in `-relative_limit..relative_limit` can be converted. Zero
    /// disables conversion. The maximum is [`Self::RELATIVE_LIMIT_MAX`].
    pub relative_limit: u32,
}

impl Bcj2Options {
    /// Default relative offset limit (240 MiB).
    pub const RELATIVE_LIMIT_DEFAULT: u32 = 0x0F00_0000;

    /// Maximum relative offset limit (2 GiB).
    pub const RELATIVE_LIMIT_MAX: u32 = 1 << 31;
}

impl Default for Bcj2Options {
    fn default() -> Self {
        Self {
            uncompressed_size: None,
            relative_limit: Self::RELATIVE_LIMIT_DEFAULT,
        }
    }
}

/// A streaming BCJ2 encoder with separate MAIN, CALL, JUMP and RC outputs.
///
/// The four outputs contain raw, uncompressed BCJ2 streams. Both the input and
/// decoded output start at position zero. Call [`Self::finish`] to encode the
/// final instruction bytes and terminate the RC stream, even for empty input.
/// `flush` retains incomplete instructions and does not terminate the stream.
///
/// Internal buffering is limited to four 16 KiB output buffers and four held
/// input bytes. An output error ends encoding and is reported again on later
/// writes and flushes. Output written before that error cannot be rolled back.
/// Dropping the writer does not finish encoding.
///
/// # Example
///
/// ```
/// # extern crate alloc;
/// # use alloc::vec::Vec;
/// # #[cfg(feature = "std")]
/// # use std::io::{Read, Write};
/// # #[cfg(not(feature = "std"))]
/// # use lzma_rust2::{Read, Write};
/// use lzma_rust2::filter::bcj2::{Bcj2Options, Bcj2Reader, Bcj2Writer};
///
/// let input = [0x90, 0xE8, 4, 0, 0, 0];
/// let outputs = core::array::from_fn(|_| Vec::new());
/// let mut writer = Bcj2Writer::new(outputs, &Bcj2Options::default()).unwrap();
/// writer.write_all(&input).unwrap();
/// let streams = writer.finish().unwrap();
///
/// let inputs = streams.iter().map(Vec::as_slice).collect();
/// let mut reader = Bcj2Reader::try_new(inputs, input.len() as u64).unwrap();
/// let mut decoded = [0; 6];
/// reader.read_exact(&mut decoded).unwrap();
/// reader.finish().unwrap();
/// assert_eq!(decoded, input);
/// ```
pub struct Bcj2Writer<W: Write> {
    encoder: Bcj2Encoder<W>,
    pending: [u8; 5],
    pending_size: usize,
    uncompressed_size: u64,
    failure: Option<StickyError>,
}

impl<W: Write> Bcj2Writer<W> {
    /// Creates a writer with outputs ordered MAIN, CALL, JUMP and RC.
    pub fn new(outputs: [W; BCJ2_NUM_STREAMS], options: &Bcj2Options) -> crate::Result<Self> {
        if options.relative_limit > Bcj2Options::RELATIVE_LIMIT_MAX {
            return Err(error_invalid_input(
                "BCJ2 relative offset limit is too large",
            ));
        }
        Ok(Self {
            encoder: Bcj2Encoder::new(outputs, options.clone()),
            pending: [0; 5],
            pending_size: 0,
            uncompressed_size: 0,
            failure: None,
        })
    }

    /// Returns the number of input bytes accepted, including held instruction bytes.
    pub fn get_uncompressed_size(&self) -> u64 {
        self.uncompressed_size
    }

    /// Unwraps the writer, returning the outputs without finishing or flushing.
    ///
    /// Buffered output and held input are discarded. Use [`Self::finish`] to
    /// produce complete streams. This method can recover the outputs after an
    /// error, but their partial contents do not form a complete BCJ2 stream.
    pub fn into_inner(self) -> [W; BCJ2_NUM_STREAMS] {
        self.encoder.into_inner()
    }

    /// Finishes encoding, flushes the outputs and returns them in their original order.
    ///
    /// If the outputs are compressors, finish each returned compressor as
    /// well: flushing an output does not end that compressor's stream.
    pub fn finish(mut self) -> crate::Result<[W; BCJ2_NUM_STREAMS]> {
        self.finish_stream()?;
        Ok(self.encoder.into_inner())
    }

    /// Finishes the current stream and resets the encoder, retaining its buffers.
    ///
    /// Returns mutable outputs in MAIN, CALL, JUMP and RC order. Clear, rewind or
    /// replace them before writing the next independent stream: resetting the
    /// encoder does not change the outputs' contents or positions. The options
    /// apply to each stream, and the accepted input size returns to zero.
    /// Truncate rewound files if the next output is shorter. Finish and replace
    /// compressor outputs before starting another stream.
    /// A finishing error persists on subsequent operations.
    pub fn finish_and_reset(&mut self) -> crate::Result<[&mut W; BCJ2_NUM_STREAMS]> {
        if let Err(error) = self.finish_stream() {
            return Err(self.fail(error));
        }
        self.pending_size = 0;
        self.uncompressed_size = 0;
        self.encoder.ip = 0;
        self.encoder.prev_byte = 0;
        self.encoder.probs.fill(BIT_MODEL_TOTAL >> 1);
        self.encoder.rc.reset();
        let [main, call, jump] = &mut self.encoder.outputs;
        Ok([
            &mut main.inner,
            &mut call.inner,
            &mut jump.inner,
            &mut self.encoder.rc.inner_mut().inner,
        ])
    }

    fn finish_stream(&mut self) -> crate::Result<()> {
        self.check_failure()?;
        if let Some(expected) = self.encoder.options.uncompressed_size {
            if expected != self.uncompressed_size {
                return Err(error_invalid_input(
                    "BCJ2 input size does not match expected size",
                ));
            }
        }
        self.encoder
            .encode(&self.pending[..self.pending_size], true)?;
        self.encoder.rc.finish()?;
        self.encoder.flush()
    }

    fn check_failure(&self) -> crate::Result<()> {
        match &self.failure {
            Some(failure) => Err(failure.report()),
            None => Ok(()),
        }
    }

    fn write_encode(&mut self, mut buf: &[u8]) -> crate::Result<()> {
        while self.pending_size != 0 && !buf.is_empty() {
            let size = buf.len().min(self.pending.len() - self.pending_size);
            self.pending[self.pending_size..self.pending_size + size].copy_from_slice(&buf[..size]);
            self.pending_size += size;
            buf = &buf[size..];
            let consumed = self
                .encoder
                .encode(&self.pending[..self.pending_size], false)?;
            self.pending.copy_within(consumed..self.pending_size, 0);
            self.pending_size -= consumed;
        }
        if !buf.is_empty() {
            let consumed = self.encoder.encode(buf, false)?;
            self.pending_size = buf.len() - consumed;
            self.pending[..self.pending_size].copy_from_slice(&buf[consumed..]);
        }
        Ok(())
    }

    fn fail(&mut self, error: crate::Error) -> crate::Error {
        let failure = StickyError::new(error);
        let reported = failure.report();
        self.failure = Some(failure);
        reported
    }
}

impl<W: Write> Write for Bcj2Writer<W> {
    fn write(&mut self, buf: &[u8]) -> crate::Result<usize> {
        self.check_failure()?;
        let size = self
            .uncompressed_size
            .checked_add(buf.len() as u64)
            .ok_or_else(|| error_invalid_input("BCJ2 input size overflow"))?;
        if let Some(expected) = self.encoder.options.uncompressed_size {
            if size > expected {
                return Err(error_invalid_input("BCJ2 input exceeds expected size"));
            }
        }
        if let Err(error) = self.write_encode(buf) {
            return Err(self.fail(error));
        }
        self.uncompressed_size = size;
        Ok(buf.len())
    }

    fn flush(&mut self) -> crate::Result<()> {
        self.check_failure()?;
        if let Err(error) = self.encoder.flush() {
            return Err(self.fail(error));
        }
        Ok(())
    }
}

struct Bcj2Encoder<W: Write> {
    outputs: [BufferedOutput<W>; 3],
    rc: RangeEncoder<BufferedOutput<W>>,
    options: Bcj2Options,
    ip: u64,
    prev_byte: u8,
    probs: [u16; 2 + 256],
}

impl<W: Write> Bcj2Encoder<W> {
    fn new(outputs: [W; BCJ2_NUM_STREAMS], options: Bcj2Options) -> Self {
        let [main, call, jump, rc] = outputs;
        Self {
            outputs: [main, call, jump].map(BufferedOutput::new),
            rc: RangeEncoder::new(BufferedOutput::new(rc)),
            options,
            ip: 0,
            prev_byte: 0,
            probs: [BIT_MODEL_TOTAL >> 1; 2 + 256],
        }
    }

    fn into_inner(self) -> [W; BCJ2_NUM_STREAMS] {
        let [main, call, jump] = self.outputs;
        [
            main.inner,
            call.inner,
            jump.inner,
            self.rc.into_inner().inner,
        ]
    }

    fn flush(&mut self) -> crate::Result<()> {
        for output in &mut self.outputs {
            output.flush()?;
        }
        self.rc.inner_mut().flush()
    }

    #[inline]
    fn encode(&mut self, buf: &[u8], finish: bool) -> crate::Result<usize> {
        // A block scan follows one scalar byte; short calls need no scan bookkeeping.
        if buf.len() >= 17 {
            self.encode_inner::<true>(buf, finish)
        } else {
            self.encode_inner::<false>(buf, finish)
        }
    }

    #[inline(never)]
    fn encode_inner<const SCAN: bool>(&mut self, buf: &[u8], finish: bool) -> crate::Result<usize> {
        let mut offset = 0;
        let mut ip = self.ip;
        let mut start = 0;
        let mut prev = self.prev_byte;
        let mut scan_end = if buf.len() >= 17 { 0 } else { buf.len() };
        'input: loop {
            let byte = 'marker: loop {
                if offset >= buf.len() {
                    break 'input;
                }
                let byte = buf[offset];
                let marker = (byte & 0xFE) == 0xE8 || (prev == 0x0F && (byte & 0xF0) == 0x80);
                if marker {
                    break 'marker byte;
                }
                prev = byte;
                offset += 1;
                if SCAN && byte != 0x0F && offset >= scan_end {
                    scan_end = buf.len();
                    while buf.len() - offset >= 16 {
                        if buf.len() - offset >= 32 {
                            let block = &buf[offset - 1..offset + 32];
                            let size = super::literal_prefix_exact(block.try_into().unwrap());
                            if size != 32 {
                                prev = block[size];
                                offset += size;
                                scan_end = offset + 16;
                                break 'marker buf[offset];
                            }
                            prev = block[32];
                            offset += 32;
                            while buf.len() - offset >= 64 {
                                let window: &[u8; 65] =
                                    buf[offset - 1..offset + 64].try_into().unwrap();
                                let size = super::literal_prefix_64(window);
                                if size != 64 {
                                    prev = window[size];
                                    offset += size;
                                    scan_end = offset + 16;
                                    break 'marker buf[offset];
                                }
                                prev = window[64];
                                offset += 64;
                            }
                        } else {
                            let block = &buf[offset - 1..offset + 16];
                            let size = super::first_marker(block.try_into().unwrap());
                            if size != 16 {
                                prev = block[size];
                                offset += size;
                                scan_end = offset + 16;
                                break 'marker buf[offset];
                            }
                            prev = block[16];
                            offset += 16;
                        }
                    }
                }
            };
            if buf.len() - offset < 5 && !finish {
                break;
            }
            self.outputs[0].write_main(&buf[start..offset + 1], byte)?;
            ip += (offset + 1 - start) as u64;
            let mut relative = 0;
            let mut convert = false;
            if buf.len() - offset >= 5 {
                relative = u32::from_le_bytes(buf[offset + 1..offset + 5].try_into().unwrap());
                // Test the signed interval [-limit, limit) with wrapping unsigned arithmetic.
                convert = (relative.wrapping_add(self.options.relative_limit) >> 1)
                    < self.options.relative_limit;
                if let Some(size) = self.options.uncompressed_size {
                    let signed = relative as i32 as i64;
                    convert &= ip
                        .checked_add_signed(signed + 4)
                        .is_some_and(|target| target < size);
                }
            }
            let index = if byte == 0xE8 {
                2 + prev as usize
            } else if byte == 0xE9 {
                1
            } else {
                0
            };
            if convert {
                self.rc.encode_bit(&mut self.probs, index, 1)?;
                ip += 4;
                let target = (ip as u32).wrapping_add(relative);
                let stream = if byte == 0xE8 { 1 } else { 2 };
                self.outputs[stream].write_u32_be(target)?;
                prev = (relative >> 24) as u8;
                offset += 5;
            } else {
                self.rc.encode_bit(&mut self.probs, index, 0)?;
                prev = byte;
                offset += 1;
            }
            start = offset;
        }
        self.outputs[0].write_all(&buf[start..offset])?;
        ip += (offset - start) as u64;
        self.ip = ip;
        self.prev_byte = prev;
        Ok(offset)
    }
}

struct BufferedOutput<W> {
    inner: W,
    buffer: Box<[u8; OUTPUT_BUF_SIZE]>,
    pos: usize,
}

impl<W: Write> BufferedOutput<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            buffer: vec![0; OUTPUT_BUF_SIZE]
                .into_boxed_slice()
                .try_into()
                .unwrap(),
            pos: 0,
        }
    }

    #[cold]
    #[inline(never)]
    fn flush_buffer(&mut self) -> crate::Result<()> {
        self.inner.write_all(&self.buffer[..self.pos])?;
        self.pos = 0;
        Ok(())
    }

    #[inline]
    fn write_u32_be(&mut self, value: u32) -> crate::Result<()> {
        if self.pos > OUTPUT_BUF_SIZE - 4 {
            self.flush_buffer()?;
        }
        self.buffer[self.pos..self.pos + 4].copy_from_slice(&value.to_be_bytes());
        self.pos += 4;
        Ok(())
    }

    #[inline(always)]
    fn write_main(&mut self, buf: &[u8], marker: u8) -> crate::Result<()> {
        match buf {
            [_] => {
                if self.pos >= OUTPUT_BUF_SIZE {
                    self.flush_buffer()?;
                }
                self.buffer[self.pos] = marker;
                self.pos += 1;
            }
            [first, second] => {
                if self.pos > OUTPUT_BUF_SIZE - 2 {
                    self.flush_buffer()?;
                }
                self.buffer[self.pos..self.pos + 2].copy_from_slice(&[*first, *second]);
                self.pos += 2;
            }
            _ if buf.len() <= 32 => {
                let size = buf.len();
                if self.pos > OUTPUT_BUF_SIZE - size {
                    self.flush_buffer()?;
                }
                let output = &mut self.buffer[self.pos..self.pos + size];
                // Overlapping fixed-size copies cover short spans without a variable-size memcpy.
                if size >= 16 {
                    output[..16].copy_from_slice(&buf[..16]);
                    output[size - 16..].copy_from_slice(&buf[size - 16..]);
                } else if size >= 8 {
                    output[..8].copy_from_slice(&buf[..8]);
                    output[size - 8..].copy_from_slice(&buf[size - 8..]);
                } else if size >= 4 {
                    output[..4].copy_from_slice(&buf[..4]);
                    output[size - 4..].copy_from_slice(&buf[size - 4..]);
                } else if size >= 2 {
                    output[..2].copy_from_slice(&buf[..2]);
                    output[size - 2..].copy_from_slice(&buf[size - 2..]);
                }
                self.pos += size;
            }
            _ => self.write_all(buf)?,
        }
        Ok(())
    }
}

impl<W: Write> Write for BufferedOutput<W> {
    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> crate::Result<()> {
        if buf.len() <= OUTPUT_BUF_SIZE - self.pos {
            self.buffer[self.pos..self.pos + buf.len()].copy_from_slice(buf);
            self.pos += buf.len();
            return Ok(());
        }
        self.flush_buffer()?;
        if buf.len() >= OUTPUT_BUF_SIZE {
            return self.inner.write_all(buf);
        }
        self.buffer[..buf.len()].copy_from_slice(buf);
        self.pos = buf.len();
        Ok(())
    }

    fn write(&mut self, buf: &[u8]) -> crate::Result<usize> {
        if self.pos >= OUTPUT_BUF_SIZE {
            self.flush_buffer()?;
        }
        if self.pos == 0 && buf.len() >= OUTPUT_BUF_SIZE {
            return self.inner.write(buf);
        }
        let size = buf.len().min(OUTPUT_BUF_SIZE - self.pos);
        self.buffer[self.pos..self.pos + size].copy_from_slice(&buf[..size]);
        self.pos += size;
        Ok(size)
    }

    fn flush(&mut self) -> crate::Result<()> {
        self.flush_buffer()?;
        self.inner.flush()
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    #[test]
    fn virtual_positions_match_reference_streams() {
        fn bytes(hex: &str) -> Vec<u8> {
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect()
        }
        for line in include_str!("../../../tests/fixtures/bcj2-position.txt")
            .lines()
            .filter(|line| !line.starts_with('#'))
        {
            let fields: Vec<_> = line.split('\t').collect();
            let position = fields[0].parse().unwrap();
            let options = Bcj2Options {
                uncompressed_size: fields[1].parse().ok(),
                ..Default::default()
            };
            let input = bytes(fields[2]);
            let expected = core::array::from_fn(|i| bytes(fields[i + 3]));
            for chunk in [1, 5, 16] {
                let mut writer =
                    Bcj2Writer::new(core::array::from_fn(|_| Vec::new()), &options).unwrap();
                writer.encoder.ip = position;
                writer.uncompressed_size = position;
                for part in input.chunks(chunk) {
                    writer.write_all(part).unwrap();
                }
                assert_eq!(
                    writer.finish().unwrap(),
                    expected,
                    "position={position} chunk={chunk}"
                );
            }
        }
    }

    #[test]
    fn input_size_overflow_is_rejected_before_encoding() {
        let mut writer = Bcj2Writer::new(
            core::array::from_fn(|_| Vec::new()),
            &Bcj2Options::default(),
        )
        .unwrap();
        writer.uncompressed_size = u64::MAX - 1;
        writer.encoder.ip = u64::MAX - 1;
        assert_eq!(
            writer.write(&[0x90; 2]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(writer.get_uncompressed_size(), u64::MAX - 1);
        writer.write_all(&[0x90]).unwrap();
        assert_eq!(writer.get_uncompressed_size(), u64::MAX);
        assert_eq!(writer.finish().unwrap()[0], [0x90]);
    }
}
