//! Little-endian cursor helpers over byte slices, std-only.

use crate::error::{Error, Result};

/// A little-endian reader over a byte slice with offset tracking.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn corrupt(&self, what: impl Into<String>) -> Error {
        Error::Corrupt {
            offset: self.pos,
            what: what.into(),
        }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(self.corrupt(format!("need {n} bytes, {} left", self.remaining())));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn bool32(&mut self) -> Result<bool> {
        Ok(self.u32()? != 0)
    }

    /// Reads a half-precision float (used by BSTriShape vertex data).
    pub fn f16(&mut self) -> Result<f32> {
        Ok(half_to_f32(self.u16()?))
    }

    /// `SizedString`: u32 length followed by that many bytes (no terminator).
    pub fn sized_string(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        if len > 0x0010_0000 {
            return Err(self.corrupt(format!("SizedString length {len} is implausible")));
        }
        let bytes = self.take(len)?;
        Ok(latin1_to_string(bytes))
    }

    /// `ExportString` / `ShortString`: u8 length *including* the null
    /// terminator, followed by the bytes (last one is `\0`).
    pub fn export_string(&mut self) -> Result<String> {
        let len = self.u8()? as usize;
        let bytes = self.take(len)?;
        let trimmed = bytes.split_last().map(|(_, rest)| rest).unwrap_or(bytes);
        Ok(latin1_to_string(trimmed))
    }

    /// Header line: bytes until `\n` (consumed, not included).
    pub fn line(&mut self) -> Result<String> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != b'\n' {
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            return Err(Error::Corrupt {
                offset: start,
                what: "unterminated header line".into(),
            });
        }
        let s = latin1_to_string(&self.data[start..self.pos]);
        self.pos += 1; // consume '\n'
        Ok(s)
    }
}

/// A little-endian writer producing an owned byte buffer.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }

    pub fn bool32(&mut self, v: bool) {
        self.u32(if v { 1 } else { 0 });
    }

    pub fn sized_string(&mut self, s: &str) {
        let bytes = string_to_latin1(s);
        self.u32(bytes.len() as u32);
        self.bytes(&bytes);
    }

    pub fn export_string(&mut self, s: &str) {
        let mut bytes = string_to_latin1(s);
        bytes.push(0);
        debug_assert!(bytes.len() <= u8::MAX as usize);
        self.u8(bytes.len() as u8);
        self.bytes(&bytes);
    }

    pub fn line(&mut self, s: &str) {
        self.bytes(&string_to_latin1(s));
        self.u8(b'\n');
    }
}

/// NIF strings are effectively Windows-1252/Latin-1; map bytes 1:1 to chars so
/// that unknown encodings survive a round-trip through `String` losslessly for
/// the 0x00..=0xFF range.
pub fn latin1_to_string(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

pub fn string_to_latin1(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
        .collect()
}

/// IEEE 754 half -> f32 (no `half` crate dependency).
pub fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let frac = (h & 0x3ff) as u32;
    let bits = match (exp, frac) {
        (0, 0) => sign << 31,
        (0, f) => {
            // subnormal: normalize
            let mut e = 127 - 15 + 1;
            let mut f = f;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            (sign << 31) | ((e as u32) << 23) | ((f & 0x3ff) << 13)
        }
        (0x1f, 0) => (sign << 31) | 0x7f80_0000,
        (0x1f, f) => (sign << 31) | 0x7f80_0000 | (f << 13),
        (e, f) => (sign << 31) | ((e + 127 - 15) << 23) | (f << 13),
    };
    f32::from_bits(bits)
}

/// f32 -> IEEE 754 half with round-to-nearest-even.
pub fn f32_to_half(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let frac = bits & 0x7f_ffff;
    if exp == 0xff {
        // inf / nan
        return sign | 0x7c00 | if frac != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00; // overflow -> inf
    }
    if e <= 0 {
        if e < -10 {
            return sign; // underflow -> 0
        }
        let f = frac | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = (f >> shift) as u16;
        let round_bit = 1u32 << (shift - 1);
        if f & round_bit != 0 && (f & (round_bit - 1) != 0 || half & 1 != 0) {
            return sign | (half + 1);
        }
        return sign | half;
    }
    let half = sign | ((e as u16) << 10) | ((frac >> 13) as u16);
    let round_bit = 1u32 << 12;
    if frac & round_bit != 0 && (frac & (round_bit - 1) != 0 || half & 1 != 0) {
        half + 1
    } else {
        half
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_roundtrip() {
        for v in [0.0f32, 1.0, -1.0, 0.5, 65504.0, 0.000061035156] {
            let h = f32_to_half(v);
            assert_eq!(half_to_f32(h), v, "value {v}");
        }
    }

    #[test]
    fn strings_roundtrip() {
        let mut w = Writer::new();
        w.sized_string("NiNode");
        w.export_string("Aether");
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.sized_string().unwrap(), "NiNode");
        assert_eq!(r.export_string().unwrap(), "Aether");
        assert!(r.is_at_end());
    }
}
