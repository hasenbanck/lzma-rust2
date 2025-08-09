//! LZIP format decoder implementation.

mod reader;

pub use reader::LZIPReader;

use crate::{error_invalid_data, ByteReader, Read, Result};

/// CRC32 used in LZIP format (same as in ZIP, PNG, etc.)
const CRC32: crc::Crc<u32, crc::Table<16>> =
    crc::Crc::<u32, crc::Table<16>>::new(&crc::CRC_32_ISO_HDLC);

/// LZIP magic bytes: "LZIP"
const LZIP_MAGIC: [u8; 4] = [b'L', b'Z', b'I', b'P'];

/// LZIP version number (currently 1)
const LZIP_VERSION: u8 = 1;

/// Header size in bytes
const HEADER_SIZE: usize = 6;

/// Trailer size in bytes  
const TRAILER_SIZE: usize = 20;

/// Minimum dictionary size (4 KiB)
const MIN_DICT_SIZE: u32 = 4 * 1024;

/// Maximum dictionary size (512 MiB)
const MAX_DICT_SIZE: u32 = 512 * 1024 * 1024;

/// LZIP header structure (6 bytes)
#[derive(Debug, Clone)]
pub(crate) struct LZIPHeader {
    version: u8,
    dict_size: u32,
}

impl LZIPHeader {
    /// Parse LZIP header from reader
    fn parse<R: Read>(reader: &mut R) -> Result<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;

        if magic != LZIP_MAGIC {
            return Err(error_invalid_data("invalid LZIP magic bytes"));
        }

        let version = reader.read_u8()?;
        if version != LZIP_VERSION {
            return Err(error_invalid_data("unsupported LZIP version"));
        }

        let dict_size_byte = reader.read_u8()?;
        let dict_size = decode_dict_size(dict_size_byte)?;

        Ok(LZIPHeader { version, dict_size })
    }
}

/// LZIP trailer structure (20 bytes)
#[derive(Debug, Clone)]
pub(crate) struct LZIPTrailer {
    crc32: u32,
    data_size: u64,
    member_size: u64,
}

impl LZIPTrailer {
    /// Parse LZIP trailer from reader
    fn parse<R: Read>(reader: &mut R) -> Result<Self> {
        let crc32 = reader.read_u32()?;
        let data_size = reader.read_u64()?;
        let member_size = reader.read_u64()?;

        Ok(LZIPTrailer {
            crc32,
            data_size,
            member_size,
        })
    }
}

/// Decode LZIP dictionary size from encoded byte:
///
/// The dictionary size is calculated by taking a power of 2 (the base size)
/// and subtracting from it a fraction between 0/16 and 7/16 of the base size.
///
/// - Bits 4-0: contain the base 2 logarithm of the base size (12 to 29)
/// - Bits 7-5: contain the numerator of the fraction (0 to 7) to subtract
///
/// Example: 0xD3 = 2^19 - 6 * 2^15 = 512 KiB - 6 * 32 KiB = 320 KiB
fn decode_dict_size(encoded: u8) -> Result<u32> {
    let base_log2 = (encoded & 0x1F) as u32;
    let fraction_num = (encoded >> 5) as u32;

    if base_log2 < 12 || base_log2 > 29 {
        return Err(error_invalid_data("invalid LZIP dictionary size base"));
    }

    if fraction_num > 7 {
        return Err(error_invalid_data("invalid LZIP dictionary size fraction"));
    }

    let base_size = 1u32 << base_log2;
    let fraction_size = if base_log2 >= 4 {
        (base_size >> 4) * fraction_num
    } else {
        0 // Should not happen with base_log2 >= 12
    };

    let dict_size = base_size - fraction_size;

    if dict_size < MIN_DICT_SIZE || dict_size > MAX_DICT_SIZE {
        return Err(error_invalid_data("LZIP dictionary size out of range"));
    }

    Ok(dict_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_dict_size() {
        let dict_size = decode_dict_size(0xD3).unwrap();
        assert_eq!(dict_size, 320 * 1024);

        let dict_size = decode_dict_size(0x0C).unwrap(); // base_log2=12, fraction=0
        assert_eq!(dict_size, 4 * 1024);

        let dict_size = decode_dict_size(0x1D).unwrap(); // base_log2=29, fraction=0
        assert_eq!(dict_size, 512 * 1024 * 1024);

        assert!(decode_dict_size(0x0B).is_err());

        assert!(decode_dict_size(0x1E).is_err());
    }
}
