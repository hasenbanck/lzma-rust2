use std::{
    io::{Read, Seek, SeekFrom, Write},
    num::NonZeroU64,
};

use lzma_rust2::{
    LzipOptions, LzipReaderMt, LzipWriter, Lzma2Options, Lzma2Reader, Lzma2ReaderMt, Lzma2Writer,
    Lzma2WriterMt, LzmaOptions, LzmaReader, LzmaWriter, XzOptions, XzReader, XzReaderMt,
    XzWriterMt, xz_uncompressed_size,
};

fn regression_lzma2_reader_mt(input_data: &[u8], expected_output: &[u8], dict_size: u32) {
    let mut uncompressed = Vec::new();

    {
        let mut reader = Lzma2ReaderMt::new(input_data, dict_size, None, 1);
        reader.read_to_end(&mut uncompressed).unwrap();
    }

    // We don't use assert_eq since the debug output would be too big.
    assert!(uncompressed.as_slice() == expected_output);
}

/// Issue: Decompressing: Corrupted input data (LZMA2:0)
///
/// https://github.com/hasenbanck/sevenz-rust2/issues/44
#[test]
fn issue_44_7z() {
    let input = std::fs::read("tests/data/issue_44_7z.lzma2").unwrap();
    let output = std::fs::read("tests/data/issue_44_7z.bin").unwrap();
    regression_lzma2_reader_mt(input.as_slice(), output.as_slice(), 8388608);
}

fn regression_xz_reader(input_data: &[u8], expected_output: &[u8]) {
    let mut uncompressed = Vec::new();

    {
        let mut reader = XzReader::new(input_data, true);
        reader.read_to_end(&mut uncompressed).unwrap();
    }

    // We don't use assert_eq since the debug output would be too big.
    assert!(uncompressed.as_slice() == expected_output);
}

/// Issue: Can't read XZ with multiple streams
///
/// https://github.com/hasenbanck/lzma-rust2/issues/56
#[test]
fn issue_56() {
    let input = std::fs::read("tests/data/issue_56.xz").unwrap();
    let output = *b"One\nTwo\n";
    regression_xz_reader(input.as_slice(), output.as_slice());
}

/// Issue: lzma2_reader overflow-checks (attempt to add with overflow)
///
/// https://github.com/hasenbanck/lzma-rust2/issues/64
#[test]
fn issue_64() {
    let input = std::fs::read("tests/data/issue_64.bin").unwrap();

    let option = Lzma2Options::with_preset(0);
    let dict_size = option.lzma_options.dict_size;

    let mut uncompressed = Vec::new();

    let mut reader = Lzma2Reader::new(input.as_slice(), dict_size, None);
    let _ = reader.read_to_end(&mut uncompressed);
}

/// Issue: LZMA roundtrip fails with "dist overflow" when using preset dictionary
///
/// https://github.com/hasenbanck/lzma-rust2/issues/94
#[test]
fn issue_94() {
    let dict = b"section></summary><div class=</a></li".to_vec();
    let data = std::fs::read("tests/data/input.html").unwrap();

    let options = {
        let mut options = LzmaOptions::with_preset(9);
        options.preset_dict = Some(dict.clone());
        options
    };

    let output = std::io::Cursor::new(Vec::new());
    let mut encoder = LzmaWriter::new_no_header(output, &options, false).unwrap();
    std::io::copy(&mut std::io::Cursor::new(data.clone()), &mut encoder).unwrap();
    let compressed = encoder.finish().unwrap().into_inner();
    println!("Encode OK");

    let mut out = std::io::Cursor::new(Vec::new());
    let mut decoder = LzmaReader::new_with_props(
        compressed.as_slice(),
        data.len() as u64,
        options.get_props(),
        options.dict_size,
        options.preset_dict.as_deref(),
    )
    .unwrap();
    std::io::copy(&mut decoder, &mut out).unwrap();
    let decompressed = out.into_inner();
    println!("Decode OK");

    // We don't use assert_eq since the debug output would be too big.
    assert!(decompressed.as_slice() == data);
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

mod allocation_tracking {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
    };

    struct Allocator;

    #[global_allocator]
    static ALLOCATOR: Allocator = Allocator;

    thread_local! {
        static USAGE: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
    }

    fn update(added: usize, removed: usize) {
        let _ = USAGE.try_with(|usage| {
            if let Some((current, peak)) = usage.get() {
                let current = current + added - removed;
                usage.set(Some((current, peak.max(current))));
            }
        });
    }

    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                update(layout.size(), 0);
            }
            ptr
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let ptr = unsafe { System.alloc_zeroed(layout) };
            if !ptr.is_null() {
                update(layout.size(), 0);
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            update(0, layout.size());
            unsafe { System.dealloc(ptr, layout) };
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let ptr = unsafe { System.realloc(ptr, layout, size) };
            if !ptr.is_null() {
                update(size, layout.size());
            }
            ptr
        }
    }

    // Measures requested live heap memory on this thread. The closure must
    // allocate and drop its own objects without dropping pre-existing ones.
    pub fn peak(f: impl FnOnce()) -> usize {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                USAGE.with(|usage| usage.set(None));
            }
        }

        USAGE.with(|usage| usage.set(Some((0, 0))));
        let _reset = Reset;
        f();
        USAGE.with(|usage| usage.get().unwrap().1)
    }
}

