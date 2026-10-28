//! NIF → graph decompiler (inverse of `compile`).
//!
//! Rebuilds an editable node graph from the particle systems found in a NIF.
//! This is a *semantic* import: service modifiers (age-death, position,
//! bound-update) are implied by the compiler and therefore dropped, and
//! anything the graph cannot express yet is reported as a warning rather
//! than silently discarded. Editing an imported file does not lose the
//! original bytes — the raw import path keeps the source file for that.

use crate::graph;
use crate::graph::{Graph, GraphNode, Link, NodeId, NodeKind, CLAMP_MODES};
use crate::value::Value;
use aether_nif::typed::controllers::{
    BSPSysMultiTargetEmitterCtlr, NiPSysModifierFloatCtlr, NiVisController,
    NiFloatData, NiFloatInterpolator, NiPSysEmitterCtlr, NiPSysModifierActiveCtlr,
    NiPSysUpdateCtlr,
    BSEffectShaderPropertyFloatController, BSEffectShaderPropertyColorController,
    NiPoint3Interpolator, NiPosData,
};
use aether_nif::typed::geometry::NiNode;
use aether_nif::typed::particles::{
    BSPSysInheritVelocityModifier, BSPSysRecycleBoundModifier, BSWindModifier,
    BSPSysLODModifier, BSPSysScaleModifier, BSPSysSimpleColorModifier, BSPSysSubTexModifier,
    EmitterBase, NiPSysAgeDeathModifier, NiPSysBombModifier, NiPSysSpawnModifier,
    NiPSysBoxEmitter, NiPSysCylinderEmitter, NiPSysData, NiPSysDragModifier, NiPSysMeshEmitter,
    NiPSysGravityModifier, NiPSysRotationModifier, NiPSysSphereEmitter, NiParticleSystem,
};
use aether_nif::typed::shaders::{BSEffectShaderProperty, NiAlphaProperty};
use aether_nif::typed::{BlockRef, Ctx, TypedBlock};
use aether_nif::NifFile;

/// Result of a decompilation: the graph plus anything that didn't fit in it.
#[derive(Debug, Default)]
pub struct Decompiled {
    pub graph: Graph,
    /// Human-readable notes about skipped/unsupported content.
    pub warnings: Vec<String>,
    /// Which node parameters an animation CLIP drives, keyed by the
    /// source block of the node that owns them.
    ///
    /// A sequence-managed controller imports with an EMPTY key track on
    /// purpose — its keys live in the clip, and a free-running copy
    /// would fight the player. Measured over the corpus that is 2,178
    /// of the 3,135 empty `Birth Rate Keys` fields, and until this map
    /// existed the node showed a blank control and said nothing about
    /// why, which is indistinguishable from "nothing animates here".
    ///
    /// Built HERE because this is where both sides are already known: a
    /// clip binds to a controller by NODE NAME plus controller type and
    /// id (its own controller copies carry a NULL target, so block
    /// indices cannot bridge the two), and resolving those names is
    /// what the walk below does anyway.
    pub driven: Vec<((u32, &'static str), Vec<crate::sequence::DrivenBy>)>,
    /// The file's clips, read once for `driven`.
    seqs: crate::sequence::SequenceSet,
    /// The source block being decoded right now.
    ///
    /// Provenance is recorded through a cursor rather than an extra
    /// argument because there are forty insertion sites and only eight
    /// places where the walk moves to a new block. An argument at each
    /// site is forty chances to pass the wrong one.
    src: Option<u32>,
}

impl Decompiled {
    /// Decode block `index` from here on: every node inserted until the
    /// next call records it as its provenance.
    fn from_block(&mut self, index: usize) {
        self.src = Some(index as u32);
    }

    /// Insert a node, stamped with the block currently being decoded.
    fn insert(&mut self, mut node: GraphNode) -> NodeId {
        node.source_block = self.src;
        self.graph.insert(node)
    }

    /// Record that a clip drives `param` of the node being decoded.
    ///
    /// Through the same provenance cursor as `insert`, for the same
    /// reason: the alternative is passing the block index to every
    /// call site and getting it wrong at one of them.
    fn drives(&mut self, param: &'static str, want: impl Fn(&crate::sequence::TrackTarget) -> bool) {
        let Some(src) = self.src else { return };
        let clips = crate::sequence::clips_driving(&self.seqs, want);
        if !clips.is_empty() {
            self.driven.push(((src, param), clips));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecompileError {
    /// Not a Skyrim SE/LE file.
    UnsupportedVersion,
    /// The file contains no particle systems at all.
    NoParticleSystems,
    /// A block failed to decode; the file is likely damaged.
    BadBlock { index: usize, what: String },
}

impl std::fmt::Display for DecompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecompileError::UnsupportedVersion => {
                write!(f, "only Skyrim SE (bs100) and LE (bs83) files can be decompiled")
            }
            DecompileError::NoParticleSystems => {
                write!(f, "the file contains no particle systems")
            }
            DecompileError::BadBlock { index, what } => {
                write!(f, "block {index} failed to decode: {what}")
            }
        }
    }
}

/// Node layout constants: columns per role, vertical lane per system.
const COL_SOURCES: f32 = 0.0;
const COL_MODIFIERS: f32 = 260.0;
const COL_SYSTEM: f32 = 560.0;
const COL_OUTPUT: f32 = 860.0;
const LANE_HEIGHT: f32 = 420.0;
const ROW_HEIGHT: f32 = 130.0;

pub fn decompile(nif: &NifFile) -> Result<Decompiled, DecompileError> {
    let ctx = Ctx::of(&nif.header).map_err(|_| DecompileError::UnsupportedVersion)?;
    let mut out = Decompiled {
        // Read once, up front: `drives` consults it at every controller
        // the walk turns into a node.
        seqs: crate::sequence::sequences_from_nif(nif),
        ..Decompiled::default()
    };

    let system_indices: Vec<usize> = nif
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            NiParticleSystem::TYPE_NAMES.contains(&b.type_name.as_str())
        })
        .map(|(i, _)| i)
        .collect();
    if system_indices.is_empty() {
        return Err(DecompileError::NoParticleSystems);
    }
    // A mesh-particle system is deliberately NOT claimed as editable
    // (its recompile cannot be faithful — the sweep measured the two
    // files that regressed when it was); it passes through as source
    // bytes. Passing through in SILENCE broke the skipped-content-
    // warns rule.
    for (i, b) in nif.blocks.iter().enumerate() {
        if b.type_name == "NiMeshParticleSystem" {
            out.warnings.push(format!(
                "block {i} (NiMeshParticleSystem) is view-only: kept as source bytes, not editable"
            ));
        }
    }

    // Scene output carries the root node's name back into the graph.
    let root_idx = nif
        .roots
        .first()
        .and_then(|r| (*r >= 0).then_some(*r as usize))
        .filter(|i| *i < nif.blocks.len());
    let root_node = root_idx
        .and_then(|i| nif.blocks.get(i))
        .filter(|b| NiNode::TYPE_NAMES.contains(&b.type_name.as_str()))
        .and_then(|b| NiNode::decode(b, ctx).ok());
    let root_name = root_node
        .as_ref()
        .and_then(|n| n.av.net.name.resolve(&nif.header).map(str::to_string))
        .unwrap_or_else(|| "AetherFX".to_string());
    let behavior = root_node
        .as_ref()
        .and_then(|n| crate::compile::root_behavior(nif, ctx, &n.av.net.extra_data))
        .map(|(_, path)| path)
        .unwrap_or_default();
    let mut output_node = GraphNode::new(NodeKind::SceneOutput, [COL_OUTPUT, 0.0]);
    output_node.set_param("Root Name", Value::Str(root_name));
    output_node.set_param("Behavior Graph", Value::Str(behavior));
    if let Some(i) = root_idx {
        out.from_block(i);
    }
    let output_id = out.insert(output_node);

    for (lane, &psys_idx) in system_indices.iter().enumerate() {
        decompile_system(nif, ctx, psys_idx, lane, output_id, &mut out)?;
    }
    out.graph.clips = document_clips(&out);
    // The per-lane seeds above only rough in the clusters; the real
    // positions come from the size-aware arrangement.
    crate::layout::arrange(&mut out.graph);
    Ok(out)
}

