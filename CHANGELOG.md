# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
