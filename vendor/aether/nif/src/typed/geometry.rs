//! Scene-graph containers and mesh geometry blocks.
//!
//! Layouts follow nif.xml; the decode semantics (what the bytes mean, and
//! the traps) are the verified findings in `docs/knowledge/mesh-rendering.md`:
//! SSE positions are always full f32 at stream 100 regardless of
//! `VF_FULLPREC`; packed normals are unsigned bytes with bias (`v/255*2-1`),
//! not signed bytes; the bitangent is split across three fields; LE has
//! Group ID + Keep/Compress Flags before the vertex count (6 bytes a naive
//! reader misses); and nif.xml's size formula for the SSE particle trailer
//! is wrong, so the stored value is what round-trips.

use super::{AvObject, BlockRef, Bound, Ctx, Nio, StringIdx, TypedBlock};
use crate::error::{Error, Result};

/// `NiNode` — also covers Bethesda fade/billboard-free variants that add no
/// fields of their own and only differ by type name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiNode {
    pub av: AvObject,
    pub children: Vec<BlockRef>,
    pub effects: Vec<BlockRef>,
}

impl TypedBlock for NiNode {
    // nif.xml derives both from NiNode with no fields of their own.
    // BSLeafAnimNode was the last block type that made a real file
    // refuse to export (the smoking pine, 2 corpus files).
    const TYPE_NAMES: &'static [&'static str] = &["NiNode", "BSFadeNode", "BSLeafAnimNode"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.av.sync(s, ctx)?;
        s.ref_list(&mut self.children)?;
        // Effects list exists for bs <= 130, i.e. both Skyrim LE and SE.
        s.ref_list(&mut self.effects)
    }
}


/// `BSMasterParticleSystem` — the root of an MPS file: a NiNode that
/// owns a budget and the list of particle systems spawning into its
/// shared pool (the magic-trail machinery).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsMasterParticleSystem {
    pub node: NiNode,
    pub max_emitter_objects: u16,
    pub particle_systems: Vec<BlockRef>,
}

impl TypedBlock for BsMasterParticleSystem {
    const TYPE_NAMES: &'static [&'static str] = &["BSMasterParticleSystem"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.node.sync(s, ctx)?;
        s.u16(&mut self.max_emitter_objects)?;
        s.ref_list(&mut self.particle_systems)
    }
}

/// `BSOrderedNode` — a NiNode whose children draw in list order (vanilla
/// uses it to stack particle systems over each other). The container the
/// giant campfire keeps its systems and emitter anchors under.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsOrderedNode {
    pub node: NiNode,
    pub alpha_sort_bound: [f32; 4],
    pub static_bound: bool,
}

impl TypedBlock for BsOrderedNode {
    const TYPE_NAMES: &'static [&'static str] = &["BSOrderedNode"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.node.sync(s, ctx)?;
        s.f32s(&mut self.alpha_sort_bound)?;
        s.bool8(&mut self.static_bound)
    }
}

/// `BSValueNode` — a NiNode carrying one value (fxFire effects hang off
/// these); the subtree renders like any other node's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsValueNode {
    pub node: NiNode,
    pub value: u32,
    pub value_node_flags: u8,
}

impl TypedBlock for BsValueNode {
    const TYPE_NAMES: &'static [&'static str] = &["BSValueNode"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.node.sync(s, ctx)?;
        s.u32(&mut self.value)?;
        s.u8(&mut self.value_node_flags)
    }
}

/// `NiSwitchNode` — only the child at `index` draws at a time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiSwitchNode {
    pub node: NiNode,
    pub switch_flags: u16,
    pub index: u32,
}

impl TypedBlock for NiSwitchNode {
    const TYPE_NAMES: &'static [&'static str] = &["NiSwitchNode"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.node.sync(s, ctx)?;
        s.u16(&mut self.switch_flags)?;
        s.u32(&mut self.index)
    }
}

/// `BSRangeNode` and its aliases (`BSBlastNode`, `BSDamageStage`,
/// `BSDebrisNode`) — destruction-stage containers; the layout is a NiNode
/// plus three bytes, identical across all four names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsRangeNode {
    pub node: NiNode,
    pub min: u8,
    pub max: u8,
    pub current: u8,
}

impl TypedBlock for BsRangeNode {
    const TYPE_NAMES: &'static [&'static str] =
        &["BSRangeNode", "BSBlastNode", "BSDamageStage", "BSDebrisNode"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.node.sync(s, ctx)?;
        s.u8(&mut self.min)?;
        s.u8(&mut self.max)?;
        s.u8(&mut self.current)
    }
}

/// `BSMultiBoundNode` — culling container; Skyrim adds the culling mode.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsMultiBoundNode {
    pub node: NiNode,
    pub multi_bound: BlockRef,
    pub culling_mode: u32,
}

impl TypedBlock for BsMultiBoundNode {
    const TYPE_NAMES: &'static [&'static str] = &["BSMultiBoundNode"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.node.sync(s, ctx)?;
        s.block_ref(&mut self.multi_bound)?;
        s.u32(&mut self.culling_mode)
    }
}

/// `BSMultiBound` — the culling volume a `BSMultiBoundNode` points at.
/// One reference, to the data block below.
///
/// The biggest untyped class in the corpus by a wide margin: 36,280
/// blocks in 21,816 files, with 35,800 `BSMultiBoundAABB` beside them.
/// Typing it is not about editing — nothing here is authored by hand
/// — but about the ref SCANNER: an untyped block's references are
/// invisible to `block_refs`, so the splice could not see that a
/// multi-bound owns its data block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsMultiBound {
    pub data: BlockRef,
}

impl TypedBlock for BsMultiBound {
    const TYPE_NAMES: &'static [&'static str] = &["BSMultiBound"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.block_ref(&mut self.data)
    }
}

/// `BSMultiBoundAABB` — an axis-aligned box: centre and half-extent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsMultiBoundAabb {
    pub position: [f32; 3],
    pub extent: [f32; 3],
}

impl TypedBlock for BsMultiBoundAabb {
    const TYPE_NAMES: &'static [&'static str] = &["BSMultiBoundAABB"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.f32s(&mut self.position)?;
        s.f32s(&mut self.extent)
    }
}

