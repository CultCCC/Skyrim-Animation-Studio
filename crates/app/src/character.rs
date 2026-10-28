//! The posed character in the viewport: a Havok skeleton, the matching
//! skeleton.nif for nodes Havok does not animate (WEAPON, SHIELD, holsters),
//! and a list of NIF pieces — body, FaceGen head, armor, a weapon in hand —
//! skinned on the CPU under the clip's pose.
//!
//! Every bone a mesh uses is reduced to "Havok bone × fixed offset": a Havok
//! bone itself, a skeleton.nif node under one, or a bone that only the
//! armor's own NIF has (cloth, hair chains) under the nearest one that
//! resolves. The offsets come from the bind poses, so such bones follow
//! their parent rigidly — what the game does without physics.

use crate::render::MeshVertex;
use aether_core::mesh::{meshes_from_nif, MeshScene, Transform};
use sds_hkx::anim::{Skeleton, Xform};
use sds_vfs::Vfs;
use std::collections::HashMap;
use std::sync::Arc;

/// World = model[havok] ∘ rel (or root ∘ rel when `havok` is None).
#[derive(Debug, Clone, Copy)]
pub struct BoneRef {
    pub havok: Option<usize>,
    pub rel: Transform,
}

pub struct SkinMesh {
    pub piece: usize,
    #[allow(dead_code)] // kept for diagnostics
    pub shape: String,
    pub texture: String,
    pub alpha_test: f32,
    pub indices: Arc<Vec<u16>>,
    pub uvs: Vec<[f32; 2]>,
    /// Skinned: (pos, normal, bone slots, weights) in skin space.
    pub raw: Vec<([f32; 3], [f32; 3], [u16; 4], [f32; 4])>,
    /// Per bone slot: where it is, and skin→bone.
    pub bones: Vec<(BoneRef, Transform)>,
    /// Rigid shapes: vertices in the attach bone's space.
    pub rigid: Option<(BoneRef, Vec<MeshVertex>)>,
}

/// One NIF to show.
#[derive(Debug, Clone)]
pub struct Piece {
    pub label: String,
    pub path: String,
    /// Rigid NIFs (weapons) hang from this node.
    pub attach: Option<String>,
    /// Shapes whose name contains one of these are not drawn (hair under a helmet).
    pub hide_shapes: Vec<String>,
    pub visible: bool,
}

pub struct Character {
    /// What this was built for (actor folder or NPC label).
    pub id: String,
    pub skeleton: Skeleton,
    pub skeleton_path: String,
    pub pieces: Vec<Piece>,
    pub meshes: Vec<SkinMesh>,
    pub notes: Vec<String>,
    /// skeleton.nif nodes Havok does not have, as offsets under Havok bones.
    extra: HashMap<String, BoneRef>,
    /// Applied to the whole actor (NPC height × race height).
    pub scale: f32,
    /// Collision capsules of skeleton.nif bones, in bone space (lowercase name).
    pub capsules: HashMap<String, Capsule>,
}

/// A capsule in some node's space: segment `a`–`b`, radius `r` (game units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capsule {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub r: f32,
}

/// `meshes/actors/<x>/...` → `x`.
pub fn actor_folder(clip_path: &str) -> Option<String> {
    let rest = clip_path.strip_prefix("meshes/actors/")?;
    Some(rest.split('/').next()?.to_string())
}

/// The skeleton.hkx for an actor folder: `character assets/skeleton.hkx`
/// first, any other `skeleton.hkx` under the folder after.
pub fn find_skeleton(vfs: &Vfs, folder: &str) -> Option<String> {
    let p = format!("meshes/actors/{folder}/character assets/skeleton.hkx");
    if vfs.exists(&p) {
        return Some(p);
    }
    let prefix = format!("meshes/actors/{folder}/");
    let mut all = vfs.paths(|p| p.starts_with(&prefix) && p.ends_with("/skeleton.hkx"));
    all.sort_by_key(|p| p.len());
    all.into_iter().next()
}

