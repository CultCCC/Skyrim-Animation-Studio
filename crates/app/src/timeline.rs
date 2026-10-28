//! The event timeline, DS Anim Studio's TAE editor in Skyrim terms: one row
//! per annotation category, point events as diamonds, start/end pairs as
//! bars, a frame/second ruler and a playhead.

use crate::app::StudioApp;
use crate::settings::TimeUnit;
use crate::theme::category_color;
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense, Shape, Stroke};
use sds_anno::{parse, spans, Category, Kind};

const LABEL_W: f32 = 140.0;
const RULER_H: f32 = 24.0;
const LANE_H: f32 = 18.0;
const ROW_PAD: f32 = 4.0;
const HIT_R: f32 = 6.0;

pub struct TimelineView {
    pub px_per_sec: f32,
    /// Time at the left edge of the track area.
    pub offset: f32,
    pub fit_next: bool,
    drag: Option<Drag>,
}

impl Default for TimelineView {
    fn default() -> Self {
        TimelineView { px_per_sec: 300.0, offset: 0.0, fit_next: true, drag: None }
    }
}

#[derive(Clone, Copy)]
enum Drag {
    /// Dragging empty track space pans the view; time changes only on the ruler.
    Pan,
    /// `grab` = pointer x at press and the annotation's time then, so the
    /// item moves by the pointer's travel instead of jumping under it.
    Anno { index: usize, time: f32, grab_x: f32, grab_time: f32 },
}

/// Text new annotations of a row start with.
pub fn template(cat: Category) -> &'static str {
    match cat {
        Category::Precision => "Collision_Add.Node(WEAPON)",
        Category::Pie => "PIE.$Alias",
        Category::SdsCombat => "SDS_NPC_TargetLockOn.1",
        Category::Scar => r#"SCAR_ActionData{"IdleAnimation":"ADXP_NPCNormalAttack", "MinDistance":0, "MaxDistance":220, "StartAngle":-60, "EndAngle":60, "Chance":100, "Type":"RA"}"#,
        Category::Combo => "MCO_WinOpen",
        Category::Attack => "HitFrame",
        Category::Sound => "SoundPlay.WPNSwingUnarmed",
        Category::Foot => "FootLeft",
        Category::Motion => "animmotion 0 0 0",
        Category::Other => "NewEvent",
    }
}

/// One menu entry: `annos` empty = the label itself at the playhead;
/// otherwise every (text, frames after the playhead) goes in as one edit.
pub struct Tpl {
    pub label: &'static str,
    pub annos: &'static [(&'static str, i32)],
}

const fn one(label: &'static str) -> Tpl {
    Tpl { label, annos: &[] }
}

impl Tpl {
    /// The annotations to add with the playhead at `t`.
    pub fn expand(&self, t: f32, ft: f32, duration: f32) -> Vec<(String, f32)> {
        if self.annos.is_empty() {
            return vec![(self.label.to_string(), t)];
        }
        self.annos.iter().map(|(text, frames)| (text.to_string(), (t + *frames as f32 * ft).clamp(0.0, duration))).collect()
    }
}

/// Templates offered in the context menus.
pub const TEMPLATES: &[(sds_i18n::Tr, &[Tpl])] = &[
    (
        sds_i18n::tr!("Precision", "Precision"),
        &[
            one("Collision_AttackStart"),
            Tpl { label: "Collision_Add.Node(WEAPON)", annos: &[("Collision_Add.Node(WEAPON)", 0), ("Collision_Remove.Node(WEAPON)", 5)] },
            Tpl { label: "Collision_Add.Node(SHIELD)", annos: &[("Collision_Add.Node(SHIELD)", 0), ("Collision_Remove.Node(SHIELD)", 5)] },
            one("Collision_AttackEnd"),
        ],
    ),
    (
        sds_i18n::tr!("SDSCombat", "SDSCombat"),
        &[
            one("SDS_NPC_TargetLockOn.1"),
            one("SDS_NPC_TurnSpeed.0.5"),
            one("SDS_NPC_TargetLockOff"),
            one("SDS_NPC_StopRotation"),
            one("SDS_NPC_TargetLockReset"),
            one("SDS_NPC_ReachTarget.150|40|190|170"),
            one("SDS_NPC_ReachTargetEnd"),
            one("SDS_NPC_BumpOff"),
            one("SDS_NPC_BumpOn"),
        ],
    ),
    (
        sds_i18n::tr!("MCO / BFCO", "MCO / BFCO"),
        &[
            Tpl {
                label: "MCO_Windows",
                annos: &[("MCO_WinOpen", 0), ("MCO_PowerWinOpen", 0), ("MCO_WinClose", 10), ("MCO_PowerWinClose", 10)],
            },
            one("MCO_Recovery"),
            one("BFCO_NextWinStart"),
            one("BFCO_NextPowerWinStart"),
            one("BFCO_DIY_Recovery"),
        ],
    ),
    (sds_i18n::tr!("Атака", "Attack"), &[one("preHitFrame"), one("HitFrame"), one("weaponSwing"), one("weaponLeftSwing"), one("attackStop"), one("CastOKStart"), one("CastOKStop")]),
    (sds_i18n::tr!("SCAR", "SCAR"), &[one("SCAR_ComboStart")]),
    (
        sds_i18n::tr!("PIE", "PIE"),
        &[
            one("PIE.$Alias"),
            Tpl {
                label: "PIE.@SGVI|MCO_next(power)attack|1",
                annos: &[("PIE.@SGVI|MCO_nextattack|1", 0), ("PIE.@SGVI|MCO_nextpowerattack|1", 0)],
            },
            one("PIE.@SGVF|MCO_AttackSpeed|1.0"),
        ],
    ),
];

