//! "Персонаж" tab: pick an NPC from the load order, see what it wears and
//! carries, switch pieces and the weapon in hand.

use crate::app::StudioApp;
use sds_records::FormKey;

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui) {
    let Some(loaded) = &app.loaded else {
        ui.label(sds_i18n::t!("Сборка не загружена.", "Build not loaded."));
        return;
    };
    let db = loaded.db.clone();

    // Current character.
    let mut clear = false;
    let mut rebuild = false;
    match &mut app.npc {
        Some(choice) => {
            let look = &choice.look;
            ui.heading(format!("{} «{}»", look.edid, look.name));
            ui.label(sds_i18n::tf!("{} · {} · {} · масштаб {:.2} · вес {:.0}", "{} · {} · {} · scale {:.2} · weight {:.0}", db.label(look.key), look.race, if look.female { sds_i18n::t!("ж", "f") } else { sds_i18n::t!("м", "m") }, look.scale, look.weight));
            ui.small(&look.skeleton_hkx);
            ui.horizontal(|ui| {
                ui.label(sds_i18n::t!("Оружие:", "Weapon:"));
                let current = choice.weapon.and_then(|i| look.weapons.get(i)).map(|w| format!("{} ({})", w.edid, w.type_name())).unwrap_or_else(|| sds_i18n::t!("пустые руки", "empty hands").into());
                egui::ComboBox::from_id_salt("npc-weapon").selected_text(current).width(ui.available_width()).show_ui(ui, |ui| {
                    if ui.selectable_label(choice.weapon.is_none(), sds_i18n::t!("пустые руки", "empty hands")).clicked() {
                        choice.weapon = None;
                        rebuild = true;
                    }
                    for (i, w) in look.weapons.iter().enumerate() {
                        let text = format!("{} «{}» — {}", w.edid, w.name, w.type_name());
                        if ui.selectable_label(choice.weapon == Some(i), text).clicked() {
                            choice.weapon = Some(i);
                            rebuild = true;
                        }
                    }
                });
            });
            if ui.button(sds_i18n::t!("Сбросить на тело по умолчанию", "Reset to the default body")).clicked() {
                clear = true;
            }
        }
        None => {
            ui.label(sds_i18n::t!("Показано тело по умолчанию для папки клипа. Выберите NPC:", "Showing the default body for the clip folder. Pick an NPC:"));
        }
    }
    if let Some(c) = &mut app.character {
        egui::CollapsingHeader::new(sds_i18n::tf!("Части ({})", "Parts ({})", c.pieces.len())).default_open(true).show(ui, |ui| {
            for p in &mut c.pieces {
                ui.checkbox(&mut p.visible, &p.label).on_hover_text(&p.path);
            }
        });
    }
    if clear {
        app.clear_npc();
        return;
    }
    if rebuild {
        app.rebuild_npc();
        return;
    }
    ui.separator();

    // Search.
    ui.add(egui::TextEdit::singleline(&mut app.npc_filter).hint_text(sds_i18n::t!("EditorID, имя или FormID…", "EditorID, name or FormID…")).desired_width(f32::INFINITY));
    let filter = app.npc_filter.trim().to_lowercase();
    if filter.len() < 2 {
        ui.small(sds_i18n::t!("Введите хотя бы 2 символа", "Type at least 2 characters"));
        return;
    }
    let mut pick: Option<FormKey> = None;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let mut shown = 0;
        for &k in db.by_sig.get(b"NPC_").map(|v| v.as_slice()).unwrap_or(&[]) {
            let Some(r) = db.get(k) else { continue };
            let edid = r.edid();
            let name = db.name(r);
            let id = format!("{:06x}", k.id);
            if !(edid.to_lowercase().contains(&filter) || name.to_lowercase().contains(&filter) || id.contains(&filter)) {
                continue;
            }
            let selected = app.npc.as_ref().is_some_and(|c| c.look.key == k);
            if ui.selectable_label(selected, format!("{edid} «{name}»")).on_hover_text(db.label(k)).clicked() {
                pick = Some(k);
            }
            shown += 1;
            if shown >= 300 {
                ui.small(sds_i18n::t!("… показаны первые 300", "… first 300 shown"));
                break;
            }
        }
        if shown == 0 {
            ui.label(sds_i18n::t!("Ничего не найдено", "Nothing found"));
        }
    });
    if let Some(k) = pick {
        app.set_npc(k);
    }
}
