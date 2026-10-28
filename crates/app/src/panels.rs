//! Library, viewport, inspector, annotation list, scene and log panels.

use crate::app::StudioApp;
use crate::doc::ClipSource;
use crate::library::{file_name, Node};
use crate::theme::category_color;
use egui::{pos2, vec2, Align2, Color32, FontId, Sense, Shape, Stroke};
use sds_anno::{parse, Category, Kind};
use std::sync::atomic::Ordering;

// ---- library --------------------------------------------------------------

pub fn library(app: &mut StudioApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(sds_i18n::t!("Сборка", "Build"))
        .default_open(app.loaded.is_none())
        .show(ui, |ui| source_settings(app, ui));

    if let Some(l) = &app.loading {
        let done = l.progress.roots_done.load(Ordering::Relaxed);
        let total = l.progress.roots_total.load(Ordering::Relaxed).max(1);
        ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(sds_i18n::tf!("Индексация: {done}/{total} модов, {} файлов, {:.0} с", "Indexing: {done}/{total} mods, {} files, {:.0} s",
            l.progress.files.load(Ordering::Relaxed),
            l.started.elapsed().as_secs_f32()
        )));
    }

    ui.horizontal(|ui| {
        ui.selectable_value(&mut app.lib_mode_oar, false, sds_i18n::t!("Папки", "Folders"));
        ui.selectable_value(&mut app.lib_mode_oar, true, "OAR");
        ui.add(egui::TextEdit::singleline(&mut app.lib_filter).hint_text(sds_i18n::t!("поиск…", "search…")).desired_width(f32::INFINITY));
    });
    ui.separator();

    let Some(loaded) = &app.loaded else {
        ui.label(sds_i18n::t!("Сборка не загружена.", "Build not loaded."));
        return;
    };
    let lib = &loaded.library;
    let current = match app.doc.as_ref().map(|d| &d.source) {
        Some(ClipSource::Vfs(p)) => Some(p.clone()),
        _ => None,
    };
    let mut open: Option<String> = None;
    let mut open_place: Option<crate::oar::Place> = None;
    let mut pick_sub: Option<String> = None;
    let mut add_folder = false;
    let mut clear_external = false;
    let filter = app.lib_filter.trim().to_lowercase();

    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        if !filter.is_empty() {
            let mut shown = 0;
            for p in &lib.anims {
                if p.contains(&filter) {
                    let sel = current.as_deref() == Some(p.as_str());
                    if ui.selectable_label(sel, file_name(p)).on_hover_text(p).clicked() {
                        open = Some(p.clone());
                    }
                    shown += 1;
                    if shown >= 1000 {
                        ui.label(sds_i18n::t!("… показаны первые 1000", "… first 1000 shown"));
                        break;
                    }
                }
            }
            if shown == 0 {
                ui.label(sds_i18n::t!("Ничего не найдено", "Nothing found"));
            }
        } else if app.lib_mode_oar {
            ui.horizontal(|ui| {
                if ui.button(sds_i18n::t!("Добавить папку OAR с диска…", "Add an OAR folder from disk…")).on_hover_text(sds_i18n::t!("Папка мода, папка OpenAnimationReplacer или целый мод — ищутся все config.json", "A mod folder, an OpenAnimationReplacer folder or a whole mod — every config.json is found")).clicked() {
                    add_folder = true;
                }
                if !app.oar_external.is_empty() && ui.button(sds_i18n::t!("Убрать внешние", "Remove external")).clicked() {
                    clear_external = true;
                }
            });
            let selected = app.oar_edit.as_ref().map(|e| e.folder.clone());
            let show_mod = |ui: &mut egui::Ui, m: &crate::oar::OarMod, open: &mut Option<crate::oar::Place>, pick: &mut Option<String>| {
                let title = format!("{}{}{}", if m.external { "📁 " } else { "" }, m.name, if m.author.is_empty() { String::new() } else { format!(" — {}", m.author) });
                egui::CollapsingHeader::new(title).id_salt(&m.folder).show(ui, |ui| {
                    for s in &m.subs {
                        let head = format!("{}  [{}]{}{}  ({})", s.name, s.priority, if s.disabled { sds_i18n::t!(" выкл", " off") } else { "" }, if s.has_user { " user.json" } else { "" }, s.files.len());
                        let resp = egui::CollapsingHeader::new(head).id_salt(&s.folder).show(ui, |ui| {
                            let is_sel = selected.as_deref() == Some(s.folder.as_str());
                            if ui.selectable_label(is_sel, sds_i18n::t!("⚙ свойства и экспорт", "⚙ properties & export")).clicked() {
                                *pick = Some(s.folder.clone());
                            }
                            for (rel, place) in &s.files {
                                let sel = match place {
                                    crate::oar::Place::Vfs(p) => current.as_deref() == Some(p.as_str()),
                                    crate::oar::Place::Disk(_) => false,
                                };
                                if ui.selectable_label(sel, rel).on_hover_text(place.label()).clicked() {
                                    *open = Some(place.clone());
                                }
                            }
                        });
                        resp.header_response.context_menu(|ui| {
                            if ui.button(sds_i18n::t!("Свойства и экспорт", "Properties & export")).clicked() {
                                *pick = Some(s.folder.clone());
                                ui.close();
                            }
                        });
                    }
                });
            };
            for m in &app.oar_external {
                show_mod(ui, m, &mut open_place, &mut pick_sub);
            }
            for m in &lib.oar {
                show_mod(ui, m, &mut open_place, &mut pick_sub);
            }
            if lib.oar.is_empty() && app.oar_external.is_empty() {
                ui.label(sds_i18n::t!("OAR-модов не найдено", "No OAR mods found"));
            }
        } else {
            tree(ui, &lib.tree, &lib.anims, "meshes", &current, &mut open);
        }
    });
    if let Some(p) = open {
        app.defer(crate::app::Deferred::Open(crate::doc::ClipSource::Vfs(p)));
    }
    match open_place {
        Some(crate::oar::Place::Vfs(p)) => app.defer(crate::app::Deferred::Open(crate::doc::ClipSource::Vfs(p))),
        Some(crate::oar::Place::Disk(p)) => app.defer(crate::app::Deferred::Open(crate::doc::ClipSource::Disk(p))),
        None => {}
    }
    if let Some(f) = pick_sub {
        crate::oar_panel::select(app, &f);
        // The editor lives in the OAR tab: open it even if the layout lost it.
        app.show_tab(crate::app::Tab::Oar);
    }
    if clear_external {
        app.oar_external.clear();
    }
    if add_folder {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            crate::oar_panel::add_external(app, &dir);
        }
    }
}

fn tree(ui: &mut egui::Ui, node: &Node, anims: &[String], path: &str, current: &Option<String>, open: &mut Option<String>) {
    for (name, child) in &node.children {
        let id = format!("{path}/{name}");
        egui::CollapsingHeader::new(format!("{name}  ({})", child.count)).id_salt(&id).show(ui, |ui| {
            tree(ui, child, anims, &id, current, open);
        });
    }
    for &i in &node.files {
        let p = &anims[i];
        let sel = current.as_deref() == Some(p.as_str());
        if ui.selectable_label(sel, file_name(p)).clicked() {
            *open = Some(p.clone());
        }
    }
}

