use std::io::{ErrorKind, Read};

use super::*;

pub(crate) struct RangeDecoder<R> {
    inner: R,
    range: u32,
    code: u32,
}

impl RangeDecoder<RangeDecoderBuffer> {
    pub(crate) fn new_buffer(len: usize) -> Self {
        Self {
            inner: RangeDecoderBuffer::new(len - 5),
            code: 0,
            range: 0,
        }
    }
}

impl<R: RangeReader> RangeDecoder<R> {
    pub(crate) fn new_stream(mut inner: R) -> std::io::Result<Self> {
        let b = inner.read_u8()?;
        if b != 0x00 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "range decoder first byte is 0",
            ));
        }
        let code = inner.read_u32_be()?;
        Ok(Self {
            inner,
            code,
            range: 0xFFFFFFFFu32,
        })
    }

    pub(crate) fn is_stream_finished(&self) -> bool {
        self.code == 0
    }
}

impl<R: RangeReader> RangeDecoder<R> {
    #[inline(always)]
    pub(crate) fn normalize(&mut self) -> std::io::Result<()> {
        if self.range < 0x0100_0000 {
            let b = self.inner.read_u8()? as u32;
            self.code = (self.code << SHIFT_BITS) | b;
            self.range <<= SHIFT_BITS;
        }
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn decode_bit(&mut self, prob: &mut u16) -> std::io::Result<i32> {
        self.normalize()?;
        let bound = (self.range >> BIT_MODEL_TOTAL_BITS) * (*prob as u32);

        // This mask will be 0 for bit 0, and 0xFFFFFFFF for bit 1.
        let mask = 0u32.wrapping_sub((self.code >= bound) as u32);

        self.range = (bound & !mask) | ((self.range - bound) & mask);
        self.code -= bound & mask;

        let p = *prob as u32;
        let offset = RC_BIT_MODEL_OFFSET & !mask;
        *prob = (p - ((p + offset) >> MOVE_BITS)) as u16;

        Ok((mask & 1) as i32)
    }

    pub(crate) fn decode_bit_tree(&mut self, probs: &mut [u16]) -> std::io::Result<i32> {
        let mut symbol = 1;
        loop {
            symbol = (symbol << 1) | self.decode_bit(&mut probs[symbol as usize])?;
            if symbol >= probs.len() as i32 {
                break;
            }
        }
        Ok(symbol - probs.len() as i32)
    }

    pub(crate) fn decode_reverse_bit_tree(&mut self, probs: &mut [u16]) -> std::io::Result<i32> {
        let mut symbol = 1;
        let mut i = 0;
        let mut result = 0;
        loop {
            let bit = self.decode_bit(&mut probs[symbol as usize])?;
            symbol = (symbol << 1) | bit;
            result |= bit << i;
            i += 1;
            if symbol >= probs.len() as i32 {
                break;
            }
        }
        Ok(result)
    }

    pub(crate) fn decode_direct_bits(&mut self, count: u32) -> std::io::Result<i32> {
        let mut result = 0;
        for _ in 0..count {
            self.normalize()?;
            self.range >>= 1;
            let t = (self.code.wrapping_sub(self.range)) >> 31;
            self.code -= self.range & (t.wrapping_sub(1));
            result = (result << 1) | (1u32.wrapping_sub(t));
        }
        Ok(result as _)
    }
}

pub(crate) struct RangeDecoderBuffer {
    buf: Vec<u8>,
    pos: usize,
}

impl RangeDecoder<RangeDecoderBuffer> {
    pub(crate) fn prepare<R: Read + RangeReader>(
        &mut self,
        mut reader: R,
        len: usize,
    ) -> std::io::Result<()> {
        if len < 5 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "buffer len must >= 5",
            ));
        }

        let b = reader.read_u8()?;
        if b != 0x00 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "first byte is 0",
            ));
        }
        self.code = reader.read_u32_be()?;

        self.range = 0xFFFFFFFFu32;
        let len = len - 5;
        let pos = self.inner.buf.len() - len;
        let end = pos + len;
        self.inner.pos = pos;
        reader.read_exact(&mut self.inner.buf[pos..end])
    }

    #[inline]
    pub(crate) fn is_finished(&self) -> bool {
        self.inner.pos == self.inner.buf.len() && self.code == 0
    }
}

impl RangeDecoderBuffer {
    pub(crate) fn new(len: usize) -> Self {
        Self {
            buf: vec![0; len],
            pos: len,
        }
    }
}

pub(crate) trait RangeReader {
    fn read_u8(&mut self) -> std::io::Result<u8>;
    fn read_u32_be(&mut self) -> std::io::Result<u32>;
}

impl<T: Read> RangeReader for T {
    #[inline(always)]
    fn read_u8(&mut self) -> std::io::Result<u8> {
        let mut buf = [0; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    #[inline(always)]
    fn read_u32_be(&mut self) -> std::io::Result<u32> {
        let mut buf = [0; 4];
        self.read_exact(buf.as_mut())?;
        Ok(u32::from_be_bytes(buf))
    }
}

impl RangeReader for RangeDecoderBuffer {
    #[inline(always)]
    fn read_u8(&mut self) -> std::io::Result<u8> {
        // Out of bound reads return an 0, which is fine, since a
        // well-implemented decoder will not go out of bound.
        // Not returning an error results in code that can be better
        // optimized in the hot path and overall 10% better decoding
        // performance.
        let byte = *self.buf.get(self.pos).unwrap_or(&0);
        self.pos += 1;

        Ok(byte)
    }

    #[inline(always)]
    fn read_u32_be(&mut self) -> std::io::Result<u32> {
        let b = u32::from_be_bytes(self.buf[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(b)
    }
}
