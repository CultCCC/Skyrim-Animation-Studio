//! Time controllers used by particle systems.

use super::{BlockRef, Ctx, Nio, StringIdx, TypedBlock};
use crate::error::Result;

/// `NiTimeController` shared fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ControllerBase {
    pub next_controller: BlockRef,
    /// Bit 3 (value 8) = active; cycle type in bits 1-2. 72 = active+clamp.
    pub flags: u16,
    pub frequency: f32,
    pub phase: f32,
    pub start_time: f32,
    pub stop_time: f32,
    /// Back-pointer to the controlled object.
    pub target: BlockRef,
}

impl Default for ControllerBase {
    fn default() -> Self {
        ControllerBase {
            next_controller: BlockRef::NONE,
            flags: 72,
            frequency: 1.0,
            phase: 0.0,
            start_time: 0.0,
            stop_time: 1.0,
            target: BlockRef::NONE,
        }
    }
}

impl ControllerBase {
    pub fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.block_ref(&mut self.next_controller)?;
        s.u16(&mut self.flags)?;
        s.f32(&mut self.frequency)?;
        s.f32(&mut self.phase)?;
        s.f32(&mut self.start_time)?;
        s.f32(&mut self.stop_time)?;
        s.block_ptr(&mut self.target)
    }
}

/// `NiPSysUpdateCtlr`: drives the whole system's simulation tick.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysUpdateCtlr {
    pub base: ControllerBase,
}

impl TypedBlock for NiPSysUpdateCtlr {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysUpdateCtlr"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)
    }
}

/// `NiPSysEmitterCtlr`: animates birth rate / emitter visibility. Finds its
/// modifier on the *target* system by `modifier_name` (target-first lookup).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysEmitterCtlr {
    pub base: ControllerBase,
    /// Birth-rate interpolator (`NiFloatInterpolator` in vanilla assets).
    pub interpolator: BlockRef,
    pub modifier_name: StringIdx,
    /// Emitter on/off interpolator (`NiBoolInterpolator` in vanilla assets).
    pub visibility_interpolator: BlockRef,
}

impl TypedBlock for NiPSysEmitterCtlr {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysEmitterCtlr"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)?;
        s.string_idx(&mut self.modifier_name)?;
        s.block_ref(&mut self.visibility_interpolator)
    }
}

/// `NiFloatInterpolator`: constant pose value when `data` is NONE, otherwise
/// keyframes live in the referenced `NiFloatData`.
#[derive(Debug, Clone, PartialEq)]
pub struct NiFloatInterpolator {
    pub value: f32,
    pub data: BlockRef,
}

impl Default for NiFloatInterpolator {
    fn default() -> Self {
        NiFloatInterpolator {
            value: 0.0,
            data: BlockRef::NONE,
        }
    }
}

impl TypedBlock for NiFloatInterpolator {
    const TYPE_NAMES: &'static [&'static str] = &["NiFloatInterpolator"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.f32(&mut self.value)?;
        s.block_ref(&mut self.data)
    }
}

/// `NiBoolInterpolator`: constant pose value when `data` is NONE, otherwise
/// keyframes live in the referenced `NiBoolData`.
///
/// The value byte is kept raw: vanilla files store bytes other than 0/1 here
/// (e.g. 2), and normalizing through `bool` would break byte-identical
/// round-trips. Any non-zero byte means "on".
#[derive(Debug, Clone, PartialEq)]
pub struct NiBoolInterpolator {
    pub value: u8,
    pub data: BlockRef,
}

impl NiBoolInterpolator {
    pub fn is_on(&self) -> bool {
        self.value != 0
    }
}

impl Default for NiBoolInterpolator {
    fn default() -> Self {
        NiBoolInterpolator {
            value: 1,
            data: BlockRef::NONE,
        }
    }
}

impl TypedBlock for NiBoolInterpolator {
    const TYPE_NAMES: &'static [&'static str] = &["NiBoolInterpolator"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u8(&mut self.value)?;
        s.block_ref(&mut self.data)
    }
}

/// `NiPSysModifierActiveCtlr`: animates a modifier's active flag by name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysModifierActiveCtlr {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
    pub modifier_name: StringIdx,
}

impl TypedBlock for NiPSysModifierActiveCtlr {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysModifierActiveCtlr"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)?;
        s.string_idx(&mut self.modifier_name)
    }
}

/// `NiFloatData` — a float key group. Keys are kept as flat floats with the
/// stride the interpolation dictates (1 linear: t,v; 2 quadratic: t,v,fwd,
/// bwd; 3 TBC: t,v,tension,bias,continuity; 5 const: t,v), so any key type
/// round-trips bit-identically.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiFloatData {
    pub interpolation: u32,
    pub floats: Vec<f32>,
}

impl NiFloatData {
    pub fn key_stride(interpolation: u32) -> Option<usize> {
        match interpolation {
            1 | 5 => Some(2),
            2 => Some(4),
            3 => Some(5),
            _ => None,
        }
    }

    /// `(time, value)` pairs regardless of key type.
    pub fn keys(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        let stride = Self::key_stride(self.interpolation).unwrap_or(2);
        self.floats.chunks_exact(stride).map(|k| (k[0], k[1]))
    }

    /// `(time, value, fwd, bwd)` — Hermite tangents when the keys are
    /// quadratic (type 2), zero otherwise. Zero tangents make the Hermite
    /// basis a smoothstep, NOT a lerp, so callers must branch on
    /// [`Self::is_quadratic`] rather than feed these to one formula.
    pub fn hermite_keys(&self) -> Vec<(f32, f32, f32, f32)> {
        let stride = Self::key_stride(self.interpolation).unwrap_or(2);
        self.floats
            .chunks_exact(stride)
            .map(|k| {
                if self.interpolation == 2 {
                    (k[0], k[1], k[2], k[3])
                } else {
                    (k[0], k[1], 0.0, 0.0)
                }
            })
            .collect()
    }

    pub fn is_quadratic(&self) -> bool {
        self.interpolation == 2
    }
}

impl TypedBlock for NiFloatData {
    const TYPE_NAMES: &'static [&'static str] = &["NiFloatData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        let stride = Self::key_stride(self.interpolation).unwrap_or(2);
        let mut n = (self.floats.len() / stride) as u32;
        s.u32(&mut n)?;
        if n > 0 {
            s.u32(&mut self.interpolation)?;
            let stride = Self::key_stride(self.interpolation).ok_or_else(|| {
                crate::error::Error::Corrupt {
                    offset: 0,
                    what: format!(
                        "NiFloatData interpolation {} has no defined key layout",
                        self.interpolation
                    ),
                }
            })?;
            if s.reading() {
                self.floats.clear();
                self.floats.resize(n as usize * stride, 0.0);
            }
            s.f32s(&mut self.floats)?;
        } else if s.reading() {
            self.floats.clear();
        }
        Ok(())
    }
}

