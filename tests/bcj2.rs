#![cfg(feature = "std")]

use std::io::{self, Read};

use lzma_rust2::filter::bcj2::Bcj2Reader;

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
