//! LZ4 frame and block decoding, std-only.
//!
//! Skyrim SE archives (BSA v105) store every compressed entry as an **LZ4
//! frame** — magic, header, a chain of length-prefixed blocks, an end mark.
//! That is not the same thing as a raw LZ4 block, which is what BA2 texture
//! chunks use; feeding a v105 payload to a raw-block decoder fails on the very
//! first sequence and produces the classic "LZ4 works everywhere except
//! Skyrim SE" bug. Both entry points are here and named for what they are.
//!
//! See `docs/knowledge/bsa-format.md` §4.5-4.6 for the verified format notes.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lz4Error {
    /// Ran out of input mid-structure.
    Truncated { what: &'static str },
    BadMagic(u32),
    /// Frame descriptor version bits were not `01`.
    UnsupportedVersion(u8),
    ReservedBitSet { field: &'static str, value: u8 },
    /// Block-size id below 4 has no defined maximum block size.
    BadBlockSizeId(u8),
    /// A dictionary would be needed and none exists.
    DictionaryRequired,
    /// A match referred to bytes before the start of the output.
    BadMatchOffset { offset: usize, produced: usize },
    /// The frame decoded, but not to the length the container promised.
    LengthMismatch { expected: usize, produced: usize },
}

impl fmt::Display for Lz4Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Lz4Error::Truncated { what } => write!(f, "LZ4 data ends inside the {what}"),
            Lz4Error::BadMagic(m) => write!(f, "not an LZ4 frame (magic {m:#010x})"),
            Lz4Error::UnsupportedVersion(v) => write!(f, "unsupported LZ4 frame version {v}"),
            Lz4Error::ReservedBitSet { field, value } => {
                write!(f, "reserved bit set in LZ4 {field} ({value:#04x})")
            }
            Lz4Error::BadBlockSizeId(id) => write!(f, "invalid LZ4 block size id {id}"),
            Lz4Error::DictionaryRequired => write!(f, "LZ4 frame needs a dictionary"),
            Lz4Error::BadMatchOffset { offset, produced } => write!(
                f,
                "LZ4 match offset {offset} reaches before the output start ({produced} bytes so far)"
            ),
            Lz4Error::LengthMismatch { expected, produced } => {
                write!(f, "LZ4 produced {produced} bytes, expected {expected}")
            }
        }
    }
}

const FRAME_MAGIC: u32 = 0x184D_2204;
const SKIPPABLE_LOW: u32 = 0x184D_2A50;
const SKIPPABLE_HIGH: u32 = 0x184D_2A5F;
/// LZ4's minimum encodable match length; the 4-bit field stores `len - 4`.
const MIN_MATCH: usize = 4;

struct Reader<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self, what: &'static str) -> Result<u8, Lz4Error> {
        let b = *self.src.get(self.pos).ok_or(Lz4Error::Truncated { what })?;
        self.pos += 1;
        Ok(b)
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, Lz4Error> {
        let end = self.pos + 4;
        let bytes = self
            .src
            .get(self.pos..end)
            .ok_or(Lz4Error::Truncated { what })?;
        self.pos = end;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn skip(&mut self, n: usize, what: &'static str) -> Result<(), Lz4Error> {
        let end = self.pos.checked_add(n).ok_or(Lz4Error::Truncated { what })?;
        if end > self.src.len() {
            return Err(Lz4Error::Truncated { what });
        }
        self.pos = end;
        Ok(())
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], Lz4Error> {
        let end = self.pos.checked_add(n).ok_or(Lz4Error::Truncated { what })?;
        let slice = self
            .src
            .get(self.pos..end)
            .ok_or(Lz4Error::Truncated { what })?;
        self.pos = end;
        Ok(slice)
    }
}

