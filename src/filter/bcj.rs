//! Branch/Call/Jump Filters for executables of different architectures.

mod arm;
mod ia64;
mod ppc;
mod riscv;
mod sparc;
mod x86;

use alloc::{vec, vec::Vec};

#[cfg(feature = "encoder")]
use crate::Write;
use crate::{copy_error, Read};

struct BCJFilter {
    is_encoder: bool,
    pos: usize,
    prev_mask: u32,
    filter: FilterFn,
}

type FilterFn = fn(filter: &mut BCJFilter, buf: &mut [u8]) -> usize;

impl BCJFilter {
    #[inline]
    fn code(&mut self, buf: &mut [u8]) -> usize {
        let filter = self.filter;
        filter(self, buf)
    }
}

const FILTER_BUF_SIZE: usize = 4096;

pub struct BCJReader<R> {
    inner: R,
    filter: BCJFilter,
    state: State,
    err: Option<crate::Error>,
}

#[derive(Debug, Default)]
struct State {
    filter_buf: Vec<u8>,
    pos: usize,
    filtered: usize,
    unfiltered: usize,
    end_reached: bool,
}

impl<R> BCJReader<R> {
    fn new(inner: R, filter: BCJFilter) -> Self {
        Self {
            inner,
            filter,
            state: State {
                filter_buf: vec![0; FILTER_BUF_SIZE],
                ..Default::default()
            },
            err: None,
        }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }

    #[inline]
    pub fn new_x86(inner: R, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_x86(start_pos, false))
    }

    #[inline]
    pub fn new_arm(inner: R, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_arm(start_pos, false))
    }

    #[inline]
    pub fn new_arm64(inner: R, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_arm64(start_pos, false))
    }

    #[inline]
    pub fn new_arm_thumb(inner: R, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_arm_thumb(start_pos, false))
    }

    #[inline]
    pub fn new_ppc(inner: R, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_power_pc(start_pos, false))
    }

    #[inline]
    pub fn new_sparc(inner: R, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_sparc(start_pos, false))
    }

    #[inline]
    pub fn new_ia64(inner: R, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_ia64(start_pos, false))
    }

    #[inline]
    pub fn new_riscv(inner: R, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_riscv(start_pos, false))
    }
}

impl<R: Read> Read for BCJReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> crate::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if let Some(e) = self.err.as_ref() {
            return Err(copy_error(e));
        }
        let mut len = buf.len();
        let mut state = core::mem::take(&mut self.state);
        let mut off = 0;
        let mut size = 0;

        loop {
            // Copy filtered data into the caller-provided buffer.
            if state.filtered > 0 {
                let copy_size = state.filtered.min(len);
                let pos = state.pos;
                buf[off..(off + copy_size)]
                    .copy_from_slice(&state.filter_buf[pos..(pos + copy_size)]);
                state.pos += copy_size;
                state.filtered -= copy_size;
                off += copy_size;
                len -= copy_size;
                size += copy_size;
            }

            // If end of filterBuf was reached, move the pending data to
            // the beginning of the buffer so that more data can be
            // copied into filterBuf on the next loop iteration.
            if state.pos + state.filtered + state.unfiltered == FILTER_BUF_SIZE {
                // state.filter_buf.copy_from_slice(src);
                state.filter_buf.rotate_left(state.pos);
                state.pos = 0;
            }

            if len == 0 || state.end_reached {
                self.state = state;
                return Ok(if size > 0 { size } else { 0 });
            }

            assert_eq!(state.filtered, 0);
            // Get more data into the temporary buffer.
            let mut in_size = FILTER_BUF_SIZE - (state.pos + state.filtered + state.unfiltered);
            let start = state.pos + state.filtered + state.unfiltered;
            let temp = &mut state.filter_buf[start..(start + in_size)];
            in_size = match self.inner.read(temp) {
                Ok(s) => s,
                Err(e) => {
                    let err = copy_error(&e);
                    self.err = Some(err);
                    self.state = state;
                    return Err(e);
                }
            };

            if in_size == 0 {
                // Mark the remaining unfiltered bytes to be ready
                // to be copied out.
                state.end_reached = true;
                state.filtered = state.unfiltered;
                state.unfiltered = 0;
            } else {
                // Filter the data in filterBuf.
                state.unfiltered += in_size;
                state.filtered = self
                    .filter
                    .code(&mut state.filter_buf[state.pos..(state.pos + state.unfiltered)]);
                assert!(state.filtered <= state.unfiltered);
                state.unfiltered -= state.filtered;
            }
        }
    }
}

