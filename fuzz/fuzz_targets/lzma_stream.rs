#![no_main]

use libfuzzer_sys::fuzz_target;
use lzma_rust2::{Action, LzmaStream, Status};

/// How much input to hand over in one call. A symbol is at most 20 bytes long
/// and the carry buffer holds 40, so 19/20/21 and 39/40/41 sit right on the
/// edges where the decoder changes what it does.
const CHUNK_SIZES: [usize; 16] = [1, 1, 2, 3, 5, 7, 19, 20, 21, 32, 39, 40, 41, 64, 512, 4096];

/// How much room to give the output.
const OUTPUT_SIZES: [usize; 8] = [1, 1, 2, 7, 19, 64, 512, 4096];

const MAX_OUTPUT: usize = 1 << 18;
const MAX_STEPS: usize = 4096;

const MEM_LIMIT_KB: u32 = 8 * 1024;

/// First 8 bytes say how to build the decoder, next 8 say how to feed it.
const HEAD_LEN: usize = 8;
const PLAN_LEN: usize = 8;

fuzz_target!(|data: &[u8]| {
    if data.len() < HEAD_LEN + PLAN_LEN {
        return;
    }
    let (head, tail) = data.split_at(HEAD_LEN);
    let (plan, rest) = tail.split_at(PLAN_LEN);

    let uncomp_size = if head[0] & 1 != 0 {
        u16::from_le_bytes([head[6], head[7]]) as u64
    } else {
        u64::MAX
    };

    // Kept small, because the raw constructors have no memory limit of their own.
    let dict_size = u16::from_le_bytes([head[4], head[5]]) as u32;

    let preset_len = if head[0] & 2 != 0 {
        (head[3] as usize * 4).min(rest.len())
    } else {
        0
    };
    let (preset, stream) = rest.split_at(preset_len);
    let preset_dict = (!preset.is_empty()).then_some(preset);

    let mut decoder = match (head[0] >> 2) & 3 {
        // With a header, so the stream itself supplies dict_size and uncomp_size.
        0 => LzmaStream::new_mem_limit(MEM_LIMIT_KB, preset_dict),
        // Raw. The properties byte is passed through untouched so that the
        // checks on it get fuzzed too.
        1 => match LzmaStream::new_with_props(uncomp_size, head[1], dict_size, preset_dict) {
            Ok(decoder) => decoder,
            Err(_) => return,
        },
        // Raw, with lc, lp and pb given separately.
        _ => {
            let lc = (head[1] % 9) as u32;
            let lp = ((head[1] >> 4) % 5) as u32;
            let pb = (head[2] % 5) as u32;
            match LzmaStream::new(uncomp_size, lc, lp, pb, dict_size, preset_dict) {
                Ok(decoder) => decoder,
                Err(_) => return,
            }
        }
    };

    let mut output = [0u8; 4096];
    let mut in_pos = 0usize;
    let mut total_out = 0usize;

    for step in 0..MAX_STEPS {
        // One plan byte per call: the low half picks the chunk size, the high
        // half picks the output size. Reading the plan from the input rather
        // than from a random number generator means changing one byte changes
        // one call, which is what lets the fuzzer learn from what it tries.
        let choice = plan[step % PLAN_LEN];
        let chunk = CHUNK_SIZES[(choice & 0x0F) as usize];
        let out_len = OUTPUT_SIZES[(choice >> 4) as usize % OUTPUT_SIZES.len()];

        let end = in_pos.saturating_add(chunk).min(stream.len());
        let action = if end >= stream.len() {
            Action::Finish
        } else {
            Action::Run
        };

        let result = match decoder.process(&stream[in_pos..end], &mut output[..out_len], action) {
            Ok(result) => result,
            Err(_) => return,
        };

        in_pos += result.bytes_consumed;
        total_out += result.bytes_produced;

        if result.status == Status::StreamEnd {
            return;
        }

        // Every call has to do something. The output buffer is never empty, and
        // the input slice is only empty once we have already said Finish, so
        // there is no case here where doing nothing is allowed.
        assert!(
            result.bytes_consumed != 0 || result.bytes_produced != 0,
            "process() did nothing at input {in_pos}/{} with {out_len} bytes of output space \
             and {action:?}",
            stream.len(),
        );

        if total_out > MAX_OUTPUT {
            return;
        }
    }
});
