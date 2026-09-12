use std::io::{Read, Write};

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

struct LimitedInput<'a> {
    remaining: &'a [u8],
    max_read: usize,
    calls: usize,
    end_error: std::io::ErrorKind,
}

impl Read for LimitedInput<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.calls += 1;
        if self.remaining.is_empty() {
            return Err(std::io::Error::new(self.end_error, "source read failed"));
        }
        let count = output.len().min(self.max_read).min(self.remaining.len());
        self.remaining.read(&mut output[..count])
    }
}

/// Source reads the decoder needs for `length` compressed bytes, when every
/// read delivers at most `max_read`. The input buffer starts at 4 KiB and
/// quadruples per refill up to 64 KiB.
fn expected_calls(length: usize, max_read: usize) -> usize {
    let mut calls = 0;
    let mut consumed = 0;
    let mut buffer = 4 * 1024;
    while consumed < length {
        consumed += buffer.min(max_read);
        calls += 1;
        buffer = (buffer * 4).min(64 * 1024);
    }
    calls
}

fn raw_stream(payload: &[u8], marker: bool) -> (Vec<u8>, LzmaOptions) {
    let options = LzmaOptions::with_preset(0);
    let mut writer = LzmaWriter::new_no_header(Vec::new(), &options, marker).unwrap();
    writer.write_all(payload).unwrap();
    (writer.finish().unwrap(), options)
}

#[test]
fn buffered_lzma_preserves_stream_boundaries() {
    for payload in [
        b"".as_slice(),
        b"a",
        b"A complete stream followed by another record.",
    ] {
        for marker in [false, true] {
            let (compressed, options) = raw_stream(payload, marker);
            for max_read in (1..=64).chain([65536]) {
                for trailer in [b"".as_slice(), b"TAIL"] {
                    let mut input = compressed.clone();
                    input.extend_from_slice(trailer);
                    let source = LimitedInput {
                        remaining: &input,
                        max_read,
                        calls: 0,
                        end_error: std::io::ErrorKind::WouldBlock,
                    };
                    let size = if marker {
                        u64::MAX
                    } else {
                        payload.len() as u64
                    };
                    let mut reader =
                        LzmaReader::new_with_props(source, size, 93, options.dict_size, None)
                            .unwrap();
                    let mut output = Vec::new();
                    reader.read_to_end(&mut output).unwrap();
                    assert_eq!(output, payload);
                    let (source, mut unused) = reader.into_parts();
                    unused.extend_from_slice(source.remaining);
                    assert_eq!(unused, trailer, "marker={marker}, chunk={max_read}");
                }
            }
        }
    }
}

#[test]
fn buffered_lzma_reports_source_errors() {
    let payload = b"Input failures must reach the caller.";
    for marker in [false, true] {
        let (compressed, options) = raw_stream(payload, marker);
        let size = if marker {
            u64::MAX
        } else {
            payload.len() as u64
        };
        for cut in 5..compressed.len() {
            for kind in [
                std::io::ErrorKind::PermissionDenied,
                std::io::ErrorKind::UnexpectedEof,
            ] {
                let source = LimitedInput {
                    remaining: &compressed[..cut],
                    max_read: 7,
                    calls: 0,
                    end_error: kind,
                };
                let mut reader =
                    LzmaReader::new_with_props(source, size, 93, options.dict_size, None).unwrap();
                let error = reader.read_to_end(&mut Vec::new()).unwrap_err();
                assert_eq!(error.kind(), kind, "cut={cut}");
                assert_eq!(error.to_string(), "source read failed");
                assert_eq!(reader.read(&mut [0]).unwrap_err().kind(), kind);
            }
            let mut reader =
                LzmaReader::new_with_props(&compressed[..cut], size, 93, options.dict_size, None)
                    .unwrap();
            assert_eq!(
                reader.read_to_end(&mut Vec::new()).unwrap_err().kind(),
                std::io::ErrorKind::UnexpectedEof
            );
        }
    }
}

#[test]
fn buffered_lzma_crosses_input_and_output_boundaries() {
    let mut state = 0x12345678u32;
    let payload: Vec<_> = (0..150000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    for marker in [false, true] {
        let (compressed, options) = raw_stream(&payload, marker);
        assert!(compressed.len() > 2 * 65536);
        let mut with_trailer = compressed.clone();
        let trailer = vec![0xC3; 70000];
        with_trailer.extend_from_slice(&trailer);
        for max_read in [31, 4093, 65536] {
            for output_size in [1, 257, 131072] {
                let source = LimitedInput {
                    remaining: &with_trailer,
                    max_read,
                    calls: 0,
                    end_error: std::io::ErrorKind::WouldBlock,
                };
                let size = if marker {
                    u64::MAX
                } else {
                    payload.len() as u64
                };
                let mut reader =
                    LzmaReader::new_with_props(source, size, 93, options.dict_size, None).unwrap();
                let mut buffer = vec![0; output_size];
                let mut output = Vec::new();
                loop {
                    let count = reader.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    output.extend_from_slice(&buffer[..count]);
                }
                assert_eq!(output, payload);
                let (source, mut unused) = reader.into_parts();
                unused.extend_from_slice(source.remaining);
                assert_eq!(unused, trailer);
                assert_eq!(source.calls, expected_calls(compressed.len(), max_read));
            }
        }
    }
}

#[test]
fn buffered_lzma_retries_interrupted_reads() {
    struct Interrupting<'a> {
        input: &'a [u8],
        interrupt: bool,
    }
    impl Read for Interrupting<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let count = output.len().min(2);
            self.input.read(&mut output[..count])
        }
    }
    let payload = b"Retry interrupted reads during initialization and decoding.";
    let (compressed, options) = raw_stream(payload, true);
    let source = Interrupting {
        input: &compressed,
        interrupt: false,
    };
    let mut reader =
        LzmaReader::new_with_props(source, u64::MAX, 93, options.dict_size, None).unwrap();
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert_eq!(output, payload);
}

