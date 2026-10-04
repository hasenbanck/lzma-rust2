#![cfg(feature = "std")]

#[cfg(feature = "encoder")]
use std::io::Write;
use std::io::{self, Read};

use lzma_rust2::filter::bcj2::Bcj2Reader;
#[cfg(feature = "encoder")]
use lzma_rust2::filter::bcj2::{Bcj2Options, Bcj2Writer};

#[test]
fn large_declared_size_does_not_truncate_to_usize() {
    let mut reader = Bcj2Reader::new(vec![&[0x90][..], &[], &[], &[0; 5]], 1 << 32);
    let mut buf = [0];
    assert_eq!(reader.read(&mut buf).unwrap(), 1);
    assert_eq!(buf, [0x90]);
}

#[test]
fn invalid_stream_counts_do_not_panic() {
    for count in [0, 1, 2, 3, 5] {
        let mut reader = Bcj2Reader::new(vec![&[][..]; count], 1);
        assert_eq!(
            reader.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

#[test]
fn checked_construction_rejects_invalid_stream_counts() {
    for count in [0, 1, 2, 3, 5] {
        let error = Bcj2Reader::<&[u8]>::try_new(vec![&[][..]; count], 0)
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
    assert!(Bcj2Reader::<&[u8]>::try_new(vec![&[][..]; 4], 0).is_ok());
}

#[test]
fn input_errors_keep_their_operating_system_code_after_progress() {
    struct Source<'a>(&'a [u8]);
    impl Read for Source<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return Err(io::Error::from_raw_os_error(13));
            }
            self.0.read(buf)
        }
    }
    let inputs = vec![Source(&[0x90]), Source(&[]), Source(&[]), Source(&[0; 5])];
    let mut reader = Bcj2Reader::new(inputs, 2);
    let mut output = [0; 8];
    assert_eq!(reader.read(&mut output).unwrap(), 1);
    assert_eq!(output[0], 0x90);
    for _ in 0..2 {
        assert_eq!(
            reader.read(&mut output).unwrap_err().raw_os_error(),
            Some(13)
        );
    }
    assert_eq!(reader.finish().err().unwrap().raw_os_error(), Some(13));
}

fn decode(inputs: [&[u8]; 4], size: u64) -> io::Result<Vec<u8>> {
    let mut reader = Bcj2Reader::new(inputs.to_vec(), size);
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}

#[test]
fn empty_output_still_requires_a_valid_range_stream() {
    assert!(decode([&[], &[], &[], &[]], 0).is_err());
    assert!(decode([&[], &[], &[], &[0; 4]], 0).is_err());
    assert!(decode([&[], &[], &[], &[1, 0, 0, 0, 0]], 0).is_err());
    assert!(decode([&[], &[], &[], &[0; 5]], 0).unwrap().is_empty());
}

