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
