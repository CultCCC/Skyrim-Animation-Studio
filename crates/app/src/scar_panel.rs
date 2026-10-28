//! "SCAR" tab: the clip's SCAR_ActionData as ring sectors around the NPC
//! (top view, NPC facing up), draggable; the target dummy; which actions the
//! target satisfies; a full ActionData editor.
//!
//! SCAR 2.0 picks a follow-up from the clip's ActionData when its combo
//! window opens (SCAR_ComboStart): an action applies when the target's
//! distance is within MinDistance..MaxDistance, the angle to it (relative to
//! the NPC's facing, + = right) within StartAngle..EndAngle, and the Chance
//! roll passes; TriggerStartTime/TriggerEndTime narrow the clip window.

use crate::app::StudioApp;
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Sense, Shape, Stroke};
use sds_anno::{parse, Kind, ScarAction, SCAR_EXTRA_KEYS, SCAR_TYPES};

pub fn type_color(ty: &str) -> Color32 {
    match ty.to_ascii_uppercase().as_str() {
        "RA" => Color32::from_rgb(240, 160, 60),
        "RPA" => Color32::from_rgb(235, 80, 70),
        "LA" => Color32::from_rgb(230, 220, 80),
        "LPA" => Color32::from_rgb(220, 90, 200),
        "BA" => Color32::from_rgb(90, 150, 240),
        "BPA" => Color32::from_rgb(70, 90, 220),
        "DA" => Color32::from_rgb(80, 220, 220),
        "DPA" => Color32::from_rgb(40, 170, 150),
        _ => Color32::from_rgb(170, 170, 180),
    }
}

/// NPC position/heading at `t` and the target in the NPC's frame: (pos, heading, distance, angle).
pub fn target_local(app: &StudioApp, t: f32) -> ([f32; 3], f32, f32, f32) {
    let path = crate::sds_path::compute(app, false);
    let (pos, heading) = match &path {
        Some(p) if app.scene.root_motion => p.at(t),
        _ => ([0.0; 3], 0.0),
    };
    let tg = crate::sds_path::target_pos(app);
    let (dx, dy) = (tg[0] - pos[0], tg[1] - pos[1]);
    let dist = dx.hypot(dy);
    let angle = sds_anno::sdscombat::wrap(dx.atan2(dy).to_degrees() - heading);
    (pos, heading, dist, angle)
}

