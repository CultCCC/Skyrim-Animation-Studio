//! The collision shapes a file carries, as line segments in model space.
//!
//! NifSkope draws collision as a wireframe and so does this: a hull, a
//! box or a chunked mesh says what it is by its EDGES, and a solid one
//! would hide the model it belongs to. What comes out is a flat list of
//! world-space segments, ready for the viewport's line painter.
//!
//! **Scale.** Havok stores collision in its own units and the game
//! multiplies by 69.991249 to reach world units — measured in 1.5.97 at
//! `bhkRigidBody::sub_14029FE00` (0x14029FE34), the constant sitting at
//! 0x141536BA0. NifSkope spends the same factor in two places, 7.0 on
//! the body transform and 10.0 on the vertices, which is 70; the
//! difference is 0.0125% and invisible, but there is no reason to carry
//! an approximation when the real number is one xref away.

use crate::mesh::Transform;
use aether_nif::typed::geometry::NiTriStripsData;
use aether_nif::typed::havok::*;
use aether_nif::typed::{block_av, Ctx, TypedBlock};
use aether_nif::NifFile;

/// Havok units -> Bethesda world units (1.5.97 @ 0x14029FE34).
pub const HAVOK_SCALE: f32 = 69.991249;

/// One collision shape, already flattened to world-space line segments.
#[derive(Debug, Clone, Default)]
pub struct CollisionShape {
    /// The `bhkCollisionObject` this came from, so a click can name it.
    pub block: usize,
    /// The scene node it hangs off, for the Scene tree's benefit.
    pub node_block: usize,
    /// Pairs of endpoints in MODEL space.
    pub lines: Vec<([f32; 3], [f32; 3])>,
}

/// Every collision object in the file, drawn.
pub fn collision_from_nif(nif: &NifFile) -> Vec<CollisionShape> {
    let Ok(ctx) = Ctx::of(&nif.header) else {
        return Vec::new();
    };
    // A collision object names its own target node, so the walk is a
    // scan rather than a descent — and it finds the ones hanging off
    // nodes the mesh walk skips, which is most of a building.
    let mut out = Vec::new();
    for (i, block) in nif.blocks.iter().enumerate() {
        let Ok(obj) = BhkCollisionObject::decode(block, ctx) else {
            continue;
        };
        let Some(node) = obj.target.get() else { continue };
        let Some(body_index) = obj.body.get() else { continue };
        let Some(body_block) = nif.blocks.get(body_index) else {
            continue;
        };
        // A collision object's body is an entity OR a phantom. A
        // phantom is a trigger volume: no physics response, but real
        // geometry, and `bhkSPCollisionObject` is how the format hangs
        // one off a node. Skipping them left every bhkNiTriStripsShape
        // in the corpus invisible, since that is what they hold.
        let (shape, body_world) = if let Ok(body) = BhkRigidBody::decode(body_block, ctx) {
            // The body's own transform counts only for the T variant --
            // nif.xml says so and the game honours it the same way.
            let w = if body_block.type_name == "bhkRigidBodyT" {
                Transform {
                    rot: quat_rows(body.info.rotation),
                    t: [
                        body.info.translation[0] * HAVOK_SCALE,
                        body.info.translation[1] * HAVOK_SCALE,
                        body.info.translation[2] * HAVOK_SCALE,
                    ],
                    s: 1.0,
                }
            } else {
                Transform::IDENTITY
            };
            (body.world.shape, w)
        } else if let Ok(ph) = BhkSimpleShapePhantom::decode(body_block, ctx) {
            // Matrix44, column-major like every other matrix down here,
            // and its translation is in Havok units. Unlike a rigid
            // body there is no non-T variant: the phantom always
            // carries its own transform.
            let m = ph.transform;
            let w = Transform {
                rot: [m[0], m[4], m[8], m[1], m[5], m[9], m[2], m[6], m[10]],
                t: [
                    m[12] * HAVOK_SCALE,
                    m[13] * HAVOK_SCALE,
                    m[14] * HAVOK_SCALE,
                ],
                s: 1.0,
            };
            (ph.world.shape, w)
        } else {
            continue;
        };
        // ...and the node chain above it puts the whole thing in the
        // model. `block_worlds` walks the file, which is the only chain
        // a collision object is reachable through.
        let node_world = crate::derive::block_worlds(nif, ctx, node)
            .map(|(_, w)| w)
            .unwrap_or(Transform::IDENTITY);
        let mut lines = Vec::new();
        let world = node_world.compose(&body_world);
        walk_shape(nif, ctx, shape.get(), &world, 0, &mut lines);
        if lines.is_empty() {
            continue;
        }
        out.push(CollisionShape {
            block: i,
            node_block: node,
            lines,
        });
    }
    out
}

