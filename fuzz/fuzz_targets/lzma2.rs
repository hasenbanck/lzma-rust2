#![no_main]

use std::io;

use libfuzzer_sys::fuzz_target;

use lzma_rust2::{Lzma2Reader, LzmaOptions};

fuzz_target!(|data: &[u8]| {
    let mut reader = Lzma2Reader::new(data, LzmaOptions::DICT_SIZE_DEFAULT, None);
    let _ = io::copy(&mut reader, &mut io::empty());
});
