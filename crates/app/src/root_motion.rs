//! Root Motion Editor: the clip's AMR root motion (`animmotion x y z`) as two
//! Bézier curves, X and Y, in a Blender-style graph editor. Every change is
//! baked back into per-frame `animmotion` keys, so the 3D view, the SDSCombat
//! path and the saved clip follow it live; one drag is one undo step.

use crate::app::StudioApp;
use crate::doc::Anno;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use sds_anno::{parse, Kind};

/// Blender's handle types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Handle {
    /// Auto-clamped: smooth, never overshoots its neighbours.
    Auto,
    /// Both handles stay on one line, lengths free.
    Aligned,
    /// Each handle on its own.
    Free,
    /// Points at the neighbouring key: straight segments.
    Vector,
}

impl Handle {
    pub const ALL: [Handle; 4] = [Handle::Auto, Handle::Aligned, Handle::Free, Handle::Vector];

    pub fn label(self) -> &'static str {
        match self {
            Handle::Auto => sds_i18n::t!("Авто", "Auto"),
            Handle::Aligned => sds_i18n::t!("Выровненные", "Aligned"),
            Handle::Free => sds_i18n::t!("Свободные", "Free"),
            Handle::Vector => sds_i18n::t!("Вектор", "Vector"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    pub t: f32,
    pub v: f32,
    /// Handle offsets from the key in (seconds, units): left has dt <= 0.
    pub hl: [f32; 2],
    pub hr: [f32; 2],
    pub mode: Handle,
    pub sel: bool,
}

impl Key {
    pub fn new(t: f32, v: f32) -> Key {
        Key { t, v, hl: [0.0; 2], hr: [0.0; 2], mode: Handle::Auto, sel: false }
    }
}

/// One channel: keys sorted by time, constant before the first and after
/// the last key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Curve {
    pub keys: Vec<Key>,
}

impl Curve {
    /// Sort the keys and recompute the Auto and Vector handles.
    pub fn update(&mut self) {
        self.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        let n = self.keys.len();
        for i in 0..n {
            let k = self.keys[i];
            let prev = (i > 0).then(|| self.keys[i - 1]);
            let next = (i + 1 < n).then(|| self.keys[i + 1]);
            match k.mode {
                Handle::Auto => {
                    let dl = prev.map(|p| (k.t - p.t) / 3.0);
                    let dr = next.map(|q| (q.t - k.t) / 3.0);
                    let slope = match (prev, next) {
                        (Some(p), Some(q)) if (k.v - p.v) * (q.v - k.v) > 0.0 => {
                            // Clamped so the handles never pass a neighbour's value.
                            let s = (q.v - p.v) / (q.t - p.t).max(1e-6);
                            let lim_l = (k.v - p.v).abs() / dl.unwrap().max(1e-6);
                            let lim_r = (q.v - k.v).abs() / dr.unwrap().max(1e-6);
                            s.signum() * s.abs().min(lim_l).min(lim_r)
                        }
                        _ => 0.0,
                    };
                    let dl = dl.or(dr).unwrap_or(0.1);
                    let dr = dr.unwrap_or(dl);
                    self.keys[i].hl = [-dl, -slope * dl];
                    self.keys[i].hr = [dr, slope * dr];
                }
                Handle::Vector => {
                    let to = |o: Option<Key>, fallback: [f32; 2]| o.map(|o| [(o.t - k.t) / 3.0, (o.v - k.v) / 3.0]).unwrap_or(fallback);
                    let hr = to(next, [0.1, 0.0]);
                    self.keys[i].hl = to(prev, [-hr[0], -hr[1]]);
                    self.keys[i].hr = hr;
                }
                Handle::Aligned | Handle::Free => {
                    self.keys[i].hl[0] = self.keys[i].hl[0].min(0.0);
                    self.keys[i].hr[0] = self.keys[i].hr[0].max(0.0);
                }
            }
        }
    }

    /// The segment's control points with handle times scaled down so the
    /// curve stays a function of time (as Blender does).
    fn segment(&self, i: usize) -> [[f32; 2]; 4] {
        let (a, b) = (self.keys[i], self.keys[i + 1]);
        let span = (b.t - a.t).max(1e-6);
        let (mut hr, mut hl) = (a.hr, b.hl);
        let sum = hr[0] + -hl[0];
        if sum > span {
            let f = span / sum;
            hr = [hr[0] * f, hr[1] * f];
            hl = [hl[0] * f, hl[1] * f];
        }
        [[a.t, a.v], [a.t + hr[0], a.v + hr[1]], [b.t + hl[0], b.v + hl[1]], [b.t, b.v]]
    }

