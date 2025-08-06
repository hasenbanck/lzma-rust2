//! XZ format decoder implementation.

mod reader;

use crc::Table;
pub use reader::XZReader;

use crate::{error_invalid_data, error_invalid_input, ByteReader, Read, Result};

const CRC32: crc::Crc<u32, Table<16>> = crc::Crc::<u32, Table<16>>::new(&crc::CRC_32_ISO_HDLC);
const CRC64: crc::Crc<u64, Table<16>> = crc::Crc::<u64, Table<16>>::new(&crc::CRC_64_XZ);

/// XZ stream magic bytes: 0xFD, '7', 'z', 'X', 'Z', 0x00
const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// XZ stream footer magic bytes
const XZ_FOOTER_MAGIC: [u8; 2] = [b'Y', b'Z'];

/// Supported checksum types in XZ format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckType {
    None = 0x00,
    Crc32 = 0x01,
    Crc64 = 0x04,
    Sha256 = 0x0A,
}

impl CheckType {
    fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            0x00 => Ok(CheckType::None),
            0x01 => Ok(CheckType::Crc32),
            0x04 => Ok(CheckType::Crc64),
            0x0A => Ok(CheckType::Sha256),
            _ => Err(error_invalid_data("Unsupported XZ check type")),
        }
    }
}

/// XZ stream header (12 bytes total)
#[derive(Debug)]
pub struct StreamHeader {
    pub check_type: CheckType,
}

