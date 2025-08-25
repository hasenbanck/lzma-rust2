# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.10.1 - 2025-08-25

### Fixed

- Fix broken BCJWriter that couldn't properly finish it's encoding process. Now has a proper finish() function.

## 0.10.0 - 2025-08-22

### Fixed

- Add missing "inner()" and "inner_mut()" function to the XZ and LZIP reader and writer.

## 0.9.0 - 2025-08-15

### Changed

- `XZReader` and `XZWriter` are now Send.
- Most reader and writer now have "inner()" and "inner_mut()" functions.

### Updated

- Internally refactoring to reduce code duplication for multi-threading code.

## 0.8.2 - 2025-08-13

### Fixed

- Make sure that LZMA reader errors when going out of bound. This could happen if no EOS was found.
  This was not a memory a safety issue, but instead a problem in how we propagated a fail state in light
  of certain optimizations.
- Bound the multithreaded reader as to not use too much memory.

## 0.8.1 - 2025-08-13

### Fixed

- Internally we updated the hash function for the match finders to use a golden ratio based hash instead of the old
  CRC table based hash. In our test data this was a net win, but it turned out, once tested with bigger datasets,
  this is a net loss. So we returned back to the CRC table approach (we speak here about a change below 0.01%, but
  measurable).

## 0.8.0 - 2025-08-10

### Added

- Added single threaded and multithreaded encoder and decoder for the LZIP file format.
- Added multithreaded encoder and decoder for the XZ file format.

### Changed

- Renamed LZMA2's "independent work unit" naming from "stream" to "chunk" to not confuse it with XZ streams.
- LZMA2Writer now take a LZMA2Option struct. This enables both the LZMA2Writer and LZMA2WriterMT to encode multiple
  chunks for multithreaded decoding.
- Changed block size of XZOptions to NonZero type.
- Unified the API of the writers as far as possible.

### Fixed

- Fixed unbounded spawning of threads when using the multithreaded version of LZMA2 encoder & decoder.
- Fixed performance regression on linux as reported by @chenxiaolong (#10)
- Fixed compatibility with liblzma when creating XZ files as reported by @chenxiaolong (#14)
- XZWriter properly writes out multiple blocks respecting the block_size.

## 0.7.0 - 2025-08-08

### Added

- Added single threaded encoder and decoder for the XZ file format.
- Ported all pre-filters used by both 7zip and XZ.

## 0.6.1 - 2025-08-03

### Fixed

- Fixed issue with the MT reader discovered by the downstream sevenz-rust2 crate because of incorrect chunk cuts
  (https://github.com/hasenbanck/sevenz-rust2/issues/44).

### Updated

- The multithreading now works more efficient, since new threads are only spawned if really needed.

## 0.6.0 - 2025-07-26

### Added

- Added no_std support by disabling the new `std` feature that is enabled by default. Custom traits and default
  implementation for &[u8] and &mut [u8] are provided.

## 0.5.1 - 2025-07-25

### Fixed

- Fixed possible deadlocks in the multithreaded encoder and decoder.

## 0.5.0 - 2025-07-24

### Added

- Added multithreaded compression for LZMA2.
- Added multithreaded decompression for LZMA2.

### Updated

- Renamed LZMA2Options to LZMAOptions, since it described the way we encode the LZMA encoder, which is shared between
  LZMA and LZMA2.

## 0.4.0 - 2025-07-16

### Updated

- Increased the encoding performance. For level 0-3 this crate now is faster than lzma.
  For 4-9 this crate is on same level with liblzma.

### Changed

- Feature "asm" changed to "optimization" and is also enabled by default.
  Have a look at the "Safety" section of the README.md for more details.

## 0.3.1 - 2025-07-12

### Fixed

- No functional changes.
- Fixed the links to the repository.

## 0.3.0 - 2025-07-12

### Updated

- Increased MSRV to v1.85
- Increased the decoding performance while using only safe Rust. On x86-64 the speed-up
  was quite large when compared to the v0.2 branch (+50% throughput).
  Have a look at the "Performance" section of the README.md for more details.
- Added feature flag "asm" which is activated at default which increases the
  decoding speed when using LZMA2.
  Have a look at the "Safety" section of the README.md for more details.
- Add EncodeMode and MFType enums to public interface (used for the encoder options).

### Removed

- Remove byteorder dependency.
- Remove internal types from public interface.

## 0.2.2 - 2025-06-28

### Updated

- No functional updated.
- Moved into its own repository.

## 0.2.1 - 2025-05-01

### Fixed

- Fix integer overflow when decompressing uncompressed files over u32::MAX
- Allow all byteorder versions with major release 1
