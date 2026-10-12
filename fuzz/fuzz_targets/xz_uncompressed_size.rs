#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use lzma_rust2::xz_uncompressed_size;

fuzz_target!(|data: &[u8]| {
    let _ = xz_uncompressed_size(Cursor::new(data));
});
