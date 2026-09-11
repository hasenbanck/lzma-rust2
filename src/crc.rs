//! The CRC-32 (ISO-HDLC) of lzip and xz and the CRC-64 (XZ) of xz, eight
//! bytes at a time.
//!
//! Both are the byte at a time table method unrolled eight times over eight
//! tables ("slicing by eight"). Table `k` carries each byte's contribution
//! from `k` bytes further back, so one step takes eight bytes with eight
//! lookups and no dependency between them, where the byte at a time method
//! has one lookup wait on the last. On aarch64 with the `optimization`
//! feature the CRC-32 uses the hardware instruction where the processor has
//! it.

const CRC32_POLY: u32 = 0xEDB88320;

const fn make_crc32_tables() -> [[u32; 256]; 8] {
    let mut tables = [[0; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ CRC32_POLY;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        tables[0][i] = crc;
        i += 1;
    }
    let mut i = 0;
    while i < 256 {
        let mut crc = tables[0][i];
        let mut k = 1;
        while k < 8 {
            crc = (crc >> 8) ^ tables[0][(crc & 0xFF) as usize];
            tables[k][i] = crc;
            k += 1;
        }
        i += 1;
    }
    tables
}

const CRC32_TABLES: [[u32; 256]; 8] = make_crc32_tables();

/// CRC_32_ISO_HDLC
#[derive(Clone, Copy)]
#[repr(transparent)]
pub(crate) struct Crc32 {
    state: u32,
}

impl Crc32 {
    pub(crate) fn new() -> Self {
        Self { state: 0xFFFFFFFF }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        #[cfg(all(feature = "optimization", target_arch = "aarch64"))]
        if aarch64::has_crc() {
            // SAFETY: the processor has the CRC instructions.
            self.state = unsafe { aarch64::crc32(self.state, data) };
            return;
        }
        self.state = crc32_by_eight(self.state, data);
    }

    pub(crate) fn finalize(self) -> u32 {
        self.state ^ 0xFFFFFFFF
    }

    pub(crate) fn checksum(data: &[u8]) -> u32 {
        let mut crc = Self::new();
        crc.update(data);
        crc.finalize()
    }
}

#[inline(always)]
fn crc32_byte(crc: u32, byte: u8) -> u32 {
    (crc >> 8) ^ CRC32_TABLES[0][((crc ^ byte as u32) & 0xFF) as usize]
}

fn crc32_by_eight(mut crc: u32, data: &[u8]) -> u32 {
    let mut chunks = data.chunks_exact(8);
    for chunk in &mut chunks {
        let low = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) ^ crc;
        let high = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        crc = CRC32_TABLES[7][(low & 0xFF) as usize]
            ^ CRC32_TABLES[6][((low >> 8) & 0xFF) as usize]
            ^ CRC32_TABLES[5][((low >> 16) & 0xFF) as usize]
            ^ CRC32_TABLES[4][(low >> 24) as usize]
            ^ CRC32_TABLES[3][(high & 0xFF) as usize]
            ^ CRC32_TABLES[2][((high >> 8) & 0xFF) as usize]
            ^ CRC32_TABLES[1][((high >> 16) & 0xFF) as usize]
            ^ CRC32_TABLES[0][(high >> 24) as usize];
    }
    for &byte in chunks.remainder() {
        crc = crc32_byte(crc, byte);
    }
    crc
}

const CRC64_POLY: u64 = 0xC96C5795D7870F42;