#[test]
fn encoder_memory_estimate_covers_custom_literal_context_peak() {
    fn peak(options: &LzmaOptions) -> usize {
        allocation_tracking::peak(|| {
            let mut writer = LzmaWriter::new_use_header(std::io::sink(), options, None).unwrap();
            writer.write_all(include_bytes!("../LICENSE")).unwrap();
            writer.finish().unwrap();
        })
    }

    for preset in [0, 5] {
        let mut default = LzmaOptions::with_preset(preset);
        default.dict_size = 1 << 16;
        let mut custom = default.clone();
        custom.lc = 8;
        custom.lp = 4;

        let default_peak = peak(&default);
        let custom_peak = peak(&custom);
        // Hold the mode and dictionary constant to isolate literal-model growth.
        let measured_growth = custom_peak
            .checked_sub(default_peak)
            .expect("custom literal contexts used less memory than the default");
        let estimated_growth =
            (custom.get_memory_usage() - default.get_memory_usage()) as usize * 1024;

        assert!(
            measured_growth.abs_diff(estimated_growth) <= 64 * 1024,
            "preset {preset}: peak grew by {measured_growth} bytes, estimate grew by {estimated_growth} bytes"
        );
        for (options, measured_peak) in [(&default, default_peak), (&custom, custom_peak)] {
            assert!(
                measured_peak <= options.get_memory_usage() as usize * 1024,
                "preset {preset}, lc={}, lp={}: peak {measured_peak} bytes exceeds estimate {} KiB",
                options.lc,
                options.lp,
                options.get_memory_usage()
            );
        }
    }
}

