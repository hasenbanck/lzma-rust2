use std::{
    io::{self, Cursor, Read, Write},
    num::NonZeroU64,
};

use lzma_rust2::{CheckType, DICT_SIZE_MIN, XzOptions, XzReader, XzWriter, xz_uncompressed_size};

static EXECUTABLE: &str = "tests/data/executable.exe";
static PG100: &str = "tests/data/pg100.txt";
static PG6800: &str = "tests/data/pg6800.txt";

fn test_round_trip(path: &str, level: u32) {
    let data = std::fs::read(path).unwrap();

    let option = XzOptions::with_preset(level);

    let mut compressed = Vec::new();

    {
        let mut writer = XzWriter::new(&mut compressed, option).unwrap();
        writer.write_all(&data).unwrap();
        writer.finish().unwrap();
    }

    // Test decompression with our own reader
    let mut uncompressed = Vec::new();
    {
        let mut reader = XzReader::new(compressed.as_slice(), false);
        reader.read_to_end(&mut uncompressed).unwrap();
    }

    // We don't use assert_eq since the debug output would be too big.
    assert!(uncompressed.as_slice() == data);

    // Also test decompression with liblzma to ensure compatibility
    let mut liblzma_uncompressed = Vec::new();
    {
        use liblzma::read::XzDecoder;
        let mut decoder = XzDecoder::new(compressed.as_slice());
        decoder.read_to_end(&mut liblzma_uncompressed).unwrap();
    }

    assert!(liblzma_uncompressed.as_slice() == data);
}

#[test]
fn empty_input_is_valid_empty_stream() {
    let encoder = XzWriter::new(Vec::new(), XzOptions::with_preset(6)).unwrap();
    let compressed = encoder.finish().unwrap();

    let reference = {
        use liblzma::write::XzEncoder;

        let encoder = XzEncoder::new(Vec::new(), 6);
        encoder.finish().unwrap()
    };

    assert_eq!(compressed, reference);

    let mut uncompressed = Vec::new();
    let mut reader = XzReader::new(compressed.as_slice(), false);
    reader.read_to_end(&mut uncompressed).unwrap();
    assert!(uncompressed.is_empty());

    let mut liblzma_uncompressed = Vec::new();
    {
        use liblzma::read::XzDecoder;
        let mut decoder = XzDecoder::new(compressed.as_slice());
        decoder.read_to_end(&mut liblzma_uncompressed).unwrap();
    }
    assert!(liblzma_uncompressed.is_empty());
}

#[test]
fn small_writes_with_flush_round_trip() {
    let mut encoder = XzWriter::new(Vec::new(), XzOptions::with_preset(6)).unwrap();
    encoder.write_all(&[0]).unwrap();
    encoder.flush().unwrap();
    encoder.write_all(&[0]).unwrap();
    encoder.flush().unwrap();
    let compressed = encoder.finish().unwrap();

    let mut uncompressed = Vec::new();
    let mut reader = XzReader::new(compressed.as_slice(), false);
    reader.read_to_end(&mut uncompressed).unwrap();
    assert_eq!(uncompressed, [0, 0]);

    let mut liblzma_uncompressed = Vec::new();
    {
        use liblzma::read::XzDecoder;
        let mut decoder = XzDecoder::new(compressed.as_slice());
        decoder.read_to_end(&mut liblzma_uncompressed).unwrap();
    }
    assert_eq!(liblzma_uncompressed, [0, 0]);
}

#[test]
fn round_trip_executable_0() {
    test_round_trip(EXECUTABLE, 0);
}

#[test]
fn round_trip_executable_1() {
    test_round_trip(EXECUTABLE, 1);
}

#[test]
fn round_trip_executable_2() {
    test_round_trip(EXECUTABLE, 2);
}

#[test]
fn round_trip_executable_3() {
    test_round_trip(EXECUTABLE, 3);
}

#[test]
fn round_trip_executable_4() {
    test_round_trip(EXECUTABLE, 4);
}

#[test]
fn round_trip_executable_5() {
    test_round_trip(EXECUTABLE, 5);
}

#[test]
fn round_trip_executable_6() {
    test_round_trip(EXECUTABLE, 6);
}

#[test]
fn round_trip_executable_7() {
    test_round_trip(EXECUTABLE, 7);
}