/// The file's clips as the document holds them: every clip, in file
/// order, with the tracks that land on a parameter some node models.
///
/// Built from `driven`, the binding the walk has already resolved -- a
/// clip names a controller by NODE NAME, and the walk is where those
/// names became nodes -- so this is a regrouping of that map by clip,
/// not a second implementation of the format's binding.
fn document_clips(out: &Decompiled) -> Vec<crate::clip::Clip> {
    use crate::clip::{Clip, ClipTrack};
    out.seqs
        .clips
        .iter()
        .enumerate()
        .map(|(ci, c)| {
            let mut tracks: Vec<ClipTrack> = Vec::new();
            for ((src, param), drivers) in &out.driven {
                // The node that came from `src` and carries the param.
                let Some(node) = out
                    .graph
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.source_block == Some(*src) && n.param(param).is_some())
                    .map(|(id, _)| *id)
                    .min()
                else {
                    continue;
                };
                for d in drivers.iter().filter(|d| d.clip_index == ci) {
                    // One slot, one track: two nodes on one block would
                    // otherwise both claim the same keys.
                    if tracks.iter().any(|t| t.source_slot == Some(d.slot as u32)) {
                        continue;
                    }
                    tracks.push(ClipTrack {
                        node,
                        param,
                        curve: d.curve.clone(),
                        source_slot: Some(d.slot as u32),
                    });
                }
            }
            tracks.sort_by_key(|t| t.source_slot);
            Clip {
                name: c.name.clone(),
                looping: c.looping,
                start_time: c.start_time,
                stop_time: c.stop_time,
                frequency: c.frequency,
                text_keys: c.text_keys.clone(),
                tracks,
                source_block: c.source_block.map(|b| b as u32),
            }
        })
        .collect()
}

fn decode<T: TypedBlock>(nif: &NifFile, ctx: Ctx, index: usize) -> Result<T, DecompileError> {
    T::decode(&nif.blocks[index], ctx).map_err(|e| DecompileError::BadBlock {
        index,
        what: format!("{e:?}"),
    })
}