#[test]
fn xz_rejects_multiple_lzma2_filters() {
    let mut input = b"\xfd7zXZ\0\0\0".to_vec();
    input.extend_from_slice(&crc32(&[0, 0]).to_le_bytes());

    // Four LZMA2 filters, each declaring a 1 MiB dictionary.
    let mut header = vec![4, 3];
    for _ in 0..4 {
        header.extend_from_slice(&[0x21, 1, 16]);
    }
    header.extend_from_slice(&[0, 0]);
    header.extend_from_slice(&crc32(&header).to_le_bytes());
    input.extend_from_slice(&header);

    let mut payload = vec![b'a'];
    for _ in 0..4 {
        let mut chunk = vec![1];
        chunk.extend_from_slice(&((payload.len() - 1) as u16).to_be_bytes());
        chunk.extend_from_slice(&payload);
        chunk.push(0);
        payload = chunk;
    }
    input.extend_from_slice(&payload);

    let mut reader = XzReader::new(input.as_slice(), false);
    let error = reader.read(&mut [0]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(reader.into_inner(), payload);

    let mut reader = XzReader::new_mem_limit(input.as_slice(), false, 1128);
    let error = reader.read(&mut [0]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(reader.into_inner(), payload);

    let mut stream = lzma_rust2::XzStream::new_mem_limit(false, 1128);
    let error = stream
        .process(&input, &mut [0], lzma_rust2::Action::Finish)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn lzma2_preset_uses_one_dictionary_buffer() {
    let dict_size = 1 << 20;
    for preset_size in [0, 1, dict_size / 2, dict_size, dict_size + 1] {
        let preset: Vec<u8> = (0..preset_size).map(|i| i as u8).collect();
        let mut options = Lzma2Options::with_preset(0);
        options.lzma_options.dict_size = dict_size as u32;
        options.lzma_options.preset_dict = (!preset.is_empty()).then(|| preset.clone());
        let mut data = [42; 8192];
        if !preset.is_empty() {
            let suffix_len = preset.len().min(data.len());
            for (i, byte) in data.iter_mut().enumerate() {
                *byte = preset[preset.len() - suffix_len + i % suffix_len];
            }
        }
        let mut writer = Lzma2Writer::new(Vec::new(), options);
        writer.write_all(&data).unwrap();
        let compressed = writer.finish().unwrap();

        for limited in [false, true] {
            let peak = allocation_tracking::peak(|| {
                let mut reader = if limited {
                    Lzma2Reader::new_mem_limit(
                        compressed.as_slice(),
                        dict_size as u32,
                        1128,
                        Some(&preset),
                    )
                    .unwrap()
                } else {
                    Lzma2Reader::new(compressed.as_slice(), dict_size as u32, Some(&preset))
                };
                let mut output = [0; 8192];
                reader.read_exact(&mut output).unwrap();
                assert_eq!(output, data);
                assert_eq!(reader.read(&mut [0]).unwrap(), 0);
            });
            // One dictionary plus the range decoder and probability model budget.
            assert!(
                peak <= dict_size + 104 * 1024,
                "preset {preset_size}: peak {peak}"
            );
        }

        let error = Lzma2Reader::new_mem_limit(
            compressed.as_slice(),
            dict_size as u32,
            1127,
            Some(&preset),
        )
        .err()
        .unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::OutOfMemory);
    }
}

fn encode_multibyte(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
    out
}

/// A crafted single-block XZ stream whose index declares a `unpadded_size` far
/// larger than the file. The multi-threaded reader must reject it instead of
/// trying to allocate a buffer of that size.
fn xz_with_huge_index_record(unpadded_size: u64) -> Vec<u8> {
    let mut stream = Vec::new();

    stream.extend_from_slice(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]);
    let stream_flags = [0u8, 0u8];
    stream.extend_from_slice(&stream_flags);
    stream.extend_from_slice(&crc32(&stream_flags).to_le_bytes());

    let mut index_body = vec![0x00];
    index_body.extend_from_slice(&encode_multibyte(1));
    index_body.extend_from_slice(&encode_multibyte(unpadded_size));
    index_body.extend_from_slice(&encode_multibyte(0));
    while index_body.len() % 4 != 0 {
        index_body.push(0);
    }
    let index_crc = crc32(&index_body);

    let index_size = index_body.len() + 4;
    let backward_size = (index_size / 4 - 1) as u32;

    stream.extend_from_slice(&index_body);
    stream.extend_from_slice(&index_crc.to_le_bytes());

    let mut footer_crc_input = Vec::new();
    footer_crc_input.extend_from_slice(&backward_size.to_le_bytes());
    footer_crc_input.extend_from_slice(&stream_flags);
    stream.extend_from_slice(&crc32(&footer_crc_input).to_le_bytes());
    stream.extend_from_slice(&backward_size.to_le_bytes());
    stream.extend_from_slice(&stream_flags);
    stream.extend_from_slice(b"YZ");

    stream
}

/// Malicious XZ where the index claims a 2^60-byte block. Previously the
/// multi-threaded reader did `vec![0u8; unpadded_size]` and aborted with OOM.
#[test]
fn xz_mt_huge_index_record_does_not_oom() {
    let input = xz_with_huge_index_record(1 << 60);

    let result = XzReaderMt::new(std::io::Cursor::new(input), false, 2).and_then(|mut reader| {
        let mut output = Vec::new();
        reader.read_to_end(&mut output)
    });
    assert!(result.is_err());
}

#[test]
fn xz_mt_index_record_past_the_index_is_not_allocated() {
    let input = xz_with_huge_index_record(1 << 30);

    let mut result = None;
    let peak = allocation_tracking::peak(|| {
        result = Some(
            XzReaderMt::new(std::io::Cursor::new(input.as_slice()), false, 2).and_then(
                |mut reader| {
                    let mut output = Vec::new();
                    reader.read_to_end(&mut output)
                },
            ),
        );
    });

    assert!(result.unwrap().is_err());
    assert!(peak < 1 << 20, "peak {peak} bytes");
}

struct FaultyReader {
    inner: std::io::Cursor<Vec<u8>>,
    armed: bool,
}

impl Read for FaultyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.armed && buf.len() > 24 {
            return Err(std::io::Error::other("injected read failure"));
        }
        self.inner.read(buf)
    }
}

impl Seek for FaultyReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// An I/O error while the multi-threaded LZIP reader fetches a member must
/// surface as an error instead of panicking a `.unwrap()`.
#[test]
fn lzip_mt_read_error_does_not_panic() {
    let mut compressed = Vec::new();
    {
        let mut writer = LzipWriter::new(&mut compressed, LzipOptions::with_preset(6));
        writer.write_all(b"hello lzip multithreaded world").unwrap();
        writer.finish().unwrap();
    }

    let reader = FaultyReader {
        inner: std::io::Cursor::new(compressed),
        armed: true,
    };

    let mut reader = LzipReaderMt::new(reader, 2).unwrap();
    let mut output = Vec::new();
    assert!(reader.read_to_end(&mut output).is_err());
}

const CHUNK_LEN: usize = 1 << 20;

fn xorshift64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Generates the ~2 GiB test input on the fly so that it never has to be
/// buffered: a long run of a single byte followed by a pseudo random tail.
///
/// The run is a single repeated byte on purpose. It leaves all but one hash slot
/// at `0`, which is exactly the state that normalization corrupts, and the
/// random tail then reaches those slots.
struct Input {
    filler_left: u64,
    tail_left: u64,
    state: u64,
}

