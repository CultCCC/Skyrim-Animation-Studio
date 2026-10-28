//! Shader and alpha properties for effect and mesh geometry.

use super::{BlockRef, Ctx, Nio, ObjectNet, TypedBlock};
use crate::error::Result;

/// `NiAlphaProperty`. Default flags 4844 = src alpha / inv src alpha blending
/// enabled, testing greater-than at the threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct NiAlphaProperty {
    pub net: ObjectNet,
    pub flags: u16,
    pub threshold: u8,
}

impl Default for NiAlphaProperty {
    fn default() -> Self {
        NiAlphaProperty {
            net: ObjectNet::default(),
            flags: 4844,
            threshold: 128,
        }
    }
}

impl TypedBlock for NiAlphaProperty {
    const TYPE_NAMES: &'static [&'static str] = &["NiAlphaProperty"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.net.sync(s, ctx)?;
        s.u16(&mut self.flags)?;
        s.u8(&mut self.threshold)
    }
}

/// `BSEffectShaderProperty` for Skyrim LE/SE. Identical layout on both: the
/// environment-map texture slots only exist from BS stream 130 (Fallout 4),
/// which is out of scope.
#[derive(Debug, Clone, PartialEq)]
pub struct BSEffectShaderProperty {
    pub net: ObjectNet,
    /// SkyrimShaderPropertyFlags1.
    pub shader_flags1: u32,
    /// SkyrimShaderPropertyFlags2.
    pub shader_flags2: u32,
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    pub source_texture: String,
    /// 3 = wrap S + wrap T.
    pub texture_clamp_mode: u8,
    pub lighting_influence: u8,
    pub env_map_min_lod: u8,
    pub unused_byte: u8,
    pub falloff_start_angle: f32,
    pub falloff_stop_angle: f32,
    pub falloff_start_opacity: f32,
    pub falloff_stop_opacity: f32,
    pub base_color: [f32; 4],
    pub base_color_scale: f32,
    pub soft_falloff_depth: f32,
    pub greyscale_texture: String,
}

impl Default for BSEffectShaderProperty {
    fn default() -> Self {
        BSEffectShaderProperty {
            net: ObjectNet::default(),
            // 0x80000000 = ZBuffer Test; vertex alpha + soft effect are the
            // usual companions for particle effect shaders.
            shader_flags1: 0x8000_0000,
            shader_flags2: 0x20,
            uv_offset: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            source_texture: String::new(),
            texture_clamp_mode: 3,
            lighting_influence: 255,
            env_map_min_lod: 0,
            unused_byte: 0,
            falloff_start_angle: 1.0,
            falloff_stop_angle: 1.0,
            falloff_start_opacity: 0.0,
            falloff_stop_opacity: 0.0,
            base_color: [1.0, 1.0, 1.0, 1.0],
            base_color_scale: 1.0,
            soft_falloff_depth: 100.0,
            greyscale_texture: String::new(),
        }
    }
}

impl TypedBlock for BSEffectShaderProperty {
    const TYPE_NAMES: &'static [&'static str] = &["BSEffectShaderProperty"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.net.sync(s, ctx)?;
        s.u32(&mut self.shader_flags1)?;
        s.u32(&mut self.shader_flags2)?;
        s.f32s(&mut self.uv_offset)?;
        s.f32s(&mut self.uv_scale)?;
        s.sized_string(&mut self.source_texture)?;
        s.u8(&mut self.texture_clamp_mode)?;
        s.u8(&mut self.lighting_influence)?;
        s.u8(&mut self.env_map_min_lod)?;
        s.u8(&mut self.unused_byte)?;
        s.f32(&mut self.falloff_start_angle)?;
        s.f32(&mut self.falloff_stop_angle)?;
        s.f32(&mut self.falloff_start_opacity)?;
        s.f32(&mut self.falloff_stop_opacity)?;
        s.f32s(&mut self.base_color)?;
        s.f32(&mut self.base_color_scale)?;
        s.f32(&mut self.soft_falloff_depth)?;
        s.sized_string(&mut self.greyscale_texture)
    }
}

/// `BSSkyShaderProperty` — the sky dome family (35 vanilla blocks in 4
/// files: atmosphere, clouds, stars, the bloodmoon shadow mask). Head
/// identical to BSEffectShaderProperty's, then the source texture and the
/// sky object TYPE (docs/knowledge/sweep-tail.md bucket 2). The cloud
/// layer index is LOAD-ORDER runtime state, not stored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSSkyShaderProperty {
    pub net: ObjectNet,
    pub shader_flags1: u32,
    pub shader_flags2: u32,
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    pub source_texture: String,
    /// 0 Sun, 1 SunGlare, 2 Atmosphere, 3 Clouds, 4 SkyQuad, 5 Stars,
    /// 6 Moon, 7 MoonShadow. Only 2/3/5/7 are ever authored in shipping
    /// files; the rest are runtime-generated geometry.
    pub sky_object_type: u32,
}