fn source_settings(app: &mut StudioApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.radio_value(&mut app.settings.use_mo2, true, "MO2");
        ui.radio_value(&mut app.settings.use_mo2, false, sds_i18n::t!("Папка Data", "Data folder"));
    });
    if app.settings.use_mo2 {
        ui.horizontal(|ui| {
            ui.label("MO2:");
            if ui.add(egui::TextEdit::singleline(&mut app.settings.mo2_dir).desired_width(ui.available_width() - 30.0)).lost_focus() {
                app.refresh_profiles();
            }
            if ui.button("…").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    app.settings.mo2_dir = p.display().to_string();
                    app.refresh_profiles();
                }
            }
        });
        egui::ComboBox::from_id_salt("profile")
            .selected_text(&app.settings.profile)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for p in app.profiles.clone() {
                    ui.selectable_value(&mut app.settings.profile, p.clone(), p);
                }
            });
    } else {
        ui.horizontal(|ui| {
            ui.label("Data:");
            ui.add(egui::TextEdit::singleline(&mut app.settings.data_dir).desired_width(ui.available_width() - 30.0));
            if ui.button("…").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    app.settings.data_dir = p.display().to_string();
                }
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label(sds_i18n::t!("Вывод:", "Output:"));
        ui.add(egui::TextEdit::singleline(&mut app.settings.output_dir).desired_width(ui.available_width() - 30.0))
            .on_hover_text(sds_i18n::t!("Папка-мод для правок: файлы ложатся с тем же путём от Data. Подключите её в MO2 выше оригинала.", "Mod folder for edits: files go there with the same path from Data. Enable it in MO2 above the original."));
        if ui.button("…").clicked() {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                app.settings.output_dir = p.display().to_string();
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.add_enabled(app.loading.is_none(), egui::Button::new(sds_i18n::t!("Загрузить", "Load"))).clicked() {
            app.settings.save();
            app.start_loading();
        }
        ui.checkbox(&mut app.settings.load_on_start, sds_i18n::t!("при запуске", "on start"));
    });
}

// ---- viewport ---------------------------------------------------------------

pub fn viewport(app: &mut StudioApp, ui: &mut egui::Ui) {
    crate::viewport3d::show(app, ui);
}

/// Top-down root-motion view (also the fallback without wgpu).
pub fn viewport_2d(app: &mut StudioApp, ui: &mut egui::Ui) {
    let id = ui.id().with("vp");
    let (mut zoom, mut pan): (f32, egui::Vec2) = ui.data(|d| d.get_temp(id)).unwrap_or((0.6, egui::Vec2::ZERO));
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, Color32::from_rgb(22, 24, 28));
    if resp.dragged() {
        pan += resp.drag_delta();
    }
    if resp.hovered() {
        let (s, z) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        if s != 0.0 || z != 1.0 {
            zoom = (zoom * (s * 0.002).exp() * z).clamp(0.02, 20.0);
        }
    }
    if resp.double_clicked() {
        zoom = 0.6;
        pan = egui::Vec2::ZERO;
    }
    ui.data_mut(|d| d.insert_temp(id, (zoom, pan)));

    // World: game units, +Y ahead of the NPC at the clip start, +X to its right.
    let origin = rect.center() + pan + vec2(0.0, rect.height() * 0.25);
    let to_screen = |x: f32, y: f32| pos2(origin.x + x * zoom, origin.y - y * zoom);
    let font = FontId::proportional(11.0);

    if app.scene.show_grid {
        let step = 64.0;
        let n = ((rect.width().max(rect.height()) / (step * zoom)) as i32 + 4).min(400);
        let c = ((origin - rect.center()) / (step * zoom)).round();
        for k in -n..=n {
            let major = k % 4 == 0;
            let col = Color32::from_white_alpha(if major { 22 } else { 9 });
            let x = (k as f32 - c.x) * step;
            let y = (k as f32 + c.y) * step;
            p.line_segment([to_screen(x, -1e5), to_screen(x, 1e5)], Stroke::new(1.0_f32, col));
            p.line_segment([to_screen(-1e5, y), to_screen(1e5, y)], Stroke::new(1.0_f32, col));
        }
        p.line_segment([to_screen(0.0, -1e5), to_screen(0.0, 1e5)], Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(80, 160, 80, 60)));
        p.line_segment([to_screen(-1e5, 0.0), to_screen(1e5, 0.0)], Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(160, 80, 80, 60)));
    }

    let Some(doc) = &app.doc else {
        p.text(rect.center(), Align2::CENTER_CENTER, sds_i18n::t!("Рендер-окно: 3D-персонаж появится на этапе 1.\nСейчас — вид сверху на путь root motion.", "Render window: the 3D character arrives in stage 1.\nFor now — a top view of the root motion path."), font, Color32::GRAY);
        return;
    };
    let dur = doc.duration();
    let m = &doc.motion;
    let use_motion = app.scene.root_motion;
    // The same simulated path the 3D view uses (SDSCombat, target, NPC scale).
    let sim = crate::sds_path::compute(app, false);
    let raw = if app.scene.compare_raw && app.sdscombat_active() { crate::sds_path::compute(app, true) } else { None };
    let pos_of = |path: &Option<sds_anno::sdscombat::Path>, t: f32| -> [f32; 3] {
        match path {
            Some(p) if use_motion => p.at(t).0,
            _ => [0.0; 3],
        }
    };
    let pos_at = |t: f32| pos_of(&sim, t);

    if app.scene.show_target {
        let tg = crate::sds_path::target_pos(app);
        let ts = to_screen(tg[0], tg[1]);
        p.circle_stroke(ts, (30.0 * zoom).max(4.0), Stroke::new(2.0_f32, Color32::from_rgb(90, 240, 115)));
        p.text(ts + vec2(0.0, -(30.0 * zoom).max(4.0) - 4.0), Align2::CENTER_BOTTOM, sds_i18n::t!("цель", "target"), font.clone(), Color32::from_rgb(90, 240, 115));
    }
    if app.scene.show_path && !m.moves.is_empty() {
        let n = 240;
        if raw.is_some() {
            let pts: Vec<egui::Pos2> = (0..=n).map(|k| { let q = pos_of(&raw, dur * k as f32 / n as f32); to_screen(q[0], q[1]) }).collect();
            p.add(Shape::line(pts, Stroke::new(1.5_f32, Color32::from_gray(150))));
        }
        let pts: Vec<egui::Pos2> = (0..=n).map(|k| {
            let q = pos_at(dur * k as f32 / n as f32);
            to_screen(q[0], q[1])
        }).collect();
        p.add(Shape::line(pts, Stroke::new(2.0_f32, category_color(Category::Motion).gamma_multiply(0.8))));
        // Frame ticks.
        let ft = doc.frame_time();
        let mut t = 0.0;
        while t <= dur + 1e-4 {
            let q = pos_at(t);
            p.circle_filled(to_screen(q[0], q[1]), 1.5, Color32::from_gray(120));
            t += ft;
        }
        // Annotation markers along the path.
        for a in &doc.annos {
            let pa = parse(&a.text);
            if matches!(pa.category, Category::Motion | Category::Foot | Category::Other) || app.scene.hidden.contains(&pa.category) {
                continue;
            }
            if pa.category == Category::SdsCombat && !app.sdscombat_active() {
                continue;
            }
            let q = pos_at(a.time);
            let s = to_screen(q[0], q[1]);
            p.circle_stroke(s, 4.0, Stroke::new(1.5_f32, category_color(pa.category)));
        }
    }

    // The NPC at the playhead.
    let q = pos_at(app.play.time);
    let yaw = match &sim {
        Some(p) if use_motion => p.at(app.play.time).1,
        _ => 0.0,
    };
    let s = to_screen(q[0], q[1]);
    let r = (24.0 * zoom).max(6.0);
    let yr = yaw.to_radians();
    let dir = vec2(yr.sin(), -yr.cos());
    p.circle_stroke(s, r, Stroke::new(2.0_f32, Color32::from_rgb(220, 220, 230)));
    p.line_segment([s, s + dir * r * 1.8], Stroke::new(2.5_f32, Color32::from_rgb(240, 200, 80)));

    let info = sds_i18n::tf!("t = {:.3} с · смещение ({:.1}, {:.1}, {:.1}) · поворот {:+.1}° · скорость {:.0} ед/с{}", "t = {:.3} s · offset ({:.1}, {:.1}, {:.1}) · turn {:+.1}° · speed {:.0} u/s{}",
        app.play.time,
        q[0],
        q[1],
        q[2],
        yaw,
        if use_motion { m.speed(app.play.time) } else { 0.0 },
        if m.is_empty() { sds_i18n::t!("\nВ клипе нет animmotion/animrotation", "\nThe clip has no animmotion/animrotation") } else { "" }
    );
    p.text(rect.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, info, font.clone(), Color32::from_gray(200));
    p.text(rect.left_bottom() + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, sds_i18n::t!("вид сверху · 1 клетка = 64 ед. · колесо — масштаб, ЛКМ — сдвиг, двойной клик — сброс · 3D — этап 1", "top view · 1 cell = 64 u · wheel — zoom, LMB — pan, double click — reset · 3D — stage 1"), FontId::proportional(10.5), Color32::from_gray(130));
}