/// A quaternion `(x, y, z, w)` as row-major rotation rows.
fn quat_rows(q: [f32; 4]) -> [f32; 9] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let n = (x * x + y * y + z * z + w * w).sqrt();
    if n < 1e-9 {
        return Transform::IDENTITY.rot;
    }
    let (x, y, z, w) = (x / n, y / n, z / n, w / n);
    [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y - z * w),
        2.0 * (x * z + y * w),
        2.0 * (x * y + z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z - x * w),
        2.0 * (x * z - y * w),
        2.0 * (y * z + x * w),
        1.0 - 2.0 * (x * x + y * y),
    ]
}

fn put(lines: &mut Vec<([f32; 3], [f32; 3])>, w: &Transform, a: [f32; 3], b: [f32; 3]) {
    lines.push((w.transform_point(a), w.transform_point(b)));
}

fn tri(lines: &mut Vec<([f32; 3], [f32; 3])>, w: &Transform, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
    put(lines, w, a, b);
    put(lines, w, b, c);
    put(lines, w, c, a);
}

/// Descend the shape tree, emitting edges. `depth` bounds a file whose
/// shapes reference each other in a cycle.
fn walk_shape(
    nif: &NifFile,
    ctx: Ctx,
    shape: Option<usize>,
    world: &Transform,
    depth: usize,
    lines: &mut Vec<([f32; 3], [f32; 3])>,
) {
    if depth > 8 {
        return;
    }
    let Some(index) = shape else { return };
    let Some(block) = nif.blocks.get(index) else {
        return;
    };
    match block.type_name.as_str() {
        // The MOPP tree is an index, not a shape: walk through it.
        "bhkMoppBvTreeShape" => {
            if let Ok(s) = BhkMoppBvTreeShape::decode(block, ctx) {
                walk_shape(nif, ctx, s.shape.get(), world, depth + 1, lines);
            }
        }
        "bhkListShape" => {
            if let Ok(s) = BhkListShape::decode(block, ctx) {
                for sub in s.sub_shapes {
                    walk_shape(nif, ctx, sub.get(), world, depth + 1, lines);
                }
            }
        }
        "bhkConvexTransformShape" | "bhkTransformShape" => {
            if let Ok(s) = BhkTransformShape::decode(block, ctx) {
                // Column-major, and its translation is in Havok units
                // like everything else down here.
                let m = s.transform;
                let inner = Transform {
                    rot: [m[0], m[4], m[8], m[1], m[5], m[9], m[2], m[6], m[10]],
                    t: [
                        m[12] * HAVOK_SCALE,
                        m[13] * HAVOK_SCALE,
                        m[14] * HAVOK_SCALE,
                    ],
                    s: 1.0,
                };
                let next = world.compose(&inner);
                walk_shape(nif, ctx, s.shape.get(), &next, depth + 1, lines);
            }
        }
        "bhkBoxShape" => {
            if let Ok(s) = BhkBoxShape::decode(block, ctx) {
                let d = [
                    s.dimensions[0] * HAVOK_SCALE,
                    s.dimensions[1] * HAVOK_SCALE,
                    s.dimensions[2] * HAVOK_SCALE,
                ];
                box_edges(lines, world, d);
            }
        }
        "bhkSphereShape" => {
            if let Ok(s) = BhkSphereShape::decode(block, ctx) {
                sphere_edges(lines, world, [0.0; 3], s.radius * HAVOK_SCALE);
            }
        }
        "bhkCapsuleShape" => {
            if let Ok(s) = BhkCapsuleShape::decode(block, ctx) {
                let a = mul(s.first_point, HAVOK_SCALE);
                let b = mul(s.second_point, HAVOK_SCALE);
                sphere_edges(lines, world, a, s.radius_1 * HAVOK_SCALE);
                sphere_edges(lines, world, b, s.radius_2 * HAVOK_SCALE);
                put(lines, world, a, b);
            }
        }
        "bhkCylinderShape" => {
            if let Ok(s) = BhkCylinderShape::decode(block, ctx) {
                // The two vertices are Vector4; the fourth float is
                // Havok padding. `cylinder_radius` is the cylinder's
                // own -- `radius` is the convex shell every
                // bhkConvexShape carries and is not the silhouette.
                let a = mul([s.vertex_a[0], s.vertex_a[1], s.vertex_a[2]], HAVOK_SCALE);
                let b = mul([s.vertex_b[0], s.vertex_b[1], s.vertex_b[2]], HAVOK_SCALE);
                cylinder_edges(lines, world, a, b, s.cylinder_radius * HAVOK_SCALE);
            }
        }
        "bhkNiTriStripsShape" => {
            if let Ok(s) = BhkNiTriStripsShape::decode(block, ctx) {
                for r in &s.strips_data {
                    let Some(di) = r.get() else { continue };
                    let Some(db) = nif.blocks.get(di) else { continue };
                    let Ok(data) = NiTriStripsData::decode(db, ctx) else {
                        continue;
                    };
                    // Unlike every other bhk* shape, these vertices are
                    // already in Bethesda units: the block carries its
                    // own scale and NifSkope draws them unscaled.
                    let pts: Vec<[f32; 3]> = data
                        .vertices
                        .iter()
                        .map(|v| [v[0] * s.scale[0], v[1] * s.scale[1], v[2] * s.scale[2]])
                        .collect();
                    for t in data.unrolled_triangles() {
                        let (i, j, k) = (t[0] as usize, t[1] as usize, t[2] as usize);
                        if i.max(j).max(k) >= pts.len() {
                            continue;
                        }
                        tri(lines, world, pts[i], pts[j], pts[k]);
                    }
                }
            }
        }
        "bhkConvexVerticesShape" => {
            if let Ok(s) = BhkConvexVerticesShape::decode(block, ctx) {
                hull_edges(lines, world, &s.vertices);
            }
        }
        "bhkCompressedMeshShape" => {
            if let Ok(s) = BhkCompressedMeshShape::decode(block, ctx) {
                let Some(di) = s.data.get() else { return };
                let Some(db) = nif.blocks.get(di) else { return };
                if let Ok(data) = BhkCompressedMeshShapeData::decode(db, ctx) {
                    compressed_mesh_edges(lines, world, &data);
                }
            }
        }
        _ => {}
    }
}

