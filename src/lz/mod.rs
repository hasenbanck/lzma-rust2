#[cfg(feature = "optimization")]
mod aligned_memory;
mod bt4;
mod hash234;
mod hc4;
mod lz_decoder;
mod lz_encoder;

#[cfg(feature = "optimization")]
pub(crate) use aligned_memory::*;
pub(crate) use lz_decoder::*;
pub use lz_encoder::*;

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
                #[cfg(all(
                    target_endian = "little",
                    not(all(target_arch = "x86_64", target_feature = "bmi1"))
                ))]
                let matching_bytes = (diff_bits.trailing_zeros() / 8) as usize;

                #[cfg(all(
                    target_endian = "little",
                    all(target_arch = "x86_64", target_feature = "bmi1")
                ))]
                let matching_bytes = (std::arch::x86_64::_tzcnt_u64(diff_bits as u64) / 8) as usize;

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
