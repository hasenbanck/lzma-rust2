use alloc::vec::Vec;

use crate::{Read, error_invalid_data, error_other, error_out_of_memory};

/// Everything a speculative decode pass may change about the dictionary, kept
/// so that the pass can be undone.
///
/// `start` is not in here: only draining output moves it, and no decode pass
/// drains.
#[must_use]
pub(crate) struct LzSpeculation {
    pos: usize,
    full: usize,
    limit: usize,
    pending_len: usize,
    pending_dist: usize,
    /// The stretch of dictionary the pass may write over, saved because it is
    /// still live history. Empty until the dictionary has wrapped: nothing
    /// reads the bytes from `pos` on until then.
    saved: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct LzDecoder {
    buf: Vec<u8>,
    buf_size: usize,
    start: usize,
    pos: usize,
    full: usize,
    limit: usize,
    pending_len: usize,
    pending_dist: usize,
    allocated: bool,
    /// While a speculative pass runs, the position it may not write past.
    spec_end: Option<usize>,
}

impl LzDecoder {
    pub(crate) fn new(dict_size: usize, preset_dict: Option<&[u8]>) -> Self {
        let mut pos = 0;
        let mut full = 0;
        let mut start = 0;
        let mut buf = Vec::new();
        if let Some(preset) = preset_dict {
            pos = preset.len().min(dict_size);
            full = pos;
            start = pos;
            let ps = preset.len() - pos;
            buf = preset[ps..].to_vec();
        }
        Self {
            buf,
            buf_size: dict_size,
            pos,
            full,
            start,
            allocated: false,
            ..Default::default()
        }
    }

    /// Grows the preset buffer to the dictionary size on first use.
    /// Returns an out of memory error if reservation fails.
    pub(crate) fn ensure_capacity(&mut self) -> crate::Result<()> {
        if self.allocated {
            return Ok(());
        }
        self.buf
            .try_reserve_exact(self.buf_size - self.buf.len())
            .map_err(|_| error_out_of_memory("dictionary allocation too large"))?;
        self.buf.resize(self.buf_size, 0);
        self.allocated = true;
        Ok(())
    }

    pub(crate) fn reset(&mut self) {
        self.start = 0;
        self.pos = 0;
        self.full = 0;
        self.limit = 0;
        self.buf[self.buf_size - 1] = 0;
    }

    pub(crate) fn set_limit(&mut self, out_max: usize) {
        self.limit = (out_max + self.pos).min(self.buf_size);
    }

    pub(crate) fn has_space(&self) -> bool {
        self.pos < self.limit
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.pending_len > 0
    }

    pub(crate) fn get_pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn get_byte(&self, dist: usize) -> u8 {
        let offset = if dist >= self.pos {
            self.buf_size
                .saturating_add(self.pos)
                .saturating_sub(dist)
                .saturating_sub(1)
        } else {
            self.pos.saturating_sub(dist).saturating_sub(1)
        };

        self.buf.get(offset).copied().unwrap_or(0)
    }

    pub(crate) fn put_byte(&mut self, b: u8) {
        self.buf[self.pos] = b;
        self.pos += 1;
        if self.full < self.pos {
            self.full = self.pos;
        }
    }

    pub(crate) fn repeat(&mut self, dist: usize, len: usize) -> crate::Result<()> {
        if dist >= self.full {
            return Err(error_other("dist overflow"));
        }
        let mut left = usize::min(self.limit - self.pos, len);
        self.pending_len = len - left;
        self.pending_dist = dist;

        let back = if self.pos < dist + 1 {
            // The distance wraps around to the end of the cyclic dictionary
            // buffer. We cannot get here if the dictionary isn't full.
            debug_assert_eq!(self.full, self.buf_size);
            let mut back = self.buf_size + self.pos - dist - 1;

            let copy_size = usize::min(self.buf_size - back, left);
            self.buf.copy_within(back..back + copy_size, self.pos);
            self.pos += copy_size;
            back = 0;
            left -= copy_size;

            if left == 0 {
                return Ok(());
            }

            back
        } else {
            self.pos - dist - 1
        };

        debug_assert!(back < self.pos);
        debug_assert!(left > 0);

        if dist >= left {
            // No overlap possible. We can copy directly.
            let (src_part, dst_part) = self.buf.split_at_mut(self.pos);
            dst_part[..left].copy_from_slice(&src_part[back..back + left]);
            self.pos += left;
        } else {
            loop {
                let copy_size = left.min(self.pos - back);
                self.buf.copy_within(back..back + copy_size, self.pos);
                self.pos += copy_size;
                left -= copy_size;
                if left == 0 {
                    break;
                }
            }
        }

        if self.full < self.pos {
            self.full = self.pos;
        }
        Ok(())
    }

