//! Copy/paste fragments: a subgraph lifted out of a document, ready
//! to drop into any graph — the same one, another tab, or a library
//! item on disk.
//!
//! A fragment IS a [`Graph`] with local ids `0..n`: that buys the
//! existing JSON round trip, the schema-default overlay on load, and
//! every invariant the type already keeps, instead of a second
//! almost-graph format.
//!
//! Two deliberate rules:
//! - **Provenance is STRIPPED.** A pasted node is new authorship; a
//!   copy that kept `source_block` would fight the original for its
//!   slot in the export splice and one of the two would win silently.
//! - **Link order is PRESERVED** (fragment links keep their relative
//!   order from the source `links` Vec, and paste connects them in
//!   fragment order), because slot order within a multi socket is an
//!   exported property — the stable class sort turns wire order into
//!   the modifier stack.

use crate::command::Command;
use aether_nif::NifFile;
use crate::graph::AuthoredBlock;
use crate::graph::{Graph, GraphNode, Link, NodeId};

/// Lift `ids` (plus every link BOTH of whose ends are in `ids`) out of
/// `graph` into a standalone fragment with local ids `0..n`. Links
/// with one end outside the set are dropped — copying degrades
/// gracefully instead of refusing.
pub fn extract(graph: &Graph, ids: &[NodeId], source: Option<&NifFile>) -> Graph {
    let mut picked: Vec<NodeId> = ids
        .iter()
        .copied()
        .filter(|id| graph.nodes.contains_key(id))
        .collect();
    picked.sort();
    picked.dedup();

    // Re-base positions on the selection's own min corner, so a paste
    // lands where asked instead of wherever the source happened to be.
    let min = picked
        .iter()
        .map(|id| graph.nodes[id].pos)
        .fold([f32::INFINITY, f32::INFINITY], |a, p| {
            [a[0].min(p[0]), a[1].min(p[1])]
        });

    let mut out = Graph::new();
    let local: std::collections::HashMap<NodeId, NodeId> = picked
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, NodeId(i as u64)))
        .collect();
    for id in &picked {
        let src = &graph.nodes[id];
        let mut node = GraphNode::new(src.kind, [src.pos[0] - min[0], src.pos[1] - min[1]]);
        node.params = src.params.clone();
        // The SLOT is dropped (a copy must never claim its original's
        // place in the splice) and the VALUES are kept: the bytes of
        // the block this node was authored from travel with it, so the
        // fields the graph does not model survive the trip to another
        // file. Those were two jobs in one field, and separating them
        // is what stops a paste from silently rewriting them to
        // `Default` -- measured at 8 byte fields across 29 fixtures,
        // one of which crashed the game.
        node.source_block = None;
        node.authored = src
            .authored
            .clone()
            .or_else(|| AuthoredBlock::capture(source?, src.source_block?));
        out.insert_with_id(local[id], node);
    }
    // Straight into the Vec, in source order: `connect` would
    // re-validate a state that was already legal, and the order of
    // `links` is the slot order.
    for l in &graph.links {
        if let (Some(from), Some(to)) = (local.get(&l.from_node), local.get(&l.to_node)) {
            out.links.push(Link {
                from_node: *from,
                from_socket: l.from_socket,
                to_node: *to,
                to_socket: l.to_socket,
            });
        }
    }
    out
}

/// Grow a selection UPSTREAM: the given nodes plus everything that
/// feeds them, transitively.
///
/// This is what "save this particle system" means. A system node on
/// its own is not a thing anyone can reuse -- `extract` keeps only the
/// links with BOTH ends inside the set, so one node yields one node
/// and no wires, and pasting it produces a system with no data, no
/// shader, no alpha, no emitter and no controllers. Every one of those
/// is an export error, which is exactly what the user saw.
///
/// UPSTREAM only, so the Scene Output the system feeds is not dragged
/// along; a library item is an apparatus, not a document.
///
/// A node shared by two systems IS included. The fragment has to stand
/// on its own, and on paste it becomes a separate node -- which is the
/// honest outcome: 24 SSE files point two systems at one
/// `NiAlphaProperty`, and a fragment that omitted it would paste a
/// system missing its alpha.
pub fn upstream_closure(graph: &Graph, roots: &[NodeId]) -> Vec<NodeId> {
    let mut seen: std::collections::BTreeSet<NodeId> = roots
        .iter()
        .copied()
        .filter(|id| graph.nodes.contains_key(id))
        .collect();
    // A visited set rather than recursion: the connectable relation is
    // a DAG today (the acyclicity property is pinned in core's tests),
    // but a walk that would hang on a cycle is not the thing to build
    // on top of that.
    let mut queue: Vec<NodeId> = seen.iter().copied().collect();
    while let Some(node) = queue.pop() {
        for l in &graph.links {
            if l.to_node == node && seen.insert(l.from_node) {
                queue.push(l.from_node);
            }
        }
    }
    seen.into_iter().collect()
}

/// Build the ONE undo step that pastes `fragment` into `target` at
/// `at` (the fragment's min corner lands there). Ids are reserved on
/// `target` here; the returned command has not been applied yet —
/// route it through `History` like every other mutation. Also returns
/// the ids the paste will create, so a canvas can select them.
pub fn paste_batch(target: &mut Graph, fragment: &Graph, at: [f32; 2]) -> (Command, Vec<NodeId>) {
    let mut locals: Vec<NodeId> = fragment.nodes.keys().copied().collect();
    locals.sort();
    let fresh: std::collections::HashMap<NodeId, NodeId> = locals
        .iter()
        .map(|local| (*local, target.reserve_id()))
        .collect();

    let mut commands: Vec<Command> = Vec::with_capacity(locals.len() + fragment.links.len());
    for local in &locals {
        let src = &fragment.nodes[local];
        let mut node = GraphNode::new(src.kind, [src.pos[0] + at[0], src.pos[1] + at[1]]);
        node.params = src.params.clone();
        node.source_block = None;
        node.authored = src.authored.clone();
        commands.push(Command::AddNode {
            id: fresh[local],
            node,
        });
    }
    for l in &fragment.links {
        commands.push(Command::Connect {
            link: Link {
                from_node: fresh[&l.from_node],
                from_socket: l.from_socket,
                to_node: fresh[&l.to_node],
                to_socket: l.to_socket,
            },
        });
    }
    let created: Vec<NodeId> = locals.iter().map(|l| fresh[l]).collect();
    (
        Command::Batch {
            label: "Paste",
            commands,
        },
        created,
    )
}
