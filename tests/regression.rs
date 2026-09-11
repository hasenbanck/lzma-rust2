use std::{
    io::{Read, Seek, SeekFrom, Write},
    num::NonZeroU64,
};

use lzma_rust2::{
    LzipOptions, LzipReaderMt, LzipWriter, Lzma2Options, Lzma2Reader, Lzma2ReaderMt, Lzma2Writer,
    Lzma2WriterMt, LzmaOptions, LzmaReader, LzmaWriter, XzOptions, XzReader, XzReaderMt,
    XzWriterMt,
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
    let output = [b'O', b'n', b'e', b'\n', b'T', b'w', b'o', b'\n'];
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

    let mut reader = XzReaderMt::new(std::io::Cursor::new(input), false, 2).unwrap();
    let mut output = Vec::new();
    assert!(reader.read_to_end(&mut output).is_err());
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