#[test]
fn round_trip_executable_8() {
    test_round_trip(EXECUTABLE, 8);
}

#[test]
fn round_trip_executable_9() {
    test_round_trip(EXECUTABLE, 9);
}

#[test]
fn round_trip_pg100_0() {
    test_round_trip(PG100, 0);
}

#[test]
fn round_trip_pg100_1() {
    test_round_trip(PG100, 1);
}

#[test]
fn round_trip_pg100_2() {
    test_round_trip(PG100, 2);
}

#[test]
fn round_trip_pg100_3() {
    test_round_trip(PG100, 3);
}

#[test]
fn round_trip_pg100_4() {
    test_round_trip(PG100, 4);
}

#[test]
fn round_trip_pg100_5() {
    test_round_trip(PG100, 5);
}

#[test]
fn round_trip_pg100_6() {
    test_round_trip(PG100, 6);
}

#[test]
fn round_trip_pg100_7() {
    test_round_trip(PG100, 7);
}

#[test]
fn round_trip_pg100_8() {
    test_round_trip(PG100, 8);
}

#[test]
fn round_trip_pg100_9() {
    test_round_trip(PG100, 9);
}

#[test]
fn round_trip_pg6800_0() {
    test_round_trip(PG6800, 0);
}

#[test]
fn round_trip_pg6800_1() {
    test_round_trip(PG6800, 1);
}

#[test]
fn round_trip_pg6800_2() {
    test_round_trip(PG6800, 2);
}

#[test]
fn round_trip_pg6800_3() {
    test_round_trip(PG6800, 3);
}

#[test]
fn round_trip_pg6800_4() {
    test_round_trip(PG6800, 4);
}

#[test]
fn round_trip_pg6800_5() {
    test_round_trip(PG6800, 5);
}

#[test]
fn round_trip_pg6800_6() {
    test_round_trip(PG6800, 6);
}

#[test]
fn round_trip_pg6800_7() {
    test_round_trip(PG6800, 7);
}

#[test]
fn round_trip_pg6800_8() {
    test_round_trip(PG6800, 8);
}

#[test]
fn round_trip_pg6800_9() {
    test_round_trip(PG6800, 9);
}

const STREAM_FOOTER_SIZE: usize = 12;

fn compress(data: &[u8], block_size: Option<u64>) -> Vec<u8> {
    let mut option = XzOptions::with_preset(1);
    // The block size is raised to at least the dictionary size.
    option.lzma_options.dict_size = DICT_SIZE_MIN;
    option.set_block_size(block_size.and_then(NonZeroU64::new));
    let mut writer = XzWriter::new(Vec::new(), option).unwrap();
    writer.write_all(data).unwrap();
    writer.finish().unwrap()
}

fn uncompressed_size(compressed: &[u8]) -> io::Result<u64> {
    xz_uncompressed_size(Cursor::new(compressed))
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

fn stream_footer(backward_size: u32, check_type: CheckType) -> Vec<u8> {
    let mut fields = backward_size.to_le_bytes().to_vec();
    fields.extend_from_slice(&[0, check_type as u8]);
    let mut footer = crc32(&fields).to_le_bytes().to_vec();
    footer.extend_from_slice(&fields);
    footer.extend_from_slice(b"YZ");
    footer
}

#[test]
fn uncompressed_size_of_a_single_stream() {
    let data = std::fs::read(PG100).unwrap();
    assert_eq!(
        uncompressed_size(&compress(&data, None)).unwrap(),
        data.len() as u64
    );
}

#[test]
fn uncompressed_size_adds_all_blocks() {
    let data = std::fs::read(PG100).unwrap();
    let compressed = compress(&data, Some(64 * 1024));
    assert_eq!(uncompressed_size(&compressed).unwrap(), data.len() as u64);
}

#[test]
fn uncompressed_size_of_an_empty_stream_is_zero() {
    assert_eq!(uncompressed_size(&compress(&[], None)).unwrap(), 0);
}

#[test]
fn uncompressed_size_of_a_liblzma_stream() {
    use liblzma::write::XzEncoder;

    let data = std::fs::read(PG6800).unwrap();
    let mut encoder = XzEncoder::new(Vec::new(), 6);
    encoder.write_all(&data).unwrap();
    let compressed = encoder.finish().unwrap();
    assert_eq!(uncompressed_size(&compressed).unwrap(), data.len() as u64);
}

#[test]
fn uncompressed_size_adds_concatenated_streams_and_skips_padding() {
    let first = b"first stream".repeat(100);
    let second = b"second stream".repeat(1000);
    let mut compressed = compress(&first, None);
    compressed.extend_from_slice(&[0; 10_000]);
    compressed.extend_from_slice(&compress(&second, Some(4096)));
    compressed.extend_from_slice(&[0; 4]);

    let expected = (first.len() + second.len()) as u64;
    assert_eq!(uncompressed_size(&compressed).unwrap(), expected);

    // The size agrees with what the reader decodes.
    let mut decoded = Vec::new();
    XzReader::new(compressed.as_slice(), true)
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded.len() as u64, expected);
}

