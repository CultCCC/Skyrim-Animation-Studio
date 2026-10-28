//! Undoable command system.
//!
//! Every mutation of a [`Graph`] goes through [`History::apply`], which makes
//! undo/redo uniform for the UI, and later lets headless automation (MCP)
//! reuse the exact same operations.

use crate::graph::{Graph, GraphNode, Link, NodeId};
use crate::value::Value;

/// A single reversible edit.
#[derive(Debug, Clone)]
pub enum Command {
    AddNode {
        id: NodeId,
        node: GraphNode,
    },
    /// Several edits as ONE undo step, all-or-nothing: a failure
    /// mid-way reverts the applied prefix, because a half-applied
    /// batch would make its own undo record a lie (the same rule
    /// NifEdit::SetTransform follows for a flat rig's bone list).
    Batch {
        label: &'static str,
        commands: Vec<Command>,
    },
    RemoveNode {
        id: NodeId,
        /// Filled in during apply so undo can restore everything —
        /// each severed link WITH its index in `links`, because slot
        /// order is exported and a re-appended wire is a wrong order.
        removed: Option<(GraphNode, Vec<(usize, Link)>)>,
    },
    SetParam {
        id: NodeId,
        key: &'static str,
        value: Value,
        previous: Option<Value>,
    },
    MoveNode {
        id: NodeId,
        to: [f32; 2],
        from: [f32; 2],
    },
    /// In-place node kind change (Box Emitter -> Sphere Emitter and
    /// friends); wires, position and provenance stay put.
    ChangeKind {
        id: NodeId,
        kind: crate::graph::NodeKind,
        /// Filled in during apply so undo restores the exact params,
        /// not a re-derivation of them.
        previous: Option<(crate::graph::NodeKind, Vec<(&'static str, Value)>)>,
    },
    /// Move a wire within a multi input socket (the visible slot
    /// order, which for same-class modifiers is the order the game
    /// applies them in).
    ReorderLink {
        node: NodeId,
        socket: usize,
        from_slot: usize,
        to_slot: usize,
        /// The RAW `links` indices the move touched, filled during
        /// apply. Undo replays the exact Vec move backwards: the
        /// slot-space inverse restores this socket's order while
        /// displacing links of other sockets between the two
        /// indices, which corrupts every index-based restore
        /// recorded before it.
        moved: Option<(usize, usize)>,
    },
    Connect {
        link: Link,
    },
    Disconnect {
        link: Link,
        /// Where the link sat, filled during apply: slot order is an
        /// exported property, so undo must restore the exact index.
        at: Option<usize>,
    },
    /// Replace one clip whole: its keys, tracks, cycle or text keys. A
    /// clip is small and an edit to it is one gesture, so the snapshot is
    /// the honest record -- the same shape `ChangeKind` uses for params.
    SetClip {
        index: usize,
        clip: Box<crate::clip::Clip>,
        previous: Option<Box<crate::clip::Clip>>,
    },
    /// Append a new clip to the document.
    AddClip {
        clip: Box<crate::clip::Clip>,
    },
    /// Remove clip `index`; the clip is kept so undo puts it back in
    /// its own place (clip order is file order).
    RemoveClip {
        index: usize,
        removed: Option<Box<crate::clip::Clip>>,
    },
}

impl Command {
    /// Set a parameter, on EVERY node that stands on the same source
    /// block.
    ///
    /// The format shares blocks: blacksmithforge01 points two particle
    /// systems and seven meshes at one `NiAlphaProperty`, and the
    /// decompiler gives each system its own node for it. They are two
    /// views of ONE block, so a value typed into either is a value of
    /// that block — and the splice enforces it from the other end: two
    /// compiled blocks may share a slot only when their bytes are
    /// IDENTICAL, so editing one alone left the other's value in the
    /// slot and the edit silently did not stick.
    ///
    /// One `Batch`, so Ctrl+Z takes it back in one press rather than
    /// leaving the pair disagreeing halfway.
    pub fn set_param(graph: &Graph, id: NodeId, key: &'static str, value: Value) -> Command {
        let block = graph.nodes.get(&id).and_then(|n| n.source_block);
        let mut also: Vec<NodeId> = match block {
            Some(b) => graph
                .nodes
                .iter()
                .filter(|(other, n)| **other != id && n.source_block == Some(b))
                // ...and only where the parameter exists: a system and
                // its alpha property share a block only with another
                // node of the same kind, but the rule is asked of the
                // PARAMETER so it cannot depend on that staying true.
                .filter(|(_, n)| n.param(key).is_some())
                .map(|(other, _)| *other)
                .collect(),
            None => Vec::new(),
        };
        if also.is_empty() {
            return Command::SetParam {
                id,
                key,
                value,
                previous: None,
            };
        }
        // Deterministic order: an undo replays the batch backwards and
        // two runs must produce the same records.
        also.sort();
        let mut commands = vec![Command::SetParam {
            id,
            key,
            value: value.clone(),
            previous: None,
        }];
        commands.extend(also.into_iter().map(|other| Command::SetParam {
            id: other,
            key,
            value: value.clone(),
            previous: None,
        }));
        Command::Batch {
            label: "Edit Parameter",
            commands,
        }
    }

    /// Replace clip `index` with `clip`.
    pub fn set_clip(_graph: &Graph, index: usize, clip: crate::clip::Clip) -> Command {
        Command::SetClip {
            index,
            clip: Box::new(clip),
            previous: None,
        }
    }

    /// Append `clip` to the document's clips.
    pub fn add_clip(_graph: &Graph, clip: crate::clip::Clip) -> Command {
        Command::AddClip { clip: Box::new(clip) }
    }

    /// Remove clip `index` from the document.
    pub fn remove_clip(_graph: &Graph, index: usize) -> Command {
        Command::RemoveClip { index, removed: None }
    }

    /// Human label for menus ("Undo Add Node").
    pub fn label(&self) -> &'static str {
        match self {
            Command::SetClip { .. } => "Edit Clip",
            Command::AddClip { .. } => "Add Clip",
            Command::RemoveClip { .. } => "Remove Clip",
            Command::AddNode { .. } => "Add Node",
            Command::Batch { label, .. } => label,
            Command::RemoveNode { .. } => "Remove Node",
            Command::SetParam { .. } => "Edit Parameter",
            Command::MoveNode { .. } => "Move Node",
            Command::ChangeKind { .. } => "Change Node Type",
            Command::ReorderLink { .. } => "Reorder Wires",
            Command::Connect { .. } => "Connect",
            Command::Disconnect { .. } => "Disconnect",
        }
    }

    fn apply(&mut self, graph: &mut Graph) -> bool {
        match self {
            Command::AddNode { id, node } => {
                graph.insert_with_id(*id, node.clone());
                true
            }
            Command::Batch { commands, .. } => {
                for i in 0..commands.len() {
                    if !commands[i].apply(graph) {
                        for done in commands[..i].iter().rev() {
                            done.revert(graph);
                        }
                        return false;
                    }
                }
                true
            }
            Command::RemoveNode { id, removed } => {
                let touching: Vec<(usize, Link)> = graph
                    .links
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.from_node == *id || l.to_node == *id)
                    .map(|(i, l)| (i, *l))
                    .collect();
                match graph.remove(*id) {
                    Some((node, _)) => {
                        *removed = Some((node, touching));
                        true
                    }
                    None => false,
                }
            }
            Command::SetParam {
                id,
                key,
                value,
                previous,
            } => {
                if let Some(node) = graph.nodes.get_mut(id) {
                    *previous = node.set_param(key, value.clone());
                    previous.is_some()
                } else {
                    false
                }
            }
            Command::MoveNode { id, to, .. } => {
                if let Some(node) = graph.nodes.get_mut(id) {
                    node.pos = *to;
                    true
                } else {
                    false
                }
            }
            Command::ChangeKind { id, kind, previous } => {
                match graph.change_kind(*id, *kind) {
                    Ok(old) => {
                        *previous = Some(old);
                        true
                    }
                    Err(_) => false,
                }
            }
            Command::ReorderLink {
                node,
                socket,
                from_slot,
                to_slot,
                moved,
            } => match graph.reorder_link(*node, *socket, *from_slot, *to_slot) {
                Ok(indices) => {
                    *moved = Some(indices);
                    true
                }
                Err(_) => false,
            },
            Command::Connect { link } => graph.connect(*link).is_ok(),
            Command::Disconnect { link, at } => {
                *at = graph.disconnect_indexed(*link);
                at.is_some()
            }
            Command::SetClip {
                index,
                clip,
                previous,
            } => match graph.clips.get_mut(*index) {
                Some(slot) => {
                    *previous = Some(Box::new(std::mem::replace(slot, (**clip).clone())));
                    true
                }
                None => false,
            },
            Command::AddClip { clip } => {
                graph.clips.push((**clip).clone());
                true
            }
            Command::RemoveClip { index, removed } => {
                if *index >= graph.clips.len() {
                    return false;
                }
                *removed = Some(Box::new(graph.clips.remove(*index)));
                true
            }
        }
    }

