//! The BCJ2 filter is a branch converter for 32-bit x86 executables (version 2).
//!
//! BCJ2 splits input into four raw streams: MAIN holds literal bytes, CALL and
//! JUMP hold converted absolute addresses in big-endian order, and RC holds
//! range-coded conversion decisions. The original relative addresses are
//! little-endian. These streams are not compressed; callers can compress each
//! one separately. Encoding and decoding use a starting position of zero.

mod decode;
#[cfg(feature = "encoder")]
mod encode;

use alloc::{vec, vec::Vec};

use decode::Bcj2Decoder;
#[cfg(feature = "encoder")]
pub use encode::{Bcj2Options, Bcj2Writer};

use crate::{Read, StickyError, error_eof, error_invalid_data, error_invalid_input};

const BUF_SIZE: usize = 1 << 18;

// Include the preceding byte so both contexts use contiguous vector loads.
#[inline(never)]
fn contains_marker(window: &[u8; 17]) -> bool {
    let mut found = false;
    for i in 0..16 {
        found |= ((window[i + 1] & 0xFE) == 0xE8)
            | ((window[i] == 0x0F) & ((window[i + 1] & 0xF0) == 0x80));
    }
    found
}

// One bit per byte locates the first marker without rescanning the matching block.
#[cfg(feature = "encoder")]
#[inline(always)]
fn marker_mask(window: &[u8; 17]) -> u16 {
    let mut mask = 0u16;
    for i in 0..16 {
        let marker = ((window[i + 1] & 0xFE) == 0xE8)
            | ((window[i] == 0x0F) & ((window[i + 1] & 0xF0) == 0x80));
        mask |= u16::from(marker) << i;
    }
    mask
}

// Keep each ARM64 half contiguous instead of rebuilding context across inlined halves.
#[cfg(feature = "encoder")]
#[cfg_attr(target_arch = "aarch64", inline(never))]
#[cfg_attr(not(target_arch = "aarch64"), inline(always))]
fn first_marker(window: &[u8; 17]) -> usize {
    marker_mask(window).trailing_zeros() as usize
}

#[cfg(feature = "encoder")]
#[inline(always)]
fn literal_prefix_exact(window: &[u8; 33]) -> usize {
    let mask = marker_mask(window[..17].try_into().unwrap());
    if mask != 0 {
        mask.trailing_zeros() as usize
    } else {
        16 + first_marker(window[16..].try_into().unwrap())
    }
}

// Return whole 16-byte halves before a marker. Separate reductions retain vectorization.
#[inline(never)]
fn literal_prefix(window: &[u8; 33]) -> usize {
    let mut first = false;
    for i in 0..16 {
        first |= ((window[i + 1] & 0xFE) == 0xE8)
            | ((window[i] == 0x0F) & ((window[i + 1] & 0xF0) == 0x80));
    }
    if first {
        return 0;
    }
    let mut second = false;
    for i in 16..32 {
        second |= ((window[i + 1] & 0xFE) == 0xE8)
            | ((window[i] == 0x0F) & ((window[i + 1] & 0xF0) == 0x80));
    }
    if second { 16 } else { 32 }
}

// Test each mask before locating a marker so empty halves need no bit scan.
#[cfg(all(feature = "encoder", not(target_arch = "aarch64")))]
#[inline(always)]
fn literal_prefix_64(window: &[u8; 65]) -> usize {
    let mask = marker_mask(window[..17].try_into().unwrap());
    if mask != 0 {
        return mask.trailing_zeros() as usize;
    }
    let mask = marker_mask(window[16..33].try_into().unwrap());
    if mask != 0 {
        return 16 + mask.trailing_zeros() as usize;
    }
    let mask = marker_mask(window[32..49].try_into().unwrap());
    if mask != 0 {
        return 32 + mask.trailing_zeros() as usize;
    }
    let mask = marker_mask(window[48..].try_into().unwrap());
    if mask != 0 {
        return 48 + mask.trailing_zeros() as usize;
    }
    64
}

// Outlining also keeps wide-scan constants out of the converted-branch path on ARM64.
#[cfg(all(feature = "encoder", target_arch = "aarch64"))]
#[inline(never)]
fn literal_prefix_64(window: &[u8; 65]) -> usize {
    for offset in [0, 16, 32, 48] {
        let half: &[u8; 17] = window[offset..offset + 17].try_into().unwrap();
        let size = first_marker(half);
        if size != 16 {
            return offset + size;
        }
    }
    64
}