#[derive(Debug)]
struct SourcePayload(u32);

impl std::fmt::Display for SourcePayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "source payload {}", self.0)
    }
}

impl std::error::Error for SourcePayload {}

struct FailingInput<'a> {
    remaining: &'a [u8],
    error: fn() -> std::io::Error,
}

impl Read for FailingInput<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining.is_empty() {
            return Err((self.error)());
        }
        let count = output.len().min(4).min(self.remaining.len());
        self.remaining.read(&mut output[..count])
    }
}

fn truncated_stream() -> (Vec<u8>, LzmaOptions) {
    let (mut compressed, options) =
        raw_stream(b"a payload that needs more than one source read", true);
    compressed.truncate(compressed.len() - 3);
    (compressed, options)
}

#[test]
fn buffered_lzma_keeps_the_operating_system_error_code() {
    let (compressed, options) = truncated_stream();
    let source = FailingInput {
        remaining: &compressed,
        error: || std::io::Error::from_raw_os_error(5),
    };
    let mut reader =
        LzmaReader::new_with_props(source, u64::MAX, 93, options.dict_size, None).unwrap();
    let first = reader.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(first.raw_os_error(), Some(5));
    // Later reads repeat the error. `std::io::Error` cannot be cloned, so they
    // carry the kind and the message rather than the original code.
    let second = reader.read(&mut [0]).unwrap_err();
    assert_eq!(second.kind(), first.kind());
    assert_eq!(second.to_string(), first.to_string());
}

#[test]
fn buffered_lzma_keeps_the_source_error_message() {
    let (compressed, options) = truncated_stream();
    let source = FailingInput {
        remaining: &compressed,
        error: || std::io::Error::other(SourcePayload(1234)),
    };
    let mut reader =
        LzmaReader::new_with_props(source, u64::MAX, 93, options.dict_size, None).unwrap();
    let first = reader.read_to_end(&mut Vec::new()).unwrap_err();
    // The reader holds a copy of the error so that it stays unwind safe, which a
    // boxed payload would cost it. The kind and the message carry over.
    assert_eq!(first.kind(), std::io::ErrorKind::Other);
    assert_eq!(first.to_string(), "source payload 1234");
    for _ in 0..3 {
        let again = reader.read(&mut [0]).unwrap_err();
        assert_eq!(again.kind(), first.kind());
        assert_eq!(again.to_string(), first.to_string());
    }
}

struct WidestRead<'a> {
    remaining: &'a [u8],
    first: usize,
    widest: usize,
}

impl Read for WidestRead<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if self.first == 0 {
            self.first = output.len();
        }
        self.widest = self.widest.max(output.len());
        self.remaining.read(output)
    }
}

#[test]
fn buffered_lzma_grows_its_input_buffer() {
    // A short stream must not pull a full sized buffer in, so that a container
    // of many small members does not copy far more than it decodes.
    let (compressed, options) = raw_stream(b"short", true);
    let source = WidestRead {
        remaining: &compressed,
        first: 0,
        widest: 0,
    };
    let mut reader =
        LzmaReader::new_with_props(source, u64::MAX, 93, options.dict_size, None).unwrap();
    reader.read_to_end(&mut Vec::new()).unwrap();
    let (source, _) = reader.into_parts();
    assert_eq!(source.first, 4 * 1024);
    assert_eq!(source.widest, 4 * 1024);

    // A long stream grows up to the full buffer size.
    let mut state = 0x9E3779B9u32;
    let payload: Vec<_> = (0..4 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    let (compressed, options) = raw_stream(&payload, true);
    assert!(compressed.len() > 64 * 1024);
    let source = WidestRead {
        remaining: &compressed,
        first: 0,
        widest: 0,
    };
    let mut reader =
        LzmaReader::new_with_props(source, u64::MAX, 93, options.dict_size, None).unwrap();
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert_eq!(output, payload);
    let (source, _) = reader.into_parts();
    assert_eq!(source.first, 4 * 1024);
    assert_eq!(source.widest, 64 * 1024);
}