impl Input {
    fn new(filler_len: u64, tail_len: u64) -> Self {
        Self {
            filler_left: filler_len,
            tail_left: tail_len,
            state: 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// Writes the next bytes into `buf` and returns how many were produced.
    /// A chunk never straddles the filler / tail boundary, which keeps the
    /// output independent of how the chunks line up.
    fn next_chunk(&mut self, buf: &mut [u8]) -> usize {
        if self.filler_left > 0 {
            let len = (buf.len() as u64).min(self.filler_left) as usize;
            buf[..len].fill(b'A');
            self.filler_left -= len as u64;
            return len;
        }

        let len = (buf.len() as u64).min(self.tail_left) as usize;
        for chunk in buf[..len].chunks_mut(8) {
            let bytes = xorshift64(&mut self.state).to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        self.tail_left -= len as u64;
        len
    }
}

/// Pushes enough data through a single stream to make the match finder
/// normalize its position tables, then feeds varied data so that the
/// normalized entries get looked up again.
fn regression_normalization(preset: u32) {
    let dict_size = LzmaOptions::with_preset(preset).dict_size;

    // Normalization fires at `lz_pos == 0x7FFFFFFF`, and `lz_pos` starts at
    // `dict_size + 1`.
    let filler_len = 0x7FFF_FFFF - dict_size as u64 - 1;
    let tail_len = 4 << 20;

    // The filler compresses down to almost nothing, so keeping the whole
    // compressed stream costs only about as much as the random tail.
    let mut compressed = Vec::new();
    let mut writer = Lzma2Writer::new(&mut compressed, Lzma2Options::with_preset(preset));

    let mut chunk = vec![0u8; CHUNK_LEN];
    let mut input = Input::new(filler_len, tail_len);
    loop {
        let len = input.next_chunk(&mut chunk);
        if len == 0 {
            break;
        }
        writer.write_all(&chunk[..len]).unwrap();
    }

    writer.finish().unwrap();

    // Silent corruption is just as bad as the panic, so decode the stream back
    // and compare it against a freshly generated copy of the input.
    let mut reader = Lzma2Reader::new(compressed.as_slice(), dict_size, None);
    let mut decoded = vec![0u8; CHUNK_LEN];
    let mut input = Input::new(filler_len, tail_len);
    let mut position = 0u64;
    loop {
        let len = input.next_chunk(&mut chunk);
        if len == 0 {
            break;
        }
        reader.read_exact(&mut decoded[..len]).unwrap();

        if decoded[..len] != chunk[..len] {
            let offset = (0..len).find(|&i| decoded[i] != chunk[i]).unwrap();
            panic!(
                "decoded byte at {} is {:#04X}, expected {:#04X}",
                position + offset as u64,
                decoded[offset],
                chunk[offset]
            );
        }
        position += len as u64;
    }

    let mut rest = Vec::new();
    reader.read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty(), "{} trailing bytes decoded", rest.len());
}

/// Issue: Encoder: `normalize_scalar` leaves negative positions, causing an
/// out-of-bounds panic after ~2 GiB in one stream
///
/// https://github.com/hasenbanck/lzma-rust2/issues/107
///
/// Both tests are `#[ignore]`d because the encoder only normalizes after
/// `0x7FFFFFFF` positions, so there is no way to reach the bug without pushing
/// ~2 GiB through a single stream. Run them with:
///
/// ```text
/// cargo test --release --test regression -- --ignored issue_107
/// ```
#[test]
#[ignore = "pushes ~2 GiB through the encoder; run with --release"]
fn issue_107_hc4() {
    regression_normalization(0);
}

/// See [`issue_107_hc4`]. Preset 6 is the default and uses the Bt4 match
/// finder, which is what the issue was reported with. Before the fix this
/// panicked in `LzEncoderData::get_byte_backward` after about a minute.
#[test]
#[ignore = "pushes ~2 GiB through the encoder; run with --release"]
fn issue_107_bt4() {
    regression_normalization(6);
}

#[test]
fn lzma2_memory_limit_includes_decoder_overhead() {
    // An uncompressed three-byte chunk followed by the end marker.
    const RAW: &[u8] = b"\x01\x00\x02raw\x00";

    for (dict_size, required_kib) in [(4096, 108), (65536, 168)] {
        let mut input = std::io::Cursor::new(RAW);
        let error = Lzma2Reader::new_mem_limit(&mut input, dict_size, required_kib - 1, None)
            .err()
            .unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::OutOfMemory);
        assert_eq!(input.position(), 0);

        let mut reader =
            Lzma2Reader::new_mem_limit(&mut input, dict_size, required_kib, None).unwrap();
        assert_eq!(reader.inner().position(), 0);
        let mut output = [0; 3];
        reader.read_exact(&mut output).unwrap();
        assert_eq!(&output, b"raw");
        assert_eq!(reader.read(&mut output).unwrap(), 0);
        assert_eq!(reader.into_inner().position(), RAW.len() as u64);
    }
}

#[test]
fn lzma2_memory_limit_rejects_maximum_dictionary_without_reading_input() {
    let mut input = std::io::Cursor::new([0]);
    let error = Lzma2Reader::new_mem_limit(&mut input, u32::MAX, 1128, None)
        .err()
        .unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::OutOfMemory);
    assert_eq!(input.position(), 0);
}

