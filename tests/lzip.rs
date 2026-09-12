use std::io::{Read, Write};

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

struct ShortInput<'a> {
    bytes: &'a [u8],
    chunk: usize,
}

impl Read for ShortInput<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let count = self.chunk.min(output.len());
        self.bytes.read(&mut output[..count])
    }
}

#[test]
fn buffered_lzip_concatenated_members() {
    let mut compressed = Vec::new();
    for payload in [b"first".as_slice(), b"", b"last member"] {
        let mut writer = LzipWriter::new(&mut compressed, LzipOptions::with_preset(0));
        writer.write_all(payload).unwrap();
        writer.finish().unwrap();
    }
    for chunk in (1..=64).chain([65536]) {
        let source = ShortInput {
            bytes: &compressed,
            chunk,
        };
        let mut reader = LzipReader::new(source);
        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();
        assert_eq!(output, b"firstlast member", "chunk={chunk}");
        assert!(reader.into_inner().bytes.is_empty());
    }
}

#[test]
fn buffered_lzip_recovers_reader_after_bad_trailer() {
    let mut writer = LzipWriter::new(Vec::new(), LzipOptions::with_preset(0));
    writer.write_all(b"member with a damaged trailer").unwrap();
    let compressed = writer.finish().unwrap();
    for missing in 1..=20 {
        let mut reader = LzipReader::new(&compressed[..compressed.len() - missing]);
        assert!(reader.read_to_end(&mut Vec::new()).is_err());
        assert!(reader.inner().is_empty());
        assert!(reader.inner_mut().is_empty());
        assert!(reader.into_inner().is_empty());
    }
    let mut damaged = compressed;
    let crc_position = damaged.len() - 20;
    damaged[crc_position] ^= 1;
    let mut reader = LzipReader::new(damaged.as_slice());
    assert_eq!(
        reader.read_to_end(&mut Vec::new()).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    assert!(reader.into_inner().is_empty());
}

/// A valid 6 byte LZIP header for a 4 KiB dictionary.
const HEADER: &[u8] = b"LZIP\x01\x0c";

fn valid_member(payload: &[u8]) -> Vec<u8> {
    let mut writer = LzipWriter::new(Vec::new(), LzipOptions::with_preset(0));
    writer.write_all(payload).unwrap();
    writer.finish().unwrap()
}

#[test]
fn lzip_recovers_reader_when_a_member_cannot_start() {
    // A member whose range coder initialisation is missing or malformed makes
    // the LZMA reader fail to construct. The LZIP reader has to keep hold of
    // its source so that a later read reports the error again.
    let mut inputs = Vec::new();
    for tail in [
        b"".as_slice(),
        b"\x00",
        b"\x00\x00\x00\x00",
        b"\xFF\x00\x00\x00\x00",
    ] {
        let mut input = HEADER.to_vec();
        input.extend_from_slice(tail);
        inputs.push(input);
    }

    for input in inputs {
        let mut reader = LzipReader::new(input.as_slice());
        let first = reader.read(&mut [0; 64]).unwrap_err();
        let second = reader.read(&mut [0; 64]).unwrap_err();
        assert_eq!(second.kind(), first.kind(), "input={input:?}");
        assert_eq!(second.to_string(), first.to_string(), "input={input:?}");
        let third = reader.read(&mut [0; 64]).unwrap_err();
        assert_eq!(third.kind(), first.kind(), "input={input:?}");
        assert!(reader.inner().len() <= input.len());
        assert!(reader.inner_mut().len() <= input.len());
        assert!(reader.into_inner().len() <= input.len());
    }
}

#[test]
fn lzip_recovers_reader_when_a_later_member_cannot_start() {
    let mut input = valid_member(b"a good first member");
    input.extend_from_slice(HEADER);
    input.extend_from_slice(b"\xFF\x00\x00\x00\x00");

    let mut reader = LzipReader::new(input.as_slice());
    let mut output = Vec::new();
    let mut buffer = [0; 8];
    let first = loop {
        match reader.read(&mut buffer) {
            Ok(0) => panic!("the corrupt second member was accepted"),
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) => break error,
        }
    };
    assert_eq!(output, b"a good first member");
    let second = reader.read(&mut buffer).unwrap_err();
    assert_eq!(second.kind(), first.kind());
    assert_eq!(second.to_string(), first.to_string());
    let _ = reader.into_inner();
}

#[test]
fn lzip_member_errors_repeat_on_every_read() {
    let good = valid_member(b"a member with a broken trailer");

    let mut cases = Vec::new();
    for missing in 1..=20 {
        cases.push(good[..good.len() - missing].to_vec());
    }
    // The trailer holds the CRC32, then the data size, then the member size.
    for offset in [20, 16, 8] {
        let mut damaged = good.clone();
        let position = damaged.len() - offset;
        damaged[position] ^= 1;
        cases.push(damaged);
    }

    for case in cases {
        let mut reader = LzipReader::new(case.as_slice());
        let first = reader.read_to_end(&mut Vec::new()).unwrap_err();
        for _ in 0..3 {
            let again = reader.read(&mut [0; 64]).unwrap_err();
            assert_eq!(again.kind(), first.kind(), "len={}", case.len());
            assert_eq!(again.to_string(), first.to_string(), "len={}", case.len());
        }
    }
}

