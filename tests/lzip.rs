use std::io::{self, Read, Write};

use lzma_rust2::{LzipOptions, LzipReader, LzipWriter};

static EXECUTABLE: &str = "tests/data/executable.exe";
static PG100: &str = "tests/data/pg100.txt";
static PG6800: &str = "tests/data/pg6800.txt";

fn test_round_trip(path: &str, level: u32) {
    let data = std::fs::read(path).unwrap();

    let option = LzipOptions::with_preset(level);

    let mut compressed = Vec::new();

    {
        let mut writer = LzipWriter::new(&mut compressed, option);
        writer.write_all(&data).unwrap();
        writer.finish().unwrap();
    }

    let mut uncompressed = Vec::new();

    {
        let mut reader = LzipReader::new(compressed.as_slice());
        reader.read_to_end(&mut uncompressed).unwrap();
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

/// Hands out at most `max` bytes per call, the way a pipe or a socket might.
struct Chunks<'a> {
    data: &'a [u8],
    max: usize,
}

impl Read for Chunks<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = out.len().min(self.max).min(self.data.len());
        out[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

fn member(data: &[u8]) -> Vec<u8> {
    let mut compressed = Vec::new();
    let mut writer = LzipWriter::new(&mut compressed, LzipOptions::with_preset(0));
    writer.write_all(data).unwrap();
    writer.finish().unwrap();
    compressed
}

/// The LZMA reader under a member reads ahead of the member's stream. However
/// few bytes arrive per read, the trailer and the member after it are read
/// from what it read ahead, and nothing is lost between them.
#[test]
fn members_are_read_whole_however_the_input_arrives() {
    let mut compressed = member(b"Hello, world!");
    compressed.extend_from_slice(&member(b"Goodbye, world!"));
    for max in 1..=64 {
        let mut reader = LzipReader::new(Chunks {
            data: &compressed,
            max,
        });
        let mut out = Vec::new();
        reader
            .read_to_end(&mut out)
            .unwrap_or_else(|error| panic!("{max} bytes per read: {error}"));
        assert_eq!(out, b"Hello, world!Goodbye, world!");
    }
}

/// A bad trailer fails the read, and the reader still has its inner reader
/// to give back afterwards.
#[test]
fn into_inner_survives_a_bad_trailer() {
    let mut compressed = member(b"Hello, world!");
    let crc_byte = compressed.len() - 20;
    compressed[crc_byte] ^= 0xFF;

    let mut reader = LzipReader::new(compressed.as_slice());
    let error = reader.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(error.to_string(), "LZIP CRC32 mismatch");
    assert!(reader.into_inner().is_empty());
}