// ---- inspector --------------------------------------------------------------

pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui) {
    if app.doc.is_none() {
        ui.label(sds_i18n::t!("Клип не открыт", "No clip open"));
        return;
    }
    let out_rel = app.output_rel_path();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let doc = app.doc.as_ref().unwrap();
        egui::CollapsingHeader::new(sds_i18n::t!("Клип", "Clip")).default_open(true).show(ui, |ui| {
            egui::Grid::new("clip-info").num_columns(2).striped(true).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                    ui.label(k);
                    ui.label(v);
                    ui.end_row();
                };
                row(ui, sds_i18n::t!("Файл", "File"), doc.name.clone());
                if let ClipSource::Vfs(p) = &doc.source {
                    row(ui, sds_i18n::t!("Путь", "Path"), p.clone());
                }
                row(ui, sds_i18n::t!("Источник", "Source"), doc.origin.clone());
                row(ui, sds_i18n::t!("Сжатие", "Compression"), doc.info.kind.to_string());
                row(ui, sds_i18n::t!("Длительность", "Duration"), sds_i18n::tf!("{:.4} с", "{:.4} s", doc.duration()));
                row(ui, sds_i18n::t!("Кадры", "Frames"), sds_i18n::tf!("{} (шаг {:.4} с)", "{} (step {:.4} s)", doc.last_frame(), doc.frame_time()));
                row(ui, sds_i18n::t!("Треки", "Tracks"), sds_i18n::tf!("{} костей, {} float", "{} bones, {} float", doc.info.transform_tracks, doc.info.float_tracks));
                row(ui, sds_i18n::t!("Треки аннотаций", "Annotation tracks"), doc.info.annotation_tracks.to_string());
                row(ui, sds_i18n::t!("Скелет", "Skeleton"), doc.info.skeleton_name.clone());
                row(ui, sds_i18n::t!("Аннотации", "Annotations"), format!("{}{}", doc.annos.len(), if doc.dirty() { sds_i18n::t!(" (изменено)", " (modified)") } else { "" }));
                if let Some(r) = &out_rel {
                    row(ui, sds_i18n::t!("Вывод", "Output"), r.clone());
                }
            });
        });
        ui.separator();
        annotation_editor(app, ui);
    });
}

