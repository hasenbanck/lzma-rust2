#![cfg(all(feature = "std", feature = "encoder", feature = "xz", feature = "lzip"))]

use std::{
    io::{self, Cursor, Read, Write},
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use lzma_rust2::{
    LzipOptions, LzipReaderMt, LzipWriterMt, Lzma2Options, Lzma2Reader, Lzma2ReaderMt,
    Lzma2WriterMt, XzOptions, XzReaderMt, XzWriterMt,
};

const BLOCK_SIZE: usize = 4096;

fn options() -> Lzma2Options {
    let mut options = Lzma2Options::with_preset(1);
    options.lzma_options.dict_size = u32::try_from(BLOCK_SIZE).unwrap();
    options.set_chunk_size(NonZeroU64::new(BLOCK_SIZE as u64));
    options
}

fn payload(size: usize) -> Vec<u8> {
    let mut state = 0x1234_5678_u32;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state.to_le_bytes()[0]
        })
        .collect()
}

fn decode(mut reader: impl Read) -> Vec<u8> {
    let mut decoded = Vec::new();
    reader.read_to_end(&mut decoded).unwrap();
    decoded
}

fn write_fragments(writer: &mut impl Write, input: &[u8]) {
    assert_eq!(writer.write(&[]).unwrap(), 0);
    for chunk in input.chunks(997) {
        assert_eq!(writer.write(chunk).unwrap(), chunk.len());
        assert_eq!(writer.write(&[]).unwrap(), 0);
    }
}

#[test]
fn bounded_lzma2_writer_round_trips_empty_partial_and_many_blocks() {
    for workers in [1, 2, 4] {
        for size in [
            0,
            1,
            BLOCK_SIZE - 1,
            BLOCK_SIZE,
            BLOCK_SIZE + 1,
            32 * BLOCK_SIZE + 137,
        ] {
            let input = payload(size);
            let mut writer = Lzma2WriterMt::new(Vec::new(), options(), workers).unwrap();
            write_fragments(&mut writer, &input);
            let compressed = writer.finish().unwrap();
            assert_eq!(
                decode(Lzma2Reader::new(
                    compressed.as_slice(),
                    u32::try_from(BLOCK_SIZE).unwrap(),
                    None
                )),
                input
            );
            assert_eq!(
                decode(Lzma2ReaderMt::new(
                    Cursor::new(compressed),
                    u32::try_from(BLOCK_SIZE).unwrap(),
                    None,
                    workers
                )),
                input
            );
        }
    }
}

#[test]
fn lzma2_flush_publishes_pending_data_without_finishing_the_stream() {
    let input = payload(5 * BLOCK_SIZE + 137);
    let mut writer = Lzma2WriterMt::new(Vec::new(), options(), 2).unwrap();
    writer.write_all(&input).unwrap();
    writer.flush().unwrap();
    // into_inner does not finish or drain the encoder. Everything submitted
    // before flush must already be present; only the stream-end byte is missing.
    let mut compressed = writer.into_inner();
    compressed.push(0);
    assert_eq!(
        decode(Lzma2Reader::new(
            compressed.as_slice(),
            u32::try_from(BLOCK_SIZE).unwrap(),
            None
        )),
        input
    );

    let mut writer = Lzma2WriterMt::new(Vec::new(), options(), 2).unwrap();
    writer.write_all(&input).unwrap();
    writer.flush().unwrap();
    writer.write_all(&input).unwrap();
    let compressed = writer.finish().unwrap();
    assert_eq!(
        decode(Lzma2Reader::new(
            compressed.as_slice(),
            u32::try_from(BLOCK_SIZE).unwrap(),
            None
        )),
        input.repeat(2)
    );
}

#[test]
fn shared_pool_lzip_writer_and_reader_round_trip_many_members() {
    let input = payload(32 * BLOCK_SIZE + 73);
    let mut options = LzipOptions::with_preset(1);
    options.lzma_options.dict_size = u32::try_from(BLOCK_SIZE).unwrap();
    options.set_member_size(NonZeroU64::new(BLOCK_SIZE as u64));
    let mut writer = LzipWriterMt::new(Vec::new(), options, 2).unwrap();
    write_fragments(&mut writer, &input);
    let compressed = writer.finish().unwrap();
    let reader = LzipReaderMt::new(Cursor::new(compressed), 2).unwrap();
    assert!(reader.member_count() > 3);
    assert_eq!(decode(reader), input);
}

