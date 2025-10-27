#![no_main]

use std::io;

use libfuzzer_sys::fuzz_target;

use lzma_rust2::XzReader;

fuzz_target!(|data: &[u8]| {
    let mut reader = XzReader::new(data, true);
    let _ = io::copy(&mut reader, &mut io::empty());
});