#[test]
fn xz_memory_limit_boundary_preserves_unread_block_data() {
    // One uncompressed LZMA2 chunk with a 4 KiB dictionary and no data checksum.
    const XZ: &[u8] = &[
        0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 0x00, 0x00, 0xFF, 0x12, 0xD9, 0x41, 0x02, 0x00, 0x21,
        0x01, 0x00, 0x00, 0x00, 0x00, 0x37, 0x27, 0x97, 0xD6, 0x01, 0x00, 0x05, 0x6C, 0x69, 0x6D,
        0x69, 0x74, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x01, 0x16, 0x06, 0xC9, 0xA5, 0x7D, 0xD5, 0x06,
        0x72, 0x9E, 0x7A, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x59, 0x5A,
    ];

    // The dictionary and decoder overhead require 108 KiB.
    let mut output = [0xA5; 6];
    let mut reader = XzReader::new_mem_limit(XZ, false, 107);
    let error = reader.read(&mut output).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::OutOfMemory);
    assert_eq!(output, [0xA5; 6]);
    assert_eq!(reader.into_inner(), &XZ[24..]);

    let mut reader = XzReader::new_mem_limit(XZ, false, 108);
    reader.read_exact(&mut output).unwrap();
    assert_eq!(&output, b"limit\n");
    assert_eq!(reader.read(&mut output).unwrap(), 0);
    assert!(reader.into_inner().is_empty());
}

/// A worker that gives up has to be reported by the next flush, not left for
/// the caller to wait on. A `pb` above four is not allowed, so the worker fails
/// as soon as it picks the work up, and flush has to hand that error back.
///
/// The writing happens on its own thread so that a flush which never returns
/// shows up here as a timeout instead of stopping the whole test run.
#[test]
fn flush_reports_worker_exit() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut options = Lzma2Options::with_preset(0);
        options.lzma_options.pb = 5;
        options.set_chunk_size(NonZeroU64::new(256 * 1024));
        let mut writer = Lzma2WriterMt::new(Vec::new(), options, 1).unwrap();
        let data: Vec<u8> = (0..4096).map(|i| ((i * 37 + i / 7) % 256) as u8).collect();
        writer.write_all(&data).unwrap();
        tx.send(writer.flush().is_err()).unwrap();
    });
    assert!(
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("flush hung after the worker exited")
    );
}

/// The same, but flushing twice. Once a worker is gone the second flush has to
/// report it too, rather than wait for work that is never coming.
#[test]
fn repeated_flush_reports_worker_failure() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut options = XzOptions::with_preset(0);
        options.lzma_options.pb = 5;
        options.set_block_size(NonZeroU64::new(256 * 1024));
        let mut writer = XzWriterMt::new(Vec::new(), options, 1).unwrap();
        let data: Vec<u8> = (0..4096).map(|i| ((i * 37 + i / 7) % 256) as u8).collect();
        writer.write_all(&data).unwrap();
        tx.send(writer.flush().is_err()).unwrap();
        tx.send(writer.flush().is_err()).unwrap();
    });
    for attempt in 1..=2 {
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap_or_else(|_| panic!("flush attempt {attempt} hung"))
        );
    }
}

/// The decoder's dictionary must grow with the decoded data, not be allocated at the size the
/// stream header declares: a few bytes of input could otherwise make the reader commit up to
/// 4 GiB before it decodes anything.
mod dictionary_follows_output {
    use super::*;

