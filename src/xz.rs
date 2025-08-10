//! XZ format decoder and encoder implementation.

mod reader;
#[cfg(feature = "encoder")]
mod writer;
#[cfg(all(feature = "encoder", feature = "std"))]
mod writer_mt;

pub use reader::XZReader;
use sha2::Digest;
#[cfg(feature = "encoder")]
pub use writer::{XZOptions, XZWriter};
#[cfg(all(feature = "encoder", feature = "std"))]
pub use writer_mt::XZWriterMT;

use crate::{error_invalid_data, error_invalid_input, ByteReader, ByteWriter, Read, Write};

const CRC32: crc::Crc<u32, crc::Table<16>> =
    crc::Crc::<u32, crc::Table<16>>::new(&crc::CRC_32_ISO_HDLC);
const CRC64: crc::Crc<u64, crc::Table<16>> = crc::Crc::<u64, crc::Table<16>>::new(&crc::CRC_64_XZ);

/// XZ stream magic bytes: 0xFD, '7', 'z', 'X', 'Z', 0x00
const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// XZ stream footer magic bytes.
const XZ_FOOTER_MAGIC: [u8; 2] = [b'Y', b'Z'];

/// XZ Index record containing block metadata.
#[derive(Debug, Clone)]
pub(crate) struct IndexRecord {
    unpadded_size: u64,
    uncompressed_size: u64,
}

/// Configuration for a filter in the XZ filter chain.
#[derive(Debug, Clone)]
pub struct FilterConfig {
    pub filter_type: FilterType,
    pub property: u32,
}

impl FilterConfig {
    /// Creates a new delta filter configuration.
    pub fn new_delta(distance: u32) -> Self {
        Self {
            filter_type: FilterType::Delta,
            property: distance,
        }
    }

    /// Creates a new BCJ x86 filter configuration.
    pub fn new_bcj_x86(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjX86,
            property: start_pos,
        }
    }

    /// Creates a new BCJ ARM filter configuration.
    pub fn new_bcj_arm(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjARM,
            property: start_pos,
        }
    }

    /// Creates a new BCJ ARM Thumb filter configuration.
    pub fn new_bcj_arm_thumb(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjARMThumb,
            property: start_pos,
        }
    }

    /// Creates a new BCJ ARM64 filter configuration.
    pub fn new_bcj_arm64(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjARM64,
            property: start_pos,
        }
    }

    /// Creates a new BCJ IA64 filter configuration.
    pub fn new_bcj_ia64(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjIA64,
            property: start_pos,
        }
    }

    /// Creates a new BCJ PPC filter configuration.
    pub fn new_bcj_ppc(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjPPC,
            property: start_pos,
        }
    }

    /// Creates a new BCJ SPARC filter configuration.
    pub fn new_bcj_sparc(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjSPARC,
            property: start_pos,
        }
    }

    /// Creates a new BCJ RISC-V filter configuration.
    pub fn new_bcj_risc_v(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjRISCV,
            property: start_pos,
        }
    }
}

/// Supported checksum types in XZ format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckType {
    /// No checksum
    None = 0x00,
    /// CRC32
    Crc32 = 0x01,
    /// CRC64
    Crc64 = 0x04,
    /// SHA-256
    Sha256 = 0x0A,
}

impl CheckType {
    fn from_byte(byte: u8) -> crate::Result<Self> {
        match byte {
            0x00 => Ok(CheckType::None),
            0x01 => Ok(CheckType::Crc32),
            0x04 => Ok(CheckType::Crc64),
            0x0A => Ok(CheckType::Sha256),
            _ => Err(error_invalid_data("unsupported XZ check type")),
        }
    }