    pub fn eval(&self, t: f32) -> f32 {
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else { return 0.0 };
        if t <= first.t {
            return first.v;
        }
        if t >= last.t {
            return last.v;
        }
        let i = self.keys.windows(2).position(|w| t <= w[1].t).unwrap_or(0);
        let p = self.segment(i);
        let bez = |c: usize, s: f32| {
            let u = 1.0 - s;
            u * u * u * p[0][c] + 3.0 * u * u * s * p[1][c] + 3.0 * u * s * s * p[2][c] + s * s * s * p[3][c]
        };
        // x(s) is monotone after the clamp in `segment`: bisect for s.
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..32 {
            let mid = 0.5 * (lo + hi);
            if bez(0, mid) < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        bez(1, 0.5 * (lo + hi))
    }

    /// Greedy fit of samples: start from the ends, keep adding the sample
    /// the curve misses most until every sample is within `tol`.
    pub fn fit(samples: &[(f32, f32)], tol: f32, max_keys: usize) -> Curve {
        let mut c = Curve::default();
        let (Some(&first), Some(&last)) = (samples.first(), samples.last()) else { return c };
        c.keys.push(Key::new(first.0, first.1));
        if last.0 > first.0 {
            c.keys.push(Key::new(last.0, last.1));
        }
        c.update();
        while c.keys.len() < max_keys {
            let worst = samples
                .iter()
                .filter(|s| !c.keys.iter().any(|k| (k.t - s.0).abs() < 1e-5))
                .map(|&(t, v)| ((c.eval(t) - v).abs(), t, v))
                .max_by(|a, b| a.0.total_cmp(&b.0));
            match worst {
                Some((err, t, v)) if err > tol => {
                    c.keys.push(Key::new(t, v));
                    c.update();
                }
                _ => break,
            }
        }
        c
    }
}

/// What a drag in the graph is doing.
#[derive(Clone)]
enum Drag {
    Keys { start: Pos2, before: [Curve; 2] },
    Handle { ch: usize, key: usize, right: bool },
    Box { start: Pos2 },
    Scrub,
    Pan { last: Pos2 },
}

pub struct RootMotionEditor {
    pub open: bool,
    /// The pointer was over the graph last frame: Delete / Home are ours.
    pub hovered: bool,
    pub curves: [Curve; 2],
    pub show: [bool; 2],
    /// Bake every Nth frame (the last frame always).
    pub step: u32,
    /// Import accuracy, game units.
    pub tolerance: f32,
    /// View: time and value ranges.
    view: Option<([f32; 2], [f32; 2])>,
    drag: Option<Drag>,
    /// Clip name + the moves this editor last imported or baked; anything
    /// else in the clip (undo, a hand edit, another clip) re-imports.
    source: String,
    synced: Vec<(f32, [f32; 3])>,
    /// The path when the clip was first opened here: drawn faint, restorable.
    original: Vec<Anno>,
    original_moves: Vec<(f32, [f32; 3])>,
    /// Z is not edited: carried over from the clip.
    z: Vec<(f32, f32)>,
    undo_key: u32,
}

impl Default for RootMotionEditor {
    fn default() -> Self {
        RootMotionEditor {
            open: false,
            hovered: false,
            curves: Default::default(),
            show: [true, true],
            step: 1,
            tolerance: 2.0,
            view: None,
            drag: None,
            source: String::new(),
            synced: Vec::new(),
            original: Vec::new(),
            original_moves: Vec::new(),
            z: Vec::new(),
            undo_key: 0,
        }
    }
}

const COLORS: [Color32; 2] = [Color32::from_rgb(235, 85, 85), Color32::from_rgb(120, 205, 90)];
const AXES: [&str; 2] = ["X", "Y"];

fn is_motion(a: &Anno) -> bool {
    matches!(parse(&a.text).kind, Kind::AnimMotion(_))
}

/// `12.5`, `-0.0312`, `0`: AMR reads any float.
fn num(v: f32) -> String {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.into() }
}

fn lerp_keys(keys: &[(f32, f32)], t: f32) -> f32 {
    let Some(first) = keys.first() else { return 0.0 };
    if t <= first.0 {
        return if first.0 > 0.0 { first.1 * (t / first.0).max(0.0) } else { first.1 };
    }
    for w in keys.windows(2) {
        if t <= w[1].0 {
            let f = (t - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
            return w[0].1 + (w[1].1 - w[0].1) * f;
        }
    }
    keys.last().unwrap().1
}

impl RootMotionEditor {
    /// Curves from the clip's `animmotion` keys (rest at time 0).
    fn import(&mut self, moves: &[(f32, [f32; 3])], duration: f32) {
        for ch in 0..2 {
            let mut s: Vec<(f32, f32)> = Vec::new();
            if moves.first().map_or(true, |m| m.0 > 1e-4) {
                s.push((0.0, 0.0));
            }
            s.extend(moves.iter().map(|m| (m.0, m.1[ch])));
            if s.len() == 1 {
                s.push((duration, s[0].1));
            }
            self.curves[ch] = Curve::fit(&s, self.tolerance.max(0.01), 32);
        }
        self.z = moves.iter().map(|m| (m.0, m.1[2])).collect();
        self.synced = moves.to_vec();
    }

    /// Per-frame `animmotion` from the curves.
    fn bake(&self, duration: f32, ft: f32, track: u16) -> Vec<Anno> {
        if self.curves.iter().all(|c| c.keys.is_empty()) {
            return Vec::new();
        }
        let last = (duration / ft).round() as u32;
        let step = self.step.max(1);
        let mut out = Vec::new();
        let mut f = 0;
        loop {
            let t = (f as f32 * ft).min(duration);
            let (x, y, z) = (self.curves[0].eval(t), self.curves[1].eval(t), lerp_keys(&self.z, t));
            if f > 0 || x.abs() + y.abs() + z.abs() > 1e-4 {
                out.push(Anno { time: t, text: format!("animmotion {} {} {}", num(x), num(y), num(z)), track });
            }
            if f >= last {
                break;
            }
            f = (f + step).min(last);
        }
        out
    }

    fn write(&mut self, app: &mut StudioApp) {
        let Some(doc) = &mut app.doc else { return };
        let track = doc.annos.iter().find(|a| is_motion(a)).map_or(0, |a| a.track);
        let new = self.bake(doc.duration(), doc.frame_time(), track);
        app.selected = doc.edit_merge(app.selected, &format!("root-motion-{}", self.undo_key), |a| {
            a.retain(|x| !is_motion(x));
            a.extend(new);
        });
        self.synced = doc.motion.moves.clone();
    }

    fn restore(&mut self, app: &mut StudioApp) {
        let Some(doc) = &mut app.doc else { return };
        let orig = self.original.clone();
        app.selected = doc.edit(app.selected, |a| {
            a.retain(|x| !is_motion(x));
            a.extend(orig);
        });
        let (moves, d) = (doc.motion.moves.clone(), doc.duration());
        self.import(&moves, d);
        self.view = None;
    }

    /// Re-import when the clip changed behind the editor's back.
    fn sync(&mut self, app: &StudioApp) {
        let Some(doc) = &app.doc else { return };
        if self.source != doc.name {
            self.source = doc.name.clone();
            self.original = doc.annos.iter().filter(|a| is_motion(a)).cloned().collect();
            self.original_moves = doc.motion.moves.clone();
            self.import(&doc.motion.moves, doc.duration());
            self.view = None;
            self.drag = None;
        } else if self.synced != doc.motion.moves && !matches!(self.drag, Some(Drag::Keys { .. } | Drag::Handle { .. })) {
            self.import(&doc.motion.moves, doc.duration());
        }
    }

    fn frame_all(&mut self, duration: f32) {
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        for (ch, c) in self.curves.iter().enumerate() {
            if !self.show[ch] {
                continue;
            }
            for k in &c.keys {
                for v in [k.v, k.v + k.hl[1], k.v + k.hr[1]] {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            for m in &self.original_moves {
                lo = lo.min(m.1[ch]);
                hi = hi.max(m.1[ch]);
            }
        }
        if hi - lo < 20.0 {
            let mid = 0.5 * (hi + lo);
            (lo, hi) = (mid - 10.0, mid + 10.0);
        }
        let (mt, mv) = (duration.max(0.1) * 0.04, (hi - lo) * 0.1);
        self.view = Some(([-mt, duration.max(0.1) + mt], [lo - mv, hi + mv]));
    }
}

/// "1, 2, 5 × 10^n" step giving at least `px` pixels per step.
fn nice_step(range: f32, pixels: f32, px: f32) -> f32 {
    let raw = range / (pixels / px).max(1.0);
    let p = 10f32.powf(raw.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if p * m >= raw {
            return p * m;
        }
    }
    p * 10.0
}

pub fn window(app: &mut StudioApp, ctx: &egui::Context) {
    if !app.rm.open {
        app.rm.hovered = false;
        return;
    }
    let mut ed = std::mem::take(&mut app.rm);
    ed.sync(app);
    let mut open = true;
    egui::Window::new(sds_i18n::t!("Редактор рут-моушена", "Root Motion Editor"))
        .id(egui::Id::new("root-motion-editor"))
        .open(&mut open)
        .default_size([980.0, 520.0])
        .min_size([520.0, 300.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| body(&mut ed, app, ui));
    ed.open = open;
    app.rm = ed;
}

fn body(ed: &mut RootMotionEditor, app: &mut StudioApp, ui: &mut egui::Ui) {
    let Some(doc) = &app.doc else {
        ui.label(sds_i18n::t!("Откройте клип.", "Open a clip."));
        ed.hovered = false;
        return;
    };
    let (duration, ft) = (doc.duration(), doc.frame_time());
    let moves = doc.motion.moves.clone();
    let has_motion = doc.annos.iter().any(is_motion);
    let mut changed = false;

    // ---- toolbar ----
    ui.horizontal_wrapped(|ui| {
        for ch in 0..2 {
            let text = egui::RichText::new(AXES[ch]).color(COLORS[ch]).strong();
            ui.toggle_value(&mut ed.show[ch], text);
        }
        ui.separator();
        ui.label(sds_i18n::t!("Ручки:", "Handles:"));
        for h in Handle::ALL {
            if ui.button(h.label()).on_hover_text(sds_i18n::t!("Тип ручек выделенных ключей", "Handle type of the selected keys")).clicked() {
                changed |= set_mode(ed, h);
            }
        }
        ui.separator();
        if ui.button(sds_i18n::t!("Ключ на плейхед (I)", "Key at playhead (I)")).clicked() {
            changed |= insert_at(ed, app.play.time, ft, duration);
        }
        if ui.button(sds_i18n::t!("Вписать (Home)", "Frame all (Home)")).clicked() {
            ed.frame_all(duration);
        }
        ui.separator();
        ui.label(sds_i18n::t!("Запекать каждый", "Bake every"));
        if ui.add(egui::DragValue::new(&mut ed.step).range(1..=10).suffix(sds_i18n::t!(" кадр", " frame"))).changed() {
            changed = true;
        }
        ui.label(sds_i18n::t!("Точность", "Accuracy"));
        ui.add(egui::DragValue::new(&mut ed.tolerance).range(0.01..=50.0).speed(0.05).suffix(sds_i18n::t!(" ед.", " u")))
            .on_hover_text(sds_i18n::t!("Допуск, с которым ключи клипа сводятся к кривой", "How close the curve must follow the clip's keys on import"));
        if ui.button(sds_i18n::t!("Переимпорт", "Re-import")).on_hover_text(sds_i18n::t!("Построить кривые заново из animmotion клипа", "Rebuild the curves from the clip's animmotion")).clicked() {
            ed.import(&moves, duration);
            ed.view = None;
        }
    });

    // ---- selected key fields ----
    let selected: Vec<(usize, usize)> = (0..2).flat_map(|ch| ed.curves[ch].keys.iter().enumerate().filter(|(_, k)| k.sel).map(move |(i, _)| (ch, i))).collect();
    ui.horizontal(|ui| {
        match selected.as_slice() {
            [] => {
                ui.weak(sds_i18n::t!("Ключ не выбран", "No key selected"));
            }
            [(ch, i)] => {
                let (ch, i) = (*ch, *i);
                ui.colored_label(COLORS[ch], AXES[ch]);
                let k = &mut ed.curves[ch].keys[i];
                let mut frame = (k.t / ft).round() as i32;
                ui.label(sds_i18n::t!("кадр", "frame"));
                if ui.add(egui::DragValue::new(&mut frame).range(0..=(duration / ft).round() as i32)).changed() {
                    k.t = frame as f32 * ft;
                    changed = true;
                }
                ui.label(sds_i18n::t!("значение", "value"));
                if ui.add(egui::DragValue::new(&mut k.v).speed(0.5).max_decimals(3)).changed() {
                    changed = true;
                }
                ui.label(sds_i18n::tf!("ручки: {}", "handles: {}", k.mode.label()));
            }
            many => {
                ui.label(sds_i18n::tf!("Выбрано ключей: {}", "Keys selected: {}", many.len()));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(!ed.original.is_empty() || has_motion, egui::Button::new(sds_i18n::t!("Вернуть исходный", "Restore original")))
                .on_hover_text(sds_i18n::t!("animmotion клипа, каким он был при открытии", "The clip's animmotion as it was when opened"))
                .clicked()
            {
                ed.restore(app);
                ed.hovered = false;
            }
            let help = sds_i18n::t!(
                "ЛКМ — выбрать/тянуть ключ или ручку · Shift — добавить к выбору · ЛКМ по пустому — рамка · Ctrl+ЛКМ — новый ключ · СКМ — сдвиг вида · колесо — зум (Ctrl — по значению, Shift — по времени) · X/Delete — удалить · A — выделить всё · I — ключ на плейхед · линейка сверху — плейхед",
                "LMB — select/drag a key or handle · Shift — add to selection · LMB on empty — box · Ctrl+LMB — new key · MMB — pan · wheel — zoom (Ctrl — value, Shift — time) · X/Delete — delete · A — select all · I — key at playhead · top ruler — playhead");
            ui.label(sds_i18n::t!("Управление (?)", "Controls (?)")).on_hover_text(help);
        });
    });
    if app.doc.is_none() {
        return;
    }

    // ---- graph + top view ----
    let avail = ui.available_size();
    let hint_h = 4.0;
    let side = (avail.y - hint_h).clamp(120.0, 260.0).min(avail.x * 0.3);
    ui.horizontal(|ui| {
        let size = Vec2::new((avail.x - side - 8.0).max(200.0), (avail.y - hint_h).max(120.0));
        changed |= graph(ed, app, ui, size, duration, ft);
        top_view(ed, app, ui, Vec2::new(side, side));
    });

    if changed {
        for c in &mut ed.curves {
            for k in &mut c.keys {
                k.t = ((k.t / ft).round() * ft).clamp(0.0, duration);
            }
            c.update();
        }
        ed.write(app);
    }
}

fn set_mode(ed: &mut RootMotionEditor, h: Handle) -> bool {
    let mut any = false;
    for c in &mut ed.curves {
        for k in c.keys.iter_mut().filter(|k| k.sel) {
            if h == Handle::Aligned {
                // Line the handles up along their average direction.
                let d = [k.hr[0] - k.hl[0], k.hr[1] - k.hl[1]];
                let len = d[0].hypot(d[1]).max(1e-6);
                let (ll, lr) = (k.hl[0].hypot(k.hl[1]), k.hr[0].hypot(k.hr[1]));
                k.hl = [-d[0] / len * ll, -d[1] / len * ll];
                k.hr = [d[0] / len * lr, d[1] / len * lr];
            }
            k.mode = h;
            any = true;
        }
        c.update();
    }
    any
}

fn insert_at(ed: &mut RootMotionEditor, t: f32, ft: f32, duration: f32) -> bool {
    let t = ((t / ft).round() * ft).clamp(0.0, duration);
    let mut any = false;
    for ch in 0..2 {
        if !ed.show[ch] {
            continue;
        }
        let c = &mut ed.curves[ch];
        let v = c.eval(t);
        c.keys.iter_mut().for_each(|k| k.sel = false);
        match c.keys.iter_mut().find(|k| (k.t - t).abs() < ft * 0.5) {
            Some(k) => k.sel = true,
            None => c.keys.push(Key { sel: true, ..Key::new(t, v) }),
        }
        c.update();
        any = true;
    }
    any
}

/// Keys at the same frame of one channel after a drag: the moved one stays.
fn dedupe(c: &mut Curve, ft: f32) {
    c.keys.sort_by(|a, b| a.t.total_cmp(&b.t).then(b.sel.cmp(&a.sel)));
    c.keys.dedup_by(|b, a| (a.t - b.t).abs() < ft * 0.5 && (a.sel || !b.sel));
    c.update();
}

/// The Blender-style graph. Returns whether the curves changed.
fn graph(ed: &mut RootMotionEditor, app: &mut StudioApp, ui: &mut egui::Ui, size: Vec2, duration: f32, ft: f32) -> bool {
    let (r, painter) = ui.allocate_painter(size, Sense::click_and_drag());
    let rect = r.rect;
    let ruler = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + 18.0));
    let plot = Rect::from_min_max(Pos2::new(rect.min.x + 44.0, ruler.max.y), rect.max);
    if ed.view.is_none() {
        ed.frame_all(duration);
    }
    let ([mut t0, mut t1], [mut v0, mut v1]) = ed.view.unwrap();
    let mut changed = false;
    ed.hovered = r.hovered() || ed.drag.is_some();

    // ---- view: wheel zoom, MMB pan ----
    let pointer = ui.input(|i| i.pointer.hover_pos());
    if let Some(p) = pointer.filter(|p| plot.contains(*p)) {
        // egui turns Ctrl+wheel into zoom_delta and Shift+wheel into x scroll.
        let (scroll, zoom, mods) = ui.input(|i| (i.smooth_scroll_delta.y + i.smooth_scroll_delta.x, i.zoom_delta(), i.modifiers));
        if scroll != 0.0 || zoom != 1.0 {
            let f = if zoom != 1.0 { 1.0 / zoom } else { (-scroll * 0.0025).exp() };
            let ct = t0 + (p.x - plot.left()) / plot.width() * (t1 - t0);
            let cv = v1 - (p.y - plot.top()) / plot.height() * (v1 - v0);
            if !mods.ctrl {
                (t0, t1) = (ct + (t0 - ct) * f, ct + (t1 - ct) * f);
            }
            if !mods.shift {
                (v0, v1) = (cv + (v0 - cv) * f, cv + (v1 - cv) * f);
            }
        }
    }
    let sx = plot.width() / (t1 - t0).max(1e-6);
    let sy = plot.height() / (v1 - v0).max(1e-6);
    let to_screen = move |t: f32, v: f32| Pos2::new(plot.left() + (t - t0) * sx, plot.bottom() - (v - v0) * sy);
    let to_value = move |p: Pos2| (t0 + (p.x - plot.left()) / sx, v0 + (plot.bottom() - p.y) / sy);

    // ---- pointer ----
    let (pressed, released, mods, mmb) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.modifiers, i.pointer.middle_down()));
    let pos = r.interact_pointer_pos().or(pointer);
    if mmb && r.hovered() && ed.drag.is_none() {
        if let Some(p) = pos {
            ed.drag = Some(Drag::Pan { last: p });
        }
    }
    if pressed && r.hovered() {
        if let Some(p) = pos {
            ed.drag = Some(press(ed, p, mods, ruler, &to_screen, &to_value, ft, duration));
            if matches!(ed.drag, Some(Drag::Keys { .. })) {
                // A Ctrl+click key exists now: write it.
                changed |= mods.command;
            }
        }
    }
    match ed.drag.clone() {
        Some(Drag::Pan { last }) => {
            if let Some(p) = pos.filter(|_| mmb) {
                let d = p - last;
                (t0, t1) = (t0 - d.x / sx, t1 - d.x / sx);
                (v0, v1) = (v0 + d.y / sy, v1 + d.y / sy);
                ed.drag = Some(Drag::Pan { last: p });
            } else {
                ed.drag = None;
            }
        }
        Some(Drag::Scrub) => {
            if let Some(p) = pos {
                app.play.playing = false;
                app.play.time = app.snap(to_value(p).0);
            }
        }
        Some(Drag::Keys { start, before }) => {
            if let Some(p) = pos {
                let mut d = p - start;
                if mods.shift {
                    if d.x.abs() > d.y.abs() { d.y = 0.0 } else { d.x = 0.0 }
                }
                if d.length() > 2.0 {
                    let (dt, dv) = (d.x / sx, -d.y / sy);
                    let dt = (dt / ft).round() * ft;
                    for ch in 0..2 {
                        let mut c = before[ch].clone();
                        for k in c.keys.iter_mut().filter(|k| k.sel) {
                            k.t = (k.t + dt).clamp(0.0, duration);
                            k.v += dv;
                        }
                        c.update();
                        if c != ed.curves[ch] {
                            ed.curves[ch] = c;
                            changed = true;
                        }
                    }
                }
            }
        }
        Some(Drag::Handle { ch, key, right }) => {
            if let (Some(p), Some(k)) = (pos, ed.curves[ch].keys.get(key).copied()) {
                let (t, v) = to_value(p);
                let mut off = [t - k.t, v - k.v];
                if right { off[0] = off[0].max(0.0) } else { off[0] = off[0].min(0.0) }
                let mut k2 = k;
                if matches!(k.mode, Handle::Auto | Handle::Vector) {
                    k2.mode = Handle::Aligned;
                }
                let other = if right { k.hl } else { k.hr };
                let other = if k2.mode == Handle::Aligned {
                    // Opposite handle on the same line, its screen length kept.
                    let (ox, oy) = (other[0] * sx, other[1] * sy);
                    let len = ox.hypot(oy);
                    let (dx, dy) = (off[0] * sx, off[1] * sy);
                    let dl = dx.hypot(dy).max(1e-6);
                    [-dx / dl * len / sx, -dy / dl * len / sy]
                } else {
                    other
                };
                if right { (k2.hr, k2.hl) = (off, other) } else { (k2.hl, k2.hr) = (off, other) }
                if k2 != k {
                    ed.curves[ch].keys[key] = k2;
                    ed.curves[ch].update();
                    changed = true;
                }
            }
        }
        Some(Drag::Box { .. }) | None => {}
    }
    if released {
        match ed.drag.take() {
            Some(Drag::Box { start }) => {
                if let Some(p) = pos {
                    let b = Rect::from_two_pos(start, p);
                    for ch in 0..2 {
                        for k in &mut ed.curves[ch].keys {
                            if ed.show[ch] && b.contains(to_screen(k.t, k.v)) {
                                k.sel = true;
                            }
                        }
                    }
                }
            }
            Some(Drag::Keys { .. }) | Some(Drag::Handle { .. }) => {
                for c in &mut ed.curves {
                    dedupe(c, ft);
                }
                ed.undo_key += 1;
                changed = true;
            }
            other => ed.drag = other.filter(|d| matches!(d, Drag::Pan { .. })),
        }
    }

    // ---- keyboard while the pointer is over the graph ----
    if r.hovered() && !ui.ctx().egui_wants_keyboard_input() {
        use egui::{Key as K, Modifiers as M};
        let (del, all, none, home, ins) = ui.input_mut(|i| {
            (
                i.consume_key(M::NONE, K::Delete) | i.consume_key(M::NONE, K::X),
                i.consume_key(M::NONE, K::A),
                i.consume_key(M::ALT, K::A),
                i.consume_key(M::NONE, K::Home),
                i.consume_key(M::NONE, K::I),
            )
        });
        if del {
            for c in &mut ed.curves {
                let n = c.keys.len();
                c.keys.retain(|k| !k.sel);
                changed |= c.keys.len() != n;
                c.update();
            }
            ed.undo_key += 1;
        }
        if all || none {
            for ch in 0..2 {
                let show = ed.show[ch];
                ed.curves[ch].keys.iter_mut().for_each(|k| k.sel = all && show);
            }
        }
        if home {
            ed.frame_all(duration);
            ([t0, t1], [v0, v1]) = ed.view.unwrap();
        }
        if ins {
            changed |= insert_at(ed, app.play.time, ft, duration);
            ed.undo_key += 1;
        }
    }

    // ---- right-click menu ----
    r.context_menu(|ui| {
        for h in Handle::ALL {
            if ui.button(h.label()).clicked() {
                changed |= set_mode(ed, h);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(sds_i18n::t!("Удалить ключи (X)", "Delete keys (X)")).clicked() {
            for c in &mut ed.curves {
                c.keys.retain(|k| !k.sel);
                c.update();
            }
            changed = true;
            ui.close();
        }
    });

    ed.view = Some(([t0, t1], [v0, v1]));
    let sx = plot.width() / (t1 - t0).max(1e-6);
    let sy = plot.height() / (v1 - v0).max(1e-6);
    let to_screen = move |t: f32, v: f32| Pos2::new(plot.left() + (t - t0) * sx, plot.bottom() - (v - v0) * sy);
    draw(ed, app, &painter, rect, ruler, plot, (t0, t1, v0, v1), &to_screen, duration, ft);
    changed
}

#[allow(clippy::too_many_arguments)]
fn press(
    ed: &mut RootMotionEditor,
    p: Pos2,
    mods: egui::Modifiers,
    ruler: Rect,
    to_screen: &dyn Fn(f32, f32) -> Pos2,
    to_value: &dyn Fn(Pos2) -> (f32, f32),
    ft: f32,
    duration: f32,
) -> Drag {
    if ruler.contains(p) {
        return Drag::Scrub;
    }
    const PICK: f32 = 8.0;
    // Handles of selected keys first: they sit on top.
    for ch in 0..2 {
        if !ed.show[ch] {
            continue;
        }
        for (i, k) in ed.curves[ch].keys.iter().enumerate().filter(|(_, k)| k.sel) {
            for right in [false, true] {
                let h = if right { k.hr } else { k.hl };
                if to_screen(k.t + h[0], k.v + h[1]).distance(p) < PICK {
                    return Drag::Handle { ch, key: i, right };
                }
            }
        }
    }
    let hit = (0..2)
        .filter(|&ch| ed.show[ch])
        .flat_map(|ch| ed.curves[ch].keys.iter().enumerate().map(move |(i, k)| (ch, i, *k)))
        .map(|(ch, i, k)| (to_screen(k.t, k.v).distance(p), ch, i))
        .filter(|h| h.0 < PICK)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, ch, i)) = hit {
        let was = ed.curves[ch].keys[i].sel;
        if mods.shift {
            ed.curves[ch].keys[i].sel = !was;
        } else if !was {
            ed.curves.iter_mut().for_each(|c| c.keys.iter_mut().for_each(|k| k.sel = false));
            ed.curves[ch].keys[i].sel = true;
        }
        return Drag::Keys { start: p, before: ed.curves.clone() };
    }
    if mods.command {
        // Ctrl+click: a key on the visible curve nearest to the pointer.
        let (t, v) = to_value(p);
        let t = ((t / ft).round() * ft).clamp(0.0, duration);
        let ch = (0..2).filter(|&ch| ed.show[ch]).min_by(|&a, &b| (ed.curves[a].eval(t) - v).abs().total_cmp(&(ed.curves[b].eval(t) - v).abs()));
        if let Some(ch) = ch {
            ed.curves.iter_mut().for_each(|c| c.keys.iter_mut().for_each(|k| k.sel = false));
            let c = &mut ed.curves[ch];
            c.keys.retain(|k| (k.t - t).abs() >= ft * 0.5);
            c.keys.push(Key { sel: true, ..Key::new(t, v) });
            c.update();
            ed.undo_key += 1;
            return Drag::Keys { start: p, before: ed.curves.clone() };
        }
    }
    if !mods.shift {
        ed.curves.iter_mut().for_each(|c| c.keys.iter_mut().for_each(|k| k.sel = false));
    }
    Drag::Box { start: p }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    ed: &RootMotionEditor,
    app: &StudioApp,
    painter: &egui::Painter,
    rect: Rect,
    ruler: Rect,
    plot: Rect,
    (t0, t1, v0, v1): (f32, f32, f32, f32),
    to_screen: &dyn Fn(f32, f32) -> Pos2,
    duration: f32,
    ft: f32,
) {
    let vis = ui_visuals(painter);
    painter.rect_filled(rect, 2.0, vis.extreme_bg_color);
    let p = painter.with_clip_rect(rect);
    let grid = vis.widgets.noninteractive.bg_stroke.color;
    let text = vis.weak_text_color();
    let font = egui::FontId::monospace(10.0);

    // Outside the clip.
    let a = to_screen(0.0, 0.0).x;
    let b = to_screen(duration, 0.0).x;
    let shade = Color32::from_black_alpha(60);
    p.rect_filled(Rect::from_min_max(plot.min, Pos2::new(a.max(plot.left()), plot.bottom())), 0.0, shade);
    p.rect_filled(Rect::from_min_max(Pos2::new(b.min(plot.right()), plot.top()), plot.max), 0.0, shade);

    // Frames.
    let fstep = (nice_step((t1 - t0) / ft, plot.width(), 48.0)).max(1.0);
    let mut f = ((t0 / ft) / fstep).floor() * fstep;
    while f * ft <= t1 {
        let x = to_screen(f * ft, 0.0).x;
        if x >= plot.left() {
            p.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.0_f32, grid));
            p.text(Pos2::new(x + 2.0, ruler.center().y), egui::Align2::LEFT_CENTER, format!("{}", f as i32), font.clone(), text);
        }
        f += fstep;
    }
    // Values.
    let vstep = nice_step(v1 - v0, plot.height(), 28.0);
    let mut v = (v0 / vstep).floor() * vstep;
    while v <= v1 {
        let y = to_screen(0.0, v).y;
        if y <= plot.bottom() && y >= plot.top() {
            let stroke = if v.abs() < vstep * 0.01 { Stroke::new(1.0_f32, vis.text_color().gamma_multiply(0.5)) } else { Stroke::new(1.0_f32, grid) };
            p.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], stroke);
            p.text(Pos2::new(plot.left() - 4.0, y), egui::Align2::RIGHT_CENTER, num(v), font.clone(), text);
        }
        v += vstep;
    }
    p.line_segment([Pos2::new(rect.left(), ruler.bottom()), Pos2::new(rect.right(), ruler.bottom())], Stroke::new(1.0_f32, grid));

    let plot_p = painter.with_clip_rect(plot);
    for ch in 0..2 {
        if !ed.show[ch] {
            continue;
        }
        let col = COLORS[ch];
        // The clip's path when opened, faint.
        if !ed.original_moves.is_empty() {
            let mut pts = vec![to_screen(0.0, 0.0)];
            pts.extend(ed.original_moves.iter().map(|m| to_screen(m.0, m.1[ch])));
            plot_p.add(egui::Shape::dashed_line(&pts, Stroke::new(1.0_f32, col.gamma_multiply(0.4)), 4.0, 3.0));
        }
        // The curve, a point every 2 px.
        let c = &ed.curves[ch];
        let n = (plot.width() / 2.0) as usize + 1;
        let pts: Vec<Pos2> = (0..=n).map(|i| {
            let t = t0 + (t1 - t0) * i as f32 / n as f32;
            to_screen(t, c.eval(t))
        }).collect();
        plot_p.add(egui::Shape::line(pts, Stroke::new(2.0_f32, col)));
        for k in &c.keys {
            let kp = to_screen(k.t, k.v);
            if k.sel {
                for h in [k.hl, k.hr] {
                    let hp = to_screen(k.t + h[0], k.v + h[1]);
                    plot_p.line_segment([kp, hp], Stroke::new(1.0_f32, Color32::from_gray(200)));
                    plot_p.circle_filled(hp, 3.5, Color32::from_gray(220));
                }
            }
            let fill = if k.sel { Color32::from_rgb(255, 170, 60) } else { Color32::from_gray(20) };
            let r = Rect::from_center_size(kp, Vec2::splat(7.0));
            plot_p.add(egui::Shape::convex_polygon(vec![r.center_top(), r.right_center(), r.center_bottom(), r.left_center()], fill, Stroke::new(1.0_f32, col)));
        }
    }

    // Box selection.
    if let (Some(Drag::Box { start }), Some(cur)) = (&ed.drag, painter.ctx().input(|i| i.pointer.hover_pos())) {
        let b = Rect::from_two_pos(*start, cur);
        p.rect_filled(b, 0.0, vis.selection.bg_fill.gamma_multiply(0.2));
        p.rect_stroke(b, 0.0, Stroke::new(1.0_f32, vis.selection.stroke.color), egui::StrokeKind::Inside);
    }

    // Playhead.
    let x = to_screen(app.play.time, 0.0).x;
    let ph = Color32::from_rgb(90, 150, 255);
    p.line_segment([Pos2::new(x, ruler.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.5_f32, ph));
    let label = format!("{}", (app.play.time / ft).round() as i32);
    let galley = p.layout_no_wrap(label, font, Color32::WHITE);
    let lr = Rect::from_center_size(Pos2::new(x, ruler.center().y), galley.size() + Vec2::new(8.0, 2.0));
    p.rect_filled(lr, 3.0, ph);
    p.galley(lr.min + Vec2::new(4.0, 1.0), galley, Color32::WHITE);
}

