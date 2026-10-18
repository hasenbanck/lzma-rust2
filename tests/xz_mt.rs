use std::{
    io::{Cursor, Read, Write},
    num::NonZeroU64,
    sync::{Arc, Mutex},
};

use lzma_rust2::{XzOptions, XzReader, XzReaderMt, XzWriterMt};

static EXECUTABLE: &str = "tests/data/executable.exe";
static PG100: &str = "tests/data/pg100.txt";
static PG6800: &str = "tests/data/pg6800.txt";

/// A sink that can be read while the writer still holds it.
#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Decodes the blocks of a stream that has no index and footer yet.
fn decode_blocks(compressed: &[u8]) -> Vec<u8> {
    let mut reader = XzReader::new(Cursor::new(compressed), false);
    let mut decoded = Vec::new();
    let mut buffer = [0; 8192];

    loop {
        match reader.read(&mut buffer) {
            // The stream ends in the middle of a record, so an error is expected here.
            Ok(0) | Err(_) => break,
            Ok(count) => decoded.extend_from_slice(&buffer[..count]),
        }
    }

    decoded
}

#[test]
fn flush_writes_out_every_block() {
    let data = std::fs::read(PG100).unwrap();
    let block_size = 128 * 1024;

    for num_workers in [1, 4] {
        let mut options = XzOptions::with_preset(6);
        options.lzma_options.dict_size = block_size as u32;
        options.set_block_size(NonZeroU64::new(block_size));

        let sink = SharedSink::default();
        let mut writer = XzWriterMt::new(sink.clone(), options, num_workers).unwrap();

        let mut written = 0;

        for chunk in data[..900 * 1024].chunks(300 * 1024) {
            writer.write_all(chunk).unwrap();
            writer.flush().unwrap();
            written += chunk.len();

            let compressed = sink.0.lock().unwrap().clone();

            // We don't use assert_eq since the debug output would be too big.
            assert!(decode_blocks(&compressed) == data[..written]);
        }

        // Writing has to continue to work after the flushes.
        writer.finish().unwrap();

        let compressed = sink.0.lock().unwrap().clone();
        let mut uncompressed = Vec::new();
        XzReader::new(Cursor::new(compressed.as_slice()), false)
            .read_to_end(&mut uncompressed)
            .unwrap();

        assert!(uncompressed == data[..written]);
    }
}

fn test_round_trip(path: &str, level: u32) {
    let data = std::fs::read(path).unwrap();

    let mut options = XzOptions::with_preset(level);
    let dict_size = options.lzma_options.dict_size as u64;
    options.set_block_size(NonZeroU64::new(dict_size));

    let mut compressed = Vec::new();

    {
        let mut writer = XzWriterMt::new(&mut compressed, options, 2).unwrap();
        writer.write_all(&data).unwrap();
        writer.finish().unwrap();
    }

    let mut uncompressed = Vec::new();
    {
        let mut reader = XzReaderMt::new(Cursor::new(compressed.as_slice()), false, 2).unwrap();
        let data_len = reader.read_to_end(&mut uncompressed).unwrap();

        if dict_size < data_len as u64 {
            assert!(reader.block_count() > 1);
        }
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
    let mut options = XzOptions::with_preset(6);
    let dict_size = options.lzma_options.dict_size as u64;
    options.set_block_size(NonZeroU64::new(dict_size));

    let encoder = XzWriterMt::new(Vec::new(), options, 2).unwrap();
    let compressed = encoder.finish().unwrap();

    let reference = {
        use liblzma::write::XzEncoder;

        let encoder = XzEncoder::new(Vec::new(), 6);
        encoder.finish().unwrap()
    };

    assert_eq!(compressed, reference);

    let mut uncompressed = Vec::new();
    let mut reader = XzReaderMt::new(Cursor::new(compressed.as_slice()), false, 2).unwrap();
    reader.read_to_end(&mut uncompressed).unwrap();
    assert_eq!(reader.block_count(), 0);
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

fn compress_mt(data: &[u8]) -> Vec<u8> {
    let mut options = XzOptions::with_preset(1);
    options.set_block_size(NonZeroU64::new(1 << 20));
    let mut writer = XzWriterMt::new(Vec::new(), options, 1).unwrap();
    writer.write_all(data).unwrap();
    writer.finish().unwrap()
}

fn decode_mt(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut reader = XzReaderMt::new(Cursor::new(compressed), true, 2)?;
    let mut uncompressed = Vec::new();
    reader.read_to_end(&mut uncompressed)?;
    Ok(uncompressed)
}

#[test]
fn a_gap_between_the_blocks_and_the_index_is_rejected() {
    let data = b"some data".repeat(100);
    let compressed = compress_mt(&data);
    assert!(decode_mt(&compressed).unwrap() == data);

    let footer_start = compressed.len() - 12;
    let backward_size = u32::from_le_bytes(
        compressed[footer_start + 4..footer_start + 8]
            .try_into()
            .unwrap(),
    );
    let index_start = footer_start - (backward_size as usize + 1) * 4;
    let mut broken = compressed[..index_start].to_vec();
    broken.extend_from_slice(&[0; 4]);
    broken.extend_from_slice(&compressed[index_start..]);

    assert_eq!(
        decode_mt(&broken).err().map(|error| error.kind()),
        Some(std::io::ErrorKind::InvalidData)
    );
}

#[test]
fn concatenated_streams_are_not_truncated() {
    let first = vec![b'A'; 100_000];
    let second = vec![b'B'; 100_000];
    let mut compressed = compress_mt(&first);
    let first_size = compressed.len();
    compressed.extend_from_slice(&compress_mt(&second));
    assert_eq!(compressed.len(), 2 * first_size);

    if let Ok(uncompressed) = decode_mt(&compressed) {
        assert!(uncompressed == [first, second].concat());
    }
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

fn empty_blocks(blocks: usize) -> Vec<u8> {
    let mut stream = vec![0xFD, b'7', b'z', b'X', b'Z', 0x00, 0x00, 0x00];
    stream.extend_from_slice(&crc32(&[0x00, 0x00]).to_le_bytes());

    // A 12 byte block header, the LZMA2 end marker and the block padding.
    let mut block = vec![0x02, 0x00, 0x21, 0x01, 0x00, 0x00, 0x00, 0x00];
    block.extend_from_slice(&crc32(&block).to_le_bytes());
    block.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    for _ in 0..blocks {
        stream.extend_from_slice(&block);
    }

    let mut index = vec![0x00];
    let mut count = blocks as u64;
    while count >= 0x80 {
        index.push(count as u8 | 0x80);
        count >>= 7;
    }
    index.push(count as u8);
    for _ in 0..blocks {
        index.extend_from_slice(&[13, 0x00]);
    }
    while index.len() % 4 != 0 {
        index.push(0);
    }
    index.extend_from_slice(&crc32(&index).to_le_bytes());

    let mut fields = ((index.len() / 4 - 1) as u32).to_le_bytes().to_vec();
    fields.extend_from_slice(&[0x00, 0x00]);
    stream.extend_from_slice(&index);
    stream.extend_from_slice(&crc32(&fields).to_le_bytes());
    stream.extend_from_slice(&fields);
    stream.extend_from_slice(b"YZ");
    stream
}

#[test]
fn many_empty_blocks_are_read() {
    let compressed = empty_blocks(20_000);

    let decoded = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || decode_mt(&compressed).map(|decoded| decoded.len()))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(decoded.unwrap(), 0);
}
