//! The chains canvas: one lane per trigger, unfolded left to right.

use crate::app::App;
use crate::theme;
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind};
use sds_i18n::{t, tf};
use sds_proj::chain::{self, Kind, Node};
use sds_proj::model::FUNCS;
use sds_proj::Sel;

const W: f32 = 250.0;
const GAP_X: f32 = 96.0;
const GAP_Y: f32 = 10.0;
const LINE_H: f32 = 15.0;
const HEAD_H: f32 = 22.0;
const LANE_HEAD: f32 = 26.0;

fn node_h(n: &Node) -> f32 {
    HEAD_H + n.lines.len() as f32 * LINE_H + 8.0
}

fn subtree_h(n: &Node) -> f32 {
    let kids: f32 = n.children.iter().map(|(_, c)| subtree_h(c)).sum::<f32>() + GAP_Y * n.children.len().saturating_sub(1) as f32;
    node_h(n).max(kids)
}

fn depth(n: &Node) -> usize {
    1 + n.children.iter().map(|(_, c)| depth(c)).max().unwrap_or(0)
}

fn matches(n: &Node, f: &str) -> bool {
    n.title.to_lowercase().contains(f) || n.lines.iter().any(|l| l.to_lowercase().contains(f)) || n.children.iter().any(|(_, c)| matches(c, f))
}

enum Act {
    Select(Sel),
    AddFunc(usize, String),
    Delete(usize),
    Duplicate(usize),
    Move(usize, isize),
    Template(crate::templates::Tpl),
    ToggleLane(usize),
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let Some(d) = app.doc() else { return };
    let cfg = d.cfg.clone();
    let sel = d.sel.clone();
    let doc_name = d.name.clone();
    let world = app.world.clone();
    let cx = chain::Ctx { c: &cfg, w: world.as_deref() };
    let mut chains = chain::build(&cx);
    for l in &mut chains.lanes {
        annotate(l, &app.foreign);
    }
    let mut acts: Vec<Act> = Vec::new();

    ui.horizontal(|ui| {
        ui.menu_button(t!("+ Цепочка", "+ Chain"), |ui| {
            for (tp, name, hint) in crate::templates::ALL {
                if ui.button(name.get()).on_hover_text(hint.get()).clicked() {
                    acts.push(Act::Template(*tp));
                    ui.close();
                }
            }
        });
        ui.add(egui::TextEdit::singleline(&mut app.chain_filter).hint_text(t!("поиск по цепочкам", "search chains")).desired_width(200.0));
        if !chains.loops.is_empty() {
            let l: Vec<String> = chains.loops.iter().map(|(a, b)| format!("#{}→#{}", a + 1, b + 1)).collect();
            ui.label(RichText::new(tf!("↻ петли: {}", "↻ loops: {}", l.join(", "))).color(theme::ERR)).on_hover_text(t!(
                "Мультикаст с callTriggers выпускает снаряд, который снова ловит тот же триггер — бесконечная цепочка, скорее всего вылет игры.",
                "A multicast with callTriggers fires a projectile the same trigger catches again — an endless chain, most likely a game crash."
            ));
        }
        ui.label(RichText::new(t!("ПКМ по триггеру — функции, копия, удаление", "RMB on a trigger — functions, copy, delete")).small().color(theme::DIM));
    });
    ui.separator();

    if chains.lanes.is_empty() {
        ui.label(RichText::new(t!("Триггеров нет. Добавь цепочку кнопкой «+ Цепочка».", "No triggers. Add a chain with «+ Chain».")).color(theme::DIM));
    }

    let filter = app.chain_filter.to_lowercase();
    let lanes: Vec<(usize, &Node)> = chains.lanes.iter().enumerate().filter(|(_, n)| filter.is_empty() || matches(n, &filter)).collect();
    let mut total_h = 0.0;
    let mut max_w: f32 = 0.0;
    for (i, n) in &lanes {
        let collapsed = app.collapsed.contains(&(doc_name.clone(), *i));
        total_h += LANE_HEAD + if collapsed { 0.0 } else { subtree_h(n) + 18.0 };
        max_w = max_w.max(depth(n) as f32 * (W + GAP_X));
    }
    let scroll_to = std::mem::take(&mut app.scroll_to_sel);

