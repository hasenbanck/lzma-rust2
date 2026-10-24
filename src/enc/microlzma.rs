use alloc::vec::Vec;

use super::{
    LzmaOptions,
    encoder::{EncodeMode, LzmaEncoder, LzmaEncoderModes},
    encoder_fast::FastEncoderMode,
    encoder_normal::NormalEncoderMode,
    range_enc::RangeEncoder,
};
use crate::{MATCH_LEN_MAX, MfType, error_invalid_input, lz::get_buf_size};

/// A reusable encoder for independent MicroLZMA streams.
///
/// Each call resets history while retaining encoder allocations. The dictionary size
/// and uncompressed size are not stored in the stream and must be supplied to the decoder.
pub struct MicroLzmaEncoder {
    lzma: LzmaEncoder,
    mode: LzmaEncoderModes,
    rc: RangeEncoder<Vec<u8>>,
    props: u8,
}

impl MicroLzmaEncoder {
    /// Creates an encoder using the supplied LZMA options.
    ///
    /// Preset dictionaries are not supported for independent streams.
    /// Invalid options or dictionary windows and match-finder indices that exceed
    /// the encoder's signed position range are rejected.
    pub fn new(options: &LzmaOptions) -> crate::Result<Self> {
        let (before, after) = match options.mode {
            EncodeMode::Fast => (
                FastEncoderMode::EXTRA_SIZE_BEFORE,
                FastEncoderMode::EXTRA_SIZE_AFTER,
            ),
            EncodeMode::Normal => (
                NormalEncoderMode::EXTRA_SIZE_BEFORE,
                NormalEncoderMode::EXTRA_SIZE_AFTER,
            ),
        };
        let max_dict_size = match options.mf {
            MfType::Hc4 => i32::MAX as u32,
            // Bt4 uses two signed tree indices per dictionary slot.
            MfType::Bt4 => (i32::MAX as u32 - 1) / 2,
        };
        if options.dict_size < 4096
            || options.dict_size > max_dict_size
            || get_buf_size(options.dict_size, before, after, MATCH_LEN_MAX as u32)
                > i32::MAX as u32
        {
            return Err(error_invalid_input("invalid dictionary size"));
        }
        if options.lc > 8
            || options.lp > 4
            || options.pb > 4
            || !(LzmaOptions::NICE_LEN_MIN..=LzmaOptions::NICE_LEN_MAX).contains(&options.nice_len)
            || options.depth_limit < 0
        {
            return Err(error_invalid_input("invalid LZMA options"));
        }
        if options
            .preset_dict
            .as_ref()
            .is_some_and(|dict| !dict.is_empty())
        {
            return Err(error_invalid_input("preset dictionary is not supported"));
        }
        let (lzma, mode) = LzmaEncoder::new(
            options.mode,
            options.lc,
            options.lp,
            options.pb,
            options.mf,
            options.depth_limit,
            options.dict_size,
            options.nice_len as usize,
        );
        Ok(Self {
            lzma,
            mode,
            rc: RangeEncoder::new(Vec::new()),
            props: options.get_props(),
        })
    }

