//! Particle system, particle data, emitters and modifiers.

use super::{AvObject, BlockRef, Bound, Ctx, Nio, StringIdx, TypedBlock};
use crate::error::Result;

/// Canonical `NiPSysModifierOrder` values (nif.xml enum).
pub mod order {
    pub const KILL_OLD_PARTICLES: u32 = 0;
    pub const BSLOD: u32 = 1;
    pub const EMITTER: u32 = 1000;
    pub const SPAWN: u32 = 2000;
    pub const GENERAL: u32 = 3000;
    pub const FORCE: u32 = 4000;
    pub const COLLIDER: u32 = 5000;
    pub const POS_UPDATE: u32 = 6000;
    pub const POSTPOS_UPDATE: u32 = 6500;
    pub const BOUND_UPDATE: u32 = 7000;
    /// Skyrim's `BSPSysStripUpdateModifier` slot (FO3 used 2500).
    pub const SK_STRIP_UPDATE: u32 = 8000;
}

/// `NiParticleSystem` / `BSStripParticleSystem`.
///
/// SE (bs100) carries bound + shader/alpha refs + vertex desc inline; LE
/// (bs83) instead references data early and stores a material list.
#[derive(Debug, Clone, PartialEq)]
pub struct NiParticleSystem {
    pub av: AvObject,
    // --- SE-only ---
    pub bound: Bound,
    pub skin: BlockRef,
    /// 8-byte BSVertexDesc (SE only).
    pub vertex_desc: u64,
    // --- LE-only ---
    pub materials: Vec<(StringIdx, i32)>,
    pub active_material: i32,
    pub default_mat_needs_update: bool,
    // --- both (different on-disk positions) ---
    pub shader_property: BlockRef,
    pub alpha_property: BlockRef,
    pub data: BlockRef,
    pub far_begin: u16,
    pub far_end: u16,
    pub near_begin: u16,
    pub near_end: u16,
    pub world_space: bool,
    pub modifiers: Vec<BlockRef>,
}

impl Default for NiParticleSystem {
    fn default() -> Self {
        NiParticleSystem {
            av: AvObject::default(),
            bound: Bound::default(),
            skin: BlockRef::NONE,
            vertex_desc: 0,
            materials: Vec::new(),
            active_material: 0,
            default_mat_needs_update: false,
            shader_property: BlockRef::NONE,
            alpha_property: BlockRef::NONE,
            data: BlockRef::NONE,
            far_begin: 0,
            far_end: 0,
            near_begin: 0,
            near_end: 0,
            world_space: true,
            modifiers: Vec::new(),
        }
    }
}

impl TypedBlock for NiParticleSystem {
    const TYPE_NAMES: &'static [&'static str] = &["NiParticleSystem", "BSStripParticleSystem"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.av.sync(s, ctx)?;
        if ctx.sse {
            self.bound.sync(s)?;
            s.block_ref(&mut self.skin)?;
            s.block_ref(&mut self.shader_property)?;
            s.block_ref(&mut self.alpha_property)?;
            s.u64(&mut self.vertex_desc)?;
        } else {
            s.block_ref(&mut self.data)?;
            s.block_ref(&mut self.skin)?;
            let mut n = self.materials.len() as u32;
            s.u32(&mut n)?;
            if s.reading() {
                self.materials = vec![(StringIdx::NONE, 0); n as usize];
            }
            for (name, _) in self.materials.iter_mut() {
                s.string_idx(name)?;
            }
            for (_, extra) in self.materials.iter_mut() {
                s.i32(extra)?;
            }
            s.i32(&mut self.active_material)?;
            s.bool8(&mut self.default_mat_needs_update)?;
            s.block_ref(&mut self.shader_property)?;
            s.block_ref(&mut self.alpha_property)?;
        }
        s.u16(&mut self.far_begin)?;
        s.u16(&mut self.far_end)?;
        s.u16(&mut self.near_begin)?;
        s.u16(&mut self.near_end)?;
        if ctx.sse {
            s.block_ref(&mut self.data)?;
        }
        s.bool8(&mut self.world_space)?;
        s.ref_list(&mut self.modifiers)
    }
}

