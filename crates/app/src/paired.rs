//! Paired clips (`paired_*.hkx`): one file animates two actors. Tracks are
//! `PairedRoot`, then per actor a group node and a full skeleton (`NPC …`
//! for the actor playing the idle, `2_NPC …` for the partner). The studio
//! shows the partner as a copy of the current character.

use crate::app::StudioApp;
use crate::character::{skin, to_transform, Character};
use crate::render::{DrawMesh, MeshVertex};
use aether_core::mesh::Transform;
use std::sync::Arc;

/// Keep `app.partner` in step: a copy of the character while a paired clip
/// with two actors is open, nothing otherwise.
pub fn sync(app: &mut StudioApp) {
    let wanted = app.anim.as_ref().is_some_and(|a| a.paired().len() >= 2) && app.character.is_some();
    if !wanted {
        app.partner = None;
        return;
    }
    if app.partner.as_ref().is_none_or(|(g, _)| *g != app.character_gen) {
        let c: Character = app.character.clone().unwrap();
        app.partner = Some((app.character_gen, c));
    }
}

/// A skinned shape ready to draw.
pub struct Skinned {
    pub id: u64,
    pub verts: Vec<MeshVertex>,
    pub indices: Arc<Vec<u16>>,
    pub texture: String,
    pub alpha_test: f32,
    pub double_sided: bool,
    pub blend: bool,
    /// Skin or face (effect shaders flagged «affect skin only»).
    pub skin: bool,
}

impl Skinned {
    pub fn draw(&self) -> DrawMesh<'_> {
        DrawMesh { id: self.id, vertices: &self.verts, indices: &self.indices, texture: &self.texture, alpha_test: self.alpha_test, tint: [1.0; 4], double_sided: self.double_sided, blend: self.blend }
    }
}

fn skin_character(c: &Character, model: &[Transform], root: &Transform, id_base: u64, out: &mut Vec<Skinned>) {
    for (i, mesh) in c.meshes.iter().enumerate() {
        if !c.pieces.get(mesh.piece).is_some_and(|p| p.visible) {
            continue;
        }
        let verts = skin(mesh, model, root);
        if verts.is_empty() {
            continue;
        }
        out.push(Skinned { id: id_base + i as u64, verts, indices: mesh.indices.clone(), texture: mesh.texture.clone(), alpha_test: mesh.alpha_test, double_sided: matches!(mesh.tint, crate::character::Tint::Skin(_) | crate::character::Tint::Face) && !mesh.decal, blend: mesh.decal, skin: matches!(mesh.tint, crate::character::Tint::Skin(_) | crate::character::Tint::Face) });
    }
}

/// Every actor of the clip at `t`, skinned under `root`: the character, and
/// for a paired clip the partner, each placed by its group track.
pub fn skin_all(app: &StudioApp, t: f32, root: &Transform) -> Vec<Skinned> {
    let mut out = Vec::new();
    let Some(c) = &app.character else { return out };
    let gen = app.character_gen * 10_000;
    let actors = app.anim.as_ref().map(|a| a.paired()).unwrap_or_default();
    match (&app.anim, actors.len() >= 2) {
        (Some(a), true) => {
            for (k, actor) in actors.iter().take(2).enumerate() {
                let who = if k == 0 { Some(c) } else { app.partner.as_ref().map(|(_, p)| p) };
                let Some(ch) = who else { continue };
                let local = sds_hkx::anim::local_pose_actor(&ch.skeleton, a, t, actor);
                let model: Vec<Transform> = sds_hkx::anim::model_pose(&ch.skeleton, &local).iter().map(to_transform).collect();
                let place = root.compose(&to_transform(&sds_hkx::anim::actor_root(a, t, actor)));
                skin_character(ch, &model, &place, gen + k as u64 * 5_000, &mut out);
            }
        }
        _ => {
            let local = match &app.anim {
                Some(a) => sds_hkx::anim::local_pose(&c.skeleton, a, t),
                None => c.skeleton.reference.clone(),
            };
            let model: Vec<Transform> = sds_hkx::anim::model_pose(&c.skeleton, &local).iter().map(to_transform).collect();
            skin_character(c, &model, root, gen, &mut out);
        }
    }
    out
}
