#[cfg(not(feature = "optimization"))]
use alloc::{vec, vec::Vec};

#[cfg(feature = "optimization")]
use super::AlignedMemoryI32;
use super::LZEncoder;

const HASH2_SIZE: u32 = 1 << 10;
const HASH2_MASK: u32 = HASH2_SIZE - 1;
const HASH3_SIZE: u32 = 1 << 16;
const HASH3_MASK: u32 = HASH3_SIZE - 1;

pub struct Hash234 {
    #[cfg(feature = "optimization")]
    hash2_table: AlignedMemoryI32,
    #[cfg(feature = "optimization")]
    hash3_table: AlignedMemoryI32,
    #[cfg(feature = "optimization")]
    hash4_table: AlignedMemoryI32,
    #[cfg(not(feature = "optimization"))]
    hash2_table: Vec<i32>,
    #[cfg(not(feature = "optimization"))]
    hash3_table: Vec<i32>,
    #[cfg(not(feature = "optimization"))]
    hash4_table: Vec<i32>,
    hash4_size: u32,
    hash4_mask: u32,
    hash2_value: i32,
    hash3_value: i32,
    hash4_value: i32,
}

impl Hash234 {
    fn get_hash4_size(dict_size: u32) -> u32 {
        let mut h = dict_size - 1;
        h |= h >> 1;
        h |= h >> 2;
        h |= h >> 4;
        h |= h >> 8;
        h >>= 1;
        h |= 0xFFFF;
        if h > (1 << 24) {
            h >>= 1;
        }
        h + 1
    }

    pub(crate) fn get_mem_usage(dict_size: u32) -> u32 {
        (HASH2_MASK + HASH2_SIZE + Self::get_hash4_size(dict_size)) / (1024 / 4) + 4
    }

    pub(crate) fn new(dict_size: u32) -> Self {
        let hash4_size = Self::get_hash4_size(dict_size);
        let hash4_mask = hash4_size - 1;

        #[cfg(feature = "optimization")]
        let hash2_table = AlignedMemoryI32::new(HASH2_SIZE as usize);
        #[cfg(feature = "optimization")]
        let hash3_table = AlignedMemoryI32::new(HASH3_SIZE as usize);
        #[cfg(feature = "optimization")]
        let hash4_table = AlignedMemoryI32::new(hash4_size as usize);

        #[cfg(not(feature = "optimization"))]
        let hash2_table = vec![0; HASH2_SIZE as usize];
        #[cfg(not(feature = "optimization"))]
        let hash3_table = vec![0; HASH3_SIZE as usize];
        #[cfg(not(feature = "optimization"))]
        let hash4_table = vec![0; hash4_size as usize];

        assert!(hash2_table.len() >= HASH2_SIZE as usize);
        assert!(hash3_table.len() >= HASH3_SIZE as usize);
        assert!(hash4_table.len() >= hash4_size as usize);

        Self {
            hash4_mask,
            hash2_table,
            hash3_table,
            hash4_table,
            hash4_size,
            hash2_value: 0,
            hash3_value: 0,
            hash4_value: 0,
        }
    }

    #[inline(always)]
    fn hash_byte(byte: u8) -> u32 {
        // Original CRC lookup replaced with a golden ratio constant as used for example TEA.
        // Is ever so slightly faster and also compresses constantly a little bit better.
        (byte as u32).wrapping_mul(0x9E3779B9)
    }

    #[inline(always)]
    pub(crate) fn calc_hashes(&mut self, buf: &[u8]) {
        let mut tmp: u32 = Self::hash_byte(buf[0]) ^ (buf[1] as u32);
        self.hash2_value = (tmp & HASH2_MASK) as i32;

        tmp ^= (buf[2] as u32) << 8;
        self.hash3_value = (tmp & HASH3_MASK) as i32;

        tmp ^= Self::hash_byte(buf[3]) << 5;
        self.hash4_value = (tmp & self.hash4_mask) as i32;
    }

    pub(crate) fn get_hash2_pos(&self) -> i32 {
        self.hash2_table[self.hash2_value as usize]
    }

    pub(crate) fn get_hash3_pos(&self) -> i32 {
        self.hash3_table[self.hash3_value as usize]
    }

    pub(crate) fn get_hash4_pos(&self) -> i32 {
        self.hash4_table[self.hash4_value as usize]
    }

    pub(crate) fn update_tables(&mut self, pos: i32) {
        self.hash2_table[self.hash2_value as usize] = pos;
        self.hash3_table[self.hash3_value as usize] = pos;
        self.hash4_table[self.hash4_value as usize] = pos;
    }

    pub(crate) fn normalize(&mut self, offset: i32) {
        LZEncoder::normalize(&mut self.hash2_table, offset);
        LZEncoder::normalize(&mut self.hash3_table, offset);
        LZEncoder::normalize(&mut self.hash4_table, offset);
    }
}