/// `NiPSysData` for Bethesda 20.2.0.7: per-particle arrays are elided, only
/// the flags/bound/subtexture metadata remain.
#[derive(Debug, Clone, PartialEq)]
pub struct NiPSysData {
    // --- NiGeometryData ---
    pub group_id: u32,
    pub num_vertices: u16,
    pub keep_flags: u8,
    pub compress_flags: u8,
    pub has_vertices: bool,
    /// BSVectorFlags (UV/tangent presence bits).
    pub bs_vector_flags: u16,
    pub material_crc: u32,
    pub has_normals: bool,
    pub bound: Bound,
    pub has_vertex_colors: bool,
    pub consistency_flags: u16,
    pub additional_data: BlockRef,
    // --- NiParticlesData ---
    pub has_radii: bool,
    pub num_active: u16,
    pub has_sizes: bool,
    pub has_rotations: bool,
    pub has_rotation_angles: bool,
    pub has_rotation_axes: bool,
    pub has_texture_indices: bool,
    pub subtexture_offsets: Vec<[f32; 4]>,
    pub aspect_ratio: f32,
    pub aspect_flags: u16,
    pub speed_to_aspect_aspect2: f32,
    pub speed_to_aspect_speed1: f32,
    pub speed_to_aspect_speed2: f32,
    // --- NiPSysData ---
    pub has_rotation_speeds: bool,
}

impl Default for NiPSysData {
    fn default() -> Self {
        NiPSysData {
            group_id: 0,
            num_vertices: 0,
            keep_flags: 0,
            compress_flags: 0,
            has_vertices: false,
            bs_vector_flags: 0,
            material_crc: 0,
            has_normals: false,
            bound: Bound::default(),
            has_vertex_colors: false,
            consistency_flags: 0,
            additional_data: BlockRef::NONE,
            has_radii: false,
            num_active: 0,
            has_sizes: false,
            has_rotations: false,
            has_rotation_angles: false,
            has_rotation_axes: false,
            has_texture_indices: false,
            subtexture_offsets: Vec::new(),
            aspect_ratio: 1.0,
            aspect_flags: 0,
            speed_to_aspect_aspect2: 0.0,
            speed_to_aspect_speed1: 0.0,
            speed_to_aspect_speed2: 0.0,
            has_rotation_speeds: false,
        }
    }
}

impl TypedBlock for NiPSysData {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u32(&mut self.group_id)?;
        s.u16(&mut self.num_vertices)?;
        s.u8(&mut self.keep_flags)?;
        s.u8(&mut self.compress_flags)?;
        s.bool8(&mut self.has_vertices)?;
        s.u16(&mut self.bs_vector_flags)?;
        s.u32(&mut self.material_crc)?;
        s.bool8(&mut self.has_normals)?;
        self.bound.sync(s)?;
        s.bool8(&mut self.has_vertex_colors)?;
        s.u16(&mut self.consistency_flags)?;
        s.block_ref(&mut self.additional_data)?;
        s.bool8(&mut self.has_radii)?;
        s.u16(&mut self.num_active)?;
        s.bool8(&mut self.has_sizes)?;
        s.bool8(&mut self.has_rotations)?;
        s.bool8(&mut self.has_rotation_angles)?;
        s.bool8(&mut self.has_rotation_axes)?;
        s.bool8(&mut self.has_texture_indices)?;
        s.vec4_list(&mut self.subtexture_offsets)?;
        s.f32(&mut self.aspect_ratio)?;
        s.u16(&mut self.aspect_flags)?;
        s.f32(&mut self.speed_to_aspect_aspect2)?;
        s.f32(&mut self.speed_to_aspect_speed1)?;
        s.f32(&mut self.speed_to_aspect_speed2)?;
        s.bool8(&mut self.has_rotation_speeds)
    }
}

/// `BSPSysStripUpdateModifier`: advances the strip path points; part of the
/// trail apparatus on every `BSStripParticleSystem`.
#[derive(Debug, Clone, PartialEq)]
pub struct BSPSysStripUpdateModifier {
    pub base: ModifierBase,
    pub update_delta_time: f32,
}

impl Default for BSPSysStripUpdateModifier {
    fn default() -> Self {
        BSPSysStripUpdateModifier {
            base: ModifierBase::default(),
            update_delta_time: 0.033333,
        }
    }
}

impl TypedBlock for BSPSysStripUpdateModifier {
    const TYPE_NAMES: &'static [&'static str] = &["BSPSysStripUpdateModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.f32(&mut self.update_delta_time)
    }
}

