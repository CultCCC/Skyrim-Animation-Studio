//! Precision capsules in the viewport: sizes from the build's Precision
//! settings, placement on the posed skeleton, wireframe and blade trail.
//!
//! Sizing (Precision 2.0.4):
//! - `WEAPON` / `SHIELD` (exact case): length = max(reach or Length(),
//!   fMinWeaponLength) × fWeaponLengthMult × LengthMult × Scale, radius =
//!   (Radius() or fWeaponCapsuleRadius) × RadiusMult × Scale, along the
//!   node's +Y from its origin; WeaponTip keeps only the last (L − R).
//!   SHIELD has no model to measure and takes the minimum length.
//! - Other nodes: the bone's own capsule from skeleton.nif, its length
//!   scaled by fWeaponLengthMult × LengthMult × Scale (Length() replaces it);
//!   the radius stays the bone's. Bones without one get a small hand-sized
//!   capsule (approximation, marked in the list).
//! - As the player: × fPlayerAttackLengthMult / fPlayerAttackRadiusMult.
//! - Recoil capsule: fRecoilCollisionLength along the same axis.

use crate::app::StudioApp;
use crate::character::{to_transform, Capsule, Character};
use crate::render::LineVertex;
use aether_core::mesh::Transform;
use sds_anno::precision::{simulate, Collision, Result as SimResult, Settings};
use sds_vfs::Vfs;

pub const ATTACK: [f32; 4] = [1.0, 0.25, 0.2, 1.0];
pub const RECOIL: [f32; 4] = [0.3, 0.55, 1.0, 1.0];

/// Precision settings of the load order: the shipped MCM defaults, then
/// the user's MCM settings over them (the order the plugin reads them).
pub fn load_settings(vfs: &Vfs) -> (Settings, Vec<String>) {
    let mut s = Settings::default();
    let mut used = Vec::new();
    for p in ["mcm/config/precision/settings.ini", "mcm/settings/precision.ini"] {
        if let Ok(b) = vfs.read(p) {
            s.apply_ini(&String::from_utf8_lossy(&b));
            used.push(format!("{p} ({})", vfs.origin(p).unwrap_or_default()));
        }
    }
    (s, used)
}

/// The clip counts as a power attack when its path says so (as the
/// previewer decides; Precision itself asks the attack data).
pub fn is_power_attack(app: &StudioApp) -> bool {
    app.doc.as_ref().is_some_and(|d| match &d.source {
        crate::doc::ClipSource::Vfs(p) => p.contains("power"),
        crate::doc::ClipSource::Disk(p) => p.to_string_lossy().to_lowercase().contains("power"),
    })
}

pub fn simulate_doc(app: &StudioApp) -> Option<SimResult> {
    let doc = app.doc.as_ref()?;
    let settings = app.loaded.as_ref().map(|l| l.precision.0.clone()).unwrap_or_default();
    let items: Vec<(f32, &str)> = doc.annos.iter().map(|a| (a.time, a.text.as_str())).collect();
    Some(simulate(&items, doc.duration(), &settings, is_power_attack(app), app.scene.precision_player))
}

/// Capsule of one collision in its node's space, plus the recoil one.
pub struct Sized {
    pub attack: Capsule,
    pub recoil: Capsule,
    pub approx: bool,
}

