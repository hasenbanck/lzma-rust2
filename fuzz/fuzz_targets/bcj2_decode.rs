#![no_main]

use std::io::Read;

use libfuzzer_sys::fuzz_target;
use lzma_rust2::filter::bcj2::Bcj2Reader;

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 || data.len() > 65536 {
        return;
    }
    let size = u16::from_le_bytes([data[0], data[1]]) as u64;
    let mut offset = 8;
    let mut streams = Vec::new();
    for i in 0..3 {
        let length = u16::from_le_bytes([data[2 + i * 2], data[3 + i * 2]]) as usize;
        let end = (offset + length).min(data.len());
        streams.push(&data[offset..end]);
        offset = end;
    }
    streams.push(&data[offset..]);
    let mut reader = Bcj2Reader::new(streams, size);
    let mut output_size = 0;
    let mut buf = [0; 31];
    while let Ok(count) = reader.read(&mut buf) {
        if count == 0 {
            break;
        }
        output_size += count as u64;
        assert!(output_size <= size);
    }
    let _ = reader.finish();
});
