//! XZ format decoder and encoder implementation.

mod reader;
#[cfg(feature = "std")]
mod reader_mt;
#[cfg(feature = "encoder")]
mod writer;
#[cfg(all(feature = "encoder", feature = "std"))]
mod writer_mt;

#[cfg(feature = "encoder")]
use alloc::vec::Vec;
use alloc::{boxed::Box, vec};
#[cfg(feature = "std")]
use std::io::{self, BufReader, Seek, SeekFrom};

pub use reader::{XzReader, XzStream};
#[cfg(feature = "std")]
pub use reader_mt::XzReaderMt;
use sha2::Digest;
#[cfg(feature = "encoder")]
pub use writer::{XzOptions, XzWriter};
#[cfg(all(feature = "encoder", feature = "std"))]
pub use writer_mt::XzWriterMt;

use crate::{
    ByteReader, Read,
    crc::{Crc32, Crc64},
    error_invalid_data, error_invalid_input, error_unsupported,
    filter::FilterType,
};
#[cfg(feature = "encoder")]
use crate::{ByteWriter, Write, filter::FilterConfig};
#[cfg(feature = "std")]
use crate::{
    Lzma2Reader,
    filter::{bcj::BcjReader, delta::DeltaReader},
};

const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

const XZ_FOOTER_MAGIC: [u8; 2] = *b"YZ";

#[derive(Debug, Clone)]
struct IndexRecord {
    unpadded_size: u64,
    uncompressed_size: u64,
}

#[derive(Debug)]
struct StreamHeader {
    pub check_type: CheckType,
}

#[derive(Debug)]
struct StreamFooter {
    pub backward_size: u32,
    pub stream_flags: [u8; 2],
}

#[derive(Debug)]
struct BlockHeader {
    header_size: usize,
    compressed_size: Option<u64>,
    uncompressed_size: Option<u64>,
    filters: [Option<FilterType>; 4],
    properties: [u32; 4],
}