const BCJ2_NUM_STREAMS: usize = 4;

const BCJ2_STREAM_MAIN: usize = 0;

const BCJ2_STREAM_CALL: usize = 1;

const BCJ2_STREAM_JUMP: usize = 2;

const BCJ2_STREAM_RC: usize = 3;

const BCJ2_DEC_STATE_ORIG_0: usize = BCJ2_NUM_STREAMS;

const BCJ2_DEC_STATE_ORIG_3: usize = BCJ2_NUM_STREAMS + 3;

const BCJ2_DEC_STATE_ORIG: usize = BCJ2_NUM_STREAMS + 4;

const BCJ2_DEC_STATE_OK: usize = BCJ2_NUM_STREAMS + 5;

const NUM_MODEL_BITS: u16 = 11;

const BIT_MODEL_TOTAL: u16 = 1 << NUM_MODEL_BITS;

const NUM_MOVE_BITS: u16 = 5;

const K_TOP_VALUE: u32 = 1 << 24;

#[inline(always)]
const fn bcj2_is_32bit_stream(s: usize) -> bool {
    (s) == BCJ2_STREAM_CALL || (s) == BCJ2_STREAM_JUMP
}

/// BCJ2 coder for x86 executables with separate streams for different instruction types.
pub struct Bcj2Coder {
    bufs: Vec<u8>,
}

impl Bcj2Coder {
    fn buf_at(&mut self, i: usize) -> &mut [u8] {
        let i = i * BUF_SIZE;
        &mut self.bufs[i..i + BUF_SIZE]
    }
}

impl Default for Bcj2Coder {
    fn default() -> Self {
        let buf_len = BUF_SIZE * (BCJ2_NUM_STREAMS);
        Self {
            bufs: vec![0; buf_len],
        }
    }
}

/// Reader for BCJ2-filtered data with multiple input streams.
///
/// The inputs contain the raw MAIN, CALL, JUMP and RC streams, in that order.
/// Reading stops at the declared output size. Call [`Self::finish`] to also
/// check that all four input streams have been consumed. A decoding or input
/// error is reported again on later reads.
pub struct Bcj2Reader<R> {
    base: Bcj2Coder,
    inputs: Vec<R>,
    decoder: Bcj2Decoder,
    extra_read_sizes: [usize; BCJ2_NUM_STREAMS],
    uncompressed_size: u64,
    finished: bool,
    failure: Option<StickyError>,
}

impl<R> Bcj2Reader<R> {
    /// Creates a new BCJ2 reader with the given input streams and expected output size.
    pub fn new(inputs: Vec<R>, uncompressed_size: u64) -> Self {
        Self {
            base: Default::default(),
            inputs,
            decoder: Bcj2Decoder::new(),
            extra_read_sizes: [0; BCJ2_NUM_STREAMS],
            uncompressed_size,
            finished: false,
            failure: None,
        }
        .init()
    }

    /// Creates a reader after checking that exactly four inputs were provided.
    ///
    /// [`Self::new`] reports an invalid input count on the first nonempty read.
    pub fn try_new(inputs: Vec<R>, uncompressed_size: u64) -> crate::Result<Self> {
        if inputs.len() != BCJ2_NUM_STREAMS {
            return Err(error_invalid_input("BCJ2 requires four input streams"));
        }
        Ok(Self::new(inputs, uncompressed_size))
    }

    fn init(mut self) -> Self {
        let mut v = 0;
        for i in 0..BCJ2_NUM_STREAMS {
            self.decoder.bufs[i] = v;
            self.decoder.lims[i] = v;
            v += BUF_SIZE;
        }

        self
    }
}