const fn make_crc64_tables() -> [[u64; 256]; 8] {
    let mut tables = [[0; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u64;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ CRC64_POLY;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        tables[0][i] = crc;
        i += 1;
    }
    let mut i = 0;
    while i < 256 {
        let mut crc = tables[0][i];
        let mut k = 1;
        while k < 8 {
            crc = (crc >> 8) ^ tables[0][(crc & 0xFF) as usize];
            tables[k][i] = crc;
            k += 1;
        }
        i += 1;
    }
    tables
}

const CRC64_TABLES: [[u64; 256]; 8] = make_crc64_tables();

/// CRC_64_XZ
#[derive(Clone, Copy)]
#[repr(transparent)]
pub(crate) struct Crc64 {
    state: u64,
}

impl Crc64 {
    pub(crate) fn new() -> Self {
        Self {
            state: 0xFFFFFFFFFFFFFFFF,
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        self.state = crc64_by_eight(self.state, data);
    }

    pub(crate) fn finalize(self) -> u64 {
        self.state ^ 0xFFFFFFFFFFFFFFFF
    }

    pub(crate) fn checksum(data: &[u8]) -> u64 {
        let mut crc = Self::new();
        crc.update(data);
        crc.finalize()
    }
}

#[inline(always)]
fn crc64_byte(crc: u64, byte: u8) -> u64 {
    (crc >> 8) ^ CRC64_TABLES[0][((crc ^ byte as u64) & 0xFF) as usize]
}

fn crc64_by_eight(mut crc: u64, data: &[u8]) -> u64 {
    let mut chunks = data.chunks_exact(8);
    for chunk in &mut chunks {
        let word = u64::from_le_bytes([
            chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
        ]) ^ crc;
        crc = CRC64_TABLES[7][(word & 0xFF) as usize]
            ^ CRC64_TABLES[6][((word >> 8) & 0xFF) as usize]
            ^ CRC64_TABLES[5][((word >> 16) & 0xFF) as usize]
            ^ CRC64_TABLES[4][((word >> 24) & 0xFF) as usize]
            ^ CRC64_TABLES[3][((word >> 32) & 0xFF) as usize]
            ^ CRC64_TABLES[2][((word >> 40) & 0xFF) as usize]
            ^ CRC64_TABLES[1][((word >> 48) & 0xFF) as usize]
            ^ CRC64_TABLES[0][(word >> 56) as usize];
    }
    for &byte in chunks.remainder() {
        crc = crc64_byte(crc, byte);
    }
    crc
}

/// The CRC-32 instructions of aarch64 (the `crc` feature, in every Armv8.1
/// processor and most Armv8.0 ones), a word at a time.
#[cfg(all(feature = "optimization", target_arch = "aarch64"))]
mod aarch64 {
    use core::arch::asm;

    /// Whether the processor has the instructions: known at compile time
    /// when the target enables them, found at run time with `std`, and
    /// assumed absent otherwise.
    #[inline(always)]
    pub(super) fn has_crc() -> bool {
        #[cfg(target_feature = "crc")]
        {
            true
        }
        #[cfg(all(not(target_feature = "crc"), feature = "std"))]
        {
            std::arch::is_aarch64_feature_detected!("crc")
        }
        #[cfg(all(not(target_feature = "crc"), not(feature = "std")))]
        {
            false
        }
    }

    /// # Safety
    ///
    /// The processor must have the CRC instructions: `has_crc()`.
    #[target_feature(enable = "crc")]
    pub(super) unsafe fn crc32(mut crc: u32, data: &[u8]) -> u32 {
        let mut chunks = data.chunks_exact(8);
        for chunk in &mut chunks {
            let word = u64::from_le_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ]);
            // SAFETY: the instruction is present (the caller's contract) and
            // touches only the named registers.
            unsafe {
                asm!(
                    "crc32x {crc:w}, {crc:w}, {word:x}",
                    crc = inout(reg) crc,
                    word = in(reg) word,
                    options(pure, nomem, nostack, preserves_flags),
                );
            }
        }
        for &byte in chunks.remainder() {
            // SAFETY: as above.
            unsafe {
                asm!(
                    "crc32b {crc:w}, {crc:w}, {byte:w}",
                    crc = inout(reg) crc,
                    byte = in(reg) byte as u32,
                    options(pure, nomem, nostack, preserves_flags),
                );
            }
        }
        crc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_DATA: &[u8] = &[1, 2, 3, 7, 16, 31, 64, 255];

    #[test]
    fn crc32_empty() {
        assert_eq!(Crc32::checksum(&[]), 0);
    }

    #[test]
    fn crc64_empty() {
        assert_eq!(Crc64::checksum(&[]), 0);
    }

    #[test]
    fn crc32_simple_data() {
        assert_eq!(Crc32::checksum(SIMPLE_DATA), 2428203834);
    }

    #[test]
    fn crc64_simple_data() {
        assert_eq!(Crc64::checksum(SIMPLE_DATA), 11721292222009571391);
    }

    /// A few hundred bytes of a simple generator, taken in pieces of every
    /// length and alignment, against the byte at a time method.
    fn pieces() -> alloc::vec::Vec<u8> {
        let mut state = 0x2545F491u32;
        (0..300)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn crc32_matches_the_byte_at_a_time_method() {
        let data = pieces();
        for start in 0..16 {
            for len in 0..40 {
                let piece = &data[start..start + len];
                let expected = piece.iter().fold(0xFFFFFFFF, |crc, &b| crc32_byte(crc, b));
                assert_eq!(crc32_by_eight(0xFFFFFFFF, piece), expected, "{start} {len}");
                assert_eq!(
                    Crc32::checksum(piece),
                    expected ^ 0xFFFFFFFF,
                    "{start} {len}"
                );
            }
        }
        let whole = data.iter().fold(0xFFFFFFFF, |crc, &b| crc32_byte(crc, b));
        assert_eq!(Crc32::checksum(&data), whole ^ 0xFFFFFFFF);
        // In two pieces, the state carried across.
        let mut crc = Crc32::new();
        crc.update(&data[..131]);
        crc.update(&data[131..]);
        assert_eq!(crc.finalize(), whole ^ 0xFFFFFFFF);
    }

    #[test]
    fn crc64_matches_the_byte_at_a_time_method() {
        let data = pieces();
        for start in 0..16 {
            for len in 0..40 {
                let piece = &data[start..start + len];
                let expected = piece
                    .iter()
                    .fold(0xFFFFFFFFFFFFFFFF, |crc, &b| crc64_byte(crc, b));
                assert_eq!(
                    crc64_by_eight(0xFFFFFFFFFFFFFFFF, piece),
                    expected,
                    "{start} {len}"
                );
            }
        }
        let whole = data
            .iter()
            .fold(0xFFFFFFFFFFFFFFFF, |crc, &b| crc64_byte(crc, b));
        let mut crc = Crc64::new();
        crc.update(&data[..131]);
        crc.update(&data[131..]);
        assert_eq!(crc.finalize(), whole ^ 0xFFFFFFFFFFFFFFFF);
    }
}
