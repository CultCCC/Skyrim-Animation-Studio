//! The 3D render window: posed character, skeleton, grid and root-motion
//! path, orbit camera.

use crate::app::StudioApp;
use crate::character::to_transform;
use crate::render::{look_at, mat_mul, perspective, project, DrawMesh, Frame, LineVertex};
use crate::theme::category_color;
use aether_core::mesh::Transform;
use egui::{pos2, vec2, Align2, Color32, FontId, Sense};
use sds_anno::{parse, Category};

fn line(out: &mut Vec<LineVertex>, a: [f32; 3], b: [f32; 3], c: [f32; 4]) {
    out.push(LineVertex { pos: a, color: c });
    out.push(LineVertex { pos: b, color: c });
}

fn rgba(c: Color32, a: f32) -> [f32; 4] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, a]
}

/// Vertical capsule wire (the target dummy).
fn dummy(out: &mut Vec<LineVertex>, at: [f32; 2], r: f32, h: f32, c: [f32; 4]) {
    let n = 20;
    for k in 0..n {
        let (a0, a1) = (k as f32 / n as f32 * std::f32::consts::TAU, (k + 1) as f32 / n as f32 * std::f32::consts::TAU);
        for z in [r, h - r, 0.5] {
            let rr = if z == 0.5 { r * 0.6 } else { r };
            line(out, [at[0] + rr * a0.cos(), at[1] + rr * a0.sin(), z], [at[0] + rr * a1.cos(), at[1] + rr * a1.sin(), z], c);
        }
    }
    for k in 0..4 {
        let a = k as f32 * std::f32::consts::FRAC_PI_2;
        line(out, [at[0] + r * a.cos(), at[1] + r * a.sin(), r], [at[0] + r * a.cos(), at[1] + r * a.sin(), h - r], c);
    }
    line(out, [at[0], at[1], h - r], [at[0], at[1], h], c);
}