/// `NiBoolData` — boolean keys. Byte keys are linear or constant in the
/// wild; the quadratic/TBC layouts do not exist for bytes, and a file
/// claiming one is refused rather than guessed at.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiBoolData {
    pub interpolation: u32,
    /// `(time, raw byte)` — non-0/1 bytes occur in vanilla data.
    pub keys: Vec<(f32, u8)>,
}

impl TypedBlock for NiBoolData {
    const TYPE_NAMES: &'static [&'static str] = &["NiBoolData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        let mut n = self.keys.len() as u32;
        s.u32(&mut n)?;
        if n > 0 {
            s.u32(&mut self.interpolation)?;
            if !matches!(self.interpolation, 1 | 5) {
                return Err(crate::error::Error::Corrupt {
                    offset: 0,
                    what: format!(
                        "NiBoolData interpolation {} is not a byte-key layout",
                        self.interpolation
                    ),
                });
            }
            if s.reading() {
                self.keys.clear();
                self.keys.resize(n as usize, (0.0, 0));
            }
            for key in &mut self.keys {
                s.f32(&mut key.0)?;
                s.u8(&mut key.1)?;
            }
        } else if s.reading() {
            self.keys.clear();
        }
        Ok(())
    }
}

/// `NiBoolTimelineInterpolator` — layout-identical to `NiBoolInterpolator`;
/// the runtime difference (no skipped keys between updates) matters to the
/// game, not to the serializer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiBoolTimelineInterpolator {
    pub value: u8,
    pub data: BlockRef,
}

impl TypedBlock for NiBoolTimelineInterpolator {
    const TYPE_NAMES: &'static [&'static str] = &["NiBoolTimelineInterpolator"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u8(&mut self.value)?;
        s.block_ref(&mut self.data)
    }
}

/// The blend interpolators, in the manager-controlled form (flags bit 0
/// set) that every sequence-driven interpolator in real files uses. The
/// free-running form serializes seven more fields plus an item array; a
/// file using it is refused so the layout is never guessed.
#[derive(Debug, Clone, PartialEq)]
pub struct NiBlendFloatInterpolator {
    pub flags: u8,
    pub array_size: u8,
    pub weight_threshold: f32,
    pub value: f32,
}

impl Default for NiBlendFloatInterpolator {
    fn default() -> Self {
        NiBlendFloatInterpolator {
            flags: 1, // manager controlled
            array_size: 0,
            weight_threshold: 0.0,
            value: f32::MIN,
        }
    }
}

fn require_manager_controlled(flags: u8, what: &str) -> Result<()> {
    if flags & 1 == 0 {
        return Err(crate::error::Error::Corrupt {
            offset: 0,
            what: format!("{what}: free-running blend interpolators are not supported"),
        });
    }
    Ok(())
}

impl TypedBlock for NiBlendFloatInterpolator {
    const TYPE_NAMES: &'static [&'static str] = &["NiBlendFloatInterpolator"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u8(&mut self.flags)?;
        require_manager_controlled(self.flags, "NiBlendFloatInterpolator")?;
        s.u8(&mut self.array_size)?;
        s.f32(&mut self.weight_threshold)?;
        s.f32(&mut self.value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NiBlendBoolInterpolator {
    pub flags: u8,
    pub array_size: u8,
    pub weight_threshold: f32,
    pub value: u8,
}

impl Default for NiBlendBoolInterpolator {
    fn default() -> Self {
        NiBlendBoolInterpolator {
            flags: 1,
            array_size: 0,
            weight_threshold: 0.0,
            value: 2,
        }
    }
}

impl TypedBlock for NiBlendBoolInterpolator {
    const TYPE_NAMES: &'static [&'static str] = &["NiBlendBoolInterpolator"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u8(&mut self.flags)?;
        require_manager_controlled(self.flags, "NiBlendBoolInterpolator")?;
        s.u8(&mut self.array_size)?;
        s.f32(&mut self.weight_threshold)?;
        s.u8(&mut self.value)
    }
}

/// `NiTransformInterpolator`: a pose transform (translation, quaternion
/// rotation, uniform scale) plus optional keyed data.
#[derive(Debug, Clone, PartialEq)]
pub struct NiTransformInterpolator {
    pub translation: [f32; 3],
    /// (w, x, y, z), NetImmerse order.
    pub rotation: [f32; 4],
    pub scale: f32,
    pub data: BlockRef,
}

impl Default for NiTransformInterpolator {
    fn default() -> Self {
        NiTransformInterpolator {
            translation: [0.0; 3],
            rotation: [1.0, 0.0, 0.0, 0.0],
            scale: 1.0,
            data: BlockRef::NONE,
        }
    }
}

impl TypedBlock for NiTransformInterpolator {
    const TYPE_NAMES: &'static [&'static str] = &["NiTransformInterpolator"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.f32s(&mut self.translation)?;
        s.f32s(&mut self.rotation)?;
        s.f32(&mut self.scale)?;
        s.block_ref(&mut self.data)
    }
}

/// One `KeyGroup<float>` — the wire struct three NiTransformData channels
/// share. Keys stay flat with the stride the interpolation dictates (see
/// `NiFloatData::key_stride`), so every key type round-trips bit-identically.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FloatKeyGroup {
    pub interpolation: u32,
    pub floats: Vec<f32>,
}

impl FloatKeyGroup {
    /// `(time, value)` pairs regardless of key type.
    pub fn keys(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        let stride = NiFloatData::key_stride(self.interpolation).unwrap_or(2);
        self.floats.chunks_exact(stride).map(|k| (k[0], k[1]))
    }

    fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        let stride = NiFloatData::key_stride(self.interpolation).unwrap_or(2);
        let mut n = (self.floats.len() / stride) as u32;
        s.u32(&mut n)?;
        if n > 0 {
            s.u32(&mut self.interpolation)?;
            let stride = NiFloatData::key_stride(self.interpolation).ok_or_else(|| {
                crate::error::Error::Corrupt {
                    offset: 0,
                    what: format!(
                        "float key group interpolation {} has no defined layout",
                        self.interpolation
                    ),
                }
            })?;
            if s.reading() {
                self.floats.clear();
                self.floats.resize(n as usize * stride, 0.0);
            }
            s.f32s(&mut self.floats)?;
        } else if s.reading() {
            self.floats.clear();
        }
        Ok(())
    }
}

