//! Extract renderable meshes from a parsed NIF.
//!
//! Walks the scene graph from the file's roots, accumulates transforms, and
//! flattens every triangle shape into a `RenderMesh` the viewport can upload
//! once and draw forever — heavy meshes are the norm, so nothing here is
//! per-frame work.
//!
//! Semantics follow `docs/knowledge/mesh-rendering.md` (verified against
//! NifSkope, nifly and BodySlide):
//! - the 9 rotation floats are matrix ROWS under column-vector math — loading
//!   them as columns inverts every rotation (§2.4);
//! - composition is `R = Rp*Rc`, `T = Tp + Rp*(Tc*sp)`, `s = sp*sc` (§2.1);
//! - hidden is bit 0 of the AVObject flags and inherits down (§2.3);
//! - NIF data is Z-up and used verbatim, 1:1 — BodySlide's axis remap is its
//!   own display convention, not format semantics (§2.5);
//! - skinned shapes carry no geometry on the shape block (SSE) or build from
//!   bone transforms (LE); both are skipped with a warning for now (§1.3).

use aether_nif::typed::geometry::{BsTriShape, NiNode, NiTriShape, NiTriShapeData};
use aether_nif::typed::shaders::{
    BSEffectShaderProperty, BSLightingShaderProperty, BSShaderTextureSet, NiAlphaProperty,
};
use aether_nif::typed::{BlockRef, Ctx, TypedBlock};
use aether_nif::NifFile;

/// One vertex in the layout the GPU pipeline consumes.
///
/// `repr(C)` is load-bearing, not decoration: the renderer uploads whole
/// vertex slices as raw bytes rather than writing twelve little-endian
/// floats per vertex, and that cast is only sound because the layout is
/// declared. Twelve `f32` and nothing else means no padding, no invalid
/// bit patterns, and exactly the 48-byte stride the pipeline declares —
/// keep it that way, and if a field is ever added, add it as `f32` and
/// widen the stride in `render::gpu` in the same change.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Straight RGBA vertex color; white when the mesh stores none.
    pub color: [f32; 4],
}

/// How a mesh wants to be composited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlendMode {
    Opaque,
    /// Ordinary src-alpha blending (premultiplied on upload).
    Alpha,
    /// `src + dst`; falls out of premultiplied blending with alpha 0.
    Additive,
    /// `dst x src.rgb` — the game's ZERO/SRC_COLOR factor pair
    /// (NiAlphaProperty bits 0x0043, 926 SSE + 366 LE blocks): dirt and
    /// shadow decals that DARKEN what they lie on. Drawn as plain alpha
    /// they washed out instead.
    Multiply,
}

/// A flattened, world-space-transformed drawable shape.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderMesh {
    pub name: String,
    /// Column-major 4x4, ready for a GPU uniform.
    pub world: [f32; 16],
    /// The scene node this shape hangs off, indexing `MeshScene::nodes`.
    pub node: Option<usize>,
    /// The block this shape was extracted from, indexing the blocks of
    /// the file it came from — which is THIS tab's file for a NIF scene
    /// and the cached model's file in a world scene, never a scene-wide
    /// id. `None` for geometry no block produced: strip trails, terrain
    /// quadrants, generated lightning.
    ///
    /// Per MODEL, not per placement: every placement of a shape clones
    /// one cached `RenderMesh`, so all of them carry the same value and
    /// it can never split an instanced batch. Never add it to `DrawKey`.
    pub block: Option<usize>,
    /// The shape's own local transform under that node.
    pub local: Transform,
    /// The shape names NO shader property, so it has no material.
    ///
    /// The game creates no render pass for such geometry; Aether bound
    /// the white fallback and drew a white sheet. Kept in the scene
    /// like an editor marker (picking, framing and the block panel
    /// still want it) and skipped at upload.
    pub no_material: bool,
    /// SLSF2 bit 0 (ZBuffer_Write) — clear means the shape draws
    /// WITHOUT writing depth, so what stands behind it still shows
    /// through. Both references honour it (NifSkope
    /// glproperty.cpp:1160/1497 -> renderer.cpp:1062); Aether drew
    /// 5,186 SSE lighting blocks in 3,852 files through the
    /// depth-writing opaque pipeline regardless. Only the OPAQUE path
    /// cares: blended draws never write depth anyway.
    pub depth_write: bool,
    /// Alpha-ordering group: the block index of the BSOrderedNode
    /// this shape hangs under, or 0 for none.
    ///
    /// A BSOrderedNode overrides `OnVisible` (1.5.97 vtable slot
    /// 0x34) to submit its children in AUTHORED order rather than by
    /// depth — that is the whole point of the type, and it is how a
    /// stack of blended cards is layered when their depths are equal
    /// or nearly so. Depth-sorting them ignores the author. The
    /// SEQUENCE inside a group needs no field: it is the mesh's
    /// position in `MeshScene::meshes`, which is file order.
    pub order_group: usize,
    /// Shared with every other placement of this shape.
    ///
    /// `Arc` and not `Vec` because a world block clones one model's shapes
    /// once per placement: at Riverwood radius 6 that was **362 MB of
    /// memcpy per rebuild**, 169 ms of the 171 ms a re-centre cost, to
    /// produce copies the GPU never sees — instanced batching uploads one
    /// representative per shape. Cloning a `RenderMesh` is a refcount bump
    /// now. It also makes `RenderMesh` `Send`, which cell assembly on a
    /// worker thread will need.
    pub vertices: std::sync::Arc<Vec<MeshVertex>>,
    /// CCW front-facing (§1.6).
    pub indices: std::sync::Arc<Vec<u16>>,
    /// `(start_index, num_primitives)` per `BSSubIndexTriShape` segment,
    /// empty for every other shape.
    ///
    /// This is the object-LOD handover: each segment is a contiguous run
    /// of the shape's OWN index buffer covering one cell, so dropping
    /// the cells that are loaded at full detail is issuing the surviving
    /// runs — no buffer rewrite, no second upload. It is why object LOD
    /// carries a segment table and terrain LOD does not: terrain is sunk
    /// by its vertex shader instead.
    pub segments: Vec<(u32, u32)>,
    /// Game-relative diffuse (lighting) or source (effect) texture path.
    pub texture: String,
    /// Multiplied over texture and vertex color. Effects carry their base
    /// color and scale here; lighting shapes carry material alpha.
    pub tint: [f32; 4],
    pub blend: BlendMode,
    /// Alpha-test threshold in 0..1, testing greater-than, when enabled.
    pub alpha_test: Option<f32>,
    pub double_sided: bool,
    /// Effect shader (glow geometry) vs lighting shader (lit mesh).
    pub effect: bool,
    /// Creation-Kit editor geometry (the shape name contains
    /// "EditorMarker"): kept for picking and framing — a marker is
    /// sometimes the only geometry a placed reference has — but never
    /// drawn, which is what the game and NifSkope both do. 2,308 SSE
    /// meshes in 2,006 files were showing up as white planes.
    pub marker: bool,
    /// SLSF1 bit 15: drawn ONLY by the refraction-normals pass. The game
    /// routes such geometry OUT of the colour pass entirely
    /// (RE 0x1412c6834: the `flags & 0x8004` branch skips colour-pass
    /// creation) — its distortion is the whole contribution, and
    /// `texture` carries the NORMAL map that distortion samples.
    pub refraction: bool,
    /// Bit 0 of the AVObject flags, own or inherited. The game does not
    /// draw these — but mesh emitters sample them, which is exactly what
    /// the Zodiac swords' EmitSphere shapes are for. Extracted, flagged,
    /// never rendered.
    pub hidden: bool,
    /// Effect-shader material extras (identity/empty for lighting shapes).
    pub material: EffectMaterial,
    /// Lighting-shader inputs (defaults for effect shapes).
    pub lighting: LightingMaterial,
    /// Free-running float controllers on the shader, evaluated per frame.
    pub shader_anims: Vec<ShaderAnim>,
    /// Free-running colour controllers, same clock.
    pub shader_color_anims: Vec<ShaderColorAnim>,
}

/// The parts of `BSEffectShaderProperty` beyond a texture and a tint that
/// change how a shape reads on screen. Semantics follow NifSkope's
/// `sk_effectshader.frag` (the only reference that renders these).
#[derive(Debug, Clone, PartialEq)]
pub struct EffectMaterial {
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    /// Preview water (BSWaterShaderProperty): bit 0 = is water, bit 1 =
    /// VERTEX_UV (mesh UV x1000 for the scroll layers), bit 2 =
    /// VERTEX_ALPHA_DEPTH (vertex alpha drives the shallow/deep fade).
    /// The wave/colour constants are DefaultWater, baked in scene.wgsl.
    pub water: u8,
    /// Greyscale-to-palette ramp texture; empty = feature off.
    pub greyscale_texture: String,
    /// SLSF1 bit 4: colour from `ramp(base.G)`.
    pub greyscale_color: bool,
    /// SLSF1 bit 5: alpha from `ramp(base.a)`.
    pub greyscale_alpha: bool,
    /// SLSF1 bit 6: view-angle falloff enabled.
    pub use_falloff: bool,
    /// (start angle, stop angle, start opacity, stop opacity).
    pub falloff: [f32; 4],
    /// `Base Color` as authored (not premultiplied by scale).
    pub base_color: [f32; 4],
    pub base_color_scale: f32,
    /// SLSF1 Soft_Effect depth: fragments fade in over this many units of
    /// separation from the opaque scene behind them (0 = off).
    pub soft_depth: f32,
}

impl Default for EffectMaterial {
    fn default() -> Self {
        EffectMaterial {
            uv_offset: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            water: 0,
            greyscale_texture: String::new(),
            greyscale_color: false,
            greyscale_alpha: false,
            use_falloff: false,
            falloff: [0.0; 4],
            base_color: [1.0; 4],
            base_color_scale: 1.0,
            soft_depth: 0.0,
        }
    }
}

/// Which controller family a float track belongs to. The variable
/// NUMBERS collide between the effect and lighting enums (5 is alpha on
/// one, undefined on the other), so the kind is not decoration — losing
/// it would animate the wrong knob with no error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderAnimKind {
    /// `BSEffectShaderPropertyFloatController`.
    Effect,
    /// `BSLightingShaderPropertyFloatController`.
    LightingFloat,
    /// `BSNiAlphaPropertyTestRefController` — the track is in RAW 0-255
    /// units (RE 0x141322C80: trunc(clamp(v, 0, 255))).
    AlphaThreshold,
}

/// One float shader controller on a shape, running free (not
/// sequence-driven): keyed values over controller time.
#[derive(Debug, Clone, PartialEq)]
pub struct ShaderAnim {
    pub kind: ShaderAnimKind,
    /// Effect: 0 emissive multiple, 5 alpha, 6-9 UV. LightingFloat:
    /// 0 refraction (inert, NifSkope parity), 8 env scale, 9 gloss,
    /// 10 spec strength, 11 emissive mult, 12 alpha, 20-23 UV.
    pub variable: u32,
    pub keys: crate::curve::FloatCurve,
    pub start_time: f32,
    pub stop_time: f32,
    pub frequency: f32,
    /// Cycle bits 1-2 of the controller flags: 0 = loop.
    pub looping: bool,
}

impl ShaderAnim {
    /// Sample at wall-clock `time` seconds, the game's controller mapping.
    pub fn sample(&self, time: f32) -> Option<f32> {
        let t = crate::sequence::controller_time(
            self.start_time + time * self.frequency,
            self.start_time,
            self.stop_time,
            self.looping,
        );
        self.keys.sample(t)
    }
}

/// One colour shader controller running free: rgb keys over controller
/// time. 0 = specular, 1 = emissive (`BSLightingShaderPropertyColorController`),
/// and the effect shader's single colour 0 = base colour rgb.
#[derive(Debug, Clone, PartialEq)]
pub struct ShaderColorAnim {
    /// True for the effect shader's controller (writes base colour);
    /// false for the lighting one (0 specular / 1 emissive).
    pub effect: bool,
    pub variable: u32,
    /// The whole stored curve, key type included (see `TrackKeys::Color`).
    pub keys: crate::curve::Curve,
    pub start_time: f32,
    pub stop_time: f32,
    pub frequency: f32,
    pub looping: bool,
}

impl ShaderColorAnim {
    pub fn sample(&self, time: f32) -> Option<[f32; 3]> {
        let t = crate::sequence::controller_time(
            self.start_time + time * self.frequency,
            self.start_time,
            self.stop_time,
            self.looping,
        );
        crate::sequence::sample_color(&self.keys, t)
    }
}

/// Local {rotation rows, translation, uniform scale}, composed the way both
/// reference implementations do it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Row-major: rows[r*3 + c], applied as `out_r = row_r . v`.
    pub rot: [f32; 9],
    pub t: [f32; 3],
    pub s: f32,
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        rot: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        t: [0.0; 3],
        s: 1.0,
    };

    pub fn from_av(translation: [f32; 3], rotation: [f32; 9], scale: f32) -> Transform {
        Transform {
            rot: rotation,
            t: translation,
            s: scale,
        }
    }

    /// Rotation rows from X/Y/Z euler radians, composed `Rx*Ry*Rz` — the
    /// same convention NifSkope's `Matrix::fromEuler` uses (verified
    /// term-by-term for the transform-track work).
    pub fn rows_from_euler(x: f32, y: f32, z: f32) -> [f32; 9] {
        let (sx, cx) = x.sin_cos();
        let (sy, cy) = y.sin_cos();
        let (sz, cz) = z.sin_cos();
        [
            cy * cz,
            -cy * sz,
            sy,
            sx * sy * cz + sz * cx,
            cx * cz - sx * sy * sz,
            -sx * cy,
            sx * sz - cx * sy * cz,
            cx * sy * sz + sx * cz,
            cx * cy,
        ]
    }

    /// The inverse of [`Self::rows_from_euler`], NifSkope's `toEuler`.
    /// Returns X/Y/Z radians; gimbal-locked inputs collapse into x.
    pub fn euler_from_rows(m: &[f32; 9]) -> [f32; 3] {
        if m[2] < 1.0 && m[2] > -1.0 {
            [(-m[5]).atan2(m[8]), m[2].asin(), (-m[1]).atan2(m[0])]
        } else {
            [m[3].atan2(m[4]), if m[2] >= 1.0 { std::f32::consts::FRAC_PI_2 } else { -std::f32::consts::FRAC_PI_2 }, 0.0]
        }
    }

    /// Rotation rows from a unit quaternion (wxyz) — the standard matrix,
    /// rows applied as `out_r = row_r . v`.
    pub fn rows_from_quat(q: [f32; 4]) -> [f32; 9] {
        let [w, x, y, z] = q;
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ]
    }

    /// This transform with sampled channels swapped in; `None` channels
    /// keep their authored values.
    pub fn with_sample(&self, sample: &crate::sequence::TransformSample) -> Transform {
        Transform {
            rot: sample
                .rotation
                .map(Transform::rows_from_quat)
                .unwrap_or(self.rot),
            t: sample.translation.unwrap_or(self.t),
            s: sample.scale.unwrap_or(self.s),
        }
    }

    pub fn rotate(&self, v: [f32; 3]) -> [f32; 3] {
        let r = &self.rot;
        [
            r[0] * v[0] + r[1] * v[1] + r[2] * v[2],
            r[3] * v[0] + r[4] * v[1] + r[5] * v[2],
            r[6] * v[0] + r[7] * v[1] + r[8] * v[2],
        ]
    }

    /// Rotate by the INVERSE: `rot` is a pure rotation (scale lives in
    /// `s`), so the transpose is the inverse. This is the unbake half
    /// of [`Self::rotate`] — what brings a system-space axis back
    /// into an anchor's local frame at export.
    pub fn unrotate(&self, v: [f32; 3]) -> [f32; 3] {
        let r = &self.rot;
        [
            r[0] * v[0] + r[3] * v[1] + r[6] * v[2],
            r[1] * v[0] + r[4] * v[1] + r[7] * v[2],
            r[2] * v[0] + r[5] * v[1] + r[8] * v[2],
        ]
    }

    /// Apply the full transform to a point: `R·(v·s) + t`.
    pub fn transform_point(&self, v: [f32; 3]) -> [f32; 3] {
        let scaled = [v[0] * self.s, v[1] * self.s, v[2] * self.s];
        let rotated = self.rotate(scaled);
        [
            rotated[0] + self.t[0],
            rotated[1] + self.t[1],
            rotated[2] + self.t[2],
        ]
    }

    /// The inverse rigid+uniform-scale transform: `Rᵀ, 1/s, −(Rᵀt)/s` —
    /// NiTransform::Invert's exact formula (1.5.97 @ 0x14039A980).
    pub fn inverse(&self) -> Transform {
        let r = &self.rot;
        let rot = [r[0], r[3], r[6], r[1], r[4], r[7], r[2], r[5], r[8]];
        let s = if self.s.abs() > 1e-9 { 1.0 / self.s } else { 1.0 };
        let t = [
            -(rot[0] * self.t[0] + rot[1] * self.t[1] + rot[2] * self.t[2]) * s,
            -(rot[3] * self.t[0] + rot[4] * self.t[1] + rot[5] * self.t[2]) * s,
            -(rot[6] * self.t[0] + rot[7] * self.t[1] + rot[8] * self.t[2]) * s,
        ];
        Transform { rot, t, s }
    }

    /// `R = Rp*Rc; T = Tp + Rp*(Tc*sp); s = sp*sc` — child-into-parent.
    pub fn compose(&self, child: &Transform) -> Transform {
        let p = &self.rot;
        let c = &child.rot;
        let mut rot = [0.0f32; 9];
        for row in 0..3 {
            for col in 0..3 {
                rot[row * 3 + col] = p[row * 3] * c[col]
                    + p[row * 3 + 1] * c[3 + col]
                    + p[row * 3 + 2] * c[6 + col];
            }
        }
        let scaled = [child.t[0] * self.s, child.t[1] * self.s, child.t[2] * self.s];
        let rotated = self.rotate(scaled);
        Transform {
            rot,
            t: [
                self.t[0] + rotated[0],
                self.t[1] + rotated[1],
                self.t[2] + rotated[2],
            ],
            s: self.s * child.s,
        }
    }

    /// Column-major 4x4 for the GPU: `m[col*4 + row]`. The stored rows go
    /// into the matrix as rows, which lands them transposed in column-major
    /// memory order.
    pub fn to_column_major(&self) -> [f32; 16] {
        let r = &self.rot;
        let s = self.s;
        [
            r[0] * s, r[3] * s, r[6] * s, 0.0, // column 0
            r[1] * s, r[4] * s, r[7] * s, 0.0, // column 1
            r[2] * s, r[5] * s, r[8] * s, 0.0, // column 2
            self.t[0], self.t[1], self.t[2], 1.0,
        ]
    }

    /// Apply a world-space delta about pivot `q` and return the block's
    /// new LOCAL transform.
    ///
    /// ONE solve for move, rotate and scale, because three formulas is
    /// three places for the pivot term to be dropped — and dropping it
    /// is invisible until the pivot is not the object's own origin, at
    /// which point a rotation also translates.
    ///
    /// `core` is the delta expressed about the ORIGIN: `{I, dt, 1}` to
    /// move, `{dR, 0, 1}` to turn, `{I, 0, k}` to scale. Conjugating it
    /// by the pivot translation gives the world delta, and
    /// `parent^-1 . delta . world` gives the local that produces it.
    ///
    /// Order matters and is checked, not assumed: `P.compose(&C)` is
    /// `P(C(v))` — the RECEIVER is the parent. A flipped order still
    /// puts the root in the right place, which is why it passes on any
    /// one-node fixture and fails everywhere below it.
    pub fn edit_local(
        parent: &Transform,
        world: &Transform,
        q: [f32; 3],
        core: &Transform,
    ) -> Transform {
        let pivot = Transform {
            rot: Transform::IDENTITY.rot,
            t: q,
            s: 1.0,
        };
        let delta = pivot.compose(core).compose(&pivot.inverse());
        parent.inverse().compose(&delta.compose(world))
    }

    /// The inverse of `to_column_major`, for reading back a matrix the
    /// renderer already baked.
    ///
    /// The scale is recovered as the length of the first column, which
    /// is exact because the format stores ONE uniform scale — a
    /// non-uniform matrix cannot come out of `Transform` in the first
    /// place. A degenerate column falls back to 1.0 rather than
    /// dividing by zero and filling the rotation with NaN.
    pub fn from_column_major(m: &[f32; 16]) -> Transform {
        let s = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt();
        let inv = if s > 1e-9 { 1.0 / s } else { 1.0 };
        Transform {
            rot: [
                m[0] * inv, m[4] * inv, m[8] * inv,
                m[1] * inv, m[5] * inv, m[9] * inv,
                m[2] * inv, m[6] * inv, m[10] * inv,
            ],
            t: [m[12], m[13], m[14]],
            s: if s > 1e-9 { s } else { 1.0 },
        }
    }
}