impl<R: Read> Read for Bcj2Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> crate::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if let Some(failure) = &self.failure {
            return Err(failure.report());
        }
        if self.inputs.len() != BCJ2_NUM_STREAMS {
            return self.fail(0, error_invalid_input("BCJ2 requires four input streams"));
        }
        if self.finished {
            return Ok(0);
        }
        let mut dest_buf = buf;
        if dest_buf.len() as u64 > self.uncompressed_size {
            dest_buf = &mut dest_buf[..self.uncompressed_size as usize];
        }
        let mut result_size = 0;
        self.decoder.set_dest(0);
        let mut offset = 0;
        loop {
            if !self.decoder.decode(&mut self.base.bufs, dest_buf) {
                return self.fail(result_size, error_invalid_data("bcj2 decode error"));
            }

            {
                let cur_size = self.decoder.dest() - offset;
                if cur_size != 0 {
                    result_size += cur_size;
                    self.uncompressed_size -= cur_size as u64;
                    offset += cur_size;
                }
            }

            if self.uncompressed_size == 0 {
                if self.decoder.state == BCJ2_STREAM_MAIN
                    || self.decoder.state == BCJ2_DEC_STATE_ORIG
                {
                    if self.decoder.code != 0 {
                        return self.fail(result_size, error_invalid_data("bcj2 decode error:4"));
                    }
                    self.finished = true;
                    break;
                }
                if self.decoder.state >= BCJ2_NUM_STREAMS {
                    return self.fail(result_size, error_invalid_data("bcj2 decode error:5"));
                }
            }
            if self.decoder.state >= BCJ2_NUM_STREAMS {
                break;
            }
            let mut total_read = self.extra_read_sizes[self.decoder.state];
            self.extra_read_sizes[self.decoder.state] = 0;
            {
                let buf_index = self.decoder.state * BUF_SIZE;
                let from = self.decoder.bufs[self.decoder.state];
                for i in 0..total_read {
                    let b = self.base.bufs[from + i];
                    self.base.bufs[buf_index + i] = b;
                }
                self.decoder.lims[self.decoder.state] = buf_index;
                self.decoder.bufs[self.decoder.state] = buf_index;
            }
            loop {
                let cur_size = BUF_SIZE - total_read;
                let read = self.inputs[self.decoder.state].read(
                    &mut self.base.buf_at(self.decoder.state)[total_read..total_read + cur_size],
                );
                let cur_size = match read {
                    Ok(size) => size,
                    Err(error) => {
                        #[cfg(feature = "std")]
                        if error.kind() == std::io::ErrorKind::Interrupted {
                            continue;
                        }
                        #[cfg(not(feature = "std"))]
                        if matches!(error, crate::Error::Interrupted) {
                            continue;
                        }
                        return self.fail(result_size, error);
                    }
                };
                if cur_size == 0 {
                    break;
                }
                total_read += cur_size;
                if !(total_read < 4 && bcj2_is_32bit_stream(self.decoder.state)) {
                    break;
                }
            }

            if total_read == 0 {
                return self.fail(result_size, error_eof("unexpected end of BCJ2 input"));
            }

            if bcj2_is_32bit_stream(self.decoder.state) {
                let extra_size = total_read & 3;
                self.extra_read_sizes[self.decoder.state] = extra_size;
                if total_read < 4 {
                    return self.fail(result_size, error_eof("incomplete BCJ2 address"));
                }
                total_read -= extra_size;
            }
            self.decoder.lims[self.decoder.state] = total_read + self.decoder.state * BUF_SIZE;
        }

        Ok(result_size)
    }
}

