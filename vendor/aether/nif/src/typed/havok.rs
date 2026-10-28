//! Havok collision blocks (`bhk*`), typed.
//!
//! Layouts follow nif.xml for 20.2.0.7 / user 12 / bs83 (LE) and bs100
//! (SE), which is the only pair Aether targets, so the version
//! conditions collapse to one branch each and are written down as
//! comments rather than as code. The corpus is the check: 27,828
//! `bhkCollisionObject` blocks over 23,095 files, and every one of them
//! must re-encode byte-identically.
//!
//! **Scale.** Havok stores collision in its own units; the game
//! multiplies by 69.991249 to get world units (1.5.97
//! `bhkRigidBody::sub_14029FE00` @ 0x14029FE34, constant at
//! 0x141536BA0). NifSkope approximates the same factor as 7.0 (the body
//! transform's scale) times 10.0 (its "Skyrim havokScale"), which is 70.
//! Aether uses the measured value.

use super::{check_len, BlockRef, Ctx, Nio, TypedBlock};
use crate::error::{Error, Result};

/// `HavokFilter`: layer, flags and group packed into four bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HavokFilter {
    pub layer: u8,
    pub flags: u8,
    pub group: u16,
}

impl HavokFilter {
    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u8(&mut self.layer)?;
        s.u8(&mut self.flags)?;
        s.u16(&mut self.group)
    }
}

/// `bhkWorldObjCInfoProperty`: three words Havok keeps for its own
/// allocator. Never interpreted here, only carried.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CInfoProperty {
    pub data: u32,
    pub size: u32,
    pub capacity_and_flags: u32,
}

impl CInfoProperty {
    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u32(&mut self.data)?;
        s.u32(&mut self.size)?;
        s.u32(&mut self.capacity_and_flags)
    }
}

/// `bhkWorldObject`: the base every body and phantom starts with.
#[derive(Debug, Clone, Default)]
pub struct WorldObject {
    pub shape: BlockRef,
    pub filter: HavokFilter,
    /// `bhkWorldObjectCInfo`.
    pub unused_01: [u8; 4],
    pub broad_phase_type: u8,
    pub unused_02: [u8; 3],
    pub property: CInfoProperty,
}

impl WorldObject {
    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        s.block_ref(&mut self.shape)?;
        self.filter.sync(s, ctx)?;
        s.bytes(&mut self.unused_01)?;
        s.u8(&mut self.broad_phase_type)?;
        s.bytes(&mut self.unused_02)?;
        self.property.sync(s, ctx)
    }
}

/// `bhkRigidBodyCInfo2010` — the Skyrim body record. Everything after
/// the transform is simulation state the editor does not model; it is
/// carried so the block re-encodes exactly.
#[derive(Debug, Clone, Default)]
pub struct RigidBodyInfo {
    pub unused_01: [u8; 4],
    pub filter: HavokFilter,
    pub unused_02: [u8; 4],
    pub unknown_int_1: u32,
    pub collision_response: u8,
    pub unused_03: u8,
    pub contact_callback_delay: u16,
    /// Only honoured for `bhkRigidBodyT`, and in HAVOK units.
    pub translation: [f32; 4],
    /// Quaternion `(x, y, z, w)`, likewise T-only.
    pub rotation: [f32; 4],
    pub linear_velocity: [f32; 4],
    pub angular_velocity: [f32; 4],
    /// `hkMatrix3`: three rows of four floats, the fourth unused.
    pub inertia: [f32; 12],
    pub center: [f32; 4],
    pub mass: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub time_factor: f32,
    pub gravity_factor: f32,
    pub friction: f32,
    pub rolling_friction: f32,
    pub restitution: f32,
    pub max_linear_velocity: f32,
    pub max_angular_velocity: f32,
    pub penetration_depth: f32,
    pub motion_system: u8,
    pub deactivator_type: u8,
    pub solver_deactivation: u8,
    pub quality_type: u8,
    pub auto_remove_level: u8,
    pub response_modifier_flags: u8,
    pub num_shape_keys: u8,
    pub force_collided_onto_ppu: u8,
    pub unused_04: [u8; 12],
}