    /// Decodes `input` completely and returns (output, peak live heap bytes).
    fn decode_peak(input: &[u8], xz: bool) -> (Vec<u8>, usize) {
        let mut out = Vec::new();
        let peak = allocation_tracking::peak(|| {
            let mut reader: Box<dyn Read> = if xz {
                Box::new(XzReader::new(input, false))
            } else {
                Box::new(LzmaReader::new_mem_limit(input, u32::MAX, None).unwrap())
            };
            let mut buf = [0u8; 4096];
            loop {
                let n = reader.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..n]);
            }
        });
        (out, peak)
    }

    /// The same through the sans-I/O decoders (`XzStream`, `LzmaStream`), which share the
    /// dictionary code with the readers: input in small slices, output into a small buffer.
    fn decode_peak_stream(input: &[u8], xz: bool) -> (Vec<u8>, usize) {
        use lzma_rust2::{Action, LzmaStream, Status, XzStream};
        let mut out = Vec::new();
        let peak = allocation_tracking::peak(|| {
            let mut xz_stream = XzStream::new(false);
            let mut lzma_stream = LzmaStream::new_mem_limit(u32::MAX, None);
            let mut buf = [0u8; 4096];
            let mut pos = 0;
            loop {
                let end = (pos + 64).min(input.len());
                let action = if end == input.len() {
                    Action::Finish
                } else {
                    Action::Run
                };
                let r = if xz {
                    xz_stream.process(&input[pos..end], &mut buf, action)
                } else {
                    lzma_stream.process(&input[pos..end], &mut buf, action)
                }
                .unwrap();
                pos += r.bytes_consumed;
                out.extend_from_slice(&buf[..r.bytes_produced]);
                if r.status == Status::StreamEnd {
                    break;
                }
                assert!(
                    r.bytes_consumed > 0 || r.bytes_produced > 0,
                    "no progress at input {pos}"
                );
            }
        });
        (out, peak)
    }

    const DATA: &[u8] = b"hello world, hello world, hello world, hello world";
    const LIMIT: usize = 4 << 20;

    /// Header-declared dictionary sizes are what an attacker controls; the encoder can't build
    /// such a dictionary, so encode with a small one and patch the header.
    #[test]
    fn xz_small_stream_declaring_a_3_gib_dictionary() {
        let mut options = XzOptions::with_preset(0);
        options.lzma_options.dict_size = 4096;
        let mut writer = lzma_rust2::XzWriter::new(Vec::new(), options).unwrap();
        writer.write_all(DATA).unwrap();
        let mut compressed = writer.finish().unwrap();

        // Block header after the 12-byte stream header: size, flags, filter id 0x21,
        // properties size 1, LZMA2 dictionary byte, padding, CRC32.
        let header_len = (usize::from(compressed[12]) + 1) * 4;
        assert_eq!(&compressed[14..16], &[0x21, 0x01]);
        compressed[16] = 39; // 3 GiB
        let crc = crc32(&compressed[12..12 + header_len - 4]);
        compressed[12 + header_len - 4..12 + header_len].copy_from_slice(&crc.to_le_bytes());

        for (name, (out, peak)) in [
            ("XzReader", decode_peak(&compressed, true)),
            ("XzStream", decode_peak_stream(&compressed, true)),
        ] {
            assert_eq!(out, DATA, "{name}");
            assert!(
                peak < LIMIT,
                "{name}: peak {peak} bytes for {} bytes of output",
                out.len()
            );
        }
    }

    #[test]
    fn lzma_small_stream_declaring_a_4_gib_dictionary_and_unknown_size() {
        let mut options = LzmaOptions::with_preset(0);
        options.dict_size = 4096;
        let mut writer = LzmaWriter::new_use_header(Vec::new(), &options, None).unwrap();
        writer.write_all(DATA).unwrap();
        let mut compressed = writer.finish().unwrap();

        // .lzma header: properties byte, dictionary size (u32 LE), uncompressed size (u64 LE).
        compressed[1..5].copy_from_slice(&lzma_rust2::DICT_SIZE_MAX.to_le_bytes());
        compressed[5..13].copy_from_slice(&u64::MAX.to_le_bytes());

        for (name, (out, peak)) in [
            ("LzmaReader", decode_peak(&compressed, false)),
            ("LzmaStream", decode_peak_stream(&compressed, false)),
        ] {
            assert_eq!(out, DATA, "{name}");
            assert!(
                peak < LIMIT,
                "{name}: peak {peak} bytes for {} bytes of output",
                out.len()
            );
        }
    }

    /// Output larger than the initial growth step, with matches reaching back across the
    /// growth boundaries, still decodes to the same bytes.
    #[test]
    fn grows_across_many_steps() {
        let data: Vec<u8> = (0..3_000_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8 ^ (i / 1000) as u8)
            .chain((0..2_000_000u32).map(|i| (i % 251) as u8))
            .collect();
        for xz in [false, true] {
            let compressed = if xz {
                let mut options = XzOptions::with_preset(6);
                options.lzma_options.dict_size = 64 << 20;
                let mut w = lzma_rust2::XzWriter::new(Vec::new(), options).unwrap();
                w.write_all(&data).unwrap();
                w.finish().unwrap()
            } else {
                let mut options = LzmaOptions::with_preset(6);
                options.dict_size = 64 << 20;
                let mut w = LzmaWriter::new_use_header(Vec::new(), &options, None).unwrap();
                w.write_all(&data).unwrap();
                w.finish().unwrap()
            };
            let (out, peak) = decode_peak(&compressed, xz);
            assert!(out == data, "xz={xz}: output differs");
            assert!(peak < 64 << 20, "xz={xz}: peak {peak} bytes");
            let (out, peak) = decode_peak_stream(&compressed, xz);
            assert!(out == data, "xz={xz} stream: output differs");
            assert!(peak < 64 << 20, "xz={xz} stream: peak {peak} bytes");
        }
    }
}

/// An XZ index declaring a huge record count must not make the reader reserve memory for it.
#[test]
fn xz_index_record_count_does_not_size_the_reservation() {
    let input = [
        0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 0x00, 0x04, 0xE6, 0xD6, 0xB4, 0x46, 0x00, 0x9E, 0xCE,
        0xEC, 0x5F, 0xB7, 0xDB, 0xFF, 0xFF, 0xE0, 0x25,
    ];

    let mut result = None;
    let peak = allocation_tracking::peak(|| {
        let mut uncompressed = Vec::new();
        result = Some(XzReader::new(input.as_slice(), true).read_to_end(&mut uncompressed));
    });

    assert!(result.unwrap().is_err());
    assert!(peak < 1 << 20, "peak {peak} bytes for 23 bytes of input");
}

mod lzma_declared_end {
    use super::*;

