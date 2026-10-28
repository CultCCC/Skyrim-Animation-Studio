//! Minimal DDS reader: enough to preview and render Skyrim's textures.
//!
//! Skyrim ships BC1/BC3/BC5 (`DXT1`/`DXT5`/`ATI2`) almost exclusively, with the
//! occasional uncompressed BGRA surface; SSE adds BC7 through the DX10 header
//! extension. One mip is decoded per call — the top one by default, or a
//! smaller stored level via `decode_capped`, which is what keeps thumbnail
//! browsing cheap. Environment cubemaps (`textures\cubemaps\*.dds`) decode
//! through [`decode_cube_capped`], which returns all six faces for a wgpu
//! cube-texture upload.
//!
//! std-only, so it lives alongside the rest of the dependency-free core.

/// A decoded top-level surface, tightly packed RGBA8.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major, no padding.
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// A decoded cubemap: six square RGBA8 faces of equal size, in DDS storage
/// order +X, -X, +Y, -Y, +Z, -Z — the order wgpu's cube view expects, so the
/// renderer can upload the array as-is.
#[derive(Debug, Clone, PartialEq)]
pub struct CubeTexture {
    pub side: u32,
    /// Each face is `side * side * 4` bytes, row-major, no padding.
    pub faces: [Vec<u8>; 6],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DdsError {
    NotADds,
    Truncated { need: usize, have: usize },
    /// A format we can identify but do not decode yet.
    UnsupportedFormat(String),
}

impl std::fmt::Display for DdsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DdsError::NotADds => write!(f, "not a DDS file"),
            DdsError::Truncated { need, have } => {
                write!(f, "DDS is truncated: need {need} bytes, have {have}")
            }
            DdsError::UnsupportedFormat(name) => write!(f, "unsupported DDS format: {name}"),
        }
    }
}

const HEADER_SIZE: usize = 124;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_ALPHA: u32 = 0x2;
const DDPF_PALETTEINDEXED8: u32 = 0x20;
const DDPF_LUMINANCE: u32 = 0x2_0000;
const DDSCAPS2_VOLUME: u32 = 0x20_0000;
const DDSCAPS2_CUBEMAP: u32 = 0x200;
/// The six positive/negative X/Y/Z face-present flags together.
const DDSCAPS2_CUBEMAP_ALL_FACES: u32 = 0xFC00;
/// `DDS_RESOURCE_MISC_TEXTURECUBE` in the DX10 extension header.
const DX10_MISC_TEXTURECUBE: u32 = 0x4;

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Decode the top mip of a DDS file to RGBA8.
pub fn decode(data: &[u8]) -> Result<Image, DdsError> {
    decode_capped(data, u32::MAX)
}

/// Decode the smallest mip whose longest side is still at least `max_side`,
/// falling back to the smallest available.
///
/// Thumbnails do not need a 2048x2048 surface: decoding a stored 128x128 mip
/// instead is ~256x less work and ~256x less memory, which is the difference
/// between a browser that scrolls and one that freezes the editor.
///
/// On a cubemap this yields the +X face: each face carries its own full mip
/// chain and +X is stored first, so the walk below never leaves it. Use
/// [`decode_cube_capped`] when all six faces are wanted.
pub fn decode_capped(data: &[u8], max_side: u32) -> Result<Image, DdsError> {
    let header = parse_header(data)?;
    let mut mip_count = header.mip_count;

    // A volume texture stores `depth` slices per level, which the 2D mip
    // walk below cannot step over; decode the top level's first slice.
    if header.caps2 & DDSCAPS2_VOLUME != 0 && header.depth > 1 {
        mip_count = 1;
    }

    // Walk the mip chain to the first level small enough for the caller,
    // skipping the bytes of every larger level.
    let mut level_w = header.width;
    let mut level_h = header.height;
    let mut body_start = header.body;
    for _ in 1..mip_count {
        if level_w.max(level_h) <= max_side || (level_w <= 1 && level_h <= 1) {
            break;
        }
        let level_bytes = surface_bytes(header.format, level_w, level_h);
        if body_start + level_bytes >= data.len() {
            break; // no room for another level; use what we have
        }
        body_start += level_bytes;
        level_w = (level_w / 2).max(1);
        level_h = (level_h / 2).max(1);
    }

    decode_surface(
        header.format,
        level_w,
        level_h,
        &data[body_start..],
        header.palette.map(|p| &data[p..p + 1024]),
    )
}

/// The file's own size, mip count and storage format, read from the
/// header alone -- what the file IS, where a decoded image is only the
/// mip a caller asked for.
pub fn info(data: &[u8]) -> Result<(u32, u32, u32, &'static str), DdsError> {
    let h = parse_header(data)?;
    let name = match h.format {
        Format::Bc1 => "BC1",
        Format::Bc2 => "BC2",
        Format::Bc3 => "BC3",
        Format::Bc4 => "BC4",
        Format::Bc5 => "BC5",
        Format::Bc7 => "BC7",
        Format::Dx10 => "DX10",
        Format::Pal8 => "P8",
        Format::Raw { bits: 32, a_mask, .. } if a_mask != 0 => "RGBA8",
        Format::Raw { bits: 32, .. } => "RGBX8",
        Format::Raw { bits: 24, .. } => "RGB8",
        Format::Raw { bits: 16, .. } => "16-bit",
        Format::Raw { bits: 8, a_mask, .. } if a_mask != 0 => "A8",
        Format::Raw { bits: 8, .. } => "L8",
        Format::Raw { .. } => "uncompressed",
    };
    Ok((h.width, h.height, h.mip_count, name))
}

