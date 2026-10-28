//! Particle preview simulation.
//!
//! This is a deliberate re-implementation of Skyrim's own particle update, not
//! a plausible-looking approximation: every formula here was read out of
//! SkyrimSE.exe 1.5.97 and is documented, with the function address it came
//! from, in `docs/knowledge/particle-runtime.md`. When something looks odd
//! (gravity's 1.6 factor, the sphere emitter's non-uniform distribution, the
//! half-range speed variation), it is odd in the game too — matching the
//! vanilla look matters more than being mathematically tidy.
//!
//! std-only and deterministic: the RNG is seeded explicitly so the preview is
//! reproducible and testable.

use crate::graph::{Graph, NodeId, NodeKind};

/// Gravity strength is scaled by this in `NiPSysGravityModifier::Update`
/// (SkyrimSE 1.5.97 `0x140D0656C`). Nothing in the file format hints at it.
const GRAVITY_STRENGTH_SCALE: f32 = 1.6;

/// Turbulence is scaled by this before being applied (same function).
const TURBULENCE_SCALE: f32 = 500.0;

/// The game resets a rotation angle that runs away past this (`0x140D200BD`).
const ROTATION_ANGLE_RESET: f32 = 10.0 * std::f32::consts::PI;

const TAU: f32 = std::f32::consts::TAU;

/// Mirrors the game's `rand()`-based helpers: values come from a 0..32767
/// integer scaled by 1/32768, so the reachable set and the variation ranges
/// match the original exactly.
#[derive(Debug, Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        // Any decent generator works; the game's LCG is not reproducible
        // across runs anyway. What must match is the *range* of the outputs.
        Rng(seed | 1)
    }

    fn next_u15(&mut self) -> u32 {
        // xorshift32, then take the top bits (better distributed than the low).
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 17) & 0x7FFF
    }

    /// `rand() * 0.000030518509` — uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        self.next_u15() as f32 * 0.000_030_518_509
    }

    /// `rand01 * 2 - 1` — uniform in `[-1, 1)`.
    pub fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

/// Emitter shape and its dimensions, in the emitter node's local space.
/// Where a shape stands, as an affine map: a row-major 3x3 (any scale
/// already folded in) and a translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshPose {
    pub m: [f32; 9],
    pub t: [f32; 3],
}

impl Default for MeshPose {
    fn default() -> Self {
        MeshPose {
            m: MAT3_IDENTITY,
            t: [0.0; 3],
        }
    }
}

impl MeshPose {
    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.m;
        [
            m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + self.t[0],
            m[3] * p[0] + m[4] * p[1] + m[5] * p[2] + self.t[1],
            m[6] * p[0] + m[7] * p[1] + m[8] * p[2] + self.t[2],
        ]
    }

    /// A normal carries no translation.
    pub fn direction(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.m;
        [
            m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
            m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
            m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
        ]
    }
}

/// What the host hands back for one named shape: its geometry in its OWN
/// local space, plus the matrix that places it in the world (column-major,
/// the layout `mesh::RenderMesh::world` already uses).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshEmitterGeometry {
    pub vertices: Vec<([f32; 3], [f32; 3])>,
    pub triangles: Vec<[u32; 3]>,
    pub world: [f32; 16],
}

/// Geometry a mesh emitter samples.
///
/// The vertices stay in the SHAPE's own space and `to_psys` says where
/// that shape currently stands relative to the system. Keeping them
/// apart is what lets an animated emitter mesh move its spawn area for
/// the price of ONE matrix per source per frame instead of
/// re-transforming every vertex: the sampler picks one point and places
/// it, exactly as the runtime does (vfunc 43 builds its relative
/// transform per particle, 1.5.97 @ 0x140D26FA0).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshEmitterSource {
    /// The shape's name, so a re-pose can find it again.
    pub name: String,
    /// SHAPE-LOCAL vertices with normals.
    pub vertices: Vec<([f32; 3], [f32; 3])>,
    pub triangles: Vec<[u32; 3]>,
    /// Shape-local -> psys space.
    pub to_psys: MeshPose,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EmitterShape {
    Box { width: f32, height: f32, depth: f32 },
    Sphere { radius: f32 },
    Cylinder { radius: f32, height: f32 },
    Mesh {
        /// VelocityType: 0 = normals, 1 = random, 2 = emission axis.
        velocity_type: u32,
        /// EmitFrom: 0 vertices, 1 face centres, 2 edge centres,
        /// 3 face surface, 4 edge surface.
        emission_type: u32,
        emission_axis: [f32; 3],
        /// Shape names, straight from the node parameter.
        mesh_names: Vec<String>,
        /// Geometry resolved from the names by the host.
        sources: Vec<MeshEmitterSource>,
    },
}

#[derive(Debug, Clone)]
pub struct Emitter {
    pub shape: EmitterShape,
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
    /// Particles per second from the wired emitter controller.
    pub birth_rate: f32,
    /// Keyed birth rates ("Birth Rate Keys"), sampled at the
    /// controller-local looping time by the track's own interpolation
    /// (linear, quadratic Hermite or TBC). Empty = use `birth_rate`.
    pub birth_keys: crate::curve::Curve,
    /// Emitter on/off track ("Emitter Active Keys"), stepped over the
    /// controller window. Empty = always on.
    pub active_keys: Vec<(f32, bool)>,
    pub start_time: f32,
    pub stop_time: f32,
    pub looping: bool,
    /// World translation of the emitter volume's anchor node. Mesh
    /// emitters ignore it — their sources are already world-space.
    pub offset: [f32; 3],
    /// World rotation of the anchor node (row-major), applied to spawn
    /// positions and launch directions. Mesh emitters ignore it too.
    pub rotation: [f32; 9],
    /// The emitter block this came from, so the host can re-resolve the
    /// anchor while an animation plays. `offset`/`rotation` above are
    /// the AUTHORED pose, which is what export and the editor want; the
    /// game reads the anchor's world fresh for every particle it
    /// spawns (1.5.97 @ 0x140D26FA0).
    pub source_block: Option<usize>,
}

impl Emitter {
    /// A mesh emitter with no geometry to sample.
    ///
    /// Covers both ways of getting there -- an empty name list
    /// (unfinished authoring, which `validate` already flags) and names
    /// that resolved to nothing -- because the GAME does not
    /// distinguish them: `NiPSysMeshEmitter` samples the vertices of the
    /// shapes it names, and with none there is nothing to sample and no
    /// particle is born.
    pub fn mesh_without_geometry(&self) -> bool {
        match &self.shape {
            EmitterShape::Mesh { sources, .. } => {
                sources.iter().all(|s| s.vertices.is_empty())
            }
            _ => false,
        }
    }

    /// The stricter case, the one worth a WARNING: it asked for shapes
    /// the scene does not have -- a typo, a renamed shape, or an export
    /// whose reference was written wrong. An empty list is not reported
    /// here; that is unfinished authoring and validation's business.
    pub fn unresolved_mesh(&self) -> bool {
        match &self.shape {
            EmitterShape::Mesh { mesh_names, .. } => {
                !mesh_names.is_empty() && self.mesh_without_geometry()
            }
            _ => false,
        }
    }
}

impl Default for Emitter {
    fn default() -> Self {
        Emitter {
            source_block: None,
            shape: EmitterShape::Box {
                width: 0.0,
                height: 0.0,
                depth: 0.0,
            },
            speed: 0.0,
            speed_variation: 0.0,
            declination: 0.0,
            declination_variation: 0.0,
            planar_angle: 0.0,
            planar_angle_variation: 0.0,
            initial_color: [1.0; 4],
            initial_radius: 1.0,
            radius_variation: 0.0,
            life_span: 1.0,
            life_span_variation: 0.0,
            birth_rate: 0.0,
            birth_keys: crate::curve::Curve::empty(crate::curve::Track::Float),
            active_keys: Vec::new(),
            start_time: 0.0,
            stop_time: 1.0,
            looping: true,
            offset: [0.0; 3],
            rotation: MAT3_IDENTITY,
        }
    }
}

