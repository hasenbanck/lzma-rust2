use std::io::{Read, Write};

use lzma_rust2::{LZMA2Options, LZMA2Reader, LZMA2Writer, LZMAReader, LZMAWriter};

static APACHE2: &str = include_str!("data/apache2.txt");
static PG100: &str = include_str!("data/pg100.txt");
static PG6800: &str = include_str!("data/pg6800.txt");

fn test_round_trip(text: &str, level: u32) {
    let option = LZMA2Options::with_preset(level);

    let mut compressed = Vec::new();

    {
        let mut writer = LZMAWriter::new_no_header(&mut compressed, &option, true).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
        writer.finish().unwrap();
    }

    let mut uncompressed = Vec::new();

    {
        let mut reader = LZMAReader::new(
            compressed.as_slice(),
            text.len() as u64,
            option.lc,
            option.lp,
            option.pb,
            option.dict_size,
            option.preset_dict.as_ref().map(|dict| dict.as_ref()),
        )
        .unwrap();
        reader.read_to_end(&mut uncompressed).unwrap();
    }

    let decoded = String::from_utf8(uncompressed).unwrap();

    // We don't use assert_eq since the debug output would be too big.
    assert!(decoded == text);
}

#[test]
fn round_apache2_0() {
    test_round_trip(APACHE2, 0);
}

#[test]
fn round_apache2_1() {
    test_round_trip(APACHE2, 1);
}

#[test]
fn round_apache2_2() {
    test_round_trip(APACHE2, 2);
}

#[test]
fn round_apache2_3() {
    test_round_trip(APACHE2, 3);
}

#[test]
fn round_apache2_4() {
    test_round_trip(APACHE2, 4);
}

#[test]
fn round_apache2_5() {
    test_round_trip(APACHE2, 5);
}

#[test]
fn round_apache2_6() {
    test_round_trip(APACHE2, 6);
}

#[test]
fn round_apache2_7() {
    test_round_trip(APACHE2, 7);
}

#[test]
fn round_apache2_8() {
    test_round_trip(APACHE2, 8);
}

#[test]
fn round_apache2_9() {
    test_round_trip(APACHE2, 9);
}

#[test]
fn round_trip_pg100_0() {
    test_round_trip(PG100, 0);
}

#[test]
fn round_trip_pg100_1() {
    test_round_trip(PG100, 1);
}

#[test]
fn round_trip_pg100_2() {
    test_round_trip(PG100, 2);
}

#[test]
fn round_trip_pg100_3() {
    test_round_trip(PG100, 3);
}

#[test]
fn round_trip_pg100_4() {
    test_round_trip(PG100, 4);
}

#[test]
fn round_trip_pg100_5() {
    test_round_trip(PG100, 5);
}

#[test]
fn round_trip_pg100_6() {
    test_round_trip(PG100, 6);
}

#[test]
fn round_trip_pg100_7() {
    test_round_trip(PG100, 7);
}

#[test]
fn round_trip_pg100_8() {
    test_round_trip(PG100, 8);
}

#[test]
fn round_trip_pg100_9() {
    test_round_trip(PG100, 9);
}

#[test]
fn round_trip_pg6800_0() {
    test_round_trip(PG6800, 0);
}

#[test]
fn round_trip_pg6800_1() {
    test_round_trip(PG6800, 1);
}

#[test]
fn round_trip_pg6800_2() {
    test_round_trip(PG6800, 2);
}

#[test]
fn round_trip_pg6800_3() {
    test_round_trip(PG6800, 3);
}

#[test]
fn round_trip_pg6800_4() {
    test_round_trip(PG6800, 4);
}

#[test]
fn round_trip_pg6800_5() {
    test_round_trip(PG6800, 5);
}

#[test]
fn round_trip_pg6800_6() {
    test_round_trip(PG6800, 6);
}

#[test]
fn round_trip_pg6800_7() {
    test_round_trip(PG6800, 7);
}

#[test]
fn round_trip_pg6800_8() {
    test_round_trip(PG6800, 8);
}

#[test]
fn round_trip_pg6800_9() {
    test_round_trip(PG6800, 9);
}