/// `BSMultiBoundOBB` — an oriented box: centre, size, and a 3x3
/// rotation, in nif.xml's field order (Center, Size, Rotation).
#[derive(Debug, Clone, PartialEq)]
pub struct BsMultiBoundObb {
    pub center: [f32; 3],
    pub size: [f32; 3],
    /// Row-major, like every other Matrix33 in this crate.
    pub rotation: [f32; 9],
}

impl Default for BsMultiBoundObb {
    fn default() -> Self {
        BsMultiBoundObb {
            center: [0.0; 3],
            size: [0.0; 3],
            rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }
}

impl TypedBlock for BsMultiBoundObb {
    const TYPE_NAMES: &'static [&'static str] = &["BSMultiBoundOBB"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.f32s(&mut self.center)?;
        s.f32s(&mut self.size)?;
        s.f32s(&mut self.rotation)
    }
}

/// `BSTreeNode` — tree LOD container with two bone lists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsTreeNode {
    pub node: NiNode,
    pub bones1: Vec<BlockRef>,
    pub bones2: Vec<BlockRef>,
}

impl TypedBlock for BsTreeNode {
    const TYPE_NAMES: &'static [&'static str] = &["BSTreeNode"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.node.sync(s, ctx)?;
        s.ptr_list(&mut self.bones1)?;
        s.ptr_list(&mut self.bones2)
    }
}

/// `NiBillboardNode` — the subtree rotates to face the camera. Vanilla
/// fire is built from flame-card meshes under these (mode 5,
/// BSROTATE_ABOUT_UP: yaw about local Z only, cards stay upright).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiBillboardNode {
    pub node: NiNode,
    pub billboard_mode: u16,
}

impl TypedBlock for NiBillboardNode {
    const TYPE_NAMES: &'static [&'static str] = &["NiBillboardNode"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.node.sync(s, ctx)?;
        s.u16(&mut self.billboard_mode)
    }
}

// ---------------------------------------------------------------------------
// SSE: BSTriShape
// ---------------------------------------------------------------------------

/// `BSVertexDesc` attribute flags (the bits above `desc >> 44`).
pub mod vf {
    pub const VERTEX: u64 = 0x001;
    pub const UV: u64 = 0x002;
    pub const NORMAL: u64 = 0x008;
    pub const TANGENT: u64 = 0x010;
    pub const COLORS: u64 = 0x020;
    pub const SKINNED: u64 = 0x040;
    pub const EYEDATA: u64 = 0x100;
}

/// One interleaved SSE vertex, kept in its raw storage forms so re-encoding
/// is bit-identical; the `*_f32` accessors decode.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SseVertex {
    /// Full-precision — stream 100 never stores half positions, whatever
    /// `VF_FULLPREC` says.
    pub position: [f32; 3],
    /// Bitangent X when tangents are present, otherwise unused padding.
    /// Same four bytes either way, so one field round-trips both.
    pub bitangent_x: f32,
    /// IEEE binary16 bits.
    pub uv: [u16; 2],
    /// Unsigned bias-packed: `v/255*2 - 1`.
    pub normal: [u8; 3],
    pub bitangent_y: u8,
    pub tangent: [u8; 3],
    pub bitangent_z: u8,
    /// RGBA, `v/255`.
    pub color: [u8; 4],
    /// IEEE binary16 bits.
    pub bone_weights: [u16; 4],
    pub bone_indices: [u8; 4],
    pub eye_data: f32,
}

impl SseVertex {
    pub fn uv_f32(&self) -> [f32; 2] {
        [half_to_f32(self.uv[0]), half_to_f32(self.uv[1])]
    }

    pub fn normal_f32(&self) -> [f32; 3] {
        [
            unpack_norm_byte(self.normal[0]),
            unpack_norm_byte(self.normal[1]),
            unpack_norm_byte(self.normal[2]),
        ]
    }

    pub fn color_f32(&self) -> [f32; 4] {
        [
            self.color[0] as f32 / 255.0,
            self.color[1] as f32 / 255.0,
            self.color[2] as f32 / 255.0,
            self.color[3] as f32 / 255.0,
        ]
    }
}

/// `BSTriShape` — SSE mesh geometry, everything interleaved on the one block.
///
/// Not covering `BSDynamicTriShape` (it appends a Vector4 position array):
/// decoding one through this layout leaves bytes over and fails cleanly, and
/// the editor's lossless raw-block invariant keeps such files intact.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsTriShape {
    pub av: AvObject,
    pub bound: Bound,
    pub skin: BlockRef,
    pub shader_property: BlockRef,
    pub alpha_property: BlockRef,
    pub vertex_desc: u64,
    /// Stored even when the arrays are absent: a skinned shape keeps its
    /// geometry on the NiSkinPartition and writes zeros here.
    pub num_triangles: u16,
    pub num_vertices: u16,
    /// Gate for the vertex/triangle arrays. Kept raw so a file that stores a
    /// count with no data (skinned shapes do) round-trips exactly.
    pub data_size: u32,
    pub vertices: Vec<SseVertex>,
    pub triangles: Vec<[u16; 3]>,
    /// The CPU-particle trailer, stream-100 only. nif.xml's calc for this
    /// field is wrong (docs/knowledge/mesh-rendering.md §1.4); the stored
    /// value is authoritative.
    pub particle_data_size: u32,
    /// Half-float triples, raw.
    pub particle_vertices: Vec<[u16; 3]>,
    pub particle_normals: Vec<[u16; 3]>,
    pub particle_triangles: Vec<[u16; 3]>,
}

impl BsTriShape {
    /// The attribute flags half of the descriptor.
    pub fn attrs(&self) -> u64 {
        self.vertex_desc >> 44
    }

    /// Vertex stride in bytes, from the descriptor's low nibble.
    pub fn stride(&self) -> usize {
        (self.vertex_desc & 0xF) as usize * 4
    }