/// The clip's ActionData: (annotation index, time, action).
pub fn actions(app: &StudioApp) -> Vec<(usize, f32, ScarAction)> {
    let Some(doc) = &app.doc else { return Vec::new() };
    doc.annos
        .iter()
        .enumerate()
        .filter_map(|(i, a)| match parse(&a.text).kind {
            Kind::ScarAction(s) => Some((i, a.time, s)),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Handle {
    Inner,
    Outer,
    Start,
    End,
    Target,
}

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui) {
    if app.doc.is_none() {
        ui.label(sds_i18n::t!("Откройте клип: здесь будут зоны SCAR_ActionData вокруг NPC.", "Open a clip: the SCAR_ActionData zones around the NPC show here."));
        return;
    }
    let t = app.play.time;
    let list = actions(app);
    let (npc_pos, heading, tdist, tangle) = target_local(app, t);
    let combo: Vec<f32> = app.doc.as_ref().unwrap().annos.iter().filter(|a| a.text.trim().eq_ignore_ascii_case("SCAR_ComboStart")).map(|a| a.time).collect();

    ui.horizontal_wrapped(|ui| {
        ui.label(format!("ActionData: {} · SCAR_ComboStart: {}", list.len(), if combo.is_empty() { sds_i18n::t!("нет", "no").to_string() } else { combo.iter().map(|c| sds_i18n::tf!("{c:.2} с", "{c:.2} s")).collect::<Vec<_>>().join(", ") }));
        ui.separator();
        ui.label(sds_i18n::tf!("цель: {tdist:.0} ед, {tangle:+.0}°", "target: {tdist:.0} u, {tangle:+.0}°"));
    });

    // ---- canvas ------------------------------------------------------------
    let side = ui.available_width().min(460.0).max(220.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), side), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, Color32::from_rgb(20, 22, 26));
    let range = list.iter().map(|(_, _, a)| a.max_distance).fold(tdist, f32::max).max(300.0) * 1.15;
    let center = rect.center();
    let scale = (side * 0.5 - 14.0) / range;
    let to_screen = |d: f32, a: f32| -> Pos2 {
        let r = a.to_radians();
        pos2(center.x + d * r.sin() * scale, center.y - d * r.cos() * scale)
    };
    let from_screen = |q: Pos2| -> (f32, f32) {
        let x = (q.x - center.x) / scale;
        let y = -(q.y - center.y) / scale;
        (x.hypot(y), x.atan2(y).to_degrees())
    };
    // Rings and spokes.
    let step = if range > 1500.0 { 500.0 } else if range > 600.0 { 200.0 } else { 100.0 };
    let mut d = step;
    while d <= range {
        p.circle_stroke(center, d * scale, Stroke::new(1.0_f32, Color32::from_white_alpha(18)));
        p.text(to_screen(d, 0.0) + vec2(3.0, -2.0), Align2::LEFT_BOTTOM, format!("{d:.0}"), FontId::proportional(9.5), Color32::from_gray(110));
        d += step;
    }
    for k in 0..12 {
        let a = k as f32 * 30.0;
        p.line_segment([center, to_screen(range, a)], Stroke::new(1.0_f32, Color32::from_white_alpha(if k % 3 == 0 { 22 } else { 10 })));
    }
    let selected = app.selected;
    // Sectors.
    for (i, at, a) in &list {
        let active = a.contains(tdist, tangle) && a.in_window(t);
        let sel = selected == Some(*i);
        let col = type_color(&a.ty);
        let fill = col.gamma_multiply(if active { 0.38 } else { 0.14 });
        let (s, mut e) = (a.start_angle, a.end_angle);
        if e < s {
            e += 360.0;
        }
        // A full circle may be written as -360..360: draw it once.
        e = e.min(s + 360.0);
        let n = (((e - s).abs() / 6.0).ceil() as usize).clamp(2, 64);
        for k in 0..n {
            let a0 = s + (e - s) * k as f32 / n as f32;
            let a1 = s + (e - s) * (k + 1) as f32 / n as f32;
            p.add(Shape::convex_polygon(
                vec![to_screen(a.min_distance, a0), to_screen(a.max_distance, a0), to_screen(a.max_distance, a1), to_screen(a.min_distance, a1)],
                fill,
                Stroke::NONE,
            ));
        }
        let stroke = Stroke::new(if sel { 2.5_f32 } else { 1.2_f32 }, if sel { Color32::WHITE } else { col });
        let arc = |d: f32| -> Vec<Pos2> { (0..=n).map(|k| to_screen(d, s + (e - s) * k as f32 / n as f32)).collect() };
        p.add(Shape::line(arc(a.max_distance), stroke));
        if a.min_distance > 0.0 {
            p.add(Shape::line(arc(a.min_distance), stroke));
        }
        if e - s < 359.9 {
            p.line_segment([to_screen(a.min_distance, s), to_screen(a.max_distance, s)], stroke);
            p.line_segment([to_screen(a.min_distance, e), to_screen(a.max_distance, e)], stroke);
        }
        let mid = (s + e) * 0.5;
        p.text(to_screen((a.min_distance + a.max_distance) * 0.5, mid), Align2::CENTER_CENTER, sds_i18n::tf!("{} {:.2}с", "{} {:.2}s", a.ty, at), FontId::proportional(10.5), if active { Color32::WHITE } else { col });
    }
    // NPC and target.
    p.add(Shape::convex_polygon(vec![center + vec2(0.0, -12.0), center + vec2(8.0, 8.0), center + vec2(-8.0, 8.0)], Color32::from_rgb(240, 210, 90), Stroke::new(1.0_f32, Color32::BLACK)));
    let tpos = to_screen(tdist, tangle);
    p.circle_filled(tpos, 7.0, Color32::from_rgb(90, 240, 115));
    p.text(tpos + vec2(9.0, 0.0), Align2::LEFT_CENTER, sds_i18n::t!("цель", "target"), FontId::proportional(10.5), Color32::from_rgb(90, 240, 115));

    // Handles of the selected action.
    let sel_action = list.iter().find(|(i, _, _)| Some(*i) == selected).cloned();
    let mut handles: Vec<(Handle, Pos2)> = vec![(Handle::Target, tpos)];
    if let Some((_, _, a)) = &sel_action {
        let mut e = a.end_angle;
        if e < a.start_angle {
            e += 360.0;
        }
        let mid = (a.start_angle + e) * 0.5;
        handles.push((Handle::Outer, to_screen(a.max_distance, mid)));
        handles.push((Handle::Inner, to_screen(a.min_distance, mid)));
        handles.push((Handle::Start, to_screen(a.max_distance, a.start_angle)));
        handles.push((Handle::End, to_screen(a.max_distance, a.end_angle)));
        for (h, q) in &handles[1..] {
            let c = match h {
                Handle::Inner | Handle::Outer => Color32::WHITE,
                _ => Color32::from_rgb(255, 200, 90),
            };
            p.circle_filled(*q, 5.5, c);
            p.circle_stroke(*q, 5.5, Stroke::new(1.0_f32, Color32::BLACK));
        }
    }
    let drag_id = ui.id().with("scar-drag");
    let mut drag: Option<Handle> = ui.data(|d| d.get_temp::<u8>(drag_id)).and_then(|v| match v {
        1 => Some(Handle::Inner),
        2 => Some(Handle::Outer),
        3 => Some(Handle::Start),
        4 => Some(Handle::End),
        5 => Some(Handle::Target),
        _ => None,
    });
    if resp.drag_started() {
        let press = ui.input(|i| i.pointer.press_origin());
        drag = press.and_then(|q| handles.iter().filter(|(_, h)| h.distance(q) < 10.0).min_by(|a, b| a.1.distance(q).total_cmp(&b.1.distance(q))).map(|(h, _)| *h));
    }
    let mut new_action: Option<(usize, ScarAction, Handle)> = None;
    let mut new_target: Option<(f32, f32)> = None;
    if resp.dragged() {
        if let (Some(h), Some(q)) = (drag, resp.interact_pointer_pos()) {
            let (d, a) = from_screen(q);
            let snap_d = (d / 5.0).round() * 5.0;
            let snap_a = a.round();
            match (h, &sel_action) {
                (Handle::Target, _) => new_target = Some((d, a)),
                (Handle::Outer, Some((i, _, s))) => new_action = Some((*i, ScarAction { max_distance: snap_d.max(s.min_distance), ..s.clone() }, h)),
                (Handle::Inner, Some((i, _, s))) => new_action = Some((*i, ScarAction { min_distance: snap_d.clamp(0.0, s.max_distance), ..s.clone() }, h)),
                (Handle::Start, Some((i, _, s))) => new_action = Some((*i, ScarAction { start_angle: snap_a, ..s.clone() }, h)),
                (Handle::End, Some((i, _, s))) => new_action = Some((*i, ScarAction { end_angle: snap_a, ..s.clone() }, h)),
                _ => {}
            }
        }
    }
    if resp.drag_stopped() {
        drag = None;
    }
    ui.data_mut(|d| {
        d.insert_temp::<u8>(
            drag_id,
            match drag {
                Some(Handle::Inner) => 1,
                Some(Handle::Outer) => 2,
                Some(Handle::Start) => 3,
                Some(Handle::End) => 4,
                Some(Handle::Target) => 5,
                None => 0,
            },
        )
    });
    // A click selects the smallest sector under the pointer.
    let mut pick: Option<usize> = None;
    if resp.clicked() {
        if let Some(q) = resp.interact_pointer_pos() {
            let (d, a) = from_screen(q);
            pick = list
                .iter()
                .filter(|(_, _, s)| s.contains(d, a))
                .min_by(|x, y| {
                    let area = |s: &ScarAction| (s.max_distance - s.min_distance) * (((s.end_angle - s.start_angle) % 360.0 + 360.0) % 360.0);
                    area(&x.2).total_cmp(&area(&y.2))
                })
                .map(|(i, _, _)| *i);
        }
    }
    ui.small(sds_i18n::t!("Вид сверху: NPC в центре, смотрит вверх, + угол — вправо. ЛКМ по зоне — выбрать; тяните белые точки (дистанции), жёлтые (углы) и зелёную цель.", "Top view: the NPC in the centre, facing up, + angle — to the right. LMB on a zone — select; drag the white dots (distances), yellow (angles) and the green target."));

    // ---- list ----------------------------------------------------------------
    let mut delete: Option<usize> = None;
    egui::Grid::new("scar-list").num_columns(5).striped(true).show(ui, |ui| {
        for (i, at, a) in &list {
            let in_zone = a.contains(tdist, tangle);
            let in_win = a.in_window(t);
            let sel = selected == Some(*i);
            let label = egui::RichText::new(format!("{} {}", a.ty, a.idle)).color(type_color(&a.ty));
            if ui.selectable_label(sel, label).clicked() {
                pick = Some(*i);
            }
            ui.label(sds_i18n::tf!("{:.0}–{:.0} ед · {:+.0}°..{:+.0}° · {:.0}%", "{:.0}–{:.0} u · {:+.0}°..{:+.0}° · {:.0}%", a.min_distance, a.max_distance, a.start_angle, a.end_angle, a.chance));
            ui.label(sds_i18n::tf!("{at:.2} с", "{at:.2} s"));
            ui.label(match (in_zone, in_win) {
                (true, true) => egui::RichText::new(sds_i18n::t!("✔ цель в зоне", "✔ target in the zone")).color(Color32::from_rgb(90, 230, 110)),
                (true, false) => egui::RichText::new(sds_i18n::t!("в зоне, вне окна времени", "in the zone, outside the time window")).color(Color32::from_rgb(230, 190, 90)),
                _ => egui::RichText::new("✗").color(Color32::from_gray(130)),
            });
            if ui.small_button("✕").on_hover_text(sds_i18n::t!("удалить", "delete")).clicked() {
                delete = Some(*i);
            }
            ui.end_row();
        }
    });
    ui.horizontal(|ui| {
        ui.menu_button(sds_i18n::t!("Добавить ActionData на плейхед", "Add ActionData at the playhead"), |ui| {
            for (ty, help) in SCAR_TYPES {
                if ui.button(format!("{ty} — {help}")).clicked() {
                    let a = ScarAction {
                        idle: if *ty == "RPA" || *ty == "LPA" || *ty == "DPA" || *ty == "BPA" { "ADXP_NPCPowerAttack".into() } else { "ADXP_NPCNormalAttack".into() },
                        min_distance: 0.0,
                        max_distance: 250.0,
                        start_angle: -60.0,
                        end_angle: 60.0,
                        chance: 100.0,
                        ty: ty.to_string(),
                        extra: Vec::new(),
                    };
                    app.add_annotation(a.to_text());
                    ui.close();
                }
            }
        });
        if ui.button(sds_i18n::t!("Добавить SCAR_ComboStart", "Add SCAR_ComboStart")).clicked() {
            app.add_annotation("SCAR_ComboStart".into());
        }
    });

    // ---- editor --------------------------------------------------------------
    let mut edited: Option<(usize, ScarAction, Option<String>)> = None;
    if let Some((i, _, a)) = &sel_action {
        ui.separator();
        ui.heading("ActionData");
        if let Some((s, key)) = editor(ui, *i, a) {
            edited = Some((*i, s, key));
        }
    }

    // ---- apply -----------------------------------------------------------------
    if let Some(i) = pick {
        app.selected = Some(i);
    }
    if let Some((d, a)) = new_target {
        // Target in the NPC's frame at t → world → distance/angle from the origin.
        let w = sds_anno::sdscombat::to_world([d * a.to_radians().sin(), d * a.to_radians().cos()], heading);
        let world = [npc_pos[0] + w[0], npc_pos[1] + w[1]];
        app.scene.target_dist = world[0].hypot(world[1]).round();
        app.scene.target_angle = world[0].atan2(world[1]).to_degrees().round();
    }
    if let Some((i, s, h)) = new_action {
        let key = format!("scar-drag:{i}:{}", h as u8);
        if let Some(doc) = &mut app.doc {
            app.selected = doc.edit_merge(Some(i), &key, |l| l[i].text = s.to_text());
        }
    }
    if let Some((i, s, key)) = edited {
        if let Some(doc) = &mut app.doc {
            app.selected = match key {
                Some(k) => doc.edit_merge(Some(i), &k, |l| l[i].text = s.to_text()),
                None => doc.edit(Some(i), |l| l[i].text = s.to_text()),
            };
        }
    }
    if let Some(i) = delete {
        app.selected = Some(i);
        app.delete_selected();
    }
}

