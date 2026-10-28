//! Targa decoding for the texture pipeline.
//!
//! Old LE-era mods reference `.tga` files directly and NifSkope resolves
//! them, so Aether does too. The format is trivial next to DDS: an 18-byte
//! header, an optional palette, then bottom-up BGR(A) pixels, optionally
//! run-length encoded. No mips, no compression worth the name — decoding
//! is always the full image.

use crate::dds::Image;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TgaError {
    Truncated,
    Unsupported(String),
}

impl std::fmt::Display for TgaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TgaError::Truncated => write!(f, "TGA is truncated"),
            TgaError::Unsupported(what) => write!(f, "unsupported TGA: {what}"),
        }
    }
}

/// Decode a TGA file to RGBA8.
pub fn decode(data: &[u8]) -> Result<Image, TgaError> {
    if data.len() < 18 {
        return Err(TgaError::Truncated);
    }
    let id_length = data[0] as usize;
    let color_map_type = data[1];
    let image_type = data[2];
    let cmap_len = u16::from_le_bytes([data[5], data[6]]) as usize;
    let cmap_entry_bits = data[7] as usize;
    let width = u16::from_le_bytes([data[12], data[13]]) as u32;
    let height = u16::from_le_bytes([data[14], data[15]]) as u32;
    let depth = data[16] as usize;
    let descriptor = data[17];
    let top_down = descriptor & 0x20 != 0;
    let right_to_left = descriptor & 0x10 != 0;

    let rle = match image_type {
        1 | 2 | 3 => false,
        9 | 10 | 11 => true,
        other => return Err(TgaError::Unsupported(format!("image type {other}"))),
    };
    let colormapped = image_type & 0x3 == 1;
    if width == 0 || height == 0 {
        return Err(TgaError::Unsupported("zero size".into()));
    }

    let mut offset = 18 + id_length;

    // The palette sits between the header and the pixels even when the
    // image itself does not use it (color_map_type says it is present).
    let cmap_entry_bytes = cmap_entry_bits.div_ceil(8);
    let mut palette: Vec<[u8; 4]> = Vec::new();
    if color_map_type == 1 {
        let cmap_bytes = cmap_len * cmap_entry_bytes;
        let raw = data
            .get(offset..offset + cmap_bytes)
            .ok_or(TgaError::Truncated)?;
        palette = raw
            .chunks_exact(cmap_entry_bytes)
            .map(|e| pixel_to_rgba(e, cmap_entry_bits))
            .collect();
        offset += cmap_bytes;
    }
    if colormapped && palette.is_empty() {
        return Err(TgaError::Unsupported("colormapped without a palette".into()));
    }

    let px_bytes = depth.div_ceil(8);
    match (colormapped, depth) {
        (true, 8) | (false, 15 | 16 | 24 | 32) => {}
        (false, 8) if image_type & 0x3 == 3 => {}
        _ => {
            return Err(TgaError::Unsupported(format!(
                "type {image_type} at {depth} bpp"
            )))
        }
    }

    // Unpack to one flat pixel stream first; RLE runs may cross row ends.
    let total = width as usize * height as usize;
    let mut stream: Vec<u8> = Vec::with_capacity(total * px_bytes);
    if rle {
        while stream.len() < total * px_bytes {
            let packet = *data.get(offset).ok_or(TgaError::Truncated)?;
            offset += 1;
            let count = (packet & 0x7F) as usize + 1;
            if packet & 0x80 != 0 {
                let px = data
                    .get(offset..offset + px_bytes)
                    .ok_or(TgaError::Truncated)?;
                offset += px_bytes;
                for _ in 0..count {
                    stream.extend_from_slice(px);
                }
            } else {
                let raw = data
                    .get(offset..offset + count * px_bytes)
                    .ok_or(TgaError::Truncated)?;
                offset += count * px_bytes;
                stream.extend_from_slice(raw);
            }
        }
        stream.truncate(total * px_bytes);
    } else {
        stream.extend_from_slice(
            data.get(offset..offset + total * px_bytes)
                .ok_or(TgaError::Truncated)?,
        );
    }

    let mut rgba = vec![0u8; total * 4];
    for i in 0..total {
        let px = &stream[i * px_bytes..][..px_bytes];
        let color = if colormapped {
            *palette.get(px[0] as usize).unwrap_or(&[0, 0, 0, 255])
        } else if image_type & 0x3 == 3 {
            [px[0], px[0], px[0], 255]
        } else {
            pixel_to_rgba(px, depth)
        };

        // TGA rows run bottom-up unless the descriptor says otherwise.
        let x = i % width as usize;
        let y = i / width as usize;
        let out_x = if right_to_left {
            width as usize - 1 - x
        } else {
            x
        };
        let out_y = if top_down {
            y
        } else {
            height as usize - 1 - y
        };
        let out = (out_y * width as usize + out_x) * 4;
        rgba[out..out + 4].copy_from_slice(&color);
    }

    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// BGR(A) packed pixel of `bits` depth to RGBA8.
fn pixel_to_rgba(px: &[u8], bits: usize) -> [u8; 4] {
    match bits {
        // Bits 0-4 blue, 5-9 green, 10-14 red, 15 attribute. The attribute
        // bit is almost always garbage in the wild; treat it as opaque.
        15 | 16 => {
            let v = u16::from_le_bytes([px[0], px[1]]);
            let scale = |c: u16| ((c & 0x1F) * 255 / 31) as u8;
            [scale(v >> 10), scale(v >> 5), scale(v), 255]
        }
        24 => [px[2], px[1], px[0], 255],
        32 => [px[2], px[1], px[0], px[3]],
        _ => [0, 0, 0, 255],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(image_type: u8, width: u16, height: u16, depth: u8, descriptor: u8) -> Vec<u8> {
        let mut h = vec![0u8; 18];
        h[2] = image_type;
        h[12..14].copy_from_slice(&width.to_le_bytes());
        h[14..16].copy_from_slice(&height.to_le_bytes());
        h[16] = depth;
        h[17] = descriptor;
        h
    }

    /// Bottom-origin 24-bit: the first stored row is the bottom row, and
    /// bytes are BGR — both must be undone.
    #[test]
    fn uncompressed_24_bit_flips_and_swizzles() {
        let mut file = header(2, 2, 2, 24, 0);
        file.extend_from_slice(&[
            255, 0, 0, /* bottom-left: blue */
            0, 0, 255, /* bottom-right: red */
            0, 255, 0, /* top-left: green */
            255, 255, 255, /* top-right: white */
        ]);
        let img = decode(&file).unwrap();
        assert_eq!(&img.rgba[0..4], &[0, 255, 0, 255], "top-left green");
        assert_eq!(&img.rgba[4..8], &[255, 255, 255, 255], "top-right white");
        assert_eq!(&img.rgba[8..12], &[0, 0, 255, 255], "bottom-left blue");
        assert_eq!(&img.rgba[12..16], &[255, 0, 0, 255], "bottom-right red");
    }

    /// An RLE run crossing a row boundary is legal and must keep alpha.
    #[test]
    fn rle_32_bit_run_crosses_rows() {
        let mut file = header(10, 2, 2, 32, 0x20);
        // One run packet covering 3 pixels, then one raw pixel.
        file.extend_from_slice(&[0x82, 10, 20, 30, 128]);
        file.extend_from_slice(&[0x00, 1, 2, 3, 255]);
        let img = decode(&file).unwrap();
        for px in 0..3 {
            assert_eq!(&img.rgba[px * 4..px * 4 + 4], &[30, 20, 10, 128]);
        }
        assert_eq!(&img.rgba[12..16], &[3, 2, 1, 255]);
    }

    #[test]
    fn grayscale_replicates_to_rgb() {
        let mut file = header(3, 2, 1, 8, 0x20);
        file.extend_from_slice(&[0, 200]);
        let img = decode(&file).unwrap();
        assert_eq!(&img.rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(&img.rgba[4..8], &[200, 200, 200, 255]);
    }

    #[test]
    fn colormapped_looks_up_the_palette() {
        let mut file = header(1, 2, 1, 8, 0x20);
        file[1] = 1; // color map present
        file[5..7].copy_from_slice(&2u16.to_le_bytes());
        file[7] = 24;
        file.extend_from_slice(&[0, 0, 255]); // entry 0: red (BGR)
        file.extend_from_slice(&[255, 0, 0]); // entry 1: blue
        file.extend_from_slice(&[1, 0]); // indices
        let img = decode(&file).unwrap();
        assert_eq!(&img.rgba[0..4], &[0, 0, 255, 255], "index 1 is blue");
        assert_eq!(&img.rgba[4..8], &[255, 0, 0, 255], "index 0 is red");
    }

    #[test]
    fn truncated_pixel_data_is_an_error_not_a_panic() {
        let mut file = header(2, 4, 4, 24, 0);
        file.extend_from_slice(&[0; 10]);
        assert_eq!(decode(&file), Err(TgaError::Truncated));
    }
}
