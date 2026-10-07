#![cfg(all(feature = "std", feature = "encoder"))]

use std::{
    io::{self, Read, Write},
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use lzma_rust2::{EncoderCancelled, Lzma2Options, Lzma2Reader, Lzma2WriterMt};

const BLOCK_SIZE: usize = 4096;

#[test]
fn cancellation_rejects_encoder_input_and_finishing_pending_work() {
    for dispatched in [false, true] {
        let flag = Arc::new(AtomicBool::new(false));
        let mut writer = Lzma2WriterMt::new(Vec::new(), options(), 2).unwrap();
        writer.set_cancellation(Arc::clone(&flag)).unwrap();
        if dispatched {
            writer.write_all(&payload(BLOCK_SIZE * 4)).unwrap();
        }
        flag.store(true, Ordering::Relaxed);
        let error = if dispatched {
            writer.finish().unwrap_err()
        } else {
            writer.write(b"input").unwrap_err()
        };
        assert!(error.get_ref().unwrap().is::<EncoderCancelled>());
    }
}

#[test]
fn cancellation_flag_cannot_change_after_input_is_buffered_or_dispatched() {
    for size in [1, BLOCK_SIZE] {
        let flag = Arc::new(AtomicBool::new(false));
        let mut writer = Lzma2WriterMt::new(Vec::new(), options(), 1).unwrap();
        writer.set_cancellation(Arc::clone(&flag)).unwrap();
        writer.write_all(&payload(size)).unwrap();
        let error = writer
            .set_cancellation(Arc::new(AtomicBool::new(false)))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        flag.store(true, Ordering::Relaxed);
        let error = writer.finish().unwrap_err();
        assert!(error.get_ref().unwrap().is::<EncoderCancelled>());
    }
}

#[test]
fn cancellation_flag_can_change_before_writing_input() {
    let old_flag = Arc::new(AtomicBool::new(true));
    let flag = Arc::new(AtomicBool::new(false));
    let mut writer = Lzma2WriterMt::new(Vec::new(), options(), 1).unwrap();
    writer.set_cancellation(old_flag).unwrap();
    writer.set_cancellation(flag).unwrap();
    let input = payload(BLOCK_SIZE * 4);
    writer.write_all(&input).unwrap();
    let encoded = writer.finish().unwrap();
    let mut decoded = Vec::new();
    Lzma2Reader::new(encoded.as_slice(), BLOCK_SIZE as u32, None)
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn cancellation_flag_cannot_be_installed_after_writing_input() {
    for size in [1, BLOCK_SIZE] {
        let input = payload(size);
        let mut writer = Lzma2WriterMt::new(Vec::new(), options(), 1).unwrap();
        writer.write_all(&input).unwrap();
        let error = writer
            .set_cancellation(Arc::new(AtomicBool::new(true)))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        let encoded = writer.finish().unwrap();
        let mut decoded = Vec::new();
        Lzma2Reader::new(encoded.as_slice(), BLOCK_SIZE as u32, None)
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, input);
    }
}

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
