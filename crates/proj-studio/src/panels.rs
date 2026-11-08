//! Menu, status bar, config list, centre tabs and small windows.

use crate::app::{fmt_name, App, Center};
use crate::settings::ExportFormat;
use crate::theme;
use egui::{Color32, RichText};
use sds_i18n::{t, tf};
use sds_proj::model::Section;
use sds_proj::validate::Level;
use sds_proj::Sel;

pub fn menu_bar(app: &mut App, ui: &mut egui::Ui) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button(t!("Файл", "File"), |ui| {
            if ui.button(t!("Новый конфиг…", "New config…")).clicked() {
                app.new_open = Some("SDS_NewProjectiles.json".into());
                ui.close();
            }
            if ui.button(t!("Открыть JSON с диска…", "Open JSON from disk…")).clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() {
                    app.open_disk(p);
                }
                ui.close();
            }
            ui.separator();
            let has = app.doc().is_some();
            if ui.add_enabled(has, egui::Button::new(t!("Экспорт в Output-мод (Ctrl+S)", "Export to the Output mod (Ctrl+S)"))).clicked() {
                let c = app.cur;
                app.export(c, None);
                ui.close();
            }
            if ui.add_enabled(has, egui::Button::new(t!("Экспорт как…", "Export as…"))).clicked() {
                let name = app.doc().map(|d| d.name.clone()).unwrap_or_default();
                if let Some(p) = rfd::FileDialog::new().add_filter("JSON", &["json"]).set_file_name(&name).save_file() {
                    let c = app.cur;
                    app.export(c, Some(p));
                }
                ui.close();
            }
            ui.separator();
            if ui.button(t!("Открыть папку Output", "Open the Output folder")).clicked() {
                let d = app.settings.out_dir();
                let _ = std::fs::create_dir_all(&d);
                let _ = std::process::Command::new("explorer").arg(d).spawn();
                ui.close();
            }
        });
        ui.menu_button(t!("Правка", "Edit"), |ui| {
            let (cu, cr) = app.doc().map(|d| (d.can_undo(), d.can_redo())).unwrap_or((false, false));
            if ui.add_enabled(cu, egui::Button::new(t!("Отменить (Ctrl+Z)", "Undo (Ctrl+Z)"))).clicked() {
                if let Some(d) = app.doc_mut() {
                    d.undo();
                }
            }
            if ui.add_enabled(cr, egui::Button::new(t!("Повторить (Ctrl+Y)", "Redo (Ctrl+Y)"))).clicked() {
                if let Some(d) = app.doc_mut() {
                    d.redo();
                }
            }
        });
        if ui.button(t!("Загрузка…", "Load order…")).clicked() {
            app.settings_open = true;
        }
        ui.separator();
        ui.label(t!("Формат экспорта:", "Export format:"));
        let auto_label = match app.world.as_ref().and_then(|w| w.dll_format) {
            Some(f) => tf!("Авто — как у DLL: {}", "Auto — as the DLL: {}", fmt_name(f)),
            None => t!("Авто — как в файле", "Auto — as the file").to_string(),
        };
        let cur = match app.settings.export_format {
            ExportFormat::Auto => auto_label.clone(),
            ExportFormat::Object => fmt_name(sds_proj::model::CondFormat::Object).to_string(),
            ExportFormat::Array => fmt_name(sds_proj::model::CondFormat::Array).to_string(),
        };
        egui::ComboBox::from_id_salt("export-fmt").selected_text(cur).width(260.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut app.settings.export_format, ExportFormat::Auto, auto_label);
            ui.selectable_value(&mut app.settings.export_format, ExportFormat::Object, fmt_name(sds_proj::model::CondFormat::Object));
            ui.selectable_value(&mut app.settings.export_format, ExportFormat::Array, fmt_name(sds_proj::model::CondFormat::Array));
        })
        .response
        .on_hover_text(t!(
            "Как записывать conditions у триггеров. Установленная в SDS DLL (апрель 2024) понимает только «объект»: {\"ProjBaseIsFormID\": \"key_x\"}. Версия с GitHub читает «массив» с invert/OR.",
            "How trigger conditions are written. The DLL SDS ships (April 2024) reads only the object form: {\"ProjBaseIsFormID\": \"key_x\"}. The GitHub build reads the array form with invert/OR."
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut en = sds_i18n::en();
            if ui.selectable_label(en, "EN").clicked() {
                en = true;
            }
            if ui.selectable_label(!en, "RU").clicked() {
                en = false;
            }
            let lang = if en { "en" } else { "ru" };
            if app.settings.language != lang {
                app.settings.language = lang.into();
                app.settings.apply_language();
            }
        });
    });
}