#[test]
fn truncated_inputs_are_rejected() {
    let converted = [0, 0x7F, 0xFF, 0xFC, 0];
    for (inputs, size) in [
        ([&[0x90][..], &[][..], &[][..], &[0; 5][..]], 2),
        ([&[0x90][..], &[][..], &[][..], &[0; 4][..]], 1),
        ([&[0xE8][..], &[][..], &[][..], &converted[..]], 5),
        ([&[0xE9][..], &[][..], &[][..], &converted[..]], 5),
    ] {
        assert_eq!(
            decode(inputs, size).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}

#[test]
fn output_before_an_error_is_returned_and_the_error_persists() {
    let inputs = vec![&[0x90][..], &[][..], &[][..], &[0; 5][..]];
    let mut reader = Bcj2Reader::new(inputs, 2);
    let mut buf = [0; 16];
    assert_eq!(reader.read(&mut buf).unwrap(), 1);
    assert_eq!(buf[0], 0x90);
    for _ in 0..3 {
        assert_eq!(
            reader.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}

#[test]
fn valid_call_and_jump_use_their_own_streams() {
    let absolute = 9u32.to_be_bytes();
    let control = [0, 0x7F, 0xFF, 0xFC, 0];
    for opcode in [0xE8, 0xE9] {
        let (call, jump) = if opcode == 0xE8 {
            (&absolute[..], &[][..])
        } else {
            (&[][..], &absolute[..])
        };
        assert_eq!(
            decode([&[opcode], call, jump, &control], 5).unwrap(),
            [opcode, 4, 0, 0, 0]
        );
    }
}

#[test]
fn checked_construction_and_strict_finish() {
    assert!(Bcj2Reader::<&[u8]>::try_new(vec![&[][..]; 3], 0).is_err());
    for inputs in [
        [&[0x90, 0x90][..], &[][..], &[][..], &[0; 5][..]],
        [&[0x90][..], &[0][..], &[][..], &[0; 5][..]],
        [&[0x90][..], &[][..], &[0][..], &[0; 5][..]],
        [&[0x90][..], &[][..], &[][..], &[0; 6][..]],
    ] {
        let mut reader = Bcj2Reader::try_new(inputs.to_vec(), 1).unwrap();
        let mut buf = [0];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [0x90]);
        assert_eq!(
            reader.finish().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    let mut reader = Bcj2Reader::try_new(vec![&[0x90][..], &[], &[], &[0; 5]], 1).unwrap();
    reader.read_exact(&mut [0]).unwrap();
    assert_eq!(reader.finish().unwrap().len(), 4);
    assert!(
        Bcj2Reader::new(vec![&[0x90][..], &[], &[], &[0; 5]], 1)
            .finish()
            .is_err()
    );
    assert!(
        Bcj2Reader::new(vec![&[][..], &[], &[], &[0; 5]], 0)
            .finish()
            .is_ok()
    );
}

struct ReferenceVector {
    name: String,
    #[cfg(feature = "encoder")]
    uncompressed_size: Option<u64>,
    #[cfg(feature = "encoder")]
    relative_limit: u32,
    original: Vec<u8>,
    streams: [Vec<u8>; 4],
}

fn reference_vectors() -> Vec<ReferenceVector> {
    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }
    include_str!("fixtures/bcj2.txt")
        .lines()
        .chain(include_str!("fixtures/bcj2-scanning.txt").lines())
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            ReferenceVector {
                name: fields[0].to_owned(),
                #[cfg(feature = "encoder")]
                uncompressed_size: fields[1].parse().ok(),
                #[cfg(feature = "encoder")]
                relative_limit: fields[2].parse().unwrap(),
                original: bytes(fields[3]),
                streams: std::array::from_fn(|i| bytes(fields[i + 4])),
            }
        })
        .collect()
}

#[test]
fn truncated_reference_streams_and_wrong_sizes_are_rejected() {
    for vector in reference_vectors() {
        for stream in 0..4 {
            if vector.streams[stream].is_empty() {
                continue;
            }
            let mut streams = vector.streams.clone();
            streams[stream].pop();
            let mut reader = Bcj2Reader::new(
                streams.iter().map(Vec::as_slice).collect(),
                vector.original.len() as u64,
            );
            let mut output = Vec::new();
            let result = reader
                .read_to_end(&mut output)
                .and_then(|_| reader.finish().map(|_| ()));
            assert!(result.is_err(), "{} stream={stream}", vector.name);
        }
        for size in [
            vector.original.len().saturating_sub(1),
            vector.original.len() + 1,
        ] {
            if size == vector.original.len() {
                continue;
            }
            let mut reader = Bcj2Reader::new(
                vector.streams.iter().map(Vec::as_slice).collect(),
                size as u64,
            );
            let mut output = Vec::new();
            let result = reader
                .read_to_end(&mut output)
                .and_then(|_| reader.finish().map(|_| ()));
            assert!(result.is_err(), "{} size={size}", vector.name);
        }
    }
}

struct Fragmented<R> {
    inner: R,
    chunk_size: usize,
    interrupt: bool,
}

impl<R: Read> Read for Fragmented<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.interrupt = !self.interrupt;
        if self.interrupt {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let size = buf.len().min(self.chunk_size);
        self.inner.read(&mut buf[..size])
    }
}

#[test]
fn reference_streams_decode_across_buffer_boundaries() {
    for vector in reference_vectors() {
        for input_size in [1, 3, 5, 1 << 18] {
            for output_size in [1, 4, 4096] {
                let inputs = vector
                    .streams
                    .iter()
                    .map(|part| Fragmented {
                        inner: part.as_slice(),
                        chunk_size: input_size,
                        interrupt: false,
                    })
                    .collect();
                let mut reader = Bcj2Reader::new(inputs, vector.original.len() as u64);
                let mut output = Vec::new();
                let mut buf = vec![0; output_size];
                loop {
                    let size = reader.read(&mut buf).unwrap_or_else(|error| {
                        panic!(
                            "{} input={input_size} output={output_size}: {error}",
                            vector.name
                        )
                    });
                    if size == 0 {
                        break;
                    }
                    output.extend_from_slice(&buf[..size]);
                }
                assert_eq!(output, vector.original, "{}", vector.name);
                reader.finish().unwrap_or_else(|error| {
                    panic!(
                        "{} input={input_size} output={output_size}: {error}",
                        vector.name
                    )
                });
            }
        }
    }
}

#[test]
fn fragmented_and_interrupted_address_words_preserve_output() {
    let parts = [
        &[0xE8][..],
        &[0, 0, 0, 9][..],
        &[][..],
        &[0, 0x7F, 0xFF, 0xFC, 0][..],
    ];
    for input_size in [1, 2, 3, 5] {
        for output_size in [1, 2, 3, 5] {
            let inputs = parts
                .iter()
                .map(|part| Fragmented {
                    inner: *part,
                    chunk_size: input_size,
                    interrupt: false,
                })
                .collect();
            let mut reader = Bcj2Reader::new(inputs, 5);
            let mut output = Vec::new();
            let mut buf = vec![0; output_size];
            loop {
                let size = reader.read(&mut buf).unwrap();
                if size == 0 {
                    break;
                }
                output.extend_from_slice(&buf[..size]);
            }
            assert_eq!(output, [0xE8, 4, 0, 0, 0]);
            reader.finish().unwrap();
        }
    }
}

#[test]
#[cfg(feature = "encoder")]
fn encoder_matches_reference_streams() {
    for vector in reference_vectors() {
        let options = Bcj2Options {
            uncompressed_size: vector.uncompressed_size,
            relative_limit: vector.relative_limit,
        };
        for chunk_size in [1, 2, 3, 5, 17, 4096] {
            let mut writer =
                Bcj2Writer::new(std::array::from_fn(|_| Vec::new()), &options).unwrap();
            for chunk in vector.original.chunks(chunk_size) {
                writer.write_all(chunk).unwrap();
            }
            let streams = writer.finish().unwrap();
            assert_eq!(
                streams, vector.streams,
                "{} chunk={chunk_size}",
                vector.name
            );
        }
    }
}

#[test]
#[cfg(feature = "encoder")]
fn short_main_spans_and_large_literals_preserve_streams() {
    fn literals(size: usize, seed: usize) -> Vec<u8> {
        (0..size)
            .map(|i| 0x20 + ((i + seed) % 0x60) as u8)
            .collect()
    }
    let mut input = literals(16383, 0);
    let mut expected: [Vec<u8>; 4] = std::array::from_fn(|_| Vec::new());
    expected[0] = input.clone();
    for repeat in 0..192 {
        for span in 1..=65 {
            let opcode: &[u8] = match span % 3 {
                1 => &[0xE8],
                2 => &[0xE9],
                _ => &[0x0F, 0x85],
            };
            let prefix = literals(span - opcode.len(), repeat + span);
            input.extend_from_slice(&prefix);
            input.extend_from_slice(opcode);
            expected[0].extend_from_slice(&prefix);
            expected[0].extend_from_slice(opcode);
            input.extend_from_slice(&[0; 4]);
            let stream = if opcode == [0xE8] { 1 } else { 2 };
            expected[stream].extend_from_slice(&(input.len() as u32).to_be_bytes());
        }
        if repeat % 16 == 0 {
            let block = literals(16387, repeat);
            input.extend_from_slice(&block);
            expected[0].extend_from_slice(&block);
        }
    }
    input.push(0x90);
    expected[0].push(0x90);
    let rc = include_str!("fixtures/bcj2-short-main-rc.txt")
        .lines()
        .find(|line| !line.starts_with('#'))
        .unwrap();
    expected[3] = (0..rc.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&rc[i..i + 2], 16).unwrap())
        .collect();
    for uncompressed_size in [None, Some(input.len() as u64)] {
        let options = Bcj2Options {
            uncompressed_size,
            ..Bcj2Options::default()
        };
        for chunk_size in [1, 7, 17, 16383, 16384, 16385, input.len()] {
            let mut writer =
                Bcj2Writer::new(std::array::from_fn(|_| Vec::new()), &options).unwrap();
            for chunk in input.chunks(chunk_size) {
                writer.write_all(chunk).unwrap();
            }
            assert_eq!(writer.finish().unwrap(), expected, "chunk={chunk_size}");
        }
    }
    assert_eq!(
        decode(expected.each_ref().map(Vec::as_slice), input.len() as u64).unwrap(),
        input
    );
}

