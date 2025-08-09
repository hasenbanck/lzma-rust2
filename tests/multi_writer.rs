use std::{
    io::{Cursor, Read, Write},
    num::NonZeroU64,
};

use lzma_rust2::{LZIPOptions, LZIPReaderMT, LZIPWriter, LZMA2Options, LZMA2ReaderMT, LZMA2Writer};

static EXECUTABLE: &str = "tests/data/executable.exe";
const LEVEL: u32 = 3;

#[test]
fn multi_writer_lzma2() {
    let data = std::fs::read(EXECUTABLE).unwrap();

    let mut option = LZMA2Options::with_preset(LEVEL);
    let dict_size = option.lzma_options.dict_size;
    option.set_stream_size(NonZeroU64::new(dict_size as u64));

    let mut compressed = Vec::new();

    {
        let mut writer = LZMA2Writer::new(&mut compressed, &option);
        writer.write_all(&data).unwrap();
        writer.finish().unwrap();
    }

    let mut uncompressed = Vec::new();

    {
        let mut reader = LZMA2ReaderMT::new(Cursor::new(compressed), dict_size, None, 1);
        reader.read_to_end(&mut uncompressed).unwrap();
        assert!(reader.stream_count() > 1);
    }

    // We don't use assert_eq since the debug output would be too big.
    assert!(uncompressed.as_slice() == data);
}

#[test]
fn multi_writer_lzip2() {
    let data = std::fs::read(EXECUTABLE).unwrap();

    let mut option = LZIPOptions::with_preset(LEVEL);
    let dict_size = option.lzma_options.dict_size;
    option.set_block_size(NonZeroU64::new(dict_size as u64));

    let mut compressed = Vec::new();

    {
        let mut writer = LZIPWriter::new(&mut compressed, option);
        writer.write_all(&data).unwrap();
        writer.finish().unwrap();
    }

    let mut uncompressed = Vec::new();

    {
        let mut reader = LZIPReaderMT::new(Cursor::new(compressed), 1).unwrap();
        reader.read_to_end(&mut uncompressed).unwrap();
        assert!(reader.member_count() > 1);
    }

    // We don't use assert_eq since the debug output would be too big.
    assert!(uncompressed.as_slice() == data);
}

// #[test]
// fn multi_writer_xz() {
//     let data = std::fs::read(EXECUTABLE).unwrap();
//
//     let mut option = XZOptions::with_preset(LEVEL);
//     let dict_size = option.lzma_options.dict_size;
//     option.set_block_size(NonZeroU64::new(dict_size as u64));
//
//     let mut compressed = Vec::new();
//
//     {
//         let mut writer = XZWriter::new(&mut compressed, option).unwrap();
//         writer.write_all(&data).unwrap();
//         writer.finish().unwrap();
//     }
//
//     let mut uncompressed = Vec::new();
//
//     {
//         let mut reader = XZReaderMT::new(Cursor::new(compressed), 1).unwrap();
//         reader.read_to_end(&mut uncompressed).unwrap();
//         assert!(reader.block_count() > 1);
//     }
//
//     // We don't use assert_eq since the debug output would be too big.
//     assert!(uncompressed.as_slice() == data);
// }
