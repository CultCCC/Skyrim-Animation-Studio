//! Built-in particle system templates.
//!
//! Each template builds a complete, valid graph the user can drop into a
//! project and immediately export. User templates (saved graphs) live in the
//! project's `templates/` folder and are handled by the app layer; these are
//! the "factory" starters.

use crate::graph::{Graph, GraphNode, Link, NodeKind};
use crate::value::Value;

pub struct Template {
    pub name: &'static str,
    pub description: &'static str,
    pub build: fn() -> Graph,
}

pub const BUILTIN_TEMPLATES: &[Template] = &[
    Template {
        name: "Empty System",
        description: "Minimum valid chain: system + data + shader + alpha + \
                      one emitter with controllers.",
        build: build_empty_system,
    },
    Template {
        name: "Torch Flame",
        description: "Rising additive flame with color-over-life and drag.",
        build: build_torch_flame,
    },
    Template {
        name: "Spark Burst",
        description: "Short one-shot burst of fast sparks under gravity.",
        build: build_spark_burst,
    },
];

/// The scaffold every template shares: output <- system <- data/shader/alpha
/// plus the update controller. Templates tweak the pieces they care about.
pub struct Scaffold {
    pub system: crate::graph::NodeId,
    pub data: crate::graph::NodeId,
    pub shader: crate::graph::NodeId,
    pub alpha: crate::graph::NodeId,
    /// X column templates place their modifiers in.
    pub column: f32,
}

fn base_system(graph: &mut Graph) -> Scaffold {
    let out = graph.insert(GraphNode::new(NodeKind::SceneOutput, [1160.0, 0.0]));
    let sys = graph.insert(GraphNode::new(NodeKind::ParticleSystem, [820.0, 0.0]));
    let data = graph.insert(GraphNode::new(NodeKind::ParticleData, [430.0, -225.0]));
    let shader = graph.insert(GraphNode::new(NodeKind::EffectShader, [430.0, -75.0]));
    let alpha = graph.insert(GraphNode::new(NodeKind::AlphaProperty, [430.0, 75.0]));
    let update = graph.insert(GraphNode::new(NodeKind::UpdateCtlr, [430.0, 225.0]));

    let mut link = |from, from_socket, to, to_socket| {
        graph
            .connect(Link {
                from_node: from,
                from_socket,
                to_node: to,
                to_socket,
            })
            .expect("template link must be valid");
    };
    link(sys, 0, out, 0);
    link(data, 0, sys, 0);
    link(shader, 0, sys, 1);
    link(alpha, 0, sys, 2);
    link(update, 0, sys, 4);
    Scaffold {
        system: sys,
        data,
        shader,
        alpha,
        column: 0.0,
    }
}

fn build_empty_system() -> Graph {
    let mut g = Graph::new();
    let sc = base_system(&mut g);
    let (sys, ex) = (sc.system, sc.column);
    let emitter = g.insert(GraphNode::new(NodeKind::SphereEmitter, [ex, -225.0]));
    let ctlr = g.insert(GraphNode::new(NodeKind::EmitterCtlr, [ex, 225.0]));
    g.connect(Link {
        from_node: emitter,
        from_socket: 0,
        to_node: sys,
        to_socket: 3,
    })
    .unwrap();
    g.connect(Link {
        from_node: emitter,
        from_socket: 0,
        to_node: ctlr,
        to_socket: 0,
    })
    .unwrap();
    g.connect(Link {
        from_node: ctlr,
        from_socket: 0,
        to_node: sys,
        to_socket: 4,
    })
    .unwrap();
    g
}

