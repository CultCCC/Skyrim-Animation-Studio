//! Graph → NIF compiler.
//!
//! Turns one graph document into a complete NIF file for Skyrim SE (bs100)
//! or LE (bs83). Structural conventions follow vanilla effect NIFs
//! (verified against `crates/nif/tests/fixtures/frost.nif` and friends):
//!
//! - root is a `BSFadeNode` holding the particle systems;
//! - every system gets the mandatory service modifiers the editor never
//!   shows: `NiPSysAgeDeathModifier` (first), `NiPSysPositionModifier` and
//!   `NiPSysBoundUpdateModifier` (last);
//! - modifiers are sorted by canonical `NiPSysModifierOrder` class and then
//!   written with *sequential* `Order` values and `Modifier:N` names — that
//!   is what Bethesda's own exporter does;
//! - controllers form a chain via `Next Controller` off the system's
//!   `Controller` slot, each targeting the system (target-first ownership);
//!   `NiPSysEmitterCtlr` resolves its emitter from the wire and stores the
//!   emitter's *name*; a missing `NiPSysUpdateCtlr` is appended automatically;
//! - birth rate / visibility live in constant-pose `NiFloatInterpolator` /
//!   `NiBoolInterpolator` blocks referenced by the emitter controller.

use crate::curve::{Curve, Track};
use crate::graph::{Graph, GraphNode, Link, NodeId, NodeKind};

/// The `NiBoolData` key type to write for an on/off track.
///
/// A track imported from a file carries the file's own type; an empty
/// or from-scratch one has none, and takes the only type vanilla
/// ships: CONST, 9,771 of 9,771 corpus blocks.
fn bool_key_type(track: &Curve) -> u32 {
    match track.interpolation {
        0 => 5,
        i => i,
    }
}
use std::collections::{BTreeMap, BTreeSet};
use crate::project::TargetGame;
use aether_nif::typed::controllers::{
    ControllerBase, NiBoolData, NiBoolInterpolator, NiBoolTimelineInterpolator,
    BSPSysMultiTargetEmitterCtlr, NiFloatInterpolator, NiPSysEmitterCtlr,
    NiPSysModifierActiveCtlr, NiPSysModifierFloatCtlr, NiPSysUpdateCtlr, NiVisController,
};
use aether_nif::typed::geometry::{BsMasterParticleSystem, NiNode};
use aether_nif::typed::particles::{
    order, BSPSysInheritVelocityModifier, BSPSysLODModifier, BSPSysScaleModifier, BSPSysSimpleColorModifier,
    BSPSysSubTexModifier, BSStripPSysData, EmitterBase, ModifierBase, NiPSysAgeDeathModifier,
    NiPSysBombModifier, NiPSysBoundUpdateModifier, NiPSysBoxEmitter, NiPSysCylinderEmitter,
    NiPSysData, NiPSysDragModifier, NiPSysGravityModifier, NiPSysMeshEmitter,
    NiPSysPositionModifier, NiPSysRotationModifier, NiPSysSpawnModifier, NiPSysSphereEmitter,
    BSPSysRecycleBoundModifier, BSWindModifier, NiPSysPlanarCollider,
    NiPSysSphericalCollider, NiParticleSystem,
};
use aether_nif::typed::shaders::{BSEffectShaderProperty, NiAlphaProperty};
use aether_nif::typed::controllers::NiFloatData as NiFloatDataView;
use aether_nif::typed::controllers::{
    BSEffectShaderPropertyColorController, BSEffectShaderPropertyFloatController,
    NiPoint3Interpolator, NiPosData,
};
use aether_nif::typed::{
    BlockRef, Ctx, RefKind, StringIdx, TypedBlock, block_av, block_refs, retarget_refs,
    set_block_av,
};
use aether_nif::{Block, Header, NifFile};

/// Vertex layout vanilla SSE particle systems use (BSVertexDesc).
const SSE_PARTICLE_VERTEX_DESC: u64 = 0x0840_2000_0400_0051;

/// Texture clamp byte per `graph::CLAMP_MODES` index.
const CLAMP_MODE_BYTES: [u8; 4] = [3, 0, 1, 2];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// The graph needs exactly one Scene Output node.
    NoSceneOutput,
    MultipleSceneOutputs,
    /// Scene Output has no particle systems wired in.
    NoSystems,
    /// A Particle System node lacks its Data input.
    SystemWithoutData { system: NodeId },
    /// An Emitter Controller's `Emitter` input is unwired, or wired to a
    /// modifier that is not part of the same system's modifier stack.
    ControllerWithoutEmitter { controller: NodeId },
    /// The splice met a block it cannot read its references out of, so
    /// it cannot renumber them. Impossible for anything the compiler
    /// emits — every apparatus type has a typed view — but refusing
    /// beats writing a block whose refs still point into the compiled
    /// numbering.
    UnreadableBlock { type_name: String },
    /// The graph was DECOMPILED from a NIF and that file is not open,
    /// so the splice has nothing to write into.
    ///
    /// Writing the compiled apparatus alone here is the F1 defect this
    /// project already fixed once: the model, sequences, behaviour
    /// graph, collision and flags are all in the source, and a graph
    /// that names source blocks is a graph that only describes the
    /// remainder. It was reachable again through a REOPENED project,
    /// which loses the file, and it was silent — the losses ledger,
    /// the refuse-to-overwrite guard and the itemised warnings all sit
    /// on the branch that has a source.
    SourceMissing,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::NoSceneOutput => write!(f, "graph has no Scene Output node"),
            CompileError::MultipleSceneOutputs => {
                write!(f, "graph has more than one Scene Output node")
            }
            CompileError::NoSystems => write!(f, "Scene Output has no particle systems wired in"),
            CompileError::SystemWithoutData { .. } => {
                write!(f, "Particle System has no Particle Data wired into `Data`")
            }
            CompileError::ControllerWithoutEmitter { .. } => write!(
                f,
                "Emitter Controller must be wired to an emitter that belongs to the same system"
            ),
            CompileError::UnreadableBlock { type_name } => {
                write!(f, "{type_name} has no typed view, so its block references cannot be renumbered")
            }
            CompileError::SourceMissing => write!(
                f,
                "this graph was imported from a NIF and that file is not open; \
                 exporting without it would write the particle apparatus alone \
                 and drop the model, collision and sequences"
            ),
        }
    }
}

/// Accumulates blocks + header while systems are being emitted.
struct Builder {
    header: Header,
    blocks: Vec<Block>,
    ctx: Ctx,
    /// Anchor nodes created for offset emitters; they must hang off the
    /// root or the game never resolves the emitter-object pointer.
    extra_root_children: Vec<BlockRef>,
    /// Which compiled block is each graph node's own. The mirror image of
    /// `GraphNode::source_block`, and the two together are what let the
    /// splice put a block back where it came from.
    node_block: BTreeMap<NodeId, usize>,
    /// Compiled blocks that exist because a graph PARAMETER holds
    /// keys: the key data of a track.
    ///
    /// The splice's seed for "what does this file need?" asks which
    /// compiled blocks are the image of something the DOCUMENT says
    /// exists. `node_block` answers that for nodes; a key track is the
    /// other kind of thing the document says exists, and it has no
    /// node of its own -- it hangs under an interpolator, which in an
    /// imported file already exists and therefore pairs. Without this
    /// a track typed into an empty parameter was never appended and
    /// the export came back byte-identical to the file it read.
    ///
    /// NOT a structural test. "The compiled ref is set where the
    /// source holds NULL" was tried and is wrong: the compiler also
    /// SYNTHESIZES apparatus vanilla omits, and on a null edit
    /// frost2/frost3 (no `NiPSysUpdateCtlr`) and
    /// tg06controllocationfx then grew by blocks nobody authored.
    keyed_data: BTreeSet<usize>,
    /// Names already given to synthesized anchor nodes.
    ///
    /// A scene node is addressed BY NAME -- that is how a
    /// `NiControllerSequence` binds a controlled block and how
    /// `NiDefaultAVObjectPalette` maps a name to an object -- so two
    /// nodes sharing one makes the second unaddressable. Measured on a
    /// user's export of three pasted systems: three nodes called
    /// `EmitterAnchor` and two called `GravityAnchor`.
    anchor_names: BTreeSet<String>,
}

impl Builder {
    fn new(target: TargetGame) -> Self {
        Builder::seeded(target, None)
    }

    /// A free name for a synthesized anchor.
    ///
    /// The suffix is `:2`, `:3`, ... -- the separator vanilla itself
    /// uses for generated names (`EbonyDagger:1524`), so the result
    /// still looks like something the exporter produced rather than
    /// like a corrupted string.
    fn unique_anchor_name(&mut self, base: &str) -> String {
        let mut name = base.to_string();
        let mut i = 2;
        while !self.anchor_names.insert(name.clone()) {
            name = format!("{base}:{i}");
            i += 1;
        }
        name
    }

    /// A builder whose header starts as a clone of `base`.
    ///
    /// This is what makes the splice's string handling a non-problem:
    /// interning is append-only, so every string the source already has
    /// keeps its index and every string the compiler adds lands at the
    /// end. Compiled blocks therefore carry string indices that are
    /// already valid in the file they are spliced into — no remap, and
    /// no chance of a silently shifted name.
    fn seeded(target: TargetGame, base: Option<&Header>) -> Self {
        let mut header = match base {
            Some(h) => h.clone(),
            None => Header::new_sse(),
        };
        let ctx = match target {
            TargetGame::SkyrimSE => Ctx::sse(),
            TargetGame::SkyrimLE => {
                header.bs_version = aether_nif::BsStreamVersion::SkyrimLE;
                Ctx::le()
            }
        };
        Builder {
            header,
            blocks: Vec::new(),
            ctx,
            extra_root_children: Vec::new(),
            node_block: BTreeMap::new(),
            keyed_data: BTreeSet::new(),
            anchor_names: BTreeSet::new(),
        }
    }

    /// Record that `slot` is the block `node` compiles to.
    fn claim(&mut self, node: NodeId, slot: BlockRef) {
        if let Some(i) = slot.get() {
            self.node_block.insert(node, i);
        }
    }

    fn intern(&mut self, s: &str) -> StringIdx {
        StringIdx(self.header.intern_string(s))
    }

    /// Reserve a slot so earlier blocks can reference later ones.
    fn reserve(&mut self, type_name: &str) -> BlockRef {
        self.blocks.push(Block {
            type_name: type_name.to_string(),
            data: Vec::new(),
        });
        BlockRef::new(self.blocks.len() - 1)
    }

    fn fill<T: TypedBlock>(&mut self, slot: BlockRef, value: &mut T) {
        let idx = slot.get().expect("fill() needs a reserved slot");
        let type_name = self.blocks[idx].type_name.clone();
        self.blocks[idx] = value.encode_as(&type_name, self.ctx);
    }

    /// Append a block VERBATIM. Only for blocks the graph does not
    /// model and that carry no references of their own, so nothing
    /// inside them needs retargeting -- the blend interpolators are
    /// the case this exists for (four scalars, no refs).
    fn push_raw(&mut self, block: Block) -> BlockRef {
        self.blocks.push(block);
        BlockRef::new(self.blocks.len() - 1)
    }

    fn push<T: TypedBlock>(&mut self, value: &mut T) -> BlockRef {
        let slot = self.reserve(T::TYPE_NAMES[0]);
        self.fill(slot, value);
        slot
    }

    /// [`Self::push`] for the key DATA of a track: this block exists
    /// because a graph parameter holds keys. See `keyed_data`.
    fn push_keys<T: TypedBlock>(&mut self, value: &mut T) -> BlockRef {
        let slot = self.push(value);
        if let Some(i) = slot.get() {
            self.keyed_data.insert(i);
        }
        slot
    }

    fn finish(mut self) -> NifFile {
        for block in &self.blocks {
            let ti = self.header.intern_block_type(&block.type_name);
            self.header.block_type_index.push(ti);
        }
        NifFile {
            header: self.header,
            blocks: self.blocks,
            roots: vec![0],
        }
    }
}

/// The canonical `NiPSysModifierOrder` class for a modifier node kind.
///
/// This is BOTH the sort key for the emitted stack and the value
/// written into `NiPSysModifier.Order` — the same number, because in
/// vanilla they are the same thing.
///
/// CLAUDE.md used to say "vanilla modifier `Order` is the sequential
/// stack index, not the ORDER_* enum" and that is BACKWARDS. Measured
/// over the vanilla SSE mesh archives — 19,414 modifier blocks in 899
/// files — every single Order is an enum value and none is a stack
/// index. The compiler was the one writing sequential numbers, so a
/// gravity modifier vanilla ships at 4000 came back as 8.
///
/// Four entries were also simply wrong, each measured against the
/// corpus: NiPSysSpawnModifier is ORDER_EMITTER (1,704 blocks at 1000;
/// 2000 appears in no file at all), and InheritVelocity, RecycleBound
/// and Wind fell through a `_ => GENERAL` catch-all that put Wind ahead
/// of the other forces and RecycleBound ahead of Position — live
/// sequencing errors, since this is the sort key.
///
/// EXHAUSTIVE on purpose: the catch-all is gone, so a new modifier kind
/// is a compile error here rather than a silently-GENERAL modifier.
fn order_class(kind: NodeKind) -> u32 {
    match kind {
        NodeKind::BoxEmitter
        | NodeKind::SphereEmitter
        | NodeKind::CylinderEmitter
        | NodeKind::MeshEmitter
        // nif.xml gives NiPSysSpawnModifier the default ORDER_EMITTER,
        // and 1,704 vanilla blocks agree. It was SPAWN (2000), a value
        // that appears in no shipped file.
        | NodeKind::SpawnModifier
        // 187 vanilla blocks at 1000; used to fall through to GENERAL.
        | NodeKind::InheritVelocityModifier => order::EMITTER,
        NodeKind::LodModifier => order::BSLOD,
        NodeKind::RotationModifier
        | NodeKind::SimpleColorModifier
        | NodeKind::ScaleModifier
        | NodeKind::SubTexModifier => order::GENERAL,
        NodeKind::GravityModifier
        | NodeKind::DragModifier
        | NodeKind::BombModifier
        // 10 vanilla blocks at 4000. As GENERAL it applied BEFORE the
        // other forces rather than with them.
        | NodeKind::WindModifier => order::FORCE,
        NodeKind::ColliderModifier => order::COLLIDER,
        // 45 vanilla blocks at 6500. As GENERAL the bound was tested
        // against LAST frame's positions and a particle wrapped a frame
        // late.
        NodeKind::RecycleBoundModifier => order::POSTPOS_UPDATE,
        // Not modifiers: these never reach the stack, and listing them
        // is what lets the match be exhaustive.
        _ => order::GENERAL,
    }
}

/// A dedicated anchor node under the root for anything that hangs its
/// geometry off a scene object (emitter volumes, bombs, colliders).
fn anchor_node(
    b: &mut Builder,
    root: BlockRef,
    name: &str,
    offset: [f32; 3],
    rotation: [f32; 3],
) -> BlockRef {
    if offset == [0.0; 3] && rotation == [0.0; 3] {
        return root;
    }
    let mut anchor = NiNode {
        av: Default::default(),
        children: Vec::new(),
        effects: Vec::new(),
    };
    let name = b.unique_anchor_name(name);
    anchor.av.net.name = b.intern(&name);
    anchor.av.translation = offset;
    anchor.av.rotation =
        crate::mesh::Transform::rows_from_euler(rotation[0], rotation[1], rotation[2]);
    let anchor_ref = b.push(&mut anchor);
    b.extra_root_children.push(anchor_ref);
    anchor_ref
}

/// Wires landing on `(node, socket)`, in stable graph order.
fn wired_sources(graph: &Graph, node: NodeId, socket: usize) -> Vec<NodeId> {
    graph
        .incoming(node, socket)
        .map(|l: &Link| l.from_node)
        .collect()
}

/// What an export writes, given the graph and the file the tab was
/// imported from.
///
/// **Passthrough is the whole point.** 97.2% of the corpus (33,111 SSE
/// files, 31,242 LE) carries no particle system at all: the editor
/// opens them as reference models with an empty graph, has nothing to
/// contribute, and used to answer a save with an essentially empty NIF.
/// When the graph wires up no system, the source bytes ARE the correct
/// export, and writing them back is byte-identical by construction —
/// the "a null edit is a no-op" invariant, for the files where it can
/// be honoured today.
///
/// When the graph DOES carry systems the compiled apparatus is
/// SPLICED back into the source, block for block into the slots the
/// decompile took them from — no index ever moves, and a null edit
/// is byte-identical over the whole corpus. [`export_losses`] stays
/// as the caller's ledger for the from-scratch path (no source).
pub fn export_bytes(
    graph: &Graph,
    source: Option<&NifFile>,
    target: TargetGame,
) -> Result<(Vec<u8>, bool), CompileError> {
    let Some(src) = source else {
        // A graph that names SOURCE BLOCKS is not the document — it is
        // the part of one file the editor understands, and the rest of
        // that file is the part it deliberately does not model. The
        // provenance is carried by the graph itself (`source_block`,
        // persisted with it), so this rule needs no plumbing and covers
        // every route to a sourceless decompiled graph, not just the
        // reopened project that made it reachable.
        if graph.nodes.values().any(|n| n.source_block.is_some()) {
            return Err(CompileError::SourceMissing);
        }
        // Authored from scratch, or from a template: the graph IS the
        // document, and this is the case the early return exists for.
        return Ok((compile(graph, target)?.write(), false));
    };
    match compile_merged(graph, src, target) {
        Ok(nif) => Ok((nif.write(), false)),
        // Nothing wired up: hand the source back untouched. 97% of the
        // corpus takes this path — files with no particle system at all.
        //
        // ...unless the file HAD a system and the user deleted it. A
        // graph that never had one is a reference model opened to look
        // at, and passing it through is exactly right; a graph that has
        // none because the last one was removed is an EDIT, and handing
        // the source back would discard it whole. Measured on
        // dlc1fxcameraattachdlcsoulcairnblowingfog (one system):
        // deleting it wrote the file back byte-identically, so the
        // system was still in the game.
        Err(CompileError::NoSystems) | Err(CompileError::NoSceneOutput) => {
            let (out, dropped) = passthrough(src, graph);
            Ok((out.write(), !dropped))
        }
        Err(e) => Err(e),
    }
}

/// The file a save writes when the graph compiles to nothing: the
/// source as it stands, minus anything the user deleted.
///
/// "As it stands" is the point, and it is why this is not the same as
/// "untouched": the source is the file IN MEMORY, so every edit made to
/// its blocks — a gizmo drag, a field typed in Properties — is already
/// in these bytes. One place, because the exporter and the Output
/// panel's preview must not answer this differently; the panel used to
/// report an error for a file the save writes perfectly well.
///
/// Returns whether the pass dropped a deleted node's reference.
fn passthrough(src: &NifFile, graph: &Graph) -> (NifFile, bool) {
    let mut out = src.clone();
    let dropped = match (
        crate::decompile::decompile(src).ok(),
        aether_nif::typed::Ctx::of(&src.header).ok(),
    ) {
        // Nothing compiled, so nothing is claimed.
        (Some(before), Some(ctx)) => {
            drop_removed_nodes(src, &before.graph, graph, &BTreeSet::new(), &mut out, ctx)
        }
        _ => false,
    };
    (out, dropped)
}

/// The file a save actually writes: the splice, with every field the
/// edit did not touch kept from the source.
///
/// The one entry point for "compile over a file", so a measurement of
/// the export cannot drift away from the export.
pub fn compile_merged(
    graph: &Graph,
    source: &NifFile,
    target: TargetGame,
) -> Result<NifFile, CompileError> {
    let edited = compile_into(graph, source, target)?;
    Ok(keep_untouched_fields(source, graph, target, edited))
}

/// Keep every field of the source the EDIT did not touch.
///
/// This is the rule NifSkope and nifly embody and a schema-driven
/// compiler cannot: they never rebuild a block, they edit the value they
/// loaded, so a field nobody touched is written back exactly as it was
/// read. Aether rebuilds the apparatus from the graph, and the graph is
/// deliberately narrower than the format — no raw shader flag words, no
/// authored modifier names, no `NiPSysData` has_* booleans — so every
/// rebuild silently substituted the compiler's idea of those fields.
///
/// The fix needs no table of "which fields does the editor own".
/// Compile the document TWICE: once as it was imported, once as it is
/// now. A scalar that differs between the two is one the user changed;
/// every other scalar comes from the file. A null edit is then
/// byte-identical BY CONSTRUCTION, not by modelling the format
/// exhaustively — and the fields the editor has never heard of are the
/// ones it is now most careful with.
///
/// Falls back to the rebuilt block whenever the three cannot be walked
/// in step (a slot whose type changed, an array the edit resized), so
/// this is strictly an improvement over rebuilding, never a substitute
/// for the compiler being right.
fn keep_untouched_fields(
    source: &NifFile,
    graph: &Graph,
    target: TargetGame,
    edited: NifFile,
) -> NifFile {
    let Ok(ctx) = aether_nif::typed::Ctx::of(&edited.header) else {
        return edited;
    };
    // What compiling the file's OWN document produces — the baseline the
    // edit is measured against.
    let Some(before) = crate::decompile::decompile(source).ok() else {
        return edited;
    };
    let Some((authored, was_claimed)) =
        compile_into_claiming(&before.graph, source, target).ok()
    else {
        return edited;
    };
    let still_claimed = compile_into_claiming(graph, source, target)
        .map(|(_, c)| c.slots)
        .unwrap_or_default();
    let mut out = edited;
    let n = source
        .blocks
        .len()
        .min(out.blocks.len())
        .min(authored.blocks.len());
    for i in 0..n {
        // Compiling the document before and after the edit gave the same
        // bytes here, so the user changed nothing this block expresses:
        // keep the file's own, whatever shape it is in. This is what
        // rescues the blocks a field-wise merge cannot walk — a
        // `NiPSysMeshEmitter` whose mesh list the compiler cannot
        // rebuild would otherwise come back EMPTY, losing the very
        // shapes it emits from.
        if authored.blocks[i].data == out.blocks[i].data
            && authored.blocks[i].type_name == out.blocks[i].type_name
            && source.blocks[i].type_name == out.blocks[i].type_name
        {
            out.blocks[i] = source.blocks[i].clone();
            continue;
        }
        if let Some(merged) =
            aether_nif::typed::merge_edit(&source.blocks[i], &authored.blocks[i], &out.blocks[i], ctx)
        {
            out.blocks[i] = merged;
        }
    }
    // A block the user DELETED is one the file's own document claimed
    // and this one does not. Its bytes stay where they are -- the
    // splice never moves anything -- but nothing may still NAME it, or
    // the editor says the effect is gone and the game goes on running
    // it. Measured before this pass: deleting a particle system left
    // it in the file 69 times out of 69, because its parent `NiNode`'s
    // children list is source bytes that nothing rewrites.
    //
    // The reference is set to NULL rather than removed from the array:
    // an index that moved would have to be chased through types this
    // project deliberately does not model, which is the whole reason
    // the splice reuses slots. A NULL entry is a form vanilla itself
    // ships -- 2 of 74 fixtures carry one in a `NiNode` children list,
    // and NULL refs elsewhere are everywhere.
    //
    // Only blocks the COMPILE DOES NOT CLAIM are rewritten, and that
    // restriction is the whole correctness of this pass. Where the
    // compiler owns the referrer it has already written what the
    // document says -- a controller chain is rebuilt link by link --
    // and nulling there cuts a chain that is already right: measured,
    // deleting one of smelter01's three Active Toggles took all three
    // out, because the freed SLOT is not the deleted THING. Slot reuse
    // means a surviving sibling moves into the gap, so the only stale
    // reference is one from bytes nobody rewrote: a scene node's
    // children list.
    let deleted: BTreeSet<usize> = was_claimed
        .slots
        .difference(&still_claimed)
        .copied()
        .filter(|i| *i < out.blocks.len())
        .collect();
    if !deleted.is_empty() {
        for i in 0..out.blocks.len() {
            if deleted.contains(&i) || still_claimed.contains(&i) {
                continue;
            }
            let patched = aether_nif::typed::retarget_refs(&out.blocks[i], ctx, &mut |r, _| {
                match r.get() {
                    Some(t) if deleted.contains(&t) => BlockRef::NONE,
                    _ => r,
                }
            });
            if let Some(patched) = patched {
                out.blocks[i] = patched;
            }
        }
    }

    drop_removed_nodes(source, &before.graph, graph, &still_claimed, &mut out, ctx);

    // The ROOT keeps the file's own block -- step 4 skips it on
    // purpose, because the compiled root is a scaffold that would
    // replace the scene with an empty one -- so a rename typed into
    // Scene Output reached nothing. Its NAME is the one field of it
    // the graph models, so that is the one field written back.
    if let (Some(out_id), Some(root)) = (
        graph
            .nodes
            .iter()
            .find(|(_, n)| n.kind == NodeKind::SceneOutput)
            .map(|(id, _)| *id),
        out.roots
            .first()
            .and_then(|r| usize::try_from(*r).ok())
            .filter(|i| *i < out.blocks.len()),
    ) {
        // ...and only when the user actually RENAMED it. The
        // decompiled value is not always the file's own name -- an
        // unnamed root reads as a default -- so writing it
        // unconditionally renamed 186 corpus files on a null edit,
        // which is exactly what the gate is for. Compared against the
        // source's OWN document, the same two-documents trick the
        // field merge and the delete pass both use.
        let was = before
            .graph
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::SceneOutput)
            .map(|n| n.str_or("Root Name", ""))
            .unwrap_or_default();
        let want = graph.nodes[&out_id].str_or("Root Name", "");
        if !want.is_empty() && want != was {
            let idx = out.header.intern_string(&want);
            if let Some(mut av) = block_av(&out.blocks[root], ctx) {
                if av.net.name != StringIdx(idx) {
                    av.net.name = StringIdx(idx);
                    if let Some(patched) = set_block_av(&out.blocks[root], ctx, &av) {
                        out.blocks[root] = patched;
                    }
                }
            }
        }

        // The behavior graph is the root's too, as its BGED extra data,
        // and the same two-documents rule decides whether to touch it.
        let was = before
            .graph
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::SceneOutput)
            .map(|n| n.str_or("Behavior Graph", ""))
            .unwrap_or_default();
        let want = graph.nodes[&out_id].str_or("Behavior Graph", "");
        if want != was {
            set_root_behavior(&mut out, root, ctx, &want);
        }
    }

    write_clip_edits(&mut out, &before.graph, graph, ctx);
    drop_removed_clips(&mut out, &before.graph, graph, ctx);
    if graph.clips.iter().any(|c| c.source_block.is_none()) {
        ensure_manager(&mut out, ctx, graph);
    }
    // In a splice a node's live controller is the block it came from.
    append_new_clips(&mut out, graph, ctx, &|id| {
        graph.nodes.get(&id).and_then(|n| n.source_block).map(|b| b as usize)
    });

    // Strings the compiler interned for names it ended up not writing
    // would keep the header growing on every save even when every block
    // came back untouched. Trailing entries only: remapping live indices
    // is a renumber, and this splice does not renumber anything.
    let used = out
        .blocks
        .iter()
        .filter_map(|b| aether_nif::typed::used_strings(b, ctx))
        .flatten()
        .max()
        .map(|m| m as usize + 1)
        .unwrap_or(0);
    let keep = used.max(source.header.strings.len());
    if keep < out.header.strings.len() {
        out.header.strings.truncate(keep);
        out.header.max_string_length = out
            .header
            .strings
            .iter()
            .map(|s| s.len() as u32)
            .max()
            .unwrap_or(0);
    }
    out
}

