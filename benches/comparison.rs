use std::{
    hint::black_box,
    io::{Read, Write},
};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use lzma_rust2::{LZMA2Options, LZMA2Reader, LZMA2Writer, LZMAReader, LZMAWriter};

static PG100: &str = include_str!("../tests/data/pg100.txt");

fn bench_compression(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression");
    group.throughput(Throughput::Bytes(PG100.len() as u64));

    let text_bytes = PG100.as_bytes();

    for level in 0..=9 {
        group.bench_with_input(BenchmarkId::new("lzma", level), &level, |b, &level| {
            let option = LZMA2Options::with_preset(level);

            b.iter(|| {
                let mut compressed = Vec::new();
                let mut writer = LZMAWriter::new_no_header(&mut compressed, &option, true).unwrap();
                writer.write_all(black_box(text_bytes)).unwrap();
                writer.finish().unwrap();
                black_box(compressed)
            });
        });

        group.bench_with_input(BenchmarkId::new("lzma2", level), &level, |b, &level| {
            let option = LZMA2Options::with_preset(level);

            b.iter(|| {
                let mut compressed = Vec::new();
                let mut writer = LZMA2Writer::new(&mut compressed, &option);
                writer.write_all(black_box(text_bytes)).unwrap();
                writer.finish().unwrap();
                black_box(compressed)
            });
        });

        group.bench_with_input(BenchmarkId::new("liblzma", level), &level, |b, &level| {
            b.iter(|| {
                let compressed = liblzma::encode_all(black_box(text_bytes), level).unwrap();
                black_box(compressed)
            });
        });
    }

    group.finish();
}

fn bench_decompression(c: &mut Criterion) {
    let mut group = c.benchmark_group("decompression");
    group.throughput(Throughput::Bytes(PG100.len() as u64));
    group.sample_size(500);

    let mut lzma_data = Vec::new();
    let mut lzma2_data = Vec::new();
    let mut liblzma_data = Vec::new();

    for level in 0..=9 {
        let option = LZMA2Options::with_preset(level);

        {
            let mut compressed = Vec::new();
            let mut writer = LZMAWriter::new_no_header(&mut compressed, &option, true).unwrap();
            writer.write_all(PG100.as_bytes()).unwrap();
            writer.finish().unwrap();
            lzma_data.push((level, compressed, option.clone()));
        }

        {
            let mut compressed = Vec::new();
            let mut writer = LZMA2Writer::new(&mut compressed, &option);
            writer.write_all(PG100.as_bytes()).unwrap();
            writer.finish().unwrap();
            lzma2_data.push((level, compressed, option.clone()));
        }

        {
            let compressed = liblzma::encode_all(PG100.as_bytes(), level).unwrap();
            liblzma_data.push((level, compressed));
        }
    }

    for (level, compressed, option) in lzma_data {
        group.bench_with_input(
            BenchmarkId::new("lzma", level),
            &(compressed, option),
            |b, (compressed, option)| {
                b.iter(|| {
                    let mut uncompressed = Vec::new();
                    let mut reader = LZMAReader::new(
                        black_box(compressed.as_slice()),
                        PG100.len() as u64,
                        option.lc,
                        option.lp,
                        option.pb,
                        option.dict_size,
                        option.preset_dict.as_ref().map(|dict| dict.as_ref()),
                    )
                    .unwrap();
                    reader.read_to_end(&mut uncompressed).unwrap();
                    black_box(uncompressed)
                });
            },
        );
    }

    for (level, compressed, option) in lzma2_data {
        group.bench_with_input(
            BenchmarkId::new("lzma2", level),
            &(compressed, option),
            |b, (compressed, option)| {
                b.iter(|| {
                    let mut uncompressed = Vec::new();
                    let mut reader =
                        LZMA2Reader::new(black_box(compressed.as_slice()), option.dict_size, None);
                    reader.read_to_end(&mut uncompressed).unwrap();
                    black_box(uncompressed)
                });
            },
        );
    }

    for (level, compressed) in liblzma_data {
        group.bench_with_input(
            BenchmarkId::new("liblzma", level),
            &compressed,
            |b, compressed| {
                b.iter(|| {
                    let uncompressed =
                        liblzma::decode_all(black_box(compressed.as_slice())).unwrap();
                    black_box(uncompressed)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_compression, bench_decompression);
criterion_main!(benches);
