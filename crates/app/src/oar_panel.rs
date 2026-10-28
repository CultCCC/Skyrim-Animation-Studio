//! "OAR" tab: the selected submod's config (editable), who competes for the
//! open clip, and export into the output mod folder.

use crate::app::StudioApp;
use crate::doc::ClipSource;
use crate::oar::{competitors, export, locate, ExportSpec, OarMod, OarSub, Place};
use egui::Color32;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct OarEdit {
    pub folder: String,
    pub base: Value,
    pub orig_mod_folder: String,
    pub orig_sub_folder: String,
    pub name: String,
    pub description: String,
    pub priority: i64,
    pub disabled: bool,
    pub interruptible: bool,
    pub replace_on_loop: bool,
    pub replace_on_echo: bool,
    pub conditions_text: String,
    pub conditions_error: Option<String>,
    pub mod_folder: String,
    pub sub_folder: String,
    pub as_user_json: bool,
    pub copy_hkx: bool,
}

fn all_mods(app: &StudioApp) -> impl Iterator<Item = &OarMod> {
    app.oar_external.iter().chain(app.loaded.iter().flat_map(|l| l.library.oar.iter()))
}

pub fn find<'a>(app: &'a StudioApp, folder: &str) -> Option<(&'a OarMod, &'a OarSub)> {
    all_mods(app).find_map(|m| m.subs.iter().find(|s| s.folder == folder).map(|s| (m, s)))
}

pub fn select(app: &mut StudioApp, folder: &str) {
    let Some((m, s)) = find(app, folder) else { return };
    let c = &s.config;
    let b = |k: &str, d: bool| c.get(k).and_then(|v| v.as_bool()).unwrap_or(d);
    let conditions = c.get("conditions").cloned().unwrap_or(Value::Array(Vec::new()));
    app.oar_edit = Some(OarEdit {
        folder: s.folder.clone(),
        base: c.clone(),
        orig_mod_folder: m.folder_name.clone(),
        orig_sub_folder: s.folder_name.clone(),
        name: s.name.clone(),
        description: c.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        priority: s.priority,
        disabled: s.disabled,
        interruptible: b("interruptible", false),
        replace_on_loop: b("replaceOnLoop", true),
        replace_on_echo: b("replaceOnEcho", false),
        conditions_text: serde_json::to_string_pretty(&conditions).unwrap_or_default(),
        conditions_error: None,
        mod_folder: m.folder_name.clone(),
        sub_folder: s.folder_name.clone(),
        as_user_json: !m.external,
        copy_hkx: false,
    });
}

pub fn add_external(app: &mut StudioApp, dir: &Path) {
    match crate::oar::scan_disk(dir) {
        Ok(mods) => {
            let n: usize = mods.iter().map(|m| m.subs.len()).sum();
            app.log(sds_i18n::tf!("OAR с диска: {} — {} модов, {} сабмодов", "OAR from disk: {} — {} mods, {} submods", dir.display(), mods.len(), n));
            app.oar_external.retain(|m| !mods.iter().any(|x| x.folder == m.folder));
            app.oar_external.extend(mods);
            app.lib_mode_oar = true;
        }
        Err(e) => app.log(sds_i18n::tf!("OAR с диска: {e}", "OAR from disk: {e}")),
    }
}

/// One line per condition, nested groups indented.
fn condition_tree(ui: &mut egui::Ui, list: &Value, depth: usize) {
    let Some(arr) = list.as_array() else { return };
    for c in arr {
        let name = c.get("condition").and_then(|v| v.as_str()).unwrap_or("?");
        let neg = c.get("negated").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut params: Vec<String> = Vec::new();
        if let Some(o) = c.as_object() {
            for (k, v) in o {
                if matches!(k.as_str(), "condition" | "requiredVersion" | "negated" | "Conditions") {
                    continue;
                }
                let s = v.to_string();
                params.push(format!("{k}: {}", if s.chars().count() > 60 { format!("{}…", s.chars().take(60).collect::<String>()) } else { s }));
            }
        }
        let line = format!("{}{}{}{}", "    ".repeat(depth), if neg { sds_i18n::t!("НЕ ", "NOT ") } else { "" }, name, if params.is_empty() { String::new() } else { format!("  ({})", params.join(", ")) });
        ui.label(egui::RichText::new(line).monospace().color(if matches!(name, "AND" | "OR" | "XOR" | "PRESET") { Color32::from_rgb(140, 190, 255) } else { Color32::from_gray(210) }));
        if let Some(ch) = c.get("Conditions") {
            condition_tree(ui, ch, depth + 1);
        }
    }
}

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        submod(app, ui);
        ui.separator();
        competition(app, ui);
    });
}

