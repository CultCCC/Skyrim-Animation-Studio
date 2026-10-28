//! Graph validation against the minimum rules a Skyrim particle system must
//! satisfy to work in game (see the knowledge base: `nif/particle-systems.md`).

use crate::graph::{Graph, NodeId, NodeKind};
use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub severity: Severity,
    pub node: Option<NodeId>,
    pub message: String,
}

fn err(node: NodeId, message: impl Into<String>) -> Finding {
    Finding {
        severity: Severity::Error,
        node: Some(node),
        message: message.into(),
    }
}

fn warn(node: NodeId, message: impl Into<String>) -> Finding {
    Finding {
        severity: Severity::Warning,
        node: Some(node),
        message: message.into(),
    }
}

/// Validate the whole graph. Sorted: errors first.
/// Hold the document's clips to the chosen STOCK behavior graph's
/// contract (`AutoplayGraph::contract`). Two mistakes the game reports
/// nowhere: a clip the graph plays that the file does not have -- the
/// state plays nothing -- and a waiting state whose clip carries no `end`
/// key -- the graph never leaves it. Names match case-insensitively,
/// because the game's string pool does. A custom graph has no known
/// contract and is not judged.
fn validate_behavior_contract(
    graph: &Graph,
    id: NodeId,
    node: &crate::graph::GraphNode,
    findings: &mut Vec<Finding>,
) {
    let path = node.str_or("Behavior Graph", "");
    let Some(stock) = crate::behavior::stock_graph(&path) else {
        return;
    };
    for req in (stock.build)().contract().clips {
        match graph.clips.iter().find(|c| c.name.eq_ignore_ascii_case(&req.sequence)) {
            None => findings.push(warn(
                id,
                format!(
                    "The behavior graph {} plays clip {}, which this file does not have: \
                     that state will play nothing.",
                    stock.path, req.sequence
                ),
            )),
            Some(c)
                if req.needs_end_key
                    && !c.text_keys.iter().any(|(_, k)| k.eq_ignore_ascii_case("end")) =>
            {
                findings.push(warn(
                    id,
                    format!(
                        "Clip {} has no `end` key, and the behavior graph {} leaves that \
                         state only on it: the effect will stay in {} forever.",
                        c.name, stock.path, req.sequence
                    ),
                ))
            }
            Some(_) => {}
        }
    }
}