#[test]
fn lzip_into_parts_returns_the_data_behind_the_stream() {
    let payload: Vec<u8> = (0..400000).map(|index| (index * 31 % 251) as u8).collect();
    let member = valid_member(&payload);

    // A stream that ends where the source ends.
    let mut reader = LzipReader::new(member.as_slice());
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert!(output == payload);
    let (source, buffered) = reader.into_parts();
    assert!(buffered.is_empty());
    assert!(source.is_empty());

    // A stream with something behind it. The reader rejects that, and hands
    // every byte of it back.
    for tail in [
        b"T".as_slice(),
        b"LZIP",
        b"a tail that is no LZIP member at all",
    ] {
        let mut input = member.clone();
        input.extend_from_slice(tail);

        let mut reader = LzipReader::new(input.as_slice());
        let mut output = Vec::new();
        assert!(reader.read_to_end(&mut output).is_err(), "tail={tail:?}");
        assert!(output == payload, "tail={tail:?}");

        let (source, mut recovered) = reader.into_parts();
        recovered.extend_from_slice(source);
        assert_eq!(recovered, tail, "tail={tail:?}");
    }
}

#[test]
fn lzip_into_parts_works_while_a_member_is_open() {
    let payload = vec![b'z'; 200000];
    let mut input = valid_member(&payload);
    let tail = b"behind the stream";
    input.extend_from_slice(tail);

    let mut reader = LzipReader::new(input.as_slice());
    assert_eq!(reader.read(&mut [0; 16]).unwrap(), 16);

    let (source, mut recovered) = reader.into_parts();
    recovered.extend_from_slice(source);
    assert!(recovered.len() < input.len());
    assert_eq!(&recovered[recovered.len() - tail.len()..], tail);
}

#[test]
fn lzip_rejects_data_behind_the_last_member() {
    let member = valid_member(b"a member with something behind it");

    for tail in [
        b"T".as_slice(),
        b"LZI",
        b"LZIP",
        // A second member that claims a version this decoder does not know.
        b"LZIP\x02\x0c",
        // A second member whose dictionary size field is out of range.
        b"LZIP\x01\x00",
        // A second member that stops after its header.
        b"LZIP\x01\x0c",
        b"a tail that is no LZIP member at all",
    ] {
        let mut input = member.clone();
        input.extend_from_slice(tail);

        let mut reader = LzipReader::new(input.as_slice());
        let mut output = Vec::new();
        let error = reader.read_to_end(&mut output).unwrap_err();
        assert_eq!(
            output, b"a member with something behind it",
            "tail={tail:?}"
        );
        // The rejection has to hold for every later read.
        let again = reader.read(&mut [0; 64]).unwrap_err();
        assert_eq!(again.kind(), error.kind(), "tail={tail:?}");
        assert_eq!(again.to_string(), error.to_string(), "tail={tail:?}");
    }
}

#[test]
fn lzip_rejects_a_file_that_holds_no_member() {
    for input in [b"T".as_slice(), b"LZI", b"LZIP", b"LZIP\x01"] {
        let mut reader = LzipReader::new(input);
        assert!(
            reader.read_to_end(&mut Vec::new()).is_err(),
            "input={input:?}"
        );
    }

    // An empty source is a complete file of zero members.
    let mut reader = LzipReader::new(b"".as_slice());
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert!(output.is_empty());
}

#[test]
fn lzip_reports_a_source_error_between_members() {
    struct Breaking<'a> {
        bytes: &'a [u8],
    }

    impl Read for Breaking<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.bytes.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "the source went away",
                ));
            }
            self.bytes.read(output)
        }
    }

    let member = valid_member(b"a member the source delivered whole");
    let source = Breaking { bytes: &member };
    let mut reader = LzipReader::new(source);
    let mut output = Vec::new();
    let error = reader.read_to_end(&mut output).unwrap_err();
    assert_eq!(output, b"a member the source delivered whole");
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
    assert_eq!(error.to_string(), "the source went away");
}

#[test]
fn lzip_retries_an_interrupted_read_between_members() {
    struct Interrupting<'a> {
        bytes: &'a [u8],
        interrupt: bool,
    }

    impl Read for Interrupting<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let count = output.len().min(3);
            self.bytes.read(&mut output[..count])
        }
    }

    let mut compressed = Vec::new();
    for payload in [b"first member".as_slice(), b"second member"] {
        let mut writer = LzipWriter::new(&mut compressed, LzipOptions::with_preset(0));
        writer.write_all(payload).unwrap();
        writer.finish().unwrap();
    }

    let source = Interrupting {
        bytes: &compressed,
        interrupt: false,
    };
    let mut reader = LzipReader::new(source);
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"first membersecond member");
}