/// Who replaces the open clip, by priority.
fn competition(app: &mut StudioApp, ui: &mut egui::Ui) {
    let Some(doc) = &app.doc else {
        ui.label(sds_i18n::t!("Откройте клип — здесь будет видно, какие OAR-сабмоды заменяют его анимацию и кто побеждает по приоритету.", "Open a clip — this shows which OAR submods replace its animation and who wins by priority."));
        return;
    };
    let path = match &doc.source {
        ClipSource::Vfs(p) => p.clone(),
        ClipSource::Disk(p) => p.to_string_lossy().replace('\\', "/").to_lowercase(),
    };
    let Some((project, own, rel)) = locate(&path) else {
        ui.label(sds_i18n::t!("Клип не в папке animations — сравнивать не с чем.", "The clip is not under animations — nothing to compare with."));
        return;
    };
    ui.heading(sds_i18n::t!("Кто заменяет этот клип", "Who replaces this clip"));
    ui.small(format!("{project} / {rel}"));
    let list = competitors(all_mods(app), &project, &rel);
    let base = format!("meshes/actors/{project}/animations/{rel}");
    let base_origin = app.loaded.as_ref().and_then(|l| l.vfs.origin(&base));
    let mut pick: Option<String> = None;
    let mut open: Option<Place> = None;
    let winner = list.iter().find(|c| !c.disabled).map(|c| c.folder.clone());
    let own_index = list.iter().position(|c| own.as_ref().is_some_and(|(m, s)| c.folder.to_lowercase().replace('\\', "/").ends_with(&format!("/{m}/{s}"))));
    if let Some(i) = own_index {
        ui.label(sds_i18n::tf!("Открытый сабмод: место {} из {} по приоритету", "Open submod: place {} of {} by priority", i + 1, list.len()));
    }
    let id = ui.id().with("oar-comp-all");
    let mut show_all: bool = ui.data(|d| d.get_temp(id)).unwrap_or(false);
    if list.len() > 10 {
        ui.checkbox(&mut show_all, sds_i18n::tf!("показать все {}", "show all {}", list.len()));
        ui.data_mut(|d| d.insert_temp(id, show_all));
    }
    egui::Grid::new("oar-comp").num_columns(4).striped(true).show(ui, |ui| {
        for (k, c) in list.iter().enumerate() {
            let is_own = own_index == Some(k);
            if !show_all && k >= 10 && !is_own {
                continue;
            }
            let mark = if Some(&c.folder) == winner.as_ref() { "★" } else { " " };
            ui.label(format!("{mark} {}", c.priority));
            let text = format!("{}{} / {}{}", if c.external { "📁 " } else { "" }, c.mod_name, c.sub_name, if c.disabled { sds_i18n::t!(" (выкл)", " (off)") } else { "" });
            let rt = if is_own { egui::RichText::new(text).strong().color(Color32::from_rgb(120, 220, 140)) } else { egui::RichText::new(text) };
            if ui.selectable_label(app.oar_edit.as_ref().is_some_and(|e| e.folder == c.folder), rt).clicked() {
                pick = Some(c.folder.clone());
            }
            if ui.small_button(sds_i18n::t!("открыть", "open")).clicked() {
                open = Some(c.place.clone());
            }
            ui.label(if is_own { sds_i18n::t!("← открыт", "← open") } else { "" });
            ui.end_row();
        }
    });
    ui.small(match base_origin {
        Some(o) => sds_i18n::tf!("Базовый клип без OAR: {base} ({o})", "Base clip without OAR: {base} ({o})"),
        None => sds_i18n::t!("Базового клипа с этим путём в сборке нет", "No base clip with this path in the build").into(),
    });
    ui.small(sds_i18n::t!("★ — наивысший приоритет среди включённых. В игре побеждает первый по приоритету сабмод, чьи условия выполнены; порядок загрузки модов не важен. Легаси-папки DAR здесь не показаны.", "★ — highest priority among the enabled ones. In game the first submod by priority whose conditions pass wins; mod load order does not matter. Legacy DAR folders are not shown here."));
    if let Some(f) = pick {
        select(app, &f);
    }
    match open {
        Some(Place::Vfs(p)) => app.defer(crate::app::Deferred::Open(crate::doc::ClipSource::Vfs(p))),
        Some(Place::Disk(p)) => app.defer(crate::app::Deferred::Open(crate::doc::ClipSource::Disk(p))),
        None => {}
    }
}