/// Decode all six faces of a cubemap at full resolution.
pub fn decode_cube(data: &[u8]) -> Result<CubeTexture, DdsError> {
    decode_cube_capped(data, u32::MAX)
}

/// Decode all six faces of a cubemap, picking the smallest stored mip whose
/// side is still at least `max_side` (same policy as [`decode_capped`]).
///
/// A cubemap stores its faces sequentially, each with its own complete mip
/// chain, so face `i`'s level sits at `body + i * face_stride + level_offset`.
/// Both container variants are accepted: the legacy header (caps2 bit 0x200
/// plus the per-face flags) and the DX10 extension (`miscFlag` TEXTURECUBE);
/// a cube array decodes as its first cube.
pub fn decode_cube_capped(data: &[u8], max_side: u32) -> Result<CubeTexture, DdsError> {
    let header = parse_header(data)?;
    let legacy = header.caps2 & DDSCAPS2_CUBEMAP != 0;
    let dx10 = header.dx10_misc & DX10_MISC_TEXTURECUBE != 0;
    if !legacy && !dx10 {
        return Err(DdsError::UnsupportedFormat("not a cubemap".into()));
    }
    // A legacy header may flag fewer than six stored faces. No Skyrim file
    // does, and the renderer needs the full cube, so partial cubes are
    // refused rather than silently mis-indexed. (DX10 cubemaps always store
    // all six, whatever the legacy caps say.)
    if !dx10 && header.caps2 & DDSCAPS2_CUBEMAP_ALL_FACES != DDSCAPS2_CUBEMAP_ALL_FACES {
        let stored = (header.caps2 & DDSCAPS2_CUBEMAP_ALL_FACES).count_ones();
        return Err(DdsError::UnsupportedFormat(format!(
            "partial cubemap ({stored} of 6 faces)"
        )));
    }
    if header.width != header.height {
        return Err(DdsError::UnsupportedFormat(format!(
            "cubemap with non-square {}x{} faces",
            header.width, header.height
        )));
    }

    // Distance from one face's data to the next: its whole mip chain.
    let mut face_stride = 0usize;
    let mut w = header.width;
    for _ in 0..header.mip_count {
        face_stride += surface_bytes(header.format, w, w);
        w = (w / 2).max(1);
    }
    let need = header.body + face_stride * 6;
    if data.len() < need {
        return Err(DdsError::Truncated {
            need,
            have: data.len(),
        });
    }

    // Same level choice as the 2D path: the first stored level within the cap.
    let mut side = header.width;
    let mut level_offset = 0usize;
    for _ in 1..header.mip_count {
        if side <= max_side || side <= 1 {
            break;
        }
        level_offset += surface_bytes(header.format, side, side);
        side = (side / 2).max(1);
    }

    let palette = header.palette.map(|p| &data[p..p + 1024]);
    let mut faces: [Vec<u8>; 6] = Default::default();
    for (i, face) in faces.iter_mut().enumerate() {
        let start = header.body + i * face_stride + level_offset;
        *face = decode_surface(header.format, side, side, &data[start..], palette)?.rgba;
    }
    Ok(CubeTexture { side, faces })
}

/// Everything the fixed header (plus the optional DX10 extension) says,
/// resolved to a decodable [`Format`] and the offset of the first surface byte.
struct Header {
    width: u32,
    height: u32,
    depth: u32,
    mip_count: u32,
    caps2: u32,
    /// DX10 `miscFlag`; 0 when the file has no DX10 extension header.
    dx10_misc: u32,
    format: Format,
    /// First surface byte, past every header and the palette if any.
    body: usize,
    /// Offset of the 1024-byte RGBA palette (`Pal8` only).
    palette: Option<usize>,
}

