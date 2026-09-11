//! The LZMA decoder in the layout of 7-Zip's `LzmaDec.c`, so that the LZMA
//! SDK's arm64 assembly of the inner loop can run over it: every probability
//! in one array, the state, the repeat distances and the input pointer in a
//! C-layout struct the assembly reads and writes back. The assembly decodes
//! while the input holds a whole symbol's worth of bytes and the output has
//! room. The loop in Rust below, written from the same C, takes the last bytes
//! of a chunk and input that arrives a byte at a time, so the two share every
//! probability and agree on every one of them.
//!
//! The struct and array layout, the probability indexing (including the
//! reverse bit trees' and the matched literal's) and the state values are
//! 7-Zip's, since the assembly is built for them. The window is the crate's
//! [`LzDecoder`]: the assembly writes into its buffer, and its position and
//! fill stand in for 7-Zip's `processedPos` and `checkDicSize`.

use alloc::{vec, vec::Vec};
use core::arch::global_asm;

use crate::{
    error_other,
    lz::LzDecoder,
    lzma_reader::IN_REQUIRED,
    range_dec::{RangeCoderState, RangeDecoder, RangeReader},
};

global_asm!(include_str!("asm/aarch64/lzma_dec_opt.S"));

unsafe extern "C" {
    /// The inner loop, from the LZMA SDK. See `asm/aarch64/lzma_dec_opt.S`.
    fn lzma_rust2_lzma_decode_real(p: *mut DecState, limit: usize, buf_limit: *const u8) -> i32;
}

// The layout of 7-Zip's `LzmaDec.c`.
const NUM_POS_BITS_MAX: usize = 4;
const NUM_POS_STATES_MAX: usize = 1 << NUM_POS_BITS_MAX;
const LEN_NUM_LOW_BITS: usize = 3;
const LEN_NUM_LOW_SYMBOLS: usize = 1 << LEN_NUM_LOW_BITS;
const LEN_NUM_HIGH_BITS: usize = 8;
const LEN_NUM_HIGH_SYMBOLS: usize = 1 << LEN_NUM_HIGH_BITS;
const LEN_LOW: usize = 0;
const LEN_HIGH: usize = LEN_LOW + 2 * (NUM_POS_STATES_MAX << LEN_NUM_LOW_BITS);
const NUM_LEN_PROBS: usize = LEN_HIGH + LEN_NUM_HIGH_SYMBOLS;
const LEN_CHOICE: usize = LEN_LOW;
const LEN_CHOICE2: usize = LEN_LOW + (1 << LEN_NUM_LOW_BITS);
const NUM_STATES: usize = 12;
const NUM_STATES2: usize = 16;
const NUM_LIT_STATES: usize = 7;
const START_POS_MODEL_INDEX: u32 = 4;
const END_POS_MODEL_INDEX: u32 = 14;
const NUM_FULL_DISTANCES: usize = 1 << (END_POS_MODEL_INDEX >> 1);
const NUM_POS_SLOT_BITS: usize = 6;
const NUM_LEN_TO_POS_STATES: usize = 4;
const NUM_ALIGN_BITS: u32 = 4;
const ALIGN_TABLE_SIZE: usize = 1 << NUM_ALIGN_BITS;
const MATCH_MIN_LEN: u32 = 2;
/// The `remain_len` the assembly leaves after the end marker.
const MATCH_SPEC_LEN_START: u32 =
    MATCH_MIN_LEN + (LEN_NUM_LOW_SYMBOLS as u32) * 2 + LEN_NUM_HIGH_SYMBOLS as u32;