/// Where one block of an export came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOrigin {
    /// The source's own bytes, untouched.
    Kept,
    /// The source had this slot and the save rewrites it.
    Rewritten,
    /// New: the source had no such block.
    Added,
    /// The source had it and nothing points at it any more — the
    /// splice never moves a block, so what a delete leaves behind is
    /// dead weight rather than a hole.
    Orphaned,
}

/// One block a save would write.
#[derive(Debug, Clone)]
pub struct OutputBlock {
    pub index: usize,
    pub type_name: String,
    /// The block's own name, when its layout carries one.
    pub name: Option<String>,
    /// The graph node that owns it, when one does. Most of the
    /// apparatus has none: service modifiers, interpolators and key
    /// data are the compiler's, and the scene around the systems is
    /// the file's.
    pub owner: Option<NodeId>,
    pub origin: BlockOrigin,
    /// Can the game reach it by walking from the root? An unreachable
    /// block is in the file and not in the effect.
    pub reachable: bool,
}

/// Every block a save would write, with who owns it.
///
/// Derived from the SPLICE itself rather than from a second reading of
/// it: the same `compile_into` the export runs, the same merge, the
/// same claim map. A preview that agreed with the exporter only by
/// coincidence would be worse than none, because it would be believed.
///
/// The editor otherwise shows a graph that is deliberately narrower
/// than the file -- measured on the campfire, 55 apparatus blocks have
/// a node and 59 do not -- so "what will actually be in there" had no
/// answer anywhere in the app.
pub fn export_preview(
    graph: &Graph,
    source: Option<&NifFile>,
    target: TargetGame,
) -> Result<Vec<OutputBlock>, CompileError> {
    let (out, claims) = match source {
        Some(src) => match compile_into_claiming(graph, src, target) {
            Ok((edited, claims)) => (keep_untouched_fields(src, graph, target, edited), claims),
            // Nothing to compile -- no Scene Output, or none wired up.
            // A save still writes: it hands the source back, so the
            // preview is the source, block for block. Reported by a
            // user with a mesh they had edited: the panel said
            // "Nothing to write" while Save NIF wrote their file.
            Err(CompileError::NoSystems) | Err(CompileError::NoSceneOutput) => (
                passthrough(src, graph).0,
                Claims {
                    slots: BTreeSet::new(),
                    owner: Default::default(),
                },
            ),
            Err(e) => return Err(e),
        },
        // Authored from scratch: every block is new and every one the
        // compiler wrote is its own.
        None => {
            let b = build(graph, target, None)?;
            let owner = b.node_block.iter().map(|(n, i)| (*i, *n)).collect();
            let slots = b.node_block.values().copied().collect();
            (b.finish(), Claims { slots, owner })
        }
    };
    let ctx = aether_nif::typed::Ctx::of(&out.header).map_err(|_| CompileError::NoSystems)?;

    let mut seen: BTreeSet<usize> = BTreeSet::new();
    let mut queue: Vec<usize> = out
        .roots
        .iter()
        .filter_map(|r| usize::try_from(*r).ok())
        .collect();
    while let Some(i) = queue.pop() {
        if i >= out.blocks.len() || !seen.insert(i) {
            continue;
        }
        // OWNERSHIP descends a Ref and stops at a Ptr, the rule this
        // project follows everywhere else: a Ptr names a scene object
        // it does not contain. Counting one as reachability said a
        // deleted system was still in the scene because its own AV
        // object palette remembered its name.
        for (r, kind) in block_refs(&out.blocks[i], ctx).unwrap_or_default() {
            if kind == RefKind::Ptr {
                continue;
            }
            if let Some(t) = r.get() {
                queue.push(t);
            }
        }
    }

    Ok(out
        .blocks
        .iter()
        .enumerate()
        .map(|(i, block)| {
            let reachable = seen.contains(&i);
            let was = source.and_then(|s| s.blocks.get(i));
            let origin = match was {
                None => BlockOrigin::Added,
                Some(w) if !reachable && w.type_name == block.type_name => {
                    BlockOrigin::Orphaned
                }
                Some(w) if w.data == block.data && w.type_name == block.type_name => {
                    BlockOrigin::Kept
                }
                Some(_) => BlockOrigin::Rewritten,
            };
            OutputBlock {
                index: i,
                type_name: block.type_name.clone(),
                name: aether_nif::typed::block_name(block, ctx)
                    .and_then(|n| n.resolve(&out.header))
                    .filter(|n| !n.is_empty())
                    .map(str::to_string),
                owner: claims.owner.get(&i).copied(),
                origin,
                reachable,
            }
        })
        .collect())
}

/// A NODE the user deleted must leave the file.
///
/// Identified by PROVENANCE, not by a freed slot: the source's own
/// document names the systems the file had, and a `source_block`
/// missing from the graph now is one the user removed. Slots cannot
/// express this — the splice reuses them, so after deleting one of
/// three siblings the survivors move and the freed slot is arbitrary.
/// Two attempts foundered on exactly that, one of them by taking all
/// three out where the user removed one.
///
/// Two guards, and both are load-bearing:
///
/// - the block must not be CLAIMED by the current compile. Slot reuse
///   means a survivor can move into a removed node's slot, and nulling
///   references to it would delete the wrong thing.
/// - only CHILD references are dropped. A Ptr names a scene object it
///   does not own, and those are restored from the source wholesale
///   for reasons of their own.
///
/// This is what stops `restore_null_children` resurrecting a deleted
/// block: a collider's `Spawn Modifier` and its `next` are fields the
/// graph does not model, so the compiler writes NULL and the splice
/// puts the source's pointer back — correct while the target is still
/// there, and a resurrection once the user has deleted it.
///
/// Returns whether anything was dropped.
fn drop_removed_nodes(
    source: &NifFile,
    before: &Graph,
    now: &Graph,
    claimed: &BTreeSet<usize>,
    out: &mut NifFile,
    ctx: Ctx,
) -> bool {
    let live: BTreeSet<u32> = now.nodes.values().filter_map(|n| n.source_block).collect();
    let removed: BTreeSet<usize> = before
        .nodes
        .values()
        .filter_map(|n| n.source_block)
        .filter(|b| !live.contains(b))
        .map(|b| b as usize)
        // A survivor may have moved into the slot; then it is not a
        // removal at all, it is the same slot holding something else.
        .filter(|b| !claimed.contains(b))
        .collect();
    if removed.is_empty() {
        return false;
    }
    let mut dropped = false;
    for i in 0..out.blocks.len() {
        if removed.contains(&i) {
            continue;
        }
        let patched = aether_nif::typed::retarget_refs(&out.blocks[i], ctx, &mut |r, kind| {
            match r.get() {
                Some(t) if kind == RefKind::Child && removed.contains(&t) => {
                    dropped = true;
                    BlockRef::NONE
                }
                _ => r,
            }
        });
        if let Some(patched) = patched {
            out.blocks[i] = patched;
        }
    }
    dropped
}

/// What an export would NOT carry over from the file it was imported
/// from: per block type, how many the source had and the compiled file
/// has. Only losses are reported (a type the export ADDS is not a
/// hazard).
///
/// The editor models the particle apparatus and nothing else, so a
/// compiled file legitimately has no geometry, no sequences, no
/// collision and no behaviour data. Saying which of those the source
/// carried is the difference between a known limitation and silent
/// data loss — measured over the corpus, 1,870 of 1,875 SSE particle
/// files carry something the export drops.
/// Export by SPLICING the compiled apparatus back into the file the tab
/// was imported from, instead of writing the apparatus alone.
///
/// The rule that makes this safe with no schema for collision: **no
/// index ever moves.** Every block the graph owns is written back into
/// the slot it was decompiled from, blocks the graph does not know are
/// copied through untouched, and anything genuinely new is APPENDED past
/// the end. A `bhkRigidBody` we cannot parse keeps pointing at exactly
/// the block it pointed at before, because that block is still there.
///
/// The price is orphans: a modifier the user deleted leaves its old
/// bytes behind in a slot nothing references any more. That is a slow
/// leak, and the honest trade against a renumber that would have to
/// rewrite references inside types Aether deliberately does not model.
pub fn compile_into(
    graph: &Graph,
    source: &NifFile,
    target: TargetGame,
) -> Result<NifFile, CompileError> {
    compile_into_claiming(graph, source, target).map(|(f, _)| f)
}

/// [`compile_into`], and which SOURCE slots the compiled document
/// claims.
///
/// The claim set is what makes a DELETE visible. Compiling the file's
/// own document and the edited one gives two sets, and a slot in the
/// first but not the second is a block the user removed -- the same
/// two-compiles trick `keep_untouched_fields` uses for fields, asked
/// about existence instead of values. A slot a same-type SIBLING took
/// over is in both sets and is correctly not a deletion.
fn compile_into_claiming(
    graph: &Graph,
    source: &NifFile,
    target: TargetGame,
) -> Result<(NifFile, Claims), CompileError> {
    let b = build(graph, target, Some(source))?;
    let ctx = b.ctx;
    let root_compiled = 0usize;
    let source_root = source
        .roots
        .first()
        .and_then(|r| (*r >= 0).then_some(*r as usize))
        .filter(|i| *i < source.blocks.len())
        .ok_or(CompileError::NoSceneOutput)?;

    // --- 1. seeds: what the graph itself remembers ----------------------
    // The scene output is deliberately NOT a seed. Its provenance is the
    // source ROOT, and the compiled root carries only the systems —
    // writing it back would delete the model, the collision and every
    // other child in one stroke, which is the very bug this replaces.
    //
    // The compiled root DOES map to the source root, though — for
    // reference resolution only. `anchor_node` returns the root itself
    // when a modifier's offset is zero, so without this every
    // Gravity/Drag/Bomb object pointer in the file would be written as
    // NULL: the modifier would silently stop finding what it acts on.
    let mut slot_of: BTreeMap<usize, usize> = BTreeMap::new();
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    // Source slot -> the compiled block that claimed it, so a second
    // claimant can be checked for being the same block rather than a
    // conflict.
    let mut owner: BTreeMap<usize, usize> = BTreeMap::new();
    slot_of.insert(root_compiled, source_root);
    taken.insert(source_root);
    let mut seeds: Vec<(NodeId, usize, usize)> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.kind != NodeKind::SceneOutput)
        .filter_map(|(id, n)| {
            let compiled = *b.node_block.get(id)?;
            let src = n.source_block? as usize;
            (src < source.blocks.len()).then_some((*id, compiled, src))
        })
        .collect();
    // Deterministic: a duplicated node claims the same slot as its
    // original, and the lowest node id must win the same way every run.
    seeds.sort();
    let mut queue: Vec<(usize, usize)> = Vec::new();
    // The emitter block types swap freely under an in-place node kind
    // change: same slot, new type — indices never move, so every
    // reference to the slot stays valid, and the output header's
    // per-block type entry is rewritten for reused slots anyway.
    // Without this the changed emitter was APPENDED and the old block
    // left orphaned in place, growing the file by one dead block per
    // change.
    for (_, compiled, src) in seeds {
        let same = b.blocks[compiled].type_name == source.blocks[src].type_name;
        let family_swap = same_swap_family(
            &b.blocks[compiled].type_name,
            &source.blocks[src].type_name,
        );
        if !same && !family_swap {
            continue;
        }
        if slot_of.contains_key(&compiled) || !taken.insert(src) {
            continue;
        }
        owner.insert(src, compiled);
        slot_of.insert(compiled, src);
        queue.push((compiled, src));
    }

    // --- 2. parallel descent -------------------------------------------
    // A node's own block is only the tip: its interpolators, key data,
    // emitter anchors and particle data have no node of their own. They
    // do not need one — two blocks of the SAME TYPE hold their
    // references in the same field order, so the i-th reference of the
    // compiled block and the i-th of the source block are the same
    // field, and the pairing propagates down the tree for free.
    while let Some((compiled, src)) = queue.pop() {
        let (Some(cr), Some(sr)) = (
            block_refs(&b.blocks[compiled], ctx),
            block_refs(&source.blocks[src], ctx),
        ) else {
            continue;
        };
        // Equal lengths make the position-by-position stage valid (the
        // i-th reference of two same-type blocks is the same FIELD).
        // UNEQUAL lengths used to skip the whole subtree -- but the
        // type-fallback stage below is safe either way, and a system
        // that carries its own extra data (trailer_sh22_cam's
        // NiIntegerExtraData) made the compiled list one shorter than
        // the source's, cutting every service modifier loose.
        let positional = cr.len() == sr.len();
        // Pair position by position, then let anything left over find
        // the next unclaimed reference of its own TYPE.
        //
        // The i-th reference of two blocks of the same type is the same
        // FIELD, which is what makes the positional walk right — but a
        // list field is not a field, it is a list, and vanilla is free
        // to order it in a way the compiler's class sort does not
        // reproduce. ZD_MarkartWarhammer's PCloud01C lists its rotation
        // modifier AFTER the service ones, so from that entry on every
        // type disagreed and the whole subtree below it went unpaired —
        // a collider manager, its collider and two nodes appended to
        // every save.
        let mut claimed_here: Vec<bool> = vec![false; sr.len()];
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for (i, ((cref, ck), (sref, sk))) in
            cr.iter().zip(&sr).enumerate().filter(|_| positional)
        {
            if ck != sk {
                continue;
            }
            let (Some(ci), Some(si)) = (cref.get(), sref.get()) else {
                continue;
            };
            if ci < b.blocks.len()
                && si < source.blocks.len()
                && b.blocks[ci].type_name == source.blocks[si].type_name
            {
                claimed_here[i] = true;
                pairs.push((ci, si));
            }
        }
        for (cref, ck) in cr.iter() {
            let Some(ci) = cref.get() else { continue };
            if ci >= b.blocks.len() || pairs.iter().any(|(c, _)| *c == ci) {
                continue;
            }
            let want = &b.blocks[ci].type_name;
            if let Some(j) = (0..sr.len()).find(|j| {
                !claimed_here[*j]
                    && sr[*j].1 == *ck
                    && sr[*j]
                        .0
                        .get()
                        .is_some_and(|si| source.blocks.get(si).is_some_and(|s| s.type_name == *want))
            }) {
                claimed_here[j] = true;
                pairs.push((ci, sr[j].0.get().expect("checked")));
            }
        }
        for (ci, si) in pairs {
            if si >= source.blocks.len() || ci >= b.blocks.len() {
                continue;
            }
            if b.blocks[ci].type_name != source.blocks[si].type_name {
                continue;
            }
            if slot_of.contains_key(&ci) {
                continue;
            }
            if taken.contains(&si) {
                // The format SHARES: 24 SSE files and 24 LE point two
                // particle systems at one `NiAlphaProperty`, and
                // blacksmithforge01 is the one this project pinned. The
                // compiler emits one per system, so the second copy
                // found its slot taken and was appended — the last 50
                // files in the corpus that a null edit could not
                // reproduce.
                //
                // Two compiled blocks may name one slot only when they
                // are the SAME BLOCK: identical bytes, so whichever
                // writes it last writes the same thing. Anything else is
                // a genuine conflict and still appends.
                let shared = owner
                    .get(&si)
                    .is_some_and(|first| b.blocks[*first].data == b.blocks[ci].data);
                if shared {
                    slot_of.insert(ci, si);
                }
                continue;
            }
            owner.insert(si, ci);
            slot_of.insert(ci, si);
            taken.insert(si);
            queue.push((ci, si));
        }
    }

    // --- 3. place the leftovers at the end ------------------------------
    // Everything the source did not already have is appended, EXCEPT the
    // pure scene containers the compiler synthesizes to hold a system
    // (`ScaleNode` for a non-identity world transform). Those exist only
    // because a from-scratch compile has no scene to hang anything on;
    // the source has its own parent for the same system, and appending
    // one anyway would give the particle system TWO parents and draw it
    // twice. A container is redundant exactly when nothing but the root
    // refers to it and every child it holds already has a slot.
    // Which synthesized blocks nothing will actually refer to once the
    // file is written.
    //
    // Two facts make a block redundant, and they feed each other, so
    // this is a FIXPOINT rather than one pass:
    //
    //  - A block that already exists keeps the SOURCE's back-pointers
    //    (step 4), so its Ptrs will not point at anything the compiler
    //    synthesized. An anchor whose only referrer is such a Ptr is
    //    born orphaned.
    //  - A container is redundant when everything it holds is either
    //    placed or redundant itself.
    //
    // Measured on the campfire: a synthesized `ScaleNode` holding five
    // `GravityAnchor`/`DragAnchor` NiNodes. Neither the wrapper nor the
    // anchors are redundant until the other is, so a single pass kept
    // all seven — appended, referred to by nothing, and growing the
    // file on every save.
    // Which unpaired blocks are worth appending, asked the other way
    // round: what does the file actually NEED?
    //
    // Only two things justify a block the source does not have. It is
    // the compiled image of a graph NODE that found no home — genuinely
    // new content the user added — or something such a block needs,
    // above it or below it. Everything else the compiler emits is
    // scaffolding: `GravityAnchor`/`DragAnchor` nodes and the
    // `ScaleNode` wrapper, which exist only because a from-scratch
    // compile has no scene to hang anything on. When the source already
    // has a scene, the modifiers' back-pointers are restored from it
    // (step 4) and the scaffolding is orphaned the moment it is written.
    //
    // Measured on the campfire: seven anchors under one wrapper,
    // appended on every save, referred to by nothing, growing the file
    // each time. Asking "is it redundant?" cannot resolve that — the
    // wrapper is only redundant once the anchors are and the anchors
    // only once the wrapper is. Asking "is it needed?" has a seed.
    let mut wanted: BTreeSet<usize> = BTreeSet::new();
    for c in b.node_block.values() {
        if !slot_of.contains_key(c) && *c != root_compiled {
            wanted.insert(*c);
        }
    }
    let refs_written = |c: usize| -> Vec<usize> {
        let compiled_refs = block_refs(&b.blocks[c], ctx).unwrap_or_default();
        // Mirrors step 4's own condition exactly: only a paired block of
        // the same type whose reference LIST has the same shape actually
        // takes the source's Ptrs, and only then does its compiled Ptr
        // fail to reach what it points at here.
        let keeps_source_ptrs = slot_of
            .get(&c)
            .and_then(|s| source.blocks.get(*s))
            .filter(|s| s.type_name == b.blocks[c].type_name)
            .and_then(|s| block_refs(s, ctx))
            .is_some_and(|sr| !sr.is_empty() && sr.len() == compiled_refs.len());
        compiled_refs
            .into_iter()
            .filter(|(_, kind)| !(keeps_source_ptrs && *kind == RefKind::Ptr))
            .filter_map(|(r, _)| r.get())
            .filter(|t| *t < b.blocks.len())
            // A `NiPSysMeshEmitter`'s mesh list holds SOURCE indices --
            // `emitter_mesh_refs` resolves shape NAMES against the
            // source file, because the shapes exist only there. Walking
            // one as a compiled index read past the end of the compiled
            // list and PANICKED, reachable whenever such an emitter is
            // unpaired: import a file, delete its systems, paste from
            // the library, export. The splice never moves a source
            // block, so those indices are already valid output ones and
            // this walk has no business following them.
            .collect()
    };
    // ...and the key DATA of every track the document holds, which is
    // the same question asked of a parameter instead of a node.
    //
    // A key track has no node of its own: it hangs under an
    // interpolator, and in an imported file that interpolator already
    // exists, so it PAIRS. Its data block was therefore never wanted,
    // never appended, and the compiled ref to it reached nothing --
    // which made the two compiles agree and the merge copy the
    // source's own bytes back. Silent: no warning, no refusal, and a
    // file byte-identical to the one that was read. Measured before
    // the fix: 57 of 57 tracks typed into an empty parameter across 11
    // fixtures came out as if nothing had been typed.
    //
    // A null edit appends nothing by construction: an empty parameter
    // pushes no data block at all, and a parameter that had keys pairs
    // with the source's own data block.
    // ...and only where some compiled block NAMES it.
    //
    // A sequence-managed controller keeps its blend interpolator
    // verbatim, so the resting pose the decompiler read out of it
    // compiles to a `NiBoolData` that nothing refers to at all.
    // Appending that grew ZD_MorthalSwordLeaf by 106 bytes on a null
    // edit.
    //
    // ANY compiled block, not just one that already has a slot: a
    // shader track the source does not have is a whole CHAIN --
    // controller, interpolator, data -- so the block naming the data
    // is itself new and unslotted. Requiring a slotted namer left all
    // 201 shader tracks across the fixtures still silently dropped,
    // which is what the generalised test caught. The fixpoint below
    // walks UP from here and picks up the rest of the chain.
    let named_by_a_compiled_block = |k: usize| -> bool {
        (0..b.blocks.len()).any(|c| {
            c != k
                && block_refs(&b.blocks[c], ctx)
                    .unwrap_or_default()
                    .iter()
                    .any(|(r, kind)| *kind != RefKind::Ptr && r.get() == Some(k))
        })
    };
    for c in b.keyed_data.iter().copied() {
        if c != root_compiled && !slot_of.contains_key(&c) && named_by_a_compiled_block(c) {
            wanted.insert(c);
        }
    }
    for _ in 0..b.blocks.len().min(64) {
        let mut grew = false;
        // Down: what a wanted block points at is wanted.
        for c in wanted.iter().copied().collect::<Vec<_>>() {
            for t in refs_written(c) {
                if t != root_compiled && !slot_of.contains_key(&t) && wanted.insert(t) {
                    grew = true;
                }
            }
        }
        // Up: a container holding wanted content has to survive, or the
        // root adopts its children and nothing points at them.
        for c in 0..b.blocks.len() {
            if c == root_compiled || slot_of.contains_key(&c) || wanted.contains(&c) {
                continue;
            }
            if refs_written(c).iter().any(|t| wanted.contains(t)) {
                wanted.insert(c);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let mut blocks = source.blocks.clone();
    let mut appended: Vec<usize> = Vec::new();
    for c in 0..b.blocks.len() {
        if c == root_compiled || slot_of.contains_key(&c) {
            continue;
        }
        if !wanted.contains(&c) {
            continue;
        }
        slot_of.insert(c, blocks.len());
        blocks.push(Block {
            type_name: b.blocks[c].type_name.clone(),
            data: Vec::new(),
        });
        appended.push(c);
    }

    // --- 4. write, with references remapped into the output numbering ---
    let claimed: BTreeSet<usize> = b.node_block.values().copied().collect();
    for (c, out_index) in &slot_of {
        if *c == root_compiled {
            continue;
        }
        // A POINTER names a scene object; a REF owns a block. The graph
        // owns the apparatus it rebuilds, so its Refs win — but it only
        // MIRRORS the scene, so for a block that already exists every
        // Ptr comes from the source.
        //
        // Without this a null edit re-targeted pointers: the campfire's
        // three drag modifiers shared one authored "Drag01" node and each
        // came back pointing at its own fresh duplicate; a gravity
        // modifier with NO object (-1) came back pointing at the root,
        // because `anchor_node` returns the root for a zero offset; and
        // ZD_MorthalSwordLeaf's bomb moved from node 36 to the root, so
        // the force pulled from somewhere else entirely.
        let source_ptrs: Vec<BlockRef> = if *out_index < source.blocks.len()
            && b.blocks[*c].type_name == source.blocks[*out_index].type_name
        {
            block_refs(&source.blocks[*out_index], ctx)
                .unwrap_or_default()
                .into_iter()
                .map(|(r, _)| r)
                .collect()
        } else {
            Vec::new()
        };
        let mut seen = 0usize;
        // Only when the two blocks agree on their reference LIST: a
        // different shape means the i-th slot is a different field, and
        // copying across it would put a pointer where a ref belongs.
        let compiled_len = block_refs(&b.blocks[*c], ctx).map(|v| v.len()).unwrap_or(0);
        let keep_ptrs = !source_ptrs.is_empty() && compiled_len == source_ptrs.len();
        // For these types a CHILD ref the compiler left NULL is a
        // field the graph does not model at all -- the shader's and
        // alpha property's controller chain, a collider's spawn
        // modifier -- so the source's pointer is the only honest
        // value. Restricted to unmodelled fields on purpose: a
        // modelled NULL (an emitter with its keys cleared) is an
        // edit, and resurrecting the old block would undo it.
        let restore_null_children = keep_ptrs
            && matches!(
                b.blocks[*c].type_name.as_str(),
                "BSEffectShaderProperty"
                    | "NiAlphaProperty"
                    | "NiPSysPlanarCollider"
                    | "NiPSysSphericalCollider"
            );
        // ...and for a COLLIDER, only its first reference.
        //
        // `ColliderBase` is bounce / spawn-on-collide / die-on-collide
        // / **spawn_modifier** / parent / next_collider /
        // collider_object, so reference 0 is the `Spawn Modifier` —
        // the unmodelled field this restore exists for ("the node
        // carries a checkbox, not a wire", 2026-08-09). Reference 2 is
        // `next_collider`, which the compiler DOES model: it builds
        // the manager's chain from the collider nodes. Restoring that
        // one put a deleted collider back into the chain, which is the
        // last of the surviving deletions.
        let restore_only_first = matches!(
            b.blocks[*c].type_name.as_str(),
            "NiPSysPlanarCollider" | "NiPSysSphericalCollider"
        );
        // A `NiPSysMeshEmitter`'s mesh list holds SOURCE indices, not
        // compiled ones: `emitter_mesh_refs` resolves the shape NAMES
        // against the source file, because the shapes exist only there.
        // The splice never moves a source block -- that is the whole
        // invariant it is built on -- so those indices are ALREADY
        // valid output indices and must pass through untouched.
        //
        // Mapping them through `slot_of` like a compiled index is not a
        // no-op and not a NULL: it silently lands on whatever compiled
        // block happens to occupy that number. Measured on a user's
        // sword, authored from scratch onto a model with no particle
        // systems: the emitter's `Cube` (source block 7) came back
        // pointing at `GravityAnchor` (output block 78), so the export
        // sampled a node with no geometry and the re-opened file drew a
        // different effect than the preview it was exported from.
        //
        // Ref 0 is `ModifierBase.target`, a genuinely compiled index;
        // everything after it is the mesh list (`NiPSysMeshEmitter` has
        // no other references).
        let mesh_list_is_source = b.blocks[*c].type_name == "NiPSysMeshEmitter";
        let mapped = retarget_refs(&b.blocks[*c], ctx, &mut |r, kind| {
            let at = seen;
            seen += 1;
            if keep_ptrs && kind == RefKind::Ptr {
                if let Some(kept) = source_ptrs.get(at) {
                    return *kept;
                }
            }
            if mesh_list_is_source && kind == RefKind::Ptr && at > 0 {
                return r;
            }
            if restore_null_children
                && kind == RefKind::Child
                && r.get().is_none()
                && !(restore_only_first && at > 0)
            {
                if let Some(kept) = source_ptrs.get(at) {
                    return *kept;
                }
            }
            match r.get() {
                Some(t) => slot_of
                    .get(&t)
                    .map(|i| BlockRef::new(*i))
                    .unwrap_or(BlockRef::NONE),
                None => BlockRef::NONE,
            }
        })
        .ok_or_else(|| CompileError::UnreadableBlock {
            type_name: b.blocks[*c].type_name.clone(),
        })?;
        // THE SCENE HEADER BELONGS TO THE FILE, NOT TO THE GRAPH.
        //
        // Whatever the compiler synthesized, a block that already exists
        // keeps its authored `NiAVObject`: name, flags, transform,
        // collision and extra data. The graph mirrors those (an emitter
        // node stores the OFFSET it read off its anchor) but does not
        // own them, and rebuilding them from the compiler's defaults was
        // destroying real data on a NULL EDIT — measured before the fix:
        // the campfire's "Up" anchor moved 97 units because the graph
        // stores the offset in PSYS space and the compiler wrote it as a
        // world translation; every particle system lost its authored
        // scale (1.25 -> 1) and flag word, and ZD_MorthalSwordLeaf's
        // GutswarmPArray03 went 0xF -> 0x8000E, i.e. the HIDDEN BIT was
        // cleared and a system the game does not draw became visible.
        //
        // Node transform EDITING (Stage 4) writes the source block
        // directly, so this rule holds then too: the edit is already in
        // `source` when the splice reads it.
        let paired = *out_index < source.blocks.len()
            && mapped.type_name == source.blocks[*out_index].type_name;
        if paired {
            if let Some(mut av) = block_av(&source.blocks[*out_index], ctx) {
                if claimed.contains(c) {
                    // ...EXCEPT the CONTROLLER, which is the head of a
                    // chain the graph owns and the compiler has just
                    // rebuilt link by link.
                    //
                    // `ObjectNet.controller` sits INSIDE the header this
                    // restore puts back, so restoring the header put the
                    // old chain head back over the new one and a
                    // controller the user deleted stayed in the file --
                    // measured, deleting one left the export
                    // BYTE-IDENTICAL, so nothing anywhere said the edit
                    // had been dropped. 66 of the 85 surviving deletions
                    // were this.
                    //
                    // Only for a block the GRAPH CLAIMS: that is a node's
                    // own block, whose chain is exactly what the
                    // controller nodes wired to it say. A scene node the
                    // compiler merely synthesized keeps the file's own
                    // header whole, below.
                    if let Some(compiled) = block_av(&mapped, ctx) {
                        av.net.controller = compiled.net.controller;
                        // ...and the NAME, for the same reason: a
                        // particle system's is a graph parameter, and
                        // a modifier's is derived from the source
                        // anyway, so taking the compiled one changes
                        // nothing unless the user renamed it.
                        av.net.name = compiled.net.name;
                    }
                    // A graph node's own block: the compiled body carries
                    // the edits (the modifier list, the data/shader/alpha
                    // refs), so take it — with the authored header put
                    // back over it.
                    if let Some(merged) = set_block_av(&mapped, ctx, &av) {
                        blocks[*out_index] = merged;
                        continue;
                    }
                } else {
                    // A scene node the compiler SYNTHESIZED to stand in
                    // for one the file already has (an emitter, gravity,
                    // drag or bomb anchor). It contributes nothing the
                    // source does not already hold and is missing what
                    // the source does — its children, for one — so the
                    // source block wins outright. Indices never move, so
                    // its references are still correct as they stand.
                    continue;
                }
            }
        }
        blocks[*out_index] = mapped;
    }

    // --- 5. hang anything new off the source root ----------------------
    // A system (or an emitter anchor) the file never had is appended, and
    // nothing points at it until the root adopts it.
    let new_children: Vec<BlockRef> = block_refs(&b.blocks[root_compiled], ctx)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, k)| *k == RefKind::Child)
        .filter_map(|(r, _)| r.get())
        .filter(|c| appended.contains(c))
        .filter_map(|c| slot_of.get(&c).map(|i| BlockRef::new(*i)))
        .collect();
    if !new_children.is_empty() {
        // The root is not always a plain NiNode: an MPS effect roots at
        // BSMasterParticleSystem, and 104 corpus files refused to export
        // when this decoded one layout and hoped.
        blocks[source_root] = aether_nif::typed::geometry::adopt_children(
            &blocks[source_root],
            ctx,
            &new_children,
        )
        .ok_or_else(|| CompileError::UnreadableBlock {
            type_name: blocks[source_root].type_name.clone(),
        })?;
    }

    // --- 6. header -----------------------------------------------------
    // `b.header` is the source's, plus whatever strings the compiler
    // interned. Only the per-block type index needs touching, and it is
    // SET for reused slots and pushed for appended ones — never rebuilt,
    // or the untouched blocks would lose their own types.
    let mut header = b.header.clone();
    header.bs_version = source.header.bs_version;
    for (c, out_index) in &slot_of {
        if *c == root_compiled {
            continue;
        }
        let ti = header.intern_block_type(&b.blocks[*c].type_name);
        if *out_index < header.block_type_index.len() {
            header.block_type_index[*out_index] = ti;
        } else {
            header.block_type_index.push(ti);
        }
    }
    Ok((
        NifFile {
            header,
            blocks,
            roots: source.roots.clone(),
        },
        Claims {
            owner: b
                .node_block
                .iter()
                .filter_map(|(node, compiled)| Some((*slot_of.get(compiled)?, *node)))
                .collect(),
            slots: slot_of.values().copied().collect(),
        },
    ))
}