/// The "add" menus shared by the timeline and the annotation list:
/// templates, a custom annotation (saved to favorites) and the favorites.
/// Returns what to add.
/// The windows the add menus open (a text field inside a popup menu loses
/// the menu on the first click).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Popup {
    Custom,
    Favorites,
}

pub fn add_menu(ui: &mut egui::Ui, t: f32, ft: f32, duration: f32, favs: &[String], popup: &mut Option<Popup>) -> Option<Vec<(String, f32)>> {
    let mut out = None;
    ui.label(sds_i18n::tf!("На плейхед: {:.3} с (кадр {})", "At playhead: {:.3} s (frame {})", t, (t / ft).round()));
    ui.menu_button(sds_i18n::t!("Добавить аннотацию", "Add annotation"), |ui| {
        for (group, items) in TEMPLATES {
            ui.menu_button(group.get(), |ui| {
                for tpl in *items {
                    let mut r = ui.button(tpl.label);
                    if !tpl.annos.is_empty() {
                        let list: Vec<String> = tpl.annos.iter().map(|(a, f)| if *f == 0 { a.to_string() } else { sds_i18n::tf!("{a}  (+{f} кадров)", "{a}  (+{f} frames)") }).collect();
                        r = r.on_hover_text(list.join("\n"));
                    }
                    if r.clicked() {
                        out = Some(tpl.expand(t, ft, duration));
                        ui.close();
                    }
                }
            });
        }
    });
    if ui.button(sds_i18n::t!("Добавить свою аннотацию…", "Add custom annotation…")).clicked() {
        *popup = Some(Popup::Custom);
        ui.close();
    }
    if ui.button(sds_i18n::tf!("Избранные аннотации… ({})", "Favorite annotations… ({})", favs.len())).clicked() {
        *popup = Some(Popup::Favorites);
        ui.close();
    }
    out
}

struct Item {
    index: usize,
    time: f32,
}

struct Bar {
    start: f32,
    end: f32,
    lane: usize,
    label: String,
    open: usize,
}

