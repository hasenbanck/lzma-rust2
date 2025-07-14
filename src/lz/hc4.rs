#[cfg(feature = "optimization")]
use super::AlignedMemoryI32;
use super::{
    hash234::Hash234,
    lz_encoder::{LZEncoder, MatchFind, Matches},
    LZEncoderData,
};

/// Hash Chain with 4-byte matching
pub(crate) struct HC4 {
    hash: Hash234,
    #[cfg(feature = "optimization")]
    chain: AlignedMemoryI32,
    #[cfg(not(feature = "optimization"))]
    chain: Vec<i32>,
    depth_limit: i32,
    cyclic_size: i32,
    cyclic_pos: i32,
    lz_pos: i32,
}

impl HC4 {
    pub(crate) fn get_mem_usage(dict_size: u32) -> u32 {
        Hash234::get_mem_usage(dict_size) + dict_size / (1024 / 4) + 10
    }

    pub(crate) fn new(dict_size: u32, nice_len: u32, depth_limit: i32) -> Self {
        #[cfg(feature = "optimization")]
        let chain = AlignedMemoryI32::new(dict_size as usize + 1);
        #[cfg(not(feature = "optimization"))]
        let chain = vec![0; dict_size as usize + 1];

        assert!(chain.len() >= (dict_size as usize + 1));

        Self {
            hash: Hash234::new(dict_size),
            chain,
            depth_limit: if depth_limit > 0 {
                depth_limit
            } else {
                4 + nice_len as i32 / 4
            },
            cyclic_size: dict_size as i32 + 1,
            cyclic_pos: -1,
            lz_pos: dict_size as i32 + 1,
        }
    }

    fn move_pos(&mut self, encoder: &mut LZEncoderData) -> i32 {
        let avail = encoder.move_pos(4, 4);
        if avail != 0 {
            self.lz_pos += 1;
            if self.lz_pos == 0x7FFFFFFF {
                let norm_offset = 0x7FFFFFFF - self.cyclic_size;
                self.hash.normalize(norm_offset);
                LZEncoder::normalize(&mut self.chain, norm_offset);
                self.lz_pos = self.lz_pos.wrapping_sub(norm_offset);
            }

            self.cyclic_pos += 1;
            if self.cyclic_pos == self.cyclic_size {
                self.cyclic_pos = 0;
            }
        }

        avail
    }
}

impl MatchFind for HC4 {
    fn find_matches(&mut self, encoder: &mut LZEncoderData, matches: &mut Matches) {
        matches.count = 0;
        let mut match_len_limit = encoder.match_len_max as i32;
        let mut nice_len_limit = encoder.nice_len as i32;
        let avail = self.move_pos(encoder);

        if avail < match_len_limit {
            if avail == 0 {
                return;
            }
            match_len_limit = avail;
            if nice_len_limit > avail {
                nice_len_limit = avail;
            }
        }
        self.hash.calc_hashes(encoder.read_buffer());
        let mut delta2 = self.lz_pos.wrapping_sub(self.hash.get_hash2_pos());
        let delta3 = self.lz_pos.wrapping_sub(self.hash.get_hash3_pos());
        let mut current_match = self.hash.get_hash4_pos();
        self.hash.update_tables(self.lz_pos);
        self.chain[self.cyclic_pos as usize] = current_match;
        let mut len_best = 0;

        if delta2 < self.cyclic_size
            && encoder.get_byte_by_pos(encoder.read_pos - delta2)
                == encoder.get_byte_by_pos(encoder.read_pos)
        {
            len_best = 2;
            matches.len[0] = 2;
            matches.dist[0] = delta2 - 1;
            matches.count = 1;
        }

        if delta2 != delta3
            && delta3 < self.cyclic_size
            && encoder.get_byte(0, delta3) == encoder.get_current_byte()
        {
            len_best = 3;
            let count = matches.count as usize;
            matches.dist[count] = delta3 - 1;
            matches.count += 1;
            delta2 = delta3;
        }

        if matches.count > 0 {
            len_best = extend_match(
                encoder.buf.as_slice(),
                encoder.read_pos,
                len_best,
                delta2,
                match_len_limit,
            );

            let count = matches.count as usize;
            matches.len[count - 1] = len_best as u32;

            // Return if it is long enough (niceLen or reached the end of
            // the dictionary).
            if len_best >= nice_len_limit {
                return;
            }
        }

        if len_best < 3 {
            len_best = 3;
        }

        let mut depth = self.depth_limit;
        loop {
            let delta = self.lz_pos - current_match;
            if {
                let tmp = depth;
                depth -= 1;
                tmp
            } == 0
                || delta >= self.cyclic_size
            {
                return;
            }
            let i = self.cyclic_pos - delta
                + if delta > self.cyclic_pos {
                    self.cyclic_size
                } else {
                    0
                };
            current_match = self.chain[i as usize];

            if encoder.get_byte(len_best, delta) == encoder.get_byte(len_best, 0)
                && encoder.get_byte(0, delta) == encoder.get_current_byte()
            {
                // Calculate the length of the match.
                let len = extend_match(
                    encoder.buf.as_slice(),
                    encoder.read_pos,
                    1,
                    delta,
                    match_len_limit,
                );

                // Use the match if and only if it is better than the longest
                // match found so far.
                if len > len_best {
                    len_best = len;
                    let count = matches.count as usize;
                    matches.len[count] = len as _;
                    matches.dist[count] = (delta - 1) as _;
                    matches.count += 1;

                    // Return if it is long enough (niceLen or reached the
                    // end of the dictionary).
                    if len >= nice_len_limit {
                        return;
                    }
                }
            }
        }
    }