impl TypedBlock for BSSkyShaderProperty {
    const TYPE_NAMES: &'static [&'static str] = &["BSSkyShaderProperty"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.net.sync(s, ctx)?;
        s.u32(&mut self.shader_flags1)?;
        s.u32(&mut self.shader_flags2)?;
        s.f32s(&mut self.uv_offset)?;
        s.f32s(&mut self.uv_scale)?;
        s.sized_string(&mut self.source_texture)?;
        s.u32(&mut self.sky_object_type)
    }
}

/// `BSWaterShaderProperty` — mesh-embedded water surfaces (33 vanilla
/// blocks: Markarth streams, mill ponds...). The block carries FLAGS
/// only; colours, normals and wave params come from the placing ACTI's
/// water TYPE record at runtime, which a lone NIF cannot name — the
/// preview bakes DefaultWater (docs/knowledge/sweep-tail.md bucket 1).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSWaterShaderProperty {
    pub net: ObjectNet,
    pub shader_flags1: u32,
    pub shader_flags2: u32,
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    /// bit0 Displacement, 1 LOD, 2 Depth, 5 Underwater, 6 Reflections,
    /// 7 Refractions, 8 VERTEX_UV (mesh UV x1000 instead of world
    /// position), 9 VERTEX_ALPHA_DEPTH (vertex alpha is the depth fade).
    pub water_flags: u32,
}

impl TypedBlock for BSWaterShaderProperty {
    const TYPE_NAMES: &'static [&'static str] = &["BSWaterShaderProperty"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.net.sync(s, ctx)?;
        s.u32(&mut self.shader_flags1)?;
        s.u32(&mut self.shader_flags2)?;
        s.f32s(&mut self.uv_offset)?;
        s.f32s(&mut self.uv_scale)?;
        s.u32(&mut self.water_flags)
    }
}

/// `BSShaderTextureSet` — the numbered texture slots of a lighting shader.
/// Slot 0 diffuse, 1 normal/gloss, 2 glow/skin, 3 height, 4 environment,
/// 5 environment mask, 6 subsurface, 7 backlight.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSShaderTextureSet {
    pub textures: Vec<String>,
}

impl BSShaderTextureSet {
    pub fn diffuse(&self) -> Option<&str> {
        self.textures.first().map(String::as_str).filter(|t| !t.is_empty())
    }
}

impl TypedBlock for BSShaderTextureSet {
    const TYPE_NAMES: &'static [&'static str] = &["BSShaderTextureSet"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        let mut n = self.textures.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            self.textures.clear();
            self.textures.resize(n.min(64) as usize, String::new());
        }
        for t in &mut self.textures {
            s.sized_string(t)?;
        }
        Ok(())
    }
}

/// The per-shader-type tail of `BSLightingShaderProperty`. The block ends
/// with fields conditional on the shader type, so the variant IS the type
/// for every kind that stores anything.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum LightingShaderTail {
    /// Types with no extra fields (default 0, glow 2, and the rest).
    #[default]
    None,
    /// Type 1.
    EnvironmentMap { scale: f32 },
    /// Type 5.
    SkinTint { color: [f32; 3] },
    /// Type 6.
    HairTint { color: [f32; 3] },
    /// Type 7 (parallax occlusion).
    ParallaxOcc { max_passes: f32, scale: f32 },
    /// Type 11 (multi-layer parallax).
    MultiLayerParallax {
        inner_layer_thickness: f32,
        refraction_scale: f32,
        inner_layer_texture_scale: [f32; 2],
        envmap_strength: f32,
    },
    /// Type 14 (sparkle snow).
    SparkleSnow { parameters: [f32; 4] },
    /// Type 16 (eye environment map).
    EyeEnvmap {
        cubemap_scale: f32,
        left_eye_center: [f32; 3],
        right_eye_center: [f32; 3],
    },
}