fn load_nif(vfs: &Vfs, path: &str) -> Result<aether_nif::NifFile, String> {
    let bytes = vfs.read(path).map_err(|e| e.to_string())?;
    aether_nif::NifFile::parse(&bytes).map_err(|e| format!("{e:?}"))
}

fn load_scene(vfs: &Vfs, path: &str) -> Result<MeshScene, String> {
    Ok(meshes_from_nif(&load_nif(vfs, path)?))
}

/// Havok units → game units (1.5.97 @ 0x14029FE34, as Aether measured).
const HAVOK_SCALE: f32 = 69.991249;

/// `bhkCapsuleShape`s hanging off a skeleton's bones (through a rigid body
/// and optional convex-transform shape), in the bone's own space.
fn skeleton_capsules(nif: &aether_nif::NifFile, scene: &MeshScene) -> HashMap<String, Capsule> {
    use aether_nif::typed::havok::{BhkBlendCollisionObject, BhkCapsuleShape, BhkCollisionObject, BhkRigidBody, BhkTransformShape};
    use aether_nif::typed::{Ctx, TypedBlock};
    let mut out = HashMap::new();
    let Ok(ctx) = Ctx::of(&nif.header) else { return out };
    for block in &nif.blocks {
        // Skeleton ragdoll bones carry bhkBlendCollisionObject.
        let obj = match BhkCollisionObject::decode(block, ctx) {
            Ok(o) => o,
            Err(_) => match BhkBlendCollisionObject::decode(block, ctx) {
                Ok(b) => b.base,
                Err(_) => continue,
            },
        };
        let (Some(node), Some(body_i)) = (obj.target.get(), obj.body.get()) else { continue };
        let Some(name) = scene.nodes.iter().find(|n| n.block == node).map(|n| n.name.to_ascii_lowercase()) else { continue };
        let Some(body_block) = nif.blocks.get(body_i) else { continue };
        let Ok(body) = BhkRigidBody::decode(body_block, ctx) else { continue };
        let mut xf = if body_block.type_name == "bhkRigidBodyT" {
            let q = body.info.rotation;
            let t = body.info.translation;
            Transform::from_av([t[0] * HAVOK_SCALE, t[1] * HAVOK_SCALE, t[2] * HAVOK_SCALE], Transform::rows_from_quat([q[3], q[0], q[1], q[2]]), 1.0)
        } else {
            Transform::IDENTITY
        };
        let mut shape = body.world.shape.get();
        for _ in 0..4 {
            let Some(si) = shape else { break };
            let sb = &nif.blocks[si];
            if let Ok(c) = BhkCapsuleShape::decode(sb, ctx) {
                let p = |v: [f32; 3]| xf.transform_point([v[0] * HAVOK_SCALE, v[1] * HAVOK_SCALE, v[2] * HAVOK_SCALE]);
                out.insert(name.clone(), Capsule { a: p(c.first_point), b: p(c.second_point), r: c.radius * HAVOK_SCALE });
                break;
            }
            if let Ok(ts) = BhkTransformShape::decode(sb, ctx) {
                let m = ts.transform;
                let t = Transform {
                    rot: [m[0], m[4], m[8], m[1], m[5], m[9], m[2], m[6], m[10]],
                    t: [m[12] * HAVOK_SCALE, m[13] * HAVOK_SCALE, m[14] * HAVOK_SCALE],
                    s: 1.0,
                };
                xf = xf.compose(&t);
                shape = ts.shape.get();
                continue;
            }
            break;
        }
    }
    out
}