#[cfg(feature = "encoder")]
pub struct BCJWriter<W> {
    inner: W,
    filter: BCJFilter,
    buffer: Vec<u8>,
}

#[cfg(feature = "encoder")]
impl<W> BCJWriter<W> {
    fn new(inner: W, filter: BCJFilter) -> Self {
        Self {
            inner,
            filter,
            buffer: Vec::with_capacity(FILTER_BUF_SIZE),
        }
    }

    pub fn into_inner(self) -> W {
        self.inner
    }

    #[inline]
    pub fn new_x86(inner: W, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_x86(start_pos, true))
    }

    #[inline]
    pub fn new_arm(inner: W, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_arm(start_pos, true))
    }

    #[inline]
    pub fn new_arm64(inner: W, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_arm64(start_pos, true))
    }

    #[inline]
    pub fn new_arm_thumb(inner: W, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_arm_thumb(start_pos, true))
    }

    #[inline]
    pub fn new_ppc(inner: W, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_power_pc(start_pos, true))
    }

    #[inline]
    pub fn new_sparc(inner: W, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_sparc(start_pos, true))
    }

    #[inline]
    pub fn new_ia64(inner: W, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_ia64(start_pos, true))
    }

    #[inline]
    pub fn new_riscv(inner: W, start_pos: usize) -> Self {
        Self::new(inner, BCJFilter::new_riscv(start_pos, true))
    }
}

#[cfg(feature = "encoder")]
impl<W: Write> Write for BCJWriter<W> {
    fn write(&mut self, buf: &[u8]) -> crate::Result<usize> {
        let data_size = buf.len();

        if data_size > self.buffer.len() {
            self.buffer.resize(data_size, 0);
        }

        self.buffer[..data_size].copy_from_slice(buf);
        let filtered_size = self.filter.code(&mut self.buffer[..data_size]);

        // BCJ filters may not process all bytes, so we need to handle the processed portion.
        if filtered_size > 0 {
            self.inner.write(&self.buffer[..filtered_size])?;
        }

        // If not all bytes were processed, we need to handle the remainder.
        if filtered_size < data_size {
            self.inner.write(&self.buffer[filtered_size..data_size])?;
        }

        Ok(data_size)
    }

    fn flush(&mut self) -> crate::Result<()> {
        self.inner.flush()
    }
}

#[cfg(all(feature = "encoder", feature = "std"))]
#[cfg(test)]
mod tests {
    use std::io::{copy, Cursor};

    use super::*;

    #[test]
    fn test_bcj_x86_roundtrip() {
        let test_data = std::fs::read("tests/data/wget-x86").unwrap();

        let mut encoded_buffer = Vec::new();
        let mut writer = BCJWriter::new_x86(Cursor::new(&mut encoded_buffer), 0);
        copy(&mut test_data.as_slice(), &mut writer).expect("Failed to encode data");

        assert!(test_data != encoded_buffer);

        let mut decoded_data = Vec::new();
        let mut reader = BCJReader::new_x86(Cursor::new(&encoded_buffer), 0);
        copy(&mut reader, &mut decoded_data).expect("Failed to decode data");

        assert!(test_data == decoded_data);
    }

    #[test]
    fn test_bcj_arm_roundtrip() {
        let test_data = std::fs::read("tests/data/wget-arm").unwrap();

        let mut encoded_buffer = Vec::new();
        let mut writer = BCJWriter::new_arm(Cursor::new(&mut encoded_buffer), 0);
        copy(&mut test_data.as_slice(), &mut writer).expect("Failed to encode data");

        assert!(test_data != encoded_buffer);

        let mut decoded_data = Vec::new();
        let mut reader = BCJReader::new_arm(Cursor::new(&encoded_buffer), 0);
        copy(&mut reader, &mut decoded_data).expect("Failed to decode data");

        assert!(test_data == decoded_data);
    }