#[test]
fn shared_pool_lzip_reader_propagates_corrupt_member_error() {
    let mut options = LzipOptions::with_preset(1);
    options.lzma_options.dict_size = u32::try_from(BLOCK_SIZE).unwrap();
    options.set_member_size(NonZeroU64::new(BLOCK_SIZE as u64));
    let mut writer = LzipWriterMt::new(Vec::new(), options, 2).unwrap();
    writer.write_all(&payload(8 * BLOCK_SIZE)).unwrap();
    let mut compressed = writer.finish().unwrap();
    // Corrupt the final member's CRC, leaving its sizes intact for the scanner.
    let crc_offset = compressed.len() - 20;
    compressed[crc_offset] ^= 1;
    let mut reader = LzipReaderMt::new(Cursor::new(compressed), 2).unwrap();
    assert!(reader.read_to_end(&mut Vec::new()).is_err());
}

#[test]
fn shared_pool_xz_writer_round_trips_many_blocks() {
    let input = payload(32 * BLOCK_SIZE + 73);
    let mut options = XzOptions::with_preset(1);
    options.lzma_options.dict_size = u32::try_from(BLOCK_SIZE).unwrap();
    options.set_block_size(NonZeroU64::new(BLOCK_SIZE as u64));
    let mut writer = XzWriterMt::new(Vec::new(), options, 2).unwrap();
    write_fragments(&mut writer, &input);
    let compressed = writer.finish().unwrap();
    assert_eq!(
        decode(XzReaderMt::new(Cursor::new(compressed), false, 2).unwrap()),
        input
    );
}

struct FailingSink;

impl Write for FailingSink {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "injected output failure",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn output_failure_aborts_writer_and_rejects_further_input() {
    let mut writer = Lzma2WriterMt::new(FailingSink, options(), 2).unwrap();
    let error = writer.write_all(&payload(32 * BLOCK_SIZE)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(error.to_string(), "injected output failure");
    assert!(writer.write_all(b"more input").is_err());
    assert!(writer.flush().is_err());
    assert!(writer.finish().is_err());
}

struct FailingFlush;

impl Write for FailingFlush {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("injected flush failure"))
    }
}

#[test]
fn flush_failure_aborts_pending_work() {
    let mut writer = Lzma2WriterMt::new(FailingFlush, options(), 2).unwrap();
    writer.write_all(&payload(2 * BLOCK_SIZE)).unwrap();
    assert_eq!(
        writer.flush().unwrap_err().to_string(),
        "injected flush failure"
    );
    assert!(writer.write_all(b"more input").is_err());
    assert!(writer.finish().is_err());
}

struct GatedSink {
    bytes: Vec<u8>,
    ready: mpsc::Sender<()>,
    release: Option<mpsc::Receiver<()>>,
}

impl Write for GatedSink {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if let Some(release) = self.release.take() {
            self.ready.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        self.bytes.write(input)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn stalled_output_bounds_input_consumption_and_resumes_in_order() {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let consumed = Arc::new(AtomicUsize::new(0));
    let progress = Arc::clone(&consumed);
    let input = payload(32 * BLOCK_SIZE + 17);
    let expected = input.clone();
    let writer_thread = thread::spawn(move || {
        let sink = GatedSink {
            bytes: Vec::new(),
            ready: ready_tx,
            release: Some(release_rx),
        };
        let mut writer = Lzma2WriterMt::new(sink, options(), 2).unwrap();
        for chunk in input.chunks(BLOCK_SIZE) {
            progress.fetch_add(chunk.len(), Ordering::SeqCst);
            writer.write_all(chunk).unwrap();
        }
        writer.finish().unwrap().bytes
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // Two workers + one pending job + the producer chunk.
    let consumed_at_stall = consumed.load(Ordering::SeqCst);
    release_tx.send(()).unwrap();
    let compressed = writer_thread.join().unwrap();
    assert!(
        consumed_at_stall <= 4 * BLOCK_SIZE,
        "consumed {consumed_at_stall} bytes while output stalled"
    );
    assert_eq!(
        decode(Lzma2Reader::new(
            compressed.as_slice(),
            u32::try_from(BLOCK_SIZE).unwrap(),
            None
        )),
        expected
    );
}