/// Added to `remain_len` by the assembly when a match reaches before the data.
const MATCH_SPEC_LEN_ERROR_DATA: u32 = 1 << 9;
const SPEC_POS: usize = 0;
const IS_REP0_LONG: usize = SPEC_POS + NUM_FULL_DISTANCES;
const REP_LEN_CODER: usize = IS_REP0_LONG + (NUM_STATES2 << NUM_POS_BITS_MAX);
const LEN_CODER: usize = REP_LEN_CODER + NUM_LEN_PROBS;
const IS_MATCH: usize = LEN_CODER + NUM_LEN_PROBS;
const ALIGN: usize = IS_MATCH + (NUM_STATES2 << NUM_POS_BITS_MAX);
const IS_REP: usize = ALIGN + ALIGN_TABLE_SIZE;
const IS_REP_G0: usize = IS_REP + NUM_STATES;
const IS_REP_G1: usize = IS_REP_G0 + NUM_STATES;
const IS_REP_G2: usize = IS_REP_G1 + NUM_STATES;
const POS_SLOT: usize = IS_REP_G2 + NUM_STATES;
const LITERAL: usize = POS_SLOT + (NUM_LEN_TO_POS_STATES << NUM_POS_SLOT_BITS);
const NUM_BASE_PROBS: usize = LITERAL;
const LIT_SIZE: usize = 0x300;
const PROB_INIT: u16 = 1024;
/// A range coder whose first code is at least this starts with a repeat
/// match, which no stream may (7-Zip's `kBadRepCode`).
const BAD_REP_CODE: u32 = 0xC000_0000 - 0x400;

/// 7-Zip's `CLzmaDec`, as far as the assembly reads it. Do not reorder.
#[repr(C)]
struct DecState {
    lc: u8,
    lp: u8,
    pb: u8,
    _pad: u8,
    dic_size: u32,
    probs: *mut u16,
    probs_1664: *mut u16,
    dic: *mut u8,
    dic_buf_size: usize,
    dic_pos: usize,
    buf: *const u8,
    range: u32,
    code: u32,
    processed_pos: u32,
    check_dic_size: u32,
    reps: [u32; 4],
    state: u32,
    remain_len: u32,
    num_probs: u32,
    temp_buf_size: u32,
    temp_buf: [u8; 20],
}

const _: () = {
    assert!(core::mem::offset_of!(DecState, dic_size) == 4);
    assert!(core::mem::offset_of!(DecState, probs) == 8);
    assert!(core::mem::offset_of!(DecState, dic) == 24);
    assert!(core::mem::offset_of!(DecState, dic_buf_size) == 32);
    assert!(core::mem::offset_of!(DecState, dic_pos) == 40);
    assert!(core::mem::offset_of!(DecState, buf) == 48);
    assert!(core::mem::offset_of!(DecState, range) == 56);
    assert!(core::mem::offset_of!(DecState, code) == 60);
    assert!(core::mem::offset_of!(DecState, processed_pos) == 64);
    assert!(core::mem::offset_of!(DecState, check_dic_size) == 68);
    assert!(core::mem::offset_of!(DecState, reps) == 72);
    assert!(core::mem::offset_of!(DecState, state) == 88);
    assert!(core::mem::offset_of!(DecState, remain_len) == 92);
};

/// The decoder: the probabilities in 7-Zip's one array, the state as 7-Zip
/// counts it (0 to 11), the repeat distances as 7-Zip keeps them (the
/// distance plus one).
pub(crate) struct LzmaDecoder {
    probs: Vec<u16>,
    lc: u32,
    lp: u32,
    pb: u32,
    state: u32,
    reps: [u32; 4],
    end_marker: bool,
}