fn decompile_system(
    nif: &NifFile,
    ctx: Ctx,
    psys_idx: usize,
    lane: usize,
    output_id: NodeId,
    out: &mut Decompiled,
) -> Result<(), DecompileError> {
    let header = &nif.header;
    let psys: NiParticleSystem = decode(nif, ctx, psys_idx)?;
    let base_y = lane as f32 * LANE_HEIGHT;
    let mut row = 0;
    let mut next_pos = |col: f32| {
        let pos = [col, base_y + row as f32 * ROW_HEIGHT];
        row += 1;
        pos
    };

    let sys_name = psys
        .av
        .net
        .name
        .resolve(header)
        .unwrap_or("ParticleFX")
        .to_string();
    out.from_block(psys_idx);
    let mut sys_node = GraphNode::new(NodeKind::ParticleSystem, [COL_SYSTEM, base_y]);
    sys_node.set_param("Name", Value::Str(sys_name.clone()));
    sys_node.set_param("World Space", Value::Bool(psys.world_space));
    // The game's world transform for this system (world-space keeps ONLY
    // the accumulated scale). Anchors below are brought into psys space
    // through its inverse; the preview multiplies the render back by it.
    let game_world = psys_game_world(nif, ctx, psys_idx, &psys.av, psys.world_space);
    sys_node.set_param("World Scale", Value::F32(game_world.s));
    // The REST of the world transform. For a world-space system this is
    // zero/identity by construction (the game zeroes it); for a local
    // system the game renders particles under the node's FULL matrix —
    // dropping it is how the scepter's swirl ended up at the origin
    // while its world-space glow sat correctly at (0, 50, 0).
    sys_node.set_param("World Offset", Value::Vec3(game_world.t));
    sys_node.set_param(
        "World Rotation",
        Value::Vec3(crate::mesh::Transform::euler_from_rows(&game_world.rot)),
    );
    let psys_inv = game_world.inverse();
    let sys_id = out.insert(sys_node);
    wire(out, sys_id, 0, output_id, 0);

    // --- data ------------------------------------------------------------
    let mut data_node_id = None;
    if let Some(data_idx) = psys.data.get() {
        out.from_block(data_idx);
        // Strip systems (weapon trails) carry BSStripPSysData — NiPSysData
        // plus the ribbon tail. Both import into the same node.
        let mut strip = None;
        let data: NiPSysData = if nif.blocks[data_idx].type_name == "BSStripPSysData" {
            let s: aether_nif::typed::particles::BSStripPSysData = decode(nif, ctx, data_idx)?;
            strip = Some(s.clone());
            s.data
        } else {
            decode(nif, ctx, data_idx)?
        };
        let mut node = GraphNode::new(NodeKind::ParticleData, next_pos(COL_SOURCES));
        if let Some(s) = strip {
            node.set_param("Strip Trails", Value::Bool(true));
            node.set_param("Max Point Count", Value::U32(s.max_point_count as u32));
            node.set_param("Start Cap Size", Value::F32(s.start_cap_size));
            node.set_param("End Cap Size", Value::F32(s.end_cap_size));
            node.set_param("Z Prepass", Value::Bool(s.do_z_prepass));
        }
        node.set_param("Max Particles", Value::U32(data.num_vertices as u32));
        node.set_param("Has Color", Value::Bool(data.has_vertex_colors));
        node.set_param("Has Rotations", Value::Bool(data.has_rotation_angles));
        let (rows, cols) = atlas_from_offsets(&data.subtexture_offsets);
        node.set_param("Subtexture Rows", Value::U32(rows));
        node.set_param("Subtexture Cols", Value::U32(cols));
        // A table the rows x cols pair cannot re-derive is carried
        // verbatim, or the export shrinks the block and the merge
        // refuses it. The common case (a real grid, or no table) keeps
        // the param empty so existing saved graphs do not change.
        if data.subtexture_offsets
            != crate::compile::derived_atlas_offsets(rows, cols)
        {
            let text = data
                .subtexture_offsets
                .iter()
                .map(|q| format!("{} {} {} {}", q[0], q[1], q[2], q[3]))
                .collect::<Vec<_>>()
                .join("; ");
            node.set_param("Atlas Offsets", Value::Str(text));
        }
        node.set_param("Aspect Ratio", Value::F32(data.aspect_ratio));
        node.set_param("Velocity Aspect", Value::Bool(data.aspect_flags & 1 != 0));
        node.set_param("Rotate To Velocity", Value::Bool(data.aspect_flags & 2 != 0));
        node.set_param("Speed To Aspect", Value::Bool(data.aspect_flags & (1 << 8) != 0));
        node.set_param("Aspect 2", Value::F32(data.speed_to_aspect_aspect2));
        node.set_param("Aspect Speed 1", Value::F32(data.speed_to_aspect_speed1));
        node.set_param("Aspect Speed 2", Value::F32(data.speed_to_aspect_speed2));
        let id = out.insert(node);
        data_node_id = Some(id);
        wire(out, id, 0, sys_id, 0);
    }

    // --- shader / alpha ---------------------------------------------------
    if let Some(idx) = psys.shader_property.get() {
        out.from_block(idx);
        if nif.blocks[idx].type_name == "BSEffectShaderProperty" {
            let shader: BSEffectShaderProperty = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::EffectShader, next_pos(COL_SOURCES));
            node.set_param("Source Texture", Value::TexturePath(shader.source_texture));
            node.set_param("Base Color", Value::Color4(shader.base_color));
            node.set_param("Base Color Scale", Value::F32(shader.base_color_scale));
            node.set_param("Soft Falloff Depth", Value::F32(shader.soft_falloff_depth));
            // The shader's own controller chain. Only FREE tracks (a
            // plain interpolator with key data) become editable text;
            // a blend interpolator's keys belong to whichever sequence
            // owns it, and the compiler re-derives those controllers
            // from the source, like every other sequence fixture.
            {
                let mut cur = shader.net.controller;
                for _ in 0..16 {
                    let Some(ci) = cur.get() else { break };
                    match nif.blocks[ci].type_name.as_str() {
                        "BSEffectShaderPropertyFloatController" => {
                            let c: BSEffectShaderPropertyFloatController =
                                decode(nif, ctx, ci)?;
                            let keys = float_keys_of(nif, ctx, c.interpolator);
                            if !keys.is_empty() {
                                // Through the one table; a variable it
                                // does not model is carried through by
                                // the compiler verbatim rather than
                                // half-shown.
                                let param = crate::graph::EFFECT_SHADER_FLOAT_KEYS
                                    .iter()
                                    .find(|(_, v)| *v == c.controlled_variable)
                                    .map(|(p, _)| *p);
                                if let Some(param) = param {
                                    node.set_param(param, Value::Curve(keys));
                                }
                            }
                            cur = c.base.next_controller;
                        }
                        "BSEffectShaderPropertyColorController" => {
                            let c: BSEffectShaderPropertyColorController =
                                decode(nif, ctx, ci)?;
                            if let Some(ii) = c.interpolator.get() {
                                if nif.blocks[ii].type_name == "NiPoint3Interpolator" {
                                    let pi: NiPoint3Interpolator = decode(nif, ctx, ii)?;
                                    if let Some(di) = pi.data.get() {
                                        let pd: NiPosData = decode(nif, ctx, di)?;
                                        let curve = crate::curve::Curve::from_pos_data(
                                            pd.interpolation,
                                            &pd.floats,
                                        );
                                        if !curve.is_empty() {
                                            node.set_param("Color Keys", Value::Curve(curve));
                                        }
                                    }
                                }
                            }
                            cur = c.base.next_controller;
                        }
                        _ => break,
                    }
                }
            }
            // The SLSF1 gate is independent of the depth: 1,169 corpus
            // blocks carry a depth with the bit CLEAR.
            node.set_param(
                "Soft Effect",
                Value::Bool(shader.shader_flags1 & 0x4000_0000 != 0),
            );
            node.set_param("Falloff Start Angle", Value::F32(shader.falloff_start_angle));
            node.set_param("Falloff Stop Angle", Value::F32(shader.falloff_stop_angle));
            node.set_param(
                "Falloff Start Opacity",
                Value::F32(shader.falloff_start_opacity),
            );
            node.set_param(
                "Falloff Stop Opacity",
                Value::F32(shader.falloff_stop_opacity),
            );
            node.set_param("Use Falloff", Value::Bool(shader.shader_flags1 & 0x40 != 0));
            node.set_param(
                "Greyscale Texture",
                Value::TexturePath(shader.greyscale_texture.clone()),
            );
            node.set_param(
                "Greyscale Color",
                Value::Bool(shader.shader_flags1 & 0x10 != 0),
            );
            node.set_param(
                "Greyscale Alpha",
                Value::Bool(shader.shader_flags1 & 0x20 != 0),
            );
            node.set_param("UV Offset U", Value::F32(shader.uv_offset[0]));
            node.set_param("UV Offset V", Value::F32(shader.uv_offset[1]));
            node.set_param("UV Scale U", Value::F32(shader.uv_scale[0]));
            node.set_param("UV Scale V", Value::F32(shader.uv_scale[1]));
            node.set_param(
                "Double Sided",
                Value::Bool(shader.shader_flags2 & 0x10 != 0),
            );
            node.set_param(
                "Lighting Influence",
                Value::U32(shader.lighting_influence as u32),
            );
            // Clamp byte 3/0/1/2 -> CLAMP_MODES index 0/1/2/3.
            let clamp_idx = match shader.texture_clamp_mode {
                3 => 0,
                0 => 1,
                1 => 2,
                _ => 3,
            };
            node.set_param("Texture Clamp", Value::Enum(CLAMP_MODES, clamp_idx));
            let id = out.insert(node);
            {
                // A shader controller's controlled block names the SCENE
                // OBJECT the property hangs off, which for a particle
                // shader is the system -- so the same name matches here
                // as in the controller chain below.
                use crate::sequence::TrackTarget as T;
                for &(param, variable) in crate::graph::EFFECT_SHADER_FLOAT_KEYS {
                    out.drives(param, |t| {
                        matches!(t, T::EffectShaderFloat { node, variable: v }
                            if *node == sys_name && *v == variable)
                    });
                }
                out.drives("Color Keys", |t| {
                    matches!(t, T::EffectShaderColor { node } if *node == sys_name)
                });
            }
            wire(out, id, 0, sys_id, 1);
        } else {
            out.warnings.push(format!(
                "{sys_name}: shader block {} ({}) is not a BSEffectShaderProperty; skipped",
                idx, nif.blocks[idx].type_name
            ));
        }
    }
    if let Some(idx) = psys.alpha_property.get() {
        out.from_block(idx);
        let alpha: NiAlphaProperty = decode(nif, ctx, idx)?;
        let mut node = GraphNode::new(NodeKind::AlphaProperty, next_pos(COL_SOURCES));
        // Every bit of the word is a parameter, so this is a total
        // decode with nothing left to warn about: a blend half that is
        // not one of the three preset words simply opens on `Custom`
        // with its own factors, which is what the node is for. It used
        // to warn and fall back to Alpha Blend, i.e. it announced a
        // value it was about to lose.
        graph::set_alpha_flags(&mut node, alpha.flags);
        node.set_param("Test Threshold", Value::U32(alpha.threshold as u32));
        let id = out.insert(node);
        wire(out, id, 0, sys_id, 2);
    }

    // --- modifiers --------------------------------------------------------
    // NIF stack name -> node id, for controller re-wiring (emitter
    // controllers and active toggles both target modifiers by name).
    let mut modifiers_by_name: Vec<(String, NodeId)> = Vec::new();
    let mut mod_row = 0;
    for mref in &psys.modifiers {
        let Some(idx) = mref.get() else { continue };
        let type_name = nif.blocks[idx].type_name.clone();
        // The strip updater is part of the strip apparatus, not a node:
        // its one field lives on the Particle Data node with the other
        // strip params, and the compiler re-emits the modifier from there.
        if type_name == "BSPSysStripUpdateModifier" {
            let m: aether_nif::typed::particles::BSPSysStripUpdateModifier =
                decode(nif, ctx, idx)?;
            if let Some(data_id) = data_node_id {
                out.graph
                    .nodes
                    .get_mut(&data_id)
                    .expect("data node just inserted")
                    .set_param("Update Delta Time", Value::F32(m.update_delta_time));
            }
            continue;
        }
        let pos = [COL_MODIFIERS, base_y + mod_row as f32 * ROW_HEIGHT];
        let made = decompile_modifier(nif, ctx, idx, &type_name, pos, &psys_inv, out)?;
        match made {
            ModifierOutcome::Node(id) => {
                mod_row += 1;
                wire(out, id, 0, sys_id, 3);
                // Every NiPSysModifier serializes its name first.
                if let Some(name) = modifier_name_of(nif, idx) {
                    modifiers_by_name.push((name, id));
                }
            }
            ModifierOutcome::Chain(ids) => {
                for (i, id) in ids.iter().enumerate() {
                    mod_row += 1;
                    wire(out, *id, 0, sys_id, 3);
                    if i == 0 {
                        if let Some(name) = modifier_name_of(nif, idx) {
                            modifiers_by_name.push((name, *id));
                        }
                    }
                }
            }
            ModifierOutcome::Service => {}
            ModifierOutcome::Unsupported => {
                out.warnings.push(format!(
                    "{sys_name}: modifier {type_name} (block {idx}) is not editable yet; skipped"
                ));
            }
        }
    }

    // --- controller chain -------------------------------------------------
    let mut ctlr_ref = psys.av.net.controller;
    let mut ctlr_row = 0;
    let mut seen = std::collections::HashSet::new();
    while let Some(idx) = ctlr_ref.get() {
        if !seen.insert(idx) {
            out.warnings
                .push(format!("{sys_name}: controller chain loops at block {idx}"));
            break;
        }
        out.from_block(idx);
        let type_name = nif.blocks[idx].type_name.clone();
        // A clip binds to this controller by the SYSTEM's name plus the
        // controller's own kind, which is exactly what is in scope
        // here. `modifier_name_of` is not: the controllers that bind to
        // one carry it as their own field and read it in their arm.
        let drives_system = |t: &crate::sequence::TrackTarget| -> bool {
            use crate::sequence::TrackTarget as T;
            matches!(t,
                T::EmitterBirthRate { system }
                | T::EmitterActive { system }
                | T::EmitterSpeed { system }
                | T::EmitterParam { system, .. }
                | T::ModifierActive { system, .. }
                | T::Visibility { node: system }
                if *system == sys_name)
        };
        let pos = [COL_MODIFIERS, base_y + 300.0 + ctlr_row as f32 * ROW_HEIGHT];
        ctlr_ref = match type_name.as_str() {
            "NiPSysEmitterCtlr" => {
                let ec: NiPSysEmitterCtlr = decode(nif, ctx, idx)?;
                let mut node = GraphNode::new(NodeKind::EmitterCtlr, pos);
                if let Some(interp_idx) = ec.interpolator.get() {
                    match nif.blocks[interp_idx].type_name.as_str() {
                        "NiFloatInterpolator" => {
                            let fi: NiFloatInterpolator = decode(nif, ctx, interp_idx)?;
                            let mut rate = fi.value;
                            if let Some(data_idx) = fi.data.get() {
                                // Keyed birth rate: keep the whole track as
                                // editable text. The pose value is typically
                                // 0 for keyed emitters, so also seed "Birth
                                // Rate" with the average — it is what plays
                                // when the key text is cleared.
                                let fd: NiFloatData = decode(nif, ctx, data_idx)?;
                                // The WHOLE curve, key type and tangents
                                // included. Flattening to (t, v) pairs
                                // re-emitted the 64,915 quadratic tracks
                                // the corpus ships as LINEAR, and the
                                // field-count mismatch made the merge
                                // refuse the block whole.
                                let curve = crate::curve::Curve::from_float_data(&fd);
                                if !curve.is_empty() {
                                    rate = curve.keys.iter().map(|k| k.value[0]).sum::<f32>()
                                        / curve.keys.len() as f32;
                                    node.set_param("Birth Rate Keys", Value::Curve(curve));
                                }
                            }
                            node.set_param("Birth Rate", Value::F32(rate));
                        }
                        "NiBlendFloatInterpolator" => {
                            // Sequence-driven birth rate: the blend value is
                            // the resting pose, and the -FLT_MAX sentinel
                            // means "no pose" — the game emits nothing until
                            // a sequence writes the rate. Import that idle
                            // state; sequence playback animates it live.
                            use aether_nif::typed::controllers::NiBlendFloatInterpolator;
                            let bi: NiBlendFloatInterpolator = decode(nif, ctx, interp_idx)?;
                            let rate = if bi.value < -3.0e38 { 0.0 } else { bi.value };
                            node.set_param("Birth Rate", Value::F32(rate));
                        }
                        _ => {}
                    }
                }
                let active = bool_track_of(nif, ctx, ec.visibility_interpolator);
                if !active.is_empty() {
                    node.set_param("Emitter Active Keys", Value::Curve(active));
                } else if let Some(vi) = ec.visibility_interpolator.get() {
                    // Sequence-driven visibility: a blend interpolator whose
                    // resting value is not an explicit "on" keeps the
                    // emitter quiet until a sequence turns it on (value 2
                    // is the "no pose" sentinel).
                    if nif.blocks[vi].type_name == "NiBlendBoolInterpolator" {
                        use aether_nif::typed::controllers::NiBlendBoolInterpolator;
                        let bb: NiBlendBoolInterpolator = decode(nif, ctx, vi)?;
                        if bb.value != 1 {
                            node.set_param(
                                "Emitter Active Keys",
                                Value::Curve(crate::curve::Curve::constant(
                                    crate::curve::Track::Bool,
                                    0.0,
                                )),
                            );
                        }
                    }
                }
                node.set_param("Start Time", Value::F32(ec.base.start_time));
                node.set_param("Stop Time", Value::F32(ec.base.stop_time));
                // Cycle bits (1-2): 0 = loop, 4 = clamp.
                node.set_param("Loop", Value::Bool(ec.base.flags & 6 == 0));
                let id = out.insert(node);
                {
                    use crate::sequence::TrackTarget as T;
                    out.drives("Birth Rate Keys", |t| {
                        drives_system(t) && matches!(t, T::EmitterBirthRate { .. })
                    });
                    out.drives("Emitter Active Keys", |t| {
                        drives_system(t) && matches!(t, T::EmitterActive { .. })
                    });
                }
                ctlr_row += 1;
                wire(out, id, 0, sys_id, 4);
                let target_name = ec.modifier_name.resolve(header).unwrap_or_default();
                if let Some((_, emitter_id)) = modifiers_by_name
                    .iter()
                    .find(|(name, _)| name == target_name)
                {
                    wire(out, *emitter_id, 0, id, 0);
                } else {
                    out.warnings.push(format!(
                        "{sys_name}: emitter controller names '{target_name}' \
                         but no such emitter was imported"
                    ));
                }
                ec.base.next_controller
            }
            "NiPSysModifierActiveCtlr" => {
                let ac: NiPSysModifierActiveCtlr = decode(nif, ctx, idx)?;
                let mut node = GraphNode::new(NodeKind::ModifierActiveCtlr, pos);
                node.set_param(
                    "Keys",
                    Value::Curve(bool_track_of(nif, ctx, ac.interpolator)),
                );
                node.set_param("Start Time", Value::F32(ac.base.start_time));
                node.set_param("Stop Time", Value::F32(ac.base.stop_time));
                node.set_param("Loop", Value::Bool(ac.base.flags & 6 == 0));
                let id = out.insert(node);
                let target_name = ac.modifier_name.resolve(header).unwrap_or_default();
                {
                    use crate::sequence::TrackTarget as T;
                    // The MODIFIER name too: a system can carry several
                    // active toggles, and they differ only by which
                    // modifier they switch.
                    out.drives("Keys", |t| {
                        drives_system(t)
                            && matches!(t, T::ModifierActive { modifier, .. }
                                if modifier == target_name)
                    });
                }
                ctlr_row += 1;
                wire(out, id, 0, sys_id, 4);
                if let Some((_, target_id)) = modifiers_by_name
                    .iter()
                    .find(|(name, _)| name == target_name)
                {
                    wire(out, *target_id, 0, id, 0);
                } else {
                    out.warnings.push(format!(
                        "{sys_name}: active toggle names '{target_name}' \
                         but no such modifier was imported"
                    ));
                }
                ac.base.next_controller
            }
            name if crate::graph::PARAM_CTLR_BLOCKS.contains(&name) => {
                let pc: NiPSysModifierFloatCtlr = decode(nif, ctx, idx)?;
                let mut node = GraphNode::new(NodeKind::ParamCtlr, pos);
                let param = crate::graph::PARAM_CTLR_BLOCKS
                    .iter()
                    .position(|n| *n == name)
                    .unwrap_or(0);
                node.set_param(
                    "Parameter",
                    Value::Enum(crate::graph::PARAM_TARGETS, param),
                );
                node.set_param(
                    "Keys",
                    Value::Curve(float_keys_of(nif, ctx, pc.interpolator)),
                );
                node.set_param("Start Time", Value::F32(pc.base.start_time));
                node.set_param("Stop Time", Value::F32(pc.base.stop_time));
                node.set_param("Loop", Value::Bool(pc.base.flags & 6 == 0));
                let id = out.insert(node);
                let target_name = pc.modifier_name.resolve(header).unwrap_or_default();
                {
                    use crate::sequence::TrackTarget as T;
                    // `Speed` has a block type of its own and so a
                    // TrackTarget of its own; the other eleven share
                    // `EmitterParam` and are told apart by the index.
                    out.drives("Keys", |t| {
                        drives_system(t)
                            && match t {
                                T::EmitterSpeed { .. } => param == 0,
                                T::EmitterParam {
                                    modifier,
                                    param: driven,
                                    ..
                                } => *driven == param && *modifier == target_name,
                                _ => false,
                            }
                    });
                }
                ctlr_row += 1;
                wire(out, id, 0, sys_id, 4);
                if let Some((_, target_id)) = modifiers_by_name
                    .iter()
                    .find(|(name, _)| name == target_name)
                {
                    wire(out, *target_id, 0, id, 0);
                } else {
                    out.warnings.push(format!(
                        "{sys_name}: param track names '{target_name}' \
                         but no such modifier was imported"
                    ));
                }
                pc.base.next_controller
            }
            "NiVisController" => {
                let vc: NiVisController = decode(nif, ctx, idx)?;
                let mut node = GraphNode::new(NodeKind::VisibilityCtlr, pos);
                node.set_param(
                    "Keys",
                    Value::Curve(bool_track_of(nif, ctx, vc.interpolator)),
                );
                node.set_param("Start Time", Value::F32(vc.base.start_time));
                node.set_param("Stop Time", Value::F32(vc.base.stop_time));
                node.set_param("Loop", Value::Bool(vc.base.flags & 6 == 0));
                let id = out.insert(node);
                {
                    use crate::sequence::TrackTarget as T;
                    out.drives("Keys", |t| {
                        drives_system(t) && matches!(t, T::Visibility { .. })
                    });
                }
                ctlr_row += 1;
                wire(out, id, 0, sys_id, 4);
                vc.base.next_controller
            }
            "BSPSysMultiTargetEmitterCtlr" => {
                let mc: BSPSysMultiTargetEmitterCtlr = decode(nif, ctx, idx)?;
                let mut node = GraphNode::new(NodeKind::MultiEmitterCtlr, pos);
                let keys = float_keys_of(nif, ctx, mc.interpolator);
                if let Some(first) = keys.keys.first() {
                    node.set_param("Birth Rate", Value::F32(first.value[0]));
                } else if let Some(i) = mc.interpolator.get() {
                    // No keys: the interpolator's pose IS the constant
                    // rate. Leaving the schema default here made the
                    // soul-rend trail import at 20/s where the file
                    // says 60 -- and export wrote the 20 back.
                    if let Ok(fi) = NiFloatInterpolator::decode(&nif.blocks[i], ctx) {
                        if fi.value.is_finite() {
                            node.set_param("Birth Rate", Value::F32(fi.value));
                        }
                    }
                } else if let Some(i) = mc.interpolator.get() {
                    // No keys: the interpolator's pose IS the constant
                    // rate. Leaving the schema default here made the
                    // soul-rend trail import at 20/s where the file
                    // says 60 -- and export wrote the 20 back.
                    if let Ok(fi) = NiFloatInterpolator::decode(&nif.blocks[i], ctx) {
                        if fi.value.is_finite() {
                            node.set_param("Birth Rate", Value::F32(fi.value));
                        }
                    }
                } else if let Some(i) = mc.interpolator.get() {
                    // No keys: the interpolator's pose IS the constant
                    // rate. Leaving the schema default here made the
                    // soul-rend trail import at 20/s where the file
                    // says 60 -- and export wrote the 20 back.
                    if let Ok(fi) = NiFloatInterpolator::decode(&nif.blocks[i], ctx) {
                        if fi.value.is_finite() {
                            node.set_param("Birth Rate", Value::F32(fi.value));
                        }
                    }
                }
                node.set_param("Birth Rate Keys", Value::Curve(keys));
                node.set_param("Max Emitters", Value::U32(mc.max_emitters as u32));
                node.set_param("Start Time", Value::F32(mc.base.start_time));
                node.set_param("Stop Time", Value::F32(mc.base.stop_time));
                node.set_param("Loop", Value::Bool(mc.base.flags & 6 == 0));
                let id = out.insert(node);
                ctlr_row += 1;
                wire(out, id, 0, sys_id, 4);
                let target_name = mc.modifier_name.resolve(header).unwrap_or_default();
                if let Some((_, emitter_id)) = modifiers_by_name
                    .iter()
                    .find(|(name, _)| name == target_name)
                {
                    wire(out, *emitter_id, 0, id, 0);
                } else {
                    out.warnings.push(format!(
                        "{sys_name}: multi emitter names '{target_name}' \
                         but no such emitter was imported"
                    ));
                }
                mc.base.next_controller
            }
            "NiPSysUpdateCtlr" => {
                let uc: NiPSysUpdateCtlr = decode(nif, ctx, idx)?;
                let id = out.insert(GraphNode::new(NodeKind::UpdateCtlr, pos));
                ctlr_row += 1;
                wire(out, id, 0, sys_id, 4);
                uc.base.next_controller
            }
            other => {
                out.warnings.push(format!(
                    "{sys_name}: controller {other} (block {idx}) is not editable yet; skipped"
                ));
                // Every NiTimeController serializes Next Controller as its
                // first field at 20.2.0.7, so the walk can continue past a
                // controller we cannot edit. Stopping here is what made the
                // giant campfire import with birth rate 0: its emitter
                // controllers sit BEHIND NiPSysModifierActiveCtlrs.
                nif.blocks[idx]
                    .data
                    .get(0..4)
                    .map(|b| BlockRef(i32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                    .unwrap_or(BlockRef::NONE)
            }
        };
    }
    Ok(())
}

enum ModifierOutcome {
    /// An editable node was created (its NIF stack name is read back via
    /// `modifier_name_of` for controller re-wiring).
    Node(NodeId),
    /// One manager expanded into several nodes (a collider CHAIN). The
    /// FIRST carries the manager's stack name for controller re-wiring;
    /// re-compiling emits one manager per node, which the game treats
    /// the same — it iterates managers in the stack exactly as it
    /// iterates colliders within one.
    Chain(Vec<NodeId>),
    /// Implied by the compiler; intentionally not shown in the graph.
    Service,
    Unsupported,
}

/// The world transform the GAME gives a particle system's geometry
/// (1.5.97 @ 0x140D039B0): world-space systems keep identity rotation and
/// zero translation but the SCALE product still accumulates down the
/// parent chain; object-space systems keep the full authored transform.
/// Shared with the compiler's axis unbake — the two must agree
/// bit-for-bit or the untouched-axis detection breaks.
pub(crate) fn psys_game_world(
    nif: &NifFile,
    ctx: Ctx,
    psys_idx: usize,
    psys_av: &aether_nif::typed::AvObject,
    world_space: bool,
) -> crate::mesh::Transform {
    use crate::mesh::Transform;
    let parent = parent_world_of(nif, ctx, psys_idx).unwrap_or(Transform::IDENTITY);
    let full = parent.compose(&Transform {
        rot: psys_av.rotation,
        t: psys_av.translation,
        s: psys_av.scale,
    });
    if world_space {
        Transform {
            rot: Transform::IDENTITY.rot,
            t: [0.0; 3],
            s: full.s,
        }
    } else {
        full
    }
}

/// World transform of the scene NODE holding `child_idx` in its children.
fn parent_world_of(nif: &NifFile, ctx: Ctx, child_idx: usize) -> Option<crate::mesh::Transform> {
    use crate::mesh::Transform;
    fn walk(
        nif: &NifFile,
        ctx: Ctx,
        index: usize,
        parent: &Transform,
        child_idx: usize,
    ) -> Option<Transform> {
        let block = nif.blocks.get(index)?;
        if !crate::mesh::is_scene_node(&block.type_name) {
            return None;
        }
        let node = crate::mesh::decode_node(block, ctx).ok()?;
        let world = parent.compose(&Transform {
            rot: node.av.rotation,
            t: node.av.translation,
            s: node.av.scale,
        });
        if node.children.iter().any(|c| c.get() == Some(child_idx)) {
            return Some(world);
        }
        node.children
            .iter()
            .filter_map(|c| c.get())
            .find_map(|c| walk(nif, ctx, c, &world, child_idx))
    }
    nif.roots
        .iter()
        .filter(|r| **r >= 0)
        .find_map(|r| walk(nif, ctx, *r as usize, &Transform::IDENTITY, child_idx))
}

/// World transform of a scene-graph block, composed through its parent
/// chain with the same row-matrix semantics `mesh.rs` uses. `None` when
/// the block is not reachable from the roots as a node.
pub(crate) fn world_transform_of(
    nif: &NifFile,
    ctx: Ctx,
    target: usize,
) -> Option<crate::mesh::Transform> {
    use crate::mesh::Transform;
    fn walk(
        nif: &NifFile,
        ctx: Ctx,
        index: usize,
        parent: &Transform,
        target: usize,
    ) -> Option<Transform> {
        let block = nif.blocks.get(index)?;
        if !crate::mesh::is_scene_node(&block.type_name) {
            return None;
        }
        let node = crate::mesh::decode_node(block, ctx).ok()?;
        let world = parent.compose(&Transform {
            rot: node.av.rotation,
            t: node.av.translation,
            s: node.av.scale,
        });
        if index == target {
            return Some(world);
        }
        node.children
            .iter()
            .filter_map(|c| c.get())
            .find_map(|c| walk(nif, ctx, c, &world, target))
    }
    nif.roots
        .iter()
        .filter(|r| **r >= 0)
        .find_map(|r| walk(nif, ctx, *r as usize, &Transform::IDENTITY, target))
}

fn decompile_modifier(
    nif: &NifFile,
    ctx: Ctx,
    idx: usize,
    type_name: &str,
    pos: [f32; 2],
    psys_inv: &crate::mesh::Transform,
    out: &mut Decompiled,
) -> Result<ModifierOutcome, DecompileError> {
    out.from_block(idx);
    // Everything anchored to a scene object is brought into psys space
    // through the system's inverse game-world transform (for world-space
    // systems that divides by the accumulated scale — 1.5.97 spawn rule).
    let rel_of = |obj: BlockRef| -> Option<crate::mesh::Transform> {
        let world = world_transform_of(nif, ctx, obj.get()?)?;
        Some(psys_inv.compose(&world))
    };
    let emitter_meta = |e: &EmitterBase, node: &mut GraphNode| {
        node.set_param("Speed", Value::F32(e.speed));
        node.set_param("Speed Variation", Value::F32(e.speed_variation));
        node.set_param("Declination", Value::F32(e.declination));
        node.set_param("Declination Variation", Value::F32(e.declination_variation));
        node.set_param("Planar Angle", Value::F32(e.planar_angle));
        node.set_param("Planar Angle Variation", Value::F32(e.planar_angle_variation));
        node.set_param("Initial Color", Value::Color4(e.initial_color));
        node.set_param("Initial Radius", Value::F32(e.initial_radius));
        node.set_param("Radius Variation", Value::F32(e.radius_variation));
        node.set_param("Life Span", Value::F32(e.life_span));
        node.set_param("Life Span Variation", Value::F32(e.life_span_variation));
        // The volume is anchored where the emitter-object node sits,
        // expressed in psys space — vanilla places flames via
        // "<name>-Emitter" nodes well above the model origin (the giant
        // campfire's fire: z 117) and orients volumes by rotating the
        // anchor.
        if let Some(rel) = rel_of(e.emitter_object) {
            node.set_param("Emitter Offset", Value::Vec3(rel.t));
            let euler = crate::mesh::Transform::euler_from_rows(&rel.rot);
            // Only kinds that expose the param store it; set_param
            // is a no-op for the others.
            node.set_param("Emitter Rotation", Value::Vec3(euler));
        }
    };

    Ok(match type_name {
        "NiPSysAgeDeathModifier" | "NiPSysPositionModifier" | "NiPSysBoundUpdateModifier" => {
            ModifierOutcome::Service
        }
        "NiPSysBoxEmitter" => {
            let m: NiPSysBoxEmitter = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::BoxEmitter, pos);
            emitter_meta(&m.emitter, &mut node);
            node.set_param("Width", Value::F32(m.width));
            node.set_param("Height", Value::F32(m.height));
            node.set_param("Depth", Value::F32(m.depth));
            ModifierOutcome::Node(out.insert(node))
        }
        "NiPSysSphereEmitter" => {
            let m: NiPSysSphereEmitter = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::SphereEmitter, pos);
            emitter_meta(&m.emitter, &mut node);
            node.set_param("Radius", Value::F32(m.radius));
            ModifierOutcome::Node(out.insert(node))
        }
        "NiPSysCylinderEmitter" => {
            let m: NiPSysCylinderEmitter = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::CylinderEmitter, pos);
            emitter_meta(&m.emitter, &mut node);
            node.set_param("Radius", Value::F32(m.radius));
            node.set_param("Height", Value::F32(m.height));
            ModifierOutcome::Node(out.insert(node))
        }
        "NiPSysMeshEmitter" => {
            let m: NiPSysMeshEmitter = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::MeshEmitter, pos);
            emitter_meta(&m.emitter, &mut node);
            // Resolve the mesh pointers to shape names: the preview looks
            // sources up by name in the imported reference model.
            let mut names: Vec<String> = Vec::new();
            for r in &m.emitter_meshes {
                let Some(target) = r.get() else { continue };
                match shape_name(nif, ctx, target) {
                    Some(n) => names.push(n),
                    None => out.warnings.push(format!(
                        "mesh emitter references block {target}, whose name \
                         could not be resolved"
                    )),
                }
            }
            node.set_param("Emitter Meshes", Value::Str(names.join(", ")));
            node.set_param(
                "Velocity Type",
                Value::Enum(
                    crate::graph::VELOCITY_TYPES,
                    (m.initial_velocity_type as usize)
                        .min(crate::graph::VELOCITY_TYPES.len() - 1),
                ),
            );
            node.set_param(
                "Emission Type",
                Value::Enum(
                    crate::graph::EMIT_FROM_TYPES,
                    (m.emission_type as usize).min(crate::graph::EMIT_FROM_TYPES.len() - 1),
                ),
            );
            node.set_param("Emission Axis", Value::Vec3(m.emission_axis));
            ModifierOutcome::Node(out.insert(node))
        }
        "NiPSysGravityModifier" => {
            let m: NiPSysGravityModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::GravityModifier, pos);
            // The gravity object is where spherical gravity pulls toward
            // (or pushes from, with negative strength) and the frame the
            // planar axis lives in — the warhammer repels its sparks from
            // a node AT the head, not from the model origin.
            match rel_of(m.gravity_object) {
                Some(rel) => {
                    let baked = rel.rotate(m.gravity_axis);
                    node.set_param("Axis", Value::Vec3(baked));
                    node.set_param("Gravity Offset", Value::Vec3(rel.t));
                }
                None => {
                    node.set_param("Axis", Value::Vec3(m.gravity_axis));
                }
            }
            node.set_param("Strength", Value::F32(m.strength));
            node.set_param("Decay", Value::F32(m.decay));
            node.set_param(
                "Force Type",
                Value::Enum(crate::graph::FORCE_TYPES, (m.force_type as usize).min(1)),
            );
            node.set_param("Turbulence", Value::F32(m.turbulence));
            node.set_param("Turbulence Scale", Value::F32(m.turbulence_scale));
            node.set_param("World Aligned", Value::Bool(m.world_aligned));
            ModifierOutcome::Node(out.insert(node))
        }
        "NiPSysRotationModifier" => {
            let m: NiPSysRotationModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::RotationModifier, pos);
            node.set_param("Rotation Speed", Value::F32(m.rotation_speed));
            node.set_param("Speed Variation", Value::F32(m.rotation_speed_variation));
            node.set_param("Rotation Angle", Value::F32(m.rotation_angle));
            node.set_param("Angle Variation", Value::F32(m.rotation_angle_variation));
            node.set_param("Random Speed Sign", Value::Bool(m.random_rot_speed_sign));
            node.set_param("Random Axis", Value::Bool(m.random_axis));
            node.set_param("Axis", Value::Vec3(m.axis));
            ModifierOutcome::Node(out.insert(node))
        }
        "BSPSysSimpleColorModifier" => {
            let m: BSPSysSimpleColorModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::SimpleColorModifier, pos);
            node.set_param("Color 1", Value::Color4(m.colors[0]));
            node.set_param("Color 2", Value::Color4(m.colors[1]));
            node.set_param("Color 3", Value::Color4(m.colors[2]));
            node.set_param("Fade In End %", Value::F32(m.fade_in_percent));
            node.set_param("Color 1 End %", Value::F32(m.color1_end_percent));
            node.set_param("Color 2 Start %", Value::F32(m.color2_start_percent));
            node.set_param("Color 2 End %", Value::F32(m.color2_end_percent));
            node.set_param("Color 3 Start %", Value::F32(m.color3_start_percent));
            node.set_param("Fade Out Start %", Value::F32(m.fade_out_percent));
            ModifierOutcome::Node(out.insert(node))
        }
        "BSPSysScaleModifier" => {
            let m: BSPSysScaleModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::ScaleModifier, pos);
            node.set_param("Scales", Value::Str(crate::scale_table::format(&m.scales)));
            ModifierOutcome::Node(out.insert(node))
        }
        "NiPSysDragModifier" => {
            let m: NiPSysDragModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::DragModifier, pos);
            // The axis lives in the drag object's local frame; bake its
            // rotation into the stored axis (identical in game since the
            // exporter re-anchors with identity rotation).
            match rel_of(m.drag_object) {
                Some(rel) => {
                    let baked = rel.rotate(m.drag_axis);
                    node.set_param("Axis", Value::Vec3(baked));
                    node.set_param("Drag Offset", Value::Vec3(rel.t));
                }
                None => {
                    node.set_param("Axis", Value::Vec3(m.drag_axis));
                    if m.drag_object.get().is_none() {
                        out.warnings.push(
                            "drag modifier has no drag object — it does nothing in game \
                             (export re-anchors it, making it live)"
                                .to_string(),
                        );
                    }
                }
            }
            node.set_param("Percentage", Value::F32(m.percentage));
            node.set_param("Range", Value::F32(m.range));
            node.set_param("Range Falloff", Value::F32(m.range_falloff));
            ModifierOutcome::Node(out.insert(node))
        }
        "BSPSysSubTexModifier" => {
            let m: BSPSysSubTexModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::SubTexModifier, pos);
            node.set_param("Start Frame", Value::F32(m.start_frame));
            node.set_param("Start Frame Fudge", Value::F32(m.start_frame_fudge));
            node.set_param("End Frame", Value::F32(m.end_frame));
            node.set_param("Loop Start Frame", Value::F32(m.loop_start_frame));
            node.set_param("Loop Start Frame Fudge", Value::F32(m.loop_start_frame_fudge));
            node.set_param("Frame Count", Value::F32(m.frame_count));
            node.set_param("Frame Count Fudge", Value::F32(m.frame_count_fudge));
            ModifierOutcome::Node(out.insert(node))
        }
        "NiPSysSpawnModifier" => {
            let m: NiPSysSpawnModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::SpawnModifier, pos);
            // Whether death triggers this spawner is stored on the system's
            // age/death service block, not here; recover it from whichever
            // one points back at this block.
            let on_death = nif
                .blocks
                .iter()
                .enumerate()
                .filter(|(_, b)| b.type_name == "NiPSysAgeDeathModifier")
                .filter_map(|(i, _)| decode::<NiPSysAgeDeathModifier>(nif, ctx, i).ok())
                .any(|ad| ad.spawn_on_death && ad.spawn_modifier.get() == Some(idx));
            node.set_param("Spawn On Death", Value::Bool(on_death));
            node.set_param(
                "Spawn Generations",
                Value::U32(m.num_spawn_generations as u32),
            );
            node.set_param("Percentage Spawned", Value::F32(m.percentage_spawned));
            node.set_param("Min Num to Spawn", Value::U32(m.min_num_to_spawn as u32));
            node.set_param("Max Num to Spawn", Value::U32(m.max_num_to_spawn as u32));
            node.set_param("Spawn Speed Variation", Value::F32(m.spawn_speed_variation));
            node.set_param("Spawn Dir Variation", Value::F32(m.spawn_dir_variation));
            node.set_param("Life Span", Value::F32(m.life_span));
            node.set_param("Life Span Variation", Value::F32(m.life_span_variation));
            ModifierOutcome::Node(out.insert(node))
        }
        "BSPSysLODModifier" => {
            let m: BSPSysLODModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::LodModifier, pos);
            node.set_param("LOD Begin Distance", Value::F32(m.lod_begin_distance));
            node.set_param("LOD End Distance", Value::F32(m.lod_end_distance));
            node.set_param("End Emit Scale", Value::F32(m.end_emit_scale));
            node.set_param("End Size", Value::F32(m.end_size));
            ModifierOutcome::Node(out.insert(node))
        }
        "NiPSysBombModifier" => {
            let m: NiPSysBombModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::BombModifier, pos);
            node.set_param("Axis", Value::Vec3(m.bomb_axis));
            node.set_param("Decay", Value::F32(m.decay));
            node.set_param("Delta V", Value::F32(m.delta_v));
            node.set_param(
                "Decay Type",
                Value::Enum(crate::graph::DECAY_TYPES, (m.decay_type as usize).min(2)),
            );
            node.set_param(
                "Symmetry",
                Value::Enum(crate::graph::SYMMETRY_TYPES, (m.symmetry_type as usize).min(2)),
            );
            if let Some(rel) = rel_of(m.bomb_object) {
                node.set_param("Bomb Offset", Value::Vec3(rel.t));
            }
            ModifierOutcome::Node(out.insert(node))
        }
        "BSPSysInheritVelocityModifier" => {
            let m: BSPSysInheritVelocityModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::InheritVelocityModifier, pos);
            // The sim tracks the anchor by NAME: only the viewport
            // knows where sequences have moved the scene's nodes.
            if let Some(target) = m.inherit_object.get() {
                if let Some(name) = node_name_of(nif, target) {
                    node.set_param("Inherit Node", Value::Str(name));
                }
            }
            node.set_param("Chance To Inherit", Value::F32(m.chance_to_inherit));
            node.set_param("Velocity Multiplier", Value::F32(m.velocity_multiplier));
            node.set_param("Velocity Variation", Value::F32(m.velocity_variation));
            if let Some(rel) = rel_of(m.inherit_object) {
                node.set_param("Inherit Offset", Value::Vec3(rel.t));
            }
            ModifierOutcome::Node(out.insert(node))
        }
        "BSPSysRecycleBoundModifier" => {
            let m: BSPSysRecycleBoundModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::RecycleBoundModifier, pos);
            node.set_param("Bound Offset", Value::Vec3(m.bound_offset));
            node.set_param("Bound Extent", Value::Vec3(m.bound_extent));
            if let Some(rel) = rel_of(m.bound_object) {
                node.set_param("Anchor Offset", Value::Vec3(rel.t));
            }
            ModifierOutcome::Node(out.insert(node))
        }
        "BSWindModifier" => {
            let m: BSWindModifier = decode(nif, ctx, idx)?;
            let mut node = GraphNode::new(NodeKind::WindModifier, pos);
            node.set_param("Strength", Value::F32(m.strength));
            ModifierOutcome::Node(out.insert(node))
        }
        "NiPSysColliderManager" => {
            use aether_nif::typed::particles::{
                NiPSysColliderManager, NiPSysPlanarCollider, NiPSysSphericalCollider,
            };
            let m: NiPSysColliderManager = decode(nif, ctx, idx)?;
            // Each collider becomes its own node; the compiler re-emits
            // one manager per node. Most files chain exactly one collider
            // per manager, but 56 (the Forgemaster, the Soul Cairn wisps)
            // chain more — the walk follows `next_collider` to the end,
            // because a dropped second collider reads as "particles fall
            // through one wall of the pit and not the other".
            let mut nodes: Vec<NodeId> = Vec::new();
            let mut next_ci = m.collider.get();
            let mut row = 0f32;
            while let Some(ci) = next_ci {
                next_ci = None;
                // The manager is one block and each collider another;
                // the node the user sees is the COLLIDER, so that is
                // the slot it owns.
                out.from_block(ci);
                let pos = [pos[0], pos[1] + row * 40.0];
                row += 1.0;
                let mut node = GraphNode::new(NodeKind::ColliderModifier, pos);
                node.set_param("Active", Value::Bool(m.base.active));
                let base = match nif.blocks[ci].type_name.as_str() {
                    "NiPSysSphericalCollider" => {
                        let c: NiPSysSphericalCollider = decode(nif, ctx, ci)?;
                        node.set_param("Shape", Value::Enum(crate::graph::COLLIDER_SHAPES, 0));
                        node.set_param("Radius", Value::F32(c.radius));
                        Some(c.collider)
                    }
                    "NiPSysPlanarCollider" => {
                        let c: NiPSysPlanarCollider = decode(nif, ctx, ci)?;
                        node.set_param("Shape", Value::Enum(crate::graph::COLLIDER_SHAPES, 1));
                        node.set_param("Width", Value::F32(c.width));
                        node.set_param("Height", Value::F32(c.height));
                        node.set_param("X Axis", Value::Vec3(c.x_axis));
                        node.set_param("Y Axis", Value::Vec3(c.y_axis));
                        Some(c.collider)
                    }
                    other => {
                        out.warnings.push(format!(
                            "collider {other} (block {ci}) is not supported; skipped"
                        ));
                        None
                    }
                };
                if let Some(base) = base {
                    next_ci = base.next_collider.get();
                    node.set_param("Bounce", Value::F32(base.bounce));
                    node.set_param("Die On Collide", Value::Bool(base.die_on_collide));
                    node.set_param("Spawn On Collide", Value::Bool(base.spawn_on_collide));
                    if let Some(rel) = rel_of(base.collider_object) {
                        node.set_param("Collider Offset", Value::Vec3(rel.t));
                        let euler = crate::mesh::Transform::euler_from_rows(&rel.rot);
                        node.set_param("Collider Rotation", Value::Vec3(euler));
                        // The game scales collider geometry by the
                        // object/psys scale ratio.
                        if (rel.s - 1.0).abs() > 1e-4 {
                            let scale = |node: &mut GraphNode, key: &str| {
                                if let Some(Value::F32(v)) = node.param(key).cloned() {
                                    node.set_param(key, Value::F32(v * rel.s));
                                }
                            };
                            // Per SHAPE, not per type. The closure reads
                            // whatever the param holds, so scaling all
                            // three multiplied a SPHERICAL collider's
                            // untouched Width/Height schema defaults —
                            // numbers the shape does not use, shown to
                            // the user as if it did.
                            match nif.blocks[ci].type_name.as_str() {
                                "NiPSysSphericalCollider" => scale(&mut node, "Radius"),
                                _ => {
                                    scale(&mut node, "Width");
                                    scale(&mut node, "Height");
                                }
                            }
                        }
                    }
                    nodes.push(out.insert(node));
                }
            }
            if nodes.is_empty() {
                ModifierOutcome::Unsupported
            } else {
                ModifierOutcome::Chain(nodes)
            }
        }
        _ => ModifierOutcome::Unsupported,
    })
}