struct Row {
    cat: Category,
    lanes: usize,
    items: Vec<Item>,
    bars: Vec<Bar>,
}

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui) {
    transport(app, ui);
    ui.separator();
    let Some(doc) = &app.doc else {
        ui.centered_and_justified(|ui| ui.label(sds_i18n::t!("Откройте клип в библиотеке или перетащите .hkx в окно", "Open a clip in the library or drop an .hkx onto the window")));
        return;
    };
    let duration = doc.duration().max(1e-3);
    let ft = doc.frame_time();

    // ---- layout of rows ------------------------------------------------
    let drag_time: Option<(usize, f32)> = match app.timeline.drag {
        Some(Drag::Anno { index, time, .. }) => Some((index, time)),
        _ => None,
    };
    let time_of = |i: usize, t: f32| match drag_time {
        Some((d, dt)) if d == i => dt,
        _ => t,
    };
    let texts: Vec<(f32, &str)> = doc.annos.iter().enumerate().map(|(i, a)| (time_of(i, a.time), a.text.as_str())).collect();
    let cats: Vec<Category> = doc.annos.iter().map(|a| parse(&a.text).category).collect();
    let mut all_spans = spans(&texts, duration);
    // Precision bars come from the rule simulation, not from name pairing.
    if let Some(sim) = crate::precision_view::simulate_doc(app) {
        all_spans.retain(|s| s.category != Category::Precision);
        for c in &sim.collisions {
            all_spans.push(sds_anno::Span {
                category: Category::Precision,
                label: format!("{} → {}", c.params.node, c.end_reason),
                open: c.add,
                close: None,
                start: c.start,
                end: c.end,
            });
        }
    }
    let mut rows: Vec<Row> = Vec::new();
    for cat in Category::ALL {
        if app.scene.hidden.contains(&cat) {
            continue;
        }
        let items: Vec<Item> = (0..doc.annos.len()).filter(|&i| cats[i] == cat).map(|i| Item { index: i, time: texts[i].0 }).collect();
        let mut bars: Vec<Bar> = Vec::new();
        let mut lane_end: Vec<f32> = Vec::new();
        let mut cs: Vec<&sds_anno::Span> = all_spans.iter().filter(|s| s.category == cat).collect();
        cs.sort_by(|a, b| a.start.total_cmp(&b.start));
        for s in cs {
            let lane = match lane_end.iter().position(|&e| e <= s.start + 1e-4) {
                Some(l) => l,
                None => {
                    lane_end.push(0.0);
                    lane_end.len() - 1
                }
            };
            lane_end[lane] = s.end;
            bars.push(Bar { start: s.start, end: s.end, lane: lane + 1, label: s.label.clone(), open: s.open });
        }
        if items.is_empty() && bars.is_empty() {
            continue;
        }
        // Root motion gets a lane of its key values under the speed curve.
        let values = usize::from(cat == Category::Motion && app.settings.motion_values);
        rows.push(Row { cat, lanes: 1 + lane_end.len() + values, items, bars });
    }

    // ---- geometry -------------------------------------------------------
    let full = ui.available_rect_before_wrap();
    let track_w = (full.width() - LABEL_W - 8.0).max(50.0);
    let view = &mut app.timeline;
    if view.fit_next {
        view.px_per_sec = track_w / duration;
        view.offset = 0.0;
        view.fit_next = false;
    }
    let px = view.px_per_sec;
    let off = view.offset;
    let x_of = |t: f32, left: f32| left + LABEL_W + (t - off) * px;
    let t_of = |x: f32, left: f32| off + (x - left - LABEL_W) / px;

    // ---- ruler ------------------------------------------------------------
    let (ruler, ruler_resp) = ui.allocate_exact_size(vec2(full.width(), RULER_H), Sense::click_and_drag());
    let painter = ui.painter_at(ruler);
    painter.rect_filled(ruler, 0.0, ui.visuals().extreme_bg_color);
    let unit = app.settings.time_unit;
    draw_ruler(&painter, ruler, duration, ft, px, off, unit);
    let ph = x_of(app.play.time, ruler.left());
    painter.add(Shape::convex_polygon(
        vec![pos2(ph - 6.0, ruler.top() + 2.0), pos2(ph + 6.0, ruler.top() + 2.0), pos2(ph, ruler.bottom())],
        Color32::from_rgb(230, 80, 60),
        Stroke::NONE,
    ));

    // ---- rows -----------------------------------------------------------
    let rows_h: f32 = rows.iter().map(|r| r.lanes as f32 * LANE_H + ROW_PAD).sum();
    let mut clicked_item: Option<usize> = None;
    let mut new_drag: Option<Drag> = app.timeline.drag;
    let mut drop_anno: Option<(usize, f32)> = None;
    let mut add_at: Option<(Category, f32)> = None;
    let mut add_text: Option<Vec<(String, f32)>> = None;
    let favs = app.settings.favorites.clone();
    let mut popup = None;
    let mut delete: Option<usize> = None;
    let mut set_time: Option<f32> = None;
    let mut deselect = false;
    let mut zoom_pan: Option<(f32, f32)> = None;
    let selected = app.selected;
    let snap = |t: f32| -> f32 {
        if app.settings.snap_to_frames {
            ((t / ft).round() * ft).clamp(0.0, duration)
        } else {
            t.clamp(0.0, duration)
        }
    };

    // The wheel zooms time here, so the rows scroll by the bar or Ctrl+wheel.
    let wheel_rows = egui::scroll_area::ScrollSource { scroll_bar: true, drag: false, mouse_wheel: false };
    egui::ScrollArea::vertical().auto_shrink(false).scroll_source(wheel_rows).show(ui, |ui| {
        let (area, resp) = ui.allocate_exact_size(vec2(full.width(), rows_h.max(ui.available_height())), Sense::click_and_drag());
        let p = ui.painter_at(area);
        let left = area.left();
        let track_left = left + LABEL_W;
        // Past the end of the clip.
        let end_x = x_of(duration, left);
        if end_x < area.right() {
            p.rect_filled(Rect::from_min_max(pos2(end_x.max(track_left), area.top()), area.right_bottom()), 0.0, Color32::from_black_alpha(90));
        }
        // Frame grid.
        let ppf = px * ft;
        if ppf >= 6.0 {
            let first = (off / ft).floor().max(0.0) as i64;
            let last = ((off + track_w / px) / ft).ceil() as i64;
            for f in first..=last {
                let x = x_of(f as f32 * ft, left);
                if x >= track_left {
                    p.line_segment([pos2(x, area.top()), pos2(x, area.bottom())], Stroke::new(1.0_f32, Color32::from_white_alpha(8)));
                }
            }
        }

        let pointer = resp.hover_pos().or(resp.interact_pointer_pos());
        // What was under the pointer when the button went DOWN: a drag
        // only starts after the pointer has travelled a few pixels, by
        // which time it may have left the diamond.
        let press = ui.input(|i| i.pointer.press_origin());
        let mut pressed: Option<usize> = None;
        let mut hovered: Option<(usize, Category)> = None;
        // Every point under the pointer: stacked diamonds on one frame.
        let mut under: Vec<usize> = Vec::new();
        let mut pressed_all: Vec<usize> = Vec::new();
        let mut hovered_row: Option<Category> = None;
        let mut y = area.top();
        let font = FontId::proportional(11.5);
        for row in &rows {
            let h = row.lanes as f32 * LANE_H + ROW_PAD;
            let row_rect = Rect::from_min_size(pos2(left, y), vec2(area.width(), h));
            let color = category_color(row.cat);
            p.rect_filled(Rect::from_min_size(row_rect.min, vec2(LABEL_W, h)), 0.0, Color32::from_rgb(34, 34, 40));
            p.line_segment([pos2(left, y + h), pos2(area.right(), y + h)], Stroke::new(1.0_f32, Color32::from_white_alpha(18)));
            p.rect_filled(Rect::from_min_size(pos2(left, y), vec2(3.0, h)), 0.0, color);
            p.text(pos2(left + 8.0, y + LANE_H * 0.5 + 1.0), Align2::LEFT_CENTER, format!("{}  ({})", row.cat.label(), row.items.len()), font.clone(), Color32::from_gray(210));
            if pointer.is_some_and(|q| row_rect.contains(q)) {
                hovered_row = Some(row.cat);
            }
            let tp = p.with_clip_rect(Rect::from_min_max(pos2(track_left, y), pos2(area.right(), y + h)));

            // Bars.
            for b in &row.bars {
                let x0 = x_of(b.start, left);
                let x1 = x_of(b.end, left).max(x0 + 2.0);
                let by = y + b.lane as f32 * LANE_H;
                let r = Rect::from_min_max(pos2(x0, by + 2.0), pos2(x1, by + LANE_H - 2.0));
                let sel = selected == Some(b.open);
                if press.is_some_and(|q| r.contains(q)) && pressed.is_none() {
                    pressed = Some(b.open);
                }
                tp.rect_filled(r, 3.0, color.gamma_multiply(if sel { 0.75 } else { 0.45 }));
                tp.rect_stroke(r, 3.0, Stroke::new(1.0_f32, color), egui::StrokeKind::Inside);
                tp.with_clip_rect(r.intersect(tp.clip_rect())).text(pos2(x0 + 4.0, by + LANE_H * 0.5), Align2::LEFT_CENTER, &b.label, font.clone(), Color32::WHITE);
            }

            // Points.
            let cy = y + LANE_H * 0.5 + 1.0;
            if row.cat == Category::Motion {
                // Hundreds of keys: ticks plus the speed curve instead of labels.
                let values_lane = app.settings.motion_values;
                let h = if values_lane { h - LANE_H } else { h };
                if values_lane {
                    let vy = y + h;
                    p.line_segment([pos2(track_left, vy), pos2(area.right(), vy)], Stroke::new(1.0_f32, Color32::from_white_alpha(10)));
                    p.text(pos2(left + 8.0, vy + LANE_H * 0.5), Align2::LEFT_CENTER, sds_i18n::t!("значения root", "root values"), FontId::proportional(10.5), Color32::from_gray(150));
                    // Each key's numbers, up to the next key.
                    let mut keys: Vec<(f32, String)> = row
                        .items
                        .iter()
                        .filter_map(|it| match parse(&doc.annos[it.index].text).kind {
                            Kind::AnimMotion(v) => Some((it.time, format!("{:.1}, {:.1}, {:.1}", v[0], v[1], v[2]))),
                            Kind::AnimRotation(d) => Some((it.time, format!("↻{d:+.1}°"))),
                            _ => None,
                        })
                        .collect();
                    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let vfont = FontId::monospace(10.0);
                    // Dense keys (one per frame): label the ones there is room
                    // for; zoom in to see every key.
                    let mut free_from = f32::MIN;
                    for (t, text) in &keys {
                        let x = x_of(*t, left);
                        if x < track_left - 200.0 || x > area.right() {
                            continue;
                        }
                        let labelled = x >= free_from;
                        tp.line_segment(
                            [pos2(x, vy + if labelled { 2.0 } else { LANE_H - 6.0 }), pos2(x, vy + LANE_H - 2.0)],
                            Stroke::new(1.0_f32, color.gamma_multiply(if labelled { 0.8 } else { 0.35 })),
                        );
                        if labelled {
                            let r = tp.text(pos2(x + 3.0, vy + LANE_H * 0.5), Align2::LEFT_CENTER, text, vfont.clone(), Color32::from_gray(190));
                            free_from = r.right() + 8.0;
                        }
                    }
                }
                let mut pts = Vec::new();
                let step = (2.0 / px).max(1.0 / 240.0);
                let mut t = off.max(0.0);
                let tmax = (off + track_w / px).min(duration);
                let mut vmax = 1.0f32;
                let mut samples = Vec::new();
                while t <= tmax {
                    let v = doc.motion.speed(t);
                    vmax = vmax.max(v);
                    samples.push((t, v));
                    t += step;
                }
                for (t, v) in samples {
                    pts.push(pos2(x_of(t, left), y + h - 3.0 - (v / vmax) * (h - 8.0)));
                }
                if pts.len() > 1 {
                    tp.add(Shape::line(pts, Stroke::new(1.5_f32, color)));
                }
                for it in &row.items {
                    let x = x_of(it.time, left);
                    tp.line_segment([pos2(x, y + h - 4.0), pos2(x, y + h - 1.0)], Stroke::new(1.0_f32, color.gamma_multiply(0.6)));
                }
                tp.text(pos2(track_left + 4.0, y + 8.0), Align2::LEFT_CENTER, sds_i18n::tf!("скорость, макс {:.0} ед/с", "speed, max {:.0} u/s", vmax), FontId::proportional(10.5), Color32::from_gray(150));
            }
            let mut sorted: Vec<&Item> = row.items.iter().collect();
            sorted.sort_by(|a, b| a.time.total_cmp(&b.time));
            for (k, it) in sorted.iter().enumerate() {
                if row.cat == Category::Motion && selected != Some(it.index) {
                    continue;
                }
                let x = x_of(it.time, left);
                let sel = selected == Some(it.index);
                let near = pointer.is_some_and(|q| (q.x - x).abs() <= HIT_R && (q.y - cy).abs() <= LANE_H * 0.5);
                if near && hovered.is_none() {
                    hovered = Some((it.index, row.cat));
                }
                if near {
                    under.push(it.index);
                }
                if press.is_some_and(|q| (q.x - x).abs() <= HIT_R + 2.0 && (q.y - cy).abs() <= LANE_H * 0.5) {
                    pressed_all.push(it.index);
                }
                let r = if sel || near { 6.0 } else { 4.5 };
                tp.add(Shape::convex_polygon(
                    vec![pos2(x, cy - r), pos2(x + r, cy), pos2(x, cy + r), pos2(x - r, cy)],
                    if sel { Color32::WHITE } else { color },
                    Stroke::new(1.0_f32, Color32::BLACK),
                ));
                // Label up to the next point.
                let next_x = sorted.get(k + 1).map(|n| x_of(n.time, left)).unwrap_or(area.right());
                if next_x - x > 24.0 {
                    let label_rect = Rect::from_min_max(pos2(x + 7.0, y), pos2(next_x - 3.0, y + LANE_H));
                    tp.with_clip_rect(label_rect.intersect(tp.clip_rect())).text(
                        pos2(x + 8.0, cy),
                        Align2::LEFT_CENTER,
                        short_label(&doc.annos[it.index].text),
                        font.clone(),
                        if sel { Color32::WHITE } else { Color32::from_gray(200) },
                    );
                }
            }
            y += h;
        }
        p.line_segment([pos2(track_left, area.top()), pos2(track_left, area.bottom())], Stroke::new(1.0_f32, Color32::from_white_alpha(30)));
        let phx = x_of(app.play.time, left);
        if phx >= track_left {
            p.line_segment([pos2(phx, area.top()), pos2(phx, area.bottom())], Stroke::new(1.5_f32, Color32::from_rgb(230, 80, 60)));
        }

        // ---- interaction ----
        // Of stacked points, a drag takes the selected one.
        if let Some(&first) = pressed_all.first() {
            pressed = Some(selected.filter(|s| pressed_all.contains(s)).unwrap_or(first));
        }
        if under.len() > 1 {
            let list: Vec<String> = under.iter().map(|&i| format!("{}{}", if selected == Some(i) { "▶ " } else { "   " }, doc.annos[i].text)).collect();
            resp.clone().on_hover_text_at_pointer(sds_i18n::tf!("{} аннотаций в этой точке — клик выбирает следующую, ПКМ — список\n{}", "{} annotations at this point — click selects the next one, RMB — list\n{}", under.len(), list.join("\n")));
        } else if let Some((i, _)) = hovered {
            resp.clone().on_hover_text_at_pointer(sds_i18n::tf!("{:.4} с · кадр {}\n{}", "{:.4} s · frame {}\n{}", doc.annos[i].time, (doc.annos[i].time / ft).round(), doc.annos[i].text));
        }
        if resp.drag_started_by(egui::PointerButton::Primary) {
            new_drag = match (pressed, press) {
                (Some(i), Some(q)) => {
                    clicked_item = Some(i);
                    let t0 = doc.annos[i].time;
                    Some(Drag::Anno { index: i, time: t0, grab_x: q.x, grab_time: t0 })
                }
                _ => Some(Drag::Pan),
            };
        }
        if resp.dragged() {
            match new_drag {
                Some(Drag::Anno { index, grab_x, grab_time, .. }) => {
                    if let Some(q) = resp.interact_pointer_pos() {
                        let t = snap(grab_time + (q.x - grab_x) / px);
                        new_drag = Some(Drag::Anno { index, time: t, grab_x, grab_time });
                    }
                }
                Some(Drag::Pan) => {
                    let dx = resp.drag_delta().x;
                    if dx != 0.0 {
                        zoom_pan = Some((1.0, dx));
                    }
                }
                None => {}
            }
        }
        if resp.drag_stopped() {
            if let Some(Drag::Anno { index, time, grab_time, .. }) = new_drag {
                if time != grab_time {
                    drop_anno = Some((index, time));
                }
            }
            new_drag = None;
        }
        // A click selects (or clears the selection); the playhead stays put.
        if resp.clicked() {
            match hovered {
                // Stacked: each click steps to the next one under the pointer.
                Some(_) if under.len() > 1 => {
                    let at = selected.and_then(|s| under.iter().position(|&x| x == s));
                    clicked_item = Some(under[at.map(|p| (p + 1) % under.len()).unwrap_or(0)]);
                }
                Some((i, _)) => clicked_item = Some(i),
                None => deselect = true,
            }
        }
        if resp.double_clicked() && hovered.is_none() {
            if let (Some(cat), Some(q)) = (hovered_row, resp.interact_pointer_pos()) {
                add_at = Some((cat, snap(t_of(q.x, left))));
            }
        }
        // The context menu adds at the playhead (time is set on the ruler).
        let menu_t = snap(app.play.time);
        if resp.secondary_clicked() {
            if let Some((i, _)) = hovered {
                if !selected.is_some_and(|s| under.contains(&s)) {
                    clicked_item = Some(i);
                }
            }
            ui.data_mut(|d| d.insert_temp(egui::Id::new("tl-stack"), under.clone()));
        }
        let stack: Vec<usize> = ui.data(|d| d.get_temp(egui::Id::new("tl-stack"))).unwrap_or_default();
        resp.context_menu(|ui| {
            if stack.len() > 1 {
                ui.label(sds_i18n::t!("В этой точке:", "At this point:"));
                for &i in &stack {
                    if let Some(a) = doc.annos.get(i) {
                        if ui.selectable_label(selected == Some(i), &a.text).clicked() {
                            clicked_item = Some(i);
                            ui.close();
                        }
                    }
                }
                ui.separator();
            }
            if let Some(i) = selected {
                if ui.button(sds_i18n::t!("Удалить выбранную", "Delete selected")).clicked() {
                    delete = Some(i);
                    ui.close();
                }
                ui.separator();
            }
            if let Some(v) = add_menu(ui, menu_t, ft, duration, &favs, &mut popup) {
                add_text = Some(v);
            }
        });
        // The wheel zooms time about the pointer; Shift+wheel (horizontal
        // scroll) pans; Ctrl+wheel scrolls the rows.
        if resp.hovered() || ruler_resp.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            let anchor = pointer.or(ruler_resp.hover_pos()).map(|q| t_of(q.x, left)).unwrap_or(off);
            if zoom != 1.0 {
                // egui turns Ctrl+wheel into a zoom factor: back into pixels.
                ui.scroll_with_delta(vec2(0.0, zoom.ln() * 400.0));
            } else if scroll.x != 0.0 {
                zoom_pan = Some((1.0, scroll.x));
            } else if scroll.y != 0.0 {
                zoom_pan = Some(((scroll.y * 0.0025).exp(), anchor));
            }
        }
    });

    // Ruler interaction: scrub.
    if ruler_resp.dragged() || ruler_resp.clicked() {
        if let Some(q) = ruler_resp.interact_pointer_pos() {
            set_time = Some(t_of(q.x, ruler.left()).clamp(0.0, duration));
        }
    }

    // ---- apply ------------------------------------------------------------
    let view = &mut app.timeline;
    view.drag = new_drag;
    if let Some((zoom, v)) = zoom_pan {
        if zoom != 1.0 {
            let anchor = v;
            let factor = zoom;
            let new_px = (view.px_per_sec * factor).clamp(20.0, 20000.0);
            // Keep the time under the pointer fixed.
            view.offset = anchor - (anchor - view.offset) * view.px_per_sec / new_px;
            view.px_per_sec = new_px;
        } else {
            view.offset -= v / view.px_per_sec;
        }
        view.offset = view.offset.clamp(-0.5, duration);
    }
    if let Some(t) = set_time {
        app.play.time = t;
        app.play.playing = false;
    }
    if deselect {
        app.selected = None;
    }
    if let Some(i) = clicked_item {
        app.selected = Some(i);
    }
    if let Some((i, t)) = drop_anno {
        if let Some(doc) = &mut app.doc {
            app.selected = doc.edit(Some(i), |a| a[i].time = t);
        }
    }
    if let Some(i) = delete {
        app.selected = Some(i);
        app.delete_selected();
    }
    if let Some((cat, t)) = add_at {
        app.play.time = t;
        app.add_annotation(template(cat).to_string());
    }
    app.open_popup(popup);
    if let Some(v) = add_text {
        app.add_annotations(v);
    }
}

