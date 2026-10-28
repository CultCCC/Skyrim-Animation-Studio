//! DEFLATE and zlib decompression (RFC 1951 / RFC 1950), std-only.
//!
//! Skyrim LE archives (BSA v104) store compressed entries as zlib streams, and
//! LE is a target game, so "link zlib" is not an option here: `crates/core`
//! carries no third-party dependencies by design. This is the whole decoder —
//! stored, fixed-Huffman and dynamic-Huffman blocks, the 32 KiB window, and
//! the zlib wrapper with its Adler-32.
//!
//! Decoding only. Nothing in the editor writes a BSA.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflateError {
    /// Ran out of input mid-symbol.
    Truncated,
    /// BTYPE 11 is reserved.
    ReservedBlockType,
    /// A stored block's length and its complement disagree.
    StoredLengthMismatch { len: u16, nlen: u16 },
    /// Code lengths did not form a valid Huffman code.
    MalformedCode { what: &'static str },
    /// A symbol outside the alphabet, or a repeat with nothing to repeat.
    BadSymbol { what: &'static str, symbol: u16 },
    /// A match pointed before the start of the output.
    BadDistance { distance: usize, produced: usize },
    /// Not a zlib stream, or one this decoder must not guess at.
    BadZlibHeader { cmf: u8, flg: u8 },
    /// The stream needs a preset dictionary, which archives never carry.
    DictionaryRequired,
    /// The stream decoded but its checksum disagrees.
    ChecksumMismatch { expected: u32, actual: u32 },
}

impl fmt::Display for InflateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InflateError::Truncated => write!(f, "compressed data ends mid-stream"),
            InflateError::ReservedBlockType => write!(f, "reserved DEFLATE block type"),
            InflateError::StoredLengthMismatch { len, nlen } => write!(
                f,
                "stored block length {len} does not match its complement {nlen}"
            ),
            InflateError::MalformedCode { what } => {
                write!(f, "malformed Huffman code in the {what}")
            }
            InflateError::BadSymbol { what, symbol } => {
                write!(f, "invalid {what} symbol {symbol}")
            }
            InflateError::BadDistance { distance, produced } => write!(
                f,
                "match distance {distance} reaches before the output start \
                 ({produced} bytes so far)"
            ),
            InflateError::BadZlibHeader { cmf, flg } => {
                write!(f, "not a zlib stream (header {cmf:#04x} {flg:#04x})")
            }
            InflateError::DictionaryRequired => {
                write!(f, "zlib stream needs a preset dictionary")
            }
            InflateError::ChecksumMismatch { expected, actual } => write!(
                f,
                "Adler-32 mismatch: stream says {expected:#010x}, data gives {actual:#010x}"
            ),
        }
    }
}

/// Longest Huffman code DEFLATE permits.
const MAX_BITS: usize = 15;

/// Length codes 257..=285: base length and extra bits.
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

/// Distance codes 0..=29: base distance and extra bits.
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// The code-length alphabet is transmitted in this order, so the common
/// lengths come first and trailing entries can be omitted entirely.
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

struct BitReader<'a> {
    src: &'a [u8],
    /// Next bit position, counted in bits from the start of `src`.
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(src: &'a [u8]) -> BitReader<'a> {
        BitReader { src, bit: 0 }
    }

    /// DEFLATE packs bits into bytes starting at the least significant bit.
    fn bits(&mut self, count: usize) -> Result<u32, InflateError> {
        let mut value = 0u32;
        for i in 0..count {
            let byte = self
                .src
                .get(self.bit >> 3)
                .ok_or(InflateError::Truncated)?;
            let bit = (byte >> (self.bit & 7)) & 1;
            value |= (bit as u32) << i;
            self.bit += 1;
        }
        Ok(value)
    }

    fn align_to_byte(&mut self) {
        self.bit = (self.bit + 7) & !7;
    }

    fn byte_position(&self) -> usize {
        self.bit >> 3
    }
}

