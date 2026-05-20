use std::io::Read;

use lzma_rust2::{
    Lzma2Options, Lzma2Reader, Lzma2ReaderMt, LzmaOptions, LzmaReader, LzmaWriter, XzReader,
};

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

/// Issue: lzma2_reader overflow-checks (attempt to add with overflow)
///
/// https://github.com/hasenbanck/lzma-rust2/issues/64
#[test]
fn issue_64() {
    let input = std::fs::read("tests/data/issue_64.bin").unwrap();

    let option = Lzma2Options::with_preset(0);
    let dict_size = option.lzma_options.dict_size;

    let mut uncompressed = Vec::new();

    let mut reader = Lzma2Reader::new(input.as_slice(), dict_size, None);
    let _ = reader.read_to_end(&mut uncompressed);
}

/// Issue: LZMA roundtrip fails with "dist overflow" when using preset dictionary
///
/// https://github.com/hasenbanck/lzma-rust2/issues/94
#[test]
fn issue_94() {
    let dict = b"section></summary><div class=</a></li".to_vec();
    let data = std::fs::read("tests/data/input.html").unwrap();

    let options = {
        let mut options = LzmaOptions::with_preset(9);
        options.preset_dict = Some(dict.clone());
        options
    };

    let output = std::io::Cursor::new(Vec::new());
    let mut encoder = LzmaWriter::new_no_header(output, &options, false).unwrap();
    std::io::copy(&mut std::io::Cursor::new(data.clone()), &mut encoder).unwrap();
    let compressed = encoder.finish().unwrap().into_inner();
    println!("Encode OK");

    let mut out = std::io::Cursor::new(Vec::new());
    let mut decoder = LzmaReader::new_with_props(
        compressed.as_slice(),
        data.len() as u64,
        options.get_props(),
        options.dict_size,
        options.preset_dict.as_deref(),
    )
    .unwrap();
    std::io::copy(&mut decoder, &mut out).unwrap();
    let decompressed = out.into_inner();
    println!("Decode OK");

    // We don't use assert_eq since the debug output would be too big.
    assert!(decompressed.as_slice() == data);
}