impl Character {
    /// The default look of an actor folder: its skeleton, and for
    /// `character` the plain male body.
    pub fn for_folder(vfs: &Vfs, folder: &str) -> Result<Character, String> {
        let hkx = find_skeleton(vfs, folder).ok_or_else(|| sds_i18n::tf!("нет skeleton.hkx в meshes/actors/{folder}", "no skeleton.hkx in meshes/actors/{folder}"))?;
        let mut pieces = Vec::new();
        if folder == "character" {
            for part in ["malebody_1", "malehands_1", "malefeet_1", "malehead"] {
                pieces.push(Piece {
                    label: part.into(),
                    path: format!("meshes/actors/character/character assets/{part}.nif"),
                    attach: None,
                    hide_shapes: Vec::new(),
                    visible: true,
                });
            }
        }
        let nif = hkx.trim_end_matches(".hkx").to_string() + ".nif";
        let mut c = Character::build(vfs, &format!("folder:{folder}"), &hkx, Some(&nif), pieces, 1.0)?;
        if folder != "character" {
            c.notes.push(sds_i18n::t!("Тело по умолчанию есть только у character; выберите NPC во вкладке «Персонаж»", "Only character has a default body; pick an NPC in the «Character» tab").into());
        }
        Ok(c)
    }

    pub fn build(vfs: &Vfs, id: &str, hkx: &str, nif: Option<&str>, pieces: Vec<Piece>, scale: f32) -> Result<Character, String> {
        let bytes = vfs.read(hkx).map_err(|e| format!("{hkx}: {e}"))?;
        let skeleton = sds_hkx::anim::read_skeletons(&bytes).map_err(|e| e.to_string())?.remove(0);
        let mut notes = vec![sds_i18n::tf!("Скелет: {hkx} ({} костей)", "Skeleton: {hkx} ({} bones)", skeleton.bones.len())];
        let mut c = Character {
            id: id.to_string(),
            skeleton,
            skeleton_path: hkx.to_string(),
            pieces: Vec::new(),
            meshes: Vec::new(),
            notes: Vec::new(),
            extra: HashMap::new(),
            scale,
            capsules: HashMap::new(),
        };
        if let Some(nif) = nif.filter(|n| vfs.exists(n)) {
            match load_nif(vfs, nif) {
                Ok(file) => {
                    let scene = meshes_from_nif(&file);
                    c.add_skeleton_nif(&scene);
                    c.capsules = skeleton_capsules(&file, &scene);
                    notes.push(sds_i18n::tf!("Капсулы костей в {nif}: {}", "Bone capsules in {nif}: {}", c.capsules.len()));
                }
                Err(e) => notes.push(format!("{nif}: {e}")),
            }
        }
        for piece in pieces {
            let index = c.pieces.len();
            match load_scene(vfs, &piece.path) {
                Ok(scene) => {
                    let n = c.meshes.len();
                    c.add_piece(index, &scene, &piece);
                    if c.meshes.len() == n {
                        notes.push(sds_i18n::tf!("{}: нет видимых шейпов", "{}: no visible shapes", piece.path));
                    }
                }
                Err(e) => notes.push(format!("{}: {e}", piece.path)),
            }
            c.pieces.push(piece);
        }
        c.notes.append(&mut notes);
        Ok(c)
    }

    fn havok(&self, name: &str) -> Option<BoneRef> {
        self.skeleton.bone(name).map(|i| BoneRef { havok: Some(i), rel: Transform::IDENTITY })
    }

    fn resolve(&self, name: &str) -> Option<BoneRef> {
        self.havok(name).or_else(|| self.extra.get(&name.to_ascii_lowercase()).copied())
    }

    /// Nodes of skeleton.nif under Havok bones.
    fn add_skeleton_nif(&mut self, scene: &MeshScene) {
        let worlds = scene.node_worlds(&HashMap::new());
        for (i, node) in scene.nodes.iter().enumerate() {
            if self.havok(&node.name).is_some() {
                continue;
            }
            let mut p = node.parent;
            while let Some(pi) = p {
                if let Some(b) = self.havok(&scene.nodes[pi].name) {
                    let rel = worlds[pi].inverse().compose(&worlds[i]);
                    self.extra.insert(node.name.to_ascii_lowercase(), BoneRef { havok: b.havok, rel });
                    break;
                }
                p = scene.nodes[pi].parent;
            }
        }
    }

