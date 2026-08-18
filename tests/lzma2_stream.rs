use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    io::{Read, Write},
};

use lzma_rust2::{Action, Lzma2Options, Lzma2Stream, Lzma2Writer, Status};

static APACHE2: &str = "tests/data/apache2.txt";
static EXECUTABLE: &str = "tests/data/executable.exe";
static PG100: &str = "tests/data/pg100.txt";
static PG6800: &str = "tests/data/pg6800.txt";
static INPUT_HTML: &str = "tests/data/input.html";

fn test_round_trip(path: &str, preset: u32) {
    let data = std::fs::read(path).unwrap();

    let opts = Lzma2Options::with_preset(preset);
    let dict_size = opts.lzma_options.dict_size;
    let mut writer = Lzma2Writer::new(Vec::new(), opts);
    writer.write_all(&data).unwrap();
    let compressed = writer.finish().unwrap();

    let mut decoder = Lzma2Stream::new(dict_size);
    let mut decompressed = Vec::new();
    let mut output_buf = [0u8; 4096];
    let mut in_pos = 0;

    loop {
        let action = if in_pos >= compressed.len() {
            Action::Finish
        } else {
            Action::Run
        };
        let result = decoder
            .process(&compressed[in_pos..], &mut output_buf, action)
            .unwrap();
        in_pos += result.bytes_consumed;
        decompressed.extend_from_slice(&output_buf[..result.bytes_produced]);
        if result.status == Status::StreamEnd {
            break;
        }
    }
    assert!(decompressed == data);
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

#[test]
fn cross_compat_with_reader() {
    let data = std::fs::read(INPUT_HTML).unwrap();

    let opts = Lzma2Options::with_preset(3);
    let dict_size = opts.lzma_options.dict_size;
    let mut writer = Lzma2Writer::new(Vec::new(), opts);
    writer.write_all(&data).unwrap();
    let compressed = writer.finish().unwrap();

    let mut decoder = Lzma2Stream::new(dict_size);
    let mut decompressed_stream = Vec::new();
    let mut output_buf = [0u8; 4096];
    let mut in_pos = 0;
    loop {
        let action = if in_pos >= compressed.len() {
            Action::Finish
        } else {
            Action::Run
        };
        let result = decoder
            .process(&compressed[in_pos..], &mut output_buf, action)
            .unwrap();
        in_pos += result.bytes_consumed;
        decompressed_stream.extend_from_slice(&output_buf[..result.bytes_produced]);
        if result.status == Status::StreamEnd {
            break;
        }
    }

    let mut reader = lzma_rust2::Lzma2Reader::new(compressed.as_slice(), dict_size, None);
    let mut decompressed_reader = Vec::new();
    reader.read_to_end(&mut decompressed_reader).unwrap();

    assert_eq!(decompressed_stream, decompressed_reader);
    assert!(decompressed_stream == data);
}

#[test]
fn zero_length_output_buffer() {
    let data = std::fs::read(INPUT_HTML).unwrap();

    let opts = Lzma2Options::with_preset(1);
    let dict_size = opts.lzma_options.dict_size;
    let mut writer = Lzma2Writer::new(Vec::new(), opts);
    writer.write_all(&data).unwrap();
    let compressed = writer.finish().unwrap();

    let mut decoder = Lzma2Stream::new(dict_size);
    let mut output_buf = [0u8; 0];
    let result = decoder
        .process(&compressed, &mut output_buf, Action::Run)
        .unwrap();
    assert_eq!(result.bytes_produced, 0);
}

#[test]
fn error_on_corrupted_payload() {
    let data = std::fs::read(INPUT_HTML).unwrap();

    let opts = Lzma2Options::with_preset(1);
    let dict_size = opts.lzma_options.dict_size;
    let mut writer = Lzma2Writer::new(Vec::new(), opts);
    writer.write_all(&data).unwrap();
    let mut compressed = writer.finish().unwrap();

    let mid = compressed.len() / 2;
    compressed[mid] ^= 0xFF;

    let mut decoder = Lzma2Stream::new(dict_size);
    let mut output_buf = [0u8; 4096];
    let mut in_pos = 0;

    let mut errored = false;
    for _ in 0..1000 {
        let action = if in_pos >= compressed.len() {
            Action::Finish
        } else {
            Action::Run
        };
        match decoder.process(&compressed[in_pos..], &mut output_buf, action) {
            Ok(result) => {
                in_pos += result.bytes_consumed;
                if result.status == Status::StreamEnd {
                    break;
                }
            }
            Err(_) => {
                errored = true;
                break;
            }
        }
    }
    assert!(errored, "Expected error on corrupted payload");
}

#[test]
fn process_after_stream_end() {
    let data = std::fs::read(INPUT_HTML).unwrap();

    let opts = Lzma2Options::with_preset(1);
    let dict_size = opts.lzma_options.dict_size;
    let mut writer = Lzma2Writer::new(Vec::new(), opts);
    writer.write_all(&data).unwrap();
    let compressed = writer.finish().unwrap();

    let mut decoder = Lzma2Stream::new(dict_size);
    let mut output_buf = [0u8; 4096];
    let mut in_pos = 0;

    loop {
        let action = if in_pos >= compressed.len() {
            Action::Finish
        } else {
            Action::Run
        };
        let result = decoder
            .process(&compressed[in_pos..], &mut output_buf, action)
            .unwrap();
        in_pos += result.bytes_consumed;
        if result.status == Status::StreamEnd {
            break;
        }
    }

    let result = decoder.process(&[], &mut output_buf, Action::Finish);
    match result {
        Ok(r) => {
            assert_eq!(r.bytes_consumed, 0);
            assert_eq!(r.bytes_produced, 0);
        }
        Err(_) => {}
    }
}

#[test]
fn incomplete_chunk_produces_error() {
    let data = std::fs::read(INPUT_HTML).unwrap();

    let opts = Lzma2Options::with_preset(6);
    let dict_size = opts.lzma_options.dict_size;
    let mut writer = Lzma2Writer::new(Vec::new(), opts);
    writer.write_all(&data).unwrap();
    let compressed = writer.finish().unwrap();

    let truncated = &compressed[..compressed.len() / 2];

    let mut decoder = Lzma2Stream::new(dict_size);
    let mut output_buf = [0u8; 4096];

    let mut in_pos = 0;
    let mut errored = false;
    for _ in 0..100 {
        let action = if in_pos >= truncated.len() {
            Action::Finish
        } else {
            Action::Run
        };
        match decoder.process(&truncated[in_pos..], &mut output_buf, action) {
            Ok(result) => {
                in_pos += result.bytes_consumed;
                if result.status == Status::StreamEnd {
                    break;
                }
            }
            Err(_) => {
                errored = true;
                break;
            }
        }
    }
    assert!(errored, "Expected error on truncated LZMA2 stream");
}

/// Regression test for issue 1: uncompressed LZMA2 chunks that exceed
/// the LZ dictionary buffer capacity must not silently drop bytes.
/// Uses a tiny dict size (4096) to force the LZ buffer to fill mid-chunk.
#[test]
fn uncompressed_chunk_exceeds_dict() {
    let data_len: usize = 8192;
    let original: Vec<u8> = (0..data_len).map(|i| (i % 251) as u8).collect();

    let mut lzma2_stream: Vec<u8> = Vec::new();
    let mut offset = 0;
    while offset < data_len {
        let chunk = (data_len - offset).min(65535);
        let control = if offset == 0 { 0x01u8 } else { 0x02u8 };
        lzma2_stream.push(control);
        let size_minus_one = (chunk - 1) as u16;
        lzma2_stream.extend_from_slice(&size_minus_one.to_be_bytes());
        lzma2_stream.extend_from_slice(&original[offset..offset + chunk]);
        offset += chunk;
    }
    lzma2_stream.push(0x00);

    let dict_size: u32 = 4096;
    let mut decoder = Lzma2Stream::new(dict_size);
    let mut decompressed = Vec::new();
    let mut output_buf = [0u8; 4096];
    let mut in_pos = 0;

    loop {
        let action = if in_pos >= lzma2_stream.len() {
            Action::Finish
        } else {
            Action::Run
        };
        let result = decoder
            .process(&lzma2_stream[in_pos..], &mut output_buf, action)
            .unwrap();
        in_pos += result.bytes_consumed;
        decompressed.extend_from_slice(&output_buf[..result.bytes_produced]);
        if result.status == Status::StreamEnd {
            break;
        }
    }
    assert_eq!(decompressed.len(), original.len());
    assert_eq!(decompressed, original);
}

/// Chunk size meaning "hand over everything that is left".
const ENTIRE: usize = usize::MAX;

/// The 19/20/21 rows straddle the 20 bytes one LZMA symbol can need, the
/// 39/40/41 rows straddle the carry capacity, and 4 and 6 split the five byte
/// range coder init that starts every compressed chunk.
const CHUNK_SIZES: &[usize] = &[1, 2, 3, 4, 5, 6, 19, 20, 21, 39, 40, 41, 4096, ENTIRE];
const OUTPUT_SIZES: &[usize] = &[1, 7, 4096];

fn compress(data: &[u8], preset: u32) -> (Vec<u8>, u32) {
    let opts = Lzma2Options::with_preset(preset);
    let dict_size = opts.lzma_options.dict_size;
    let mut writer = Lzma2Writer::new(Vec::new(), opts);
    writer.write_all(data).unwrap();
    (writer.finish().unwrap(), dict_size)
}

/// Walks the chunk headers, returning the offset just past every chunk, the
/// end of stream marker included.
fn chunk_ends(compressed: &[u8]) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut pos = 0;
    loop {
        let control = compressed[pos];
        if control == 0x00 {
            ends.push(pos + 1);
            return ends;
        }
        if control >= 0x80 {
            let header = if control >= 0xC0 { 6 } else { 5 };
            let compressed_size =
                u16::from_be_bytes([compressed[pos + 3], compressed[pos + 4]]) as usize + 1;
            pos += header + compressed_size;
        } else {
            let size = u16::from_be_bytes([compressed[pos + 1], compressed[pos + 2]]) as usize + 1;
            pos += 3 + size;
        }
        ends.push(pos);
    }
}