/// Long annotations (SCAR JSON, Precision chains) shortened for the row.
fn short_label(text: &str) -> String {
    match parse(text).kind {
        Kind::ScarAction(a) => format!("SCAR {} {:.0}–{:.0} {:.0}°..{:.0}°", a.ty, a.min_distance, a.max_distance, a.start_angle, a.end_angle),
        _ => text.to_string(),
    }
}

fn draw_ruler(p: &egui::Painter, r: Rect, duration: f32, ft: f32, px: f32, off: f32, unit: TimeUnit) {
    let left = r.left() + LABEL_W;
    let font = FontId::proportional(10.5);
    p.text(pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, match unit { TimeUnit::Frames => sds_i18n::t!("кадры", "frames"), TimeUnit::Seconds => sds_i18n::t!("секунды", "seconds") }, font.clone(), Color32::from_gray(150));
    // Pick a step that keeps labels >= 60 px apart.
    let step = match unit {
        TimeUnit::Frames => {
            let frames = [1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 150.0, 300.0];
            frames.iter().map(|f| f * ft).find(|s| s * px >= 60.0).unwrap_or(300.0 * ft)
        }
        TimeUnit::Seconds => {
            let secs = [0.01, 0.02, 0.05, 0.1, 0.2, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0];
            *secs.iter().find(|s| *s * px >= 60.0).unwrap_or(&10.0)
        }
    };
    let first = (off / step).floor().max(0.0) as i64;
    let last = ((off + (r.right() - left) / px) / step).ceil() as i64;
    for k in first..=last {
        let t = k as f32 * step;
        if t > duration + 1e-4 {
            break;
        }
        let x = left + (t - off) * px;
        if x < left {
            continue;
        }
        p.line_segment([pos2(x, r.bottom() - 8.0), pos2(x, r.bottom())], Stroke::new(1.0_f32, Color32::from_gray(140)));
        let label = match unit {
            TimeUnit::Frames => format!("{}", (t / ft).round() as i64),
            TimeUnit::Seconds => format!("{:.2}", t),
        };
        p.text(pos2(x + 3.0, r.top() + 8.0), Align2::LEFT_CENTER, label, font.clone(), Color32::from_gray(190));
    }
    let end = left + (duration - off) * px;
    if end >= left {
        p.line_segment([pos2(end, r.top()), pos2(end, r.bottom())], Stroke::new(1.5_f32, Color32::from_rgb(200, 200, 90)));
    }
}