pub fn size(c: &Collision, ch: &Character, s: &Settings, player: bool) -> Sized {
    let p = &c.params;
    let (lm, rm) = if player { (s.player_length_mult, s.player_radius_mult) } else { (1.0, 1.0) };
    let mut approx = false;
    let attack = if p.node == "WEAPON" || p.node == "SHIELD" {
        let reach = p.length.or(if p.node == "WEAPON" { ch.weapon_reach("WEAPON") } else { None }).unwrap_or(s.min_weapon_length);
        let k = s.weapon_length_mult * p.length_mult * p.scale * lm;
        let len = reach.max(s.min_weapon_length) * k;
        let r = p.radius.unwrap_or(s.weapon_capsule_radius) * p.radius_mult * p.scale * rm;
        let start = if p.weapon_tip { (len - r).max(0.0) } else { 0.0 };
        Capsule { a: [0.0, start, 0.0], b: [0.0, len, 0.0], r }
    } else {
        let base = ch.capsules.get(&p.node.to_ascii_lowercase()).copied().unwrap_or_else(|| {
            approx = true;
            Capsule { a: [0.0; 3], b: [8.0, 0.0, 0.0], r: 7.0 }
        });
        let d = [base.b[0] - base.a[0], base.b[1] - base.a[1], base.b[2] - base.a[2]];
        let l0 = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-4);
        let len = p.length.unwrap_or(l0 * s.weapon_length_mult) * p.length_mult * p.scale * lm;
        let f = len / l0;
        Capsule { a: base.a, b: [base.a[0] + d[0] * f, base.a[1] + d[1] * f, base.a[2] + d[2] * f], r: base.r }
    };
    let dir = {
        let d = [attack.b[0] - attack.a[0], attack.b[1] - attack.a[1], attack.b[2] - attack.a[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-4);
        [d[0] / l, d[1] / l, d[2] / l]
    };
    let recoil = Capsule { a: [0.0; 3], b: [dir[0] * s.recoil_length, dir[1] * s.recoil_length, dir[2] * s.recoil_length], r: attack.r };
    Sized { attack, recoil, approx }
}

/// Node frame of a collision: the node, then Translation(), then Rotation() (X, Y, Z).
pub fn frame(c: &Collision, ch: &Character, model: &[Transform], root: &Transform) -> Option<Transform> {
    let w = ch.node_world(model, root, &c.params.node)?;
    let p = &c.params;
    let r = p.rotation;
    let rot = Transform::rows_from_euler(r[0].to_radians(), r[1].to_radians(), r[2].to_radians());
    Some(w.compose(&Transform::from_av(p.translation, rot, 1.0)))
}