impl RigidBodyInfo {
    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        s.bytes(&mut self.unused_01)?;
        self.filter.sync(s, ctx)?;
        s.bytes(&mut self.unused_02)?;
        s.u32(&mut self.unknown_int_1)?;
        s.u8(&mut self.collision_response)?;
        s.u8(&mut self.unused_03)?;
        s.u16(&mut self.contact_callback_delay)?;
        s.f32s(&mut self.translation)?;
        s.f32s(&mut self.rotation)?;
        s.f32s(&mut self.linear_velocity)?;
        s.f32s(&mut self.angular_velocity)?;
        s.f32s(&mut self.inertia)?;
        s.f32s(&mut self.center)?;
        s.f32(&mut self.mass)?;
        s.f32(&mut self.linear_damping)?;
        s.f32(&mut self.angular_damping)?;
        s.f32(&mut self.time_factor)?;
        s.f32(&mut self.gravity_factor)?;
        s.f32(&mut self.friction)?;
        s.f32(&mut self.rolling_friction)?;
        s.f32(&mut self.restitution)?;
        s.f32(&mut self.max_linear_velocity)?;
        s.f32(&mut self.max_angular_velocity)?;
        s.f32(&mut self.penetration_depth)?;
        s.u8(&mut self.motion_system)?;
        s.u8(&mut self.deactivator_type)?;
        s.u8(&mut self.solver_deactivation)?;
        s.u8(&mut self.quality_type)?;
        s.u8(&mut self.auto_remove_level)?;
        s.u8(&mut self.response_modifier_flags)?;
        s.u8(&mut self.num_shape_keys)?;
        s.u8(&mut self.force_collided_onto_ppu)?;
        s.bytes(&mut self.unused_04)
    }
}

/// `bhkRigidBody` and `bhkRigidBodyT`. The T variant is the same record;
/// only it honours `translation`/`rotation` (nif.xml says so outright,
/// and NifSkope keys on the block name for exactly that reason).
#[derive(Debug, Clone, Default)]
pub struct BhkRigidBody {
    pub world: WorldObject,
    /// `bhkEntityCInfo`.
    pub entity_response: u8,
    pub entity_unused: u8,
    pub entity_contact_delay: u16,
    pub info: RigidBodyInfo,
    pub constraints: Vec<BlockRef>,
    /// `ushort` at bs >= 76, which both Skyrim streams are.
    pub body_flags: u16,
}

impl TypedBlock for BhkRigidBody {
    const TYPE_NAMES: &'static [&'static str] = &["bhkRigidBody", "bhkRigidBodyT"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.world.sync(s, ctx)?;
        s.u8(&mut self.entity_response)?;
        s.u8(&mut self.entity_unused)?;
        s.u16(&mut self.entity_contact_delay)?;
        self.info.sync(s, ctx)?;
        s.ref_list(&mut self.constraints)?;
        s.u16(&mut self.body_flags)
    }
}

/// `bhkCollisionObject`: what ties a scene node to a body.
#[derive(Debug, Clone, Default)]
pub struct BhkCollisionObject {
    /// `NiCollisionObject::Target` — a back-POINTER to the node.
    pub target: BlockRef,
    pub flags: u16,
    pub body: BlockRef,
}

impl TypedBlock for BhkCollisionObject {
    const TYPE_NAMES: &'static [&'static str] =
        &["bhkCollisionObject", "bhkSPCollisionObject", "bhkPCollisionObject"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.block_ptr(&mut self.target)?;
        s.u16(&mut self.flags)?;
        s.block_ref(&mut self.body)
    }
}

/// `bhkBlendCollisionObject`: the same, plus two gains.
#[derive(Debug, Clone, Default)]
pub struct BhkBlendCollisionObject {
    pub base: BhkCollisionObject,
    pub heir_gain: f32,
    pub vel_gain: f32,
}

impl TypedBlock for BhkBlendCollisionObject {
    const TYPE_NAMES: &'static [&'static str] = &["bhkBlendCollisionObject"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.base.sync(s, ctx)?;
        s.f32(&mut self.heir_gain)?;
        s.f32(&mut self.vel_gain)
    }
}

/// `bhkMoppBvTreeShape`: a bounding-volume tree wrapped around the shape
/// that actually has geometry. Drawing walks straight through it, as
/// NifSkope does — the MOPP code is an acceleration structure, not a
/// shape.
#[derive(Debug, Clone, Default)]
pub struct BhkMoppBvTreeShape {
    pub shape: BlockRef,
    pub unused_01: [u8; 12],
    pub scale: f32,
    pub mopp_data_size: u32,
    pub mopp_offset: [f32; 4],
    pub build_type: u8,
    pub mopp_data: Vec<u8>,
}

impl TypedBlock for BhkMoppBvTreeShape {
    const TYPE_NAMES: &'static [&'static str] = &["bhkMoppBvTreeShape"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.block_ref(&mut self.shape)?;
        s.bytes(&mut self.unused_01)?;
        s.f32(&mut self.scale)?;
        let mut n = self.mopp_data.len() as u32;
        s.u32(&mut n)?;
        self.mopp_data_size = n;
        s.f32s(&mut self.mopp_offset)?;
        s.u8(&mut self.build_type)?;
        if s.reading() {
            check_len(n)?;
            self.mopp_data = vec![0u8; n as usize];
        }
        s.bytes(&mut self.mopp_data)
    }
}

/// `bhkListShape`: a bag of sub-shapes.
#[derive(Debug, Clone, Default)]
pub struct BhkListShape {
    pub sub_shapes: Vec<BlockRef>,
    pub material: u32,
    pub child_shape_property: CInfoProperty,
    pub child_filter_property: CInfoProperty,
    pub filters: Vec<HavokFilter>,
}

