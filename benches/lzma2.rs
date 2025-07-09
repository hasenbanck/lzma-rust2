use std::{
    hint::black_box,
    io::{Read, Write},
};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use lzma_rust2::{LZMA2Options, LZMA2Reader, LZMA2Writer};

static APACHE2: &str = include_str!("../tests/data/apache2.txt");

fn bench_encoder(c: &mut Criterion) {
    let mut group = c.benchmark_group("lzma2_encoder");
    group.throughput(Throughput::Bytes(APACHE2.len() as u64));

    for level in 0..=9 {
        group.bench_with_input(BenchmarkId::new("apache2", level), &level, |b, &level| {
            let option = LZMA2Options::with_preset(level);
            let text_bytes = APACHE2.as_bytes();

            b.iter(|| {
                let mut compressed = Vec::new();
                let mut writer = LZMA2Writer::new(&mut compressed, &option);
                writer.write_all(black_box(text_bytes)).unwrap();
                writer.finish().unwrap();
                black_box(compressed)
            });
        });
    }

    group.finish();
}

fn bench_decoder(c: &mut Criterion) {
    let mut group = c.benchmark_group("lzma2_decoder");
    group.throughput(Throughput::Bytes(APACHE2.len() as u64));

    let mut compressed_data = Vec::new();

    for level in 0..=9 {
        let option = LZMA2Options::with_preset(level);
        let mut compressed = Vec::new();

        {
            let mut writer = LZMA2Writer::new(&mut compressed, &option);
            writer.write_all(APACHE2.as_bytes()).unwrap();
            writer.finish().unwrap();
        }

        compressed_data.push((level, compressed, option.dict_size));
    }

    for (level, compressed, dict_size) in compressed_data {
        group.bench_with_input(
            BenchmarkId::new("apache2", level),
            &(compressed, dict_size),
            |b, (compressed, dict_size)| {
                b.iter(|| {
                    let mut uncompressed = Vec::new();
                    let mut reader =
                        LZMA2Reader::new(black_box(compressed.as_slice()), *dict_size, None);
                    reader.read_to_end(&mut uncompressed).unwrap();
                    black_box(uncompressed)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_encoder, bench_decoder);
criterion_main!(benches);