/// Drives a stream to completion, feeding at most `chunk` bytes and accepting at
/// most `out_size` bytes per call.
fn decode(
    mut stream: Lzma2Stream,
    compressed: &[u8],
    chunk: usize,
    out_size: usize,
) -> std::io::Result<Vec<u8>> {
    let mut decompressed = Vec::new();
    let mut output = vec![0u8; out_size];
    let mut pos = 0usize;

    loop {
        let end = pos.saturating_add(chunk).min(compressed.len());
        let action = if end >= compressed.len() {
            Action::Finish
        } else {
            Action::Run
        };

        let result = stream.process(&compressed[pos..end], &mut output, action)?;
        pos += result.bytes_consumed;
        decompressed.extend_from_slice(&output[..result.bytes_produced]);

        if result.status == Status::StreamEnd {
            assert!(stream.is_finished());
            assert_eq!(stream.total_out(), decompressed.len() as u64);
            return Ok(decompressed);
        }

        // Neither consuming nor producing anything while output space is
        // available is a stall: the caller would loop forever.
        assert!(
            result.bytes_consumed != 0 || result.bytes_produced != 0,
            "stalled at input {pos}/{} after {} bytes of output",
            compressed.len(),
            decompressed.len()
        );
    }
}

fn test_chunk_matrix(data: &[u8], preset: u32) {
    let (compressed, dict_size) = compress(data, preset);
    for &chunk in CHUNK_SIZES {
        for &out_size in OUTPUT_SIZES {
            let decompressed = decode(Lzma2Stream::new(dict_size), &compressed, chunk, out_size)
                .unwrap_or_else(|error| {
                    panic!("preset {preset} chunk {chunk} out {out_size}: {error}")
                });
            assert!(
                decompressed == data,
                "preset {preset} chunk {chunk} out {out_size}"
            );
        }
    }
}

