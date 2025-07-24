use std::{
    hint::black_box,
    io::{Read, Write},
};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use liblzma::{bufread::*, stream::*};
use lzma_rust2::{LZMA2Reader, LZMA2Writer, LZMAOptions, LZMAReader, LZMAWriter};

static TEST_DATA: &[u8] = include_bytes!("../tests/data/executable.exe");

fn bench_compression_lzma(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression lzma");
    group.throughput(Throughput::Bytes(TEST_DATA.len() as u64));
    group.sample_size(25);

    for level in 0..=9 {
        group.bench_with_input(
            BenchmarkId::new("lzma-rust2", level),
            &level,
            |b, &level| {
                let option = LZMAOptions::with_preset(level);

                b.iter(|| {
                    let mut compressed = Vec::new();
                    let mut writer =
                        LZMAWriter::new_no_header(black_box(&mut compressed), &option, true)
                            .unwrap();
                    writer.write_all(black_box(TEST_DATA)).unwrap();
                    writer.finish().unwrap();
                    black_box(compressed)
                });
            },
        );

        group.bench_with_input(BenchmarkId::new("liblzma", level), &level, |b, &level| {
            let option = LzmaOptions::new_preset(level).unwrap();
            b.iter(|| {
                let mut compressed = Vec::new();
                let stream = Stream::new_lzma_encoder(&option).unwrap();
                let mut encoder = XzEncoder::new_stream(black_box(TEST_DATA), stream);
                encoder.read_to_end(black_box(&mut compressed)).unwrap();
                black_box(compressed)
            });
        });
    }

    group.finish();
}

fn bench_compression_lzma2(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression lzma2");
    group.throughput(Throughput::Bytes(TEST_DATA.len() as u64));
    group.sample_size(25);

    for level in 0..=9 {
        group.bench_with_input(
            BenchmarkId::new("lzma-rust2", level),
            &level,
            |b, &level| {
                let option = LZMAOptions::with_preset(level);

                b.iter(|| {
                    let mut compressed = Vec::new();
                    let mut writer = LZMA2Writer::new(black_box(&mut compressed), &option);
                    writer.write_all(black_box(TEST_DATA)).unwrap();
                    writer.finish().unwrap();
                    black_box(compressed)
                });
            },
        );

        group.bench_with_input(BenchmarkId::new("liblzma", level), &level, |b, &level| {
            b.iter(|| {
                let mut compressed = Vec::new();
                let stream = Stream::new_easy_encoder(level, Check::None).unwrap();
                let mut encoder = XzEncoder::new_stream(black_box(TEST_DATA), stream);
                encoder.read_to_end(black_box(&mut compressed)).unwrap();
                black_box(compressed)
            });
        });
    }

    group.finish();
}

fn bench_decompression_lzma(c: &mut Criterion) {
    let mut group = c.benchmark_group("decompression lzma");
    group.throughput(Throughput::Bytes(TEST_DATA.len() as u64));
    group.sample_size(100);

    let mut lzma_data = Vec::new();
    let mut liblzma_data = Vec::new();

    for level in 0..=9 {
        {
            let option = LZMAOptions::with_preset(level);
            let mut compressed = Vec::new();
            let mut writer = LZMAWriter::new_no_header(&mut compressed, &option, true).unwrap();
            writer.write_all(TEST_DATA).unwrap();
            writer.finish().unwrap();
            lzma_data.push((compressed, option));
        }

        {
            let option = LzmaOptions::new_preset(level).unwrap();
            let mut compressed = Vec::new();
            let stream = Stream::new_lzma_encoder(&option).unwrap();
            let mut encoder = XzEncoder::new_stream(TEST_DATA, stream);
            encoder.read_to_end(black_box(&mut compressed)).unwrap();
            liblzma_data.push(compressed);
        }
    }

    for level in 0..=9 {
        group.bench_with_input(
            BenchmarkId::new("lzma-rust2", level),
            &lzma_data[level],
            |b, (compressed, option)| {
                b.iter(|| {
                    let mut uncompressed = Vec::new();
                    let mut reader = LZMAReader::new(
                        black_box(compressed.as_slice()),
                        TEST_DATA.len() as u64,
                        option.lc,
                        option.lp,
                        option.pb,
                        option.dict_size,
                        option.preset_dict.as_ref().map(|dict| dict.as_ref()),
                    )
                    .unwrap();
                    reader.read_to_end(black_box(&mut uncompressed)).unwrap();
                    black_box(uncompressed)
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("liblzma", level),
            &liblzma_data[level],
            |b, compressed| {
                b.iter(|| {
                    let mut uncompressed = Vec::new();
                    let stream = Stream::new_lzma_decoder(256 * 1024 * 1024).unwrap();
                    let mut r = XzDecoder::new_stream(black_box(compressed.as_slice()), stream);
                    r.read_to_end(black_box(&mut uncompressed)).unwrap();
                    black_box(uncompressed)
                });
            },
        );
    }

    group.finish();
}

fn bench_decompression_lzma2(c: &mut Criterion) {
    let mut group = c.benchmark_group("decompression lzma2");
    group.throughput(Throughput::Bytes(TEST_DATA.len() as u64));
    group.sample_size(100);

    let mut lzma2_data = Vec::new();
    let mut liblzma_data = Vec::new();

    for level in 0..=9 {
        let option = LZMAOptions::with_preset(level);
        {
            let mut compressed = Vec::new();
            let mut writer = LZMA2Writer::new(&mut compressed, &option);
            writer.write_all(TEST_DATA).unwrap();
            writer.finish().unwrap();
            lzma2_data.push((compressed, option));
        }

        {
            let mut compressed = Vec::new();
            let stream = Stream::new_easy_encoder(level, Check::None).unwrap();
            let mut encoder = XzEncoder::new_stream(TEST_DATA, stream);
            encoder.read_to_end(black_box(&mut compressed)).unwrap();
            liblzma_data.push(compressed);
        }
    }

    for level in 0..=9 {
        group.bench_with_input(
            BenchmarkId::new("lzma-rust2", level),
            &lzma2_data[level],
            |b, (compressed, option)| {
                b.iter(|| {
                    let mut uncompressed = Vec::new();
                    let mut reader =
                        LZMA2Reader::new(black_box(compressed.as_slice()), option.dict_size, None);
                    reader.read_to_end(black_box(&mut uncompressed)).unwrap();
                    black_box(uncompressed)
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("liblzma", level),
            &liblzma_data[level],
            |b, compressed| {
                b.iter(|| {
                    let mut uncompressed = Vec::new();
                    let mut r = XzDecoder::new(black_box(compressed.as_slice()));
                    r.read_to_end(black_box(&mut uncompressed)).unwrap();
                    black_box(uncompressed)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_compression_lzma,
    bench_compression_lzma2,
    bench_decompression_lzma,
    bench_decompression_lzma2,
);
criterion_main!(benches);