/// One scene-graph node kept for animation: sequences target nodes by name
/// and replace their *local* transforms, so the tree must survive
/// flattening. Parents always precede children in the vec.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneNode {
    /// The block this node was walked from, indexing `NifFile::blocks`.
    ///
    /// Not `Option`: every `SceneNode` IS a block — nothing anywhere
    /// synthesizes one.
    pub block: usize,
    pub name: String,
    pub parent: Option<usize>,
    pub local: Transform,
    /// `NiBillboardNode` mode; the subtree turns toward the camera every
    /// frame (0/2/3/4 face fully, 1/5/9 yaw about the vertical only).
    pub billboard: Option<u16>,
}

/// Which cached model, and which shape inside it, a mesh was cloned from.
///
/// Equal sources mean byte-identical geometry — that is the whole basis
/// for drawing two placements as one instanced batch, and it is a fact the
/// PRODUCER knows for free. The consumer could only recover it by hashing
/// the vertices, which at a 13x13 block means hashing 250 MB per rebuild
/// to reconstruct something that was thrown away one function earlier.
///
/// Geometry identity is necessary but not sufficient: draw state has to
/// match too, and that check belongs where the draw state lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShapeSource {
    /// Interned model id — one per distinct loaded model.
    pub model: u32,
    /// Index of the shape WITHIN that model's shape list, counted before
    /// hidden and effect shapes are skipped, so the numbering does not
    /// shift with what a particular placement happens to draw.
    pub shape: u32,
}

/// Everything extracted from one file.
#[derive(Debug, Default)]
pub struct MeshScene {
    pub meshes: Vec<RenderMesh>,
    /// Procedural lightning: one rig per `BSProceduralLightningController`,
    /// generated per strike by the viewport (`crate::lightning`).
    pub lightning: Vec<LightningRig>,
    /// Node names hidden AT REST by an `hkVis:*` float extra data whose
    /// value is 0.0 — the blood-bowl piles ship hidden-bit CLEAR and
    /// rely on this channel. Runtime state, not a mesh flag: the meshes
    /// upload normally and the viewport hides them by name, so a
    /// sequence driving the channel can SHOW them again
    /// (docs/knowledge/sweep-tail.md bucket 3).
    pub hk_rest_hidden: std::collections::HashSet<String>,
    /// Re-posable skinned meshes: the raw skinning inputs kept so bone
    /// motion (sequences, free controllers, path followers) can un-bake
    /// the bind pose per frame. Indices point into `meshes`.
    ///
    /// SSE ONLY. The LE path draws a skin but keeps no re-pose inputs
    /// (there is no animated LE skinned content in the corpus), so this
    /// answers "can it be re-posed", never "is it a skin" —
    /// [`Self::skinned_blocks`] is the second question.
    pub skinned: Vec<SkinnedMesh>,
    /// Every block drawn as a skinned shape, SSE and LE alike.
    ///
    /// Separate from `skinned` because that vec is the re-pose input and
    /// exists only on the SSE path: anything asking "is this a skin?"
    /// through it is blind on LE, which is how an LE skinned shape came
    /// to accept a transform edit the game ignores instead of refusing
    /// it.
    pub skinned_blocks: Vec<usize>,
    /// Free-running node animations: `NiTransformController`s hanging off
    /// nodes directly (NOT sequence-managed — those are blend
    /// interpolators owned by the manager). The hawk circles on a path
    /// interpolator and flaps through keyframed ones, with no sequence
    /// anywhere in the file.
    pub node_anims: Vec<NodeAnim>,
    /// Parallel to `meshes` when non-empty: which cached model and shape
    /// each one was cloned from. EMPTY means "nothing here shares anything
    /// with anything" — a single NIF's own shapes — and the renderer draws
    /// every shape as its own batch. Either empty or the same length as
    /// `meshes`; nothing in between.
    pub sources: Vec<Option<ShapeSource>>,
    /// Index into `meshes` where the LOD horizon backdrop begins; `None`
    /// when there is none.
    ///
    /// The backdrop is appended rather than kept apart because everything
    /// downstream — upload, culling, the draw order — should treat it as
    /// ordinary geometry. What must NOT treat it as ordinary is anything
    /// that answers a question about the scene the user is editing:
    /// picking (a click on the horizon is not a click on a record),
    /// framing (Home must frame your cell, not the province) and the
    /// world-reference table (which stays parallel to the meshes BEFORE
    /// this point). One index answers all three.
    pub backdrop_from: Option<usize>,
    /// The node tree the meshes hang off (see [`SceneNode`]).
    pub nodes: Vec<SceneNode>,
    /// Content that was present but not renderable (yet); shown in the log so
    /// "why is my mesh missing" has an answer.
    pub warnings: Vec<String>,
}

impl MeshScene {
    /// Node lookup the way the game does it: case-insensitive.
    pub fn node_index(&self, name: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| n.name.eq_ignore_ascii_case(name))
    }

    /// Any billboard subtrees present? The viewport then re-poses worlds
    /// every frame instead of only during sequence playback.
    pub fn has_billboards(&self) -> bool {
        self.nodes.iter().any(|n| n.billboard.is_some())
    }

    /// Local-transform overrides that turn every billboard node toward the
    /// camera. `eye` is the camera position; `up` the world up axis the
    /// yaw-only modes rotate about (+Z for Skyrim scenes).
    ///
    /// Full-face modes reproduce NifSkope's rule — the node's axes align
    /// with the camera's (geometry is authored in the node's X-Y plane,
    /// facing +Z). Yaw-only modes (1/5/9) rotate about Z alone so flame
    /// cards stay upright, per the BSROTATE_ABOUT_UP description.
    pub fn billboard_overrides(
        &self,
        eye: [f32; 3],
        cam_right: [f32; 3],
        cam_up: [f32; 3],
    ) -> std::collections::HashMap<usize, Transform> {
        let mut overrides = std::collections::HashMap::new();
        if !self.has_billboards() {
            return overrides;
        }
        // Authored world transforms: no overrides, because a billboard
        // is computed FROM the authored pose and its result IS the
        // override the caller then applies.
        let worlds = self.node_worlds(&Default::default());
        for (i, node) in self.nodes.iter().enumerate() {
            let Some(mode) = node.billboard else { continue };
            let pos = worlds[i].t;
            let world_rot = match mode {
                // Yaw about world Z: local +Y turns toward the camera,
                // horizontally.
                1 | 5 | 9 => {
                    let dir = [eye[0] - pos[0], eye[1] - pos[1]];
                    let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
                    if len < 1e-6 {
                        continue;
                    }
                    let (dx, dy) = (dir[0] / len, dir[1] / len);
                    // Rz with the +Y column equal to (dx, dy).
                    [dy, dx, 0.0, -dx, dy, 0.0, 0.0, 0.0, 1.0]
                }
                // Full facing: node axes = camera axes (X right, Y up on
                // screen, Z toward the viewer).
                _ => {
                    let d = [eye[0] - pos[0], eye[1] - pos[1], eye[2] - pos[2]];
                    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let back = if len > 1e-6 {
                        [d[0] / len, d[1] / len, d[2] / len]
                    } else {
                        [0.0, 0.0, 1.0]
                    };
                    [
                        cam_right[0], cam_up[0], back[0],
                        cam_right[1], cam_up[1], back[1],
                        cam_right[2], cam_up[2], back[2],
                    ]
                }
            };
            // The override replaces the node's LOCAL rotation; undo the
            // parent's rotation so the composed world one lands on target.
            let parent_rot = node
                .parent
                .map(|p| worlds[p].rot)
                .unwrap_or(Transform::IDENTITY.rot);
            overrides.insert(
                i,
                Transform {
                    rot: mat3_mul(&mat3_transpose(&parent_rot), &world_rot),
                    t: node.local.t,
                    s: node.local.s,
                },
            );
        }
        overrides
    }

    /// World transform of every node under `overrides`, parents-first.
    ///
    /// Parents always precede children in `nodes`, so one forward pass
    /// settles the whole tree. THE place that composes the node chain:
    /// `animated_worlds`, `billboard_overrides` and the viewport's node
    /// skeleton all call it, because three open-coded copies of a compose
    /// order is three chances for one of them to drift into
    /// `local.compose(&parent)` — which still puts the ROOT in the right
    /// place and is wrong for everything below it.
    pub fn node_worlds(
        &self,
        overrides: &std::collections::HashMap<usize, Transform>,
    ) -> Vec<Transform> {
        let mut worlds: Vec<Transform> = Vec::with_capacity(self.nodes.len());
        for (i, node) in self.nodes.iter().enumerate() {
            let local = overrides.get(&i).unwrap_or(&node.local);
            let world = match node.parent {
                Some(p) => worlds[p].compose(local),
                None => *local,
            };
            worlds.push(world);
        }
        worlds
    }

    /// The node chain above a mesh, innermost first: its own node, that
    /// node's parent, and so on to the root. Empty when the mesh hangs off
    /// no node (a skinned shape, a terrain quadrant, a generated ribbon).
    ///
    /// This is what the `[` walk steps along, so it must terminate on
    /// damaged input too: the walk is bounded by the node count, which a
    /// cycle cannot exceed.
    pub fn mesh_ancestors(&self, mesh: usize) -> Vec<usize> {
        let mut chain = Vec::new();
        let mut up = self.meshes.get(mesh).and_then(|m| m.node);
        while let Some(i) = up {
            if chain.len() > self.nodes.len() {
                break;
            }
            chain.push(i);
            up = self.nodes.get(i).and_then(|n| n.parent);
        }
        chain
    }

    /// Every mesh whose chain passes through `node`, itself included.
    ///
    /// Ancestor-OR-SELF: a node selected part-way up the chain covers the
    /// whole subtree below it, which is what the selection outline draws.
    pub fn meshes_under(&self, node: usize) -> Vec<usize> {
        (0..self.meshes.len())
            .filter(|m| {
                self.meshes[*m].node == Some(node) || self.mesh_ancestors(*m).contains(&node)
            })
            .collect()
    }

    /// Move a node and settle everything under it.
    ///
    /// Rewrites the node's LOCAL transform, then recomputes the world
    /// matrix of every mesh — one forward pass, because parents precede
    /// children.
    ///
    /// This is not optional bookkeeping. The viewport writes
    /// `meshes[i].world` back into the GPU uniforms the first frame the
    /// animation overrides go empty, so an edit that touched only the
    /// NIF block snaps back one frame later and looks like it did not
    /// take. The scene IS the thing on screen; the block is what gets
    /// saved.
    ///
    /// Returns false when the index is out of range, so a caller acting
    /// on a stale selection is told rather than silently ignored.
    pub fn apply_node_local(&mut self, node: usize, local: Transform) -> bool {
        if node >= self.nodes.len() {
            return false;
        }
        self.nodes[node].local = local;
        let worlds = self.node_worlds(&Default::default());
        for m in &mut self.meshes {
            if let Some(n) = m.node {
                m.world = worlds[n].compose(&m.local).to_column_major();
            }
        }
        self.rebake_skins();
        true
    }

    /// Re-skin every skinned mesh INTO the scene under the current node
    /// locals.
    ///
    /// The per-frame animation re-pose deliberately writes only the GPU
    /// vertex buffer -- it runs sixty times a second and must not
    /// allocate. An EDIT is different: it changes the rest pose itself,
    /// permanently, and every CPU surface reads `meshes[i].vertices` --
    /// the selection outline, the picking ray, the mesh boxes, the
    /// gizmo's own origin and Home's framing. Left at the pose the file
    /// was imported with, moving a bone would move the geometry on
    /// screen and nothing else, so the outline would sit where the
    /// bellows used to be and a click would hit them there.
    ///
    /// Idempotent: `repose_skinned` reads positions from `raw`, which no
    /// re-bake touches, and copies UV and colour from the baked vertex.
    fn rebake_skins(&mut self) {
        if self.skinned.is_empty() {
            return;
        }
        let rest = std::collections::HashMap::new();
        let baked: Vec<(usize, Vec<MeshVertex>)> = self
            .skinned
            .iter()
            .map(|sk| (sk.mesh, self.repose_skinned(sk, &rest)))
            .collect();
        for (mesh, vertices) in baked {
            self.meshes[mesh].vertices = std::sync::Arc::new(vertices);
        }
    }

    /// The nodes that PLACE the skinned shape drawn by `block`: the
    /// roots of its bone forest.
    ///
    /// A skinned shape's own transform does not place it. The game
    /// builds every vertex from `boneWorld o skinToBone`, and the
    /// shape's own AV transform is REDUNDANT with
    /// `NiSkinData.skin_transform` -- measured equal to four decimals on
    /// blacksmithforge01, which is simply how a well-formed file is
    /// baked. So "move this thing" means "move its bones".
    ///
    /// Returns the bones with no other bone above them, which is one
    /// node for a NESTED rig and the whole bone list for a FLAT one. The
    /// corpus is split almost evenly and the flat half is the one that
    /// matters most: of 56,174 skinned shapes over 67,578 files, 26,522
    /// are nested (blacksmithforge01's bellows: Center/Top/Bottom under
    /// Nozzle) and 29,652 are flat -- every character, because a Skyrim
    /// character NIF lists its bones as siblings of the file root and
    /// keeps the real hierarchy in the skeleton. chicken.nif has 32
    /// roots, alduin 77. Moving all of them by one world delta is a
    /// rigid move of the skin either way, which is why this returns a
    /// LIST rather than refusing on the common case.
    ///
    /// Empty when the skin cannot be moved as one: no re-pose inputs at
    /// all (every LE skin), or a bone that did not resolve into the
    /// walked tree. A PARTIALLY resolved bone set is refused rather than
    /// silently narrowed -- moving the roots of the bones we found would
    /// shear the mesh, dragging the vertices they own away from the
    /// rest. Measured 0 of 56,174 skins over 67,578 files, so this
    /// refuses nothing that ships; it exists so the contract and the
    /// code say the same thing.
    ///
    /// A root that also carries OTHER drawn geometry is allowed on
    /// purpose (353 skins, e.g. the DLC2 spider's fire cloak): those
    /// shapes are children of the bone and the game moves them with it
    /// too. Measured: no root is ever the scene root itself, so "move
    /// this shape" cannot become "move the whole file".
    pub fn skin_root_nodes(&self, block: usize) -> Vec<usize> {
        let Some(sk) = self
            .skinned
            .iter()
            .find(|s| self.meshes.get(s.mesh).and_then(|m| m.block) == Some(block))
        else {
            return Vec::new();
        };
        if sk.bones.iter().any(|b| b.node.is_none()) {
            return Vec::new();
        }
        let mut bones: Vec<usize> = sk.bones.iter().filter_map(|b| b.node).collect();
        bones.sort_unstable();
        bones.dedup();
        bones
            .iter()
            .copied()
            .filter(|n| {
                let mut up = self.nodes[*n].parent;
                let mut guard = 0;
                while let Some(p) = up {
                    if guard > self.nodes.len() {
                        break;
                    }
                    guard += 1;
                    if bones.binary_search(&p).is_ok() {
                        return false;
                    }
                    up = self.nodes[p].parent;
                }
                true
            })
            .collect()
    }

    /// Whether `node` is a bone of some skin.
    ///
    /// A bone usually carries no geometry of its own, so "nothing hangs
    /// off this node" used to be reported as "moving it will not change
    /// anything on screen". Since a node edit re-bakes the skins that
    /// is no longer true, and it is false for exactly the nodes the
    /// gizmo now tells the user to move.
    pub fn node_is_bone(&self, node: usize) -> bool {
        self.skinned
            .iter()
            .any(|s| s.bones.iter().any(|b| b.node == Some(node)))
    }

    /// Whether `block` is drawn as a skinned shape at all -- the
    /// question that tells "no root bone to move" apart from "not
    /// skinned", which are opposite answers for a caller.
    ///
    /// Reads `skinned_blocks`, which BOTH extraction paths fill, not
    /// `skinned`, which only the SSE one does.
    pub fn block_is_skinned(&self, block: usize) -> bool {
        self.skinned_blocks.contains(&block)
    }

    /// Move whatever the FILE's block `block` draws as, and settle it.
    ///
    /// The gizmo and the Block panel both name a BLOCK, and the block a
    /// plain click resolves to is a SHAPE, never a scene node: `nodes`
    /// holds the containers a transform can be inherited through and a
    /// shape is a `RenderMesh` hanging off one. Measured over the
    /// fixtures: of 39 meshes on the campfire, the forge and the
    /// scepter, ZERO carry a block that is also a node. So routing an
    /// edit through `apply_node_local` alone moved nothing on screen for
    /// the primary gesture — the file changed, the revision bumped, and
    /// the meshes were re-uploaded with the world they already had. A
    /// drag looked inert in all three gizmo modes while quietly editing
    /// the file, which is the worse half of that bug.
    ///
    /// Returns false when the scene cannot follow, so the caller can say
    /// so rather than leave the file and the screen disagreeing in
    /// silence.
    pub fn apply_block_local(&mut self, block: usize, local: Transform) -> bool {
        if let Some(n) = self.nodes.iter().position(|n| n.block == block) {
            return self.apply_node_local(n, local);
        }
        let targets: Vec<usize> = (0..self.meshes.len())
            .filter(|i| self.meshes[*i].block == Some(block))
            .collect();
        if targets.is_empty() {
            return false;
        }
        let worlds = self.node_worlds(&Default::default());
        let mut moved = false;
        for i in targets {
            // A skinned shape's world is skin->global out of NiSkinData
            // and its own AV transform is dead weight — the game ignores
            // it too, which is why it hangs off no node here. Leave it
            // where its bones put it and report that nothing moved.
            let Some(n) = self.meshes[i].node else { continue };
            self.meshes[i].local = local;
            self.meshes[i].world = worlds[n].compose(&local).to_column_major();
            moved = true;
        }
        moved
    }

    /// Whether any drawn geometry comes from this block — the question
    /// that tells "the scene cannot show this edit" apart from "the
    /// scene has nothing to show".
    pub fn block_is_drawn(&self, block: usize) -> bool {
        self.meshes.iter().any(|m| m.block == Some(block))
    }

    /// One node's world transform under the same overrides
    /// [`Self::animated_worlds`] takes — walking the parent chain only,
    /// because a caller asking about one anchor per frame should not pay
    /// for the whole tree.
    pub fn node_world(
        &self,
        index: usize,
        overrides: &std::collections::HashMap<usize, Transform>,
    ) -> Transform {
        let node = &self.nodes[index];
        let local = overrides.get(&index).unwrap_or(&node.local);
        match node.parent {
            Some(p) => self.node_world(p, overrides).compose(local),
            None => *local,
        }
    }

    /// World matrices for every mesh with some nodes' local transforms
    /// replaced. Parents precede children, so one forward pass settles the
    /// whole tree.
    pub fn animated_worlds(
        &self,
        overrides: &std::collections::HashMap<usize, Transform>,
    ) -> Vec<[f32; 16]> {
        let node_worlds = self.node_worlds(overrides);
        self.meshes
            .iter()
            .map(|m| match m.node {
                Some(n) => node_worlds[n].compose(&m.local).to_column_major(),
                // No node, no chain to recompose: the stored matrix IS the
                // answer, and it is the only copy of it. Rebuilding one from
                // `local` teleported every skinned shape to the scene origin
                // the moment any override existed, because a skinned mesh's
                // world is skin->global (its vertices are re-skinned in place
                // and stay in skin space) while its `local` is IDENTITY --
                // blacksmithforge01's bellows sit 160 units off, and only
                // files whose NiSkinData transform happens to be identity
                // (chicken.nif) were immune. `apply_node_local` already
                // leaves these meshes' worlds alone; this agrees with it.
                None => m.world,
            })
            .collect()
    }
}