pub fn validate(graph: &Graph) -> Vec<Finding> {
    let mut findings = Vec::new();

    let mut has_output = false;
    for (&id, node) in &graph.nodes {
        match node.kind {
            NodeKind::SceneOutput => {
                has_output = true;
                if graph.incoming(id, 0).count() == 0 {
                    findings.push(warn(id, "Scene Output has no systems connected"));
                }
                validate_behavior_contract(graph, id, node, &mut findings);
            }
            NodeKind::ParticleSystem => validate_system(graph, id, &mut findings),
            NodeKind::EmitterCtlr => {
                if graph.incoming(id, 0).count() == 0 {
                    findings.push(err(
                        id,
                        "Emitter Controller is not bound to an emitter (wire one into \
                         `Emitter`): it exports with an empty modifier name, binds to \
                         nothing in game and emits no particles",
                    ));
                }
            }
            NodeKind::MeshEmitter => {
                // This used to warn unconditionally that the mesh list
                // "is not written on export yet". It IS written now,
                // resolved back to the source shapes by name, so the
                // finding belongs to the case that can still go wrong:
                // no shape named at all.
                let named = node
                    .param("Emitter Meshes")
                    .and_then(|v| match v {
                        Value::Str(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .map(|s| s.split(',').filter(|p| !p.trim().is_empty()).count())
                    .unwrap_or(0);
                if named == 0 {
                    findings.push(warn(
                        id,
                        "Mesh Emitter names no shape, so it has nothing to \
                         sample and will emit nothing in game",
                    ));
                }
            }
            // A ZERO dimension is a POINT emitter, not a mistake: the
            // spawner places a particle at `(u - 0.5) * size`, so a size
            // of zero puts every one of them exactly on the anchor --
            // which is what a trail wants, since the thing that moves is
            // the anchor. Measured over the vanilla corpus: 438 flagged
            // box dimensions, ALL of them zero on all three axes, none
            // negative, across 242 files. `should be positive` was
            // therefore wrong advice 1,314 times and never right once.
            //
            // A NEGATIVE dimension is still worth saying: it mirrors the
            // volume rather than sizing it, which nothing authors on
            // purpose. Zero such dimensions in the corpus, so this arm
            // is for what a user can type, not for what ships.
            NodeKind::BoxEmitter => {
                for key in ["Width", "Height", "Depth"] {
                    if let Some(Value::F32(v)) = node.param(key) {
                        if *v < 0.0 {
                            findings.push(warn(
                                id,
                                format!(
                                    "Box emitter {key} is negative, which mirrors the \
                                     volume rather than sizing it; use 0 for a point \
                                     emitter"
                                ),
                            ));
                        }
                    }
                }
            }
            NodeKind::SphereEmitter | NodeKind::CylinderEmitter => {
                // Same rule, same reason: radius 0 is a point.
                if let Some(Value::F32(v)) = node.param("Radius") {
                    if *v < 0.0 {
                        findings.push(warn(
                            id,
                            "Emitter radius is negative; use 0 for a point emitter",
                        ));
                    }
                }
            }
            NodeKind::EffectShader => {
                // The greyscale flags redirect sampling through the palette
                // texture; without one the game shows the raw greyscale tile.
                let palette_missing = match node.param("Greyscale Texture") {
                    Some(Value::TexturePath(p)) => p.trim().is_empty(),
                    _ => true,
                };
                let flag_on = |key| matches!(node.param(key), Some(Value::Bool(true)));
                if palette_missing && (flag_on("Greyscale Color") || flag_on("Greyscale Alpha"))
                {
                    findings.push(warn(
                        id,
                        "Greyscale Color/Alpha needs a Greyscale Texture (palette ramp); \
                         without one the game samples nothing",
                    ));
                }
            }
            _ => {}
        }

        // Negative variations are suspicious on any node.
        for (key, value) in &node.params {
            if key.ends_with("Variation") {
                if let Value::F32(v) = value {
                    if *v < 0.0 {
                        findings.push(warn(id, format!("{key} is negative")));
                    }
                }
            }
        }
    }

    if !has_output && !graph.nodes.is_empty() {
        findings.push(Finding {
            severity: Severity::Warning,
            node: None,
            message: "Graph has no Scene Output node; nothing will be exported".into(),
        });
    }

    findings.sort_by_key(|f| match f.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    });
    findings
}

fn validate_system(graph: &Graph, id: NodeId, findings: &mut Vec<Finding>) {
    // Socket indices of ParticleSystem inputs; see schema.
    const DATA: usize = 0;
    const SHADER: usize = 1;
    const ALPHA: usize = 2;
    const MODIFIERS: usize = 3;
    const CONTROLLERS: usize = 4;

    if graph.incoming(id, DATA).count() == 0 {
        findings.push(err(id, "Particle System needs Particle Data (NiPSysData)"));
    }
    if graph.incoming(id, SHADER).count() == 0 {
        findings.push(err(
            id,
            "Particle System needs an Effect Shader (BSEffectShaderProperty)",
        ));
    }
    if graph.incoming(id, ALPHA).count() == 0 {
        findings.push(err(
            id,
            "Particle System needs an Alpha property (NiAlphaProperty)",
        ));
    }

    // The update controller is what advances a system, and the game
    // simply does not simulate one without it (1.5.97: the emitter
    // controller's chain is what the update pass walks). The compiler
    // used to APPEND one silently when the document had none -- which
    // meant deleting the node changed nothing in the file and said
    // nothing about it. The document is written as it stands now, so
    // the rule lives here, where a rule the user can violate belongs.
    if !graph
        .incoming(id, CONTROLLERS)
        .any(|l| graph.nodes.get(&l.from_node).is_some_and(|n| n.kind == NodeKind::UpdateCtlr))
    {
        findings.push(err(
            id,
            "Particle System has no Update Controller (NiPSysUpdateCtlr): the game will \
             not simulate this system at all",
        ));
    }

    // "Active" is a property of the collider MANAGER, and the format
    // ships exactly one manager per system (measured: 164 systems
    // across both corpora and the mod roots, every one with a single
    // manager chaining up to 7 colliders). NiPSysColliderBase carries
    // bounce, the two collide flags, its parent, its next and its
    // object — no per-collider enable exists. So a system whose
    // colliders disagree cannot be written faithfully.
    let collider_active: Vec<bool> = graph
        .incoming(id, MODIFIERS)
        .filter_map(|link| graph.nodes.get(&link.from_node))
        .filter(|n| n.kind == NodeKind::ColliderModifier)
        .map(|n| n.bool_or("Active", true))
        .collect();
    if collider_active.len() > 1
        && collider_active.iter().any(|a| *a)
        && collider_active.iter().any(|a| !*a)
    {
        findings.push(Finding {
            node: Some(id),
            severity: Severity::Warning,
            message: "Colliders on one system disagree about Active, but the format stores that flag once per system (on the shared NiPSysColliderManager) — the export will leave the colliders ON. Switch them all off to disable containment."
                .to_string(),
        });
    }

    let mut has_emitter = false;
    let mut needs_color = false;
    let mut needs_rotation = false;
    for link in graph.incoming(id, MODIFIERS) {
        if let Some(src) = graph.nodes.get(&link.from_node) {
            match src.kind {
                NodeKind::BoxEmitter
                | NodeKind::SphereEmitter
                | NodeKind::CylinderEmitter
                | NodeKind::MeshEmitter => {
                    has_emitter = true;
                }
                NodeKind::SimpleColorModifier => needs_color = true,
                NodeKind::RotationModifier => needs_rotation = true,
                _ => {}
            }
        }
    }
    if !has_emitter {
        findings.push(err(
            id,
            "Particle System has no emitter; no particles will spawn",
        ));
    }

    let mut has_update = false;
    let mut has_emitter_ctlr = false;
    for link in graph.incoming(id, CONTROLLERS) {
        if let Some(src) = graph.nodes.get(&link.from_node) {
            match src.kind {
                NodeKind::UpdateCtlr => has_update = true,
                // A MULTI emitter controller is an emitter controller:
                // `BSPSysMultiTargetEmitterCtlr` carries the same Birth
                // Rate, for the addon-node system vanilla uses on every
                // MPS trail. Counting only the plain one told 186 of the
                // 187 vanilla files that fire this rule that they will
                // not emit in game, while they emit at their authored
                // rate -- the soul-rend trail at 60/s over 4 emitters.
                // (The one file left really has no controller at all.)
                NodeKind::EmitterCtlr | NodeKind::MultiEmitterCtlr => has_emitter_ctlr = true,
                _ => {}
            }
        }
    }
    if !has_update {
        findings.push(err(
            id,
            "Particle System needs an Update Controller (NiPSysUpdateCtlr)",
        ));
    }
    if has_emitter && !has_emitter_ctlr {
        findings.push(warn(
            id,
            "No Emitter Controller: Birth Rate lives on NiPSysEmitterCtlr, so the \
             emitter will not emit in game",
        ));
    }

    // Data flags must match modifiers that depend on them.
    if let Some(data_link) = graph.incoming(id, DATA).next() {
        if let Some(data) = graph.nodes.get(&data_link.from_node) {
            if needs_color && data.param("Has Color") != Some(&Value::Bool(true)) {
                findings.push(err(
                    id,
                    "Color Over Life requires `Has Color` enabled on Particle Data",
                ));
            }
            // Strip trails draw ribbons, not spinning billboards, and even
            // billboard content ships this combination and works in game
            // (kitty spell pack) — so it warns rather than blocks export.
            let strips = data.param("Strip Trails") == Some(&Value::Bool(true));
            if needs_rotation && !strips && data.param("Has Rotations") != Some(&Value::Bool(true))
            {
                findings.push(warn(
                    id,
                    "Rotation modifier present but `Has Rotations` is off on Particle \
                     Data — the game may ignore per-particle spin for this system",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::BUILTIN_TEMPLATES;

    #[test]
    fn templates_have_no_errors() {
        for t in BUILTIN_TEMPLATES {
            let g = (t.build)();
            let findings = validate(&g);
            let errors: Vec<_> = findings
                .iter()
                .filter(|f| f.severity == Severity::Error)
                .collect();
            assert!(errors.is_empty(), "{}: {:?}", t.name, errors);
        }
    }

    #[test]
    fn empty_system_is_flagged() {
        use crate::graph::{GraphNode, NodeKind};
        let mut g = Graph::new();
        g.insert(GraphNode::new(NodeKind::ParticleSystem, [0.0, 0.0]));
        let findings = validate(&g);
        assert!(findings.iter().any(|f| f.severity == Severity::Error));
    }
}