#[test]
fn uncompressed_size_rejects_broken_files() {
    let compressed = compress(b"some data", None);

    let mut truncated = compressed.clone();
    truncated.pop();
    assert!(uncompressed_size(&truncated).is_err());

    let mut trailing = compressed.clone();
    trailing.extend_from_slice(b"garbage!");
    assert!(uncompressed_size(&trailing).is_err());

    let mut unaligned_padding = compressed.clone();
    unaligned_padding.extend_from_slice(&[0; 2]);
    assert!(uncompressed_size(&unaligned_padding).is_err());

    let mut leading = vec![0; 4];
    leading.extend_from_slice(&compressed);
    assert!(uncompressed_size(&leading).is_err());

    let mut bad_index_crc = compressed.clone();
    let index_end = bad_index_crc.len() - STREAM_FOOTER_SIZE;
    bad_index_crc[index_end - 1] ^= 1;
    assert!(uncompressed_size(&bad_index_crc).is_err());

    assert!(uncompressed_size(&[0; 10_000]).is_err());
    assert!(uncompressed_size(&[]).is_err());
}

#[test]
fn uncompressed_size_starts_at_the_reader_position() {
    let data = b"some data";
    let prefix = b"prefix";
    let mut file = prefix.to_vec();
    file.extend_from_slice(&compress(data, None));

    let mut reader = Cursor::new(file);
    reader.set_position(prefix.len() as u64);
    assert_eq!(
        xz_uncompressed_size(&mut reader).unwrap(),
        data.len() as u64
    );
}

#[test]
fn uncompressed_size_rejects_an_index_shorter_than_the_backward_size() {
    let empty = compress(&[], None);
    let (header_and_index, footer) = empty.split_at(empty.len() - STREAM_FOOTER_SIZE);
    assert_eq!(footer, stream_footer(1, CheckType::Crc64));

    // The backward size covers the 8 bytes of the empty index and 4 more.
    let mut broken = header_and_index.to_vec();
    broken.extend_from_slice(&[0xAA; 4]);
    broken.extend_from_slice(&stream_footer(2, CheckType::Crc64));
    assert!(uncompressed_size(&broken).is_err());
}

#[test]
fn uncompressed_size_rejects_footer_flags_that_differ_from_the_header() {
    let mut broken = compress(&[], None);
    broken.truncate(broken.len() - STREAM_FOOTER_SIZE);
    broken.extend_from_slice(&stream_footer(1, CheckType::Crc32));
    assert!(uncompressed_size(&broken).is_err());
}

fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn xz_reader_rejects_unaligned_trailing_padding() {
    let compressed = compress(b"some data", None);

    let mut aligned = compressed.clone();
    aligned.extend_from_slice(&[0; 4]);
    let mut decoded = Vec::new();
    XzReader::new(aligned.as_slice(), true)
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, b"some data");

    for padding in 1..4 {
        let mut unaligned = compressed.clone();
        unaligned.extend_from_slice(&vec![0; padding]);
        let result = XzReader::new(unaligned.as_slice(), true).read_to_end(&mut Vec::new());
        assert!(result.is_err(), "{padding} bytes of padding");
    }
}

#[test]
fn xz_reader_reads_many_empty_streams() {
    let empty = compress(&[], None);
    let compressed = empty.repeat(10_000);

    let decoded = on_small_stack(move || {
        let mut decoded = Vec::new();
        XzReader::new(compressed.as_slice(), true)
            .read_to_end(&mut decoded)
            .map(|_| decoded.len())
    });
    assert_eq!(decoded.unwrap(), 0);
}
