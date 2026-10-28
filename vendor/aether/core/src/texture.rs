//! One entry point for anything that wants pixels out of a texture file.
//!
//! The asset resolver can hand back a `.dds` or a `.tga` (NifSkope-style
//! alternate extensions), and callers should not care which; they call
//! [`decode_capped`] and get RGBA8 either way.

pub use crate::dds::{CubeTexture, Image};

/// Decode a texture of any supported container to RGBA8.
///
/// `max_side` picks a smaller stored DDS mip when one exists (see
/// `dds::decode_capped`); TGA has no mip chain, so it always decodes in
/// full and the cap only documents intent.
pub fn decode_capped(bytes: &[u8], max_side: u32) -> Result<Image, String> {
    if bytes.starts_with(b"DDS ") {
        return crate::dds::decode_capped(bytes, max_side).map_err(|e| e.to_string());
    }
    // TGA has no magic number; anything that is not a DDS gets one try.
    crate::tga::decode(bytes).map_err(|e| e.to_string())
}

/// What a texture file IS, read from its header: not the mip a preview
/// happened to decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureInfo {
    pub width: u32,
    pub height: u32,
    pub mip_count: u32,
    pub format: &'static str,
}

/// Read [`TextureInfo`] from a DDS or TGA header without decoding pixels.
pub fn info(bytes: &[u8]) -> Option<TextureInfo> {
    if bytes.starts_with(b"DDS ") {
        let (width, height, mip_count, format) = crate::dds::info(bytes).ok()?;
        return Some(TextureInfo { width, height, mip_count, format });
    }
    // TGA: no magic, no mips; the size sits at a fixed offset.
    if bytes.len() < 18 {
        return None;
    }
    Some(TextureInfo {
        width: u16::from_le_bytes([bytes[12], bytes[13]]) as u32,
        height: u16::from_le_bytes([bytes[14], bytes[15]]) as u32,
        mip_count: 1,
        format: "TGA",
    })
}

/// Decode all six faces of an environment cubemap to RGBA8.
///
/// Cubemaps only ever ship as DDS — TGA has no cube container — so unlike
/// [`decode_capped`] there is no fallback: anything that is not a DDS cubemap
/// is an error, which is what lets the renderer distinguish "bad file" from
/// "flat texture wired into a cube slot".
pub fn decode_cube_capped(bytes: &[u8], max_side: u32) -> Result<CubeTexture, String> {
    crate::dds::decode_cube_capped(bytes, max_side).map_err(|e| e.to_string())
}
