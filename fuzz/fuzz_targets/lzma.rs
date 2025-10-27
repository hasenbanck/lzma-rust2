#![no_main]

use std::io;

use libfuzzer_sys::{arbitrary, fuzz_target};

use lzma_rust2::{LzmaOptions, LzmaReader};

#[derive(Debug)]
struct LzmaData(Vec<u8>);

impl<'a> arbitrary::Arbitrary<'a> for LzmaData {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> Result<Self, arbitrary::Error> {
        let c1 = u8::arbitrary(u)?;
        let c2 = u8::arbitrary(u)?;
        let c3 = u8::arbitrary(u)?;
        let c4 = u8::arbitrary(u)?;
        let mut bytes = vec![0, c1, c2, c3, c4];

        let vec: Vec<u8> = Vec::arbitrary(u)?;
        bytes.extend(vec);
        Ok(Self(bytes))
    }

    #[inline]
    fn size_hint(depth: usize) -> (usize, Option<usize>) {
        Vec::<u8>::size_hint(depth)
    }
}

fuzz_target!(|data: LzmaData| {
    let options = LzmaOptions::with_preset(6);
    let uncomp_size = 1 << 20;
    let props = options.get_props();
    let dict_size = options.dict_size;

    let r = data.0.as_slice();
    let mut reader = LzmaReader::new_with_props(r, uncomp_size, props, dict_size, None).unwrap();
    let _ = io::copy(&mut reader, &mut io::empty());
});
