//! The NIF's block graph as a flat, virtualizable tree.
//!
//! **It walks the FILE, not `MeshScene`.** The extracted scene is a
//! rendering product: it covers 2-27% of a file's blocks (27 of 180 in
//! the giant campfire, 7 of 231 in the aedric scepter), a
//! `NiParticleSystem` is neither one of its nodes nor one of its meshes,
//! a `NiSwitchNode`'s unselected children are absent by design, and its
//! sibling order is REVERSED against the file because the walk is LIFO.
//! A tree built on it would disagree with NifSkope, with the block
//! indices the breadcrumb prints, and with the file itself.
//!
//! So the walk is over `block_refs(.., Child)` in field order, and
//! `MeshScene` is JOINED to it by block index — which is exactly what
//! `SceneNode.block` and `RenderMesh.block` were added for.
//!
//! The output is a flat `Vec<TreeRow>` of only the VISIBLE rows.
//! Flattened and not nested for the reason `navigator.rs` already
//! records: a nested layout lays out every child every frame, and the
//! corpus reaches 9,485 blocks in one file.

use crate::mesh::MeshScene;
use aether_nif::typed::{block_name, block_refs, has_typed_view, Ctx, RefKind};
use aether_nif::NifFile;
use std::collections::{BTreeMap, BTreeSet};

/// What a row stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A container: `NiNode` and its variants.
    SceneNode,
    /// Drawn geometry.
    Shape,
    /// A particle system. Its apparatus hangs behind one folded row.
    ParticleSystem,
    /// The stand-in for a particle system's apparatus — the modifiers,
    /// controllers, interpolators and key data the node graph already
    /// shows. Expanding it is deliberate, not the default.
    ApparatusFold { blocks: usize },
    /// Anything else: properties, extra data, collision, controllers.
    Other,
}

/// What the tree says about a row without the user opening it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Badges {
    /// Translation / rotation / scale driven at runtime.
    ///
    /// PER CHANNEL, not per node, and that is a measured decision:
    /// 9,764 of 125,133 corpus nodes are driven, but 3,048 of those keep
    /// at least one authored channel — 1,186 keep their authored
    /// TRANSLATION, which is exactly what a move gizmo writes. A
    /// per-node warning would cry wolf on a third of the cases it fires
    /// on.
    pub driven: [bool; 3],
    /// The shape carries the hidden flag and the game does not draw it.
    pub hidden: bool,
    /// A Creation-Kit editor marker: kept in the scene for picking and
    /// framing, never uploaded.
    pub marker: bool,
    /// Why this block produces no geometry, when it should have.
    pub undrawn: Option<&'static str>,
}

/// One visible line.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow {
    pub depth: u16,
    /// The block this row is, indexing `NifFile::blocks`. For an
    /// `ApparatusFold` it is the particle system the apparatus belongs
    /// to, so the row still addresses something real.
    pub block: usize,
    /// NifSkope's own spelling: `27 (L1_Land:4)` when the block has a
    /// name, `27 [BSFadeNode]` when it does not.
    pub label: String,
    pub kind: RowKind,
    pub expandable: bool,
    pub expanded: bool,
    /// Join into `MeshScene::meshes` — the FIRST, when a block was
    /// walked into more than once.
    pub mesh: Option<usize>,
    /// Join into `MeshScene::nodes`.
    pub node: Option<usize>,
    pub badges: Badges,
}

/// The tree, plus the joins a panel needs to answer a click.
#[derive(Debug, Default)]
pub struct SceneTree {
    pub rows: Vec<TreeRow>,
    pub node_of_block: BTreeMap<usize, usize>,
    /// One block can produce several meshes: a subtree referenced twice
    /// is walked twice, deliberately, because the game draws it twice.
    pub mesh_of_block: BTreeMap<usize, Vec<usize>>,
}

/// A block's label, in NifSkope's spelling.
///
/// ONE function, shared with the viewport breadcrumb: two spellings of
/// the same thing is how surfaces drift apart, which the Records work
/// already paid for once.
pub fn block_label(nif: &NifFile, ctx: Option<Ctx>, block: usize) -> String {
    let Some(b) = nif.blocks.get(block) else {
        return format!("{block} [?]");
    };
    let name = ctx
        .and_then(|c| block_name(b, c))
        .and_then(|s| s.resolve(&nif.header))
        .filter(|s| !s.is_empty());
    match name {
        Some(n) => format!("{block} ({n})"),
        None => format!("{block} [{}]", b.type_name),
    }
}