/// `a * b` for 3x3 rotation rows (`rows[r*3+c]`, column-vector convention).
fn mat3_mul(a: &[f32; 9], b: &[f32; 9]) -> [f32; 9] {
    let mut out = [0.0f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] =
                a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
        }
    }
    out
}

/// Transpose = inverse for a pure rotation.
fn mat3_transpose(m: &[f32; 9]) -> [f32; 9] {
    [m[0], m[3], m[6], m[1], m[4], m[7], m[2], m[5], m[8]]
}

/// The shading state pulled off a shape's property refs.
struct ShapeStyle {
    texture: String,
    tint: [f32; 4],
    blend: BlendMode,
    alpha_test: Option<f32>,
    double_sided: bool,
    /// SLSF2 bit 0; see RenderMesh::depth_write.
    depth_write: bool,
    /// The shape carries NO shader property at all; see
    /// RenderMesh::no_material.
    no_material: bool,
    effect: bool,
    /// SLSF1 bit 15: the game renders this as screen-space refraction
    /// (heat haze); drawing its texture instead reads as a grey pancake.
    refraction: bool,
    /// Type 12 / SLSF2 bit 29: vertex alpha is the wind-sway weight, not
    /// opacity — the extraction forces it to 1 so trees stay solid.
    tree_anim: bool,
    /// SLSF1 bit 3 (Vertex_Alpha) CLEAR: the vertex colours' alpha is
    /// not opacity, and both the game (Lighting.hlsl `#if defined(VC)`)
    /// and NifSkope force it to 1. Where an alpha test is also on,
    /// honouring the stored alpha DELETES triangles the game draws
    /// (38 SSE blocks, 30 of them guaranteed loss; LE none).
    no_vertex_alpha: bool,
    /// BSSkyShaderProperty type 2: the three gradient colours
    /// (Horizon / SkyLower / SkyUpper) to matrix vertex colours through
    /// at extraction — they are constant, so the bake costs nothing at
    /// draw time. Clear-day stand-ins; in game they are weather state.
    sky_gradient: Option<[[f32; 3]; 3]>,
    material: EffectMaterial,
    lighting: LightingMaterial,
    shader_anims: Vec<ShaderAnim>,
    shader_color_anims: Vec<ShaderColorAnim>,
}

impl Default for ShapeStyle {
    fn default() -> Self {
        ShapeStyle {
            texture: String::new(),
            tint: [1.0; 4],
            blend: BlendMode::Opaque,
            alpha_test: None,
            double_sided: false,
            // The format's default and what every reference does
            // when the flag word is absent: opaque geometry writes.
            depth_write: true,
            no_material: false,
            effect: false,
            refraction: false,
            tree_anim: false,
            no_vertex_alpha: false,
            sky_gradient: None,
            material: EffectMaterial::default(),
            lighting: LightingMaterial::default(),
            shader_anims: Vec::new(),
            shader_color_anims: Vec::new(),
        }
    }
}

/// `BSLightingShaderProperty` shading inputs beyond the diffuse map, per
/// docs/knowledge/mesh-rendering.md §3.3/§3.5. Zeroed strengths turn a
/// feature off; empty paths bind reference-viewer fallbacks (normal =
/// flat, env mask = white, glow = black).
#[derive(Debug, Clone, PartialEq)]
pub struct LightingMaterial {
    /// Slot 1 `_n` normal map; alpha is the specular factor. When `msn`
    /// is set this is the `_msn` MODEL-space map instead (decode swizzle
    /// `(r, b, g) * 2 − 1`, no tangent frame).
    pub normal_texture: String,
    /// SLSF1 bit 12: model-space normals (bodies, heads).
    pub msn: bool,
    /// Slot 2 glow map, multiplied into the emissive term (SLSF2 bit 6).
    pub glow_texture: String,
    /// Slot 4 environment cubemap (SLSF1 bit 7, or the SSE rule that
    /// MultiLayerParallax implies env mapping).
    pub env_texture: String,
    /// Slot 5 environment mask; falls back to the normal map alpha.
    pub env_mask_texture: String,
    /// Cube term = `cube.rgb * env_scale * mask.r`, added to albedo.
    /// 0 = environment mapping off.
    pub env_scale: f32,
    pub specular_color: [f32; 3],
    /// 0 when SLSF1_Specular is clear.
    pub specular_strength: f32,
    pub glossiness: f32,
    /// Emissive color × multiple; zeroed when Own_Emit is clear.
    pub emissive: [f32; 3],
    pub emissive_mult: f32,
    /// Slot 3 `_p` height map (shader type 3 + SLSF1 bit 11); one tap,
    /// hardcoded 0.08/−0.04 — the format stores no scale for type 3.
    pub height_texture: String,
    /// Slot 7 `_s` dedicated specular mask (MSN only, red channel);
    /// empty when backlight owns the slot.
    pub specular_texture: String,
    /// Slot 7 `_b` backlight mask (SLSF2 bit 27; wins slot 7 over the
    /// specular map).
    pub backlight_texture: String,
    /// Slot 2 `_sk` soft/rim mask, shared by both effects.
    pub softrim_texture: String,
    /// SLSF2 bit 25 / 26.
    pub soft_lighting: bool,
    pub rim_lighting: bool,
    /// Lighting Effect 1: the soft-light wrap rolloff.
    pub lighting_effect1: f32,
    /// Lighting Effect 2: the rim power.
    pub lighting_effect2: f32,
    /// Shader type 11 (MultiLayerParallax): slot 6 inner-layer texture.
    /// Its ALPHA (sampled at the un-offset uv) modulates the thickness;
    /// its RGB is the inner layer sampled through the transmission offset.
    pub inner_texture: String,
    /// Distance between the outer and inner layers.
    pub mlp_thickness: f32,
    /// 0 = transmit about the face normal, 1 = about the normal map.
    pub mlp_refraction: f32,
    /// Inner layer tiling.
    pub mlp_inner_scale: [f32; 2],
    /// Type 5: the authored tail tint through the game's quadratic
    /// overlay `b² + 2·t·b·(1−b)` (runtime replaces t per actor).
    pub skin_tint: bool,
    /// Type 6: `lit × lerp(1, tint, vertexColor.g)` REPLACES the plain
    /// vertex-colour multiply — vc green is a painted tint-strength mask.
    pub hair_tint: bool,
    /// Type 4: heads; the runtime tint/detail composites aren't in the
    /// NIF, so the viewer applies the tint curve with t = base (the
    /// smoothstep contrast the shipped formula reduces to).
    pub facegen: bool,
    /// The tail Color3 for types 5/6 (white when absent).
    pub tint_color: [f32; 3],
    /// SLSF2 bit 21: hair's anisotropic specular.
    pub aniso: bool,
    /// Shader type 18 (LODLandscapeNoise): the game breaks LOD terrain
    /// up with a tiled noise texture and forces glossiness to 0
    /// (Lighting.hlsl:1392-1400). Every `.btr` tile in both corpora is
    /// this type — 19,168 SSE and 9,584 LE — so without it the whole
    /// Horizon backdrop is flat and systematically brighter.
    pub lod_land_noise: bool,
    /// The NIF Refraction Strength (runtime material+0x84), uploaded RAW
    /// as the writer's RefractionPower. 240 corpus shapes rest at 0 and
    /// live entirely off var-0 float controllers.
    pub refraction_strength: f32,
    /// SLSF1 bit 16 (Fire Refraction): multiplies the written strength by
    /// N·V. It does NOT route the shape anywhere different (RE
    /// 0x1412c67e3: it only ORs the FALLOFF technique bit).
    pub refraction_falloff: bool,
}

impl Default for LightingMaterial {
    fn default() -> Self {
        LightingMaterial {
            normal_texture: String::new(),
            msn: false,
            glow_texture: String::new(),
            env_texture: String::new(),
            env_mask_texture: String::new(),
            env_scale: 0.0,
            specular_color: [1.0; 3],
            specular_strength: 0.0,
            glossiness: 80.0,
            emissive: [0.0; 3],
            emissive_mult: 0.0,
            height_texture: String::new(),
            specular_texture: String::new(),
            backlight_texture: String::new(),
            softrim_texture: String::new(),
            soft_lighting: false,
            rim_lighting: false,
            lighting_effect1: 0.3,
            lighting_effect2: 2.0,
            inner_texture: String::new(),
            mlp_thickness: 0.0,
            mlp_refraction: 0.0,
            mlp_inner_scale: [1.0, 1.0],
            skin_tint: false,
            hair_tint: false,
            facegen: false,
            tint_color: [1.0; 3],
            aniso: false,
            lod_land_noise: false,
            refraction_strength: 0.0,
            refraction_falloff: false,
        }
    }
}