    egui::ScrollArea::both().id_salt("chains").auto_shrink([false, false]).show(ui, |ui| {
        let size = vec2(max_w.max(ui.available_width()), total_h.max(ui.available_height()));
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let painter = ui.painter_at(rect);
        let mut y = rect.top();
        for (i, n) in &lanes {
            let i = *i;
            let collapsed = app.collapsed.contains(&(doc_name.clone(), i));
            // Lane header.
            let head = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), LANE_HEAD - 4.0));
            painter.rect_filled(head, 3.0, Color32::from_rgb(38, 38, 44));
            let arrow = if collapsed { "▶" } else { "▼" };
            let mut text = tf!("{arrow} Цепочка #{}", "{arrow} Chain #{}", i + 1);
            let callers: Vec<String> = chains.called_from.iter().filter(|(c, _)| *c == i).map(|(_, r)| format!("#{}", r + 1)).collect();
            if !callers.is_empty() {
                text += &tf!("   ← продолжает цепочку {}", "   ← continues chain {}", callers.join(", "));
            }
            painter.text(head.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, text, FontId::proportional(13.0), Color32::from_rgb(200, 200, 210));
            let hr = ui.interact(head, ui.id().with(("lane", i)), Sense::click());
            if hr.clicked() {
                acts.push(Act::ToggleLane(i));
            }
            y += LANE_HEAD;
            if collapsed {
                continue;
            }
            let ctx = Draw { app_sel: &sel, scroll_to, foreign: &app.foreign, world: world.as_deref() };
            draw_node(ui, &painter, &ctx, n, pos2(rect.left() + 6.0, y), &mut acts, i, n.kind == Kind::Trigger, &cfg.triggers.len());
            y += subtree_h(n) + 18.0;
        }
    });

    // Apply actions.
    for a in acts {
        match a {
            Act::Select(s) => app.select(s),
            Act::ToggleLane(i) => {
                let k = (doc_name.clone(), i);
                if !app.collapsed.remove(&k) {
                    app.collapsed.insert(k);
                }
            }
            Act::Template(tp) => {
                if let Some(d) = app.doc_mut() {
                    let i = crate::templates::add(&mut d.cfg, tp, None);
                    d.sel = Sel::Trigger(i);
                }
                app.scroll_to_sel = true;
            }
            Act::AddFunc(i, kind) => {
                if let Some(d) = app.doc_mut() {
                    if let Some(t) = d.cfg.triggers.get_mut(i) {
                        t.tf.functions.push(crate::templates::new_func(&kind));
                        let fi = t.tf.functions.len() - 1;
                        d.sel = Sel::Func(Box::new(Sel::Trigger(i)), fi);
                    }
                }
            }
            Act::Delete(i) => {
                if let Some(d) = app.doc_mut() {
                    if i < d.cfg.triggers.len() {
                        d.cfg.triggers.remove(i);
                        d.sel = Sel::Root;
                    }
                }
            }
            Act::Duplicate(i) => {
                if let Some(d) = app.doc_mut() {
                    if let Some(t) = d.cfg.triggers.get(i).cloned() {
                        d.cfg.triggers.insert(i + 1, t);
                        d.sel = Sel::Trigger(i + 1);
                    }
                }
            }
            Act::Move(i, dir) => {
                if let Some(d) = app.doc_mut() {
                    let j = i as isize + dir;
                    if j >= 0 && (j as usize) < d.cfg.triggers.len() {
                        d.cfg.triggers.swap(i, j as usize);
                        d.sel = Sel::Trigger(j as usize);
                    }
                }
            }
        }
    }
}

struct Draw<'a> {
    app_sel: &'a Sel,
    scroll_to: bool,
    foreign: &'a std::collections::HashMap<sds_records::FormKey, Vec<crate::app::ForeignCatch>>,
    world: Option<&'a sds_proj::world::World>,
}