#[test]
fn chunk_matrix_apache2() {
    let data = std::fs::read(APACHE2).unwrap();
    for preset in 0..=9 {
        test_chunk_matrix(&data, preset);
    }
}

#[test]
fn chunk_matrix_input_html() {
    let data = std::fs::read(INPUT_HTML).unwrap();
    for preset in 0..=9 {
        test_chunk_matrix(&data, preset);
    }
}

/// The chunk size matrix above fits in a single LZMA2 chunk, so it never splits
/// a call across a chunk boundary. This one does: the input is big enough for
/// several chunks, each of which restarts the range coder.
#[test]
fn chunk_matrix_across_chunk_boundaries() {
    let data = std::fs::read(PG100).unwrap()[..300 * 1024].to_vec();
    let (compressed, dict_size) = compress(&data, 1);
    assert!(
        chunk_ends(&compressed).len() > 2,
        "expected more than one compressed chunk"
    );

    for &chunk in &[1usize, 5, 19, 20, 21, 39, 40, 41, 4096, ENTIRE] {
        for &out_size in &[7usize, 4096] {
            let decompressed = decode(Lzma2Stream::new(dict_size), &compressed, chunk, out_size)
                .unwrap_or_else(|error| panic!("chunk {chunk} out {out_size}: {error}"));
            assert!(decompressed == data, "chunk {chunk} out {out_size}");
        }
    }
}

