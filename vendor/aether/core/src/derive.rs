//! The one and only writer of graph parameters derived from a node's
//! transform.
//!
//! The graph does not store a NIF transform; it stores DERIVED WORLD
//! QUANTITIES read off the file at import — a particle system's world
//! scale/offset/rotation, and every modifier's anchor offset measured in
//! that system's space. The moment a user can move a node, all of them
//! have to be recomputed, or the graph goes on claiming an offset
//! measured against where the node used to be.
//!
//! So the derivation is a WHOLE-GRAPH pass rather than ten expressions
//! scattered through `decompile`, and `decompile` runs the same pass an
//! edit does. Import and edit cannot drift because there is one of them.
//!
//! **Total, never incremental.** Several of these are PRODUCTS of an
//! authored field and a matrix — the gravity axis is `rel.rot · authored`
//! and a spherical collider's radius is `authored · rel.s` — so patching
//! the stored parameter compounds. Every operand is re-read from the
//! block each time. The census that establishes which parameters these
//! are, and which are raw, is `docs/DERIVED-PARAMS.md`.

use crate::graph::{Graph, NodeId, NodeKind};
use crate::mesh::Transform;
use crate::value::Value;
use aether_nif::typed::{particles::*, BlockRef, Ctx, TypedBlock};
use aether_nif::NifFile;
use std::collections::BTreeMap;

/// A particle system's world frame, and its inverse.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    /// What the game renders the system under (RE 0x140D039B0): for a
    /// world-space system, rotation and translation are ZEROED and only
    /// the accumulated scale survives.
    pub world: Transform,
    /// `world.inverse()`, the map from world space into system space —
    /// which is where every anchor offset is measured.
    pub inv: Transform,
}

/// Parent chain of a block, once, so an anchor lookup is not a fresh
/// walk from the roots: `forgmasterfx.nif` has 29 anchors over 900+
/// blocks.
pub fn parents_of(nif: &NifFile, ctx: Ctx) -> BTreeMap<usize, usize> {
    let mut out = BTreeMap::new();
    for (i, b) in nif.blocks.iter().enumerate() {
        let Some(refs) = aether_nif::typed::block_refs(b, ctx) else {
            continue;
        };
        for (r, k) in refs {
            if k != aether_nif::typed::RefKind::Child {
                continue;
            }
            if let Some(t) = r.get() {
                out.entry(t).or_insert(i);
            }
        }
    }
    out
}

/// Local transforms substituted for the ones the file stores, keyed by
/// BLOCK — what a playing animation has done to the scene this frame.
///
/// Empty means "the authored pose", which is what import derives from.
pub type Posed = BTreeMap<usize, Transform>;

/// A block's world transform, walking the parent chain.
fn world_of(
    nif: &NifFile,
    ctx: Ctx,
    parents: &BTreeMap<usize, usize>,
    posed: &Posed,
    block: usize,
) -> Option<Transform> {
    let mut chain = Vec::new();
    let mut at = Some(block);
    while let Some(i) = at {
        if chain.len() > nif.blocks.len() {
            break;
        }
        chain.push(i);
        at = parents.get(&i).copied();
    }
    let mut world = Transform::IDENTITY;
    for i in chain.iter().rev() {
        let local = match posed.get(i) {
            Some(t) => *t,
            None => {
                let av = aether_nif::typed::block_av(nif.blocks.get(*i)?, ctx)?;
                Transform::from_av(av.translation, av.rotation, av.scale)
            }
        };
        world = world.compose(&local);
    }
    Some(world)
}

/// A block's world transform under a POSE, for a caller that already
/// holds the parent map (a per-frame recompute must not rebuild it).
pub fn block_world_posed(
    nif: &NifFile,
    ctx: Ctx,
    parents: &BTreeMap<usize, usize>,
    posed: &Posed,
    block: usize,
) -> Option<Transform> {
    world_of(nif, ctx, parents, posed, block)
}

/// A block's world transform, and its parent's.
///
/// The pair a transform edit needs: the gizmo works in world space, and
/// the file stores a LOCAL transform, so the solve needs both ends.
pub fn block_worlds(nif: &NifFile, ctx: Ctx, block: usize) -> Option<(Transform, Transform)> {
    let parents = parents_of(nif, ctx);
    let posed = Posed::new();
    let world = world_of(nif, ctx, &parents, &posed, block)?;
    let parent = match parents.get(&block) {
        Some(p) => world_of(nif, ctx, &parents, &posed, *p)?,
        None => Transform::IDENTITY,
    };
    Some((parent, world))
}

/// The frame a particle system renders in.
pub fn system_frame(nif: &NifFile, ctx: Ctx, psys_block: usize) -> Option<Frame> {
    let parents = parents_of(nif, ctx);
    system_frame_with(nif, ctx, &parents, &Posed::new(), psys_block)
}

