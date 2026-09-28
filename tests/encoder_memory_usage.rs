#![cfg(feature = "encoder")]

use lzma_rust2::LzmaOptions;

#[test]
fn memory_estimate_accounts_for_custom_literal_contexts() {
    for preset in [0, 5] {
        let mut options = LzmaOptions::with_preset(preset);
        let default_kib = options.get_memory_usage();

        options.lc = 8;
        options.lp = 4;

        // 4,096 literal models use 6,144 KiB; the default eight use 12 KiB.
        assert_eq!(options.get_memory_usage(), default_kib + 6_144 - 12);
    }
}

#[test]
fn memory_estimate_reports_full_custom_context_usage() {
    let mut options = LzmaOptions::with_preset(5);
    options.dict_size = 16 << 20;
    options.lc = 8;
    options.lp = 4;

    assert_eq!(options.get_memory_usage(), 195_493);
}

#[test]
fn memory_estimate_tracks_literal_context_and_position_bits() {
    for preset in 0..=9 {
        let mut options = LzmaOptions::with_preset(preset);
        let default_kib = options.get_memory_usage();

        for lc in 0..=8 {
            for lp in 0..=4 {
                options.lc = lc;
                options.lp = lp;
                let literal_kib = (1536u32 << (lc + lp)).div_ceil(1024);
                assert_eq!(
                    options.get_memory_usage(),
                    default_kib - 12 + literal_kib,
                    "preset={preset}, lc={lc}, lp={lp}"
                );
            }
        }
    }
}

#[test]
fn memory_estimate_saturates_on_unrepresentable_literal_models() {
    for preset in [0, 5] {
        let mut options = LzmaOptions::with_preset(preset);
        for (lc, lp) in [(22, 0), (32, 0), (0, 32), (u32::MAX, 1), (1, u32::MAX)] {
            options.lc = lc;
            options.lp = lp;
            assert_eq!(options.get_memory_usage(), u32::MAX);
        }
    }
}