/// The point of decoding chunks as they arrive: a chunk has to produce output
/// before its last compressed byte has been handed over. Buffering the whole
/// chunk first fails this.
#[test]
fn output_arrives_before_the_chunk_ends() {
    let data = std::fs::read(PG100).unwrap()[..1024 * 1024].to_vec();
    let (compressed, dict_size) = compress(&data, 6);
    let first_chunk_end = chunk_ends(&compressed)[0];
    assert!(
        first_chunk_end > 60 * 1024,
        "expected a chunk near the 64 KiB compressed maximum, got {first_chunk_end}"
    );

    let mut stream = Lzma2Stream::new(dict_size);
    let mut output = vec![0u8; 4096];
    let mut produced = 0;
    let mut in_pos = 0;

    // Everything but the last byte of the first chunk, one byte at a time.
    while in_pos < first_chunk_end - 1 {
        let end = in_pos + 1;
        let result = stream
            .process(&compressed[in_pos..end], &mut output, Action::Run)
            .unwrap();
        in_pos += result.bytes_consumed;
        produced += result.bytes_produced;
        assert!(
            result.bytes_consumed != 0 || result.bytes_produced != 0,
            "stalled at {in_pos}"
        );
    }

    assert!(
        produced > 0,
        "no output before the last byte of the first chunk"
    );
}

/// Truncation has to come back as an error rather than a stall or a silent
/// short read, wherever the cut falls.
#[test]
fn truncation_is_rejected() {
    let data = std::fs::read(PG100).unwrap()[..300 * 1024].to_vec();
    let (compressed, dict_size) = compress(&data, 1);
    let ends = chunk_ends(&compressed);
    let first = ends[0];

    let mut cuts = vec![1usize, 2, 3, 5, 6, 7, 8, 10, 11, 100, 1000];
    // Inside the second chunk, including its header and range coder init.
    cuts.extend([first + 1, first + 3, first + 6, first + 8, first + 30]);
    cuts.push(first);

    for cut in cuts {
        assert!(cut < compressed.len());
        let truncated = &compressed[..cut];
        assert!(
            decode(Lzma2Stream::new(dict_size), truncated, ENTIRE, 4096).is_err(),
            "truncating to {cut} bytes was accepted"
        );
    }
}

/// `Action::Run` alone still ends the stream: the `0x00` control byte says so
/// without any help from the caller.
#[test]
fn run_alone_reaches_stream_end() {
    let data = std::fs::read(INPUT_HTML).unwrap();
    let (compressed, dict_size) = compress(&data, 1);

    let mut stream = Lzma2Stream::new(dict_size);
    let mut output = vec![0u8; 4096];
    let mut decompressed = Vec::new();
    let mut in_pos = 0;

    loop {
        let result = stream
            .process(&compressed[in_pos..], &mut output, Action::Run)
            .unwrap();
        in_pos += result.bytes_consumed;
        decompressed.extend_from_slice(&output[..result.bytes_produced]);
        if result.status == Status::StreamEnd {
            break;
        }
        assert!(result.bytes_consumed != 0 || result.bytes_produced != 0);
    }

    assert!(decompressed == data);
    assert_eq!(in_pos, compressed.len());
}

