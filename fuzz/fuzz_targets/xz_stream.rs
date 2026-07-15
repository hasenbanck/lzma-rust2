#![no_main]

use libfuzzer_sys::fuzz_target;
use lzma_rust2::{Action, Status, XzStream};

fuzz_target!(|data: &[u8]| {
    let mut decoder = XzStream::new(false);
    let mut output_buf = [0u8; 4096];
    let mut in_pos = 0;

    loop {
        let action = if in_pos >= data.len() {
            Action::Finish
        } else {
            Action::Run
        };
        let result = match decoder.process(&data[in_pos..], &mut output_buf, action) {
            Ok(r) => r,
            Err(_) => return,
        };
        in_pos += result.bytes_consumed;
        if result.status == Status::StreamEnd {
            return;
        }
        if result.bytes_consumed == 0 && result.bytes_produced == 0 {
            return;
        }
    }
});
