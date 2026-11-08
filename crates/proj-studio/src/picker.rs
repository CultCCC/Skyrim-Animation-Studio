//! Record picker: search the load order by EditorID, name or FormID.

use crate::app::App;
use crate::theme;
use egui::RichText;
use sds_i18n::{t, tf};
use sds_proj::world::PICK_SIGS;

pub fn window(app: &mut App, ctx: &egui::Context) {
    let Some(req) = &mut app.picker else { return };
    let Some(w) = app.world.clone() else {
        app.picker = None;
        app.log(t!("Сначала загрузи профиль MO2", "Load an MO2 profile first").into());
        return;
    };
    let mut open = true;
    let mut chosen = None;
    egui::Window::new(t!("Выбор записи", "Pick a record")).open(&mut open).default_size([560.0, 520.0]).collapsible(false).show(ctx, |ui| {
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut req.query).hint_text(t!("EditorID, имя или FormID", "EditorID, name or FormID")).desired_width(300.0));
            if req.just_opened {
                r.request_focus();
                req.just_opened = false;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(t!("Типы:", "Types:"));
            for s in PICK_SIGS {
                let mut on = req.sigs.is_empty() || req.sigs.contains(*s);
                if ui.checkbox(&mut on, String::from_utf8_lossy(*s)).changed() {
                    if req.sigs.is_empty() {
                        req.sigs = PICK_SIGS.iter().map(|x| **x).collect();
                    }
                    if on {
                        req.sigs.push(**s);
                    } else {
                        req.sigs.retain(|x| x != *s);
                    }
                }
            }
        });
        ui.separator();
        let q: Vec<String> = req.query.to_lowercase().split_whitespace().map(String::from).collect();
        let hits: Vec<_> = w.entries.iter().filter(|e| (req.sigs.is_empty() || req.sigs.contains(&e.sig)) && q.iter().all(|t| e.hay.contains(t))).take(400).collect();
        ui.label(RichText::new(tf!("найдено: {}{}", "found: {}{}", hits.len(), if hits.len() == 400 { "+" } else { "" })).small().color(theme::DIM));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("pick").striped(true).num_columns(4).show(ui, |ui| {
                for e in hits {
                    ui.label(RichText::new(String::from_utf8_lossy(&e.sig)).monospace().color(theme::DIM));
                    if ui.link(&e.edid).clicked() {
                        chosen = Some(e.formid.clone());
                    }
                    ui.label(&e.name);
                    ui.label(RichText::new(&e.formid).small().color(theme::DIM));
                    ui.end_row();
                }
            });
        });
    });
    if let Some(f) = chosen {
        app.picked = Some((req.target, f));
        open = false;
    }
    if !open {
        app.picker = None;
    }
}