pub fn status_bar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if app.is_loading() {
            ui.spinner();
            ui.label(t!("Чтение профиля MO2…", "Reading the MO2 profile…"));
        } else if let Some(l) = app.log.last() {
            ui.label(RichText::new(l).color(theme::DIM));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(d) = app.doc() {
                let (e, w) = (d.issues.iter().filter(|i| i.level == Level::Error).count(), d.issues.iter().filter(|i| i.level == Level::Warn).count());
                let txt = tf!("ошибок {e} · предупреждений {w}", "errors {e} · warnings {w}");
                let col = if e > 0 { theme::ERR } else if w > 0 { theme::WARN } else { theme::OK };
                if ui.link(RichText::new(txt).color(col)).clicked() {
                    app.center = Center::Issues;
                }
            }
        });
    });
}

pub fn file_panel(app: &mut App, ui: &mut egui::Ui) {
    ui.heading(t!("Конфиги загрузки", "Load-order configs"));
    ui.label(RichText::new("Data/HomingProjectiles/*.json").small().color(theme::DIM));
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut app.file_filter).hint_text(t!("фильтр", "filter")).desired_width(170.0));
        if ui.button("+").on_hover_text(t!("Новый конфиг", "New config")).clicked() {
            app.new_open = Some("SDS_NewProjectiles.json".into());
        }
    });
    ui.separator();
    let Some(w) = app.world.clone() else {
        if app.is_loading() {
            ui.spinner();
        } else if ui.button(t!("Выбрать профиль MO2…", "Choose an MO2 profile…")).clicked() {
            app.settings_open = true;
        }
        return;
    };
    let filter = app.file_filter.to_lowercase();
    let out_dir = app.settings.out_dir();
    egui::ScrollArea::vertical().id_salt("files").max_height(ui.available_height() * 0.55).show(ui, |ui| {
        for f in &w.configs {
            if !filter.is_empty() && !f.name.to_lowercase().contains(&filter) && !f.origin.to_lowercase().contains(&filter) {
                continue;
            }
            let open = app.docs.iter().position(|d| d.source == f.vfs_path);
            let dirty = open.is_some_and(|i| app.docs[i].dirty());
            let exported = out_dir.join(&f.name).is_file();
            let sel = open == Some(app.cur) && open.is_some();
            let mut text = RichText::new(format!("{}{}", f.name, if dirty { " •" } else { "" }));
            if exported {
                text = text.color(theme::OK);
            }
            let r = ui.selectable_label(sel, text).on_hover_text(tf!(
                "мод: {}{}",
                "mod: {}{}",
                f.origin,
                if exported { t!("\nесть экспорт в Output — откроется он", "\nan Output export exists — it opens instead") } else { "" }
            ));
            if r.clicked() {
                app.open_config(&f.vfs_path.clone());
            }
        }
    });
    ui.separator();
    // Outline of the open config.
    let Some(d) = app.doc() else {
        ui.label(RichText::new(t!("Открой конфиг слева или создай новый.", "Open a config above or create a new one.")).color(theme::DIM));
        return;
    };
    ui.label(RichText::new(&d.name).strong());
    let cfg = d.cfg.clone();
    let cur_sel = d.sel.clone();
    let mut nav = None;
    egui::ScrollArea::vertical().id_salt("outline").show(ui, |ui| {
        let head = |ui: &mut egui::Ui, title: &str, n: usize| {
            ui.label(RichText::new(format!("{title} ({n})")).color(theme::DIM).small());
        };
        head(ui, t!("Триггеры", "Triggers"), cfg.triggers.len());
        for (i, t) in cfg.triggers.iter().enumerate() {
            let s = Sel::Trigger(i);
            let label = format!("#{} {}", i + 1, sds_proj::chain::event_label(&t.event));
            if ui.selectable_label(cur_sel.owner() == &s, RichText::new(label).color(theme::kind_color(sds_proj::chain::Kind::Trigger))).clicked() {
                nav = Some(s);
            }
        }
        let sections: [(Section, &str, sds_proj::chain::Kind); 6] = [
            (Section::Homing, t!("Самонаведение (HomingData)", "Homing (HomingData)"), sds_proj::chain::Kind::Homing),
            (Section::Multicast, t!("Мультикасты (MulticastData)", "Multicasts (MulticastData)"), sds_proj::chain::Kind::Multicast),
            (Section::SpawnGroups, t!("Группы спавна (SpawnGroups)", "Spawn groups"), sds_proj::chain::Kind::McItem),
            (Section::Emitters, t!("Эмиттеры (EmittersData)", "Emitters (EmittersData)"), sds_proj::chain::Kind::Emitter),
            (Section::Followers, t!("Спутники (FollowersData)", "Followers (FollowersData)"), sds_proj::chain::Kind::Follower),
            (Section::FormIds, "FormIDs", sds_proj::chain::Kind::Projectile),
        ];
        for (sec, title, kind) in sections {
            let keys = cfg.keys(sec);
            if keys.is_empty() {
                continue;
            }
            ui.add_space(4.0);
            head(ui, title, keys.len());
            for k in keys {
                let s = sds_proj::validate::sel_of(sec, &k);
                let used = if sec == Section::FormIds { 1 } else { cfg.ref_count(sec, &k) };
                let mut txt = RichText::new(&k).color(theme::kind_color(kind));
                if used == 0 {
                    txt = txt.italics().color(theme::DIM);
                }
                let selected = match (&cur_sel, &s) {
                    (Sel::McItem(a, _), Sel::Multicast(b)) => a == b,
                    (Sel::EmFunc(a, _), Sel::Emitter(b)) => a == b,
                    (x, y) => x.owner() == y,
                };
                if ui.selectable_label(selected, txt).clicked() {
                    nav = Some(s);
                }
            }
        }
    });
    if let Some(s) = nav {
        app.select(s);
    }
}