    /// Encodes an independently decodable prefix, returning (consumed, written).
    ///
    /// If the complete stream fits, the entire input is consumed.
    /// `output` must contain at least five bytes for range-coder termination.
    /// Zero input consumption is possible when only an empty stream fits.
    pub fn compress_prefix(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> crate::Result<(usize, usize)> {
        if output.len() < 5 {
            return Err(error_invalid_input("output needs at least five bytes"));
        }
        self.lzma.reset_stream(&mut self.mode);
        self.rc.reset();
        self.rc.inner_mut().clear();
        let mut offset = 0usize;
        let mut consumed = 0;
        loop {
            let end = input
                .len()
                .min(offset.saturating_add(self.lzma.lz.buf_size));
            offset += self.lzma.lz.fill_window(&input[offset..end]);
            if offset == input.len() {
                self.lzma.lz.set_finishing();
            }
            let stopped = self
                .lzma
                .encode_for_limit(&mut self.rc, &mut self.mode, output.len())?;
            consumed += self.lzma.data.uncompressed_size as usize;
            self.lzma.reset_uncompressed_size();
            if stopped || offset == input.len() {
                break;
            }
        }
        self.rc.finish()?;
        let encoded = self.rc.inner();
        let target = &mut output[..encoded.len()];
        target.copy_from_slice(encoded);
        target[0] = !self.props;
        Ok((consumed, target.len()))
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::{LzmaReader, LzmaWriter, MfType, Read, Write};

    fn check_stream(encoded: &[u8], expected: &[u8], dict_size: u32) {
        let mut raw = encoded.to_vec();
        let props = !raw[0];
        raw[0] = 0;
        let mut reader = LzmaReader::new_with_props(
            raw.as_slice(),
            expected.len() as u64,
            props,
            dict_size,
            None,
        )
        .unwrap();
        let mut decoded = Vec::new();
        reader.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, expected);
        assert!(reader.into_inner().is_empty());

        let mut alone = vec![props];
        alone.extend_from_slice(&dict_size.to_le_bytes());
        alone.extend_from_slice(&(expected.len() as u64).to_le_bytes());
        alone.extend_from_slice(&raw);
        let mut native = liblzma::stream::Stream::new_lzma_decoder(u64::MAX).unwrap();
        let mut decoded = vec![0; expected.len() + 1];
        assert_eq!(
            native
                .process(&alone, &mut decoded, liblzma::stream::Action::Finish)
                .unwrap(),
            liblzma::stream::Status::StreamEnd,
        );
        assert_eq!(native.total_in() as usize, alone.len());
        assert_eq!(native.total_out() as usize, expected.len());
        assert_eq!(&decoded[..expected.len()], expected);
    }

    #[test]
    fn prefix_roundtrips_and_reuses_state() {
        fn assert_send<T: Send>() {}
        assert_send::<MicroLzmaEncoder>();

        let mut seed = 7u32;
        let random: Vec<_> = (0..65537)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let zeros = vec![0; random.len()];
        for mode in [EncodeMode::Fast, EncodeMode::Normal] {
            for mf in [MfType::Hc4, MfType::Bt4] {
                let mut options = LzmaOptions::with_preset(1);
                options.dict_size = 4096;
                options.mode = mode;
                options.mf = mf;
                options.lc = 2;
                options.lp = 1;
                options.pb = 1;
                let mut reused = MicroLzmaEncoder::new(&options).unwrap();
                let window = reused.lzma.lz.buf.as_ptr();
                for input in [
                    &random[..0],
                    &random[..1],
                    &random[..49],
                    &random[..],
                    &zeros[..],
                    &random[..31],
                ] {
                    for capacity in [5, 6, 7, 31, 32, 128, 4096, input.len() * 2 + 64] {
                        let mut output = vec![0xA5; capacity + 1];
                        let (used, written) = reused
                            .compress_prefix(input, &mut output[..capacity])
                            .unwrap();
                        assert!(used <= input.len() && written <= capacity);
                        assert!(output[written..].iter().all(|&byte| byte == 0xA5));
                        check_stream(&output[..written], &input[..used], options.dict_size);
                        let mut fresh = MicroLzmaEncoder::new(&options).unwrap();
                        let mut expected = vec![0; capacity];
                        assert_eq!(
                            fresh.compress_prefix(input, &mut expected).unwrap(),
                            (used, written),
                        );
                        assert_eq!(&output[..written], &expected[..written]);
                        assert_eq!(reused.lzma.lz.buf.as_ptr(), window);
                    }
                }
            }
        }
    }

    #[test]
    fn reset_wraps_match_finder_positions() {
        let mut seed = 5u32;
        let half: Vec<_> = (0..3000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let input = [half.as_slice(), half.as_slice()].concat();
        for mf in [MfType::Hc4, MfType::Bt4] {
            let mut options = LzmaOptions::with_preset(1);
            options.dict_size = 8 * 1024 * 1024;
            options.mf = mf;
            let mut encoder = MicroLzmaEncoder::new(&options).unwrap();
            let mut expected = vec![0; 8192];
            let (used, written) = encoder.compress_prefix(&input, &mut expected).unwrap();
            assert_eq!(used, input.len());
            check_stream(&expected[..written], &input, options.dict_size);
            let mut output = vec![0; 8192];
            // 300 resets advance the match finder positions past i32::MAX.
            for _ in 0..300 {
                assert_eq!(
                    encoder.compress_prefix(&input, &mut output).unwrap(),
                    (used, written)
                );
                assert_eq!(output[..written], expected[..written]);
            }
        }
    }

    #[test]
    fn input_can_exceed_dictionary_and_window() {
        let mut seed = 11u32;
        let input: Vec<_> = (0..(1 << 20))
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        for preset in [1, 6] {
            for dict_size in [4096, 128 * 1024] {
                let mut options = LzmaOptions::with_preset(preset);
                options.dict_size = dict_size;
                let mut encoder = MicroLzmaEncoder::new(&options).unwrap();
                assert!(input.len() > encoder.lzma.lz.buf_size);
                let mut expected = LzmaWriter::new_no_header(Vec::new(), &options, false).unwrap();
                expected.write_all(&input).unwrap();
                let mut expected = expected.finish().unwrap();
                expected[0] = !options.get_props();

                let mut full = vec![0; input.len() * 2];
                let (used, written) = encoder.compress_prefix(&input, &mut full).unwrap();
                assert_eq!(used, input.len());
                assert_eq!(&full[..written], expected);
                check_stream(&full[..written], &input, dict_size);
                let range_buffer = encoder.rc.inner().as_ptr();
                for capacity in [5, 6, 32, 4096, 512 * 1024, full.len()] {
                    let mut output = vec![0; capacity];
                    let (used, written) = encoder.compress_prefix(&input, &mut output).unwrap();
                    assert!(used <= input.len() && written <= capacity);
                    if capacity >= expected.len() {
                        assert_eq!(used, input.len());
                        assert_eq!(&output[..written], expected);
                    }
                    check_stream(&output[..written], &input[..used], dict_size);
                    assert_eq!(encoder.rc.inner().as_ptr(), range_buffer);
                }
            }
        }
    }

    #[test]
    fn invalid_options_and_short_outputs_are_rejected() {
        let mut options = LzmaOptions::with_preset(1);
        options.dict_size = 4096;
        for field in 0..11 {
            let mut invalid = options.clone();
            match field {
                0 => invalid.dict_size = 0,
                1 => invalid.dict_size = u32::MAX,
                2 => invalid.lc = 9,
                3 => invalid.lp = 5,
                4 => invalid.pb = 5,
                5 => invalid.nice_len = 274,
                6 => invalid.depth_limit = -1,
                7 => invalid.preset_dict = Some(vec![1]),
                8 => invalid.dict_size = i32::MAX as u32,
                9 => {
                    invalid.mf = MfType::Bt4;
                    invalid.dict_size = 1 << 30;
                }
                _ => invalid.nice_len = 7,
            }
            assert!(MicroLzmaEncoder::new(&invalid).is_err());
        }
        let mut encoder = MicroLzmaEncoder::new(&options).unwrap();
        let mut output = [0xA5; 64];
        for capacity in 0..5 {
            assert!(
                encoder
                    .compress_prefix(b"input", &mut output[..capacity])
                    .is_err()
            );
        }
        assert_eq!(output, [0xA5; 64]);
        let (used, written) = encoder.compress_prefix(b"input", &mut output).unwrap();
        assert_eq!(used, 5);
        check_stream(&output[..written], b"input", options.dict_size);
    }
}
