use std::io::{self, ErrorKind, Read, Write};

use lzma_rust2::{LzmaOptions, LzmaReader, LzmaWriter};

static EXECUTABLE: &str = "tests/data/executable.exe";
static PG100: &str = "tests/data/pg100.txt";
static PG6800: &str = "tests/data/pg6800.txt";

fn test_round_trip(path: &str, level: u32) {
    let data = std::fs::read(path).unwrap();

    let option = LzmaOptions::with_preset(level);

    let mut compressed = Vec::new();

    {
        let mut writer = LzmaWriter::new_no_header(&mut compressed, &option, true).unwrap();
        writer.write_all(&data).unwrap();
        writer.finish().unwrap();
    }

    let mut uncompressed = Vec::new();

    {
        let mut reader = LzmaReader::new(
            compressed.as_slice(),
            data.len() as u64,
            option.lc,
            option.lp,
            option.pb,
            option.dict_size,
            option.preset_dict.as_ref().map(|dict| dict.as_ref()),
        )
        .unwrap();
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

/// Fails with the given error once its bytes are gone, instead of reporting
/// an end. A socket that stays open with nothing to say does that.
struct FailsAfter<'a> {
    data: &'a [u8],
    kind: ErrorKind,
    message: &'static str,
}

impl Read for FailsAfter<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.data.is_empty() {
            return Err(io::Error::new(self.kind, self.message));
        }
        self.data.read(out)
    }
}

/// "Hello, world!" as a raw LZMA stream at preset 0, with or without an end
/// marker, and the size to open it with.
fn hello(end_marker: bool) -> (Vec<u8>, u64) {
    let options = LzmaOptions::with_preset(0);
    let mut compressed = Vec::new();
    let mut writer = LzmaWriter::new_no_header(&mut compressed, &options, end_marker).unwrap();
    writer.write_all(b"Hello, world!").unwrap();
    writer.finish().unwrap();
    let size = if end_marker { u64::MAX } else { 13 };
    (compressed, size)
}

fn hello_reader<R: Read>(reader: R, size: u64) -> LzmaReader<R> {
    let options = LzmaOptions::with_preset(0);
    LzmaReader::new_with_props(reader, size, 93, options.dict_size, None).unwrap()
}

/// The reader buffers its input. What it read past the end of the LZMA stream
/// is handed back with the inner reader, so a caller can go on from there. A
/// stream of known size ends with its last byte, and one of unknown size with
/// its end marker.
#[test]
fn into_parts_returns_the_bytes_after_the_stream() {
    let data = std::fs::read(PG6800).unwrap();
    let option = LzmaOptions::with_preset(3);
    for end_marker in [false, true] {
        let mut compressed = Vec::new();
        {
            let mut writer =
                LzmaWriter::new_no_header(&mut compressed, &option, end_marker).unwrap();
            writer.write_all(&data).unwrap();
            writer.finish().unwrap();
        }
        let stream_len = compressed.len();
        let trailer: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        compressed.extend_from_slice(&trailer);

        let size = if end_marker {
            u64::MAX
        } else {
            data.len() as u64
        };
        let mut reader = LzmaReader::new(
            std::io::Cursor::new(compressed),
            size,
            option.lc,
            option.lp,
            option.pb,
            option.dict_size,
            None,
        )
        .unwrap();
        let mut uncompressed = Vec::new();
        reader.read_to_end(&mut uncompressed).unwrap();
        assert!(uncompressed == data);

        let (mut inner, leftover) = reader.into_parts();
        let mut rest = leftover;
        inner.read_to_end(&mut rest).unwrap();
        assert!(
            rest == trailer,
            "end marker {end_marker}: {} bytes over",
            rest.len() - trailer.len()
        );
        assert_eq!(inner.position() as usize, stream_len + trailer.len());
    }
}

/// However few bytes the inner reader hands out per call, the bytes after the
/// stream come back whole. A read that ends a few bytes short of a symbol must
/// not lose what came after the stream.
#[test]
fn into_parts_is_exact_however_the_input_arrives() {
    for end_marker in [false, true] {
        let (mut compressed, size) = hello(end_marker);
        compressed.extend_from_slice(b"TAIL");
        for max in [1, 2, 3, 7, 19, 64, 65536] {
            let chunks = Chunks {
                data: &compressed,
                max,
            };
            let mut reader = hello_reader(chunks, size);
            let mut out = Vec::new();
            reader.read_to_end(&mut out).unwrap();
            assert_eq!(out, b"Hello, world!");
            let (mut inner, mut rest) = reader.into_parts();
            inner.read_to_end(&mut rest).unwrap();
            assert_eq!(
                rest, b"TAIL",
                "end marker {end_marker}, {max} bytes per read"
            );
        }
    }
}

/// A stream that has arrived whole ends on its own. The reader must not ask
/// the source for more, since a source that stays open, like a socket, would
/// block or fail rather than report an end.
#[test]
fn a_complete_stream_ends_without_the_source_saying_so() {
    for end_marker in [false, true] {
        let (compressed, size) = hello(end_marker);
        let source = FailsAfter {
            data: &compressed,
            kind: ErrorKind::WouldBlock,
            message: "asked for more",
        };
        let mut reader = hello_reader(source, size);
        let mut out = Vec::new();
        let result = reader.read_to_end(&mut out);
        assert!(result.is_ok(), "end marker {end_marker}: {result:?}");
        assert_eq!(out, b"Hello, world!");
    }
}

/// An error from the source comes back from `read` as it was. Before, the
/// decoder went on as if it had read ones, and reported corrupt data at best.
#[test]
fn an_error_from_the_source_comes_back_as_it_was() {
    let (compressed, size) = hello(true);
    let cut = compressed.len() - 3;

    let source = FailsAfter {
        data: &compressed[..cut],
        kind: ErrorKind::ConnectionReset,
        message: "the peer went away",
    };
    let mut reader = hello_reader(source, size);
    let error = reader.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ConnectionReset);
    assert_eq!(error.to_string(), "the peer went away");

    let mut reader = hello_reader(&compressed[..cut], size);
    let error = reader.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
}