/// Free-running shader controllers hanging off a block (a shader
/// property or an alpha property), following the Next Controller chain.
///
/// Sequence-managed forms (blend interpolators) contribute NOTHING here
/// — their keys live in the sequence and play through the sequence
/// player; a free-running copy would fight it.
fn shader_anims_of(
    file: &NifFile,
    ctx: Ctx,
    controller: BlockRef,
) -> (Vec<ShaderAnim>, Vec<ShaderColorAnim>) {
    use aether_nif::typed::controllers::{
        BSEffectShaderPropertyColorController, BSEffectShaderPropertyFloatController, BSLightingShaderPropertyColorController,
        BSLightingShaderPropertyFloatController, BSNiAlphaPropertyTestRefController,
        ControllerBase, NiFloatData, NiFloatInterpolator, NiPoint3Interpolator, NiPosData,
    };
    let float_keys = |interp: BlockRef| -> crate::curve::FloatCurve {
        interp
            .get()
            .and_then(|i| file.blocks.get(i))
            .and_then(|b| NiFloatInterpolator::decode(b, ctx).ok())
            .and_then(|fi| fi.data.get())
            .and_then(|d| file.blocks.get(d))
            .and_then(|b| NiFloatData::decode(b, ctx).ok())
            .map(|fd| crate::curve::FloatCurve::from_data(&fd))
            .unwrap_or_default()
    };
    let color_keys = |interp: BlockRef| -> crate::curve::Curve {
        interp
            .get()
            .and_then(|i| file.blocks.get(i))
            .and_then(|b| NiPoint3Interpolator::decode(b, ctx).ok())
            .and_then(|pi| pi.data.get())
            .and_then(|d| file.blocks.get(d))
            .and_then(|b| NiPosData::decode(b, ctx).ok())
            .map(|pd| crate::curve::Curve::from_pos_data(pd.interpolation, &pd.floats))
            .unwrap_or_else(|| crate::curve::Curve::empty(crate::curve::Track::Vec3))
    };
    let push_float = |anims: &mut Vec<ShaderAnim>,
                      kind: ShaderAnimKind,
                      variable: u32,
                      base: &ControllerBase,
                      keys: crate::curve::FloatCurve| {
        if !keys.is_empty() {
            anims.push(ShaderAnim {
                kind,
                variable,
                keys,
                start_time: base.start_time,
                stop_time: base.stop_time,
                frequency: base.frequency,
                looping: base.flags & 6 == 0,
            });
        }
    };
    let mut anims = Vec::new();
    let mut colors = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut next = controller;
    while let Some(idx) = next.get() {
        if !seen.insert(idx) {
            break;
        }
        let Some(block) = file.blocks.get(idx) else { break };
        next = match block.type_name.as_str() {
            "BSLightingShaderPropertyFloatController" => {
                match BSLightingShaderPropertyFloatController::decode(block, ctx) {
                    Ok(c) => {
                        push_float(
                            &mut anims,
                            ShaderAnimKind::LightingFloat,
                            c.controlled_variable,
                            &c.base,
                            float_keys(c.interpolator),
                        );
                        c.base.next_controller
                    }
                    Err(_) => next_of(block),
                }
            }
            "BSLightingShaderPropertyColorController" => {
                match BSLightingShaderPropertyColorController::decode(block, ctx) {
                    Ok(c) => {
                        let keys = color_keys(c.interpolator);
                        if !keys.is_empty() {
                            colors.push(ShaderColorAnim {
                                effect: false,
                                variable: c.controlled_color,
                                keys,
                                start_time: c.base.start_time,
                                stop_time: c.base.stop_time,
                                frequency: c.base.frequency,
                                looping: c.base.flags & 6 == 0,
                            });
                        }
                        c.base.next_controller
                    }
                    Err(_) => next_of(block),
                }
            }
            "BSNiAlphaPropertyTestRefController" => {
                match BSNiAlphaPropertyTestRefController::decode(block, ctx) {
                    Ok(c) => {
                        push_float(
                            &mut anims,
                            ShaderAnimKind::AlphaThreshold,
                            0,
                            &c.base,
                            float_keys(c.interpolator),
                        );
                        c.base.next_controller
                    }
                    Err(_) => next_of(block),
                }
            }
            "BSEffectShaderPropertyColorController" => {
                // Its own typed view: `decode` gates on TYPE_NAMES, so
                // handing this block to the LIGHTING view (as this arm
                // used to) always took the Err path and walked on in
                // silence. Variable 0 is the base colour rgb, written
                // by 1.5.97 `0x141322860` into material[+72].
                match BSEffectShaderPropertyColorController::decode(block, ctx) {
                    Ok(c) => {
                        let keys = color_keys(c.interpolator);
                        if !keys.is_empty() {
                            colors.push(ShaderColorAnim {
                                effect: true,
                                variable: c.controlled_color,
                                keys,
                                start_time: c.base.start_time,
                                stop_time: c.base.stop_time,
                                frequency: c.base.frequency,
                                looping: c.base.flags & 6 == 0,
                            });
                        }
                        c.base.next_controller
                    }
                    Err(_) => next_of(block),
                }
            }
            _ => match BSEffectShaderPropertyFloatController::decode(block, ctx) {
            Ok(c) => {
                let keys = float_keys(c.interpolator);
                push_float(
                    &mut anims,
                    ShaderAnimKind::Effect,
                    c.controlled_variable,
                    &c.base,
                    keys,
                );
                c.base.next_controller
            }
            // Every NiTimeController stores Next Controller first; keep
            // walking past types we do not evaluate.
            Err(_) => next_of(block),
            },
        };
    }
    (anims, colors)
}

/// A controller block's Next Controller — the first field of every
/// NiTimeController — so the walk continues past unhandled types.
fn next_of(block: &aether_nif::Block) -> BlockRef {
    block
        .data
        .get(0..4)
        .map(|b| BlockRef(i32::from_le_bytes([b[0], b[1], b[2], b[3]])))
        .unwrap_or(BlockRef::NONE)
}

pub fn meshes_from_nif(file: &NifFile) -> MeshScene {
    let mut scene = MeshScene::default();
    let Ok(ctx) = Ctx::of(&file.header) else {
        scene
            .warnings
            .push("not a Skyrim SE/LE stream; meshes not extracted".into());
        return scene;
    };

    // (block index, parent node in scene.nodes, parent world, parent hidden)
    // (block, parent node, parent world, hidden, alpha-order group)
    let mut stack: Vec<(usize, Option<usize>, Transform, bool, usize)> = file
        .roots
        .iter()
        .filter(|r| **r >= 0)
        .map(|r| (*r as usize, None, Transform::IDENTITY, false, 0))
        .collect();

    // Skinned shapes need their BONE nodes' world transforms, and a bone
    // can be visited after the shape referencing it — so record every node
    // world by block index and process skinned shapes after the walk.
    let mut node_worlds: std::collections::HashMap<usize, Transform> =
        std::collections::HashMap::new();
    // ...and which SCENE node each block became, so a re-posable skinned
    // mesh can follow its bones through the live node tree.
    let mut block_to_node: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    // ...and the inverse, parallel to `scene.nodes`, for the cycle guard.
    // (block, parent node, hidden, alpha-order group)
    let mut skinned: Vec<(usize, Option<usize>, bool, usize)> = Vec::new();

    // The hkVis resting scan: suffix-bound, like the game's
    // BShkVisibilityController (an exact == 0.0 hides).
    {
        use aether_nif::typed::controllers::NiFloatExtraData;
        scene.hk_rest_hidden = file
            .blocks
            .iter()
            .filter(|b| b.type_name == "NiFloatExtraData")
            .filter_map(|b| NiFloatExtraData::decode(b, ctx).ok())
            .filter(|ed| ed.value == 0.0)
            .filter_map(|ed| {
                ed.name
                    .resolve(&file.header)
                    .and_then(|n| n.strip_prefix("hkVis:"))
                    .map(|s| s.to_string())
            })
            .collect();
    }

    while let Some((index, parent_node, parent, parent_hidden, order_group)) = stack.pop() {
        let Some(block) = file.blocks.get(index) else {
            continue;
        };
        let type_name = block.type_name.as_str();
        // Children of a BSOrderedNode inherit ITS block index as the
        // group; a nested one wins, like the innermost list does in
        // the game's own walk.
        let child_group = if type_name == "BSOrderedNode" {
            index
        } else {
            order_group
        };

        if is_scene_node(type_name) {
            let SceneNodeDecode {
                node,
                billboard,
                switch_index,
            } = match decode_scene_node(block, ctx) {
                Ok(n) => n,
                Err(e) => {
                    scene.warnings.push(format!("{type_name} {index}: {e}"));
                    continue;
                }
            };
            // Hidden (bit 0) inherits down. The subtree is still walked:
            // hidden shapes are not drawn, but they are the geometry mesh
            // emitters sample from.
            let hidden = parent_hidden || node.av.flags & 1 != 0;
            let local = Transform::from_av(
                node.av.translation,
                node.av.rotation,
                node.av.scale,
            );
            let world = parent.compose(&local);
            node_worlds.insert(index, world);
            let node_index = scene.nodes.len();
            block_to_node.insert(index, node_index);
            scene.nodes.push(SceneNode {
                block: index,
                name: resolve_name(file, node.av.net.name),
                parent: parent_node,
                local,
                billboard,
            });
            extract_lightning(file, ctx, index, &node, node_index, &mut scene);
            extract_node_anims(file, ctx, &node, node_index, &mut scene);
            // A switch draws exactly one child (RE 0x140C63AE0); an index
            // outside the list is not something the game can draw either,
            // so say so and fall back to the whole subtree rather than
            // showing nothing.
            let only_child = switch_index.and_then(|i| {
                let ok = (i as usize) < node.children.len();
                if !ok {
                    scene.warnings.push(format!(
                        "{type_name} {index}: switch index {i} is outside its {} children; drawing all of them",
                        node.children.len()
                    ));
                }
                ok.then_some(i as usize)
            });
            // Reversed, because `stack` is LIFO: pushing in file order
            // visits children BACKWARDS, which made extraction order
            // the reverse of the file and any order-dependent
            // tie-break not merely unsorted but inverted.
            for (slot, child) in node.children.iter().enumerate().rev() {
                if only_child.is_some_and(|k| k != slot) {
                    continue;
                }
                if let Some(c) = child.get() {
                    // Cycle guard: a block that is its own ancestor would
                    // walk forever. Duplicates under DIFFERENT parents are
                    // left alone deliberately — the game walks its children
                    // lists the same way and draws them twice too.
                    let mut up = Some(node_index);
                    let mut cyclic = false;
                    while let Some(a) = up {
                        // `up` starts at `node_index`, the node pushed
                        // just above, so this index is always in range.
                        if scene.nodes[a].block == c {
                            cyclic = true;
                            break;
                        }
                        up = scene.nodes[a].parent;
                    }
                    if cyclic {
                        scene.warnings.push(format!(
                            "{type_name} {index}: child block {c} is its own ancestor; the subtree is cyclic and was cut here"
                        ));
                        continue;
                    }
                    stack.push((c, Some(node_index), world, hidden, child_group));
                }
            }
        } else if type_name == "BSTriShape" {
            match BsTriShape::decode(block, ctx) {
                Ok(shape) if shape.skin.get().is_some() => {
                    skinned.push((index, parent_node, parent_hidden, order_group));
                }
                Ok(shape) => extract_sse_shape(
                    file,
                    ctx,
                    index,
                    &shape,
                    parent_node,
                    &parent,
                    parent_hidden,
                    order_group,
                    &mut scene,
                ),
                Err(e) => scene.warnings.push(format!("BSTriShape {index}: {e}")),
            }
        } else if type_name == "BSSubIndexTriShape" {
            // Object LOD geometry. It draws exactly like a BSTriShape;
            // the extra segment table only says which cell each run of
            // triangles belongs to, which matters when swapping a cell
            // to full detail, not when drawing it.
            match aether_nif::typed::geometry::BsSubIndexTriShape::decode(block, ctx) {
                Ok(sub) => {
                    let before = scene.meshes.len();
                    extract_sse_shape(
                        file,
                        ctx,
                        index,
                        &sub.shape,
                        parent_node,
                        &parent,
                        parent_hidden,
                        order_group,
                        &mut scene,
                    );
                    // The table belongs to the shape the call just
                    // pushed; a shape that produced nothing (hidden,
                    // empty) has nothing to attach it to.
                    for mesh in &mut scene.meshes[before..] {
                        mesh.segments = sub
                            .segments
                            .iter()
                            .map(|s| (s.start_index, s.num_primitives))
                            .collect();
                    }
                }
                Err(e) => scene
                    .warnings
                    .push(format!("BSSubIndexTriShape {index}: {e}")),
            }
        } else if type_name == "BSDynamicTriShape" {
            // Facegen heads: always skinned, positions on the shape itself.
            skinned.push((index, parent_node, parent_hidden, order_group));
        } else if matches!(
            type_name,
            "NiTriShape" | "NiTriStrips" | "BSLODTriShape" | "BSSegmentedTriShape"
        ) {
            match le_shape(block, ctx) {
                Ok(shape) => {
                    if shape.skin_instance.get().is_some() {
                        skinned.push((index, parent_node, parent_hidden, order_group));
                    } else {
                        extract_le_shape(
                            file,
                            ctx,
                            index,
                            &shape,
                            parent_node,
                            &parent,
                            parent_hidden,
                            order_group,
                            &mut scene,
                        )
                    }
                }
                Err(e) => scene.warnings.push(format!("NiTriShape {index}: {e}")),
            }
        } else if type_name.ends_with("Node") {
            scene.warnings.push(format!(
                "{type_name} {index}: unsupported container, its subtree is not shown"
            ));
        }
        // Everything else (particle systems, controllers, collision) is
        // handled by other subsystems or irrelevant to mesh extraction.
    }

    for (index, parent_node, hidden, order_group) in skinned {
        extract_skinned_shape(
            file,
            ctx,
            index,
            parent_node,
            hidden,
            &node_worlds,
            &block_to_node,
            order_group,
            &mut scene,
        );
    }

    scene
}

/// One `BSProceduralLightningController` found in the scene: the recipe,
/// its tracks, and the material of the dummy child shape the game clones
/// the bolt's shader from. The bolt itself is generated at play time.
#[derive(Debug, Clone)]
pub struct LightningRig {
    /// The target node — bolt coordinates are in ITS local frame.
    pub node: usize,
    pub params: crate::lightning::LightningParams,
    pub animate_arc_offset: bool,
    /// The controller window, for the free-running clocks.
    pub start_time: f32,
    pub stop_time: f32,
    pub frequency: f32,
    pub looping: bool,
    /// Free-running Generation / Mutation bool tracks (empty when the
    /// file is sequence-managed — the keys then live in the sequences).
    pub generation_keys: Vec<(f32, bool)>,
    pub mutation_keys: Vec<(f32, bool)>,
    /// Free-running ArcOffset float track.
    pub arc_keys: crate::curve::FloatCurve,
    /// The dummy child's effect material, cloned onto the bolt exactly
    /// like the game does (RE 0x140D5F8D1: BOTH properties or nothing).
    pub texture: String,
    pub tint: [f32; 4],
    pub material: EffectMaterial,
    pub additive: bool,
    pub double_sided: bool,
}

/// What a free-running node controller animates.
#[derive(Debug, Clone)]
pub enum NodeAnimKeys {
    /// Spline path + percent track (`NiPathInterpolator`).
    Path(crate::path::PathKeys),
    /// Keyframed local-transform channels (`NiTransformInterpolator`).
    Transform(Box<crate::sequence::TransformKeys>),
}

/// One free-running `NiTransformController` on a scene node. Sequence
/// playback WINS over these when it drives the same node — a sequence that
/// poses a node and a free controller fighting it would flicker.
#[derive(Debug, Clone)]
pub struct NodeAnim {
    pub node: usize,
    /// The controller window, same mapping as [`ShaderAnim`].
    pub start_time: f32,
    pub stop_time: f32,
    pub frequency: f32,
    pub looping: bool,
    pub keys: NodeAnimKeys,
}

impl NodeAnim {
    /// Sample at wall-clock `time` seconds through the controller's local
    /// looping window.
    pub fn sample(&self, time: f32) -> crate::sequence::TransformSample {
        let local = crate::sequence::controller_time(
            self.start_time + time * self.frequency,
            self.start_time,
            self.stop_time,
            self.looping,
        );
        match &self.keys {
            NodeAnimKeys::Path(keys) => crate::path::sample_path(keys, local),
            NodeAnimKeys::Transform(keys) => crate::sequence::sample_transform(keys, local),
        }
    }
}

/// A node carrying a `BSProceduralLightningController` becomes a
/// lightning rig. The material comes from the node's FIRST CHILD — the
/// hidden dummy BSTriShape whose whole job is to hold the effect shader
/// and alpha property; the game clones both onto the generated bolt and
/// Free-running `NiTransformController`s on a node's controller chain —
/// the hawk circles on a path interpolator and flaps through keyframed
/// ones with no sequence anywhere in its file. Sequence-managed transforms
/// never land here: the manager drives those through
/// `NiMultiTargetTransformController` and blend interpolators, and the
/// node's own chain in such files carries no `NiTransformController`.
fn extract_node_anims(
    file: &NifFile,
    ctx: Ctx,
    node: &NiNode,
    node_index: usize,
    scene: &mut MeshScene,
) {
    use aether_nif::typed::controllers::NiTransformController;
    let mut next = node.av.net.controller;
    let mut hops = 0;
    while let Some(ci) = next.get() {
        hops += 1;
        if hops > 32 {
            break;
        }
        let Some(block) = file.blocks.get(ci) else {
            break;
        };
        if block.type_name != "NiTransformController" {
            next = next_of(block);
            continue;
        }
        let Ok(c) = NiTransformController::decode(block, ctx) else {
            next = next_of(block);
            continue;
        };
        next = c.base.next_controller;
        // Inactive controllers (bit 3 clear) do not run in game either.
        if c.base.flags & 8 == 0 {
            continue;
        }
        let keys = c
            .interpolator
            .get()
            .and_then(|i| file.blocks.get(i))
            .and_then(|b| match b.type_name.as_str() {
                "NiPathInterpolator" => {
                    crate::path::build(file, ctx, b).map(NodeAnimKeys::Path)
                }
                "NiTransformInterpolator" => crate::sequence::transform_keys_from(file, ctx, b)
                    .map(|k| NodeAnimKeys::Transform(Box::new(k))),
                // Blend interpolators belong to a manager; anything else
                // is not a transform source.
                _ => None,
            });
        if let Some(keys) = keys {
            scene.node_anims.push(NodeAnim {
                node: node_index,
                start_time: c.base.start_time,
                stop_time: c.base.stop_time,
                frequency: c.base.frequency,
                looping: (c.base.flags >> 1) & 3 == 0,
                keys,
            });
        }
    }
}

/// One bone of a re-posable skinned mesh.
#[derive(Debug, Clone)]
pub struct SkinBone {
    /// Scene-node index when the bone block is in the walked tree.
    pub node: Option<usize>,
    /// The bone's world at import — the fallback for bones outside the
    /// tree, and exactly what `node_world` reproduces with no overrides.
    pub bind_world: Transform,
    pub skin_to_bone: Transform,
}

/// The raw (pre-skinning) inputs of one skinned mesh — what the import
/// bake consumed. Kept so the viewport can re-skin under CURRENT bone
/// worlds when an animation moves them; the baked mesh in `meshes` stays
/// the bind pose the world tab and static views draw.
#[derive(Debug, Clone)]
pub struct SkinnedMesh {
    /// Index into `MeshScene::meshes`.
    pub mesh: usize,
    pub bones: Vec<SkinBone>,
    /// global→skin. The mesh's `world` is its inverse and NEVER changes:
    /// re-posed vertices stay in skin space, so a re-pose rewrites vertex
    /// bytes and nothing else.
    pub to_skin: Transform,
    /// `(position, normal, bone indices, weights)` per vertex, parallel
    /// to the mesh's vertex array.
    pub raw: Vec<([f32; 3], [f32; 3], [u16; 4], [f32; 4])>,
}