/// Every ActionData field; returns the changed action and a merge key for drags.
fn editor(ui: &mut egui::Ui, index: usize, a: &ScarAction) -> Option<(ScarAction, Option<String>)> {
    let mut s = a.clone();
    let mut changed = false;
    let mut merge: Option<String> = None;
    egui::Grid::new(("scar-edit", index)).num_columns(2).show(ui, |ui| {
        ui.label("Type");
        egui::ComboBox::from_id_salt(("scar-type", index)).selected_text(&s.ty).show_ui(ui, |ui| {
            for (ty, help) in SCAR_TYPES {
                if ui.selectable_label(s.ty.eq_ignore_ascii_case(ty), format!("{ty} — {help}")).clicked() {
                    s.ty = ty.to_string();
                    changed = true;
                }
            }
        });
        ui.end_row();
        ui.label("IdleAnimation");
        if ui.text_edit_singleline(&mut s.idle).on_hover_text(sds_i18n::t!("EditorID формы IDLE, которую сыграет SCAR", "EditorID of the IDLE form SCAR will play")).changed() {
            changed = true;
            merge = Some(format!("scar:{index}:idle"));
        }
        ui.end_row();
        let mut num = |ui: &mut egui::Ui, label: &str, v: &mut f32, speed: f64, key: &str| {
            ui.label(label);
            if ui.add(egui::DragValue::new(v).speed(speed)).changed() {
                changed = true;
                merge = Some(format!("scar:{index}:{key}"));
            }
            ui.end_row();
        };
        num(ui, "MinDistance", &mut s.min_distance, 1.0, "min");
        num(ui, "MaxDistance", &mut s.max_distance, 1.0, "max");
        num(ui, "StartAngle", &mut s.start_angle, 1.0, "sa");
        num(ui, "EndAngle", &mut s.end_angle, 1.0, "ea");
        num(ui, "Chance", &mut s.chance, 1.0, "ch");
        for (key, help) in SCAR_EXTRA_KEYS {
            let at = s.extra.iter().position(|(k, _)| k == key);
            let mut on = at.is_some();
            if ui.checkbox(&mut on, *key).on_hover_text(help.get()).changed() {
                if on {
                    let v: serde_json::Value = match *key {
                        "WeaponLength" => true.into(),
                        "CoolDownAlias" | "ConditionsAlias" => "".into(),
                        "VariantID" => 0.into(),
                        _ => 0.0.into(),
                    };
                    s.extra.push((key.to_string(), v));
                } else if let Some(k) = at {
                    s.extra.remove(k);
                }
                changed = true;
            }
            if let Some(k) = s.extra.iter().position(|(n, _)| n == key) {
                let v = &mut s.extra[k].1;
                match v {
                    serde_json::Value::Bool(b) => {
                        if ui.checkbox(b, "").changed() {
                            changed = true;
                        }
                    }
                    serde_json::Value::String(t) => {
                        if ui.text_edit_singleline(t).changed() {
                            changed = true;
                            merge = Some(format!("scar:{index}:{key}"));
                        }
                    }
                    _ => {
                        let mut x = v.as_f64().unwrap_or(0.0);
                        if ui.add(egui::DragValue::new(&mut x).speed(0.01).max_decimals(3)).changed() {
                            *v = if *key == "VariantID" { (x.round() as i64).into() } else { x.into() };
                            changed = true;
                            merge = Some(format!("scar:{index}:{key}"));
                        }
                    }
                }
            } else {
                ui.label("");
            }
            ui.end_row();
        }
    });
    changed.then_some((s, merge))
}
