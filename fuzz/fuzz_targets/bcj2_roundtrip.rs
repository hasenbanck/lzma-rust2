#![no_main]

use std::io::{Read, Write};

use libfuzzer_sys::fuzz_target;
use lzma_rust2::filter::bcj2::{Bcj2Options, Bcj2Reader, Bcj2Writer};

fuzz_target!(|data: &[u8]| {
    if data.len() < 3 || data.len() > 65539 {
        return;
    }
    let chunk_size = 1 + data[0] as usize;
    let options = Bcj2Options {
        uncompressed_size: (data[1] & 1 != 0).then_some((data.len() - 3) as u64),
        relative_limit: if data[1] & 2 != 0 {
            Bcj2Options::RELATIVE_LIMIT_MAX
        } else {
            Bcj2Options::RELATIVE_LIMIT_DEFAULT
        },
    };
    let mut writer = Bcj2Writer::new(std::array::from_fn(|_| Vec::new()), &options).unwrap();
    for chunk in data[3..].chunks(chunk_size) {
        writer.write_all(chunk).unwrap();
        if data[2] & 1 != 0 {
            writer.flush().unwrap();
        }
    }
    let streams = writer.finish().unwrap();
    let mut reader = Bcj2Reader::new(
        streams.iter().map(Vec::as_slice).collect(),
        (data.len() - 3) as u64,
    );
    let mut output = Vec::new();
    let mut buf = vec![0; 1 + data[2] as usize];
    loop {
        let size = reader.read(&mut buf).unwrap();
        if size == 0 {
            break;
        }
        output.extend_from_slice(&buf[..size]);
    }
    reader.finish().unwrap();
    assert_eq!(output, &data[3..]);
});
