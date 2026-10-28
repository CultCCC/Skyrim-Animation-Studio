//! Effects in the viewport: art NIFs from PIE cues (effect meshes and
//! particle systems run by Aether's game-accurate simulator) and Precision
//! weapon trails. Everything here becomes `FxBatch`es — camera-facing or
//! world-space triangles, textured, alpha- or additive-blended, with the
//! effect shader's greyscale-to-palette ramp.

use crate::render::{FxBatch, FxVertex};
use aether_core::mesh::{BlendMode, MeshScene, RenderMesh, ShaderAnimKind, Transform};
use aether_core::sequence::{sample_bool, SequenceClip, TrackKeys, TrackTarget};
use aether_core::sim::{systems_from_graph, EmitterShape, MeshEmitterGeometry, Simulation, SystemDesc};
use sds_vfs::Vfs;
use std::collections::HashMap;
use std::sync::Arc;

pub struct FxMesh {
    /// Shape name and its node's name (visibility tracks name either).
    pub name: String,
    pub node: String,
    pub verts: Vec<([f32; 3], [f32; 2], [f32; 4])>,
    pub indices: Vec<u16>,
    pub texture: String,
    pub palette: String,
    pub additive: bool,
    pub mode: f32,
    pub grey: [f32; 3],
    /// The shape as read, for re-posing and shader animation per frame.
    pub src: Option<RenderMesh>,
}

/// One art NIF, read once.
pub struct FxAsset {
    pub meshes: Vec<FxMesh>,
    pub systems: Vec<SystemDesc>,
    /// Some mesh emitter names geometry the NIF does not have: in game the
    /// engine binds it to the target's 3D (the weapon for blade effects).
    pub needs_target: bool,
    /// `AttachT` extra data `NamedNode&<node>`: the engine hangs the art's
    /// root on that node of the target (BSAttachTechniques), e.g. WEAPON.
    pub attach_node: Option<String>,
    /// NIF sequences (mBegin / mLoop / mEnd of BeginLoopEnd effects).
    pub clips: Vec<SequenceClip>,
    /// Node tree, billboards and free-running node controllers.
    pub scene: MeshScene,
    pub notes: Vec<String>,
}