fn annotation_editor(app: &mut StudioApp, ui: &mut egui::Ui) {
    let Some(i) = app.selected else {
        ui.label(sds_i18n::t!("Выберите аннотацию на таймлайне или в списке.\nДвойной клик по строке таймлайна — новая аннотация, ПКМ — шаблоны.", "Pick an annotation on the timeline or in the list.\nDouble-click a timeline row — new annotation, RMB — templates."));
        return;
    };
    let sdscombat = app.sdscombat_active();
    let snap_on = app.settings.snap_to_frames;
    let sim = crate::precision_view::simulate_doc(app);
    let pie_cue: Option<crate::pie::Cue> = match (&app.doc, &app.loaded) {
        (Some(d), Some(l)) => {
            let items: Vec<(f32, &str)> = d.annos.iter().map(|a| (a.time, a.text.as_str())).collect();
            crate::pie::resolve(&items, d.duration(), &l.pie, Some(&l.db)).into_iter().find(|c| c.index == i)
        }
        _ => None,
    };
    // Node suggestions: weapon nodes, bones with their own capsule, then the rest.
    let bones: Vec<(String, bool)> = match &app.character {
        Some(c) => {
            let mut v: Vec<(String, bool)> = vec![("WEAPON".into(), true), ("SHIELD".into(), true)];
            let mut rest = Vec::new();
            for b in &c.skeleton.bones {
                if c.capsules.contains_key(&b.to_ascii_lowercase()) {
                    v.push((b.clone(), true));
                } else {
                    rest.push((b.clone(), false));
                }
            }
            v.extend(rest);
            v
        }
        None => vec![("WEAPON".into(), true), ("SHIELD".into(), true)],
    };
    let sizes: Vec<(usize, String)> = match (&sim, &app.character, &app.loaded) {
        (Some(s), Some(ch), Some(l)) => s
            .collisions
            .iter()
            .map(|c| {
                let z = crate::precision_view::size(c, ch, &l.precision.0, app.scene.precision_player);
                let d = [z.attack.b[0] - z.attack.a[0], z.attack.b[1] - z.attack.a[1], z.attack.b[2] - z.attack.a[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                (c.add, sds_i18n::tf!("Капсула: длина {len:.1}, радиус {:.1}{}", "Capsule: length {len:.1}, radius {:.1}{}", z.attack.r, if z.approx { sds_i18n::t!(" (приблизительно)", " (approximate)") } else { "" }))
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut merge_key: Option<String> = None;
    let Some(doc) = &mut app.doc else { return };
    let Some(a) = doc.annos.get(i).cloned() else {
        app.selected = None;
        return;
    };
    let ft = doc.frame_time();
    let dur = doc.duration();
    let parsed = parse(&a.text);
    let color = category_color(parsed.category);
    ui.horizontal(|ui| {
        ui.colored_label(color, "◆");
        ui.heading(parsed.category.label());
    });

    let mut new_time: Option<f32> = None;
    let mut new_text: Option<String> = None;
    egui::Grid::new("anno-edit").num_columns(2).show(ui, |ui| {
        ui.label(sds_i18n::t!("Время", "Time"));
        let mut t = a.time;
        if ui.add(egui::DragValue::new(&mut t).speed(0.002).range(0.0..=dur).max_decimals(4).suffix(sds_i18n::t!(" с", " s"))).changed() {
            new_time = Some(t);
        }
        ui.end_row();
        ui.label(sds_i18n::t!("Кадр", "Frame"));
        let mut f = (a.time / ft).round() as i64;
        if ui.add(egui::DragValue::new(&mut f).range(0..=doc.last_frame() as i64)).changed() {
            new_time = Some((f as f32 * ft).min(dur));
        }
        ui.end_row();
        ui.label(sds_i18n::t!("Трек", "Track"));
        ui.label(a.track.to_string());
        ui.end_row();
    });
    if !snap_on || (a.time / ft - (a.time / ft).round()).abs() > 0.01 {
        ui.small(sds_i18n::tf!("между кадрами: {:.2}", "between frames: {:.2}", a.time / ft));
    }

    ui.label(sds_i18n::t!("Текст", "Text"));
    let id = ui.id().with(("anno-text", i, &a.text));
    let mut buf: String = ui.data(|d| d.get_temp(id)).unwrap_or_else(|| a.text.clone());
    let r = ui.add(egui::TextEdit::multiline(&mut buf).desired_rows(2).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
    ui.data_mut(|d| d.insert_temp(id, buf.clone()));
    let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
    if (r.lost_focus() || enter) && buf.trim() != a.text {
        new_text = Some(buf.trim().to_string());
    }
    ui.small(sds_i18n::t!("Enter или уход фокуса — применить", "Enter or leaving the field — apply"));

    ui.separator();
    // Typed fields.
    match &parsed.kind {
        Kind::ScarAction(s) => {
            let mut s = s.clone();
            let mut ch = false;
            egui::Grid::new("scar").num_columns(2).show(ui, |ui| {
                ui.label("IdleAnimation");
                ch |= ui.text_edit_singleline(&mut s.idle).lost_focus();
                ui.end_row();
                ui.label("Type");
                ch |= ui.text_edit_singleline(&mut s.ty).lost_focus();
                ui.end_row();
                for (label, v) in [("MinDistance", &mut s.min_distance), ("MaxDistance", &mut s.max_distance), ("StartAngle", &mut s.start_angle), ("EndAngle", &mut s.end_angle), ("Chance", &mut s.chance)] {
                    ui.label(label);
                    ch |= ui.add(egui::DragValue::new(v).speed(1.0)).changed();
                    ui.end_row();
                }
            });
            if ch {
                new_text = Some(s.to_text());
            }
            ui.small(sds_i18n::t!("Полный редактор, круг и проверка цели — вкладка SCAR", "Full editor, circle and target check — the SCAR tab"));
        }
        Kind::SdsNpc { event, values } => {
            let names: &[&str] = match event.to_ascii_lowercase().as_str() {
                "sds_npc_reachtarget" => &["ref", "min", "max", "arrive"],
                "sds_npc_targetlockon" | "sds_npc_turnspeed" => &[sds_i18n::t!("сила 0..1", "strength 0..1")],
                _ => &[],
            };
            let mut vals = values.clone();
            if vals.len() < names.len() {
                vals.resize(names.len(), 0.0);
            }
            let mut ch = false;
            egui::Grid::new("sds").num_columns(2).show(ui, |ui| {
                for (k, n) in names.iter().enumerate() {
                    ui.label(*n);
                    ch |= ui.add(egui::DragValue::new(&mut vals[k]).speed(if names.len() == 1 { 0.01 } else { 1.0 })).changed();
                    ui.end_row();
                }
            });
            if ch {
                let v: Vec<String> = vals.iter().map(|x| trim_float(*x)).collect();
                new_text = Some(format!("{event}.{}", v.join("|")));
            }
            if event.eq_ignore_ascii_case("SDS_NPC_BumpOff") {
                ui.small(sds_i18n::t!(
                    "Отключает «магнетизм» движения Animation Motion Fix (AMF_bForbidMoveMagnetism): NPC проходит сквозь цель, а не упирается в её коллизию. До SDS_NPC_BumpOn или конца клипа.",
                    "Turns off Animation Motion Fix's movement magnetism (AMF_bForbidMoveMagnetism): the NPC passes its target instead of sticking on its collision. Until SDS_NPC_BumpOn or the clip end."
                ));
            }
            if event.eq_ignore_ascii_case("SDS_NPC_BumpOn") {
                ui.small(sds_i18n::t!("Возвращает магнетизм движения (после SDS_NPC_BumpOff).", "Brings movement magnetism back (after SDS_NPC_BumpOff)."));
            }
            if event.eq_ignore_ascii_case("SDS_NPC_ReachTarget") {
                ui.small(sds_i18n::t!("k = clamp(D − arrive, min, max) / ref; ref/min/max × масштаб NPC", "k = clamp(D − arrive, min, max) / ref; ref/min/max × NPC scale"));
            }
            if !sdscombat {
                ui.colored_label(Color32::from_rgb(220, 160, 80), sds_i18n::t!("SDSCombat.dll не найден в сборке (или выключен в «Сцене») — в предпросмотре игнорируется", "SDSCombat.dll not found in the build (or turned off in «Scene») — ignored in the preview"));
            }
        }
        Kind::Precision { event, params } => {
            if let Some((text, key)) = precision_form(ui, i, event, params, &a.text, &bones) {
                new_text = Some(text);
                merge_key = key;
            }
            for (_, s) in sizes.iter().filter(|(add, _)| *add == i) {
                ui.label(s);
            }
            if let Some(sim) = &sim {
                for c in sim.collisions.iter().filter(|c| c.add == i) {
                    ui.label(sds_i18n::tf!("Коллизия {:.3}–{:.3} с, конец: {}", "Collision {:.3}–{:.3} s, end: {}", c.start, c.end, c.end_reason));
                    match c.recoil_end {
                        Some(r) => ui.label(sds_i18n::tf!("Капсула отдачи до {r:.3} с", "Recoil capsule until {r:.3} s")),
                        None => ui.label(sds_i18n::t!("Без капсулы отдачи", "No recoil capsule")),
                    };
                }
                for n in sim.notes.iter().filter(|n| n.index == i) {
                    ui.colored_label(Color32::from_rgb(240, 160, 80), &n.text);
                }
            }
        }
        Kind::Pie { command, args } => {
            if !command.starts_with('$') {
                // A direct instruction: every parameter named and editable.
                if let Some(text) = pie_form(ui, i, command, args) {
                    new_text = Some(text);
                    merge_key = Some(format!("pie:{i}"));
                }
            } else {
                ui.label(sds_i18n::tf!("Алиас: {command}", "Alias: {command}"));
            }
            match &pie_cue {
                Some(c) => {
                    if let Some(f) = &c.alias_file {
                        ui.small(sds_i18n::tf!("из {f}", "from {f}"));
                    }
                    if command.starts_with('$') {
                        for ins in &c.instructions {
                            ui.separator();
                            if ins.delay > 0.0 {
                                ui.small(sds_i18n::tf!("через {:.2} с", "after {:.2} s", ins.delay));
                            }
                            pie_labels(ui, &ins.command);
                        }
                    }
                    for s in &c.spells {
                        ui.label(format!("{} {} «{}»", s.sig, s.edid, s.name));
                    }
                    for p in &c.pieces {
                        let at = match &p.anchor {
                            crate::pie::Anchor::Node(n) => n.clone(),
                            crate::pie::Anchor::Target => sds_i18n::t!("цель", "target").into(),
                        };
                        ui.label(sds_i18n::tf!("• {}: {} → {} ({:.2}–{:.2} с)", "• {}: {} → {} ({:.2}–{:.2} s)", p.slot.label(), p.model, at, p.start, p.start + p.life)).on_hover_text(&p.effect);
                    }
                    for s in &c.sounds {
                        ui.label(sds_i18n::tf!("🔊 {:.2} с — {}", "🔊 {:.2} s — {}", s.time, s.label));
                    }
                    for s in &c.spells {
                        for e in &s.silent {
                            ui.small(sds_i18n::tf!("без арта: {e}", "no art: {e}"));
                        }
                    }
                    for n in &c.notes {
                        ui.colored_label(Color32::from_rgb(230, 180, 90), n);
                    }
                    ui.small(sds_i18n::t!("Арт с эмиттером по мешу цели ставится на WEAPON и излучает с клинка.", "Art with a target-mesh emitter goes on WEAPON and emits from the blade."));
                }
                None => {
                    ui.small(sds_i18n::t!("Сборка не загружена — алиас не разобран", "Build not loaded — alias not resolved"));
                }
            }
        }
        Kind::AnimMotion(v) => {
            ui.label(sds_i18n::tf!("animmotion: x {:.3}, y {:.3}, z {:.3} (накопленное от начала клипа)", "animmotion: x {:.3}, y {:.3}, z {:.3} (accumulated from the clip start)", v[0], v[1], v[2]));
        }
        Kind::AnimRotation(d) => {
            ui.label(sds_i18n::tf!("animrotation: {d:+.3}° с начала клипа", "animrotation: {d:+.3}° since the clip start"));
        }
        Kind::Sound { descriptor } => {
            ui.label(sds_i18n::tf!("Дескриптор: {descriptor}", "Descriptor: {descriptor}"));
            match app.sound_info(descriptor) {
                Some(d) => {
                    egui::Grid::new(("snd", i)).num_columns(2).striped(true).show(ui, |ui| {
                        ui.label("SNDR");
                        ui.label(&d.edid);
                        ui.end_row();
                        ui.label(sds_i18n::t!("категория", "category"));
                        ui.label(format!("{} (×{:.2})", d.category, d.category_volume));
                        ui.end_row();
                        ui.label(sds_i18n::t!("ослабление", "attenuation"));
                        ui.label(sds_i18n::tf!("{:.2} дБ ± {:.0} дБ", "{:.2} dB ± {:.0} dB", d.attenuation_db, d.db_variance));
                        ui.end_row();
                        ui.label(sds_i18n::t!("высота", "pitch"));
                        ui.label(format!("{:+.0}% ± {:.0}%", d.freq_shift, d.freq_variance));
                        ui.end_row();
                        ui.label(sds_i18n::t!("цикл", "loop"));
                        ui.label(if d.looping { sds_i18n::t!("да", "yes") } else { sds_i18n::t!("нет", "no") });
                        ui.end_row();
                    });
                    ui.label(sds_i18n::tf!("Файлы ({}), берётся случайный:", "Files ({}), one picked at random:", d.files.len()));
                    for f in &d.files {
                        ui.small(f);
                    }
                    for n in &d.notes {
                        ui.small(n);
                    }
                }
                None if app.loaded.is_some() => {
                    ui.colored_label(Color32::from_rgb(230, 180, 90), sds_i18n::t!("В сборке нет SNDR / SOUN с таким EditorID", "No SNDR / SOUN with this EditorID in the build"));
                }
                None => {
                    ui.small(sds_i18n::t!("Сборка не загружена", "Build not loaded"));
                }
            }
            if ui.button(sds_i18n::t!("▶ Прослушать", "▶ Listen")).clicked() {
                let d = descriptor.clone();
                app.preview_sound(&d, None);
            }
        }
        Kind::Plain => {
            ui.label(sds_i18n::tf!("Событие: {}", "Event: {}", parsed.name));
            // Sounds the engine plays for this event (swing, footsteps).
            let name = parsed.name.clone();
            match crate::sound::engine_sound(app, &name) {
                Some(Ok((key, how))) => {
                    let edid = app.loaded.as_ref().and_then(|l| l.db.get(key)).map(|r| r.edid()).unwrap_or_default();
                    ui.label(sds_i18n::tf!("Звук движка: {}", "Engine sound: {}", edid));
                    ui.small(how);
                    if name.to_ascii_lowercase().starts_with("foot") {
                        ui.small(sds_i18n::tf!("поверхность: {} (вкладка «Сцена»)", "ground: {} (the «Scene» tab)", app.settings.ground));
                    }
                    if ui.button(sds_i18n::t!("▶ Прослушать", "▶ Listen")).clicked() {
                        app.preview_sound(&edid, Some(key));
                    }
                }
                Some(Err(why)) => {
                    ui.colored_label(Color32::from_rgb(230, 180, 90), why);
                }
                None => {}
            }
        }
    }

    ui.separator();
    ui.horizontal(|ui| {
        if ui.button(sds_i18n::t!("Удалить", "Delete")).clicked() {
            app.delete_selected();
        }
        if ui.button(sds_i18n::t!("Копировать", "Copy")).clicked() {
            app.copy_selected();
        }
        if ui.button(sds_i18n::t!("К плейхеду", "To playhead")).on_hover_text(sds_i18n::t!("Перенести на текущее время", "Move to the current time")).clicked() {
            new_time = Some(app.snap(app.play.time));
        }
    });

    if new_time.is_none() && new_text.is_none() {
        return;
    }
    let Some(doc) = &mut app.doc else { return };
    let apply = |list: &mut Vec<crate::doc::Anno>| {
        if let Some(t) = new_time {
            list[i].time = t;
        }
        if let Some(s) = new_text {
            list[i].text = s;
        }
    };
    app.selected = match &merge_key {
        Some(k) => doc.edit_merge(Some(i), k, apply),
        None => doc.edit(Some(i), apply),
    };
}

fn trim_float(x: f32) -> String {
    let s = format!("{x:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" { "0".into() } else { s.to_string() }
}

// ---- list -------------------------------------------------------------------

pub fn annotation_list(app: &mut StudioApp, ui: &mut egui::Ui) {
    let Some(doc) = &app.doc else {
        ui.label(sds_i18n::t!("Клип не открыт", "No clip open"));
        return;
    };
    let id = ui.id().with("hide-motion");
    let mut hide_motion: bool = ui.data(|d| d.get_temp(id)).unwrap_or(true);
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut app.list_filter).hint_text(sds_i18n::t!("фильтр…", "filter…")).desired_width(160.0));
        ui.checkbox(&mut hide_motion, sds_i18n::t!("скрыть animmotion", "hide animmotion"));
    });
    ui.data_mut(|d| d.insert_temp(id, hide_motion));
    let filter = app.list_filter.to_lowercase();
    let ft = doc.frame_time();
    let duration = doc.duration();
    let mut pick: Option<usize> = None;
    let favs = app.settings.favorites.clone();
    let mut popup = None;
    let mut add: Option<Vec<(String, f32)>> = None;
    let mut delete = false;
    let menu_t = app.snap(app.play.time);
    let selected = app.selected;
    let menu = |ui: &mut egui::Ui, add: &mut Option<Vec<(String, f32)>>, delete: &mut bool, popup: &mut Option<crate::timeline::Popup>| {
        if selected.is_some() {
            if ui.button(sds_i18n::t!("Удалить выбранную", "Delete selected")).clicked() {
                *delete = true;
                ui.close();
            }
            ui.separator();
        }
        if let Some(v) = crate::timeline::add_menu(ui, menu_t, ft, duration, &favs, popup) {
            *add = Some(v);
        }
    };
    // RMB on empty space of the list.
    let bg = ui.interact(ui.available_rect_before_wrap(), ui.id().with("anno-list-bg"), egui::Sense::click());
    bg.context_menu(|ui| menu(ui, &mut add, &mut delete, &mut popup));
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("anno-list").num_columns(3).striped(true).show(ui, |ui| {
            for (i, a) in doc.annos.iter().enumerate() {
                let p = parse(&a.text);
                if hide_motion && p.category == Category::Motion {
                    continue;
                }
                if !filter.is_empty() && !a.text.to_lowercase().contains(&filter) {
                    continue;
                }
                ui.label(format!("{:.3}", a.time));
                ui.label(format!("{}", (a.time / ft).round()));
                let sel = app.selected == Some(i);
                let text = egui::RichText::new(&a.text).color(category_color(p.category));
                let r = ui.selectable_label(sel, text);
                if r.clicked() || r.secondary_clicked() {
                    pick = Some(i);
                }
                r.context_menu(|ui| menu(ui, &mut add, &mut delete, &mut popup));
                ui.end_row();
            }
        });
    });
    if let Some(i) = pick {
        app.selected = Some(i);
        app.play.time = doc.annos[i].time;
        app.play.playing = false;
    }
    app.open_popup(popup);
    if delete {
        app.delete_selected();
    }
    if let Some(v) = add {
        app.add_annotations(v);
    }
}

// ---- scene ------------------------------------------------------------------

pub fn scene(app: &mut StudioApp, ui: &mut egui::Ui) {
    ui.heading(sds_i18n::t!("Слои", "Layers"));
    ui.checkbox(&mut app.scene.root_motion, "Root motion (animmotion/animrotation)");
    ui.checkbox(&mut app.scene.show_path, sds_i18n::t!("Путь NPC", "NPC path"));
    ui.checkbox(&mut app.scene.show_grid, sds_i18n::t!("Сетка", "Grid"));
    let has_dll = app.loaded.as_ref().is_some_and(|l| l.vfs.has_sdscombat());
    ui.add_enabled_ui(has_dll, |ui| {
        ui.checkbox(&mut app.scene.sdscombat, sds_i18n::t!("Учитывать аннотации SDSCombat", "Use SDSCombat annotations"));
    });
    ui.small(if has_dll { sds_i18n::t!("SKSE\\Plugins\\SDSCombat.dll найден в сборке", "SKSE\\Plugins\\SDSCombat.dll found in the build") } else { sds_i18n::t!("SDSCombat.dll в сборке нет — его аннотации игнорируются", "No SDSCombat.dll in the build — its annotations are ignored") });
    ui.separator();
    ui.heading(sds_i18n::t!("Звук", "Sound"));
    ui.horizontal(|ui| {
        ui.checkbox(&mut app.settings.sound_on, sds_i18n::t!("вкл", "on"));
        ui.add(egui::Slider::new(&mut app.settings.volume, 0.0..=100.0).suffix("%").fixed_decimals(0));
    });
    ui.horizontal(|ui| {
        ui.label(sds_i18n::t!("Поверхность для шагов", "Footstep ground"));
        let mats = app.loaded.as_ref().map(|l| l.db.materials()).unwrap_or_default();
        egui::ComboBox::from_id_salt("ground").selected_text(app.settings.ground.clone()).height(400.0).show_ui(ui, |ui| {
            for (edid, _) in mats.iter().filter(|(e, _)| !e.to_ascii_lowercase().contains("stairs")) {
                ui.selectable_value(&mut app.settings.ground, edid.clone(), edid);
            }
        });
    });
    ui.small(sds_i18n::t!(
        "weaponSwing — звук взмаха оружия NPC (WEAP TNAM), без оружия — WPNSwingUnarmed. Foot* — набор шагов обуви NPC (ARMA → FSTS), звук по тегу события и поверхности.",
        "weaponSwing — the NPC weapon's swing sound (WEAP TNAM), unarmed — WPNSwingUnarmed. Foot* — the NPC footwear's footstep set (ARMA → FSTS), sound by event tag and ground."
    ));
    ui.separator();
    ui.heading(sds_i18n::t!("Строки таймлайна", "Timeline rows"));
    for c in Category::ALL {
        let mut on = !app.scene.hidden.contains(&c);
        let text = egui::RichText::new(c.label()).color(category_color(c));
        if ui.checkbox(&mut on, text).changed() {
            if on {
                app.scene.hidden.remove(&c);
            } else {
                app.scene.hidden.insert(c);
            }
        }
    }
    ui.separator();
    ui.separator();
    sdscombat_scene(app, ui);
    ui.separator();
    ui.checkbox(&mut app.scene.show_precision, sds_i18n::t!("Хитбоксы Precision", "Precision hitboxes"));
    ui.horizontal(|ui| {
        ui.radio_value(&mut app.scene.precision_player, false, sds_i18n::t!("как NPC", "as NPC"));
        ui.radio_value(&mut app.scene.precision_player, true, sds_i18n::t!("как игрок", "as player"));
    });
    precision_summary(app, ui);
    ui.checkbox(&mut app.scene.show_fx, sds_i18n::t!("FX от PIE (арт заклинаний, частицы)", "PIE FX (spell art, particles)"));
    ui.checkbox(&mut app.scene.show_trails, sds_i18n::t!("Трейлы Precision", "Precision trails"));
    trails_summary(app, ui);
    ui.checkbox(&mut app.scene.show_scar, sds_i18n::t!("Зоны SCAR_ActionData в 3D (редактор — вкладка SCAR)", "SCAR_ActionData zones in 3D (editor — the SCAR tab)"));
}

// ---- log --------------------------------------------------------------------

pub fn log(app: &mut StudioApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
        for l in &app.log {
            ui.label(egui::RichText::new(l).monospace());
        }
    });
}

