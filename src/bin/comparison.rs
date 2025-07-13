use std::io::{Read, Write};

use liblzma::{
    bufread::XzEncoder,
    stream::{Check, Stream},
};
use lzma_rust2::{LZMA2Options, LZMA2Writer, LZMAWriter};

static PG100: &str = include_str!("../../tests/data/pg100.txt");

fn compress_lzma(data: &[u8], level: u32) -> Vec<u8> {
    let option = LZMA2Options::with_preset(level);
    let mut compressed = Vec::new();
    // Using new_no_header to be closer to a raw LZMA stream
    let mut writer = LZMAWriter::new_no_header(&mut compressed, &option, true).unwrap();
    writer.write_all(data).unwrap();
    writer.finish().unwrap();
    compressed
}
fn compress_lzma2(data: &[u8], level: u32) -> Vec<u8> {
    let option = LZMA2Options::with_preset(level);
    let mut compressed = Vec::new();
    let mut writer = LZMA2Writer::new(&mut compressed, &option);
    writer.write_all(data).unwrap();
    writer.finish().unwrap();
    compressed
}

fn compress_liblzma(data: &[u8], level: u32) -> Vec<u8> {
    let mut compressed = Vec::new();
    let stream = Stream::new_easy_encoder(level, Check::None).unwrap();
    let mut encoder = XzEncoder::new_stream(data, stream);
    encoder.read_to_end(&mut compressed).unwrap();
    compressed
}

fn main() {
    let text_bytes = PG100.as_bytes();
    let original_size = text_bytes.len();

    println!("Comparing compression rates for a {original_size} byte file (pg100.txt)\n");
    println!(
        "{:<6} | {:<12} | {:<12} | {:<12} | {:<20} | {:<20}",
        "Level",
        "liblzma (B)",
        "lzma (B)",
        "lzma2 (B)",
        "% better than liblzma",
        "% better than liblzma"
    );
    println!(
        "{:<6} | {:<12} | {:<12} | {:<12} | {:<20} | {:<20}",
        "", "", "", "", "(lzma)", "(lzma2)"
    );
    println!("{:-<105}", "");

    for level in 0..=9 {
        let liblzma_compressed = compress_liblzma(text_bytes, level);
        let lzma_compressed = compress_lzma(text_bytes, level);
        let lzma2_compressed = compress_lzma2(text_bytes, level);

        let liblzma_size = liblzma_compressed.len() as f64;
        let lzma_size = lzma_compressed.len() as f64;
        let lzma2_size = lzma2_compressed.len() as f64;

        let lzma_improvement = (1.0 - lzma_size / liblzma_size) * 100.0;
        let lzma2_improvement = (1.0 - lzma2_size / liblzma_size) * 100.0;

        println!(
            "{:<6} | {:<12} | {:<12} | {:<12} | {:<+20.2}% | {:<+20.2}%",
            level,
            liblzma_size as usize,
            lzma_size as usize,
            lzma2_size as usize,
            lzma_improvement,
            lzma2_improvement
        );
    }
}