/// `KeyGroup<Vector3>` — translation keys. Same flat-floats scheme; the
/// stride grows by three per tangent set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vec3KeyGroup {
    pub interpolation: u32,
    pub floats: Vec<f32>,
}

impl Vec3KeyGroup {
    pub fn key_stride(interpolation: u32) -> Option<usize> {
        match interpolation {
            1 | 5 => Some(4),  // t + xyz
            2 => Some(10),     // t + xyz + forward + backward
            3 => Some(7),      // t + xyz + TBC
            _ => None,
        }
    }

    /// `(time, value)` pairs regardless of key type.
    pub fn keys(&self) -> impl Iterator<Item = (f32, [f32; 3])> + '_ {
        let stride = Self::key_stride(self.interpolation).unwrap_or(4);
        self.floats
            .chunks_exact(stride)
            .map(|k| (k[0], [k[1], k[2], k[3]]))
    }

    fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        let stride = Self::key_stride(self.interpolation).unwrap_or(4);
        let mut n = (self.floats.len() / stride) as u32;
        s.u32(&mut n)?;
        if n > 0 {
            s.u32(&mut self.interpolation)?;
            let stride = Self::key_stride(self.interpolation).ok_or_else(|| {
                crate::error::Error::Corrupt {
                    offset: 0,
                    what: format!(
                        "vector key group interpolation {} has no defined layout",
                        self.interpolation
                    ),
                }
            })?;
            if s.reading() {
                self.floats.clear();
                self.floats.resize(n as usize * stride, 0.0);
            }
            s.f32s(&mut self.floats)?;
        } else if s.reading() {
            self.floats.clear();
        }
        Ok(())
    }
}

/// `NiTransformData` (né `NiKeyframeData`, renamed at 10.2 with the same
/// layout): quaternion *or* per-axis euler rotation keys, plus translation
/// and scale key groups.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiTransformData {
    /// As stored: the quat key count, or *1* when `rotation_type` is 4
    /// (XYZ), whose real counts live in the per-axis groups.
    pub num_rotation_keys: u32,
    /// Valid only when `num_rotation_keys != 0`.
    pub rotation_type: u32,
    /// Flat quat keys: `t, w, x, y, z` (stride 5), plus TBC (stride 8) for
    /// type 3. QuatKeys never have tangents, whatever the type says.
    pub quaternion_keys: Vec<f32>,
    /// Euler channels when `rotation_type` is 4.
    pub xyz_rotations: [FloatKeyGroup; 3],
    pub translations: Vec3KeyGroup,
    pub scales: FloatKeyGroup,
}

impl NiTransformData {
    pub fn quat_key_stride(rotation_type: u32) -> Option<usize> {
        match rotation_type {
            1 | 2 | 5 => Some(5),
            3 => Some(8),
            _ => None,
        }
    }

    /// `(time, quaternion wxyz)` pairs regardless of key type.
    pub fn quat_keys(&self) -> impl Iterator<Item = (f32, [f32; 4])> + '_ {
        let stride = Self::quat_key_stride(self.rotation_type).unwrap_or(5);
        self.quaternion_keys
            .chunks_exact(stride)
            .map(|k| (k[0], [k[1], k[2], k[3], k[4]]))
    }
}

impl TypedBlock for NiTransformData {
    const TYPE_NAMES: &'static [&'static str] = &["NiTransformData", "NiKeyframeData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u32(&mut self.num_rotation_keys)?;
        if self.num_rotation_keys != 0 {
            s.u32(&mut self.rotation_type)?;
            if self.rotation_type == 4 {
                for group in &mut self.xyz_rotations {
                    group.sync(s)?;
                }
            } else {
                let stride =
                    Self::quat_key_stride(self.rotation_type).ok_or_else(|| {
                        crate::error::Error::Corrupt {
                            offset: 0,
                            what: format!(
                                "NiTransformData rotation type {} has no defined layout",
                                self.rotation_type
                            ),
                        }
                    })?;
                if s.reading() {
                    self.quaternion_keys.clear();
                    self.quaternion_keys
                        .resize(self.num_rotation_keys as usize * stride, 0.0);
                }
                s.f32s(&mut self.quaternion_keys)?;
            }
        } else if s.reading() {
            self.quaternion_keys.clear();
        }
        self.translations.sync(s)?;
        self.scales.sync(s)
    }
}

/// `BSEffectShaderPropertyFloatController` — animates one float of a
/// `BSEffectShaderProperty`, selected by `controlled_variable`
/// (0 emissive multiple, 5 alpha transparency, 6/7 UV offsets ...).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSEffectShaderPropertyFloatController {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
    pub controlled_variable: u32,
}

impl TypedBlock for BSEffectShaderPropertyFloatController {
    const TYPE_NAMES: &'static [&'static str] = &["BSEffectShaderPropertyFloatController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)?;
        s.u32(&mut self.controlled_variable)
    }
}






/// `BSProceduralLightningController` — the game generates the bolt mesh
/// at runtime; the block stores the recipe. Nine interpolator slots in
/// GetInterpolatorID order (Generation, Mutation, Subdivision,
/// NumBranches, NumBranchesVar, Length, LengthVar, Width, ArcOffset —
/// RE 0x140D5FC50), then the streamed parameters. 95-byte payload,
/// verified on all 74 vanilla instances with zero leftover bytes.
///
/// The trailing shader ref is NULL in every vanilla file and the 1.5.97
/// render path never reads it — the bolt's material is CLONED from the
/// target node's first child, a hidden dummy BSTriShape whose whole job
/// is to carry the effect shader and alpha property.
#[derive(Debug, Clone, PartialEq)]
pub struct BSProceduralLightningController {
    pub base: ControllerBase,
    pub generation: BlockRef,
    pub mutation: BlockRef,
    pub subdivision: BlockRef,
    pub num_branches_interp: BlockRef,
    pub num_branches_var_interp: BlockRef,
    pub length_interp: BlockRef,
    pub length_var_interp: BlockRef,
    pub width_interp: BlockRef,
    pub arc_offset_interp: BlockRef,
    /// log2 of the segment count; the runtime refuses 0 and > 12.
    pub subdivisions: u16,
    pub num_branches: u16,
    pub num_branches_variation: u16,
    pub length: f32,
    pub length_variation: f32,
    pub width: f32,
    pub child_width_mult: f32,
    pub arc_offset: f32,
    pub fade_main_bolt: bool,
    pub fade_child_bolts: bool,
    pub animate_arc_offset: bool,
    pub shader_property: BlockRef,
}