    fn revert(&self, graph: &mut Graph) {
        match self {
            Command::AddNode { id, .. } => {
                graph.remove(*id);
            }
            Command::Batch { commands, .. } => {
                for cmd in commands.iter().rev() {
                    cmd.revert(graph);
                }
            }
            Command::RemoveNode { id, removed } => {
                if let Some((node, links)) = removed {
                    graph.insert_with_id(*id, node.clone());
                    // Ascending index keeps every later index valid.
                    for (at, l) in links {
                        graph.reconnect_at(*at, *l);
                    }
                }
            }
            Command::SetParam {
                id, key, previous, ..
            } => {
                if let (Some(node), Some(prev)) = (graph.nodes.get_mut(id), previous) {
                    node.set_param(key, prev.clone());
                }
            }
            Command::ReorderLink { moved, .. } => {
                // The exact Vec inverse, not the slot-space one.
                if let Some((from, to)) = moved {
                    graph.move_link(*to, *from);
                }
            }
            Command::ChangeKind { id, previous, .. } => {
                if let (Some(node), Some((kind, params))) =
                    (graph.nodes.get_mut(id), previous.as_ref())
                {
                    node.kind = *kind;
                    node.params = params.clone();
                }
            }
            Command::MoveNode { id, from, .. } => {
                if let Some(node) = graph.nodes.get_mut(id) {
                    node.pos = *from;
                }
            }
            Command::Connect { link } => {
                graph.disconnect(*link);
            }
            Command::Disconnect { link, at } => {
                graph.reconnect_at(at.unwrap_or(usize::MAX), *link);
            }
            Command::SetClip {
                index, previous, ..
            } => {
                if let (Some(slot), Some(prev)) = (graph.clips.get_mut(*index), previous) {
                    *slot = (**prev).clone();
                }
            }
            Command::AddClip { .. } => {
                graph.clips.pop();
            }
            Command::RemoveClip { index, removed } => {
                if let Some(clip) = removed {
                    graph.clips.insert((*index).min(graph.clips.len()), (**clip).clone());
                }
            }
        }
    }
}