impl TypedBlock for BhkListShape {
    const TYPE_NAMES: &'static [&'static str] = &["bhkListShape"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        s.ref_list(&mut self.sub_shapes)?;
        s.u32(&mut self.material)?;
        self.child_shape_property.sync(s, ctx)?;
        self.child_filter_property.sync(s, ctx)?;
        let mut n = self.filters.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            check_len(n)?;
            self.filters = vec![HavokFilter::default(); n as usize];
        }
        for f in self.filters.iter_mut() {
            f.sync(s, ctx)?;
        }
        Ok(())
    }
}

/// `bhkConvexTransformShape` and `bhkTransformShape`: a shape under a
/// 4x4. Column-major, like every other matrix in the format.
#[derive(Debug, Clone, Default)]
pub struct BhkTransformShape {
    pub shape: BlockRef,
    pub material: u32,
    pub radius: f32,
    pub unused_01: [u8; 8],
    pub transform: [f32; 16],
}

impl TypedBlock for BhkTransformShape {
    const TYPE_NAMES: &'static [&'static str] =
        &["bhkConvexTransformShape", "bhkTransformShape"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.block_ref(&mut self.shape)?;
        s.u32(&mut self.material)?;
        s.f32(&mut self.radius)?;
        s.bytes(&mut self.unused_01)?;
        s.f32s(&mut self.transform)
    }
}

/// `bhkBoxShape`: half extents, in Havok units.
#[derive(Debug, Clone, Default)]
pub struct BhkBoxShape {
    pub material: u32,
    pub radius: f32,
    pub unused_01: [u8; 8],
    pub dimensions: [f32; 3],
    pub unused_float: f32,
}

impl TypedBlock for BhkBoxShape {
    const TYPE_NAMES: &'static [&'static str] = &["bhkBoxShape"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u32(&mut self.material)?;
        s.f32(&mut self.radius)?;
        s.bytes(&mut self.unused_01)?;
        s.f32s(&mut self.dimensions)?;
        s.f32(&mut self.unused_float)
    }
}

/// `bhkSphereShape`: the convex base and nothing else.
#[derive(Debug, Clone, Default)]
pub struct BhkSphereShape {
    pub material: u32,
    pub radius: f32,
}

impl TypedBlock for BhkSphereShape {
    const TYPE_NAMES: &'static [&'static str] = &["bhkSphereShape"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u32(&mut self.material)?;
        s.f32(&mut self.radius)
    }
}

/// `bhkCapsuleShape`: two points and two radii.
#[derive(Debug, Clone, Default)]
pub struct BhkCapsuleShape {
    pub material: u32,
    pub radius: f32,
    pub unused_01: [u8; 8],
    pub first_point: [f32; 3],
    pub radius_1: f32,
    pub second_point: [f32; 3],
    pub radius_2: f32,
}

impl TypedBlock for BhkCapsuleShape {
    const TYPE_NAMES: &'static [&'static str] = &["bhkCapsuleShape"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u32(&mut self.material)?;
        s.f32(&mut self.radius)?;
        s.bytes(&mut self.unused_01)?;
        s.f32s(&mut self.first_point)?;
        s.f32(&mut self.radius_1)?;
        s.f32s(&mut self.second_point)?;
        s.f32(&mut self.radius_2)
    }
}

/// `bhkCylinderShape`: a cylinder given by the two centres of its end
/// caps and a radius.
///
/// nif.xml 3070: the two vertices are Vector4, not Vector3 -- the
/// fourth float is Havok padding and is kept so the block re-encodes
/// byte-identically. `radius` is the convex-shell radius every
/// bhkConvexShape carries; `cylinder_radius` is the cylinder's own.
#[derive(Debug, Clone, Default)]
pub struct BhkCylinderShape {
    pub material: u32,
    pub radius: f32,
    pub unused_01: [u8; 8],
    pub vertex_a: [f32; 4],
    pub vertex_b: [f32; 4],
    pub cylinder_radius: f32,
    pub unused_02: [u8; 12],
}

impl TypedBlock for BhkCylinderShape {
    const TYPE_NAMES: &'static [&'static str] = &["bhkCylinderShape"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u32(&mut self.material)?;
        s.f32(&mut self.radius)?;
        s.bytes(&mut self.unused_01)?;
        s.f32s(&mut self.vertex_a)?;
        s.f32s(&mut self.vertex_b)?;
        s.f32(&mut self.cylinder_radius)?;
        s.bytes(&mut self.unused_02)
    }
}

/// `bhkNiTriStripsShape`: a shape collection whose geometry lives in
/// `NiTriStripsData` blocks -- the LE-era way of storing a collision
/// mesh, before `bhkCompressedMeshShape`.
///
/// nif.xml 3207. The scale is a Vector4 and applies to the strip data;
/// there is one filter per strips-data entry.
#[derive(Debug, Clone, Default)]
pub struct BhkNiTriStripsShape {
    pub material: u32,
    pub radius: f32,
    pub unused_01: [u8; 20],
    pub grow_by: u32,
    pub scale: [f32; 4],
    pub strips_data: Vec<BlockRef>,
    pub filters: Vec<HavokFilter>,
}