/// `BSStripPSysData`: `NiPSysData` plus the trail-strip tail. Strip systems
/// (`BSStripParticleSystem`) draw each particle as a ribbon of up to
/// `max_point_count` path points instead of a billboard.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BSStripPSysData {
    pub data: NiPSysData,
    pub max_point_count: u16,
    pub start_cap_size: f32,
    pub end_cap_size: f32,
    pub do_z_prepass: bool,
}

impl TypedBlock for BSStripPSysData {
    const TYPE_NAMES: &'static [&'static str] = &["BSStripPSysData"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.data.sync(s, ctx)?;
        s.u16(&mut self.max_point_count)?;
        s.f32(&mut self.start_cap_size)?;
        s.f32(&mut self.end_cap_size)?;
        s.bool8(&mut self.do_z_prepass)
    }
}

/// `NiPSysModifier` base: name, order, back-pointer to the system, active.
#[derive(Debug, Clone, PartialEq)]
pub struct ModifierBase {
    pub name: StringIdx,
    pub order: u32,
    pub target: BlockRef,
    pub active: bool,
}

impl Default for ModifierBase {
    fn default() -> Self {
        ModifierBase {
            name: StringIdx::NONE,
            order: order::GENERAL,
            target: BlockRef::NONE,
            active: true,
        }
    }
}

impl ModifierBase {
    pub fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.string_idx(&mut self.name)?;
        s.u32(&mut self.order)?;
        s.block_ptr(&mut self.target)?;
        s.bool8(&mut self.active)
    }
}

/// `NiPSysEmitter` shared fields.
#[derive(Debug, Clone, PartialEq)]
pub struct EmitterBase {
    pub base: ModifierBase,
    pub speed: f32,
    pub speed_variation: f32,
    pub declination: f32,
    pub declination_variation: f32,
    pub planar_angle: f32,
    pub planar_angle_variation: f32,
    pub initial_color: [f32; 4],
    pub initial_radius: f32,
    pub radius_variation: f32,
    pub life_span: f32,
    pub life_span_variation: f32,
    /// `NiPSysVolumeEmitter`: node the volume is anchored to.
    pub emitter_object: BlockRef,
}

impl Default for EmitterBase {
    fn default() -> Self {
        EmitterBase {
            base: ModifierBase {
                order: order::EMITTER,
                ..ModifierBase::default()
            },
            speed: 0.0,
            speed_variation: 0.0,
            declination: 0.0,
            declination_variation: 0.0,
            planar_angle: 0.0,
            planar_angle_variation: 0.0,
            initial_color: [1.0, 1.0, 1.0, 1.0],
            initial_radius: 1.0,
            radius_variation: 0.0,
            life_span: 1.0,
            life_span_variation: 0.0,
            emitter_object: BlockRef::NONE,
        }
    }
}

impl EmitterBase {
    /// The `NiPSysEmitter` fields every emitter shares. The mesh emitter
    /// stops here — `emitter_object` belongs to `NiPSysVolumeEmitter`, one
    /// rung further down, which only the box/sphere/cylinder shapes inherit.
    pub fn sync_core(&mut self, s: &mut dyn Nio) -> Result<()> {
        self.base.sync(s)?;
        s.f32(&mut self.speed)?;
        s.f32(&mut self.speed_variation)?;
        s.f32(&mut self.declination)?;
        s.f32(&mut self.declination_variation)?;
        s.f32(&mut self.planar_angle)?;
        s.f32(&mut self.planar_angle_variation)?;
        s.f32s(&mut self.initial_color)?;
        s.f32(&mut self.initial_radius)?;
        s.f32(&mut self.radius_variation)?;
        s.f32(&mut self.life_span)?;
        s.f32(&mut self.life_span_variation)
    }

    pub fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        self.sync_core(s)?;
        s.block_ptr(&mut self.emitter_object)
    }
}

/// `NiPSysMeshEmitter` — emits from the surface of referenced meshes. What
/// every animated Zodiac weapon and the vanilla campfires use, which is why
/// "0 particles" was the preview for both.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysMeshEmitter {
    pub emitter: EmitterBase,
    /// `NiAVObject` pointers to the source shapes.
    pub emitter_meshes: Vec<BlockRef>,
    /// VelocityType: 0 = mesh normals, 1 = random, 2 = emission axis.
    pub initial_velocity_type: u32,
    /// EmitFrom: 0 vertices, 1 face centres, 2 edge centres, 3 face
    /// surface, 4 edge surface.
    pub emission_type: u32,
    pub emission_axis: [f32; 3],
}