/// A reversible edit to the FILE the tab was imported from.
///
/// Separate from `Command` because it does not touch the `Graph` — but
/// in the SAME stack, because one user action is both: moving a node
/// rewrites the block AND every graph parameter derived from that node's
/// transform. Two stacks would let Ctrl+Z undo half of it.
/// One block's new local transform, with room for the old one.
#[derive(Debug, Clone)]
pub struct BlockTransform {
    pub block: usize,
    /// Filled during apply so undo is exact rather than recomputed.
    pub from: Option<([f32; 3], [f32; 9], f32)>,
    pub to: ([f32; 3], [f32; 9], f32),
}

#[derive(Debug, Clone)]
pub enum NifEdit {
    SetTransform {
        /// One entry per block, applied and reverted TOGETHER.
        ///
        /// Usually one. It is a list because moving a skinned shape
        /// means moving the bones that place it, and 53% of the
        /// corpus's skinned shapes (29,652 of 56,174, every character
        /// among them) carry a FLAT bone list — 32 siblings on
        /// chicken.nif, 77 on alduin — where no single node carries the
        /// skin and a rigid move is the same world delta on every root
        /// bone. One step, or Ctrl+Z would take a dragon apart one bone
        /// at a time.
        blocks: Vec<BlockTransform>,
        /// The derived parameters this edit re-derived, with their old
        /// values, so undo restores them in the SAME step. Same shape as
        /// `RemoveNode`'s `removed`.
        params: Vec<(NodeId, &'static str, Value, Option<Value>)>,
    },
}

impl NifEdit {
    pub fn label(&self) -> &'static str {
        match self {
            NifEdit::SetTransform { .. } => "Move Block",
        }
    }