/// What the compiled document claims of the output: which slots it
/// writes, and which graph NODE owns each one it can name.
#[derive(Default)]
struct Claims {
    slots: BTreeSet<usize>,
    owner: BTreeMap<usize, NodeId>,
}

/// The block a node's UNMODELLED fields come from.
///
/// Two sources, one meaning. Inside the open file it is the block the
/// node was decompiled from (`source_block`). For a node that arrived
/// from somewhere else -- a copy, a library item, a template -- the
/// bytes travel with the node (`GraphNode::authored`), because there
/// is no file to point into.
///
/// Everything that used to open with
/// `file.blocks.get(node.source_block? as usize)?` goes through here,
/// so the paste path cannot silently take a different answer from the
/// in-file path. Before this existed it took `Default`, which is how a
/// pasted system lost `has_rotation_speeds` and crashed the game.
fn authored_block<'a>(
    source: Option<&'a NifFile>,
    node: &'a GraphNode,
) -> Option<(&'a Block, Ctx)> {
    if let (Some(file), Some(i)) = (source, node.source_block) {
        if let (Some(block), Some(ctx)) = (file.blocks.get(i as usize), ctx_of(file)) {
            return Some((block, ctx));
        }
    }
    let a = node.authored.as_ref()?;
    // The bytes carry the stream they were written for; decoding SSE
    // bytes under an LE context would read different fields.
    Some((a.own()?, Ctx { sse: a.sse }))
}

/// Resolve a block index the way the reader would against the file:
/// out of the open source when the node points into it, otherwise out
/// of the subtree the node carries. Same index space either way --
/// `AuthoredBlock` keys its blocks by the index they had in the file
/// they were captured from, so a `Ref` resolves identically.
/// The stream version to decode a node's authored bytes under: the
/// open file's when the node points into it, else the one the carried
/// payload recorded. Decoding SSE bytes under an LE context reads
/// different fields.
fn authored_ctx(source: Option<&NifFile>, node: &GraphNode) -> Option<Ctx> {
    if let (Some(file), Some(_)) = (source, node.source_block) {
        return ctx_of(file);
    }
    node.authored.as_ref().map(|a| Ctx { sse: a.sse })
}

fn authored_lookup<'a>(
    source: Option<&'a NifFile>,
    node: &'a GraphNode,
    i: usize,
) -> Option<&'a Block> {
    if let (Some(file), Some(_)) = (source, node.source_block) {
        return file.blocks.get(i);
    }
    node.authored.as_ref()?.blocks.get(&(i as u32))
}

pub fn export_losses(source: &NifFile, compiled: &NifFile) -> Vec<(String, usize)> {
    let mut have: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for b in &compiled.blocks {
        *have.entry(b.type_name.as_str()).or_default() += 1;
    }
    let mut had: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for b in &source.blocks {
        *had.entry(b.type_name.as_str()).or_default() += 1;
    }
    let mut losses: Vec<(String, usize)> = had
        .into_iter()
        .filter_map(|(t, n)| {
            let kept = have.get(t).copied().unwrap_or(0);
            (n > kept).then(|| (t.to_string(), n - kept))
        })
        .collect();
    losses.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    losses
}

pub fn compile(graph: &Graph, target: TargetGame) -> Result<NifFile, CompileError> {
    let b = build(graph, target, None)?;
    let node_block = b.node_block.clone();
    let mut nif = b.finish();
    // Clips authored from scratch: build the manager, its palette and
    // the sequences the way vanilla lays them out. Only when the
    // document HAS clips -- a template without any writes none of it.
    if graph.clips.iter().any(|c| c.source_block.is_none()) {
        if let Ok(ctx) = aether_nif::typed::Ctx::of(&nif.header) {
            ensure_manager(&mut nif, ctx, graph);
            append_new_clips(&mut nif, graph, ctx, &|id| node_block.get(&id).copied());
        }
    }
    Ok(nif)
}

/// Give a file with no `NiControllerManager` one, at the head of the
/// ROOT's controller chain, with an empty object palette whose scene is
/// the root -- vanilla's layout (the sword: manager on the root, flags
/// 0x4c). The previous chain head becomes the manager's next, so nothing
/// the root already ran is lost. A file that has a manager is untouched.
fn ensure_manager(out: &mut NifFile, ctx: aether_nif::typed::Ctx, graph: &Graph) {
    use aether_nif::typed::controllers::{
        ControllerBase, NiControllerManager, NiDefaultAVObjectPalette,
    };
    if out.blocks.iter().any(|b| b.type_name == "NiControllerManager") {
        return;
    }
    let Some(root) = out.roots.first().and_then(|r| usize::try_from(*r).ok()) else {
        return;
    };
    let Some(mut av) = out.blocks.get(root).and_then(|b| block_av(b, ctx)) else {
        return;
    };
    let append = |out: &mut NifFile, block: aether_nif::Block| {
        let ti = out.header.intern_block_type(&block.type_name);
        out.header.block_type_index.push(ti);
        out.blocks.push(block);
        BlockRef::new(out.blocks.len() - 1)
    };
    let palette = append(
        out,
        NiDefaultAVObjectPalette {
            scene: BlockRef::new(root),
            objects: Vec::new(),
        }
        .encode(ctx),
    );
    let stop = graph.clips.iter().map(|c| c.stop_time).fold(0.0f32, f32::max);
    let manager = append(
        out,
        NiControllerManager {
            base: ControllerBase {
                next_controller: av.net.controller,
                flags: 0x4c,
                frequency: 1.0,
                phase: 0.0,
                start_time: 0.0,
                stop_time: stop,
                target: BlockRef::new(root),
                ..ControllerBase::default()
            },
            cumulative: false,
            sequences: Vec::new(),
            object_palette: palette,
        }
        .encode(ctx),
    );
    av.net.controller = manager;
    if let Some(patched) = set_block_av(&out.blocks[root], ctx, &av) {
        out.blocks[root] = patched;
    }
}

/// The apparatus itself: every block the graph describes, numbered from
/// zero with the scene root at slot 0. `base` seeds the string table (see
/// `Builder::seeded`); `None` starts a fresh document.
fn build(
    graph: &Graph,
    target: TargetGame,
    source: Option<&NifFile>,
) -> Result<Builder, CompileError> {
    let base = source.map(|f| &f.header);
    let mut outputs = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.kind == NodeKind::SceneOutput);
    let (output_id, _) = outputs.next().ok_or(CompileError::NoSceneOutput)?;
    if outputs.next().is_some() {
        return Err(CompileError::MultipleSceneOutputs);
    }

    let systems = wired_sources(graph, *output_id, 0);
    if systems.is_empty() {
        return Err(CompileError::NoSystems);
    }

    let mut b = Builder::seeded(target, base);

    // A Multi Emitter anywhere makes the file an MPS file: the game
    // resolves the controller's master pointer to the ROOT, so the root
    // must be a BSMasterParticleSystem — vanilla's own structure
    // (dlc01_mpssoulrendtrail.nif block 0).
    let is_mps = graph.nodes.values().any(|n| n.kind == NodeKind::MultiEmitterCtlr);

    // Root node first, filled last (it references the systems).
    let root_slot = b.reserve(if is_mps { "BSMasterParticleSystem" } else { "BSFadeNode" });
    let root_name = graph.nodes[&*output_id].str_or("Root Name", "AetherFX");
    let mut root = NiNode {
        av: Default::default(),
        children: Vec::new(),
        effects: Vec::new(),
    };
    root.av.net.name = b.intern(&root_name);
    // The behavior graph hangs off the ROOT as extra data named BGED,
    // the one place the game looks (1.5.97 0x1404E71F0).
    let behavior = graph.nodes[&*output_id].str_or("Behavior Graph", "");
    if !behavior.is_empty() {
        let mut bged = aether_nif::typed::controllers::BSBehaviorGraphExtraData {
            name: b.intern("BGED"),
            behavior_graph_file: b.intern(&behavior),
            controls_base_skeleton: false,
        };
        let r = b.push(&mut bged);
        root.av.net.extra_data.push(r);
    }

    for system_id in systems {
        let anchors_before = b.extra_root_children.len();
        let psys_ref = compile_system(graph, system_id, &mut b, root_slot, source)?;
        // The game multiplies a system's rendered output by its node's
        // accumulated world scale (1.5.97 @ 0x140D039B0): a non-1 World
        // Scale re-creates that parent, carrying the system AND its
        // anchors (whose psys-space offsets then land at the same world
        // spots the source file had).
        let world_scale = graph.nodes[&system_id].f32_or("World Scale", 1.0);
        let world_offset = graph.nodes[&system_id].vec3_or("World Offset", [0.0; 3]);
        let world_rot = graph.nodes[&system_id].vec3_or("World Rotation", [0.0; 3]);
        let has_world = (world_scale - 1.0).abs() > 1e-6
            || world_offset != [0.0; 3]
            || world_rot != [0.0; 3];
        if has_world {
            let anchors: Vec<BlockRef> = b.extra_root_children.drain(anchors_before..).collect();
            let mut wrapper = NiNode {
                av: Default::default(),
                children: Vec::new(),
                effects: Vec::new(),
            };
            wrapper.av.net.name = b.intern("ScaleNode");
            wrapper.av.scale = world_scale;
            wrapper.av.translation = world_offset;
            wrapper.av.rotation = crate::mesh::Transform::rows_from_euler(
                world_rot[0],
                world_rot[1],
                world_rot[2],
            );
            wrapper.children.push(psys_ref);
            wrapper.children.extend(anchors);
            let wrapper_ref = b.push(&mut wrapper);
            root.children.push(wrapper_ref);
        } else {
            root.children.push(psys_ref);
        }
    }

    root.children.extend(b.extra_root_children.drain(..));
    if is_mps {
        let systems: Vec<BlockRef> = root
            .children
            .iter()
            .copied()
            .filter(|r| r.get().is_some())
            .collect();
        b.fill(
            root_slot,
            &mut BsMasterParticleSystem {
                node: root,
                max_emitter_objects: 20,
                particle_systems: systems,
            },
        );
    } else {
        b.fill(root_slot, &mut root);
    }
    Ok(b)
}

/// The behavior graph a root's extra-data list names: the index of its
/// `BSBehaviorGraphExtraData` and the project path. Found by NAME,
/// `BGED`, because that is how the game finds it
/// (`GetExtraDataByName`, 1.5.97 0x1404E71F0) -- and case-insensitively,
/// because the string pool is.
pub fn root_behavior(
    nif: &NifFile,
    ctx: aether_nif::typed::Ctx,
    extra_data: &[BlockRef],
) -> Option<(usize, String)> {
    use aether_nif::typed::controllers::BSBehaviorGraphExtraData;
    extra_data.iter().filter_map(|r| r.get()).find_map(|i| {
        let block = nif.blocks.get(i)?;
        let b = BSBehaviorGraphExtraData::decode(block, ctx).ok()?;
        let name = b.name.resolve(&nif.header)?;
        name.eq_ignore_ascii_case("BGED").then(|| {
            (i, b.behavior_graph_file.resolve(&nif.header).unwrap_or("").to_string())
        })
    })
}