pub fn wire(out: &mut Vec<LineVertex>, w: &Transform, c: &Capsule, color: [f32; 4]) {
    let a = w.transform_point(c.a);
    let b = w.transform_point(c.b);
    let r = c.r * w.s;
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let n = if len > 1e-4 { [d[0] / len, d[1] / len, d[2] / len] } else { [0.0, 0.0, 1.0] };
    let helper = if n[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
    let cross = |x: [f32; 3], y: [f32; 3]| [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    let norm = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let u = norm(cross(n, helper));
    let v = cross(n, u);
    let at = |o: [f32; 3], x: [f32; 3], y: [f32; 3], ang: f32| {
        let (s, co) = ang.sin_cos();
        [o[0] + (x[0] * co + y[0] * s) * r, o[1] + (x[1] * co + y[1] * s) * r, o[2] + (x[2] * co + y[2] * s) * r]
    };
    let mut seg = |p: [f32; 3], q: [f32; 3]| {
        out.push(LineVertex { pos: p, color });
        out.push(LineVertex { pos: q, color });
    };
    const N: usize = 16;
    let step = std::f32::consts::TAU / N as f32;
    for k in 0..N {
        let (t0, t1) = (k as f32 * step, (k + 1) as f32 * step);
        seg(at(a, u, v, t0), at(a, u, v, t1));
        seg(at(b, u, v, t0), at(b, u, v, t1));
    }
    for k in 0..4 {
        let ang = k as f32 * std::f32::consts::FRAC_PI_2;
        seg(at(a, u, v, ang), at(b, u, v, ang));
    }
    // Hemispherical caps in two planes.
    let neg = [-n[0], -n[1], -n[2]];
    for side in [u, v] {
        for k in 0..N / 2 {
            let (t0, t1) = (k as f32 * step, (k + 1) as f32 * step);
            seg(at(b, side, n, t0), at(b, side, n, t1));
            seg(at(a, side, neg, t0), at(a, side, neg, t1));
        }
    }
}

/// Lines for every collision live at `t`, and the trail its blade tip has
/// swept since it started.
pub fn draw(app: &StudioApp, t: f32, root_at: &dyn Fn(f32) -> Transform, out: &mut Vec<LineVertex>) {
    if !app.scene.show_precision {
        return;
    }
    let (Some(ch), Some(sim)) = (&app.character, simulate_doc(app)) else { return };
    let settings = app.loaded.as_ref().map(|l| l.precision.0.clone()).unwrap_or_default();
    let model_at = |time: f32| -> Vec<Transform> {
        match &app.anim {
            Some(a) => {
                let local = sds_hkx::anim::local_pose(&ch.skeleton, a, time);
                sds_hkx::anim::model_pose(&ch.skeleton, &local).iter().map(to_transform).collect()
            }
            None => sds_hkx::anim::model_pose(&ch.skeleton, &ch.skeleton.reference).iter().map(to_transform).collect(),
        }
    };
    let model_now = model_at(t);
    let root_now = root_at(t);
    for c in &sim.collisions {
        if t < c.start || t >= c.end {
            continue;
        }
        let Some(w) = frame(c, ch, &model_now, &root_now) else { continue };
        let sz = size(c, ch, &settings, app.scene.precision_player);
        wire(out, &w, &sz.attack, ATTACK);
        if c.recoil_end.is_some_and(|r| t < r) {
            wire(out, &w, &sz.recoil, RECOIL);
        }
        // Trail of the far end since the collision began.
        let mut prev: Option<[f32; 3]> = None;
        let mut s = c.start;
        while s <= t {
            if let Some(ws) = frame(c, ch, &model_at(s), &root_at(s)) {
                let p = ws.transform_point(sz.attack.b);
                if let Some(q) = prev {
                    out.push(LineVertex { pos: q, color: [1.0, 0.45, 0.3, 0.55] });
                    out.push(LineVertex { pos: p, color: [1.0, 0.45, 0.3, 0.55] });
                }
                prev = Some(p);
            }
            s += 1.0 / 60.0;
        }
    }
}

/// Where a trail's look comes from, for the scene panel.
pub struct TrailLook {
    pub color: [f32; 4],
    pub lifetime: f32,
    pub mesh: String,
    pub source: String,
}

/// The trail look for one collision: its own Trail* parameters first, then
/// the toml definition matching the NPC's weapon, then the MCM defaults.
pub fn trail_look(app: &StudioApp, text: &str) -> TrailLook {
    let (ts, defs) = match &app.loaded {
        Some(l) => (l.trails.0.clone(), l.trails.1.clone()),
        None => (crate::fx::TrailSettings::default(), Vec::new()),
    };
    let mut look = TrailLook {
        color: ts.color,
        lifetime: ts.segment_lifetime,
        mesh: "meshes/effects/weapontrails/attacktrail.nif".into(),
        source: sds_i18n::t!("настройки MCM", "MCM settings").into(),
    };
    let mut scale = ts.color_scale_mult;
    // toml definition for the weapon in hand.
    if let (Some(n), Some(l)) = (&app.npc, &app.loaded) {
        if let Some(w) = n.weapon.and_then(|i| n.look.weapons.get(i)) {
            let facts = l.db.weapon_facts(w.key);
            if let Some(d) = crate::fx::pick_trail(&defs, &facts) {
                if let Some(c) = d.color {
                    look.color = c;
                }
                scale *= d.color_scale_mult.unwrap_or(1.0);
                look.lifetime *= d.lifetime_mult.unwrap_or(1.0);
                if let Some(m) = &d.mesh {
                    look.mesh = mesh_path(m);
                }
                look.source = sds_i18n::tf!("TrailDefinition из {}", "TrailDefinition from {}", d.source);
            }
        }
    }
    // The annotation's own overrides win and skip the toml lookup.
    if let sds_anno::Kind::Precision { params, .. } = sds_anno::parse(text).kind {
        let get = |k: &str| params.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
        let nums = |v: &str| -> Vec<f32> { v.split(|c: char| c == ',' || c.is_whitespace()).filter_map(|x| x.trim().parse().ok()).collect() };
        let mut own = false;
        if let Some(v) = get("TrailBaseColorOverride") {
            let c = nums(&v);
            if c.len() >= 3 {
                look.color = [c[0], c[1], c[2], c.get(3).copied().unwrap_or(1.0)];
                own = true;
            }
        }
        if let Some(v) = get("TrailBaseColorScaleMult").and_then(|v| v.trim().parse::<f32>().ok()) {
            scale = ts.color_scale_mult * v;
            own = true;
        }
        if let Some(v) = get("TrailLifetimeMult").and_then(|v| v.trim().parse::<f32>().ok()) {
            look.lifetime = ts.segment_lifetime * v;
            own = true;
        }
        if let Some(m) = get("TrailMeshOverride") {
            look.mesh = mesh_path(&m);
            own = true;
        }
        if own {
            look.source = sds_i18n::t!("параметры аннотации", "annotation parameters").into();
        }
    }
    look.color = [look.color[0] * scale, look.color[1] * scale, look.color[2] * scale, look.color[3]];
    look
}

fn mesh_path(m: &str) -> String {
    let m = m.replace('\\', "/");
    if m.to_ascii_lowercase().starts_with("meshes/") {
        m
    } else {
        format!("meshes/{m}")
    }
}

/// Precision weapon trails: a ribbon through the blade's base and tip over
/// the last trail lifetime, fading out over fTrailFadeOutTime.
pub fn trails(app: &StudioApp, player: &mut crate::fx::FxPlayer, t: f32, root_at: &dyn Fn(f32) -> Transform, out: &mut Vec<crate::render::FxBatch>) {
    if !app.scene.show_trails {
        return;
    }
    let (Some(ch), Some(sim), Some(doc)) = (&app.character, simulate_doc(app), &app.doc) else { return };
    let Some(loaded) = &app.loaded else { return };
    let ts = &loaded.trails.0;
    if !ts.enabled {
        return;
    }
    let settings = loaded.precision.0.clone();
    let model_at = |time: f32| -> Vec<Transform> {
        match &app.anim {
            Some(a) => {
                let local = sds_hkx::anim::local_pose(&ch.skeleton, a, time);
                sds_hkx::anim::model_pose(&ch.skeleton, &local).iter().map(to_transform).collect()
            }
            None => sds_hkx::anim::model_pose(&ch.skeleton, &ch.skeleton.reference).iter().map(to_transform).collect(),
        }
    };
    for c in &sim.collisions {
        if c.params.node != "WEAPON" && c.params.node != "SHIELD" {
            continue;
        }
        let text = doc.annos.get(c.add).map(|a| a.text.clone()).unwrap_or_default();
        if let sds_anno::Kind::Precision { params, .. } = sds_anno::parse(&text).kind {
            if params.iter().any(|(k, _)| k.eq_ignore_ascii_case("NoTrail")) {
                continue;
            }
        }
        let look = trail_look(app, &text);
        let span = look.lifetime + ts.fade_time;
        if t < c.start || t > c.end + span {
            continue;
        }
        let true_len = text.to_ascii_lowercase().contains("trailusetruelength") || ts.use_attack_length;
        let len = if true_len {
            let s = size(c, ch, &settings, app.scene.precision_player);
            s.attack.b[1]
        } else {
            ch.weapon_reach("WEAPON").unwrap_or(settings.min_weapon_length)
        };
        let step = 1.0 / ts.segments_per_second.max(10.0);
        let from = c.start.max(t - span);
        let to = t.min(c.end);
        let mut samples = Vec::new();
        let mut s = from;
        while s <= to + 1e-5 {
            if let Some(w) = frame(c, ch, &model_at(s), &root_at(s)) {
                let age = t - s;
                let alpha = if age <= look.lifetime { 1.0 } else { (1.0 - (age - look.lifetime) / ts.fade_time.max(1e-3)).max(0.0) };
                samples.push((w.transform_point([0.0, len * 0.1, 0.0]), w.transform_point([0.0, len, 0.0]), alpha));
            }
            s += step;
        }
        let vfs = loaded.vfs.clone();
        let asset = player.asset(&vfs, &look.mesh);
        let texture = asset.meshes.first().map(|m| m.texture.clone()).unwrap_or_default();
        crate::fx::ribbon(&samples, look.color, &texture, out);
    }
}