/// The same frame under a POSE — what the system renders in while an
/// animation is playing.
///
/// The game recomputes this every update (it reads `psys->world` inside
/// the spawn routine, 1.5.97 @ 0x140D26FA0), so a system hanging off a
/// node a controller is turning travels with it. Import derives the
/// authored pose; this is the identical derivation with the scene's
/// current locals substituted, so the two cannot drift.
pub fn system_frame_posed(
    nif: &NifFile,
    ctx: Ctx,
    parents: &BTreeMap<usize, usize>,
    posed: &Posed,
    psys_block: usize,
) -> Option<Frame> {
    system_frame_with(nif, ctx, parents, posed, psys_block)
}

fn system_frame_with(
    nif: &NifFile,
    ctx: Ctx,
    parents: &BTreeMap<usize, usize>,
    posed: &Posed,
    psys_block: usize,
) -> Option<Frame> {
    let full = world_of(nif, ctx, parents, posed, psys_block)?;
    let psys = NiParticleSystem::decode(nif.blocks.get(psys_block)?, ctx).ok()?;
    let world = if psys.world_space {
        // The game zeroes rotation and translation for a world-space
        // system and KEEPS the accumulated scale.
        Transform {
            rot: Transform::IDENTITY.rot,
            t: [0.0; 3],
            s: full.s,
        }
    } else {
        full
    };
    Some(Frame {
        inv: world.inverse(),
        world,
    })
}

/// An anchor's transform in the system's own space.
///
/// `None` when the pointer is null or does not resolve — which is a real
/// state, not an error: a drag modifier with no object does nothing in
/// game, and the graph records that as a zero offset.
pub fn anchor_rel(
    nif: &NifFile,
    ctx: Ctx,
    frame: &Frame,
    obj: BlockRef,
    parents: &BTreeMap<usize, usize>,
) -> Option<Transform> {
    anchor_rel_posed(nif, ctx, frame, obj, parents, &Posed::new())
}

/// The same relative transform under a POSE.
///
/// This is what the game resolves per SPAWNED PARTICLE: vfunc 43 reads
/// `emitterObject->world` and `psys->world` fresh inside the spawn loop
/// (1.5.97 @ 0x140D26FA0, called from 0x140D09AD0), so an emitter
/// anchored to a turning node emits from wherever that node is NOW.
pub fn anchor_rel_posed(
    nif: &NifFile,
    ctx: Ctx,
    frame: &Frame,
    obj: BlockRef,
    parents: &BTreeMap<usize, usize>,
    posed: &Posed,
) -> Option<Transform> {
    let i = obj.get()?;
    let w = world_of(nif, ctx, parents, posed, i)?;
    Some(frame.inv.compose(&w))
}