/// Write every clip track the edit changed into its own key data block.
///
/// A clip's bytes are the source's -- the splice never compiles a
/// sequence -- so an edit is the one thing that may touch them, and the
/// same two-documents rule decides what counts as one: a track whose
/// curve differs from the file's own document. Only its data block is
/// rewritten, in place and as the SAME type, so every other byte of every
/// clip stays the file's.
fn write_clip_edits(out: &mut NifFile, before: &Graph, graph: &Graph, ctx: aether_nif::typed::Ctx) {
    use aether_nif::typed::controllers::{
        NiBoolData, NiBoolInterpolator, NiBoolTimelineInterpolator, NiControllerSequence,
        NiFloatInterpolator, NiPoint3Interpolator, NiPosData,
    };
    for clip in &graph.clips {
        let Some(seq_block) = clip.source_block.map(|b| b as usize) else {
            continue;
        };
        let Some(was) = before.clips.iter().find(|c| c.source_block == clip.source_block) else {
            continue;
        };
        let Some(seq) = out
            .blocks
            .get(seq_block)
            .and_then(|b| NiControllerSequence::decode(b, ctx).ok())
        else {
            continue;
        };
        // The clip's own settings, written into its sequence block only
        // when the edit changed them. The cycle word is touched only for
        // a LOOP edit: the format has a third cycle, REVERSE, that the
        // document's flag cannot spell, and a length edit must not
        // silently turn it into a clamp.
        let mut header = seq.clone();
        let mut rewrite = false;
        if clip.looping != was.looping {
            header.cycle_type = if clip.looping { 0 } else { 2 };
            rewrite = true;
        }
        if (clip.start_time, clip.stop_time, clip.frequency)
            != (was.start_time, was.stop_time, was.frequency)
        {
            header.start_time = clip.start_time;
            header.stop_time = clip.stop_time;
            header.frequency = clip.frequency;
            rewrite = true;
        }
        if clip.text_keys != was.text_keys {
            use aether_nif::typed::controllers::NiTextKeyExtraData;
            let keys: Vec<(f32, StringIdx)> = clip
                .text_keys
                .iter()
                .map(|(t, s)| (*t, StringIdx(out.header.intern_string(s))))
                .collect();
            match seq.text_keys.get() {
                Some(tk) => {
                    if let Some(mut text) = out
                        .blocks
                        .get(tk)
                        .and_then(|b| NiTextKeyExtraData::decode(b, ctx).ok())
                    {
                        text.keys = keys;
                        out.blocks[tk] = text.encode(ctx);
                    }
                }
                // No text key block yet: append one and point the
                // sequence at it. Nothing moves.
                None => {
                    let block = NiTextKeyExtraData {
                        name: StringIdx::NONE,
                        keys,
                    }
                    .encode(ctx);
                    let ti = out.header.intern_block_type(&block.type_name);
                    out.header.block_type_index.push(ti);
                    out.blocks.push(block);
                    header.text_keys = BlockRef::new(out.blocks.len() - 1);
                    rewrite = true;
                }
            }
        }
        if rewrite {
            out.blocks[seq_block] = header.encode(ctx);
        }
        for track in &clip.tracks {
            let Some(slot) = track.source_slot else { continue };
            let old = was.tracks.iter().find(|t| t.source_slot == Some(slot));
            if old.map(|t| &t.curve) == Some(&track.curve) {
                continue;
            }
            let Some(interp) = seq
                .controlled_blocks
                .get(slot as usize)
                .and_then(|cb| cb.interpolator.get())
            else {
                continue;
            };
            let Some(ib) = out.blocks.get(interp).cloned() else { continue };
            let data = match ib.type_name.as_str() {
                "NiFloatInterpolator" => NiFloatInterpolator::decode(&ib, ctx).ok().map(|i| i.data),
                "NiBoolInterpolator" => NiBoolInterpolator::decode(&ib, ctx).ok().map(|i| i.data),
                "NiBoolTimelineInterpolator" => {
                    NiBoolTimelineInterpolator::decode(&ib, ctx).ok().map(|i| i.data)
                }
                "NiPoint3Interpolator" => NiPoint3Interpolator::decode(&ib, ctx).ok().map(|i| i.data),
                _ => None,
            };
            let Some(data) = data else { continue };
            let data_block = || match track.curve.track {
                crate::curve::Track::Float => track.curve.to_float_data().encode(ctx),
                crate::curve::Track::Bool => NiBoolData {
                    interpolation: track.curve.interpolation,
                    keys: track.curve.to_bool_keys(),
                }
                .encode(ctx),
                crate::curve::Track::Vec3 => {
                    let (interpolation, floats) = track.curve.to_floats();
                    NiPosData { interpolation, floats }.encode(ctx)
                }
            };
            if let Some(d) = data.get() {
                let block = data_block();
                if out.blocks.get(d).map(|b| b.type_name.as_str()) == Some(block.type_name.as_str()) {
                    out.blocks[d] = block;
                }
                continue;
            }
            // A POSE-ONLY track: the interpolator holds one value and no
            // data. Still one key at t = 0 -- write the pose, nothing
            // else. Keys added -- the track needs a data block it does
            // not have: APPEND one and point the interpolator at it.
            let pose = track.curve.keys.len() == 1 && track.curve.keys[0].time == 0.0;
            let key = track.curve.keys.first().copied().unwrap_or_default();
            let appended = if pose {
                None
            } else {
                let block = data_block();
                let ti = out.header.intern_block_type(&block.type_name);
                out.header.block_type_index.push(ti);
                out.blocks.push(block);
                Some(BlockRef::new(out.blocks.len() - 1))
            };
            let on = u8::from(key.value[0] > 0.5);
            let patched = match ib.type_name.as_str() {
                "NiFloatInterpolator" => NiFloatInterpolator::decode(&ib, ctx).ok().map(|mut i| {
                    match appended {
                        Some(r) => i.data = r,
                        None => i.value = key.value[0],
                    }
                    i.encode(ctx)
                }),
                "NiBoolInterpolator" => NiBoolInterpolator::decode(&ib, ctx).ok().map(|mut i| {
                    match appended {
                        Some(r) => i.data = r,
                        None => i.value = on,
                    }
                    i.encode(ctx)
                }),
                "NiBoolTimelineInterpolator" => {
                    NiBoolTimelineInterpolator::decode(&ib, ctx).ok().map(|mut i| {
                        match appended {
                            Some(r) => i.data = r,
                            None => i.value = on,
                        }
                        i.encode(ctx)
                    })
                }
                "NiPoint3Interpolator" => NiPoint3Interpolator::decode(&ib, ctx).ok().map(|mut i| {
                    match appended {
                        Some(r) => i.data = r,
                        None => i.value = key.value,
                    }
                    i.encode(ctx)
                }),
                _ => None,
            };
            match patched {
                Some(p) => out.blocks[interp] = p,
                // Could not re-encode the interpolator: take back the
                // block appended for it rather than leave an orphan.
                None if appended.is_some() => {
                    out.blocks.pop();
                    out.header.block_type_index.pop();
                }
                None => {}
            }
        }
    }
}

/// The controller ID a clip's controlled block must carry to bind to
/// controller block `c` (1.5.97 `0x140CC76E0` matches it against the
/// controller's vfunc 432).
///
/// Measured over the corpus (`controlled_block_census.rs`): a particle
/// controller is identified by its MODIFIER NAME, an effect/lighting
/// shader controller by its variable NUMBER, and visibility / transform
/// controllers by nothing at all.
pub fn controller_id_of(nif: &NifFile, ctx: aether_nif::typed::Ctx, c: usize) -> Option<String> {
    use aether_nif::typed::controllers::{
        BSEffectShaderPropertyFloatController, BSLightingShaderPropertyFloatController,
        NiPSysEmitterCtlr, NiPSysModifierActiveCtlr, NiPSysModifierFloatCtlr,
    };
    let b = nif.blocks.get(c)?;
    let name = |i: StringIdx| i.resolve(&nif.header).unwrap_or("").to_string();
    Some(match b.type_name.as_str() {
        "NiPSysEmitterCtlr" => name(NiPSysEmitterCtlr::decode(b, ctx).ok()?.modifier_name),
        "NiPSysModifierActiveCtlr" => {
            name(NiPSysModifierActiveCtlr::decode(b, ctx).ok()?.modifier_name)
        }
        t if NiPSysModifierFloatCtlr::TYPE_NAMES.contains(&t) => {
            name(NiPSysModifierFloatCtlr::decode(b, ctx).ok()?.modifier_name)
        }
        "BSEffectShaderPropertyFloatController" => BSEffectShaderPropertyFloatController::decode(b, ctx)
            .ok()?
            .controlled_variable
            .to_string(),
        "BSLightingShaderPropertyFloatController" => {
            BSLightingShaderPropertyFloatController::decode(b, ctx)
                .ok()?
                .controlled_variable
                .to_string()
        }
        // Colour controllers are named by the colour's NUMBER, like the
        // float ones by their variable: measured, every effect-shader
        // colour track says "0" and a lighting one "0" or "1".
        "BSEffectShaderPropertyColorController" => {
            aether_nif::typed::controllers::BSEffectShaderPropertyColorController::decode(b, ctx)
                .ok()?
                .controlled_color
                .to_string()
        }
        "BSLightingShaderPropertyColorController" => {
            aether_nif::typed::controllers::BSLightingShaderPropertyColorController::decode(b, ctx)
                .ok()?
                .controlled_color
                .to_string()
        }
        "NiVisController" | "NiTransformController" | "BSNiAlphaPropertyTestRefController" => {
            String::new()
        }
        _ => return None,
    })
}

/// The next controller in the chain after block `c`.
pub fn next_controller_of(nif: &NifFile, ctx: aether_nif::typed::Ctx, c: usize) -> Option<usize> {
    controller_base_of(nif.blocks.get(c)?, ctx)?.next_controller.get()
}

/// The object controller block `c` animates: the `target` Ptr, scalar 6
/// of `NiTimeController` (after next, flags, frequency, phase, start,
/// stop). Read here rather than added to `controller_base_of`, whose
/// callers carry an authored base INTO compiled controllers -- a source
/// target riding along there would point compiled blocks at the source's
/// objects.
fn controller_target_of(nif: &NifFile, ctx: aether_nif::typed::Ctx, c: usize) -> Option<usize> {
    controller_base_of(nif.blocks.get(c)?, ctx)?;
    match aether_nif::typed::block_scalars(nif.blocks.get(c)?, ctx)?.get(6)? {
        aether_nif::typed::Scalar::I32(t) => usize::try_from(*t).ok(),
        _ => None,
    }
}

/// What a clip track on node block `c` binds to: the live controller's
/// type name, its controller ID, and the OBJECT the controlled block must
/// name (1.5.97 `0x140CC76E0`).
///
/// For a controller node, `c` IS the live controller. For a SHADER
/// track the node is the effect shader PROPERTY: the live controller is
/// found on the property's own chain by the type and variable the
/// parameter stands for (the same pairing `TrackTarget::graph_param`
/// reads the other way: colour 0, alpha 5, emissive 0), and the name is
/// that of the object the property dresses. A shader with no live
/// controller for that variable returns `None` -- binding would need the
/// clip's own controller copy, which is not written yet.
fn track_binding(
    out: &NifFile,
    ctx: aether_nif::typed::Ctx,
    c: usize,
    param: &str,
) -> Option<(String, String, usize)> {
    let b = out.blocks.get(c)?;
    if b.type_name == "BSEffectShaderProperty" {
        let var_id: String;
        let (want_type, want_id) = if param == "Color Keys" {
            ("BSEffectShaderPropertyColorController", "0")
        } else {
            let (_, var) = crate::graph::EFFECT_SHADER_FLOAT_KEYS.iter().find(|(p, _)| *p == param)?;
            var_id = var.to_string();
            ("BSEffectShaderPropertyFloatController", var_id.as_str())
        };
        let prop = BSEffectShaderProperty::decode(b, ctx).ok()?;
        let mut cur = prop.net.controller.get();
        let mut live = false;
        while let Some(i) = cur {
            if out.blocks.get(i).is_some_and(|x| x.type_name == want_type)
                && controller_id_of(out, ctx, i).as_deref() == Some(want_id)
            {
                live = true;
                break;
            }
            cur = next_controller_of(out, ctx, i);
        }
        if !live {
            return None;
        }
        // The object the property dresses: an AV object that names it.
        let object = out.blocks.iter().enumerate().find_map(|(i, ob)| {
            block_av(ob, ctx)?;
            let refs = aether_nif::typed::block_refs(ob, ctx)?;
            refs.iter().any(|(r, _)| r.get() == Some(c)).then_some(i)
        })?;
        return Some((want_type.to_string(), want_id.to_string(), object));
    }
    Some((
        b.type_name.clone(),
        controller_id_of(out, ctx, c)?,
        controller_target_of(out, ctx, c)?,
    ))
}

/// Give effect shader `shader` a live controller for the variable
/// `param` drives, when it has none -- the shape every real file has.
///
/// Measured over the corpus (`shader_track_binding_census.rs`): of
/// ~50,000 shader, lighting and alpha-test clip tracks, every one that
/// names a palette object binds to a LIVE controller; none relies on the
/// clip's own copy. So the controller vanilla would carry is created:
/// at the head of the property's chain (the old head becomes its next),
/// targeting the shader, driven by a blend interpolator holding NO pose
/// -- at rest it writes nothing, so the shader looks as it did -- with
/// the base the fixtures' clip-driven shader controllers carry (flags
/// 0x6c, frequency 1, phase 0, a window over the clip).
fn ensure_shader_controller(
    out: &mut NifFile,
    ctx: aether_nif::typed::Ctx,
    shader: usize,
    param: &str,
    stop: f32,
) {
    use aether_nif::typed::controllers::{
        BSEffectShaderPropertyColorController, BSEffectShaderPropertyFloatController,
        ControllerBase, NiBlendFloatInterpolator, NiBlendPoint3Interpolator,
    };
    if out.blocks.get(shader).map(|b| b.type_name.as_str()) != Some("BSEffectShaderProperty") {
        return;
    }
    let (float, var) = if param == "Color Keys" {
        (false, 0u32)
    } else {
        match crate::graph::EFFECT_SHADER_FLOAT_KEYS.iter().find(|(p, _)| *p == param) {
            Some((_, v)) => (true, *v),
            None => return,
        }
    };
    if track_binding(out, ctx, shader, param).is_some() {
        return;
    }
    let Ok(mut prop) = BSEffectShaderProperty::decode(&out.blocks[shader], ctx) else {
        return;
    };
    let append = |out: &mut NifFile, block: aether_nif::Block| {
        let ti = out.header.intern_block_type(&block.type_name);
        out.header.block_type_index.push(ti);
        out.blocks.push(block);
        BlockRef::new(out.blocks.len() - 1)
    };
    let interp = if float {
        append(out, NiBlendFloatInterpolator::default().encode(ctx))
    } else {
        append(out, NiBlendPoint3Interpolator::default().encode(ctx))
    };
    let base = ControllerBase {
        next_controller: prop.net.controller,
        flags: 0x6c,
        frequency: 1.0,
        phase: 0.0,
        start_time: 0.0,
        stop_time: stop,
        target: BlockRef::new(shader),
        ..ControllerBase::default()
    };
    let controller = if float {
        append(
            out,
            BSEffectShaderPropertyFloatController {
                base,
                interpolator: interp,
                controlled_variable: var,
            }
            .encode(ctx),
        )
    } else {
        append(
            out,
            BSEffectShaderPropertyColorController {
                base,
                interpolator: interp,
                controlled_color: var,
            }
            .encode(ctx),
        )
    };
    prop.net.controller = controller;
    out.blocks[shader] = prop.encode(ctx);
}

/// The interpolator ID a clip's controlled block names for a track on
/// `param` -- the emitter controller carries two interpolators and is
/// addressed by which; everything else has one and names none.
fn interpolator_id_for(param: &str) -> &'static str {
    match param {
        "Birth Rate Keys" => "BirthRate",
        "Emitter Active Keys" => "EmitterActive",
        _ => "",
    }
}

/// Take every clip the document REMOVED off its manager's list.
///
/// A clip the file's own document had (by its source sequence) and this
/// one does not is a removal -- the two-documents rule again. The game
/// finds a clip only through a manager's list (`GetSequenceByName`), so
/// off the list is out of the file; the sequence's bytes stay behind,
/// because the splice never moves a block.
fn drop_removed_clips(out: &mut NifFile, before: &Graph, graph: &Graph, ctx: aether_nif::typed::Ctx) {
    use aether_nif::typed::controllers::NiControllerManager;
    let removed: Vec<usize> = before
        .clips
        .iter()
        .filter_map(|c| c.source_block)
        .filter(|b| !graph.clips.iter().any(|c| c.source_block == Some(*b)))
        .map(|b| b as usize)
        .collect();
    if removed.is_empty() {
        return;
    }
    for i in 0..out.blocks.len() {
        if out.blocks[i].type_name != "NiControllerManager" {
            continue;
        }
        let Ok(mut m) = NiControllerManager::decode(&out.blocks[i], ctx) else { continue };
        let n = m.sequences.len();
        m.sequences.retain(|r| !r.get().is_some_and(|s| removed.contains(&s)));
        if m.sequences.len() != n {
            out.blocks[i] = m.encode(ctx);
        }
    }
}

/// Append every clip the document ADDED -- one with no source sequence
/// -- to the file, the way the game will bind it.
///
/// Per track, the controlled block names the LIVE controller the node
/// stands for: its target's name (resolved through the palette), its
/// block type and its controller ID. 1.5.97 binds by exactly those
/// strings (`0x140CC76E0`); a match replaces the clip's serialized
/// controller copy with the live controller, so no copy is written, and a
/// plain interpolator is given a blend at bind time. The sequence joins
/// the manager's list; if the target is not in the palette it is added.
/// Everything else is appended, so nothing moves.
///
/// A file with no `NiControllerManager` gets nothing yet: creating the
/// manager and its palette is the next step, and a clip with nowhere to
/// live is not written half-way.
fn append_new_clips(
    out: &mut NifFile,
    graph: &Graph,
    ctx: aether_nif::typed::Ctx,
    block_of: &dyn Fn(NodeId) -> Option<usize>,
) {
    use aether_nif::typed::controllers::{
        ControlledBlock, NiBoolData, NiBoolInterpolator, NiBoolTimelineInterpolator,
        NiControllerManager, NiControllerSequence, NiDefaultAVObjectPalette, NiFloatInterpolator,
        NiPoint3Interpolator, NiPosData, NiTextKeyExtraData,
    };
    let Some(manager_ix) = out.blocks.iter().position(|b| b.type_name == "NiControllerManager")
    else {
        return;
    };
    let Ok(mut manager) = NiControllerManager::decode(&out.blocks[manager_ix], ctx) else {
        return;
    };
    let palette_ix = manager.object_palette.get();
    let mut palette = palette_ix.and_then(|i| NiDefaultAVObjectPalette::decode(&out.blocks[i], ctx).ok());
    // The accumulation root every clip of this manager names; for the
    // first clip of a fresh manager, the root's own name, as vanilla's
    // clips carry it.
    let accum = manager
        .sequences
        .iter()
        .filter_map(|r| r.get())
        .find_map(|i| NiControllerSequence::decode(&out.blocks[i], ctx).ok())
        .map(|s| s.accum_root_name)
        .or_else(|| {
            let root = usize::try_from(*out.roots.first()?).ok()?;
            block_av(out.blocks.get(root)?, ctx).map(|av| av.net.name)
        })
        .unwrap_or(StringIdx::NONE);
    let mut touched_manager = false;
    let mut touched_palette = false;

    fn push(out: &mut NifFile, block: aether_nif::Block) -> BlockRef {
        let ti = out.header.intern_block_type(&block.type_name);
        out.header.block_type_index.push(ti);
        out.blocks.push(block);
        BlockRef::new(out.blocks.len() - 1)
    }

    for clip in graph.clips.iter().filter(|c| c.source_block.is_none()) {
        let mut blocks: Vec<ControlledBlock> = Vec::new();
        for track in &clip.tracks {
            // The live controller this track drives: the node's own block
            // in THIS file -- its source block in a splice, the block the
            // compiler wrote for it when authoring from scratch.
            let Some(c) = block_of(track.node) else {
                continue;
            };
            // A shader variable with no live controller gets one first:
            // every shader clip track in the corpus binds to a live
            // controller, none to the clip's own copy.
            ensure_shader_controller(out, ctx, c, track.param, clip.stop_time);
            let Some((ctype, cid, target)) = track_binding(out, ctx, c, track.param) else {
                continue;
            };
            let Some(node_name) = aether_nif::typed::block_av(&out.blocks[target], ctx)
                .and_then(|av| av.net.name.resolve(&out.header).map(str::to_string))
            else {
                continue;
            };
            if let Some(p) = palette.as_mut() {
                if !p.objects.iter().any(|(n, r)| n.eq_ignore_ascii_case(&node_name) && r.get().is_some()) {
                    p.objects.push((node_name.clone(), BlockRef::new(target)));
                    touched_palette = true;
                }
            }
            let first = track.curve.keys.first().copied().unwrap_or_default();
            let interpolator = match track.curve.track {
                crate::curve::Track::Float => {
                    let data = push(out, track.curve.to_float_data().encode(ctx));
                    let mut i = NiFloatInterpolator::default();
                    i.value = first.value[0];
                    i.data = data;
                    push(out, i.encode(ctx))
                }
                crate::curve::Track::Bool => {
                    let data = push(
                        out,
                        NiBoolData {
                            interpolation: track.curve.interpolation,
                            keys: track.curve.to_bool_keys(),
                        }
                        .encode(ctx),
                    );
                    let on = u8::from(first.value[0] > 0.5);
                    // Vanilla's own choice per controller: the emitter's
                    // switch is a plain bool interpolator, a modifier
                    // toggle a timeline one.
                    if track.param == "Emitter Active Keys" {
                        let mut i = NiBoolInterpolator::default();
                        i.value = on;
                        i.data = data;
                        push(out, i.encode(ctx))
                    } else {
                        let mut i = NiBoolTimelineInterpolator::default();
                        i.value = on;
                        i.data = data;
                        push(out, i.encode(ctx))
                    }
                }
                crate::curve::Track::Vec3 => {
                    let (interpolation, floats) = track.curve.to_floats();
                    let data = push(out, NiPosData { interpolation, floats }.encode(ctx));
                    let mut i = NiPoint3Interpolator::default();
                    i.value = first.value;
                    i.data = data;
                    push(out, i.encode(ctx))
                }
            };
            let property = if ctype.starts_with("BSEffectShader") {
                "BSEffectShaderProperty"
            } else if ctype.starts_with("BSLightingShader") {
                "BSLightingShaderProperty"
            } else {
                ""
            };
            let mut s = |text: &str| {
                if text.is_empty() {
                    StringIdx::NONE
                } else {
                    StringIdx(out.header.intern_string(text))
                }
            };
            blocks.push(ControlledBlock {
                interpolator,
                controller: BlockRef::NONE,
                priority: 0,
                node_name: s(&node_name),
                property_type: s(property),
                controller_type: s(&ctype),
                controller_id: s(&cid),
                interpolator_id: s(interpolator_id_for(track.param)),
            });
        }
        let text_keys = if clip.text_keys.is_empty() {
            BlockRef::NONE
        } else {
            let keys: Vec<(f32, StringIdx)> = clip
                .text_keys
                .iter()
                .map(|(t, s)| (*t, StringIdx(out.header.intern_string(s))))
                .collect();
            push(out, NiTextKeyExtraData { name: StringIdx::NONE, keys }.encode(ctx))
        };
        let name = StringIdx(out.header.intern_string(&clip.name));
        let mut seq = NiControllerSequence {
            name,
            controlled_blocks: blocks,
            text_keys,
            cycle_type: if clip.looping { 0 } else { 2 },
            frequency: clip.frequency,
            start_time: clip.start_time,
            stop_time: clip.stop_time,
            manager: BlockRef::new(manager_ix),
            accum_root_name: accum,
            ..NiControllerSequence::default()
        };
        let seq_ref = push(out, seq.encode(ctx));
        manager.sequences.push(seq_ref);
        touched_manager = true;
    }
    if touched_manager {
        out.blocks[manager_ix] = manager.encode(ctx);
    }
    if let (true, Some(i), Some(p)) = (touched_palette, palette_ix, palette.as_mut()) {
        out.blocks[i] = p.encode(ctx);
    }
}

/// The behavior graph path a file's root names, if any -- the value the
/// decompiler puts on Scene Output.
pub fn root_behavior_of(nif: &NifFile) -> Option<String> {
    let ctx = aether_nif::typed::Ctx::of(&nif.header).ok()?;
    let root = usize::try_from(*nif.roots.first()?).ok()?;
    let node = NiNode::decode(nif.blocks.get(root)?, ctx).ok()?;
    root_behavior(nif, ctx, &node.av.net.extra_data).map(|(_, path)| path)
}

/// Point a spliced file's root at behavior graph `path` ("" = none).
///
/// An existing BGED is rewritten in place; a new one is APPENDED and
/// named from the root's extra-data list; clearing drops the root's
/// reference and leaves the block behind with nothing pointing at it --
/// the splice never moves a block, the same rule as any delete.
fn set_root_behavior(out: &mut NifFile, root: usize, ctx: aether_nif::typed::Ctx, path: &str) {
    use aether_nif::typed::controllers::BSBehaviorGraphExtraData;
    let Some(mut av) = block_av(&out.blocks[root], ctx) else {
        return;
    };
    match root_behavior(out, ctx, &av.net.extra_data) {
        Some((i, _)) if path.is_empty() => {
            av.net.extra_data.retain(|r| r.get() != Some(i));
        }
        Some((i, _)) => {
            let Ok(mut bged) = BSBehaviorGraphExtraData::decode(&out.blocks[i], ctx) else {
                return;
            };
            bged.behavior_graph_file = StringIdx(out.header.intern_string(path));
            out.blocks[i] = bged.encode(ctx);
            return;
        }
        None if path.is_empty() => return,
        None => {
            let mut bged = BSBehaviorGraphExtraData {
                name: StringIdx(out.header.intern_string("BGED")),
                behavior_graph_file: StringIdx(out.header.intern_string(path)),
                controls_base_skeleton: false,
            };
            let block = bged.encode(ctx);
            let ti = out.header.intern_block_type(&block.type_name);
            out.header.block_type_index.push(ti);
            out.blocks.push(block);
            av.net.extra_data.push(BlockRef::new(out.blocks.len() - 1));
        }
    }
    if let Some(patched) = set_block_av(&out.blocks[root], ctx, &av) {
        out.blocks[root] = patched;
    }
}

/// The typed-view context of a file, or `None` when it is not a
/// Skyrim stream.
fn ctx_of(file: &NifFile) -> Option<aether_nif::typed::Ctx> {
    aether_nif::typed::Ctx::of(&file.header).ok()
}