    fn data() -> Vec<u8> {
        let mut data = Vec::new();
        for i in 0..4000u32 {
            data.extend_from_slice(b"abcdefghijklmnopqrstuvwxyz0123456789");
            data.push((i % 251) as u8);
        }
        data
    }

    fn encode(data: &[u8], use_end_marker: bool) -> Vec<u8> {
        let options = LzmaOptions::with_preset(6);
        let size = Some(data.len() as u64);
        let mut writer = LzmaWriter::new(Vec::new(), &options, true, use_end_marker, size).unwrap();
        writer.write_all(data).unwrap();
        writer.finish().unwrap()
    }

    fn with_declared_size(compressed: &[u8], size: usize) -> Vec<u8> {
        let mut compressed = compressed.to_vec();
        compressed[5..13].copy_from_slice(&(size as u64).to_le_bytes());
        compressed
    }

    fn reader(compressed: &[u8], relaxed: bool) -> LzmaReader<&[u8]> {
        let mut reader = LzmaReader::new_mem_limit(compressed, u32::MAX, None).unwrap();
        reader.set_relaxed_end_condition(relaxed);
        reader
    }

    fn read_to_end_fails(compressed: &[u8], relaxed: bool) -> bool {
        reader(compressed, relaxed)
            .read_to_end(&mut Vec::new())
            .is_err()
    }

    fn assert_bounded_reads_fail(compressed: &[u8], size: usize, relaxed: bool) {
        let mut output = vec![0; size];
        let exact = reader(compressed, relaxed).read_exact(&mut output);
        let taken = reader(compressed, relaxed)
            .take(size as u64)
            .read_to_end(&mut Vec::new());
        assert!(exact.is_err(), "read_exact of {size} bytes succeeded");
        assert!(taken.is_err(), "take({size}).read_to_end succeeded");
    }

    #[test]
    fn declared_size_inside_a_match_fails_a_bounded_read() {
        let data = data();
        let compressed = encode(&data, false);

        let size = ((data.len() - 200)..data.len())
            .find(|&size| read_to_end_fails(&with_declared_size(&compressed, size), true))
            .expect("no declared size ends inside a match");

        assert_bounded_reads_fail(&with_declared_size(&compressed, size), size, true);
    }

    #[test]
    fn strict_end_condition_fails_a_bounded_read() {
        let data = data();
        let compressed = encode(&data, false);

        for size in (data.len() - 50)..data.len() {
            let compressed = with_declared_size(&compressed, size);
            assert!(read_to_end_fails(&compressed, false), "size {size}");
            assert_bounded_reads_fail(&compressed, size, false);
        }
    }

    #[test]
    fn strict_end_condition_accepts_known_size_with_end_marker() {
        let data = data();
        let compressed = encode(&data, true);

        let mut output = Vec::new();
        reader(&compressed, false).read_to_end(&mut output).unwrap();
        assert!(output == data);

        let mut output = vec![0; data.len()];
        reader(&compressed, false).read_exact(&mut output).unwrap();
        assert!(output == data);
    }

    #[test]
    fn strict_completion_accepts_both_endings_and_empty_streams() {
        for data in [Vec::new(), b"x".to_vec(), data()] {
            for marker in [false, true] {
                let compressed = encode(&data, marker);
                let mut output = Vec::new();
                reader(&compressed, false).read_to_end(&mut output).unwrap();
                assert_eq!(output, data);
                let mut output = vec![0; data.len()];
                reader(&compressed, false).read_exact(&mut output).unwrap();
                assert_eq!(output, data);
                let mut output = Vec::new();
                reader(&compressed, false)
                    .take(data.len() as u64)
                    .read_to_end(&mut output)
                    .unwrap();
                assert_eq!(output, data);
            }
        }
    }