impl TypedBlock for BhkNiTriStripsShape {
    const TYPE_NAMES: &'static [&'static str] = &["bhkNiTriStripsShape"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        s.u32(&mut self.material)?;
        s.f32(&mut self.radius)?;
        s.bytes(&mut self.unused_01)?;
        s.u32(&mut self.grow_by)?;
        s.f32s(&mut self.scale)?;
        s.ref_list(&mut self.strips_data)?;
        let mut n = self.filters.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            check_len(n)?;
            self.filters = vec![HavokFilter::default(); n as usize];
        }
        for f in self.filters.iter_mut() {
            f.sync(s, ctx)?;
        }
        Ok(())
    }
}

/// `bhkConvexVerticesShape`: a hull as points plus half-space planes.
#[derive(Debug, Clone, Default)]
pub struct BhkConvexVerticesShape {
    pub material: u32,
    pub radius: f32,
    pub vertices_property: CInfoProperty,
    pub normals_property: CInfoProperty,
    /// `w` is 0 and carried.
    pub vertices: Vec<[f32; 4]>,
    /// `xyz` is the outward normal, `w` the signed plane distance.
    pub normals: Vec<[f32; 4]>,
}

impl TypedBlock for BhkConvexVerticesShape {
    const TYPE_NAMES: &'static [&'static str] = &["bhkConvexVerticesShape"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        s.u32(&mut self.material)?;
        s.f32(&mut self.radius)?;
        self.vertices_property.sync(s, ctx)?;
        self.normals_property.sync(s, ctx)?;
        s.vec4_list(&mut self.vertices)?;
        s.vec4_list(&mut self.normals)
    }
}

/// One `bhkCMSChunk`.
#[derive(Debug, Clone, Default)]
pub struct CmsChunk {
    pub translation: [f32; 4],
    pub material_index: u32,
    pub reference: u16,
    pub transform_index: u16,
    /// Quantised positions, three `u16` per vertex, in Havok units x1000
    /// relative to `translation` plus the chunk transform.
    pub vertices: Vec<u16>,
    pub indices: Vec<u16>,
    /// Lengths of the strips that lead the index list; whatever follows
    /// them is plain triangles.
    pub strips: Vec<u16>,
    pub welding_info: Vec<u16>,
}

/// One `bhkQsTransform`.
#[derive(Debug, Clone, Default)]
pub struct CmsTransform {
    pub translation: [f32; 4],
    /// `(x, y, z, w)`.
    pub rotation: [f32; 4],
}

/// One `bhkCMSBigTri`.
#[derive(Debug, Clone, Default)]
pub struct CmsBigTri {
    pub triangle: [u16; 3],
    pub material: u32,
    pub welding_info: u16,
}

/// `bhkCompressedMeshShapeData`: the real geometry of nearly every
/// static collision in the game — 16,577 blocks over 15,956 files.
#[derive(Debug, Clone, Default)]
pub struct BhkCompressedMeshShapeData {
    pub bits_per_index: u32,
    pub bits_per_w_index: u32,
    pub mask_w_index: u32,
    pub mask_index: u32,
    pub error: f32,
    pub aabb_min: [f32; 4],
    pub aabb_max: [f32; 4],
    pub welding_type: u8,
    pub material_type: u8,
    pub materials_32: Vec<u32>,
    pub materials_16: Vec<u32>,
    pub materials_8: Vec<u32>,
    /// `bhkMeshMaterial`: a material word and a filter word.
    pub chunk_materials: Vec<(u32, HavokFilter)>,
    pub num_named_materials: u32,
    pub chunk_transforms: Vec<CmsTransform>,
    pub big_verts: Vec<[f32; 4]>,
    pub big_tris: Vec<CmsBigTri>,
    pub chunks: Vec<CmsChunk>,
    pub num_convex_piece_a: u32,
}

fn u16_list(s: &mut dyn Nio, v: &mut Vec<u16>, n: u32) -> Result<()> {
    if s.reading() {
        check_len(n)?;
        *v = vec![0u16; n as usize];
    }
    for x in v.iter_mut() {
        s.u16(x)?;
    }
    Ok(())
}