    fn skip(&mut self, encoder: &mut LZEncoderData, mut len: usize) {
        while len > 0 {
            len -= 1;
            if self.move_pos(encoder) != 0 {
                self.hash.calc_hashes(encoder.read_buffer());
                self.chain[self.cyclic_pos as usize] = self.hash.get_hash4_pos();
                self.hash.update_tables(self.lz_pos);
            }
        }
    }
}

/// Extends a match to its maximum possible length within a specified limit.
///
/// This function is optimized using native word-at-a-time comparisons.
#[cfg(feature = "optimization")]
#[inline(always)]
fn extend_match(buf: &[u8], read_pos: i32, current_len: i32, distance: i32, limit: i32) -> i32 {
    const WORD_SIZE: usize = size_of::<usize>();

    // Safety: The following unsafe blog is safe because we properly bound check.
    assert!(read_pos >= distance, "lower bound check");
    assert!(read_pos + limit <= buf.len() as i32, "upper bound check");

    let extension_limit = (limit - current_len) as usize;

    unsafe {
        let mut extended_len = 0;

        let mut ptr1 = buf.as_ptr().add((read_pos + current_len) as usize);
        let mut ptr2 = ptr1.sub(distance as usize);

        while extended_len + WORD_SIZE <= extension_limit {
            let word1 = ptr1.cast::<usize>().read_unaligned();
            let word2 = ptr2.cast::<usize>().read_unaligned();

            if word1 == word2 {
                extended_len += WORD_SIZE;
                ptr1 = ptr1.add(WORD_SIZE);
                ptr2 = ptr2.add(WORD_SIZE);
            } else {
                let diff_bits = word1 ^ word2;
                #[cfg(target_endian = "little")]
                let matching_bytes = (diff_bits.trailing_zeros() / 8) as usize;
                #[cfg(target_endian = "big")]
                let matching_bytes = (diff_bits.leading_zeros() / 8) as usize;
                return current_len + (extended_len + matching_bytes) as i32;
            }
        }

        while extended_len < extension_limit && *ptr1 == *ptr2 {
            extended_len += 1;
            ptr1 = ptr1.add(1);
            ptr2 = ptr2.add(1);
        }

        current_len + extended_len as i32
    }
}

/// Extends a match to its maximum possible length within a specified limit.
///
/// Unoptimized byte for byte version.
#[cfg(not(feature = "optimization"))]
#[inline(always)]
fn extend_match(buf: &[u8], read_pos: i32, current_len: i32, distance: i32, limit: i32) -> i32 {
    let extension_limit = limit - current_len;

    if extension_limit == 0 {
        return current_len;
    }

    let start1 = (read_pos + current_len) as usize;
    let s1 = &buf[start1..start1 + extension_limit as usize];

    let start2 = start1 - distance as usize;
    let s2 = &buf[start2..start2 + extension_limit as usize];

    let extended_len = s1
        .iter()
        .zip(s2.iter())
        .take_while(|&(byte1, byte2)| byte1 == byte2)
        .count();

    current_len + (extended_len as i32)
}