fn kind_of(type_name: &str) -> RowKind {
    if crate::mesh::is_particle_system(type_name) {
        RowKind::ParticleSystem
    } else if crate::mesh::is_scene_node(type_name) {
        RowKind::SceneNode
    } else if crate::mesh::is_geometry(type_name) {
        RowKind::Shape
    } else {
        RowKind::Other
    }
}

/// Ownership children of every block, in field order.
fn children_of(nif: &NifFile, ctx: Option<Ctx>) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); nif.blocks.len()];
    let Some(ctx) = ctx else { return out };
    for (i, b) in nif.blocks.iter().enumerate() {
        // A block with no typed view terminates the walk THERE and is
        // still shown: the Havok tree is 12 of the campfire's 180
        // blocks, and hiding what we cannot read is the silence class
        // that kept BSLODTriShape invisible for months.
        let Some(refs) = block_refs(b, ctx) else { continue };
        out[i] = refs
            .into_iter()
            .filter(|(_, k)| *k == RefKind::Child)
            .filter_map(|(r, _)| r.get())
            .filter(|t| *t < nif.blocks.len())
            .collect();
    }
    out
}

/// Blocks a particle system owns and nothing outside it does.
///
/// The node graph already shows the apparatus, so the tree folds it
/// away — but only the part that is genuinely private. Measured: the
/// campfire's systems hold 82 exclusive blocks, the scepter 198, the ZD
/// sword 75; exactly one block corpus-wide is shared with ordinary
/// geometry (`blacksmithforge01.nif` block 162, an `NiAlphaProperty`
/// two systems and a shape all point at), and folding that away would
/// hide a block the user can reach no other way.
fn apparatus_blocks(nif: &NifFile, children: &[Vec<usize>], roots: &[usize]) -> BTreeSet<usize> {
    let systems: Vec<usize> = (0..nif.blocks.len())
        .filter(|i| crate::mesh::is_particle_system(&nif.blocks[*i].type_name))
        .collect();
    if systems.is_empty() {
        return BTreeSet::new();
    }
    let reach = |seeds: &[usize], stop_at_systems: bool| -> BTreeSet<usize> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<usize> = seeds.to_vec();
        while let Some(i) = stack.pop() {
            if !seen.insert(i) {
                continue;
            }
            if stop_at_systems && crate::mesh::is_particle_system(&nif.blocks[i].type_name) {
                continue;
            }
            stack.extend(children[i].iter().copied());
        }
        seen
    };
    let mut inside = BTreeSet::new();
    for s in &systems {
        for c in &children[*s] {
            inside.extend(reach(&[*c], false));
        }
    }
    let outside = reach(roots, true);
    inside.difference(&outside).copied().collect()
}