    fn sync_vertex(v: &mut SseVertex, s: &mut dyn Nio, attrs: u64) -> Result<()> {
        if attrs & vf::VERTEX != 0 {
            s.f32s(&mut v.position)?;
            // Bitangent X or unused W: same slot, one field.
            s.f32(&mut v.bitangent_x)?;
        }
        if attrs & vf::UV != 0 {
            s.u16(&mut v.uv[0])?;
            s.u16(&mut v.uv[1])?;
        }
        if attrs & vf::NORMAL != 0 {
            for b in &mut v.normal {
                s.u8(b)?;
            }
            s.u8(&mut v.bitangent_y)?;
            // Tangents are nested inside normals: (flags & 0x18) == 0x18.
            if attrs & vf::TANGENT != 0 {
                for b in &mut v.tangent {
                    s.u8(b)?;
                }
                s.u8(&mut v.bitangent_z)?;
            }
        }
        if attrs & vf::COLORS != 0 {
            for b in &mut v.color {
                s.u8(b)?;
            }
        }
        if attrs & vf::SKINNED != 0 {
            for w in &mut v.bone_weights {
                s.u16(w)?;
            }
            for b in &mut v.bone_indices {
                s.u8(b)?;
            }
        }
        if attrs & vf::EYEDATA != 0 {
            s.f32(&mut v.eye_data)?;
        }
        Ok(())
    }
}

fn sync_tri_list(s: &mut dyn Nio, list: &mut Vec<[u16; 3]>, count: usize) -> Result<()> {
    if s.reading() {
        list.clear();
        list.resize(count, [0; 3]);
    }
    for t in list.iter_mut() {
        s.u16(&mut t[0])?;
        s.u16(&mut t[1])?;
        s.u16(&mut t[2])?;
    }
    Ok(())
}

impl TypedBlock for BsTriShape {
    const TYPE_NAMES: &'static [&'static str] = &["BSTriShape"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        if !ctx.sse {
            // The block type does not exist at stream 83; refusing beats
            // decoding garbage with an SSE layout.
            return Err(Error::Corrupt {
                offset: 0,
                what: "BSTriShape is an SSE (bs100) block".into(),
            });
        }
        self.av.sync(s, ctx)?;
        self.bound.sync(s)?;
        s.block_ref(&mut self.skin)?;
        s.block_ref(&mut self.shader_property)?;
        s.block_ref(&mut self.alpha_property)?;
        s.u64(&mut self.vertex_desc)?;
        // u16 at stream 100; the u32 variant starts at stream 130 (FO4).
        s.u16(&mut self.num_triangles)?;
        s.u16(&mut self.num_vertices)?;
        s.u32(&mut self.data_size)?;

        if self.data_size > 0 {
            let attrs = self.attrs();
            if s.reading() {
                self.vertices.clear();
                self.vertices
                    .resize(self.num_vertices as usize, SseVertex::default());
            }
            for v in &mut self.vertices {
                Self::sync_vertex(v, s, attrs)?;
            }
            sync_tri_list(s, &mut self.triangles, self.num_triangles as usize)?;
        } else if s.reading() {
            self.vertices.clear();
            self.triangles.clear();
        }

        s.u32(&mut self.particle_data_size)?;
        if self.particle_data_size > 0 {
            let n = self.num_vertices as usize;
            sync_tri_list(s, &mut self.particle_vertices, n)?;
            sync_tri_list(s, &mut self.particle_normals, n)?;
            sync_tri_list(s, &mut self.particle_triangles, self.num_triangles as usize)?;
        } else if s.reading() {
            self.particle_vertices.clear();
            self.particle_normals.clear();
            self.particle_triangles.clear();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// LE: NiTriShape + NiTriShapeData
// ---------------------------------------------------------------------------

/// `NiTriShape` — the LE geometry node; the mesh itself lives in the data
/// block it references.
#[derive(Debug, Clone, PartialEq)]
pub struct NiTriShape {
    pub av: AvObject,
    pub data: BlockRef,
    pub skin_instance: BlockRef,
    pub material_names: Vec<StringIdx>,
    pub material_extra: Vec<i32>,
    pub active_material: i32,
    pub material_needs_update: bool,
    pub shader_property: BlockRef,
    pub alpha_property: BlockRef,
}

impl Default for NiTriShape {
    fn default() -> Self {
        NiTriShape {
            av: AvObject::default(),
            data: BlockRef::NONE,
            skin_instance: BlockRef::NONE,
            material_names: Vec::new(),
            material_extra: Vec::new(),
            active_material: -1,
            material_needs_update: false,
            shader_property: BlockRef::NONE,
            alpha_property: BlockRef::NONE,
        }
    }
}

impl TypedBlock for NiTriShape {
    // NiTriStrips is byte-identical at the shape level (both are
    // NiTriBasedGeom); the difference is the referenced data block —
    // NiTriStripsData instead of NiTriShapeData. 19 vanilla LE files
    // ship strips (the sigil stone, table02, the imperial sword GO...).
    const TYPE_NAMES: &'static [&'static str] = &["NiTriShape", "NiTriStrips"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.av.sync(s, ctx)?;
        s.block_ref(&mut self.data)?;
        s.block_ref(&mut self.skin_instance)?;
        // MaterialData at 20.2.0.7: parallel name/extra lists, one count.
        let mut n = self.material_names.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            self.material_names.clear();
            self.material_names.resize(n as usize, StringIdx::NONE);
            self.material_extra.clear();
            self.material_extra.resize(n as usize, -1);
        }
        for name in &mut self.material_names {
            s.string_idx(name)?;
        }
        for extra in &mut self.material_extra {
            s.i32(extra)?;
        }
        s.i32(&mut self.active_material)?;
        s.bool8(&mut self.material_needs_update)?;
        s.block_ref(&mut self.shader_property)?;
        s.block_ref(&mut self.alpha_property)
    }
}

/// `NiTriShapeData` — LE mesh data: parallel full-precision arrays.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiTriShapeData {
    pub group_id: i32,
    pub keep_flags: u8,
    pub compress_flags: u8,
    pub has_vertices: bool,
    pub vertices: Vec<[f32; 3]>,
    /// `BSGeometryDataFlags`: bit 0 = has UV, bits 12-15 = NBT (tangents).
    pub bs_data_flags: u16,
    pub material_crc: u32,
    pub has_normals: bool,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    pub bitangents: Vec<[f32; 3]>,
    pub bound: Bound,
    pub has_vertex_colors: bool,
    /// Full-float RGBA — 16 bytes per vertex, unlike SSE's packed 4.
    pub vertex_colors: Vec<[f32; 4]>,
    pub uv_sets: Vec<Vec<[f32; 2]>>,
    pub consistency_flags: u16,
    pub additional_data: BlockRef,
    pub num_triangle_points: u32,
    pub has_triangles: bool,
    pub triangles: Vec<[u16; 3]>,
    pub match_groups: Vec<Vec<u16>>,
}