const MAT3_IDENTITY: [f32; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

/// Row-major `R * v`, same convention as `mesh::Transform`.
/// The emitter's spawn volume as line segments, for the viewport's
/// debug wireframe.
///
/// Lives HERE, next to [`Simulation::spawn_from_shape`], on purpose: a
/// debug view whose outline disagrees with where particles actually
/// appear is worse than no view at all, so the two rules must sit
/// where one cannot be edited without seeing the other. The pinning
/// test spawns real particles and asserts they land inside these
/// segments' own bounds.
///
/// The result is in the SAME space `spawn_one` produces positions in
/// (emitter rotation and offset applied, the system's world transform
/// NOT applied), so a caller draws it through exactly the transform it
/// already uses for particles.
///
/// `rings` is the segment count of a circle; `mesh_budget` caps the
/// edges a mesh emitter contributes, since it samples real geometry.
pub fn emitter_wireframe(
    e: &Emitter,
    rings: usize,
    mesh_budget: usize,
) -> Vec<([f32; 3], [f32; 3])> {
    let rings = rings.max(3);
    let mut out: Vec<([f32; 3], [f32; 3])> = Vec::new();
    // Mesh sources are already world-space and `spawn_one` leaves them
    // alone, so they skip the anchor placement below too.
    if let EmitterShape::Mesh { sources, .. } = &e.shape {
        for src in sources {
            for t in &src.triangles {
                if out.len() + 3 > mesh_budget {
                    return out;
                }
                // Placed by the source's own pose, the same one the
                // spawner uses -- an outline that ignored it would sit
                // at bind pose while the particles came out of the
                // turning shape.
                let v = |i: u32| {
                    src.vertices
                        .get(i as usize)
                        .map(|(p, _)| src.to_psys.point(*p))
                };
                if let (Some(a), Some(b), Some(c)) = (v(t[0]), v(t[1]), v(t[2])) {
                    out.push((a, b));
                    out.push((b, c));
                    out.push((c, a));
                }
            }
        }
        return out;
    }

    // Everything below is built in shape-local space and placed by the
    // anchor at the end, the one order `spawn_one` uses.
    let mut local: Vec<([f32; 3], [f32; 3])> = Vec::new();
    let ring = |axis: usize, radius: f32, at: f32, into: &mut Vec<([f32; 3], [f32; 3])>| {
        let point = |k: usize| -> [f32; 3] {
            let a = k as f32 / rings as f32 * TAU;
            let (c, s) = (a.cos() * radius, a.sin() * radius);
            match axis {
                0 => [at, c, s],
                1 => [c, at, s],
                _ => [c, s, at],
            }
        };
        for k in 0..rings {
            into.push((point(k), point((k + 1) % rings)));
        }
    };

    match &e.shape {
        EmitterShape::Box {
            width,
            height,
            depth,
        } => {
            // Centred on the anchor, half-extents each way -- the shape
            // `spawn_from_shape` samples as `(u - 0.5) * size`.
            let (x, y, z) = (width * 0.5, height * 0.5, depth * 0.5);
            let c = |i: usize| -> [f32; 3] {
                [
                    if i & 1 == 0 { -x } else { x },
                    if i & 2 == 0 { -y } else { y },
                    if i & 4 == 0 { -z } else { z },
                ]
            };
            // Every pair of corners differing in exactly one bit is an
            // edge: 12 of them, with no hand-written table to mistype.
            for i in 0..8usize {
                for bit in [1usize, 2, 4] {
                    let j = i | bit;
                    if j != i {
                        local.push((c(i), c(j)));
                    }
                }
            }
        }
        EmitterShape::Sphere { radius } => {
            for axis in 0..3 {
                ring(axis, *radius, 0.0, &mut local);
            }
        }
        EmitterShape::Cylinder { radius, height } => {
            // The axis is Z: `spawn_from_shape` puts the circle in XY
            // and the run in Z.
            let h = height * 0.5;
            ring(2, *radius, -h, &mut local);
            ring(2, *radius, h, &mut local);
            for k in 0..4usize {
                let a = k as f32 / 4.0 * TAU;
                let (c, s) = (a.cos() * radius, a.sin() * radius);
                local.push(([c, s, -h], [c, s, h]));
            }
        }
        EmitterShape::Mesh { .. } => unreachable!("handled above"),
    }

    out.extend(local.into_iter().map(|(a, b)| {
        let place = |p: [f32; 3]| {
            let r = rotate3(&e.rotation, p);
            [r[0] + e.offset[0], r[1] + e.offset[1], r[2] + e.offset[2]]
        };
        (place(a), place(b))
    }));
    out
}

fn rotate3(r: &[f32; 9], v: [f32; 3]) -> [f32; 3] {
    [
        r[0] * v[0] + r[1] * v[1] + r[2] * v[2],
        r[3] * v[0] + r[4] * v[1] + r[5] * v[2],
        r[6] * v[0] + r[7] * v[1] + r[8] * v[2],
    ]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ForceKind {
    /// `NiPSysGravityModifier`, planar (`force_type` 0).
    GravityPlanar,
    /// `NiPSysGravityModifier`, spherical (`force_type` 1).
    GravitySpherical,
    /// `NiPSysDragModifier`.
    Drag,
    /// `BSWindModifier` — acceleration along the weather's wind vector
    /// (`Simulation::wind`, host-fed) × strength × per-particle dt
    /// (RE 0x140D3D4B0). Zero wind (indoors) makes it a no-op, like the
    /// game.
    Wind,
    /// `NiPSysBombModifier` — radial acceleration from `origin` (RE'd
    /// @ 0x140D0FDC0): `strength` is Delta V (units/s², negative
    /// attracts), `decay` doubles as a hard spherical range cutoff when
    /// `decay_type` != 0, and `symmetry`/`decay_type` follow nif.xml.
    Bomb,
}

/// `BSPSysInheritVelocityModifier`, applied once at spawn
/// (RE 0x140D269E0): with `chance`% odds the particle adds the anchor
/// node's world velocity × uniform[max(0, M−M·V), M+M·V] — the
/// variation is RELATIVE to the multiplier.
#[derive(Debug, Clone)]
pub struct InheritVelocity {
    pub chance: f32,
    pub multiplier: f32,
    pub variation: f32,
    /// The node whose motion is inherited; the HOST tracks it and feeds
    /// [`Simulation::anchor_velocity`], because only the viewport knows
    /// where sequence playback has moved the scene's nodes.
    pub node: String,
}

/// `BSPSysRecycleBoundModifier` (RE 0x140D25DE0): particles leaving the
/// box wrap toroidally back in — never killed, velocity and age
/// untouched. `corner` is the box's MIN corner (the game anchors the
/// bound-centre point there, not at the middle), spanning +extent.
#[derive(Debug, Clone)]
pub struct RecycleBound {
    pub corner: [f32; 3],
    pub extent: [f32; 3],
}

/// One `NiPSysModifierFloatCtlr`-family track: sampled every update at
/// controller-local looping time and written RAW into its target field
/// (RE 0x140D27390 — no scaling, no clamping, no restore-on-stop).
#[derive(Debug, Clone)]
pub struct ParamTrack {
    pub target: ParamTarget,
    pub keys: crate::curve::Curve,
    pub start_time: f32,
    pub stop_time: f32,
    pub looping: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamTarget {
    /// A field of `emitters[index]`.
    Emitter { index: usize, param: EmitterParam },
    /// `NiPSysGravityStrengthCtlr` on `forces[index]` — unlike the
    /// emitter params this bites every particle immediately.
    ForceStrength { index: usize },
    /// The rotation modifier's spawn parameters.
    Rotation { param: RotationParam },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitterParam {
    Speed,
    InitialRadius,
    LifeSpan,
    Declination,
    DeclinationVariation,
    PlanarAngle,
    PlanarAngleVariation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationParam {
    Speed,
    SpeedVariation,
    Angle,
    AngleVariation,
}

/// `NiVisController` in the system's chain: a stepped bool track on the
/// system's hidden flag. The sim keeps running while hidden, exactly
/// like the game — only the drawing stops.
#[derive(Debug, Clone)]
pub struct VisTrack {
    pub keys: Vec<(f32, bool)>,
    pub start_time: f32,
    pub stop_time: f32,
    pub looping: bool,
}

#[derive(Debug, Clone)]
pub struct Force {
    pub kind: ForceKind,
    pub axis: [f32; 3],
    pub strength: f32,
    pub decay: f32,
    pub turbulence: f32,
    pub turbulence_scale: f32,
    /// Drag only.
    pub percentage: f32,
    /// Drag only: full strength within `range`, fading to nothing at
    /// `range_falloff` (both default MAX = uniform everywhere).
    pub range: f32,
    pub range_falloff: f32,
    /// Bomb only: `DecayType` (0 none, 1 linear, 2 exponential) and
    /// `SymmetryType` (0 spherical, 1 cylindrical, 2 planar).
    pub decay_type: u32,
    pub symmetry: u32,
    /// Position of the force's object in psys space.
    pub origin: [f32; 3],
    /// `NiPSysModifierActiveCtlr` — the force only applies while the
    /// stepped bool track says so (the campfire pulses its drags for
    /// gusty smoke).
    pub toggle: Option<ForceToggle>,
}

#[derive(Debug, Clone)]
pub struct ForceToggle {
    /// `(time, on)`, stepped over the controller window.
    pub keys: Vec<(f32, bool)>,
    pub start_time: f32,
    pub stop_time: f32,
    pub looping: bool,
}

impl ForceToggle {
    pub fn active_at(&self, time: f32) -> bool {
        let t = crate::sequence::controller_time(
            time,
            self.start_time,
            self.stop_time,
            self.looping,
        );
        crate::sequence::sample_bool(&self.keys, t).unwrap_or(true)
    }
}

#[derive(Debug, Clone)]
pub struct ColorOverLife {
    pub colors: [[f32; 4]; 3],
    pub fade_in: f32,
    pub fade_out: f32,
    pub color1_end: f32,
    pub color2_start: f32,
    pub color2_end: f32,
    pub color3_start: f32,
}

#[derive(Debug, Clone)]
pub struct Rotation {
    pub speed: f32,
    pub speed_variation: f32,
    pub angle: f32,
    pub angle_variation: f32,
    pub random_speed_sign: bool,
}

/// `BSPSysSubTexModifier` — atlas frame animation. Semantics RE'd from
/// SkyrimSE 1.5.97 `BSPSysSubTexModifier::Update` @ 0x140D64BF0
/// (docs/knowledge/particle-runtime.md): every fudge is ADDITIVE
/// `value + rand01*fudge`, never centred.
#[derive(Debug, Clone)]
pub struct SubTexFrames {
    pub start_frame: f32,
    pub start_frame_fudge: f32,
    pub end_frame: f32,
    pub loop_start_frame: f32,
    pub loop_start_frame_fudge: f32,
    pub frame_count: f32,
    pub frame_count_fudge: f32,
}

/// Everything the simulation needs about one particle system, extracted from
/// the graph once so stepping does not re-read the document every frame.
#[derive(Debug, Clone)]
pub struct SystemDesc {
    pub name: String,
    /// The `NiParticleSystem` block this was decompiled from, when the
    /// graph still knows. It is what lets the host re-derive the
    /// system's FRAME while an animation plays — the game recomputes it
    /// every update, so a system hanging off a turning node travels
    /// with it.
    pub source_block: Option<usize>,
    pub max_particles: usize,
    pub emitters: Vec<Emitter>,
    pub forces: Vec<Force>,
    pub colliders: Vec<Collider>,
    pub color: Option<ColorOverLife>,
    /// `BSPSysScaleModifier` samples, uniformly spaced over the lifetime.
    pub scales: Option<Vec<f32>>,
    pub rotation: Option<Rotation>,
    /// Effect shader's source texture, game-relative (empty = untextured).
    pub texture: String,
    /// Effect shader's base colour, multiplied into every particle.
    pub tint: [f32; 4],
    /// Free-running shader tracks, absolute overwrites of the fields
    /// above when non-empty. Emissive Scale = variable 0, Alpha =
    /// variable 5, Color = the colour controller's Emissive Color.
    pub emissive_scale_keys: crate::curve::Curve,
    pub alpha_keys: crate::curve::Curve,
    pub color_keys: crate::curve::Curve,
    /// UV scroll tracks, (variable, keys) for variables 6-9 from
    /// `graph::EFFECT_SHADER_FLOAT_KEYS`: U offset, U scale, V offset,
    /// V scale -- written into `uv_offset` / `uv_scale` per frame.
    pub uv_keys: Vec<(u32, crate::curve::Curve)>,
    /// Greyscale-to-palette ramp texture (empty = no palette). The colour
    /// and alpha flags only apply when it resolves — same as the game.
    pub greyscale_texture: String,
    /// SLSF1 0x10: replace RGB with ramp(base.g, particle.r * BaseColor.r)
    /// — the ramp row is the RED channel (Effect.hlsl `baseColorMul.x`).
    pub greyscale_color: bool,
    /// SLSF1 0x20: replace alpha with ramp(base.a, computed alpha).
    pub greyscale_alpha: bool,
    /// Multiplies the ramp output (Base Color Scale).
    pub base_color_scale: f32,
    /// Material UV transform — the game applies it to particle UVs on top
    /// of the atlas cell (Effect.hlsl `texCoordOffset`).
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    /// SLSF1 0x40 + the falloff quad. Camera-facing billboards see the
    /// view head-on (|N·V| = 1), so for them this folds to one constant
    /// alpha factor; see `falloff_factor_head_on`.
    pub use_falloff: bool,
    pub falloff: [f32; 4],
    /// SLSF1 Soft_Effect depth: fragments fade in over this many units of
    /// separation from the opaque scene behind them (0 = off).
    pub soft_depth: f32,
    /// Alpha property preset: additive (true) or standard alpha blend.
    pub additive: bool,
    /// The system node's accumulated world scale — the game multiplies
    /// all rendered particle output by it (1.5.97 @ 0x140D039B0); the sim
    /// itself stays in psys space.
    pub world_scale: f32,
    /// ...and the rest of the node's world matrix, for NON-world-space
    /// systems (the game zeroes both for world-space ones). Render-time
    /// only, like the scale: `world = R·(p·s) + t`.
    pub world_rotation: [f32; 9],
    pub world_offset: [f32; 3],
    /// `BSStripParticleSystem`: particles leave ribbon trails. The preview
    /// records per-particle path points at the game's cadence.
    pub strips: Option<StripDesc>,
    /// Camera-distance LOD; the host feeds the distance per frame.
    pub lod: Option<LodDesc>,
    /// Aspect handling, RE'd from the 1.5.97 quad fill: the presence of
    /// rotation angles DISABLES the whole aspect feature; otherwise a
    /// nonzero low flag byte aligns the quad to in-plane velocity (bits 0
    /// and 1 are indistinguishable to the engine) and a nonzero high byte
    /// drives the aspect from speed between the two thresholds. With all
    /// flags clear the quad stays square and `aspect_ratio` never applies.
    pub aspect_ratio: f32,
    pub velocity_aspect: bool,
    pub speed_to_aspect: bool,
    pub aspect2: f32,
    pub aspect_speed1: f32,
    pub aspect_speed2: f32,
    /// Subtexture atlas layout from the Particle Data node (1x1 = no atlas).
    pub atlas_rows: u32,
    pub atlas_cols: u32,
    /// The cells themselves, `(U offset, U size, V offset, V size)`, when
    /// they come from a graph: an explicit table wins over rows x cols,
    /// because the table is what the game samples. Empty = derive from
    /// rows x cols (a `SystemDesc` built by hand).
    pub atlas_table: Vec<[f32; 4]>,
    /// Atlas frame animation; `None` keeps the spawn-chosen cell for life.
    pub subtex: Option<SubTexFrames>,
    /// Spawn-time velocity inheritance from a moving anchor.
    pub inherit: Option<InheritVelocity>,
    /// Toroidal position wrap.
    pub recycle: Option<RecycleBound>,
    /// Keyed parameter tracks, sampled every update.
    pub param_tracks: Vec<ParamTrack>,
    /// The system's visibility track.
    pub vis: Option<VisTrack>,
}

impl Default for SystemDesc {
    fn default() -> Self {
        SystemDesc {
            name: String::new(),
            source_block: None,
            max_particles: 100,
            emitters: Vec::new(),
            forces: Vec::new(),
            colliders: Vec::new(),
            color: None,
            scales: None,
            rotation: None,
            texture: String::new(),
            tint: [1.0; 4],
            emissive_scale_keys: crate::curve::Curve::empty(crate::curve::Track::Float),
            alpha_keys: crate::curve::Curve::empty(crate::curve::Track::Float),
            color_keys: crate::curve::Curve::default(),
            uv_keys: Vec::new(),
            greyscale_texture: String::new(),
            greyscale_color: false,
            greyscale_alpha: false,
            base_color_scale: 1.0,
            uv_offset: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            use_falloff: false,
            falloff: [1.0, 1.0, 0.0, 0.0],
            soft_depth: 0.0,
            additive: true,
            world_scale: 1.0,
            world_rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            world_offset: [0.0; 3],
            strips: None,
            lod: None,
            aspect_ratio: 1.0,
            velocity_aspect: false,
            speed_to_aspect: false,
            aspect2: 0.0,
            aspect_speed1: 0.0,
            aspect_speed2: 0.0,
            atlas_rows: 1,
            atlas_cols: 1,
            atlas_table: Vec::new(),
            subtex: None,
            inherit: None,
            recycle: None,
            param_tracks: Vec::new(),
            vis: None,
        }
    }
}

/// One collider in psys space (`NiPSysCollider` + shape, RE'd from
/// 1.5.97 — see `docs/knowledge/particle-rendering.md`).
#[derive(Debug, Clone)]
pub struct Collider {
    pub active: bool,
    /// Whole reflected velocity is scaled by this.
    pub bounce: f32,
    pub die_on_collide: bool,
    pub shape: ColliderShape,
}

#[derive(Debug, Clone)]
pub enum ColliderShape {
    Sphere {
        center: [f32; 3],
        radius: f32,
    },
    /// Bounded rectangle: `normal = cross(x_axis, y_axis)`, unit axes.
    Plane {
        origin: [f32; 3],
        normal: [f32; 3],
        x_axis: [f32; 3],
        y_axis: [f32; 3],
        half_width: f32,
        half_height: f32,
    },
}

impl ColliderShape {
    pub fn is_plane(&self) -> bool {
        matches!(self, ColliderShape::Plane { .. })
    }

    /// Outward surface normal at a hit point.
    pub fn normal_at(&self, point: [f32; 3]) -> [f32; 3] {
        match self {
            ColliderShape::Sphere { center, radius } => {
                let d = sub(point, *center);
                let r = radius.max(1e-6);
                [d[0] / r, d[1] / r, d[2] / r]
            }
            ColliderShape::Plane { normal, .. } => *normal,
        }
    }

    /// Earliest intersection of the particle's path inside the window
    /// `[t0, best)`, as `(absolute time, hit point)` — the game's
    /// FindIntersection semantics (0x140D21820 / 0x140D1D880).
    pub fn intersect(
        &self,
        position: [f32; 3],
        velocity: [f32; 3],
        t0: f32,
        best: f32,
    ) -> Option<(f32, [f32; 3])> {
        match self {
            ColliderShape::Sphere { center, radius } => {
                let q = sub(position, *center);
                let r2 = radius * radius;
                let f = dot(q, q) - r2;
                if f.abs() <= 0.001 {
                    return None; // deadband on the surface
                }
                let v2 = dot(velocity, velocity);
                if v2 <= 1e-12 {
                    return None;
                }
                let b = -dot(velocity, q);
                let tca = b / v2;
                let t = if f > 0.0 {
                    // Outside: entry point, must be approaching.
                    if b <= 0.0 {
                        return None;
                    }
                    let m = [
                        q[0] + velocity[0] * tca,
                        q[1] + velocity[1] * tca,
                        q[2] + velocity[2] * tca,
                    ];
                    let m2 = dot(m, m);
                    if m2 >= r2 {
                        return None;
                    }
                    tca - ((r2 - m2) / v2).sqrt()
                } else {
                    // Inside: the EXIT point — containment.
                    let m = [
                        q[0] + velocity[0] * tca,
                        q[1] + velocity[1] * tca,
                        q[2] + velocity[2] * tca,
                    ];
                    tca + ((r2 - dot(m, m)).max(0.0) / v2).sqrt()
                };
                if t >= 0.0 && t < best - t0 {
                    let point = [
                        position[0] + velocity[0] * t,
                        position[1] + velocity[1] * t,
                        position[2] + velocity[2] * t,
                    ];
                    Some((t0 + t, point))
                } else {
                    None
                }
            }
            ColliderShape::Plane {
                origin,
                normal,
                x_axis,
                y_axis,
                half_width,
                half_height,
            } => {
                let in_rect = |point: [f32; 3]| {
                    let dl = sub(point, *origin);
                    let dx = dot(dl, *x_axis);
                    let dy = dot(dl, *y_axis);
                    dx * dx < half_width * half_width && dy * dy < half_height * half_height
                };
                let d0 = dot(*normal, sub(position, *origin));
                if d0.abs() < 0.001 {
                    // Already on the plane.
                    return if in_rect(position) {
                        Some((t0, position))
                    } else {
                        None
                    };
                }
                let vn = dot(*normal, velocity);
                let window = best - t0;
                let mut d_end = d0 + vn * window;
                if d_end.abs() < 0.001 {
                    d_end = 0.0;
                }
                if d_end * d0 >= 0.0 {
                    return None; // no crossing inside the window
                }
                let t = -d0 / vn;
                let point = [
                    position[0] + velocity[0] * t,
                    position[1] + velocity[1] * t,
                    position[2] + velocity[2] * t,
                ];
                if in_rect(point) {
                    Some((t0 + t, point))
                } else {
                    None
                }
            }
        }
    }
}

/// `BSPSysLODModifier` (1.5.97 @ 0x1413243A0): scales emission and
/// initial size by camera distance. The distance is NORMALIZED —
/// `worldDist × (FOV/80) / fLODFadeOutMultObjects(5) / 7200`, i.e.
/// `dist/36000` at defaults — then lerped 1 → end value between the
/// begin/end thresholds.
#[derive(Debug, Clone)]
pub struct LodDesc {
    pub begin: f32,
    pub end: f32,
    pub end_emit_scale: f32,
    pub end_size: f32,
}

impl LodDesc {
    /// `(emit multiplier, size multiplier)` for a camera at `world_dist`
    /// game units from the effect, default FOV and INI settings.
    pub fn factors(&self, world_dist: f32) -> (f32, f32) {
        let d = world_dist / 36000.0;
        if d <= self.begin {
            (1.0, 1.0)
        } else if d <= self.end {
            let t = (d - self.begin) / (self.end - self.begin).max(1e-9);
            (
                1.0 + (self.end_emit_scale - 1.0) * t,
                1.0 + (self.end_size - 1.0) * t,
            )
        } else {
            (self.end_emit_scale, self.end_size)
        }
    }
}

/// Strip-trail parameters (`BSStripPSysData`), RE'd from 1.5.97:
/// point cadence is a hardcoded 1/60 s (the NIF's Update Delta Time is
/// dead — the gate time is never written after init), point lifetime is
/// `max_point_count / 60` seconds, ring capacity `max_point_count + 3`.
#[derive(Debug, Clone)]
pub struct StripDesc {
    pub max_point_count: u32,
}

impl StripDesc {
    pub fn capacity(&self) -> usize {
        self.max_point_count as usize + 3
    }
    pub fn point_lifetime(&self) -> f32 {
        self.max_point_count as f32 / 60.0
    }
}

/// One recorded path point of a strip trail.
#[derive(Debug, Clone, Copy)]
pub struct TrailPoint {
    pub position: [f32; 3],
    /// Ribbon half-width source: `radius * size` at record time.
    pub width: f32,
    pub color: [f32; 4],
    pub time: f32,
}

/// A particle's ribbon: newest point last. Detached trails (dead
/// particles) keep draining by expiry, like the game's RemoveParticle
/// override which only frees a particle once its strip is spent.
#[derive(Debug, Clone, Default)]
pub struct Trail {
    pub points: Vec<TrailPoint>,
    /// Time of the last COMMITTED point; the newest point is rewritten in
    /// place every frame until the 1/60 s cadence commits a fresh one.
    pub last_commit: f32,
    /// High-water mark of committed points — the game's `segmentCount`,
    /// which spreads the 0..1 texture over the trail while it grows.
    pub segments: u32,
}

/// Normalize, falling back to +Z for degenerate input — a spawn direction
/// must always be usable.
fn normalize_or_up(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

impl SystemDesc {
    /// Apply the FREE shader controller tracks at ambient time `t`,
    /// as absolute overwrites of the stored material fields — the
    /// same fields the game's own Update writes, read out of 1.5.97:
    /// variable 0 lands in material float 27, the Emissive Multiple
    /// (registrar `0x1413223F0`); variable 5 routes through
    /// `SetMaterialAlpha` (`0x1412D6410`), which clamps BELOW at zero
    /// and nowhere else; the colour controller writes material RGB and
    /// leaves alpha alone (`0x141322860`).
    ///
    /// The preview LOOPS every track over its key span. The authored
    /// controller flags (clamp vs loop) are deliberately not in the
    /// graph, and an editor preview that played a 31-second clamp
    /// track once and went static would read as broken — the
    /// divergence is documented here rather than hidden.
    ///
    /// Overwrites are ABSOLUTE (curve value replaces the field), so
    /// calling this every frame cannot drift; an empty curve leaves
    /// its field alone, which is also what makes it safe next to the
    /// sequence player — a variable is either sequence-driven
    /// (blend interpolator, empty param) or free (keys here), never
    /// both, because a controller has ONE interpolator.
    pub fn apply_shader_tracks(&mut self, t: f32) {
        fn looped_f(c: &crate::curve::Curve, t: f32) -> Option<f32> {
            let span = c.keys.last()?.time;
            c.sample_f32(crate::sequence::shader_track_time(t, span))
        }
        if let Some(v) = looped_f(&self.emissive_scale_keys, t) {
            self.base_color_scale = v;
        }
        if let Some(v) = looped_f(&self.alpha_keys, t) {
            self.tint[3] = v.max(0.0);
        }
        // UV scroll: the variables the game's table writes into the
        // material's UV offset and scale, absolutely per frame like the
        // others (6 U offset, 7 U scale, 8 V offset, 9 V scale).
        for (var, keys) in &self.uv_keys {
            if let Some(v) = looped_f(keys, t) {
                match var {
                    6 => self.uv_offset[0] = v,
                    7 => self.uv_scale[0] = v,
                    8 => self.uv_offset[1] = v,
                    9 => self.uv_scale[1] = v,
                    _ => {}
                }
            }
        }
        if !self.color_keys.is_empty() {
            let span = self.color_keys.keys.last().map(|k| k.time).unwrap_or(0.0);
            let tt = crate::sequence::shader_track_time(t, span);
            if let Some(rgb) = self.color_keys.sample(tt) {
                self.tint[0] = rgb[0];
                self.tint[1] = rgb[1];
                self.tint[2] = rgb[2];
            }
        }
    }

    /// The Effect.hlsl view-angle falloff evaluated head-on (`|N·V| = 1`),
    /// which is what a camera-facing billboard always presents: one
    /// constant alpha multiplier
    /// `lerp(startOpacity, stopOpacity, smoothstep(saturate((1-start)/(stop-start))))`.
    pub fn falloff_factor_head_on(&self) -> f32 {
        if !self.use_falloff {
            return 1.0;
        }
        let [start, stop, op_start, op_stop] = self.falloff;
        let denom = stop - start;
        let f = if denom.abs() < 1e-6 {
            if 1.0 >= start {
                1.0
            } else {
                0.0
            }
        } else {
            ((1.0 - start) / denom).clamp(0.0, 1.0)
        };
        let param = f * f * (3.0 - 2.0 * f);
        op_start + (op_stop - op_start) * param
    }

    /// UV rectangle of a particle's atlas cell as `(u0, v0, u1, v1)`.
    /// Matches the compiled `subtexture_offsets` layout: U varies fastest.
    pub fn atlas_uv(&self, index: u32) -> (f32, f32, f32, f32) {
        if !self.atlas_table.is_empty() {
            let q = self.atlas_table[index as usize % self.atlas_table.len()];
            return (q[0], q[2], q[0] + q[1], q[2] + q[3]);
        }
        let cols = self.atlas_cols.max(1);
        let rows = self.atlas_rows.max(1);
        let cell = index % (cols * rows);
        let u0 = (cell % cols) as f32 / cols as f32;
        let v0 = (cell / cols) as f32 / rows as f32;
        (u0, v0, u0 + 1.0 / cols as f32, v0 + 1.0 / rows as f32)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Particle {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub age: f32,
    pub life_span: f32,
    /// The game's per-particle clock; every modifier derives `dt` from it.
    pub last_update: f32,
    /// Spawn radius. The rendered billboard half-extent is
    /// `radius * size * 0.5` — NifSkope draws `sizes[p] * radius` and the
    /// giant campfire's numbers only make sense that way (150-unit smoke
    /// radius × 0.5-curve = 75-unit puffs).
    pub radius: f32,
    /// Scale multiplier over the radius: 1 at birth; the scale modifier
    /// *overwrites* it with its curve sample (never multiplies).
    pub size: f32,
    pub color: [f32; 4],
    pub rotation_angle: f32,
    pub rotation_speed: f32,
    /// Atlas cell, chosen at spawn like the game does (`rand % cells`);
    /// with a SubTex modifier present, overwritten on the first update.
    pub atlas_index: u32,
    /// SubTex pacing state: the last frame step taken (-1 = newborn) and
    /// this particle's own frame budget, mirroring the two scratch shorts
    /// the game keeps in `NiParticleInfo` (+0x1C/+0x1E).
    pub subtex_step: i32,
    pub subtex_frames: f32,
}

impl Particle {
    /// Normalized lifetime, the input to every over-life curve.
    pub fn t(&self) -> f32 {
        if self.life_span > 0.0 {
            self.age / self.life_span
        } else {
            1.0
        }
    }
}

/// One running particle system.
#[derive(Debug, Clone)]
pub struct Simulation {
    pub desc: SystemDesc,
    pub particles: Vec<Particle>,
    /// Strip systems: one trail per particle, index-parallel. Empty for
    /// billboard systems.
    pub trails: Vec<Trail>,
    /// Trails of dead particles, still draining by point expiry — the
    /// game's RemoveParticle override keeps a particle until its strip
    /// has at most one point; detaching is visually equivalent.
    pub detached_trails: Vec<Trail>,
    pub time: f32,
    /// Camera-distance LOD multipliers, fed by the host per frame
    /// (`set_camera_distance`); 1.0 without a LOD modifier.
    pub lod_emit: f32,
    pub lod_size: f32,
    rng: Rng,
    /// What `rng` was seeded with, so `reset` can rewind it too.
    seed: u32,
    /// Fractional birth carry per emitter, mirroring the game's `this+0x30`.
    /// Runtime state, so it lives here rather than in the description.
    accumulators: Vec<f32>,
    /// The weather wind vector, host-fed (`windSpeed × 100 × (−sin θ,
    /// cos θ, 0)` in game terms). Zero = indoors.
    pub wind: [f32; 3],
    /// World velocity of the inherit anchor, host-fed per frame — only
    /// the viewport knows where sequences moved the node.
    pub anchor_velocity: [f32; 3],
    /// Sampled from the visibility track each step; the host skips
    /// drawing while false. Simulation continues, like the game.
    pub visible: bool,
}

impl Simulation {
    pub fn new(desc: SystemDesc, seed: u32) -> Self {
        Simulation {
            particles: Vec::with_capacity(desc.max_particles),
            trails: Vec::new(),
            detached_trails: Vec::new(),
            accumulators: vec![0.0; desc.emitters.len()],
            desc,
            time: 0.0,
            lod_emit: 1.0,
            lod_size: 1.0,
            rng: Rng::new(seed),
            seed,
            wind: [0.0; 3],
            anchor_velocity: [0.0; 3],
            visible: true,
        }
    }

    /// Update the camera-distance LOD multipliers (world-space distance
    /// from the camera to the effect, in game units).
    pub fn set_camera_distance(&mut self, world_dist: f32) {
        let (emit, size) = match &self.desc.lod {
            Some(lod) => lod.factors(world_dist),
            None => (1.0, 1.0),
        };
        self.lod_emit = emit;
        self.lod_size = size;
    }

    /// Back to t = 0, including the RNG: a reset that kept the rng's
    /// position gave every restart a different particle stream, which
    /// a timeline cannot afford — scrubbing back to a time must show
    /// the same picture it showed the first time, or the timeline
    /// lies. (The game never restarts an effect in place, so there is
    /// no runtime behaviour to match here.)
    pub fn reset(&mut self) {
        self.particles.clear();
        self.trails.clear();
        self.detached_trails.clear();
        self.time = 0.0;
        self.accumulators.iter_mut().for_each(|a| *a = 0.0);
        self.rng = Rng::new(self.seed);
    }

    /// True once a one-shot effect has played out: nothing alive and no
    /// emitter can ever fire again. Looping systems are never finished.
    /// The preview uses this to replay bursts; the simulation itself just
    /// keeps running, exactly as the game would.
    pub fn is_finished(&self) -> bool {
        !self.desc.emitters.is_empty()
            && self.particles.is_empty()
            && self
                .desc
                .emitters
                .iter()
                .all(|e| !e.looping && self.time > e.stop_time)
    }

    /// Advance by `dt` seconds. Modifier order follows the compiled stack:
    /// age/death (0) → emitter (1000) → general (3000) → forces (4000) →
    /// position update (6000), which is also the order the game runs them in.
    pub fn step(&mut self, dt: f32) {
        let dt = dt.max(0.0);
        self.time += dt;

        self.age_and_kill();
        self.apply_param_tracks();
        self.spawn(dt);
        self.apply_general();
        self.apply_forces();
        self.apply_colliders();
        self.integrate_positions();
        self.apply_recycle_bound();
        self.record_trails();
    }

    /// The float-controller family: sample each track at its own
    /// controller-local looping clock and write the value RAW into the
    /// target field, before spawning — so this update's spawns already
    /// see it, matching the game's modifier-before-emitter chain order.
    fn apply_param_tracks(&mut self) {
        let time = self.time;
        // Values first, writes second: sampling borrows the tracks while
        // the writes need the rest of the description mutably.
        let writes: Vec<(ParamTarget, f32)> = self
            .desc
            .param_tracks
            .iter()
            .filter_map(|t| {
                if t.keys.is_empty() {
                    return None;
                }
                let local =
                    crate::sequence::controller_time(time, t.start_time, t.stop_time, t.looping);
                t.keys.sample_f32(local).map(|v| (t.target, v))
            })
            .collect();
        for (target, value) in writes {
            match target {
                ParamTarget::Emitter { index, param } => {
                    let Some(e) = self.desc.emitters.get_mut(index) else {
                        continue;
                    };
                    match param {
                        EmitterParam::Speed => e.speed = value,
                        EmitterParam::InitialRadius => e.initial_radius = value,
                        EmitterParam::LifeSpan => e.life_span = value,
                        EmitterParam::Declination => e.declination = value,
                        EmitterParam::DeclinationVariation => e.declination_variation = value,
                        EmitterParam::PlanarAngle => e.planar_angle = value,
                        EmitterParam::PlanarAngleVariation => e.planar_angle_variation = value,
                    }
                }
                ParamTarget::ForceStrength { index } => {
                    if let Some(f) = self.desc.forces.get_mut(index) {
                        f.strength = value;
                    }
                }
                ParamTarget::Rotation { param } => {
                    if let Some(r) = self.desc.rotation.as_mut() {
                        match param {
                            RotationParam::Speed => r.speed = value,
                            RotationParam::SpeedVariation => r.speed_variation = value,
                            RotationParam::Angle => r.angle = value,
                            RotationParam::AngleVariation => r.angle_variation = value,
                        }
                    }
                }
            }
        }
        if let Some(vis) = &self.desc.vis {
            let local = crate::sequence::controller_time(
                time,
                vis.start_time,
                vis.stop_time,
                vis.looping,
            );
            self.visible = crate::sequence::sample_bool(&vis.keys, local).unwrap_or(true);
        }
    }

    /// `BSPSysRecycleBoundModifier` (RE 0x140D25DE0): runs after the
    /// position update (ORDER_POSTPOS_UPDATE), wrapping each axis into
    /// [corner, corner + extent) with a double-fmod — a particle already
    /// inside maps to itself exactly. Never kills; velocity, age and
    /// trails untouched.
    fn apply_recycle_bound(&mut self) {
        let Some(rb) = &self.desc.recycle else { return };
        if rb.extent.iter().any(|e| *e <= 0.0) {
            return;
        }
        for p in &mut self.particles {
            for axis in 0..3 {
                let local = p.position[axis] - rb.corner[axis];
                let e = rb.extent[axis];
                let wrapped = ((local % e) + e) % e;
                p.position[axis] = rb.corner[axis] + wrapped;
            }
        }
    }

    /// `NiPSysAgeDeathModifier::Update` — backwards so swap-removal is safe.
    fn age_and_kill(&mut self) {
        let strips = self.desc.strips.is_some();
        for i in (0..self.particles.len()).rev() {
            let dt = (self.time - self.particles[i].last_update).max(0.0);
            self.particles[i].age += dt;
            if self.particles[i].age > self.particles[i].life_span {
                self.particles.swap_remove(i);
                if strips {
                    let trail = self.trails.swap_remove(i);
                    if trail.points.len() > 1 {
                        self.detached_trails.push(trail);
                    }
                }
            }
        }
    }

    /// `BSPSysStripUpdateModifier` + `BSStripPSysData::RecordStripPoint`,
    /// RE'd from 1.5.97: the newest point glues to the particle every
    /// update; one point COMMITS per 1/60 s (the NIF's Update Delta Time
    /// is dead in this runtime); points expire from the tail after
    /// `max_point_count / 60` seconds, which also drains detached trails.
    fn record_trails(&mut self) {
        let Some(strip) = self.desc.strips.clone() else {
            return;
        };
        let capacity = strip.capacity();
        let cutoff = self.time - strip.point_lifetime();
        for (p, trail) in self.particles.iter().zip(self.trails.iter_mut()) {
            let point = TrailPoint {
                position: p.position,
                width: p.radius * p.size,
                color: p.color,
                time: self.time,
            };
            match trail.points.last_mut() {
                None => {
                    trail.points.push(point);
                    trail.last_commit = self.time;
                    trail.segments = 1;
                }
                Some(head) => {
                    // Head glued to the particle; commit a permanent point
                    // once the cadence elapses and the ring has room.
                    *head = point;
                    if self.time - trail.last_commit > 1.0 / 60.0
                        && trail.points.len() < capacity - 1
                    {
                        trail.points.push(point);
                        trail.last_commit = self.time;
                        trail.segments = trail.segments.max(trail.points.len() as u32);
                    }
                }
            }
            while trail.points.first().is_some_and(|p| p.time < cutoff) {
                trail.points.remove(0);
            }
        }
        for trail in &mut self.detached_trails {
            while trail.points.first().is_some_and(|p| p.time < cutoff) {
                trail.points.remove(0);
            }
        }
        self.detached_trails.retain(|t| t.points.len() > 1);
    }

    /// `NiPSysEmitter::SpawnParticles` — fractional birth rate carries over.
    fn spawn(&mut self, dt: f32) {
        let time = self.time;
        let capacity = self.desc.max_particles;
        for index in 0..self.desc.emitters.len() {
            // A mesh emitter whose shapes resolved to NOTHING emits
            // nothing, exactly as the game does: `NiPSysMeshEmitter`
            // samples the vertices of the shapes it names, and a name
            // that reaches no geometry leaves it nothing to sample.
            //
            // This used to fall through to "spawn at the origin, fly
            // up" -- visible and wrong, on the theory that it beats
            // invisible and wrong. It does not: a user's exported sword
            // pointed its emitter at a node with no geometry, the
            // PREVIEW showed a working effect, the GAME showed none, and
            // the divergence cost a round trip to find. Matching the
            // game and SAYING SO is the honest pair; the saying is
            // `unresolved_mesh_emitters`.
            if self.desc.emitters[index].mesh_without_geometry() {
                continue;
            }
            let (requested, active) = {
                let e = &self.desc.emitters[index];
                // An UNSET window is not a window the clock can be
                // outside of, so it does not gate: the runtime has no
                // such gate at all, it only clamps the scaled time.
                let mut active = if e.looping
                    || crate::sequence::window_unset(e.start_time, e.stop_time)
                {
                    true
                } else {
                    time >= e.start_time && time <= e.stop_time
                };
                // Keyed tracks run on the controller-local clock, looping
                // over the controller window like force toggles do.
                let local =
                    crate::sequence::controller_time(time, e.start_time, e.stop_time, e.looping);
                if !e.active_keys.is_empty() {
                    active &=
                        crate::sequence::sample_bool(&e.active_keys, local).unwrap_or(true);
                }
                let rate = if e.birth_keys.is_empty() {
                    e.birth_rate
                } else {
                    e.birth_keys.sample_f32(local).unwrap_or(e.birth_rate)
                };
                // Camera-distance LOD multiplies the sampled rate
                // (1.5.97 ScheduleSpawnTimes @ 0x140D054D0).
                (rate * self.lod_emit * dt, active)
            };
            if !active {
                continue;
            }
            let count = {
                let acc = &mut self.accumulators[index];
                *acc += requested;
                let n = acc.trunc();
                *acc -= n;
                n as usize
            };
            for _ in 0..count {
                if self.particles.len() >= capacity {
                    break;
                }
                let p = self.spawn_one(index);
                self.particles.push(p);
                if self.desc.strips.is_some() {
                    self.trails.push(Trail::default());
                }
            }
        }
    }

    fn spawn_one(&mut self, emitter_index: usize) -> Particle {
        // Field-disjoint borrows: the emitter is read through `&self.desc`
        // while the RNG mutates — no clone, because a mesh emitter's shape
        // carries its sampled geometry and cloning it per particle would be
        // an allocation storm.
        let e = &self.desc.emitters[emitter_index];

        // Note the two different variation ranges — this is what the game does:
        // speed and lifespan get half the variation, angles and radius get all
        // of it. See docs/knowledge/particle-runtime.md.
        let speed = e.speed + (self.rng.unit() - 0.5) * e.speed_variation;
        let mut life_span = e.life_span + (self.rng.unit() - 0.5) * e.life_span_variation;
        if life_span <= 0.0 {
            life_span = 1e-6;
        }
        let declination = e.declination + self.rng.signed() * e.declination_variation;
        let planar = e.planar_angle + self.rng.signed() * e.planar_angle_variation;
        // The LOD size multiplier applies at SPAWN (the game folds the
        // current psys+0x188 into the initial size, 0x140D09DC3).
        let radius = (e.initial_radius + self.rng.signed() * e.radius_variation) * self.lod_size;
        let initial_color = e.initial_color;

        let (mut position, direction) = Self::spawn_from_shape(&mut self.rng, &e.shape);
        if !matches!(e.shape, EmitterShape::Mesh { .. }) {
            // The anchor node orients the volume and its launch frame.
            position = rotate3(&e.rotation, position);
            position[0] += e.offset[0];
            position[1] += e.offset[1];
            position[2] += e.offset[2];
        }
        let velocity = match direction {
            // Mesh emitters own the direction; declination does not apply.
            Some(dir) => [dir[0] * speed, dir[1] * speed, dir[2] * speed],
            None => {
                // Declination is measured from +Z, planar angle is the azimuth.
                let (sin_d, cos_d) = declination.sin_cos();
                let (sin_p, cos_p) = planar.sin_cos();
                let local = [speed * sin_d * cos_p, speed * sin_d * sin_p, speed * cos_d];
                rotate3(&e.rotation, local)
            }
        };

        // BSPSysInheritVelocityModifier (RE 0x140D269E0): strict
        // rand·100 < chance; the multiplier's variation is RELATIVE —
        // uniform in [max(0, M−M·V), M+M·V] — and the anchor's world
        // velocity adds straight into the particle's, no space change.
        let mut velocity = velocity;
        if let Some(inh) = &self.desc.inherit {
            if self.rng.unit() * 100.0 < inh.chance {
                let m = inh.multiplier;
                let lo = (m - m * inh.variation).max(0.0);
                let hi = m + m * inh.variation;
                let k = lo + self.rng.unit() * (hi - lo);
                for axis in 0..3 {
                    velocity[axis] += self.anchor_velocity[axis] * k;
                }
            }
        }

        let (rotation_angle, rotation_speed) = match &self.desc.rotation {
            Some(r) => {
                let mut s = r.speed + self.rng.signed() * r.speed_variation;
                if r.random_speed_sign && self.rng.unit() < 0.5 {
                    s = -s;
                }
                (r.angle + self.rng.signed() * r.angle_variation, s)
            }
            None => (0.0, 0.0),
        };

        let atlas_cells = if self.desc.atlas_table.is_empty() {
            (self.desc.atlas_rows.max(1) * self.desc.atlas_cols.max(1)).max(1)
        } else {
            self.desc.atlas_table.len() as u32
        };
        Particle {
            position,
            velocity,
            age: 0.0,
            life_span,
            last_update: self.time,
            radius,
            size: 1.0,
            color: initial_color,
            rotation_angle,
            rotation_speed,
            atlas_index: (self.rng.unit() * atlas_cells as f32) as u32 % atlas_cells,
            subtex_step: -1,
            subtex_frames: 0.0,
        }
    }

    /// The shape-specific `ComputeSpawnPosition` vfuncs. The sphere and
    /// cylinder distributions are intentionally the game's biased ones.
    /// Mesh emitters also return the particle's direction, which replaces
    /// the declination path entirely.
    fn spawn_from_shape(rng: &mut Rng, shape: &EmitterShape) -> ([f32; 3], Option<[f32; 3]>) {
        match shape {
            EmitterShape::Box {
                width,
                height,
                depth,
            } => (
                [
                    (rng.unit() - 0.5) * width,
                    (rng.unit() - 0.5) * height,
                    (rng.unit() - 0.5) * depth,
                ],
                None,
            ),
            EmitterShape::Sphere { radius } => {
                // r is uniform in [0, radius] (no cube root) and phi sweeps a
                // full turn rather than [0, pi]: both quirks are in the game.
                let r = rng.unit() * radius;
                let theta = rng.unit() * TAU;
                let phi = rng.unit() * TAU;
                let rxy = phi.sin() * r;
                ([rxy * theta.cos(), rxy * theta.sin(), phi.cos() * r], None)
            }
            EmitterShape::Cylinder { radius, height } => {
                let r = rng.unit() * radius;
                let theta = rng.unit() * TAU;
                (
                    [
                        theta.cos() * r,
                        theta.sin() * r,
                        (rng.unit() - 0.5) * height,
                    ],
                    None,
                )
            }
            EmitterShape::Mesh {
                velocity_type,
                emission_type,
                emission_axis,
                sources,
                ..
            } => {
                let (position, normal) = Self::sample_mesh(rng, sources, *emission_type);
                let direction = match velocity_type {
                    0 => normal,
                    2 => normalize_or_up(*emission_axis),
                    _ => {
                        // Random unit direction; the biased sphere trick is
                        // fine for a preview scatter.
                        let theta = rng.unit() * TAU;
                        let phi = rng.unit() * TAU;
                        [phi.sin() * theta.cos(), phi.sin() * theta.sin(), phi.cos()]
                    }
                };
                (position, Some(direction))
            }
        }
    }

    /// Pick a point (and its normal) on the emitter meshes per `EmitFrom`.
    fn sample_mesh(
        rng: &mut Rng,
        sources: &[MeshEmitterSource],
        emission_type: u32,
    ) -> ([f32; 3], [f32; 3]) {
        let usable: Vec<&MeshEmitterSource> = sources
            .iter()
            .filter(|s| !s.vertices.is_empty())
            .collect();
        if usable.is_empty() {
            // Unresolved sources: spawn at the origin, fly up. Visible and
            // wrong beats invisible and wrong.
            return ([0.0; 3], [0.0, 0.0, 1.0]);
        }
        let source = usable[(rng.unit() * usable.len() as f32) as usize % usable.len()];

        let vertex_mode = source.triangles.is_empty()
            || matches!(emission_type, 0 | 2 | 4);
        if vertex_mode {
            let (p, n) =
                source.vertices[(rng.unit() * source.vertices.len() as f32) as usize
                    % source.vertices.len()];
            return (
                source.to_psys.point(p),
                normalize_or_up(source.to_psys.direction(n)),
            );
        }

        let tri = source.triangles
            [(rng.unit() * source.triangles.len() as f32) as usize % source.triangles.len()];
        let (a, an) = source.vertices[tri[0] as usize];
        let (b, bn) = source.vertices[tri[1] as usize];
        let (c, cn) = source.vertices[tri[2] as usize];
        let (u, v) = if emission_type == 1 {
            // Face centre.
            (1.0 / 3.0, 1.0 / 3.0)
        } else {
            // Uniform point on the triangle via the sqrt trick.
            let r1 = rng.unit().max(1e-6).sqrt();
            (1.0 - r1, rng.unit() * r1)
        };
        let w = 1.0 - u - v;
        let position = [
            a[0] * u + b[0] * v + c[0] * w,
            a[1] * u + b[1] * v + c[1] * w,
            a[2] * u + b[2] * v + c[2] * w,
        ];
        let normal = [
            an[0] * u + bn[0] * v + cn[0] * w,
            an[1] * u + bn[1] * v + cn[1] * w,
            an[2] * u + bn[2] * v + cn[2] * w,
        ];
        (
            source.to_psys.point(position),
            normalize_or_up(source.to_psys.direction(normal)),
        )
    }

    /// ORDER_GENERAL modifiers: colour, scale and rotation over life.
    fn apply_general(&mut self) {
        if let Some(color) = &self.desc.color {
            for p in &mut self.particles {
                p.color = color.sample(p.t());
            }
        }
        if let Some(scales) = &self.desc.scales {
            for p in &mut self.particles {
                // The scale modifier *overwrites* size, it does not multiply.
                p.size = sample_curve(scales, p.t());
            }
        }
        if self.desc.rotation.is_some() {
            let time = self.time;
            for p in &mut self.particles {
                let dt = (time - p.last_update).max(0.0);
                let mut angle = p.rotation_angle + dt * p.rotation_speed;
                if angle > ROTATION_ANGLE_RESET {
                    angle = 0.0;
                } else {
                    while angle > TAU {
                        angle -= TAU;
                    }
                }
                p.rotation_angle = angle;
            }
        }
        if let Some(st) = &self.desc.subtex {
            // BSPSysSubTexModifier::Update @ 0x140D64BF0 (1.5.97), verbatim:
            // the frame advances AT MOST one per update, paced so the
            // per-particle frame budget fits one lifespan; start→end once,
            // then looping over [loop start, end], the loop entry
            // re-randomized on every wrap. Every fudge adds 0..fudge.
            for p in &mut self.particles {
                if p.subtex_step == -1 {
                    p.subtex_frames =
                        (st.frame_count + self.rng.unit() * st.frame_count_fudge).trunc();
                }
                let step = if p.life_span > 0.0 {
                    (p.age / p.life_span * p.subtex_frames).trunc() as i32
                } else {
                    0
                };
                if step > p.subtex_step {
                    let newborn = p.subtex_step == -1;
                    p.subtex_step = step;
                    // The game stores a u8; `as u8` saturates rather than
                    // wraps, which only differs on data no file authors.
                    if newborn {
                        p.atlas_index =
                            (st.start_frame + self.rng.unit() * st.start_frame_fudge) as u8 as u32;
                    } else if (p.atlas_index + 1) as f32 <= st.end_frame {
                        p.atlas_index += 1;
                    } else {
                        p.atlas_index = (st.loop_start_frame
                            + self.rng.unit() * st.loop_start_frame_fudge)
                            as u8 as u32;
                    }
                }
            }
        }
    }

    /// ORDER_FORCE modifiers, accumulating into velocity.
    fn apply_forces(&mut self) {
        let time = self.time;
        for force in &self.desc.forces {
            if let Some(toggle) = &force.toggle {
                if !toggle.active_at(time) {
                    continue;
                }
            }
            for p in &mut self.particles {
                let dt = (time - p.last_update).max(0.0);
                if dt <= 0.0 {
                    continue;
                }
                match force.kind {
                    ForceKind::GravityPlanar | ForceKind::GravitySpherical => {
                        let (dir, distance) = if force.kind == ForceKind::GravityPlanar {
                            let dir = normalize(force.axis);
                            let delta = sub(force.origin, p.position);
                            (dir, dot(delta, dir))
                        } else {
                            let delta = sub(force.origin, p.position);
                            (normalize(delta), length(delta))
                        };
                        let decay = if force.decay == 0.0 {
                            1.0
                        } else {
                            (-(distance.abs()) * force.decay).exp()
                        };
                        let accel = force.strength * GRAVITY_STRENGTH_SCALE * decay;
                        let turbulence =
                            force.turbulence * force.turbulence_scale * TURBULENCE_SCALE;
                        for axis in 0..3 {
                            let turb = if turbulence != 0.0 {
                                self.rng.signed() * turbulence
                            } else {
                                0.0
                            };
                            p.velocity[axis] += (dir[axis] * accel + turb) * dt;
                        }
                    }
                    // RE 0x140D3D4B0: velocity += wind × strength × dt.
                    // The direction is global weather state, not a field
                    // — self.wind is the host's stand-in for it.
                    ForceKind::Wind => {
                        for axis in 0..3 {
                            p.velocity[axis] += self.wind[axis] * force.strength * dt;
                        }
                    }
                    // 1.5.97 @ 0x140D150B0: drag removes ONLY the velocity
                    // component along the axis; percentage is calibrated
                    // per 1/30 s frame (the binary multiplies by
                    // 30.000029), clamped so at most the whole component
                    // goes; range/rangeFalloff window the strength by
                    // distance from the drag object.
                    ForceKind::Drag => {
                        let mut pct = force.percentage;
                        if force.range < f32::MAX {
                            let d = length(sub(p.position, force.origin));
                            if d > force.range {
                                if d >= force.range_falloff {
                                    continue;
                                }
                                pct *= 1.0
                                    - (d - force.range) / (force.range_falloff - force.range);
                            }
                        }
                        let k = (pct * dt * 30.000_029).min(1.0);
                        let axis = force.axis;
                        let proj = dot(axis, p.velocity);
                        for a in 0..3 {
                            p.velocity[a] -= axis[a] * proj * k;
                        }
                    }
                    // 1.5.97 @ 0x140D0FDC0. `decay` is ALSO a hard range
                    // cutoff (spherical distance) when a decay type is set;
                    // the decay factor itself uses the symmetry-specific
                    // distance. Delta V is an acceleration, dt-scaled.
                    ForceKind::Bomb => {
                        let dp = sub(p.position, force.origin);
                        let dist = length(dp);
                        if force.decay_type != 0 && dist > force.decay {
                            continue;
                        }
                        let axis = force.axis;
                        let (dir, d) = match force.symmetry {
                            // Cylindrical: distance in the plane
                            // perpendicular to the axis.
                            1 => {
                                let along = dot(dp, axis);
                                let perp = [
                                    dp[0] - axis[0] * along,
                                    dp[1] - axis[1] * along,
                                    dp[2] - axis[2] * along,
                                ];
                                let n = length(perp);
                                if n <= 1e-6 {
                                    continue;
                                }
                                ([perp[0] / n, perp[1] / n, perp[2] / n], n)
                            }
                            // Planar: two-sided — always pushes (or pulls)
                            // along the axis away from the plane.
                            2 => {
                                let along = dot(dp, axis);
                                if along < 0.0 {
                                    ([-axis[0], -axis[1], -axis[2]], -along)
                                } else {
                                    (axis, along)
                                }
                            }
                            // Spherical.
                            _ => {
                                if dist <= 1e-6 {
                                    continue;
                                }
                                ([dp[0] / dist, dp[1] / dist, dp[2] / dist], dist)
                            }
                        };
                        let factor = match force.decay_type {
                            1 => {
                                if force.decay <= 0.0 {
                                    continue;
                                }
                                (force.decay - d) / force.decay
                            }
                            2 => {
                                if force.decay <= 0.0 {
                                    continue;
                                }
                                (-d / force.decay).exp()
                            }
                            _ => 1.0,
                        };
                        let accel = factor * force.strength * dt;
                        for axis_i in 0..3 {
                            p.velocity[axis_i] += dir[axis_i] * accel;
                        }
                    }
                }
            }
        }
    }

    /// `NiPSysColliderManager::Update` (1.5.97 @ 0x140D12700), run before
    /// position integration: the earliest hit in the window
    /// `[last_update, time]` reflects the velocity (whole vector × bounce),
    /// moves the particle to the hit point and rewinds `last_update` to
    /// the hit time so integration covers only the remainder. Spheres hit
    /// from BOTH sides — an inside particle reflects off the exit point,
    /// which is what contains an effect. Inactive colliders are skipped,
    /// exactly like the game skips inactive modifiers.
    fn apply_colliders(&mut self) {
        if self.desc.colliders.is_empty() {
            return;
        }
        let time = self.time;
        let strips = self.desc.strips.is_some();
        for i in (0..self.particles.len()).rev() {
            let (hit, died) = {
                let p = &mut self.particles[i];
                let t0 = p.last_update;
                if time <= t0 {
                    continue;
                }
                let mut best = time;
                let mut hit: Option<(usize, [f32; 3])> = None;
                for (ci, c) in self.desc.colliders.iter().enumerate() {
                    if !c.active {
                        continue;
                    }
                    if let Some((t_abs, point)) =
                        c.shape.intersect(p.position, p.velocity, t0, best)
                    {
                        best = t_abs;
                        hit = Some((ci, point));
                    }
                }
                let Some((ci, point)) = hit else { continue };
                let c = &self.desc.colliders[ci];
                let n = c.shape.normal_at(point);
                let vn = dot(n, p.velocity);
                if c.shape.is_plane() && best >= time && vn < 0.001 {
                    // Resting/grazing on a plane: slide, no bounce.
                    for a in 0..3 {
                        p.velocity[a] -= n[a] * vn;
                    }
                } else {
                    for a in 0..3 {
                        p.velocity[a] = (p.velocity[a] - 2.0 * n[a] * vn) * c.bounce;
                    }
                }
                p.position = point;
                p.last_update = best;
                (true, c.die_on_collide)
            };
            if hit && died {
                self.particles.swap_remove(i);
                if strips {
                    let trail = self.trails.swap_remove(i);
                    if trail.points.len() > 1 {
                        self.detached_trails.push(trail);
                    }
                }
            }
        }
    }

    /// `NiPSysPositionModifier::Update` — the only place `last_update` moves.
    fn integrate_positions(&mut self) {
        let time = self.time;
        for p in &mut self.particles {
            let dt = (time - p.last_update).max(0.0);
            for axis in 0..3 {
                p.position[axis] += p.velocity[axis] * dt;
            }
            p.last_update = time;
        }
    }
}

impl ColorOverLife {
    /// RGB uses the four colour stops; alpha uses its own fade pair. A stop of
    /// exactly zero disables its branch, which is how vanilla turns a fade off.
    pub fn sample(&self, t: f32) -> [f32; 4] {
        let [c0, c1, c2] = self.colors;
        let rgb = if self.color2_start - self.color1_end != 0.0 && t < self.color2_start {
            if t >= self.color1_end {
                let u = (t - self.color1_end) / (self.color2_start - self.color1_end);
                lerp3(c0, c1, u)
            } else {
                [c0[0], c0[1], c0[2]]
            }
        } else if self.color3_start - self.color2_end == 0.0 || t <= self.color2_end {
            [c1[0], c1[1], c1[2]]
        } else if t <= self.color3_start {
            let u = (t - self.color2_end) / (self.color3_start - self.color2_end);
            lerp3(c1, c2, u)
        } else {
            [c2[0], c2[1], c2[2]]
        };

        let alpha = if self.fade_out != 0.0 && t > self.fade_out {
            let u = (t - self.fade_out) / (1.0 - self.fade_out);
            c1[3] + (c2[3] - c1[3]) * u
        } else if self.fade_in != 0.0 && t < self.fade_in {
            c0[3] + (c1[3] - c0[3]) * (t / self.fade_in)
        } else {
            c1[3]
        };

        [rgb[0], rgb[1], rgb[2], alpha]
    }
}

/// `BSTableCurve::EvaluateNormalized` — uniformly spaced samples, lerped.
pub fn sample_curve(values: &[f32], t: f32) -> f32 {
    crate::scale_table::sample(values, t)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length(v: [f32; 3]) -> f32 {
    dot(v, v).sqrt()
}

/// The game treats anything below 1e-6 as a zero-length vector.
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = length(v);
    if len <= 1e-6 {
        [0.0, 0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

fn lerp3(a: [f32; 4], b: [f32; 4], u: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * u,
        a[1] + (b[1] - a[1]) * u,
        a[2] + (b[2] - a[2]) * u,
    ]
}

// ---------------------------------------------------------------------------
// Graph extraction
// ---------------------------------------------------------------------------

/// Build simulation descriptions for every Particle System in the graph.
impl SystemDesc {
    /// Resolve mesh-emitter shape names into geometry. The lookup is the
    /// host's business — the viewport answers from the imported model —
    /// which keeps this crate free of any scene ownership.
    pub fn resolve_mesh_emitters(
        &mut self,
        lookup: &mut dyn FnMut(&str) -> Option<MeshEmitterGeometry>,
    ) {
        let inv = self.psys_world_inverse();
        for e in &mut self.emitters {
            if let EmitterShape::Mesh {
                mesh_names,
                sources,
                ..
            } = &mut e.shape
            {
                sources.clear();
                for name in mesh_names.iter() {
                    let Some(g) = lookup(name) else { continue };
                    sources.push(MeshEmitterSource {
                        name: name.clone(),
                        vertices: g.vertices,
                        triangles: g.triangles,
                        to_psys: inv.compose_world(&g.world),
                    });
                }
            }
        }
    }

    /// The shape names no mesh emitter of this system could resolve.
    ///
    /// The viewport reports these to the Log: an emitter that silently
    /// produces nothing is indistinguishable from one whose birth rate
    /// is zero, and the user has no way to tell which they are looking
    /// at.
    pub fn unresolved_mesh_emitters(&self) -> Vec<String> {
        let mut out = Vec::new();
        for e in &self.emitters {
            if !e.unresolved_mesh() {
                continue;
            }
            if let EmitterShape::Mesh { mesh_names, .. } = &e.shape {
                out.extend(mesh_names.iter().cloned());
            }
        }
        out
    }

    /// Re-place every mesh emitter's geometry under the CURRENT scene
    /// poses, without re-reading it.
    ///
    /// Volume emitters have followed animation since the anchor work;
    /// mesh emitters were resolved once and stayed at bind pose, so an
    /// effect whose emitter shapes are animated span the spawn area
    /// frozen. Measured on blizzardhazard.nif, whose `mLoop` turns
    /// `RotHelper`, `RotHelper01` and `RotHelper02`: the rings TURN, and
    /// the particles must be laid around the turning circle.
    ///
    /// `world_of` returns the shape's current world matrix, in the same
    /// column-major layout `resolve_mesh_emitters` took. A name that no
    /// longer resolves keeps the pose it had -- a missing lookup must
    /// not teleport a spawn area to the origin.
    pub fn repose_mesh_emitters(
        &mut self,
        world_of: &mut dyn FnMut(&str) -> Option<[f32; 16]>,
    ) {
        let inv = self.psys_world_inverse();
        for e in &mut self.emitters {
            if let EmitterShape::Mesh { sources, .. } = &mut e.shape {
                for src in sources.iter_mut() {
                    if let Some(world) = world_of(&src.name) {
                        src.to_psys = inv.compose_world(&world);
                    }
                }
            }
        }
    }

    /// The inverse of this system's world transform, for taking scene
    /// geometry into the space particles live in.
    fn psys_world_inverse(&self) -> InverseWorld {
        InverseWorld {
            rotation: self.world_rotation,
            translation: self.world_offset,
            scale: self.world_scale,
        }
    }
}

/// The inverse of a system's world transform.
///
/// `world = R * (s * p) + t`, so `p = R^T * (world - t) / s`. The
/// rotation is stored as a pure rotation with the scale kept apart, so
/// the transpose IS the inverse -- the same reasoning the gizmo's Local
/// basis rests on.
struct InverseWorld {
    rotation: [f32; 9],
    translation: [f32; 3],
    scale: f32,
}

impl InverseWorld {
    fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let d = [
            p[0] - self.translation[0],
            p[1] - self.translation[1],
            p[2] - self.translation[2],
        ];
        let r = self.direction(d);
        let s = if self.scale.abs() > 1e-6 { self.scale } else { 1.0 };
        [r[0] / s, r[1] / s, r[2] / s]
    }

    /// `shape-local -> psys` for a shape standing at `world`
    /// (column-major 4x4, the scene's own layout).
    ///
    /// `psys = R^T * (world_point - T) / s` and
    /// `world_point = Mw * local + Tw`, so the composition folds into one
    /// affine: `M = R^T * Mw / s`, `t = R^T * (Tw - T) / s`.
    fn compose_world(&self, world: &[f32; 16]) -> MeshPose {
        let s = if self.scale.abs() > 1e-6 { self.scale } else { 1.0 };
        // Columns of the scene matrix are its basis vectors.
        let col = |i: usize| [world[i * 4], world[i * 4 + 1], world[i * 4 + 2]];
        let mut m = [0.0f32; 9];
        for (c, v) in [col(0), col(1), col(2)].into_iter().enumerate() {
            let r = self.direction(v);
            m[c] = r[0] / s;
            m[3 + c] = r[1] / s;
            m[6 + c] = r[2] / s;
        }
        MeshPose {
            m,
            t: self.point([world[12], world[13], world[14]]),
        }
    }

    /// Rotation only -- a normal carries no translation and no scale.
    fn direction(&self, v: [f32; 3]) -> [f32; 3] {
        let r = &self.rotation;
        [
            r[0] * v[0] + r[3] * v[1] + r[6] * v[2],
            r[1] * v[0] + r[4] * v[1] + r[7] * v[2],
            r[2] * v[0] + r[5] * v[1] + r[8] * v[2],
        ]
    }
}

pub fn systems_from_graph(graph: &Graph) -> Vec<SystemDesc> {
    let mut out = Vec::new();
    let mut ids: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.kind == NodeKind::ParticleSystem)
        .map(|(id, _)| *id)
        .collect();
    ids.sort();
    for id in ids {
        out.push(system_from_graph(graph, id));
    }
    out
}

fn wired(graph: &Graph, node: NodeId, socket: usize) -> Vec<NodeId> {
    graph.incoming(node, socket).map(|l| l.from_node).collect()
}

fn system_from_graph(graph: &Graph, system: NodeId) -> SystemDesc {
    let node = &graph.nodes[&system];
    let mut desc = SystemDesc {
        name: node.str_or("Name", "ParticleFX"),
        source_block: node.source_block.map(|b| b as usize),
        max_particles: 100,
        world_scale: node.f32_or("World Scale", 1.0).max(1e-3),
        world_rotation: {
            let e = node.vec3_or("World Rotation", [0.0; 3]);
            crate::mesh::Transform::rows_from_euler(e[0], e[1], e[2])
        },
        world_offset: node.vec3_or("World Offset", [0.0; 3]),
        ..SystemDesc::default()
    };

    if let Some(data) = wired(graph, system, 0).first() {
        let n = &graph.nodes[data];
        desc.max_particles = n.u32_or("Max Particles", 100) as usize;
        desc.atlas_rows = n.u32_or("Subtexture Rows", 1).max(1);
        desc.atlas_cols = n.u32_or("Subtexture Cols", 1).max(1);
        desc.atlas_table = crate::shader_preview::atlas_cells(n);
        if n.bool_or("Strip Trails", false) {
            desc.strips = Some(StripDesc {
                max_point_count: n.u32_or("Max Point Count", 32).max(2),
            });
        }
        // The 1.5.97 quad fill: a rotation-angles array disables the whole
        // aspect feature, and flag bits 0/1 are indistinguishable.
        if !n.bool_or("Has Rotations", false) {
            desc.aspect_ratio = n.f32_or("Aspect Ratio", 1.0);
            desc.velocity_aspect =
                n.bool_or("Velocity Aspect", false) || n.bool_or("Rotate To Velocity", false);
            desc.speed_to_aspect = n.bool_or("Speed To Aspect", false);
            desc.aspect2 = n.f32_or("Aspect 2", 0.0);
            desc.aspect_speed1 = n.f32_or("Aspect Speed 1", 0.0);
            desc.aspect_speed2 = n.f32_or("Aspect Speed 2", 0.0);
        }
    }

    // Shader: texture + tint + greyscale palette. Alpha: blend preset
    // (index 0 = Additive).
    if let Some(shader) = wired(graph, system, 1).first() {
        let n = &graph.nodes[shader];
        desc.texture = n.str_or("Source Texture", "");
        desc.tint = n.color4_or("Base Color", [1.0; 4]);
        desc.base_color_scale = n.f32_or("Base Color Scale", 1.0);
        desc.greyscale_texture = n.str_or("Greyscale Texture", "");
        // The flags sample nothing without a ramp texture — same gate the
        // game and the mesh path apply.
        if !desc.greyscale_texture.is_empty() {
            desc.greyscale_color = n.bool_or("Greyscale Color", false);
            desc.greyscale_alpha = n.bool_or("Greyscale Alpha", false);
        }
        desc.uv_offset = [n.f32_or("UV Offset U", 0.0), n.f32_or("UV Offset V", 0.0)];
        desc.uv_scale = [n.f32_or("UV Scale U", 1.0), n.f32_or("UV Scale V", 1.0)];
        desc.soft_depth = n.f32_or("Soft Falloff Depth", 0.0);
        // The free controller tracks the shader node carries as key
        // text. Empty text parses to an empty curve, which the
        // per-frame apply leaves alone.
        desc.emissive_scale_keys = n.curve_or("Emissive Scale Keys", crate::curve::Track::Float);
        desc.alpha_keys = n.curve_or("Alpha Keys", crate::curve::Track::Float);
        desc.color_keys = n.curve_or("Color Keys", crate::curve::Track::Vec3);
        desc.uv_keys = crate::graph::EFFECT_SHADER_FLOAT_KEYS
            .iter()
            .filter(|(_, v)| (6..=9).contains(v))
            .map(|(p, v)| (*v, n.curve_or(p, crate::curve::Track::Float)))
            .filter(|(_, c)| !c.is_empty())
            .collect();
        desc.use_falloff = n.bool_or("Use Falloff", false);
        desc.falloff = [
            n.f32_or("Falloff Start Angle", 1.0),
            n.f32_or("Falloff Stop Angle", 1.0),
            n.f32_or("Falloff Start Opacity", 0.0),
            n.f32_or("Falloff Stop Opacity", 0.0),
        ];
    }
    if let Some(alpha) = wired(graph, system, 2).first() {
        // Read out of the FLAG WORD, not out of the preset, because the
        // game reads the word: the effect shader's technique selection
        // branches on the DESTINATION factor (1.5.97 0x140D5B81), so a
        // Custom blend that happens to end in ONE is additive in game
        // and must be additive here. One rule, one place.
        let flags = crate::graph::alpha_flags(&graph.nodes[alpha]);
        desc.additive =
            crate::mesh::blend_from_alpha_flags(flags) == Some(crate::mesh::BlendMode::Additive);
    }

    // Birth rate lives on the emitter controller, which points at its
    // emitter; active toggles point at their target modifier the same way.
    let mut rate_for: Vec<(NodeId, &crate::graph::GraphNode)> = Vec::new();
    let mut toggle_for: Vec<(NodeId, ForceToggle)> = Vec::new();
    // Param tracks: (target node, controller node). Resolved to indices
    // AFTER the modifier loop, because an index into `desc.emitters` or
    // `desc.forces` only exists once those vectors are built.
    let mut param_ctlrs: Vec<(NodeId, &crate::graph::GraphNode)> = Vec::new();
    let mut emitter_nodes: Vec<NodeId> = Vec::new();
    let mut force_nodes: Vec<NodeId> = Vec::new();
    for ctlr in wired(graph, system, 4) {
        let c = &graph.nodes[&ctlr];
        match c.kind {
            // The multi-target controller degenerates to a plain
            // emitter controller with one anchor — the game's own
            // arithmetic (RE 0x140D58520: perTarget = n·min(max,1)/1),
            // and it never reads active keys (RE 0x140D582F0), which
            // the Multi Emitter node does not have anyway.
            NodeKind::EmitterCtlr | NodeKind::MultiEmitterCtlr => {
                if let Some(emitter) = wired(graph, ctlr, 0).first() {
                    rate_for.push((*emitter, c));
                }
            }
            NodeKind::ParamCtlr => {
                if let Some(target) = wired(graph, ctlr, 0).first() {
                    param_ctlrs.push((*target, c));
                }
            }
            NodeKind::VisibilityCtlr => {
                let keys: Vec<(f32, bool)> =
                    c.curve_or("Keys", crate::curve::Track::Bool)
                        .to_bool_keys()
                        .into_iter()
                        .map(|(t, v)| (t, v != 0))
                        .collect();
                if !keys.iter().all(|(_, v)| *v) {
                    desc.vis = Some(VisTrack {
                        keys,
                        start_time: c.f32_or("Start Time", 0.0),
                        stop_time: c.f32_or("Stop Time", 1.0),
                        looping: c.bool_or("Loop", true),
                    });
                }
            }
            NodeKind::ModifierActiveCtlr => {
                if let Some(target) = wired(graph, ctlr, 0).first() {
                    let keys = c
                        .curve_or("Keys", crate::curve::Track::Bool)
                        .to_bool_keys()
                        .into_iter()
                        .map(|(t, v)| (t, v != 0))
                        .collect();
                    toggle_for.push((
                        *target,
                        ForceToggle {
                            keys,
                            start_time: c.f32_or("Start Time", 0.0),
                            stop_time: c.f32_or("Stop Time", 1.0),
                            looping: c.bool_or("Loop", true),
                        },
                    ));
                }
            }
            _ => {}
        }
    }
    let toggle_of = |m: NodeId| -> Option<ForceToggle> {
        toggle_for
            .iter()
            .find(|(target, _)| *target == m)
            .map(|(_, t)| t.clone())
    };

    for m in wired(graph, system, 3) {
        let n = &graph.nodes[&m];
        match n.kind {
            NodeKind::BoxEmitter
            | NodeKind::SphereEmitter
            | NodeKind::CylinderEmitter
            | NodeKind::MeshEmitter => {
                let shape = match n.kind {
                    NodeKind::BoxEmitter => EmitterShape::Box {
                        width: n.f32_or("Width", 10.0),
                        height: n.f32_or("Height", 10.0),
                        depth: n.f32_or("Depth", 10.0),
                    },
                    NodeKind::SphereEmitter => EmitterShape::Sphere {
                        radius: n.f32_or("Radius", 10.0),
                    },
                    NodeKind::MeshEmitter => EmitterShape::Mesh {
                        velocity_type: n.enum_index("Velocity Type") as u32,
                        emission_type: n.enum_index("Emission Type") as u32,
                        emission_axis: n.vec3_or("Emission Axis", [1.0, 0.0, 0.0]),
                        mesh_names: n
                            .str_or("Emitter Meshes", "")
                            .split(',')
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                            .map(str::to_string)
                            .collect(),
                        sources: Vec::new(),
                    },
                    _ => EmitterShape::Cylinder {
                        radius: n.f32_or("Radius", 10.0),
                        height: n.f32_or("Height", 10.0),
                    },
                };
                let ctlr = rate_for.iter().find(|(e, _)| *e == m).map(|(_, c)| *c);
                desc.emitters.push(Emitter {
                    shape,
                    speed: n.f32_or("Speed", 0.0),
                    speed_variation: n.f32_or("Speed Variation", 0.0),
                    declination: n.f32_or("Declination", 0.0),
                    declination_variation: n.f32_or("Declination Variation", 0.0),
                    planar_angle: n.f32_or("Planar Angle", 0.0),
                    planar_angle_variation: n.f32_or("Planar Angle Variation", 0.0),
                    initial_color: n.color4_or("Initial Color", [1.0; 4]),
                    initial_radius: n.f32_or("Initial Radius", 1.0),
                    radius_variation: n.f32_or("Radius Variation", 0.0),
                    life_span: n.f32_or("Life Span", 1.0),
                    life_span_variation: n.f32_or("Life Span Variation", 0.0),
                    birth_rate: ctlr.map(|c| c.f32_or("Birth Rate", 20.0)).unwrap_or(0.0),
                    birth_keys: ctlr
                        .map(|c| c.curve_or("Birth Rate Keys", crate::curve::Track::Float))
                        .unwrap_or_else(|| crate::curve::Curve::empty(crate::curve::Track::Float)),
                    active_keys: ctlr
                        .map(|c| {
                            let keys = c
                                .curve_or("Emitter Active Keys", crate::curve::Track::Bool)
                                .to_bool_keys();
                            if keys.iter().all(|(_, v)| *v != 0) {
                                Vec::new() // always on — skip the gate
                            } else {
                                keys.into_iter().map(|(t, v)| (t, v != 0)).collect()
                            }
                        })
                        .unwrap_or_default(),
                    start_time: ctlr.map(|c| c.f32_or("Start Time", 0.0)).unwrap_or(0.0),
                    stop_time: ctlr.map(|c| c.f32_or("Stop Time", 1.0)).unwrap_or(0.0),
                    looping: ctlr.map(|c| c.bool_or("Loop", true)).unwrap_or(true),
                    offset: n.vec3_or("Emitter Offset", [0.0; 3]),
                    rotation: {
                        let e = n.vec3_or("Emitter Rotation", [0.0; 3]);
                        crate::mesh::Transform::rows_from_euler(e[0], e[1], e[2])
                    },
                    source_block: n.source_block.map(|b| b as usize),
                });
                emitter_nodes.push(m);
            }
            NodeKind::GravityModifier => {
                force_nodes.push(m);
                desc.forces.push(Force {
                kind: if n.enum_index("Force Type") == 1 {
                    ForceKind::GravitySpherical
                } else {
                    ForceKind::GravityPlanar
                },
                axis: n.vec3_or("Axis", [0.0, 0.0, -1.0]),
                strength: n.f32_or("Strength", 9.8),
                decay: n.f32_or("Decay", 0.0),
                turbulence: n.f32_or("Turbulence", 0.0),
                turbulence_scale: n.f32_or("Turbulence Scale", 1.0),
                percentage: 0.0,
                range: f32::MAX,
                range_falloff: f32::MAX,
                decay_type: 0,
                symmetry: 0,
                    origin: n.vec3_or("Gravity Offset", [0.0; 3]),
                    toggle: toggle_of(m),
                })
            }
            NodeKind::DragModifier => {
                force_nodes.push(m);
                desc.forces.push(Force {
                kind: ForceKind::Drag,
                axis: normalize_or_up(n.vec3_or("Axis", [0.0, 0.0, 1.0])),
                strength: 0.0,
                decay: 0.0,
                turbulence: 0.0,
                turbulence_scale: 1.0,
                percentage: n.f32_or("Percentage", 0.05),
                range: n.f32_or("Range", f32::MAX),
                range_falloff: n.f32_or("Range Falloff", f32::MAX),
                decay_type: 0,
                symmetry: 0,
                    origin: n.vec3_or("Drag Offset", [0.0; 3]),
                    toggle: toggle_of(m),
                })
            }
            NodeKind::BombModifier => {
                force_nodes.push(m);
                desc.forces.push(Force {
                kind: ForceKind::Bomb,
                axis: normalize_or_up(n.vec3_or("Axis", [0.0, 0.0, 1.0])),
                strength: n.f32_or("Delta V", 10.0),
                decay: n.f32_or("Decay", 0.0),
                turbulence: 0.0,
                turbulence_scale: 1.0,
                percentage: 0.0,
                range: f32::MAX,
                range_falloff: f32::MAX,
                decay_type: n.enum_index("Decay Type") as u32,
                symmetry: n.enum_index("Symmetry") as u32,
                    origin: n.vec3_or("Bomb Offset", [0.0; 3]),
                    toggle: toggle_of(m),
                })
            }
            NodeKind::WindModifier => {
                force_nodes.push(m);
                desc.forces.push(Force {
                    kind: ForceKind::Wind,
                    axis: [0.0, 1.0, 0.0],
                    strength: n.f32_or("Strength", 0.25),
                    decay: 0.0,
                    turbulence: 0.0,
                    turbulence_scale: 1.0,
                    percentage: 0.0,
                    range: f32::MAX,
                    range_falloff: f32::MAX,
                    decay_type: 0,
                    symmetry: 0,
                    origin: [0.0; 3],
                    toggle: toggle_of(m),
                })
            }
            NodeKind::InheritVelocityModifier => {
                desc.inherit = Some(InheritVelocity {
                    chance: n.f32_or("Chance To Inherit", 100.0),
                    multiplier: n.f32_or("Velocity Multiplier", 0.5),
                    variation: n.f32_or("Velocity Variation", 0.0),
                    node: n.str_or("Inherit Node", ""),
                });
            }
            NodeKind::RecycleBoundModifier => {
                let anchor = n.vec3_or("Anchor Offset", [0.0; 3]);
                let offset = n.vec3_or("Bound Offset", [0.0; 3]);
                desc.recycle = Some(RecycleBound {
                    corner: [
                        anchor[0] + offset[0],
                        anchor[1] + offset[1],
                        anchor[2] + offset[2],
                    ],
                    extent: n.vec3_or("Bound Extent", [100.0; 3]),
                });
            }
            NodeKind::LodModifier => {
                desc.lod = Some(LodDesc {
                    begin: n.f32_or("LOD Begin Distance", 0.1),
                    end: n.f32_or("LOD End Distance", 0.7),
                    end_emit_scale: n.f32_or("End Emit Scale", 0.2),
                    end_size: n.f32_or("End Size", 1.0),
                });
            }
            NodeKind::ColliderModifier => {
                let offset = n.vec3_or("Collider Offset", [0.0; 3]);
                let rot = n.vec3_or("Collider Rotation", [0.0; 3]);
                let rows = crate::mesh::Transform::rows_from_euler(rot[0], rot[1], rot[2]);
                let shape = if n.enum_index("Shape") == 1 {
                    let x = normalize_or_up(rotate3(&rows, n.vec3_or("X Axis", [1.0, 0.0, 0.0])));
                    let y = normalize_or_up(rotate3(&rows, n.vec3_or("Y Axis", [0.0, 1.0, 0.0])));
                    let normal = normalize_or_up([
                        x[1] * y[2] - x[2] * y[1],
                        x[2] * y[0] - x[0] * y[2],
                        x[0] * y[1] - x[1] * y[0],
                    ]);
                    ColliderShape::Plane {
                        origin: offset,
                        normal,
                        x_axis: x,
                        y_axis: y,
                        half_width: n.f32_or("Width", 10.0) * 0.5,
                        half_height: n.f32_or("Height", 10.0) * 0.5,
                    }
                } else {
                    ColliderShape::Sphere {
                        center: offset,
                        radius: n.f32_or("Radius", 10.0),
                    }
                };
                desc.colliders.push(Collider {
                    active: n.bool_or("Active", true),
                    bounce: n.f32_or("Bounce", 1.0),
                    die_on_collide: n.bool_or("Die On Collide", false),
                    shape,
                });
            }
            NodeKind::SimpleColorModifier => {
                desc.color = Some(ColorOverLife {
                    colors: [
                        n.color4_or("Color 1", [1.0, 1.0, 1.0, 0.0]),
                        n.color4_or("Color 2", [1.0; 4]),
                        n.color4_or("Color 3", [1.0, 1.0, 1.0, 0.0]),
                    ],
                    fade_in: n.f32_or("Fade In End %", 0.2),
                    fade_out: n.f32_or("Fade Out Start %", 0.8),
                    color1_end: n.f32_or("Color 1 End %", 0.3),
                    color2_start: n.f32_or("Color 2 Start %", 0.4),
                    color2_end: n.f32_or("Color 2 End %", 0.6),
                    color3_start: n.f32_or("Color 3 Start %", 0.7),
                })
            }
            NodeKind::ScaleModifier => {
                let values = crate::scale_table::parse(&n.str_or("Scales", "1.0, 1.0"));
                if !values.is_empty() {
                    desc.scales = Some(values);
                }
            }
            NodeKind::RotationModifier => {
                desc.rotation = Some(Rotation {
                    speed: n.f32_or("Rotation Speed", 0.0),
                    speed_variation: n.f32_or("Speed Variation", 0.0),
                    angle: n.f32_or("Rotation Angle", 0.0),
                    angle_variation: n.f32_or("Angle Variation", 0.0),
                    random_speed_sign: n.bool_or("Random Speed Sign", false),
                })
            }
            NodeKind::SubTexModifier => {
                desc.subtex = Some(SubTexFrames {
                    start_frame: n.f32_or("Start Frame", 0.0),
                    start_frame_fudge: n.f32_or("Start Frame Fudge", 0.0),
                    end_frame: n.f32_or("End Frame", 15.0),
                    loop_start_frame: n.f32_or("Loop Start Frame", 0.0),
                    loop_start_frame_fudge: n.f32_or("Loop Start Frame Fudge", 0.0),
                    frame_count: n.f32_or("Frame Count", 16.0),
                    frame_count_fudge: n.f32_or("Frame Count Fudge", 0.0),
                })
            }
            // NodeKind::SpawnModifier is deliberately inert: SkyrimSE
            // 1.5.97 stubs the engine's spawn path out (RE @ 0x140D20B90),
            // so a preview that spawned would show what the game cannot.
            _ => {}
        }
    }

    // Param tracks resolve LAST: an index into `desc.emitters` or
    // `desc.forces` only means something once both vectors are final.
    // The enum indices match graph::PARAM_TARGETS by position.
    for (target, c) in param_ctlrs {
        let keys = c.curve_or("Keys", crate::curve::Track::Float);
        if keys.is_empty() {
            continue;
        }
        let target_node = &graph.nodes[&target];
        let resolved = match c.enum_index("Parameter") {
            0 => emitter_nodes.iter().position(|n| *n == target).map(|index| {
                ParamTarget::Emitter { index, param: EmitterParam::Speed }
            }),
            1 => force_nodes
                .iter()
                .position(|n| *n == target)
                .filter(|_| target_node.kind == NodeKind::GravityModifier)
                .map(|index| ParamTarget::ForceStrength { index }),
            2 => emitter_nodes.iter().position(|n| *n == target).map(|index| {
                ParamTarget::Emitter { index, param: EmitterParam::InitialRadius }
            }),
            3 => emitter_nodes.iter().position(|n| *n == target).map(|index| {
                ParamTarget::Emitter { index, param: EmitterParam::LifeSpan }
            }),
            4 => emitter_nodes.iter().position(|n| *n == target).map(|index| {
                ParamTarget::Emitter { index, param: EmitterParam::Declination }
            }),
            5 => emitter_nodes.iter().position(|n| *n == target).map(|index| {
                ParamTarget::Emitter { index, param: EmitterParam::DeclinationVariation }
            }),
            6 => emitter_nodes.iter().position(|n| *n == target).map(|index| {
                ParamTarget::Emitter { index, param: EmitterParam::PlanarAngle }
            }),
            7 => emitter_nodes.iter().position(|n| *n == target).map(|index| {
                ParamTarget::Emitter { index, param: EmitterParam::PlanarAngleVariation }
            }),
            8 => Some(ParamTarget::Rotation { param: RotationParam::Speed }),
            9 => Some(ParamTarget::Rotation { param: RotationParam::SpeedVariation }),
            10 => Some(ParamTarget::Rotation { param: RotationParam::Angle }),
            11 => Some(ParamTarget::Rotation { param: RotationParam::AngleVariation }),
            _ => None,
        };
        if let Some(target) = resolved {
            desc.param_tracks.push(ParamTrack {
                target,
                keys,
                start_time: c.f32_or("Start Time", 0.0),
                stop_time: c.f32_or("Stop Time", 1.0),
                looping: c.bool_or("Loop", true),
            });
        }
    }
    desc
}

#[cfg(test)]
mod shader_track_tests {
    use super::SystemDesc;
    use crate::curve::{Curve, Track};

    /// The runtime semantics, pinned: variable 5 clamps below at ZERO
        /// and nowhere else (`SetMaterialAlpha`, 0x1412D6410), the colour
    /// track writes RGB and leaves alpha alone (0x141322860), variable
    /// 0 replaces the emissive multiple, and empty curves touch
    /// nothing. The preview loops over the key span.
    #[test]
    fn tracks_overwrite_the_material_the_way_the_game_does() {
        let mut d = SystemDesc::default();
        d.tint = [0.2, 0.4, 0.6, 0.8];
        d.base_color_scale = 3.0;

        // Empty curves: nothing moves.
        d.apply_shader_tracks(5.0);
        assert_eq!(d.tint, [0.2, 0.4, 0.6, 0.8]);
        assert_eq!(d.base_color_scale, 3.0);

        // Alpha: overwritten, negative clamps to zero, above one is
                // ALLOWED (the game only floors it).
        d.alpha_keys = Curve::parse(Track::Float, "0 = -0.5, 1 = 2.0");
        d.apply_shader_tracks(0.0);
        assert_eq!(d.tint[3], 0.0, "negative floors at zero");
        d.apply_shader_tracks(0.999_999);
        assert!(d.tint[3] > 1.0, "the game does not cap alpha at one");

        // ...and the clock LOOPS over the span: t = 1.5 lands at 0.5.
        d.apply_shader_tracks(1.5);
        let looped = d.tint[3];
        d.apply_shader_tracks(0.5);
        assert_eq!(d.tint[3], looped, "t past the span wraps");

        // Colour: RGB replaced, alpha untouched.
        d.color_keys = Curve::parse(Track::Vec3, "0 = 1 0 0, 2 = 0 0 1");
        d.apply_shader_tracks(0.0);
        assert_eq!([d.tint[0], d.tint[1], d.tint[2]], [1.0, 0.0, 0.0]);
        let alpha_before = d.tint[3];
        d.apply_shader_tracks(1.0);
        assert_eq!(d.tint[3], alpha_before, "colour never touches alpha");
        assert!(d.tint[2] > 0.0, "the sweep moved toward blue");

        // Emissive multiple: variable 0.
        d.emissive_scale_keys = Curve::parse(Track::Float, "0 = 10, 1 = 20");
        d.apply_shader_tracks(0.5);
        assert!((d.base_color_scale - 3.0).abs() > 0.5, "the scale animated");
    }

    /// `systems_from_graph` reads the key params off the shader node,
        /// so an edit in the Inspector reaches the preview.
    #[test]
    fn the_graph_params_reach_the_system_desc() {
        use crate::graph::{Graph, GraphNode, Link, NodeKind};
        use crate::value::Value;
        let mut g = Graph::new();
        let out = g.insert(GraphNode::new(NodeKind::SceneOutput, [0.0, 0.0]));
        let sys = g.insert(GraphNode::new(NodeKind::ParticleSystem, [0.0, 0.0]));
        let data = g.insert(GraphNode::new(NodeKind::ParticleData, [0.0, 0.0]));
        let shader = g.insert(GraphNode::new(NodeKind::EffectShader, [0.0, 0.0]));
        g.nodes
            .get_mut(&shader)
            .unwrap()
            .set_param(
                "Alpha Keys",
                Value::Curve(Curve::parse(Track::Float, "0 = 0, 2 = 1")),
            );
        for (from, to, input) in [(sys, out, 0), (data, sys, 0), (shader, sys, 1)] {
            g.connect(Link {
                from_node: from,
                from_socket: 0,
                to_node: to,
                to_socket: input,
            })
            .unwrap();
        }
        let systems = super::systems_from_graph(&g);
        assert_eq!(systems.len(), 1);
        assert_eq!(systems[0].alpha_keys.keys.len(), 2, "the param reached the desc");
    }
}