    /// A node of a piece's own NIF: by name, else under its nearest resolvable ancestor.
    fn resolve_in(&self, scene: &MeshScene, worlds: &[Transform], node: usize) -> Option<BoneRef> {
        if let Some(b) = self.resolve(&scene.nodes[node].name) {
            return Some(b);
        }
        let mut p = scene.nodes[node].parent;
        while let Some(pi) = p {
            if let Some(b) = self.resolve(&scene.nodes[pi].name) {
                let rel = worlds[pi].inverse().compose(&worlds[node]);
                return Some(BoneRef { havok: b.havok, rel: b.rel.compose(&rel) });
            }
            p = scene.nodes[pi].parent;
        }
        None
    }

    fn add_piece(&mut self, index: usize, scene: &MeshScene, piece: &Piece) {
        let worlds = scene.node_worlds(&HashMap::new());
        let hidden = |name: &str| {
            let n = name.to_ascii_lowercase();
            piece.hide_shapes.iter().any(|h| n.contains(h.as_str()))
        };
        let skip = |m: &aether_core::mesh::RenderMesh| m.hidden || m.marker || m.no_material || m.effect || hidden(&m.name);
        let skinned: std::collections::HashSet<usize> = scene.skinned.iter().map(|s| s.mesh).collect();
        for sk in &scene.skinned {
            let m = &scene.meshes[sk.mesh];
            if skip(m) {
                continue;
            }
            let bones = sk
                .bones
                .iter()
                .map(|b| {
                    let r = b.node.and_then(|n| self.resolve_in(scene, &worlds, n)).unwrap_or(BoneRef { havok: None, rel: b.bind_world });
                    (r, b.skin_to_bone)
                })
                .collect();
            self.meshes.push(SkinMesh {
                piece: index,
                shape: m.name.clone(),
                texture: m.texture.clone(),
                alpha_test: m.alpha_test.unwrap_or(0.0),
                indices: m.indices.clone(),
                uvs: m.vertices.iter().map(|v| v.uv).collect(),
                raw: sk.raw.clone(),
                bones,
                rigid: None,
            });
        }
        // Rigid shapes: under the piece's attach node, else where they stand.
        let attach = piece.attach.as_deref().and_then(|a| self.resolve(a));
        for (i, m) in scene.meshes.iter().enumerate() {
            if skinned.contains(&i) || skip(m) {
                continue;
            }
            let w = Transform::from_column_major(&m.world);
            let verts = m
                .vertices
                .iter()
                .map(|v| MeshVertex { pos: w.transform_point(v.position), normal: norm(w.rotate(v.normal)), uv: v.uv })
                .collect();
            let at = attach.or_else(|| m.node.and_then(|n| self.resolve_in(scene, &worlds, n))).unwrap_or(BoneRef { havok: None, rel: Transform::IDENTITY });
            // Without an attach node the shape's own world already includes
            // its node; express it relative to that bone instead.
            let verts = if attach.is_none() && at.havok.is_some() {
                let back = m.node.map(|n| worlds[n].inverse()).unwrap_or(Transform::IDENTITY);
                let rel_back = back;
                verts_map(verts, &rel_back)
            } else {
                verts
            };
            self.meshes.push(SkinMesh {
                piece: index,
                shape: m.name.clone(),
                texture: m.texture.clone(),
                alpha_test: m.alpha_test.unwrap_or(0.0),
                indices: m.indices.clone(),
                uvs: Vec::new(),
                raw: Vec::new(),
                bones: Vec::new(),
                rigid: Some((at, verts)),
            });
        }
    }

    /// World transform of a bone reference under a pose.
    pub fn world(model: &[Transform], root: &Transform, b: &BoneRef) -> Transform {
        match b.havok.and_then(|i| model.get(i)) {
            Some(m) => root.compose(m).compose(&b.rel),
            None => root.compose(&b.rel),
        }
    }

