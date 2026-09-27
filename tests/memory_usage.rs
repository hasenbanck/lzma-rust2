#![cfg(feature = "encoder")]

use lzma_rust2::LzmaOptions;

#[test]
fn level_five_memory_estimate_reports_kibibytes_for_a_16_mib_dictionary() {
    let mut options = LzmaOptions::with_preset(5);
    options.dict_size = 16 << 20;

    assert_eq!(options.get_memory_usage(), 189_361);
}