/// Supported checksum types in XZ format.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckType {
    /// No checksum
    None = 0x00,
    /// CRC32
    Crc32 = 0x01,
    /// CRC64
    #[default]
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
            _ => Err(error_unsupported("unsupported XZ check type")),
        }
    }

    #[cfg(any(feature = "encoder", feature = "xz"))]
    fn checksum_size(self) -> u64 {
        match self {
            CheckType::None => 0,
            CheckType::Crc32 => 4,
            CheckType::Crc64 => 8,
            CheckType::Sha256 => 32,
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

impl BlockHeader {
    fn parse<R: Read>(reader: &mut R) -> crate::Result<Option<Self>> {
        let header_size_encoded = reader.read_u8()?;

        if header_size_encoded == 0 {
            // If header size is 0, this indicates end of blocks (index follows).
            return Ok(None);
        }

        let header_size = (header_size_encoded as usize + 1) * 4;
        if !(8..=1024).contains(&header_size) {
            return Err(error_invalid_data("invalid XZ block header size"));
        }

        // -1 because we already read the size byte.
        let mut header_data = vec![0u8; header_size - 1];
        reader.read_exact(&mut header_data)?;

        // The header carries its own checksum. It is verified first. A corrupt
        // header is reported as corrupt and not as unsupported.
        let crc_offset = header_data.len() - 4;
        let expected_crc = u32::from_le_bytes([
            header_data[crc_offset],
            header_data[crc_offset + 1],
            header_data[crc_offset + 2],
            header_data[crc_offset + 3],
        ]);
        let mut crc = Crc32::new();
        crc.update(&[header_size_encoded]);
        crc.update(&header_data[..crc_offset]);
        if expected_crc != crc.finalize() {
            return Err(error_invalid_data("XZ block header CRC32 mismatch"));
        }

        let block_flags = header_data[0];
        // Bits 2-5 are reserved. A set bit means the header has an unknown
        // field. It should not be parsed.
        if block_flags & 0x3C != 0 {
            return Err(error_unsupported("reserved block flag bits are set"));
        }
        let num_filters = ((block_flags & 0x03) + 1) as usize;
        let has_compressed_size = (block_flags & 0x40) != 0;
        let has_uncompressed_size = (block_flags & 0x80) != 0;

        let mut offset = 1;
        let mut compressed_size = None;
        let mut uncompressed_size = None;

        // Parse optional compressed size.
        if has_compressed_size {
            if offset + 8 > header_data.len() {
                return Err(error_invalid_data(
                    "XZ block header too short for compressed size",
                ));
            }
            compressed_size = Some(parse_multibyte_integer(&header_data[offset..])?);
            offset += count_multibyte_integer_size(&header_data[offset..]);
        }

        if has_uncompressed_size {
            if offset >= header_data.len() {
                return Err(error_invalid_data(
                    "XZ block header too short for uncompressed size",
                ));
            }
            uncompressed_size = Some(parse_multibyte_integer(&header_data[offset..])?);
            offset += count_multibyte_integer_size(&header_data[offset..]);
        }

        let mut filters = [None; 4];
        let mut properties = [0; 4];

        for i in 0..num_filters {
            if offset >= header_data.len() {
                return Err(error_invalid_data("XZ block header too short for filters"));
            }

            let filter_type =
                FilterType::try_from(parse_multibyte_integer(&header_data[offset..])?)
                    .map_err(|_| error_invalid_input("unsupported filter type found"))?;

            offset += count_multibyte_integer_size(&header_data[offset..]);

            let property = match filter_type {
                FilterType::Delta => {
                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for Delta properties",
                        ));
                    }

                    let props_size = parse_multibyte_integer(&header_data[offset..])?;
                    offset += count_multibyte_integer_size(&header_data[offset..]);

                    if props_size != 1 {
                        return Err(error_invalid_data("invalid Delta properties size"));
                    }

                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for Delta properties",
                        ));
                    }

                    let distance_prop = header_data[offset];
                    offset += 1;

                    // Distance is encoded as byte value + 1, range [1, 256].
                    (distance_prop as u32) + 1
                }
                FilterType::BcjX86
                | FilterType::BcjPpc
                | FilterType::BcjIa64
                | FilterType::BcjArm
                | FilterType::BcjArmThumb
                | FilterType::BcjSparc
                | FilterType::BcjArm64
                | FilterType::BcjRiscv => {
                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for BCJ properties",
                        ));
                    }

                    let props_size = parse_multibyte_integer(&header_data[offset..])?;
                    offset += count_multibyte_integer_size(&header_data[offset..]);

                    match props_size {
                        0 => {
                            // No start offset specified, use default (0).
                            0
                        }
                        4 => {
                            // 4-byte start offset specified.
                            if offset + 4 > header_data.len() {
                                return Err(error_invalid_data(
                                    "XZ block header too short for BCJ start offset",
                                ));
                            }

                            let start_offset_value = u32::from_le_bytes([
                                header_data[offset],
                                header_data[offset + 1],
                                header_data[offset + 2],
                                header_data[offset + 3],
                            ]);
                            offset += 4;

                            // Validate alignment based on filter type.
                            let bcj_alignment = match filter_type {
                                FilterType::BcjX86 => 1,
                                FilterType::BcjPpc => 4,
                                FilterType::BcjIa64 => 16,
                                FilterType::BcjArm => 4,
                                FilterType::BcjArmThumb => 2,
                                FilterType::BcjSparc => 4,
                                FilterType::BcjArm64 => 4,
                                FilterType::BcjRiscv => 2,
                                _ => {
                                    return Err(error_invalid_data("invalid BCJ filter type"));
                                }
                            };

                            if start_offset_value % bcj_alignment != 0 {
                                return Err(error_invalid_data(
                                    "BCJ start offset not aligned to filter requirements",
                                ));
                            }

                            start_offset_value
                        }
                        _ => {
                            return Err(error_invalid_data("invalid BCJ properties size"));
                        }
                    }
                }
                FilterType::Lzma2 => {
                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for LZMA2 properties",
                        ));
                    }

                    let props_size = parse_multibyte_integer(&header_data[offset..])?;
                    offset += count_multibyte_integer_size(&header_data[offset..]);

                    if props_size != 1 {
                        return Err(error_invalid_data("invalid LZMA2 properties size"));
                    }

                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for LZMA2 properties",
                        ));
                    }

                    let dict_size_prop = header_data[offset];
                    offset += 1;

                    if dict_size_prop > 40 {
                        return Err(error_invalid_data("invalid LZMA2 dictionary size"));
                    }

                    if dict_size_prop == 40 {
                        0xFFFFFFFF
                    } else {
                        let base = 2 | ((dict_size_prop & 1) as u32);
                        base << (dict_size_prop / 2 + 11)
                    }
                }
            };

            filters[i] = Some(filter_type);
            properties[i] = property;
        }

        if filters.iter().filter_map(|x| *x).next_back() != Some(FilterType::Lzma2) {
            return Err(error_invalid_input(
                "XZ block's last filter must be a LZMA2 filter",
            ));
        }
        if filters[..num_filters - 1].contains(&Some(FilterType::Lzma2)) {
            return Err(error_invalid_data("LZMA2 must be the last XZ block filter"));
        }

        // Header must be padded so that the total header size matches the declared size.
        // We need to pad until: 1 (size byte) + offset + 4 (CRC32) == header_size
        let expected_offset = header_size - 1 - 4; // header_size - size_byte - crc32_size
        while offset < expected_offset {
            if offset >= header_data.len() || header_data[offset] != 0 {
                return Err(error_invalid_data("invalid XZ block header padding"));
            }
            offset += 1;
        }

        // Last 4 bytes should be CRC32 of the header (excluding the CRC32 itself).
        if offset + 4 != header_data.len() {
            return Err(error_invalid_data("invalid XZ block header CRC32 position"));
        }

        Ok(Some(BlockHeader {
            header_size,
            compressed_size,
            uncompressed_size,
            filters,
            properties,
        }))
    }

    pub fn parse_from_slice(
        block_data: &[u8],
    ) -> crate::Result<([Option<FilterType>; 4], [u32; 4], usize)> {
        if block_data.is_empty() {
            return Err(error_invalid_data("Empty block data"));
        }

        let header_size_encoded = block_data[0];
        if header_size_encoded == 0 {
            return Err(error_invalid_data("Invalid block header size"));
        }

        let header_size = (header_size_encoded as usize + 1) * 4;
        if header_size > block_data.len() {
            return Err(error_invalid_data("Block data too short for header"));
        }

        let header_data = &block_data[1..header_size];
        let block_flags = header_data[0];
        // Bits 2-5 are reserved. A set bit means the header has an unknown
        // field. It should not be parsed.
        if block_flags & 0x3C != 0 {
            return Err(error_unsupported("reserved block flag bits are set"));
        }
        let num_filters = ((block_flags & 0x03) + 1) as usize;
        let has_compressed_size = (block_flags & 0x40) != 0;
        let has_uncompressed_size = (block_flags & 0x80) != 0;

        let mut offset = 1;

        // Skip optional compressed size.
        if has_compressed_size {
            if offset >= header_data.len() {
                return Err(error_invalid_data(
                    "Block header too short for compressed size",
                ));
            }
            offset += count_multibyte_integer_size(&header_data[offset..]);
        }

        // Skip optional uncompressed size.
        if has_uncompressed_size {
            if offset >= header_data.len() {
                return Err(error_invalid_data(
                    "Block header too short for uncompressed size",
                ));
            }
            offset += count_multibyte_integer_size(&header_data[offset..]);
        }

        let mut filters = [None; 4];
        let mut properties = [0; 4];

        // Parse filters.
        for i in 0..num_filters {
            if offset >= header_data.len() {
                return Err(error_invalid_data("Block header too short for filters"));
            }

            let filter_id = parse_multibyte_integer(&header_data[offset..])?;
            let filter_type = FilterType::try_from(filter_id)
                .map_err(|_| error_invalid_data("Unsupported filter type"))?;

            offset += count_multibyte_integer_size(&header_data[offset..]);

            let property = match filter_type {
                FilterType::Delta => {
                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "Block header too short for Delta properties",
                        ));
                    }

                    let props_size = parse_multibyte_integer(&header_data[offset..])?;
                    offset += count_multibyte_integer_size(&header_data[offset..]);

                    if props_size != 1 {
                        return Err(error_invalid_data("Invalid Delta properties size"));
                    }

                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "Block header too short for Delta properties",
                        ));
                    }

                    let distance_prop = header_data[offset];
                    offset += 1;
                    (distance_prop as u32) + 1
                }
                FilterType::BcjX86
                | FilterType::BcjPpc
                | FilterType::BcjIa64
                | FilterType::BcjArm
                | FilterType::BcjArmThumb
                | FilterType::BcjSparc
                | FilterType::BcjArm64
                | FilterType::BcjRiscv => {
                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "Block header too short for BCJ properties",
                        ));
                    }

                    let props_size = parse_multibyte_integer(&header_data[offset..])?;
                    offset += count_multibyte_integer_size(&header_data[offset..]);

                    match props_size {
                        0 => 0,
                        4 => {
                            if offset + 4 > header_data.len() {
                                return Err(error_invalid_data(
                                    "Block header too short for BCJ start offset",
                                ));
                            }

                            let start_offset = u32::from_le_bytes([
                                header_data[offset],
                                header_data[offset + 1],
                                header_data[offset + 2],
                                header_data[offset + 3],
                            ]);
                            offset += 4;
                            start_offset
                        }
                        _ => return Err(error_invalid_data("Invalid BCJ properties size")),
                    }
                }
                FilterType::Lzma2 => {
                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "Block header too short for LZMA2 properties",
                        ));
                    }

                    let props_size = parse_multibyte_integer(&header_data[offset..])?;
                    offset += count_multibyte_integer_size(&header_data[offset..]);

                    if props_size != 1 {
                        return Err(error_invalid_data("Invalid LZMA2 properties size"));
                    }

                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "Block header too short for LZMA2 properties",
                        ));
                    }

                    let dict_size_prop = header_data[offset];
                    offset += 1;

                    if dict_size_prop > 40 {
                        return Err(error_invalid_data("Invalid LZMA2 dictionary size"));
                    }

                    if dict_size_prop == 40 {
                        0xFFFFFFFF
                    } else {
                        let base = 2 | ((dict_size_prop & 1) as u32);
                        base << (dict_size_prop / 2 + 11)
                    }
                }
            };

            filters[i] = Some(filter_type);
            properties[i] = property;
        }

        if filters.iter().filter_map(|x| *x).next_back() != Some(FilterType::Lzma2) {
            return Err(error_invalid_data(
                "XZ block's last filter must be a LZMA2 filter",
            ));
        }
        if filters[..num_filters - 1].contains(&Some(FilterType::Lzma2)) {
            return Err(error_invalid_data("LZMA2 must be the last XZ block filter"));
        }

        Ok((filters, properties, header_size))
    }
}