/// Every parameter that is a function of a node transform, recomputed.
///
/// Returns the CHANGES rather than mutating behind the caller's back:
/// the app wraps them in one undoable command, and a pass that found
/// nothing returns an empty list, which is what makes idempotence
/// checkable.
pub fn rederive(graph: &Graph, nif: &NifFile) -> Vec<(NodeId, &'static str, Value)> {
    let Ok(ctx) = Ctx::of(&nif.header) else {
        return Vec::new();
    };
    let parents = parents_of(nif, ctx);
    let mut out = Vec::new();

    // Which system each modifier belongs to: modifiers wire into socket
    // 3 of their ParticleSystem node.
    let mut system_of: BTreeMap<NodeId, NodeId> = BTreeMap::new();
    for link in &graph.links {
        if graph.nodes.get(&link.to_node).map(|n| n.kind) == Some(NodeKind::ParticleSystem) {
            system_of.insert(link.from_node, link.to_node);
        }
    }

    // Float-tolerant, and that is not laziness: `decompile` reaches some
    // of these by a different order of operations (it scales a param it
    // just wrote; this re-reads the block and scales once), so exact
    // equality reports a change of 8e-6 as a change. A tolerance keeps
    // "nothing moved" meaning nothing moved.
    fn same(a: Option<&Value>, b: &Value) -> bool {
        match (a, b) {
            (Some(Value::F32(x)), Value::F32(y)) => (x - y).abs() <= 1e-4 * x.abs().max(1.0),
            (Some(Value::Vec3(x)), Value::Vec3(y)) => (0..3)
                .all(|i| (x[i] - y[i]).abs() <= 1e-4 * x[i].abs().max(1.0)),
            (Some(x), y) => x == y,
            (None, _) => false,
        }
    }
    let mut push = |out: &mut Vec<_>, id: NodeId, key: &'static str, v: Value| {
        if !same(graph.nodes[&id].param(key), &v) {
            out.push((id, key, v));
        }
    };

    for (id, node) in &graph.nodes {
        let Some(src) = node.source_block.map(|b| b as usize) else {
            continue;
        };
        if src >= nif.blocks.len() {
            continue;
        }
        // The system this node's frame comes from: itself, or the system
        // it is wired into.
        let sys_block = match node.kind {
            NodeKind::ParticleSystem => Some(src),
            _ => system_of
                .get(id)
                .and_then(|s| graph.nodes.get(s))
                .and_then(|s| s.source_block)
                .map(|b| b as usize),
        };
        let Some(frame) =
            sys_block.and_then(|b| system_frame_with(nif, ctx, &parents, &Posed::new(), b))
        else {
            continue;
        };
        let block = &nif.blocks[src];
        let rel = |obj: BlockRef| anchor_rel(nif, ctx, &frame, obj, &parents);
        // Zero, not "leave it alone": the guards that used to skip a
        // zero meant a parameter could never be driven BACK to zero once
        // the node moved home again.
        let t_of = |r: &Option<Transform>| Value::Vec3(r.map(|x| x.t).unwrap_or([0.0; 3]));
        let e_of = |r: &Option<Transform>| {
            Value::Vec3(
                r.map(|x| Transform::euler_from_rows(&x.rot))
                    .unwrap_or([0.0; 3]),
            )
        };

        match node.kind {
            NodeKind::ParticleSystem => {
                push(&mut out, *id, "World Scale", Value::F32(frame.world.s));
                push(&mut out, *id, "World Offset", Value::Vec3(frame.world.t));
                push(
                    &mut out,
                    *id,
                    "World Rotation",
                    Value::Vec3(Transform::euler_from_rows(&frame.world.rot)),
                );
            }
            NodeKind::BoxEmitter | NodeKind::SphereEmitter | NodeKind::CylinderEmitter => {
                let Ok(e) = NiPSysBoxEmitter::decode(block, ctx).map(|b| b.emitter).or_else(|_| {
                    NiPSysSphereEmitter::decode(block, ctx)
                        .map(|b| b.emitter)
                        .or_else(|_| NiPSysCylinderEmitter::decode(block, ctx).map(|b| b.emitter))
                }) else {
                    continue;
                };
                let r = rel(e.emitter_object);
                push(&mut out, *id, "Emitter Offset", t_of(&r));
                push(&mut out, *id, "Emitter Rotation", e_of(&r));
            }
            NodeKind::GravityModifier => {
                let Ok(m) = NiPSysGravityModifier::decode(block, ctx) else { continue };
                let r = rel(m.gravity_object);
                push(&mut out, *id, "Gravity Offset", t_of(&r));
                // BAKED: re-read the AUTHORED axis from the block. The
                // stored param is the product and is not invertible —
                // nothing records whether the anchor resolved.
                let axis = match &r {
                    Some(x) => x.rotate(m.gravity_axis),
                    None => m.gravity_axis,
                };
                push(&mut out, *id, "Axis", Value::Vec3(axis));
            }
            NodeKind::DragModifier => {
                let Ok(m) = NiPSysDragModifier::decode(block, ctx) else { continue };
                let r = rel(m.drag_object);
                push(&mut out, *id, "Drag Offset", t_of(&r));
                let axis = match &r {
                    Some(x) => x.rotate(m.drag_axis),
                    None => m.drag_axis,
                };
                push(&mut out, *id, "Axis", Value::Vec3(axis));
            }
            NodeKind::BombModifier => {
                let Ok(m) = NiPSysBombModifier::decode(block, ctx) else { continue };
                push(&mut out, *id, "Bomb Offset", t_of(&rel(m.bomb_object)));
                // RAW, deliberately: the bomb's axis is NOT baked, which
                // is a real asymmetry with gravity and drag. Dispatching
                // on the key name alone would corrupt it.
            }
            NodeKind::InheritVelocityModifier => {
                let Ok(m) = BSPSysInheritVelocityModifier::decode(block, ctx) else { continue };
                push(&mut out, *id, "Inherit Offset", t_of(&rel(m.inherit_object)));
            }
            NodeKind::RecycleBoundModifier => {
                let Ok(m) = BSPSysRecycleBoundModifier::decode(block, ctx) else { continue };
                push(&mut out, *id, "Anchor Offset", t_of(&rel(m.bound_object)));
            }
            NodeKind::ColliderModifier => {
                let base = match block.type_name.as_str() {
                    "NiPSysSphericalCollider" => {
                        NiPSysSphericalCollider::decode(block, ctx).ok().map(|c| (c.collider, Some(c.radius), None))
                    }
                    "NiPSysPlanarCollider" => NiPSysPlanarCollider::decode(block, ctx)
                        .ok()
                        .map(|c| (c.collider, None, Some((c.width, c.height)))),
                    _ => None,
                };
                let Some((base, radius, plane)) = base else { continue };
                let r = rel(base.collider_object);
                push(&mut out, *id, "Collider Offset", t_of(&r));
                push(&mut out, *id, "Collider Rotation", e_of(&r));
                // BAKED DESTRUCTIVELY: the multiplier is stored nowhere,
                // so this must come from the block, never from the param.
                let s = r.map(|x| x.s).unwrap_or(1.0);
                if let Some(radius) = radius {
                    push(&mut out, *id, "Radius", Value::F32(radius * s));
                }
                if let Some((w, h)) = plane {
                    push(&mut out, *id, "Width", Value::F32(w * s));
                    push(&mut out, *id, "Height", Value::F32(h * s));
                }
            }
            _ => {}
        }
    }
    out
}