impl TypedBlock for NiPSysMeshEmitter {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysMeshEmitter"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.emitter.sync_core(s)?;
        s.ptr_list(&mut self.emitter_meshes)?;
        s.u32(&mut self.initial_velocity_type)?;
        s.u32(&mut self.emission_type)?;
        s.f32s(&mut self.emission_axis)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysBoxEmitter {
    pub emitter: EmitterBase,
    pub width: f32,
    pub height: f32,
    pub depth: f32,
}

impl TypedBlock for NiPSysBoxEmitter {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysBoxEmitter"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.emitter.sync(s)?;
        s.f32(&mut self.width)?;
        s.f32(&mut self.height)?;
        s.f32(&mut self.depth)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysSphereEmitter {
    pub emitter: EmitterBase,
    pub radius: f32,
}

impl TypedBlock for NiPSysSphereEmitter {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysSphereEmitter"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.emitter.sync(s)?;
        s.f32(&mut self.radius)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysCylinderEmitter {
    pub emitter: EmitterBase,
    pub radius: f32,
    pub height: f32,
}

impl TypedBlock for NiPSysCylinderEmitter {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysCylinderEmitter"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.emitter.sync(s)?;
        s.f32(&mut self.radius)?;
        s.f32(&mut self.height)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysAgeDeathModifier {
    pub base: ModifierBase,
    pub spawn_on_death: bool,
    pub spawn_modifier: BlockRef,
}

impl TypedBlock for NiPSysAgeDeathModifier {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysAgeDeathModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.bool8(&mut self.spawn_on_death)?;
        s.block_ref(&mut self.spawn_modifier)
    }
}

/// `BSPSysSubTexModifier` — atlas frame animation over particle life.
/// Frame values are floats indexing the `Subtexture Offsets` quads in
/// `NiPSysData`; the runtime formula lives in
/// `docs/knowledge/particle-runtime.md` (RE'd from 1.5.97 @ 0x140D64BF0).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSPSysSubTexModifier {
    pub base: ModifierBase,
    pub start_frame: f32,
    pub start_frame_fudge: f32,
    pub end_frame: f32,
    pub loop_start_frame: f32,
    pub loop_start_frame_fudge: f32,
    pub frame_count: f32,
    pub frame_count_fudge: f32,
}

impl TypedBlock for BSPSysSubTexModifier {
    const TYPE_NAMES: &'static [&'static str] = &["BSPSysSubTexModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.f32(&mut self.start_frame)?;
        s.f32(&mut self.start_frame_fudge)?;
        s.f32(&mut self.end_frame)?;
        s.f32(&mut self.loop_start_frame)?;
        s.f32(&mut self.loop_start_frame_fudge)?;
        s.f32(&mut self.frame_count)?;
        s.f32(&mut self.frame_count_fudge)
    }
}

/// `NiPSysSpawnModifier` — referenced by `NiPSysAgeDeathModifier` (death)
/// and colliders (impact). NOTE: in SkyrimSE 1.5.97 the engine's spawn
/// implementation is a stub — the fields load but no particle is ever
/// spawned (RE @ 0x140D20B90). Serialized faithfully; simulated as inert.
#[derive(Debug, Clone, PartialEq)]
pub struct NiPSysSpawnModifier {
    pub base: ModifierBase,
    pub num_spawn_generations: u16,
    pub percentage_spawned: f32,
    pub min_num_to_spawn: u16,
    pub max_num_to_spawn: u16,
    pub spawn_speed_variation: f32,
    pub spawn_dir_variation: f32,
    pub life_span: f32,
    pub life_span_variation: f32,
}

impl Default for NiPSysSpawnModifier {
    fn default() -> Self {
        NiPSysSpawnModifier {
            base: ModifierBase {
                order: order::SPAWN,
                ..ModifierBase::default()
            },
            num_spawn_generations: 0,
            percentage_spawned: 1.0,
            min_num_to_spawn: 1,
            max_num_to_spawn: 1,
            spawn_speed_variation: 0.0,
            spawn_dir_variation: 0.0,
            life_span: 0.0,
            life_span_variation: 0.0,
        }
    }
}

impl TypedBlock for NiPSysSpawnModifier {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysSpawnModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.u16(&mut self.num_spawn_generations)?;
        s.f32(&mut self.percentage_spawned)?;
        s.u16(&mut self.min_num_to_spawn)?;
        s.u16(&mut self.max_num_to_spawn)?;
        s.f32(&mut self.spawn_speed_variation)?;
        s.f32(&mut self.spawn_dir_variation)?;
        s.f32(&mut self.life_span)?;
        s.f32(&mut self.life_span_variation)
    }
}