/// Handles checksum calculation for different XZ check types.
enum ChecksumCalculator {
    None,
    Crc32(Crc32),
    Crc64(Crc64),
    Sha256(sha2::Sha256),
}

impl ChecksumCalculator {
    fn new(check_type: CheckType) -> Self {
        match check_type {
            CheckType::None => Self::None,
            CheckType::Crc32 => Self::Crc32(Crc32::new()),
            CheckType::Crc64 => Self::Crc64(Crc64::new()),
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

impl StreamHeader {
    fn parse<R: Read>(reader: &mut R) -> crate::Result<Self> {
        let mut magic = [0u8; 6];
        reader.read_exact(&mut magic)?;
        if magic != XZ_MAGIC {
            return Err(error_invalid_data("invalid XZ magic bytes"));
        }

        Self::parse_stream_header_flags_and_crc(reader)
    }

    pub(crate) fn parse_stream_header_flags_and_crc<R: Read>(
        reader: &mut R,
    ) -> crate::Result<Self> {
        let mut flags = [0u8; 2];
        reader.read_exact(&mut flags)?;

        let expected_crc = reader.read_u32()?;

        if expected_crc != Crc32::checksum(&flags) {
            return Err(error_invalid_data("XZ stream header CRC32 mismatch"));
        }

        // The first byte and the upper nibble of the second byte are reserved.
        // The checksum check comes first. A corrupt header is reported as
        // corrupt and not as unsupported.
        if flags[0] != 0 || flags[1] & 0xF0 != 0 {
            return Err(error_unsupported("invalid XZ stream flags"));
        }

        let check_type = CheckType::from_byte(flags[1])?;

        Ok(StreamHeader { check_type })
    }
}

impl StreamFooter {
    pub(crate) fn parse<R: Read>(reader: &mut R) -> crate::Result<Self> {
        let expected_crc = reader.read_u32()?;

        let backward_size = reader.read_u32()?;

        let mut stream_flags = [0u8; 2];
        reader.read_exact(&mut stream_flags)?;

        // Verify CRC32 of backward size + stream flags.
        let mut crc = Crc32::new();
        crc.update(&backward_size.to_le_bytes());
        crc.update(&stream_flags);

        if expected_crc != crc.finalize() {
            return Err(error_invalid_data("stream footer CRC32 mismatch"));
        }

        let mut footer_magic = [0u8; 2];
        reader.read_exact(&mut footer_magic)?;
        if footer_magic != XZ_FOOTER_MAGIC {
            return Err(error_invalid_data("invalid XZ footer magic bytes"));
        }

        Ok(StreamFooter {
            backward_size,
            stream_flags,
        })
    }
}

/// Parses an index whose indicator byte is already consumed and passes each
/// record to `on_record`. The records are not collected. Returns the size of
/// the index in bytes, the indicator included.
fn parse_index<R: Read>(
    reader: &mut R,
    mut on_record: impl FnMut(IndexRecord) -> crate::Result<()>,
) -> crate::Result<u64> {
    let mut crc = Crc32::new();
    crc.update(&[0]);
    let mut size = 1;

    let number_of_records = read_index_integer(reader, &mut crc, &mut size)?;
    for _ in 0..number_of_records {
        let unpadded_size = read_index_integer(reader, &mut crc, &mut size)?;
        let uncompressed_size = read_index_integer(reader, &mut crc, &mut size)?;

        if unpadded_size == 0 {
            return Err(error_invalid_data("invalid index record unpadded size"));
        }

        on_record(IndexRecord {
            unpadded_size,
            uncompressed_size,
        })?;
    }

    let padding_needed = ((4 - size % 4) % 4) as usize;
    let mut padding = [0u8; 3];
    reader.read_exact(&mut padding[..padding_needed])?;
    if padding[..padding_needed].iter().any(|&byte| byte != 0) {
        return Err(error_invalid_data("invalid index padding"));
    }
    crc.update(&padding[..padding_needed]);

    if reader.read_u32()? != crc.finalize() {
        return Err(error_invalid_data("index CRC32 mismatch"));
    }

    Ok(size + padding_needed as u64 + 4)
}

/// Reads a multibyte integer of the index and adds its bytes to `crc` and `size`.
fn read_index_integer<R: Read>(
    reader: &mut R,
    crc: &mut Crc32,
    size: &mut u64,
) -> crate::Result<u64> {
    let mut bytes = [0u8; 9];
    for len in 1..=bytes.len() {
        bytes[len - 1] = reader.read_u8()?;
        if bytes[len - 1] & 0x80 == 0 {
            crc.update(&bytes[..len]);
            *size += len as u64;
            return parse_multibyte_integer(&bytes[..len]);
        }
    }

    Err(error_invalid_data("XZ multibyte integer too long"))
}

#[cfg(feature = "encoder")]
fn write_xz_stream_header<W: Write>(writer: &mut W, check_type: CheckType) -> crate::Result<()> {
    writer.write_all(&XZ_MAGIC)?;

    let stream_flags = [0u8, check_type as u8];
    writer.write_all(&stream_flags)?;

    let crc = Crc32::checksum(&stream_flags);
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

fn update_crc_with_padding(crc: &mut Crc32, padding_needed: usize) {
    match padding_needed {
        1 => crc.update(&[0]),
        2 => crc.update(&[0, 0]),
        3 => crc.update(&[0, 0, 0]),
        _ => {}
    }
}

/// Returns the uncompressed size that the stream indexes declare for the XZ
/// data from the current position of `reader` to its end.
///
/// Concatenated streams and stream padding are supported. Only the stream
/// headers, indexes and footers are read and checked.
///
/// The position of `reader` is unspecified afterwards.
#[cfg(feature = "std")]
pub fn uncompressed_size<R: Read + Seek>(mut reader: R) -> io::Result<u64> {
    const STREAM_HEADER_SIZE: u64 = 12;
    const STREAM_FOOTER_SIZE: u64 = 12;
    // A stream header, an empty index and a stream footer.
    const MIN_STREAM_SIZE: u64 = STREAM_HEADER_SIZE + 8 + STREAM_FOOTER_SIZE;

    let data_start = reader.stream_position()?;
    let mut stream_end = reader.seek(SeekFrom::End(0))?;
    let mut total = 0u64;

    loop {
        stream_end = skip_stream_padding(&mut reader, data_start, stream_end)?;
        if stream_end.saturating_sub(data_start) < MIN_STREAM_SIZE {
            return Err(error_invalid_data(
                "file too small to contain a valid XZ stream",
            ));
        }

        reader.seek(SeekFrom::Start(stream_end - STREAM_FOOTER_SIZE))?;
        let stream_footer = StreamFooter::parse(&mut reader)?;

        let index_size = (u64::from(stream_footer.backward_size) + 1) * 4;
        let index_start = (stream_end - STREAM_FOOTER_SIZE)
            .checked_sub(index_size)
            .ok_or_else(|| error_invalid_data("XZ index size larger than file"))?;

        let blocks_space = index_start
            .checked_sub(data_start)
            .and_then(|space| space.checked_sub(STREAM_HEADER_SIZE))
            .ok_or_else(|| error_invalid_data("XZ index size larger than file"))?;

        reader.seek(SeekFrom::Start(index_start))?;
        let mut index_reader = BufReader::new(reader.by_ref().take(index_size));
        if index_reader.read_u8()? != 0 {
            return Err(error_invalid_data("invalid XZ index indicator"));
        }

        let mut blocks_size = 0u64;
        let mut stream_size = 0u64;
        let parsed_size = parse_index(&mut index_reader, |record| {
            blocks_size = record
                .unpadded_size
                .checked_next_multiple_of(4)
                .and_then(|padded| blocks_size.checked_add(padded))
                .filter(|&size| size <= blocks_space)
                .ok_or_else(|| error_invalid_data("XZ blocks larger than file"))?;
            stream_size = stream_size
                .checked_add(record.uncompressed_size)
                .ok_or_else(|| error_invalid_data("XZ uncompressed size overflow"))?;
            Ok(())
        })?;
        if parsed_size != index_size {
            return Err(error_invalid_data(
                "XZ index size does not match the backward size",
            ));
        }

        let stream_start = index_start - blocks_size - STREAM_HEADER_SIZE;

        reader.seek(SeekFrom::Start(stream_start))?;
        let stream_header = StreamHeader::parse(&mut reader)?;
        if stream_footer.stream_flags != [0, stream_header.check_type as u8] {
            return Err(error_invalid_data(
                "stream header and footer flags mismatch",
            ));
        }

        total = total
            .checked_add(stream_size)
            .ok_or_else(|| error_invalid_data("XZ uncompressed size overflow"))?;

        if stream_start == data_start {
            return Ok(total);
        }
        stream_end = stream_start;
    }
}

/// Moves `end` back over the stream padding: null bytes in multiples of four.
#[cfg(feature = "std")]
fn skip_stream_padding<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    mut end: u64,
) -> io::Result<u64> {
    const CHUNK_SIZE: u64 = 4096;

    let mut buf = [0u8; CHUNK_SIZE as usize];
    loop {
        let len = end.saturating_sub(start).min(CHUNK_SIZE) / 4 * 4;
        if len == 0 {
            return Ok(end);
        }
        let chunk = &mut buf[..len as usize];
        reader.seek(SeekFrom::Start(end - len))?;
        reader.read_exact(chunk)?;
        let zero_words = chunk
            .rchunks_exact(4)
            .take_while(|word| word.iter().all(|&byte| byte == 0))
            .count() as u64;
        end -= zero_words * 4;
        if zero_words * 4 < len {
            return Ok(end);
        }
    }
}

#[cfg(feature = "std")]
fn create_filter_chain<'reader>(
    mut chain_reader: Box<dyn Read + 'reader>,
    filters: &[Option<FilterType>],
    properties: &[u32],
) -> Box<dyn Read + 'reader> {
    for (filter, property) in filters
        .iter()
        .copied()
        .zip(properties)
        .filter_map(|(filter, property)| filter.map(|filter| (filter, *property)))
        .rev()
    {
        chain_reader = match filter {
            FilterType::Delta => {
                let distance = property as usize;
                Box::new(DeltaReader::new(chain_reader, distance))
            }
            FilterType::BcjX86 => {
                let start_offset = property as usize;
                Box::new(BcjReader::new_x86(chain_reader, start_offset))
            }
            FilterType::BcjPpc => {
                let start_offset = property as usize;
                Box::new(BcjReader::new_ppc(chain_reader, start_offset))
            }
            FilterType::BcjIa64 => {
                let start_offset = property as usize;
                Box::new(BcjReader::new_ia64(chain_reader, start_offset))
            }
            FilterType::BcjArm => {
                let start_offset = property as usize;
                Box::new(BcjReader::new_arm(chain_reader, start_offset))
            }
            FilterType::BcjArmThumb => {
                let start_offset = property as usize;
                Box::new(BcjReader::new_arm_thumb(chain_reader, start_offset))
            }
            FilterType::BcjSparc => {
                let start_offset = property as usize;
                Box::new(BcjReader::new_sparc(chain_reader, start_offset))
            }
            FilterType::BcjArm64 => {
                let start_offset = property as usize;
                Box::new(BcjReader::new_arm64(chain_reader, start_offset))
            }
            FilterType::BcjRiscv => {
                let start_offset = property as usize;
                Box::new(BcjReader::new_riscv(chain_reader, start_offset))
            }
            FilterType::Lzma2 => {
                let dict_size = property;
                Box::new(Lzma2Reader::new(chain_reader, dict_size, None))
            }
        };
    }

    chain_reader
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
            FilterType::BcjPpc => 0x05,
            FilterType::BcjIa64 => 0x06,
            FilterType::BcjArm => 0x07,
            FilterType::BcjArmThumb => 0x08,
            FilterType::BcjSparc => 0x09,
            FilterType::BcjArm64 => 0x0A,
            FilterType::BcjRiscv => 0x0B,
            FilterType::Lzma2 => 0x21,
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
            | FilterType::BcjPpc
            | FilterType::BcjIa64
            | FilterType::BcjArm
            | FilterType::BcjArmThumb
            | FilterType::BcjSparc
            | FilterType::BcjArm64
            | FilterType::BcjRiscv => {
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
            FilterType::Lzma2 => {
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
fn write_xz_block_header<W: Write>(
    writer: &mut W,
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
    let mut crc = Crc32::new();
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
fn write_xz_index<W: Write>(writer: &mut W, index_records: &[IndexRecord]) -> crate::Result<()> {
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

    let mut crc = Crc32::new();
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
fn write_xz_stream_footer<W: Write>(
    writer: &mut W,
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
    let mut crc = Crc32::new();
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
