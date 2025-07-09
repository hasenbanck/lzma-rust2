# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.0 - unreleases

### Updated

- Increased the decoding performance without using unsafe code. On aarch64 (Apple Silicon M4) we are
  faster than liblzma. On x86-64 liblzma is faster, since it has custom ASM code for x86-64.
- Removed LZMACoder from public interface.
- Add EncodeMode and MFType enums to public interface.

### Removed

- Remove byteorder dependency.

## 0.2.2 - 2025-06-28

### Updated

- No functional updated.
- Moved into its own repository.

## 0.2.1 - 2025-05-01

### Fixed

- Fix integer overflow when decompressing uncompressed files over u32::MAX
- Allow all byteorder versions with major release 1