impl TypedBlock for BhkCompressedMeshShapeData {
    const TYPE_NAMES: &'static [&'static str] = &["bhkCompressedMeshShapeData"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        s.u32(&mut self.bits_per_index)?;
        s.u32(&mut self.bits_per_w_index)?;
        s.u32(&mut self.mask_w_index)?;
        s.u32(&mut self.mask_index)?;
        s.f32(&mut self.error)?;
        s.f32s(&mut self.aabb_min)?;
        s.f32s(&mut self.aabb_max)?;
        s.u8(&mut self.welding_type)?;
        s.u8(&mut self.material_type)?;
        for list in [
            &mut self.materials_32,
            &mut self.materials_16,
            &mut self.materials_8,
        ] {
            let mut n = list.len() as u32;
            s.u32(&mut n)?;
            if s.reading() {
                check_len(n)?;
                *list = vec![0u32; n as usize];
            }
            for x in list.iter_mut() {
                s.u32(x)?;
            }
        }
        let mut n = self.chunk_materials.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            check_len(n)?;
            self.chunk_materials = vec![(0u32, HavokFilter::default()); n as usize];
        }
        for (m, f) in self.chunk_materials.iter_mut() {
            s.u32(m)?;
            f.sync(s, ctx)?;
        }
        s.u32(&mut self.num_named_materials)?;
        let mut n = self.chunk_transforms.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            check_len(n)?;
            self.chunk_transforms = vec![CmsTransform::default(); n as usize];
        }
        for t in self.chunk_transforms.iter_mut() {
            s.f32s(&mut t.translation)?;
            s.f32s(&mut t.rotation)?;
        }
        s.vec4_list(&mut self.big_verts)?;
        let mut n = self.big_tris.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            check_len(n)?;
            self.big_tris = vec![CmsBigTri::default(); n as usize];
        }
        for t in self.big_tris.iter_mut() {
            for i in 0..3 {
                s.u16(&mut t.triangle[i])?;
            }
            s.u32(&mut t.material)?;
            s.u16(&mut t.welding_info)?;
        }
        let mut n = self.chunks.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            check_len(n)?;
            self.chunks = vec![CmsChunk::default(); n as usize];
        }
        for c in self.chunks.iter_mut() {
            s.f32s(&mut c.translation)?;
            s.u32(&mut c.material_index)?;
            s.u16(&mut c.reference)?;
            s.u16(&mut c.transform_index)?;
            // Counted in u16s, not in vertices: nif.xml stores
            // `Num Vertices` already multiplied by three.
            let mut nv = c.vertices.len() as u32;
            s.u32(&mut nv)?;
            u16_list(s, &mut c.vertices, nv)?;
            let mut ni = c.indices.len() as u32;
            s.u32(&mut ni)?;
            u16_list(s, &mut c.indices, ni)?;
            let mut ns = c.strips.len() as u32;
            s.u32(&mut ns)?;
            u16_list(s, &mut c.strips, ns)?;
            let mut nw = c.welding_info.len() as u32;
            s.u32(&mut nw)?;
            u16_list(s, &mut c.welding_info, nw)?;
        }
        s.u32(&mut self.num_convex_piece_a)
    }
}

/// `bhkCompressedMeshShape`: the wrapper that names the data and the
/// node it belongs to.
#[derive(Debug, Clone, Default)]
pub struct BhkCompressedMeshShape {
    pub target: BlockRef,
    pub user_data: u32,
    pub radius: f32,
    pub unknown_float_1: f32,
    pub scale: [f32; 4],
    pub radius_copy: f32,
    pub scale_copy: [f32; 4],
    pub data: BlockRef,
}

impl TypedBlock for BhkCompressedMeshShape {
    const TYPE_NAMES: &'static [&'static str] = &["bhkCompressedMeshShape"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.block_ptr(&mut self.target)?;
        s.u32(&mut self.user_data)?;
        s.f32(&mut self.radius)?;
        s.f32(&mut self.unknown_float_1)?;
        s.f32s(&mut self.scale)?;
        s.f32(&mut self.radius_copy)?;
        s.f32s(&mut self.scale_copy)?;
        s.block_ref(&mut self.data)
    }
}

// The two remaining pieces of the collision tree Aether meets but does
// not draw: a phantom is a trigger volume, and `NiCollisionObject` is
// what an unrecognised collision object falls back to.
/// `bhkSimpleShapePhantom`.
#[derive(Debug, Clone, Default)]
pub struct BhkSimpleShapePhantom {
    pub world: WorldObject,
    pub unused_01: [u8; 8],
    pub transform: [f32; 16],
}

impl TypedBlock for BhkSimpleShapePhantom {
    const TYPE_NAMES: &'static [&'static str] = &["bhkSimpleShapePhantom"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.world.sync(s, ctx)?;
        s.bytes(&mut self.unused_01)?;
        s.f32s(&mut self.transform)
    }
}

// ---------------------------------------------------------------------
// Constraints. They carry no geometry and nothing here is authored by
// hand; the reason to type them is the same as the multi-bounds: each
// one holds two (or a chain of) PTRS to the rigid bodies it joins, and
// an untyped block's references are invisible to `block_refs`, so every
// ownership walk over a ragdoll file had holes. Layouts are nif.xml's
// modern branch (`!#NI_BS_LTE_16#`, since 20.2.0.7) — the only branch
// a Skyrim stream can carry, which is all `Ctx::of` admits.

