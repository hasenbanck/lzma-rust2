# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.6.1 - UNRELEASED

### Fixed

- Fixed issue with the MT reader discovered by the downstream sevenz-rust2 crate because of incorrect chunk cuts
  (https://github.com/hasenbanck/sevenz-rust2/issues/44).

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
