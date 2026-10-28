//! Writing a PNG, so a screenshot is a file anyone can open.
//!
//! No encoder dependency and no compression search: PNG's zlib stream
//! accepts **stored** deflate blocks, which are a length, its complement
//! and the bytes. That costs about 0.03% over the raw size and needs
//! sixty lines instead of a compressor — and this crate is std-only by
//! rule, so the alternative was a dependency, not a shortcut.
//!
//! It exists for the screenshot harness: an editor whose output is a
//! picture cannot be verified by a test that only proves it compiled.

/// CRC-32/ISO-HDLC, the one PNG chunks carry.
fn crc32(bytes: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *entry = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for b in bytes {
        crc = table[((crc ^ *b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// Adler-32, the checksum a zlib stream ends with.
fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in bytes {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc_input);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// A zlib stream of `data`, as stored deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    // CMF/FLG for deflate, 32K window, no preset dictionary. 0x78 0x01
    // is the "no compression" pair whose product is divisible by 31.
    let mut out = vec![0x78, 0x01];
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    }
    let mut offset = 0usize;
    while offset < data.len() {
        let take = (data.len() - offset).min(0xFFFF);
        let final_block = offset + take == data.len();
        out.push(if final_block { 1 } else { 0 });
        out.extend_from_slice(&(take as u16).to_le_bytes());
        out.extend_from_slice(&(!(take as u16)).to_le_bytes());
        out.extend_from_slice(&data[offset..offset + take]);
        offset += take;
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Encode 8-bit RGBA, row-major from the top-left, as a PNG.
///
/// `pixels` must be `width * height * 4` bytes; anything else is a
/// caller bug and returns `None` rather than writing a file no viewer
/// will open.
pub fn encode_rgba(width: u32, height: u32, pixels: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return None;
    }
    let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if pixels.len() != expected {
        return None;
    }

    // Every row carries a leading filter byte; 0 is "none", which is what
    // makes the stored-block trick worth taking.
    let mut raw = Vec::with_capacity(expected + height as usize);
    for row in pixels.chunks_exact(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    let mut out = Vec::with_capacity(raw.len() + 1024);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, none, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two checksums against their published test vectors. Getting
    /// either wrong produces a file that opens nowhere and says nothing
    /// about why.
    #[test]
    fn the_checksums_match_their_known_vectors() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }

    /// A stored zlib stream has to round-trip through this crate's own
    /// inflate — which is the decoder the BSA reader uses, so if these
    /// two ever disagree one of them is wrong about DEFLATE.
    #[test]
    fn a_stored_stream_inflates_back() {
        // Longer than one 65,535-byte block, so the multi-block path is
        // the one under test rather than an afterthought.
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let stream = zlib_stored(&data);
        let back = crate::inflate::zlib_decompress(&stream, data.len()).expect("our own inflate reads it");
        assert_eq!(back, data);
    }

    /// The file really is a PNG: signature, a well-formed IHDR carrying
    /// the size asked for, and chunk CRCs that check out.
    #[test]
    fn the_encoded_file_is_a_well_formed_png() {
        let (w, h) = (3u32, 2u32);
        let pixels: Vec<u8> = (0..w * h * 4).map(|i| i as u8).collect();
        let png = encode_rgba(w, h, &pixels).expect("encodes");

        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // Walk the chunks, checking every CRC as we go.
        let mut at = 8usize;
        let mut kinds = Vec::new();
        while at + 12 <= png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind = &png[at + 4..at + 8];
            let body = &png[at + 4..at + 8 + len];
            let stored = u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().unwrap());
            assert_eq!(crc32(body), stored, "CRC of {:?}", std::str::from_utf8(kind));
            kinds.push(String::from_utf8_lossy(kind).into_owned());
            at += 12 + len;
        }
        assert_eq!(at, png.len(), "no trailing bytes");
        assert_eq!(kinds, vec!["IHDR", "IDAT", "IEND"]);

        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), w);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), h);
        assert_eq!(&png[24..29], &[8, 6, 0, 0, 0]);
    }

    /// The PIXELS come back, filter bytes and all.
    ///
    /// Everything above checks the envelope — signature, chunk order,
    /// CRCs, IHDR fields — and none of it opens the IDAT. Deleting the
    /// per-row `raw.push(0)` leaves all of that intact and produces a
    /// file no viewer can open, which matters more here than in most
    /// projects: this encoder is the screenshot harness that every
    /// visual check in CLAUDE.md runs through, so a silently broken
    /// image reads as "the fix did nothing".
    ///
    /// Planted defects this catches: the missing filter byte; rows
    /// emitted bottom-up; a stride computed from the wrong width; and
    /// RGBA written as RGB.
    #[test]
    fn the_idat_inflates_back_to_the_exact_rows() {
        let (w, h) = (3u32, 2u32);
        // Distinct in every channel of every pixel, so a channel swap
        // or a row reversal cannot look like the original.
        let pixels: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 + 1) as u8).collect();
        let png = encode_rgba(w, h, &pixels).expect("encodes");

        // Find IDAT by walking, rather than assuming an offset.
        let mut at = 8usize;
        let mut idat = Vec::new();
        while at + 12 <= png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            if &png[at + 4..at + 8] == b"IDAT" {
                idat.extend_from_slice(&png[at + 8..at + 8 + len]);
            }
            at += 12 + len;
        }
        assert!(!idat.is_empty(), "no IDAT payload");

        let raw = crate::inflate::zlib_decompress(&idat, (w as usize * 4 + 1) * h as usize)
            .expect("the IDAT is a valid zlib stream");
        let mut want = Vec::new();
        for row in pixels.chunks_exact(w as usize * 4) {
            want.push(0u8); // filter: none
            want.extend_from_slice(row);
        }
        assert_eq!(raw, want, "the decoded scanlines are not the pixels given");
    }

    /// A size that does not match the buffer is refused, rather than
    /// writing a file that opens as garbage.
    #[test]
    fn a_mismatched_buffer_is_refused() {
        assert!(encode_rgba(2, 2, &[0; 15]).is_none());
        assert!(encode_rgba(0, 4, &[]).is_none());
    }
}
