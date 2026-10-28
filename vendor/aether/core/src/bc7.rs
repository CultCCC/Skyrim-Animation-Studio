//! BC7 (BPTC) block decoding, std-only.
//!
//! Modern SSE-era mod textures ship as BC7 behind a DX10 header — the Zodiac
//! weapons are all BC7 — so a viewer without it shows white models while the
//! files resolve perfectly.
//!
//! Algorithm and tables follow the detex decoder (ISC licence, Harm
//! Hanemaaijer) as vendored in NifSkope's libfo76utils
//! (`decompress-bptc.c`, `bptc-tables.c`), re-expressed over one `u128`
//! bit stream instead of two juggled 64-bit words. The partition, anchor and
//! weight tables are transcribed verbatim — they are format constants from
//! the D3D spec, and retyping them from memory is how decoders grow subtle
//! per-partition bugs.

/// Subsets per mode.
const NS: [usize; 8] = [3, 2, 3, 2, 1, 1, 1, 2];
/// Partition-selection bits per mode.
const PB: [u32; 8] = [4, 6, 6, 6, 0, 0, 0, 6];
/// Rotation bits per mode.
const RB: [u32; 8] = [0, 0, 0, 0, 2, 2, 0, 0];
/// Colour endpoint precision, without P-bit.
const COLOR_PREC: [u32; 8] = [4, 6, 5, 7, 5, 7, 7, 5];
/// Colour precision including the P-bit.
const COLOR_PREC_P: [u32; 8] = [5, 7, 5, 8, 5, 7, 8, 6];
/// Alpha endpoint precision, without P-bit (0 = no stored alpha).
const ALPHA_PREC: [u32; 8] = [0, 0, 0, 0, 6, 8, 7, 5];
/// Alpha precision including the P-bit.
const ALPHA_PREC_P: [u32; 8] = [0, 0, 0, 0, 6, 8, 8, 6];
/// Primary index bits per mode.
const IB: [u32; 8] = [3, 3, 2, 2, 2, 2, 4, 2];
/// Secondary index bits (modes with separate alpha indices).
const IB2: [u32; 8] = [0, 0, 0, 0, 3, 2, 0, 0];
/// P-bit scheme: 0 none, 1 unique per endpoint, 2 shared per subset.
const PBIT_MODE: [u32; 8] = [1, 2, 0, 1, 0, 0, 1, 1];