/// `BSPSysInheritVelocityModifier` — at spawn, a particle takes a share
/// of the inherit object's own motion (the storm atronach's cloak swirls
/// with the atronach). Layout verified against nif.xml: Ptr + 3 floats,
/// no version gates, identical bs83/bs100.
#[derive(Debug, Clone, PartialEq)]
pub struct BSPSysInheritVelocityModifier {
    pub base: ModifierBase,
    /// The scene object whose velocity is sampled (Ptr, not Ref — it
    /// points UP the tree and must not be walked as a child).
    pub inherit_object: BlockRef,
    /// Percentage 0-100 per spawn.
    pub chance_to_inherit: f32,
    pub velocity_multiplier: f32,
    pub velocity_variation: f32,
}

impl Default for BSPSysInheritVelocityModifier {
    fn default() -> Self {
        BSPSysInheritVelocityModifier {
            base: ModifierBase {
                order: order::EMITTER,
                ..ModifierBase::default()
            },
            inherit_object: BlockRef::NONE,
            chance_to_inherit: 100.0,
            velocity_multiplier: 0.5,
            velocity_variation: 0.0,
        }
    }
}

impl TypedBlock for BSPSysInheritVelocityModifier {
    const TYPE_NAMES: &'static [&'static str] = &["BSPSysInheritVelocityModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ptr(&mut self.inherit_object)?;
        s.f32(&mut self.chance_to_inherit)?;
        s.f32(&mut self.velocity_multiplier)?;
        s.f32(&mut self.velocity_variation)
    }
}

/// `BSPSysRecycleBoundModifier` — particles leaving a box are recycled.
/// The box is centred on the bound object (or the psys) plus an offset,
/// with half-extents per axis.
#[derive(Debug, Clone, PartialEq)]
pub struct BSPSysRecycleBoundModifier {
    pub base: ModifierBase,
    pub bound_offset: [f32; 3],
    pub bound_extent: [f32; 3],
    /// Ptr, same up-the-tree rule as the inherit object.
    pub bound_object: BlockRef,
}

impl Default for BSPSysRecycleBoundModifier {
    fn default() -> Self {
        BSPSysRecycleBoundModifier {
            base: ModifierBase {
                order: order::POSTPOS_UPDATE,
                ..ModifierBase::default()
            },
            bound_offset: [0.0; 3],
            bound_extent: [0.0; 3],
            bound_object: BlockRef::NONE,
        }
    }
}

impl TypedBlock for BSPSysRecycleBoundModifier {
    const TYPE_NAMES: &'static [&'static str] = &["BSPSysRecycleBoundModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.f32s(&mut self.bound_offset)?;
        s.f32s(&mut self.bound_extent)?;
        s.block_ptr(&mut self.bound_object)
    }
}

/// `BSWindModifier` — one float of wind strength; the direction comes
/// from the weather at runtime, so the block stores none.
#[derive(Debug, Clone, PartialEq)]
pub struct BSWindModifier {
    pub base: ModifierBase,
    pub strength: f32,
}

impl Default for BSWindModifier {
    fn default() -> Self {
        BSWindModifier {
            base: ModifierBase {
                order: order::FORCE,
                ..ModifierBase::default()
            },
            strength: 0.25,
        }
    }
}

impl TypedBlock for BSWindModifier {
    const TYPE_NAMES: &'static [&'static str] = &["BSWindModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.f32(&mut self.strength)
    }
}

/// `BSPSysLODModifier` — scales emission and particle size down with
/// camera distance. The preview runs at authoring distance, so the sim
/// leaves it inert; the block edits and round-trips.
#[derive(Debug, Clone, PartialEq)]
pub struct BSPSysLODModifier {
    pub base: ModifierBase,
    pub lod_begin_distance: f32,
    pub lod_end_distance: f32,
    pub end_emit_scale: f32,
    pub end_size: f32,
}

impl Default for BSPSysLODModifier {
    fn default() -> Self {
        BSPSysLODModifier {
            base: ModifierBase::default(),
            lod_begin_distance: 0.1,
            lod_end_distance: 0.7,
            end_emit_scale: 0.2,
            end_size: 1.0,
        }
    }
}