fn mul(v: [f32; 3], k: f32) -> [f32; 3] {
    [v[0] * k, v[1] * k, v[2] * k]
}

fn box_edges(lines: &mut Vec<([f32; 3], [f32; 3])>, w: &Transform, d: [f32; 3]) {
    let c = |i: usize| {
        [
            if i & 1 == 0 { -d[0] } else { d[0] },
            if i & 2 == 0 { -d[1] } else { d[1] },
            if i & 4 == 0 { -d[2] } else { d[2] },
        ]
    };
    for (a, b) in [
        (0, 1), (2, 3), (4, 5), (6, 7),
        (0, 2), (1, 3), (4, 6), (5, 7),
        (0, 4), (1, 5), (2, 6), (3, 7),
    ] {
        put(lines, w, c(a), c(b));
    }
}

/// Three rings, which is how every tool of this class draws a sphere in
/// wireframe: enough to read the radius, cheap enough to draw hundreds.
fn sphere_edges(lines: &mut Vec<([f32; 3], [f32; 3])>, w: &Transform, at: [f32; 3], r: f32) {
    const STEPS: usize = 16;
    for axis in 0..3 {
        let mut prev = None;
        for i in 0..=STEPS {
            let t = i as f32 / STEPS as f32 * std::f32::consts::TAU;
            let (c, s) = (t.cos() * r, t.sin() * r);
            let p = match axis {
                0 => [at[0], at[1] + c, at[2] + s],
                1 => [at[0] + c, at[1], at[2] + s],
                _ => [at[0] + c, at[1] + s, at[2]],
            };
            if let Some(q) = prev {
                put(lines, w, q, p);
            }
            prev = Some(p);
        }
    }
}