impl Default for BSProceduralLightningController {
    fn default() -> Self {
        BSProceduralLightningController {
            base: ControllerBase::default(),
            generation: BlockRef::NONE,
            mutation: BlockRef::NONE,
            subdivision: BlockRef::NONE,
            num_branches_interp: BlockRef::NONE,
            num_branches_var_interp: BlockRef::NONE,
            length_interp: BlockRef::NONE,
            length_var_interp: BlockRef::NONE,
            width_interp: BlockRef::NONE,
            arc_offset_interp: BlockRef::NONE,
            subdivisions: 6,
            num_branches: 1,
            num_branches_variation: 1,
            length: 512.0,
            length_variation: 30.0,
            width: 16.0,
            child_width_mult: 0.75,
            arc_offset: 20.0,
            fade_main_bolt: true,
            fade_child_bolts: true,
            animate_arc_offset: true,
            shader_property: BlockRef::NONE,
        }
    }
}

impl TypedBlock for BSProceduralLightningController {
    const TYPE_NAMES: &'static [&'static str] = &["BSProceduralLightningController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.generation)?;
        s.block_ref(&mut self.mutation)?;
        s.block_ref(&mut self.subdivision)?;
        s.block_ref(&mut self.num_branches_interp)?;
        s.block_ref(&mut self.num_branches_var_interp)?;
        s.block_ref(&mut self.length_interp)?;
        s.block_ref(&mut self.length_var_interp)?;
        s.block_ref(&mut self.width_interp)?;
        s.block_ref(&mut self.arc_offset_interp)?;
        s.u16(&mut self.subdivisions)?;
        s.u16(&mut self.num_branches)?;
        s.u16(&mut self.num_branches_variation)?;
        s.f32(&mut self.length)?;
        s.f32(&mut self.length_variation)?;
        s.f32(&mut self.width)?;
        s.f32(&mut self.child_width_mult)?;
        s.f32(&mut self.arc_offset)?;
        s.bool8(&mut self.fade_main_bolt)?;
        s.bool8(&mut self.fade_child_bolts)?;
        s.bool8(&mut self.animate_arc_offset)?;
        s.block_ref(&mut self.shader_property)
    }
}

/// `BSLightingShaderPropertyFloatController` — one float of a lit
/// material, chosen by `controlled_variable` (LightingShaderControlledFloat:
/// 0 refraction, 8 env-map scale, 9 glossiness, 10 specular strength,
/// 11 emissive multiple, 12 alpha, 20-23 UV offset/scale — the values
/// vanilla actually ships; runtime map RE'd @ 0x141321310).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSLightingShaderPropertyFloatController {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
    pub controlled_variable: u32,
}

impl TypedBlock for BSLightingShaderPropertyFloatController {
    const TYPE_NAMES: &'static [&'static str] = &["BSLightingShaderPropertyFloatController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)?;
        s.u32(&mut self.controlled_variable)
    }
}

/// `BSLightingShaderPropertyColorController` — 0 = specular rgb,
/// 1 = emissive rgb (RE @ 0x141321D90). Carries NiPoint3 tracks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSLightingShaderPropertyColorController {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
    pub controlled_color: u32,
}

impl TypedBlock for BSLightingShaderPropertyColorController {
    const TYPE_NAMES: &'static [&'static str] = &["BSLightingShaderPropertyColorController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)?;
        s.u32(&mut self.controlled_color)
    }
}

/// `BSEffectShaderPropertyColorController` — the effect shader's own
/// colour animator. Byte-identical in layout to the lighting one, but a
/// DIFFERENT block type with a different enum: `EffectShaderControlledColor`
/// has exactly one member, 0 = Emissive Color, where
/// `LightingShaderControlledColor` numbers 0 = Specular, 1 = Emissive.
///
/// Confirmed in 1.5.97 `Update` @ `0x141322860`: with the variable 0 it
/// writes three floats into `target->material[+72]`, i.e. the base
/// colour RGB, leaving alpha alone.
///
/// It needs its own view rather than a second name on the lighting one
/// because `decode` gates on `TYPE_NAMES` — the walk used to hand this
/// block to the lighting view, which refused it, and the `Err` arm
/// silently walked on. 662 blocks in 322 files never animated.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSEffectShaderPropertyColorController {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
    pub controlled_color: u32,
}

impl TypedBlock for BSEffectShaderPropertyColorController {
    const TYPE_NAMES: &'static [&'static str] = &["BSEffectShaderPropertyColorController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)?;
        s.u32(&mut self.controlled_color)
    }
}

/// `BSNiAlphaPropertyTestRefController` — animates the alpha-test
/// threshold. The float track is in RAW 0-255 units; the runtime stores
/// `(u8)trunc(clamp(v, 0, 255))` (RE @ 0x141322C80). No fields of its
/// own.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSNiAlphaPropertyTestRefController {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
}

impl TypedBlock for BSNiAlphaPropertyTestRefController {
    const TYPE_NAMES: &'static [&'static str] = &["BSNiAlphaPropertyTestRefController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)
    }
}

/// `NiBlendPoint3Interpolator` — the colour twin of
/// [`NiBlendFloatInterpolator`]: manager-controlled compact form only,
/// value is the "no pose" −FLT_MAX sentinel per component at rest.
#[derive(Debug, Clone, PartialEq)]
pub struct NiBlendPoint3Interpolator {
    pub flags: u8,
    pub array_size: u8,
    pub weight_threshold: f32,
    pub value: [f32; 3],
}

impl Default for NiBlendPoint3Interpolator {
    fn default() -> Self {
        NiBlendPoint3Interpolator {
            flags: 1, // manager controlled
            array_size: 0,
            weight_threshold: 0.0,
            value: [f32::MIN; 3],
        }
    }
}

impl TypedBlock for NiBlendPoint3Interpolator {
    const TYPE_NAMES: &'static [&'static str] = &["NiBlendPoint3Interpolator"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u8(&mut self.flags)?;
        require_manager_controlled(self.flags, "NiBlendPoint3Interpolator")?;
        s.u8(&mut self.array_size)?;
        s.f32(&mut self.weight_threshold)?;
        s.f32s(&mut self.value)
    }
}

/// `NiPoint3Interpolator` — colour tracks. Value is the pose (each
/// component -FLT_MAX = "no pose" sentinel, same convention as floats).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPoint3Interpolator {
    pub value: [f32; 3],
    pub data: BlockRef,
}