/// Build the visible rows.
///
/// `expanded` holds block indices the user opened; `driven` maps a
/// `MeshScene` node index to its per-channel runtime drivers (the caller
/// computes it, because the sequence player is not this module's
/// business).
pub fn build(
    nif: &NifFile,
    scene: Option<&MeshScene>,
    expanded: &BTreeSet<usize>,
    driven: &BTreeMap<usize, [bool; 3]>,
) -> SceneTree {
    let ctx = Ctx::of(&nif.header).ok();
    let children = children_of(nif, ctx);
    let roots: Vec<usize> = nif
        .roots
        .iter()
        .filter(|r| **r >= 0)
        .map(|r| *r as usize)
        .filter(|r| *r < nif.blocks.len())
        .collect();
    let apparatus = apparatus_blocks(nif, &children, &roots);

    let mut out = SceneTree::default();
    if let Some(scene) = scene {
        for (i, n) in scene.nodes.iter().enumerate() {
            out.node_of_block.entry(n.block).or_insert(i);
        }
        for (i, m) in scene.meshes.iter().enumerate() {
            if let Some(b) = m.block {
                out.mesh_of_block.entry(b).or_default().push(i);
            }
        }
    }

    // Depth-first, in field order, with an ANCESTOR set as the cycle
    // guard: a block that is its own ancestor would walk forever, while
    // a block reachable twice under DIFFERENT parents appears twice —
    // which is right, because the game draws it twice.
    struct Frame {
        block: usize,
        depth: u16,
        next: usize,
    }
    let mut ancestors: BTreeSet<usize> = BTreeSet::new();
    for root in &roots {
        let mut stack = vec![Frame { block: *root, depth: 0, next: 0 }];
        ancestors.insert(*root);
        emit(nif, ctx, scene, &children, &apparatus, expanded, driven, *root, 0, &mut out);
        while let Some(top) = stack.last_mut() {
            let parent = top.block;
            let depth = top.depth;
            // A folded apparatus stops the walk; an unexpanded row does
            // too, and that is what keeps a collapsed tree O(roots).
            let open = expanded.contains(&parent);
            let kid = if open { children[parent].get(top.next).copied() } else { None };
            top.next += 1;
            let Some(kid) = kid else {
                ancestors.remove(&parent);
                stack.pop();
                continue;
            };
            if ancestors.contains(&kid) {
                continue;
            }
            if apparatus.contains(&kid) {
                continue;
            }
            emit(
                nif, ctx, scene, &children, &apparatus, expanded, driven, kid, depth + 1,
                &mut out,
            );
            ancestors.insert(kid);
            stack.push(Frame { block: kid, depth: depth + 1, next: 0 });
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn emit(
    nif: &NifFile,
    ctx: Option<Ctx>,
    scene: Option<&MeshScene>,
    children: &[Vec<usize>],
    apparatus: &BTreeSet<usize>,
    expanded: &BTreeSet<usize>,
    driven: &BTreeMap<usize, [bool; 3]>,
    block: usize,
    depth: u16,
    out: &mut SceneTree,
) {
    let type_name = nif.blocks[block].type_name.as_str();
    let kind = kind_of(type_name);
    let node = out.node_of_block.get(&block).copied();
    let mesh = out.mesh_of_block.get(&block).and_then(|v| v.first().copied());

    let mut badges = Badges::default();
    if let (Some(scene), Some(m)) = (scene, mesh) {
        badges.hidden = scene.meshes[m].hidden;
        badges.marker = scene.meshes[m].marker;
    }
    if let Some(n) = node {
        badges.driven = driven.get(&n).copied().unwrap_or_default();
    }
    if kind == RowKind::Shape && mesh.is_none() {
        // A geometry block the walk produced nothing for. Saying so is
        // the whole point of the row: that silence is how a whole shape
        // family stayed invisible for months.
        badges.undrawn = Some(if !has_typed_view(type_name) {
            "no typed view for this block"
        } else {
            "extracted no geometry"
        });
    }

    // Everything the block owns EXCEPT what the apparatus fold covers.
    let visible_kids = children[block]
        .iter()
        .filter(|c| !apparatus.contains(c))
        .count();
    let folded = children[block]
        .iter()
        .filter(|c| apparatus.contains(c))
        .count();

    out.rows.push(TreeRow {
        depth,
        block,
        label: block_label(nif, ctx, block),
        kind,
        expandable: visible_kids > 0,
        expanded: expanded.contains(&block),
        mesh,
        node,
        badges,
    });

    // The apparatus, as ONE row. It sits under the system rather than
    // inline because the node graph already shows it, and a duplicated
    // surface drifts.
    if kind == RowKind::ParticleSystem && folded > 0 {
        let owned: BTreeSet<usize> = {
            let mut seen = BTreeSet::new();
            let mut stack: Vec<usize> = children[block]
                .iter()
                .copied()
                .filter(|c| apparatus.contains(c))
                .collect();
            while let Some(i) = stack.pop() {
                if !seen.insert(i) {
                    continue;
                }
                stack.extend(children[i].iter().copied().filter(|c| apparatus.contains(c)));
            }
            seen
        };
        out.rows.push(TreeRow {
            depth: depth + 1,
            block,
            label: format!("Particle apparatus ({})", owned.len()),
            kind: RowKind::ApparatusFold { blocks: owned.len() },
            expandable: false,
            expanded: false,
            mesh: None,
            node: None,
            badges: Badges::default(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nif(types: &[&str]) -> NifFile {
        NifFile {
            header: aether_nif::header::Header::new_sse(),
            blocks: types
                .iter()
                .map(|t| aether_nif::file::Block {
                    type_name: (*t).into(),
                    data: Vec::new(),
                })
                .collect(),
            roots: vec![0],
        }
    }

    /// A file whose blocks cannot be decoded still yields a tree.
    ///
    /// Six vanilla LE files are pre-Skyrim Gamebryo debris that `Ctx::of`
    /// refuses. A named row beats an empty panel: the user can still see
    /// what the file contains.
    #[test]
    fn an_undecodable_file_still_lists_its_root() {
        let mut f = nif(&["NiNode", "NiTriShape"]);
        f.header.bs_version = aether_nif::header::BsStreamVersion::Other(11);
        let tree = build(&f, None, &BTreeSet::new(), &BTreeMap::new());
        assert_eq!(tree.rows.len(), 1, "no ctx means no children, not no tree");
        assert_eq!(tree.rows[0].label, "0 [NiNode]");
        assert!(!tree.rows[0].expandable);
    }
}
