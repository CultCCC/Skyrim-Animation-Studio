//! What the Effect Shader node previews, kept in core so it can be tested
//! and so the node body and Properties cannot disagree.
//!
//! Two facts shape it. The atlas a shader is drawn through is NOT on the
//! shader: rows/cols and the explicit cell table live on the Particle Data
//! of each system the shader is wired into, and the frames played come
//! from that system's Atlas Frames. So the preview reads them off the
//! wires ([`atlas_views`]), one view per system. And "As game" means the
//! shader's own arithmetic ([`composite`]): the greyscale-to-palette ramp
//! picked by the texture's green (colour) and alpha, the tint, the UV
//! transform and the clamp mode -- the same rules `fs_mesh` applies, at a
//! white vertex colour and a head-on view, so falloff is left out.

use crate::graph::{Graph, GraphNode, NodeId, NodeKind};
use crate::texture::Image;
use crate::value::Value;

/// Atlas Frames playback, fudge-free.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameRange {
    pub start: f32,
    pub end: f32,
    pub loop_start: f32,
    pub frame_count: f32,
}

impl FrameRange {
    /// The frame a particle aged `t` shows, stepping `frame_count` times
    /// per `life_span`: start..=end once, then looping from Loop Start --
    /// the simulator's `subtex` stepping without the per-particle fudge.
    pub fn frame_at(&self, t: f32, life_span: f32) -> u32 {
        // The game truncates frames to a byte (`as u8` in the sim).
        let s = self.start.max(0.0) as i64;
        let e = self.end.max(0.0) as i64;
        let l = self.loop_start.max(0.0) as i64;
        if e < s {
            return s as u32;
        }
        let k = (t.max(0.0) * self.frame_count.max(0.0) / life_span.max(1e-6)).floor() as i64;
        if s + k <= e {
            return (s + k) as u32;
        }
        let span = (e - l + 1).max(1);
        let r = k - (e - s + 1);
        (l.min(e) + r.rem_euclid(span)) as u32
    }
}

/// One system a shader is drawn through, and the atlas it is drawn with.
#[derive(Debug, Clone, PartialEq)]
pub struct AtlasView {
    pub system: NodeId,
    pub system_name: String,
    /// `(U offset, U size, V offset, V size)` per cell; never empty.
    pub cells: Vec<[f32; 4]>,
    pub frames: Option<FrameRange>,
    /// The first emitter's Life Span, for frame playback.
    pub life_span: Option<f32>,
}

/// Every Particle System whose Shader input is `shader`, in node order.
pub fn atlas_views(graph: &Graph, shader: NodeId) -> Vec<AtlasView> {
    let mut systems: Vec<NodeId> = graph
        .nodes
        .iter()
        .filter(|(id, n)| {
            n.kind == NodeKind::ParticleSystem
                && graph.incoming(**id, 1).any(|l| l.from_node == shader)
        })
        .map(|(id, _)| *id)
        .collect();
    systems.sort();
    systems
        .into_iter()
        .map(|sys| {
            let node = &graph.nodes[&sys];
            let cells = graph
                .incoming(sys, 0)
                .next()
                .and_then(|l| graph.nodes.get(&l.from_node))
                .map(atlas_cells)
                .unwrap_or_else(|| vec![WHOLE]);
            let modifiers: Vec<&GraphNode> = graph
                .incoming(sys, 3)
                .filter_map(|l| graph.nodes.get(&l.from_node))
                .collect();
            let frames = modifiers
                .iter()
                .find(|n| n.kind == NodeKind::SubTexModifier)
                .map(|n| FrameRange {
                    start: n.f32_or("Start Frame", 0.0),
                    end: n.f32_or("End Frame", 15.0),
                    loop_start: n.f32_or("Loop Start Frame", 0.0),
                    frame_count: n.f32_or("Frame Count", 16.0),
                });
            let life_span = modifiers
                .iter()
                .find(|n| {
                    matches!(
                        n.kind,
                        NodeKind::BoxEmitter
                            | NodeKind::SphereEmitter
                            | NodeKind::CylinderEmitter
                            | NodeKind::MeshEmitter
                    )
                })
                .map(|n| n.f32_or("Life Span", 1.0));
            AtlasView {
                system: sys,
                system_name: node.str_or("Name", "ParticleFX"),
                cells,
                frames,
                life_span,
            }
        })
        .collect()
}

const WHOLE: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