fn parse_header(data: &[u8]) -> Result<Header, DdsError> {
    if data.len() < 4 + HEADER_SIZE || &data[0..4] != b"DDS " {
        return Err(DdsError::NotADds);
    }
    let height = u32_at(data, 12);
    let width = u32_at(data, 16);
    let depth = u32_at(data, 24);
    let mip_count = u32_at(data, 28).max(1);
    let caps2 = u32_at(data, 112);

    // DDS_PIXELFORMAT starts at offset 76 (4 magic + 72 into the header).
    let pf = 76;
    let pf_flags = u32_at(data, pf + 4);
    let four_cc = &data[pf + 8..pf + 12];
    let rgb_bit_count = u32_at(data, pf + 12);
    let r_mask = u32_at(data, pf + 16);
    let g_mask = u32_at(data, pf + 20);
    let b_mask = u32_at(data, pf + 24);
    let a_mask = u32_at(data, pf + 28);

    let mut offset = 4 + HEADER_SIZE;
    let mut format = if pf_flags & DDPF_FOURCC != 0 {
        match four_cc {
            b"DXT1" => Format::Bc1,
            // DXT2/DXT4 are the premultiplied-alpha twins; the block layout
            // is identical and the preview does not un-premultiply.
            b"DXT2" | b"DXT3" => Format::Bc2,
            b"DXT4" | b"DXT5" => Format::Bc3,
            b"ATI1" | b"BC4U" => Format::Bc4,
            b"ATI2" | b"BC5U" => Format::Bc5,
            b"DX10" => Format::Dx10,
            other => {
                return Err(DdsError::UnsupportedFormat(
                    String::from_utf8_lossy(other).into_owned(),
                ))
            }
        }
    } else if pf_flags & DDPF_RGB != 0 {
        Format::Raw {
            bits: rgb_bit_count,
            r_mask,
            g_mask,
            b_mask,
            a_mask: if pf_flags & DDPF_ALPHAPIXELS != 0 {
                a_mask
            } else {
                0
            },
        }
    } else if pf_flags & DDPF_LUMINANCE != 0 {
        // L8 / L16 / A8L8 / A4L4: one grey channel, replicated to RGB.
        Format::Raw {
            bits: rgb_bit_count,
            r_mask,
            g_mask: r_mask,
            b_mask: r_mask,
            a_mask: if pf_flags & DDPF_ALPHAPIXELS != 0 {
                a_mask
            } else {
                0
            },
        }
    } else if pf_flags & DDPF_ALPHA != 0 {
        // A8: alpha only, colour reads as black (D3D semantics).
        Format::Raw {
            bits: rgb_bit_count,
            r_mask: 0,
            g_mask: 0,
            b_mask: 0,
            a_mask,
        }
    } else if pf_flags & DDPF_PALETTEINDEXED8 != 0 {
        Format::Pal8
    } else {
        return Err(DdsError::UnsupportedFormat("unknown pixel format".into()));
    };

    let mut dx10_misc = 0;
    if format == Format::Dx10 {
        // DXGI_FORMAT is the first field of the DX10 extension header.
        if data.len() < offset + 20 {
            return Err(DdsError::Truncated {
                need: offset + 20,
                have: data.len(),
            });
        }
        let dxgi = u32_at(data, offset);
        dx10_misc = u32_at(data, offset + 8);
        offset += 20;
        format = match dxgi {
            // *_UNORM and *_UNORM_SRGB decode identically here; the preview
            // does not do colour management.
            71 | 72 => Format::Bc1,
            74 | 75 => Format::Bc2,
            77 | 78 => Format::Bc3,
            80 | 81 => Format::Bc4,
            83 | 84 => Format::Bc5,
            98 | 99 => Format::Bc7,
            28 | 29 => Format::Raw {
                bits: 32,
                r_mask: 0x0000_00FF,
                g_mask: 0x0000_FF00,
                b_mask: 0x00FF_0000,
                a_mask: 0xFF00_0000,
            },
            87 | 88 => Format::Raw {
                bits: 32,
                r_mask: 0x00FF_0000,
                g_mask: 0x0000_FF00,
                b_mask: 0x0000_00FF,
                a_mask: 0xFF00_0000,
            },
            // R8_UNORM: single channel, shown as grey like L8.
            61 => Format::Raw {
                bits: 8,
                r_mask: 0xFF,
                g_mask: 0xFF,
                b_mask: 0xFF,
                a_mask: 0,
            },
            // A8_UNORM.
            65 => Format::Raw {
                bits: 8,
                r_mask: 0,
                g_mask: 0,
                b_mask: 0,
                a_mask: 0xFF,
            },
            // B5G6R5 / B5G5R5A1 / B4G4R4A4.
            85 => Format::Raw {
                bits: 16,
                r_mask: 0xF800,
                g_mask: 0x07E0,
                b_mask: 0x001F,
                a_mask: 0,
            },
            86 => Format::Raw {
                bits: 16,
                r_mask: 0x7C00,
                g_mask: 0x03E0,
                b_mask: 0x001F,
                a_mask: 0x8000,
            },
            115 => Format::Raw {
                bits: 16,
                r_mask: 0x0F00,
                g_mask: 0x00F0,
                b_mask: 0x000F,
                a_mask: 0xF000,
            },
            other => return Err(DdsError::UnsupportedFormat(format!("DXGI {other}"))),
        };
    }

    // A palettized surface stores its 256 RGBA entries between the header
    // and the index data.
    let palette = if format == Format::Pal8 {
        if data.len() < offset + 1024 {
            return Err(DdsError::Truncated {
                need: offset + 1024,
                have: data.len(),
            });
        }
        let p = offset;
        offset += 1024;
        Some(p)
    } else {
        None
    };

    Ok(Header {
        width,
        height,
        depth,
        mip_count,
        caps2,
        dx10_misc,
        format,
        body: offset,
        palette,
    })
}