/// A canonical Huffman code, stored as counts per length plus the symbols in
/// canonical order. Decoding walks one bit at a time, which is a fraction of
/// the code of a lookup table and fast enough for archive-sized payloads.
struct Huffman {
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8], what: &'static str) -> Result<Huffman, InflateError> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &len in lengths {
            if len as usize > MAX_BITS {
                return Err(InflateError::MalformedCode { what });
            }
            counts[len as usize] += 1;
        }
        // Length 0 means "symbol unused", so it never takes part in the code.
        counts[0] = 0;

        // An over-subscribed code would make two symbols share a prefix; an
        // incomplete one leaves bit patterns undefined. Both mean the stream
        // is not what it claims, and decoding on would invent data.
        let mut left = 1i32;
        for len in 1..=MAX_BITS {
            left <<= 1;
            left -= counts[len] as i32;
            if left < 0 {
                return Err(InflateError::MalformedCode { what });
            }
        }

        // Where each length's symbols start in canonical order: sorted by
        // length, then by symbol value.
        let mut offsets = [0usize; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offsets[len + 1] = offsets[len] + counts[len] as usize;
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len != 0 {
                symbols[offsets[len as usize]] = symbol as u16;
                offsets[len as usize] += 1;
            }
        }
        Ok(Huffman { counts, symbols })
    }

    fn decode(&self, reader: &mut BitReader, what: &'static str) -> Result<u16, InflateError> {
        // Canonical codes are contiguous within each length, so the first code
        // of length n and a running symbol index are enough to identify a
        // symbol without any table.
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..=MAX_BITS {
            code |= reader.bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                let at = (index + (code - first)) as usize;
                return self
                    .symbols
                    .get(at)
                    .copied()
                    .ok_or(InflateError::MalformedCode { what });
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(InflateError::MalformedCode { what })
    }
}

fn fixed_literal_code() -> Result<Huffman, InflateError> {
    let mut lengths = [0u8; 288];
    lengths[..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..].fill(8);
    Huffman::new(&lengths, "fixed literal code")
}

fn fixed_distance_code() -> Result<Huffman, InflateError> {
    Huffman::new(&[5u8; 30], "fixed distance code")
}

/// Inflate a raw DEFLATE stream, returning the bytes and how many input bytes
/// were consumed.
pub fn inflate(src: &[u8], size_hint: usize) -> Result<(Vec<u8>, usize), InflateError> {
    let mut reader = BitReader::new(src);
    let mut out = Vec::with_capacity(size_hint);

    loop {
        let final_block = reader.bits(1)? == 1;
        let block_type = reader.bits(2)?;
        match block_type {
            0 => {
                reader.align_to_byte();
                let len = reader.bits(16)? as u16;
                let nlen = reader.bits(16)? as u16;
                if len != !nlen {
                    return Err(InflateError::StoredLengthMismatch { len, nlen });
                }
                let start = reader.byte_position();
                let end = start + len as usize;
                let bytes = src.get(start..end).ok_or(InflateError::Truncated)?;
                out.extend_from_slice(bytes);
                reader.bit = end * 8;
            }
            1 => inflate_block(&mut reader, &fixed_literal_code()?, &fixed_distance_code()?, &mut out)?,
            2 => {
                let (literal, distance) = read_dynamic_codes(&mut reader)?;
                inflate_block(&mut reader, &literal, &distance, &mut out)?;
            }
            _ => return Err(InflateError::ReservedBlockType),
        }
        if final_block {
            break;
        }
    }

    // Round up: the final block's last byte is partially consumed.
    reader.align_to_byte();
    Ok((out, reader.byte_position()))
}