    pub(crate) fn repeat_pending(&mut self) -> crate::Result<()> {
        if self.pending_len > 0 {
            self.repeat(self.pending_dist, self.pending_len)?;
        }
        Ok(())
    }

    pub(crate) fn copy_uncompressed<R: Read>(
        &mut self,
        mut in_data: R,
        len: usize,
    ) -> crate::Result<()> {
        let copy_size = (self.buf_size - self.pos).min(len);
        let buf = &mut self.buf[self.pos..(self.pos + copy_size)];
        in_data.read_exact(buf)?;
        self.pos += copy_size;
        if self.full < self.pos {
            self.full = self.pos;
        }
        Ok(())
    }

    pub(crate) fn copy_uncompressed_from_slice(&mut self, data: &[u8]) -> crate::Result<()> {
        let copy_size = (self.buf_size - self.pos).min(data.len());
        self.buf[self.pos..self.pos + copy_size].copy_from_slice(&data[..copy_size]);
        self.pos += copy_size;
        if self.full < self.pos {
            self.full = self.pos;
        }
        Ok(())
    }

    pub(crate) fn available_space(&self) -> usize {
        self.spec_end.unwrap_or(self.buf_size) - self.pos
    }

    /// Starts a speculative decode pass that may write at most `max_output`
    /// bytes.
    ///
    /// Undoing one means putting the dictionary bytes back, not only the
    /// positions. Once the dictionary has wrapped, the bytes from `pos` on are
    /// the oldest history still in the window, and a match at a large enough
    /// distance reads them back.
    pub(crate) fn begin_speculation(&mut self, max_output: usize) -> LzSpeculation {
        debug_assert!(
            self.spec_end.is_none(),
            "a speculative pass is already open"
        );
        // What the saving below rests on: the bytes from `pos` on are either
        // history a match can still reach or ground no read has ever covered.
        debug_assert!(
            self.full == self.pos || self.full == self.buf_size,
            "the dictionary is neither still filling nor full"
        );
        let end = self.pos.saturating_add(max_output).min(self.buf_size);
        self.spec_end = Some(end);

        let mut saved = Vec::new();
        if self.full == self.buf_size {
            saved.extend_from_slice(&self.buf[self.pos..end]);
        }

        LzSpeculation {
            pos: self.pos,
            full: self.full,
            limit: self.limit,
            pending_len: self.pending_len,
            pending_dist: self.pending_dist,
            saved,
        }
    }

    /// Takes the saved state so that a pass cannot end without saying whether
    /// what it did is kept.
    pub(crate) fn commit_speculation(&mut self, _spec: LzSpeculation) {
        self.spec_end = None;
    }

    pub(crate) fn rollback_speculation(&mut self, spec: LzSpeculation) {
        self.pos = spec.pos;
        self.full = spec.full;
        self.limit = spec.limit;
        self.pending_len = spec.pending_len;
        self.pending_dist = spec.pending_dist;
        self.buf[spec.pos..spec.pos + spec.saved.len()].copy_from_slice(&spec.saved);
        self.spec_end = None;
    }

    pub(crate) fn has_output(&self) -> bool {
        self.pos > self.start
    }

    pub(crate) fn flush_partial(&mut self, out: &mut [u8]) -> usize {
        let available = self.pos.saturating_sub(self.start);
        let copy_size = available.min(out.len());
        if copy_size > 0 {
            out[..copy_size].copy_from_slice(&self.buf[self.start..self.start + copy_size]);
            self.start += copy_size;
        }
        if self.start == self.pos && self.pos == self.buf_size {
            self.pos = 0;
            self.start = 0;
        }
        copy_size
    }

    pub(crate) fn flush(&mut self, out: &mut [u8], out_off: usize) -> crate::Result<usize> {
        let copy_size = self.pos.saturating_sub(self.start);

        if self.pos == self.buf_size {
            self.pos = 0;
        }

        let src = self
            .buf
            .get(self.start..(self.start + copy_size))
            .ok_or(error_invalid_data("invalid source range"))?;

        let dst = out
            .get_mut(out_off..(out_off + copy_size))
            .ok_or(error_invalid_data("invalid destination range"))?;

        dst.copy_from_slice(src);

        self.start = self.pos;

        Ok(copy_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_capacity_rejects_impossible_size() {
        let mut lz = LzDecoder::new(usize::MAX, None);
        assert!(lz.ensure_capacity().is_err());
    }
}