/// Decode a complete LZ4 frame, returning the bytes and how much input the
/// frame consumed.
///
/// The consumed count is the caller's cheapest integrity check: a BSA entry
/// records the payload length independently, and a frame that decodes but
/// stops short means the entry boundaries are wrong.
pub fn decode_frame(src: &[u8], size_hint: usize) -> Result<(Vec<u8>, usize), Lz4Error> {
    let mut r = Reader { src, pos: 0 };
    // Every block decodes into ONE buffer. When the frame is not
    // block-independent, a match in block N may reach back into block N-1's
    // output; per-block buffers would then produce wrong bytes with no error
    // at all. Sharing costs nothing and handles both modes.
    let mut out = Vec::with_capacity(size_hint);

    loop {
        let magic = r.u32("frame magic")?;
        if (SKIPPABLE_LOW..=SKIPPABLE_HIGH).contains(&magic) {
            let len = r.u32("skippable frame length")? as usize;
            r.skip(len, "skippable frame body")?;
            continue;
        }
        if magic != FRAME_MAGIC {
            return Err(Lz4Error::BadMagic(magic));
        }
        break;
    }

    let flg = r.u8("frame descriptor")?;
    let version = flg >> 6;
    if version != 0b01 {
        return Err(Lz4Error::UnsupportedVersion(version));
    }
    if flg & 0b10 != 0 {
        return Err(Lz4Error::ReservedBitSet {
            field: "FLG",
            value: flg,
        });
    }
    let block_checksum = flg & 0b1_0000 != 0;
    let content_size = flg & 0b1000 != 0;
    let content_checksum = flg & 0b100 != 0;
    let dict_id = flg & 0b1 != 0;

    let bd = r.u8("frame descriptor")?;
    let block_size_id = (bd >> 4) & 0b111;
    if block_size_id < 4 {
        return Err(Lz4Error::BadBlockSizeId(block_size_id));
    }
    if bd & 0b1000_1111 != 0 {
        return Err(Lz4Error::ReservedBitSet {
            field: "BD",
            value: bd,
        });
    }

    if content_size {
        r.skip(8, "content size")?;
    }
    if dict_id {
        return Err(Lz4Error::DictionaryRequired);
    }
    // Header checksum. Verifying it needs xxHash32, and the caller's own
    // length check is both stronger and cheaper, so skip the byte.
    r.skip(1, "header checksum")?;

    loop {
        let bs = r.u32("block size")?;
        if bs == 0 {
            break;
        }
        let stored = bs & 0x8000_0000 != 0;
        let n = (bs & 0x7FFF_FFFF) as usize;
        if stored {
            // Real archives do use this: an incompressible tail is written
            // through verbatim rather than expanded by a compressed header.
            let raw = r.take(n, "stored block")?;
            out.extend_from_slice(raw);
        } else {
            let block = r.take(n, "compressed block")?;
            decode_block_into(block, &mut out)?;
        }
        if block_checksum {
            r.skip(4, "block checksum")?;
        }
    }
    if content_checksum {
        r.skip(4, "content checksum")?;
    }

    Ok((out, r.pos))
}