    #[test]
    fn test_bcj_arm64_roundtrip() {
        let test_data = std::fs::read("tests/data/wget-arm64").unwrap();

        let mut encoded_buffer = Vec::new();
        let mut writer = BCJWriter::new_arm64(Cursor::new(&mut encoded_buffer), 0);
        copy(&mut test_data.as_slice(), &mut writer).expect("Failed to encode data");

        assert!(test_data != encoded_buffer);

        let mut decoded_data = Vec::new();
        let mut reader = BCJReader::new_arm64(Cursor::new(&encoded_buffer), 0);
        copy(&mut reader, &mut decoded_data).expect("Failed to decode data");

        assert!(test_data == decoded_data);
    }

    #[test]
    fn test_bcj_arm_thumb_roundtrip() {
        let test_data = std::fs::read("tests/data/wget-arm-thumb").unwrap();

        let mut encoded_buffer = Vec::new();
        let mut writer = BCJWriter::new_arm_thumb(Cursor::new(&mut encoded_buffer), 0);
        copy(&mut test_data.as_slice(), &mut writer).expect("Failed to encode data");

        assert!(test_data != encoded_buffer);

        let mut decoded_data = Vec::new();
        let mut reader = BCJReader::new_arm_thumb(Cursor::new(&encoded_buffer), 0);
        copy(&mut reader, &mut decoded_data).expect("Failed to decode data");

        assert!(test_data == decoded_data);
    }

    #[test]
    fn test_bcj_ppc_roundtrip() {
        let test_data = std::fs::read("tests/data/wget-ppc").unwrap();

        let mut encoded_buffer = Vec::new();
        let mut writer = BCJWriter::new_ppc(Cursor::new(&mut encoded_buffer), 0);
        copy(&mut test_data.as_slice(), &mut writer).expect("Failed to encode data");

        assert!(test_data != encoded_buffer);

        let mut decoded_data = Vec::new();
        let mut reader = BCJReader::new_ppc(Cursor::new(&encoded_buffer), 0);
        copy(&mut reader, &mut decoded_data).expect("Failed to decode data");

        assert!(test_data == decoded_data);
    }

    #[test]
    fn test_bcj_sparc_roundtrip() {
        let test_data = std::fs::read("tests/data/wget-sparc").unwrap();

        let mut encoded_buffer = Vec::new();
        let mut writer = BCJWriter::new_sparc(Cursor::new(&mut encoded_buffer), 0);
        copy(&mut test_data.as_slice(), &mut writer).expect("Failed to encode data");

        assert!(test_data != encoded_buffer);

        let mut decoded_data = Vec::new();
        let mut reader = BCJReader::new_sparc(Cursor::new(&encoded_buffer), 0);
        copy(&mut reader, &mut decoded_data).expect("Failed to decode data");

        assert!(test_data == decoded_data);
    }

    #[test]
    fn test_bcj_ia64_roundtrip() {
        let test_data = std::fs::read("tests/data/wget-ia64").unwrap();

        let mut encoded_buffer = Vec::new();
        let mut writer = BCJWriter::new_ia64(Cursor::new(&mut encoded_buffer), 0);
        copy(&mut test_data.as_slice(), &mut writer).expect("Failed to encode data");

        assert!(test_data != encoded_buffer);

        let mut decoded_data = Vec::new();
        let mut reader = BCJReader::new_ia64(Cursor::new(&encoded_buffer), 0);
        copy(&mut reader, &mut decoded_data).expect("Failed to decode data");

        assert!(test_data == decoded_data);
    }

    #[test]
    fn test_bcj_riscv_roundtrip() {
        let test_data = std::fs::read("tests/data/wget-riscv").unwrap();

        let mut encoded_buffer = Vec::new();
        let mut writer = BCJWriter::new_riscv(Cursor::new(&mut encoded_buffer), 0);
        copy(&mut test_data.as_slice(), &mut writer).expect("Failed to encode data");

        assert!(test_data != encoded_buffer);

        let mut decoded_data = Vec::new();
        let mut reader = BCJReader::new_riscv(Cursor::new(&encoded_buffer), 0);
        copy(&mut reader, &mut decoded_data).expect("Failed to decode data");

        assert!(test_data == decoded_data);
    }
}