impl TypedBlock for NiPoint3Interpolator {
    const TYPE_NAMES: &'static [&'static str] = &["NiPoint3Interpolator"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.f32s(&mut self.value)?;
        s.block_ref(&mut self.data)
    }
}

/// `NiPosData` — a Vector3 key group, same KeyGroup encoding as
/// [`NiFloatData`]: count, then interpolation ONLY when count > 0, then
/// keys whose stride the interpolation dictates (1/5: t+xyz; 2:
/// t+xyz+fwd+bwd; 3: t+xyz+tbc). Flat floats so every type
/// round-trips bit-identically.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPosData {
    pub interpolation: u32,
    pub floats: Vec<f32>,
}

impl NiPosData {
    pub fn key_stride(interpolation: u32) -> Option<usize> {
        match interpolation {
            1 | 5 => Some(4),
            2 => Some(10),
            3 => Some(7),
            _ => None,
        }
    }

    /// `(time, xyz)` regardless of key type.
    pub fn keys(&self) -> impl Iterator<Item = (f32, [f32; 3])> + '_ {
        let stride = Self::key_stride(self.interpolation).unwrap_or(4);
        self.floats
            .chunks_exact(stride)
            .map(|k| (k[0], [k[1], k[2], k[3]]))
    }

    /// `(time, value, fwd, bwd)` — Hermite tangents when the keys are
    /// quadratic (type 2, the only type real path data uses), zero
    /// otherwise. Callers must branch on [`Self::is_quadratic`]: a Hermite
    /// with zero tangents eases, it does not lerp.
    pub fn hermite_keys(&self) -> Vec<(f32, [f32; 3], [f32; 3], [f32; 3])> {
        let stride = Self::key_stride(self.interpolation).unwrap_or(4);
        self.floats
            .chunks_exact(stride)
            .map(|k| {
                let v = [k[1], k[2], k[3]];
                if self.interpolation == 2 {
                    (k[0], v, [k[4], k[5], k[6]], [k[7], k[8], k[9]])
                } else {
                    (k[0], v, [0.0; 3], [0.0; 3])
                }
            })
            .collect()
    }

    pub fn is_quadratic(&self) -> bool {
        self.interpolation == 2
    }
}

impl TypedBlock for NiPosData {
    const TYPE_NAMES: &'static [&'static str] = &["NiPosData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        let stride = Self::key_stride(self.interpolation).unwrap_or(4);
        let mut n = (self.floats.len() / stride) as u32;
        s.u32(&mut n)?;
        if n > 0 {
            s.u32(&mut self.interpolation)?;
            let stride = Self::key_stride(self.interpolation).ok_or_else(|| {
                crate::error::Error::Corrupt {
                    offset: 0,
                    what: format!(
                        "NiPosData interpolation {} has no defined key layout",
                        self.interpolation
                    ),
                }
            })?;
            if s.reading() {
                self.floats.clear();
                self.floats.resize(n as usize * stride, 0.0);
            }
            s.f32s(&mut self.floats)?;
        } else if s.reading() {
            self.floats.clear();
        }
        Ok(())
    }
}

/// The `NiPSysModifierFloatCtlr` family — every emitter-parameter track.
///
/// nif.xml gives all of them ZERO fields of their own: the abstract
/// base's `Data` ref died at 10.1.0.103, so at 20.2.0.7 each is exactly
/// base + interpolator + modifier name. ONE struct with many type names,
/// because eleven identical structs is eleven places for the same
/// layout to drift apart. Which parameter a block animates is its TYPE
/// NAME, and the compiler picks the name when it reserves the block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysModifierFloatCtlr {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
    pub modifier_name: StringIdx,
}

impl TypedBlock for NiPSysModifierFloatCtlr {
    const TYPE_NAMES: &'static [&'static str] = &[
        // Speed is here TOO: the standalone `NiPSysEmitterSpeedCtlr`
        // struct predates this family view and stays for its callers,
        // but the decompiler goes through this one for all twelve.
        "NiPSysEmitterSpeedCtlr",
        "NiPSysGravityStrengthCtlr",
        "NiPSysEmitterDeclinationCtlr",
        "NiPSysEmitterDeclinationVarCtlr",
        "NiPSysEmitterPlanarAngleCtlr",
        "NiPSysEmitterPlanarAngleVarCtlr",
        "NiPSysEmitterInitialRadiusCtlr",
        "NiPSysEmitterLifeSpanCtlr",
        "NiPSysInitialRotSpeedCtlr",
        "NiPSysInitialRotSpeedVarCtlr",
        "NiPSysInitialRotAngleCtlr",
        "NiPSysInitialRotAngleVarCtlr",
    ];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)?;
        s.string_idx(&mut self.modifier_name)
    }
}

/// `BSPSysMultiTargetEmitterCtlr` — an emitter controller that spawns
/// into a master particle system's pool. NiPSysEmitterCtlr's exact
/// layout plus two fields; verified: Max Emitters is a bare u16 with NO
/// padding before the master Ptr.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSPSysMultiTargetEmitterCtlr {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
    pub modifier_name: StringIdx,
    pub visibility_interpolator: BlockRef,
    pub max_emitters: u16,
    /// Ptr<BSMasterParticleSystem> — points up/across the tree.
    pub master_particle_system: BlockRef,
}

impl TypedBlock for BSPSysMultiTargetEmitterCtlr {
    const TYPE_NAMES: &'static [&'static str] = &["BSPSysMultiTargetEmitterCtlr"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)?;
        s.string_idx(&mut self.modifier_name)?;
        s.block_ref(&mut self.visibility_interpolator)?;
        s.u16(&mut self.max_emitters)?;
        s.block_ptr(&mut self.master_particle_system)
    }
}

/// `NiVisController` — flips an object's hidden flag from a bool track.
/// At 20.2.0.7 it owns nothing: base + interpolator (the `Data` ref died
/// at 10.1.0.103).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiVisController {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
}

impl TypedBlock for NiVisController {
    const TYPE_NAMES: &'static [&'static str] = &["NiVisController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)
    }
}

/// `NiIntegerExtraData` and `BSXFlags` — one named uint. Identical
/// layouts: nif.xml derives BSXFlags straight from NiIntegerExtraData
/// with no fields of its own. BSXFlags is the most common untyped block
/// in the corpus, and its bits gate animation, collision and the
/// "complex" flag the game checks before building a Havok world.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiIntegerExtraData {
    pub name: StringIdx,
    pub value: u32,
}