impl StreamHeader {
    /// Parse stream header from reader
    pub fn parse<R: Read>(reader: &mut R) -> Result<Self> {
        // Read magic bytes (6 bytes)
        let mut magic = [0u8; 6];
        reader.read_exact(&mut magic)?;
        if magic != XZ_MAGIC {
            return Err(error_invalid_data("Invalid XZ magic bytes"));
        }

        // Read stream flags (2 bytes)
        let mut flags = [0u8; 2];
        reader.read_exact(&mut flags)?;

        // First byte of flags must be 0
        if flags[0] != 0 {
            return Err(error_invalid_data("Invalid XZ stream flags"));
        }

        let check_type = CheckType::from_byte(flags[1])?;

        // Read and verify CRC32 of the flags (4 bytes)
        let expected_crc = reader.read_u32()?;
        let actual_crc = CRC32.checksum(&flags);
        if expected_crc != actual_crc {
            return Err(error_invalid_data("XZ stream header CRC32 mismatch"));
        }

        Ok(StreamHeader { check_type })
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

    fn try_from(value: u64) -> std::result::Result<Self, Self::Error> {
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

/// XZ block header information
#[derive(Debug)]
pub struct BlockHeader {
    pub compressed_size: Option<u64>,
    pub uncompressed_size: Option<u64>,
    pub filters: [Option<FilterType>; 4],
    pub properties: [u32; 4],
}

impl BlockHeader {
    /// Parse block header from reader
    pub fn parse<R: Read>(reader: &mut R) -> Result<Option<Self>> {
        let header_size_encoded = reader.read_u8()?;

        if header_size_encoded == 0 {
            // If header size is 0, this indicates end of blocks (index follows)
            return Ok(None);
        }

        let header_size = (header_size_encoded as usize + 1) * 4;
        if !(8..=1024).contains(&header_size) {
            return Err(error_invalid_data("Invalid XZ block header size"));
        }

        // -1 because we already read the size byte
        let mut header_data = vec![0u8; header_size - 1];
        reader.read_exact(&mut header_data)?;

        let block_flags = header_data[0];
        let num_filters = ((block_flags & 0x03) + 1) as usize;
        let has_compressed_size = (block_flags & 0x40) != 0;
        let has_uncompressed_size = (block_flags & 0x80) != 0;

        let mut offset = 1;
        let mut compressed_size = None;
        let mut uncompressed_size = None;

        // Parse optional compressed size
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

                    // Distance is encoded as byte value + 1, range [1, 256]
                    (distance_prop as u32) + 1
                }
                FilterType::BcjX86
                | FilterType::BcjPPC
                | FilterType::BcjIA64
                | FilterType::BcjARM
                | FilterType::BcjARMThumb
                | FilterType::BcjSPARC
                | FilterType::BcjARM64
                | FilterType::BcjRISCV => {
                    if offset >= header_data.len() {
                        return Err(error_invalid_data(
                            "XZ block header too short for BCJ properties",
                        ));
                    }

                    let props_size = parse_multibyte_integer(&header_data[offset..])?;
                    offset += count_multibyte_integer_size(&header_data[offset..]);

                    match props_size {
                        0 => {
                            // No start offset specified, use default (0)
                            0
                        }
                        4 => {
                            // 4-byte start offset specified
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

                            // Validate alignment based on filter type
                            let bcj_alignment = match filter_type {
                                FilterType::BcjX86 { .. } => 1,
                                FilterType::BcjPPC { .. } => 4,
                                FilterType::BcjIA64 { .. } => 16,
                                FilterType::BcjARM { .. } => 4,
                                FilterType::BcjARMThumb { .. } => 2,
                                FilterType::BcjSPARC { .. } => 4,
                                FilterType::BcjARM64 { .. } => 4,
                                FilterType::BcjRISCV { .. } => 2,
                                _ => unreachable!(),
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
                FilterType::LZMA2 => {
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

        if filters.iter().filter_map(|x| *x).last() != Some(FilterType::LZMA2) {
            return Err(error_invalid_input(
                "XZ block's last filter must be a LZMA2 filter",
            ));
        }

        // Header must be padded so that the total header size matches the declared size
        // We need to pad until: 1 (size byte) + offset + 4 (CRC32) == header_size
        let expected_offset = header_size - 1 - 4; // header_size - size_byte - crc32_size
        while offset < expected_offset {
            if offset >= header_data.len() || header_data[offset] != 0 {
                return Err(error_invalid_data("invalid XZ block header padding"));
            }
            offset += 1;
        }

        // Last 4 bytes should be CRC32 of the header (excluding the CRC32 itself)
        if offset + 4 != header_data.len() {
            return Err(error_invalid_data("invalid XZ block header CRC32 position"));
        }

        let expected_crc = u32::from_le_bytes([
            header_data[offset],
            header_data[offset + 1],
            header_data[offset + 2],
            header_data[offset + 3],
        ]);

        // Calculate CRC32 of header size byte + header data (excluding CRC32)
        let mut crc_data = vec![header_size_encoded];
        crc_data.extend_from_slice(&header_data[..offset]);
        let actual_crc = CRC32.checksum(&crc_data);

        if expected_crc != actual_crc {
            return Err(error_invalid_data("XZ block header CRC32 mismatch"));
        }

        Ok(Some(BlockHeader {
            compressed_size,
            uncompressed_size,
            filters,
            properties,
        }))
    }
}

/// Parse XZ multibyte integer (variable length encoding)
fn parse_multibyte_integer(data: &[u8]) -> Result<u64> {
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

/// Count the number of bytes used by a multibyte integer
fn count_multibyte_integer_size(data: &[u8]) -> usize {
    for (i, &byte) in data.iter().enumerate() {
        if (byte & 0x80) == 0 {
            return i + 1;
        }
    }
    data.len()
}

mod specification {
    /*
    The .xz File Format
    ===================

    Version 1.2.1 (2024-04-08)


            0. Preface
               0.1. Notices and Acknowledgements
               0.2. Getting the Latest Version
               0.3. Version History
            1. Conventions
               1.1. Byte and Its Representation
               1.2. Multibyte Integers
            2. Overall Structure of .xz File
               2.1. Stream
                    2.1.1. Stream Header
                           2.1.1.1. Header Magic Bytes
                           2.1.1.2. Stream Flags
                           2.1.1.3. CRC32
                    2.1.2. Stream Footer
                           2.1.2.1. CRC32
                           2.1.2.2. Backward Size
                           2.1.2.3. Stream Flags
                           2.1.2.4. Footer Magic Bytes
               2.2. Stream Padding
            3. Block
               3.1. Block Header
                    3.1.1. Block Header Size
                    3.1.2. Block Flags
                    3.1.3. Compressed Size
                    3.1.4. Uncompressed Size
                    3.1.5. List of Filter Flags
                    3.1.6. Header Padding
                    3.1.7. CRC32
               3.2. Compressed Data
               3.3. Block Padding
               3.4. Check
            4. Index
               4.1. Index Indicator
               4.2. Number of Records
               4.3. List of Records
                    4.3.1. Unpadded Size
                    4.3.2. Uncompressed Size
               4.4. Index Padding
               4.5. CRC32
            5. Filter Chains
               5.1. Alignment
               5.2. Security
               5.3. Filters
                    5.3.1. LZMA2
                    5.3.2. Branch/Call/Jump Filters for Executables
                    5.3.3. Delta
                           5.3.3.1. Format of the Encoded Output
               5.4. Custom Filter IDs
                    5.4.1. Reserved Custom Filter ID Ranges
            6. Cyclic Redundancy Checks
            7. References


    0. Preface

            This document describes the .xz file format (filename suffix
            ".xz", MIME type "application/x-xz"). It is intended that this
            this format replace the old .lzma format used by LZMA SDK and
            LZMA Utils.


    0.1. Notices and Acknowledgements

            This file format was designed by Lasse Collin
            <lasse.collin@tukaani.org> and Igor Pavlov.

            Special thanks for helping with this document goes to
            Ville Koskinen. Thanks for helping with this document goes to
            Mark Adler, H. Peter Anvin, Mikko Pouru, and Lars Wirzenius.

            This document has been put into the public domain.


    0.2. Getting the Latest Version

            The latest official version of this document can be downloaded
            from <https://tukaani.org/xz/xz-file-format.txt>.

            Specific versions of this document have a filename
            xz-file-format-X.Y.Z.txt where X.Y.Z is the version number.
            For example, the version 1.0.0 of this document is available
            at <https://tukaani.org/xz/xz-file-format-1.0.0.txt>.


    0.3. Version History

            Version   Date          Description

            1.2.1     2024-04-08    The URLs of this specification and
                                    XZ Utils were changed back to the
                                    original ones in Sections 0.2 and 7.

            1.2.0     2024-01-19    Added RISC-V filter and updated URLs in
                                    Sections 0.2 and 7. The URL of this
                                    specification was changed.

            1.1.0     2022-12-11    Added ARM64 filter and clarified 32-bit
                                    ARM endianness in Section 5.3.2,
                                    language improvements in Section 5.4

            1.0.4     2009-08-27    Language improvements in Sections 1.2,
                                    2.1.1.2, 3.1.1, 3.1.2, and 5.3.1

            1.0.3     2009-06-05    Spelling fixes in Sections 5.1 and 5.4

            1.0.2     2009-06-04    Typo fixes in Sections 4 and 5.3.1

            1.0.1     2009-06-01    Typo fix in Section 0.3 and minor
                                    clarifications to Sections 2, 2.2,
                                    3.3, 4.4, and 5.3.2

            1.0.0     2009-01-14    The first official version


    1. Conventions

            The key words "MUST", "MUST NOT", "REQUIRED", "SHOULD",
            "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this
            document are to be interpreted as described in [RFC-2119].

            Indicating a warning means displaying a message, returning
            appropriate exit status, or doing something else to let the
            user know that something worth warning occurred. The operation
            SHOULD still finish if a warning is indicated.

            Indicating an error means displaying a message, returning
            appropriate exit status, or doing something else to let the
            user know that something prevented successfully finishing the
            operation. The operation MUST be aborted once an error has
            been indicated.


    1.1. Byte and Its Representation

            In this document, byte is always 8 bits.

            A "null byte" has all bits unset. That is, the value of a null
            byte is 0x00.

            To represent byte blocks, this document uses notation that
            is similar to the notation used in [RFC-1952]:

                +-------+
                |  Foo  |   One byte.
                +-------+

                +---+---+
                |  Foo  |   Two bytes; that is, some of the vertical bars
                +---+---+   can be missing.

                +=======+
                |  Foo  |   Zero or more bytes.
                +=======+

            In this document, a boxed byte or a byte sequence declared
            using this notation is called "a field". The example field
            above would be called "the Foo field" or plain "Foo".

            If there are many fields, they may be split to multiple lines.
            This is indicated with an arrow ("--->"):

                +=====+
                | Foo |
                +=====+

                     +=====+
                ---> | Bar |
                     +=====+

            The above is equivalent to this:

                +=====+=====+
                | Foo | Bar |
                +=====+=====+


    1.2. Multibyte Integers

            Multibyte integers of static length, such as CRC values,
            are stored in little endian byte order (least significant
            byte first).

            When smaller values are more likely than bigger values (for
            example file sizes), multibyte integers are encoded in a
            variable-length representation:
              - Numbers in the range [0, 127] are copied as is, and take
                one byte of space.
              - Bigger numbers will occupy two or more bytes. All but the
                last byte of the multibyte representation have the highest
                (eighth) bit set.

            For now, the value of the variable-length integers is limited
            to 63 bits, which limits the encoded size of the integer to
            nine bytes. These limits may be increased in the future if
            needed.

            The following C code illustrates encoding and decoding of
            variable-length integers. The functions return the number of
            bytes occupied by the integer (1-9), or zero on error.

                #include <stddef.h>
                #include <inttypes.h>

                size_t
                encode(uint8_t buf[static 9], uint64_t num)
                {
                    if (num > UINT64_MAX / 2)
                        return 0;

                    size_t i = 0;

                    while (num >= 0x80) {
                        buf[i++] = (uint8_t)(num) | 0x80;
                        num >>= 7;
                    }

                    buf[i++] = (uint8_t)(num);

                    return i;
                }

                size_t
                decode(const uint8_t buf[], size_t size_max, uint64_t *num)
                {
                    if (size_max == 0)
                        return 0;

                    if (size_max > 9)
                        size_max = 9;

                    *num = buf[0] & 0x7F;
                    size_t i = 0;

                    while (buf[i++] & 0x80) {
                        if (i >= size_max || buf[i] == 0x00)
                            return 0;

                        *num |= (uint64_t)(buf[i] & 0x7F) << (i * 7);
                    }

                    return i;
                }


    2. Overall Structure of .xz File

            A standalone .xz files consist of one or more Streams which may
            have Stream Padding between or after them:

                +========+================+========+================+
                | Stream | Stream Padding | Stream | Stream Padding | ...
                +========+================+========+================+

            The sizes of Stream and Stream Padding are always multiples
            of four bytes, thus the size of every valid .xz file MUST be
            a multiple of four bytes.

            While a typical file contains only one Stream and no Stream
            Padding, a decoder handling standalone .xz files SHOULD support
            files that have more than one Stream or Stream Padding.

            In contrast to standalone .xz files, when the .xz file format
            is used as an internal part of some other file format or
            communication protocol, it usually is expected that the decoder
            stops after the first Stream, and doesn't look for Stream
            Padding or possibly other Streams.


    2.1. Stream

            +-+-+-+-+-+-+-+-+-+-+-+-+=======+=======+     +=======+
            |     Stream Header     | Block | Block | ... | Block |
            +-+-+-+-+-+-+-+-+-+-+-+-+=======+=======+     +=======+

                 +=======+-+-+-+-+-+-+-+-+-+-+-+-+
            ---> | Index |     Stream Footer     |
                 +=======+-+-+-+-+-+-+-+-+-+-+-+-+

            All the above fields have a size that is a multiple of four. If
            Stream is used as an internal part of another file format, it
            is RECOMMENDED to make the Stream start at an offset that is
            a multiple of four bytes.

            Stream Header, Index, and Stream Footer are always present in
            a Stream. The maximum size of the Index field is 16 GiB (2^34).

            There are zero or more Blocks. The maximum number of Blocks is
            limited only by the maximum size of the Index field.

            Total size of a Stream MUST be less than 8 EiB (2^63 bytes).
            The same limit applies to the total amount of uncompressed
            data stored in a Stream.

            If an implementation supports handling .xz files with multiple
            concatenated Streams, it MAY apply the above limits to the file
            as a whole instead of limiting per Stream basis.


    2.1.1. Stream Header

            +---+---+---+---+---+---+-------+------+--+--+--+--+
            |  Header Magic Bytes   | Stream Flags |   CRC32   |
            +---+---+---+---+---+---+-------+------+--+--+--+--+


    2.1.1.1. Header Magic Bytes

            The first six (6) bytes of the Stream are so called Header
            Magic Bytes. They can be used to identify the file type.

                Using a C array and ASCII:
                const uint8_t HEADER_MAGIC[6]
                        = { 0xFD, '7', 'z', 'X', 'Z', 0x00 };

                In plain hexadecimal:
                FD 37 7A 58 5A 00

            Notes:
              - The first byte (0xFD) was chosen so that the files cannot
                be erroneously detected as being in .lzma format, in which
                the first byte is in the range [0x00, 0xE0].
              - The sixth byte (0x00) was chosen to prevent applications
                from misdetecting the file as a text file.

            If the Header Magic Bytes don't match, the decoder MUST
            indicate an error.


    2.1.1.2. Stream Flags

            The first byte of Stream Flags is always a null byte. In the
            future, this byte may be used to indicate a new Stream version
            or other Stream properties.

            The second byte of Stream Flags is a bit field:

                Bit(s)  Mask  Description
                 0-3    0x0F  Type of Check (see Section 3.4):
                                  ID    Size      Check name
                                  0x00   0 bytes  None
                                  0x01   4 bytes  CRC32
                                  0x02   4 bytes  (Reserved)
                                  0x03   4 bytes  (Reserved)
                                  0x04   8 bytes  CRC64
                                  0x05   8 bytes  (Reserved)
                                  0x06   8 bytes  (Reserved)
                                  0x07  16 bytes  (Reserved)
                                  0x08  16 bytes  (Reserved)
                                  0x09  16 bytes  (Reserved)
                                  0x0A  32 bytes  SHA-256
                                  0x0B  32 bytes  (Reserved)
                                  0x0C  32 bytes  (Reserved)
                                  0x0D  64 bytes  (Reserved)
                                  0x0E  64 bytes  (Reserved)
                                  0x0F  64 bytes  (Reserved)
                 4-7    0xF0  Reserved for future use; MUST be zero for now.

            Implementations SHOULD support at least the Check IDs 0x00
            (None) and 0x01 (CRC32). Supporting other Check IDs is
            OPTIONAL. If an unsupported Check is used, the decoder SHOULD
            indicate a warning or error.

            If any reserved bit is set, the decoder MUST indicate an error.
            It is possible that there is a new field present which the
            decoder is not aware of, and can thus parse the Stream Header
            incorrectly.


    2.1.1.3. CRC32

            The CRC32 is calculated from the Stream Flags field. It is
            stored as an unsigned 32-bit little endian integer. If the
            calculated value does not match the stored one, the decoder
            MUST indicate an error.

            The idea is that Stream Flags would always be two bytes, even
            if new features are needed. This way old decoders will be able
            to verify the CRC32 calculated from Stream Flags, and thus
            distinguish between corrupt files (CRC32 doesn't match) and
            files that the decoder doesn't support (CRC32 matches but
            Stream Flags has reserved bits set).


    2.1.2. Stream Footer

            +-+-+-+-+---+---+---+---+-------+------+----------+---------+
            | CRC32 | Backward Size | Stream Flags | Footer Magic Bytes |
            +-+-+-+-+---+---+---+---+-------+------+----------+---------+


    2.1.2.1. CRC32

            The CRC32 is calculated from the Backward Size and Stream Flags
            fields. It is stored as an unsigned 32-bit little endian
            integer. If the calculated value does not match the stored one,
            the decoder MUST indicate an error.

            The reason to have the CRC32 field before the Backward Size and
            Stream Flags fields is to keep the four-byte fields aligned to
            a multiple of four bytes.


    2.1.2.2. Backward Size

            Backward Size is stored as a 32-bit little endian integer,
            which indicates the size of the Index field as multiple of
            four bytes, minimum value being four bytes:

                real_backward_size = (stored_backward_size + 1) * 4;

            If the stored value does not match the real size of the Index
            field, the decoder MUST indicate an error.

            Using a fixed-size integer to store Backward Size makes
            it slightly simpler to parse the Stream Footer when the
            application needs to parse the Stream backwards.


    2.1.2.3. Stream Flags

            This is a copy of the Stream Flags field from the Stream
            Header. The information stored to Stream Flags is needed
            when parsing the Stream backwards. The decoder MUST compare
            the Stream Flags fields in both Stream Header and Stream
            Footer, and indicate an error if they are not identical.


    2.1.2.4. Footer Magic Bytes

            As the last step of the decoding process, the decoder MUST
            verify the existence of Footer Magic Bytes. If they don't
            match, an error MUST be indicated.

                Using a C array and ASCII:
                const uint8_t FOOTER_MAGIC[2] = { 'Y', 'Z' };

                In hexadecimal:
                59 5A

            The primary reason to have Footer Magic Bytes is to make
            it easier to detect incomplete files quickly, without
            uncompressing. If the file does not end with Footer Magic Bytes
            (excluding Stream Padding described in Section 2.2), it cannot
            be undamaged, unless someone has intentionally appended garbage
            after the end of the Stream.


    2.2. Stream Padding

            Only the decoders that support decoding of concatenated Streams
            MUST support Stream Padding.

            Stream Padding MUST contain only null bytes. To preserve the
            four-byte alignment of consecutive Streams, the size of Stream
            Padding MUST be a multiple of four bytes. Empty Stream Padding
            is allowed. If these requirements are not met, the decoder MUST
            indicate an error.

            Note that non-empty Stream Padding is allowed at the end of the
            file; there doesn't need to be a new Stream after non-empty
            Stream Padding. This can be convenient in certain situations
            [GNU-tar].

            The possibility of Stream Padding MUST be taken into account
            when designing an application that parses Streams backwards,
            and the application supports concatenated Streams.


    3. Block

            +==============+=================+===============+=======+
            | Block Header | Compressed Data | Block Padding | Check |
            +==============+=================+===============+=======+


    3.1. Block Header

            +-------------------+-------------+=================+
            | Block Header Size | Block Flags | Compressed Size |
            +-------------------+-------------+=================+

                 +===================+======================+
            ---> | Uncompressed Size | List of Filter Flags |
                 +===================+======================+

                 +================+--+--+--+--+
            ---> | Header Padding |   CRC32   |
                 +================+--+--+--+--+


    3.1.1. Block Header Size

            This field overlaps with the Index Indicator field (see
            Section 4.1).

            This field contains the size of the Block Header field,
            including the Block Header Size field itself. Valid values are
            in the range [0x01, 0xFF], which indicate the size of the Block
            Header as multiples of four bytes, minimum size being eight
            bytes:

                real_header_size = (encoded_header_size + 1) * 4;

            If a Block Header bigger than 1024 bytes is needed in the
            future, a new field can be added between the Block Header and
            Compressed Data fields. The presence of this new field would
            be indicated in the Block Header field.


    3.1.2. Block Flags

            The Block Flags field is a bit field:

                Bit(s)  Mask  Description
                 0-1    0x03  Number of filters (1-4)
                 2-5    0x3C  Reserved for future use; MUST be zero for now.
                  6     0x40  The Compressed Size field is present.
                  7     0x80  The Uncompressed Size field is present.

            If any reserved bit is set, the decoder MUST indicate an error.
            It is possible that there is a new field present which the
            decoder is not aware of, and can thus parse the Block Header
            incorrectly.


    3.1.3. Compressed Size

            This field is present only if the appropriate bit is set in
            the Block Flags field (see Section 3.1.2).

            The Compressed Size field contains the size of the Compressed
            Data field, which MUST be non-zero. Compressed Size is stored
            using the encoding described in Section 1.2. If the Compressed
            Size doesn't match the size of the Compressed Data field, the
            decoder MUST indicate an error.


    3.1.4. Uncompressed Size

            This field is present only if the appropriate bit is set in
            the Block Flags field (see Section 3.1.2).

            The Uncompressed Size field contains the size of the Block
            after uncompressing. Uncompressed Size is stored using the
            encoding described in Section 1.2. If the Uncompressed Size
            does not match the real uncompressed size, the decoder MUST
            indicate an error.

            Storing the Compressed Size and Uncompressed Size fields serves
            several purposes:
              - The decoder knows how much memory it needs to allocate
                for a temporary buffer in multithreaded mode.
              - Simple error detection: wrong size indicates a broken file.
              - Seeking forwards to a specific location in streamed mode.

            It should be noted that the only reliable way to determine
            the real uncompressed size is to uncompress the Block,
            because the Block Header and Index fields may contain
            (intentionally or unintentionally) invalid information.


    3.1.5. List of Filter Flags

            +================+================+     +================+
            | Filter 0 Flags | Filter 1 Flags | ... | Filter n Flags |
            +================+================+     +================+

            The number of Filter Flags fields is stored in the Block Flags
            field (see Section 3.1.2).

            The format of each Filter Flags field is as follows:

                +===========+====================+===================+
                | Filter ID | Size of Properties | Filter Properties |
                +===========+====================+===================+

            Both Filter ID and Size of Properties are stored using the
            encoding described in Section 1.2. Size of Properties indicates
            the size of the Filter Properties field as bytes. The list of
            officially defined Filter IDs and the formats of their Filter
            Properties are described in Section 5.3.

            Filter IDs greater than or equal to 0x4000_0000_0000_0000
            (2^62) are reserved for implementation-specific internal use.
            These Filter IDs MUST never be used in List of Filter Flags.


    3.1.6. Header Padding

            This field contains as many null byte as it is needed to make
            the Block Header have the size specified in Block Header Size.
            If any of the bytes are not null bytes, the decoder MUST
            indicate an error. It is possible that there is a new field
            present which the decoder is not aware of, and can thus parse
            the Block Header incorrectly.


    3.1.7. CRC32

            The CRC32 is calculated over everything in the Block Header
            field except the CRC32 field itself. It is stored as an
            unsigned 32-bit little endian integer. If the calculated
            value does not match the stored one, the decoder MUST indicate
            an error.

            By verifying the CRC32 of the Block Header before parsing the
            actual contents allows the decoder to distinguish between
            corrupt and unsupported files.


    3.2. Compressed Data

            The format of Compressed Data depends on Block Flags and List
            of Filter Flags. Excluding the descriptions of the simplest
            filters in Section 5.3, the format of the filter-specific
            encoded data is out of scope of this document.


    3.3. Block Padding

            Block Padding MUST contain 0-3 null bytes to make the size of
            the Block a multiple of four bytes. This can be needed when
            the size of Compressed Data is not a multiple of four. If any
            of the bytes in Block Padding are not null bytes, the decoder
            MUST indicate an error.


    3.4. Check

            The type and size of the Check field depends on which bits
            are set in the Stream Flags field (see Section 2.1.1.2).

            The Check, when used, is calculated from the original
            uncompressed data. If the calculated Check does not match the
            stored one, the decoder MUST indicate an error. If the selected
            type of Check is not supported by the decoder, it SHOULD
            indicate a warning or error.


    4. Index

            +-----------------+===================+
            | Index Indicator | Number of Records |
            +-----------------+===================+

                 +=================+===============+-+-+-+-+
            ---> | List of Records | Index Padding | CRC32 |
                 +=================+===============+-+-+-+-+

            Index serves several purposes. Using it, one can
              - verify that all Blocks in a Stream have been processed;
              - find out the uncompressed size of a Stream; and
              - quickly access the beginning of any Block (random access).


    4.1. Index Indicator

            This field overlaps with the Block Header Size field (see
            Section 3.1.1). The value of Index Indicator is always 0x00.


    4.2. Number of Records

            This field indicates how many Records there are in the List
            of Records field, and thus how many Blocks there are in the
            Stream. The value is stored using the encoding described in
            Section 1.2. If the decoder has decoded all the Blocks of the
            Stream, and then notices that the Number of Records doesn't
            match the real number of Blocks, the decoder MUST indicate an
            error.


    4.3. List of Records

            List of Records consists of as many Records as indicated by the
            Number of Records field:

                +========+========+
                | Record | Record | ...
                +========+========+

            Each Record contains information about one Block:

                +===============+===================+
                | Unpadded Size | Uncompressed Size |
                +===============+===================+

            If the decoder has decoded all the Blocks of the Stream, it
            MUST verify that the contents of the Records match the real
            Unpadded Size and Uncompressed Size of the respective Blocks.

            Implementation hint: It is possible to verify the Index with
            constant memory usage by calculating for example SHA-256 of
            both the real size values and the List of Records, then
            comparing the hash values. Implementing this using
            non-cryptographic hash like CRC32 SHOULD be avoided unless
            small code size is important.

            If the decoder supports random-access reading, it MUST verify
            that Unpadded Size and Uncompressed Size of every completely
            decoded Block match the sizes stored in the Index. If only
            partial Block is decoded, the decoder MUST verify that the
            processed sizes don't exceed the sizes stored in the Index.


    4.3.1. Unpadded Size

            This field indicates the size of the Block excluding the Block
            Padding field. That is, Unpadded Size is the size of the Block
            Header, Compressed Data, and Check fields. Unpadded Size is
            stored using the encoding described in Section 1.2. The value
            MUST never be zero; with the current structure of Blocks, the
            actual minimum value for Unpadded Size is five.

            Implementation note: Because the size of the Block Padding
            field is not included in Unpadded Size, calculating the total
            size of a Stream or doing random-access reading requires
            calculating the actual size of the Blocks by rounding Unpadded
            Sizes up to the next multiple of four.

            The reason to exclude Block Padding from Unpadded Size is to
            ease making a raw copy of Compressed Data without Block
            Padding. This can be useful, for example, if someone wants
            to convert Streams to some other file format quickly.


    4.3.2. Uncompressed Size

            This field indicates the Uncompressed Size of the respective
            Block as bytes. The value is stored using the encoding
            described in Section 1.2.


    4.4. Index Padding

            This field MUST contain 0-3 null bytes to pad the Index to
            a multiple of four bytes. If any of the bytes are not null
            bytes, the decoder MUST indicate an error.


    4.5. CRC32

            The CRC32 is calculated over everything in the Index field
            except the CRC32 field itself. The CRC32 is stored as an
            unsigned 32-bit little endian integer. If the calculated
            value does not match the stored one, the decoder MUST indicate
            an error.


    5. Filter Chains

            The Block Flags field defines how many filters are used. When
            more than one filter is used, the filters are chained; that is,
            the output of one filter is the input of another filter. The
            following figure illustrates the direction of data flow.

                        v   Uncompressed Data   ^
                        |       Filter 0        |
                Encoder |       Filter 1        | Decoder
                        |       Filter n        |
                        v    Compressed Data    ^


    5.1. Alignment

            Alignment of uncompressed input data is usually the job of
            the application producing the data. For example, to get the
            best results, an archiver tool should make sure that all
            PowerPC executable files in the archive stream start at
            offsets that are multiples of four bytes.

            Some filters, for example LZMA2, can be configured to take
            advantage of specified alignment of input data. Note that
            taking advantage of aligned input can be beneficial also when
            a filter is not the first filter in the chain. For example,
            if you compress PowerPC executables, you may want to use the
            PowerPC filter and chain that with the LZMA2 filter. Because
            not only the input but also the output alignment of the PowerPC
            filter is four bytes, it is now beneficial to set LZMA2
            settings so that the LZMA2 encoder can take advantage of its
            four-byte-aligned input data.

            The output of the last filter in the chain is stored to the
            Compressed Data field, which is is guaranteed to be aligned
            to a multiple of four bytes relative to the beginning of the
            Stream. This can increase
              - speed, if the filtered data is handled multiple bytes at
                a time by the filter-specific encoder and decoder,
                because accessing aligned data in computer memory is
                usually faster; and
              - compression ratio, if the output data is later compressed
                with an external compression tool.


    5.2. Security

            If filters would be allowed to be chained freely, it would be
            possible to create malicious files, that would be very slow to
            decode. Such files could be used to create denial of service
            attacks.

            Slow files could occur when multiple filters are chained:

                v   Compressed input data
                |   Filter 1 decoder (last filter)
                |   Filter 0 decoder (non-last filter)
                v   Uncompressed output data

            The decoder of the last filter in the chain produces a lot of
            output from little input. Another filter in the chain takes the
            output of the last filter, and produces very little output
            while consuming a lot of input. As a result, a lot of data is
            moved inside the filter chain, but the filter chain as a whole
            gets very little work done.

            To prevent this kind of slow files, there are restrictions on
            how the filters can be chained. These restrictions MUST be
            taken into account when designing new filters.

            The maximum number of filters in the chain has been limited to
            four, thus there can be at maximum of three non-last filters.
            Of these three non-last filters, only two are allowed to change
            the size of the data.

            The non-last filters, that change the size of the data, MUST
            have a limit how much the decoder can compress the data: the
            decoder SHOULD produce at least n bytes of output when the
            filter is given 2n bytes of input. This  limit is not
            absolute, but significant deviations MUST be avoided.

            The above limitations guarantee that if the last filter in the
            chain produces 4n bytes of output, the chain as a whole will
            produce at least n bytes of output.


    5.3. Filters

    5.3.1. LZMA2

            LZMA (Lempel-Ziv-Markov chain-Algorithm) is a general-purpose
            compression algorithm with high compression ratio and fast
            decompression. LZMA is based on LZ77 and range coding
            algorithms.

            LZMA2 is an extension on top of the original LZMA. LZMA2 uses
            LZMA internally, but adds support for flushing the encoder,
            uncompressed chunks, eases stateful decoder implementations,
            and improves support for multithreading. Thus, the plain LZMA
            will not be supported in this file format.

                Filter ID:                  0x21
                Size of Filter Properties:  1 byte
                Changes size of data:       Yes
                Allow as a non-last filter: No
                Allow as the last filter:   Yes

                Preferred alignment:
                    Input data:             Adjustable to 1/2/4/8/16 byte(s)
                    Output data:            1 byte

            The format of the one-byte Filter Properties field is as
            follows:

                Bits   Mask   Description
                0-5    0x3F   Dictionary Size
                6-7    0xC0   Reserved for future use; MUST be zero for now.

            Dictionary Size is encoded with one-bit mantissa and five-bit
            exponent. The smallest dictionary size is 4 KiB and the biggest
            is 4 GiB.

                Raw value   Mantissa   Exponent   Dictionary size
                    0           2         11         4 KiB
                    1           3         11         6 KiB
                    2           2         12         8 KiB
                    3           3         12        12 KiB
                    4           2         13        16 KiB
                    5           3         13        24 KiB
                    6           2         14        32 KiB
                  ...         ...        ...      ...
                   35           3         27       768 MiB
                   36           2         28      1024 MiB
                   37           3         29      1536 MiB
                   38           2         30      2048 MiB
                   39           3         30      3072 MiB
                   40           2         31      4096 MiB - 1 B

            Instead of having a table in the decoder, the dictionary size
            can be decoded using the following C code:

                const uint8_t bits = get_dictionary_flags() & 0x3F;
                if (bits > 40)
                    return DICTIONARY_TOO_BIG; // Bigger than 4 GiB

                uint32_t dictionary_size;
                if (bits == 40) {
                    dictionary_size = UINT32_MAX;
                } else {
                    dictionary_size = 2 | (bits & 1);
                    dictionary_size <<= bits / 2 + 11;
                }


    5.3.2. Branch/Call/Jump Filters for Executables

            These filters convert relative branch, call, and jump
            instructions to their absolute counterparts in executable
            files. This conversion increases redundancy and thus
            compression ratio.

                Size of Filter Properties:  0 or 4 bytes
                Changes size of data:       No
                Allow as a non-last filter: Yes
                Allow as the last filter:   No

            Below is the list of filters in this category. The alignment
            is the same for both input and output data.

                Filter ID   Alignment   Description
                  0x04       1 byte     x86 filter (BCJ)
                  0x05       4 bytes    PowerPC (big endian) filter
                  0x06      16 bytes    IA64 filter
                  0x07       4 bytes    ARM filter [1]
                  0x08       2 bytes    ARM Thumb filter [1]
                  0x09       4 bytes    SPARC filter
                  0x0A       4 bytes    ARM64 filter [2]
                  0x0B       2 bytes    RISC-V filter

                  [1] These are for little endian instruction encoding.
                      This must not be confused with data endianness.
                      A processor configured for big endian data access
                      may still use little endian instruction encoding.
                      The filters don't care about the data endianness.

                  [2] 4096-byte alignment gives the best results
                      because the address in the ADRP instruction
                      is a multiple of 4096 bytes.

            If the size of Filter Properties is four bytes, the Filter
            Properties field contains the start offset used for address
            conversions. It is stored as an unsigned 32-bit little endian
            integer. The start offset MUST be a multiple of the alignment
            of the filter as listed in the table above; if it isn't, the
            decoder MUST indicate an error. If the size of Filter
            Properties is zero, the start offset is zero.

            Setting the start offset may be useful if an executable has
            multiple sections, and there are many cross-section calls.
            Taking advantage of this feature usually requires usage of
            the Subblock filter, whose design is not complete yet.


    5.3.3. Delta

            The Delta filter may increase compression ratio when the value
            of the next byte correlates with the value of an earlier byte
            at specified distance.

                Filter ID:                  0x03
                Size of Filter Properties:  1 byte
                Changes size of data:       No
                Allow as a non-last filter: Yes
                Allow as the last filter:   No

                Preferred alignment:
                    Input data:             1 byte
                    Output data:            Same as the original input data

            The Properties byte indicates the delta distance, which can be
            1-256 bytes backwards from the current byte: 0x00 indicates
            distance of 1 byte and 0xFF distance of 256 bytes.


    5.3.3.1. Format of the Encoded Output

            The code below illustrates both encoding and decoding with
            the Delta filter.

                // Distance is in the range [1, 256].
                const unsigned int distance = get_properties_byte() + 1;
                uint8_t pos = 0;
                uint8_t delta[256];

                memset(delta, 0, sizeof(delta));

                while (1) {
                    const int byte = read_byte();
                    if (byte == EOF)
                        break;

                    uint8_t tmp = delta[(uint8_t)(distance + pos)];
                    if (is_encoder) {
                        tmp = (uint8_t)(byte) - tmp;
                        delta[pos] = (uint8_t)(byte);
                    } else {
                        tmp = (uint8_t)(byte) + tmp;
                        delta[pos] = tmp;
                    }

                    write_byte(tmp);
                    --pos;
                }


    5.4. Custom Filter IDs

            If a developer wants to use custom Filter IDs, there are two
            choices. The first choice is to contact Lasse Collin and ask
            him to allocate a range of IDs for the developer.

            The second choice is to generate a 40-bit random integer
            which the developer can use as a personal Developer ID.
            To minimize the risk of collisions, Developer ID has to be
            a randomly generated integer, not manually selected "hex word".
            The following command, which works on many free operating
            systems, can be used to generate Developer ID:

                dd if=/dev/urandom bs=5 count=1 | hexdump

            The developer can then use the Developer ID to create unique
            (well, hopefully unique) Filter IDs.

                Bits    Mask                    Description
                 0-15   0x0000_0000_0000_FFFF   Filter ID
                16-55   0x00FF_FFFF_FFFF_0000   Developer ID
                56-62   0x3F00_0000_0000_0000   Static prefix: 0x3F

            The resulting 63-bit integer will use 9 bytes of space when
            stored using the encoding described in Section 1.2. To get
            a shorter ID, see the beginning of this Section how to
            request a custom ID range.


    5.4.1. Reserved Custom Filter ID Ranges

            Range                       Description
            0x0000_0300 - 0x0000_04FF   Reserved to ease .7z compatibility
            0x0002_0000 - 0x0007_FFFF   Reserved to ease .7z compatibility
            0x0200_0000 - 0x07FF_FFFF   Reserved to ease .7z compatibility


    6. Cyclic Redundancy Checks

            There are several incompatible variations to calculate CRC32
            and CRC64. For simplicity and clarity, complete examples are
            provided to calculate the checks as they are used in this file
            format. Implementations MAY use different code as long as it
            gives identical results.

            The program below reads data from standard input, calculates
            the CRC32 and CRC64 values, and prints the calculated values
            as big endian hexadecimal strings to standard output.

                #include <stddef.h>
                #include <inttypes.h>
                #include <stdio.h>

                uint32_t crc32_table[256];
                uint64_t crc64_table[256];

                void
                init(void)
                {
                    static const uint32_t poly32 = UINT32_C(0xEDB88320);
                    static const uint64_t poly64
                            = UINT64_C(0xC96C5795D7870F42);

                    for (size_t i = 0; i < 256; ++i) {
                        uint32_t crc32 = i;
                        uint64_t crc64 = i;

                        for (size_t j = 0; j < 8; ++j) {
                            if (crc32 & 1)
                                crc32 = (crc32 >> 1) ^ poly32;
                            else
                                crc32 >>= 1;

                            if (crc64 & 1)
                                crc64 = (crc64 >> 1) ^ poly64;
                            else
                                crc64 >>= 1;
                        }

                        crc32_table[i] = crc32;
                        crc64_table[i] = crc64;
                    }
                }

                uint32_t
                crc32(const uint8_t *buf, size_t size, uint32_t crc)
                {
                    crc = ~crc;
                    for (size_t i = 0; i < size; ++i)
                        crc = crc32_table[buf[i] ^ (crc & 0xFF)]
                                ^ (crc >> 8);
                    return ~crc;
                }

                uint64_t
                crc64(const uint8_t *buf, size_t size, uint64_t crc)
                {
                    crc = ~crc;
                    for (size_t i = 0; i < size; ++i)
                        crc = crc64_table[buf[i] ^ (crc & 0xFF)]
                                ^ (crc >> 8);
                    return ~crc;
                }

                int
                main()
                {
                    init();

                    uint32_t value32 = 0;
                    uint64_t value64 = 0;
                    uint64_t total_size = 0;
                    uint8_t buf[8192];

                    while (1) {
                        const size_t buf_size
                                = fread(buf, 1, sizeof(buf), stdin);
                        if (buf_size == 0)
                            break;

                        total_size += buf_size;
                        value32 = crc32(buf, buf_size, value32);
                        value64 = crc64(buf, buf_size, value64);
                    }

                    printf("Bytes:  %" PRIu64 "\n", total_size);
                    printf("CRC-32: 0x%08" PRIX32 "\n", value32);
                    printf("CRC-64: 0x%016" PRIX64 "\n", value64);

                    return 0;
                }


    7. References

            LZMA SDK - The original LZMA implementation
            https://7-zip.org/sdk.html

            LZMA Utils - LZMA adapted to POSIX-like systems
            https://tukaani.org/lzma/

            XZ Utils - The next generation of LZMA Utils
            https://tukaani.org/xz/

            [RFC-1952]
            GZIP file format specification version 4.3
            https://www.ietf.org/rfc/rfc1952.txt
              - Notation of byte boxes in section "2.1. Overall conventions"

            [RFC-2119]
            Key words for use in RFCs to Indicate Requirement Levels
            https://www.ietf.org/rfc/rfc2119.txt

            [GNU-tar]
            GNU tar 1.35 manual
            https://www.gnu.org/software/tar/manual/html_node/Blocking-Factor.html
              - Node 9.4.2 "Blocking Factor", paragraph that begins
                "gzip will complain about trailing garbage"
              - Note that this URL points to the latest version of the
                manual, and may some day not contain the note which is in
                1.35. For the exact version of the manual, download GNU
                tar 1.35: ftp://ftp.gnu.org/pub/gnu/tar/tar-1.35.tar.gz
    */
}
