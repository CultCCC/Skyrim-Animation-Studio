//! Automatic node arrangement for particle graphs.
//!
//! The decompiler used to drop nodes into fixed 420-unit lanes, which the
//! campfire's eleven-modifier stack overflowed into the next system's band
//! — imported graphs opened as a pile. This pass sizes every band from
//! the nodes actually in it.
//!
//! Layout model: left-to-right dataflow columns per system —
//! sources (data/shader/alpha) → modifiers → controllers → system — with
//! the scene output on the far right. Each system gets a horizontal band
//! tall enough for its tallest column; columns are vertically centred in
//! the band so short stacks sit next to the middle of long ones.

use crate::graph::{Graph, NodeId, NodeKind};

/// Column x-origins, sized so the widest node of the previous column plus
/// a wire gap fits — bodies now show every parameter (with ± pairs), so
/// nodes run ~380 wide.
const COLS: [f32; 5] = [0.0, 430.0, 860.0, 1290.0, 1690.0];
/// Vertical gap between stacked nodes and between system bands.
const NODE_GAP: f32 = 18.0;
const BAND_GAP: f32 = 70.0;

/// Estimated on-canvas height of a node: header plus one row per visible
/// body param or socket, whichever side is taller. The estimate only has
/// to be an upper bound of the real snarl height for stacks not to
/// overlap.
fn node_height(graph: &Graph, id: NodeId) -> f32 {
    let node = &graph.nodes[&id];
    let schema = node.kind.schema();
    // Bodies show every param; "X ± X Variation" pairs share a row.
    let variations = node
        .params
        .iter()
        .filter(|(k, _)| k.ends_with(" Variation"))
        .count();
    let rows = (node.params.len() - variations)
        .max(schema.inputs.len())
        .max(schema.outputs.len())
        // Preview strips (colour ramp, scale curve) take a row.
        + 1;
    48.0 + rows as f32 * 27.0
}

/// Arrange every node that belongs to a system cluster (and the scene
/// output). Free-floating nodes the user dropped are left where they are.
pub fn arrange(graph: &mut Graph) {
    let outputs: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|(_, n)| n.kind == NodeKind::SceneOutput)
        .map(|(id, _)| *id)
        .collect();
    let Some(output_id) = outputs.first().copied() else {
        return;
    };

    let wired = |node: NodeId, socket: usize| -> Vec<NodeId> {
        graph.incoming(node, socket).map(|l| l.from_node).collect()
    };

    // Plan first, apply second — computing the plan reads the graph, and
    // the borrow must end before positions are written.
    let mut plan: Vec<(NodeId, [f32; 2])> = Vec::new();
    let systems = wired(output_id, 0);
    let mut band_top = 0.0f32;
    for system in systems {
        // Column membership per system, in wire (stack) order.
        let sources: Vec<NodeId> = (0..3).flat_map(|s| wired(system, s)).collect();
        let modifiers = wired(system, 3);
        let controllers = wired(system, 4);
        let columns: [&[NodeId]; 4] = [&sources, &modifiers, &controllers, &[system]];

        let col_height = |ids: &[NodeId]| -> f32 {
            let sum: f32 = ids.iter().map(|id| node_height(graph, *id)).sum();
            sum + NODE_GAP * ids.len().saturating_sub(1) as f32
        };
        let band_height = columns
            .iter()
            .map(|c| col_height(c))
            .fold(0.0f32, f32::max)
            .max(1.0);

        for (col, ids) in columns.iter().enumerate() {
            let mut y = band_top + (band_height - col_height(ids)) / 2.0;
            for id in ids.iter() {
                plan.push((*id, [COLS[col], y]));
                y += node_height(graph, *id) + NODE_GAP;
            }
        }
        band_top += band_height + BAND_GAP;
    }
    // The output sits to the right, centred over everything it collects.
    plan.push((output_id, [COLS[4], (band_top - BAND_GAP) / 2.0 - 40.0]));

    for (id, pos) in plan {
        if let Some(node) = graph.nodes.get_mut(&id) {
            node.pos = pos;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphNode, Link};

    fn wire(g: &mut Graph, from: NodeId, to: NodeId, socket: usize) {
        g.connect(Link {
            from_node: from,
            from_socket: 0,
            to_node: to,
            to_socket: socket,
        })
        .unwrap();
    }

    /// Two systems with very different stack sizes: no node rectangle may
    /// overlap another — the exact failure the fixed lanes produced.
    #[test]
    fn arranged_nodes_do_not_overlap() {
        let mut g = Graph::default();
        let out = g.insert(GraphNode::new(NodeKind::SceneOutput, [0.0, 0.0]));
        for n_mods in [11usize, 2] {
            let sys = g.insert(GraphNode::new(NodeKind::ParticleSystem, [0.0, 0.0]));
            let data = g.insert(GraphNode::new(NodeKind::ParticleData, [0.0, 0.0]));
            wire(&mut g, data, sys, 0);
            for _ in 0..n_mods {
                let m = g.insert(GraphNode::new(NodeKind::GravityModifier, [0.0, 0.0]));
                wire(&mut g, m, sys, 3);
            }
            let c = g.insert(GraphNode::new(NodeKind::UpdateCtlr, [0.0, 0.0]));
            wire(&mut g, c, sys, 4);
            wire(&mut g, sys, out, 0);
        }
        arrange(&mut g);

        let rects: Vec<(NodeId, [f32; 2], f32)> = g
            .nodes
            .iter()
            .map(|(id, n)| (*id, n.pos, node_height(&g, *id)))
            .collect();
        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                let same_col = (a.1[0] - b.1[0]).abs() < 1.0;
                if !same_col {
                    continue; // different columns never overlap by x
                }
                let a_range = a.1[1]..a.1[1] + a.2;
                let overlap = a_range.contains(&b.1[1]) || (b.1[1]..b.1[1] + b.2).contains(&a.1[1]);
                assert!(
                    !overlap,
                    "{:?} at {:?} overlaps {:?} at {:?}",
                    g.nodes[&a.0].kind, a.1, g.nodes[&b.0].kind, b.1
                );
            }
        }
    }
}