fn transport(app: &mut StudioApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let has = app.doc.is_some();
        ui.add_enabled_ui(has, |ui| {
            if ui.button("⏮").on_hover_text(sds_i18n::t!("В начало (Home)", "To start (Home)")).clicked() {
                app.play.time = 0.0;
            }
            if ui.button("◀|").on_hover_text(sds_i18n::t!("Кадр назад (←)", "Frame back (←)")).clicked() {
                app.step_frames(-1);
            }
            let label = if app.play.playing { "⏸" } else { "▶" };
            if ui.button(label).on_hover_text(sds_i18n::t!("Пуск/пауза (Пробел)", "Play/pause (Space)")).clicked() {
                app.play.playing = !app.play.playing;
            }
            if ui.button("|▶").on_hover_text(sds_i18n::t!("Кадр вперёд (→)", "Frame forward (→)")).clicked() {
                app.step_frames(1);
            }
            if ui.button("⏭").on_hover_text(sds_i18n::t!("В конец (End)", "To end (End)")).clicked() {
                if let Some(d) = &app.doc {
                    app.play.time = d.duration();
                }
            }
            ui.checkbox(&mut app.play.looping, sds_i18n::t!("Цикл", "Loop"));
            ui.checkbox(&mut app.settings.motion_values, sds_i18n::t!("значения root", "root values"))
                .on_hover_text(sds_i18n::t!("Числа animmotion / animrotation под строкой Root motion", "animmotion / animrotation numbers under the Root motion row"));
            ui.add(egui::DragValue::new(&mut app.play.speed).speed(0.01).range(0.05..=4.0).prefix("×"));
            ui.separator();
            let icon = if !app.settings.sound_on || app.settings.volume <= 0.0 { "🔇" } else { "🔊" };
            if ui.selectable_label(app.settings.sound_on, icon).on_hover_text(sds_i18n::t!("Звук: SoundPlay и звуки эффектов PIE при проигрывании (вкл/выкл)", "Sound: SoundPlay and PIE effect sounds during playback (on/off)")).clicked() {
                app.settings.sound_on = !app.settings.sound_on;
            }
            ui.add_enabled(app.settings.sound_on, egui::Slider::new(&mut app.settings.volume, 0.0..=100.0).show_value(true).suffix("%").fixed_decimals(0))
                .on_hover_text(sds_i18n::t!("Громкость", "Volume"));
        });
        ui.separator();
        if let Some(d) = &app.doc {
            let ft = d.frame_time();
            let mut frame = (app.play.time / ft).round() as i64;
            ui.label(sds_i18n::t!("кадр", "frame"));
            if ui.add(egui::DragValue::new(&mut frame).range(0..=d.last_frame() as i64)).changed() {
                app.play.time = (frame as f32 * ft).min(d.duration());
                app.play.playing = false;
            }
            ui.label(format!("/ {}", d.last_frame()));
            let mut sec = app.play.time;
            if ui.add(egui::DragValue::new(&mut sec).speed(0.005).range(0.0..=d.duration()).max_decimals(3).suffix(sds_i18n::t!(" с", " s"))).changed() {
                app.play.time = sec;
                app.play.playing = false;
            }
            ui.label(sds_i18n::tf!("из {:.3} с · {:.1} к/с", "of {:.3} s · {:.1} fps", d.duration(), 1.0 / ft));
        }
        ui.separator();
        ui.selectable_value(&mut app.settings.time_unit, TimeUnit::Frames, sds_i18n::t!("Кадры", "Frames"));
        ui.selectable_value(&mut app.settings.time_unit, TimeUnit::Seconds, sds_i18n::t!("Секунды", "Seconds"));
        ui.checkbox(&mut app.settings.snap_to_frames, sds_i18n::t!("Привязка", "Snap"));
        if ui.button(sds_i18n::t!("Вписать", "Fit")).on_hover_text(sds_i18n::t!("Колесо — масштаб времени, Shift+колесо — прокрутка по времени, Ctrl+колесо — прокрутка строк", "Wheel — time zoom, Shift+wheel — scroll in time, Ctrl+wheel — scroll rows")).clicked() {
            app.timeline.fit_next = true;
        }
    });
}