/// A scene block's own name (NiObjectNET), for anchoring by name.
fn node_name_of(nif: &NifFile, idx: usize) -> Option<String> {
    let block = nif.blocks.get(idx)?;
    if !crate::mesh::is_scene_node(&block.type_name) {
        return None;
    }
    let ctx = Ctx::of(&nif.header).ok()?;
    let node = crate::mesh::decode_node(block, ctx).ok()?;
    node.av.net.name.resolve(&nif.header).map(str::to_string)
}

/// A modifier block's stack name — the first field of every NiPSysModifier.
fn modifier_name_of(nif: &NifFile, idx: usize) -> Option<String> {
    let raw = nif.blocks[idx].data.get(0..4)?;
    aether_nif::typed::StringIdx(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
        .resolve(&nif.header)
        .map(str::to_string)
}

/// Bool keys behind a (timeline) bool interpolator, `(time, raw byte)`.
/// A float interpolator's key track, empty for the blend (sequence-
/// managed) forms — which is the correct import: a blend interpolator's
/// pose comes from whichever sequence owns it, and a free-running track
/// with those keys would fight the sequence player.
fn float_keys_of(nif: &NifFile, ctx: Ctx, interp: BlockRef) -> crate::curve::Curve {
    use aether_nif::typed::controllers::{NiFloatData, NiFloatInterpolator};
    let out = crate::curve::Curve::empty(crate::curve::Track::Float);
    let Some(i) = interp.get() else { return out };
    let Some(block) = nif.blocks.get(i) else { return out };
    if block.type_name != "NiFloatInterpolator" {
        return out;
    }
    NiFloatInterpolator::decode(block, ctx)
        .ok()
        .and_then(|f| f.data.get())
        .and_then(|di| nif.blocks.get(di))
        .and_then(|b| NiFloatData::decode(b, ctx).ok())
        .map(|d| crate::curve::Curve::from_float_data(&d))
        .unwrap_or(out)
}

/// The on/off track behind a bool interpolator, keeping the block's
/// own key TYPE — which the old text form threw away, because
/// `"0 = on"` has nowhere to put it.
fn bool_track_of(nif: &NifFile, ctx: Ctx, interp: BlockRef) -> crate::curve::Curve {
    use aether_nif::typed::controllers::NiBoolData;
    let empty = crate::curve::Curve::empty(crate::curve::Track::Bool);
    let Some(i) = interp.get() else { return empty };
    let Some(block) = nif.blocks.get(i) else {
        return empty;
    };
    let data = match block.type_name.as_str() {
        "NiBoolTimelineInterpolator" => {
            use aether_nif::typed::controllers::NiBoolTimelineInterpolator;
            NiBoolTimelineInterpolator::decode(block, ctx).ok().map(|b| b.data)
        }
        "NiBoolInterpolator" => {
            use aether_nif::typed::controllers::NiBoolInterpolator;
            NiBoolInterpolator::decode(block, ctx).ok().map(|b| b.data)
        }
        _ => None,
    };
    data.and_then(|d| d.get())
        .and_then(|di| nif.blocks.get(di))
        .and_then(|b| NiBoolData::decode(b, ctx).ok())
        .map(|d| crate::curve::Curve::from_bool_keys(d.interpolation, &d.keys))
        .unwrap_or(empty)
}

/// Recover (rows, cols) from compiled subtexture offsets; `(1, 1)` if none.
fn atlas_from_offsets(offsets: &[[f32; 4]]) -> (u32, u32) {
    match offsets.first() {
        Some([_, u_size, _, v_size]) if *u_size > 0.0 && *v_size > 0.0 => {
            let cols = (1.0 / u_size).round().max(1.0) as u32;
            let rows = (1.0 / v_size).round().max(1.0) as u32;
            (rows, cols)
        }
        _ => (1, 1),
    }
}

fn wire(out: &mut Decompiled, from: NodeId, from_socket: usize, to: NodeId, to_socket: usize) {
    let link = Link {
        from_node: from,
        from_socket,
        to_node: to,
        to_socket,
    };
    if let Err(e) = out.graph.connect(link) {
        out.warnings
            .push(format!("could not reconnect imported wire: {e}"));
    }
}

/// Name of an AVObject-derived block, for the types a mesh emitter points at.
pub(crate) fn shape_name(nif: &NifFile, ctx: Ctx, idx: usize) -> Option<String> {
    use aether_nif::typed::geometry::{BsTriShape, NiTriShape};
    let block = nif.blocks.get(idx)?;
    let name = match block.type_name.as_str() {
        "BSTriShape" => BsTriShape::decode(block, ctx).ok()?.av.net.name,
        "NiTriShape" => NiTriShape::decode(block, ctx).ok()?.av.net.name,
        t if NiNode::TYPE_NAMES.contains(&t) => NiNode::decode(block, ctx).ok()?.av.net.name,
        _ => return None,
    };
    name.resolve(&nif.header).map(str::to_string)
}