    #[test]
    fn known_size_with_end_marker_matches_liblzma() {
        let data = data();
        let compressed = encode(&data, true);
        let stream = liblzma::stream::Stream::new_lzma_decoder(u64::MAX).unwrap();
        let mut reference = liblzma::read::XzDecoder::new_stream(&compressed[..], stream);
        let mut expected = Vec::new();
        reference.read_to_end(&mut expected).unwrap();
        assert_eq!(expected, data);
        let mut output = Vec::new();
        reader(&compressed, false).read_to_end(&mut output).unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn strict_completion_preserves_bytes_after_end_marker() {
        let data = data();
        let mut compressed = encode(&data, true);
        compressed.extend_from_slice(b"next member");
        let mut decoder = reader(&compressed, false);
        let mut output = Vec::new();
        decoder.read_to_end(&mut output).unwrap();
        assert_eq!(output, data);
        let (remaining, mut unused) = decoder.into_parts();
        unused.extend_from_slice(remaining);
        assert_eq!(unused, b"next member");
    }

    #[test]
    fn strict_completion_rejects_truncated_end_markers() {
        let data = data();
        let compressed = encode(&data, true);
        for removed in 1..=5 {
            let truncated = &compressed[..compressed.len() - removed];
            assert!(read_to_end_fails(truncated, false));
            assert_bounded_reads_fail(truncated, data.len(), false);
        }
    }

    #[test]
    fn deferred_terminal_error_preserves_output_and_ignores_empty_reads() {
        let data = data();
        let compressed = encode(&data, false);
        let size = data.len() - 50;
        let compressed = with_declared_size(&compressed, size);
        let mut decoder = reader(&compressed, false);
        decoder.set_defer_errors(true);
        let mut output = vec![0; size];
        decoder.read_exact(&mut output).unwrap();
        assert_eq!(output, data[..size]);
        assert_eq!(decoder.read(&mut []).unwrap(), 0);
        assert_eq!(
            decoder.read(&mut [0]).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn deferred_decode_error_preserves_recovered_prefix() {
        let data = data();
        let mut compressed = encode(&data, true);
        compressed.truncate(compressed.len() - 20);
        let mut decoder = reader(&compressed, false);
        decoder.set_defer_errors(true);
        let mut output = vec![0; data.len()];
        let count = decoder.read(&mut output).unwrap();
        assert!(count > 0 && count < data.len());
        assert_eq!(output[..count], data[..count]);
        assert!(decoder.read(&mut [0]).is_err());
    }

    #[test]
    fn strict_completion_handles_fragmented_input_and_dictionary_wraps() {
        struct Fragmented<'a> {
            input: &'a [u8],
            chunk: usize,
        }
        impl Read for Fragmented<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let limit = buf.len().min(self.chunk);
                self.input.read(&mut buf[..limit])
            }
        }
        let data = data();
        let mut options = LzmaOptions::with_preset(6);
        options.dict_size = 4096;
        for marker in [false, true] {
            let mut writer =
                LzmaWriter::new(Vec::new(), &options, true, marker, Some(data.len() as u64))
                    .unwrap();
            writer.write_all(&data).unwrap();
            let compressed = writer.finish().unwrap();
            for chunk in [1, 17] {
                for output_size in [1, 17, 4096] {
                    let input = Fragmented {
                        input: &compressed,
                        chunk,
                    };
                    let mut decoder = LzmaReader::new_mem_limit(input, u32::MAX, None).unwrap();
                    decoder.set_relaxed_end_condition(false);
                    let mut output = Vec::new();
                    let mut buf = vec![0; output_size];
                    loop {
                        let size = decoder.read(&mut buf).unwrap();
                        if size == 0 {
                            break;
                        }
                        output.extend_from_slice(&buf[..size]);
                    }
                    assert_eq!(output, data);
                }
            }
        }
    }
}

fn xz_index_flood(records: usize, block_bytes: usize) -> Vec<u8> {
    let mut stream = vec![0xFD, b'7', b'z', b'X', b'Z', 0x00, 0x00, 0x00];
    stream.extend_from_slice(&crc32(&[0x00, 0x00]).to_le_bytes());
    stream.resize(stream.len() + block_bytes, 0);

    let mut index = vec![0x00];
    let mut count = records as u64;
    while count >= 0x80 {
        index.push(count as u8 | 0x80);
        count >>= 7;
    }
    index.push(count as u8);
    for _ in 0..records {
        index.extend_from_slice(&[0x01, 0x00]);
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

const FLOOD_RECORDS: usize = 1 << 20;

#[test]
fn xz_reader_does_not_collect_index_records() {
    let input = xz_index_flood(FLOOD_RECORDS, 0);

    let mut result = None;
    let peak = allocation_tracking::peak(|| {
        let mut uncompressed = Vec::new();
        result = Some(XzReader::new(input.as_slice(), true).read_to_end(&mut uncompressed));
    });

    assert!(result.unwrap().is_err());
    assert!(peak < 1 << 20, "peak {peak} bytes");
}

#[test]
fn xz_reader_mt_rejects_index_records_past_the_index() {
    let input = xz_index_flood(FLOOD_RECORDS, 0);

    let mut result = None;
    let peak = allocation_tracking::peak(|| {
        result = Some(XzReaderMt::new(std::io::Cursor::new(input.as_slice()), true, 1).map(|_| ()));
    });

    assert!(result.unwrap().is_err());
    assert!(peak < 1 << 20, "peak {peak} bytes");
}

#[test]
fn xz_uncompressed_size_does_not_collect_index_records() {
    let input = xz_index_flood(FLOOD_RECORDS, 4 * FLOOD_RECORDS);

    let mut result = None;
    let peak = allocation_tracking::peak(|| {
        result = Some(xz_uncompressed_size(std::io::Cursor::new(input.as_slice())));
    });

    assert_eq!(result.unwrap().unwrap(), 0);
    assert!(peak < 1 << 20, "peak {peak} bytes");
}

#[test]
fn xz_uncompressed_size_rejects_index_records_past_the_index() {
    let input = xz_index_flood(FLOOD_RECORDS, 0);

    let mut result = None;
    let peak = allocation_tracking::peak(|| {
        result = Some(xz_uncompressed_size(std::io::Cursor::new(input.as_slice())));
    });

    assert!(result.unwrap().is_err());
    assert!(peak < 1 << 20, "peak {peak} bytes");
}