pub fn load_asset(vfs: &Vfs, path: &str) -> FxAsset {
    let empty = |note: String| FxAsset { meshes: Vec::new(), systems: Vec::new(), needs_target: false, attach_node: None, clips: Vec::new(), scene: MeshScene::default(), notes: vec![note] };
    let bytes = match vfs.read(path) {
        Ok(b) => b,
        Err(e) => return empty(format!("{path}: {e}")),
    };
    let nif = match aether_nif::NifFile::parse(&bytes) {
        Ok(n) => n,
        Err(e) => return empty(format!("{path}: {e:?}")),
    };
    let mut notes = Vec::new();
    let mut attach_node = None;
    if let Ok(ctx) = aether_nif::typed::Ctx::of(&nif.header) {
        use aether_nif::typed::TypedBlock;
        for b in &nif.blocks {
            if let Ok(e) = aether_nif::typed::controllers::NiStringsExtraData::decode(b, ctx) {
                if e.name.resolve(&nif.header).is_some_and(|n| n.eq_ignore_ascii_case("AttachT")) {
                    for v in &e.values {
                        if let Some(node) = v.strip_prefix("NamedNode&") {
                            attach_node = Some(node.trim().to_string());
                        } else {
                            notes.push(sds_i18n::tf!("{path}: AttachT «{v}» не поддержан", "{path}: AttachT «{v}» is not supported"));
                        }
                    }
                }
            }
        }
    }
    let scene = aether_core::mesh::meshes_from_nif(&nif);
    let mut meshes = Vec::new();
    for m in &scene.meshes {
        // Refraction shapes only distort what is behind them (their texture
        // is a normal map); the game draws them in no colour pass.
        if m.hidden || m.marker || m.no_material || m.refraction {
            continue;
        }
        let mut f = fx_mesh(m);
        f.name = m.name.clone();
        f.node = m.node.and_then(|n| scene.nodes.get(n)).map(|n| n.name.clone()).unwrap_or_default();
        f.src = Some(m.clone());
        meshes.push(f);
    }
    let mut needs_target = false;
    let clips;
    let systems = match aether_core::decompile::decompile(&nif) {
        Ok(d) => {
            let mut v = systems_from_graph(&d.graph);
            for desc in &mut v {
                // Mesh emitters sample the NIF's own shapes when it has them.
                desc.resolve_mesh_emitters(&mut |name: &str| {
                    let mesh = scene.meshes.iter().find(|m| m.name == name)?;
                    Some(MeshEmitterGeometry {
                        vertices: mesh.vertices.iter().map(|v| (v.position, v.normal)).collect(),
                        triangles: mesh.indices.chunks_exact(3).map(|t| [t[0] as u32, t[1] as u32, t[2] as u32]).collect(),
                        world: mesh.world,
                    })
                });
                if std::env::var("SDS_FX_DEBUG").is_ok() {
                    eprintln!("FXDEBUG {path} {}: tex={:?} pal={:?} gc={} ga={} add={} tint={:?} bcs={}", desc.name, desc.texture, desc.greyscale_texture, desc.greyscale_color, desc.greyscale_alpha, desc.additive, desc.tint, desc.base_color_scale);
                }
                let unresolved = desc.unresolved_mesh_emitters();
                if !unresolved.is_empty() {
                    needs_target = true;
                    notes.push(sds_i18n::tf!("{}: эмиттер по мешу цели ({}) — излучает с геометрии оружия", "{}: target-mesh emitter ({}) — emits from the weapon geometry", desc.name, unresolved.join(", ")));
                }
            }
            v
        }
        Err(aether_core::decompile::DecompileError::NoParticleSystems) => Vec::new(),
        Err(e) => {
            notes.push(sds_i18n::tf!("{path}: частицы не разобраны: {e}", "{path}: particles not parsed: {e}"));
            Vec::new()
        }
    };
    // Shapes that emitters sample are spawn areas, not something drawn.
    let emitter_shapes: Vec<String> = systems
        .iter()
        .flat_map(|d| d.emitters.iter())
        .filter_map(|e| match &e.shape {
            EmitterShape::Mesh { mesh_names, .. } => Some(mesh_names.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    if std::env::var("SDS_FX_DEBUG").is_ok() {
        eprintln!("FXM {path}: emitter shapes {:?}; meshes {:?}", emitter_shapes, meshes.iter().map(|m| (m.name.clone(), m.node.clone(), m.texture.clone())).collect::<Vec<_>>());
    }
    meshes.retain(|m| !emitter_shapes.iter().any(|n| n == &m.name));
    clips = aether_core::sequence::sequences_from_nif(&nif).clips;
    if std::env::var("SDS_FX_DEBUG").is_ok() {
        for c in &clips {
            eprintln!("FXC {path} clip {} {:.2}s: {:?}", c.name, c.duration(), c.tracks.iter().map(|t| format!("{:?}", t.target)).collect::<Vec<_>>());
        }
    }
    FxAsset { meshes, systems, needs_target, attach_node, clips, scene, notes }
}

/// An effect-shaded mesh as an FX mesh, its own world applied.
fn fx_mesh(m: &aether_core::mesh::RenderMesh) -> FxMesh {
    let w = Transform::from_column_major(&m.world);
    let mat = &m.material;
    let grey = !mat.greyscale_texture.is_empty() && (mat.greyscale_color || mat.greyscale_alpha);
    let mode = if grey { (mat.greyscale_color as u32 | ((mat.greyscale_alpha as u32) << 1)) as f32 } else { 0.0 };
    // Ramped RGB never shows the tint: keep the vertex colour straight then.
    let tint = if grey && mat.greyscale_color { [1.0, 1.0, 1.0, m.tint[3]] } else { m.tint };
    FxMesh {
        name: String::new(),
        node: String::new(),
        verts: m
            .vertices
            .iter()
            .map(|v| {
                let uv = [v.uv[0] * mat.uv_scale[0] + mat.uv_offset[0], v.uv[1] * mat.uv_scale[1] + mat.uv_offset[1]];
                (w.transform_point(v.position), uv, [v.color[0] * tint[0], v.color[1] * tint[1], v.color[2] * tint[2], v.color[3] * tint[3]])
            })
            .collect(),
        indices: m.indices.to_vec(),
        texture: m.texture.clone(),
        palette: if grey { mat.greyscale_texture.clone() } else { String::new() },
        additive: m.blend == BlendMode::Additive,
        mode,
        grey: [mat.base_color_scale.max(0.0), mat.base_color[0], mat.base_color[3]],
        src: None,
    }
}

/// Animated shader values of one effect shape: (variable, value) floats
/// (0 emissive multiple, 5 alpha, 6-9 UV) and base colours, written the way
/// the game's effect-shader controllers write them.
fn animate_shader(m: &mut RenderMesh, floats: &[(u32, f32)], colors: &[[f32; 3]]) {
    let mat = &mut m.material;
    for rgb in colors {
        mat.base_color[0] = rgb[0];
        mat.base_color[1] = rgb[1];
        mat.base_color[2] = rgb[2];
    }
    let mut alpha = m.tint[3];
    for &(var, v) in floats {
        match var {
            0 => mat.base_color_scale = v,
            5 => alpha = v.clamp(0.0, 1.0),
            6 => mat.uv_offset[0] = v,
            7 => mat.uv_scale[0] = v,
            8 => mat.uv_offset[1] = v,
            9 => mat.uv_scale[1] = v,
            _ => {}
        }
    }
    mat.base_color[3] = alpha;
    let s = mat.base_color_scale;
    m.tint = [mat.base_color[0] * s, mat.base_color[1] * s, mat.base_color[2] * s, alpha];
}

fn push_mesh(m: &FxMesh, world: &Transform, out: &mut Vec<FxBatch>) {
    let verts = m
        .verts
        .iter()
        .map(|(p, uv, c)| FxVertex { pos: world.transform_point(*p), uv: *uv, color: *c, additive: if m.additive { 1.0 } else { 0.0 }, mode: m.mode, grey: m.grey })
        .collect();
    out.push(FxBatch { verts, indices: m.indices.iter().map(|&i| i as u32).collect(), texture: m.texture.clone(), palette: m.palette.clone() });
}

/// Geometry a target-bound emitter samples, in the art's own space.
pub struct TargetGeometry {
    pub vertices: Vec<([f32; 3], [f32; 3])>,
    pub triangles: Vec<[u32; 3]>,
}

/// Running simulations of one piece of one cue.
struct Running {
    sims: Vec<Simulation>,
    time: f32,
    used: bool,
}

#[derive(Default)]
pub struct FxPlayer {
    assets: HashMap<String, Arc<FxAsset>>,
    running: HashMap<String, Running>,
    pub notes: Vec<String>,
}

pub struct Camera {
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
}

const DT: f32 = 1.0 / 60.0;

const IDENTITY16: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

impl FxPlayer {
    pub fn asset(&mut self, vfs: &Vfs, path: &str) -> Arc<FxAsset> {
        if let Some(a) = self.assets.get(path) {
            return a.clone();
        }
        let a = Arc::new(load_asset(vfs, path));
        for n in &a.notes {
            self.notes.push(n.clone());
        }
        self.assets.insert(path.to_string(), a.clone());
        a
    }

    pub fn begin_frame(&mut self) {
        for r in self.running.values_mut() {
            r.used = false;
        }
    }

    pub fn end_frame(&mut self) {
        self.running.retain(|_, r| r.used);
    }

    /// The clip playing at `t` seconds into the art, and the time inside it:
    /// mBegin once, then mLoop looping (the BeginLoopEnd autoplay); a NIF
    /// with other sequences loops its first one.
    fn clip_at(clips: &[SequenceClip], t: f32) -> Option<(&SequenceClip, f32)> {
        let named = |n: &str| clips.iter().find(|c| c.name.eq_ignore_ascii_case(n));
        let begin = named("mBegin");
        let looped = named("mLoop");
        if let Some(b) = begin {
            let d = b.duration();
            if t < d || looped.is_none() {
                return Some((b, t.min(d)));
            }
            let l = looped.unwrap();
            let ld = l.duration().max(1e-3);
            return Some((l, (t - d) % ld));
        }
        if let Some(l) = looped {
            return Some((l, t % l.duration().max(1e-3)));
        }
        clips.first().map(|c| (c, t % c.duration().max(1e-3)))
    }

    /// Emitter tracks of a clip at one time, as Aether's player applies them.
    fn apply_sim_tracks(sims: &mut [Simulation], clip: &SequenceClip, t: f32) {
        for track in &clip.tracks {
            match (&track.target, &track.keys) {
                (TrackTarget::EmitterBirthRate { system }, TrackKeys::Float(keys)) => {
                    if let Some(rate) = keys.sample(t) {
                        for sim in sims.iter_mut().filter(|s| s.desc.name == *system) {
                            for e in &mut sim.desc.emitters {
                                e.birth_rate = rate;
                                e.birth_keys.keys.clear();
                                e.active_keys.clear();
                                e.start_time = f32::MIN;
                                e.stop_time = f32::MAX;
                                e.looping = true;
                            }
                        }
                    }
                }
                (TrackTarget::EmitterActive { system }, TrackKeys::Bool(keys)) => {
                    if let Some(active) = sample_bool(keys, t) {
                        for sim in sims.iter_mut().filter(|s| s.desc.name == *system) {
                            for e in &mut sim.desc.emitters {
                                e.active_keys.clear();
                                if active {
                                    e.start_time = f32::MIN;
                                    e.stop_time = f32::MAX;
                                } else {
                                    e.start_time = f32::MAX;
                                    e.stop_time = f32::MAX;
                                }
                            }
                        }
                    }
                }
                (TrackTarget::EmitterSpeed { system }, TrackKeys::Float(keys)) => {
                    if let Some(speed) = keys.sample(t) {
                        for sim in sims.iter_mut().filter(|s| s.desc.name == *system) {
                            for e in &mut sim.desc.emitters {
                                e.speed = speed;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Nodes a clip hides at one time (visibility tracks).
    fn hidden_at(clip: &SequenceClip, t: f32) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        for track in &clip.tracks {
            match (&track.target, &track.keys) {
                (TrackTarget::Visibility { node }, TrackKeys::Bool(keys)) => {
                    if let Some(v) = sample_bool(keys, t) {
                        out.push((node.clone(), !v));
                    }
                }
                (TrackTarget::Visibility { node }, TrackKeys::Float(keys)) => {
                    if let Some(v) = keys.sample(t) {
                        out.push((node.clone(), v == 0.0));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Batches for one placed art piece, `local_t` seconds after it began.
    /// `target` binds the emitters that name the target's 3D.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, key: &str, asset: &FxAsset, world: &Transform, local_t: f32, cam: &Camera, target: Option<&TargetGeometry>, out: &mut Vec<FxBatch>) {
        // Effect meshes, minus what the playing sequence hides, posed and
        // shaded by it (and by free-running controllers), billboards
        // turned to the camera.
        let playing = Self::clip_at(&asset.clips, local_t);
        let hidden = playing.map(|(c, ct)| Self::hidden_at(c, ct)).unwrap_or_default();
        let is_hidden = |names: &[&str]| hidden.iter().any(|(n, h)| *h && names.iter().any(|x| !x.is_empty() && x.eq_ignore_ascii_case(n)));
        let scene = &asset.scene;
        let mut overrides = if scene.has_billboards() {
            // Camera into the art's own space.
            let inv = world.inverse();
            let norm = |v: [f32; 3]| {
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
                [v[0] / l, v[1] / l, v[2] / l]
            };
            scene.billboard_overrides(inv.transform_point(cam.eye), norm(inv.rotate(cam.right)), norm(inv.rotate(cam.up)))
        } else {
            HashMap::new()
        };
        let mut seq_floats: Vec<(String, u32, f32)> = Vec::new();
        let mut seq_colors: Vec<(String, [f32; 3])> = Vec::new();
        let mut seq_nodes: Vec<String> = Vec::new();
        if let Some((clip, ct)) = playing {
            for track in &clip.tracks {
                match (&track.target, &track.keys) {
                    (TrackTarget::EffectShaderFloat { node, variable }, TrackKeys::Float(keys)) => {
                        if let Some(v) = keys.sample(ct) {
                            seq_floats.push((node.clone(), *variable, v));
                        }
                    }
                    (TrackTarget::EffectShaderColor { node }, TrackKeys::Color(keys)) => {
                        if let Some(rgb) = aether_core::sequence::sample_color(keys, ct) {
                            seq_colors.push((node.clone(), rgb));
                        }
                    }
                    (TrackTarget::NodeTransform { node }, keys) => {
                        let sample = match keys {
                            TrackKeys::Transform(k) => Some(aether_core::sequence::sample_transform(k, ct)),
                            TrackKeys::Path(k) => Some(aether_core::path::sample_path(k, ct)),
                            _ => None,
                        };
                        if let (Some(s), Some(i)) = (sample, scene.node_index(node)) {
                            overrides.insert(i, scene.nodes[i].local.with_sample(&s));
                            seq_nodes.push(node.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        for anim in &scene.node_anims {
            let name = &scene.nodes[anim.node].name;
            if !seq_nodes.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                overrides.insert(anim.node, scene.nodes[anim.node].local.with_sample(&anim.sample(local_t)));
            }
        }
        for m in &asset.meshes {
            if is_hidden(&[&m.name, &m.node]) {
                continue;
            }
            let Some(src) = &m.src else {
                push_mesh(m, world, out);
                continue;
            };
            let named = |n: &str| n.eq_ignore_ascii_case(&m.name) || (!m.node.is_empty() && n.eq_ignore_ascii_case(&m.node));
            let mut floats: Vec<(u32, f32)> = src.shader_anims.iter().filter(|a| a.kind == ShaderAnimKind::Effect).filter_map(|a| a.sample(local_t).map(|v| (a.variable, v))).collect();
            let mut colors: Vec<[f32; 3]> = src.shader_color_anims.iter().filter(|a| a.effect && a.variable == 0).filter_map(|a| a.sample(local_t)).collect();
            // A sequence wins over a free-running controller on the same slot.
            for (n, var, v) in &seq_floats {
                if named(n) {
                    floats.retain(|(x, _)| x != var);
                    floats.push((*var, *v));
                }
            }
            for (n, rgb) in &seq_colors {
                if named(n) {
                    colors.clear();
                    colors.push(*rgb);
                }
            }
            if overrides.is_empty() && floats.is_empty() && colors.is_empty() {
                push_mesh(m, world, out);
                continue;
            }
            let mut r = src.clone();
            if let (Some(n), false) = (r.node, overrides.is_empty()) {
                r.world = scene.node_world(n, &overrides).compose(&r.local).to_column_major();
            }
            animate_shader(&mut r, &floats, &colors);
            if r.tint[3] <= 0.0 {
                continue;
            }
            push_mesh(&fx_mesh(&r), world, out);
        }
        if asset.systems.is_empty() {
            return;
        }
        let r = self.running.entry(key.to_string()).or_insert_with(|| Running {
            sims: asset
                .systems
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let mut d = d.clone();
                    if let Some(g) = target {
                        if !d.unresolved_mesh_emitters().is_empty() {
                            // The art sits on the weapon node, and the
                            // weapon vertices are already in that space.
                            d.resolve_mesh_emitters(&mut |_name: &str| {
                                Some(MeshEmitterGeometry { vertices: g.vertices.clone(), triangles: g.triangles.clone(), world: IDENTITY16 })
                            });
                        }
                    }
                    Simulation::new(d, 0x5D5_0000 + i as u32)
                })
                .collect(),
            time: 0.0,
            used: true,
        });
        r.used = true;
        // Scrubbing back replays from the start; forward steps on.
        if local_t + 1e-4 < r.time {
            for s in &mut r.sims {
                s.reset();
            }
            r.time = 0.0;
        }
        let mut steps = 0;
        while r.time + DT <= local_t && steps < 1200 {
            if let Some((clip, ct)) = Self::clip_at(&asset.clips, r.time) {
                Self::apply_sim_tracks(&mut r.sims, clip, ct);
            }
            for s in &mut r.sims {
                s.step(DT);
            }
            r.time += DT;
            steps += 1;
        }
        for sim in &r.sims {
            if !sim.visible || is_hidden(&[&sim.desc.name]) {
                continue;
            }
            // Strip systems draw their trails as ribbons, not billboards.
            if sim.desc.strips.is_some() {
                if let Some(rm) = aether_core::strip::strip_render_mesh(sim) {
                    push_mesh(&fx_mesh(&rm), world, out);
                }
                continue;
            }
            if sim.particles.is_empty() {
                continue;
            }
            let d = &sim.desc;
            let psys = Transform::from_av(d.world_offset, d.world_rotation, d.world_scale);
            let place = world.compose(&psys);
            let grey = !d.greyscale_texture.is_empty() && (d.greyscale_color || d.greyscale_alpha);
            let mode = if grey { (d.greyscale_color as u32 | ((d.greyscale_alpha as u32) << 1)) as f32 } else { 0.0 };
            let fold_rgb = !(grey && d.greyscale_color);
            let falloff = d.falloff_factor_head_on();
            let mut batch = FxBatch {
                verts: Vec::with_capacity(sim.particles.len() * 4),
                indices: Vec::with_capacity(sim.particles.len() * 6),
                texture: d.texture.clone(),
                palette: if grey { d.greyscale_texture.clone() } else { String::new() },
            };
            let add = if d.additive { 1.0 } else { 0.0 };
            if std::env::var("SDS_FX_DEBUG").is_ok() {
                if let Some(p) = sim.particles.first() {
                    eprintln!("FXP {} n={} color={:?} size={} radius={} atlas={} uvs={:?} mode={} grey=({}, {}, {})", d.name, sim.particles.len(), p.color, p.size, p.radius, p.atlas_index, d.atlas_uv(p.atlas_index), mode, d.base_color_scale, d.tint[0], d.tint[3]);
                }
            }
            for p in &sim.particles {
                let c = place.transform_point(p.position);
                let h = p.radius * p.size * 0.5 * place.s * sim.lod_size;
                if h <= 0.0 {
                    continue;
                }
                let (sa, ca) = p.rotation_angle.sin_cos();
                let rx = [cam.right[0] * ca + cam.up[0] * sa, cam.right[1] * ca + cam.up[1] * sa, cam.right[2] * ca + cam.up[2] * sa];
                let ry = [cam.up[0] * ca - cam.right[0] * sa, cam.up[1] * ca - cam.right[1] * sa, cam.up[2] * ca - cam.right[2] * sa];
                let (mut u0, mut v0, mut u1, mut v1) = d.atlas_uv(p.atlas_index);
                u0 = u0 * d.uv_scale[0] + d.uv_offset[0];
                u1 = u1 * d.uv_scale[0] + d.uv_offset[0];
                v0 = v0 * d.uv_scale[1] + d.uv_offset[1];
                v1 = v1 * d.uv_scale[1] + d.uv_offset[1];
                let k = |i: usize| if fold_rgb { d.tint[i] } else { 1.0 };
                let col = [p.color[0] * k(0), p.color[1] * k(1), p.color[2] * k(2), p.color[3] * d.tint[3] * falloff];
                let base = batch.verts.len() as u32;
                for (sx, sy, u, v) in [(-1.0, -1.0, u0, v1), (1.0, -1.0, u1, v1), (1.0, 1.0, u1, v0), (-1.0, 1.0, u0, v0)] {
                    let pos = [c[0] + (rx[0] * sx + ry[0] * sy) * h, c[1] + (rx[1] * sx + ry[1] * sy) * h, c[2] + (rx[2] * sx + ry[2] * sy) * h];
                    batch.verts.push(FxVertex { pos, uv: [u, v], color: col, additive: add, mode, grey: [d.base_color_scale.max(0.0), d.tint[0], d.tint[3]] });
                }
                batch.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
            if !batch.indices.is_empty() {
                out.push(batch);
            }
        }
    }
}

// ---- Precision trails ----------------------------------------------------------

/// A trail definition from Precision's toml (`[[TrailDefinition]]`).
#[derive(Debug, Clone, Default)]
pub struct TrailDef {
    pub priority: i64,
    pub all: bool,
    pub weapon_names: Vec<String>,
    pub weapon_keywords: Vec<String>,
    pub enchantment_names: Vec<String>,
    pub effect_names: Vec<String>,
    pub effect_keywords: Vec<String>,
    /// (plugin lowercase, local id)
    pub effect_shaders: Vec<(String, u32)>,
    pub lifetime_mult: Option<f32>,
    pub color: Option<[f32; 4]>,
    pub color_scale_mult: Option<f32>,
    pub mesh: Option<String>,
    pub source: String,
}

fn toml_strings(v: &str) -> Vec<String> {
    v.trim().trim_start_matches('[').trim_end_matches(']').split(',').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect()
}

fn toml_num(v: &str) -> Option<f64> {
    let v = v.trim();
    if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        return u64::from_str_radix(h, 16).ok().map(|x| x as f64);
    }
    v.parse().ok()
}

/// `{ r = 1, g = 0.35, ... }` or `{ FormID = 0x4E810, Plugin = "Skyrim.esm" }` → key/value pairs.
fn toml_inline(v: &str) -> Vec<(String, String)> {
    v.trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"').to_string()))
        .collect()
}

/// The `[[TrailDefinition]]` tables of one toml file (the subset Precision uses).
pub fn parse_trail_defs(text: &str, source: &str) -> Vec<TrailDef> {
    let mut out: Vec<TrailDef> = Vec::new();
    let mut in_trail = false;
    let mut pending = String::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        // Join arrays / tables that span lines.
        pending.push_str(line);
        pending.push(' ');
        let opens = pending.matches(['[', '{']).count();
        let closes = pending.matches([']', '}']).count();
        if pending.trim_start().starts_with('[') && !pending.contains('=') {
            // a table header, complete on one line
        } else if opens > closes {
            continue;
        }
        let stmt = std::mem::take(&mut pending);
        let stmt = stmt.trim();
        if stmt.starts_with('[') && !stmt.contains('=') {
            let header = stmt.trim_matches(['[', ']', ' ']);
            if stmt.starts_with("[[") {
                in_trail = header.eq_ignore_ascii_case("TrailDefinition");
                if in_trail {
                    out.push(TrailDef { source: source.to_string(), ..Default::default() });
                }
            } else if !header.to_ascii_lowercase().starts_with("traildefinition.") {
                in_trail = false;
            }
            continue;
        }
        if !in_trail {
            continue;
        }
        let Some((k, v)) = stmt.split_once('=') else { continue };
        let Some(d) = out.last_mut() else { continue };
        match k.trim().to_ascii_lowercase().as_str() {
            "priority" => d.priority = toml_num(v).unwrap_or(0.0) as i64,
            "all" => d.all = v.trim() == "true",
            "weaponnames" => d.weapon_names = toml_strings(v),
            "weaponkeywords" => d.weapon_keywords = toml_strings(v),
            "enchantmentnames" => d.enchantment_names = toml_strings(v),
            "effectnames" => d.effect_names = toml_strings(v),
            "effectkeywords" => d.effect_keywords = toml_strings(v),
            "effectshaders" => {
                let body = v.trim().trim_start_matches('[').trim_end_matches(']');
                for part in body.split('}') {
                    let kv = toml_inline(part.trim_start_matches(',').trim());
                    let id = kv.iter().find(|(k, _)| k == "formid").and_then(|(_, v)| toml_num(v));
                    let plugin = kv.iter().find(|(k, _)| k == "plugin").map(|(_, v)| v.to_ascii_lowercase());
                    if let (Some(id), Some(p)) = (id, plugin) {
                        d.effect_shaders.push((p, id as u32 & 0xFFFFFF));
                    }
                }
            }
            "lifetimemult" => d.lifetime_mult = toml_num(v).map(|x| x as f32),
            "basecolorscalemult" => d.color_scale_mult = toml_num(v).map(|x| x as f32),
            "basecoloroverride" => {
                let kv = toml_inline(v);
                let g = |n: &str, dflt: f32| kv.iter().find(|(k, _)| k == n).and_then(|(_, v)| v.parse().ok()).unwrap_or(dflt);
                d.color = Some([g("r", 1.0), g("g", 1.0), g("b", 1.0), g("a", 1.0)]);
            }
            "trailmeshoverride" => d.mesh = Some(v.trim().trim_matches('"').to_string()),
            _ => {}
        }
    }
    out
}

impl TrailDef {
    fn conditions(&self) -> usize {
        self.weapon_names.len() + self.weapon_keywords.len() + self.enchantment_names.len() + self.effect_names.len() + self.effect_keywords.len() + self.effect_shaders.len()
    }

    /// Does the weapon match (any / all of the conditions)?
    pub fn matches(&self, w: &sds_records::fx::WeaponFacts) -> bool {
        let has = |hay: &str, needle: &str| !needle.is_empty() && hay.to_lowercase().contains(&needle.to_lowercase());
        let any_in = |list: &[String], needle: &str| list.iter().any(|h| has(h, needle));
        let mut results: Vec<bool> = Vec::new();
        results.extend(self.weapon_names.iter().map(|n| has(&w.name, n)));
        results.extend(self.weapon_keywords.iter().map(|n| any_in(&w.keywords, n)));
        results.extend(self.enchantment_names.iter().map(|n| has(&w.enchantment, n)));
        results.extend(self.effect_names.iter().map(|n| has(&w.effect_name, n)));
        results.extend(self.effect_keywords.iter().map(|n| any_in(&w.effect_keywords, n)));
        let shader_hit = w.effect_shader.as_ref().is_some_and(|(p, id)| self.effect_shaders.iter().any(|(sp, sid)| sp.eq_ignore_ascii_case(p) && sid == id));
        if !self.effect_shaders.is_empty() {
            results.push(shader_hit);
        }
        if results.is_empty() {
            return false;
        }
        if self.all {
            results.iter().all(|r| *r)
        } else {
            results.iter().any(|r| *r)
        }
    }
}

/// Precision's choice: ALL-definitions first, then by priority.
pub fn pick_trail<'a>(defs: &'a [TrailDef], w: &sds_records::fx::WeaponFacts) -> Option<&'a TrailDef> {
    let mut m: Vec<&TrailDef> = defs.iter().filter(|d| d.conditions() > 0 && d.matches(w)).collect();
    m.sort_by(|a, b| b.all.cmp(&a.all).then(b.priority.cmp(&a.priority)));
    m.into_iter().next()
}

/// Trail settings from Precision's MCM ini ([Trails]).
#[derive(Debug, Clone)]
pub struct TrailSettings {
    pub enabled: bool,
    pub use_attack_length: bool,
    pub segment_lifetime: f32,
    pub fade_time: f32,
    pub segments_per_second: f32,
    pub color: [f32; 4],
    pub color_scale_mult: f32,
}

impl Default for TrailSettings {
    fn default() -> Self {
        TrailSettings { enabled: true, use_attack_length: false, segment_lifetime: 0.1, fade_time: 0.1, segments_per_second: 120.0, color: [0.53, 0.53, 0.53, 1.0], color_scale_mult: 1.0 }
    }
}

impl TrailSettings {
    pub fn apply_ini(&mut self, text: &str) {
        for line in text.lines() {
            let Some((k, v)) = line.trim().split_once('=') else { continue };
            let f = v.trim().parse::<f32>().ok();
            match k.trim().to_ascii_lowercase().as_str() {
                "bdisplaytrails" => self.enabled = f.is_none_or(|x| x != 0.0),
                "btrailuseattackcollisionlength" => self.use_attack_length = f.is_some_and(|x| x != 0.0),
                "ftrailsegmentlifetime" => self.segment_lifetime = f.unwrap_or(self.segment_lifetime),
                "ftrailfadeouttime" => self.fade_time = f.unwrap_or(self.fade_time),
                "utrailsegmentspersecond" => self.segments_per_second = f.unwrap_or(self.segments_per_second),
                "ftraildefaultbasecolorr" => self.color[0] = f.unwrap_or(self.color[0]),
                "ftraildefaultbasecolorg" => self.color[1] = f.unwrap_or(self.color[1]),
                "ftraildefaultbasecolorb" => self.color[2] = f.unwrap_or(self.color[2]),
                "ftraildefaultbasecolora" => self.color[3] = f.unwrap_or(self.color[3]),
                "ftrailbasecolorscalemult" => self.color_scale_mult = f.unwrap_or(self.color_scale_mult),
                _ => {}
            }
        }
    }
}

/// A ribbon through (base, tip) samples, newest last; alpha by age.
pub fn ribbon(samples: &[([f32; 3], [f32; 3], f32)], color: [f32; 4], texture: &str, out: &mut Vec<FxBatch>) {
    if samples.len() < 2 {
        return;
    }
    let mut b = FxBatch { verts: Vec::new(), indices: Vec::new(), texture: texture.to_string(), palette: String::new() };
    let n = samples.len();
    for (k, (base, tip, alpha)) in samples.iter().enumerate() {
        let u = k as f32 / (n - 1) as f32;
        let c = [color[0], color[1], color[2], color[3] * alpha];
        b.verts.push(FxVertex { pos: *base, uv: [u, 1.0], color: c, additive: 1.0, mode: 0.0, grey: [1.0, 1.0, 1.0] });
        b.verts.push(FxVertex { pos: *tip, uv: [u, 0.0], color: c, additive: 1.0, mode: 0.0, grey: [1.0, 1.0, 1.0] });
    }
    for k in 0..n as u32 - 1 {
        let i = k * 2;
        b.indices.extend_from_slice(&[i, i + 1, i + 3, i, i + 3, i + 2]);
    }
    out.push(b);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trail_toml_subset() {
        let text = r#"
[[AttackDefinitions]]
    X = 1
[[TrailDefinition]]
    [TrailDefinition.Conditions]
        EffectKeywords = ["MagicDamageFire"]
        EffectShaders = [ { FormID = 0x4E810, Plugin = "Skyrim.esm" } ]  #EnchFireFXShader
    [TrailDefinition.Visuals]
        LifetimeMult = 1.5
        BaseColorOverride = { r = 1, g = 0.35, b = 0.1, a = 1 }
        BaseColorScaleMult = 2.5
        TrailMeshOverride = "Effects/WeaponTrails/AttackTrailMagic.nif"
[[TrailDefinition]]
    Priority = 3
    [TrailDefinition.Conditions]
        WeaponNames = [
            "Dawn",
            "Bane"
        ]
    [TrailDefinition.Visuals]
        LifetimeMult = 1.0
"#;
        let d = parse_trail_defs(text, "t.toml");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].effect_keywords, vec!["MagicDamageFire"]);
        assert_eq!(d[0].effect_shaders, vec![("skyrim.esm".to_string(), 0x4E810)]);
        assert_eq!(d[0].color, Some([1.0, 0.35, 0.1, 1.0]));
        assert_eq!(d[0].mesh.as_deref(), Some("Effects/WeaponTrails/AttackTrailMagic.nif"));
        assert_eq!((d[1].priority, d[1].weapon_names.len()), (3, 2));
        let w = sds_records::fx::WeaponFacts { name: "Dawnbreaker".into(), effect_shader: Some(("Skyrim.esm".into(), 0x4E810)), ..Default::default() };
        let p = pick_trail(&d, &w).unwrap();
        assert_eq!(p.priority, 3);
    }
}
