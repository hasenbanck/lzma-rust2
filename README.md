# LZMA / LZMA2 in native Rust

[![Crate](https://img.shields.io/crates/v/lzma-rust2.svg)](https://crates.io/crates/lzma-rust2)
[![Documentation](https://docs.rs/lzma-rust2/badge.svg)](https://docs.rs/lzma-rust2)

LZMA/LZMA2 codec ported from [tukaani xz for java](https://tukaani.org/xz/java.html).

This is a fork of the original, unmaintained lzma-rust crate to continue the development and maintenance.

## Safety

Only the `asm` feature uses unsafe Rust, which is needed to use handwritten optimizations for a special decoding
function in the hot loop. The speed penality of not using this handwritten ASM code when decoding is around 2% on
aarch64 and 4% on x86_64. The ASM code doesn't write into the memory, does properly bound the reading into the memory
and is rather short in size, so we are very confident that they are safe to use in general.

Deactivating the `asm` feature will result in 100% safe Rust code.

## Performance

The following part is strictly about single threaded performance. This crate doesn't expose a multithreaded API yet
to support decoding LZMA2's chunked stream in parallel yet.

When compared against the `liblzma` crate, which uses the C library of the same name, this crate has improved decoding
speed (verified with both under aarch64 and x86_64). For example on an x86_64 system (AMD Ryzen 9950X3D) when
decompressing data compressed with level 6:

```
decompression/lzma2/6   time:   [38.167 ms 38.250 ms 38.334 ms]
                        thrpt:  [140.27 MiB/s 140.58 MiB/s 140.89 MiB/s]
decompression/liblzma/6 time:   [47.760 ms 47.853 ms 47.948 ms]
                        thrpt:  [112.15 MiB/s 112.37 MiB/s 112.59 MiB/s]
```

Encoding hasn't been optimized yet and is in general slower. Level 0 and 1 have nearly identical performance, but
starting with level 2 the performance drops of when compared with `liblzma`:

```
```

## License

Licensed under the [Apache License, Version 2.0](https://www.apache.org/licenses/LICENSE-2.0).