/// `Action::Finish` on a stream that really is complete ends it, and on one cut
/// short in any of the three places it can be cut short it errors.
#[test]
fn finish_semantics() {
    let data = std::fs::read(INPUT_HTML).unwrap();
    let (compressed, dict_size) = compress(&data, 1);

    let decompressed = decode(Lzma2Stream::new(dict_size), &compressed, ENTIRE, 4096).unwrap();
    assert!(decompressed == data);

    // Mid chunk header, mid range coder init, mid chunk payload.
    for cut in [2usize, 8, 40] {
        let mut stream = Lzma2Stream::new(dict_size);
        let mut output = vec![0u8; 4096];
        let mut in_pos = 0;
        let mut errored = false;
        for _ in 0..100 {
            match stream.process(&compressed[in_pos..cut], &mut output, Action::Finish) {
                Ok(result) => {
                    in_pos += result.bytes_consumed;
                    assert_ne!(
                        result.status,
                        Status::StreamEnd,
                        "cut {cut} ended the stream"
                    );
                }
                Err(_) => {
                    errored = true;
                    break;
                }
            }
        }
        assert!(errored, "Action::Finish accepted a stream cut at {cut}");
    }
}

/// The compressed chunk is no longer buffered, so nothing 64 KiB sized may be
/// allocated up front, and the dictionary stays lazy behind `ensure_capacity`.
#[test]
fn new_allocates_no_chunk_buffer() {
    let before = allocated();
    let stream = Lzma2Stream::new(1 << 24);
    let after = allocated();
    drop(stream);
    assert!(
        after - before < 4096,
        "Lzma2Stream::new allocated {} bytes",
        after - before
    );
}

thread_local! {
    /// Bytes this thread has asked the allocator for. Per thread, so tests
    /// running side by side do not disturb each other.
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

fn allocated() -> usize {
    ALLOCATED.with(|counter| counter.get())
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATED.try_with(|counter| counter.set(counter.get() + layout.size()));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATED.try_with(|counter| {
            counter.set(counter.get() + new_size.saturating_sub(layout.size()))
        });
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// Grows the first chunk's declared compressed size by `junk` and pads its
/// payload to match, so the chunk claims bytes no symbol will ever read.
fn pad_first_chunk(compressed: &[u8], junk: usize) -> Vec<u8> {
    assert!(compressed[0] >= 0x80, "not an LZMA chunk");
    let header = if compressed[0] >= 0xC0 { 6 } else { 5 };
    let declared = u16::from_be_bytes([compressed[3], compressed[4]]) as usize + 1;

    let mut padded = compressed[..header].to_vec();
    let grown = (declared + junk - 1) as u16;
    padded[3] = (grown >> 8) as u8;
    padded[4] = (grown & 0xFF) as u8;
    padded.extend_from_slice(&compressed[header..header + declared]);
    padded.extend(core::iter::repeat_n(0u8, junk));
    padded.extend_from_slice(&compressed[header + declared..]);
    padded
}

/// A chunk that declares more compressed bytes than its symbols read has to be
/// rejected, however the caller slices the input.
///
/// The budget counts bytes taken in, and the last few of those can still be in
/// the carry when it reaches zero, so a used up budget alone does not mean the
/// chunk was really consumed. `Lzma2Reader` rejects these, and the two decoders
/// have to agree.
#[test]
fn chunk_declaring_unread_bytes_is_rejected() {
    let data = std::fs::read(INPUT_HTML).unwrap();
    let (compressed, dict_size) = compress(&data, 6);

    // The unpadded stream has to decode, or the rest proves nothing.
    decode(Lzma2Stream::new(dict_size), &compressed, ENTIRE, 4096).unwrap();

    for junk in [1usize, 2, 5, 10, 19, 20] {
        let padded = pad_first_chunk(&compressed, junk);
        for &chunk in CHUNK_SIZES {
            for &out_size in OUTPUT_SIZES {
                let result = decode(Lzma2Stream::new(dict_size), &padded, chunk, out_size);
                assert!(
                    result.is_err(),
                    "junk {junk} chunk {chunk} out {out_size}: accepted a chunk \
                     with {junk} bytes the decoder never read"
                );
            }
        }
    }
}
