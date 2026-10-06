#![cfg(all(feature = "std", feature = "encoder"))]

use std::{
    io::Write,
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use lzma_rust2::{EncoderCancelled, Lzma2Options, Lzma2WriterMt};

const BLOCK_SIZE: usize = 4096;

#[test]
fn cancellation_rejects_encoder_input_and_finishing_pending_work() {
    for dispatched in [false, true] {
        let flag = Arc::new(AtomicBool::new(false));
        let mut writer = Lzma2WriterMt::new(Vec::new(), options(), 2).unwrap();
        writer.set_cancellation(Arc::clone(&flag));
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