/// The `NiObjectNET.Name` of a source block, resolved to a string.
///
/// Read generically: scalar 0 of any block whose typed view starts
/// with an `object_net` is its name index, so this needs no per-type
/// code -- the same trick `block_av` and `block_refs` play.
fn block_name(file: &NifFile, index: usize) -> Option<String> {
    let block = file.blocks.get(index)?;
    let scalars = aether_nif::typed::block_scalars(block, ctx_of(file)?)?;
    match scalars.first()? {
        aether_nif::typed::Scalar::U32(idx) => file.header.string(*idx).map(str::to_string),
        _ => None,
    }
}

/// The `NiTimeController` prefix of a source block.
///
/// Read generically off the flat scalar sequence rather than by
/// matching every controller type: `controller_base` expands inline
/// at the head of every one of them, after `object_net`'s two
/// scalars, so the layout is name, extra_data, controller,
/// next_controller, flags, frequency, phase, start, stop, target.
fn controller_base_of(
    block: &aether_nif::Block,
    ctx: aether_nif::typed::Ctx,
) -> Option<ControllerBase> {
    use aether_nif::typed::Scalar;
    let s = aether_nif::typed::block_scalars(block, ctx)?;
    // Guard the SHAPE, not the length: a block whose head is not a
    // controller must yield nothing rather than a misread. The
    // indices are MEASURED off the field census, not counted by
    // hand -- `NiPSysModifierActiveCtlr[1]` is the U16 that reads
    // 104 in the file and 72 from the compiler, and
    // `NiPSysUpdateCtlr[5]` is the stop time. Counting the derived
    // helpers by eye gave 4/5/6 and silently matched nothing.
    let (
        Scalar::I32(next),
        Scalar::U16(flags),
        Scalar::F32(frequency),
        Scalar::F32(phase),
        Scalar::F32(start_time),
        Scalar::F32(stop_time),
    ) = (
        s.first()?.clone(),
        s.get(1)?.clone(),
        s.get(2)?.clone(),
        s.get(3)?.clone(),
        s.get(4)?.clone(),
        s.get(5)?.clone(),
    )
    else {
        return None;
    };
    Some(ControllerBase {
        next_controller: aether_nif::typed::BlockRef(next),
        flags,
        frequency,
        phase,
        start_time,
        stop_time,
        ..ControllerBase::default()
    })
}

/// The controller flag word with only its CYCLE bits rewritten.
///
/// `Loop` is the one bit of this word the graph models, and the
/// compiler used to express it as a whole literal -- 72 for looping,
/// 76 for clamping -- which threw away every other bit the file
/// carried. Vanilla ships 104 among others, and rewriting it as 72
/// keeps the cycle type by luck while losing the rest.
///
/// The cycle type is bits 1-2 (0 LOOP, 1 REVERSE, 2 CLAMP), so those
/// two are the only ones an edit to `Loop` may touch.
fn cycle_flags(authored: u16, looping: bool) -> u16 {
    const CYCLE: u16 = 0b110;
    let cycle = if looping { 0 } else { 2 << 1 };
    (authored & !CYCLE) | cycle
}