pub fn center(app: &mut App, ui: &mut egui::Ui) {
    // Open documents as tabs.
    ui.horizontal_wrapped(|ui| {
        let mut close = None;
        for i in 0..app.docs.len() {
            let d = &app.docs[i];
            let label = format!("{}{}", d.name, if d.dirty() { " •" } else { "" });
            if ui.selectable_label(i == app.cur, label).on_hover_text(format!("{}\n{}", d.source, d.origin)).clicked() {
                app.cur = i;
            }
            if ui.small_button("×").clicked() {
                close = Some(i);
            }
            ui.add_space(6.0);
        }
        if let Some(i) = close {
            if app.docs[i].dirty() {
                app.confirm_close = Some(i);
            } else {
                app.close_doc(i);
            }
        }
    });
    ui.separator();
    if app.doc().is_none() {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new(t!(
                "Выбери конфиг слева.\nКаждый JSON — это триггеры («когда»), которые превращают снаряд\nв самонаводящийся, эмиттер, спутника или запускают мультикаст.",
                "Pick a config on the left.\nEach JSON holds triggers («when») that turn a projectile into\na homing one, an emitter, a follower, or launch a multicast."
            ))
            .color(theme::DIM));
        });
        return;
    }
    ui.horizontal(|ui| {
        let n_issues = app.doc().map(|d| d.issues.iter().filter(|i| i.level != Level::Info).count()).unwrap_or(0);
        ui.selectable_value(&mut app.center, Center::Chains, t!("⛓ Цепочки", "⛓ Chains"));
        ui.selectable_value(&mut app.center, Center::Json, t!("{ } JSON экспорта", "{ } Export JSON"));
        ui.selectable_value(&mut app.center, Center::Issues, tf!("⚠ Проблемы ({n_issues})", "⚠ Problems ({n_issues})"));
    });
    ui.separator();
    match app.center {
        Center::Chains => crate::chain_view::show(app, ui),
        Center::Json => json_view(app, ui),
        Center::Issues => issues_view(app, ui),
    }
}