impl NiTriShapeData {
    pub(crate) fn sync_vec3_list(
        s: &mut dyn Nio,
        list: &mut Vec<[f32; 3]>,
        present: bool,
        count: usize,
    ) -> Result<()> {
        if s.reading() {
            list.clear();
            if present {
                list.resize(count, [0.0; 3]);
            }
        }
        if present {
            for v in list.iter_mut() {
                s.f32s(v)?;
            }
        }
        Ok(())
    }
}

impl TypedBlock for NiTriShapeData {
    const TYPE_NAMES: &'static [&'static str] = &["NiTriShapeData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        // Group ID and the two flag bytes sit around the vertex count; a
        // reader that skips them misparses every LE file by 6 bytes.
        s.i32(&mut self.group_id)?;
        let mut num_vertices = self.vertices.len() as u16;
        s.u16(&mut num_vertices)?;
        let n = num_vertices as usize;
        s.u8(&mut self.keep_flags)?;
        s.u8(&mut self.compress_flags)?;

        s.bool8(&mut self.has_vertices)?;
        Self::sync_vec3_list(s, &mut self.vertices, self.has_vertices, n)?;

        s.u16(&mut self.bs_data_flags)?;
        s.u32(&mut self.material_crc)?;

        s.bool8(&mut self.has_normals)?;
        Self::sync_vec3_list(s, &mut self.normals, self.has_normals, n)?;
        // Tangent gate: nifly accepts any NBT bit (0xF000); vanilla files
        // set exactly 0x1000, so both readings agree on real data.
        let nbt = self.has_normals && (self.bs_data_flags & 0xF000) != 0;
        Self::sync_vec3_list(s, &mut self.tangents, nbt, n)?;
        Self::sync_vec3_list(s, &mut self.bitangents, nbt, n)?;

        self.bound.sync(s)?;

        s.bool8(&mut self.has_vertex_colors)?;
        if s.reading() {
            self.vertex_colors.clear();
            if self.has_vertex_colors {
                self.vertex_colors.resize(n, [0.0; 4]);
            }
        }
        if self.has_vertex_colors {
            for c in &mut self.vertex_colors {
                s.f32s(c)?;
            }
        }

        // UV set count comes from the flags word: bit 0 only at bs >= 34.
        let uv_count = (self.bs_data_flags & 1) as usize;
        if s.reading() {
            self.uv_sets.clear();
            self.uv_sets.resize(uv_count, Vec::new());
        }
        for set in &mut self.uv_sets {
            if s.reading() {
                set.clear();
                set.resize(n, [0.0; 2]);
            }
            for uv in set.iter_mut() {
                s.f32s(uv)?;
            }
        }

        s.u16(&mut self.consistency_flags)?;
        s.block_ref(&mut self.additional_data)?;

        // NiTriBasedGeomData adds the triangle count; then this block's own
        // lists follow.
        let mut num_triangles = self.triangles.len() as u16;
        s.u16(&mut num_triangles)?;
        s.u32(&mut self.num_triangle_points)?;
        s.bool8(&mut self.has_triangles)?;
        if self.has_triangles {
            sync_tri_list(s, &mut self.triangles, num_triangles as usize)?;
        } else if s.reading() {
            self.triangles.clear();
        }

        let mut groups = self.match_groups.len() as u16;
        s.u16(&mut groups)?;
        if s.reading() {
            self.match_groups.clear();
            self.match_groups.resize(groups as usize, Vec::new());
        }
        for group in &mut self.match_groups {
            let mut count = group.len() as u16;
            s.u16(&mut count)?;
            if s.reading() {
                group.clear();
                group.resize(count as usize, 0);
            }
            for index in group.iter_mut() {
                s.u16(index)?;
            }
        }
        Ok(())
    }
}

/// `BSLODTriShape` — NiTriShape plus three vertex-group sizes for
/// visibility control (nif.xml 8202: LOD0/1/2 Size). The geometry is an
/// ordinary NiTriShapeData; the three counts only say how much of it the
/// game shows at each distance, and a viewer draws the whole shape.
/// 2,458 blocks in the LE corpus (1,280 files) and 43 in SSE (DLC trees)
/// were dropped on the floor until this existed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsLodTriShape {
    pub shape: NiTriShape,
    pub lod0_size: u32,
    pub lod1_size: u32,
    pub lod2_size: u32,
}

impl TypedBlock for BsLodTriShape {
    const TYPE_NAMES: &'static [&'static str] = &["BSLODTriShape"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.shape.sync(s, ctx)?;
        s.u32(&mut self.lod0_size)?;
        s.u32(&mut self.lod1_size)?;
        s.u32(&mut self.lod2_size)
    }
}

/// `BSSegmentedTriShape` — NiTriShape plus the same per-cell segment
/// table [`BsSubIndexTriShape`] carries on the SSE side (nif.xml 6957).
/// This is what LE terrain LOD (`.btr`) is made of: 6,475 blocks across
/// 5,368 files, i.e. the whole LE horizon.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsSegmentedTriShape {
    pub shape: NiTriShape,
    pub segments: Vec<BsGeometrySegment>,
}

impl TypedBlock for BsSegmentedTriShape {
    const TYPE_NAMES: &'static [&'static str] = &["BSSegmentedTriShape"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.shape.sync(s, ctx)?;
        let mut count = self.segments.len() as u32;
        s.u32(&mut count)?;
        if s.reading() {
            self.segments.clear();
            self.segments
                .resize(count as usize, BsGeometrySegment::default());
        }
        for segment in &mut self.segments {
            s.u8(&mut segment.flags)?;
            s.u32(&mut segment.start_index)?;
            s.u32(&mut segment.num_primitives)?;
        }
        Ok(())
    }
}