fn read_dynamic_codes(reader: &mut BitReader) -> Result<(Huffman, Huffman), InflateError> {
    let hlit = reader.bits(5)? as usize + 257;
    let hdist = reader.bits(5)? as usize + 1;
    let hclen = reader.bits(4)? as usize + 4;

    let mut code_lengths = [0u8; 19];
    for i in 0..hclen {
        code_lengths[CODE_LENGTH_ORDER[i]] = reader.bits(3)? as u8;
    }
    let code_length_code = Huffman::new(&code_lengths, "code length alphabet")?;

    // The literal and distance lengths are transmitted as one run, so a
    // repeat can straddle the boundary between them.
    let total = hlit + hdist;
    let mut lengths = vec![0u8; total];
    let mut i = 0usize;
    while i < total {
        let symbol = code_length_code.decode(reader, "code length alphabet")?;
        match symbol {
            0..=15 => {
                lengths[i] = symbol as u8;
                i += 1;
            }
            16 => {
                // Repeat the previous length; with nothing before it there is
                // nothing to repeat and the stream is malformed.
                if i == 0 {
                    return Err(InflateError::BadSymbol {
                        what: "code length repeat",
                        symbol,
                    });
                }
                let previous = lengths[i - 1];
                let repeat = 3 + reader.bits(2)? as usize;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(InflateError::MalformedCode {
                            what: "code length run",
                        });
                    }
                    lengths[i] = previous;
                    i += 1;
                }
            }
            17 | 18 => {
                let repeat = if symbol == 17 {
                    3 + reader.bits(3)? as usize
                } else {
                    11 + reader.bits(7)? as usize
                };
                if i + repeat > total {
                    return Err(InflateError::MalformedCode {
                        what: "code length run",
                    });
                }
                i += repeat; // lengths are already zero
            }
            _ => {
                return Err(InflateError::BadSymbol {
                    what: "code length",
                    symbol,
                })
            }
        }
    }

    let literal = Huffman::new(&lengths[..hlit], "literal/length code")?;
    let distance = Huffman::new(&lengths[hlit..], "distance code")?;
    Ok((literal, distance))
}

fn inflate_block(
    reader: &mut BitReader,
    literal: &Huffman,
    distance: &Huffman,
    out: &mut Vec<u8>,
) -> Result<(), InflateError> {
    loop {
        let symbol = literal.decode(reader, "literal/length code")?;
        match symbol {
            0..=255 => out.push(symbol as u8),
            256 => return Ok(()),
            257..=285 => {
                let index = symbol as usize - 257;
                let length =
                    LENGTH_BASE[index] as usize + reader.bits(LENGTH_EXTRA[index] as usize)? as usize;

                let dist_symbol = distance.decode(reader, "distance code")? as usize;
                if dist_symbol >= DISTANCE_BASE.len() {
                    return Err(InflateError::BadSymbol {
                        what: "distance",
                        symbol: dist_symbol as u16,
                    });
                }
                let dist = DISTANCE_BASE[dist_symbol] as usize
                    + reader.bits(DISTANCE_EXTRA[dist_symbol] as usize)? as usize;
                if dist == 0 || dist > out.len() {
                    return Err(InflateError::BadDistance {
                        distance: dist,
                        produced: out.len(),
                    });
                }

                // Byte by byte: the distance is often shorter than the length,
                // and that overlap is how DEFLATE encodes a run.
                let start = out.len() - dist;
                for k in 0..length {
                    let byte = out[start + k];
                    out.push(byte);
                }
            }
            _ => {
                return Err(InflateError::BadSymbol {
                    what: "literal/length",
                    symbol,
                })
            }
        }
    }
}

/// Decompress a zlib stream (RFC 1950): 2-byte header, DEFLATE, Adler-32.
///
/// The header is validated rather than skipped. A payload that is not zlib
/// must be reported as such — silently retrying as raw DEFLATE or gzip is how
/// a reader ends up serving plausible garbage.
pub fn zlib_decompress(src: &[u8], size_hint: usize) -> Result<Vec<u8>, InflateError> {
    let (&cmf, rest) = src.split_first().ok_or(InflateError::Truncated)?;
    let (&flg, body) = rest.split_first().ok_or(InflateError::Truncated)?;

    let method = cmf & 0x0F;
    let check = ((cmf as u16) << 8 | flg as u16) % 31;
    if method != 8 || check != 0 {
        return Err(InflateError::BadZlibHeader { cmf, flg });
    }
    if flg & 0x20 != 0 {
        return Err(InflateError::DictionaryRequired);
    }

    let (out, used) = inflate(body, size_hint)?;

    // The trailer is the one end-to-end check available here, and it costs a
    // few lines: a decoder bug that produces the right length but wrong bytes
    // would otherwise be invisible. It is also required, not optional — a
    // stream cut off exactly at the end of its DEFLATE data would otherwise
    // decode "successfully" with nothing verified.
    let trailer = body
        .get(used..used + 4)
        .ok_or(InflateError::Truncated)?;
    let expected = u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let actual = adler32(&out);
    if expected != actual {
        return Err(InflateError::ChecksumMismatch { expected, actual });
    }
    Ok(out)
}

pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a = 1u32;
    let mut b = 0u32;
    // Chunked so the accumulators cannot overflow before the modulo.
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a stored-block DEFLATE stream by hand.
    fn stored(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0b001]; // BFINAL=1, BTYPE=00
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(data.len() as u16)).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn stored_blocks_pass_data_through() {
        let data = b"incompressible bytes";
        let (out, used) = inflate(&stored(data), 0).expect("inflates");
        assert_eq!(out, data);
        assert_eq!(used, 5 + data.len(), "must consume exactly the block");
    }

    #[test]
    fn a_stored_block_with_a_bad_complement_is_refused() {
        // The complement is the format's own corruption check; ignoring it
        // would let a truncated archive produce silent garbage.
        let mut bytes = stored(b"abcd");
        bytes[3] ^= 0xFF;
        assert!(matches!(
            inflate(&bytes, 0),
            Err(InflateError::StoredLengthMismatch { .. })
        ));
    }

    #[test]
    fn adler32_matches_the_specification_vectors() {
        // Wrong Adler-32 would turn every real stream into a false alarm.
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"a"), 0x0062_0062);
        assert_eq!(adler32(b"abc"), 0x024D_0127);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn an_over_subscribed_huffman_code_is_refused() {
        // Three symbols cannot all have a 1-bit code. Accepting it would make
        // two symbols share a bit pattern and invent data from then on.
        assert!(matches!(
            Huffman::new(&[1, 1, 1], "test"),
            Err(InflateError::MalformedCode { .. })
        ));
    }

    #[test]
    fn a_canonical_code_assigns_codes_in_length_then_symbol_order() {
        // Symbols A=2 bits, B=1 bit, C=3 bits, D=3 bits gives B=0, A=10,
        // C=110, D=111 — the ordering the whole decoder depends on.
        let code = Huffman::new(&[2, 1, 3, 3], "test").expect("valid");
        let decode_of = |bits: &[u8]| {
            let mut byte = 0u8;
            for (i, b) in bits.iter().enumerate() {
                byte |= b << i;
            }
            let data = [byte];
            let mut reader = BitReader::new(&data);
            code.decode(&mut reader, "test").expect("decodes")
        };
        assert_eq!(decode_of(&[0]), 1);
        assert_eq!(decode_of(&[1, 0]), 0);
        assert_eq!(decode_of(&[1, 1, 0]), 2);
        assert_eq!(decode_of(&[1, 1, 1]), 3);
    }

    #[test]
    fn a_zlib_header_that_is_not_zlib_is_refused_rather_than_retried() {
        // Guessing raw DEFLATE or gzip here is how a reader serves garbage.
        let err = zlib_decompress(b"\x00\x00rest", 0).unwrap_err();
        assert!(matches!(err, InflateError::BadZlibHeader { .. }), "got {err}");
    }

    #[test]
    fn a_preset_dictionary_is_refused() {
        // FDICT set means bytes we do not have; no archive carries one.
        // CMF 0x78, FLG with FDICT: pick a FLG that keeps the check word
        // divisible by 31.
        for flg in 0u16..256 {
            if (0x78u16 << 8 | flg) % 31 == 0 && flg & 0x20 != 0 {
                let bytes = [0x78u8, flg as u8, 0, 0, 0, 0];
                assert_eq!(
                    zlib_decompress(&bytes, 0),
                    Err(InflateError::DictionaryRequired)
                );
                return;
            }
        }
        panic!("no FLG with FDICT satisfies the check word");
    }

    #[test]
    fn truncated_input_is_an_error_not_a_panic() {
        // Archive bytes are untrusted; every prefix must come back cleanly.
        let full = stored(b"some data here");
        for cut in 0..full.len() {
            let _ = inflate(&full[..cut], 0);
        }
        assert_eq!(inflate(&full[..3], 0), Err(InflateError::Truncated));
    }
}
