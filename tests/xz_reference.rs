use std::io::{Cursor, Read};

use lzma_rust2::XZReader;

#[test]
fn executable_lzma2_blocks_1() {
    let original = std::fs::read("tests/data/executable.exe").unwrap();
    let compressed = std::fs::read("tests/data/executable.exe_lzma2_blocks_1.xz").unwrap();

    let mut reader = XZReader::new(Cursor::new(compressed));

    let mut uncompressed = Vec::with_capacity(original.len());
    let count = reader.read_to_end(&mut uncompressed).unwrap();

    assert_eq!(count, original.len());
    assert!(original == uncompressed);
}

#[test]
fn executable_lzma2_blocks_4() {
    let original = std::fs::read("tests/data/executable.exe").unwrap();
    let compressed = std::fs::read("tests/data/executable.exe_lzma2_blocks_4.xz").unwrap();

    let mut reader = XZReader::new(Cursor::new(compressed));

    let mut uncompressed = Vec::with_capacity(original.len());
    let count = reader.read_to_end(&mut uncompressed).unwrap();

    assert_eq!(count, original.len());
    assert!(original == uncompressed);
}