    /// The blocks this edit writes.
    pub fn blocks(&self) -> Vec<usize> {
        match self {
            NifEdit::SetTransform { blocks, .. } => blocks.iter().map(|b| b.block).collect(),
        }
    }

    fn apply(&mut self, graph: &mut Graph, nif: &mut aether_nif::NifFile) -> bool {
        match self {
            NifEdit::SetTransform { blocks, params } => {
                let Ok(ctx) = aether_nif::typed::Ctx::of(&nif.header) else {
                    return false;
                };
                if blocks.is_empty() {
                    return false;
                }
                // Rewrite every block FIRST, then commit them together:
                // a run that fails half way through would leave a
                // skeleton with some bones moved and some not, and the
                // undo step recorded for it would be a lie.
                let mut written = Vec::with_capacity(blocks.len());
                for entry in blocks.iter() {
                    let Some(b) = nif.blocks.get(entry.block) else {
                        return false;
                    };
                    let Some(mut av) = aether_nif::typed::block_av(b, ctx) else {
                        return false;
                    };
                    let was = (av.translation, av.rotation, av.scale);
                    av.translation = entry.to.0;
                    av.rotation = entry.to.1;
                    av.scale = entry.to.2;
                    let Some(w) = aether_nif::typed::set_block_av(b, ctx, &av) else {
                        return false;
                    };
                    written.push((entry.block, was, w));
                }
                for ((block, was, w), entry) in written.into_iter().zip(blocks.iter_mut()) {
                    if entry.from.is_none() {
                        entry.from = Some(was);
                    }
                    nif.blocks[block] = w;
                }
                for (id, key, value, previous) in params.iter_mut() {
                    if let Some(node) = graph.nodes.get_mut(id) {
                        if previous.is_none() {
                            *previous = node.param(key).cloned();
                        }
                        node.set_param(key, value.clone());
                    }
                }
                true
            }
        }
    }

    fn revert(&self, graph: &mut Graph, nif: &mut aether_nif::NifFile) {
        match self {
            NifEdit::SetTransform { blocks, params } => {
                if let Ok(ctx) = aether_nif::typed::Ctx::of(&nif.header) {
                    for entry in blocks {
                        let Some(from) = entry.from else { continue };
                        if let Some(b) = nif.blocks.get(entry.block) {
                            if let Some(mut av) = aether_nif::typed::block_av(b, ctx) {
                                av.translation = from.0;
                                av.rotation = from.1;
                                av.scale = from.2;
                                if let Some(w) = aether_nif::typed::set_block_av(b, ctx, &av) {
                                    nif.blocks[entry.block] = w;
                                }
                            }
                        }
                    }
                }
                for (id, key, _, previous) in params {
                    if let (Some(node), Some(previous)) = (graph.nodes.get_mut(id), previous) {
                        node.set_param(key, previous.clone());
                    }
                }
            }
        }
    }
}

/// One step of the undo stack: a graph edit or a file edit.
#[derive(Debug, Clone)]
pub enum Edit {
    Graph(Command),
    Nif(NifEdit),
}

impl Edit {
    pub fn label(&self) -> &'static str {
        match self {
            Edit::Graph(c) => c.label(),
            Edit::Nif(e) => e.label(),
        }
    }
}

