use std::io::Read;

use lzma_rust2::{Lzma2ReaderMt, XzReader};

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
