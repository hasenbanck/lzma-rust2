use std::{
    io::{Cursor, Read, Write},
    num::{NonZero, NonZeroU64},
    sync::{Arc, Mutex},
};

use lzma_rust2::{LzipOptions, LzipReader, LzipReaderMt, LzipWriterMt};

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

#[test]
fn flush_writes_out_every_member() {
    let data = std::fs::read(PG100).unwrap();
    let member_size = 128 * 1024;

    for num_workers in [1, 4] {
        let mut options = LzipOptions::with_preset(6);
        options.lzma_options.dict_size = member_size as u32;
        options.set_member_size(NonZeroU64::new(member_size));

        let sink = SharedSink::default();
        let mut writer = LzipWriterMt::new(sink.clone(), options, num_workers).unwrap();

        let mut written = 0;

        for chunk in data[..900 * 1024].chunks(300 * 1024) {
            writer.write_all(chunk).unwrap();
            writer.flush().unwrap();
            written += chunk.len();

            // Every member that was flushed is complete, so this is a valid file.
            let compressed = sink.0.lock().unwrap().clone();
            let mut uncompressed = Vec::new();
            LzipReader::new(Cursor::new(compressed.as_slice()))
                .read_to_end(&mut uncompressed)
                .unwrap();

            // We don't use assert_eq since the debug output would be too big.
            assert!(uncompressed == data[..written]);
        }

        // Writing has to continue to work after the flushes.
        writer.finish().unwrap();

        let compressed = sink.0.lock().unwrap().clone();
        let mut uncompressed = Vec::new();
        LzipReader::new(Cursor::new(compressed.as_slice()))
            .read_to_end(&mut uncompressed)
            .unwrap();

        assert!(uncompressed == data[..written]);
    }
}

fn test_round_trip(path: &str, level: u32) {
    let data = std::fs::read(path).unwrap();
    let data_len = data.len() as u32;

    let available_parallelism = std::thread::available_parallelism()
        .unwrap_or(NonZero::new(1).unwrap())
        .get()
        .min(256) as u32;

    let mut options = LzipOptions::with_preset(level);
    let dict_size = options.lzma_options.dict_size;
    options.set_member_size(NonZeroU64::new(dict_size as u64));

    let mut compressed = Vec::new();

    {
        let mut writer =
            LzipWriterMt::new(&mut compressed, options, available_parallelism).unwrap();
        writer.write_all(&data).unwrap();
        writer.finish().unwrap();
    }

    let mut uncompressed = Vec::new();

    {
        let cursor = Cursor::new(compressed);
        let mut reader = LzipReaderMt::new(cursor, available_parallelism).unwrap();
        reader.read_to_end(&mut uncompressed).unwrap();

        if dict_size < data_len {
            assert!(reader.member_count() > 1);
        }
    }

    // We don't use assert_eq since the debug output would be too big.
    assert!(uncompressed.as_slice() == data);
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