/// Undo/redo stack. The tech doc requires at least 64 steps; we keep 256.
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// The blocks a NIF undo or redo just restored, waiting to be taken.
    ///
    /// Undo puts the FILE back and the scene is not the file: nothing
    /// moves on screen until someone settles the `MeshScene` onto those
    /// blocks. Recorded here rather than returned, so every existing
    /// caller of the `bool` keeps working, and TAKEN rather than polled
    /// so one undo settles exactly one thing.
    moved_blocks: Vec<usize>,
    capacity: usize,
    /// Bumped on every successful mutation; used for dirty tracking.
    pub revision: u64,
}

impl Default for History {
    fn default() -> Self {
        Self::new(256)
    }
}

impl History {
    pub fn new(capacity: usize) -> Self {
        History {
            undo: Vec::new(),
            redo: Vec::new(),
            moved_blocks: Vec::new(),
            capacity,
            revision: 0,
        }
    }

    /// Apply a command; on success push to the undo stack and clear redo.
    pub fn apply(&mut self, graph: &mut Graph, mut cmd: Command) -> bool {
        if !cmd.apply(graph) {
            return false;
        }
        self.revision += 1;
        self.redo.clear();
        self.undo.push(Edit::Graph(cmd));
        if self.undo.len() > self.capacity {
            let excess = self.undo.len() - self.capacity;
            self.undo.drain(..excess);
        }
        true
    }