impl LzmaDecoder {
    pub(crate) fn new(lc: u32, lp: u32, pb: u32) -> Self {
        let probs = vec![PROB_INIT; NUM_BASE_PROBS + (LIT_SIZE << (lc + lp))];
        Self {
            probs,
            lc,
            lp,
            pb,
            state: 0,
            reps: [1; 4],
            end_marker: false,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.probs.fill(PROB_INIT);
        self.state = 0;
        self.reps = [1; 4];
        self.end_marker = false;
    }

    /// Whether the last `decode` stopped at the end of payload marker. The
    /// marker surfaces as a "dist overflow" error, as it always has, and
    /// this says whether that error was the marker.
    pub(crate) fn end_marker_detected(&self) -> bool {
        self.end_marker
    }

    /// Decodes into `lz` until its output is full or `rc` cannot start
    /// another symbol.
    pub(crate) fn decode<R: RangeReader>(
        &mut self,
        lz: &mut LzDecoder,
        rc: &mut RangeDecoder<R>,
    ) -> crate::Result<()> {
        lz.repeat_pending()?;
        if self.end_marker {
            return Err(error_other("dist overflow"));
        }
        // A stream may not open with a repeat match: 7-Zip refuses the range
        // coder's first code when it would, and the assembly relies on that.
        if lz.full() == 0 && lz.get_pos() == 0 && rc.state().code >= BAD_REP_CODE {
            return Err(error_other("dist overflow"));
        }
        while lz.has_space() {
            if rc.inner().is_buffer() && self.decode_fast(lz, rc)? {
                continue;
            }
            if !rc.can_start_symbol() {
                break;
            }
            self.decode_symbol(lz, rc)?;
        }
        if !lz.has_space() && rc.can_normalize() {
            rc.normalize();
        }
        Ok(())
    }

    /// The assembly over the buffered input, while a whole symbol's worth of
    /// bytes is there. Returns whether it decoded anything.
    fn decode_fast<R: RangeReader>(
        &mut self,
        lz: &mut LzDecoder,
        rc: &mut RangeDecoder<R>,
    ) -> crate::Result<bool> {
        let (input_len, pos, symbol_limit) = {
            let inner = rc.inner();
            (inner.buf().len(), inner.pos(), inner.symbol_limit())
        };
        // The assembly reads up to twenty bytes past a symbol's start.
        let limit_index = symbol_limit.min(input_len.saturating_sub(IN_REQUIRED));
        if pos >= limit_index {
            return Ok(false);
        }
        rc.normalize();
        let pos = rc.inner().pos();
        if pos >= limit_index {
            return Ok(false);
        }
        let dic_buf_size = lz.buf_size();
        let dic_pos = lz.get_pos();
        let full = lz.full();
        let limit = lz.limit();
        let mut state = DecState {
            lc: self.lc as u8,
            lp: self.lp as u8,
            pb: self.pb as u8,
            _pad: 0,
            dic_size: u32::try_from(dic_buf_size).unwrap_or(u32::MAX),
            probs: self.probs.as_mut_ptr(),
            probs_1664: core::ptr::null_mut(),
            dic: lz.buf_mut().as_mut_ptr(),
            dic_buf_size,
            dic_pos,
            buf: core::ptr::null(),
            range: rc.state().range,
            code: rc.state().code,
            processed_pos: dic_pos as u32,
            check_dic_size: if full >= dic_buf_size {
                u32::try_from(dic_buf_size).unwrap_or(u32::MAX)
            } else {
                0
            },
            reps: self.reps,
            state: self.state,
            remain_len: 0,
            num_probs: 0,
            temp_buf_size: 0,
            temp_buf: [0; 20],
        };
        let result;
        let consumed;
        {
            let input = rc.inner().buf();
            state.buf = input[pos..].as_ptr();
            // SAFETY: `state` mirrors 7-Zip's `CLzmaDec` field for field (the
            // offsets are asserted above). `probs` holds every index the
            // layout can produce for these `lc`, `lp` and `pb`. `dic` is the
            // window, `dic_buf_size` bytes long, and `limit` at most that,
            // so every write lands inside it. `buf` points into `input`, and
            // with `buf_limit` at least twenty bytes before its end the
            // function never reads past `input` (its contract, above the
            // assembly). Nothing else is touched.
            result = unsafe {
                lzma_rust2_lzma_decode_real(&mut state, limit, input[limit_index..].as_ptr())
            };
            consumed = usize::try_from(unsafe { state.buf.offset_from(input.as_ptr()) })
                .unwrap_or(input.len());
        }
        rc.inner_mut().set_pos(consumed);
        rc.set_state(RangeCoderState {
            range: state.range,
            code: state.code,
        });
        lz.set_pos(state.dic_pos);
        self.reps = state.reps;
        self.state = state.state & 0xF;
        let remain = state.remain_len;
        if result != 0 || remain >= MATCH_SPEC_LEN_ERROR_DATA {
            return Err(error_other("dist overflow"));
        }
        if remain == MATCH_SPEC_LEN_START {
            self.end_marker = true;
            return Err(error_other("dist overflow"));
        }
        if remain > 0 {
            lz.set_pending(remain as usize, (self.reps[0] - 1) as usize);
        }
        Ok(true)
    }

    #[inline(always)]
    fn bit<R: RangeReader>(&mut self, rc: &mut RangeDecoder<R>, index: usize) -> u32 {
        rc.decode_bit(&mut self.probs[index]) as u32
    }

    /// A bit tree of `bits` bits at `base`, 7-Zip's `TREE_DECODE`.
    #[inline(always)]
    fn tree<R: RangeReader>(&mut self, rc: &mut RangeDecoder<R>, base: usize, bits: u32) -> u32 {
        let limit = 1usize << bits;
        let mut i = 1usize;
        while i < limit {
            i = (i << 1) | self.bit(rc, base + i) as usize;
        }
        (i - limit) as u32
    }

    /// One LZMA symbol, as 7-Zip's `LzmaDec_DecodeReal` decodes it, into
    /// the window.
    fn decode_symbol<R: RangeReader>(
        &mut self,
        lz: &mut LzDecoder,
        rc: &mut RangeDecoder<R>,
    ) -> crate::Result<()> {
        let pb_mask = (1u32 << self.pb) - 1;
        // 7-Zip indexes the position-state tables as (pos_state << 4) + state.
        let pos_state = (lz.get_pos() as u32 & pb_mask) as usize;
        let mut state = self.state as usize;
        if self.bit(rc, IS_MATCH + (pos_state << NUM_POS_BITS_MAX) + state) == 0 {
            let lp_mask = (0x100u32 << self.lp) - (0x100u32 >> self.lc);
            let mut base = LITERAL;
            if lz.full() != 0 {
                let previous = u32::from(lz.get_byte(0));
                let context = (((lz.get_pos() as u32) << 8) + previous) & lp_mask;
                base += 3 * (context << self.lc) as usize;
            }
            let symbol = if state < NUM_LIT_STATES {
                state -= if state < 4 { state } else { 3 };
                self.tree(rc, base, 8)
            } else {
                let mut match_byte = u32::from(lz.get_byte((self.reps[0] - 1) as usize));
                state -= if state < 10 { 3 } else { 6 };
                let mut offs = 0x100u32;
                let mut symbol = 1u32;
                while symbol < 0x100 {
                    match_byte += match_byte;
                    let bit = offs;
                    offs &= match_byte;
                    let decoded = self.bit(rc, base + (offs + bit + symbol) as usize);
                    symbol = (symbol << 1) | decoded;
                    if decoded == 0 {
                        offs ^= bit;
                    }
                }
                symbol - 0x100
            };
            self.state = state as u32;
            lz.put_byte(symbol as u8);
            return Ok(());
        }
        let len_base = if self.bit(rc, IS_REP + state) == 0 {
            state += NUM_STATES;
            LEN_CODER
        } else {
            if self.bit(rc, IS_REP_G0 + state) == 0 {
                if self.bit(rc, IS_REP0_LONG + (pos_state << NUM_POS_BITS_MAX) + state) == 0 {
                    // A short repeat: one byte from the last distance.
                    if lz.full() == 0 {
                        return Err(error_other("dist overflow"));
                    }
                    self.state = if state < NUM_LIT_STATES { 9 } else { 11 };
                    return lz.repeat((self.reps[0] - 1) as usize, 1);
                }
            } else {
                let distance = if self.bit(rc, IS_REP_G1 + state) == 0 {
                    self.reps[1]
                } else {
                    let distance = if self.bit(rc, IS_REP_G2 + state) == 0 {
                        self.reps[2]
                    } else {
                        let distance = self.reps[3];
                        self.reps[3] = self.reps[2];
                        distance
                    };
                    self.reps[2] = self.reps[1];
                    distance
                };
                self.reps[1] = self.reps[0];
                self.reps[0] = distance;
            }
            state = if state < NUM_LIT_STATES { 8 } else { 11 };
            REP_LEN_CODER
        };
        let mut len = self.decode_len(rc, len_base, pos_state);
        if state >= NUM_STATES {
            let len_state = (len as usize).min(NUM_LEN_TO_POS_STATES - 1);
            let mut distance = self.tree(rc, POS_SLOT + (len_state << NUM_POS_SLOT_BITS), 6);
            if distance >= START_POS_MODEL_INDEX {
                let pos_slot = distance;
                let mut direct_bits = (distance >> 1) - 1;
                distance = 2 | (distance & 1);
                if pos_slot < END_POS_MODEL_INDEX {
                    distance <<= direct_bits;
                    // The reverse bit tree of the special positions, indexed as
                    // 7-Zip indexes it.
                    let mut m = 1u32;
                    let mut i = distance + 1;
                    loop {
                        if self.bit(rc, SPEC_POS + i as usize) == 0 {
                            i += m;
                            m += m;
                        } else {
                            m += m;
                            i += m;
                        }
                        direct_bits -= 1;
                        if direct_bits == 0 {
                            break;
                        }
                    }
                    distance = i - m;
                } else {
                    direct_bits -= NUM_ALIGN_BITS;
                    distance =
                        (distance << direct_bits) | rc.decode_direct_bits(direct_bits) as u32;
                    distance <<= NUM_ALIGN_BITS;
                    // The align bits, reversed as 7-Zip reverses them.
                    let mut i = 1u32;
                    for m in [1u32, 2, 4] {
                        if self.bit(rc, ALIGN + i as usize) == 0 {
                            i += m;
                        } else {
                            i += m * 2;
                        }
                    }
                    if self.bit(rc, ALIGN + i as usize) == 0 {
                        i -= 8;
                    }
                    distance |= i;
                    if distance == 0xFFFF_FFFF {
                        self.state = (state - NUM_STATES) as u32;
                        self.end_marker = true;
                        return Err(error_other("dist overflow"));
                    }
                }
            }
            self.reps[3] = self.reps[2];
            self.reps[2] = self.reps[1];
            self.reps[1] = self.reps[0];
            self.reps[0] = distance.wrapping_add(1);
            state = if state < NUM_STATES + NUM_LIT_STATES {
                NUM_LIT_STATES
            } else {
                NUM_LIT_STATES + 3
            };
            self.state = state as u32;
            if distance as usize >= lz.full() {
                return Err(error_other("dist overflow"));
            }
        } else {
            self.state = state as u32;
        }
        len += MATCH_MIN_LEN;
        lz.repeat((self.reps[0] - 1) as usize, len as usize)
    }

    /// A length, 7-Zip's layout: the two choice bits at the head of the low
    /// table, low and mid trees of three bits per position state, a high
    /// tree of eight.
    fn decode_len<R: RangeReader>(
        &mut self,
        rc: &mut RangeDecoder<R>,
        base: usize,
        pos_state: usize,
    ) -> u32 {
        let len_state = pos_state << (LEN_NUM_LOW_BITS + 1);
        if self.bit(rc, base + LEN_CHOICE) == 0 {
            self.tree(rc, base + LEN_LOW + len_state, LEN_NUM_LOW_BITS as u32)
        } else if self.bit(rc, base + LEN_CHOICE2) == 0 {
            LEN_NUM_LOW_SYMBOLS as u32
                + self.tree(
                    rc,
                    base + LEN_LOW + len_state + LEN_NUM_LOW_SYMBOLS,
                    LEN_NUM_LOW_BITS as u32,
                )
        } else {
            2 * LEN_NUM_LOW_SYMBOLS as u32
                + self.tree(rc, base + LEN_HIGH, LEN_NUM_HIGH_BITS as u32)
        }
    }
}