/// The cells a Particle Data node draws with: its explicit table when it
/// carries one -- that is what the game samples -- else the rows x cols
/// grid, else the whole texture as one cell.
pub fn atlas_cells(data: &GraphNode) -> Vec<[f32; 4]> {
    let explicit = crate::compile::parse_atlas_offsets(&data.str_or("Atlas Offsets", ""));
    if !explicit.is_empty() {
        return explicit;
    }
    let derived = crate::compile::derived_atlas_offsets(
        data.u32_or("Subtexture Rows", 1).max(1),
        data.u32_or("Subtexture Cols", 1).max(1),
    );
    if derived.is_empty() {
        vec![WHOLE]
    } else {
        derived
    }
}

/// The shader parameters the composite reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub tint: [f32; 4],
    pub base_color_scale: f32,
    pub greyscale_color: bool,
    pub greyscale_alpha: bool,
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    pub clamp_u: bool,
    pub clamp_v: bool,
}

pub fn look_of(node: &GraphNode) -> Look {
    // The flags sample nothing without a ramp texture: the gate the game,
    // the mesh path and the particle path all apply.
    let ramp = !node.str_or("Greyscale Texture", "").is_empty();
    let clamp = match node.params.iter().find(|(k, _)| *k == "Texture Clamp") {
        Some((_, Value::Enum(opts, i))) => opts.get(*i).copied().unwrap_or(""),
        _ => "",
    };
    Look {
        tint: node.color4_or("Base Color", [1.0; 4]),
        base_color_scale: node.f32_or("Base Color Scale", 1.0),
        greyscale_color: ramp && node.bool_or("Greyscale Color", false),
        greyscale_alpha: ramp && node.bool_or("Greyscale Alpha", false),
        uv_offset: [node.f32_or("UV Offset U", 0.0), node.f32_or("UV Offset V", 0.0)],
        uv_scale: [node.f32_or("UV Scale U", 1.0), node.f32_or("UV Scale V", 1.0)],
        clamp_u: clamp == "Clamp Both" || clamp.starts_with("Clamp U"),
        clamp_v: clamp == "Clamp Both" || clamp.ends_with("Clamp V"),
    }
}

/// Nearest-texel sample at `(u, v)`, wrapped or clamped per axis.
fn sample(img: &Image, u: f32, v: f32, clamp_u: bool, clamp_v: bool) -> [f32; 4] {
    if img.width == 0 || img.height == 0 {
        return [0.0; 4];
    }
    let fold = |t: f32, clamp: bool| if clamp { t.clamp(0.0, 1.0) } else { t - t.floor() };
    let x = ((fold(u, clamp_u) * img.width as f32) as u32).min(img.width - 1);
    let y = ((fold(v, clamp_v) * img.height as f32) as u32).min(img.height - 1);
    let p = img.pixel(x, y);
    [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]
}

/// The texture as the game draws it through this shader, `w x h`, straight
/// RGBA. `ramp` is the greyscale palette, when one resolved.
///
/// Mirrors `fs_mesh`: `c = base * tint`; with Greyscale Color the ramp is
/// sampled at `(base.g, Base Color.r)` and times Base Color Scale; with
/// Greyscale Alpha at `(base.a, tint.a^2)` and its alpha taken.
pub fn composite(base: &Image, ramp: Option<&Image>, look: &Look, w: u32, h: u32) -> Image {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32 * look.uv_scale[0] + look.uv_offset[0];
            let v = (y as f32 + 0.5) / h as f32 * look.uv_scale[1] + look.uv_offset[1];
            let b = sample(base, u, v, look.clamp_u, look.clamp_v);
            let t = look.tint;
            let mut c = [b[0] * t[0], b[1] * t[1], b[2] * t[2], b[3] * t[3]];
            if let Some(ramp) = ramp {
                if look.greyscale_color {
                    let r = sample(ramp, b[1].clamp(0.0, 1.0), t[0].clamp(0.0, 1.0), true, true);
                    let s = look.base_color_scale;
                    c[0] = r[0] * s;
                    c[1] = r[1] * s;
                    c[2] = r[2] * s;
                }
                if look.greyscale_alpha {
                    let row = (t[3] * t[3]).clamp(0.0, 1.0);
                    c[3] = sample(ramp, b[3].clamp(0.0, 1.0), row, true, true)[3];
                }
            }
            rgba.extend(c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8));
        }
    }
    Image { width: w, height: h, rgba }
}