impl TypedBlock for NiIntegerExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["NiIntegerExtraData", "BSXFlags"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        s.u32(&mut self.value)
    }
}

/// `NiStringExtraData` — one named header-table string. Carries prn
/// attachment points, "Bone" markers and the FX strings the CK reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiStringExtraData {
    pub name: StringIdx,
    pub value: StringIdx,
}

impl TypedBlock for NiStringExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["NiStringExtraData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        s.string_idx(&mut self.value)
    }
}

/// `NiStringsExtraData` — a counted list of INLINE strings, not header
/// indices (nif.xml types them `SizedString`, unlike every other string
/// at 20.2.0.7). Reading them as indices decodes a plausible-looking
/// block and loses every character.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiStringsExtraData {
    pub name: StringIdx,
    pub values: Vec<String>,
}

impl TypedBlock for NiStringsExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["NiStringsExtraData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        let mut n = self.values.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            super::check_len(n)?;
            self.values.clear();
            self.values.resize(n as usize, String::new());
        }
        for v in &mut self.values {
            s.sized_string(v)?;
        }
        Ok(())
    }
}

/// `BSInvMarker` — how the inventory menu poses an item. Three ushort
/// euler angles (the CK stores them in units of 1/1000 radian) and a
/// zoom.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSInvMarker {
    pub name: StringIdx,
    pub rotation: [u16; 3],
    pub zoom: f32,
}

impl TypedBlock for BSInvMarker {
    const TYPE_NAMES: &'static [&'static str] = &["BSInvMarker"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        for r in &mut self.rotation {
            s.u16(r)?;
        }
        s.f32(&mut self.zoom)
    }
}

/// `NiFloatExtraData` — one named float (name + value, 8 bytes). The
/// `hkVis:*` family mirrors Havok visibility channels: the skeleton
/// declares a same-named slot, and BShkVisibilityController hides the
/// node named by the SUFFIX on an exact == 0.0
/// (docs/knowledge/sweep-tail.md bucket 3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiFloatExtraData {
    pub name: StringIdx,
    pub value: f32,
}

impl TypedBlock for NiFloatExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["NiFloatExtraData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        s.f32(&mut self.value)
    }
}

/// `NiFloatExtraDataController` — animates a NAMED float extra data on
/// its target (34 bytes: controller base + interpolator + the name). The
/// NIF-side write is a runtime no-op (Update @ 0x140CD7A40 stores the
/// float and nothing reads it back); the effect the author meant lives
/// in the same-named Havok channel.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiFloatExtraDataController {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
    pub extra_data_name: StringIdx,
}

impl TypedBlock for NiFloatExtraDataController {
    const TYPE_NAMES: &'static [&'static str] = &["NiFloatExtraDataController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)?;
        s.string_idx(&mut self.extra_data_name)
    }
}

/// `NiTransformController` — a single-interpolator controller animating its
/// target's local transform. In vanilla data the interpolator is either an
/// NiTransformInterpolator (keyframes) or an NiPathInterpolator (critters
/// following a spline — the hawk circles through one of these free-running).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiTransformController {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
}

impl TypedBlock for NiTransformController {
    const TYPE_NAMES: &'static [&'static str] = &["NiTransformController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)
    }
}

/// `NiPathInterpolator` — position along a spline. Field order per nif.xml:
/// flags, bank dir, max bank angle, smoothing, follow axis, then refs to the
/// NiPosData path and the NiFloatData percent track. Byte-verified on the
/// salmon (24-byte payload, flags 0x63) and the fishbucket (flags 0x70).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPathInterpolator {
    /// PathFlags: bit 0 CVDataNeedsUpdate, 1 CurveTypeOpen, 2 AllowFlip,
    /// 3 Bank, 4 ConstantVelocity, 5 Follow, 6 Flip.
    pub flags: u16,
    /// -1 = negative, 1 = positive.
    pub bank_dir: i32,
    /// Radians.
    pub max_bank_angle: f32,
    pub smoothing: f32,
    /// 0/1/2 = X/Y/Z — which local axis follows the tangent. nif.xml says
    /// `short`; the raw u16 round-trips the same bytes and the value never
    /// exceeds 2 in real data.
    pub follow_axis: u16,
    pub path_data: BlockRef,
    pub percent_data: BlockRef,
}

impl TypedBlock for NiPathInterpolator {
    const TYPE_NAMES: &'static [&'static str] = &["NiPathInterpolator"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u16(&mut self.flags)?;
        s.i32(&mut self.bank_dir)?;
        s.f32(&mut self.max_bank_angle)?;
        s.f32(&mut self.smoothing)?;
        s.u16(&mut self.follow_axis)?;
        s.block_ref(&mut self.path_data)?;
        s.block_ref(&mut self.percent_data)
    }
}

/// `NiMultiTargetTransformController` — the transform multiplexer owned by
/// the controller manager.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiMultiTargetTransformController {
    pub base: ControllerBase,
    pub extra_targets: Vec<BlockRef>,
}

impl TypedBlock for NiMultiTargetTransformController {
    const TYPE_NAMES: &'static [&'static str] = &["NiMultiTargetTransformController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        let mut n = self.extra_targets.len() as u16;
        s.u16(&mut n)?;
        if s.reading() {
            self.extra_targets.clear();
            self.extra_targets.resize(n as usize, BlockRef::NONE);
        }
        for r in &mut self.extra_targets {
            s.block_ptr(r)?;
        }
        Ok(())
    }
}

/// `NiTextKeyExtraData` — the begin/loop/end markers of a sequence.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiTextKeyExtraData {
    pub name: StringIdx,
    pub keys: Vec<(f32, StringIdx)>,
}

impl TypedBlock for NiTextKeyExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["NiTextKeyExtraData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        let mut n = self.keys.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            self.keys.clear();
            self.keys.resize(n as usize, (0.0, StringIdx::NONE));
        }
        for key in &mut self.keys {
            s.f32(&mut key.0)?;
            s.string_idx(&mut key.1)?;
        }
        Ok(())
    }
}

/// One controlled block of a sequence: which interpolator drives which
/// controller on which object, all by header-table strings at 20.2.0.7.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControlledBlock {
    pub interpolator: BlockRef,
    pub controller: BlockRef,
    pub priority: u8,
    pub node_name: StringIdx,
    pub property_type: StringIdx,
    pub controller_type: StringIdx,
    pub controller_id: StringIdx,
    pub interpolator_id: StringIdx,
}