/// Interpolation weights, `(a*(64-w) + b*w + 32) >> 6`.
const W2: [u16; 4] = [0, 21, 43, 64];
const W3: [u16; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const W4: [u16; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

#[rustfmt::skip]
const P2: [[u8; 16]; 64] = [
    [0,0,1,1,0,0,1,1,0,0,1,1,0,0,1,1],
    [0,0,0,1,0,0,0,1,0,0,0,1,0,0,0,1],
    [0,1,1,1,0,1,1,1,0,1,1,1,0,1,1,1],
    [0,0,0,1,0,0,1,1,0,0,1,1,0,1,1,1],
    [0,0,0,0,0,0,0,1,0,0,0,1,0,0,1,1],
    [0,0,1,1,0,1,1,1,0,1,1,1,1,1,1,1],
    [0,0,0,1,0,0,1,1,0,1,1,1,1,1,1,1],
    [0,0,0,0,0,0,0,1,0,0,1,1,0,1,1,1],
    [0,0,0,0,0,0,0,0,0,0,0,1,0,0,1,1],
    [0,0,1,1,0,1,1,1,1,1,1,1,1,1,1,1],
    [0,0,0,0,0,0,0,1,0,1,1,1,1,1,1,1],
    [0,0,0,0,0,0,0,0,0,0,0,1,0,1,1,1],
    [0,0,0,1,0,1,1,1,1,1,1,1,1,1,1,1],
    [0,0,0,0,0,0,0,0,1,1,1,1,1,1,1,1],
    [0,0,0,0,1,1,1,1,1,1,1,1,1,1,1,1],
    [0,0,0,0,0,0,0,0,0,0,0,0,1,1,1,1],
    [0,0,0,0,1,0,0,0,1,1,1,0,1,1,1,1],
    [0,1,1,1,0,0,0,1,0,0,0,0,0,0,0,0],
    [0,0,0,0,0,0,0,0,1,0,0,0,1,1,1,0],
    [0,1,1,1,0,0,1,1,0,0,0,1,0,0,0,0],
    [0,0,1,1,0,0,0,1,0,0,0,0,0,0,0,0],
    [0,0,0,0,1,0,0,0,1,1,0,0,1,1,1,0],
    [0,0,0,0,0,0,0,0,1,0,0,0,1,1,0,0],
    [0,1,1,1,0,0,1,1,0,0,1,1,0,0,0,1],
    [0,0,1,1,0,0,0,1,0,0,0,1,0,0,0,0],
    [0,0,0,0,1,0,0,0,1,0,0,0,1,1,0,0],
    [0,1,1,0,0,1,1,0,0,1,1,0,0,1,1,0],
    [0,0,1,1,0,1,1,0,0,1,1,0,1,1,0,0],
    [0,0,0,1,0,1,1,1,1,1,1,0,1,0,0,0],
    [0,0,0,0,1,1,1,1,1,1,1,1,0,0,0,0],
    [0,1,1,1,0,0,0,1,1,0,0,0,1,1,1,0],
    [0,0,1,1,1,0,0,1,1,0,0,1,1,1,0,0],
    [0,1,0,1,0,1,0,1,0,1,0,1,0,1,0,1],
    [0,0,0,0,1,1,1,1,0,0,0,0,1,1,1,1],
    [0,1,0,1,1,0,1,0,0,1,0,1,1,0,1,0],
    [0,0,1,1,0,0,1,1,1,1,0,0,1,1,0,0],
    [0,0,1,1,1,1,0,0,0,0,1,1,1,1,0,0],
    [0,1,0,1,0,1,0,1,1,0,1,0,1,0,1,0],
    [0,1,1,0,1,0,0,1,0,1,1,0,1,0,0,1],
    [0,1,0,1,1,0,1,0,1,0,1,0,0,1,0,1],
    [0,1,1,1,0,0,1,1,1,1,0,0,1,1,1,0],
    [0,0,0,1,0,0,1,1,1,1,0,0,1,0,0,0],
    [0,0,1,1,0,0,1,0,0,1,0,0,1,1,0,0],
    [0,0,1,1,1,0,1,1,1,1,0,1,1,1,0,0],
    [0,1,1,0,1,0,0,1,1,0,0,1,0,1,1,0],
    [0,0,1,1,1,1,0,0,1,1,0,0,0,0,1,1],
    [0,1,1,0,0,1,1,0,1,0,0,1,1,0,0,1],
    [0,0,0,0,0,1,1,0,0,1,1,0,0,0,0,0],
    [0,1,0,0,1,1,1,0,0,1,0,0,0,0,0,0],
    [0,0,1,0,0,1,1,1,0,0,1,0,0,0,0,0],
    [0,0,0,0,0,0,1,0,0,1,1,1,0,0,1,0],
    [0,0,0,0,0,1,0,0,1,1,1,0,0,1,0,0],
    [0,1,1,0,1,1,0,0,1,0,0,1,0,0,1,1],
    [0,0,1,1,0,1,1,0,1,1,0,0,1,0,0,1],
    [0,1,1,0,0,0,1,1,1,0,0,1,1,1,0,0],
    [0,0,1,1,1,0,0,1,1,1,0,0,0,1,1,0],
    [0,1,1,0,1,1,0,0,1,1,0,0,1,0,0,1],
    [0,1,1,0,0,0,1,1,0,0,1,1,1,0,0,1],
    [0,1,1,1,1,1,1,0,1,0,0,0,0,0,0,1],
    [0,0,0,1,1,0,0,0,1,1,1,0,0,1,1,1],
    [0,0,0,0,1,1,1,1,0,0,1,1,0,0,1,1],
    [0,0,1,1,0,0,1,1,1,1,1,1,0,0,0,0],
    [0,0,1,0,0,0,1,0,1,1,1,0,1,1,1,0],
    [0,1,0,0,0,1,0,0,0,1,1,1,0,1,1,1],
];

#[rustfmt::skip]
const P3: [[u8; 16]; 64] = [
    [0,0,1,1,0,0,1,1,0,2,2,1,2,2,2,2],
    [0,0,0,1,0,0,1,1,2,2,1,1,2,2,2,1],
    [0,0,0,0,2,0,0,1,2,2,1,1,2,2,1,1],
    [0,2,2,2,0,0,2,2,0,0,1,1,0,1,1,1],
    [0,0,0,0,0,0,0,0,1,1,2,2,1,1,2,2],
    [0,0,1,1,0,0,1,1,0,0,2,2,0,0,2,2],
    [0,0,2,2,0,0,2,2,1,1,1,1,1,1,1,1],
    [0,0,1,1,0,0,1,1,2,2,1,1,2,2,1,1],
    [0,0,0,0,0,0,0,0,1,1,1,1,2,2,2,2],
    [0,0,0,0,1,1,1,1,1,1,1,1,2,2,2,2],
    [0,0,0,0,1,1,1,1,2,2,2,2,2,2,2,2],
    [0,0,1,2,0,0,1,2,0,0,1,2,0,0,1,2],
    [0,1,1,2,0,1,1,2,0,1,1,2,0,1,1,2],
    [0,1,2,2,0,1,2,2,0,1,2,2,0,1,2,2],
    [0,0,1,1,0,1,1,2,1,1,2,2,1,2,2,2],
    [0,0,1,1,2,0,0,1,2,2,0,0,2,2,2,0],
    [0,0,0,1,0,0,1,1,0,1,1,2,1,1,2,2],
    [0,1,1,1,0,0,1,1,2,0,0,1,2,2,0,0],
    [0,0,0,0,1,1,2,2,1,1,2,2,1,1,2,2],
    [0,0,2,2,0,0,2,2,0,0,2,2,1,1,1,1],
    [0,1,1,1,0,1,1,1,0,2,2,2,0,2,2,2],
    [0,0,0,1,0,0,0,1,2,2,2,1,2,2,2,1],
    [0,0,0,0,0,0,1,1,0,1,2,2,0,1,2,2],
    [0,0,0,0,1,1,0,0,2,2,1,0,2,2,1,0],
    [0,1,2,2,0,1,2,2,0,0,1,1,0,0,0,0],
    [0,0,1,2,0,0,1,2,1,1,2,2,2,2,2,2],
    [0,1,1,0,1,2,2,1,1,2,2,1,0,1,1,0],
    [0,0,0,0,0,1,1,0,1,2,2,1,1,2,2,1],
    [0,0,2,2,1,1,0,2,1,1,0,2,0,0,2,2],
    [0,1,1,0,0,1,1,0,2,0,0,2,2,2,2,2],
    [0,0,1,1,0,1,2,2,0,1,2,2,0,0,1,1],
    [0,0,0,0,2,0,0,0,2,2,1,1,2,2,2,1],
    [0,0,0,0,0,0,0,2,1,1,2,2,1,2,2,2],
    [0,2,2,2,0,0,2,2,0,0,1,2,0,0,1,1],
    [0,0,1,1,0,0,1,2,0,0,2,2,0,2,2,2],
    [0,1,2,0,0,1,2,0,0,1,2,0,0,1,2,0],
    [0,0,0,0,1,1,1,1,2,2,2,2,0,0,0,0],
    [0,1,2,0,1,2,0,1,2,0,1,2,0,1,2,0],
    [0,1,2,0,2,0,1,2,1,2,0,1,0,1,2,0],
    [0,0,1,1,2,2,0,0,1,1,2,2,0,0,1,1],
    [0,0,1,1,1,1,2,2,2,2,0,0,0,0,1,1],
    [0,1,0,1,0,1,0,1,2,2,2,2,2,2,2,2],
    [0,0,0,0,0,0,0,0,2,1,2,1,2,1,2,1],
    [0,0,2,2,1,1,2,2,0,0,2,2,1,1,2,2],
    [0,0,2,2,0,0,1,1,0,0,2,2,0,0,1,1],
    [0,2,2,0,1,2,2,1,0,2,2,0,1,2,2,1],
    [0,1,0,1,2,2,2,2,2,2,2,2,0,1,0,1],
    [0,0,0,0,2,1,2,1,2,1,2,1,2,1,2,1],
    [0,1,0,1,0,1,0,1,0,1,0,1,2,2,2,2],
    [0,2,2,2,0,1,1,1,0,2,2,2,0,1,1,1],
    [0,0,0,2,1,1,1,2,0,0,0,2,1,1,1,2],
    [0,0,0,0,2,1,1,2,2,1,1,2,2,1,1,2],
    [0,2,2,2,0,1,1,1,0,1,1,1,0,2,2,2],
    [0,0,0,2,1,1,1,2,1,1,1,2,0,0,0,2],
    [0,1,1,0,0,1,1,0,0,1,1,0,2,2,2,2],
    [0,0,0,0,0,0,0,0,2,1,1,2,2,1,1,2],
    [0,1,1,0,0,1,1,0,2,2,2,2,2,2,2,2],
    [0,0,2,2,0,0,1,1,0,0,1,1,0,0,2,2],
    [0,0,2,2,1,1,2,2,1,1,2,2,0,0,2,2],
    [0,0,0,0,0,0,0,0,0,0,0,0,2,1,1,2],
    [0,0,0,2,0,0,0,1,0,0,0,2,0,0,0,1],
    [0,2,2,2,1,2,2,2,0,2,2,2,1,2,2,2],
    [0,1,0,1,2,2,2,2,2,2,2,2,2,2,2,2],
    [0,1,1,1,2,0,1,1,2,2,0,1,2,2,2,0],
];

#[rustfmt::skip]
const ANCHOR2: [u8; 64] = [
    15,15,15,15,15,15,15,15,
    15,15,15,15,15,15,15,15,
    15, 2, 8, 2, 2, 8, 8,15,
     2, 8, 2, 2, 8, 8, 2, 2,
    15,15, 6, 8, 2, 8,15,15,
     2, 8, 2, 2, 2,15,15, 6,
     6, 2, 6, 8,15,15, 2, 2,
    15,15,15,15,15, 2, 2,15,
];

#[rustfmt::skip]
const ANCHOR3_2: [u8; 64] = [
     3, 3,15,15, 8, 3,15,15,
     8, 8, 6, 6, 6, 5, 3, 3,
     3, 3, 8,15, 3, 3, 6,10,
     5, 8, 8, 6, 8, 5,15,15,
     8,15, 3, 5, 6,10, 8,15,
    15, 3,15, 5,15,15,15,15,
     3,15, 5, 5, 5, 8, 5,10,
     5,10, 8,13,15,12, 3, 3,
];

#[rustfmt::skip]
const ANCHOR3_3: [u8; 64] = [
    15, 8, 8, 3,15,15, 3, 8,
    15,15,15,15,15,15,15, 8,
    15, 8,15, 3,15, 8,15, 8,
     3,15, 6,10,15,15,10, 8,
    15, 3,15,10,10, 8, 9,10,
     6,15, 8,15, 3, 6, 6, 8,
    15, 3,15,15,15,15,15,15,
    15,15,15,15, 3,15,15, 8,
];

/// Consume `n` low bits of the stream.
fn take(bits: &mut u128, n: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    let v = (*bits & ((1u128 << n) - 1)) as u32;
    *bits >>= n;
    v
}

fn interpolate(a: u16, b: u16, index: u32, index_bits: u32) -> u8 {
    let w = match index_bits {
        2 => W2[index as usize],
        3 => W3[index as usize],
        _ => W4[index as usize],
    };
    (((64 - w) * a + w * b + 32) >> 6) as u8
}

/// Decode one 128-bit BC7 block into 4x4 RGBA texels (row-major).
pub fn decode_block(bytes: &[u8; 16]) -> [[u8; 4]; 16] {
    let mut bits = u128::from_le_bytes(*bytes);
    // The mode is unary-coded: the number of leading zero bits.
    let mode = (bits as u8).trailing_zeros() as usize;
    if mode >= 8 {
        // A zero mode byte is an invalid block; the spec leaves the result
        // undefined and the reference decoders emit transparent black.
        return [[0, 0, 0, 0]; 16];
    }
    bits >>= mode as u32 + 1;

    let ns = NS[mode];
    let partition = take(&mut bits, PB[mode]) as usize;
    let rotation = take(&mut bits, RB[mode]);
    let index_selection = if mode == 4 { take(&mut bits, 1) } else { 0 };

    // Endpoints, component-major exactly as stored: all reds, all greens,
    // all blues, then alpha; subsets outer, endpoints inner.
    let mut ep = [[[0u16; 4]; 2]; 3];
    for c in 0..3 {
        for s in ep.iter_mut().take(ns) {
            for e in s.iter_mut() {
                e[c] = take(&mut bits, COLOR_PREC[mode]) as u16;
            }
        }
    }
    if ALPHA_PREC[mode] > 0 {
        for s in ep.iter_mut().take(ns) {
            for e in s.iter_mut() {
                e[3] = take(&mut bits, ALPHA_PREC[mode]) as u16;
            }
        }
    }

    // P-bits extend every component by one low bit.
    match PBIT_MODE[mode] {
        1 => {
            for s in ep.iter_mut().take(ns) {
                for e in s.iter_mut() {
                    let p = take(&mut bits, 1) as u16;
                    for c in e.iter_mut() {
                        *c = (*c << 1) | p;
                    }
                }
            }
        }
        2 => {
            // Mode 1: one bit per subset, shared by both endpoints.
            for s in ep.iter_mut().take(ns) {
                let p = take(&mut bits, 1) as u16;
                for e in s.iter_mut() {
                    for c in e.iter_mut() {
                        *c = (*c << 1) | p;
                    }
                }
            }
        }
        _ => {}
    }

    // Dequantize: shift the value up to bit 7 and replicate the MSBs into
    // the vacated low bits.
    let cpp = COLOR_PREC_P[mode];
    let app = ALPHA_PREC_P[mode];
    for s in ep.iter_mut().take(ns) {
        for e in s.iter_mut() {
            for c in 0..3 {
                let v = e[c] << (8 - cpp);
                e[c] = (v | (v >> cpp)) & 0xFF;
            }
            if app > 0 {
                let v = e[3] << (8 - app);
                e[3] = (v | (v >> app)) & 0xFF;
            } else {
                e[3] = 255;
            }
        }
    }

    let subset_of = |i: usize| -> usize {
        match ns {
            1 => 0,
            2 => P2[partition][i] as usize,
            _ => P3[partition][i] as usize,
        }
    };
    let anchor_of = |subset: usize| -> usize {
        match (ns, subset) {
            (_, 0) => 0,
            (2, _) => ANCHOR2[partition] as usize,
            (_, 1) => ANCHOR3_2[partition] as usize,
            _ => ANCHOR3_3[partition] as usize,
        }
    };

    // Primary index bits; the anchor texel of each subset stores one less
    // bit (its top bit is implicitly zero).
    let ib = IB[mode];
    let mut primary = [0u32; 16];
    for (i, slot) in primary.iter_mut().enumerate() {
        let n = if i == anchor_of(subset_of(i)) { ib - 1 } else { ib };
        *slot = take(&mut bits, n);
    }
    let ib2 = IB2[mode];
    let mut secondary = [0u32; 16];
    if ib2 > 0 {
        for (i, slot) in secondary.iter_mut().enumerate() {
            let n = if i == anchor_of(subset_of(i)) { ib2 - 1 } else { ib2 };
            *slot = take(&mut bits, n);
        }
    }

    // Which index array drives colour vs alpha. Mode 4's selection bit swaps
    // them; modes without a secondary array share the primary for both.
    let (color_idx, color_bits, alpha_idx, alpha_bits) = if ib2 == 0 {
        (&primary, ib, &primary, ib)
    } else if index_selection == 1 {
        (&secondary, ib2, &primary, ib)
    } else {
        (&primary, ib, &secondary, ib2)
    };

    let mut out = [[0u8; 4]; 16];
    for i in 0..16 {
        let s = subset_of(i);
        let e0 = &ep[s][0];
        let e1 = &ep[s][1];
        let mut px = [
            interpolate(e0[0], e1[0], color_idx[i], color_bits),
            interpolate(e0[1], e1[1], color_idx[i], color_bits),
            interpolate(e0[2], e1[2], color_idx[i], color_bits),
            interpolate(e0[3], e1[3], alpha_idx[i], alpha_bits),
        ];
        // Rotation swaps alpha with one colour channel (modes 4/5), letting
        // the encoder give any single channel the alpha index precision.
        if rotation > 0 {
            px.swap(3, rotation as usize - 1);
        }
        out[i] = px;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Little-endian bit writer for hand-building known blocks.
    struct Writer {
        bits: u128,
        pos: u32,
    }

    impl Writer {
        fn new() -> Writer {
            Writer { bits: 0, pos: 0 }
        }
        fn push(&mut self, value: u32, n: u32) {
            self.bits |= (value as u128 & ((1u128 << n) - 1)) << self.pos;
            self.pos += n;
        }
        fn finish(self, expect_bits: u32) -> [u8; 16] {
            assert_eq!(self.pos, expect_bits, "block layout must be exactly 128 bits");
            self.bits.to_le_bytes()
        }
    }

    #[test]
    fn a_solid_mode6_block_decodes_to_its_endpoint_color() {
        // Mode 6: RGBA 7.7.7.7 endpoints + unique P-bits, 4-bit indices.
        // Both endpoints white with P-bits set: every texel must be exactly
        // opaque white, which pins mode parsing, endpoint order, P-bit
        // application and dequantization all at once.
        let mut w = Writer::new();
        w.push(0b1000000, 7); // mode 6
        for _ in 0..8 {
            w.push(0x7F, 7); // r0 r1 g0 g1 b0 b1 a0 a1
        }
        w.push(0b11, 2); // both P-bits
        w.push(0, 3); // anchor index (3 bits)
        for _ in 0..15 {
            w.push(0, 4);
        }
        let out = decode_block(&w.finish(128));
        for px in out {
            assert_eq!(px, [255, 255, 255, 255]);
        }
    }

    #[test]
    fn mode6_interpolates_between_its_endpoints() {
        // Black to white with index 8 of 16: weight 34 -> (255*34+32)>>6 = 135.
        let mut w = Writer::new();
        w.push(0b1000000, 7);
        for channel in 0..4 {
            let _ = channel;
            w.push(0x00, 7); // endpoint 0
            w.push(0x7F, 7); // endpoint 1
        }
        w.push(0b10, 2); // p0 = 0, p1 = 1
        w.push(0, 3); // anchor stays at endpoint 0
        for _ in 0..15 {
            w.push(8, 4);
        }
        let out = decode_block(&w.finish(128));
        assert_eq!(out[0], [0, 0, 0, 0], "anchor texel is endpoint 0");
        for px in &out[1..] {
            assert_eq!(*px, [135, 135, 135, 135]);
        }
    }

    #[test]
    fn mode5_rotation_swaps_alpha_into_the_red_channel() {
        // Mode 5: RGB 7.7.7 + A8, 2-bit rotation. Rotation 1 = swap A and R.
        // Colour (127*2+1=255, 0, 0), alpha 128, all indices 0.
        let mut w = Writer::new();
        w.push(0b100000, 6); // mode 5
        w.push(1, 2); // rotation: swap A <-> R
        w.push(0x7F, 7); // r0
        w.push(0x7F, 7); // r1
        w.push(0x00, 7); // g0
        w.push(0x00, 7); // g1
        w.push(0x00, 7); // b0
        w.push(0x00, 7); // b1
        w.push(128, 8); // a0
        w.push(128, 8); // a1
        w.push(0, 1); // colour anchor (1 bit)
        for _ in 0..15 {
            w.push(0, 2);
        }
        w.push(0, 1); // alpha anchor
        for _ in 0..15 {
            w.push(0, 2);
        }
        let out = decode_block(&w.finish(128));
        for px in out {
            // Dequantized 7-bit 0x7F -> 255. After the swap the red channel
            // carries the 128 alpha and alpha carries the 255 red.
            assert_eq!(px, [128, 0, 0, 255]);
        }
    }

    #[test]
    fn mode1_shares_pbits_per_subset() {
        // Mode 1: RGB 6.6.6, two subsets, one shared P-bit each, 3-bit
        // indices. Partition 0 splits the block into left/right halves
        // (texel pattern 0,0,1,1 per row). Subset 0 black, subset 1 with
        // P-bit set: 0x3F<<1|1 = 0x7F -> dequantized 255.
        let mut w = Writer::new();
        w.push(0b10, 2); // mode 1
        w.push(0, 6); // partition 0
        // Reds: s0e0, s0e1, s1e0, s1e1 — then greens, then blues.
        for _ in 0..3 {
            w.push(0x00, 6);
            w.push(0x00, 6);
            w.push(0x3F, 6);
            w.push(0x3F, 6);
        }
        w.push(0b10, 2); // shared P-bits: subset0 = 0, subset1 = 1
        // Indices: anchor of subset 0 is texel 0 (2 bits); anchor of subset 1
        // for partition 0 is texel 15 (2 bits); the rest 3 bits.
        for i in 0..16 {
            let n = if i == 0 || i == 15 { 2 } else { 3 };
            w.push(0, n);
        }
        let out = decode_block(&w.finish(128));
        for (i, px) in out.iter().enumerate() {
            let expected = if P2[0][i] == 0 {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            };
            assert_eq!(*px, expected, "texel {i}");
        }
    }
}