impl MeshScene {
    /// Re-skin one mesh under the current node worlds: per-bone matrix =
    /// bone world ∘ skin→bone, weighted sum, back into skin space — the
    /// same composition the import bake used (§12), with the bone worlds
    /// live instead of authored. UV and colour never change; they are
    /// copied from the baked vertices.
    pub fn repose_skinned(
        &self,
        sk: &SkinnedMesh,
        overrides: &std::collections::HashMap<usize, Transform>,
    ) -> Vec<MeshVertex> {
        let baked = &self.meshes[sk.mesh].vertices;
        let bones: Vec<Transform> = sk
            .bones
            .iter()
            .map(|b| {
                let world = match b.node {
                    Some(i) => self.node_world(i, overrides),
                    None => b.bind_world,
                };
                world.compose(&b.skin_to_bone)
            })
            .collect();
        sk.raw
            .iter()
            .zip(baked.iter())
            .map(|((pos, nrm, bi, wts), old)| {
                let mut p = [0.0f32; 3];
                let mut n = [0.0f32; 3];
                let mut total = 0.0f32;
                for k in 0..4 {
                    let w = wts[k];
                    if w <= 0.0 {
                        continue;
                    }
                    let Some(m) = bones.get(bi[k] as usize) else {
                        continue;
                    };
                    let tp = m.transform_point(*pos);
                    let tn = m.rotate(*nrm);
                    for a in 0..3 {
                        p[a] += tp[a] * w;
                        n[a] += tn[a] * w;
                    }
                    total += w;
                }
                if total <= 0.0 {
                    p = *pos;
                    n = *nrm;
                }
                MeshVertex {
                    position: sk.to_skin.transform_point(p),
                    normal: normalize3(sk.to_skin.rotate(n)),
                    uv: old.uv,
                    color: old.color,
                }
            })
            .collect()
    }
}

/// draws nothing without them (RE 0x140D601A0 / 0x140D5F8D1).
fn extract_lightning(
    file: &NifFile,
    ctx: Ctx,
    node_block: usize,
    node: &NiNode,
    node_index: usize,
    scene: &mut MeshScene,
) {
    use aether_nif::typed::controllers::{
        BSProceduralLightningController, NiBoolData, NiBoolTimelineInterpolator, NiFloatData,
        NiFloatInterpolator,
    };
    let _ = node_block;
    let Some(ci) = node.av.net.controller.get() else {
        return;
    };
    let Some(block) = file.blocks.get(ci) else { return };
    if block.type_name != "BSProceduralLightningController" {
        return;
    }
    let Ok(c) = BSProceduralLightningController::decode(block, ctx) else {
        return;
    };

    let bool_keys = |r: aether_nif::typed::BlockRef| -> Vec<(f32, bool)> {
        r.get()
            .and_then(|i| file.blocks.get(i))
            .and_then(|b| NiBoolTimelineInterpolator::decode(b, ctx).ok())
            .and_then(|bi| bi.data.get())
            .and_then(|d| file.blocks.get(d))
            .and_then(|b| NiBoolData::decode(b, ctx).ok())
            .map(|d| d.keys.iter().map(|(t, v)| (*t, *v != 0)).collect())
            .unwrap_or_default()
    };
    let float_keys = |r: aether_nif::typed::BlockRef| -> crate::curve::FloatCurve {
        r.get()
            .and_then(|i| file.blocks.get(i))
            .and_then(|b| NiFloatInterpolator::decode(b, ctx).ok())
            .and_then(|fi| fi.data.get())
            .and_then(|d| file.blocks.get(d))
            .and_then(|b| NiFloatData::decode(b, ctx).ok())
            .map(|d| crate::curve::FloatCurve::from_data(&d))
            .unwrap_or_default()
    };

    // The dummy child's material. Both properties or nothing — the same
    // all-or-nothing rule the runtime applies.
    let mut texture = String::new();
    let mut tint = [1.0; 4];
    let mut material = EffectMaterial::default();
    let mut additive = true;
    let mut double_sided = true;
    let mut have_material = false;
    'child: for child in &node.children {
        let Some(gi) = child.get() else { continue };
        let Some(gb) = file.blocks.get(gi) else { continue };
        // SE dummies are BSTriShape; the LE beam authors the SAME
        // hidden first-child pattern with NiTriShape — both carry
        // dedicated shader/alpha refs at bs83/bs100.
        let (shader_ref, alpha_ref) = match gb.type_name.as_str() {
            "BSTriShape" => match BsTriShape::decode(gb, ctx) {
                Ok(shape) => (shape.shader_property, shape.alpha_property),
                Err(_) => continue,
            },
            "NiTriShape" => match NiTriShape::decode(gb, ctx) {
                Ok(shape) => (shape.shader_property, shape.alpha_property),
                Err(_) => continue,
            },
            _ => continue,
        };
        if shader_ref.get().is_none() || alpha_ref.get().is_none() {
            continue;
        }
        let mut warnings = Vec::new();
        let style = style_from_properties(
            file,
            ctx,
            shader_ref,
            alpha_ref,
            &mut warnings,
        );
        texture = style.texture;
        tint = style.tint;
        material = style.material;
        additive = style.blend == BlendMode::Additive;
        double_sided = style.double_sided;
        have_material = true;
        break 'child;
    }
    if !have_material {
        scene.warnings.push(format!(
            "lightning controller {ci}: its target node has no dummy shape \
             with BOTH an effect shader and an alpha property — the game \
             draws nothing for it, and neither do we"
        ));
        return;
    }

    scene.lightning.push(LightningRig {
        node: node_index,
        params: crate::lightning::LightningParams {
            subdivisions: c.subdivisions,
            num_branches: c.num_branches,
            num_branches_variation: c.num_branches_variation,
            length: c.length,
            length_variation: c.length_variation,
            width: c.width,
            child_width_mult: c.child_width_mult,
            arc_offset: c.arc_offset,
            fade_main_bolt: c.fade_main_bolt,
            fade_child_bolts: c.fade_child_bolts,
        },
        animate_arc_offset: c.animate_arc_offset,
        start_time: c.base.start_time,
        stop_time: c.base.stop_time,
        frequency: c.base.frequency,
        looping: c.base.flags & 6 == 0,
        generation_keys: bool_keys(c.generation),
        mutation_keys: bool_keys(c.mutation),
        arc_keys: float_keys(c.arc_offset_interp),
        texture,
        tint,
        material,
        additive,
        double_sided,
    });
}

/// Container types the scene walk descends into.
pub fn is_scene_node(type_name: &str) -> bool {
    NiNode::TYPE_NAMES.contains(&type_name)
        || type_name == "BSOrderedNode"
        || type_name == "NiBillboardNode"
        || type_name == "BSValueNode"
        || type_name == "NiSwitchNode"
        || type_name == "BSMultiBoundNode"
        || type_name == "BSTreeNode"
        || type_name == "BSMasterParticleSystem"
        || aether_nif::typed::geometry::BsRangeNode::TYPE_NAMES.contains(&type_name)
}

/// A particle system block: the two on-disk names one layout covers.
pub fn is_particle_system(type_name: &str) -> bool {
    aether_nif::typed::particles::NiParticleSystem::TYPE_NAMES.contains(&type_name)
}

/// A block that carries drawable geometry, on either target.
///
/// The list IS the dispatch the walk uses; keeping it here means a new
/// shape kind is added in one place rather than found by whoever notices
/// the geometry missing — which is how BSLODTriShape stayed invisible.
pub fn is_geometry(type_name: &str) -> bool {
    use aether_nif::typed::geometry::{
        BsDynamicTriShape, BsLodTriShape, BsSegmentedTriShape, BsSubIndexTriShape, BsTriShape,
        NiTriShape,
    };
    BsTriShape::TYPE_NAMES.contains(&type_name)
        || BsSubIndexTriShape::TYPE_NAMES.contains(&type_name)
        || BsDynamicTriShape::TYPE_NAMES.contains(&type_name)
        || NiTriShape::TYPE_NAMES.contains(&type_name)
        || BsLodTriShape::TYPE_NAMES.contains(&type_name)
        || BsSegmentedTriShape::TYPE_NAMES.contains(&type_name)
}

pub(crate) fn decode_node(
    block: &aether_nif::Block,
    ctx: Ctx,
) -> std::result::Result<NiNode, aether_nif::Error> {
    decode_scene_node(block, ctx).map(|d| d.node)
}

/// Decode any scene container to its NiNode core plus the billboard mode
/// when the container is one.
/// A decoded container: the node itself, its billboard mode, and — for
/// `NiSwitchNode` — the index of the ONE child the game draws.
pub(crate) struct SceneNodeDecode {
    pub node: NiNode,
    pub billboard: Option<u16>,
    pub switch_index: Option<u32>,
}

pub(crate) fn decode_scene_node(
    block: &aether_nif::Block,
    ctx: Ctx,
) -> std::result::Result<SceneNodeDecode, aether_nif::Error> {
    let plain = |node: NiNode| SceneNodeDecode {
        node,
        billboard: None,
        switch_index: None,
    };
    if NiNode::TYPE_NAMES.contains(&block.type_name.as_str()) {
        return NiNode::decode(block, ctx).map(plain);
    }
    // BSOrderedNode only adds draw-order data past the NiNode prefix; for
    // scene purposes the inner node is the whole story.
    if block.type_name == "BSOrderedNode" {
        return aether_nif::typed::geometry::BsOrderedNode::decode(block, ctx)
            .map(|o| plain(o.node));
    }
    if block.type_name == "NiBillboardNode" {
        return aether_nif::typed::geometry::NiBillboardNode::decode(block, ctx).map(|b| {
            SceneNodeDecode {
                node: b.node,
                billboard: Some(b.billboard_mode),
                switch_index: None,
            }
        });
    }
    // Containers that only append data past the NiNode prefix: the scene
    // walk cares about the inner node alone — except NiSwitchNode, whose
    // stored index picks the ONE child the game draws (OnVisible @
    // 0x140C63AE0 culls `children[index]` with no loop). That index is
    // AUTHORED, not runtime state: every one of the 344 SSE / 164 LE
    // blocks stores 0 with exactly two children, and drawing both put
    // 592 extra shapes (167,404 triangles) on screen in SSE alone.
    if block.type_name == "BSValueNode" {
        return aether_nif::typed::geometry::BsValueNode::decode(block, ctx)
            .map(|v| plain(v.node));
    }
    if block.type_name == "NiSwitchNode" {
        return aether_nif::typed::geometry::NiSwitchNode::decode(block, ctx).map(|v| {
            SceneNodeDecode {
                node: v.node,
                billboard: None,
                switch_index: Some(v.index),
            }
        });
    }
    if block.type_name == "BSMultiBoundNode" {
        return aether_nif::typed::geometry::BsMultiBoundNode::decode(block, ctx)
            .map(|v| plain(v.node));
    }
    if block.type_name == "BSTreeNode" {
        return aether_nif::typed::geometry::BsTreeNode::decode(block, ctx)
            .map(|v| plain(v.node));
    }
    // The MPS root: a NiNode plus an emitter budget and a list of the
    // systems that spawn into its pool. For the scene walk the node is
    // the whole story; the controller that points back at it re-reads
    // the extras itself.
    if block.type_name == "BSMasterParticleSystem" {
        return aether_nif::typed::geometry::BsMasterParticleSystem::decode(block, ctx)
            .map(|v| plain(v.node));
    }
    if aether_nif::typed::geometry::BsRangeNode::TYPE_NAMES.contains(&block.type_name.as_str()) {
        return aether_nif::typed::geometry::BsRangeNode::decode(block, ctx)
            .map(|v| plain(v.node));
    }
    // Layout-identical type under another name (BSLeafAnimNode): decode
    // through a renamed copy so the type check stays strict elsewhere.
    let renamed = aether_nif::Block {
        type_name: NiNode::TYPE_NAMES[0].to_string(),
        data: block.data.clone(),
    };
    NiNode::decode(&renamed, ctx).map(plain)
}

/// What an `NiAlphaProperty` flag word means for blending.
///
/// `None` when blending is off. AlphaFunction (nif.xml 1094): 0 ONE,
/// 1 ZERO, 2 SRC_COLOR. A destination factor of ONE is additive;
/// SRC_COLOR (with src ZERO in all shipped data) is modulation, where
/// the result is dst x src.rgb and alpha does not participate.
/// Everything else renders acceptably as src-alpha.
///
/// Pulled out of the walk so it can be tested by CALLING it. The test
/// that stood here asserted `4109 & 1 == 1` and four more statements
/// of the same shape -- arithmetic on literals, touching no code in
/// this module at all. It could not fail for any change to the decode.
pub fn blend_from_alpha_flags(flags: u16) -> Option<BlendMode> {
    if flags & 1 == 0 {
        return None;
    }
    Some(match (flags >> 5) & 0xF {
        0 => BlendMode::Additive,
        2 => BlendMode::Multiply,
        _ => BlendMode::Alpha,
    })
}

/// Bit 9 of the same word: the alpha TEST, independent of blending.
pub fn alpha_test_enabled(flags: u16) -> bool {
    flags & (1 << 9) != 0
}