fn is_selected(cur: &Sel, n: &Sel) -> bool {
    if cur == n {
        return true;
    }
    match (cur, n) {
        (Sel::Func(o, _), Sel::Trigger(_)) => o.as_ref() == n,
        (Sel::McItem(a, _), Sel::Multicast(b)) => a == b,
        (Sel::EmFunc(a, _), Sel::Emitter(b)) => a == b,
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_node(ui: &mut egui::Ui, painter: &egui::Painter, dc: &Draw, n: &Node, at: Pos2, acts: &mut Vec<Act>, lane: usize, root: bool, n_triggers: &usize) {
    let h = node_h(n);
    let r = Rect::from_min_size(at, vec2(W, h));
    let col = theme::kind_color(n.kind);
    let selected = is_selected(dc.app_sel, &n.sel);
    let fill = if n.dim { Color32::from_rgb(30, 30, 34) } else { Color32::from_rgb(44, 44, 52) };
    painter.rect_filled(r, 5.0, fill);
    let head = Rect::from_min_size(r.min, vec2(W, HEAD_H));
    painter.rect_filled(head, egui::CornerRadius { nw: 5, ne: 5, sw: 0, se: 0 }, col.gamma_multiply(if n.dim { 0.25 } else { 0.45 }));
    let stroke = if selected {
        Stroke::new(2.0_f32, Color32::WHITE)
    } else if n.alert {
        Stroke::new(1.5_f32, theme::ERR)
    } else {
        Stroke::new(1.0_f32, col.gamma_multiply(0.8))
    };
    painter.rect_stroke(r, 5.0, stroke, StrokeKind::Inside);
    let title = clip(&n.title, 32);
    painter.text(head.left_center() + vec2(7.0, 0.0), Align2::LEFT_CENTER, title, FontId::proportional(13.0), Color32::WHITE);
    let tcol = if n.dim { theme::DIM } else { Color32::from_rgb(205, 205, 215) };
    let extra: Vec<String> = Vec::new();
    let _ = dc.foreign;
    for (li, l) in n.lines.iter().enumerate() {
        let c = if l.starts_with('✖') || l.contains('!') {
            theme::WARN
        } else if l.starts_with('⇢') {
            theme::OK
        } else {
            tcol
        };
        painter.text(pos2(r.left() + 8.0, r.top() + HEAD_H + 4.0 + li as f32 * LINE_H), Align2::LEFT_TOP, clip(l, 40), FontId::proportional(12.0), c);
    }
    let resp = ui.interact(r, ui.id().with(("node", lane, at.x as i32, at.y as i32)), Sense::click());
    if dc.scroll_to && selected {
        ui.scroll_to_rect(r, None);
    }
    let mut tip: Vec<String> = Vec::new();
    tip.push(n.title.clone());
    tip.extend(n.lines.iter().cloned());
    tip.extend(extra.iter().cloned());
    if let (Sel::Record(k), Some(w)) = (&n.sel, dc.world) {
        tip.push(w.formid(*k));
    }
    let resp = resp.on_hover_text(tip.join("\n"));
    if resp.clicked() && n.kind != Kind::Note {
        acts.push(Act::Select(n.sel.clone()));
    }
    if root {
        if let Sel::Trigger(ti) = n.sel {
            resp.context_menu(|ui| {
                ui.menu_button(t!("Добавить функцию", "Add function"), |ui| {
                    for (name, _) in FUNCS {
                        if ui.button(format!("{} — {}", chain::func_label(name), name)).clicked() {
                            acts.push(Act::AddFunc(ti, name.to_string()));
                            ui.close();
                        }
                    }
                });
                if ui.button(t!("Дублировать", "Duplicate")).clicked() {
                    acts.push(Act::Duplicate(ti));
                    ui.close();
                }
                if ti > 0 && ui.button(t!("Выше", "Move up")).clicked() {
                    acts.push(Act::Move(ti, -1));
                    ui.close();
                }
                if ti + 1 < *n_triggers && ui.button(t!("Ниже", "Move down")).clicked() {
                    acts.push(Act::Move(ti, 1));
                    ui.close();
                }
                ui.separator();
                if ui.button(RichText::new(t!("Удалить триггер", "Delete trigger")).color(theme::ERR)).clicked() {
                    acts.push(Act::Delete(ti));
                    ui.close();
                }
            });
        }
    }
    if !extra.is_empty() {
        let p = pos2(r.left() + 8.0, r.bottom() - 2.0);
        painter.text(p, Align2::LEFT_BOTTOM, clip(&extra[0], 40), FontId::proportional(11.0), theme::OK);
    }

    // Children.
    let mut cy = at.y;
    let from = pos2(r.right(), r.top() + HEAD_H * 0.5);
    for (label, c) in &n.children {
        let cat = pos2(at.x + W + GAP_X, cy);
        let to = pos2(cat.x, cat.y + HEAD_H * 0.5);
        let ecol = if c.dim { Color32::from_rgb(80, 80, 90) } else { theme::kind_color(c.kind).gamma_multiply(0.9) };
        let mid = (from.x + to.x) * 0.5;
        let bez = egui::epaint::CubicBezierShape::from_points_stroke([from, pos2(mid, from.y), pos2(mid, to.y), to], false, Color32::TRANSPARENT, Stroke::new(if c.dim { 1.0_f32 } else { 1.6 }, ecol));
        painter.add(bez);
        painter.circle_filled(to, 3.0, ecol);
        if !label.is_empty() {
            painter.text(pos2(to.x - 6.0, to.y - 2.0), Align2::RIGHT_BOTTOM, clip(label, 16), FontId::proportional(10.0), ecol);
        }
        draw_node(ui, painter, dc, c, cat, acts, lane, false, n_triggers);
        cy += subtree_h(c) + GAP_Y;
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    } else {
        s.to_string()
    }
}

/// Projectiles also caught in other configs of the load order.
fn annotate(n: &mut Node, foreign: &std::collections::HashMap<sds_records::FormKey, Vec<crate::app::ForeignCatch>>) {
    if n.kind == Kind::Projectile {
        if let Sel::Record(k) = &n.sel {
            if let Some(f) = foreign.get(k) {
                let l: Vec<String> = f.iter().take(2).map(|c| format!("{} #{}", c.file.trim_end_matches(".json"), c.trigger + 1)).collect();
                let more = if f.len() > 2 { format!(" +{}", f.len() - 2) } else { String::new() };
                n.lines.push(tf!("⇢ ловят: {}{more}", "⇢ caught by: {}{more}", l.join(", ")));
            }
        }
    }
    for (_, c) in &mut n.children {
        annotate(c, foreign);
    }
}
