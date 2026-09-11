// The inner loop of the LZMA decoder in arm64 assembly, from the LZMA SDK
// by Igor Pavlov (LzmaDecOpt.S and 7zAsm.S, 2021-04-25, public domain), as
// shipped with 7-Zip. The C preprocessor has been run over the two files so
// that the assembly stands alone (`cc -E -P -x assembler-with-cpp`), and the
// entry symbol is renamed so that a program linking 7-Zip as well can hold
// both. One thing is changed: the SDK copies a match a byte at a time, and
// here a match whose source lies a word or more from its destination is
// copied sixteen or eight bytes at a time, or in one masked word when it is
// shorter than a word (COPY MATCH FAST, below). The register plan, the
// struct offsets and the probability layout are those of 7-Zip's LzmaDec.c,
// mirrored in `DecState` and the constants of `decoder_fast.rs`.
//
// int lzma_rust2_lzma_decode_real(DecState *p, size_t limit, const u8 *buf_limit)
//   In: the range coder normalised, p->dic_pos < limit, and at least 20 bytes
//   readable from p->buf past every symbol start up to buf_limit (the
//   function never reads buf_limit[20]).
//   Out: 0 on success, 1 when a match reaches before the dictionary's data.
//   p->buf, p->range, p->code, p->dic_pos, p->processed_pos, p->reps,
//   p->state and p->remain_len are written back, the range coder normalised.
//   p->remain_len is the tail of a match the output limit cut, or 274 after
//   the end marker, or 0x200 and up after an error.

.macro p2_add reg:req, param:req
        add \reg, \reg, \param
.endm
.macro p2_sub reg:req, param:req
        sub \reg, \reg, \param
.endm
.macro p2_sub_s reg:req, param:req
        subs \reg, \reg, \param
.endm
.macro p2_and reg:req, param:req
        and \reg, \reg, \param
.endm
.macro xor reg:req, param:req
        eor \reg, \reg, \param
.endm
.macro or reg:req, param:req
        orr \reg, \reg, \param
.endm
.macro shl reg:req, param:req
        lsl \reg, \reg, \param
.endm
.macro shr reg:req, param:req
        lsr \reg, \reg, \param
.endm
.macro sar reg:req, param:req
        asr \reg, \reg, \param
.endm
.macro p1_neg reg:req
        neg \reg, \reg
.endm
.macro dec reg:req
        sub \reg, \reg, 1
.endm
.macro dec_s reg:req
        subs \reg, \reg, 1
.endm
.macro inc reg:req
        add \reg, \reg, 1
.endm
.macro inc_s reg:req
        adds \reg, \reg, 1
.endm
.macro imul reg:req, param:req
        mul \reg, \reg, \param
.endm
.macro jmp lab:req
        b \lab
.endm
.macro je lab:req
        b.eq \lab
.endm
.macro jz lab:req
        b.eq \lab
.endm
.macro jnz lab:req
        b.ne \lab
.endm
.macro jne lab:req
        b.ne \lab
.endm
.macro jb lab:req
        b.lo \lab
.endm
.macro jbe lab:req
        b.ls \lab
.endm
.macro ja lab:req
        b.hi \lab
.endm
.macro jae lab:req
        b.hs \lab
.endm
.macro cmove dest:req, srcTrue:req
        csel \dest, \srcTrue, \dest, eq
.endm
.macro cmovne dest:req, srcTrue:req
        csel \dest, \srcTrue, \dest, ne
.endm
.macro cmovs dest:req, srcTrue:req
        csel \dest, \srcTrue, \dest, mi
.endm
.macro cmovns dest:req, srcTrue:req
        csel \dest, \srcTrue, \dest, pl
.endm
.macro cmovb dest:req, srcTrue:req
        csel \dest, \srcTrue, \dest, lo
.endm
.macro cmovae dest:req, srcTrue:req
        csel \dest, \srcTrue, \dest, hs
.endm
.macro MY_ALIGN_16 macro
 .p2align 4,, (1 << 4) - 1
.endm
.macro MY_ALIGN_32 macro
        .p2align 5,, (1 << 5) - 1
.endm
.macro MY_ALIGN_64 macro
        .p2align 6,, (1 << 6) - 1
