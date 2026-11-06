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

/// How the game colours a shape at runtime (Lighting.hlsl, through Aether's
/// shader-type flags): baked into the texture by `load_texture`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tint {
    None,
    /// Type 5 skin: `b² + 2·t·b·(1−b)`, t = the actor's skin tone (NIF colour by default).
    Skin([f32; 3]),
    /// Type 6 hair: `b × t` (the vertex-green mask taken as full).
    Hair([f32; 3]),
    /// Type 4 FaceGen head: the skin curve with t = the NPC's facetint map.
    Face,
}

#[derive(Clone)]
pub struct SkinMesh {
    pub piece: usize,
    /// Diffuse path as the NIF names it; `texture` is the tinted key.
    pub base_texture: String,
    pub tint: Tint,
    /// Alpha-blended skin decal (scars, marks): drawn blended over the face.
    pub decal: bool,
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
    /// Weapons: Some(true) keeps only the scabbard (`Scb*` shapes or nodes),
    /// Some(false) drops it; None keeps everything.
    pub scabbard: Option<bool>,
    pub visible: bool,
}

#[derive(Clone)]
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
                    scabbard: None,
                    visible: true,
                });
            }
        }
        let nif = hkx.trim_end_matches(".hkx").to_string() + ".nif";
        let mut c = Character::build(vfs, &format!("folder:{folder}"), &hkx, Some(&nif), pieces, 1.0, 1.0)?;
        if folder != "character" {
            c.notes.push(sds_i18n::t!("Тело по умолчанию есть только у character; выберите NPC во вкладке «Персонаж»", "Only character has a default body; pick an NPC in the «Character» tab").into());
        }
        Ok(c)
    }

    /// `weight` is the NPC weight 0..1: `_0`/`_1` NIF pairs (body, hands,
    /// feet, armor) are blended by it, as the game does. FaceGen heads are
    /// baked for the NPC's weight, so the neck only meets the body when the
    /// body is blended the same way.
    pub fn build(vfs: &Vfs, id: &str, hkx: &str, nif: Option<&str>, pieces: Vec<Piece>, scale: f32, weight: f32) -> Result<Character, String> {
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
            match load_scene(vfs, &piece.path).map(|s| blend_weight(vfs, &piece.path, s, weight)) {
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
        c.apply_tints(None);
        if std::env::var_os("SDS_SHAPES").is_some() {
            for m in &c.meshes {
                eprintln!("SHAPE {} | {} | {} | {:?} | a{}", c.pieces[m.piece].path, m.shape, m.base_texture, m.tint, m.alpha_test);
            }
        }
        Ok(c)
    }

    /// Texture keys with the game's runtime tints baked in. `skin_tone`
    /// (the NPC's QNAM) replaces the NIF skin colour, as the engine does.
    pub fn apply_tints(&mut self, skin_tone: Option<[f32; 3]>) {
        let col = |c: [f32; 3]| format!("{:.4},{:.4},{:.4}", c[0], c[1], c[2]);
        for m in &mut self.meshes {
            if m.base_texture.is_empty() {
                continue;
            }
            let flat = |v: &str| std::env::var_os(v).is_some();
            if (flat("SDS_FLAT_FACE") && m.tint == Tint::Face) || (flat("SDS_FLAT_SKIN") && matches!(m.tint, Tint::Skin(_))) {
                m.texture = String::new();
                continue;
            }
            m.texture = match m.tint {
                Tint::None => m.base_texture.clone(),
                Tint::Skin(c) => format!("{}#skin={}", m.base_texture, col(skin_tone.unwrap_or(c))),
                Tint::Hair(c) => format!("{}#mul={}", m.base_texture, col(c)),
                Tint::Face => {
                    let p = self.pieces.get(m.piece).map(|p| p.path.to_ascii_lowercase().replace('\\', "/")).unwrap_or_default();
                    match p.find("facegendata/facegeom/") {
                        Some(i) if p.ends_with(".nif") => {
                            let rest = &p[i + "facegendata/facegeom/".len()..p.len() - 4];
                            let tone = skin_tone.map(|c| format!("#skin={}", col(c))).unwrap_or_default();
                            format!("{}#face=textures/actors/character/facegendata/facetint/{rest}.dds{tone}", m.base_texture)
                        }
                        // Generic race head: the body's tone.
                        _ => match skin_tone {
                            Some(c) => format!("{}#skin={}", m.base_texture, col(c)),
                            None => m.base_texture.clone(),
                        },
                    }
                }
            };
        }
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
                || std::env::var("SDS_HIDE").is_ok_and(|v| v.split(',').any(|h| !h.is_empty() && n.contains(&h.to_ascii_lowercase())))
        };
        // A weapon NIF's scabbard: a shape or an ancestor node named Scb*.
        let in_scb = |m: &aether_core::mesh::RenderMesh| {
            let scb = |n: &str| n.to_ascii_lowercase().starts_with("scb");
            let mut node = m.node;
            let mut found = scb(&m.name);
            while let (false, Some(n)) = (found, node) {
                found = scb(&scene.nodes[n].name);
                node = scene.nodes[n].parent;
            }
            found
        };
        if piece.scabbard == Some(true) && piece.attach.as_deref().and_then(|a| self.resolve(a)).is_none() {
            // No sheath node on this skeleton: leave the scabbard out.
            return;
        }
        let skip = |m: &aether_core::mesh::RenderMesh| m.hidden || m.marker || m.no_material || m.effect || hidden(&m.name) || piece.scabbard.is_some_and(|keep| in_scb(m) != keep);
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
                base_texture: m.texture.clone(),
                tint: tint_of(m),
                decal: m.blend == aether_core::mesh::BlendMode::Alpha && m.lighting.skin_tint,
                // Blended hair cards (brows, hair lines) have no depth sort here:
                // cut them at half alpha instead of drawing an opaque quad. Eyes
                // blend too, but their alpha is a specular mask: keep them opaque.
                alpha_test: m.alpha_test.unwrap_or(if m.blend == aether_core::mesh::BlendMode::Alpha && m.lighting.hair_tint { 0.5 } else { 0.0 }),
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
                base_texture: m.texture.clone(),
                tint: tint_of(m),
                decal: m.blend == aether_core::mesh::BlendMode::Alpha && m.lighting.skin_tint,
                // Blended hair cards (brows, hair lines) have no depth sort here:
                // cut them at half alpha instead of drawing an opaque quad. Eyes
                // blend too, but their alpha is a specular mask: keep them opaque.
                alpha_test: m.alpha_test.unwrap_or(if m.blend == aether_core::mesh::BlendMode::Alpha && m.lighting.hair_tint { 0.5 } else { 0.0 }),
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

fn tint_of(m: &aether_core::mesh::RenderMesh) -> Tint {
    let l = &m.lighting;
    if l.facegen {
        Tint::Face
    } else if l.skin_tint {
        Tint::Skin(l.tint_color)
    } else if l.hair_tint {
        Tint::Hair(l.tint_color)
    } else {
        Tint::None
    }
}

/// Decode a texture key of `Character::apply_tints`: `path[#op=..]...`.
/// `#skin=r,g,b` and `#face=<tint dds>` apply the game's skin curve
/// `b² + 2·t·b·(1−b)` (the face takes t per texel from its facetint map,
/// falling back to a `#skin` tone when the map is missing); `#mul=r,g,b`
/// multiplies (hair).
pub fn load_texture(vfs: &Vfs, key: &str) -> Option<(u32, u32, Vec<u8>)> {
    let mut parts = key.split('#');
    let path = parts.next()?;
    let read = |p: &str| {
        let bytes = vfs.read(p).ok().or_else(|| vfs.read(&format!("textures/{p}")).ok())?;
        let img = aether_core::dds::decode_capped(&bytes, 1024).ok()?;
        Some((img.width, img.height, img.rgba))
    };
    let (w, h, mut rgba) = read(path)?;
    let parse = |v: &str| {
        let c: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
        (c.len() == 3).then(|| [c[0], c[1], c[2]])
    };
    let curve = |b: f32, t: f32| (b * b + 2.0 * t * b * (1.0 - b)).clamp(0.0, 1.0);
    let ops: Vec<(&str, &str)> = parts.filter_map(|p| p.split_once('=')).collect();
    let face = ops.iter().find(|(k, _)| *k == "face").and_then(|(_, v)| read(v));
    for (k, v) in &ops {
        match *k {
            "face" => {
                let Some((tw, th, tint)) = &face else { continue };
                for y in 0..h {
                    let ty = (y as u64 * *th as u64 / h as u64) as u32;
                    for x in 0..w {
                        let tx = (x as u64 * *tw as u64 / w as u64) as u32;
                        let ti = ((ty * tw + tx) * 4) as usize;
                        let i = ((y * w + x) * 4) as usize;
                        for c in 0..3 {
                            let b = rgba[i + c] as f32 / 255.0;
                            rgba[i + c] = (curve(b, tint[ti + c] as f32 / 255.0) * 255.0 + 0.5) as u8;
                        }
                    }
                }
            }
            "skin" if !(face.is_some() && ops.iter().any(|(k, _)| *k == "face")) => {
                let Some(t) = parse(v) else { continue };
                for px in rgba.chunks_exact_mut(4) {
                    for c in 0..3 {
                        px[c] = (curve(px[c] as f32 / 255.0, t[c]) * 255.0 + 0.5) as u8;
                    }
                }
            }
            "mul" => {
                let Some(t) = parse(v) else { continue };
                for px in rgba.chunks_exact_mut(4) {
                    for c in 0..3 {
                        px[c] = (px[c] as f32 * t[c]).clamp(0.0, 255.0) as u8;
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(dir) = std::env::var_os("SDS_TEX_DUMP") {
        let name: String = key.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
        let name = &name[name.len().saturating_sub(120)..];
        if let Some(png) = aether_core::png::encode_rgba(w, h, &rgba) {
            let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), png);
        }
    }
    Some((w, h, rgba))
}

/// The game's weight morph: a `_0.nif`/`_1.nif` pair lerped by NPC weight
/// (0 = thin `_0`, 1 = heavy `_1`). Shapes are matched by index, name and
/// vertex count; anything that does not match keeps the loaded variant.
fn blend_weight(vfs: &Vfs, path: &str, scene: MeshScene, weight: f32) -> MeshScene {
    let lower = path.to_ascii_lowercase();
    let (thin, heavy) = if lower.ends_with("_1.nif") {
        (format!("{}_0.nif", &path[..path.len() - 6]), path.to_string())
    } else if lower.ends_with("_0.nif") {
        (path.to_string(), format!("{}_1.nif", &path[..path.len() - 6]))
    } else {
        return scene;
    };
    let w = weight.clamp(0.0, 1.0);
    let other_path = if lower.ends_with("_1.nif") { &thin } else { &heavy };
    let Ok(other) = load_scene(vfs, other_path) else { return scene };
    // `scene` is the loaded file; f = how far to move it towards `other`.
    let f = if lower.ends_with("_1.nif") { 1.0 - w } else { w };
    if f <= 0.0 {
        return scene;
    }
    let mut scene = scene;
    let lerp = |a: [f32; 3], b: [f32; 3]| [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f];
    let unit = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
        [v[0] / l, v[1] / l, v[2] / l]
    };
    for (i, m) in scene.meshes.iter_mut().enumerate() {
        let Some(o) = other.meshes.get(i).filter(|o| o.name == m.name && o.vertices.len() == m.vertices.len()) else { continue };
        let verts = std::sync::Arc::make_mut(&mut m.vertices);
        for (v, ov) in verts.iter_mut().zip(o.vertices.iter()) {
            v.position = lerp(v.position, ov.position);
            v.normal = unit(lerp(v.normal, ov.normal));
        }
    }
    for sk in scene.skinned.iter_mut() {
        let Some(o) = other.skinned.iter().find(|o| o.mesh == sk.mesh && o.raw.len() == sk.raw.len()) else { continue };
        if other.meshes.get(o.mesh).map(|m| &m.name) != scene.meshes.get(sk.mesh).map(|m| &m.name) {
            continue;
        }
        for (r, or) in sk.raw.iter_mut().zip(o.raw.iter()) {
            r.0 = lerp(r.0, or.0);
            r.1 = unit(lerp(r.1, or.1));
        }
    }
    scene
}
