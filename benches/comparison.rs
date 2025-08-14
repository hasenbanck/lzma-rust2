use std::{
    hint::black_box,
    io::{Cursor, Read, Write},
    num::NonZeroU64,
};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use liblzma::{bufread::*, stream::*};
use lzma_rust2::{
    LZIPOptions, LZIPReaderMT, LZIPWriter, LZIPWriterMT, LZMA2Options, LZMA2Reader, LZMA2ReaderMT,
    LZMA2Writer, LZMA2WriterMT, LZMAOptions, LZMAReader, LZMAWriter, XZOptions, XZReaderMT,
    XZWriter, XZWriterMT,
};

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
                b.iter(|| {
                    let mut compressed = Vec::new();
                    let option = LZMA2Options::with_preset(level);
                    let mut writer = LZMA2Writer::new(black_box(&mut compressed), option);
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
        let option = LZMA2Options::with_preset(level);
        {
            let mut compressed = Vec::new();
            let mut writer = LZMA2Writer::new(&mut compressed, option.clone());
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
                    let mut reader = LZMA2Reader::new(
                        black_box(compressed.as_slice()),
                        option.lzma_options.dict_size,
                        None,
                    );
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

fn bench_compression_mt(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression mt");
    group.throughput(Throughput::Bytes(TEST_DATA.len() as u64));
    group.sample_size(25);

    let num_workers = std::thread::available_parallelism().unwrap().get() as u32;

    group.bench_function(BenchmarkId::new("lzma2", 3), |b| {
        let mut option = LZMA2Options::with_preset(3);
        option.set_chunk_size(NonZeroU64::new(option.lzma_options.dict_size as u64));

        b.iter(|| {
            let mut compressed = Vec::new();
            let mut writer =
                LZMA2WriterMT::new(black_box(&mut compressed), option.clone(), num_workers)
                    .unwrap();
            writer.write_all(black_box(TEST_DATA)).unwrap();
            writer.finish().unwrap();
            black_box(compressed)
        });
    });

    group.bench_function(BenchmarkId::new("lzip", 3), |b| {
        let mut option = LZIPOptions::with_preset(3);
        option.set_member_size(NonZeroU64::new(option.lzma_options.dict_size as u64));

        b.iter(|| {
            let mut compressed = Vec::new();
            let mut writer =
                LZIPWriterMT::new(black_box(&mut compressed), option.clone(), num_workers).unwrap();
            writer.write_all(black_box(TEST_DATA)).unwrap();
            writer.finish().unwrap();
            black_box(compressed)
        });
    });

    group.bench_function(BenchmarkId::new("xz", 3), |b| {
        let mut option = XZOptions::with_preset(3);
        option.set_block_size(NonZeroU64::new(option.lzma_options.dict_size as u64));

        b.iter(|| {
            let mut compressed = Vec::new();
            let mut writer =
                XZWriterMT::new(black_box(&mut compressed), option.clone(), num_workers).unwrap();
            writer.write_all(black_box(TEST_DATA)).unwrap();
            writer.finish().unwrap();
            black_box(compressed)
        });
    });

    group.finish();
}

fn bench_decompression_mt(c: &mut Criterion) {
    let mut group = c.benchmark_group("decompression mt");
    group.throughput(Throughput::Bytes(TEST_DATA.len() as u64));
    group.sample_size(100);

    let num_workers = std::thread::available_parallelism().unwrap().get() as u32;

    let mut lzma2_option = LZMA2Options::with_preset(3);
    lzma2_option.set_chunk_size(NonZeroU64::new(lzma2_option.lzma_options.dict_size as u64));
    let mut lzma2_data = Vec::new();
    let mut writer = LZMA2Writer::new(&mut lzma2_data, lzma2_option.clone());
    writer.write_all(TEST_DATA).unwrap();
    writer.finish().unwrap();

    let mut lzip_option = LZIPOptions::with_preset(3);
    lzip_option.set_member_size(NonZeroU64::new(lzip_option.lzma_options.dict_size as u64));
    let mut lzip_data = Vec::new();
    let mut writer = LZIPWriter::new(&mut lzip_data, lzip_option.clone());
    writer.write_all(TEST_DATA).unwrap();
    writer.finish().unwrap();

    let mut xz_option = XZOptions::with_preset(3);
    xz_option.set_block_size(NonZeroU64::new(xz_option.lzma_options.dict_size as u64));
    let mut xz_data = Vec::new();
    let mut writer = XZWriter::new(&mut xz_data, xz_option.clone()).unwrap();
    writer.write_all(TEST_DATA).unwrap();
    writer.finish().unwrap();

    group.bench_function(BenchmarkId::new("lzma2", 3), |b| {
        b.iter(|| {
            let mut uncompressed = Vec::new();
            let mut reader = LZMA2ReaderMT::new(
                black_box(lzma2_data.as_slice()),
                lzma2_option.lzma_options.dict_size,
                None,
                num_workers,
            );
            reader.read_to_end(black_box(&mut uncompressed)).unwrap();
            black_box(uncompressed)
        });
    });

    group.bench_function(BenchmarkId::new("lzip", 3), |b| {
        b.iter(|| {
            let mut uncompressed = Vec::new();
            let mut reader =
                LZIPReaderMT::new(black_box(Cursor::new(lzip_data.as_slice())), num_workers)
                    .unwrap();
            reader.read_to_end(black_box(&mut uncompressed)).unwrap();
            black_box(uncompressed)
        });
    });

    group.bench_function(BenchmarkId::new("xz", 3), |b| {
        b.iter(|| {
            let mut uncompressed = Vec::new();
            let mut reader = XZReaderMT::new(
                black_box(Cursor::new(xz_data.as_slice())),
                false,
                num_workers,
            )
            .unwrap();
            reader.read_to_end(black_box(&mut uncompressed)).unwrap();
            black_box(uncompressed)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_compression_lzma,
    bench_compression_lzma2,
    bench_compression_mt,
    bench_decompression_lzma,
    bench_decompression_lzma2,
    bench_decompression_mt,
);
criterion_main!(benches);
