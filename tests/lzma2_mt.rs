use std::io::{Cursor, Read, Write};

use lzma_rust2::{LZMA2ReaderMT, LZMA2WriterMT, LZMAOptions, MIN_STREAM_SIZE};

static EXECUTABLE: &[u8] = include_bytes!("data/executable.exe");
static PG100: &[u8] = include_bytes!("data/pg100.txt");
static PG6800: &[u8] = include_bytes!("data/pg6800.txt");

fn test_round_trip(data: &[u8], level: u32) {
    let option = LZMAOptions::with_preset(level);

    let mut compressed = Vec::new();

    {
        let mut writer = LZMA2WriterMT::new(&mut compressed, &option, MIN_STREAM_SIZE, 32);
        writer.write_all(data).unwrap();
        writer.finish().unwrap();
    }

    let mut uncompressed = Vec::new();

    {
        let mut reader = LZMA2ReaderMT::new(Cursor::new(compressed), option.dict_size, None, 32);
        reader.read_to_end(&mut uncompressed).unwrap();
        assert!(reader.stream_count() > 1);
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
