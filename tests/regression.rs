use std::io::Read;

use lzma_rust2::LZMA2ReaderMT;

fn regression_lzma2_reader_mt(input_data: &[u8], expected_output: &[u8], dict_size: u32) {
    let mut uncompressed = Vec::new();

    {
        let mut reader = LZMA2ReaderMT::new(input_data, dict_size, None, 1);
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