/// Bytes one mip level occupies for a format.
fn surface_bytes(format: Format, width: u32, height: u32) -> usize {
    match format {
        Format::Raw { bits, .. } => width as usize * height as usize * (bits / 8) as usize,
        Format::Pal8 => width as usize * height as usize,
        Format::Bc1 | Format::Bc4 => width.div_ceil(4) as usize * height.div_ceil(4) as usize * 8,
        Format::Bc2 | Format::Bc3 | Format::Bc5 | Format::Bc7 => {
            width.div_ceil(4) as usize * height.div_ceil(4) as usize * 16
        }
        Format::Dx10 => 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc7,
    Dx10,
    Pal8,
    Raw {
        bits: u32,
        r_mask: u32,
        g_mask: u32,
        b_mask: u32,
        a_mask: u32,
    },
}

fn decode_surface(
    format: Format,
    width: u32,
    height: u32,
    body: &[u8],
    palette: Option<&[u8]>,
) -> Result<Image, DdsError> {
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];

    match format {
        Format::Pal8 => {
            let palette = palette.expect("Pal8 always carries a palette");
            let need = width as usize * height as usize;
            if body.len() < need {
                return Err(DdsError::Truncated {
                    need,
                    have: body.len(),
                });
            }
            for i in 0..need {
                let entry = &palette[body[i] as usize * 4..][..4];
                rgba[i * 4..i * 4 + 4].copy_from_slice(entry);
            }
        }
        Format::Raw {
            bits,
            r_mask,
            g_mask,
            b_mask,
            a_mask,
        } => {
            let bytes = (bits / 8) as usize;
            if bytes == 0 || bytes > 4 {
                return Err(DdsError::UnsupportedFormat(format!("{bits}-bit raw")));
            }
            let need = width as usize * height as usize * bytes;
            if body.len() < need {
                return Err(DdsError::Truncated {
                    need,
                    have: body.len(),
                });
            }
            for i in 0..(width as usize * height as usize) {
                let mut px = 0u32;
                for b in 0..bytes {
                    px |= (body[i * bytes + b] as u32) << (8 * b);
                }
                let out = i * 4;
                rgba[out] = extract(px, r_mask);
                rgba[out + 1] = extract(px, g_mask);
                rgba[out + 2] = extract(px, b_mask);
                rgba[out + 3] = if a_mask == 0 { 255 } else { extract(px, a_mask) };
            }
        }
        _ => {
            let block_bytes = match format {
                Format::Bc1 | Format::Bc4 => 8,
                Format::Bc2 | Format::Bc3 | Format::Bc5 | Format::Bc7 => 16,
                Format::Dx10 => unreachable!("resolved above"),
                Format::Raw { .. } | Format::Pal8 => unreachable!("handled above"),
            };
            let bw = width.div_ceil(4) as usize;
            let bh = height.div_ceil(4) as usize;
            let need = bw * bh * block_bytes;
            if body.len() < need {
                return Err(DdsError::Truncated {
                    need,
                    have: body.len(),
                });
            }
            for by in 0..bh {
                for bx in 0..bw {
                    let block = &body[(by * bw + bx) * block_bytes..][..block_bytes];
                    let texels = match format {
                        Format::Bc1 => decode_bc1(block),
                        Format::Bc2 => decode_bc2(block),
                        Format::Bc3 => decode_bc3(block),
                        Format::Bc4 => decode_bc4(block),
                        Format::Bc5 => decode_bc5(block),
                        Format::Bc7 => {
                            crate::bc7::decode_block(block.try_into().expect("16-byte block"))
                        }
                        _ => unreachable!(),
                    };
                    blit_block(&mut rgba, width, height, bx, by, &texels);
                }
            }
        }
    }

    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// Pull a channel out of a packed pixel and scale it to 8 bits.
fn extract(px: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let width = mask.count_ones();
    let value = (px & mask) >> shift;
    let max = (1u32 << width) - 1;
    ((value * 255 + max / 2) / max) as u8
}

fn blit_block(rgba: &mut [u8], width: u32, height: u32, bx: usize, by: usize, texels: &[[u8; 4]; 16]) {
    for ty in 0..4 {
        let y = by * 4 + ty;
        if y >= height as usize {
            break;
        }
        for tx in 0..4 {
            let x = bx * 4 + tx;
            if x >= width as usize {
                break;
            }
            let dst = (y * width as usize + x) * 4;
            rgba[dst..dst + 4].copy_from_slice(&texels[ty * 4 + tx]);
        }
    }
}

fn rgb565(value: u16) -> [u8; 3] {
    let r = ((value >> 11) & 0x1F) as u32;
    let g = ((value >> 5) & 0x3F) as u32;
    let b = (value & 0x1F) as u32;
    [
        ((r * 255 + 15) / 31) as u8,
        ((g * 255 + 31) / 63) as u8,
        ((b * 255 + 15) / 31) as u8,
    ]
}

/// The four-colour palette shared by BC1/BC2/BC3 colour blocks. BC1 encodes
/// 1-bit alpha by ordering the endpoints, which BC2/BC3 never do.
fn color_palette(block: &[u8], punch_through: bool) -> ([[u8; 4]; 4], bool) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let a = rgb565(c0);
    let b = rgb565(c1);
    let mut palette = [[0u8; 4]; 4];
    palette[0] = [a[0], a[1], a[2], 255];
    palette[1] = [b[0], b[1], b[2], 255];

    let one_bit_alpha = punch_through && c0 <= c1;
    if one_bit_alpha {
        for i in 0..3 {
            palette[2][i] = ((a[i] as u16 + b[i] as u16) / 2) as u8;
        }
        palette[2][3] = 255;
        palette[3] = [0, 0, 0, 0];
    } else {
        for i in 0..3 {
            palette[2][i] = ((2 * a[i] as u16 + b[i] as u16) / 3) as u8;
            palette[3][i] = ((a[i] as u16 + 2 * b[i] as u16) / 3) as u8;
        }
        palette[2][3] = 255;
        palette[3][3] = 255;
    }
    (palette, one_bit_alpha)
}

