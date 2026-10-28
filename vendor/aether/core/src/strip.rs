//! Strip-trail ribbon geometry, built from a simulation's recorded trails.
//!
//! The rule is RE'd from SkyrimSE 1.5.97 (`ComputeStripPointVertsAndFrame`
//! @ 0x140D25050, `FillStripVertexIndexBuffers` @ 0x140D76D80): each trail
//! extrudes a CROSSED DOUBLE RIBBON — two perpendicular quad strips sharing
//! the centerline — with a rotation-minimizing frame transported along the
//! path from the fixed reference vector (0, 0.1, 1)/√1.01. The texture's U
//! runs 0 at the oldest point to 1 at the head, stepped by the high-water
//! segment count so it unreveals while the trail grows; V spans each
//! ribbon. Ribbon width is the billboard rule's `radius × size`.

use crate::mesh::{BlendMode, EffectMaterial, MeshVertex, RenderMesh, Transform};
use crate::sim::{Simulation, Trail};

fn norm(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(v: [f32; 3], s: f32) -> [f32; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Rotate `v` about unit `axis` by `angle` (Rodrigues).
fn rotate_about(v: [f32; 3], axis: [f32; 3], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    let k_cross_v = cross(axis, v);
    let k_dot_v = dot(axis, v);
    [
        v[0] * c + k_cross_v[0] * s + axis[0] * k_dot_v * (1.0 - c),
        v[1] * c + k_cross_v[1] * s + axis[1] * k_dot_v * (1.0 - c),
        v[2] * c + k_cross_v[2] * s + axis[2] * k_dot_v * (1.0 - c),
    ]
}

/// Append one trail's crossed ribbons. Returns false when the vertex
/// budget (u16 indices) is exhausted.
fn append_trail(trail: &Trail, vertices: &mut Vec<MeshVertex>, indices: &mut Vec<u16>) -> bool {
    let n = trail.points.len();
    if n < 2 {
        return true;
    }
    if vertices.len() + n * 4 > u16::MAX as usize {
        return false;
    }

    // Averaged tangents: first uses the leading segment, middles average
    // the incoming direction with the outgoing segment, the newest point
    // reuses its predecessor's direction (the game's rule).
    let mut dirs = Vec::with_capacity(n);
    let mut prev = norm(sub(trail.points[1].position, trail.points[0].position));
    dirs.push(prev);
    for i in 1..n - 1 {
        let seg = norm(sub(trail.points[i + 1].position, trail.points[i].position));
        let d = norm(add(seg, prev));
        dirs.push(d);
        prev = d;
    }
    dirs.push(prev);

    // Rotation-minimizing frame from the game's fixed reference vector.
    let reference = norm([0.0, 0.1, 1.0]);
    let mut side1 = norm(cross(reference, dirs[0]));
    let mut side0 = norm(cross(side1, dirs[0]));

    let base = vertices.len() as u16;
    let seg_span = (trail.segments.max(2) - 1) as f32;
    for (i, point) in trail.points.iter().enumerate() {
        if i > 0 {
            let axis = cross(dirs[i - 1], dirs[i]);
            let axis_len = (dot(axis, axis)).sqrt();
            if axis_len > 1e-6 {
                let angle = dot(dirs[i - 1], dirs[i]).clamp(-1.0, 1.0).acos();
                let axis = scale(axis, 1.0 / axis_len);
                side0 = norm(rotate_about(side0, axis, angle));
                side1 = norm(rotate_about(side1, axis, angle));
            }
        }
        let half = point.width * 0.5;
        let u = (i as f32 / seg_span).min(1.0);
        let c = point.position;
        // Ribbon A spans ±side1 and faces side0; ribbon B the reverse.
        for (offset, v, normal) in [
            (scale(side1, half), 0.0, side0),
            (scale(side1, -half), 1.0, side0),
            (scale(side0, half), 0.0, side1),
            (scale(side0, -half), 1.0, side1),
        ] {
            vertices.push(MeshVertex {
                position: add(c, offset),
                normal,
                uv: [u, v],
                color: point.color,
            });
        }
    }
    for i in 0..(n - 1) as u16 {
        let p = base + i * 4;
        let q = p + 4;
        for (a, b) in [(0u16, 1u16), (2, 3)] {
            indices.extend_from_slice(&[p + a, p + b, q + a, q + a, p + b, q + b]);
        }
    }
    true
}

/// Build the ribbons of every live and draining trail as one renderable
/// mesh in system space (identity world), carrying the system's effect
/// material so the ordinary mesh path shades it. `None` when the system
/// has no strips or nothing long enough to draw.
pub fn strip_render_mesh(sim: &Simulation) -> Option<RenderMesh> {
    sim.desc.strips.as_ref()?;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for trail in sim.trails.iter().chain(sim.detached_trails.iter()) {
        if !append_trail(trail, &mut vertices, &mut indices) {
            break;
        }
    }
    if indices.is_empty() {
        return None;
    }
    let desc = &sim.desc;
    // The ribbons are psys-space geometry; the system's accumulated world
    // scale applies at render, exactly like the billboards.
    let world = Transform {
        rot: desc.world_rotation,
        t: desc.world_offset,
        s: desc.world_scale,
    };
    Some(RenderMesh {
        no_material: false,
        depth_write: true,
        // Generated geometry: no authored draw-order group.
        order_group: 0,
        // A ribbon is generated per frame from the simulation; no
        // block in any file produced it.
        block: None,
        name: format!("{} (trails)", desc.name),
        world: world.to_column_major(),
        node: None,
        local: Transform::IDENTITY,
        vertices: std::sync::Arc::new(vertices),
        indices: std::sync::Arc::new(indices),
        segments: Vec::new(),
        texture: desc.texture.clone(),
        tint: desc.tint,
        blend: if desc.additive {
            BlendMode::Additive
        } else {
            BlendMode::Alpha
        },
        alpha_test: None,
        double_sided: true,
        effect: true,
        refraction: false,
        marker: false,
        hidden: false,
        material: EffectMaterial {
            uv_offset: desc.uv_offset,
            uv_scale: desc.uv_scale,
            water: 0,
            greyscale_texture: desc.greyscale_texture.clone(),
            greyscale_color: desc.greyscale_color,
            greyscale_alpha: desc.greyscale_alpha,
            use_falloff: desc.use_falloff,
            falloff: desc.falloff,
            base_color: [desc.tint[0], desc.tint[1], desc.tint[2], desc.tint[3]],
            base_color_scale: desc.base_color_scale,
            soft_depth: desc.soft_depth,
        },
        lighting: crate::mesh::LightingMaterial::default(),
        shader_anims: Vec::new(),
        shader_color_anims: Vec::new(),
    })
}