/// `bhkConstraintCInfo`: the two joined entities. `num_entities` is
/// "always 2 (hardcoded)" per nif.xml, but it is stored, so it is kept.
#[derive(Debug, Clone, Default)]
pub struct ConstraintInfo {
    pub num_entities: u32,
    /// Back-POINTERS to the two `bhkRigidBody` blocks this joins.
    pub entity_a: BlockRef,
    pub entity_b: BlockRef,
    /// 1 = PSI (regular steps), 3 = TOI (also at time of impact).
    pub priority: u32,
}

impl ConstraintInfo {
    fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.u32(&mut self.num_entities)?;
        s.block_ptr(&mut self.entity_a)?;
        s.block_ptr(&mut self.entity_b)?;
        s.u32(&mut self.priority)
    }
}

/// `bhkConstraintMotorCInfo`: a type byte, then N floats and M raw
/// bool-bytes — every motor variant is floats-then-bools, so one
/// shape covers all three. Bools stay raw u8: the format does not
/// normalize them, the same lesson the interpolator pose byte taught.
#[derive(Debug, Clone, Default)]
pub struct ConstraintMotor {
    pub motor_type: u8,
    pub params: Vec<f32>,
    pub flags: Vec<u8>,
}

impl ConstraintMotor {
    /// (float count, bool count) per motor type; None = no such type.
    fn arity(motor_type: u8) -> Option<(usize, usize)> {
        match motor_type {
            0 => Some((0, 0)),
            1 => Some((6, 1)), // position: forces, tau, damping, recovery x2
            2 => Some((4, 2)), // velocity: forces, tau, target + 2 bools
            3 => Some((4, 1)), // spring damper: forces, constant, damping
            _ => None,
        }
    }

    fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.u8(&mut self.motor_type)?;
        let (nf, nb) = Self::arity(self.motor_type).ok_or_else(|| Error::Corrupt {
            offset: 0,
            what: format!("constraint motor type {} has no layout", self.motor_type),
        })?;
        if s.reading() {
            self.params = vec![0.0; nf];
            self.flags = vec![0; nb];
        }
        s.f32s(&mut self.params)?;
        s.bytes(&mut self.flags)
    }
}

/// `bhkRagdollConstraintCInfo`, modern branch: a cone joint with
/// per-side frames, five limit angles, friction and a motor.
#[derive(Debug, Clone, Default)]
pub struct RagdollInfo {
    pub twist_a: [f32; 4],
    pub plane_a: [f32; 4],
    pub motor_a: [f32; 4],
    pub pivot_a: [f32; 4],
    pub twist_b: [f32; 4],
    pub plane_b: [f32; 4],
    pub motor_b: [f32; 4],
    pub pivot_b: [f32; 4],
    pub cone_max_angle: f32,
    pub plane_min_angle: f32,
    pub plane_max_angle: f32,
    pub twist_min_angle: f32,
    pub twist_max_angle: f32,
    pub max_friction: f32,
    pub motor: ConstraintMotor,
}

impl RagdollInfo {
    fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.f32s(&mut self.twist_a)?;
        s.f32s(&mut self.plane_a)?;
        s.f32s(&mut self.motor_a)?;
        s.f32s(&mut self.pivot_a)?;
        s.f32s(&mut self.twist_b)?;
        s.f32s(&mut self.plane_b)?;
        s.f32s(&mut self.motor_b)?;
        s.f32s(&mut self.pivot_b)?;
        s.f32(&mut self.cone_max_angle)?;
        s.f32(&mut self.plane_min_angle)?;
        s.f32(&mut self.plane_max_angle)?;
        s.f32(&mut self.twist_min_angle)?;
        s.f32(&mut self.twist_max_angle)?;
        s.f32(&mut self.max_friction)?;
        self.motor.sync(s)
    }
}

/// `bhkLimitedHingeConstraintCInfo`, modern branch.
#[derive(Debug, Clone, Default)]
pub struct LimitedHingeInfo {
    pub axis_a: [f32; 4],
    pub perp_axis_a1: [f32; 4],
    pub perp_axis_a2: [f32; 4],
    pub pivot_a: [f32; 4],
    pub axis_b: [f32; 4],
    pub perp_axis_b1: [f32; 4],
    pub perp_axis_b2: [f32; 4],
    pub pivot_b: [f32; 4],
    pub min_angle: f32,
    pub max_angle: f32,
    pub max_friction: f32,
    pub motor: ConstraintMotor,
}

impl LimitedHingeInfo {
    fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.f32s(&mut self.axis_a)?;
        s.f32s(&mut self.perp_axis_a1)?;
        s.f32s(&mut self.perp_axis_a2)?;
        s.f32s(&mut self.pivot_a)?;
        s.f32s(&mut self.axis_b)?;
        s.f32s(&mut self.perp_axis_b1)?;
        s.f32s(&mut self.perp_axis_b2)?;
        s.f32s(&mut self.pivot_b)?;
        s.f32(&mut self.min_angle)?;
        s.f32(&mut self.max_angle)?;
        s.f32(&mut self.max_friction)?;
        self.motor.sync(s)
    }
}