/// `NiTriStripsData` — LE mesh data as triangle STRIPS: the same
/// NiTriBasedGeomData prefix as [`NiTriShapeData`], then per-strip point
/// lists instead of a triangle list. The stored triangle count is the
/// authored sum of `len-2` per strip; [`Self::unrolled_triangles`]
/// re-derives it (parity winding, degenerates skipped — NifSkope's
/// triangulateStrip rule).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiTriStripsData {
    pub group_id: i32,
    pub keep_flags: u8,
    pub compress_flags: u8,
    pub has_vertices: bool,
    pub vertices: Vec<[f32; 3]>,
    pub bs_data_flags: u16,
    pub material_crc: u32,
    pub has_normals: bool,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    pub bitangents: Vec<[f32; 3]>,
    pub bound: Bound,
    pub has_vertex_colors: bool,
    pub vertex_colors: Vec<[f32; 4]>,
    pub uv_sets: Vec<Vec<[f32; 2]>>,
    pub consistency_flags: u16,
    pub additional_data: BlockRef,
    /// NiTriBasedGeomData's authored triangle count (sum of len-2).
    pub num_triangles: u16,
    pub has_points: bool,
    pub strips: Vec<Vec<u16>>,
}

impl NiTriStripsData {
    /// Unroll the strips: triangle i of a strip is points (i, i+1, i+2),
    /// winding flipped on odd i, degenerate triangles (any repeated
    /// index) skipped — the reference viewers' rule.
    pub fn unrolled_triangles(&self) -> Vec<[u16; 3]> {
        let mut out = Vec::new();
        for strip in &self.strips {
            for i in 2..strip.len() {
                let (a, b, c) = (strip[i - 2], strip[i - 1], strip[i]);
                if a == b || b == c || a == c {
                    continue;
                }
                if i % 2 == 0 {
                    out.push([a, b, c]);
                } else {
                    out.push([b, a, c]);
                }
            }
        }
        out
    }
}

impl TypedBlock for NiTriStripsData {
    const TYPE_NAMES: &'static [&'static str] = &["NiTriStripsData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.i32(&mut self.group_id)?;
        let mut num_vertices = self.vertices.len() as u16;
        s.u16(&mut num_vertices)?;
        let n = num_vertices as usize;
        s.u8(&mut self.keep_flags)?;
        s.u8(&mut self.compress_flags)?;

        s.bool8(&mut self.has_vertices)?;
        NiTriShapeData::sync_vec3_list(s, &mut self.vertices, self.has_vertices, n)?;

        s.u16(&mut self.bs_data_flags)?;
        s.u32(&mut self.material_crc)?;

        s.bool8(&mut self.has_normals)?;
        NiTriShapeData::sync_vec3_list(s, &mut self.normals, self.has_normals, n)?;
        let nbt = self.has_normals && (self.bs_data_flags & 0xF000) != 0;
        NiTriShapeData::sync_vec3_list(s, &mut self.tangents, nbt, n)?;
        NiTriShapeData::sync_vec3_list(s, &mut self.bitangents, nbt, n)?;

        self.bound.sync(s)?;

        s.bool8(&mut self.has_vertex_colors)?;
        if s.reading() {
            self.vertex_colors.clear();
            if self.has_vertex_colors {
                self.vertex_colors.resize(n, [0.0; 4]);
            }
        }
        if self.has_vertex_colors {
            for c in &mut self.vertex_colors {
                s.f32s(c)?;
            }
        }

        let uv_count = (self.bs_data_flags & 1) as usize;
        if s.reading() {
            self.uv_sets.clear();
            self.uv_sets.resize(uv_count, Vec::new());
        }
        for set in &mut self.uv_sets {
            if s.reading() {
                set.clear();
                set.resize(n, [0.0; 2]);
            }
            for uv in set.iter_mut() {
                s.f32s(uv)?;
            }
        }

        s.u16(&mut self.consistency_flags)?;
        s.block_ref(&mut self.additional_data)?;
        s.u16(&mut self.num_triangles)?;

        let mut num_strips = self.strips.len() as u16;
        s.u16(&mut num_strips)?;
        // Strip lengths first, then Has Points, then all the point lists.
        let mut lengths: Vec<u16> = self.strips.iter().map(|st| st.len() as u16).collect();
        if s.reading() {
            lengths.clear();
            lengths.resize(num_strips as usize, 0);
        }
        for l in lengths.iter_mut() {
            s.u16(l)?;
        }
        s.bool8(&mut self.has_points)?;
        if s.reading() {
            self.strips.clear();
            self.strips
                .resize(num_strips as usize, Vec::new());
            if self.has_points {
                for (strip, l) in self.strips.iter_mut().zip(&lengths) {
                    strip.resize(*l as usize, 0);
                }
            }
        }
        if self.has_points {
            for strip in &mut self.strips {
                for p in strip.iter_mut() {
                    s.u16(p)?;
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Skinning: instance, data, partition
// ---------------------------------------------------------------------------
//
// Layouts per nif.xml 5067-5101 / 2143-2282, cross-checked against nifly
// Skin.cpp and byte-walked on vanilla files (multi-agent research pass,
// 2026-08-06; recorded in docs/knowledge/mesh-rendering.md §1.3). The
// SSE-specific rule: a skinned BSTriShape stores zero counts and the WHOLE
// vertex buffer lives on the NiSkinPartition; the partition's vertex count
// is never stored — it is Data Size / Vertex Size.

/// `NiTransform`: rotation rows + translation + uniform scale, 52 bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NifTransform {
    /// Row-major 3x3, same convention as `AvObject`.
    pub rotation: [f32; 9],
    pub translation: [f32; 3],
    pub scale: f32,
}

impl Default for NifTransform {
    fn default() -> Self {
        NifTransform {
            rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            translation: [0.0; 3],
            scale: 1.0,
        }
    }
}

impl NifTransform {
    pub fn sync(&mut self, s: &mut dyn Nio) -> Result<()> {
        s.f32s(&mut self.rotation)?;
        s.f32s(&mut self.translation)?;
        s.f32(&mut self.scale)
    }
}

/// `NiSkinInstance` — bone list + refs to skin data and partition.
///
/// Skeleton Root and the bone entries are weak Ptrs (usually ancestors of
/// the shape); they serialize as raw i32 like any ref.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiSkinInstance {
    pub data: BlockRef,
    pub skin_partition: BlockRef,
    pub skeleton_root: BlockRef,
    /// Order defines the bone indices the partitions and vertex data use.
    pub bones: Vec<BlockRef>,
}

impl TypedBlock for NiSkinInstance {
    const TYPE_NAMES: &'static [&'static str] = &["NiSkinInstance"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        s.block_ref(&mut self.data)?;
        s.block_ref(&mut self.skin_partition)?;
        s.block_ptr(&mut self.skeleton_root)?;
        s.ptr_list(&mut self.bones)
    }
}

/// One dismember-body-part entry: editor/boneset flags + body slot.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DismemberPartition {
    /// Bit 0 = editor-visible, bit 8 = starts a shared net boneset.
    pub part_flag: u16,
    /// `BSDismemberBodyPartType` (32 = body, 34 = hands, ...).
    pub body_part: u16,
}

