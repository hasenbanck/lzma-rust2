# LZMA / LZMA2 in native Rust

[![Crate](https://img.shields.io/crates/v/lzma-rust2.svg)](https://crates.io/crates/lzma-rust2)
[![Documentation](https://docs.rs/lzma-rust2/badge.svg)](https://docs.rs/lzma-rust2)

LZMA/LZMA2 codec ported from [tukaani xz for java](https://tukaani.org/xz/java.html).

This is a fork of the original, unmaintained lzma-rust crate to continue the development and maintenance.

## Performance

When compared against the `liblzma` crate, which uses the C library of the same name, this crate has improved decoding
speed (verified with both under aarch64 and x86_64). For example on a x86_64 system (AMD Ryzen 9950X3D) when
decompressing data compressed with level 6:

```
decompression/lzma/6    time:   [39.784 ms 39.854 ms 39.925 ms]
                        thrpt:  [134.69 MiB/s 134.92 MiB/s 135.16 MiB/s]
decompression/lzma2/6   time:   [38.167 ms 38.250 ms 38.334 ms]
                        thrpt:  [140.27 MiB/s 140.58 MiB/s 140.89 MiB/s]
decompression/liblzma/6 time:   [47.760 ms 47.853 ms 47.948 ms]
                        thrpt:  [112.15 MiB/s 112.37 MiB/s 112.59 MiB/s]
```

Encoding hasn't been optimized yet and is in general slower.

## Safety

Only the `asm` feature uses unsafe Rust, which is needed to use handwritten optimizations for a special function in
the hot loop. The speed penality of not using this handwritten ASM code is around 2% on aarch64 and 4% on x86_64.
The ASM code doesn't write into the memory, does properly bound the reading into the memory and is rather short in size,
so we are very confident that they are safe to use in general.

Deactivating the `asm` feature will result in 100% safe Rust code only.

## License

Licensed under the [Apache License, Version 2.0](https://www.apache.org/licenses/LICENSE-2.0).