/// A convex hull's own vertices, joined to their nearest neighbours.
///
/// The file stores the half-space PLANES, not the faces, so recovering
/// the exact silhouette means intersecting them — which NifSkope does
/// and which costs more than a wireframe is worth here. Joining each
/// vertex to the few nearest ones draws the same solid at a glance and
/// cannot produce a face that is not there.
/// A cylinder as two end rings joined by four rails. The ring plane is
/// perpendicular to the axis, so a degenerate axis (both centres equal)
/// draws nothing rather than dividing by zero.
fn cylinder_edges(
    lines: &mut Vec<([f32; 3], [f32; 3])>,
    w: &Transform,
    a: [f32; 3],
    b: [f32; 3],
    r: f32,
) {
    const STEPS: usize = 16;
    let axis = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if len < 1e-6 || r <= 0.0 {
        return;
    }
    let n = [axis[0] / len, axis[1] / len, axis[2] / len];
    // Any vector not parallel to the axis gives a valid first tangent.
    let seed = if n[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let cross = |p: [f32; 3], q: [f32; 3]| {
        [
            p[1] * q[2] - p[2] * q[1],
            p[2] * q[0] - p[0] * q[2],
            p[0] * q[1] - p[1] * q[0],
        ]
    };
    let u0 = cross(n, seed);
    let ul = (u0[0] * u0[0] + u0[1] * u0[1] + u0[2] * u0[2]).sqrt();
    let u = [u0[0] / ul, u0[1] / ul, u0[2] / ul];
    let v = cross(n, u);
    let at = |c: [f32; 3], t: f32| {
        let (co, si) = (t.cos() * r, t.sin() * r);
        [
            c[0] + u[0] * co + v[0] * si,
            c[1] + u[1] * co + v[1] * si,
            c[2] + u[2] * co + v[2] * si,
        ]
    };
    for (i, centre) in [a, b].iter().enumerate() {
        let mut prev = None;
        for k in 0..=STEPS {
            let t = k as f32 / STEPS as f32 * std::f32::consts::TAU;
            let p = at(*centre, t);
            if let Some(q) = prev {
                put(lines, w, q, p);
            }
            prev = Some(p);
            // Four rails, drawn once (from the first ring only).
            if i == 0 && k < STEPS && k % (STEPS / 4) == 0 {
                put(lines, w, p, at(b, t));
            }
        }
    }
}

fn hull_edges(lines: &mut Vec<([f32; 3], [f32; 3])>, w: &Transform, verts: &[[f32; 4]]) {
    let pts: Vec<[f32; 3]> = verts
        .iter()
        .map(|v| [v[0] * HAVOK_SCALE, v[1] * HAVOK_SCALE, v[2] * HAVOK_SCALE])
        .collect();
    if pts.len() < 2 || pts.len() > 512 {
        return;
    }
    for (i, a) in pts.iter().enumerate() {
        let mut near: Vec<(f32, usize)> = pts
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(j, b)| {
                let d = (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>();
                (d, j)
            })
            .collect();
        near.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
        for (_, j) in near.into_iter().take(3) {
            if j > i {
                put(lines, w, *a, pts[j]);
            }
        }
    }
}

/// The chunked mesh, decoded the way NifSkope decodes it
/// (`gltools.cpp::drawCMS`): big triangles are plain, and each chunk
/// carries quantised vertex offsets in Havok units x1000, relative to
/// its own translation plus the transform it names, with the transform's
/// ROTATION applied to the finished point.
fn compressed_mesh_edges(
    lines: &mut Vec<([f32; 3], [f32; 3])>,
    w: &Transform,
    data: &BhkCompressedMeshShapeData,
) {
    for t in &data.big_tris {
        let get = |i: u16| -> Option<[f32; 3]> {
            data.big_verts
                .get(i as usize)
                .map(|v| [v[0] * HAVOK_SCALE, v[1] * HAVOK_SCALE, v[2] * HAVOK_SCALE])
        };
        if let (Some(a), Some(b), Some(c)) =
            (get(t.triangle[0]), get(t.triangle[1]), get(t.triangle[2]))
        {
            tri(lines, w, a, b, c);
        }
    }
    for chunk in &data.chunks {
        let tf = data.chunk_transforms.get(chunk.transform_index as usize);
        let (ct, rot) = match tf {
            Some(t) => (t.translation, quat_rows(t.rotation)),
            None => ([0.0; 4], Transform::IDENTITY.rot),
        };
        let spin = Transform {
            rot,
            t: [0.0; 3],
            s: 1.0,
        };
        let here = w.compose(&spin);
        let verts: Vec<[f32; 3]> = chunk
            .vertices
            .chunks_exact(3)
            .map(|o| {
                [
                    (chunk.translation[0] + ct[0] + o[0] as f32 / 1000.0) * HAVOK_SCALE,
                    (chunk.translation[1] + ct[1] + o[1] as f32 / 1000.0) * HAVOK_SCALE,
                    (chunk.translation[2] + ct[2] + o[2] as f32 / 1000.0) * HAVOK_SCALE,
                ]
            })
            .collect();
        let at = |i: usize| chunk.indices.get(i).and_then(|k| verts.get(*k as usize)).copied();
        let mut offset = 0usize;
        for strip in &chunk.strips {
            let n = *strip as usize;
            for i in 0..n.saturating_sub(2) {
                if let (Some(a), Some(b), Some(c)) =
                    (at(offset + i), at(offset + i + 1), at(offset + i + 2))
                {
                    tri(lines, &here, a, b, c);
                }
            }
            offset += n;
        }
        let mut i = offset;
        while i + 2 < chunk.indices.len() {
            if let (Some(a), Some(b), Some(c)) = (at(i), at(i + 1), at(i + 2)) {
                tri(lines, &here, a, b, c);
            }
            i += 3;
        }
    }
}

/// Keeps `block_av` reachable for callers that want the target node's
/// own name; the walk itself does not need it.
#[allow(dead_code)]
fn _av(nif: &NifFile, ctx: Ctx, i: usize) -> Option<()> {
    block_av(nif.blocks.get(i)?, ctx).map(|_| ())
}