/// `BSDismemberSkinInstance` — a NiSkinInstance plus one body-part entry
/// per skin partition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsDismemberSkinInstance {
    pub skin: NiSkinInstance,
    pub partitions: Vec<DismemberPartition>,
}

impl TypedBlock for BsDismemberSkinInstance {
    const TYPE_NAMES: &'static [&'static str] = &["BSDismemberSkinInstance"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.skin.sync(s, ctx)?;
        let mut n = self.partitions.len() as u32;
        s.u32(&mut n)?;
        if s.reading() {
            super::check_len(n)?;
            self.partitions.clear();
            self.partitions
                .resize(n as usize, DismemberPartition::default());
        }
        for p in &mut self.partitions {
            s.u16(&mut p.part_flag)?;
            s.u16(&mut p.body_part)?;
        }
        Ok(())
    }
}

/// Per-bone record inside `NiSkinData`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkinBoneData {
    /// Skin-CS -> bone-CS (nifly's `transformSkinToBone`); composed with
    /// the bone node's world transform to place skinned vertices.
    pub skin_to_bone: NifTransform,
    /// Center is in bone coordinates.
    pub bound: Bound,
    /// ALWAYS stored, even when the weight arrays are absent — vanilla
    /// files keep real counts with Has Vertex Weights = 0, and nifly-style
    /// zeroing breaks byte-identity.
    pub num_vertices: u16,
    /// (shape vertex index, weight); present only when the block's
    /// Has Vertex Weights byte is nonzero.
    pub weights: Vec<(u16, f32)>,
}

/// `NiSkinData` — the skin's transforms and (optionally) per-bone weights.
///
/// When a NiSkinPartition exists the references render from the partition
/// and ignore these weights entirely; they still must round-trip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiSkinData {
    /// Global-CS -> skin-CS.
    pub skin_transform: NifTransform,
    /// Raw byte, not forced to 0/1 — values above 1 exist in the wild
    /// (nifly clamps on read, which is exactly what we must not do).
    pub has_vertex_weights: u8,
    pub bones: Vec<SkinBoneData>,
}

impl TypedBlock for NiSkinData {
    const TYPE_NAMES: &'static [&'static str] = &["NiSkinData"];

    fn sync(&mut self, s: &mut dyn Nio, _ctx: Ctx) -> Result<()> {
        self.skin_transform.sync(s)?;
        let mut n = self.bones.len() as u32;
        s.u32(&mut n)?;
        s.u8(&mut self.has_vertex_weights)?;
        if s.reading() {
            super::check_len(n)?;
            self.bones.clear();
            self.bones.resize(n as usize, SkinBoneData::default());
        }
        for bone in &mut self.bones {
            bone.skin_to_bone.sync(s)?;
            bone.bound.sync(s)?;
            s.u16(&mut bone.num_vertices)?;
            if self.has_vertex_weights != 0 {
                if s.reading() {
                    bone.weights.clear();
                    bone.weights.resize(bone.num_vertices as usize, (0, 0.0));
                }
                for (index, weight) in &mut bone.weights {
                    s.u16(index)?;
                    s.f32(weight)?;
                }
            } else if s.reading() {
                bone.weights.clear();
            }
        }
        Ok(())
    }
}

/// One partition inside `NiSkinPartition`.
///
/// Presence gates are raw bytes (same non-normalized-bool caution as
/// everywhere else at this version), and the count fields stay explicit
/// because counts can be stored while their arrays are absent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkinPartitionEntry {
    pub num_vertices: u16,
    /// When stripped this is Σ(strip len − 2); the Triangles array is
    /// present only in the list (non-strip) case.
    pub num_triangles: u16,
    pub num_weights_per_vertex: u16,
    /// Indices into the NiSkinInstance bone array.
    pub bones: Vec<u16>,
    pub has_vertex_map: u8,
    /// Partition-local vertex -> shape vertex.
    pub vertex_map: Vec<u16>,
    pub has_vertex_weights: u8,
    /// `num_vertices × num_weights_per_vertex` floats.
    pub vertex_weights: Vec<f32>,
    pub strip_lengths: Vec<u16>,
    pub has_faces: u8,
    pub strips: Vec<Vec<u16>>,
    /// Indices into the Vertex Map (partition-local), NOT shape vertices.
    pub triangles: Vec<[u16; 3]>,
    pub has_bone_indices: u8,
    /// `num_vertices × num_weights_per_vertex` bytes; each indexes the
    /// partition's own `bones` list (LE). The SSE vertex buffer's indices
    /// are instance-global instead.
    pub bone_indices: Vec<u8>,
    pub lod_level: u8,
    pub global_vb: u8,
    /// bs100 only; equals the block-level desc in every observed file.
    pub vertex_desc: u64,
    /// bs100 only: the same faces with SHAPE-vertex indices ("true
    /// triangles") — what the renderer actually consumes.
    pub true_triangles: Vec<[u16; 3]>,
}

/// `NiSkinPartition` — bone partitions, and on SSE the skinned shape's
/// entire vertex buffer (the BSTriShape itself stores zeros).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiSkinPartition {
    /// bs100 header trio; the vertex count is `data_size / vertex_size`,
    /// never stored.
    pub data_size: u32,
    pub vertex_size: u32,
    pub vertex_desc: u64,
    pub vertices: Vec<SseVertex>,
    pub partitions: Vec<SkinPartitionEntry>,
}