fn style_from_properties(
    file: &NifFile,
    ctx: Ctx,
    shader_property: BlockRef,
    alpha_property: BlockRef,
    warnings: &mut Vec<String>,
) -> ShapeStyle {
    let mut style = ShapeStyle::default();
    // A shape with NO shader property has no material to draw with:
    // the game creates no render pass for it at all, and Aether bound
    // the white fallback and drew it as a white sheet — 16,114 SSE
    // shapes in 12,966 files, 12,002 of them the terrain-LOD water
    // planes that `lod.rs` already had to special-case by an
    // empty-texture heuristic.
    style.no_material = shader_property.get().is_none();

    if let Some(i) = shader_property.get() {
        if let Some(block) = file.blocks.get(i) {
            match block.type_name.as_str() {
                "BSLightingShaderProperty" => {
                    if let Ok(shader) = BSLightingShaderProperty::decode(block, ctx) {
                        let (anims, colors) =
                            shader_anims_of(file, ctx, shader.net.controller);
                        style.shader_anims = anims;
                        style.shader_color_anims = colors;
                        style.refraction = shader.shader_flags1 & 0x8000 != 0;
                        style.no_vertex_alpha = shader.shader_flags1 & 0x8 == 0;
                        style.lighting.refraction_strength = shader.refraction_strength;
                        style.lighting.refraction_falloff =
                            shader.shader_flags1 & 0x1_0000 != 0;
                        style.double_sided = shader.shader_flags2 & 0x10 != 0;
                        style.depth_write = shader.shader_flags2 & 1 != 0;
                        style.tint = [1.0, 1.0, 1.0, shader.alpha.clamp(0.0, 1.0)];
                        // Material alpha below 1 forces blending even with no
                        // NiAlphaProperty — both reference viewers do this.
                        if shader.alpha < 1.0 {
                            style.blend = BlendMode::Alpha;
                        }
                        style.material.uv_offset = shader.uv_offset;
                        style.material.uv_scale = shader.uv_scale;
                        let mut slots: Vec<String> = Vec::new();
                        if let Some(ts) = shader.texture_set.get() {
                            if let Some(ts_block) = file.blocks.get(ts) {
                                if let Ok(set) = BSShaderTextureSet::decode(ts_block, ctx) {
                                    style.texture =
                                        set.diffuse().unwrap_or_default().to_string();
                                    slots = set.textures.clone();
                                }
                            }
                        }
                        let slot = |n: usize| -> String {
                            slots.get(n).cloned().unwrap_or_default()
                        };
                        // Tree sway (type 12 or SLSF2 bit 29 — vanilla only
                        // ships the flag): vertex alpha is the sway WEIGHT,
                        // not opacity; the game skips the vertex-alpha
                        // multiply on these (Lighting.hlsl:1474-1476).
                        style.tree_anim =
                            shader.shader_type == 12 || shader.shader_flags2 & (1 << 29) != 0;
                        let lit = &mut style.lighting;
                        lit.normal_texture = slot(1);
                        lit.msn = shader.shader_flags1 & (1 << 12) != 0;
                        // SLSF2 bit 6 = glow map; emissive only when
                        // Own_Emit (SLSF1 bit 22) is set.
                        if shader.shader_flags1 & (1 << 22) != 0 {
                            lit.emissive = shader.emissive_color;
                            lit.emissive_mult = shader.emissive_multiple;
                        }
                        if shader.shader_flags2 & (1 << 6) != 0 {
                            lit.glow_texture = slot(2);
                        }
                        if shader.shader_flags1 & 1 != 0 {
                            lit.specular_color = shader.specular_color;
                            lit.specular_strength = shader.specular_strength;
                            lit.glossiness = shader.glossiness;
                        }
                        // Parallax: type 3 is the key; bit 11 + a slot-3
                        // height map ride along on every shipping file.
                        if shader.shader_type == 3 && shader.shader_flags1 & (1 << 11) != 0 {
                            lit.height_texture = slot(3);
                        }
                        // Slot 7: backlight mask (SLSF2 bit 27) WINS over
                        // the MSN specular map (NifSkope renderer.cpp:779).
                        if shader.shader_flags2 & (1 << 27) != 0 {
                            lit.backlight_texture = slot(7);
                        } else if lit.msn && shader.shader_flags1 & 1 != 0 {
                            lit.specular_texture = slot(7);
                        }
                        lit.soft_lighting = shader.shader_flags2 & (1 << 25) != 0;
                        lit.rim_lighting = shader.shader_flags2 & (1 << 26) != 0;
                        if lit.soft_lighting || lit.rim_lighting {
                            lit.softrim_texture = slot(2);
                        }
                        lit.lighting_effect1 = shader.lighting_effect_1;
                        lit.lighting_effect2 = shader.lighting_effect_2;
                        // Type-keyed looks (verified against the 1.5.97
                        // decompile; technique map from 0x14132EAD0).
                        lit.facegen = shader.shader_type == 4;
                        lit.skin_tint = shader.shader_type == 5;
                        lit.hair_tint = shader.shader_type == 6;
                        lit.aniso = shader.shader_flags2 & (1 << 21) != 0;
                        // Type 18 (LODLandscapeNoise): the LOD-terrain
                        // break-up pass. Its normals decode through the
                        // ordinary MSN swizzle (all 28,752 corpus blocks
                        // set SLSF1 bit 12), and the game zeroes
                        // glossiness on it.
                        lit.lod_land_noise = shader.shader_type == 18;
                        if lit.lod_land_noise {
                            lit.glossiness = 0.0;
                        }
                        match &shader.tail {
                            LightingShaderTail::SkinTint { color }
                            | LightingShaderTail::HairTint { color } => {
                                lit.tint_color = *color;
                            }
                            _ => {}
                        }
                        // Env mapping: type 1 with SLSF1 bit 7, plus the SSE
                        // rule that MultiLayerParallax implies it
                        // (mesh-rendering.md §3.5).
                        use aether_nif::typed::shaders::LightingShaderTail;
                        let env_scale = match &shader.tail {
                            LightingShaderTail::EnvironmentMap { scale }
                                if shader.shader_flags1 & (1 << 7) != 0 =>
                            {
                                Some(*scale)
                            }
                            LightingShaderTail::MultiLayerParallax {
                                inner_layer_thickness,
                                refraction_scale,
                                inner_layer_texture_scale,
                                envmap_strength,
                            } => {
                                // Type 11: the inner-layer transmission
                                // (sk_multilayer.frag; same construction in
                                // the vanilla MULTI_LAYER_PARALLAX branch).
                                lit.inner_texture = slot(6);
                                lit.mlp_thickness = *inner_layer_thickness;
                                lit.mlp_refraction = *refraction_scale;
                                lit.mlp_inner_scale = *inner_layer_texture_scale;
                                Some(*envmap_strength)
                            }
                            LightingShaderTail::EyeEnvmap { cubemap_scale, .. }
                                if shader.shader_flags1 & (1 << 17) != 0 =>
                            {
                                Some(*cubemap_scale)
                            }
                            _ => None,
                        };
                        if let Some(scale) = env_scale {
                            lit.env_scale = scale;
                            lit.env_texture = slot(4);
                            lit.env_mask_texture = slot(5);
                        }
                    } else {
                        warnings.push(format!("shader property {i} did not decode"));
                    }
                }
                "BSEffectShaderProperty" => {
                    if let Ok(shader) = BSEffectShaderProperty::decode(block, ctx) {
                        style.effect = true;
                        style.texture = shader.source_texture.clone();
                        style.double_sided = shader.shader_flags2 & 0x10 != 0;
                        style.depth_write = shader.shader_flags2 & 1 != 0;
                        // The Refraction flag is INERT on effect shaders:
                        // BSEffectShaderProperty::GetRenderPasses
                        // (0x1412D5380) never tests bit 15, and zero
                        // vanilla effect blocks carry it. Such a shape
                        // renders as ordinary effect geometry.
                        let s = shader.base_color_scale;
                        style.tint = [
                            shader.base_color[0] * s,
                            shader.base_color[1] * s,
                            shader.base_color[2] * s,
                            shader.base_color[3],
                        ];
                        style.material = EffectMaterial {
                            uv_offset: shader.uv_offset,
                            uv_scale: shader.uv_scale,
                            water: 0,
                            greyscale_texture: shader.greyscale_texture.clone(),
                            greyscale_color: shader.shader_flags1 & 0x10 != 0,
                            greyscale_alpha: shader.shader_flags1 & 0x20 != 0,
                            use_falloff: shader.shader_flags1 & 0x40 != 0,
                            falloff: [
                                shader.falloff_start_angle,
                                shader.falloff_stop_angle,
                                shader.falloff_start_opacity,
                                shader.falloff_stop_opacity,
                            ],
                            base_color: shader.base_color,
                            base_color_scale: shader.base_color_scale,
                            soft_depth: if shader.shader_flags1 & 0x4000_0000 != 0 {
                                shader.soft_falloff_depth
                            } else {
                                0.0
                            },
                        };
                        let (anims, colors) =
                            shader_anims_of(file, ctx, shader.net.controller);
                        style.shader_anims = anims;
                        style.shader_color_anims = colors;
                    } else {
                        warnings.push(format!("effect shader {i} did not decode"));
                    }
                }
                "BSSkyShaderProperty" => {
                    use aether_nif::typed::shaders::BSSkyShaderProperty;
                    if let Ok(shader) = BSSkyShaderProperty::decode(block, ctx) {
                        // Unlit path for every sky type; the 0.83 VParams
                        // gain is folded into the bakes.
                        style.effect = true;
                        style.texture = shader.source_texture.clone();
                        match shader.sky_object_type {
                            2 => {
                                // Atmosphere dome: rgb = vc.r*Horizon +
                                // vc.g*SkyLower + vc.b*SkyUpper. The three
                                // colours are weather state in game;
                                // these are clear-day stand-ins.
                                style.sky_gradient = Some([
                                    [0.62, 0.68, 0.73],
                                    [0.37, 0.51, 0.70],
                                    [0.18, 0.33, 0.60],
                                ]);
                            }
                            3 => {
                                // Clouds: flat neutral tint x authored
                                // vertex alpha, blended. Texture/colour/
                                // scroll live in the weather system and
                                // the source texture is empty in every
                                // vanilla block.
                                style.blend = BlendMode::Alpha;
                                style.tint = [0.55, 0.55, 0.58, 1.0];
                            }
                            5 => {
                                // Stars: authored texture, the 1.5 gain
                                // of Sky.hlsl; height fade needs the
                                // game's eye convention — skipped.
                                style.blend = BlendMode::Alpha;
                                style.material.base_color_scale = 1.5;
                            }
                            7 => {
                                // MoonShadow: the texture verbatim with
                                // alpha-test discard.
                                style.alpha_test = Some(0.5);
                            }
                            other => warnings.push(format!(
                                "sky object type {other} is never authored \
                                 in shipping data; drawn as plain geometry"
                            )),
                        }
                    }
                }
                "BSWaterShaderProperty" => {
                    use aether_nif::typed::shaders::BSWaterShaderProperty;
                    if let Ok(shader) = BSWaterShaderProperty::decode(block, ctx) {
                        // Preview water: translucent DefaultWater-tinted
                        // surface with three scrolled normal layers.
                        // Reflection/refraction/displacement are
                        // screen-space passes over render targets the
                        // preview does not have — refused, documented.
                        style.effect = true;
                        style.blend = BlendMode::Alpha;
                        style.texture = "textures\\water\\defaultwater.dds".into();
                        style.material.water = 1
                            | ((shader.water_flags & 0x100 != 0) as u8) << 1
                            | ((shader.water_flags & 0x200 != 0) as u8) << 2;
                    }
                }
                other => warnings.push(format!("unsupported shader property {other}")),
            }
        }
    }

    if let Some(i) = alpha_property.get() {
        if let Some(block) = file.blocks.get(i) {
            if let Ok(alpha) = NiAlphaProperty::decode(block, ctx) {
                // BSNiAlphaPropertyTestRefController targets the ALPHA
                // property, so its chain hangs here, not off the shader.
                let (mut anims, _) = shader_anims_of(file, ctx, alpha.net.controller);
                style.shader_anims.append(&mut anims);
                let flags = alpha.flags;
                if let Some(mode) = blend_from_alpha_flags(flags) {
                    style.blend = mode;
                }
                if alpha_test_enabled(flags) {
                    style.alpha_test = Some(alpha.threshold as f32 / 255.0);
                }
            }
        }
    }

    // A refractive shape never draws colour; what its pass samples is the
    // NORMAL map, so that is the texture the mesh carries (slot 0 in these
    // files is Black.dds or a copy of the normal map — never meant to be
    // seen).
    if style.refraction {
        style.texture = style.lighting.normal_texture.clone();
    }
    style
}

fn extract_sse_shape(
    file: &NifFile,
    ctx: Ctx,
    // The block this shape IS. Threaded down rather than recovered later:
    // there is no way back from a `RenderMesh` to its block, and a shape
    // that quietly kept `None` would produce no warning anywhere.
    block: usize,
    shape: &BsTriShape,
    parent_node: Option<usize>,
    parent: &Transform,
    parent_hidden: bool,
    order_group: usize,
    scene: &mut MeshScene,
) {
    let name = resolve_name(file, shape.av.net.name);
    let hidden = parent_hidden || shape.av.flags & 1 != 0;
    if shape.skin.get().is_some() {
        scene
            .warnings
            .push(format!("{name}: skinned mesh, not rendered yet"));
        return;
    }
    if shape.vertices.is_empty() || shape.triangles.is_empty() {
        scene.warnings.push(format!(
            "{name}: shape has {} vertices and {} triangles — nothing to draw",
            shape.vertices.len(),
            shape.triangles.len()
        ));
        return;
    }

    let style = style_from_properties(
        file,
        ctx,
        shape.shader_property,
        shape.alpha_property,
        &mut scene.warnings,
    );
    if style.no_material {
        // Not a refusal — the shape is kept for picking, framing and
        // the Block panel. But drawing it as a white sheet was the
        // silence this rule exists to break.
        scene.warnings.push(format!(
            "{name}: no shader property, so nothing to draw it with; kept but not rendered"
        ));
    }
    let local = Transform::from_av(
        shape.av.translation,
        shape.av.rotation,
        shape.av.scale,
    );
    let world = parent.compose(&local);

    let has_colors = shape.attrs() & aether_nif::typed::geometry::vf::COLORS != 0;
    let mut vertices: Vec<MeshVertex> = shape
        .vertices
        .iter()
        .map(|v| MeshVertex {
            position: v.position,
            normal: v.normal_f32(),
            uv: v.uv_f32(),
            color: if has_colors { v.color_f32() } else { [1.0; 4] },
        })
        .collect();
    if style.tree_anim {
        for v in &mut vertices {
            v.color[3] = 1.0;
        }
    }
    if style.no_vertex_alpha {
        // SLSF1 bit 3 clear: the alpha channel of the vertex colours is
        // not opacity. Both the game and NifSkope force it to 1; keeping
        // the stored value deletes triangles under an alpha test.
        for v in &mut vertices {
            v.color[3] = 1.0;
        }
    }
    if let Some(g) = style.sky_gradient {
        // The atmosphere gradient bake: vc channels select the three sky
        // colours (x0.83, the hardcoded VParams gain).
        for v in &mut vertices {
            let (r, gr, b) = (v.color[0], v.color[1], v.color[2]);
            for c in 0..3 {
                v.color[c] = (r * g[0][c] + gr * g[1][c] + b * g[2][c]) * 0.83;
            }
        }
    }
    let indices = shape
        .triangles
        .iter()
        .flat_map(|t| t.iter().copied())
        .collect();

    // Creation-Kit markers: extracted (picking and framing want them)
    // but never drawn, like the game and NifSkope.
    let marker = name.to_ascii_lowercase().contains("editormarker");
    scene.meshes.push(RenderMesh {
        order_group,
        name,
        world: world.to_column_major(),
        node: parent_node,
        block: Some(block),
        local,
        vertices: std::sync::Arc::new(vertices),
        indices: std::sync::Arc::new(indices),
        segments: Vec::new(),
        texture: style.texture,
        tint: style.tint,
        blend: style.blend,
        alpha_test: style.alpha_test,
        double_sided: style.double_sided,
        depth_write: style.depth_write,
        no_material: style.no_material,
        effect: style.effect,
        refraction: style.refraction,
        // Creation-Kit markers: extracted (picking and framing want
        // them) but never drawn, like the game and NifSkope.
        marker,
        hidden,
        material: style.material,
        lighting: style.lighting,
        shader_anims: style.shader_anims,
        shader_color_anims: style.shader_color_anims,
    });
}

/// Decode any of the four LE geometry SHAPE kinds down to the
/// NiTriShape they all are underneath. BSLODTriShape and
/// BSSegmentedTriShape carry extra tables after the shape fields (LOD
/// sizes / a per-cell segment table); neither changes what a viewer
/// draws, and both were silently invisible until this existed —
/// 8,933 shapes in the LE corpus, 43 in SSE.
fn le_shape(
    block: &aether_nif::Block,
    ctx: Ctx,
) -> std::result::Result<NiTriShape, aether_nif::error::Error> {
    use aether_nif::typed::geometry::{BsLodTriShape, BsSegmentedTriShape};
    match block.type_name.as_str() {
        "BSLODTriShape" => BsLodTriShape::decode(block, ctx).map(|s| s.shape),
        "BSSegmentedTriShape" => BsSegmentedTriShape::decode(block, ctx).map(|s| s.shape),
        _ => NiTriShape::decode(block, ctx),
    }
}

/// Decode either LE geometry data kind into the triangle-list form the
/// extraction consumes; strips are unrolled here so every consumer sees
/// one shape of data.
fn le_mesh_data(
    file: &NifFile,
    ctx: Ctx,
    data_index: usize,
) -> std::result::Result<NiTriShapeData, String> {
    use aether_nif::typed::geometry::NiTriStripsData;
    let Some(block) = file.blocks.get(data_index) else {
        return Err(format!("mesh data block {data_index} is missing"));
    };
    match block.type_name.as_str() {
        "NiTriShapeData" => NiTriShapeData::decode(block, ctx).map_err(|e| e.to_string()),
        "NiTriStripsData" => {
            let d = NiTriStripsData::decode(block, ctx).map_err(|e| e.to_string())?;
            let triangles = d.unrolled_triangles();
            Ok(NiTriShapeData {
                group_id: d.group_id,
                keep_flags: d.keep_flags,
                compress_flags: d.compress_flags,
                has_vertices: d.has_vertices,
                vertices: d.vertices,
                bs_data_flags: d.bs_data_flags,
                material_crc: d.material_crc,
                has_normals: d.has_normals,
                normals: d.normals,
                tangents: d.tangents,
                bitangents: d.bitangents,
                bound: d.bound,
                has_vertex_colors: d.has_vertex_colors,
                vertex_colors: d.vertex_colors,
                uv_sets: d.uv_sets,
                consistency_flags: d.consistency_flags,
                additional_data: d.additional_data,
                num_triangle_points: 0,
                has_triangles: !triangles.is_empty(),
                triangles,
                match_groups: Vec::new(),
            })
        }
        other => Err(format!("mesh data is {other}, not LE geometry data")),
    }
}