.endm
 .text
 .align 2
 .p2align 4,,15
        .globl _lzma_rust2_lzma_decode_real
        .globl lzma_rust2_lzma_decode_real
        .macro PLOAD dest:req, mem:req
                ldrh \dest, [\mem]
        .endm
        .macro PLOAD_PREINDEXED dest:req, mem:req, offset:req
                ldrh \dest, [\mem, \offset]!
        .endm
        .macro PLOAD_2 dest:req, mem1:req, mem2:req
                ldrh \dest, [\mem1, \mem2]
        .endm
        .macro PLOAD_LSL dest:req, mem1:req, mem2:req
                ldrh \dest, [\mem1, \mem2, lsl #1]
        .endm
        .macro PSTORE src:req, mem:req
                strh \src, [\mem]
        .endm
        .macro PSTORE_2 src:req, mem1:req, mem2:req
                strh \src, [\mem1, \mem2]
        .endm
        .macro PSTORE_LSL src:req, mem1:req, mem2:req
                strh \src, [\mem1, \mem2, lsl #1]
        .endm
        .macro PSTORE_LSL_M1 src:req, mem1:req, mem2:req, temp_reg:req
                strh \src, [\mem1, \mem2]
        .endm
.equ PMULT , (1 << 1)
.equ PMULT_2 , (2 << 1)
.equ kMatchSpecLen_Error_Data , (1 << 9)
 # x7 t0 : NORM_CALC : prob2 (IF_BIT_1)
 # x6 t1 : NORM_CALC : probs_state
 # x8 t2 : (LITM) temp : (TREE) temp
 # x4 t3 : (LITM) bit : (TREE) temp : UPDATE_0/UPDATE_0 temp
 # x10 t4 : (LITM) offs : (TREE) probs_PMULT : numBits
 # x9 t5 : (LITM) match : sym2 (ShortDist)
 # x1 t6 : (LITM) litm_prob : (TREE) prob_reg : pbPos
 # x2 t7 : (LITM) prm : probBranch : cnt
 # x3 sym : dist
 # x12 len
 # x0 range
 # x5 cod
.equ kNumBitModelTotalBits , 11
.equ kBitModelTotal , (1 << kNumBitModelTotalBits)
.equ kNumMoveBits , 5
.equ kBitModelOffset , (kBitModelTotal - (1 << kNumMoveBits) + 1)
.macro NORM_2 macro
        ldrb w7, [x15], 1
        shl w0, 8
        orr w5, w7, w5, lsl 8
.endm
.macro TEST_HIGH_BYTE_range macro
        tst w0, 0xFF000000
.endm
.macro NORM macro
        TEST_HIGH_BYTE_range
        jnz 1f
        NORM_2
1:
.endm
 # ---------- Branch MACROS ----------
.macro UPDATE_0__0
        sub w7, w2, kBitModelOffset
.endm
.macro UPDATE_0__1
        sub w2, w2, w7, asr #(kNumMoveBits)
.endm
.macro UPDATE_0__2 probsArray:req, probOffset:req, probDisp:req
     .if \probDisp == 0
        PSTORE_2 w2, \probsArray, \probOffset
    .elseif \probOffset == 0
        PSTORE_2 w2, \probsArray, \probDisp * PMULT
    .else
        .error "unsupported"
        PSTORE_2 w2, x4, \probDisp * PMULT
    .endif
.endm
.macro UPDATE_0 probsArray:req, probOffset:req, probDisp:req
        UPDATE_0__0
        UPDATE_0__1
        UPDATE_0__2 \probsArray, \probOffset, \probDisp
.endm
.macro UPDATE_1 probsArray:req, probOffset:req, probDisp:req
        p2_sub w5, w0
        sub w0, w7, w0
        sub w7, w2, w2, lsr #(kNumMoveBits)
    .if \probDisp == 0
        PSTORE_2 w7, \probsArray, \probOffset
    .elseif \probOffset == 0
        PSTORE_2 w7, \probsArray, \probDisp * PMULT
    .else
        .error "unsupported"
        PSTORE_2 w7, x4, \probDisp * PMULT
    .endif
.endm
.macro CMP_COD_BASE
        NORM
        mov w7, w0
        shr w0, kNumBitModelTotalBits
        imul w0, w2
        cmp w5, w0
.endm
.macro CMP_COD_1 probsArray:req
        PLOAD w2, \probsArray
        CMP_COD_BASE
.endm
.macro CMP_COD_3 probsArray:req, probOffset:req, probDisp:req
    .if \probDisp == 0
        PLOAD_2 w2, \probsArray, \probOffset
    .elseif \probOffset == 0
        PLOAD_2 w2, \probsArray, \probDisp * PMULT
    .else
        .error "unsupported"
        add x4, \probsArray, \probOffset
        PLOAD_2 w2, x4, \probDisp * PMULT
    .endif
        CMP_COD_BASE
.endm
.macro IF_BIT_1_NOUP probsArray:req, probOffset:req, probDisp:req, toLabel:req
        CMP_COD_3 \probsArray, \probOffset, \probDisp
        jae \toLabel
.endm
.macro IF_BIT_1 probsArray:req, probOffset:req, probDisp:req, toLabel:req
        IF_BIT_1_NOUP \probsArray, \probOffset, \probDisp, \toLabel
        UPDATE_0 \probsArray, \probOffset, \probDisp
.endm
.macro IF_BIT_0_NOUP probsArray:req, probOffset:req, probDisp:req, toLabel:req
        CMP_COD_3 \probsArray, \probOffset, \probDisp
        jb \toLabel
.endm
.macro IF_BIT_0_NOUP_1 probsArray:req, toLabel:req
        CMP_COD_1 \probsArray
        jb \toLabel
.endm
 # ---------- CMOV MACROS ----------
.macro NORM_LSR
        NORM
        lsr w7, w0, #kNumBitModelTotalBits
.endm
.macro COD_RANGE_SUB
        subs w6, w5, w7
        p2_sub w0, w7
.endm
.macro RANGE_IMUL prob:req
        imul w7, \prob
.endm
.macro NORM_CALC prob:req
        NORM_LSR
        RANGE_IMUL \prob
        COD_RANGE_SUB
.endm
.macro CMOV_range
        cmovb w0, w7
.endm
.macro CMOV_code
        cmovae w5, w6
.endm
.macro CMOV_code_Model_Pre prob:req
        sub w7, \prob, kBitModelOffset
        CMOV_code
        cmovae w7, \prob
.endm
.macro PUP_BASE_2 prob:req, dest_reg:req
        # only sar works for both 16/32 w4 prob modes
        sub \dest_reg, \prob, \dest_reg, asr #(kNumMoveBits)
.endm
.macro PUP prob:req, probPtr:req, mem2:req
        PUP_BASE_2 \prob, w7
        PSTORE_2 w7, \probPtr, \mem2
.endm
.macro BIT_01
        add x10, x11, PMULT
.endm
.macro BIT_0_R prob:req
        PLOAD_2 \prob, x11, 1 * PMULT
        NORM_LSR
            sub w4, \prob, kBitModelOffset
        RANGE_IMUL \prob
            PLOAD_2 w8, x11, 1 * PMULT_2
        COD_RANGE_SUB
        CMOV_range
            cmovae w4, \prob
        PLOAD_2 w7, x11, 1 * PMULT_2 + PMULT
            PUP_BASE_2 \prob, w4
        csel \prob, w8, w7, lo
            CMOV_code
        mov w3, 2
        PSTORE_2 w4, x11, 1 * PMULT
            adc w3, w3, wzr
        BIT_01
.endm
.macro BIT_1_R prob:req
        NORM_LSR
            p2_add w3, w3
            sub w4, \prob, kBitModelOffset
        RANGE_IMUL \prob
            PLOAD_LSL w8, x11, x3
        COD_RANGE_SUB
        CMOV_range
            cmovae w4, \prob
        PLOAD_LSL w7, x10, x3
            PUP_BASE_2 \prob, w4
        csel \prob, w8, w7, lo
            CMOV_code
        PSTORE_LSL_M1 w4, x11, x3, x8
            adc w3, w3, wzr
.endm
.macro BIT_2_R prob:req
        NORM_LSR
            p2_add w3, w3
            sub w4, \prob, kBitModelOffset
        RANGE_IMUL \prob
        COD_RANGE_SUB
        CMOV_range
            cmovae w4, \prob
            CMOV_code
            PUP_BASE_2 \prob, w4
        PSTORE_LSL_M1 w4, x11, x3, x8
            adc w3, w3, wzr
.endm
 # ---------- MATCHED LITERAL ----------
.macro LITM_0 macro
        shl w9, (1 + 1)
        and w4, w9, 256 * PMULT
        add x2, x11, 256 * PMULT + 1 * PMULT
        p2_add w9, w9
        p2_add x2, x4
        eor w10, w4, 256 * PMULT
        PLOAD w1, x2
        NORM_LSR
            sub w8, w1, kBitModelOffset
        RANGE_IMUL w1
        COD_RANGE_SUB
        cmovae w10, w4
            CMOV_range
        and w4, w9, w10
            cmovae w8, w1
            CMOV_code
            mov w3, 2
        PUP_BASE_2 w1, w8
        PSTORE w8, x2
        add x2, x11, x10
        adc w3, w3, wzr
.endm
.macro LITM macro
        p2_add x2, x4
            xor w10, w4
        PLOAD_LSL w1, x2, x3
        NORM_LSR
            p2_add w9, w9
            sub w8, w1, kBitModelOffset
        RANGE_IMUL w1
        COD_RANGE_SUB
        cmovae w10, w4
            CMOV_range
        and w4, w9, w10
            cmovae w8, w1
            CMOV_code
        PUP_BASE_2 w1, w8
        PSTORE_LSL w8, x2, x3
        add x2, x11, x10
        adc w3, w3, w3
.endm
.macro LITM_2 macro
        p2_add x2, x4
        PLOAD_LSL w1, x2, x3
        NORM_LSR
            sub w8, w1, kBitModelOffset
        RANGE_IMUL w1
        COD_RANGE_SUB
            CMOV_range
            cmovae w8, w1
            CMOV_code
        PUP_BASE_2 w1, w8
        PSTORE_LSL w8, x2, x3
        adc w3, w3, w3
.endm
 # ---------- REVERSE BITS ----------
.macro REV_0 prob:req
        NORM_CALC \prob
        CMOV_range
        PLOAD w8, x9
        PLOAD_2 w4, x11, 3 * PMULT
        CMOV_code_Model_Pre \prob
        add x6, x11, 3 * PMULT
        cmovae x9, x6
        PUP \prob, x11, 1 * PMULT
        csel \prob, w8, w4, lo
.endm
.macro REV_1 prob:req, step:req
        NORM_LSR
            PLOAD_PREINDEXED w8, x9, (\step * PMULT)
        RANGE_IMUL \prob
        COD_RANGE_SUB
        CMOV_range
        PLOAD_2 w4, x9, (\step * PMULT)
        sub w7, \prob, kBitModelOffset
        CMOV_code
        add x6, x9, \step * PMULT
        cmovae w7, \prob
        cmovae x9, x6
        PUP_BASE_2 \prob, w7
        csel \prob, w8, w4, lo
        PSTORE_2 w7, x6, 0 - \step * PMULT_2
.endm
.macro REV_2 prob:req, step:req
        sub x6, x9, x11
        NORM_LSR
            orr w3, w3, w6, lsr #1
        RANGE_IMUL \prob
        COD_RANGE_SUB
        sub w8, w3, \step
        CMOV_range
        cmovb w3, w8
        CMOV_code_Model_Pre \prob
        PUP \prob, x9, 0
.endm
.macro REV_1_VAR prob:req
        PLOAD \prob, x3
        mov x11, x3
        p2_add x3, x9
        NORM_LSR
            add x8, x3, x9
        RANGE_IMUL \prob
        COD_RANGE_SUB
        cmovae x3, x8
        CMOV_range
        CMOV_code_Model_Pre \prob
        p2_add w9, w9
        PUP \prob, x11, 0
.endm
.macro add_big dest:req, src:req, param:req
    .if (\param) < (1 << 12)
        add \dest, \src, \param
    .else
          .error "unexpcted add_big expansion"
        add \dest, \src, (\param) / 2
        add \dest, \dest, (\param) - (\param) / 2
    .endif
.endm
.macro sub_big dest:req, src:req, param:req
    .if (\param) < (1 << 12)
        sub \dest, \src, \param
    .else
          .error "unexpcted sub_big expansion"
        sub \dest, \src, (\param) / 2
        sub \dest, \dest, (\param) - (\param) / 2
    .endif
.endm
.macro SET_probs offset:req
        add x11, x25, ((\offset) - IsMatch) * PMULT
.endm
.macro LIT_PROBS
        add w3, w3, w28, lsl 8
        inc w28
        UPDATE_0__0
        shl w3, w30
        SET_probs Literal
        p2_and w3, w30
        p2_add x11, x3
        UPDATE_0__1
        add x11, x11, x3, lsl 1
        UPDATE_0__2 x6, x1, 0
.endm
.equ kNumPosBitsMax , 4
.equ kNumPosStatesMax , (1 << kNumPosBitsMax)
.equ kLenNumLowBits , 3
.equ kLenNumLowSymbols , (1 << kLenNumLowBits)
.equ kLenNumHighBits , 8
.equ kLenNumHighSymbols , (1 << kLenNumHighBits)
.equ kNumLenProbs , (2 * kLenNumLowSymbols * kNumPosStatesMax + kLenNumHighSymbols)
.equ LenLow , 0
.equ LenChoice , LenLow
.equ LenChoice2 , (LenLow + kLenNumLowSymbols)
.equ LenHigh , (LenLow + 2 * kLenNumLowSymbols * kNumPosStatesMax)
.equ kNumStates , 12
.equ kNumStates2 , 16
.equ kNumLitStates , 7
.equ kStartPosModelIndex , 4
.equ kEndPosModelIndex , 14
.equ kNumFullDistances , (1 << (kEndPosModelIndex >> 1))
.equ kNumPosSlotBits , 6
.equ kNumLenToPosStates , 4
.equ kNumAlignBits , 4
.equ kAlignTableSize , (1 << kNumAlignBits)
.equ kMatchMinLen , 2
.equ kMatchSpecLenStart , (kMatchMinLen + kLenNumLowSymbols * 2 + kLenNumHighSymbols)
.equ kStartOffset , 0
.equ SpecPos , (-kStartOffset)
.equ IsRep0Long , (SpecPos + kNumFullDistances)
.equ RepLenCoder , (IsRep0Long + (kNumStates2 << kNumPosBitsMax))
.equ LenCoder , (RepLenCoder + kNumLenProbs)
.equ IsMatch , (LenCoder + kNumLenProbs)
.equ kAlign , (IsMatch + (kNumStates2 << kNumPosBitsMax))
.equ IsRep , (kAlign + kAlignTableSize)
.equ IsRepG0 , (IsRep + kNumStates)
.equ IsRepG1 , (IsRepG0 + kNumStates)
.equ IsRepG2 , (IsRepG1 + kNumStates)
.equ PosSlot , (IsRepG2 + kNumStates)
.equ Literal , (PosSlot + (kNumLenToPosStates << kNumPosSlotBits))
.equ NUM_BASE_PROBS , (Literal + kStartOffset)
.if kStartOffset != 0
  .error "Stop_Compiling_Bad_StartOffset"
.endif
.if NUM_BASE_PROBS != 1984
  .error "Stop_Compiling_Bad_LZMA_PROBS"
.endif
.equ offset_lc , 0
.equ offset_lp , 1
.equ offset_pb , 2
.equ offset_dicSize , 4
.equ offset_probs , 4 + offset_dicSize
.equ offset_probs_1664 , 8 + offset_probs
.equ offset_dic , 8 + offset_probs_1664
.equ offset_dicBufSize , 8 + offset_dic
.equ offset_dicPos , 8 + offset_dicBufSize
.equ offset_buf , 8 + offset_dicPos
.equ offset_range , 8 + offset_buf
.equ offset_code , 4 + offset_range
.equ offset_processedPos , 4 + offset_code
.equ offset_checkDicSize , 4 + offset_processedPos
.equ offset_rep0 , 4 + offset_checkDicSize
.equ offset_rep1 , 4 + offset_rep0
.equ offset_rep2 , 4 + offset_rep1
.equ offset_rep3 , 4 + offset_rep2
.equ offset_state , 4 + offset_rep3
.equ offset_remainLen , 4 + offset_state
.equ offset_TOTAL_SIZE , 4 + offset_remainLen
.if offset_TOTAL_SIZE != 96
  .error "Incorrect offset_TOTAL_SIZE"
.endif
.macro IsMatchBranch_Pre
        # prob = x11 + IsMatch + (w13 << kNumPosBitsMax) + posState;
        and w1, w29, w28, lsl #(kLenNumLowBits + 1 + 1)
        add x6, x25, x13
.endm
.macro CheckLimits
        cmp x15, x16
        jae Lfin_OK
        cmp x14, x19
        jae Lfin_OK
.endm
.macro LOAD_LZMA_VAR reg:req, struct_offs:req
        ldr \reg, [x0, \struct_offs]
.endm
.macro LOAD_LZMA_BYTE reg:req, struct_offs:req
        ldrb \reg, [x0, \struct_offs]
.endm
.macro LOAD_LZMA_PAIR reg0:req, reg1:req, struct_offs:req
        ldp \reg0, \reg1, [x0, \struct_offs]
.endm
lzma_rust2_lzma_decode_real:
_lzma_rust2_lzma_decode_real:
 stp x19, x20, [sp, -128]!
 stp x21, x22, [sp, 16]
 stp x23, x24, [sp, 32]
 stp x25, x26, [sp, 48]
 stp x27, x28, [sp, 64]
 stp x29, x30, [sp, 80]
        str x0, [sp, 120]
        mov x16, x2
        mov x19, x1
        LOAD_LZMA_PAIR x24, x17, offset_dic
        LOAD_LZMA_PAIR x14, x15, offset_dicPos
        LOAD_LZMA_PAIR w20, w21, offset_rep0
        LOAD_LZMA_PAIR w22, w23, offset_rep2
        mov w7, 1 << (kLenNumLowBits + 1 + 1)
        LOAD_LZMA_BYTE w29, offset_pb
        p2_add x19, x24
        mov w12, wzr
        lsl w29, w7, w29
        p2_add x14, x24
        p2_sub w29, w7
        LOAD_LZMA_BYTE w30, offset_lc
        mov w7, 256 << 1
        LOAD_LZMA_BYTE w6, offset_lp
        p2_add w6, w30
        p2_sub w30, (256 << 1) - 1
        shl w7, w6
        p2_add w30, w7
        LOAD_LZMA_VAR x26, offset_probs
        LOAD_LZMA_VAR w27, offset_checkDicSize
        LOAD_LZMA_VAR w28, offset_processedPos
        LOAD_LZMA_VAR w13, offset_state
        LOAD_LZMA_PAIR w0, w5, offset_range
        mov w3, wzr
        shl w13, 1
        add_big x25, x26, ((IsMatch - SpecPos) << 1)
        orr w7, w27, w28
        cbz w7, 1f
        add x7, x17, x24
        cmp x14, x24
        cmovne x7, x14
        ldrb w3, [x7, -1]
1:
        IsMatchBranch_Pre
        cmp w13, 4 * PMULT
        jb Llit_end
        cmp w13, kNumLitStates * PMULT
        jb Llit_matched_end
        jmp Llz_end
 # ---------- LITERAL ----------
MY_ALIGN_64
Llit_start:
        mov w13, wzr
Llit_start_2:
        LIT_PROBS
        BIT_0_R w1
        BIT_1_R w1
        BIT_1_R w1
        BIT_1_R w1
        BIT_1_R w1
        BIT_1_R w1
        BIT_1_R w1
        BIT_2_R w1
        IsMatchBranch_Pre
        strb w3, [x14], 1
        p2_and w3, 255
        CheckLimits
Llit_end:
        IF_BIT_0_NOUP x6, x1, (IsMatch - IsMatch), Llit_start
        # jmp LIsMatch_label
 # ---------- MATCHES ----------
 # MY_ALIGN_32
LIsMatch_label:
        UPDATE_1 x6, x1, (IsMatch - IsMatch)
        IF_BIT_1 x6, 0, (IsRep - IsMatch), LIsRep_label
        SET_probs LenCoder
        or w13, (1 << (4 + 1))
 # ---------- LEN DECODE ----------
Llen_decode:
        mov w12, 8 - kMatchMinLen
        IF_BIT_0_NOUP_1 x11, Llen_mid_0
        UPDATE_1 x11, 0, 0
        p2_add x11, (1 << (kLenNumLowBits + 1))
        mov w12, 0 - kMatchMinLen
        IF_BIT_0_NOUP_1 x11, Llen_mid_0
        UPDATE_1 x11, 0, 0
        p2_add x11, LenHigh * PMULT - (1 << (kLenNumLowBits + 1))
        PLOAD_2 w1, x11, 1 * PMULT
        mov w3, 1
        BIT_01
MY_ALIGN_32
Llen8_loop:
        BIT_1_R w1
        tbz w3, 6, Llen8_loop
        mov w12, (kLenNumHighSymbols - kLenNumLowSymbols * 2) - kMatchMinLen
        jmp Llen_mid_2
MY_ALIGN_32
Llen_mid_0:
        UPDATE_0 x11, 0, 0
        p2_add x11, x1
        BIT_0_R w1
Llen_mid_2:
        BIT_1_R w1
        BIT_2_R w1
        sub w12, w3, w12
        tbz w13, (4 + 1), Lcopy_match
 # ---------- DECODE DISTANCE ----------
        mov w7, 3 + kMatchMinLen
        cmp w12, 3 + kMatchMinLen
        cmovb w7, w12
        SET_probs PosSlot - (kMatchMinLen << (kNumPosSlotBits))
        add x11, x11, x7, lsl #(kNumPosSlotBits + 1)
        BIT_0_R w1
        BIT_1_R w1
        BIT_1_R w1
        BIT_1_R w1
        BIT_1_R w1
        mov w10, w3
        BIT_2_R w1
        p2_and w3, 3
        cmp w10, 32 + kEndPosModelIndex / 2
        jb Lshort_dist
        SET_probs kAlign
        # unsigned numDirectBits = (unsigned)(((distance >> 1) - 1));
        p2_sub w10, (32 + 1 + kNumAlignBits)
        # distance = (2 | (distance & 1));
        or w3, 2
        PLOAD_2 w1, x11, 1 * PMULT
        add x9, x11, 2 * PMULT
 # ---------- DIRECT DISTANCE ----------
.macro DIRECT_1
        shr w0, 1
        subs w7, w5, w0
        p2_add w3, w3
        csel w5, w5, w7, mi
        csinc w3, w3, w3, mi
        dec_s w10
        je Ldirect_end
.endm
.macro DIRECT_2
        TEST_HIGH_BYTE_range
        jz Ldirect_unroll
        DIRECT_1
.endm
        DIRECT_2
        DIRECT_2
        DIRECT_2
        DIRECT_2
        DIRECT_2
        DIRECT_2
        DIRECT_2
        DIRECT_2
Ldirect_unroll:
        NORM_2
        DIRECT_1
        DIRECT_1
        DIRECT_1
        DIRECT_1
        DIRECT_1
        DIRECT_1
        DIRECT_1
        DIRECT_1
        jmp Ldirect_unroll
MY_ALIGN_32
Ldirect_end:
        shl w3, kNumAlignBits
        REV_0 w1
        REV_1 w1, 2
        REV_1 w1, 4
        REV_2 w1, 8
Ldecode_dist_end:
        tst w27, w27
        csel w7, w28, w27, eq
        cmp w3, w7
        jae Lend_of_payload
        mov w23, w22
        mov w22, w21
        mov w21, w20
        add w20, w3, 1
.macro STATE_UPDATE_FOR_MATCH
        cmp w13, kNumLitStates * PMULT + (1 << (4 + 1))
        mov w13, kNumLitStates * PMULT
        mov w7, (kNumLitStates + 3) * PMULT
        cmovae w13, w7
.endm
        STATE_UPDATE_FOR_MATCH
 # ---------- COPY MATCH ----------
Lcopy_match:
        subs x2, x19, x14
        jz Lfin_OK
        cmp x2, x12
        cmovae w2, w12
        sub x7, x14, x24
        p2_add x14, x2
        p2_add w28, w2
        p2_sub w12, w2
        p2_sub_s x7, x20
        jae 1f
        cmn x7, x2
        p2_add x7, x17
        ja Lcopy_match_cross
1:
 # ---------- COPY MATCH FAST ----------
    # x7 : src_pos
    # The SDK copies a byte at a time. A match whose source lies at least
    # sixteen (or eight) bytes from its destination in the window, either
    # way round, is copied in words of that size instead, the last word
    # overlapping the one before it so that exactly the match's bytes are
    # written. A source that far behind holds only bytes written before the
    # word that reads it, and one that far ahead (a distance near the
    # window's size, wrapped) is never overwritten before it is read. The
    # source is known not to wrap here.
        add x7, x7, x24
        sub x3, x14, x2
        cmp x2, 16
        jb Lcopy_lt16
        sub x9, x7, x3
        cmp x9, 16
        b.ge Lcopy_v16
        cmn x9, 16
        b.gt Lcopy_lt16
Lcopy_v16:
        sub x4, x2, 16
        add x9, x7, x4
        add x10, x3, x4
2:
        ldr q0, [x7], 16
        subs x4, x4, 16
        str q0, [x3], 16
        jae 2b
        ldr q0, [x9]
        str q0, [x10]
        jmp Lcopy_words_end
Lcopy_lt16:
        cmp x2, 8
        jb Lcopy_short
        sub x9, x7, x3
        cmp x9, 8
        b.ge Lcopy_v8
        cmn x9, 8
        b.gt Lcopy_bytes
Lcopy_v8:
        sub x4, x2, 8
        add x9, x7, x4
        add x10, x3, x4
2:
        ldr x8, [x7], 8
        subs x4, x4, 8
        str x8, [x3], 8
        jae 2b
        ldr x8, [x9]
        str x8, [x10]
Lcopy_words_end:
        dec x14
        ldrb w3, [x14]
        IsMatchBranch_Pre
        jmp Lcopy_end
 # ---------- COPY SHORT MATCH ----------
    # A match shorter than a word, with the same distance condition and both
    # words inside the window: one word read from the source, one from the
    # destination, and the destination's bytes past the match kept, so that
    # the copy is a few instructions with no loop to mispredict.
Lcopy_short:
        sub x9, x7, x3
        cmp x9, 8
        b.ge 3f
        cmn x9, 8
        b.gt Lcopy_bytes
3:
        add x4, x24, x17
        add x8, x7, 8
        cmp x8, x4
        b.hi Lcopy_bytes
        add x8, x3, 8
        cmp x8, x4
        b.hi Lcopy_bytes
        ldr x8, [x7]
        ldr x10, [x3]
        lsl x4, x2, 3
        mov x9, -1
        lsl x9, x9, x4
        and x10, x10, x9
        bic x8, x8, x9
        orr x8, x8, x10
        str x8, [x3]
        jmp Lcopy_words_end
Lcopy_bytes:
        ldrb w3, [x7]
        p2_add x7, x2
        p1_neg x2
Lcopy_common:
        dec x14
    # x14 : (ptr_to_last_dest_BYTE) 
    # x7 : (src_lim)
    # x2 : (-curLen)
        IsMatchBranch_Pre
        inc_s x2
        jz Lcopy_end
        cmp w20, 1
        je Lcopy_match_0
MY_ALIGN_32
1:
        strb w3, [x14, x2]
        ldrb w3, [x7, x2]
        inc_s x2
        jz Lcopy_end
        strb w3, [x14, x2]
        ldrb w3, [x7, x2]
        inc_s x2
        jnz 1b
Lcopy_end:
Llz_end_match:
        strb w3, [x14], 1
        # IsMatchBranch_Pre
        CheckLimits
Llz_end:
        IF_BIT_1_NOUP x6, x1, (IsMatch - IsMatch), LIsMatch_label
 # ---------- LITERAL MATCHED ----------
        LIT_PROBS
        sub x7, x14, x24
        p2_sub_s x7, x20
        jae 1f
        p2_add x7, x17
1:
        ldrb w9, [x24, x7]
        sub w3, w13, 6 * PMULT
        cmp w13, 10 * PMULT
        p2_sub w13, 3 * PMULT
        cmovae w13, w3
        LITM_0
        LITM
        LITM
        LITM
        LITM
        LITM
        LITM
        LITM_2
        IsMatchBranch_Pre
        strb w3, [x14], 1
        p2_and w3, 255
        CheckLimits
Llit_matched_end:
        IF_BIT_1_NOUP x6, x1, (IsMatch - IsMatch), LIsMatch_label
        # IsMatchBranch
        p2_sub w13, 3 * PMULT
        jmp Llit_start_2
 # ---------- REP 0 LITERAL ----------
MY_ALIGN_32
LIsRep0Short_label:
        UPDATE_0 x6, x1, 0
        sub x7, x14, x24
        or w13, 1 * PMULT
        # the caller doesn't allow (dicPos >= limit) case for REP_SHORT
        # so we don't need the following (dicPos == limit) check here:
        # cmp x14, x19
        # jae fin_dicPos_LIMIT_REP_SHORT
        inc w28
        IsMatchBranch_Pre
        p2_sub_s x7, x20
        jae 1f
        p2_add x7, x17
1:
        ldrb w3, [x24, x7]
        jmp Llz_end_match
MY_ALIGN_32
LIsRep_label:
        UPDATE_1 x6, 0, (IsRep - IsMatch)
        # The (w27 == 0 && w28 == 0) case was checked before in LzmaDec.c with kBadRepCode.
        # So we don't check it here.
        # mov w7, w28
        # or w7, w27
        # jz fin_ERROR_2
        cmp w13, kNumLitStates * PMULT
        mov w13, 8 * PMULT
        mov w2, 11 * PMULT
        cmovae w13, w2
        SET_probs RepLenCoder
        IF_BIT_1 x6, 0, (IsRepG0 - IsMatch), LIsRepG0_label
        sub_big x6, x6, (IsMatch - IsRep0Long) << 1
        IF_BIT_0_NOUP x6, x1, 0, LIsRep0Short_label
        UPDATE_1 x6, x1, 0
        jmp Llen_decode
MY_ALIGN_32
LIsRepG0_label:
        UPDATE_1 x6, 0, (IsRepG0 - IsMatch)
        IF_BIT_1 x6, 0, (IsRepG1 - IsMatch), LIsRepG1_label
        mov w3, w21
        mov w21, w20
        mov w20, w3
        jmp Llen_decode
 # MY_ALIGN_32
LIsRepG1_label:
        UPDATE_1 x6, 0, (IsRepG1 - IsMatch)
        IF_BIT_1 x6, 0, (IsRepG2 - IsMatch), LIsRepG2_label
        mov w3, w22
        mov w22, w21
        mov w21, w20
        mov w20, w3
        jmp Llen_decode
 # MY_ALIGN_32
LIsRepG2_label:
        UPDATE_1 x6, 0, (IsRepG2 - IsMatch)
        mov w3, w23
        mov w23, w22
        mov w22, w21
        mov w21, w20
        mov w20, w3
        jmp Llen_decode
 # ---------- SPEC SHORT DISTANCE ----------
MY_ALIGN_32
Lshort_dist:
        p2_sub_s w10, 32 + 1
        jbe Ldecode_dist_end
        or w3, 2
        shl w3, w10
        add x3, x26, x3, lsl #1
        p2_add x3, SpecPos * PMULT + 1 * PMULT
        mov w9, PMULT
MY_ALIGN_32
Lspec_loop:
        REV_1_VAR w1
        dec_s w10
        jnz Lspec_loop
        p2_add x9, x26
    .if SpecPos != 0
        p2_add x9, SpecPos * PMULT
    .endif
        p2_sub x3, x9
        shr w3, 1
        jmp Ldecode_dist_end
 # ---------- COPY MATCH 0 ----------
MY_ALIGN_32
Lcopy_match_0:
        strb w3, [x14, x2]
        inc_s x2
        jz Lcopy_end
        strb w3, [x14, x2]
        inc_s x2
        jz Lcopy_end
        strb w3, [x14, x2]
        inc_s x2
        jz Lcopy_end
        orr w4, w3, w3, lsl 8
        p2_and x2, -4
        orr w4, w4, w4, lsl 16
MY_ALIGN_16
1:
        str w4, [x14, x2]
        adds x2, x2, 4
        jnz 1b
2:
    jmp Lcopy_end
 # ---------- COPY MATCH CROSS ----------
Lcopy_match_cross:
        # x7 - src pos
        # x2 - total copy w12
        p1_neg x2
1:
        ldrb w3, [x24, x7]
        inc x7
        strb w3, [x14, x2]
        inc x2
        cmp x7, x17
        jne 1b
        ldrb w3, [x24]
        sub x7, x24, x2
        jmp Lcopy_common
Lfin_ERROR_MATCH_DIST:
        # w20 = distance + 1;
        p2_add w12, kMatchSpecLen_Error_Data
        mov w23, w22
        mov w22, w21
        mov w21, w20
        mov w20, w3
        STATE_UPDATE_FOR_MATCH
        # jmp Lfin_OK
        mov w3, 1
        jmp Lfin
Lend_of_payload:
        inc_s w3
        jnz Lfin_ERROR_MATCH_DIST
        mov w12, kMatchSpecLenStart
        xor w13, (1 << (4 + 1))
        jmp Lfin_OK
Lfin_OK:
        mov w3, wzr
Lfin:
        NORM
   .macro STORE_LZMA_VAR reg:req, struct_offs:req
        str \reg, [x7, \struct_offs]
   .endm
   .macro STORE_LZMA_PAIR reg0:req, reg1:req, struct_offs:req
        stp \reg0, \reg1, [x7, \struct_offs]
   .endm
        ldr x7, [sp, 120]
        p2_sub x14, x24
        shr w13, 1
        STORE_LZMA_PAIR x14, x15, offset_dicPos
        STORE_LZMA_PAIR w0, w5, offset_range
        STORE_LZMA_VAR w28, offset_processedPos
        STORE_LZMA_PAIR w20, w21, offset_rep0
        STORE_LZMA_PAIR w22, w23, offset_rep2
        STORE_LZMA_PAIR w13, w12, offset_state
        mov w0, w3
 ldp x29, x30, [sp, 80]
 ldp x27, x28, [sp, 64]
 ldp x25, x26, [sp, 48]
        ldp x23, x24, [sp, 32]
 ldp x21, x22, [sp, 16]
 ldp x19, x20, [sp], 128
        ret