impl ControlledBlock {
    fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.block_ref(&mut self.interpolator)?;
        s.block_ptr(&mut self.controller)?;
        s.u8(&mut self.priority)?;
        s.string_idx(&mut self.node_name)?;
        s.string_idx(&mut self.property_type)?;
        s.string_idx(&mut self.controller_type)?;
        s.string_idx(&mut self.controller_id)?;
        s.string_idx(&mut self.interpolator_id)
    }
}

/// `NiControllerSequence` — one named animation clip.
#[derive(Debug, Clone, PartialEq)]
pub struct NiControllerSequence {
    pub name: StringIdx,
    pub array_grow_by: u32,
    pub controlled_blocks: Vec<ControlledBlock>,
    pub weight: f32,
    pub text_keys: BlockRef,
    /// 0 = loop, 1 = reverse, 2 = clamp.
    pub cycle_type: u32,
    pub frequency: f32,
    pub start_time: f32,
    pub stop_time: f32,
    pub manager: BlockRef,
    pub accum_root_name: StringIdx,
    pub anim_note_arrays: Vec<BlockRef>,
}

impl Default for NiControllerSequence {
    fn default() -> Self {
        NiControllerSequence {
            name: StringIdx::NONE,
            array_grow_by: 1,
            controlled_blocks: Vec::new(),
            weight: 1.0,
            text_keys: BlockRef::NONE,
            cycle_type: 2,
            frequency: 1.0,
            start_time: 0.0,
            stop_time: 1.0,
            manager: BlockRef::NONE,
            accum_root_name: StringIdx::NONE,
            anim_note_arrays: Vec::new(),
        }
    }
}

impl TypedBlock for NiControllerSequence {
    const TYPE_NAMES: &'static [&'static str] = &["NiControllerSequence"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        let mut n = self.controlled_blocks.len() as u32;
        s.u32(&mut n)?;
        s.u32(&mut self.array_grow_by)?;
        if s.reading() {
            super::check_len(n)?;
            self.controlled_blocks.clear();
            self.controlled_blocks
                .resize(n as usize, ControlledBlock::default());
        }
        for block in &mut self.controlled_blocks {
            block.sync(s)?;
        }
        s.f32(&mut self.weight)?;
        s.block_ref(&mut self.text_keys)?;
        s.u32(&mut self.cycle_type)?;
        s.f32(&mut self.frequency)?;
        s.f32(&mut self.start_time)?;
        s.f32(&mut self.stop_time)?;
        s.block_ptr(&mut self.manager)?;
        s.string_idx(&mut self.accum_root_name)?;
        // Anim note arrays exist for Bethesda streams above 28, which is
        // both Skyrim LE (83) and SE (100).
        let mut notes = self.anim_note_arrays.len() as u16;
        s.u16(&mut notes)?;
        if s.reading() {
            self.anim_note_arrays.clear();
            self.anim_note_arrays.resize(notes as usize, BlockRef::NONE);
        }
        for r in &mut self.anim_note_arrays {
            s.block_ref(r)?;
        }
        Ok(())
    }
}

/// `NiControllerManager` — owns the sequences of its scene branch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiControllerManager {
    pub base: ControllerBase,
    pub cumulative: bool,
    pub sequences: Vec<BlockRef>,
    pub object_palette: BlockRef,
}

impl TypedBlock for NiControllerManager {
    const TYPE_NAMES: &'static [&'static str] = &["NiControllerManager"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.bool8(&mut self.cumulative)?;
        s.ref_list(&mut self.sequences)?;
        s.block_ref(&mut self.object_palette)
    }
}

/// `NiDefaultAVObjectPalette` — the manager's name-to-object lookup.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiDefaultAVObjectPalette {
    pub scene: BlockRef,
    pub objects: Vec<(String, BlockRef)>,
}

impl TypedBlock for NiDefaultAVObjectPalette {
    const TYPE_NAMES: &'static [&'static str] = &["NiDefaultAVObjectPalette"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.block_ptr(&mut self.scene)?;
        let mut n = self.objects.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            self.objects.clear();
            self.objects
                .resize(n as usize, (String::new(), BlockRef::NONE));
        }
        for (name, obj) in &mut self.objects {
            s.sized_string(name)?;
            s.block_ptr(obj)?;
        }
        Ok(())
    }
}

/// `BSBehaviorGraphExtraData` ("BGED") — links a NIF to its Havok behavior
/// project. The path is a header-table string; `controls_base_skeleton` is
/// actor blending machinery statics leave at false.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSBehaviorGraphExtraData {
    pub name: StringIdx,
    pub behavior_graph_file: StringIdx,
    pub controls_base_skeleton: bool,
}

impl TypedBlock for BSBehaviorGraphExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["BSBehaviorGraphExtraData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        s.string_idx(&mut self.behavior_graph_file)?;
        s.bool8(&mut self.controls_base_skeleton)
    }
}

// ---------------------------------------------------------------------
// The extra-data and controller tail: the last flat structures the
// census still listed as untyped. None is editable through the graph;
// typing them closes the ref scanner's remaining holes (the camera's
// scene Ref, the bone-LOD node groups' Ptrs, the look-at chain) and
// lets the splice's field merger walk them instead of copying blind.

/// `BSDistantObjectLargeRefExtraData` — SSE's large-reference flag,
/// carried by object-LOD tiles (3,044 blocks, all in .bto files).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSDistantObjectLargeRefExtraData {
    pub name: StringIdx,
    /// Raw byte: the format does not normalize bools.
    pub large_ref: u8,
}

impl TypedBlock for BSDistantObjectLargeRefExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["BSDistantObjectLargeRefExtraData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        s.u8(&mut self.large_ref)
    }
}

/// `NiBooleanExtraData` — one raw byte.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiBooleanExtraData {
    pub name: StringIdx,
    pub value: u8,
}

impl TypedBlock for NiBooleanExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["NiBooleanExtraData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        s.u8(&mut self.value)
    }
}

/// `NiBinaryExtraData` — an opaque counted byte blob (Oblivion-era
/// tangent storage; mods still ship it).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiBinaryExtraData {
    pub name: StringIdx,
    pub data: Vec<u8>,
}

impl TypedBlock for NiBinaryExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["NiBinaryExtraData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        let mut n = self.data.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            crate::typed::check_len(n)?;
            self.data = vec![0; n as usize];
        }
        s.bytes(&mut self.data)
    }
}