impl TypedBlock for BSPSysLODModifier {
    const TYPE_NAMES: &'static [&'static str] = &["BSPSysLODModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.f32(&mut self.lod_begin_distance)?;
        s.f32(&mut self.lod_end_distance)?;
        s.f32(&mut self.end_emit_scale)?;
        s.f32(&mut self.end_size)
    }
}

/// `NiPSysColliderManager`: holds the head of a collider chain. The
/// manager is an ordinary modifier (order 5000, before position update)
/// and the game skips it entirely when its Active byte is 0 — the Markarth
/// warhammer ships all three managers authored inactive.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysColliderManager {
    pub base: ModifierBase,
    pub collider: BlockRef,
}

impl TypedBlock for NiPSysColliderManager {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysColliderManager"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ref(&mut self.collider)
    }
}

/// `NiPSysCollider` shared fields, in exact on-disk order.
#[derive(Debug, Clone, PartialEq)]
pub struct ColliderBase {
    pub bounce: f32,
    /// Spawns nothing in SkyrimSE 1.5.97 — vfunc 42 is the same stub as
    /// spawn-on-death.
    pub spawn_on_collide: bool,
    pub die_on_collide: bool,
    pub spawn_modifier: BlockRef,
    pub parent: BlockRef,
    pub next_collider: BlockRef,
    pub collider_object: BlockRef,
}

impl Default for ColliderBase {
    fn default() -> Self {
        ColliderBase {
            bounce: 1.0,
            spawn_on_collide: false,
            die_on_collide: false,
            spawn_modifier: BlockRef::NONE,
            parent: BlockRef::NONE,
            next_collider: BlockRef::NONE,
            collider_object: BlockRef::NONE,
        }
    }
}

impl ColliderBase {
    pub fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.f32(&mut self.bounce)?;
        s.bool8(&mut self.spawn_on_collide)?;
        s.bool8(&mut self.die_on_collide)?;
        s.block_ref(&mut self.spawn_modifier)?;
        s.block_ptr(&mut self.parent)?;
        s.block_ref(&mut self.next_collider)?;
        s.block_ptr(&mut self.collider_object)
    }
}

/// `NiPSysPlanarCollider`: a bounded rectangle; the plane normal is
/// `cross(x_axis, y_axis)`, axes in the collider object's local space.
#[derive(Debug, Clone, PartialEq)]
pub struct NiPSysPlanarCollider {
    pub collider: ColliderBase,
    pub width: f32,
    pub height: f32,
    pub x_axis: [f32; 3],
    pub y_axis: [f32; 3],
}

impl Default for NiPSysPlanarCollider {
    fn default() -> Self {
        NiPSysPlanarCollider {
            collider: ColliderBase::default(),
            width: 10.0,
            height: 10.0,
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
        }
    }
}

impl TypedBlock for NiPSysPlanarCollider {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysPlanarCollider"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.collider.sync(s)?;
        s.f32(&mut self.width)?;
        s.f32(&mut self.height)?;
        s.f32s(&mut self.x_axis)?;
        s.f32s(&mut self.y_axis)
    }
}

/// `NiPSysSphericalCollider`: particles hit the sphere from BOTH sides —
/// an inside hit reflects at the exit point, which is how a sphere
/// contains an effect.
#[derive(Debug, Clone, PartialEq)]
pub struct NiPSysSphericalCollider {
    pub collider: ColliderBase,
    pub radius: f32,
}

impl Default for NiPSysSphericalCollider {
    fn default() -> Self {
        NiPSysSphericalCollider {
            collider: ColliderBase::default(),
            radius: 10.0,
        }
    }
}