fn extract_le_shape(
    file: &NifFile,
    ctx: Ctx,
    block: usize,
    shape: &NiTriShape,
    parent_node: Option<usize>,
    parent: &Transform,
    parent_hidden: bool,
    order_group: usize,
    scene: &mut MeshScene,
) {
    let name = resolve_name(file, shape.av.net.name);
    let hidden = parent_hidden || shape.av.flags & 1 != 0;
    if shape.skin_instance.get().is_some() {
        scene
            .warnings
            .push(format!("{name}: skinned mesh, not rendered yet"));
        return;
    }
    let Some(data_index) = shape.data.get() else {
        return;
    };
    let Some(data_block) = file.blocks.get(data_index) else {
        return;
    };
    let _ = data_block;
    let data = match le_mesh_data(file, ctx, data_index) {
        Ok(d) => d,
        Err(e) => {
            scene.warnings.push(format!("{name}: {e}"));
            return;
        }
    };
    if data.vertices.is_empty() || data.triangles.is_empty() {
        // Silence is how BSLODTriShape stayed invisible for months.
        scene.warnings.push(format!(
            "{name}: shape has {} vertices and {} triangles — nothing to draw",
            data.vertices.len(),
            data.triangles.len()
        ));
        return;
    }

    let style = style_from_properties(
        file,
        ctx,
        shape.shader_property,
        shape.alpha_property,
        &mut scene.warnings,
    );
    if style.no_material {
        // Not a refusal — the shape is kept for picking, framing and
        // the Block panel. But drawing it as a white sheet was the
        // silence this rule exists to break.
        scene.warnings.push(format!(
            "{name}: no shader property, so nothing to draw it with; kept but not rendered"
        ));
    }
    let local = Transform::from_av(
        shape.av.translation,
        shape.av.rotation,
        shape.av.scale,
    );
    let world = parent.compose(&local);

    let uv = data.uv_sets.first();
    let mut vertices: Vec<MeshVertex> = (0..data.vertices.len())
        .map(|i| MeshVertex {
            position: data.vertices[i],
            normal: data.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]),
            uv: uv.and_then(|set| set.get(i)).copied().unwrap_or([0.0, 0.0]),
            color: data.vertex_colors.get(i).copied().unwrap_or([1.0; 4]),
        })
        .collect();
    if style.tree_anim {
        for v in &mut vertices {
            v.color[3] = 1.0;
        }
    }
    if style.no_vertex_alpha {
        // SLSF1 bit 3 clear: the alpha channel of the vertex colours is
        // not opacity. Both the game and NifSkope force it to 1; keeping
        // the stored value deletes triangles under an alpha test.
        for v in &mut vertices {
            v.color[3] = 1.0;
        }
    }
    if let Some(g) = style.sky_gradient {
        // The atmosphere gradient bake: vc channels select the three sky
        // colours (x0.83, the hardcoded VParams gain).
        for v in &mut vertices {
            let (r, gr, b) = (v.color[0], v.color[1], v.color[2]);
            for c in 0..3 {
                v.color[c] = (r * g[0][c] + gr * g[1][c] + b * g[2][c]) * 0.83;
            }
        }
    }
    let indices = data
        .triangles
        .iter()
        .flat_map(|t| t.iter().copied())
        .collect();

    // Creation-Kit markers: extracted (picking and framing want them)
    // but never drawn, like the game and NifSkope.
    let marker = name.to_ascii_lowercase().contains("editormarker");
    scene.meshes.push(RenderMesh {
        order_group,
        name,
        world: world.to_column_major(),
        node: parent_node,
        block: Some(block),
        local,
        vertices: std::sync::Arc::new(vertices),
        indices: std::sync::Arc::new(indices),
        segments: Vec::new(),
        texture: style.texture,
        tint: style.tint,
        blend: style.blend,
        alpha_test: style.alpha_test,
        double_sided: style.double_sided,
        depth_write: style.depth_write,
        no_material: style.no_material,
        effect: style.effect,
        refraction: style.refraction,
        // Creation-Kit markers: extracted (picking and framing want
        // them) but never drawn, like the game and NifSkope.
        marker,
        hidden,
        material: style.material,
        lighting: style.lighting,
        shader_anims: style.shader_anims,
        shader_color_anims: style.shader_color_anims,
    });
}

/// One-time CPU skinning at bind pose, the reference viewers' algorithm
/// (verified multi-agent research 2026-08-06, mesh-rendering.md §1.3):
/// per-bone matrix = bone node's world transform ∘ the stored skin-to-bone
/// transform; positions are the weighted sum through those matrices,
/// normals go through the rotation part only and renormalize. The shape's
/// own node-hierarchy transform is NOT applied (`transformRigid = false` —
/// the game replaces the world matrix with blended bone matrices).
///
/// The result is expressed in SKIN space with `world` set to the inverse
/// of NiSkinData's global→skin transform, so the shader's world rotation
/// doubles as the blended-bone rotation MSN normal maps need at bind pose.
#[allow(clippy::too_many_arguments)]
fn extract_skinned_shape(
    file: &NifFile,
    ctx: Ctx,
    index: usize,
    parent_node: Option<usize>,
    parent_hidden: bool,
    node_worlds: &std::collections::HashMap<usize, Transform>,
    block_to_node: &std::collections::HashMap<usize, usize>,
    order_group: usize,
    scene: &mut MeshScene,
) {
    use aether_nif::typed::geometry::{
        half_to_f32, BsDynamicTriShape, BsSubIndexTriShape, NiSkinData, NiSkinPartition,
    };

    let Some(block) = file.blocks.get(index) else {
        return;
    };
    // Decode the shape by its concrete type; the dynamic variant carries
    // its positions on the shape as Vector4s.
    let (shape, dynamic): (BsTriShape, Vec<[f32; 4]>) = if block.type_name == "BSDynamicTriShape" {
        match BsDynamicTriShape::decode(block, ctx) {
            Ok(d) => (d.shape, d.dynamic),
            Err(e) => {
                scene
                    .warnings
                    .push(format!("BSDynamicTriShape {index}: {e}"));
                return;
            }
        }
    } else if block.type_name == "BSSubIndexTriShape" {
        // Object LOD geometry. The segment table says which cell each
        // run of triangles belongs to; the mesh itself draws exactly
        // like a BSTriShape, which is why this arm only has to unwrap
        // it.
        match BsSubIndexTriShape::decode(block, ctx) {
            Ok(s) => (s.shape, Vec::new()),
            Err(e) => {
                scene
                    .warnings
                    .push(format!("BSSubIndexTriShape {index}: {e}"));
                return;
            }
        }
    } else if block.type_name == "BSTriShape" {
        match BsTriShape::decode(block, ctx) {
            Ok(s) => (s, Vec::new()),
            Err(e) => {
                scene.warnings.push(format!("BSTriShape {index}: {e}"));
                return;
            }
        }
    } else {
        // LE: NiTriShape + NiTriShapeData + partition weights.
        extract_skinned_le(
            file,
            ctx,
            index,
            parent_node,
            parent_hidden,
            node_worlds,
            block_to_node,
            order_group,
            scene,
        );
        return;
    };

    let name = resolve_name(file, shape.av.net.name);
    let hidden = parent_hidden || shape.av.flags & 1 != 0;
    let Some(skin_index) = shape.skin.get() else {
        return;
    };
    let instance = match decode_skin_instance(file, ctx, skin_index) {
        Ok(i) => i,
        Err(e) => {
            scene.warnings.push(format!("{name}: {e}"));
            return;
        }
    };
    let skin_data = instance
        .data
        .get()
        .and_then(|i| file.blocks.get(i))
        .and_then(|b| NiSkinData::decode(b, ctx).ok());
    let Some(partition) = instance
        .skin_partition
        .get()
        .and_then(|i| file.blocks.get(i))
        .and_then(|b| NiSkinPartition::decode(b, ctx).ok())
    else {
        scene
            .warnings
            .push(format!("{name}: skinned shape without a usable NiSkinPartition"));
        return;
    };
    if partition.vertices.is_empty() {
        scene
            .warnings
            .push(format!("{name}: skinned shape's partition stores no vertices"));
        return;
    }

    let bones = bone_matrices(&instance, skin_data.as_ref(), node_worlds);
    let has_colors = partition.vertex_desc >> 44 & aether_nif::typed::geometry::vf::COLORS != 0;

    // global→skin from NiSkinData (identity when absent), so the mesh's
    // world matrix carries skin→global.
    let to_skin = skin_data
        .as_ref()
        .map(|d| nif_transform(&d.skin_transform))
        .unwrap_or(Transform::IDENTITY);
    let world = to_skin.inverse();

    let mut vertices: Vec<MeshVertex> = Vec::with_capacity(partition.vertices.len());
    let mut raw: Vec<([f32; 3], [f32; 3], [u16; 4], [f32; 4])> =
        Vec::with_capacity(partition.vertices.len());
    for (i, v) in partition.vertices.iter().enumerate() {
        let raw_pos = dynamic
            .get(i)
            .map(|d| [d[0], d[1], d[2]])
            .unwrap_or(v.position);
        let raw_normal = v.normal_f32();
        let mut weights = [0.0f32; 4];
        let mut pos = [0.0f32; 3];
        let mut normal = [0.0f32; 3];
        let mut total = 0.0f32;
        for k in 0..4 {
            let w = half_to_f32(v.bone_weights[k]);
            weights[k] = w;
            if w <= 0.0 {
                continue;
            }
            // SSE buffer indices are skin-instance-global.
            let Some(m) = bones.get(v.bone_indices[k] as usize) else {
                continue;
            };
            let p = m.transform_point(raw_pos);
            let n = m.rotate(raw_normal);
            for a in 0..3 {
                pos[a] += p[a] * w;
                normal[a] += n[a] * w;
            }
            total += w;
        }
        if total <= 0.0 {
            pos = raw_pos;
            normal = raw_normal;
        }
        raw.push((
            raw_pos,
            raw_normal,
            [
                v.bone_indices[0] as u16,
                v.bone_indices[1] as u16,
                v.bone_indices[2] as u16,
                v.bone_indices[3] as u16,
            ],
            weights,
        ));
        vertices.push(MeshVertex {
            position: to_skin.transform_point(pos),
            normal: normalize3(to_skin.rotate(normal)),
            uv: v.uv_f32(),
            color: if has_colors { v.color_f32() } else { [1.0; 4] },
        });
    }

    // The renderable faces are the per-partition TRUE triangles (shape-
    // vertex indices), concatenated — NifSkope's rule.
    let indices: Vec<u16> = partition
        .partitions
        .iter()
        .flat_map(|p| p.true_triangles.iter().flat_map(|t| t.iter().copied()))
        .collect();
    if vertices.is_empty() || indices.is_empty() {
        scene.warnings.push(format!(
            "{name}: skinned shape produced {} vertices and {} indices — nothing to draw",
            vertices.len(),
            indices.len()
        ));
        return;
    }

    push_skinned_mesh(
        file,
        ctx,
        index,
        name,
        shape.shader_property,
        shape.alpha_property,
        parent_node,
        hidden,
        world,
        vertices,
        indices,
        order_group,
        scene,
    );
    // The re-pose inputs, tied to the mesh just pushed. Bones resolve to
    // scene nodes when their blocks were walked; anything else keeps its
    // import world (a static bone re-poses as bind).
    let skin_bones: Vec<SkinBone> = instance
        .bones
        .iter()
        .enumerate()
        .map(|(i, bone_ref)| {
            let block = bone_ref.get();
            SkinBone {
                node: block.and_then(|b| block_to_node.get(&b)).copied(),
                bind_world: block
                    .and_then(|b| node_worlds.get(&b))
                    .copied()
                    .unwrap_or(Transform::IDENTITY),
                skin_to_bone: skin_data
                    .as_ref()
                    .and_then(|d| d.bones.get(i))
                    .map(|b| nif_transform(&b.skin_to_bone))
                    .unwrap_or(Transform::IDENTITY),
            }
        })
        .collect();
    scene.skinned.push(SkinnedMesh {
        mesh: scene.meshes.len() - 1,
        bones: skin_bones,
        to_skin,
        raw,
    });
}

/// LE skinned path: geometry from NiTriShapeData, weights from the
/// partition's per-partition arrays (vertex map + partition-local bone
/// indices; first partition wins per vertex — NifSkope glmesh.cpp:885).
fn extract_skinned_le(
    file: &NifFile,
    ctx: Ctx,
    index: usize,
    parent_node: Option<usize>,
    parent_hidden: bool,
    node_worlds: &std::collections::HashMap<usize, Transform>,
    block_to_node: &std::collections::HashMap<usize, usize>,
    order_group: usize,
    scene: &mut MeshScene,
) {
    use aether_nif::typed::geometry::{NiSkinData, NiSkinPartition};

    let Some(block) = file.blocks.get(index) else {
        return;
    };
    let shape = match NiTriShape::decode(block, ctx) {
        Ok(s) => s,
        Err(e) => {
            scene.warnings.push(format!("NiTriShape {index}: {e}"));
            return;
        }
    };
    let name = resolve_name(file, shape.av.net.name);
    let hidden = parent_hidden || shape.av.flags & 1 != 0;
    let Some(mut data) = shape
        .data
        .get()
        .and_then(|i| le_mesh_data(file, ctx, i).ok())
    else {
        scene.warnings.push(format!("{name}: mesh data did not decode"));
        return;
    };
    if data.vertices.is_empty() {
        scene
            .warnings
            .push(format!("{name}: skinned shape has no vertices"));
        return;
    }
    let Some(skin_index) = shape.skin_instance.get() else {
        return;
    };
    let instance = match decode_skin_instance(file, ctx, skin_index) {
        Ok(i) => i,
        Err(e) => {
            scene.warnings.push(format!("{name}: {e}"));
            return;
        }
    };
    let skin_data = instance
        .data
        .get()
        .and_then(|i| file.blocks.get(i))
        .and_then(|b| NiSkinData::decode(b, ctx).ok());
    let Some(partition) = instance
        .skin_partition
        .get()
        .and_then(|i| file.blocks.get(i))
        .and_then(|b| NiSkinPartition::decode(b, ctx).ok())
    else {
        scene
            .warnings
            .push(format!("{name}: skinned shape without a usable NiSkinPartition"));
        return;
    };

    // LE keeps a skinned shape's VERTICES in NiTriShapeData but its
    // TRIANGLES may live only in the partition — the Dawnguard aspens
    // do exactly that (729 vertices, 0 triangles, 528 in three
    // partitions). Bailing on the empty list dropped 108 LE shapes in
    // 54 files with no warning, and on a switch node it dropped the very
    // child the game draws. Partition indices are partition-LOCAL and go
    // through the vertex map.
    if data.triangles.is_empty() {
        for part in &partition.partitions {
            for tri in &part.triangles {
                let mapped: Vec<u16> = tri
                    .iter()
                    .map(|&i| {
                        part.vertex_map
                            .get(i as usize)
                            .copied()
                            .unwrap_or(u16::MAX)
                    })
                    .collect();
                if mapped.iter().all(|&v| (v as usize) < data.vertices.len()) {
                    data.triangles.push([mapped[0], mapped[1], mapped[2]]);
                }
            }
        }
        if data.triangles.is_empty() {
            scene.warnings.push(format!(
                "{name}: skinned shape has no triangles in its data block or its partition — nothing to draw"
            ));
            return;
        }
    }

    let bones = bone_matrices(&instance, skin_data.as_ref(), node_worlds);
    let to_skin = skin_data
        .as_ref()
        .map(|d| nif_transform(&d.skin_transform))
        .unwrap_or(Transform::IDENTITY);
    let world = to_skin.inverse();

    // Per-shape-vertex influences, first partition wins.
    let mut influences: Vec<Option<Vec<(usize, f32)>>> = vec![None; data.vertices.len()];
    for part in &partition.partitions {
        let wpv = part.num_weights_per_vertex as usize;
        for (local, shape_vertex) in part.vertex_map.iter().enumerate() {
            let slot = influences.get_mut(*shape_vertex as usize);
            let Some(slot) = slot else { continue };
            if slot.is_some() {
                continue;
            }
            let mut list = Vec::with_capacity(wpv);
            for k in 0..wpv {
                let w = part
                    .vertex_weights
                    .get(local * wpv + k)
                    .copied()
                    .unwrap_or(0.0);
                if w <= 0.0 {
                    continue;
                }
                let bone_local = part.bone_indices.get(local * wpv + k).copied().unwrap_or(0);
                let Some(bone) = part.bones.get(bone_local as usize) else {
                    continue;
                };
                list.push((*bone as usize, w));
            }
            *slot = Some(list);
        }
    }

    let uv = data.uv_sets.first();
    let vertices: Vec<MeshVertex> = (0..data.vertices.len())
        .map(|i| {
            let raw_pos = data.vertices[i];
            let raw_normal = data.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]);
            let mut pos = [0.0f32; 3];
            let mut normal = [0.0f32; 3];
            let mut total = 0.0f32;
            if let Some(Some(list)) = influences.get(i) {
                for (bone, w) in list {
                    let Some(m) = bones.get(*bone) else { continue };
                    let p = m.transform_point(raw_pos);
                    let n = m.rotate(raw_normal);
                    for a in 0..3 {
                        pos[a] += p[a] * w;
                        normal[a] += n[a] * w;
                    }
                    total += w;
                }
            }
            if total <= 0.0 {
                pos = raw_pos;
                normal = raw_normal;
            }
            MeshVertex {
                position: to_skin.transform_point(pos),
                normal: normalize3(to_skin.rotate(normal)),
                uv: uv.and_then(|set| set.get(i)).copied().unwrap_or([0.0, 0.0]),
                color: data.vertex_colors.get(i).copied().unwrap_or([1.0; 4]),
            }
        })
        .collect();
    let indices: Vec<u16> = data
        .triangles
        .iter()
        .flat_map(|t| t.iter().copied())
        .collect();

    push_skinned_mesh(
        file,
        ctx,
        index,
        name,
        shape.shader_property,
        shape.alpha_property,
        parent_node,
        hidden,
        world,
        vertices,
        indices,
        order_group,
        scene,
    );

    // The re-pose inputs, exactly as the SSE tail builds them — until
    // now `MeshScene::skinned` was filled by the SSE path ALONE, so
    // every LE skinned shape stood at bind pose while its bones
    // animated (6,893 LE files). The influence lists above already
    // carry what the SSE partition hands over 4-wide; a vertex with
    // more than four influences keeps its four HEAVIEST, which is
    // what the format's own 4-wide storage does on the SSE side.
    let skin_bones: Vec<SkinBone> = instance
        .bones
        .iter()
        .enumerate()
        .map(|(i, bone_ref)| {
            let block = bone_ref.get();
            SkinBone {
                node: block.and_then(|b| block_to_node.get(&b)).copied(),
                bind_world: block
                    .and_then(|b| node_worlds.get(&b))
                    .copied()
                    .unwrap_or(Transform::IDENTITY),
                skin_to_bone: skin_data
                    .as_ref()
                    .and_then(|d| d.bones.get(i))
                    .map(|b| nif_transform(&b.skin_to_bone))
                    .unwrap_or(Transform::IDENTITY),
            }
        })
        .collect();
    let raw: Vec<([f32; 3], [f32; 3], [u16; 4], [f32; 4])> = (0..data.vertices.len())
        .map(|i| {
            let mut idx = [0u16; 4];
            let mut w = [0.0f32; 4];
            if let Some(Some(list)) = influences.get(i) {
                let mut list = list.clone();
                list.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                });
                for (k, (bone, weight)) in list.into_iter().take(4).enumerate() {
                    idx[k] = bone as u16;
                    w[k] = weight;
                }
            }
            (
                data.vertices[i],
                data.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]),
                idx,
                w,
            )
        })
        .collect();
    scene.skinned.push(SkinnedMesh {
        mesh: scene.meshes.len() - 1,
        bones: skin_bones,
        to_skin,
        raw,
    });
}