/// `BSLightingShaderProperty` for Skyrim LE/SE (identical at bs83/bs100).
///
/// The shader type u32 is serialized BEFORE the name: nif.xml models it as a
/// conditional first field of NiObjectNET, which is where the bytes really
/// sit in every Skyrim file.
#[derive(Debug, Clone, PartialEq)]
pub struct BSLightingShaderProperty {
    pub shader_type: u32,
    pub net: ObjectNet,
    /// SkyrimShaderPropertyFlags1.
    pub shader_flags1: u32,
    /// SkyrimShaderPropertyFlags2.
    pub shader_flags2: u32,
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    pub texture_set: BlockRef,
    pub emissive_color: [f32; 3],
    pub emissive_multiple: f32,
    /// TexClampMode: 3 = wrap S + wrap T.
    pub texture_clamp_mode: u32,
    pub alpha: f32,
    pub refraction_strength: f32,
    pub glossiness: f32,
    pub specular_color: [f32; 3],
    pub specular_strength: f32,
    pub lighting_effect_1: f32,
    pub lighting_effect_2: f32,
    pub tail: LightingShaderTail,
}

impl Default for BSLightingShaderProperty {
    fn default() -> Self {
        BSLightingShaderProperty {
            shader_type: 0,
            net: ObjectNet::default(),
            shader_flags1: 0x8240_0301,
            shader_flags2: 0x8021,
            uv_offset: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            texture_set: BlockRef::NONE,
            emissive_color: [0.0; 3],
            emissive_multiple: 1.0,
            texture_clamp_mode: 3,
            alpha: 1.0,
            refraction_strength: 0.0,
            glossiness: 80.0,
            specular_color: [1.0; 3],
            specular_strength: 1.0,
            lighting_effect_1: 0.3,
            lighting_effect_2: 2.0,
            tail: LightingShaderTail::None,
        }
    }
}

impl TypedBlock for BSLightingShaderProperty {
    const TYPE_NAMES: &'static [&'static str] = &["BSLightingShaderProperty"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        s.u32(&mut self.shader_type)?;
        self.net.sync(s, ctx)?;
        s.u32(&mut self.shader_flags1)?;
        s.u32(&mut self.shader_flags2)?;
        s.f32s(&mut self.uv_offset)?;
        s.f32s(&mut self.uv_scale)?;
        s.block_ref(&mut self.texture_set)?;
        s.f32s(&mut self.emissive_color)?;
        s.f32(&mut self.emissive_multiple)?;
        s.u32(&mut self.texture_clamp_mode)?;
        s.f32(&mut self.alpha)?;
        s.f32(&mut self.refraction_strength)?;
        s.f32(&mut self.glossiness)?;
        s.f32s(&mut self.specular_color)?;
        s.f32(&mut self.specular_strength)?;
        s.f32(&mut self.lighting_effect_1)?;
        s.f32(&mut self.lighting_effect_2)?;

        // The tail is picked by shader type. On read, replace whatever the
        // default was; on write, the variant must match the type — sync from
        // the variant the struct actually holds.
        if s.reading() {
            self.tail = match self.shader_type {
                1 => LightingShaderTail::EnvironmentMap { scale: 0.0 },
                5 => LightingShaderTail::SkinTint { color: [0.0; 3] },
                6 => LightingShaderTail::HairTint { color: [0.0; 3] },
                7 => LightingShaderTail::ParallaxOcc {
                    max_passes: 0.0,
                    scale: 0.0,
                },
                11 => LightingShaderTail::MultiLayerParallax {
                    inner_layer_thickness: 0.0,
                    refraction_scale: 0.0,
                    inner_layer_texture_scale: [0.0; 2],
                    envmap_strength: 0.0,
                },
                14 => LightingShaderTail::SparkleSnow { parameters: [0.0; 4] },
                16 => LightingShaderTail::EyeEnvmap {
                    cubemap_scale: 0.0,
                    left_eye_center: [0.0; 3],
                    right_eye_center: [0.0; 3],
                },
                _ => LightingShaderTail::None,
            };
        }
        match &mut self.tail {
            LightingShaderTail::None => {}
            LightingShaderTail::EnvironmentMap { scale } => s.f32(scale)?,
            LightingShaderTail::SkinTint { color } | LightingShaderTail::HairTint { color } => {
                s.f32s(color)?
            }
            LightingShaderTail::ParallaxOcc { max_passes, scale } => {
                s.f32(max_passes)?;
                s.f32(scale)?;
            }
            LightingShaderTail::MultiLayerParallax {
                inner_layer_thickness,
                refraction_scale,
                inner_layer_texture_scale,
                envmap_strength,
            } => {
                s.f32(inner_layer_thickness)?;
                s.f32(refraction_scale)?;
                s.f32s(inner_layer_texture_scale)?;
                s.f32(envmap_strength)?;
            }
            LightingShaderTail::SparkleSnow { parameters } => s.f32s(parameters)?,
            LightingShaderTail::EyeEnvmap {
                cubemap_scale,
                left_eye_center,
                right_eye_center,
            } => {
                s.f32(cubemap_scale)?;
                s.f32s(left_eye_center)?;
                s.f32s(right_eye_center)?;
            }
        }
        Ok(())
    }
}