    /// Merge consecutive `SetParam`/`MoveNode` commands on the same target so
    /// dragging a slider produces one undo step, not hundreds.
    pub fn apply_merged(&mut self, graph: &mut Graph, cmd: Command) -> bool {
        // A key dragged on a clip lane: one SetClip per frame, merged
        // like a slider -- the FIRST snapshot stays as `previous`, so one
        // undo takes the whole drag back.
        if let (
            Some(Edit::Graph(Command::SetClip {
                index: last_index,
                clip: last_clip,
                ..
            })),
            Command::SetClip { index, clip, .. },
        ) = (self.undo.last_mut(), &cmd)
        {
            if last_index == index {
                return match graph.clips.get_mut(*index) {
                    Some(slot) => {
                        *slot = (**clip).clone();
                        *last_clip = clip.clone();
                        self.revision += 1;
                        true
                    }
                    None => false,
                };
            }
        }
        if let (Some(Edit::Graph(last)), Command::SetParam { id, key, .. }) =
            (self.undo.last_mut(), &cmd)
        {
            if let Command::SetParam {
                id: lid,
                key: lkey,
                value: lvalue,
                ..
            } = last
            {
                if lid == id && lkey == key {
                    // Mutate in place: keep original `previous`, update value.
                    if let Command::SetParam { value, .. } = &cmd {
                        *lvalue = value.clone();
                        if let Some(node) = graph.nodes.get_mut(id) {
                            node.set_param(key, value.clone());
                            self.revision += 1;
                            return true;
                        }
                    }
                    return false;
                }
            }
        }
        if let (Some(Edit::Graph(last)), Command::MoveNode { id, to, .. }) =
            (self.undo.last_mut(), &cmd)
        {
            if let Command::MoveNode {
                id: lid, to: lto, ..
            } = last
            {
                if lid == id {
                    *lto = *to;
                    if let Some(node) = graph.nodes.get_mut(id) {
                        node.pos = *to;
                        self.revision += 1;
                        return true;
                    }
                    return false;
                }
            }
        }
        self.apply(graph, cmd)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The block the last undo or redo restored, taken once.
    pub fn take_moved_blocks(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.moved_blocks)
    }

    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.last().map(|c| c.label())
    }

    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo.last().map(|c| c.label())
    }

    /// Apply an edit to the FILE and the graph together, as one step.
    pub fn apply_nif(
        &mut self,
        graph: &mut Graph,
        nif: &mut aether_nif::NifFile,
        mut edit: NifEdit,
    ) -> bool {
        if !edit.apply(graph, nif) {
            return false;
        }
        self.revision += 1;
        self.redo.clear();
        self.undo.push(Edit::Nif(edit));
        if self.undo.len() > self.capacity {
            let excess = self.undo.len() - self.capacity;
            self.undo.drain(..excess);
        }
        true
    }

    /// Undo one step.
    ///
    /// A file edit needs the file. When the top of the stack is one and
    /// `nif` is absent, this returns false WITHOUT popping: silently
    /// skipping it would undo the step below out of order, which is
    /// worse than doing nothing.
    pub fn undo(&mut self, graph: &mut Graph, nif: Option<&mut aether_nif::NifFile>) -> bool {
        match self.undo.last() {
            Some(Edit::Nif(_)) if nif.is_none() => return false,
            None => return false,
            _ => {}
        }
        let Some(edit) = self.undo.pop() else {
            return false;
        };
        match &edit {
            Edit::Graph(c) => c.revert(graph),
            Edit::Nif(e) => {
                let nif = nif.expect("checked above");
                e.revert(graph, nif);
                self.moved_blocks = e.blocks();
            }
        }
        self.revision += 1;
        self.redo.push(edit);
        true
    }

    pub fn redo(&mut self, graph: &mut Graph, nif: Option<&mut aether_nif::NifFile>) -> bool {
        match self.redo.last() {
            Some(Edit::Nif(_)) if nif.is_none() => return false,
            None => return false,
            _ => {}
        }
        let Some(mut edit) = self.redo.pop() else {
            return false;
        };
        let ok = match &mut edit {
            Edit::Graph(c) => c.apply(graph),
            Edit::Nif(e) => {
                let ok = e.apply(graph, nif.expect("checked above"));
                if ok {
                    self.moved_blocks = e.blocks();
                }
                ok
            }
        };
        if ok {
            self.revision += 1;
            self.undo.push(edit);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::NodeKind;

    #[test]
    fn undo_redo_add_remove() {
        let mut g = Graph::new();
        let mut h = History::default();

        let id = NodeId(0);
        let node = GraphNode::new(NodeKind::ParticleSystem, [0.0, 0.0]);
        assert!(h.apply(&mut g, Command::AddNode { id, node }));
        assert_eq!(g.nodes.len(), 1);

        assert!(h.apply(&mut g, Command::RemoveNode { id, removed: None }));
        assert_eq!(g.nodes.len(), 0);

        h.undo(&mut g, None);
        assert_eq!(g.nodes.len(), 1, "remove undone");
        h.undo(&mut g, None);
        assert_eq!(g.nodes.len(), 0, "add undone");
        h.redo(&mut g, None);
        assert_eq!(g.nodes.len(), 1, "add redone");
    }

    #[test]
    fn remove_restores_links() {
        let mut g = Graph::new();
        let mut h = History::default();
        let sys = g.insert(GraphNode::new(NodeKind::ParticleSystem, [0.0, 0.0]));
        let data = g.insert(GraphNode::new(NodeKind::ParticleData, [0.0, 0.0]));
        g.connect(Link {
            from_node: data,
            from_socket: 0,
            to_node: sys,
            to_socket: 0,
        })
        .unwrap();

        h.apply(
            &mut g,
            Command::RemoveNode {
                id: data,
                removed: None,
            },
        );
        assert_eq!(g.links.len(), 0);
        h.undo(&mut g, None);
        assert_eq!(g.links.len(), 1, "link restored with node");
    }

    #[test]
    fn param_merge_keeps_single_undo_step() {
        let mut g = Graph::new();
        let mut h = History::default();
        let id = g.insert(GraphNode::new(NodeKind::BoxEmitter, [0.0, 0.0]));

        for i in 1..=10 {
            h.apply_merged(
                &mut g,
                Command::SetParam {
                    id,
                    key: "Speed",
                    value: Value::F32(i as f32),
                    previous: None,
                },
            );
        }
        assert_eq!(g.nodes[&id].param("Speed"), Some(&Value::F32(10.0)));
        h.undo(&mut g, None);
        // One undo returns to the original default (30.0), not to 9.0.
        assert_eq!(g.nodes[&id].param("Speed"), Some(&Value::F32(30.0)));
    }
}