fn json_view(app: &mut App, ui: &mut egui::Ui) {
    let Some(d) = app.doc() else { return };
    let fmt = app.export_format(d);
    let text = d.cfg.to_json(fmt);
    let path = app.settings.out_dir().join(&d.name);
    ui.horizontal(|ui| {
        ui.label(tf!("Формат условий: {}", "Condition format: {}", fmt_name(fmt)));
        if ui.button(t!("Копировать", "Copy")).clicked() {
            ui.ctx().copy_text(text.clone());
        }
        if ui.button(t!("Экспорт", "Export")).on_hover_text(path.display().to_string()).clicked() {
            let c = app.cur;
            app.export(c, None);
        }
    });
    ui.label(RichText::new(tf!("→ {}", "→ {}", path.display())).small().color(theme::DIM));
    egui::ScrollArea::both().id_salt("json").auto_shrink([false, false]).show(ui, |ui| {
        let mut t = text.as_str();
        ui.add(egui::TextEdit::multiline(&mut t).code_editor().desired_width(f32::INFINITY));
    });
}

fn issues_view(app: &mut App, ui: &mut egui::Ui) {
    let Some(d) = app.doc() else { return };
    let issues = d.issues.clone();
    if issues.is_empty() {
        ui.label(RichText::new(t!("Проблем не найдено.", "No problems found.")).color(theme::OK));
        return;
    }
    let mut nav = None;
    egui::ScrollArea::vertical().id_salt("issues").auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("issues-grid").striped(true).num_columns(3).show(ui, |ui| {
            for i in &issues {
                let (txt, col) = match i.level {
                    Level::Error => (t!("ошибка", "error"), theme::ERR),
                    Level::Warn => (t!("внимание", "warning"), theme::WARN),
                    Level::Info => (t!("инфо", "info"), theme::DIM),
                };
                ui.label(RichText::new(txt).color(col));
                if ui.link(sel_name(&i.at)).clicked() {
                    nav = Some(i.at.clone());
                }
                ui.label(&i.msg);
                ui.end_row();
            }
        });
    });
    if let Some(s) = nav {
        app.select(s);
        app.center = Center::Chains;
    }
}

pub fn sel_name(s: &Sel) -> String {
    match s {
        Sel::Root => t!("файл", "file").into(),
        Sel::Trigger(i) => tf!("триггер #{}", "trigger #{}", i + 1),
        Sel::Func(o, i) => format!("{} · f{}", sel_name(o), i + 1),
        Sel::FormId(k) | Sel::Homing(k) | Sel::Group(k) | Sel::Multicast(k) | Sel::Emitter(k) | Sel::Follower(k) => k.clone(),
        Sel::McItem(k, i) | Sel::EmFunc(k, i) => format!("{k}[{i}]"),
        Sel::Record(r) => format!("{:06X}", r.id),
    }
}