fn ring(out: &mut Vec<LineVertex>, at: [f32; 2], r: f32, z: f32, c: [f32; 4]) {
    let n = 48;
    for k in 0..n {
        let (a0, a1) = (k as f32 / n as f32 * std::f32::consts::TAU, (k + 1) as f32 / n as f32 * std::f32::consts::TAU);
        line(out, [at[0] + r * a0.cos(), at[1] + r * a0.sin(), z], [at[0] + r * a1.cos(), at[1] + r * a1.sin(), z], c);
    }
}

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui) {
    crate::paired::sync(app);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut app.scene.view3d, true, "3D");
        ui.selectable_value(&mut app.scene.view3d, false, sds_i18n::t!("Сверху 2D", "Top 2D"));
        ui.separator();
        ui.checkbox(&mut app.scene.show_body, sds_i18n::t!("Тело", "Body"));
        ui.checkbox(&mut app.scene.show_skeleton, sds_i18n::t!("Скелет", "Skeleton"));
        ui.checkbox(&mut app.scene.root_motion, "Root motion");
        if ui.add_enabled(app.doc.is_some(), egui::Button::new(sds_i18n::t!("Редактор…", "Editor…"))).on_hover_text(sds_i18n::t!("Редактор рут-моушена: кривые X/Y", "Root Motion Editor: X/Y curves")).clicked() {
            app.rm.open = true;
        }
        ui.checkbox(&mut app.scene.follow, sds_i18n::t!("Камера за NPC", "Camera follows NPC"));
        if ui.button(sds_i18n::t!("Сброс камеры", "Reset camera")).clicked() {
            app.camera.yaw = 25f32.to_radians();
            app.camera.pitch = 12f32.to_radians();
            app.camera.dist = 320.0;
        }
        ui.menu_button("🖱", |ui| {
            ui.label(sds_i18n::t!("Вращение камеры (ЛКМ)", "Camera orbit (LMB)"));
            ui.checkbox(&mut app.settings.invert_x, sds_i18n::t!("Инвертировать по горизонтали", "Invert horizontal"));
            ui.checkbox(&mut app.settings.invert_y, sds_i18n::t!("Инвертировать по вертикали", "Invert vertical"));
        })
        .response
        .on_hover_text(sds_i18n::t!("Инверсия управления камерой", "Camera control inversion"));
        ui.separator();
        crate::export::toolbar(app, ui);
    });
    if !app.scene.view3d || app.gpu.is_none() {
        crate::panels::viewport_2d(app, ui);
        return;
    }

    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    // Camera input.
    let (ix, iy) = (if app.settings.invert_x { -1.0 } else { 1.0 }, if app.settings.invert_y { -1.0 } else { 1.0 });
    let cam = &mut app.camera;
    if resp.dragged_by(egui::PointerButton::Primary) {
        let d = resp.drag_delta();
        cam.yaw -= d.x * 0.01 * ix;
        cam.pitch = (cam.pitch + d.y * 0.01 * iy).clamp(-1.4, 1.4);
    }
    let (fwd, right) = {
        let (s, c) = cam.yaw.sin_cos();
        ([s, c, 0.0f32], [c, -s, 0.0f32])
    };
    if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) {
        let d = resp.drag_delta() * cam.dist * 0.0025;
        app.scene.follow = false;
        for k in 0..3 {
            cam.target[k] -= right[k] * d.x;
        }
        cam.target[2] += d.y;
    }
    if resp.hovered() {
        let (s, z) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        cam.dist = (cam.dist * (-s * 0.002).exp() / z).clamp(20.0, 20000.0);
    }
    let _ = fwd;

    let t = app.play.time;
    // The path is simulated once per frame; everything below samples it.
    let path = crate::sds_path::compute(app, false);
    let raw_path = if app.scene.compare_raw && app.sdscombat_active() { crate::sds_path::compute(app, true) } else { None };
    let (npc_k, rm) = (crate::sds_path::npc_scale(app), app.scene.root_motion);
    let root_at = |time: f32| crate::sds_path::root(npc_k, rm, path.as_ref(), time);
    let root = root_at(t);
    if app.scene.follow {
        app.camera.target = [root.t[0], root.t[1], root.t[2] + 80.0 * root.s];
    }
    let cam = &app.camera;
    let eye = [
        cam.target[0] + cam.dist * cam.pitch.cos() * cam.yaw.sin(),
        cam.target[1] + cam.dist * cam.pitch.cos() * cam.yaw.cos(),
        cam.target[2] + cam.dist * cam.pitch.sin(),
    ];
    let ppp = ui.ctx().pixels_per_point();
    let size_px = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
    let aspect = rect.width().max(1.0) / rect.height().max(1.0);
    let view_proj = mat_mul(&perspective(45f32.to_radians(), aspect, 2.0, 50000.0), &look_at(eye, cam.target, [0.0, 0.0, 1.0]));

    // Pose.
    let model: Option<Vec<Transform>> = match (&app.character, &app.anim) {
        (Some(c), Some(a)) => {
            let local = sds_hkx::anim::local_pose(&c.skeleton, a, t);
            Some(sds_hkx::anim::model_pose(&c.skeleton, &local).iter().map(to_transform).collect())
        }
        (Some(c), None) => {
            Some(sds_hkx::anim::model_pose(&c.skeleton, &c.skeleton.reference).iter().map(to_transform).collect())
        }
        _ => None,
    };

    let skinned = if app.scene.show_body { crate::paired::skin_all(app, t, &root) } else { Vec::new() };
    let mut lines = Vec::new();
    let mut overlay = Vec::new();
    if app.scene.show_grid {
        let n = 16;
        let step = 64.0;
        let ext = n as f32 * step;
        for k in -n..=n {
            let v = k as f32 * step;
            let a = if k == 0 { 0.35 } else if k % 4 == 0 { 0.18 } else { 0.08 };
            line(&mut lines, [v, -ext, 0.0], [v, ext, 0.0], [1.0, 1.0, 1.0, a]);
            line(&mut lines, [-ext, v, 0.0], [ext, v, 0.0], [1.0, 1.0, 1.0, a]);
        }
        line(&mut lines, [0.0, 0.0, 0.1], [0.0, 60.0, 0.1], [0.3, 0.9, 0.3, 0.9]);
    }
    if let Some(doc) = &app.doc {
        if app.scene.show_path && app.scene.root_motion && !doc.motion.moves.is_empty() {
            let dur = doc.duration();
            let n = 200;
            let pc = rgba(category_color(Category::Motion), 0.9);
            let mut prev = root_at(0.0).t;
            for k in 1..=n {
                let q = root_at(dur * k as f32 / n as f32).t;
                line(&mut overlay, [prev[0], prev[1], 1.0], [q[0], q[1], 1.0], pc);
                prev = q;
            }
            // The same clip without SDSCombat, for comparison.
            if let Some(rp) = &raw_path {
                let mut prev = rp.at(0.0).0;
                for k in 1..=n {
                    let q = rp.at(dur * k as f32 / n as f32).0;
                    line(&mut overlay, [prev[0], prev[1], 0.6], [q[0], q[1], 0.6], [0.75, 0.75, 0.75, 0.8]);
                    prev = q;
                }
            }
            for a in &doc.annos {
                let p = parse(&a.text);
                if matches!(p.category, Category::Motion | Category::Foot | Category::Other) || app.scene.hidden.contains(&p.category) {
                    continue;
                }
                if p.category == Category::SdsCombat && !app.sdscombat_active() {
                    continue;
                }
                let q = root_at(a.time).t;
                line(&mut overlay, [q[0], q[1], 1.0], [q[0], q[1], 14.0], rgba(category_color(p.category), 1.0));
            }
        }
    }
    if let (Some(c), Some(m)) = (&app.character, &model) {
        if app.scene.show_skeleton || !app.scene.show_body || c.meshes.is_empty() {
            for (i, &p) in c.skeleton.parents.iter().enumerate() {
                if p < 0 || i >= m.len() {
                    continue;
                }
                let a = root.transform_point(m[p as usize].t);
                let b = root.transform_point(m[i].t);
                line(&mut overlay, a, b, [1.0, 0.85, 0.25, 1.0]);
            }
        }
    }

    // Target dummy, the line to it, ReachTarget arrive rings, and the NPC's
    // heading coloured by the lock state.
    let target = crate::sds_path::target_pos(app);
    let state = path.as_ref().and_then(|p| p.state(t).copied());
    if app.scene.show_target && app.doc.is_some() {
        dummy(&mut lines, target, 30.0, 128.0, [0.35, 0.95, 0.45, 0.9]);
        line(&mut lines, [root.t[0], root.t[1], 2.0], [target[0], target[1], 2.0], [0.35, 0.95, 0.45, 0.25]);
        if let Some(doc) = &app.doc {
            if app.sdscombat_active() {
                for a in &doc.annos {
                    if let sds_anno::Kind::SdsNpc { event, values } = parse(&a.text).kind {
                        if event.eq_ignore_ascii_case("SDS_NPC_ReachTarget") {
                            if let Some(arrive) = values.get(3) {
                                ring(&mut lines, target, *arrive, 1.5, [1.0, 0.65, 0.2, 0.7]);
                            }
                        }
                    }
                }
            }
        }
    }
    {
        let (hc, len) = match state {
            Some(s) if s.stop => ([1.0, 0.3, 0.3, 1.0], 70.0),
            Some(s) if s.lock.is_some() => ([0.3, 0.9, 1.0, 1.0], 90.0),
            _ => ([1.0, 0.85, 0.3, 1.0], 60.0),
        };
        let f = root.rotate([0.0, len, 0.0]);
        line(&mut overlay, [root.t[0], root.t[1], 3.0], [root.t[0] + f[0], root.t[1] + f[1], 3.0], hc);
    }

    // SCAR_ActionData sectors on the ground around the NPC (its current pose).
    if app.scene.show_scar {
        let (_, _, td, ta) = crate::scar_panel::target_local(app, t);
        for (_, _, a) in crate::scar_panel::actions(app) {
            let active = a.contains(td, ta) && a.in_window(t);
            let c = crate::scar_panel::type_color(&a.ty);
            let col = rgba(c, if active { 1.0 } else { 0.45 });
            let z = if active { 3.0 } else { 1.2 };
            let (s, mut e) = (a.start_angle, a.end_angle);
            if e < s {
                e += 360.0;
            }
            e = e.min(s + 360.0);
            let n = (((e - s) / 6.0).ceil() as usize).clamp(2, 64);
            let pt = |d: f32, ang: f32| {
                let r = ang.to_radians();
                let local = root.rotate([d * r.sin() / root.s, d * r.cos() / root.s, 0.0]);
                [root.t[0] + local[0] * root.s, root.t[1] + local[1] * root.s, z]
            };
            for k in 0..n {
                let a0 = s + (e - s) * k as f32 / n as f32;
                let a1 = s + (e - s) * (k + 1) as f32 / n as f32;
                line(&mut overlay, pt(a.max_distance, a0), pt(a.max_distance, a1), col);
                if a.min_distance > 0.0 {
                    line(&mut overlay, pt(a.min_distance, a0), pt(a.min_distance, a1), col);
                }
            }
            if e - s < 359.9 {
                line(&mut overlay, pt(a.min_distance, s), pt(a.max_distance, s), col);
                line(&mut overlay, pt(a.min_distance, e), pt(a.max_distance, e), col);
            }
        }
    }

    crate::precision_view::draw(app, t, &root_at, &mut overlay);

    let meshes: Vec<DrawMesh> = skinned.iter().map(|s| s.draw()).collect();
    // Key light over the camera's shoulder, so the side we look at is lit.
    let to_eye = [eye[0] - cam.target[0], eye[1] - cam.target[1], eye[2] - cam.target[2]];
    let l = (to_eye[0].hypot(to_eye[1])).max(1e-3);
    let light = [to_eye[0] / l * 0.8 + to_eye[1] / l * 0.35, to_eye[1] / l * 0.8 - to_eye[0] / l * 0.35, 0.9];
    // Effects: PIE art and Precision trails (simulations live in app.fx).
    let mut fx_batches = Vec::new();
    let fx_notes: Vec<String>;
    {
        let norm = |v: [f32; 3]| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
            [v[0] / l, v[1] / l, v[2] / l]
        };
        let fwd = norm([cam.target[0] - eye[0], cam.target[1] - eye[1], cam.target[2] - eye[2]]);
        let right = norm([fwd[1], -fwd[0], 0.0]);
        let up = [right[1] * fwd[2] - right[2] * fwd[1], right[2] * fwd[0] - right[0] * fwd[2], right[0] * fwd[1] - right[1] * fwd[0]];
        let fcam = crate::fx::Camera { eye, right, up };
        let mut player = std::mem::take(&mut app.fx);
        if player.addons.is_none() {
            player.addons = app.loaded.as_ref().map(|l| std::sync::Arc::new(l.db.addon_models()));
        }
        player.begin_frame();
        if app.scene.show_fx {
            if let (Some(doc), Some(l)) = (&app.doc, &app.loaded) {
                let items: Vec<(f32, &str)> = doc.annos.iter().map(|a| (a.time, a.text.as_str())).collect();
                let cues = crate::pie::resolve(&items, doc.duration(), &l.pie, Some(&l.db));
                let tgt = crate::sds_path::target_pos(app);
                let vfs = l.vfs.clone();
                let weapon_geo = app.character.as_ref().and_then(|c| c.weapon_geometry());
                // Spell projectiles: flight, explosion and hit art on impact.
                let flights = crate::projectile::flights(app, &cues, path.as_ref());
                if !flights.is_empty() {
                    let yaw = (root.t[0] - tgt[0]).atan2(root.t[1] - tgt[1]);
                    let (s, c) = (-yaw).sin_cos();
                    let target = Transform::from_av([tgt[0], tgt[1], 0.0], [c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0], 1.0);
                    crate::projectile::draw(&flights, t, &mut player, &vfs, &fcam, &target, &mut fx_batches);
                    if app.scene.show_path {
                        for f in &flights {
                            let col = if f.impact.is_some() { [1.0, 0.6, 0.2, 0.7] } else { [0.6, 0.6, 0.6, 0.5] };
                            let n = 24;
                            let span = (f.end - f.shot.start).max(0.0);
                            let pts: Vec<[f32; 3]> = (0..=n).map(|i| f.pos(f.shot.start + span * i as f32 / n as f32)).collect();
                            let pts = if span == 0.0 { vec![f.from, f.to] } else { pts };
                            for w in pts.windows(2) {
                                line(&mut lines, w[0], w[1], col);
                            }
                            // The blast radius around where it went off.
                            if let (Some(ti), true) = (f.impact, f.shot.blast_radius > 0.0) {
                                if t >= ti && t < ti + crate::projectile::EXPLOSION_LIFE {
                                    let (c, r) = (f.hit_point, f.shot.blast_radius);
                                    let ring: Vec<[f32; 3]> = (0..=48).map(|i| {
                                        let a = i as f32 / 48.0 * std::f32::consts::TAU;
                                        [c[0] + r * a.cos(), c[1] + r * a.sin(), 1.0]
                                    }).collect();
                                    for w in ring.windows(2) {
                                        line(&mut lines, w[0], w[1], [1.0, 0.35, 0.2, 0.8]);
                                    }
                                }
                            }
                        }
                    }
                }
                for cue in &cues {
                    for (k, piece) in cue.pieces.iter().enumerate() {
                        if t < piece.start || t >= piece.start + piece.life {
                            continue;
                        }
                        let asset = player.asset(&vfs, &piece.model);
                        // Emitters that name the target's 3D (blade effects): the
                        // art goes on the weapon and emits from its surface.
                        // AttachT NamedNode&X puts the art on node X of whoever it plays
                        // on (the target dummy has no nodes and keeps it at its feet).
                        let anchor = match (&asset.attach_node, &piece.anchor) {
                            (Some(n), crate::pie::Anchor::Node(_)) => crate::pie::Anchor::Node(n.clone()),
                            (None, crate::pie::Anchor::Node(_)) if asset.needs_target && weapon_geo.is_some() => crate::pie::Anchor::Node("WEAPON".into()),
                            _ => piece.anchor.clone(),
                        };
                        let on_weapon = weapon_geo.is_some() && matches!(&anchor, crate::pie::Anchor::Node(n) if n.eq_ignore_ascii_case("WEAPON"));
                        let world = match &anchor {
                            crate::pie::Anchor::Node(n) => match (&app.character, &model) {
                                (Some(c), Some(m)) => c.node_world(m, &root, n).unwrap_or(root),
                                _ => root,
                            },
                            crate::pie::Anchor::Target => {
                                let yaw = (root.t[0] - tgt[0]).atan2(root.t[1] - tgt[1]);
                                let (s, c) = (-yaw).sin_cos();
                                Transform::from_av([tgt[0], tgt[1], 0.0], [c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0], 1.0)
                            }
                        };
                        let world = world.compose(&Transform::from_av(piece.offset, Transform::IDENTITY.rot, piece.scale));
                        let key = format!("{}:{}:{}:{}:{}", cue.index, k, piece.start, piece.model, on_weapon);
                        player.draw_full(&vfs, &key, &asset, &world, t - piece.start, &fcam, if on_weapon { weapon_geo.as_ref() } else { None }, &mut fx_batches);
                    }
                }
            }
        }
        crate::precision_view::trails(app, &mut player, t, &root_at, &mut fx_batches);
        player.end_frame();
        fx_notes = player.notes.drain(..).collect();
        app.fx = player;
    }
    // Effect shaders (EFSH) over the NPC's own meshes, not the partner's.
    let shaders = match (&app.doc, &app.loaded) {
        (Some(doc), Some(l)) if app.scene.show_fx => {
            let items: Vec<(f32, &str)> = doc.annos.iter().map(|a| (a.time, a.text.as_str())).collect();
            crate::pie::active_shaders(&crate::pie::resolve(&items, doc.duration(), &l.pie, Some(&l.db)), &l.db, t)
        }
        _ => Vec::new(),
    };
    let gen = app.character_gen * 10_000;
    let mut shells = Vec::new();
    for a in &shaders {
        let s = &a.shader;
        for m in skinned.iter().filter(|m| (gen..gen + 5_000).contains(&m.id) && (!s.skin_only || m.skin) && !m.blend) {
            shells.push(crate::render::Shell {
                mesh: m.id,
                fill_texture: &s.fill_texture,
                palette: if s.grayscale { &s.palette } else { "" },
                fill: a.fill,
                edge: a.edge,
                uv_scale: s.uv_scale,
                uv_offset: [(s.uv_speed[0] * a.age).fract(), (s.uv_speed[1] * a.age).fract()],
                eye,
                falloff: s.falloff,
            });
        }
    }
    let frame = Frame { view_proj, light, meshes, lines, overlay, fx: fx_batches, shells, clear: [0.085, 0.09, 0.105], clear_alpha: 1.0, ambient: 0.0 };
    let vfs = app.loaded.as_ref().map(|l| l.vfs.clone());
    // Effect textures are often written without the textures\ prefix (handled there).
    let mut load = |path: &str| crate::character::load_texture(vfs.as_ref()?, path);
    let tex = app.gpu.as_mut().unwrap().render(size_px, &frame, &mut load);
    let painter = ui.painter_at(rect);
    painter.image(tex, rect, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

    // Labels for annotation markers on the path.
    if let Some(doc) = &app.doc {
        if let Some(i) = app.selected {
            if let Some(a) = doc.annos.get(i) {
                let q = root_at(a.time).t;
                if let Some(ndc) = project(&view_proj, [q[0], q[1], 16.0]) {
                    let s = pos2(rect.left() + (ndc[0] + 1.0) * 0.5 * rect.width(), rect.top() + (1.0 - ndc[1]) * 0.5 * rect.height());
                    painter.text(s, Align2::CENTER_BOTTOM, &a.text, FontId::proportional(11.0), Color32::WHITE);
                }
            }
        }
        let heading = path.as_ref().map_or(0.0, |p| p.at(t).1);
        let dist = (target[0] - root.t[0]).hypot(target[1] - root.t[1]);
        let to_target = sds_anno::sdscombat::wrap((target[0] - root.t[0]).atan2(target[1] - root.t[1]).to_degrees() - heading);
        let lock = match state {
            _ if !app.sdscombat_active() => sds_i18n::t!("SDSCombat выкл", "SDSCombat off").to_string(),
            Some(s) if s.stop => "StopRotation".to_string(),
            Some(s) => match s.lock {
                Some(l) => sds_i18n::tf!("лок {l:.0}°/с", "lock {l:.0}°/s"),
                None => sds_i18n::t!("без лока", "no lock").to_string(),
            },
            None => String::new(),
        };
        let reach = state.and_then(|s| s.reach).map(|k| sds_i18n::tf!(" · рывок ×{k:.2}", " · lunge ×{k:.2}")).unwrap_or_default();
        let info = sds_i18n::tf!("t = {:.3} с · кадр {} · позиция ({:.0}, {:.0}) · курс {:+.1}° · до цели {:.0} (угол {:+.0}°) · {}{}{}", "t = {:.3} s · frame {} · position ({:.0}, {:.0}) · heading {:+.1}° · to target {:.0} (angle {:+.0}°) · {}{}{}",
            t,
            (t / doc.frame_time()).round(),
            root.t[0],
            root.t[1],
            heading,
            dist,
            to_target,
            lock,
            reach,
            if app.anim.is_none() { sds_i18n::t!(" · поза не декодирована", " · pose not decoded") } else { "" }
        );
        painter.text(rect.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, info, FontId::proportional(12.0), Color32::from_gray(220));
    }
    let mismatch = match (&app.character, &app.anim) {
        (Some(c), Some(a)) => a.paired().is_empty() && (0..a.tracks).any(|t| a.bone_of_track(t) >= c.skeleton.bones.len()),
        _ => false,
    };
    if mismatch {
        painter.text(rect.left_top() + vec2(8.0, 26.0), Align2::LEFT_TOP, sds_i18n::t!("Клип рассчитан на другой скелет (треков больше, чем костей)", "The clip is made for another skeleton (more tracks than bones)"), FontId::proportional(12.0), Color32::from_rgb(240, 160, 80));
    }
    let hint = match &app.character {
        Some(c) => sds_i18n::tf!("{} · ЛКМ — вращать, ПКМ — сдвиг, колесо — зум", "{} · LMB — rotate, RMB — pan, wheel — zoom", c.skeleton_path),
        None => sds_i18n::t!("Персонаж не загружен (нужна сборка и клип из meshes/actors/…)", "Character not loaded (needs the build and a clip from meshes/actors/…)").into(),
    };
    painter.text(rect.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, hint, FontId::proportional(10.5), Color32::from_gray(140));
    for n in fx_notes {
        app.log(format!("FX: {n}"));
    }
    if app.play.playing {
        ui.ctx().request_repaint();
    }
}