/// Decode the skin instance whether plain or dismember (the dismember
/// list adds body parts the renderer does not consume).
fn decode_skin_instance(
    file: &NifFile,
    ctx: Ctx,
    index: usize,
) -> std::result::Result<aether_nif::typed::geometry::NiSkinInstance, String> {
    use aether_nif::typed::geometry::{BsDismemberSkinInstance, NiSkinInstance};
    let Some(block) = file.blocks.get(index) else {
        return Err(format!("skin instance block {index} is missing"));
    };
    match block.type_name.as_str() {
        "NiSkinInstance" => {
            NiSkinInstance::decode(block, ctx).map_err(|e| format!("skin instance: {e}"))
        }
        "BSDismemberSkinInstance" => BsDismemberSkinInstance::decode(block, ctx)
            .map(|d| d.skin)
            .map_err(|e| format!("skin instance: {e}")),
        other => Err(format!("unsupported skin instance type {other}")),
    }
}

/// Per-bone matrices: bone node world ∘ stored skin-to-bone. A bone whose
/// node is missing from the walked scene gets an identity world (NifSkope
/// leaves `boneTrans = view` in that case).
fn bone_matrices(
    instance: &aether_nif::typed::geometry::NiSkinInstance,
    skin_data: Option<&aether_nif::typed::geometry::NiSkinData>,
    node_worlds: &std::collections::HashMap<usize, Transform>,
) -> Vec<Transform> {
    instance
        .bones
        .iter()
        .enumerate()
        .map(|(i, bone_ref)| {
            let bone_world = bone_ref
                .get()
                .and_then(|b| node_worlds.get(&b))
                .copied()
                .unwrap_or(Transform::IDENTITY);
            let skin_to_bone = skin_data
                .and_then(|d| d.bones.get(i))
                .map(|b| nif_transform(&b.skin_to_bone))
                .unwrap_or(Transform::IDENTITY);
            bone_world.compose(&skin_to_bone)
        })
        .collect()
}

fn nif_transform(t: &aether_nif::typed::geometry::NifTransform) -> Transform {
    Transform {
        rot: t.rotation,
        t: t.translation,
        s: t.scale,
    }
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Shared tail of both skinned paths: style, refraction gate, push.
#[allow(clippy::too_many_arguments)]
fn push_skinned_mesh(
    file: &NifFile,
    ctx: Ctx,
    block: usize,
    name: String,
    shader_property: BlockRef,
    alpha_property: BlockRef,
    _parent_node: Option<usize>,
    hidden: bool,
    world: Transform,
    mut vertices: Vec<MeshVertex>,
    indices: Vec<u16>,
    order_group: usize,
    scene: &mut MeshScene,
) {
    let style = style_from_properties(file, ctx, shader_property, alpha_property, &mut scene.warnings);
    if style.tree_anim {
        for v in &mut vertices {
            v.color[3] = 1.0;
        }
    }
    if style.no_vertex_alpha {
        // SLSF1 bit 3 clear: the alpha channel of the vertex colours is
        // not opacity. Both the game and NifSkope force it to 1; keeping
        // the stored value deletes triangles under an alpha test.
        for v in &mut vertices {
            v.color[3] = 1.0;
        }
    }
    if let Some(g) = style.sky_gradient {
        // The atmosphere gradient bake: vc channels select the three sky
        // colours (x0.83, the hardcoded VParams gain).
        for v in &mut vertices {
            let (r, gr, b) = (v.color[0], v.color[1], v.color[2]);
            for c in 0..3 {
                v.color[c] = (r * g[0][c] + gr * g[1][c] + b * g[2][c]) * 0.83;
            }
        }
    }
    // Creation-Kit markers: extracted (picking and framing want them)
    // but never drawn, like the game and NifSkope.
    let marker = name.to_ascii_lowercase().contains("editormarker");
    // Every shape drawn as a skin, recorded HERE because this is the tail
    // both skinned paths share. `MeshScene::skinned` is the SSE-only
    // re-pose input, so anything keyed on it is blind to LE — which is
    // how an LE skinned shape came to accept a transform edit the game
    // ignores instead of refusing it.
    scene.skinned_blocks.push(block);
    scene.meshes.push(RenderMesh {
        order_group,
        name,
        world: world.to_column_major(),
        // Bind pose only: sequences that move bones do not re-pose skinned
        // shapes (known gap), so the mesh does not follow a scene node.
        node: None,
        block: Some(block),
        local: Transform::IDENTITY,
        vertices: std::sync::Arc::new(vertices),
        indices: std::sync::Arc::new(indices),
        segments: Vec::new(),
        texture: style.texture,
        tint: style.tint,
        blend: style.blend,
        alpha_test: style.alpha_test,
        double_sided: style.double_sided,
        depth_write: style.depth_write,
        no_material: style.no_material,
        effect: style.effect,
        refraction: style.refraction,
        // Creation-Kit markers: extracted (picking and framing want
        // them) but never drawn, like the game and NifSkope.
        marker,
        hidden,
        material: style.material,
        lighting: style.lighting,
        shader_anims: style.shader_anims,
        shader_color_anims: style.shader_color_anims,
    });
}

fn resolve_name(file: &NifFile, name: aether_nif::typed::StringIdx) -> String {
    name.resolve(&file.header).unwrap_or("(unnamed)").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_rows_compose_like_the_reference_implementations() {
        // 90 degrees about Z as stored in a file (rows): X maps to +Y under
        // column-vector math. Loading rows as columns would send X to -Y —
        // the exact inversion the research doc warns about.
        let rot_z = Transform {
            rot: [0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            t: [0.0; 3],
            s: 1.0,
        };
        let v = rot_z.rotate([1.0, 0.0, 0.0]);
        assert!((v[0]).abs() < 1e-6 && (v[1] - 1.0).abs() < 1e-6, "{v:?}");

        // Child translation is scaled by the parent, then rotated: the
        // formula T = Tp + Rp*(Tc*sp), pinned with numbers.
        let parent = Transform {
            rot: rot_z.rot,
            t: [10.0, 0.0, 0.0],
            s: 2.0,
        };
        let child = Transform {
            rot: Transform::IDENTITY.rot,
            t: [1.0, 0.0, 0.0],
            s: 3.0,
        };
        let world = parent.compose(&child);
        // (1,0,0)*2 rotated 90° about Z = (0,2,0); plus (10,0,0).
        assert!((world.t[0] - 10.0).abs() < 1e-6, "{:?}", world.t);
        assert!((world.t[1] - 2.0).abs() < 1e-6, "{:?}", world.t);
        assert!((world.s - 6.0).abs() < 1e-6);
    }

    #[test]
    fn column_major_matrix_multiplies_like_the_transform() {
        // The 4x4 the GPU gets must move points exactly like the CPU-side
        // transform, or meshes render somewhere else than they test.
        let t = Transform {
            rot: [0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            t: [5.0, -3.0, 2.0],
            s: 2.0,
        };
        let m = t.to_column_major();
        let p = [1.0f32, 2.0, 3.0];
        let cpu = {
            let r = t.rotate([p[0] * t.s, p[1] * t.s, p[2] * t.s]);
            [r[0] + t.t[0], r[1] + t.t[1], r[2] + t.t[2]]
        };
        let gpu = [
            m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
            m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
            m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
        ];
        for i in 0..3 {
            assert!((cpu[i] - gpu[i]).abs() < 1e-5, "{cpu:?} vs {gpu:?}");
        }
    }

        /// `from_column_major` must undo `to_column_major` exactly.
    ///
    /// The Block panel reads a shape's world transform back out of the
    /// matrix the renderer already baked — there is no other source for
    /// it, because a shape is not a scene node and has no parent chain
    /// of its own. A transposed inverse reads a rotated shape as turned
    /// the other way and nothing else notices.
        /// `edit_local` must produce the local that yields the intended
    /// world, for all three kinds of delta, under a NON-identity parent.
    ///
    /// The parent is the point: `local.compose(&parent)` — the flipped
    /// order — still puts a root-level node exactly where it belongs, so
    /// a test on a single node passes a solve that is wrong for every
    /// child in the file.
    #[test]
    fn edit_local_produces_the_world_that_was_asked_for() {
        let mut parent = Transform::IDENTITY;
        parent.rot = Transform::rows_from_euler(0.4, -0.9, 1.7);
        parent.t = [120.0, -35.0, 8.5];
        parent.s = 2.0;
        let mut local = Transform::IDENTITY;
        local.rot = Transform::rows_from_euler(-0.2, 0.6, 0.1);
        local.t = [4.0, 9.0, -2.0];
        local.s = 0.5;
        let world = parent.compose(&local);

        // --- move: the world origin shifts by exactly the delta -------
        let mut mv = Transform::IDENTITY;
        mv.t = [10.0, -4.0, 3.0];
        let moved = Transform::edit_local(&parent, &world, world.t, &mv);
        let got = parent.compose(&moved);
        for k in 0..3 {
            assert!(
                (got.t[k] - (world.t[k] + mv.t[k])).abs() < 1e-3,
                "move: axis {k} landed at {} not {}",
                got.t[k],
                world.t[k] + mv.t[k]
            );
        }
        assert!((got.s - world.s).abs() < 1e-5, "a move must not rescale");

        // --- rotate about the object's OWN origin: it must not move ---
        let mut rot = Transform::IDENTITY;
        rot.rot = Transform::rows_from_euler(0.0, 0.0, std::f32::consts::FRAC_PI_2);
        let turned = Transform::edit_local(&parent, &world, world.t, &rot);
        let got = parent.compose(&turned);
        for k in 0..3 {
            assert!(
                (got.t[k] - world.t[k]).abs() < 1e-3,
                "rotating about its own origin translated it on axis {k}"
            );
        }
        // ...and it really turned: the world rotation is dR . R.
        let want = rot.compose(&world);
        for k in 0..9 {
            assert!((got.rot[k] - want.rot[k]).abs() < 1e-4, "rot[{k}]");
        }

        // --- rotate about a DIFFERENT pivot: it must swing around it --
        let q = [world.t[0] + 100.0, world.t[1], world.t[2]];
        let swung = parent.compose(&Transform::edit_local(&parent, &world, q, &rot));
        let r0 = (world.t[0] - q[0]).hypot(world.t[1] - q[1]);
        let r1 = (swung.t[0] - q[0]).hypot(swung.t[1] - q[1]);
        assert!((r0 - r1).abs() < 1e-2, "the distance to the pivot must be preserved");
        assert!(
            (swung.t[0] - world.t[0]).abs() > 50.0,
            "an off-origin pivot must actually move the object"
        );

        // --- scale ----------------------------------------------------
        let mut sc = Transform::IDENTITY;
        sc.s = 3.0;
        let scaled = parent.compose(&Transform::edit_local(&parent, &world, world.t, &sc));
        assert!((scaled.s - world.s * 3.0).abs() < 1e-4);
        for k in 0..3 {
            assert!(
                (scaled.t[k] - world.t[k]).abs() < 1e-3,
                "scaling about its own origin moved it"
            );
        }

        // --- identity delta is a no-op --------------------------------
        let same = Transform::edit_local(&parent, &world, world.t, &Transform::IDENTITY);
        for k in 0..3 {
            assert!((same.t[k] - local.t[k]).abs() < 1e-3);
        }
        assert!((same.s - local.s).abs() < 1e-5);
    }

    #[test]
    fn a_baked_matrix_reads_back_as_the_transform_that_made_it() {
        let mut t = Transform::IDENTITY;
        t.rot = Transform::rows_from_euler(0.3, -1.1, 2.4);
        t.t = [11.5, -3.25, 700.0];
        t.s = 1.75;
        let back = Transform::from_column_major(&t.to_column_major());
        for i in 0..9 {
            assert!((back.rot[i] - t.rot[i]).abs() < 1e-5, "rot[{i}] {} vs {}", back.rot[i], t.rot[i]);
        }
        for i in 0..3 {
            assert!((back.t[i] - t.t[i]).abs() < 1e-4);
        }
        assert!((back.s - t.s).abs() < 1e-5, "scale {} vs {}", back.s, t.s);

        // A degenerate matrix must not fill the rotation with NaN.
        let zero = Transform::from_column_major(&[0.0; 16]);
        assert!(zero.rot.iter().all(|v| v.is_finite()));
        assert_eq!(zero.s, 1.0);
    }

    #[test]
    fn animated_worlds_recompose_the_node_chain() {
        // Root -> arm node at (1,0,0) -> mesh offset another (1,0,0).
        // Rotating the arm 90° about Z must swing the mesh to (1,1,0):
        // the whole point of keeping the tree instead of baked matrices.
        let arm_local = Transform {
            rot: Transform::IDENTITY.rot,
            t: [1.0, 0.0, 0.0],
            s: 1.0,
        };
        let scene = MeshScene {
            backdrop_from: None,
            sources: Vec::new(),
            lightning: Vec::new(),
            hk_rest_hidden: std::collections::HashSet::new(),
            skinned: Vec::new(),
            skinned_blocks: Vec::new(),
            node_anims: Vec::new(),
            nodes: vec![
                SceneNode {
                    block: 1,
                    name: "Root".into(),
                    parent: None,
                    local: Transform::IDENTITY,
                    billboard: None,
                },
                SceneNode {
                    block: 0,
                    name: "Arm".into(),
                    parent: Some(0),
                    local: arm_local,
                    billboard: None,
                },
            ],
            meshes: vec![RenderMesh {
                no_material: false,
                depth_write: true,
                order_group: 0,
                block: None,
                name: "blade".into(),
                world: Transform::IDENTITY.to_column_major(),
                node: Some(1),
                local: arm_local,
                vertices: std::sync::Arc::new(Vec::new()),
                indices: std::sync::Arc::new(Vec::new()),
                segments: Vec::new(),
                texture: String::new(),
                tint: [1.0; 4],
                blend: BlendMode::Opaque,
                alpha_test: None,
                double_sided: false,
                effect: false,
                refraction: false,
            marker: false,
                hidden: false,
                material: EffectMaterial::default(),
                lighting: LightingMaterial::default(),
                shader_anims: Vec::new(),
            shader_color_anims: Vec::new(),
            }],
            warnings: Vec::new(),
        };

        // Unmodified: chain composes to (2,0,0).
        let base = scene.animated_worlds(&Default::default());
        assert!((base[0][12] - 2.0).abs() < 1e-5, "{:?}", &base[0][12..15]);

        // Sequence sample: rotate the arm, keep its authored translation.
        let half = std::f32::consts::FRAC_PI_4;
        let sample = crate::sequence::TransformSample {
            rotation: Some([half.cos(), 0.0, 0.0, half.sin()]),
            ..Default::default()
        };
        let mut overrides = std::collections::HashMap::new();
        overrides.insert(1, scene.nodes[1].local.with_sample(&sample));
        let worlds = scene.animated_worlds(&overrides);
        let t = &worlds[0][12..15];
        assert!((t[0] - 1.0).abs() < 1e-5, "{t:?}");
        assert!((t[1] - 1.0).abs() < 1e-5, "{t:?}");
        assert!(t[2].abs() < 1e-5, "{t:?}");

        // The lookup sequences use is case-insensitive, like the game's.
        assert_eq!(scene.node_index("ARM"), Some(1));
    }

    /// Real vanilla flag words through the real decode.
    ///
    /// What stood here asserted `4109 & 1 == 1` and four more of the
    /// same -- arithmetic on literals, calling nothing. It was a
    /// statement about the number 4109, and any change to the blend
    /// decode left it green.
    #[test]
    fn blend_and_alpha_test_decode_from_real_flag_words() {
        // 4109: blend on, src SRC_ALPHA(6), dst ONE(0) -- the additive
        // preset vanilla effects use.
        assert_eq!(blend_from_alpha_flags(4109), Some(BlendMode::Additive));
        // 4845: the standard alpha preset (4844) plus the blend bit,
        // dst INV_SRC_ALPHA(7), and the alpha TEST bit set.
        assert_eq!(blend_from_alpha_flags(4845), Some(BlendMode::Alpha));
        assert!(alpha_test_enabled(4845));
        // 0x0043: dst SRC_COLOR(2) -- modulation, the dirt and shadow
        // decals that must DARKEN what they lie on.
        assert_eq!(blend_from_alpha_flags(0x0043), Some(BlendMode::Multiply));
        // Blending off: the flag word still carries a dst factor, and it
        // must be ignored rather than read.
        assert_eq!(blend_from_alpha_flags(4108), None);
        assert_eq!(blend_from_alpha_flags(0), None);
        assert!(!alpha_test_enabled(4109));
    }
}