fn build_torch_flame() -> Graph {
    let mut g = Graph::new();
    let sc = base_system(&mut g);
    let (sys, ex) = (sc.system, sc.column);

    let emitter = g.insert(GraphNode::new(NodeKind::CylinderEmitter, [ex, -225.0]));
    {
        let e = g.nodes.get_mut(&emitter).unwrap();
        e.set_param("Speed", Value::F32(12.0));
        e.set_param("Speed Variation", Value::F32(4.0));
        e.set_param("Life Span", Value::F32(0.8));
        e.set_param("Life Span Variation", Value::F32(0.2));
        e.set_param("Initial Radius", Value::F32(6.0));
        e.set_param("Radius Variation", Value::F32(2.0));
        e.set_param("Radius", Value::F32(4.0));
        e.set_param("Height", Value::F32(2.0));
    }
    // Texture and atlas taken from a vanilla campfire
    // (meshes/clutter/giant/giantcampfire01burning.nif), so the template
    // previews like a real Skyrim fire rather than a coloured blob.
    if let Some(sh) = g.nodes.get_mut(&sc.shader) {
        sh.set_param(
            "Source Texture",
            Value::TexturePath(r"textures\effects\FXfireAnim04loop.dds".into()),
        );
    }
    if let Some(d) = g.nodes.get_mut(&sc.data) {
        d.set_param("Subtexture Rows", Value::U32(4));
        d.set_param("Subtexture Cols", Value::U32(4));
    }

    let ctlr = g.insert(GraphNode::new(NodeKind::EmitterCtlr, [ex, 225.0]));
    if let Some(c) = g.nodes.get_mut(&ctlr) {
        c.set_param("Birth Rate", Value::F32(40.0));
    }

    let color = g.insert(GraphNode::new(NodeKind::SimpleColorModifier, [ex, -75.0]));
    if let Some(c) = g.nodes.get_mut(&color) {
        c.set_param("Color 1", Value::Color4([1.0, 0.85, 0.4, 0.0]));
        c.set_param("Color 2", Value::Color4([1.0, 0.45, 0.1, 1.0]));
        c.set_param("Color 3", Value::Color4([0.4, 0.05, 0.02, 0.0]));
    }

    let drag = g.insert(GraphNode::new(NodeKind::DragModifier, [ex, 75.0]));
    if let Some(d) = g.nodes.get_mut(&drag) {
        d.set_param("Percentage", Value::F32(0.08));
    }

    for m in [emitter, color, drag] {
        g.connect(Link {
            from_node: m,
            from_socket: 0,
            to_node: sys,
            to_socket: 3,
        })
        .unwrap();
    }
    g.connect(Link {
        from_node: emitter,
        from_socket: 0,
        to_node: ctlr,
        to_socket: 0,
    })
    .unwrap();
    g.connect(Link {
        from_node: ctlr,
        from_socket: 0,
        to_node: sys,
        to_socket: 4,
    })
    .unwrap();
    g
}

fn build_spark_burst() -> Graph {
    let mut g = Graph::new();
    let sc = base_system(&mut g);
    let (sys, ex) = (sc.system, sc.column);

    let emitter = g.insert(GraphNode::new(NodeKind::SphereEmitter, [ex, -225.0]));
    {
        let e = g.nodes.get_mut(&emitter).unwrap();
        e.set_param("Speed", Value::F32(120.0));
        e.set_param("Speed Variation", Value::F32(60.0));
        e.set_param("Life Span", Value::F32(0.5));
        e.set_param("Life Span Variation", Value::F32(0.3));
        e.set_param("Initial Radius", Value::F32(1.5));
        e.set_param("Radius", Value::F32(1.0));
    }
    let ctlr = g.insert(GraphNode::new(NodeKind::EmitterCtlr, [ex, 225.0]));
    if let Some(c) = g.nodes.get_mut(&ctlr) {
        c.set_param("Birth Rate", Value::F32(200.0));
        c.set_param("Stop Time", Value::F32(0.15));
        c.set_param("Loop", Value::Bool(false));
    }

    if let Some(sh) = g.nodes.get_mut(&sc.shader) {
        sh.set_param(
            "Source Texture",
            Value::TexturePath(r"textures\effects\fxglowwhite01.dds".into()),
        );
    }

    let gravity = g.insert(GraphNode::new(NodeKind::GravityModifier, [ex, -75.0]));
    if let Some(gr) = g.nodes.get_mut(&gravity) {
        gr.set_param("Strength", Value::F32(400.0));
    }

    for m in [emitter, gravity] {
        g.connect(Link {
            from_node: m,
            from_socket: 0,
            to_node: sys,
            to_socket: 3,
        })
        .unwrap();
    }
    g.connect(Link {
        from_node: emitter,
        from_socket: 0,
        to_node: ctlr,
        to_socket: 0,
    })
    .unwrap();
    g.connect(Link {
        from_node: ctlr,
        from_socket: 0,
        to_node: sys,
        to_socket: 4,
    })
    .unwrap();
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_templates_build_valid_graphs() {
        for t in BUILTIN_TEMPLATES {
            let g = (t.build)();
            assert!(!g.nodes.is_empty(), "{} produced empty graph", t.name);
            // Every link must satisfy the type system when replayed.
            let mut fresh = Graph::new();
            for (id, node) in &g.nodes {
                fresh.insert_with_id(*id, node.clone());
            }
            for l in &g.links {
                fresh
                    .connect(*l)
                    .unwrap_or_else(|e| panic!("{}: invalid template link {:?}: {e}", t.name, l));
            }
        }
    }

    #[test]
    fn templates_serialize() {
        for t in BUILTIN_TEMPLATES {
            let g = (t.build)();
            let restored = Graph::from_json(&g.to_json()).expect(t.name);
            assert_eq!(restored.nodes.len(), g.nodes.len());
            assert_eq!(restored.links.len(), g.links.len());
        }
    }
}
