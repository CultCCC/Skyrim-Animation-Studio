//! Animation clips as part of the document.
//!
//! A clip is a `NiControllerSequence`: what a behavior graph plays by
//! name (BeginLoopEnd plays mBegin, mLoop, mEnd). Each of its tracks
//! drives one parameter of one node -- the same parameter the node's own
//! key track drives when it runs free -- so a clip track is stored as
//! (node, parameter, curve), not as a second copy of the format's
//! controlled-block plumbing. Measured over the corpus's behavior-bound
//! particle files, 72% of clip tracks land on a parameter the graph
//! models; tracks on anything else (scene node transforms, mesh
//! shaders) stay in the file's bytes, untouched.
//!
//! Provenance mirrors `GraphNode::source_block`: a clip remembers the
//! sequence block it came from and a track its controlled-block slot,
//! which is what lets an edit be written back into exactly that key
//! data and nowhere else.

use crate::curve::Curve;
use crate::graph::NodeId;

/// One track of a clip: the keys a clip plays into one node parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipTrack {
    pub node: NodeId,
    /// The node parameter it drives ("Birth Rate Keys", "Alpha Keys"...).
    pub param: &'static str,
    pub curve: Curve,
    /// The controlled-block index in the source sequence, when the track
    /// came from a file.
    pub source_slot: Option<u32>,
}

/// Which clips drive which node parameter, keyed like the decompiler's
/// `driven` map, but read off the DOCUMENT's clips.
///
/// The decompiler's map is a snapshot of the FILE; this is the document,
/// so a node drawing "driven by mLoop" draws the curve a save will write.
/// Clips in document order and each clip's tracks in slot order, which is
/// the order the decompiler builds its map in, so at import the two are
/// the same list. Keyed by source block like the decompiler's; a node
/// without provenance has no key in it yet.
pub fn driven_view(
    graph: &crate::graph::Graph,
) -> Vec<((u32, &'static str), Vec<crate::sequence::DrivenBy>)> {
    let mut out: Vec<((u32, &'static str), Vec<crate::sequence::DrivenBy>)> = Vec::new();
    for (clip_index, clip) in graph.clips.iter().enumerate() {
        for track in &clip.tracks {
            let Some(block) = graph.nodes.get(&track.node).and_then(|n| n.source_block) else {
                continue;
            };
            let entry = crate::sequence::DrivenBy {
                clip: clip.name.clone(),
                clip_index,
                slot: track.source_slot.unwrap_or(0) as usize,
                curve: track.curve.clone(),
            };
            let key = (block, track.param);
            match out.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(entry),
                None => out.push((key, vec![entry])),
            }
        }
    }
    out
}

impl Clip {
    /// A new, empty clip meeting one requirement of a behavior graph's
    /// contract.
    ///
    /// A clip whose state waits on `end` must END: clamped, `end` at its
    /// last frame. One the game moves on from (StopEffect, a script's
    /// event) runs until it does: looped. Both carry `start` and `end`,
    /// the convention every one of the 185 vanilla BeginLoopEnd files
    /// follows. One second long; the tracks are the author's to add.
    pub fn for_requirement(req: &crate::behavior::ClipRequirement) -> Clip {
        Clip {
            name: req.sequence.clone(),
            looping: !req.needs_end_key,
            start_time: 0.0,
            stop_time: 1.0,
            frequency: 1.0,
            text_keys: vec![(0.0, "start".to_string()), (1.0, "end".to_string())],
            tracks: Vec::new(),
            source_block: None,
        }
    }
}

/// One clip: a `NiControllerSequence`.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub name: String,
    /// CycleType LOOP; otherwise CLAMP, which holds the last values.
    pub looping: bool,
    pub start_time: f32,
    pub stop_time: f32,
    pub frequency: f32,
    /// `(time, text)`: `start`, `end`, and events such as
    /// `SoundPlay.MAGRuneImpact`. An `end` key is what advances a
    /// behavior graph out of a state that waits on it.
    pub text_keys: Vec<(f32, String)>,
    pub tracks: Vec<ClipTrack>,
    /// The sequence block this clip was decompiled from.
    pub source_block: Option<u32>,
}