    /// Precision's weapon reach: the merged bounding sphere of the weapon
    /// model, radius plus the centre's distance along the blade (+Y of the
    /// WEAPON node). The sphere here is the vertex AABB's centre and the
    /// farthest vertex from it, a close stand-in for the NIF bounds.
    pub fn weapon_reach(&self, node: &str) -> Option<f32> {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        let mut pts = Vec::new();
        for m in &self.meshes {
            let Some(p) = self.pieces.get(m.piece) else { continue };
            if !p.visible || p.attach.as_deref() != Some(node) {
                continue;
            }
            if let Some((_, verts)) = &m.rigid {
                for v in verts {
                    for k in 0..3 {
                        lo[k] = lo[k].min(v.pos[k]);
                        hi[k] = hi[k].max(v.pos[k]);
                    }
                    pts.push(v.pos);
                }
            }
        }
        if pts.is_empty() {
            return None;
        }
        let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
        let r = pts.iter().map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt()).fold(0.0, f32::max);
        Some(r + c[1])
    }

    /// The weapon in hand as emitter geometry, in the WEAPON node's space.
    pub fn weapon_geometry(&self) -> Option<crate::fx::TargetGeometry> {
        let mut g = crate::fx::TargetGeometry { vertices: Vec::new(), triangles: Vec::new() };
        for m in &self.meshes {
            let Some(p) = self.pieces.get(m.piece) else { continue };
            if p.attach.as_deref() != Some("WEAPON") {
                continue;
            }
            let Some((_, verts)) = &m.rigid else { continue };
            let base = g.vertices.len() as u32;
            g.vertices.extend(verts.iter().map(|v| (v.pos, v.normal)));
            g.triangles.extend(m.indices.chunks_exact(3).map(|t| [base + t[0] as u32, base + t[1] as u32, base + t[2] as u32]));
        }
        (!g.triangles.is_empty()).then_some(g)
    }

    /// Node world by name (for markers), under a pose.
    pub fn node_world(&self, model: &[Transform], root: &Transform, name: &str) -> Option<Transform> {
        self.resolve(name).map(|b| Self::world(model, root, &b))
    }
}

fn verts_map(v: Vec<MeshVertex>, t: &Transform) -> Vec<MeshVertex> {
    v.into_iter().map(|x| MeshVertex { pos: t.transform_point(x.pos), normal: norm(t.rotate(x.normal)), uv: x.uv }).collect()
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    [v[0] / l, v[1] / l, v[2] / l]
}

/// Havok model-space transform as an Aether `Transform`.
pub fn to_transform(x: &Xform) -> Transform {
    let q = [x.r[3], x.r[0], x.r[1], x.r[2]]; // wxyz
    Transform::from_av(x.t, Transform::rows_from_quat(q), x.s[0])
}

/// Skin one mesh under `model` (Havok model-space bones), placed by `root`.
pub fn skin(mesh: &SkinMesh, model: &[Transform], root: &Transform) -> Vec<MeshVertex> {
    if let Some((at, verts)) = &mesh.rigid {
        let w = Character::world(model, root, at);
        return verts.iter().map(|v| MeshVertex { pos: w.transform_point(v.pos), normal: norm(w.rotate(v.normal)), uv: v.uv }).collect();
    }
    let mats: Vec<Transform> = mesh.bones.iter().map(|(b, skin_to_bone)| Character::world(model, root, b).compose(skin_to_bone)).collect();
    mesh.raw
        .iter()
        .zip(&mesh.uvs)
        .map(|((pos, nrm, bi, wts), uv)| {
            let mut p = [0.0f32; 3];
            let mut n = [0.0f32; 3];
            let mut total = 0.0;
            for k in 0..4 {
                let w = wts[k];
                if w <= 0.0 {
                    continue;
                }
                let Some(m) = mats.get(bi[k] as usize) else { continue };
                let tp = m.transform_point(*pos);
                let tn = m.rotate(*nrm);
                for a in 0..3 {
                    p[a] += tp[a] * w;
                    n[a] += tn[a] * w;
                }
                total += w;
            }
            if total <= 0.0 {
                p = root.transform_point(*pos);
                n = *nrm;
            } else if (total - 1.0).abs() > 1e-3 {
                for a in 0..3 {
                    p[a] /= total;
                }
            }
            MeshVertex { pos: p, normal: norm(n), uv: *uv }
        })
        .collect()
}