impl NiSkinPartition {
    fn sync_partition(p: &mut SkinPartitionEntry, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        s.u16(&mut p.num_vertices)?;
        s.u16(&mut p.num_triangles)?;
        let mut num_bones = p.bones.len() as u16;
        s.u16(&mut num_bones)?;
        let mut num_strips = p.strip_lengths.len() as u16;
        s.u16(&mut num_strips)?;
        s.u16(&mut p.num_weights_per_vertex)?;
        if s.reading() {
            p.bones.clear();
            p.bones.resize(num_bones as usize, 0);
        }
        for b in &mut p.bones {
            s.u16(b)?;
        }

        let nv = p.num_vertices as usize;
        let wpv = p.num_weights_per_vertex as usize;
        s.u8(&mut p.has_vertex_map)?;
        if p.has_vertex_map != 0 {
            if s.reading() {
                p.vertex_map.clear();
                p.vertex_map.resize(nv, 0);
            }
            for v in &mut p.vertex_map {
                s.u16(v)?;
            }
        } else if s.reading() {
            p.vertex_map.clear();
        }
        s.u8(&mut p.has_vertex_weights)?;
        if p.has_vertex_weights != 0 {
            if s.reading() {
                p.vertex_weights.clear();
                p.vertex_weights.resize(nv * wpv, 0.0);
            }
            for w in &mut p.vertex_weights {
                s.f32(w)?;
            }
        } else if s.reading() {
            p.vertex_weights.clear();
        }

        if s.reading() {
            p.strip_lengths.clear();
            p.strip_lengths.resize(num_strips as usize, 0);
        }
        for len in &mut p.strip_lengths {
            s.u16(len)?;
        }
        s.u8(&mut p.has_faces)?;
        if p.has_faces != 0 && num_strips != 0 {
            if s.reading() {
                p.strips.clear();
                p.strips.resize(num_strips as usize, Vec::new());
            }
            for (strip, len) in p.strips.iter_mut().zip(p.strip_lengths.iter()) {
                if s.reading() {
                    strip.clear();
                    strip.resize(*len as usize, 0);
                }
                for v in strip.iter_mut() {
                    s.u16(v)?;
                }
            }
        } else if s.reading() {
            p.strips.clear();
        }
        if p.has_faces != 0 && num_strips == 0 {
            sync_tri_list(s, &mut p.triangles, p.num_triangles as usize)?;
        } else if s.reading() {
            p.triangles.clear();
        }

        s.u8(&mut p.has_bone_indices)?;
        if p.has_bone_indices != 0 {
            if s.reading() {
                p.bone_indices.clear();
                p.bone_indices.resize(nv * wpv, 0);
            }
            for b in &mut p.bone_indices {
                s.u8(b)?;
            }
        } else if s.reading() {
            p.bone_indices.clear();
        }

        // The user-12 trailer exists on BOTH streams (83 and 100); only
        // the desc + true triangles are SSE-only. Skipping the first two
        // on LE truncates every partition after the first.
        s.u8(&mut p.lod_level)?;
        s.u8(&mut p.global_vb)?;
        if ctx.sse {
            s.u64(&mut p.vertex_desc)?;
            sync_tri_list(s, &mut p.true_triangles, p.num_triangles as usize)?;
        } else if s.reading() {
            p.vertex_desc = 0;
            p.true_triangles.clear();
        }
        Ok(())
    }
}

impl TypedBlock for NiSkinPartition {
    const TYPE_NAMES: &'static [&'static str] = &["NiSkinPartition"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        let mut num_partitions = self.partitions.len() as u32;
        s.u32(&mut num_partitions)?;
        if ctx.sse {
            s.u32(&mut self.data_size)?;
            s.u32(&mut self.vertex_size)?;
            s.u64(&mut self.vertex_desc)?;
            if self.data_size > 0 {
                if self.vertex_size == 0 || self.data_size % self.vertex_size != 0 {
                    return Err(Error::Corrupt {
                        offset: 0,
                        what: format!(
                            "NiSkinPartition data size {} is not a multiple of vertex size {}",
                            self.data_size, self.vertex_size
                        ),
                    });
                }
                let count = (self.data_size / self.vertex_size) as usize;
                let attrs = self.vertex_desc >> 44;
                if s.reading() {
                    super::check_len(count as u32)?;
                    self.vertices.clear();
                    self.vertices.resize(count, SseVertex::default());
                }
                for v in &mut self.vertices {
                    BsTriShape::sync_vertex(v, s, attrs)?;
                }
            } else if s.reading() {
                self.vertices.clear();
            }
        } else if s.reading() {
            self.data_size = 0;
            self.vertex_size = 0;
            self.vertex_desc = 0;
            self.vertices.clear();
        }
        if s.reading() {
            super::check_len(num_partitions)?;
            self.partitions.clear();
            self.partitions
                .resize(num_partitions as usize, SkinPartitionEntry::default());
        }
        for p in &mut self.partitions {
            Self::sync_partition(p, s, ctx)?;
        }
        Ok(())
    }
}

/// `BSDynamicTriShape` — a BSTriShape whose POSITIONS live on the shape as
/// full Vector4s (facegen heads morph these at runtime); UVs, normals and
/// skinning stay in the skin partition's buffer. Its stored `num_vertices`
/// is real (unlike plain skinned BSTriShapes, which zero it).
/// One run of triangles inside a [`BsSubIndexTriShape`].
///
/// Object LOD uses these to swap between the baked LOD copy of a cell
/// and the real references: a LOD4 tile covers 4x4 cells and carries up
/// to 16 segments, one per cell, so the game can drop exactly the
/// triangles of a cell it has just attached.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsGeometrySegment {
    pub flags: u8,
    pub start_index: u32,
    pub num_primitives: u32,
}

/// `BSTriShape` plus a per-cell segment table — the shape every object
/// LOD (`.bto`) file is built from, and the ONLY block that stood
/// between Aether and the game's own distant geometry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsSubIndexTriShape {
    pub shape: BsTriShape,
    pub segments: Vec<BsGeometrySegment>,
}