    #[cfg(feature = "encoder")]
    fn checksum_size(self) -> u64 {
        match self {
            CheckType::None => 0,
            CheckType::Crc32 => 4,
            CheckType::Crc64 => 8,
            CheckType::Sha256 => 32,
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum FilterType {
    /// Delta filter
    Delta,
    /// BCJ x86 filter
    BcjX86,
    /// BCJ PowerPC filter
    BcjPPC,
    /// BCJ IA64 filter
    BcjIA64,
    /// BCJ ARM filter
    BcjARM,
    /// BCJ ARM Thumb
    BcjARMThumb,
    /// BCJ SPARC filter
    BcjSPARC,
    /// BCJ ARM64 filter
    BcjARM64,
    /// BCJ RISC-V filter
    BcjRISCV,
    /// LZMA2 filter
    LZMA2,
}

impl TryFrom<u64> for FilterType {
    type Error = ();

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0x03 => Ok(FilterType::Delta),
            0x04 => Ok(FilterType::BcjX86),
            0x05 => Ok(FilterType::BcjPPC),
            0x06 => Ok(FilterType::BcjIA64),
            0x07 => Ok(FilterType::BcjARM),
            0x08 => Ok(FilterType::BcjARMThumb),
            0x09 => Ok(FilterType::BcjSPARC),
            0x0A => Ok(FilterType::BcjARM64),
            0x0B => Ok(FilterType::BcjRISCV),
            0x21 => Ok(FilterType::LZMA2),
            _ => Err(()),
        }
    }
}

/// Parse XZ multibyte integer (variable length encoding).
fn parse_multibyte_integer(data: &[u8]) -> crate::Result<u64> {
    let mut result = 0u64;
    let mut shift = 0;

    for &byte in data {
        if shift >= 63 {
            return Err(error_invalid_data("XZ multibyte integer too large"));
        }

        result |= ((byte & 0x7F) as u64) << shift;
        shift += 7;

        if (byte & 0x80) == 0 {
            return Ok(result);
        }
    }

    Err(error_invalid_data("incomplete XZ multibyte integer"))
}

/// Count the number of bytes used by a multibyte integer.
fn count_multibyte_integer_size(data: &[u8]) -> usize {
    for (i, &byte) in data.iter().enumerate() {
        if (byte & 0x80) == 0 {
            return i + 1;
        }
    }
    data.len()
}

fn parse_multibyte_integer_from_reader<R: Read>(reader: &mut R) -> crate::Result<u64> {
    let mut result = 0u64;
    let mut shift = 0;

    for _ in 0..9 {
        // Max 9 bytes for 63-bit value
        let byte = reader.read_u8()?;

        if shift >= 63 {
            return Err(error_invalid_data("XZ multibyte integer too large"));
        }

        result |= ((byte & 0x7F) as u64) << shift;
        shift += 7;

        if (byte & 0x80) == 0 {
            return Ok(result);
        }
    }

    Err(error_invalid_data("XZ multibyte integer too long"))
}

fn count_multibyte_integer_size_for_value(mut value: u64) -> usize {
    if value == 0 {
        return 1;
    }

    let mut count = 0;
    while value > 0 {
        count += 1;
        value >>= 7;
    }
    count
}

fn encode_multibyte_integer(mut value: u64, buf: &mut [u8]) -> crate::Result<usize> {
    if value > (u64::MAX / 2) {
        return Err(error_invalid_data("value too big to encode"));
    }

    let mut i = 0;
    while value >= 0x80 && i < buf.len() {
        buf[i] = (value as u8) | 0x80;
        value >>= 7;
        i += 1;
    }

    if i < buf.len() {
        buf[i] = value as u8;
        i += 1;
    }

    Ok(i)
}

/// Handles checksum calculation for different XZ check types.
enum ChecksumCalculator {
    None,
    Crc32(crc::Digest<'static, u32, crc::Table<16>>),
    Crc64(crc::Digest<'static, u64, crc::Table<16>>),
    Sha256(sha2::Sha256),
}

impl ChecksumCalculator {
    fn new(check_type: CheckType) -> Self {
        match check_type {
            CheckType::None => Self::None,
            CheckType::Crc32 => Self::Crc32(CRC32.digest()),
            CheckType::Crc64 => Self::Crc64(CRC64.digest()),
            CheckType::Sha256 => Self::Sha256(sha2::Sha256::new()),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            ChecksumCalculator::None => {}
            ChecksumCalculator::Crc32(crc) => {
                crc.update(data);
            }
            ChecksumCalculator::Crc64(crc) => {
                crc.update(data);
            }
            ChecksumCalculator::Sha256(sha) => {
                sha.update(data);
            }
        }
    }

    fn verify(self, expected: &[u8]) -> bool {
        match self {
            ChecksumCalculator::None => true,
            ChecksumCalculator::Crc32(crc) => {
                if expected.len() != 4 {
                    return false;
                }

                let expected_crc =
                    u32::from_le_bytes([expected[0], expected[1], expected[2], expected[3]]);

                let final_crc = crc.finalize();

                final_crc == expected_crc
            }
            ChecksumCalculator::Crc64(crc) => {
                if expected.len() != 8 {
                    return false;
                }

                let expected_crc = u64::from_le_bytes([
                    expected[0],
                    expected[1],
                    expected[2],
                    expected[3],
                    expected[4],
                    expected[5],
                    expected[6],
                    expected[7],
                ]);

                let final_crc = crc.finalize();

                final_crc == expected_crc
            }
            ChecksumCalculator::Sha256(sha) => {
                if expected.len() != 32 {
                    return false;
                }

                let final_sha = sha.finalize();

                &final_sha[..32] == expected
            }
        }
    }

    #[cfg(feature = "encoder")]
    fn finalize_to_bytes(self) -> Vec<u8> {
        match self {
            ChecksumCalculator::None => Vec::new(),
            ChecksumCalculator::Crc32(crc) => crc.finalize().to_le_bytes().to_vec(),
            ChecksumCalculator::Crc64(crc) => crc.finalize().to_le_bytes().to_vec(),
            ChecksumCalculator::Sha256(sha) => sha.finalize().to_vec(),
        }
    }
}

#[cfg(feature = "encoder")]
fn write_xz_stream_header<W: Write + ?Sized>(
    mut writer: &mut W,
    check_type: CheckType,
) -> crate::Result<()> {
    writer.write_all(&XZ_MAGIC)?;

    let stream_flags = [0u8, check_type as u8];
    writer.write_all(&stream_flags)?;

    let crc = CRC32.checksum(&stream_flags);
    writer.write_u32(crc)?;

    Ok(())
}

#[cfg(feature = "encoder")]
fn encode_lzma2_dict_size(dict_size: u32) -> crate::Result<u8> {
    if dict_size < 4096 {
        return Err(error_invalid_input("LZMA2 dictionary size too small"));
    }

    if dict_size == 0xFFFFFFFF {
        return Ok(40);
    }

    // Find the appropriate property value.
    for prop in 0u8..40 {
        let base = 2 | ((prop & 1) as u32);
        let size = base << (prop / 2 + 11);

        if size >= dict_size {
            return Ok(prop);
        }
    }

    Err(error_invalid_input("LZMA2 dictionary size too large"))
}

fn update_crc_with_padding(crc: &mut crc::Digest<'_, u32, crc::Table<16>>, padding_needed: usize) {
    match padding_needed {
        1 => crc.update(&[0]),
        2 => crc.update(&[0, 0]),
        3 => crc.update(&[0, 0, 0]),
        _ => {}
    }
}

#[cfg(feature = "encoder")]
fn add_padding<W: Write + ?Sized>(writer: &mut W, padding_needed: usize) -> crate::Result<()> {
    match padding_needed {
        1 => writer.write_all(&[0]),
        2 => writer.write_all(&[0, 0]),
        3 => writer.write_all(&[0, 0, 0]),
        _ => Ok(()),
    }
}

#[cfg(feature = "encoder")]
fn generate_block_header_data(
    filters: &[FilterConfig],
    lzma_dict_size: u32,
) -> crate::Result<Vec<u8>> {
    let mut header_data = Vec::new();
    let num_filters = filters.len();

    if num_filters > 4 {
        return Err(error_invalid_input("too many filters in chain (maximum 4)"));
    }

    // Block flags: no compressed size, no uncompressed size, filter count
    let block_flags = (num_filters - 1) as u8; // -1 because 0 means 1 filter, 3 means 4 filters
    header_data.push(block_flags);

    let mut temp_buf = [0u8; 10];

    for filter_config in filters {
        // Write filter ID.
        let filter_id = match filter_config.filter_type {
            FilterType::Delta => 0x03,
            FilterType::BcjX86 => 0x04,
            FilterType::BcjPPC => 0x05,
            FilterType::BcjIA64 => 0x06,
            FilterType::BcjARM => 0x07,
            FilterType::BcjARMThumb => 0x08,
            FilterType::BcjSPARC => 0x09,
            FilterType::BcjARM64 => 0x0A,
            FilterType::BcjRISCV => 0x0B,
            FilterType::LZMA2 => 0x21,
        };
        let size = encode_multibyte_integer(filter_id, &mut temp_buf)?;
        header_data.extend_from_slice(&temp_buf[..size]);

        // Write filter properties.
        match filter_config.filter_type {
            FilterType::Delta => {
                // Properties size (1 byte)
                let size = encode_multibyte_integer(1, &mut temp_buf)?;
                header_data.extend_from_slice(&temp_buf[..size]);
                // Distance property (encoded as distance - 1)
                let distance_prop = (filter_config.property - 1) as u8;
                header_data.push(distance_prop);
            }
            FilterType::BcjX86
            | FilterType::BcjPPC
            | FilterType::BcjIA64
            | FilterType::BcjARM
            | FilterType::BcjARMThumb
            | FilterType::BcjSPARC
            | FilterType::BcjARM64
            | FilterType::BcjRISCV => {
                if filter_config.property == 0 {
                    // No start offset.
                    let size = encode_multibyte_integer(0, &mut temp_buf)?;
                    header_data.extend_from_slice(&temp_buf[..size]);
                } else {
                    // 4-byte start offset.
                    let size = encode_multibyte_integer(4, &mut temp_buf)?;
                    header_data.extend_from_slice(&temp_buf[..size]);
                    header_data.extend_from_slice(&filter_config.property.to_le_bytes());
                }
            }
            FilterType::LZMA2 => {
                let size = encode_multibyte_integer(1, &mut temp_buf)?;
                header_data.extend_from_slice(&temp_buf[..size]);

                let dict_size_prop = encode_lzma2_dict_size(lzma_dict_size)?;
                header_data.push(dict_size_prop);
            }
        }
    }

    Ok(header_data)
}

#[cfg(feature = "encoder")]
fn write_xz_block_header<W: Write + ?Sized>(
    mut writer: &mut W,
    filters: &[FilterConfig],
    lzma_dict_size: u32,
) -> crate::Result<u64> {
    let header_data = generate_block_header_data(filters, lzma_dict_size)?;

    // Calculate header size (including size byte and CRC32, rounded up to multiple of 4)
    let total_size_needed: usize = 1 + header_data.len() + 4;
    let header_size = total_size_needed.div_ceil(4) * 4;
    let header_size_encoded = ((header_size / 4) - 1) as u8;

    let padding_needed = header_size - 1 - header_data.len() - 4;

    // Calculate and write CRC32 of header size byte + header data + padding
    let mut crc = CRC32.digest();
    crc.update(&[header_size_encoded]);
    crc.update(&header_data);
    update_crc_with_padding(&mut crc, padding_needed);

    let crc_value = crc.finalize();

    // Now write everything to the writer
    writer.write_u8(header_size_encoded)?;
    writer.write_all(&header_data)?;
    add_padding(writer, padding_needed)?;
    writer.write_u32(crc_value)?;

    Ok(header_size as u64)
}

#[cfg(feature = "encoder")]
fn write_xz_index<W: Write + ?Sized>(
    mut writer: &mut W,
    index_records: &[IndexRecord],
) -> crate::Result<()> {
    let mut index_data = Vec::new();

    let mut temp_buf = [0u8; 10];
    let size = encode_multibyte_integer(index_records.len() as u64, &mut temp_buf)?;
    index_data.extend_from_slice(&temp_buf[..size]);

    for record in index_records {
        let size = encode_multibyte_integer(record.unpadded_size, &mut temp_buf)?;
        index_data.extend_from_slice(&temp_buf[..size]);

        let size = encode_multibyte_integer(record.uncompressed_size, &mut temp_buf)?;
        index_data.extend_from_slice(&temp_buf[..size]);
    }

    let bytes_written = 1 + index_data.len(); // indicator + index data
    let padding_needed = (4 - (bytes_written % 4)) % 4;

    let mut crc = CRC32.digest();
    crc.update(&[0x00]);
    crc.update(&index_data);
    update_crc_with_padding(&mut crc, padding_needed);

    let crc_value = crc.finalize();

    // Index indicator (0x00).
    writer.write_u8(0x00)?;
    writer.write_all(&index_data)?;
    add_padding(writer, padding_needed)?;
    writer.write_u32(crc_value)?;

    Ok(())
}

#[cfg(feature = "encoder")]
fn write_xz_stream_footer<W: Write + ?Sized>(
    mut writer: &mut W,
    index_records: &[IndexRecord],
    check_type: CheckType,
) -> crate::Result<()> {
    // Calculate backward size (index size in 4-byte blocks).
    let mut index_size = 1; // indicator
    index_size += count_multibyte_integer_size_for_value(index_records.len() as u64);

    for record in index_records {
        index_size += count_multibyte_integer_size_for_value(record.unpadded_size);
        index_size += count_multibyte_integer_size_for_value(record.uncompressed_size);
    }

    let padding_needed = (4 - (index_size % 4)) % 4;
    index_size += padding_needed;
    index_size += 4; // CRC32

    let backward_size = ((index_size / 4) - 1) as u32;

    // Stream flags (same as header).
    let stream_flags = [0u8, check_type as u8];

    // Calculate CRC32 of backward size + stream flags
    let mut crc = CRC32.digest();
    crc.update(&backward_size.to_le_bytes());
    crc.update(&stream_flags);

    writer.write_u32(crc.finalize())?;
    writer.write_u32(backward_size)?;
    writer.write_all(&stream_flags)?;
    writer.write_all(&XZ_FOOTER_MAGIC)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_multibyte_integer() {
        let values = [0, 127, 128, 16383, 16384, 2097151, 2097152];

        for &value in &values {
            let mut buf = [0u8; 9];
            let encoded_size = encode_multibyte_integer(value, &mut buf).unwrap();

            let decoded = parse_multibyte_integer(&buf[..encoded_size]).unwrap();
            assert_eq!(decoded, value);

            let size_for_value = count_multibyte_integer_size_for_value(value);
            assert_eq!(size_for_value, encoded_size);
        }
    }

    #[test]
    fn test_multibyte_integer_limits() {
        // Test maximum allowed value (63 bits)
        let max_value = u64::MAX / 2;
        let mut buf = [0u8; 9];
        let encoded_size = encode_multibyte_integer(max_value, &mut buf).unwrap();

        let decoded = parse_multibyte_integer(&buf[..encoded_size]).unwrap();
        assert_eq!(decoded, max_value);

        // Test value that's too large
        let too_large = u64::MAX;
        let encoded_size = encode_multibyte_integer(too_large, &mut buf);
        assert!(encoded_size.is_err());
    }

    #[test]
    fn test_index_record_creation() {
        let record = IndexRecord {
            unpadded_size: 1024,
            uncompressed_size: 2048,
        };

        assert_eq!(record.unpadded_size, 1024);
        assert_eq!(record.uncompressed_size, 2048);
    }

    #[test]
    fn test_checksum_calculator_crc32() {
        let mut calc = ChecksumCalculator::new(CheckType::Crc32);
        calc.update(b"123456789");

        // CRC32 of "123456789" in little-endian format
        let expected = [0x26, 0x39, 0xF4, 0xCB];
        assert!(calc.verify(&expected));
    }

    #[test]
    fn test_checksum_calculator_crc64() {
        let mut calc = ChecksumCalculator::new(CheckType::Crc64);
        calc.update(b"123456789");

        // CRC64 of "123456789" in little-endian format.
        let expected = [250, 57, 25, 223, 187, 201, 93, 153];
        assert!(calc.verify(&expected));
    }

    #[test]
    fn test_checksum_calculator_sha256() {
        let mut calc = ChecksumCalculator::new(CheckType::Sha256);
        calc.update(b"123456789");

        // SHA256 of "123456789"
        let expected = [
            21, 226, 176, 211, 195, 56, 145, 235, 176, 241, 239, 96, 158, 196, 25, 66, 12, 32, 227,
            32, 206, 148, 198, 95, 188, 140, 51, 18, 68, 142, 178, 37,
        ];
        assert!(calc.verify(&expected));
    }
}
