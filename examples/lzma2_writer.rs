use std::io::{self, Write};

use lzma_rust2::{Lzma2Options, Lzma2Writer};

fn main() -> io::Result<()> {
    let input = b"Hello, world!";
    let mut writer = Lzma2Writer::new(Vec::new(), Lzma2Options::default());
    writer.write_all(input)?;

    println!("{input:?} in");
    println!("{:?} out", writer.finish()?);
    Ok(())
}