impl TypedBlock for BsSubIndexTriShape {
    const TYPE_NAMES: &'static [&'static str] = &["BSSubIndexTriShape"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.shape.sync(s, ctx)?;
        // The SSE branch of nif.xml: a plain count, then 9 bytes each.
        // (Fallout 4's form carries a second, richer table; nothing in
        // a bs100 file has one, and a block that did would fail the
        // consume-everything rule rather than be silently misread.)
        let mut count = self.segments.len() as u32;
        s.u32(&mut count)?;
        if s.reading() {
            super::check_len(count)?;
            self.segments.clear();
            self.segments
                .resize(count as usize, BsGeometrySegment::default());
        }
        for segment in &mut self.segments {
            s.u8(&mut segment.flags)?;
            s.u32(&mut segment.start_index)?;
            s.u32(&mut segment.num_primitives)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BsDynamicTriShape {
    pub shape: BsTriShape,
    pub dynamic_data_size: u32,
    /// xyz = position, w = bitangent X.
    pub dynamic: Vec<[f32; 4]>,
}

impl TypedBlock for BsDynamicTriShape {
    const TYPE_NAMES: &'static [&'static str] = &["BSDynamicTriShape"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.shape.sync(s, ctx)?;
        s.u32(&mut self.dynamic_data_size)?;
        if s.reading() {
            let count = (self.dynamic_data_size / 16) as usize;
            super::check_len(count as u32)?;
            self.dynamic.clear();
            self.dynamic.resize(count, [0.0; 4]);
        }
        for v in &mut self.dynamic {
            s.f32s(v)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Packed-value decoding
// ---------------------------------------------------------------------------

/// IEEE 754 binary16 -> f32, std-only (no `f16` in stable Rust).
pub fn half_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 1) as u32;
    let exponent = ((bits >> 10) & 0x1F) as u32;
    let mantissa = (bits & 0x3FF) as u32;
    let f = match (exponent, mantissa) {
        (0, 0) => sign << 31,
        (0, m) => {
            // Subnormal: value is m * 2^-24, which always normalizes in
            // f32's larger exponent range. The highest set bit becomes the
            // implicit leading 1: exponent 127 - 24 + h, the rest shifts up
            // into the 23-bit mantissa.
            let h = 31 - m.leading_zeros();
            let e = 103 + h;
            let mantissa = (m << (23 - h)) & 0x7F_FFFF;
            (sign << 31) | (e << 23) | mantissa
        }
        (0x1F, 0) => (sign << 31) | 0x7F80_0000,
        (0x1F, m) => (sign << 31) | 0x7F80_0000 | (m << 13),
        (e, m) => (sign << 31) | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(f)
}

/// Bias-packed unsigned byte -> [-1, 1]. Not a signed byte, not 127-centred:
/// `v/255*2 - 1` is what both nifly and NifSkope implement and what nif.xml
/// states.
pub fn unpack_norm_byte(v: u8) -> f32 {
    v as f32 / 255.0 * 2.0 - 1.0
}

/// Hang extra children off a scene container, whatever kind it is.
///
/// Every container in the format is an `NiNode` with a tail of its own
/// fields, so this is one match over the wrappers rather than nine
/// call sites that each remember to reach through `.node`. Returns
/// `None` for a block that is not a container — the caller must not
/// guess, because writing an `NiNode` over a `bhkCollisionObject` would
/// re-encode it from a layout that is not its own.
pub fn adopt_children(
    block: &crate::file::Block,
    ctx: Ctx,
    extra: &[BlockRef],
) -> Option<crate::file::Block> {
    macro_rules! wrap {
        ($t:ty) => {{
            let mut v = <$t>::decode(block, ctx).ok()?;
            v.node.children.extend_from_slice(extra);
            return Some(v.encode_as(&block.type_name, ctx));
        }};
    }
    let name = block.type_name.as_str();
    if NiNode::TYPE_NAMES.contains(&name) {
        let mut v = NiNode::decode(block, ctx).ok()?;
        v.children.extend_from_slice(extra);
        return Some(v.encode_as(name, ctx));
    }
    if BsMasterParticleSystem::TYPE_NAMES.contains(&name) {
        wrap!(BsMasterParticleSystem)
    }
    if BsOrderedNode::TYPE_NAMES.contains(&name) {
        wrap!(BsOrderedNode)
    }
    if BsValueNode::TYPE_NAMES.contains(&name) {
        wrap!(BsValueNode)
    }
    if NiSwitchNode::TYPE_NAMES.contains(&name) {
        wrap!(NiSwitchNode)
    }
    if BsRangeNode::TYPE_NAMES.contains(&name) {
        wrap!(BsRangeNode)
    }
    if BsMultiBoundNode::TYPE_NAMES.contains(&name) {
        wrap!(BsMultiBoundNode)
    }
    if BsTreeNode::TYPE_NAMES.contains(&name) {
        wrap!(BsTreeNode)
    }
    if NiBillboardNode::TYPE_NAMES.contains(&name) {
        wrap!(NiBillboardNode)
    }
    None
}

/// `NiCamera` — frustum, viewport and a scene Ref. The two trailing
/// counts are deprecated and always written zero, but they are STORED,
/// so they are kept for the file where one is not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NiCamera {
    pub av: AvObject,
    pub camera_flags: u16,
    /// left, right, top, bottom, near, far.
    pub frustum: [f32; 6],
    pub orthographic: bool,
    /// left, right, top, bottom.
    pub viewport: [f32; 4],
    pub lod_adjust: f32,
    pub scene: BlockRef,
    pub num_screen_polygons: u32,
    pub num_screen_textures: u32,
}

impl TypedBlock for NiCamera {
    const TYPE_NAMES: &'static [&'static str] = &["NiCamera"];

    fn sync(&mut self, s: &mut dyn Nio, ctx: Ctx) -> Result<()> {
        self.av.sync(s, ctx)?;
        s.u16(&mut self.camera_flags)?;
        s.f32s(&mut self.frustum)?;
        s.bool8(&mut self.orthographic)?;
        s.f32s(&mut self.viewport)?;
        s.f32(&mut self.lod_adjust)?;
        s.block_ref(&mut self.scene)?;
        s.u32(&mut self.num_screen_polygons)?;
        s.u32(&mut self.num_screen_textures)
    }
}