/// One furniture sitting/leaning position — the Skyrim form of
/// `FurniturePosition` (offset, z heading, type, entry-point bits).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FurniturePosition {
    pub offset: [f32; 3],
    pub heading: f32,
    pub animation_type: u16,
    pub entry_points: u16,
}

/// `BSFurnitureMarkerNode` — where an actor sits on this mesh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSFurnitureMarkerNode {
    pub name: StringIdx,
    pub positions: Vec<FurniturePosition>,
}

impl TypedBlock for BSFurnitureMarkerNode {
    const TYPE_NAMES: &'static [&'static str] = &["BSFurnitureMarkerNode"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        let mut n = self.positions.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            crate::typed::check_len(n)?;
            self.positions = vec![FurniturePosition::default(); n as usize];
        }
        for p in self.positions.iter_mut() {
            s.f32s(&mut p.offset)?;
            s.f32(&mut p.heading)?;
            s.u16(&mut p.animation_type)?;
            s.u16(&mut p.entry_points)?;
        }
        Ok(())
    }
}

/// `BSLagBoneController` — trails a bone behind an actor (capes,
/// tails). Three floats on the controller base.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSLagBoneController {
    pub base: ControllerBase,
    pub linear_velocity: f32,
    pub linear_rotation: f32,
    pub maximum_distance: f32,
}

impl TypedBlock for BSLagBoneController {
    const TYPE_NAMES: &'static [&'static str] = &["BSLagBoneController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.f32(&mut self.linear_velocity)?;
        s.f32(&mut self.linear_rotation)?;
        s.f32(&mut self.maximum_distance)
    }
}

/// `BSFrustumFOVController` — a float interp controller with no
/// fields of its own; the float track drives the camera FOV.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSFrustumFOVController {
    pub base: ControllerBase,
    pub interpolator: BlockRef,
}

impl TypedBlock for BSFrustumFOVController {
    const TYPE_NAMES: &'static [&'static str] = &["BSFrustumFOVController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.interpolator)
    }
}

/// `BSBoneLODExtraData` — (distance, bone name) pairs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSBoneLODExtraData {
    pub name: StringIdx,
    pub entries: Vec<(u32, StringIdx)>,
}

impl TypedBlock for BSBoneLODExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["BSBoneLODExtraData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        let mut n = self.entries.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            crate::typed::check_len(n)?;
            self.entries = vec![(0, StringIdx::NONE); n as usize];
        }
        for (d, b) in self.entries.iter_mut() {
            s.u32(d)?;
            s.string_idx(b)?;
        }
        Ok(())
    }
}

/// `NiBSBoneLODController` — bone sets per LOD level. The group
/// count and the stored `Num Node Groups` are SEPARATE fields that
/// usually agree; both are kept so a file where they differ
/// round-trips.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiBSBoneLODController {
    pub base: ControllerBase,
    pub lod: u32,
    pub num_node_groups: u32,
    /// One Ptr set per LOD level.
    pub node_groups: Vec<Vec<BlockRef>>,
}

impl TypedBlock for NiBSBoneLODController {
    const TYPE_NAMES: &'static [&'static str] = &["NiBSBoneLODController"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.u32(&mut self.lod)?;
        let mut n = self.node_groups.len() as u32;
        s.u32(&mut n)?;
        s.u32(&mut self.num_node_groups)?;
        if s.reading() {
            crate::typed::check_len(n)?;
            self.node_groups = vec![Vec::new(); n as usize];
        }
        for g in self.node_groups.iter_mut() {
            let mut gn = g.len() as u32;
            s.u32(&mut gn)?;
            if s.reading() {
                crate::typed::check_len(gn)?;
                *g = vec![BlockRef::NONE; gn as usize];
            }
            for r in g.iter_mut() {
                s.block_ptr(r)?;
            }
        }
        Ok(())
    }
}

/// `NiLookAtInterpolator` — faces a target node; carries a rest
/// transform and three sub-interpolators. 13 corpus blocks, all in one
/// oversized mod file, so its real-file pin lives in the install-gated
/// suite rather than a shipped fixture.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiLookAtInterpolator {
    pub flags: u16,
    pub look_at: BlockRef,
    pub look_at_name: StringIdx,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: f32,
    pub interp_translation: BlockRef,
    pub interp_roll: BlockRef,
    pub interp_scale: BlockRef,
}

impl TypedBlock for NiLookAtInterpolator {
    const TYPE_NAMES: &'static [&'static str] = &["NiLookAtInterpolator"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u16(&mut self.flags)?;
        s.block_ptr(&mut self.look_at)?;
        s.string_idx(&mut self.look_at_name)?;
        s.f32s(&mut self.translation)?;
        s.f32s(&mut self.rotation)?;
        s.f32(&mut self.scale)?;
        s.block_ref(&mut self.interp_translation)?;
        s.block_ref(&mut self.interp_roll)?;
        s.block_ref(&mut self.interp_scale)
    }
}

/// One decal placement block: paired point and normal lists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DecalVectorBlock {
    pub points: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}

/// `BSDecalPlacementVectorExtraData` — inherits NiFloatExtraData, so
/// a float rides between the name and the vector blocks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSDecalPlacementVectorExtraData {
    pub name: StringIdx,
    pub float_data: f32,
    pub blocks: Vec<DecalVectorBlock>,
}

impl TypedBlock for BSDecalPlacementVectorExtraData {
    const TYPE_NAMES: &'static [&'static str] = &["BSDecalPlacementVectorExtraData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        s.f32(&mut self.float_data)?;
        let mut n = self.blocks.len() as u16;
        s.u16(&mut n)?;
        if s.reading() {
            self.blocks = vec![DecalVectorBlock::default(); n as usize];
        }
        for b in self.blocks.iter_mut() {
            let mut vn = b.points.len() as u16;
            s.u16(&mut vn)?;
            if s.reading() {
                b.points = vec![[0.0; 3]; vn as usize];
                b.normals = vec![[0.0; 3]; vn as usize];
            }
            for v in b.points.iter_mut() {
                s.f32s(v)?;
            }
            for v in b.normals.iter_mut() {
                s.f32s(v)?;
            }
        }
        Ok(())
    }
}

/// `BSBound` — a skeleton's collision bounding box: centre and
/// half-dimensions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSBound {
    pub name: StringIdx,
    pub center: [f32; 3],
    pub dimensions: [f32; 3],
}

impl TypedBlock for BSBound {
    const TYPE_NAMES: &'static [&'static str] = &["BSBound"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.string_idx(&mut self.name)?;
        s.f32s(&mut self.center)?;
        s.f32s(&mut self.dimensions)
    }
}