/// Decode a raw LZ4 block, appending to `out`.
///
/// Matches resolve against everything already in `out`, not just this block —
/// that is what makes block-linked frames work.
pub fn decode_block_into(src: &[u8], out: &mut Vec<u8>) -> Result<(), Lz4Error> {
    let mut i = 0usize;
    while i < src.len() {
        let token = src[i];
        i += 1;

        let mut literals = (token >> 4) as usize;
        if literals == 15 {
            loop {
                let b = *src.get(i).ok_or(Lz4Error::Truncated {
                    what: "literal length",
                })?;
                i += 1;
                literals += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        let end = i.checked_add(literals).ok_or(Lz4Error::Truncated {
            what: "literal run",
        })?;
        let run = src.get(i..end).ok_or(Lz4Error::Truncated {
            what: "literal run",
        })?;
        out.extend_from_slice(run);
        i = end;

        // The last sequence in a block is literals only, with no match after
        // it — running off the end here is the normal way a block ends.
        if i == src.len() {
            break;
        }

        let offset_bytes = src.get(i..i + 2).ok_or(Lz4Error::Truncated {
            what: "match offset",
        })?;
        let offset = u16::from_le_bytes([offset_bytes[0], offset_bytes[1]]) as usize;
        i += 2;
        if offset == 0 || offset > out.len() {
            return Err(Lz4Error::BadMatchOffset {
                offset,
                produced: out.len(),
            });
        }

        let mut match_len = (token & 0x0F) as usize;
        if match_len == 15 {
            loop {
                let b = *src.get(i).ok_or(Lz4Error::Truncated {
                    what: "match length",
                })?;
                i += 1;
                match_len += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        match_len += MIN_MATCH;

        // Byte by byte on purpose: `offset` is routinely smaller than the
        // match length, and that self-overlap is how LZ4 encodes runs. A bulk
        // copy of the whole match would read bytes that do not exist yet.
        let start = out.len() - offset;
        for k in 0..match_len {
            let byte = out[start + k];
            out.push(byte);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a block that emits `literals`, then a match of `match_len` bytes
    /// starting `offset` back.
    fn block(literals: &[u8], offset: u16, match_len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let lit_token = if literals.len() >= 15 { 15 } else { literals.len() };
        let encoded_match = match_len - MIN_MATCH;
        let match_token = if encoded_match >= 15 { 15 } else { encoded_match };
        out.push(((lit_token as u8) << 4) | match_token as u8);
        if lit_token == 15 {
            let mut left = literals.len() - 15;
            while left >= 255 {
                out.push(255);
                left -= 255;
            }
            out.push(left as u8);
        }
        out.extend_from_slice(literals);
        out.extend_from_slice(&offset.to_le_bytes());
        if match_token == 15 {
            let mut left = encoded_match - 15;
            while left >= 255 {
                out.push(255);
                left -= 255;
            }
            out.push(left as u8);
        }
        out
    }

    #[test]
    fn a_match_shorter_than_its_own_length_repeats_the_run() {
        // offset 1, length 6 means "repeat the previous byte six times". This
        // is the overlap case a bulk copy gets wrong, and it is the common
        // case, not an edge case.
        let mut out = Vec::new();
        decode_block_into(&block(b"ab", 1, 6), &mut out).expect("decodes");
        assert_eq!(out, b"abbbbbbb".to_vec());
    }

    #[test]
    fn a_partially_overlapping_match_reads_bytes_it_just_produced() {
        // offset 2, length 5 over "ab" appends "ababa": its last three bytes
        // are read out of output this very match wrote.
        let mut out = Vec::new();
        decode_block_into(&block(b"ab", 2, 5), &mut out).expect("decodes");
        assert_eq!(out, b"abababa".to_vec());
    }

    #[test]
    fn long_literal_and_match_lengths_use_the_255_continuation() {
        // Both length fields escape through a chain of 255s; an off-by-one in
        // either loop shows up as a truncated or overlong result.
        let literals: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
        let mut out = Vec::new();
        decode_block_into(&block(&literals, 300, 280), &mut out).expect("decodes");
        assert_eq!(out.len(), 580);
        assert_eq!(&out[..300], &literals[..]);
        // The match copies from the very start of the output.
        assert_eq!(&out[300..580], &literals[..280]);
    }

    #[test]
    fn a_match_reaching_before_the_output_is_rejected() {
        // Corrupt data must not be served as content; this is the check that
        // stops a bad offset from panicking on an underflow.
        let mut out = Vec::new();
        let err = decode_block_into(&block(b"ab", 9, 4), &mut out).unwrap_err();
        assert_eq!(
            err,
            Lz4Error::BadMatchOffset {
                offset: 9,
                produced: 2
            }
        );
    }

    /// Wrap blocks in a minimal frame: FLG=0x60 (version 1, independent, no
    /// checksums, no content size), BD=0x70 (4 MiB).
    fn frame(blocks: &[Vec<u8>], stored: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&FRAME_MAGIC.to_le_bytes());
        out.push(0x60);
        out.push(0x70);
        out.push(0x00); // header checksum, not verified
        for b in blocks {
            let mut size = b.len() as u32;
            if stored {
                size |= 0x8000_0000;
            }
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(b);
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // EndMark
        out
    }

    #[test]
    fn a_frame_decodes_its_blocks_and_reports_what_it_consumed() {
        // The consumed count is what lets the BSA reader prove an entry's
        // payload boundary was right, so it has to be exact.
        let data = frame(&[block(b"hello", 1, 4)], false);
        let (out, used) = decode_frame(&data, 0).expect("decodes");
        assert_eq!(out, b"hellooooo".to_vec());
        assert_eq!(used, data.len(), "frame must consume exactly its own bytes");
    }

    #[test]
    fn stored_blocks_pass_through_verbatim() {
        // Real archives contain these: an incompressible tail is written raw
        // with bit 31 set on the block size.
        let raw = b"not compressible".to_vec();
        let data = frame(&[raw.clone()], true);
        let (out, used) = decode_frame(&data, 0).expect("decodes");
        assert_eq!(out, raw);
        assert_eq!(used, data.len());
    }

    #[test]
    fn a_later_block_can_match_into_an_earlier_one() {
        // Block-linked frames are legal and every vanilla archive happens to
        // be independent, which is exactly what makes a per-block output
        // buffer a bug nobody notices.
        let first = block(b"abcdefgh", 1, 4);
        // Second block emits nothing of its own and copies 8 bytes from 12
        // back — reachable only if the first block's output is still there.
        let second = block(b"", 12, 8);
        let data = frame(&[first, second], false);
        let (out, _) = decode_frame(&data, 0).expect("decodes");
        assert_eq!(out, b"abcdefghhhhhabcdefgh".to_vec());
    }

    #[test]
    fn skippable_frames_are_stepped_over() {
        let mut data = Vec::new();
        data.extend_from_slice(&SKIPPABLE_LOW.to_le_bytes());
        data.extend_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(b"junk");
        data.extend_from_slice(&frame(&[block(b"ok", 1, 4)], false));
        let (out, used) = decode_frame(&data, 0).expect("decodes");
        assert_eq!(out, b"okkkkk".to_vec());
        assert_eq!(used, data.len());
    }

    #[test]
    fn a_raw_block_is_not_mistaken_for_a_frame() {
        // The "LZ4 works everywhere except Skyrim SE" bug in reverse: feeding
        // frame data to the wrong entry point must fail loudly, not silently
        // produce garbage.
        let raw = block(b"hello", 1, 4);
        let err = decode_frame(&raw, 0).unwrap_err();
        assert!(matches!(err, Lz4Error::BadMagic(_)), "got {err:?}");
    }

    #[test]
    fn truncated_input_is_an_error_not_a_panic() {
        let data = frame(&[block(b"hello", 1, 4)], false);
        for cut in 1..data.len() {
            // Any prefix must come back as an error; the reader is fed
            // attacker-controlled archive bytes.
            let _ = decode_frame(&data[..cut], 0);
        }
        let err = decode_frame(&data[..6], 0).unwrap_err();
        assert!(matches!(err, Lz4Error::Truncated { .. }), "got {err:?}");
    }
}