#[test]
#[cfg(feature = "encoder")]
fn partial_buffer_flush_and_large_write_failures_are_sticky() {
    let short_spans = (3..=32).map(|size| {
        let mut input = vec![0x91; size - 1];
        input.extend_from_slice(&[0xE8, 0, 0, 0, 0]);
        (16384 - size + 1, input, 5)
    });
    let large_span = std::iter::once((17, vec![0x91; 16385], 22));
    for (prefix_size, input, fail_after) in short_spans.chain(large_span) {
        for finish in [false, true] {
            let mut sinks = test_sinks();
            sinks[0].fail_after = Some(fail_after);
            let mut writer = Bcj2Writer::new(sinks, &Bcj2Options::default()).unwrap();
            writer.write_all(&vec![0x90; prefix_size]).unwrap();
            assert_eq!(
                writer.write_all(&input).unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
            assert_eq!(writer.get_uncompressed_size(), prefix_size as u64);
            assert_eq!(
                writer.write_all(&[0x90]).unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
            assert_eq!(
                writer.flush().unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
            if finish {
                assert_eq!(
                    writer.finish().err().unwrap().kind(),
                    io::ErrorKind::BrokenPipe
                );
            } else {
                let output = writer.into_inner()[0].data.clone();
                assert_eq!(output.len(), fail_after);
                assert!(
                    output[..prefix_size.min(fail_after)]
                        .iter()
                        .all(|&b| b == 0x90)
                );
                assert!(
                    output[prefix_size.min(fail_after)..]
                        .iter()
                        .all(|&b| b == 0x91)
                );
            }
        }
    }
}

#[test]
#[cfg(feature = "encoder")]
fn single_branches_after_literal_prefixes() {
    let opcodes: Vec<_> = [vec![0xE8], vec![0xE9]]
        .into_iter()
        .chain((0x80..=0x8F).map(|byte| vec![0x0F, byte]))
        .collect();
    for prefix in 0..=129 {
        for opcode in &opcodes {
            let mut input = vec![0x90; prefix];
            input.extend_from_slice(opcode);
            input.extend_from_slice(&4u32.to_le_bytes());
            input.extend_from_slice(&[0x90; 64]);
            let mut expected: [Vec<u8>; 4] = std::array::from_fn(|_| Vec::new());
            expected[0].extend_from_slice(&input[..prefix + opcode.len()]);
            expected[0].extend_from_slice(&[0x90; 64]);
            let stream = if opcode[0] == 0xE8 { 1 } else { 2 };
            expected[stream].extend_from_slice(&((prefix + opcode.len() + 8) as u32).to_be_bytes());
            expected[3].extend_from_slice(&[0, 0x7F, 0xFF, 0xFC, 0]);
            for chunk in [
                1, 15, 16, 17, 31, 32, 33, 63, 64, 65, 96, 97, 98, 127, 128, 129, 4096,
            ] {
                let mut writer =
                    Bcj2Writer::new(std::array::from_fn(|_| Vec::new()), &Default::default())
                        .unwrap();
                for part in input.chunks(chunk) {
                    writer.write_all(part).unwrap();
                }
                assert_eq!(
                    writer.finish().unwrap(),
                    expected,
                    "prefix={prefix} chunk={chunk}"
                );
            }
            for output_size in [15, 16, 17, 31, 32, 33, 64, 128] {
                let mut reader = Bcj2Reader::new(
                    expected.iter().map(Vec::as_slice).collect(),
                    input.len() as u64,
                );
                let mut output = Vec::new();
                let mut buf = vec![0; output_size];
                loop {
                    let size = reader.read(&mut buf).unwrap_or_else(|error| {
                        panic!("prefix={prefix} opcode={opcode:02x?} output_size={output_size}: {error}")
                    });
                    if size == 0 {
                        break;
                    }
                    output.extend_from_slice(&buf[..size]);
                }
                reader.finish().unwrap();
                assert_eq!(output, input);
            }
        }
    }
}

#[test]
#[cfg(feature = "encoder")]
fn flush_does_not_finish_a_held_instruction() {
    for vector in reference_vectors() {
        let options = Bcj2Options {
            uncompressed_size: vector.uncompressed_size,
            relative_limit: vector.relative_limit,
        };
        let mut streams = std::array::from_fn(|_| Vec::new());
        let mut writer = Bcj2Writer::new(streams.each_mut(), &options).unwrap();
        writer.write_all(&[]).unwrap();
        for byte in &vector.original {
            writer.write_all(&[*byte]).unwrap();
            writer.flush().unwrap();
        }
        assert_eq!(writer.get_uncompressed_size(), vector.original.len() as u64);
        writer.finish().unwrap();
        assert_eq!(streams, vector.streams, "{}", vector.name);
    }
}

#[test]
#[cfg(feature = "encoder")]
fn options_and_declared_sizes_are_checked() {
    let outputs = || std::array::from_fn(|_| Vec::new());
    let mut options = Bcj2Options {
        relative_limit: Bcj2Options::RELATIVE_LIMIT_MAX + 1,
        ..Default::default()
    };
    assert_eq!(
        Bcj2Writer::new(outputs(), &options).err().unwrap().kind(),
        io::ErrorKind::InvalidInput
    );
    options.relative_limit = Bcj2Options::RELATIVE_LIMIT_DEFAULT;
    options.uncompressed_size = Some(1);
    let mut writer = Bcj2Writer::new(outputs(), &options).unwrap();
    assert_eq!(
        writer.write(&[0x90; 2]).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(writer.get_uncompressed_size(), 0);
    writer.write_all(&[0x90]).unwrap();
    let streams = writer.finish().unwrap();
    assert_eq!(streams[0], [0x90]);
    let writer = Bcj2Writer::new(outputs(), &options).unwrap();
    assert_eq!(
        writer.finish().unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[cfg(feature = "encoder")]
struct TestSink {
    data: Vec<u8>,
    chunk_size: usize,
    interrupt: bool,
    fail_after: Option<usize>,
    fail_flush: bool,
}

#[cfg(feature = "encoder")]
impl Write for TestSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.interrupt = !self.interrupt;
        if self.interrupt {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let available = match self.fail_after {
            Some(limit) => limit
                .checked_sub(self.data.len())
                .filter(|&n| n != 0)
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "BCJ2 sink failed"))?,
            None => usize::MAX,
        };
        let size = buf.len().min(self.chunk_size).min(available);
        self.data.extend_from_slice(&buf[..size]);
        Ok(size)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.fail_flush {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

#[cfg(feature = "encoder")]
fn test_sinks() -> [TestSink; 4] {
    std::array::from_fn(|_| TestSink {
        data: Vec::new(),
        chunk_size: 3,
        interrupt: false,
        fail_after: None,
        fail_flush: false,
    })
}

#[test]
#[cfg(feature = "encoder")]
fn short_and_interrupted_writes_preserve_reference_output() {
    for vector in reference_vectors() {
        let options = Bcj2Options {
            uncompressed_size: vector.uncompressed_size,
            relative_limit: vector.relative_limit,
        };
        let mut writer = Bcj2Writer::new(test_sinks(), &options).unwrap();
        writer.write_all(&vector.original).unwrap();
        let output = writer.finish().unwrap().map(|sink| sink.data);
        assert_eq!(output, vector.streams, "{}", vector.name);
    }
}

#[test]
#[cfg(feature = "encoder")]
fn failures_on_each_output_are_sticky() {
    let data = (0..8192)
        .flat_map(|_| [0x90, 0xE8, 0, 0, 0, 0, 0xE9, 0, 0, 0, 0])
        .collect::<Vec<_>>();
    for stream in 0..4 {
        let mut sinks = test_sinks();
        sinks[stream].fail_after = Some(1);
        let mut writer = Bcj2Writer::new(sinks, &Bcj2Options::default()).unwrap();
        // The short RC stream can remain buffered until flush.
        let result = writer.write_all(&data).and_then(|_| writer.flush());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        for _ in 0..2 {
            assert_eq!(
                writer.write(&[0x90]).unwrap_err().to_string(),
                error.to_string()
            );
            assert_eq!(writer.flush().unwrap_err().kind(), error.kind());
        }
        assert_eq!(writer.finish().err().unwrap().kind(), error.kind());
    }
}

#[test]
#[cfg(feature = "encoder")]
fn write_zero_and_finish_errors_are_reported() {
    for stream in 0..4 {
        let mut sinks = test_sinks();
        sinks[stream].chunk_size = 0;
        let mut writer = Bcj2Writer::new(sinks, &Bcj2Options::default()).unwrap();
        writer
            .write_all(&[0x90, 0xE8, 0, 0, 0, 0, 0xE9, 0, 0, 0, 0])
            .unwrap();
        assert_eq!(
            writer.finish().err().unwrap().kind(),
            io::ErrorKind::WriteZero
        );
        let mut sinks = test_sinks();
        sinks[stream].fail_flush = true;
        let writer = Bcj2Writer::new(sinks, &Bcj2Options::default()).unwrap();
        assert_eq!(
            writer.finish().err().unwrap().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}

#[test]
#[cfg(feature = "encoder")]
fn randomized_chunks_round_trip_and_do_not_change_streams() {
    let mut state = 17u32;
    for size in [0, 1, 4, 5, 6, 31, 4096, 16385, 262149] {
        let mut data = Vec::with_capacity(size);
        for _ in 0..size {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            data.push((state >> 24) as u8);
        }
        for options in [
            Bcj2Options::default(),
            Bcj2Options {
                uncompressed_size: Some(size as u64),
                relative_limit: Bcj2Options::RELATIVE_LIMIT_MAX,
            },
        ] {
            let mut writer =
                Bcj2Writer::new(std::array::from_fn(|_| Vec::new()), &options).unwrap();
            writer.write_all(&data).unwrap();
            let expected = writer.finish().unwrap();
            let mut writer =
                Bcj2Writer::new(std::array::from_fn(|_| Vec::new()), &options).unwrap();
            let mut offset = 0;
            while offset < data.len() {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                let end = (offset + 1 + (state as usize % 257)).min(data.len());
                writer.write_all(&data[offset..end]).unwrap();
                if state & 7 == 0 {
                    writer.flush().unwrap();
                }
                offset = end;
            }
            let streams = writer.finish().unwrap();
            assert_eq!(streams, expected);
            let inputs = streams
                .iter()
                .map(|stream| Fragmented {
                    inner: stream.as_slice(),
                    chunk_size: 7,
                    interrupt: false,
                })
                .collect();
            let mut reader = Bcj2Reader::new(inputs, size as u64);
            let mut decoded = Vec::new();
            reader.read_to_end(&mut decoded).unwrap();
            assert_eq!(decoded, data);
            reader.finish().unwrap();
        }
    }
}