/// Collisions of the open clip with their sizes, and why some were ignored.
fn precision_summary(app: &mut StudioApp, ui: &mut egui::Ui) {
    let Some(sim) = crate::precision_view::simulate_doc(app) else { return };
    let settings = app.loaded.as_ref().map(|l| l.precision.clone()).unwrap_or_default();
    egui::CollapsingHeader::new(sds_i18n::tf!("Precision: {} коллизий, {} замечаний", "Precision: {} collisions, {} notes", sim.collisions.len(), sim.notes.len()))
        .default_open(true)
        .show(ui, |ui| {
            if crate::precision_view::is_power_attack(app) {
                ui.small(sds_i18n::t!("Силовая атака (в пути есть «power»): Duration(0) × множитель", "Power attack («power» in the path): Duration(0) × multiplier"));
            }
            for c in &sim.collisions {
                let size = app.character.as_ref().map(|ch| crate::precision_view::size(c, ch, &settings.0, app.scene.precision_player));
                let dims = match &size {
                    Some(s) => {
                        let d = [s.attack.b[0] - s.attack.a[0], s.attack.b[1] - s.attack.a[1], s.attack.b[2] - s.attack.a[2]];
                        sds_i18n::tf!("длина {:.1}, радиус {:.1}{}", "length {:.1}, radius {:.1}{}",
                            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(),
                            s.attack.r,
                            if s.approx { sds_i18n::t!(" (приблизительно: у кости нет капсулы)", " (approximate: the bone has no capsule)") } else { "" }
                        )
                    }
                    None => String::new(),
                };
                ui.label(sds_i18n::tf!("{} {:.3}–{:.3} с · {} · {}", "{} {:.3}–{:.3} s · {} · {}", c.params.node, c.start, c.end, c.end_reason, dims));
            }
            for n in &sim.notes {
                let t = app.doc.as_ref().and_then(|d| d.annos.get(n.index)).map(|a| format!("{:.3} {}", a.time, a.text)).unwrap_or_default();
                ui.colored_label(Color32::from_rgb(240, 160, 80), format!("{t}: {}", n.text));
            }
            if settings.1.is_empty() {
                ui.small(sds_i18n::t!("Настройки Precision: значения по умолчанию (ini в сборке не найдены)", "Precision settings: defaults (no ini found in the build)"));
            } else {
                ui.small(sds_i18n::tf!("Настройки: {}", "Settings: {}", settings.1.join(" → ")));
            }
            let s = &settings.0;
            ui.small(sds_i18n::tf!("fWeaponLengthMult {} · fWeaponCapsuleRadius {} · fMinWeaponLength {} · lifetime {} (×{} power) · отдача NPC {} / игрок {}", "fWeaponLengthMult {} · fWeaponCapsuleRadius {} · fMinWeaponLength {} · lifetime {} (×{} power) · recoil NPC {} / player {}",
                s.weapon_length_mult, s.weapon_capsule_radius, s.min_weapon_length, s.default_lifetime, s.lifetime_power_mult, s.recoil_npc, s.recoil_player
            ));
        });
}