/// `bhkHingeConstraintCInfo`, modern branch: two frames, no limits, no
/// motor.
#[derive(Debug, Clone, Default)]
pub struct HingeInfo {
    pub axis_a: [f32; 4],
    pub perp_axis_a1: [f32; 4],
    pub perp_axis_a2: [f32; 4],
    pub pivot_a: [f32; 4],
    pub axis_b: [f32; 4],
    pub perp_axis_b1: [f32; 4],
    pub perp_axis_b2: [f32; 4],
    pub pivot_b: [f32; 4],
}

impl HingeInfo {
    fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.f32s(&mut self.axis_a)?;
        s.f32s(&mut self.perp_axis_a1)?;
        s.f32s(&mut self.perp_axis_a2)?;
        s.f32s(&mut self.pivot_a)?;
        s.f32s(&mut self.axis_b)?;
        s.f32s(&mut self.perp_axis_b1)?;
        s.f32s(&mut self.perp_axis_b2)?;
        s.f32s(&mut self.pivot_b)
    }
}

/// `bhkRagdollConstraint` — 1,504 corpus blocks, every creature
/// skeleton.
#[derive(Debug, Clone, Default)]
pub struct BhkRagdollConstraint {
    pub info: ConstraintInfo,
    pub ragdoll: RagdollInfo,
}

impl TypedBlock for BhkRagdollConstraint {
    const TYPE_NAMES: &'static [&'static str] = &["bhkRagdollConstraint"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.info.sync(s)?;
        self.ragdoll.sync(s)
    }
}

/// `bhkLimitedHingeConstraint` — 1,650 corpus blocks: doors, tails,
/// dangling props.
#[derive(Debug, Clone, Default)]
pub struct BhkLimitedHingeConstraint {
    pub info: ConstraintInfo,
    pub hinge: LimitedHingeInfo,
}

impl TypedBlock for BhkLimitedHingeConstraint {
    const TYPE_NAMES: &'static [&'static str] = &["bhkLimitedHingeConstraint"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.info.sync(s)?;
        self.hinge.sync(s)
    }
}

/// `bhkHingeConstraint` — the unlimited form, 16 corpus blocks.
#[derive(Debug, Clone, Default)]
pub struct BhkHingeConstraint {
    pub info: ConstraintInfo,
    pub hinge: HingeInfo,
}

impl TypedBlock for BhkHingeConstraint {
    const TYPE_NAMES: &'static [&'static str] = &["bhkHingeConstraint"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.info.sync(s)?;
        self.hinge.sync(s)
    }
}

/// `bhkBallAndSocketConstraint` — a point-to-point joint.
#[derive(Debug, Clone, Default)]
pub struct BhkBallAndSocketConstraint {
    pub info: ConstraintInfo,
    pub pivot_a: [f32; 4],
    pub pivot_b: [f32; 4],
}

impl TypedBlock for BhkBallAndSocketConstraint {
    const TYPE_NAMES: &'static [&'static str] = &["bhkBallAndSocketConstraint"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.info.sync(s)?;
        s.f32s(&mut self.pivot_a)?;
        s.f32s(&mut self.pivot_b)
    }
}

/// `bhkStiffSpringConstraint` — holds two bodies a fixed distance
/// apart.
#[derive(Debug, Clone, Default)]
pub struct BhkStiffSpringConstraint {
    pub info: ConstraintInfo,
    pub pivot_a: [f32; 4],
    pub pivot_b: [f32; 4],
    pub length: f32,
}

impl TypedBlock for BhkStiffSpringConstraint {
    const TYPE_NAMES: &'static [&'static str] = &["bhkStiffSpringConstraint"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.info.sync(s)?;
        s.f32s(&mut self.pivot_a)?;
        s.f32s(&mut self.pivot_b)?;
        s.f32(&mut self.length)
    }
}

/// `bhkBallSocketConstraintChain` — a rope of ball joints. Inherits
/// bhkSerializable, NOT bhkConstraint, so there is no leading CInfo;
/// the chain's own entity list comes first and the CInfo trails it.
///
/// The stored pivot count counts individual Vector4s while the array
/// holds PAIRS (nif.xml: `calc = len * 2`, `length = count / 2`), so
/// the vec here is of pairs and the count written is `len * 2`.
#[derive(Debug, Clone, Default)]
pub struct BhkBallSocketConstraintChain {
    pub pivots: Vec<[[f32; 4]; 2]>,
    pub tau: f32,
    pub damping: f32,
    pub constraint_force_mixing: f32,
    pub max_error_distance: f32,
    /// Back-pointers to every chained `bhkRigidBody`.
    pub chained_entities: Vec<BlockRef>,
    pub info: ConstraintInfo,
}