fn color_indices(block: &[u8], palette: &[[u8; 4]; 4]) -> [[u8; 4]; 16] {
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    let mut out = [[0u8; 4]; 16];
    for (i, texel) in out.iter_mut().enumerate() {
        *texel = palette[((bits >> (2 * i)) & 0b11) as usize];
    }
    out
}

fn decode_bc1(block: &[u8]) -> [[u8; 4]; 16] {
    let (palette, _) = color_palette(block, true);
    color_indices(block, &palette)
}

fn decode_bc2(block: &[u8]) -> [[u8; 4]; 16] {
    let (palette, _) = color_palette(&block[8..], false);
    let mut out = color_indices(&block[8..], &palette);
    // 4 bits of alpha per texel in the first 8 bytes.
    for (i, texel) in out.iter_mut().enumerate() {
        let nibble = (block[i / 2] >> (4 * (i % 2))) & 0x0F;
        texel[3] = nibble * 17;
    }
    out
}

fn decode_bc3(block: &[u8]) -> [[u8; 4]; 16] {
    let (palette, _) = color_palette(&block[8..], false);
    let mut out = color_indices(&block[8..], &palette);
    let alpha = decode_alpha_block(block);
    for (i, texel) in out.iter_mut().enumerate() {
        texel[3] = alpha[i];
    }
    out
}

fn decode_bc4(block: &[u8]) -> [[u8; 4]; 16] {
    let red = decode_alpha_block(block);
    let mut out = [[0u8; 4]; 16];
    for (i, texel) in out.iter_mut().enumerate() {
        *texel = [red[i], red[i], red[i], 255];
    }
    out
}

fn decode_bc5(block: &[u8]) -> [[u8; 4]; 16] {
    let red = decode_alpha_block(&block[0..8]);
    let green = decode_alpha_block(&block[8..16]);
    let mut out = [[0u8; 4]; 16];
    for (i, texel) in out.iter_mut().enumerate() {
        // Blue is reconstructed as a unit normal's Z, which is what BC5 is
        // used for in these games (tangent-space normal maps).
        let nx = red[i] as f32 / 127.5 - 1.0;
        let ny = green[i] as f32 / 127.5 - 1.0;
        let nz = (1.0 - (nx * nx + ny * ny).clamp(0.0, 1.0)).sqrt();
        *texel = [red[i], green[i], ((nz + 1.0) * 127.5) as u8, 255];
    }
    out
}