/// Precision payload editor: event, then every parameter of the mod's
/// documentation with an on/off box. Returns the rebuilt text, and a merge
/// key when the change is one step of a continuous edit (dragging a number).
fn precision_form(
    ui: &mut egui::Ui,
    index: usize,
    event: &str,
    params: &[(String, String)],
    text: &str,
    bones: &[(String, bool)],
) -> Option<(String, Option<String>)> {
    use sds_anno::precision::{build, separator_of, ParamKind, EVENTS, PARAMS};
    let sep = separator_of(text);
    let mut ev = event.to_string();
    let mut list: Vec<(String, String)> = params.to_vec();
    let mut changed = false;
    let mut merge: Option<String> = None;

    ui.horizontal(|ui| {
        ui.label(sds_i18n::t!("Событие", "Event"));
        egui::ComboBox::from_id_salt(("prec-event", index)).selected_text(&ev).show_ui(ui, |ui| {
            for (name, help) in EVENTS {
                if ui.selectable_label(ev.eq_ignore_ascii_case(name), *name).on_hover_text(help.get()).clicked() {
                    ev = name.to_string();
                    changed = true;
                }
            }
        });
    });
    let lower = ev.to_ascii_lowercase();
    let is_add = lower == "collision_add";
    let is_remove = lower == "collision_remove";
    if let Some((_, help)) = EVENTS.iter().find(|(n, _)| n.eq_ignore_ascii_case(&ev)) {
        ui.small(help.get());
    }
    if !is_add && !is_remove {
        // Start/End/Clear/Cancel take no parameters.
        if changed {
            return Some((build(&ev, &[], sep), None));
        }
        return None;
    }
    let pos = |list: &Vec<(String, String)>, name: &str| list.iter().position(|(k, _)| k.eq_ignore_ascii_case(name));
    let floats = |v: &str| -> Vec<f32> { v.split(|c: char| c == ',' || c.is_whitespace()).filter_map(|x| x.trim().parse().ok()).collect() };

    egui::Grid::new(("prec-form", index)).num_columns(2).striped(true).show(ui, |ui| {
        for s in PARAMS {
            if s.add_only && !is_add {
                continue;
            }
            let at = pos(&list, s.name);
            let mut on = at.is_some();
            if ui.checkbox(&mut on, s.name).on_hover_text(s.help.get()).changed() {
                if on {
                    list.push((s.name.to_string(), if s.kind == ParamKind::Flag { String::new() } else { s.default.to_string() }));
                } else if let Some(k) = at {
                    list.remove(k);
                }
                changed = true;
            }
            let Some(k) = pos(&list, s.name) else {
                ui.end_row();
                continue;
            };
            let key = format!("prec:{index}:{}", s.name);
            let value = list[k].1.clone();
            let mut set = |v: String, list: &mut Vec<(String, String)>, merging: bool| {
                if list[k].1 != v {
                    list[k].1 = v;
                    changed = true;
                    if merging {
                        merge = Some(key.clone());
                    }
                }
            };
            match s.kind {
                ParamKind::Flag => {
                    ui.label(sds_i18n::t!("флаг", "flag"));
                }
                ParamKind::Node => {
                    ui.horizontal(|ui| {
                        let mut v = value.clone();
                        if ui.add(egui::TextEdit::singleline(&mut v).desired_width(170.0)).changed() {
                            set(v, &mut list, true);
                        }
                        ui.menu_button("▾", |ui| {
                            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                for (b, capsule) in bones {
                                    let label = if *capsule { format!("{b}  ●") } else { b.clone() };
                                    if ui.button(label).on_hover_text(if *capsule { sds_i18n::t!("есть своя капсула / размер оружия", "has its own capsule / weapon size") } else { sds_i18n::t!("капсулы нет — размер приблизительный", "no capsule — approximate size") }).clicked() {
                                        set(b.clone(), &mut list, false);
                                        ui.close();
                                    }
                                }
                            });
                        });
                    });
                }
                ParamKind::Int => {
                    let mut n: i64 = value.trim().parse().unwrap_or(0);
                    if ui.add(egui::DragValue::new(&mut n).speed(0.1)).changed() {
                        set(n.to_string(), &mut list, true);
                    }
                }
                ParamKind::Float => {
                    let mut x: f32 = value.trim().parse().unwrap_or(0.0);
                    let speed = if s.name.contains("Mult") || s.name == "Duration" || s.name == "Scale" { 0.01 } else { 0.5 };
                    if ui.add(egui::DragValue::new(&mut x).speed(speed).max_decimals(3)).changed() {
                        set(trim_float(x), &mut list, true);
                    }
                }
                ParamKind::Vec3 | ParamKind::Shake => {
                    let mut v = floats(&value);
                    v.resize(3, 0.0);
                    let labels: [&str; 3] = if s.kind == ParamKind::Shake { [sds_i18n::t!("сила ", "strength "), sds_i18n::t!("длит ", "dur "), sds_i18n::t!("част ", "freq ")] } else { ["x ", "y ", "z "] };
                    let mut ch = false;
                    ui.horizontal(|ui| {
                        for (j, l) in labels.iter().enumerate() {
                            ch |= ui.add(egui::DragValue::new(&mut v[j]).speed(if s.kind == ParamKind::Shake && j == 1 { 0.01 } else { 0.5 }).prefix(*l)).changed();
                        }
                    });
                    if ch {
                        set(v.iter().map(|x| trim_float(*x)).collect::<Vec<_>>().join(", "), &mut list, true);
                    }
                }
                ParamKind::Color4 => {
                    let mut v = floats(&value);
                    v.resize(4, 1.0);
                    let mut c = [v[0], v[1], v[2], v[3]];
                    if ui.color_edit_button_rgba_unmultiplied(&mut c).changed() {
                        set(c.iter().map(|x| trim_float(*x)).collect::<Vec<_>>().join(", "), &mut list, true);
                    }
                }
                ParamKind::Path => {
                    let mut v = value.clone();
                    if ui.add(egui::TextEdit::singleline(&mut v).desired_width(220.0)).changed() {
                        set(v, &mut list, true);
                    }
                }
            }
            ui.end_row();
        }
    });
    let unknown: Vec<&(String, String)> = list.iter().filter(|(k, _)| sds_anno::precision::spec(k).is_none()).collect();
    if !unknown.is_empty() {
        ui.small(sds_i18n::tf!("Неизвестные параметры сохранены как есть: {}", "Unknown parameters kept as is: {}", unknown.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if is_add && pos(&list, "Node").is_none() {
        ui.colored_label(Color32::from_rgb(240, 160, 80), sds_i18n::t!("Collision_Add без Node() Precision отклонит", "Precision rejects Collision_Add without Node()"));
    }
    ui.small(sds_i18n::tf!("Разделитель параметров: «{sep}» (как в исходном тексте)", "Parameter separator: «{sep}» (as in the source text)"));
    if !changed {
        return None;
    }
    Some((build(&ev, &list, sep), merge))
}

/// Target dummy and SDSCombat path settings, with what ReachTarget computed.
fn sdscombat_scene(app: &mut StudioApp, ui: &mut egui::Ui) {
    ui.heading(sds_i18n::t!("Цель и SDSCombat", "Target and SDSCombat"));
    egui::Grid::new("sds-scene").num_columns(2).show(ui, |ui| {
        ui.label(sds_i18n::t!("Цель (игрок)", "Target (player)"));
        ui.checkbox(&mut app.scene.show_target, sds_i18n::t!("показывать", "show"));
        ui.end_row();
        ui.label(sds_i18n::t!("Дистанция", "Distance"));
        ui.add(egui::DragValue::new(&mut app.scene.target_dist).speed(2.0).range(0.0..=5000.0).suffix(sds_i18n::t!(" ед", " u")));
        ui.end_row();
        ui.label(sds_i18n::t!("Угол", "Angle"));
        ui.add(egui::DragValue::new(&mut app.scene.target_angle).speed(1.0).range(-180.0..=180.0).suffix("°"))
            .on_hover_text(sds_i18n::t!("0 — прямо перед NPC в начале клипа, + — справа", "0 — straight ahead of the NPC at the clip start, + — to the right"));
        ui.end_row();
        ui.label(sds_i18n::t!("Поворот ИИ без лока", "AI turn without lock"));
        ui.add(egui::DragValue::new(&mut app.scene.ai_turn_speed).speed(1.0).range(0.0..=720.0).suffix(sds_i18n::t!(" °/с", " °/s")))
            .on_hover_text(sds_i18n::t!("Как быстро ИИ доворачивает на цель, когда лока нет (нет аннотаций или TargetLockOff). В игре зависит от ИИ и расы; 0 — не поворачивает.", "How fast the AI turns toward the target without a lock (no annotations or TargetLockOff). In game it depends on the AI and race; 0 — no turning."));
        ui.end_row();
    });
    ui.checkbox(&mut app.scene.compare_raw, sds_i18n::t!("Серым — путь без SDSCombat", "Grey — the path without SDSCombat"));
    if let Some(l) = &app.loaded {
        let s = &l.sdscombat.0;
        ui.small(format!(
            "{} · fMaxEngineTurnSpeed {} · fTurnSpeed {} · ReachTarget k {}..{} · AnimMotion {} · bHoldHeading {}",
            l.sdscombat.1.clone().unwrap_or_else(|| sds_i18n::t!("SDSCombat.ini не найден — значения по умолчанию", "SDSCombat.ini not found — defaults").into()),
            s.max_engine_turn_speed,
            s.turn_speed,
            s.reach_min_scale,
            s.reach_max_scale,
            if s.anim_motion { sds_i18n::t!("вкл", "on") } else { sds_i18n::t!("выкл", "off") },
            if s.hold_heading { sds_i18n::t!("вкл", "on") } else { sds_i18n::t!("выкл", "off") }
        ));
    }
    if !app.sdscombat_active() {
        ui.colored_label(Color32::from_rgb(220, 160, 80), sds_i18n::t!("Аннотации SDS_NPC_* сейчас не учитываются (нет DLL или выключено выше)", "SDS_NPC_* annotations are not applied now (no DLL or turned off above)"));
        return;
    }
    if let Some(path) = crate::sds_path::compute(app, false) {
        for e in &path.events {
            ui.small(sds_i18n::tf!("{:.3} с: {}", "{:.3} s: {}", e.t, e.text));
        }
        if let (Some(raw), Some(doc)) = (crate::sds_path::compute(app, true), &app.doc) {
            let (a, ha) = path.at(doc.duration());
            let (b, hb) = raw.at(doc.duration());
            ui.small(sds_i18n::tf!("Конец клипа: с SDSCombat ({:.0}, {:.0}) курс {:+.0}°, без — ({:.0}, {:.0}) курс {:+.0}°", "Clip end: with SDSCombat ({:.0}, {:.0}) heading {:+.0}°, without — ({:.0}, {:.0}) heading {:+.0}°",
                a[0], a[1], ha, b[0], b[1], hb
            ));
        }
    }
}

/// Which trail look the weapon in hand gets, and where the definitions came from.
fn trails_summary(app: &mut StudioApp, ui: &mut egui::Ui) {
    let Some(l) = &app.loaded else { return };
    let (ts, defs, files) = (&l.trails.0, &l.trails.1, &l.trails.2);
    egui::CollapsingHeader::new(sds_i18n::tf!("Трейлы: {} определений из toml", "Trails: {} definitions from toml", defs.len())).id_salt("trails-summary").show(ui, |ui| {
        ui.small(sds_i18n::tf!("MCM: показ {} · жизнь сегмента {:.2} с · угасание {:.2} с · {} сегм/с · длина {}", "MCM: display {} · segment life {:.2} s · fade {:.2} s · {} segm/s · length {}",
            if ts.enabled { sds_i18n::t!("вкл", "on") } else { sds_i18n::t!("выкл", "off") },
            ts.segment_lifetime,
            ts.fade_time,
            ts.segments_per_second,
            if ts.use_attack_length { sds_i18n::t!("по коллизии", "by collision") } else { sds_i18n::t!("по мешу оружия", "by weapon mesh") }
        ));
        for f in files {
            ui.small(f);
        }
        let weapon = app.npc.as_ref().and_then(|n| n.weapon.and_then(|i| n.look.weapons.get(i)));
        match weapon {
            Some(w) => {
                let facts = l.db.weapon_facts(w.key);
                ui.label(sds_i18n::tf!("Оружие: {} «{}»", "Weapon: {} «{}»", w.edid, facts.name));
                if !facts.enchantment.is_empty() {
                    ui.small(sds_i18n::tf!("зачарование «{}», эффект «{}» {:?}", "enchantment «{}», effect «{}» {:?}", facts.enchantment, facts.effect_name, facts.effect_keywords));
                }
                match crate::fx::pick_trail(defs, &facts) {
                    Some(d) => {
                        ui.small(sds_i18n::tf!("TrailDefinition из {}: цвет {:?}, ×{:?} яркость, ×{:?} жизнь, меш {:?}", "TrailDefinition from {}: colour {:?}, ×{:?} brightness, ×{:?} life, mesh {:?}", d.source, d.color, d.color_scale_mult, d.lifetime_mult, d.mesh));
                    }
                    None => {
                        ui.small(sds_i18n::t!("подходящего TrailDefinition нет — трейл по умолчанию", "no matching TrailDefinition — default trail"));
                    }
                }
            }
            None => {
                ui.small(sds_i18n::t!("Оружие не выбрано (NPC во вкладке «Персонаж») — трейл по умолчанию", "No weapon (NPC in the «Character» tab) — default trail"));
            }
        }
        ui.small(sds_i18n::t!("Параметры аннотации (TrailBaseColorOverride, TrailBaseColorScaleMult, TrailLifetimeMult, TrailMeshOverride, NoTrail, TrailUseTrueLength) важнее toml.", "Annotation parameters (TrailBaseColorOverride, TrailBaseColorScaleMult, TrailLifetimeMult, TrailMeshOverride, NoTrail, TrailUseTrueLength) take precedence over toml."));
    });
}

/// Read-only parameter list of one instruction, named per the PIE README.
fn pie_labels(ui: &mut egui::Ui, command: &str) {
    let parts: Vec<&str> = command.split('|').map(str::trim).collect();
    let op = parts.first().copied().unwrap_or("");
    ui.label(egui::RichText::new(op).strong());
    ui.small(crate::pie::command_help(op));
    let schema = crate::pie::schema(op);
    egui::Grid::new(("pie-labels", command)).num_columns(2).striped(true).show(ui, |ui| {
        for (k, v) in parts.iter().skip(1).enumerate() {
            let name = schema.and_then(|s| s.get(k)).map(|(n, _)| n.get()).unwrap_or(sds_i18n::t!("(лишний параметр)", "(extra parameter)"));
            ui.label(name);
            let shown = match schema.and_then(|s| s.get(k)).map(|(_, kind)| *kind) {
                Some(crate::pie::ArgKind::Bool) => format!("{v} ({})", if v.trim() == "1" { sds_i18n::t!("да", "yes") } else { sds_i18n::t!("нет", "no") }),
                Some(crate::pie::ArgKind::BodyPart) => {
                    let (n, node) = crate::pie::body_part(v.trim().parse().unwrap_or(-1));
                    format!("{v} — {n} ({node})")
                }
                _ => v.to_string(),
            };
            ui.label(shown);
            ui.end_row();
        }
    });
}

/// Editor of a direct `PIE.@COMMAND|...`: each parameter by its README type.
fn pie_form(ui: &mut egui::Ui, index: usize, command: &str, args: &[String]) -> Option<String> {
    use crate::pie::ArgKind;
    let (delay, op) = crate::pie::split_delay(command);
    ui.label(egui::RichText::new(op).strong());
    ui.small(crate::pie::command_help(op));
    let Some(schema) = crate::pie::schema(op) else {
        pie_labels(ui, &format!("{op}|{}", args.join("|")));
        return None;
    };
    let mut vals: Vec<String> = args.to_vec();
    if vals.len() < schema.len() {
        // Missing trailing parameters show as their neutral value.
        for (_, kind) in &schema[vals.len()..] {
            vals.push(match kind {
                ArgKind::Text | ArgKind::Plugin => String::new(),
                ArgKind::FormId => "0x000000".into(),
                ArgKind::Flags => "0U".into(),
                _ => "0".into(),
            });
        }
    }
    let mut changed = false;
    egui::Grid::new(("pie-form", index)).num_columns(2).striped(true).show(ui, |ui| {
        for (k, (name, kind)) in schema.iter().enumerate() {
            ui.label(name.get());
            let v = &mut vals[k];
            match kind {
                ArgKind::Bool => {
                    let mut b = v.trim() == "1";
                    if ui.checkbox(&mut b, "").changed() {
                        *v = if b { "1".into() } else { "0".into() };
                        changed = true;
                    }
                }
                ArgKind::Float => {
                    let mut x: f32 = v.trim().parse().unwrap_or(0.0);
                    if ui.add(egui::DragValue::new(&mut x).speed(0.01).max_decimals(3)).changed() {
                        *v = trim_float(x);
                        changed = true;
                    }
                }
                ArgKind::Int => {
                    let mut x: i64 = v.trim().parse().unwrap_or(0);
                    if ui.add(egui::DragValue::new(&mut x).speed(0.1)).changed() {
                        *v = x.to_string();
                        changed = true;
                    }
                }
                ArgKind::BodyPart => {
                    let mut x: i64 = v.trim().parse().unwrap_or(0);
                    let (n, node) = crate::pie::body_part(x);
                    ui.horizontal(|ui| {
                        if ui.add(egui::DragValue::new(&mut x).range(0..=41)).changed() {
                            *v = x.to_string();
                            changed = true;
                        }
                        ui.small(format!("{n} → {node}"));
                    });
                }
                _ => {
                    if ui.add(egui::TextEdit::singleline(v).desired_width(200.0)).changed() {
                        changed = true;
                    }
                }
            }
            ui.end_row();
        }
    });
    if vals.len() > schema.len() {
        ui.small(sds_i18n::tf!("лишние параметры: {}", "extra parameters: {}", vals[schema.len()..].join("|")));
    }
    if !changed {
        return None;
    }
    let prefix = if delay > 0.0 { format!("![{}]", trim_float(delay)) } else { String::new() };
    Some(format!("PIE.{prefix}{op}|{}", vals.join("|")))
}
