#![cfg(feature = "std")]

use std::io::{self, Read};

use lzma_rust2::filter::bcj2::Bcj2Reader;

#[test]
fn large_declared_size_does_not_truncate_to_usize() {
    let mut reader = Bcj2Reader::new(vec![&[0x90][..], &[], &[], &[0; 5]], 1 << 32);
    let mut buf = [0];
    assert_eq!(reader.read(&mut buf).unwrap(), 1);
    assert_eq!(buf, [0x90]);
}

#[test]
fn invalid_stream_counts_do_not_panic() {
    for count in [0, 1, 2, 3, 5] {
        let mut reader = Bcj2Reader::new(vec![&[][..]; count], 1);
        assert_eq!(
            reader.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

#[test]
fn checked_construction_rejects_invalid_stream_counts() {
    for count in [0, 1, 2, 3, 5] {
        let error = Bcj2Reader::<&[u8]>::try_new(vec![&[][..]; count], 0)
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
    assert!(Bcj2Reader::<&[u8]>::try_new(vec![&[][..]; 4], 0).is_ok());
}

#[test]
fn input_errors_keep_their_operating_system_code_after_progress() {
    struct Source<'a>(&'a [u8]);
    impl Read for Source<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return Err(io::Error::from_raw_os_error(13));
            }
            self.0.read(buf)
        }
    }
    let inputs = vec![Source(&[0x90]), Source(&[]), Source(&[]), Source(&[0; 5])];
    let mut reader = Bcj2Reader::new(inputs, 2);
    let mut output = [0; 8];
    assert_eq!(reader.read(&mut output).unwrap(), 1);
    assert_eq!(output[0], 0x90);
    for _ in 0..2 {
        assert_eq!(
            reader.read(&mut output).unwrap_err().raw_os_error(),
            Some(13)
        );
    }
}

fn decode(inputs: [&[u8]; 4], size: u64) -> io::Result<Vec<u8>> {
    let mut reader = Bcj2Reader::new(inputs.to_vec(), size);
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    Ok(output)
}

#[test]
fn empty_output_still_requires_a_valid_range_stream() {
    assert!(decode([&[], &[], &[], &[]], 0).is_err());
    assert!(decode([&[], &[], &[], &[0; 4]], 0).is_err());
    assert!(decode([&[], &[], &[], &[1, 0, 0, 0, 0]], 0).is_err());
    assert!(decode([&[], &[], &[], &[0; 5]], 0).unwrap().is_empty());
}