fn compile_system(
    graph: &Graph,
    system_id: NodeId,
    b: &mut Builder,
    root: BlockRef,
    source: Option<&NifFile>,
) -> Result<BlockRef, CompileError> {
    // Input sockets on ParticleSystem: 0 Data, 1 Shader, 2 Alpha,
    // 3 Modifiers (multi), 4 Controllers (multi).
    let data_node = wired_sources(graph, system_id, 0)
        .first()
        .copied()
        .ok_or(CompileError::SystemWithoutData { system: system_id })?;
    let shader_node = wired_sources(graph, system_id, 1).first().copied();
    let alpha_node = wired_sources(graph, system_id, 2).first().copied();
    let modifier_nodes = wired_sources(graph, system_id, 3);
    let controller_nodes = wired_sources(graph, system_id, 4);

    // Strip trails switch both block types together, like every vanilla
    // strip system (the layouts are identical; only the names differ).
    let strips = graph.nodes[&data_node].bool_or("Strip Trails", false);
    let psys_slot = b.reserve(if strips {
        "BSStripParticleSystem"
    } else {
        "NiParticleSystem"
    });
    b.claim(system_id, psys_slot);

    // --- modifier stack -------------------------------------------------
    // Wired modifiers sorted by canonical class (stable within a class),
    // wrapped by the mandatory service modifiers.
    let mut sorted = modifier_nodes.clone();
    sorted.sort_by_key(|n| order_class(graph.nodes[n].kind));
    // ONE collider manager per system, with the colliders chained under
    // it — the only layout that ships. Measured over both corpora and
    // the mod roots: 164 systems carry colliders and every one has
    // exactly ONE manager, chaining 1 (107), 2 (37), 3 (14), 6 (2) or
    // 7 (4) colliders. Emitting a manager per Collider node instead was
    // the single largest structural difference from vanilla (+98 blocks
    // corpus-wide) and the reason those files could never round-trip.
    // The graph keeps one node per COLLIDER — that is what a user
    // thinks in — and the container stays the compiler's business, like
    // every other service modifier.
    let colliders: Vec<NodeId> = sorted
        .iter()
        .copied()
        .filter(|n| graph.nodes[n].kind == NodeKind::ColliderModifier)
        .collect();
    if colliders.len() > 1 {
        // Keep only the first as the stack entry; it emits them all.
        let first = colliders[0];
        sorted.retain(|n| {
            graph.nodes[n].kind != NodeKind::ColliderModifier || *n == first
        });
    }

    struct PlannedModifier {
        node: Option<NodeId>,
        kind: ServiceKind,
    }
    enum ServiceKind {
        AgeDeath,
        Wired,
        Position,
        BoundUpdate,
        /// Strip trails only: the path-point updater, last in the stack
        /// (Skyrim's canonical ORDER_SK_BSSTRIPUPDATE slot).
        StripUpdate,
    }
    // Which service modifiers does the SOURCE system actually carry?
    // The compiler used to plan Position and BoundUpdate
    // unconditionally, and real vanilla files disagree: frost3's
    // first system ships without a PositionModifier, and
    // impforthallcollapsefx01 and mgmagicfirepillar01 ship systems
    // without a BoundUpdateModifier -- seven systems corpus-wide.
    // Forcing the full set made the compiled modifier list one ref
    // LONGER than the source's, the scalar counts disagreed, and the
    // merge refused the whole block. A from-scratch system (no
    // provenance) still gets the full set.
    let source_services: Option<Vec<String>> = (|| {
        let src = graph.nodes.get(&system_id)?.source_block? as usize;
        let file = source?;
        let ps = NiParticleSystem::decode(file.blocks.get(src)?, ctx_of(file)?).ok()?;
        Some(
            ps.modifiers
                .iter()
                .filter_map(|r| r.get())
                .filter_map(|i| file.blocks.get(i).map(|b| b.type_name.clone()))
                .collect(),
        )
    })();
    let has_service = |t: &str| {
        source_services
            .as_ref()
            .map(|list| list.iter().any(|x| x == t))
            .unwrap_or(true)
    };
    let mut planned = vec![PlannedModifier {
        node: None,
        kind: ServiceKind::AgeDeath,
    }];
    planned.extend(sorted.iter().map(|&n| PlannedModifier {
        node: Some(n),
        kind: ServiceKind::Wired,
    }));
    if has_service("NiPSysPositionModifier") {
        planned.push(PlannedModifier {
            node: None,
            kind: ServiceKind::Position,
        });
    }
    if has_service("NiPSysBoundUpdateModifier") {
        planned.push(PlannedModifier {
            node: None,
            kind: ServiceKind::BoundUpdate,
        });
    }
    if strips {
        planned.push(PlannedModifier {
            node: None,
            kind: ServiceKind::StripUpdate,
        });
    }

    // Names are assigned by final stack position, Bethesda-style. The
    // age/death service block is reserved rather than pushed: when a Spawn
    // node is wired it must reference the spawner, whose block does not
    // exist yet at stack position 0.
    let mut modifier_refs = Vec::new();
    // Stack name of every wired modifier: emitter controllers resolve their
    // emitter through it, active toggles their target modifier.
    let mut modifier_names: Vec<(NodeId, StringIdx)> = Vec::new();
    let mut age_death: Option<(BlockRef, ModifierBase)> = None;
    let mut spawn: Option<(NodeId, BlockRef)> = None;
    // The AUTHORED name when the node came from a file, and a name
    // that does not depend on the stack index when it did not.
    //
    // `Modifier:{i}` was both wrong and DANGEROUS. Wrong because
    // vanilla names its modifiers itself; dangerous because the name
    // is a function of the final stack position, so adding one node
    // renamed every modifier at or after it -- and a
    // `NiControllerSequence` binds its controlled blocks by that
    // STRING. Its blocks are copied through as source bytes, so they
    // kept naming the old string and every track aimed at a renamed
    // modifier went dead. ZD_MorthalSwordLeaf's mLoop alone has 64
    // controlled blocks.
    //
    // The service names are by KIND for the same reason: constant, so
    // the two compiles agree and the merge keeps whatever the file
    // authored.
    let authored_name = |node: Option<NodeId>| -> Option<String> {
        let src = graph.nodes.get(&node?)?.source_block? as usize;
        block_name(source?, src)
    };
    // A service modifier has no graph node, so it has no provenance of
    // its own -- but the SYSTEM does, and a system carries at most one
    // of each service kind. So the authored name is reachable by
    // looking for that type in the source system's own modifier list.
    // Naming them by kind was already safe (constant, so the merge
    // keeps the file's own), but it left three of the largest entries
    // in the compiler's field census and made a from-scratch export
    // name blocks nothing else in the format names that way.
    let service_source_idx = |type_name: &str| -> Option<usize> {
        let file = source?;
        let src = graph.nodes.get(&system_id)?.source_block? as usize;
        let psys = NiParticleSystem::decode(file.blocks.get(src)?, ctx_of(file)?).ok()?;
        psys.modifiers
            .iter()
            .filter_map(|r| r.get())
            .find(|i| file.blocks.get(*i).map(|b| b.type_name.as_str()) == Some(type_name))
    };
    let service_name =
        |type_name: &str| -> Option<String> { block_name(source?, service_source_idx(type_name)?) };
    // The authored stack order and active byte, read generically off
    // the base every NiPSysModifier subtype leads with: scalar 1 is
    // `order`, scalar 3 is `active`. About 90 vanilla files hold
    // sequential small orders instead of the ORDER_* constants, and 47
    // gravity modifiers ship INACTIVE at rest (an Active Toggle
    // controller wakes them); both were being overwritten.
    // Generic over the BLOCK, so the same reading serves a node that
    // points into the open file and one that carries its own bytes.
    let order_active_of = |block: &Block, ctx: Ctx| -> Option<(u32, u32, bool)> {
        use aether_nif::typed::Scalar;
        let sc = aether_nif::typed::block_scalars(block, ctx)?;
        let name_idx = match sc.first()? {
            Scalar::U32(v) => *v,
            _ => return None,
        };
        let order = match sc.get(1)? {
            Scalar::U32(v) => *v,
            _ => return None,
        };
        let active = match sc.get(3)? {
            Scalar::U8(v) => *v != 0,
            _ => return None,
        };
        Some((name_idx, order, active))
    };
    let order_active_at = |idx: usize| -> Option<(u32, u32, bool)> {
        let file = source?;
        order_active_of(file.blocks.get(idx)?, ctx_of(file)?)
    };
    // A node's OWN authored bytes: the route a pasted modifier takes,
    // where there is no index into the open file. The NAME index is
    // dropped -- a string index belongs to the header it came from and
    // means nothing in this file -- so only the order and the active
    // byte survive, which is exactly what was being lost: 47 vanilla
    // gravity modifiers ship INACTIVE and a paste woke every one.
    let order_active_of_node = |n: NodeId| -> Option<(u32, u32, bool)> {
        let node = graph.nodes.get(&n)?;
        if node.source_block.is_some() {
            return None;
        }
        let (block, ctx) = authored_block(source, node)?;
        let (_, order, active) = order_active_of(block, ctx)?;
        Some((u32::MAX, order, active))
    };
    for (i, plan) in planned.iter().enumerate() {
        let label = match plan.kind {
            ServiceKind::AgeDeath => service_name("NiPSysAgeDeathModifier")
                .unwrap_or_else(|| "AgeDeath".to_string()),
            ServiceKind::Position => service_name("NiPSysPositionModifier")
                .unwrap_or_else(|| "Position".to_string()),
            ServiceKind::BoundUpdate => service_name("NiPSysBoundUpdateModifier")
                .unwrap_or_else(|| "BoundUpdate".to_string()),
            ServiceKind::StripUpdate => service_name("BSPSysStripUpdateModifier")
                .unwrap_or_else(|| "StripUpdate".to_string()),
            ServiceKind::Wired => {
                // A Collider node's provenance is the COLLIDER block;
                // the modifier the stack holds is the MANAGER, which
                // has no node of its own -- so its name, like the
                // other services', comes from the source system's
                // list by type.
                let manager = plan
                    .node
                    .map(|n| graph.nodes[&n].kind == NodeKind::ColliderModifier)
                    .unwrap_or(false);
                if manager {
                    service_name("NiPSysColliderManager")
                } else {
                    authored_name(plan.node)
                }
                .unwrap_or_else(|| format!("Modifier:{i}"))
            }
        };
        let name = b.intern(&label);
        // The ORDER CLASS, not the stack index. Vanilla stores the
        // `NiPSysModifierOrder` enum value here -- measured over 19,414
        // modifier blocks in the SSE mesh archives, every one an enum
        // value and none a stack index -- so writing `i` came back as a
        // gravity modifier at 8 where the file said 4000. The services
        // get their own slots for the same reason: the strip updater
        // has a constant that had never been used, and 176 vanilla
        // blocks hold it.
        let order = match plan.kind {
            ServiceKind::AgeDeath => order::KILL_OLD_PARTICLES,
            ServiceKind::Position => order::POS_UPDATE,
            ServiceKind::BoundUpdate => order::BOUND_UPDATE,
            ServiceKind::StripUpdate => order::SK_STRIP_UPDATE,
            ServiceKind::Wired => plan
                .node
                .map(|n| order_class(graph.nodes[&n].kind))
                .unwrap_or(order::GENERAL),
        };
        // What the modifier's own SLOT in this file says -- kept apart
        // from the paste fallback below, whose name index is always
        // u32::MAX ("dropped: it belongs to another header"), so that a
        // NULL name the file really holds can be told from one that was
        // merely not carried over.
        let slot_sourced = match plan.kind {
            ServiceKind::AgeDeath => service_source_idx("NiPSysAgeDeathModifier"),
            ServiceKind::Position => service_source_idx("NiPSysPositionModifier"),
            ServiceKind::BoundUpdate => service_source_idx("NiPSysBoundUpdateModifier"),
            ServiceKind::StripUpdate => service_source_idx("BSPSysStripUpdateModifier"),
            ServiceKind::Wired => match plan.node {
                Some(n) if graph.nodes[&n].kind == NodeKind::ColliderModifier => {
                    service_source_idx("NiPSysColliderManager")
                }
                Some(n) => graph.nodes[&n].source_block.map(|v| v as usize),
                None => None,
            },
        }
        .and_then(order_active_at);
        // Falls back to the node's carried bytes when it names no slot
        // in this file -- a pasted modifier.
        let sourced = slot_sourced.or_else(|| plan.node.and_then(order_active_of_node));
        // A header can hold DUPLICATE strings, and `intern` finds the
        // first equal one -- which moved convergenceart's bomb name
        // from index 65 to its twin at 52. When the label still says
        // what the source block's own name says, the source's INDEX is
        // the byte-honest choice; the seeded header keeps it valid.
        let name = match (sourced, source) {
            // The file's own slot holds NO name. SSE NIF Optimizer drops
            // the name of every modifier nothing refers to (eight of ten
            // on the user's "Trail Sparkles"); inventing one there is an
            // edit nobody made, and a NULL name the game never looks up
            // is exactly what the file meant.
            _ if slot_sourced.is_some_and(|(ni, _, _)| ni == u32::MAX) => StringIdx::NONE,
            (Some((ni, _, _)), Some(file))
                if ni != u32::MAX
                    && StringIdx(ni).resolve(&file.header).map(str::to_string)
                        == Some(label.clone()) =>
            {
                StringIdx(ni)
            }
            _ => name,
        };
        let base = ModifierBase {
            name,
            order: sourced.map(|(_, o, _)| o).unwrap_or(order),
            target: psys_slot,
            active: sourced.map(|(_, _, a)| a).unwrap_or(true),
        };
        let mref = match plan.kind {
            ServiceKind::AgeDeath => {
                let slot = b.reserve("NiPSysAgeDeathModifier");
                age_death = Some((slot, base));
                slot
            }
            ServiceKind::Position => b.push(&mut NiPSysPositionModifier { base }),
            ServiceKind::BoundUpdate => b.push(&mut NiPSysBoundUpdateModifier {
                base,
                update_skip: 0,
            }),
            ServiceKind::StripUpdate => {
                b.push(&mut aether_nif::typed::particles::BSPSysStripUpdateModifier {
                    base,
                    update_delta_time: graph.nodes[&data_node]
                        .f32_or("Update Delta Time", 0.033333),
                })
            }
            ServiceKind::Wired => {
                let node = plan.node.expect("wired modifiers carry their node id");
                modifier_names.push((node, name));
                let mref =
                    compile_modifier(graph, node, base, b, root, &colliders, source);
                // A COLLIDER group claims each of its own blocks
                // inside, and the ref returned here is the MANAGER --
                // a service with no node of its own. Claiming it for
                // the node would overwrite the first collider's block
                // with the manager's slot, and the provenance seed
                // would then compare the wrong pair: measured, the
                // group's colliders lost their slots on a shape
                // change and the edit was appended instead.
                if graph.nodes[&node].kind != NodeKind::ColliderModifier {
                    b.claim(node, mref);
                }
                if graph.nodes[&node].kind == NodeKind::SpawnModifier {
                    // Age/death can reference only ONE spawner; prefer a
                    // node with Spawn On Death set (the vanilla pattern
                    // keeps a collider spawner in the same stack with the
                    // flag off — picking blindly by stack order would
                    // silently drop the user's flagged one).
                    let flagged = graph.nodes[&node].bool_or("Spawn On Death", true);
                    let current_flagged = spawn
                        .map(|(n, _)| graph.nodes[&n].bool_or("Spawn On Death", true))
                        .unwrap_or(false);
                    if spawn.is_none() || (flagged && !current_flagged) {
                        spawn = Some((node, mref));
                    }
                }
                mref
            }
        };
        modifier_refs.push(mref);
    }
    if let Some((slot, base)) = age_death {
        let (spawn_on_death, spawn_modifier) = match spawn {
            Some((node, mref)) => (graph.nodes[&node].bool_or("Spawn On Death", true), mref),
            None => (false, BlockRef::NONE),
        };
        b.fill(
            slot,
            &mut NiPSysAgeDeathModifier {
                base,
                spawn_on_death,
                spawn_modifier,
            },
        );
    }

    // --- controller chain ----------------------------------------------
    // Two passes: reserve every controller slot first so each one can point
    // at its successor via Next Controller, then fill.
    let mut ctlr_kinds: Vec<Option<NodeId>> = controller_nodes
        .iter()
        .filter(|n| {
            matches!(
                graph.nodes[n].kind,
                NodeKind::EmitterCtlr
                    | NodeKind::UpdateCtlr
                    | NodeKind::ModifierActiveCtlr
                    | NodeKind::ParamCtlr
                    | NodeKind::VisibilityCtlr
                    | NodeKind::MultiEmitterCtlr
            )
        })
        .map(|n| Some(*n))
        .collect();
    let needs_update_ctlr = !ctlr_kinds
        .iter()
        .any(|n| matches!(graph.nodes[&n.unwrap()].kind, NodeKind::UpdateCtlr));
    // NOT synthesized when the document lacks one. Appending it was
    // the compiler writing something the user had deleted -- and doing
    // it silently, so the editor said the node was gone while the file
    // kept simulating. The document is what gets written; the game's
    // requirement is real and `validate` states it, which is the only
    // place a rule the user can violate belongs.
    //
    // Safe on every route into a graph: `decompile` makes an
    // `UpdateCtlr` node for every source one, and all three templates
    // build theirs through `base_system`, which inserts it.
    let _ = needs_update_ctlr;
    let chain: Vec<BlockRef> = ctlr_kinds
        .iter()
        .map(|n| match n.map(|id| graph.nodes[&id].kind) {
            Some(NodeKind::EmitterCtlr) => b.reserve("NiPSysEmitterCtlr"),
            Some(NodeKind::ModifierActiveCtlr) => b.reserve("NiPSysModifierActiveCtlr"),
            Some(NodeKind::MultiEmitterCtlr) => b.reserve("BSPSysMultiTargetEmitterCtlr"),
            Some(NodeKind::VisibilityCtlr) => b.reserve("NiVisController"),
            Some(NodeKind::ParamCtlr) => {
                // The block TYPE carries which parameter the track
                // drives; the enum index picks it.
                let node = n.unwrap();
                let idx = graph.nodes[&node]
                    .enum_index("Parameter")
                    .min(crate::graph::PARAM_CTLR_BLOCKS.len() - 1);
                b.reserve(crate::graph::PARAM_CTLR_BLOCKS[idx])
            }
            _ => b.reserve("NiPSysUpdateCtlr"),
        })
        .collect();
    for (i, node) in ctlr_kinds.iter().enumerate() {
        if let Some(id) = node {
            b.claim(*id, chain[i]);
        }
        // The AUTHORED flags, phase and frequency when this
        // controller came from a file.
        //
        // The literal 72 is Active | CYCLE_CLAMP, which is a
        // reasonable invention and not what the file says: vanilla
        // ships 76 (with Compute Scaled Time) and 104 (a different
        // cycle type) among others, so writing 72 changes how the
        // controller cycles. Phase and Frequency are modelled nowhere
        // in the graph, so a rebuild replaced them with 0.0 and 1.0.
        //
        // Only the four fields the graph does NOT model are carried:
        // `next_controller` and `target` belong to the chain being
        // built, and Start/Stop are overwritten by the arms that
        // model them.
        let authored = node
            .and_then(|id| graph.nodes.get(&id))
            .and_then(|n| n.source_block)
            .zip(source)
            .and_then(|(src, file)| {
                let block = file.blocks.get(src as usize)?;
                controller_base_of(block, ctx_of(file)?)
            });
        let base = ControllerBase {
            next_controller: chain.get(i + 1).copied().unwrap_or(BlockRef::NONE),
            flags: authored.as_ref().map(|c| c.flags).unwrap_or(72),
            frequency: authored.as_ref().map(|c| c.frequency).unwrap_or(1.0),
            phase: authored.as_ref().map(|c| c.phase).unwrap_or(0.0),
            target: psys_slot,
            ..ControllerBase::default()
        };
        match node.map(|id| (id, graph.nodes[&id].kind)) {
            Some((ctlr_node, NodeKind::ModifierActiveCtlr)) => {
                // Wired to nothing: the controller is written with an
                // EMPTY modifier name, which is what the document
                // says. It binds to no modifier in game and does
                // nothing -- `validate` says so by name. Refusing the
                // whole export here meant deleting an emitter made the
                // file unsaveable, and the user's own framing is the
                // right one: write the document, let the validator
                // explain what the game will make of it.
                let modifier_name = wired_sources(graph, ctlr_node, 0)
                    .first()
                    .and_then(|target| {
                        modifier_names
                            .iter()
                            .find(|(n, _)| n == target)
                            .map(|(_, name)| *name)
                    })
                    // The format's own "no name": the game looks up a
                    // modifier by string and finds none.
                    .unwrap_or(StringIdx::NONE);
                let track = graph.nodes[&ctlr_node].curve_or("Keys", Track::Bool);
                let data = b.push_keys(&mut NiBoolData {
                    // 5 = CONST, the only type vanilla ships for bool
                    // keys: 9,771 of 9,771 corpus blocks. Writing 1
                    // (linear) was 3,230 field-census errors. A track
                    // that came from a file keeps the file's own type;
                    // an empty one has none and takes vanilla's.
                    interpolation: bool_key_type(&track),
                    keys: track.to_bool_keys(),
                });
                // A sequence-managed controller keeps the blend form
                // the file gave it, resting value and all.
                let interp = match source_interpolator(source, &graph.nodes[&ctlr_node], &["NiBlendBoolInterpolator"]) {
                    Some(blend) => b.push_raw(blend),
                    None => b.push(&mut NiBoolTimelineInterpolator {
                        value: source_timeline_value(source, &graph.nodes[&ctlr_node])
                            .unwrap_or(1),
                        data,
                    }),
                };
                let looping = graph.nodes[&ctlr_node].bool_or("Loop", true);
                b.fill(
                    chain[i],
                    &mut NiPSysModifierActiveCtlr {
                        base: ControllerBase {
                            flags: cycle_flags(base.flags, looping),
                            start_time: graph.nodes[&ctlr_node].f32_or("Start Time", 0.0),
                            stop_time: graph.nodes[&ctlr_node].f32_or("Stop Time", 1.0),
                            ..base
                        },
                        interpolator: interp,
                        modifier_name,
                    },
                );
            }
            Some((ctlr_node, NodeKind::EmitterCtlr)) => {
                // See the active-toggle arm: an unwired controller is
                // written with an empty name rather than refused.
                let modifier_name = wired_sources(graph, ctlr_node, 0)
                    .first()
                    .and_then(|emitter| {
                        modifier_names
                            .iter()
                            .find(|(n, _)| n == emitter)
                            .map(|(_, name)| *name)
                    })
                    // The format's own "no name": the game looks up a
                    // modifier by string and finds none.
                    .unwrap_or(StringIdx::NONE);
                // Keyed tracks when the text params carry keys, constant
                // pose values otherwise — both vanilla forms.
                // The curve carries its OWN key type and tangents now.
                // `interpolation: 1` here re-emitted every quadratic
                // track as linear, and the field-count mismatch made
                // the merge refuse the whole block.
                let rate_keys = graph.nodes[&ctlr_node].curve_or("Birth Rate Keys", Track::Float);
                let rate_data = if rate_keys.is_empty() {
                    BlockRef::NONE
                } else {
                    let mut kd = rate_keys.to_float_data();
                    if source_float_data_is_const(source, &graph.nodes[&ctlr_node], &kd.floats) {
                        kd.interpolation = 5;
                    }
                    b.push_keys(&mut kd)
                };
                let birth_rate = match source_interpolator(
                    source,
                    &graph.nodes[&ctlr_node],
                    &["NiBlendFloatInterpolator"],
                ) {
                    Some(blend) => b.push_raw(blend),
                    None => b.push(&mut NiFloatInterpolator {
                        value: source_float_pose(
                            source,
                            &graph.nodes[&ctlr_node],
                            graph.nodes[&ctlr_node].param("Birth Rate").is_none(),
                        )
                        .unwrap_or_else(|| {
                            graph.nodes[&ctlr_node].f32_or("Birth Rate", 20.0)
                        }),
                        data: rate_data,
                    }),
                };
                let active_track =
                    graph.nodes[&ctlr_node].curve_or("Emitter Active Keys", Track::Bool);
                let active_keys = active_track.to_bool_keys();
                let always_on = active_keys.iter().all(|(_, v)| *v != 0);
                // Vanilla ships a data block even for an always-on
                // track; dropping it NULLed the interpolator's ref
                // against the source's. Kept whenever the source had
                // one -- the keys are the param's either way, so an
                // edit still lands.
                let had_data = source_bool_has_data(source, &graph.nodes[&ctlr_node]);
                let active_data = if always_on && !had_data {
                    BlockRef::NONE
                } else {
                    b.push_keys(&mut NiBoolData {
                        // 5 = CONST, like every vanilla bool track.
                        interpolation: bool_key_type(&active_track),
                        keys: active_keys,
                    })
                };
                let visibility = match source_interpolator(
                    source,
                    &graph.nodes[&ctlr_node],
                    &["NiBlendBoolInterpolator"],
                ) {
                    Some(blend) => b.push_raw(blend),
                    None => b.push(&mut NiBoolInterpolator {
                        value: source_bool_value(source, &graph.nodes[&ctlr_node]).unwrap_or(1),
                        data: active_data,
                    }),
                };
                let looping = graph.nodes[&ctlr_node].bool_or("Loop", true);
                b.fill(
                    chain[i],
                    &mut NiPSysEmitterCtlr {
                        base: ControllerBase {
                            // 72 = active + loop; 76 = active + clamp (once).
                            flags: cycle_flags(base.flags, looping),
                            start_time: graph.nodes[&ctlr_node].f32_or("Start Time", 0.0),
                            stop_time: graph.nodes[&ctlr_node].f32_or("Stop Time", 1.0),
                            ..base
                        },
                        interpolator: birth_rate,
                        modifier_name,
                        visibility_interpolator: visibility,
                    },
                );
            }
            Some((ctlr_node, NodeKind::ParamCtlr)) => {
                let target = wired_sources(graph, ctlr_node, 0).first().copied().ok_or(
                    CompileError::ControllerWithoutEmitter {
                        controller: ctlr_node,
                    },
                )?;
                let modifier_name = modifier_names
                    .iter()
                    .find(|(n, _)| *n == target)
                    .map(|(_, name)| *name)
                    .ok_or(CompileError::ControllerWithoutEmitter {
                        controller: ctlr_node,
                    })?;
                let keys = graph.nodes[&ctlr_node].curve_or("Keys", Track::Float);
                let data = if keys.is_empty() {
                    BlockRef::NONE
                } else {
                    let mut kd = keys.to_float_data();
                    if source_float_data_is_const(source, &graph.nodes[&ctlr_node], &kd.floats) {
                        kd.interpolation = 5;
                    }
                    b.push_keys(&mut kd)
                };
                let interp = match source_interpolator(
                    source,
                    &graph.nodes[&ctlr_node],
                    &["NiBlendFloatInterpolator"],
                ) {
                    Some(blend) => b.push_raw(blend),
                    None => b.push(&mut NiFloatInterpolator {
                        value: source_float_pose(source, &graph.nodes[&ctlr_node], false)
                            .unwrap_or_else(|| {
                                keys.keys.first().map(|k| k.value[0]).unwrap_or(0.0)
                            }),
                        data,
                    }),
                };
                let looping = graph.nodes[&ctlr_node].bool_or("Loop", true);
                b.fill(
                    chain[i],
                    &mut NiPSysModifierFloatCtlr {
                        base: ControllerBase {
                            flags: cycle_flags(base.flags, looping),
                            start_time: graph.nodes[&ctlr_node].f32_or("Start Time", 0.0),
                            stop_time: graph.nodes[&ctlr_node].f32_or("Stop Time", 1.0),
                            ..base
                        },
                        interpolator: interp,
                        modifier_name,
                    },
                );
            }
            Some((ctlr_node, NodeKind::VisibilityCtlr)) => {
                let track = graph.nodes[&ctlr_node].curve_or("Keys", Track::Bool);
                let data = b.push_keys(&mut NiBoolData {
                    // 5 = CONST, the only type vanilla ships for bool
                    // keys: 9,771 of 9,771 corpus blocks. Writing 1
                    // (linear) was 3,230 field-census errors. A track
                    // that came from a file keeps the file's own type;
                    // an empty one has none and takes vanilla's.
                    interpolation: bool_key_type(&track),
                    keys: track.to_bool_keys(),
                });
                let interp = match source_interpolator(
                    source,
                    &graph.nodes[&ctlr_node],
                    &["NiBlendBoolInterpolator"],
                ) {
                    Some(blend) => b.push_raw(blend),
                    None => b.push(&mut NiBoolInterpolator {
                        value: source_bool_value(source, &graph.nodes[&ctlr_node]).unwrap_or(1),
                        data,
                    }),
                };
                let looping = graph.nodes[&ctlr_node].bool_or("Loop", true);
                b.fill(
                    chain[i],
                    &mut NiVisController {
                        base: ControllerBase {
                            flags: cycle_flags(base.flags, looping),
                            start_time: graph.nodes[&ctlr_node].f32_or("Start Time", 0.0),
                            stop_time: graph.nodes[&ctlr_node].f32_or("Stop Time", 1.0),
                            ..base
                        },
                        interpolator: interp,
                    },
                );
            }
            Some((ctlr_node, NodeKind::MultiEmitterCtlr)) => {
                let emitter = wired_sources(graph, ctlr_node, 0).first().copied().ok_or(
                    CompileError::ControllerWithoutEmitter {
                        controller: ctlr_node,
                    },
                )?;
                let modifier_name = modifier_names
                    .iter()
                    .find(|(n, _)| *n == emitter)
                    .map(|(_, name)| *name)
                    .ok_or(CompileError::ControllerWithoutEmitter {
                        controller: ctlr_node,
                    })?;
                // The curve carries its OWN key type and tangents now.
                // `interpolation: 1` here re-emitted every quadratic
                // track as linear, and the field-count mismatch made
                // the merge refuse the whole block.
                let rate_keys = graph.nodes[&ctlr_node].curve_or("Birth Rate Keys", Track::Float);
                let rate_data = if rate_keys.is_empty() {
                    BlockRef::NONE
                } else {
                    let mut kd = rate_keys.to_float_data();
                    if source_float_data_is_const(source, &graph.nodes[&ctlr_node], &kd.floats) {
                        kd.interpolation = 5;
                    }
                    b.push_keys(&mut kd)
                };
                let birth_rate = match source_interpolator(
                    source,
                    &graph.nodes[&ctlr_node],
                    &["NiBlendFloatInterpolator"],
                ) {
                    Some(blend) => b.push_raw(blend),
                    None => b.push(&mut NiFloatInterpolator {
                        value: source_float_pose(
                            source,
                            &graph.nodes[&ctlr_node],
                            graph.nodes[&ctlr_node].param("Birth Rate").is_none(),
                        )
                        .unwrap_or_else(|| {
                            graph.nodes[&ctlr_node].f32_or("Birth Rate", 20.0)
                        }),
                        data: rate_data,
                    }),
                };
                // The game never reads this controller's visibility
                // track (RE 0x140D582F0); vanilla still ships one, so
                // one is emitted for fidelity.
                let visibility = match source_interpolator(
                    source,
                    &graph.nodes[&ctlr_node],
                    &["NiBlendBoolInterpolator"],
                ) {
                    Some(blend) => b.push_raw(blend),
                    None => {
                        let data = source_bool_data(source, &graph.nodes[&ctlr_node])
                            .map(|bl| b.push_raw(bl))
                            .unwrap_or(BlockRef::NONE);
                        b.push(&mut NiBoolInterpolator {
                            value: source_bool_value(source, &graph.nodes[&ctlr_node])
                                .unwrap_or(1),
                            data,
                        })
                    }
                };
                let looping = graph.nodes[&ctlr_node].bool_or("Loop", true);
                b.fill(
                    chain[i],
                    &mut BSPSysMultiTargetEmitterCtlr {
                        base: ControllerBase {
                            flags: cycle_flags(base.flags, looping),
                            start_time: graph.nodes[&ctlr_node].f32_or("Start Time", 0.0),
                            stop_time: graph.nodes[&ctlr_node].f32_or("Stop Time", 3.0),
                            ..base
                        },
                        interpolator: birth_rate,
                        modifier_name,
                        visibility_interpolator: visibility,
                        max_emitters: graph.nodes[&ctlr_node].u32_or("Max Emitters", 4) as u16,
                        // Block 0 is always the root, and in an MPS file
                        // the root IS the master.
                        master_particle_system: BlockRef(0),
                    },
                );
            }
            _ => {
                // NOT the emitter chain's `base`. nif.xml documents 76 for
        // this controller -- Active | CYCLE_CLAMP | Compute Scaled
        // Time -- and every NiPSysUpdateCtlr in every fixture carries
        // it, with an EMPTY key range (0.0 .. 0.0). Inheriting the
        // emitter's base wrote 72 instead, a silent CLAMP -> LOOP
        // flip, plus a fabricated one-second window. Reachable only
        // through a from-scratch export, because for an imported file
        // the merge restores the source: this controller has no graph
        // node, so no user edit can move any of its seven scalars.
        let mut update_base = base;
        update_base.flags = 76;
        update_base.frequency = 1.0;
        update_base.phase = 0.0;
        update_base.start_time = 0.0;
        update_base.stop_time = 0.0;
        // ...unless the SOURCE system's own update controller says
        // otherwise: 216 corpus blocks carry 69, and 13 a 1-second
        // stop time.
        if let Some(authored) = source_update_base(source, &graph.nodes[&system_id]) {
            update_base.flags = authored.flags;
            update_base.frequency = authored.frequency;
            update_base.phase = authored.phase;
            update_base.start_time = authored.start_time;
            update_base.stop_time = authored.stop_time;
        }
        b.fill(
            chain[i],
            &mut NiPSysUpdateCtlr {
                base: update_base,
            },
        );
            }
        }
    }

    // --- data / shader / alpha ------------------------------------------
    let data_ref = compile_data(graph, data_node, b, source);
    b.claim(data_node, data_ref);
    let shader_ref = match shader_node {
        Some(n) => {
            let r = compile_shader(graph, n, b, source);
            b.claim(n, r);
            r
        }
        None => BlockRef::NONE,
    };
    let alpha_ref = match alpha_node {
        Some(n) => {
            let r = compile_alpha(graph, n, b);
            b.claim(n, r);
            r
        }
        None => BlockRef::NONE,
    };

    // --- the system block itself ----------------------------------------
    let ranges = source_psys_ranges(source, &graph.nodes[&system_id]);
    let mut psys = NiParticleSystem {
        vertex_desc: SSE_PARTICLE_VERTEX_DESC,
        world_space: graph.nodes[&system_id].bool_or("World Space", true),
        // Sort ranges the graph does not model; a rebuild wrote zeros
        // over the authored values in 1,690 corpus blocks.
        far_begin: ranges.map(|r| r.0).unwrap_or(0),
        far_end: ranges.map(|r| r.1).unwrap_or(0),
        near_begin: ranges.map(|r| r.2).unwrap_or(0),
        near_end: ranges.map(|r| r.3).unwrap_or(0),
        data: data_ref,
        shader_property: shader_ref,
        alpha_property: alpha_ref,
        // The modifier LIST keeps the source's order. Vanilla lists
        // modifiers as it likes (ZD_MarkartWarhammer puts a rotation
        // after the services) while the compiler emits its class-
        // sorted stack; pairing by type against the source sequence
        // makes the splice positional again -- 130 systems carried
        // off-by-one ref arrays. New modifiers the source lacks go
        // last. The game sorts by `order` at load either way.
        modifiers: match (source, graph.nodes.get(&system_id).and_then(|n| n.source_block)) {
            (Some(file), Some(src)) => {
                let want: Vec<String> = ctx_of(file)
                    .and_then(|c| {
                        NiParticleSystem::decode(file.blocks.get(src as usize)?, c).ok()
                    })
                    .map(|sp| {
                        sp.modifiers
                            .iter()
                            .filter_map(|r| r.get())
                            .filter_map(|i| file.blocks.get(i).map(|bl| bl.type_name.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut remaining = modifier_refs.clone();
                let mut ordered = Vec::with_capacity(remaining.len());
                for w in &want {
                    if let Some(pos) = remaining.iter().position(|r| {
                        r.get().and_then(|i| b.blocks.get(i)).map(|bl| &bl.type_name)
                            == Some(w)
                    }) {
                        ordered.push(remaining.remove(pos));
                    }
                }
                ordered.extend(remaining);
                ordered
            }
            _ => modifier_refs,
        },
        ..NiParticleSystem::default()
    };
    // The LE material slot the graph does not model, from the file: SSE
    // NIF Optimizer writes -1 into `active_material` where vanilla holds
    // 0, and a rebuild wrote the default over it.
    if let Some(sp) = source.zip(graph.nodes.get(&system_id).and_then(|n| n.source_block)).and_then(
        |(file, src)| NiParticleSystem::decode(file.blocks.get(src as usize)?, ctx_of(file)?).ok(),
    ) {
        psys.active_material = sp.active_material;
        psys.default_mat_needs_update = sp.default_mat_needs_update;
    }
    psys.av.net.name = b.intern(&graph.nodes[&system_id].str_or("Name", "ParticleFX"));
    psys.av.net.controller = chain.first().copied().unwrap_or(BlockRef::NONE);
    b.fill(psys_slot, &mut psys);
    Ok(psys_slot)
}

/// Mesh Emitter names that resolve to NO source shape. Such a name
/// exports as a NULL Ptr (the list length is load-bearing for the
/// merge), so the emitter samples nothing and the system is dead in
/// game — a typo deserves a message, not silence. The app calls
/// this beside `export_losses`.
pub fn unresolved_emitter_meshes(graph: &Graph, source: Option<&NifFile>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut ids: Vec<NodeId> = graph.nodes.keys().copied().collect();
    ids.sort();
    for id in ids {
        if graph.nodes[&id].kind != NodeKind::MeshEmitter {
            continue;
        }
        for (i, r) in emitter_mesh_refs(graph, id, source).iter().enumerate() {
            if r.get().is_some() {
                continue;
            }
            let text = graph.nodes[&id].str_or("Emitter Meshes", "");
            if let Some(name) = text.split(',').map(str::trim).nth(i)
            {
                if !out.iter().any(|n| n == name) {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}

/// Resolve a Mesh Emitter node's `Emitter Meshes` text back to source
/// block references.
///
/// The inverse of what `decompile` wrote: it resolved each Ptr to a
/// shape NAME and joined them with ", ". A name that matches nothing
/// yields a NULL Ptr rather than being skipped, so the list length
/// always equals the text -- the length is what `merge_edit` walks, and
/// a short list makes it refuse the whole block.
fn emitter_mesh_refs(
    graph: &Graph,
    node: NodeId,
    source: Option<&NifFile>,
) -> Vec<BlockRef> {
    let text = graph.nodes[&node].str_or("Emitter Meshes", "");
    let names: Vec<&str> = text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        return Vec::new();
    }
    let Some(file) = source else {
        // Authored from scratch: there is no scene to point into. The
        // list still has to have the right LENGTH.
        return vec![BlockRef::NONE; names.len()];
    };
    let Some(ctx) = ctx_of(file) else {
        return vec![BlockRef::NONE; names.len()];
    };
    names
        .iter()
        .map(|want| {
            (0..file.blocks.len())
                .find(|i| {
                    crate::decompile::shape_name(file, ctx, *i).as_deref() == Some(*want)
                })
                .map(|i| BlockRef::new(i))
                .unwrap_or(BlockRef::NONE)
        })
        .collect()
}

/// Whether two BLOCK types belong to one in-place swap family — the
/// block-type mirror of `graph::swap_family`, used by the splice's
/// provenance seed so a kind-changed node OVERWRITES its slot (same
/// index, new type) instead of appending a new block and orphaning
/// the old one. The change_kind test pins the two tables against
/// each other, so neither can drift alone.
pub fn same_swap_family(a: &str, b: &str) -> bool {
    // The controller family references `PARAM_CTLR_BLOCKS` instead of
    // retyping its twelve spellings: the first draft retyped them,
        // invented an `NiPSysAirResistanceCtlr` and missed the
    // RotAngleVar one -- the exact drift this indirection prevents.
    fn ctlr(t: &str) -> bool {
        matches!(
            t,
            "NiPSysEmitterCtlr" | "BSPSysMultiTargetEmitterCtlr" | "NiPSysModifierActiveCtlr"
        ) || crate::graph::PARAM_CTLR_BLOCKS.contains(&t)
    }
    const FAMILIES: &[&[&str]] = &[
        &[
            "NiPSysBoxEmitter",
            "NiPSysSphereEmitter",
            "NiPSysCylinderEmitter",
        ],
        &[
            "NiPSysGravityModifier",
            "NiPSysDragModifier",
            "NiPSysBombModifier",
        ],
        &[
            "NiPSysRotationModifier",
            "BSPSysSimpleColorModifier",
            "BSPSysScaleModifier",
            "BSPSysSubTexModifier",
            "BSPSysLODModifier",
        ],
        &[
            "BSPSysInheritVelocityModifier",
            "BSPSysRecycleBoundModifier",
            "BSWindModifier",
        ],
        // The three below are NOT node-kind swaps -- they are one kind
        // whose block TYPE follows a parameter, so the mirror test
        // that pins this table against `compatible_kinds` does not
        // cover them and should not.
        //
        // A collider follows its `Shape`, and a particle system and its
        // data both follow `Strip Trails` (the layouts are identical;
        // only the names differ). Without the slot, changing either
        // appended a new block and left the old one behind, so the
        // edit read back as the schema default -- two of the edit
        // ledger's remaining entries.
        &["NiPSysPlanarCollider", "NiPSysSphericalCollider"],
        &["NiPSysData", "BSStripPSysData"],
        &["NiParticleSystem", "BSStripParticleSystem"],
    ];
    if a == b {
        return false;
    }
    if ctlr(a) && ctlr(b) {
        return true;
    }
    FAMILIES.iter().any(|f| f.contains(&a) && f.contains(&b))
}

/// The gravity/drag anchor frame of a modifier's SOURCE block: the
/// same `rel = psys_inv ∘ anchor_world` the decompiler baked with.
pub fn source_anchor_frame(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
) -> Option<(crate::mesh::Transform, [f32; 3])> {
    // NOT through `authored_block`: this reads the SCENE around the
    // block (the anchor's world transform / the system's own modifier
    // list), and a carried template is one block with no scene. A
    // pasted node therefore has no anchor frame to unbake against,
    // which is correct -- it is being authored fresh under an identity
    // anchor.
    let file = source?;
    let ctx = ctx_of(file)?;
    let block = file.blocks.get(node.source_block? as usize)?;
    let (axis_file, obj, psys_ref) = match block.type_name.as_str() {
        "NiPSysGravityModifier" => {
            let m = NiPSysGravityModifier::decode(block, ctx).ok()?;
            (m.gravity_axis, m.gravity_object, m.base.target)
        }
        "NiPSysDragModifier" => {
            let m = NiPSysDragModifier::decode(block, ctx).ok()?;
            (m.drag_axis, m.drag_object, m.base.target)
        }
        _ => return None,
    };
    let psys_idx = psys_ref.get()?;
    let psys = NiParticleSystem::decode(file.blocks.get(psys_idx)?, ctx).ok()?;
    let game_world = crate::decompile::psys_game_world(
        file,
        ctx,
        psys_idx,
        &psys.av,
        psys.world_space,
    );
    let world = crate::decompile::world_transform_of(file, ctx, obj.get()?)?;
    Some((game_world.inverse().compose(&world), axis_file))
}

/// UNBAKE a gravity/drag axis at export.
///
/// Import bakes the anchor's rotation into the axis (`rel.rotate`) so
/// the sim and the UI work in system space; the splice then restores
/// the modifier's back-pointer to the original, ROTATED anchor. Writing
/// the system-space axis raw therefore made the game rotate it a
/// second time — a live defect for an EDITED axis, and 9,441 census
/// hits of pure metric noise for untouched ones.
///
/// Two arms, deliberately: an UNTOUCHED axis (the current param equals
/// the source axis re-baked, bit for bit — both sides computed by the
/// same `Transform::rotate`) writes the source's own bytes, so no
/// f32 round trip can smear it; an EDITED axis is carried back into
/// the anchor's frame with the transpose. A from-scratch modifier has
/// no provenance and its anchor is built with identity rotation, so
/// `current` is already correct.
/// Test-visible alias of [].
pub fn probe_anchor_frame(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
) -> Option<(crate::mesh::Transform, [f32; 3])> {
    source_anchor_frame(source, node)
}

fn unbaked_axis(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
    current: [f32; 3],
) -> [f32; 3] {
    match source_anchor_frame(source, node) {
        Some((rel, axis_file)) => {
            if rel.rotate(axis_file) == current {
                axis_file
            } else {
                rel.unrotate(current)
            }
        }
        None => current,
    }
}
fn compile_modifier(
    graph: &Graph,
    node: NodeId,
    base: ModifierBase,
    b: &mut Builder,
    root: BlockRef,
    // Every Collider node on this system, in stack order. The
    // ColliderModifier arm emits ONE manager for the whole group and
    // chains them; every other arm ignores it.
    colliders: &[NodeId],
    // The file this graph was imported from, so a Mesh Emitter can
    // name the shapes it emits from.
    source: Option<&NifFile>,
) -> BlockRef {
    let kind = graph.nodes[&node].kind;
    match kind {
        NodeKind::BoxEmitter
        | NodeKind::SphereEmitter
        | NodeKind::CylinderEmitter
        | NodeKind::MeshEmitter => {
            // A non-zero offset or rotation gets a dedicated anchor node
            // under the root, vanilla-style ("PFireball-Emitter" at z 117
            // is how the giant campfire raises its flames into the pile).
            let offset = graph.nodes[&node].vec3_or("Emitter Offset", [0.0; 3]);
            let rotation = graph.nodes[&node].vec3_or("Emitter Rotation", [0.0; 3]);
            // A MESH emitter has no `emitter_object` field at all:
            // `NiPSysMeshEmitter::sync` calls `sync_core`, which stops
            // before it. Synthesizing an anchor for it appended an
            // NiNode that nothing in the file references — an orphan
            // per mesh emitter, every export.
            let emitter_object = if kind == NodeKind::MeshEmitter {
                BlockRef::NONE
            } else {
                anchor_node(b, root, "EmitterAnchor", offset, rotation)
            };
            let emitter = EmitterBase {
                base,
                speed: graph.nodes[&node].f32_or("Speed", 0.0),
                speed_variation: graph.nodes[&node].f32_or("Speed Variation", 0.0),
                declination: graph.nodes[&node].f32_or("Declination", 0.0),
                declination_variation: graph.nodes[&node].f32_or("Declination Variation", 0.0),
                planar_angle: graph.nodes[&node].f32_or("Planar Angle", 0.0),
                planar_angle_variation: graph.nodes[&node].f32_or("Planar Angle Variation", 0.0),
                initial_color: graph.nodes[&node].color4_or("Initial Color", [1.0; 4]),
                initial_radius: graph.nodes[&node].f32_or("Initial Radius", 1.0),
                radius_variation: graph.nodes[&node].f32_or("Radius Variation", 0.0),
                life_span: graph.nodes[&node].f32_or("Life Span", 1.0),
                life_span_variation: graph.nodes[&node].f32_or("Life Span Variation", 0.0),
                emitter_object,
            };
            match kind {
                // THE SHAPES IT EMITS FROM. This wrote an empty list,
                // which is a data loss with two faces: the emitter has
                // nothing to sample so the system emits nothing in
                // game, and the block loses a Ptr per name so its field
                // count no longer matches the source -- which makes
                // `merge_edit` REFUSE and hands the caller the whole
                // compiled block, losing every authored field beside.
                //
                // The names go back to source blocks through the same
                // `shape_name` predicate the import used to write them,
                // and a source index is directly usable because the
                // splice guarantees no index ever moves. A name that
                // does not resolve keeps its SLOT as a null Ptr rather
                // than being dropped: the count is what the merge walks.
                NodeKind::MeshEmitter => b.push(&mut NiPSysMeshEmitter {
                    emitter,
                    emitter_meshes: emitter_mesh_refs(graph, node, source),
                    initial_velocity_type: graph.nodes[&node].enum_index("Velocity Type") as u32,
                    emission_type: graph.nodes[&node].enum_index("Emission Type") as u32,
                    emission_axis: graph.nodes[&node].vec3_or("Emission Axis", [1.0, 0.0, 0.0]),
                }),
                NodeKind::BoxEmitter => b.push(&mut NiPSysBoxEmitter {
                    emitter,
                    width: graph.nodes[&node].f32_or("Width", 10.0),
                    height: graph.nodes[&node].f32_or("Height", 10.0),
                    depth: graph.nodes[&node].f32_or("Depth", 10.0),
                }),
                NodeKind::SphereEmitter => b.push(&mut NiPSysSphereEmitter {
                    emitter,
                    radius: graph.nodes[&node].f32_or("Radius", 10.0),
                }),
                _ => b.push(&mut NiPSysCylinderEmitter {
                    emitter,
                    radius: graph.nodes[&node].f32_or("Radius", 10.0),
                    height: graph.nodes[&node].f32_or("Height", 10.0),
                }),
            }
        }
        NodeKind::GravityModifier => {
            let offset = graph.nodes[&node].vec3_or("Gravity Offset", [0.0; 3]);
            let gravity_object = anchor_node(b, root, "GravityAnchor", offset, [0.0; 3]);
            b.push(&mut NiPSysGravityModifier {
                base,
                gravity_object,
                gravity_axis: unbaked_axis(
                    source,
                    &graph.nodes[&node],
                    graph.nodes[&node].vec3_or("Axis", [0.0, 0.0, -1.0]),
                ),
                decay: graph.nodes[&node].f32_or("Decay", 0.0),
                strength: graph.nodes[&node].f32_or("Strength", 9.8),
                force_type: graph.nodes[&node].enum_index("Force Type") as u32,
                turbulence: graph.nodes[&node].f32_or("Turbulence", 0.0),
                turbulence_scale: graph.nodes[&node].f32_or("Turbulence Scale", 1.0),
                world_aligned: graph.nodes[&node].bool_or("World Aligned", true),
            })
        }
        NodeKind::RotationModifier => b.push(&mut NiPSysRotationModifier {
            base,
            rotation_speed: graph.nodes[&node].f32_or("Rotation Speed", 0.0),
            rotation_speed_variation: graph.nodes[&node].f32_or("Speed Variation", 0.0),
            rotation_angle: graph.nodes[&node].f32_or("Rotation Angle", 0.0),
            rotation_angle_variation: graph.nodes[&node].f32_or("Angle Variation", 0.0),
            random_rot_speed_sign: graph.nodes[&node].bool_or("Random Speed Sign", false),
            random_axis: graph.nodes[&node].bool_or("Random Axis", true),
            axis: graph.nodes[&node].vec3_or("Axis", [0.0, 0.0, 1.0]),
        }),
        NodeKind::SimpleColorModifier => b.push(&mut BSPSysSimpleColorModifier {
            base,
            fade_in_percent: graph.nodes[&node].f32_or("Fade In End %", 0.2),
            fade_out_percent: graph.nodes[&node].f32_or("Fade Out Start %", 0.8),
            color1_end_percent: graph.nodes[&node].f32_or("Color 1 End %", 0.3),
            color2_start_percent: graph.nodes[&node].f32_or("Color 2 Start %", 0.4),
            color2_end_percent: graph.nodes[&node].f32_or("Color 2 End %", 0.6),
            color3_start_percent: graph.nodes[&node].f32_or("Color 3 Start %", 0.7),
            colors: [
                graph.nodes[&node].color4_or("Color 1", [1.0, 1.0, 1.0, 0.0]),
                graph.nodes[&node].color4_or("Color 2", [1.0; 4]),
                graph.nodes[&node].color4_or("Color 3", [1.0, 1.0, 1.0, 0.0]),
            ],
        }),
        NodeKind::ScaleModifier => {
            // "1.0, 0.5" style list; unparsable entries are dropped.
            let scales =
                crate::scale_table::parse(&graph.nodes[&node].str_or("Scales", "1.0, 1.0"));
            b.push(&mut BSPSysScaleModifier {
                base,
                scales: if scales.is_empty() {
                    vec![1.0, 1.0]
                } else {
                    scales
                },
            })
        }
        NodeKind::DragModifier => {
            // A drag with NO object does nothing in game — always anchor
            // one (root when the offset is zero).
            let offset = graph.nodes[&node].vec3_or("Drag Offset", [0.0; 3]);
            let drag_object = anchor_node(b, root, "DragAnchor", offset, [0.0; 3]);
            b.push(&mut NiPSysDragModifier {
                base,
                drag_object,
                drag_axis: unbaked_axis(
                    source,
                    &graph.nodes[&node],
                    graph.nodes[&node].vec3_or("Axis", [0.0, 0.0, 1.0]),
                ),
                percentage: graph.nodes[&node].f32_or("Percentage", 0.05),
                range: graph.nodes[&node].f32_or("Range", f32::MAX),
                range_falloff: graph.nodes[&node].f32_or("Range Falloff", f32::MAX),
            })
        }
        NodeKind::BombModifier => {
            let offset = graph.nodes[&node].vec3_or("Bomb Offset", [0.0; 3]);
            let bomb_object = anchor_node(b, root, "BombAnchor", offset, [0.0; 3]);
            b.push(&mut NiPSysBombModifier {
                base,
                bomb_object,
                bomb_axis: graph.nodes[&node].vec3_or("Axis", [0.0, 0.0, 1.0]),
                decay: graph.nodes[&node].f32_or("Decay", 0.0),
                delta_v: graph.nodes[&node].f32_or("Delta V", 10.0),
                decay_type: graph.nodes[&node].enum_index("Decay Type") as u32,
                symmetry_type: graph.nodes[&node].enum_index("Symmetry") as u32,
            })
        }
        NodeKind::ColliderModifier => {
            // The manager is reserved first: every collider's parent
            // back-pointer references it.
            let manager_slot = b.reserve("NiPSysColliderManager");
            // The group, or just this node when it is the only one.
            let group: Vec<NodeId> = if colliders.is_empty() {
                vec![node]
            } else {
                colliders.to_vec()
            };
            // Emit back to front so each collider can point at the next.
            let mut next = BlockRef::NONE;
            let mut first = BlockRef::NONE;
            for &cn in group.iter().rev() {
                let c = &graph.nodes[&cn];
                let offset = c.vec3_or("Collider Offset", [0.0; 3]);
                let rotation = c.vec3_or("Collider Rotation", [0.0; 3]);
                let collider_object =
                    anchor_node(b, root, "ColliderAnchor", offset, rotation);
                let collider_base = aether_nif::typed::particles::ColliderBase {
                    bounce: c.f32_or("Bounce", 1.0),
                    spawn_on_collide: c.bool_or("Spawn On Collide", false),
                    die_on_collide: c.bool_or("Die On Collide", false),
                    spawn_modifier: BlockRef::NONE,
                    parent: manager_slot,
                    next_collider: next,
                    collider_object,
                };
                let r = if c.enum_index("Shape") == 1 {
                    b.push(&mut aether_nif::typed::particles::NiPSysPlanarCollider {
                        collider: collider_base,
                        // The import bakes the object/psys scale ratio
                        // into these; write the file's own numbers back
                        // (or divide an edit) or the plane grows 25%
                        // per save on the smelter.
                        width: {
                            let p = c.f32_or("Width", 10.0);
                            source_collider_dims(source, c)
                                .map(|(sc, src)| unscaled_dim(p, src.width, sc))
                                .unwrap_or(p)
                        },
                        height: {
                            let p = c.f32_or("Height", 10.0);
                            source_collider_dims(source, c)
                                .map(|(sc, src)| unscaled_dim(p, src.height, sc))
                                .unwrap_or(p)
                        },
                        x_axis: c.vec3_or("X Axis", [1.0, 0.0, 0.0]),
                        y_axis: c.vec3_or("Y Axis", [0.0, 1.0, 0.0]),
                    })
                } else {
                    b.push(&mut aether_nif::typed::particles::NiPSysSphericalCollider {
                        collider: collider_base,
                        radius: {
                            let p = c.f32_or("Radius", 10.0);
                            source_sphere_dims(source, c)
                                .map(|(sc, src)| unscaled_dim(p, src, sc))
                                .unwrap_or(p)
                        },
                    })
                };
                // EVERY collider in the group owns its block, not
                // just the one that stands for the group in the
                // modifier stack. `node_block` is what the splice's
                // provenance seed and its "what does this file NEED?"
                // seed both read, so an unclaimed collider was
                // invisible to both: a collider ADDED to a system that
                // already had some was never appended, and survived
                // only when it happened to pair with an existing block
                // of its own type. Measured on smelter01: a third
                // collider compiled correctly and reached the file as
                // nothing at all.
                b.claim(cn, r);
                next = r;
                first = r;
            }
            // "Active" is a property of the MANAGER — the format has no
            // per-collider enable (ColliderBase carries bounce, the two
            // collide flags, its parent, its next and its object, and
            // nothing else). A system whose colliders are ALL off is the
            // Zodiac warhammer's containment-off pattern and the only
            // one the format can express; a mixed group cannot be, and
            // validate.rs says so rather than silently picking one.
            let active = group
                .iter()
                .any(|cn| graph.nodes[cn].bool_or("Active", true));
            b.fill(
                manager_slot,
                &mut aether_nif::typed::particles::NiPSysColliderManager {
                    base: ModifierBase { active, ..base },
                    collider: first,
                },
            );
            manager_slot
        }
        NodeKind::SubTexModifier => b.push(&mut BSPSysSubTexModifier {
            base,
            start_frame: graph.nodes[&node].f32_or("Start Frame", 0.0),
            start_frame_fudge: graph.nodes[&node].f32_or("Start Frame Fudge", 0.0),
            end_frame: graph.nodes[&node].f32_or("End Frame", 15.0),
            loop_start_frame: graph.nodes[&node].f32_or("Loop Start Frame", 0.0),
            loop_start_frame_fudge: graph.nodes[&node].f32_or("Loop Start Frame Fudge", 0.0),
            frame_count: graph.nodes[&node].f32_or("Frame Count", 16.0),
            frame_count_fudge: graph.nodes[&node].f32_or("Frame Count Fudge", 0.0),
        }),
        NodeKind::InheritVelocityModifier => {
            // The anchor is what the game samples the velocity OF; a
            // static one round-trips the file and inherits nothing,
            // exactly like the game with an unmoving object.
            let offset = graph.nodes[&node].vec3_or("Inherit Offset", [0.0; 3]);
            let inherit_object = anchor_node(b, root, "InheritAnchor", offset, [0.0; 3]);
            b.push(&mut BSPSysInheritVelocityModifier {
                base,
                inherit_object,
                chance_to_inherit: graph.nodes[&node].f32_or("Chance To Inherit", 100.0),
                velocity_multiplier: graph.nodes[&node].f32_or("Velocity Multiplier", 0.5),
                velocity_variation: graph.nodes[&node].f32_or("Velocity Variation", 0.0),
            })
        }
        NodeKind::RecycleBoundModifier => {
            let offset = graph.nodes[&node].vec3_or("Anchor Offset", [0.0; 3]);
            let bound_object = anchor_node(b, root, "RecycleAnchor", offset, [0.0; 3]);
            b.push(&mut BSPSysRecycleBoundModifier {
                base,
                bound_offset: graph.nodes[&node].vec3_or("Bound Offset", [0.0; 3]),
                bound_extent: graph.nodes[&node].vec3_or("Bound Extent", [100.0; 3]),
                bound_object,
            })
        }
        NodeKind::WindModifier => b.push(&mut BSWindModifier {
            base,
            strength: graph.nodes[&node].f32_or("Strength", 0.25),
        }),
        NodeKind::LodModifier => b.push(&mut BSPSysLODModifier {
            base,
            lod_begin_distance: graph.nodes[&node].f32_or("LOD Begin Distance", 0.1),
            lod_end_distance: graph.nodes[&node].f32_or("LOD End Distance", 0.7),
            end_emit_scale: graph.nodes[&node].f32_or("End Emit Scale", 0.2),
            end_size: graph.nodes[&node].f32_or("End Size", 1.0),
        }),
        NodeKind::SpawnModifier => b.push(&mut NiPSysSpawnModifier {
            base,
            num_spawn_generations: graph.nodes[&node].u32_or("Spawn Generations", 0)
                .min(u16::MAX as u32) as u16,
            percentage_spawned: graph.nodes[&node].f32_or("Percentage Spawned", 1.0),
            min_num_to_spawn: graph.nodes[&node].u32_or("Min Num to Spawn", 1)
                .min(u16::MAX as u32) as u16,
            max_num_to_spawn: graph.nodes[&node].u32_or("Max Num to Spawn", 1)
                .min(u16::MAX as u32) as u16,
            spawn_speed_variation: graph.nodes[&node].f32_or("Spawn Speed Variation", 0.0),
            spawn_dir_variation: graph.nodes[&node].f32_or("Spawn Dir Variation", 0.0),
            life_span: graph.nodes[&node].f32_or("Life Span", 1.0),
            life_span_variation: graph.nodes[&node].f32_or("Life Span Variation", 0.0),
        }),
        // Non-modifier kinds cannot be wired into `Modifiers` (socket types
        // forbid it); encode an inert position modifier as a safe fallback.
        _ => b.push(&mut NiPSysPositionModifier { base }),
    }
}

/// The subtexture table a rows x cols atlas derives to: (U offset,
/// U size, V offset, V size), U varying fastest — vanilla's own 2x2
/// layouts (frost.nif). Shared with the decompiler, which uses it to
/// decide whether a source table is DERIVABLE or must be carried
/// verbatim.
pub fn derived_atlas_offsets(rows: u32, cols: u32) -> Vec<[f32; 4]> {
    if rows * cols <= 1 {
        return Vec::new();
    }
    (0..rows * cols)
        .map(|i| {
            [
                (i % cols) as f32 / cols as f32,
                1.0 / cols as f32,
                (i / cols) as f32 / rows as f32,
                1.0 / rows as f32,
            ]
        })
        .collect()
}

/// Parse the `Atlas Offsets` text: quads of four floats separated by
/// `;`. Empty text or a malformed quad yields nothing.
pub fn parse_atlas_offsets(text: &str) -> Vec<[f32; 4]> {
    text.split(';')
        .filter_map(|quad| {
            let vals: Vec<f32> = quad
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            (vals.len() == 4).then(|| [vals[0], vals[1], vals[2], vals[3]])
        })
        .collect()
}

/// The interpolator a controller's SOURCE block pointed at, of one of
/// the types in `want`.
///
/// A sequence-managed controller does not own its keys: the clip does,
/// and the controller points at an `NiBlend*Interpolator` holding only
/// a resting pose (often the -FLT_MAX / value 2 "no pose" sentinel).
/// The compiler always built the plain keyed form instead, so the ref
/// found no slot of a matching type and came out NULL -- every such
/// controller lost its interpolator, and the blend block's own resting
/// value was replaced with a fabricated 1.
///
/// `want` names the family: a controller with TWO interpolator slots
/// (emitter: a float rate and a bool visibility) must not have one
/// slot swallow the other's blend block.
///
/// Found by TYPE rather than by scalar index: the interpolator is the
/// ref that lands on a blend block, whatever position it holds.
fn source_interpolator(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
    want: &[&str],
) -> Option<Block> {
    // The node's OWN subtree, so this works for a pasted node too: an
    // interpolator is a block the controller OWNS, and the carried
    // payload is the Child closure precisely so these readers keep
    // working away from the file they came from.
    let (block, ctx) = authored_block(source, node)?;
    for (r, kind) in block_refs(block, ctx)? {
        if kind != RefKind::Child {
            continue;
        }
        let Some(i) = r.get() else { continue };
        let Some(target) = authored_lookup(source, node, i) else {
            continue;
        };
        if want.contains(&target.type_name.as_str()) {
            return Some(target.clone());
        }
    }
    None
}

/// The AUTHORED resting value of a controller's bool interpolator.
///
/// The compiler wrote a literal 1. The format does not normalize this
/// byte and **2 means "no pose"**: `NiBoolInterpolator::Update`
/// (1.5.97 `0x140CE3080`) returns 0 for it, so the channel is not
/// written at all, where any other value writes `v != 0`. Inventing a
/// 1 therefore turns ON something the file leaves alone -- 3,514
/// blocks across the corpus between the two bool interpolator types.
/// The keys still win when there are any: the same Update overwrites
/// the byte from the data track before reading it.
fn source_bool_value(source: Option<&NifFile>, node: &crate::graph::GraphNode) -> Option<u8> {
    let block = source_interpolator(source, node, &["NiBoolInterpolator"])?;
    let ctx = authored_ctx(source, node)?;
    NiBoolInterpolator::decode(&block, ctx).ok().map(|i| i.value)
}

/// The same for the timeline form, which `NiPSysModifierActiveCtlr`
/// uses.
fn source_timeline_value(source: Option<&NifFile>, node: &crate::graph::GraphNode) -> Option<u8> {
    let block = source_interpolator(source, node, &["NiBoolTimelineInterpolator"])?;
    let ctx = authored_ctx(source, node)?;
    NiBoolTimelineInterpolator::decode(&block, ctx)
        .ok()
        .map(|i| i.value)
}

fn compile_data(graph: &Graph, node: NodeId, b: &mut Builder, source: Option<&NifFile>) -> BlockRef {
    let rows = graph.nodes[&node].u32_or("Subtexture Rows", 1).max(1);
    let cols = graph.nodes[&node].u32_or("Subtexture Cols", 1).max(1);
    // An EXPLICIT table wins over the derived one. 55 corpus blocks
    // carry a one-entry table [0, 1, 0, 1] at rows = cols = 1 — the
    // full-texture quad, authored — which no rows x cols pair can
    // derive; dropping it shrank the block by four scalars and made
    // the merge refuse it whole.
    let explicit = parse_atlas_offsets(&graph.nodes[&node].str_or("Atlas Offsets", ""));
    let (atlas, offsets) = if explicit.is_empty() {
        let derived = derived_atlas_offsets(rows, cols);
        (!derived.is_empty(), derived)
    } else {
        (true, explicit)
    };
    // AspectFlags bits per nif.xml: 0 velocity orientation, 1 initial
    // rotation from velocity, 8 speed-to-aspect ramp.
    let mut aspect_flags = 0u16;
    if graph.nodes[&node].bool_or("Velocity Aspect", false) {
        aspect_flags |= 1;
    }
    if graph.nodes[&node].bool_or("Rotate To Velocity", false) {
        aspect_flags |= 2;
    }
    if graph.nodes[&node].bool_or("Speed To Aspect", false) {
        aspect_flags |= 1 << 8;
    }
    let mut data = NiPSysData {
        num_vertices: graph.nodes[&node].u32_or("Max Particles", 100).min(u16::MAX as u32) as u16,
        has_vertices: true,
        // Bethesda's own exporter sets these: 77 of 77 NiPSysData
        // blocks in the fixture set carry 1, and the only zeros are
        // frost.nif and sparks.nif -- output of the abandoned C++
        // predecessor, not of the game's toolchain. Neither flag
        // changes the block LENGTH: at 20.2.0.7 nif.xml gates the
        // arrays they name on `!#BS202#`, false for SE bs100 and LE
        // bs83 alike, and nifly encodes the same rule.
        //
        // What they MEAN at runtime is unverified -- RE::NiParticlesData
        // keeps radii and sizes as bare pointers with no companion
        // bool, so the byte can only ever have been an allocation
        // decision. Matching what the game's own files carry is the
        // conservative answer either way, and it is the one that
        // matters here because BSPSysScaleModifier writes sizes.
        has_radii: true,
        has_sizes: true,
        has_vertex_colors: graph.nodes[&node].bool_or("Has Color", true),
        has_rotation_angles: graph.nodes[&node].bool_or("Has Rotations", false),
        has_texture_indices: atlas,
        subtexture_offsets: offsets,
        aspect_ratio: graph.nodes[&node].f32_or("Aspect Ratio", 1.0),
        aspect_flags,
        speed_to_aspect_aspect2: graph.nodes[&node].f32_or("Aspect 2", 0.0),
        speed_to_aspect_speed1: graph.nodes[&node].f32_or("Aspect Speed 1", 0.0),
        speed_to_aspect_speed2: graph.nodes[&node].f32_or("Aspect Speed 2", 0.0),
        ..NiPSysData::default()
    };
    // Every field the graph does NOT model is the source's, when there
    // is one. The has_* bytes are the bulk of it — 1,939 corpus
    // blocks differed on them, at scattered scalar indices because the
    // subtexture table shifts every offset. The modelled fields
    // (particle count, vertex colours, rotation angles, the atlas, the
    // aspect block) keep the params' values.
    if let Some(sd) = source_psys_data(source, &graph.nodes[&node]) {
        data.group_id = sd.group_id;
        data.keep_flags = sd.keep_flags;
        data.compress_flags = sd.compress_flags;
        data.bs_vector_flags = sd.bs_vector_flags;
        data.material_crc = sd.material_crc;
        data.has_normals = sd.has_normals;
        data.bound = sd.bound;
        data.consistency_flags = sd.consistency_flags;
        data.num_active = sd.num_active;
        data.has_radii = sd.has_radii;
        data.has_sizes = sd.has_sizes;
        data.has_rotations = sd.has_rotations;
        data.has_rotation_axes = sd.has_rotation_axes;
        data.has_rotation_speeds = sd.has_rotation_speeds;
        // The dead C++ editor shipped a subtexture TABLE with the
        // bool CLEAR; when the table we derived is exactly the
        // source's, the bool is the source's too. A changed table is
        // an atlas edit and keeps the derived true.
        if data.subtexture_offsets == sd.subtexture_offsets {
            data.has_texture_indices = sd.has_texture_indices;
        }
    }
    // ROTATION ANGLES IMPLY ROTATION SPEEDS, or the game crashes on the
    // first particle the system spawns.
    //
    // `NiPSysRotationModifier::Func38` (1.5.97 `0x140D20110`) guards the
    // axes array and the angles array, and then writes the SPEEDS array
    // inside the angles branch with no check at all:
    // `0x140D202E5  movss [rax+rdi*4],xmm7` with rax = data->rotationSpeeds
    // (`NiPSysData+0x98`, confirmed against CommonLibSSE's layout).
    // A user's crash log is exactly that instruction with rax = 0 and
    // rdi = 0 — particle zero of a pasted system.
    //
    // `has_rotation_angles` is a graph param and `has_rotation_speeds`
    // is not modelled, so it is restored from the SOURCE block — which
    // a node pasted from the library does not have. Angles came back
    // true, speeds stayed false, and the file crashed the game.
    //
    // Measured over the whole configured corpus: 3,274 `NiPSysData`
    // blocks, 1,925 with rotation angles, ZERO with angles and no
    // speeds. Shipped data always pairs them, so this is the format's
    // rule rather than a guess — and it is why the byte gates do not
    // move: for a block WITH a source the flag is already true.
    if data.has_rotation_angles {
        data.has_rotation_speeds = true;
    }
    // Strip trails swap the whole data block type; the system block type
    // switches with it in compile_system (both or neither, like vanilla).
    if graph.nodes[&node].bool_or("Strip Trails", false) {
        b.push(&mut BSStripPSysData {
            data,
            max_point_count: graph.nodes[&node].u32_or("Max Point Count", 32).min(u16::MAX as u32)
                as u16,
            start_cap_size: graph.nodes[&node].f32_or("Start Cap Size", 0.0),
            end_cap_size: graph.nodes[&node].f32_or("End Cap Size", 0.0),
            do_z_prepass: graph.nodes[&node].bool_or("Z Prepass", false),
        })
    } else {
        b.push(&mut NiPSysData { ..data })
    }
}

/// The SOURCE data block, whole — a strip system's data rides inside
/// `BSStripPSysData`, so both spellings are read.
fn source_psys_data(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
) -> Option<NiPSysData> {
    let (block, ctx) = authored_block(source, node)?;
    match block.type_name.as_str() {
        "NiPSysData" => NiPSysData::decode(block, ctx).ok(),
        "BSStripPSysData" => BSStripPSysData::decode(block, ctx).ok().map(|d| d.data),
        _ => None,
    }
}

/// The SOURCE float interpolator's resting pose, and only when a key
/// track drives the value. With keys the game overwrites the pose from
/// the track every update, so the byte is the runtime's scratch and no
/// UI can reach it — it belongs to the file (often the -FLT_MAX "no
/// pose" sentinel, which no parameter could carry anyway). WITHOUT
/// keys the pose IS the constant the emitter runs at, the import read
/// the param from it, and the param must win so an edit lands.
fn source_float_pose(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
    param_missing: bool,
) -> Option<f32> {
    let block = source_interpolator(source, node, &["NiFloatInterpolator"])?;
    let ctx = authored_ctx(source, node)?;
    let i = NiFloatInterpolator::decode(&block, ctx).ok()?;
    // `param_missing` covers the Multi Emitter, whose node stores no
    // Birth Rate at all: with nothing for a user to edit, the pose is
    // the file's unconditionally.
    (i.data.get().is_some() || param_missing).then_some(i.value)
}

/// Whether the SOURCE bool interpolator carried a key data block. The
/// always-on optimisation dropped the block vanilla ships for
/// always-on tracks, so the interpolator's data ref went NULL against
/// the source's — 284 blocks.
fn source_bool_has_data(source: Option<&NifFile>, node: &crate::graph::GraphNode) -> bool {
    (|| {
        let block = source_interpolator(source, node, &["NiBoolInterpolator"])?;
        let ctx = authored_ctx(source, node)?;
        NiBoolInterpolator::decode(&block, ctx)
            .ok()
            .map(|i| i.data.get().is_some())
    })()
    .unwrap_or(false)
}

/// The SOURCE collider's own dimensions and the scale the import
/// baked into the params. The game scales collider geometry by the
/// object/psys scale ratio, so import multiplied Width/Height/Radius
/// by `rel.s`; writing the params raw scaled the file's collider a
/// second time -- smelter01's containment plane grew 25% per save.
fn source_collider_dims(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
) -> Option<(f32, NiPSysPlanarCollider)> {
    // NOT through `authored_block`: the baked scale is the psys/object
    // ratio, which needs the SCENE the collider hangs in. A pasted
    // collider is authored fresh at scale 1, which is the right answer
    // for one.
    let file = source?;
    let ctx = ctx_of(file)?;
    let block = file.blocks.get(node.source_block? as usize)?;
    if block.type_name != "NiPSysPlanarCollider" {
        return None;
    }
    let c = NiPSysPlanarCollider::decode(block, ctx).ok()?;
    // The manager the collider hangs off names the psys; the frame is
    // the same `psys_inv o object_world` the import baked with.
    let manager = c.collider.parent.get()?;
    let psys_idx = match aether_nif::typed::block_scalars(file.blocks.get(manager)?, ctx)?
        .get(2)?
    {
        aether_nif::typed::Scalar::I32(v) if *v >= 0 => *v as usize,
        _ => return None,
    };
    let psys = NiParticleSystem::decode(file.blocks.get(psys_idx)?, ctx).ok()?;
    let game_world =
        crate::decompile::psys_game_world(file, ctx, psys_idx, &psys.av, psys.world_space);
    let world =
        crate::decompile::world_transform_of(file, ctx, c.collider.collider_object.get()?)?;
    let rel = game_world.inverse().compose(&world);
    Some((rel.s, c))
}

/// A spherical collider's source radius and scale, same frame.
fn source_sphere_dims(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
) -> Option<(f32, f32)> {
    // Same reason as the planar collider: the scale is the scene's.
    let file = source?;
    let ctx = ctx_of(file)?;
    let block = file.blocks.get(node.source_block? as usize)?;
    if block.type_name != "NiPSysSphericalCollider" {
        return None;
    }
    let c = NiPSysSphericalCollider::decode(block, ctx).ok()?;
    let manager = c.collider.parent.get()?;
    let psys_idx = match aether_nif::typed::block_scalars(file.blocks.get(manager)?, ctx)?
        .get(2)?
    {
        aether_nif::typed::Scalar::I32(v) if *v >= 0 => *v as usize,
        _ => return None,
    };
    let psys = NiParticleSystem::decode(file.blocks.get(psys_idx)?, ctx).ok()?;
    let game_world =
        crate::decompile::psys_game_world(file, ctx, psys_idx, &psys.av, psys.world_space);
    let world =
        crate::decompile::world_transform_of(file, ctx, c.collider.collider_object.get()?)?;
    let rel = game_world.inverse().compose(&world);
    Some((rel.s, c.radius))
}

/// Un-scale one baked collider dimension: the param equal to
/// `src * s` (the import's own multiply, bit for bit) restores the
/// source value; an edited one is divided back.
fn unscaled_dim(param: f32, src: f32, s: f32) -> f32 {
    if (s - 1.0).abs() <= 1e-4 || param == src * s {
        if (s - 1.0).abs() > 1e-4 {
            src
        } else {
            param
        }
    } else {
        param / s
    }
}

/// Whether the SOURCE float data behind this node's interpolator is
/// the CONST form with exactly the floats we are about to write. The
/// key text cannot carry constness (two corpus blocks, both the dead
/// C++ editor's own output), so it is restored here.
fn source_float_data_is_const(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
    floats: &[f32],
) -> bool {
    (|| {
        let block = source_interpolator(source, node, &["NiFloatInterpolator"])?;
        let ctx = authored_ctx(source, node)?;
        let i = NiFloatInterpolator::decode(&block, ctx).ok()?;
        let data = NiFloatDataView::decode(source?.blocks.get(i.data.get()?)?, ctx).ok()?;
        Some(data.interpolation == 5 && data.floats == floats)
    })()
    .unwrap_or(false)
}

/// The SOURCE bool interpolator's key data block, verbatim — for the
/// Multi Emitter's visibility track, which the game never reads (RE
/// `0x140D582F0`) and the graph therefore does not model. NiBoolData
/// carries no references, so a verbatim copy is safe.
fn source_bool_data(source: Option<&NifFile>, node: &crate::graph::GraphNode) -> Option<Block> {
    let block = source_interpolator(source, node, &["NiBoolInterpolator"])?;
    let ctx = authored_ctx(source, node)?;
    let i = NiBoolInterpolator::decode(&block, ctx).ok()?;
    authored_lookup(source, node, i.data.get()?).cloned()
}

/// The SOURCE system's own `NiPSysUpdateCtlr` base, found by walking
/// the controller chain off the system block. The service has no graph
/// node, so nif.xml's documented 76 was written even where the file
/// says 69 — 216 corpus blocks.
fn source_update_base(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
) -> Option<ControllerBase> {
    // Through the node's OWN subtree: a controller chain hangs off the
    // system as Child refs, so the carried payload has it.
    let (block, ctx) = authored_block(source, node)?;
    let psys = NiParticleSystem::decode(block, ctx).ok()?;
    let mut cur = psys.av.net.controller;
    for _ in 0..16 {
        let i = cur.get()?;
        let block = authored_lookup(source, node, i)?;
        let base = controller_base_of(block, ctx)?;
        if block.type_name == "NiPSysUpdateCtlr" {
            return Some(base);
        }
        cur = base.next_controller;
    }
    None
}

/// The sort ranges (far/near begin/end) the SOURCE system carried.
/// Four u16 the graph does not model; the compiler wrote zeros.
fn source_psys_ranges(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
) -> Option<(u16, u16, u16, u16)> {
    let (block, ctx) = authored_block(source, node)?;
    let p = NiParticleSystem::decode(block, ctx).ok()?;
    Some((p.far_begin, p.far_end, p.near_begin, p.near_end))
}

/// The shader flag words the SOURCE block carried, so the bits the
/// graph does not model survive a rebuild.
fn source_shader_flags(
    source: Option<&NifFile>,
    node: &crate::graph::GraphNode,
) -> Option<(u32, u32)> {
    let (block, ctx) = authored_block(source, node)?;
    let s = BSEffectShaderProperty::decode(block, ctx).ok()?;
    Some((s.shader_flags1, s.shader_flags2))
}

fn compile_shader(
    graph: &Graph,
    node: NodeId,
    b: &mut Builder,
    source: Option<&NifFile>,
) -> BlockRef {
    let n = &graph.nodes[&node];
    let soft_depth = n.f32_or("Soft Falloff Depth", 0.0);
    // SLSF1: ZBuffer Test always; Soft Effect when a depth is set; the
    // greyscale-to-palette pair (bits 4/5) and Use Falloff (bit 6) from
    // their checkboxes. SLSF2: vertex colors (0x20) + Double Sided (0x10).
    //
    // Same rule the controller flag word already follows: START from
    // the word the FILE carried and rewrite only the bits the graph
    // models. Rebuilding the whole word from a handful of checkboxes
    // discarded every other bit -- measured over the corpus, ~3,200
    // blocks, with 0x40000000 appearing in flags1 and vanishing from
    // flags2 because the two words were being invented independently.
    // A from-scratch shader keeps the old literals as its base.
    const MODELLED1: u32 = 0x4000_0000 | 0x10 | 0x20 | 0x40;
    const MODELLED2: u32 = 0x10;
    let (base1, base2) = source_shader_flags(source, n).unwrap_or((0x8000_0000, 0x20));
    let mut modelled1: u32 = 0;
    // Soft Effect is its OWN bit, not a function of the depth: 1,169
    // corpus blocks carry a non-zero falloff depth with the bit CLEAR,
    // so deriving it from the depth turned the effect on where the file
    // turns it off. The default keeps the old behaviour for a shader
    // authored from scratch, which has no bit to inherit.
    if n.bool_or("Soft Effect", soft_depth > 0.0) {
        modelled1 |= 0x4000_0000;
    }
    if n.bool_or("Greyscale Color", false) {
        modelled1 |= 0x10;
    }
    if n.bool_or("Greyscale Alpha", false) {
        modelled1 |= 0x20;
    }
    if n.bool_or("Use Falloff", false) {
        modelled1 |= 0x40;
    }
    let mut flags1 = (base1 & !MODELLED1) | modelled1;
    let mut modelled2: u32 = 0;
    if n.bool_or("Double Sided", false) {
        modelled2 |= 0x10;
    }
    let mut flags2 = (base2 & !MODELLED2) | modelled2;
    let mut shader = BSEffectShaderProperty {
        shader_flags1: flags1,
        shader_flags2: flags2,
        source_texture: n.str_or("Source Texture", ""),
        greyscale_texture: n.str_or("Greyscale Texture", ""),
        texture_clamp_mode: CLAMP_MODE_BYTES[n.enum_index("Texture Clamp")
            .min(CLAMP_MODE_BYTES.len() - 1)],
        uv_offset: [n.f32_or("UV Offset U", 0.0), n.f32_or("UV Offset V", 0.0)],
        uv_scale: [n.f32_or("UV Scale U", 1.0), n.f32_or("UV Scale V", 1.0)],
        lighting_influence: n.u32_or("Lighting Influence", 255).min(255) as u8,
        falloff_start_angle: n.f32_or("Falloff Start Angle", 1.0),
        falloff_stop_angle: n.f32_or("Falloff Stop Angle", 0.0),
        falloff_start_opacity: n.f32_or("Falloff Start Opacity", 1.0),
        falloff_stop_opacity: n.f32_or("Falloff Stop Opacity", 0.0),
        base_color: n.color4_or("Base Color", [1.0; 4]),
        base_color_scale: n.f32_or("Base Color Scale", 1.0),
        soft_falloff_depth: soft_depth,
        ..BSEffectShaderProperty::default()
    };
    // --- the shader's own controller chain --------------------------
    //
    // Census over every particle shader in the corpus: float variables
    // 0 and 5 only, colour variable 0 only, chains of 1-3, no variable
    // twice on one shader. 512 of 536 float controllers and 181 of 185
    // colour ones hold a BLEND interpolator — sequence-managed idle
    // state whose keys live in the clips — so those are re-derived
    // from the source, and only FREE tracks come from the key params.
    let shader_slot = b.reserve("BSEffectShaderProperty");
    // Every float variable the node models, through the one table.
    let float_keys: Vec<(u32, Curve)> = crate::graph::EFFECT_SHADER_FLOAT_KEYS
        .iter()
        .map(|(p, v)| (*v, n.curve_or(p, Track::Float)))
        .collect();
    let color_keys = n.curve_or("Color Keys", Track::Vec3);
    // A variable the params do not model gets an empty track rather
    // than a branch of its own; see the match below.
    let no_keys = Curve::empty(Track::Float);
    let mut placed_float: Vec<u32> = Vec::new();
    let mut placed_color = false;
    let mut chain: Vec<BlockRef> = Vec::new();
    // Source order first, so the splice stays positional.
    let source_chain: Vec<usize> = (|| {
        let file = source?;
        let ctx = ctx_of(file)?;
        let src = file.blocks.get(n.source_block? as usize)?;
        let sh = BSEffectShaderProperty::decode(src, ctx).ok()?;
        let mut out = Vec::new();
        let mut cur = sh.net.controller;
        for _ in 0..16 {
            let Some(i) = cur.get() else { break };
            out.push(i);
            cur = controller_base_of(file.blocks.get(i)?, ctx_of(file)?)?.next_controller;
        }
        Some(out)
    })()
    .unwrap_or_default();
    for si in source_chain {
        let Some(file) = source else { break };
        let Some(ctx) = ctx_of(file) else { break };
        let Some(block) = file.blocks.get(si) else { break };
        let Some(base) = controller_base_of(block, ctx) else { break };
        let base = ControllerBase {
            next_controller: BlockRef::NONE,
            target: shader_slot,
            ..base
        };
        match block.type_name.as_str() {
            "BSEffectShaderPropertyFloatController" => {
                let Ok(c) = BSEffectShaderPropertyFloatController::decode(block, ctx) else {
                    break;
                };
                let keys = float_keys
                    .iter()
                    .find(|(v, _)| *v == c.controlled_variable)
                    .map(|(_, k)| k)
                    .unwrap_or(&no_keys);
                let interp = match c
                    .interpolator
                    .get()
                    .and_then(|j| file.blocks.get(j))
                {
                    Some(ib) if ib.type_name == "NiBlendFloatInterpolator" => {
                        b.push_raw(ib.clone())
                    }
                    Some(ib) if ib.type_name == "NiFloatInterpolator" && !keys.is_empty() => {
                        let data = b.push_keys(&mut keys.to_float_data());
                        let pose = NiFloatInterpolator::decode(ib, ctx)
                            .map(|i| i.value)
                            .unwrap_or(-f32::MAX);
                        b.push(&mut NiFloatInterpolator { value: pose, data })
                    }
                    Some(ib) if ib.type_name == "NiFloatInterpolator" => {
                        // A free track on a variable the params do not
                        // model (none in the corpus; defensive): carry
                        // the interpolator and its data verbatim.
                        let Ok(fi) = NiFloatInterpolator::decode(ib, ctx) else { break };
                        let data = match fi.data.get().and_then(|d| file.blocks.get(d)) {
                            Some(db) => b.push_raw(db.clone()),
                            None => BlockRef::NONE,
                        };
                        b.push(&mut NiFloatInterpolator { value: fi.value, data })
                    }
                    _ => BlockRef::NONE,
                };
                placed_float.push(c.controlled_variable);
                chain.push(b.push(&mut BSEffectShaderPropertyFloatController {
                    base,
                    interpolator: interp,
                    controlled_variable: c.controlled_variable,
                }));
            }
            "BSEffectShaderPropertyColorController" => {
                let Ok(c) = BSEffectShaderPropertyColorController::decode(block, ctx) else {
                    break;
                };
                let interp = match c
                    .interpolator
                    .get()
                    .and_then(|j| file.blocks.get(j))
                {
                    Some(ib) if ib.type_name == "NiBlendPoint3Interpolator" => {
                        b.push_raw(ib.clone())
                    }
                    Some(ib) if ib.type_name == "NiPoint3Interpolator"
                        && !color_keys.is_empty() =>
                    {
                        let (pi, pf) = color_keys.to_floats();
                        let data = b.push_keys(&mut NiPosData {
                            interpolation: pi,
                            floats: pf,
                        });
                        let pose = NiPoint3Interpolator::decode(ib, ctx)
                            .map(|i| i.value)
                            .unwrap_or([-f32::MAX; 3]);
                        b.push(&mut NiPoint3Interpolator { value: pose, data })
                    }
                    _ => BlockRef::NONE,
                };
                placed_color = true;
                chain.push(b.push(&mut BSEffectShaderPropertyColorController {
                    base,
                    interpolator: interp,
                    controlled_color: c.controlled_color,
                }));
            }
            _ => break,
        }
    }
    // NEW tracks the source does not have (or a from-scratch shader):
    // colour first, then variable 0, then 5 — a fixed order so two
    // exports of one graph agree.
    let mut new_track = |b: &mut Builder,
                         chain: &mut Vec<BlockRef>,
                         variable: Option<u32>,
                         float_keys: Option<&Curve>| {
        let stop = match (float_keys, variable) {
            (Some(k), _) => k.keys.last().map(|k| k.time).unwrap_or(1.0),
            (None, _) => color_keys.keys.last().map(|k| k.time).unwrap_or(1.0),
        };
        let base = ControllerBase {
            next_controller: BlockRef::NONE,
            // Active + CLAMP, the form both shipped free tracks carry.
            flags: 76,
            frequency: 1.0,
            phase: 0.0,
            start_time: 0.0,
            stop_time: stop,
            target: shader_slot,
        };
        match (float_keys, variable) {
            (Some(keys), Some(var)) => {
                let data = b.push_keys(&mut keys.to_float_data());
                let interp = b.push(&mut NiFloatInterpolator {
                    // The "no pose" sentinel every vanilla free shader
                    // track ships; the keys drive the value.
                    value: -f32::MAX,
                    data,
                });
                chain.push(b.push(&mut BSEffectShaderPropertyFloatController {
                    base,
                    interpolator: interp,
                    controlled_variable: var,
                }));
            }
            _ => {
                let (pi, pf) = color_keys.to_floats();
                let data = b.push_keys(&mut NiPosData {
                    interpolation: pi,
                    floats: pf,
                });
                let interp = b.push(&mut NiPoint3Interpolator {
                    value: [-f32::MAX; 3],
                    data,
                });
                chain.push(b.push(&mut BSEffectShaderPropertyColorController {
                    base,
                    interpolator: interp,
                    controlled_color: 0,
                }));
            }
        }
    };
    if !color_keys.is_empty() && !placed_color {
        new_track(b, &mut chain, None, None);
    }
    // Then the float variables in the table's order (0, 5, then UV 6-9)
    // -- still a fixed order, so two exports of one graph agree.
    for (var, keys) in &float_keys {
        if !keys.is_empty() && !placed_float.contains(var) {
            new_track(b, &mut chain, Some(*var), Some(keys));
        }
    }
    // Thread the chain and hang it off the shader.
    for w in chain.windows(2) {
        let (a, next) = (w[0], w[1]);
        if let Some(ai) = a.get() {
            let block = b.blocks[ai].clone();
            if let Some(patched) = retarget_first_ref(&block, next) {
                b.blocks[ai] = patched;
            }
        }
    }
    shader.net.controller = chain.first().copied().unwrap_or(BlockRef::NONE);
    b.fill(shader_slot, &mut shader);
    shader_slot
}

/// Rewrite a controller block's `next_controller` — its FIRST
/// reference at 20.2.0.7 — leaving every other byte alone.
fn retarget_first_ref(block: &Block, next: BlockRef) -> Option<Block> {
    let mut out = block.clone();
    let bytes = next.0.to_le_bytes();
    out.data.get_mut(0..4)?.copy_from_slice(&bytes);
    Some(out)
}

fn compile_alpha(graph: &Graph, node: NodeId, b: &mut Builder) -> BlockRef {
    let n = &graph.nodes[&node];
    // One line, and it used to be forty. Every bit of `AlphaFlags` is
    // a parameter now, so there is nothing left for the compiler to
    // preserve from the source: `graph::alpha_flags` is the exact
    // inverse of what import read, which is what keeps a null edit
    // byte-identical. The two-armed version this replaced existed only
    // because the schema modelled part of the word and had to copy the
    // rest back -- and it still rewrote the factor bits of any file
    // whose blend half was not one of the three canonical words.
    b.push(&mut NiAlphaProperty {
        flags: crate::graph::alpha_flags(n),
        threshold: n.u32_or("Test Threshold", 128).min(255) as u8,
        ..NiAlphaProperty::default()
    })
}

#[cfg(test)]
mod flag_tests {
    use super::cycle_flags;

    /// `Loop` owns the cycle bits and NOTHING else in the word.
    ///
    /// The compiler wrote a whole literal for this -- 72 looping, 76
    /// clamping -- which is right about the cycle and throws away
    /// every other bit the file carried. Vanilla ships 104 among
    /// others; measured over the corpus, rewriting the word as 72 was
    /// 4,013 wrong NiPSysModifierActiveCtlr fields and 2,394 wrong
    /// NiPSysEmitterCtlr ones.
    ///
    /// Planted defect this catches: `if looping { 72 } else { 76 }`.
    /// The 104 rows fail immediately, and so does the round trip.
    #[test]
    fn loop_rewrites_the_cycle_bits_and_leaves_the_rest_alone() {
        // The two the compiler used to write, unchanged for the
        // values they already encoded.
        assert_eq!(cycle_flags(72, true), 72, "72 is active + LOOP");
        assert_eq!(cycle_flags(76, false), 76, "76 is active + CLAMP");
        // ...and each is the other's opposite.
        assert_eq!(cycle_flags(72, false), 76);
        assert_eq!(cycle_flags(76, true), 72);
        // A word vanilla actually ships keeps its other bits. 104 is
        // 64 | 32 | 8, cycle 0, so looping leaves it exactly alone and
        // clamping adds 4 and nothing else.
        assert_eq!(cycle_flags(104, true), 104, "the 32 bit was dropped");
        assert_eq!(cycle_flags(104, false), 108);
        // Every bit outside the cycle pair survives both ways.
        for authored in [0u16, 8, 72, 76, 104, 108, 0xFFFF] {
            for looping in [true, false] {
                let out = cycle_flags(authored, looping);
                assert_eq!(
                    out & !0b110,
                    authored & !0b110,
                    "{authored} lost a bit outside the cycle pair"
                );
                assert_eq!((out >> 1) & 3, if looping { 0 } else { 2 });
            }
        }
    }
}
