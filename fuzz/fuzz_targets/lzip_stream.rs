#![no_main]

use libfuzzer_sys::fuzz_target;
use lzma_rust2::{Action, LzipStream, Status};

/// How much input to hand over in one call. A member hands back up to 40 bytes
/// when it ends, so 19/20/21 and 39/40/41 sit right on the edges where that
/// spills past the trailer into the next member.
const CHUNK_SIZES: [usize; 16] = [1, 1, 2, 3, 5, 7, 19, 20, 21, 32, 39, 40, 41, 64, 512, 4096];

/// How much room to give the output.
const OUTPUT_SIZES: [usize; 8] = [1, 1, 2, 7, 19, 64, 512, 4096];

const MAX_OUTPUT: usize = 1 << 18;
const MAX_STEPS: usize = 4096;

/// A member header may ask for a dictionary of up to 512 MiB, and a file can
/// hold any number of members, so the limit is what keeps a few input bytes from
/// turning into minutes of allocating.
const MEM_LIMIT_KB: u32 = 8 * 1024;

/// The first eight bytes say how to feed the decoder.
const PLAN_LEN: usize = 8;

fuzz_target!(|data: &[u8]| {
    if data.len() < PLAN_LEN {
        return;
    }
    let (plan, stream) = data.split_at(PLAN_LEN);

    let mut decoder = LzipStream::new_mem_limit(MEM_LIMIT_KB);

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