fn submod(app: &mut StudioApp, ui: &mut egui::Ui) {
    let Some(folder) = app.oar_edit.as_ref().map(|e| e.folder.clone()) else {
        ui.label(sds_i18n::t!("Сабмод не выбран: в библиотеке (режим OAR) нажмите «⚙ свойства и экспорт» или выберите строку выше.", "No submod selected: in the library (OAR mode) click «⚙ properties & export» or pick a row above."));
        return;
    };
    let Some((m, s)) = find(app, &folder) else {
        ui.label(sds_i18n::t!("Выбранный сабмод больше не найден (сборка перезагружена?)", "The selected submod is gone (build reloaded?)"));
        return;
    };
    let (mod_name, author, mod_desc, origin, has_user, nfiles, mod_config) =
        (m.name.clone(), m.author.clone(), m.description.clone(), s.config_origin.clone(), s.has_user, s.files.len(), m.config.clone());
    ui.heading(sds_i18n::t!("Сабмод", "Submod"));
    ui.label(format!("{}{}", mod_name, if author.is_empty() { String::new() } else { format!(" — {author}") }));
    if !mod_desc.is_empty() {
        ui.small(mod_desc);
    }
    ui.small(sds_i18n::tf!("{} · {} клипов · конфиг: {}", "{} · {} clips · config: {}", folder, nfiles, origin));
    if has_user {
        ui.colored_label(Color32::from_rgb(240, 180, 90), sds_i18n::t!("Действует user.json — он полностью заменяет config.json этого сабмода.", "A user.json is in effect — it fully replaces this submod's config.json."));
    }
    let e = app.oar_edit.as_mut().unwrap();

    egui::Grid::new("oar-sub").num_columns(2).show(ui, |ui| {
        ui.label("name");
        ui.text_edit_singleline(&mut e.name);
        ui.end_row();
        ui.label("description");
        ui.text_edit_singleline(&mut e.description);
        ui.end_row();
        ui.label("priority");
        ui.add(egui::DragValue::new(&mut e.priority).speed(1.0)).on_hover_text(sds_i18n::t!("Выше — побеждает. Держите приоритеты уникальными: при равных победитель не определён.", "Higher wins. Keep priorities unique: with equal ones the winner is undefined."));
        ui.end_row();
        ui.label("disabled");
        ui.checkbox(&mut e.disabled, "");
        ui.end_row();
        ui.label("interruptible");
        ui.checkbox(&mut e.interruptible, "").on_hover_text(sds_i18n::t!("Замену можно прервать посреди проигрывания и переоценить условия", "The replacement can be interrupted mid-playback and the conditions re-evaluated"));
        ui.end_row();
        ui.label("replaceOnLoop");
        ui.checkbox(&mut e.replace_on_loop, "").on_hover_text(sds_i18n::t!("Переоценивать условия на каждом цикле (по умолчанию да)", "Re-evaluate conditions on every loop (default yes)"));
        ui.end_row();
        ui.label("replaceOnEcho");
        ui.checkbox(&mut e.replace_on_echo, "").on_hover_text(sds_i18n::t!("Переоценивать условия на эхо", "Re-evaluate conditions on echo"));
        ui.end_row();
    });

    ui.label(sds_i18n::t!("Условия:", "Conditions:"));
    match serde_json::from_str::<Value>(&e.conditions_text) {
        Ok(v) => {
            e.conditions_error = None;
            if v.as_array().is_some_and(|a| a.is_empty()) {
                ui.small(sds_i18n::t!("условий нет — сабмод применяется всегда", "no conditions — the submod always applies"));
            }
            condition_tree(ui, &v, 0);
        }
        Err(err) => e.conditions_error = Some(err.to_string()),
    }
    egui::CollapsingHeader::new(sds_i18n::t!("JSON условий (правка)", "Conditions JSON (edit)")).id_salt(("oar-json", &folder)).show(ui, |ui| {
        ui.add(egui::TextEdit::multiline(&mut e.conditions_text).code_editor().desired_rows(10).desired_width(f32::INFINITY));
        ui.small(sds_i18n::t!("Верхний массив — `conditions`; внутри AND / OR / XOR / PRESET — `Conditions` с большой буквы.", "The top-level array is `conditions`; inside AND / OR / XOR / PRESET it is capital-C `Conditions`."));
    });
    if let Some(err) = &e.conditions_error {
        ui.colored_label(Color32::from_rgb(240, 110, 90), sds_i18n::tf!("JSON с ошибкой: {err}", "JSON error: {err}"));
    }

    ui.separator();
    ui.heading(sds_i18n::t!("Экспорт в папку вывода", "Export to the output folder"));
    ui.horizontal_wrapped(|ui| {
        if ui.button(sds_i18n::t!("Переопределить на месте (user.json)", "Override in place (user.json)")).on_hover_text(sds_i18n::t!("Тот же путь, только user.json: чужой мод не трогается, папку вывода включите в MO2 ниже/выше по нужде", "Same path, user.json only: the other mod is untouched; enable the output folder in MO2 below/above as needed")).clicked() {
            e.mod_folder = e.orig_mod_folder.clone();
            e.sub_folder = e.orig_sub_folder.clone();
            e.as_user_json = true;
            e.copy_hkx = false;
        }
        if ui.button(sds_i18n::t!("Новый сабмод", "New submod")).on_hover_text(sds_i18n::t!("Копия рядом с оригиналом под новым именем папки и с приоритетом выше", "A copy next to the original under a new folder name and a higher priority")).clicked() {
            e.mod_folder = e.orig_mod_folder.clone();
            e.sub_folder = format!("{} SDS", e.orig_sub_folder);
            e.priority += 1;
            e.as_user_json = false;
            e.copy_hkx = true;
        }
        if ui.button(sds_i18n::t!("Новый OAR-мод", "New OAR mod")).on_hover_text(sds_i18n::t!("Сабмод в отдельном OAR-моде «SDS Anim Studio»", "A submod in a separate OAR mod «SDS Anim Studio»")).clicked() {
            e.mod_folder = "SDS Anim Studio".into();
            e.as_user_json = false;
            e.copy_hkx = true;
        }
    });
    egui::Grid::new("oar-export").num_columns(2).show(ui, |ui| {
        ui.label(sds_i18n::t!("Папка OAR-мода", "OAR mod folder"));
        ui.text_edit_singleline(&mut e.mod_folder);
        ui.end_row();
        ui.label(sds_i18n::t!("Папка сабмода", "Submod folder"));
        ui.text_edit_singleline(&mut e.sub_folder);
        ui.end_row();
        ui.label(sds_i18n::t!("Файл конфига", "Config file"));
        ui.horizontal(|ui| {
            ui.radio_value(&mut e.as_user_json, false, "config.json");
            ui.radio_value(&mut e.as_user_json, true, "user.json");
        });
        ui.end_row();
        ui.label(sds_i18n::t!("Клипы .hkx", ".hkx clips"));
        ui.checkbox(&mut e.copy_hkx, sds_i18n::t!("копировать (с правками из студии)", "copy (with the studio's edits)"));
        ui.end_row();
    });
    let same_place = e.mod_folder.eq_ignore_ascii_case(&e.orig_mod_folder) && e.sub_folder.eq_ignore_ascii_case(&e.orig_sub_folder);
    if e.as_user_json && !same_place {
        ui.colored_label(Color32::from_rgb(240, 180, 90), sds_i18n::t!("user.json имеет смысл только на месте оригинала (те же папки).", "user.json only makes sense in place of the original (same folders)."));
    }
    let out_root = PathBuf::from(&app.settings.output_dir);
    let dest_hint = out_root.join("meshes").join("actors").join(find(app, &folder).map(|(_, s)| s.project.clone()).unwrap_or_default()).join("animations").join("OpenAnimationReplacer");
    let e = app.oar_edit.as_ref().unwrap();
    ui.small(format!("→ {}\\{}\\{}", dest_hint.display(), e.mod_folder, e.sub_folder));
    // Same priority as another submod for the same clips is ambiguous in OAR.
    // Only submods that replace one of the same clips compete.
    let own_files: Vec<(String, String)> = find(app, &folder).map(|(_, s)| s.files.iter().map(|(r, _)| (s.project.clone(), r.clone())).collect()).unwrap_or_default();
    let dup: Vec<String> = all_mods(app)
        .flat_map(|m| m.subs.iter().map(move |s| (m, s)))
        .filter(|(_, s)| s.priority == e.priority && s.folder != e.folder)
        .filter(|(_, s)| s.files.iter().any(|(r, _)| own_files.iter().any(|(p, o)| *p == s.project && o == r)))
        .map(|(m, s)| format!("{} / {}", m.name, s.name))
        .take(5)
        .collect();
    if !dup.is_empty() {
        ui.colored_label(Color32::from_rgb(240, 180, 90), sds_i18n::tf!("Тот же приоритет у сабмодов с общими клипами — победитель не определён: {}", "Submods sharing clips have the same priority — the winner is undefined: {}", dup.join(", ")));
    }
    let can = e.conditions_error.is_none();
    if ui.add_enabled(can, egui::Button::new(sds_i18n::t!("Экспортировать", "Export"))).clicked() {
        run_export(app, &folder, mod_config);
    }
}