/// The 8-byte interpolated-scalar block used by BC3 alpha, BC4 and BC5.
fn decode_alpha_block(block: &[u8]) -> [u8; 16] {
    let a0 = block[0];
    let a1 = block[1];
    let mut palette = [0u8; 8];
    palette[0] = a0;
    palette[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            palette[i + 1] = (((7 - i) as u16 * a0 as u16 + i as u16 * a1 as u16) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            palette[i + 1] = (((5 - i) as u16 * a0 as u16 + i as u16 * a1 as u16) / 5) as u8;
        }
        palette[6] = 0;
        palette[7] = 255;
    }

    // 16 three-bit indices packed into the remaining 6 bytes.
    let mut bits: u64 = 0;
    for (i, &b) in block[2..8].iter().enumerate() {
        bits |= (b as u64) << (8 * i);
    }
    let mut out = [0u8; 16];
    for (i, value) in out.iter_mut().enumerate() {
        *value = palette[((bits >> (3 * i)) & 0b111) as usize];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal DDS around a body; `four_cc` empty means uncompressed
    /// 32-bit BGRA.
    fn dds(width: u32, height: u32, four_cc: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 4 + HEADER_SIZE];
        d[0..4].copy_from_slice(b"DDS ");
        d[4..8].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        d[12..16].copy_from_slice(&height.to_le_bytes());
        d[16..20].copy_from_slice(&width.to_le_bytes());
        let pf = 76;
        d[pf..pf + 4].copy_from_slice(&32u32.to_le_bytes());
        if four_cc == b"\0\0\0\0" {
            d[pf + 4..pf + 8].copy_from_slice(&(DDPF_RGB | DDPF_ALPHAPIXELS).to_le_bytes());
            d[pf + 12..pf + 16].copy_from_slice(&32u32.to_le_bytes());
            d[pf + 16..pf + 20].copy_from_slice(&0x00FF_0000u32.to_le_bytes()); // R
            d[pf + 20..pf + 24].copy_from_slice(&0x0000_FF00u32.to_le_bytes()); // G
            d[pf + 24..pf + 28].copy_from_slice(&0x0000_00FFu32.to_le_bytes()); // B
            d[pf + 28..pf + 32].copy_from_slice(&0xFF00_0000u32.to_le_bytes()); // A
        } else {
            d[pf + 4..pf + 8].copy_from_slice(&DDPF_FOURCC.to_le_bytes());
            d[pf + 8..pf + 12].copy_from_slice(four_cc);
        }
        d.extend_from_slice(body);
        d
    }

    /// A BC1 block: two endpoints plus 2-bit indices.
    fn bc1_block(c0: u16, c1: u16, indices: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&c0.to_le_bytes());
        b.extend_from_slice(&c1.to_le_bytes());
        b.extend_from_slice(&indices.to_le_bytes());
        b
    }

    const RED_565: u16 = 0xF800;
    const BLUE_565: u16 = 0x001F;

    #[test]
    fn rejects_non_dds() {
        assert_eq!(decode(b"not a dds file at all").unwrap_err(), DdsError::NotADds);
    }

    #[test]
    fn bc1_decodes_endpoints_and_interpolants() {
        // index 0 -> c0 (red), 1 -> c1 (blue), 2 -> 2/3 red, 3 -> 1/3 red.
        let indices = 0b11_10_01_00;
        let file = dds(4, 4, b"DXT1", &bc1_block(RED_565, BLUE_565, indices));
        let img = decode(&file).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255], "index 0 is endpoint 0");
        assert_eq!(img.pixel(1, 0), [0, 0, 255, 255], "index 1 is endpoint 1");
        let two_thirds = img.pixel(2, 0);
        assert!(
            two_thirds[0] > two_thirds[2],
            "index 2 leans toward endpoint 0, got {two_thirds:?}"
        );
        let one_third = img.pixel(3, 0);
        assert!(
            one_third[2] > one_third[0],
            "index 3 leans toward endpoint 1, got {one_third:?}"
        );
    }

    #[test]
    fn bc1_punch_through_alpha_is_honoured() {
        // c0 <= c1 selects the 3-colour + transparent palette; index 3 is a
        // fully transparent texel. Getting this backwards makes every
        // cutout texture opaque.
        let indices = 0b11_11_11_11;
        let file = dds(4, 4, b"DXT1", &bc1_block(BLUE_565, RED_565, indices));
        let img = decode(&file).unwrap();
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 0]);

        // With c0 > c1 the same index is an opaque interpolant.
        let file = dds(4, 4, b"DXT1", &bc1_block(RED_565, BLUE_565, indices));
        let img = decode(&file).unwrap();
        assert_eq!(img.pixel(0, 0)[3], 255);
    }

    #[test]
    fn bc3_alpha_block_interpolates_between_endpoints() {
        let mut body = vec![0u8; 16];
        body[0] = 255; // a0
        body[1] = 0; // a1
        // All indices 0 -> alpha 255 everywhere.
        body[8..10].copy_from_slice(&RED_565.to_le_bytes());
        body[10..12].copy_from_slice(&BLUE_565.to_le_bytes());
        let img = decode(&dds(4, 4, b"DXT5", &body)).unwrap();
        assert_eq!(img.pixel(0, 0)[3], 255);

        // Index 1 everywhere -> alpha a1 = 0.
        let mut body2 = body.clone();
        for b in body2[2..8].iter_mut() {
            *b = 0b0100_1001; // repeating 3-bit value 1
        }
        let img2 = decode(&dds(4, 4, b"DXT5", &body2)).unwrap();
        assert_eq!(img2.pixel(0, 0)[3], 0);
    }

    #[test]
    fn uncompressed_bgra_maps_channels_correctly() {
        // One 2x1 surface: opaque red, half-transparent green (B,G,R,A order).
        let body = vec![0, 0, 255, 255, 0, 255, 0, 128];
        let img = decode(&dds(2, 1, b"\0\0\0\0", &body)).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 255, 0, 128]);
    }

    #[test]
    fn non_multiple_of_four_sizes_are_cropped_not_padded() {
        // A 3x2 BC1 texture still occupies one 4x4 block, but the decoded
        // image must be exactly 3x2 so UVs line up.
        let file = dds(3, 2, b"DXT1", &bc1_block(RED_565, BLUE_565, 0));
        let img = decode(&file).unwrap();
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(img.rgba.len(), 3 * 2 * 4);
    }

    #[test]
    fn capped_decoding_picks_a_smaller_stored_mip() {
        // 4x4 BC1 with 3 mips (4, 2, 1) — one block each at this size. The
        // levels get different endpoints so the chosen one is identifiable by
        // colour alone.
        let mut body = Vec::new();
        body.extend(bc1_block(RED_565, RED_565, 0)); // 4x4
        body.extend(bc1_block(BLUE_565, BLUE_565, 0)); // 2x2
        body.extend(bc1_block(RED_565, RED_565, 0)); // 1x1
        let mut file = dds(4, 4, b"DXT1", &body);
        // dwMipMapCount sits 24 bytes into the header, which starts at 4.
        file[28..32].copy_from_slice(&3u32.to_le_bytes());

        let full = decode(&file).unwrap();
        assert_eq!((full.width, full.height), (4, 4));
        assert_eq!(full.pixel(0, 0), [255, 0, 0, 255], "top mip is red");

        let small = decode_capped(&file, 2).unwrap();
        assert_eq!((small.width, small.height), (2, 2), "picked the 2x2 mip");
        assert_eq!(small.pixel(0, 0), [0, 0, 255, 255], "2x2 mip is blue");

        // Asking for less than the smallest level still yields something.
        let tiny = decode_capped(&file, 1).unwrap();
        assert_eq!((tiny.width, tiny.height), (1, 1));
    }

    #[test]
    fn capped_decoding_is_a_no_op_without_mips() {
        // No mip chain to walk: the top level is all there is, whatever the cap.
        let file = dds(4, 4, b"DXT1", &bc1_block(RED_565, BLUE_565, 0));
        let img = decode_capped(&file, 1).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
    }

    #[test]
    fn truncated_data_is_reported_not_read_past() {
        let file = dds(64, 64, b"DXT1", &[0u8; 8]);
        assert!(matches!(decode(&file), Err(DdsError::Truncated { .. })));
    }

    /// An uncompressed DDS with explicit pixel-format flags and masks.
    fn dds_uncompressed(
        width: u32,
        height: u32,
        flags: u32,
        bits: u32,
        masks: [u32; 4],
        body: &[u8],
    ) -> Vec<u8> {
        let mut d = dds(width, height, b"\0\0\0\0", body);
        let pf = 76;
        d[pf + 4..pf + 8].copy_from_slice(&flags.to_le_bytes());
        d[pf + 12..pf + 16].copy_from_slice(&bits.to_le_bytes());
        d[pf + 16..pf + 20].copy_from_slice(&masks[0].to_le_bytes());
        d[pf + 20..pf + 24].copy_from_slice(&masks[1].to_le_bytes());
        d[pf + 24..pf + 28].copy_from_slice(&masks[2].to_le_bytes());
        d[pf + 28..pf + 32].copy_from_slice(&masks[3].to_le_bytes());
        d
    }

    #[test]
    fn luminance8_replicates_to_grey() {
        // L8, the format of vanilla noisevolume.dds: one channel, grey out.
        let file = dds_uncompressed(2, 1, DDPF_LUMINANCE, 8, [0xFF, 0, 0, 0], &[0, 200]);
        let img = decode(&file).unwrap();
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [200, 200, 200, 255]);
    }

    #[test]
    fn a8l8_keeps_luminance_and_alpha_independent() {
        let file = dds_uncompressed(
            1,
            1,
            DDPF_LUMINANCE | DDPF_ALPHAPIXELS,
            16,
            [0x00FF, 0, 0, 0xFF00],
            &[200, 128],
        );
        let img = decode(&file).unwrap();
        assert_eq!(img.pixel(0, 0), [200, 200, 200, 128]);
    }

    #[test]
    fn alpha_only_is_black_with_alpha() {
        // A8 has D3D semantics: colour reads as black, alpha from the data.
        let file = dds_uncompressed(2, 1, DDPF_ALPHA, 8, [0, 0, 0, 0xFF], &[64, 255]);
        let img = decode(&file).unwrap();
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 64]);
        assert_eq!(img.pixel(1, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn pal8_looks_up_the_palette() {
        let mut palette = vec![0u8; 1024];
        palette[0..4].copy_from_slice(&[255, 0, 0, 255]); // entry 0: red
        palette[4..8].copy_from_slice(&[0, 0, 255, 128]); // entry 1: translucent blue
        let mut body = palette;
        body.extend_from_slice(&[1, 0]); // indices
        let file = dds_uncompressed(2, 1, DDPF_PALETTEINDEXED8, 8, [0, 0, 0, 0], &body);
        let img = decode(&file).unwrap();
        assert_eq!(img.pixel(0, 0), [0, 0, 255, 128]);
        assert_eq!(img.pixel(1, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn dxt2_and_dxt4_alias_their_unpremultiplied_twins() {
        // Same body, different fourcc — the block layout is identical.
        let mut body = vec![0u8; 16];
        body[0..8].copy_from_slice(&u64::MAX.to_le_bytes()); // BC2 alpha: all 15
        body[8..10].copy_from_slice(&RED_565.to_le_bytes());
        body[10..12].copy_from_slice(&RED_565.to_le_bytes());
        let via_dxt2 = decode(&dds(4, 4, b"DXT2", &body)).unwrap();
        let via_dxt3 = decode(&dds(4, 4, b"DXT3", &body)).unwrap();
        assert_eq!(via_dxt2.rgba, via_dxt3.rgba);
        assert_eq!(via_dxt2.pixel(0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn volume_textures_decode_the_top_slice_and_skip_the_mip_walk() {
        // A 2x2x2 L8 volume with a bogus mip count: the 2D mip walk cannot
        // step over slices, so the cap must not move past the top level.
        let body = [10u8, 20, 30, 40, 50, 60, 70, 80];
        let mut file = dds_uncompressed(2, 2, DDPF_LUMINANCE, 8, [0xFF, 0, 0, 0], &body);
        file[24..28].copy_from_slice(&2u32.to_le_bytes()); // depth
        file[28..32].copy_from_slice(&2u32.to_le_bytes()); // mip count
        file[112..116].copy_from_slice(&DDSCAPS2_VOLUME.to_le_bytes());
        let img = decode_capped(&file, 1).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixel(0, 0), [10, 10, 10, 255]);
    }

    #[test]
    fn legacy_cubemap_decodes_six_faces_with_their_own_mip_chains() {
        // Six 4x4 BC1 faces, two mips each. Every face gets a unique top
        // colour and, as its 2x2 mip, the top colour of the OPPOSITE face —
        // so a wrong face stride or level offset changes what comes out.
        const GREEN_565: u16 = 0x07E0;
        const WHITE_565: u16 = 0xFFFF;
        const BLACK_565: u16 = 0x0000;
        const MAGENTA_565: u16 = 0xF81F;
        let tops = [RED_565, GREEN_565, BLUE_565, WHITE_565, BLACK_565, MAGENTA_565];
        let mut body = Vec::new();
        for i in 0..6 {
            body.extend(bc1_block(tops[i], tops[i], 0)); // 4x4
            body.extend(bc1_block(tops[5 - i], tops[5 - i], 0)); // 2x2 mip
        }
        let mut file = dds(4, 4, b"DXT1", &body);
        file[28..32].copy_from_slice(&2u32.to_le_bytes());
        file[112..116]
            .copy_from_slice(&(DDSCAPS2_CUBEMAP | DDSCAPS2_CUBEMAP_ALL_FACES).to_le_bytes());

        let cube = decode_cube(&file).unwrap();
        assert_eq!(cube.side, 4);
        let expect = [
            [255u8, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
            [0, 0, 0, 255],
            [255, 0, 255, 255],
        ];
        for (i, face) in cube.faces.iter().enumerate() {
            assert_eq!(face.len(), 4 * 4 * 4);
            assert_eq!(&face[0..4], &expect[i], "face {i} top mip");
        }

        // The cap walks each face's own chain: face i's 2x2 mip carries
        // face (5-i)'s colour.
        let capped = decode_cube_capped(&file, 2).unwrap();
        assert_eq!(capped.side, 2);
        for (i, face) in capped.faces.iter().enumerate() {
            assert_eq!(&face[0..4], &expect[5 - i], "face {i} 2x2 mip");
        }

        // The 2D entry points keep returning the +X face, mips included —
        // the thumbnail browser must not change behaviour on cubemaps.
        let flat = decode(&file).unwrap();
        assert_eq!((flat.width, flat.height), (4, 4));
        assert_eq!(flat.rgba, cube.faces[0]);
        let flat_small = decode_capped(&file, 2).unwrap();
        assert_eq!((flat_small.width, flat_small.height), (2, 2));
        assert_eq!(flat_small.rgba, capped.faces[0]);

        // Cutting the last face short must be reported, not sliced past.
        let short = &file[..file.len() - 8];
        assert!(matches!(decode_cube(short), Err(DdsError::Truncated { .. })));
    }

    #[test]
    fn dx10_cubemap_is_detected_by_the_misc_flag() {
        // 1x1 RGBA8 faces behind a DX10 header with TEXTURECUBE set and no
        // legacy cube caps — the shape modern packers emit.
        let mut ext = Vec::new();
        ext.extend_from_slice(&28u32.to_le_bytes()); // DXGI R8G8B8A8_UNORM
        ext.extend_from_slice(&3u32.to_le_bytes()); // DDS_DIMENSION_TEXTURE2D
        ext.extend_from_slice(&DX10_MISC_TEXTURECUBE.to_le_bytes());
        ext.extend_from_slice(&6u32.to_le_bytes()); // arraySize
        ext.extend_from_slice(&0u32.to_le_bytes()); // miscFlags2
        for i in 0..6u8 {
            ext.extend_from_slice(&[i * 40, 255 - i * 40, i, 255]);
        }
        let file = dds(1, 1, b"DX10", &ext);
        let cube = decode_cube(&file).unwrap();
        assert_eq!(cube.side, 1);
        for (i, face) in cube.faces.iter().enumerate() {
            let v = i as u8;
            assert_eq!(face.as_slice(), &[v * 40, 255 - v * 40, v, 255], "face {i}");
        }
    }

    #[test]
    fn non_cubemaps_and_partial_cubemaps_are_refused() {
        // A plain 2D texture must not decode as a cube.
        let flat = dds(4, 4, b"DXT1", &bc1_block(RED_565, BLUE_565, 0));
        assert!(matches!(
            decode_cube(&flat),
            Err(DdsError::UnsupportedFormat(msg)) if msg == "not a cubemap"
        ));

        // Legacy caps can flag fewer than six faces; indexing the missing
        // ones would read other faces' data, so refuse outright.
        let mut partial = dds(4, 4, b"DXT1", &bc1_block(RED_565, BLUE_565, 0));
        partial[112..116].copy_from_slice(&(DDSCAPS2_CUBEMAP | 0x400 | 0x800).to_le_bytes());
        assert!(matches!(
            decode_cube(&partial),
            Err(DdsError::UnsupportedFormat(msg)) if msg.contains("partial cubemap")
        ));
    }
}
