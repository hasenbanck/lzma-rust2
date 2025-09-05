use std::io::{self, Read, Write};

use lzma_rust2::{Lzma2Options, Lzma2Reader, Lzma2Writer, LzmaOptions};

fn main() -> io::Result<()> {
    let input = b"Hello, world!";

    let mut writer = Lzma2Writer::new(Vec::new(), Lzma2Options::default());
    writer.write_all(input)?;
    let bytes = writer.finish()?;
    assert_ne!(bytes, input);

    let mut reader = Lzma2Reader::new(bytes.as_slice(), LzmaOptions::DICT_SIZE_DEFAULT, None);
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    assert_eq!(buf.as_bytes(), input);
    Ok(())
}