impl<R: Read> Bcj2Reader<R> {
    /// Checks that the output and all four input streams have ended, returning the inputs.
    ///
    /// Read the declared output before calling this method. For empty output,
    /// this method initializes and validates the RC stream. The input readers
    /// must be limited to their BCJ2 stream lengths: checking for trailing data
    /// can read one further byte from each input.
    pub fn finish(mut self) -> crate::Result<Vec<R>> {
        if let Some(failure) = &self.failure {
            return Err(failure.report());
        }
        if self.uncompressed_size != 0 {
            return Err(error_invalid_input("BCJ2 output has not been fully read"));
        }
        let _ = self.read(&mut [0])?;
        for i in 0..BCJ2_NUM_STREAMS {
            if self.decoder.bufs[i] != self.decoder.lims[i] || self.extra_read_sizes[i] != 0 {
                return Err(error_invalid_data("trailing BCJ2 input"));
            }
            loop {
                match self.inputs[i].read(&mut [0]) {
                    Ok(0) => break,
                    Ok(_) => return Err(error_invalid_data("trailing BCJ2 input")),
                    Err(error) => {
                        #[cfg(feature = "std")]
                        if error.kind() == std::io::ErrorKind::Interrupted {
                            continue;
                        }
                        #[cfg(not(feature = "std"))]
                        if matches!(error, crate::Error::Interrupted) {
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
        }
        Ok(self.inputs)
    }
}

impl<R> Bcj2Reader<R> {
    fn fail(&mut self, result_size: usize, error: crate::Error) -> crate::Result<usize> {
        let failure = StickyError::new(error);
        let reported = failure.report();
        self.failure = Some(failure);
        if result_size == 0 {
            Err(reported)
        } else {
            Ok(result_size)
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[cfg(feature = "encoder")]
    #[test]
    fn wide_literal_prefix_stops_at_first_marker() {
        for position in 0..64 {
            for byte in 0..=u8::MAX {
                let mut window = [0x90; 65];
                window[0] = 0;
                window[position + 1] = byte;
                assert_eq!(
                    literal_prefix_64(&window),
                    if matches!(byte, 0xE8 | 0xE9) {
                        position
                    } else {
                        64
                    }
                );
                window[position] = 0x0F;
                assert_eq!(
                    literal_prefix_64(&window),
                    if matches!(byte, 0x80..=0x8F | 0xE8 | 0xE9) {
                        position
                    } else {
                        64
                    },
                    "position={position} byte={byte:02x}"
                );
            }
        }
        let mut window = [0x90; 65];
        window[64] = 0x0F;
        assert_eq!(literal_prefix_64(&window), 64);
        window[0] = 0x0F;
        window[1] = 0x85;
        assert_eq!(literal_prefix_64(&window), 0);
        window = [0x90; 65];
        for position in (0..64).rev() {
            window[position + 1] = 0xE9;
            assert_eq!(literal_prefix_64(&window), position);
        }
    }

    #[cfg(feature = "encoder")]
    #[test]
    fn exact_marker_prefix_preserves_context_and_first_match() {
        for position in 0..32 {
            for byte in 0..=u8::MAX {
                for previous in [0, 0x0F] {
                    let mut window = [0x90; 33];
                    window[position] = previous;
                    window[position + 1] = byte;
                    let expected = if matches!(byte, 0xE8 | 0xE9)
                        || (previous == 0x0F && matches!(byte, 0x80..=0x8F))
                    {
                        position
                    } else {
                        32
                    };
                    assert_eq!(literal_prefix_exact(&window), expected);
                    for start in [0, 16] {
                        let half = window[start..start + 17].try_into().unwrap();
                        let expected = if (start..start + 16).contains(&expected) {
                            expected - start
                        } else {
                            16
                        };
                        assert_eq!(first_marker(half), expected);
                    }
                }
            }
        }
        let mut window = [0x90; 33];
        window[32] = 0x0F;
        assert_eq!(literal_prefix_exact(&window), 32);
        for position in (0..32).rev() {
            window[position + 1] = 0xE8;
            assert_eq!(literal_prefix_exact(&window), position);
        }
    }

    #[test]
    fn literal_prefix_stops_before_marker_half() {
        for position in 0..32 {
            for byte in 0..=u8::MAX {
                let mut window = [0x90; 33];
                window[0] = 0;
                window[position + 1] = byte;
                let marker_half = if position < 16 { 0 } else { 16 };
                assert_eq!(
                    literal_prefix(&window),
                    if matches!(byte, 0xE8 | 0xE9) {
                        marker_half
                    } else {
                        32
                    }
                );
                window[position] = 0x0F;
                assert_eq!(
                    literal_prefix(&window),
                    if matches!(byte, 0x80..=0x8F | 0xE8 | 0xE9) {
                        marker_half
                    } else {
                        32
                    },
                    "position={position} byte={byte:02x}"
                );
            }
        }
        let mut window = [0x90; 33];
        window[32] = 0x0F;
        assert_eq!(literal_prefix(&window), 32);
        window[0] = 0x0F;
        window[1] = 0x85;
        assert_eq!(literal_prefix(&window), 0);
    }

    #[test]
    fn marker_blocks_preserve_previous_byte_context() {
        let mut window = [0x90; 17];
        window[0] = 0x0F;
        assert!(!contains_marker(&window));
        for position in 0..16 {
            for byte in 0..=u8::MAX {
                let mut window = [0x90; 17];
                window[0] = 0;
                window[position + 1] = byte;
                assert_eq!(contains_marker(&window), matches!(byte, 0xE8 | 0xE9));
                window[position] = 0x0F;
                assert_eq!(
                    contains_marker(&window),
                    matches!(byte, 0x80..=0x8F | 0xE8 | 0xE9),
                    "position={position} byte={byte:02x}"
                );
            }
        }
        let mut window = [0x90; 17];
        window[16] = 0x0F;
        assert!(!contains_marker(&window));
        window[0] = 0x0F;
        window[1] = 0x85;
        assert!(contains_marker(&window));
    }
}