fn ui_visuals(painter: &egui::Painter) -> egui::Visuals {
    painter.ctx().global_style().visuals.clone()
}

/// The path seen from above (+Y ahead), NPC at the playhead.
fn top_view(ed: &RootMotionEditor, app: &StudioApp, ui: &mut egui::Ui, size: Vec2) {
    let (r, p) = ui.allocate_painter(size, Sense::hover());
    let rect = r.rect;
    let vis = ui.visuals().clone();
    p.rect_filled(rect, 2.0, vis.extreme_bg_color);
    let Some(doc) = &app.doc else { return };
    let duration = doc.duration();
    let n = 120;
    let path: Vec<[f32; 2]> = (0..=n).map(|i| {
        let t = duration * i as f32 / n as f32;
        [ed.curves[0].eval(t), ed.curves[1].eval(t)]
    }).collect();
    let mut orig = vec![[0.0f32, 0.0f32]];
    orig.extend(ed.original_moves.iter().map(|m| [m.1[0], m.1[1]]));
    let (mut lo, mut hi) = ([0.0f32; 2], [0.0f32; 2]);
    for q in path.iter().chain(&orig) {
        for c in 0..2 {
            lo[c] = lo[c].min(q[c]);
            hi[c] = hi[c].max(q[c]);
        }
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(40.0) * 1.15;
    let mid = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let k = rect.width().min(rect.height()) / span;
    let at = |q: [f32; 2]| Pos2::new(rect.center().x + (q[0] - mid[0]) * k, rect.center().y - (q[1] - mid[1]) * k);
    let grid = vis.widgets.noninteractive.bg_stroke.color;
    p.line_segment([at([lo[0] - span, 0.0]), at([hi[0] + span, 0.0])], Stroke::new(1.0_f32, grid));
    p.line_segment([at([0.0, lo[1] - span]), at([0.0, hi[1] + span])], Stroke::new(1.0_f32, grid));
    let pc = painter_clip(&p, rect);
    if orig.len() > 1 {
        pc.add(egui::Shape::dashed_line(&orig.iter().map(|q| at(*q)).collect::<Vec<_>>(), Stroke::new(1.0_f32, vis.weak_text_color()), 4.0, 3.0));
    }
    pc.add(egui::Shape::line(path.iter().map(|q| at(*q)).collect(), Stroke::new(2.0_f32, Color32::from_rgb(240, 200, 80))));
    let t = app.play.time;
    let now = at([ed.curves[0].eval(t), ed.curves[1].eval(t)]);
    pc.circle_filled(at([0.0, 0.0]), 3.0, vis.weak_text_color());
    pc.circle_filled(now, 5.0, Color32::from_rgb(90, 150, 255));
    p.text(rect.left_top() + Vec2::new(4.0, 2.0), egui::Align2::LEFT_TOP, sds_i18n::t!("сверху, +Y вперёд", "top, +Y ahead"), egui::FontId::proportional(10.0), vis.weak_text_color());
    p.text(rect.left_bottom() + Vec2::new(4.0, -2.0), egui::Align2::LEFT_BOTTOM, format!("x {}  y {}", num(ed.curves[0].eval(t)), num(ed.curves[1].eval(t))), egui::FontId::monospace(10.0), vis.text_color());
}

fn painter_clip(p: &egui::Painter, rect: Rect) -> egui::Painter {
    p.with_clip_rect(rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_hits_keys_and_stays_flat_outside() {
        let mut c = Curve { keys: vec![Key::new(0.0, 0.0), Key::new(0.5, 40.0), Key::new(1.0, 100.0)] };
        c.update();
        assert!((c.eval(0.5) - 40.0).abs() < 1e-3);
        assert!((c.eval(1.0) - 100.0).abs() < 1e-3);
        assert_eq!(c.eval(-1.0), 0.0);
        assert_eq!(c.eval(2.0), 100.0);
        // Auto-clamped: monotone data gives a monotone curve.
        let mut last = -1.0;
        for i in 0..=100 {
            let v = c.eval(i as f32 / 100.0);
            assert!(v >= last - 1e-3);
            last = v;
        }
    }

    #[test]
    fn vector_handles_are_straight() {
        let mut c = Curve { keys: vec![Key::new(0.0, 0.0), Key::new(1.0, 30.0)] };
        c.keys.iter_mut().for_each(|k| k.mode = Handle::Vector);
        c.update();
        assert!((c.eval(0.25) - 7.5).abs() < 1e-2);
    }

    #[test]
    fn fit_follows_samples() {
        let s: Vec<(f32, f32)> = (0..=30).map(|i| {
            let t = i as f32 / 30.0;
            (t, 150.0 * (t * std::f32::consts::PI).sin())
        }).collect();
        let c = Curve::fit(&s, 0.5, 48);
        assert!(c.keys.len() < 20);
        for (t, v) in &s {
            assert!((c.eval(*t) - v).abs() <= 0.5 + 1e-3, "{t}: {} vs {v}", c.eval(*t));
        }
    }

    #[test]
    fn bake_writes_every_frame() {
        let mut ed = RootMotionEditor::default();
        ed.import(&[(0.5, [0.0, 50.0, 1.0]), (1.0, [10.0, 100.0, 2.0])], 1.0);
        let out = ed.bake(1.0, 0.1, 0);
        assert_eq!(out.len(), 10);
        assert_eq!(out.last().unwrap().text, "animmotion 10 100 2");
        assert!(out.iter().all(|a| is_motion(a)));
    }
}