impl TypedBlock for NiPSysSphericalCollider {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysSphericalCollider"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.collider.sync(s)?;
        s.f32(&mut self.radius)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysBoundUpdateModifier {
    pub base: ModifierBase,
    pub update_skip: u16,
}

impl TypedBlock for NiPSysBoundUpdateModifier {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysBoundUpdateModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.u16(&mut self.update_skip)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiPSysPositionModifier {
    pub base: ModifierBase,
}

impl TypedBlock for NiPSysPositionModifier {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysPositionModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NiPSysGravityModifier {
    pub base: ModifierBase,
    pub gravity_object: BlockRef,
    pub gravity_axis: [f32; 3],
    pub decay: f32,
    pub strength: f32,
    /// 0 = planar, 1 = spherical.
    pub force_type: u32,
    pub turbulence: f32,
    pub turbulence_scale: f32,
    pub world_aligned: bool,
}

impl Default for NiPSysGravityModifier {
    fn default() -> Self {
        NiPSysGravityModifier {
            base: ModifierBase {
                order: order::FORCE,
                ..ModifierBase::default()
            },
            gravity_object: BlockRef::NONE,
            gravity_axis: [1.0, 0.0, 0.0],
            decay: 0.0,
            strength: 1.0,
            force_type: 0,
            turbulence: 0.0,
            turbulence_scale: 1.0,
            world_aligned: false,
        }
    }
}

impl TypedBlock for NiPSysGravityModifier {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysGravityModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ptr(&mut self.gravity_object)?;
        s.f32s(&mut self.gravity_axis)?;
        s.f32(&mut self.decay)?;
        s.f32(&mut self.strength)?;
        s.u32(&mut self.force_type)?;
        s.f32(&mut self.turbulence)?;
        s.f32(&mut self.turbulence_scale)?;
        s.bool8(&mut self.world_aligned)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NiPSysRotationModifier {
    pub base: ModifierBase,
    pub rotation_speed: f32,
    pub rotation_speed_variation: f32,
    pub rotation_angle: f32,
    pub rotation_angle_variation: f32,
    pub random_rot_speed_sign: bool,
    pub random_axis: bool,
    pub axis: [f32; 3],
}

impl Default for NiPSysRotationModifier {
    fn default() -> Self {
        NiPSysRotationModifier {
            base: ModifierBase::default(),
            rotation_speed: 0.0,
            rotation_speed_variation: 0.0,
            rotation_angle: 0.0,
            rotation_angle_variation: 0.0,
            random_rot_speed_sign: false,
            random_axis: true,
            axis: [1.0, 0.0, 0.0],
        }
    }
}

impl TypedBlock for NiPSysRotationModifier {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysRotationModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.f32(&mut self.rotation_speed)?;
        s.f32(&mut self.rotation_speed_variation)?;
        s.f32(&mut self.rotation_angle)?;
        s.f32(&mut self.rotation_angle_variation)?;
        s.bool8(&mut self.random_rot_speed_sign)?;
        s.bool8(&mut self.random_axis)?;
        s.f32s(&mut self.axis)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NiPSysDragModifier {
    pub base: ModifierBase,
    pub drag_object: BlockRef,
    pub drag_axis: [f32; 3],
    pub percentage: f32,
    pub range: f32,
    pub range_falloff: f32,
}

impl Default for NiPSysDragModifier {
    fn default() -> Self {
        NiPSysDragModifier {
            base: ModifierBase {
                order: order::FORCE,
                ..ModifierBase::default()
            },
            drag_object: BlockRef::NONE,
            drag_axis: [1.0, 0.0, 0.0],
            percentage: 0.05,
            range: f32::MAX,
            range_falloff: f32::MAX,
        }
    }
}

impl TypedBlock for NiPSysDragModifier {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysDragModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ptr(&mut self.drag_object)?;
        s.f32s(&mut self.drag_axis)?;
        s.f32(&mut self.percentage)?;
        s.f32(&mut self.range)?;
        s.f32(&mut self.range_falloff)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NiPSysBombModifier {
    pub base: ModifierBase,
    pub bomb_object: BlockRef,
    pub bomb_axis: [f32; 3],
    pub decay: f32,
    pub delta_v: f32,
    /// 0 = none, 1 = linear, 2 = exponential.
    pub decay_type: u32,
    /// 0 = spherical, 1 = cylindrical, 2 = planar.
    pub symmetry_type: u32,
}

impl Default for NiPSysBombModifier {
    fn default() -> Self {
        NiPSysBombModifier {
            base: ModifierBase {
                order: order::FORCE,
                ..ModifierBase::default()
            },
            bomb_object: BlockRef::NONE,
            bomb_axis: [1.0, 0.0, 0.0],
            decay: 0.0,
            delta_v: 0.0,
            decay_type: 0,
            symmetry_type: 0,
        }
    }
}

impl TypedBlock for NiPSysBombModifier {
    const TYPE_NAMES: &'static [&'static str] = &["NiPSysBombModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.block_ptr(&mut self.bomb_object)?;
        s.f32s(&mut self.bomb_axis)?;
        s.f32(&mut self.decay)?;
        s.f32(&mut self.delta_v)?;
        s.u32(&mut self.decay_type)?;
        s.u32(&mut self.symmetry_type)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BSPSysScaleModifier {
    pub base: ModifierBase,
    /// Scale curve sampled uniformly over particle lifetime.
    pub scales: Vec<f32>,
}

impl TypedBlock for BSPSysScaleModifier {
    const TYPE_NAMES: &'static [&'static str] = &["BSPSysScaleModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.f32_list(&mut self.scales)
    }
}

/// Three-color gradient over particle lifetime with fade in/out.
#[derive(Debug, Clone, PartialEq)]
pub struct BSPSysSimpleColorModifier {
    pub base: ModifierBase,
    pub fade_in_percent: f32,
    pub fade_out_percent: f32,
    pub color1_end_percent: f32,
    pub color2_start_percent: f32,
    pub color2_end_percent: f32,
    pub color3_start_percent: f32,
    pub colors: [[f32; 4]; 3],
}

impl Default for BSPSysSimpleColorModifier {
    fn default() -> Self {
        BSPSysSimpleColorModifier {
            base: ModifierBase::default(),
            fade_in_percent: 0.1,
            fade_out_percent: 0.9,
            color1_end_percent: 0.2,
            color2_start_percent: 0.3,
            color2_end_percent: 0.7,
            color3_start_percent: 0.8,
            colors: [[1.0; 4]; 3],
        }
    }
}

impl TypedBlock for BSPSysSimpleColorModifier {
    const TYPE_NAMES: &'static [&'static str] = &["BSPSysSimpleColorModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.f32(&mut self.fade_in_percent)?;
        s.f32(&mut self.fade_out_percent)?;
        s.f32(&mut self.color1_end_percent)?;
        s.f32(&mut self.color2_start_percent)?;
        s.f32(&mut self.color2_end_percent)?;
        s.f32(&mut self.color3_start_percent)?;
        for c in self.colors.iter_mut() {
            s.f32s(c)?;
        }
        Ok(())
    }
}

/// `NiMeshParticleSystem` — layout-identical to `NiParticleSystem`, but
/// deliberately its OWN view: the decompiler keys "is this an editable
/// system" on `NiParticleSystem::TYPE_NAMES`, and adding the name there
/// made it claim mesh-particle systems it cannot faithfully recompile.
/// Measured: the null-edit count fell 67,578 -> 67,576 (the two
/// fxdragoncrashfurrow carriers) until this split.
#[derive(Debug, Clone, Default)]
pub struct NiMeshParticleSystem {
    pub system: NiParticleSystem,
}

impl TypedBlock for NiMeshParticleSystem {
    const TYPE_NAMES: &'static [&'static str] = &["NiMeshParticleSystem"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.system.sync(s, ctx)
    }
}

/// `NiMeshPSysData` — NiPSysData plus mesh-particle pools and a Ref
/// to the node whose children are instanced per particle.
#[derive(Debug, Clone, Default)]
pub struct NiMeshPSysData {
    pub data: NiPSysData,
    pub default_pool_size: u32,
    pub fill_pools_on_load: bool,
    pub generations: Vec<u32>,
    pub particle_meshes: BlockRef,
}

impl TypedBlock for NiMeshPSysData {
    const TYPE_NAMES: &'static [&'static str] = &["NiMeshPSysData"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.data.sync(s, ctx)?;
        s.u32(&mut self.default_pool_size)?;
        s.bool8(&mut self.fill_pools_on_load)?;
        let mut n = self.generations.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            crate::typed::check_len(n)?;
            self.generations = vec![0; n as usize];
        }
        for g in self.generations.iter_mut() {
            s.u32(g)?;
        }
        s.block_ref(&mut self.particle_meshes)
    }
}

/// `BSPSysHavokUpdateModifier` — mesh-update modifier plus a wrapped
/// modifier Ref; drives Havok-simulated mesh particles (dragon debris).
#[derive(Debug, Clone, Default)]
pub struct BSPSysHavokUpdateModifier {
    pub base: ModifierBase,
    pub meshes: Vec<BlockRef>,
    pub modifier: BlockRef,
}

impl TypedBlock for BSPSysHavokUpdateModifier {
    const TYPE_NAMES: &'static [&'static str] = &["BSPSysHavokUpdateModifier"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.base.sync(s)?;
        s.ref_list(&mut self.meshes)?;
        s.block_ref(&mut self.modifier)
    }
}