impl TypedBlock for BhkBallSocketConstraintChain {
    const TYPE_NAMES: &'static [&'static str] = &["bhkBallSocketConstraintChain"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        let mut n = self.pivots.len() as u32 * 2;
        s.u32(&mut n)?;
        if s.reading() {
            check_len(n)?;
            self.pivots = vec![[[0.0; 4]; 2]; n as usize / 2];
        }
        for p in self.pivots.iter_mut() {
            s.f32s(&mut p[0])?;
            s.f32s(&mut p[1])?;
        }
        s.f32(&mut self.tau)?;
        s.f32(&mut self.damping)?;
        s.f32(&mut self.constraint_force_mixing)?;
        s.f32(&mut self.max_error_distance)?;
        let mut ne = self.chained_entities.len() as u32;
        s.u32(&mut ne)?;
        if s.reading() {
            check_len(ne)?;
            self.chained_entities = vec![BlockRef::NONE; ne as usize];
        }
        for e in self.chained_entities.iter_mut() {
            s.block_ptr(e)?;
        }
        self.info.sync(s)
    }
}

/// The payload variants `bhkWrappedConstraintData` selects by type.
#[derive(Debug, Clone)]
pub enum WrappedConstraint {
    BallAndSocket { pivot_a: [f32; 4], pivot_b: [f32; 4] },
    Hinge(HingeInfo),
    LimitedHinge(LimitedHingeInfo),
    Ragdoll(RagdollInfo),
    StiffSpring { pivot_a: [f32; 4], pivot_b: [f32; 4], length: f32 },
}

impl Default for WrappedConstraint {
    fn default() -> Self {
        WrappedConstraint::BallAndSocket {
            pivot_a: [0.0; 4],
            pivot_b: [0.0; 4],
        }
    }
}

/// `bhkBreakableConstraint` — a wrapper that stops constraining past a
/// force threshold. Its payload is POLYMORPHIC (`bhkWrappedConstraintData`:
/// a type enum, a second CInfo with two more entity Ptrs, then the
/// wrapped constraint's own fields). Prismatic (6) and malleable (13)
/// are refused until a real file carries one — a refusal decodes as
/// Err and the raw path still round-trips the block untouched.
#[derive(Debug, Clone, Default)]
pub struct BhkBreakableConstraint {
    pub info: ConstraintInfo,
    pub wrapped_type: u32,
    pub wrapped_info: ConstraintInfo,
    pub wrapped: WrappedConstraint,
    pub threshold: f32,
    pub remove_when_broken: u8,
}

impl TypedBlock for BhkBreakableConstraint {
    const TYPE_NAMES: &'static [&'static str] = &["bhkBreakableConstraint"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.info.sync(s)?;
        s.u32(&mut self.wrapped_type)?;
        self.wrapped_info.sync(s)?;
        if s.reading() {
            self.wrapped = match self.wrapped_type {
                0 => WrappedConstraint::default(),
                1 => WrappedConstraint::Hinge(HingeInfo::default()),
                2 => WrappedConstraint::LimitedHinge(LimitedHingeInfo::default()),
                7 => WrappedConstraint::Ragdoll(RagdollInfo::default()),
                8 => WrappedConstraint::StiffSpring {
                    pivot_a: [0.0; 4],
                    pivot_b: [0.0; 4],
                    length: 0.0,
                },
                t => {
                    return Err(Error::Corrupt {
                        offset: 0,
                        what: format!("breakable constraint wraps unsupported type {t}"),
                    })
                }
            };
        }
        match &mut self.wrapped {
            WrappedConstraint::BallAndSocket { pivot_a, pivot_b } => {
                s.f32s(pivot_a)?;
                s.f32s(pivot_b)?;
            }
            WrappedConstraint::Hinge(h) => h.sync(s)?,
            WrappedConstraint::LimitedHinge(h) => h.sync(s)?,
            WrappedConstraint::Ragdoll(r) => r.sync(s)?,
            WrappedConstraint::StiffSpring { pivot_a, pivot_b, length } => {
                s.f32s(pivot_a)?;
                s.f32s(pivot_b)?;
                s.f32(length)?;
            }
        }
        s.f32(&mut self.threshold)?;
        s.u8(&mut self.remove_when_broken)
    }
}

/// `bhkPlaneShape` — a bounded half-space: normal, distance, and the
/// AABB that clips it.
#[derive(Debug, Clone, Default)]
pub struct BhkPlaneShape {
    pub material: u32,
    pub unused_01: [u8; 12],
    pub plane_normal: [f32; 3],
    pub plane_constant: f32,
    pub half_extents: [f32; 4],
    pub center: [f32; 4],
}

impl TypedBlock for BhkPlaneShape {
    const TYPE_NAMES: &'static [&'static str] = &["bhkPlaneShape"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.u32(&mut self.material)?;
        s.bytes(&mut self.unused_01)?;
        s.f32s(&mut self.plane_normal)?;
        s.f32(&mut self.plane_constant)?;
        s.f32s(&mut self.half_extents)?;
        s.f32s(&mut self.center)
    }
}