fn run_export(app: &mut StudioApp, folder: &str, mod_config: Option<Value>) {
    let Some((m, s)) = find(app, folder) else { return };
    let sub = s.clone();
    let orig_mod_name = m.name.clone();
    let e = app.oar_edit.as_ref().unwrap();
    let mut config = e.base.clone();
    if !config.is_object() {
        config = Value::Object(Default::default());
    }
    config["name"] = e.name.clone().into();
    if e.description.is_empty() {
        if let Some(o) = config.as_object_mut() {
            o.remove("description");
        }
    } else {
        config["description"] = e.description.clone().into();
    }
    config["priority"] = e.priority.into();
    config["disabled"] = e.disabled.into();
    config["interruptible"] = e.interruptible.into();
    config["replaceOnLoop"] = e.replace_on_loop.into();
    config["replaceOnEcho"] = e.replace_on_echo.into();
    config["conditions"] = serde_json::from_str(&e.conditions_text).unwrap_or(Value::Array(Vec::new()));
    let new_mod = !e.mod_folder.eq_ignore_ascii_case(&e.orig_mod_folder);
    let same_place = !new_mod && e.sub_folder.eq_ignore_ascii_case(&e.orig_sub_folder);
    let mod_cfg = if new_mod {
        Some(serde_json::json!({
            "name": e.mod_folder.clone(),
            "author": mod_config.as_ref().and_then(|c| c.get("author")).cloned().unwrap_or(Value::String(String::new())),
            "description": sds_i18n::tf!("Экспорт SDS Anim Studio из «{orig_mod_name}»", "SDS Anim Studio export of «{orig_mod_name}»"),
        }))
    } else {
        None
    };
    let spec = ExportSpec { mod_folder: e.mod_folder.clone(), sub_folder: e.sub_folder.clone(), as_user_json: e.as_user_json, copy_hkx: e.copy_hkx, config, mod_config: mod_cfg };

    // Clips edited in the studio: the open one (unsaved edits included) and
    // ones already saved into the output folder at their original path.
    let out_root = PathBuf::from(&app.settings.output_dir);
    let vfs = app.loaded.as_ref().map(|l| l.vfs.clone());
    let doc_edit: Option<(Place, Vec<sds_hkx::Annotation>)> = app.doc.as_ref().filter(|d| d.dirty()).map(|d| {
        let place = match &d.source {
            ClipSource::Vfs(p) => Place::Vfs(p.clone()),
            ClipSource::Disk(p) => Place::Disk(p.clone()),
        };
        (place, d.annos.iter().map(|a| sds_hkx::Annotation { time: a.time, text: a.text.clone(), track: a.track }).collect())
    });
    let files = sub.files.clone();
    let edited = |rel: &str| -> Option<Vec<u8>> {
        let (_, place) = files.iter().find(|(r, _)| r == rel)?;
        if let Some((p, annos)) = &doc_edit {
            if p == place {
                let original = place.read(vfs.as_deref()).ok()?;
                return sds_hkx::write::write_annotations(&original, annos).ok();
            }
        }
        if let Place::Vfs(p) = place {
            let saved = out_root.join(p.replace('/', std::path::MAIN_SEPARATOR_STR));
            if saved.is_file() {
                return std::fs::read(saved).ok();
            }
        }
        None
    };
    match export(&sub, &spec, &out_root, vfs.as_deref(), &edited) {
        Ok(r) => {
            app.log(sds_i18n::tf!("OAR экспорт: {} — записано {} файлов ({} с правками студии){}", "OAR export: {} — {} files written ({} with studio edits){}",
                r.dest.display(),
                r.written,
                r.edited,
                if r.failed.is_empty() { String::new() } else { sds_i18n::tf!(", ошибок {}", ", {} errors", r.failed.len()) }
            ));
            for f in r.failed {
                app.log(format!("  {f}"));
            }
            if !spec.copy_hkx && !same_place {
                app.log(sds_i18n::t!("  Внимание: config.json без .hkx — сабмод на новом месте ничего не заменит (нужны клипы или overrideAnimationsFolder)", "  Warning: config.json without .hkx — the submod in its new place replaces nothing (needs clips or overrideAnimationsFolder)"));
            }
        }
        Err(e) => app.log(sds_i18n::tf!("OAR экспорт не удался: {e}", "OAR export failed: {e}")),
    }
}