pub fn settings_window(app: &mut App, ctx: &egui::Context) {
    if !app.settings_open {
        return;
    }
    let mut open = true;
    let mut load = false;
    egui::Window::new(t!("Порядок загрузки", "Load order")).open(&mut open).collapsible(false).default_width(560.0).show(ctx, |ui| {
        let s = &mut app.settings;
        ui.horizontal(|ui| {
            ui.radio_value(&mut s.use_mo2, true, t!("Профиль MO2", "MO2 profile"));
            ui.radio_value(&mut s.use_mo2, false, t!("Папка Data", "Data folder"));
        });
        egui::Grid::new("set").num_columns(2).show(ui, |ui| {
            if s.use_mo2 {
                ui.label(t!("Папка MO2", "MO2 folder"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut s.mo2_dir).desired_width(380.0));
                    if ui.button("…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            s.mo2_dir = p.display().to_string();
                        }
                    }
                });
                ui.end_row();
                ui.label(t!("Профиль", "Profile"));
                let profiles = sds_vfs::Mo2Instance::open(std::path::Path::new(&s.mo2_dir)).map(|m| m.profiles()).unwrap_or_default();
                egui::ComboBox::from_id_salt("profile").selected_text(&s.profile).width(380.0).show_ui(ui, |ui| {
                    for p in profiles {
                        ui.selectable_value(&mut s.profile, p.clone(), p);
                    }
                });
                ui.end_row();
            } else {
                ui.label("Data");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut s.data_dir).desired_width(380.0));
                    if ui.button("…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            s.data_dir = p.display().to_string();
                        }
                    }
                });
                ui.end_row();
            }
            ui.label(t!("Output-мод", "Output mod"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut s.output_dir).desired_width(380.0));
                if ui.button("…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        s.output_dir = p.display().to_string();
                    }
                }
            });
            ui.end_row();
        });
        ui.label(
            RichText::new(t!(
                "Экспорт пишется в <Output-мод>\\HomingProjectiles\\. Исходные моды не изменяются. Подключи Output-мод в MO2 ниже мода с конфигом, чтобы он победил.",
                "Exports go to <Output mod>\\HomingProjectiles\\. Source mods are never changed. Enable the Output mod in MO2 below the mod with the config so it wins."
            ))
            .small()
            .color(theme::DIM),
        );
        ui.checkbox(&mut s.load_on_start, t!("Загружать при запуске", "Load on start"));
        ui.checkbox(&mut s.auto_keys, t!("Выбор записи создаёт ключ key_<EditorID> в FormIDs", "Picking a record creates a key_<EditorID> FormIDs entry"));
        ui.separator();
        if ui.add_enabled(!app.is_loading() && app.settings.source().is_some(), egui::Button::new(t!("Загрузить", "Load"))).clicked() {
            load = true;
        }
    });
    if load {
        let now = ctx.input(|i| i.time);
        app.settings.save();
        app.start_load(now);
        open = false;
    }
    app.settings_open = open;
}

pub fn new_window(app: &mut App, ctx: &egui::Context) {
    let Some(mut name) = app.new_open.take() else { return };
    let mut open = true;
    let mut create = false;
    egui::Window::new(t!("Новый конфиг", "New config")).open(&mut open).collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.label(t!("Имя файла в Data/HomingProjectiles:", "File name in Data/HomingProjectiles:"));
        let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(320.0));
        r.request_focus();
        let clash = app.world.as_ref().is_some_and(|w| w.configs.iter().any(|c| c.name.eq_ignore_ascii_case(&name) || c.name.eq_ignore_ascii_case(&format!("{name}.json"))));
        if clash {
            ui.label(RichText::new(t!("Такой файл уже есть в загрузке — экспорт его заменит.", "A file with this name is in the load order — the export will replace it.")).color(theme::WARN));
        }
        if ui.button(t!("Создать", "Create")).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
            create = true;
        }
    });
    if create && !name.trim().is_empty() {
        app.new_config(name.trim().to_string());
    } else if open {
        app.new_open = Some(name);
    }
}

pub fn confirm_close_window(app: &mut App, ctx: &egui::Context) {
    let Some(i) = app.confirm_close else { return };
    let name = app.docs.get(i).map(|d| d.name.clone()).unwrap_or_default();
    let mut decided = None;
    egui::Window::new(t!("Закрыть без экспорта?", "Close without exporting?")).collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.label(tf!("В {name} есть неэкспортированные изменения.", "{name} has changes that were not exported."));
        ui.horizontal(|ui| {
            if ui.button(t!("Экспортировать и закрыть", "Export and close")).clicked() {
                decided = Some(true);
            }
            if ui.button(RichText::new(t!("Закрыть без экспорта", "Close without export")).color(Color32::from_rgb(230, 120, 110))).clicked() {
                decided = Some(false);
            }
            if ui.button(t!("Отмена", "Cancel")).clicked() {
                app.confirm_close = None;
            }
        });
    });
    if let Some(exp) = decided {
        if exp {
            app.export(i, None);
        }
        app.close_doc(i);
        app.confirm_close = None;
    }
}