/// The centred «custom annotation» window: text at the playhead.
pub fn custom_window(app: &mut StudioApp, ctx: &egui::Context) {
    if !app.custom_open {
        return;
    }
    let mut open = true;
    let mut add = false;
    let t = app.snap(app.play.time);
    let ft = app.doc.as_ref().map(|d| d.frame_time()).unwrap_or(1.0 / 30.0);
    egui::Window::new(sds_i18n::t!("Своя аннотация", "Custom annotation"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(sds_i18n::tf!("На плейхед: {:.3} с (кадр {})", "At playhead: {:.3} s (frame {})", t, (t / ft).round()));
            let r = ui.add(egui::TextEdit::singleline(&mut app.custom_anno).hint_text(sds_i18n::t!("текст аннотации…", "annotation text…")).desired_width(380.0));
            if !r.has_focus() && app.custom_anno.is_empty() {
                r.request_focus();
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                if ui.add_enabled(!app.custom_anno.trim().is_empty(), egui::Button::new(sds_i18n::t!("Добавить", "Add"))).clicked() || (enter && !app.custom_anno.trim().is_empty()) {
                    add = true;
                }
                if ui.button(sds_i18n::t!("Отмена", "Cancel")).clicked() {
                    app.custom_open = false;
                }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    if add {
        let text = app.custom_anno.trim().to_string();
        app.add_annotations(vec![(text, t)]);
        app.custom_anno.clear();
        open = false;
    }
    if !open {
        app.custom_open = false;
    }
}

/// The centred «favorite annotations» window: click one to put it at the
/// playhead, ✕ to drop it, «Add Favorite» to add a new one to the list.
pub fn favorites_window(app: &mut StudioApp, ctx: &egui::Context) {
    if !app.favs_open {
        return;
    }
    let mut open = true;
    let mut place: Option<String> = None;
    let mut remove: Option<usize> = None;
    let mut add_new = false;
    let t = app.snap(app.play.time);
    let ft = app.doc.as_ref().map(|d| d.frame_time()).unwrap_or(1.0 / 30.0);
    let has_doc = app.doc.is_some();
    egui::Window::new(sds_i18n::t!("Избранные аннотации", "Favorite annotations"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(420.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(sds_i18n::tf!("На плейхед: {:.3} с (кадр {})", "At playhead: {:.3} s (frame {})", t, (t / ft).round()));
            ui.separator();
            if app.settings.favorites.is_empty() {
                ui.small(sds_i18n::t!("Список пуст — добавьте аннотацию полем ниже.", "The list is empty — add one with the field below."));
            }
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (k, f) in app.settings.favorites.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(has_doc, egui::Button::new(f)).on_hover_text(sds_i18n::t!("Поставить на плейхед", "Put at the playhead")).clicked() {
                            place = Some(f.clone());
                        }
                        if ui.small_button("✕").on_hover_text(sds_i18n::t!("Убрать из избранного", "Remove from favorites")).clicked() {
                            remove = Some(k);
                        }
                    });
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut app.fav_new).hint_text(sds_i18n::t!("новая аннотация…", "new annotation…")).desired_width(300.0));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let ok = !app.fav_new.trim().is_empty();
                if (ui.add_enabled(ok, egui::Button::new("Add Favorite")).on_hover_text(sds_i18n::t!("Добавить в список избранного (на таймлайн не ставится)", "Add to the favorites list (not placed on the timeline)")).clicked() || enter) && ok {
                    add_new = true;
                }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    if add_new {
        let text = app.fav_new.trim().to_string();
        if !app.settings.favorites.contains(&text) {
            app.settings.favorites.push(text);
        }
        app.fav_new.clear();
    }
    if let Some(k) = remove {
        app.settings.favorites.remove(k);
    }
    if let Some(text) = place {
        app.add_annotations(vec![(text, t)]);
        open = false;
    }
    if !open {
        app.favs_open = false;
    }
}
