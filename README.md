# LZMA / LZMA2 in native Rust

[![Crate](https://img.shields.io/crates/v/lzma-rust2.svg)](https://crates.io/crates/lzma-rust2)
[![Documentation](https://docs.rs/lzma-rust2/badge.svg)](https://docs.rs/lzma-rust2)

LZMA/LZMA2 codec ported from [tukaani xz for java](https://tukaani.org/xz/java.html).

This is a fork of the original, unmaintained lzma-rust crate to continue the development and maintenance.

## Safety

Only the `optimization` feature uses unsafe Rust features to implement optimizations, that are
not possible in safe Rust. Those optimizations are properly guarded and are of course sound.
This includes creation of aligned memory, handwritten assembly code for hot functions and some
pointer logic. Those optimization are well localized and generally consider safe to use, even
with untrusted input.

Deactivating the `optimization` feature will result in 100% standard Rust code.

## Performance

The following part is strictly about single threaded performance. This crate doesn't expose a multithreaded API yet
to support compression or decompressing LZMA2's chunked stream in parallel yet.

When compared against the `liblzma` crate, which uses the C library of the same name, this crate has improved decoding
speed.

![Decompression Speed LZMA2](./assets/decompression_lzma2.svg)
![Decompression Speed LZMA](./assets/decompression_lzma.svg)

Encoding is also well optimized and is surpassing `liblzma` for level 0 to 3 and matches it for level 4 to 9.

![Compression Speed LZMA2](./assets/compression_lzma2.svg)
![Compression Speed LZMA](./assets/compression_lzma.svg)

Data was assembled using lzma-rust2 v0.4.0 and liblzma v0.4.2.

## License

Licensed under the [Apache License, Version 2.0](https://www.apache.org/licenses/LICENSE-2.0).
