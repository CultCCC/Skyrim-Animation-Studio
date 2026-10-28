//! "Combo graph" tab: the SCAR moveset as a graph (clips → ActionData →
//! clips), filtered by where the target stands, with a step simulator.

use crate::app::StudioApp;
use crate::combo::{self, Graph, ScarActionExt, Sim, Step};
use crate::doc::ClipSource;
use crate::oar::Place;
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use std::collections::HashMap;

const NODE_W: f32 = 180.0;
const NODE_H: f32 = 46.0;

pub struct ComboState {
    pub contexts: Vec<combo::Context>,
    pub ctx: usize,
    pub graph: Option<Graph>,
    /// The context key the graph was built for.
    built: String,
    from_open: bool,
    /// Graph-space node positions (draggable).
    pos: HashMap<usize, Pos2>,
    pan: Vec2,
    zoom: f32,
    pub filter: bool,
    labels: bool,
    pub dist: f32,
    pub angle: f32,
    selected_edge: Option<usize>,
    drag_node: Option<usize>,
    pub sim: Sim,
    step_pending: bool,
    /// Steps to run on their own (the `--combo-sim` screenshot check).
    pub auto_steps: u32,
    /// Show only the clips reachable from the focus within `depth` steps.
    pub only_next: bool,
    pub depth: usize,
    /// The clicked clip; without one, the clip open in the studio.
    focus: Option<usize>,
    /// Layout of the focused view, rebuilt when (focus, depth) changes.
    focus_pos: HashMap<usize, Pos2>,
    focus_key: Option<(usize, usize, Vec<usize>)>,
    /// Clips walked through before the focus, oldest first: they stay on
    /// screen to the left with the edges between them.
    trail: Vec<usize>,
    /// (open clip, signature of its combo annotations): an edit rebuilds.
    doc_sig: (String, u64),
}

impl Default for ComboState {
    fn default() -> Self {
        ComboState {
            contexts: Vec::new(),
            ctx: 0,
            graph: None,
            built: String::new(),
            from_open: false,
            pos: HashMap::new(),
            pan: vec2(70.0, 30.0),
            zoom: 0.85,
            filter: false,
            labels: true,
            dist: 250.0,
            angle: 0.0,
            selected_edge: None,
            drag_node: None,
            sim: Sim { auto: true, use_path: true, ..Default::default() },
            step_pending: false,
            auto_steps: 0,
            only_next: false,
            depth: 1,
            focus: None,
            focus_pos: HashMap::new(),
            focus_key: None,
            trail: Vec::new(),
            doc_sig: (String::new(), 0),
        }
    }
}

fn open_place(app: &mut StudioApp, place: &Place, select: Option<(String, f32)>) {
    app.select_after_open = select;
    match place {
        Place::Vfs(p) => app.defer(crate::app::Deferred::Open(ClipSource::Vfs(p.clone()))),
        Place::Disk(p) => app.defer(crate::app::Deferred::Open(ClipSource::Disk(p.clone()))),
    }
}

fn short(name: &str) -> &str {
    name.strip_suffix(".hkx").unwrap_or(name)
}

fn edge_label(e: &combo::Edge) -> String {
    let a = &e.action;
    let ang = if a.end_angle - a.start_angle >= 360.0 { "360°".to_string() } else { format!("{:+.0}°..{:+.0}°", a.start_angle, a.end_angle) };
    let v = if e.variant != 0 { format!(" V{}", e.variant) } else { String::new() };
    format!("{} {:.0}–{:.0} {} {:.0}%{}", a.ty, a.min_distance, a.max_distance, ang, a.chance, v)
}

/// Layered layout: one column per BFS depth.
fn layout(g: &Graph) -> HashMap<usize, Pos2> {
    let mut cols: Vec<Vec<usize>> = Vec::new();
    for (i, &d) in g.depth.iter().enumerate() {
        let d = if d == usize::MAX { g.depth.iter().filter(|&&x| x != usize::MAX).max().map(|m| m + 1).unwrap_or(0) } else { d };
        if cols.len() <= d {
            cols.resize(d + 1, Vec::new());
        }
        cols[d].push(i);
    }
    let mut out = HashMap::new();
    for (c, col) in cols.iter().enumerate() {
        for (r, &i) in col.iter().enumerate() {
            out.insert(i, pos2(c as f32 * (NODE_W + 190.0), r as f32 * (NODE_H + 90.0)));
        }
    }
    out
}

fn bezier(p: [Pos2; 4], t: f32) -> Pos2 {
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    pos2(
        w[0] * p[0].x + w[1] * p[1].x + w[2] * p[2].x + w[3] * p[3].x,
        w[0] * p[0].y + w[1] * p[1].y + w[2] * p[2].y + w[3] * p[3].y,
    )
}

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui) {
    let mut st = std::mem::take(&mut app.combo);
    show_inner(app, &mut st, ui);
    app.combo = st;
}

fn rebuild(app: &StudioApp, st: &mut ComboState) {
    let Some(ctx) = st.contexts.get(st.ctx) else { return };
    let start = if st.from_open { app.doc.as_ref().map(|d| d.name.to_ascii_lowercase()) } else { None };
    let g = combo::build(app, ctx, start.as_deref());
    st.pos = layout(&g);
    st.built = ctx.key.clone();
    st.selected_edge = None;
    st.focus = None;
    st.focus_key = None;
    st.trail.clear();
    st.sim = Sim { auto: st.sim.auto, use_path: st.sim.use_path, ..Default::default() };
    st.graph = Some(g);
}

/// The open clip and a hash of what the graph reads from it (ActionData,
/// ComboStart, MCO_next*).
fn doc_signature(app: &StudioApp) -> (String, u64) {
    use std::hash::{Hash, Hasher};
    let Some(d) = &app.doc else { return (String::new(), 0) };
    let src = match &d.source {
        ClipSource::Vfs(p) => p.to_ascii_lowercase(),
        ClipSource::Disk(p) => p.display().to_string(),
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for a in &d.annos {
        let l = a.text.to_ascii_lowercase();
        if l.starts_with("scar_") || l.contains("mco_next") {
            a.text.hash(&mut h);
            a.time.to_bits().hash(&mut h);
        }
    }
    (src, h.finish())
}

/// Rebuilds after an edit, keeping positions, focus, path and the
/// simulator's place (node indices may change, clips are matched by name
/// and submod).
fn refresh_keep(app: &StudioApp, st: &mut ComboState) {
    let Some(old) = st.graph.clone() else { return rebuild(app, st) };
    let key = |g: &Graph, i: usize| (g.nodes[i].name.to_ascii_lowercase(), g.nodes[i].sub);
    let pos: HashMap<(String, usize), Pos2> = st.pos.iter().filter(|(i, _)| **i < old.nodes.len()).map(|(i, p)| (key(&old, *i), *p)).collect();
    let fpos: HashMap<(String, usize), Pos2> = st.focus_pos.iter().filter(|(i, _)| **i < old.nodes.len()).map(|(i, p)| (key(&old, *i), *p)).collect();
    let focus = st.focus.filter(|&i| i < old.nodes.len()).map(|i| key(&old, i));
    let trail: Vec<(String, usize)> = st.trail.iter().filter(|&&i| i < old.nodes.len()).map(|&i| key(&old, i)).collect();
    let current = st.sim.current.filter(|&i| i < old.nodes.len()).map(|i| key(&old, i));
    let (sim_auto, sim_path, chain_len) = (st.sim.auto, st.sim.use_path, st.sim.chain.len());
    let pan = st.pan;
    rebuild(app, st);
    let Some(g) = &st.graph else { return };
    let find = |k: &(String, usize)| (0..g.nodes.len()).find(|&i| key(g, i) == *k);
    let fresh = st.pos.clone();
    st.pos = (0..g.nodes.len()).map(|i| (i, pos.get(&key(g, i)).copied().unwrap_or(fresh[&i]))).collect();
    st.focus_pos = (0..g.nodes.len()).filter_map(|i| fpos.get(&key(g, i)).map(|p| (i, *p))).collect();
    st.focus = focus.as_ref().and_then(find);
    st.trail = trail.iter().filter_map(find).collect();
    st.sim.current = current.as_ref().and_then(find);
    st.sim.auto = sim_auto;
    st.sim.use_path = sim_path;
    st.pan = pan;
    // Keep the focused layout when the same clips stay on screen.
    st.focus_key = None;
    if chain_len > 0 {
        st.sim.chain.clear();
    }
}

fn refresh_contexts(app: &StudioApp, st: &mut ComboState) {
    let Some(l) = &app.loaded else { return };
    st.contexts = combo::contexts(l.library.oar.iter().chain(app.oar_external.iter()));
    // The moveset of the open clip.
    if let Some(ClipSource::Vfs(p)) = app.doc.as_ref().map(|d| &d.source) {
        let p = p.to_ascii_lowercase();
        // States stack, so many hold the clip's submod: take the most general.
        if let Some((i, _)) = st
            .contexts
            .iter()
            .enumerate()
            .filter(|(_, c)| c.subs.iter().any(|s| p.starts_with(&format!("{}/", s.folder.to_ascii_lowercase()))))
            .min_by_key(|(_, c)| c.conds.len())
        {
            st.ctx = i;
        }
    }
}

fn show_inner(app: &mut StudioApp, st: &mut ComboState, ui: &mut egui::Ui) {
    if app.loaded.is_none() {
        ui.label(sds_i18n::t!("Сборка не загружена", "Build not loaded"));
        return;
    }
    if st.contexts.is_empty() {
        refresh_contexts(app, st);
    }

    // ---- toolbar ----
    ui.horizontal_wrapped(|ui| {
        ui.label(sds_i18n::t!("Набор:", "Moveset:"));
        let cur = st.contexts.get(st.ctx).map(|c| c.label.clone()).unwrap_or_default();
        egui::ComboBox::from_id_salt("combo-ctx").width(360.0).selected_text(cur).show_ui(ui, |ui| {
            for (i, c) in st.contexts.iter().enumerate() {
                ui.selectable_value(&mut st.ctx, i, &c.label);
            }
        });
        if ui.button(sds_i18n::t!("К открытому клипу", "Open clip's moveset")).on_hover_text(sds_i18n::t!("Выбрать набор, в котором лежит открытый клип", "Pick the moveset the open clip belongs to")).clicked() {
            refresh_contexts(app, st);
        }
        ui.checkbox(&mut st.from_open, sds_i18n::t!("от открытого клипа", "from the open clip"))
            .on_hover_text(sds_i18n::t!("Строить граф от открытого клипа, а не от scar_* dummy", "Build from the open clip instead of the scar_* dummy"));
        if ui.button(sds_i18n::t!("⟳ Построить", "⟳ Build")).clicked() {
            rebuild(app, st);
        }
        ui.checkbox(&mut st.labels, sds_i18n::t!("подписи рёбер", "edge labels"));
        ui.separator();
        ui.checkbox(&mut st.only_next, sds_i18n::t!("Только следующие атаки", "Show only next combo attacks")).on_hover_text(sds_i18n::t!(
            "Скрыть клипы, не участвующие в цепочке выбранного клипа (клик по узлу) или открытого в студии",
            "Hide clips outside the chain of the selected clip (click a node) or the one open in the studio"
        ));
        ui.add_enabled(st.only_next, egui::DragValue::new(&mut st.depth).range(1..=6).prefix(sds_i18n::t!("шагов: ", "steps: ")));
        if ui.button(sds_i18n::t!("Раскладка", "Layout")).on_hover_text(sds_i18n::t!("Расставить узлы заново по глубине", "Re-arrange nodes by depth")).clicked() {
            if let Some(g) = &st.graph {
                st.pos = layout(g);
            }
        }
    });
    let ctx_key = st.contexts.get(st.ctx).map(|c| c.key.clone()).unwrap_or_default();
    if st.graph.is_none() || st.built != ctx_key {
        rebuild(app, st);
        st.doc_sig = doc_signature(app);
    } else {
        // The open clip's combo annotations changed (an ActionData deleted,
        // a transition edited): re-read the moveset.
        let sig = doc_signature(app);
        if sig != st.doc_sig {
            let edited = sig.0 == st.doc_sig.0;
            st.doc_sig = sig;
            let in_graph = st.graph.as_ref().is_some_and(|g| (0..g.nodes.len()).any(|i| combo::is_open(app, g, i)));
            if edited && in_graph {
                refresh_keep(app, st);
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut st.filter, sds_i18n::t!("Цель:", "Target:")).on_hover_text(sds_i18n::t!(
            "Показать шансы переходов для цели на этой дистанции и угле (+ — справа)",
            "Show transition odds for a target at this distance and angle (+ — right)"
        ));
        ui.add(egui::DragValue::new(&mut st.dist).range(0.0..=5000.0).speed(5.0).suffix(sds_i18n::t!(" ед", " u")));
        ui.add(egui::DragValue::new(&mut st.angle).range(-180.0..=180.0).speed(1.0).suffix("°"));
        if ui.button(sds_i18n::t!("из сцены", "from scene")).clicked() {
            st.dist = app.scene.target_dist;
            st.angle = app.scene.target_angle;
            st.filter = true;
        }
        ui.separator();
        ui.small(sds_i18n::t!(
            "Выбор SCAR: время ActionData — вес; действия в зоне перебираются от самого позднего, каждое бросает Chance, первое удачное побеждает.",
            "SCAR choice: the ActionData time is its weight; in-range actions are tried from the latest, each rolls Chance, the first success wins."
        ));
    });

    let Some(g) = st.graph.clone() else { return };
    if st.only_next {
        let focus = st.focus.filter(|&f| f < g.nodes.len()).or_else(|| (0..g.nodes.len()).find(|&i| combo::is_open(app, &g, i)));
        ui.horizontal_wrapped(|ui| {
            ui.label(sds_i18n::t!("Путь:", "Path:"));
            let mut jump = None;
            for (k, &n) in st.trail.iter().enumerate() {
                if ui.link(short(&g.nodes[n].name)).on_hover_text(sds_i18n::t!("Вернуться к этому клипу", "Go back to this clip")).clicked() {
                    jump = Some(k);
                }
                ui.label("→");
            }
            match focus {
                Some(f) => {
                    ui.strong(short(&g.nodes[f].name));
                }
                None => {
                    ui.small(sds_i18n::t!("кликните узел или откройте клип", "click a node or open a clip"));
                }
            }
            if let Some(k) = jump {
                st.focus = Some(st.trail[k]);
                st.trail.truncate(k);
                open_place(app, &g.nodes[st.focus.unwrap()].place.clone(), None);
            }
            if !st.trail.is_empty() && ui.small_button(sds_i18n::t!("⟲ сбросить путь", "⟲ clear path")).clicked() {
                st.trail.clear();
            }
        });
    }
    for n in &g.notes {
        ui.colored_label(Color32::from_rgb(230, 180, 90), n);
    }
    let ctx = st.contexts.get(st.ctx).cloned();

    // ---- simulator (right) + canvas ----
    egui::Panel::right("combo-sim").resizable(true).default_size(300.0).show_inside(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| simulator(ui, app, st, &g, ctx.as_ref()));
    });
    canvas(ui, app, st, &g, ctx.as_ref());
}

fn sub_label(ctx: Option<&combo::Context>, sub: usize) -> String {
    ctx.and_then(|c| c.subs.get(sub)).map(|s| match s.variant {
        Some(v) => format!("{} · V{v}", s.name),
        None => s.name.clone(),
    }).unwrap_or_default()
}

fn canvas(ui: &mut egui::Ui, app: &mut StudioApp, st: &mut ComboState, g: &Graph, ctx: Option<&combo::Context>) {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, Color32::from_rgb(24, 24, 28));
    let pointer = resp.hover_pos();

    // Zoom about the pointer.
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let old = st.zoom;
            st.zoom = (st.zoom * (1.0 + scroll * 0.0015)).clamp(0.3, 2.5);
            if let Some(q) = pointer {
                let rel = q - rect.min - st.pan;
                st.pan += rel - rel * (st.zoom / old);
            }
        }
    }
    // Focused view: the focus clip and what it leads to within `depth` steps.
    let focus = st.focus.filter(|&f| f < g.nodes.len()).or_else(|| (0..g.nodes.len()).find(|&i| combo::is_open(app, g, i)));
    let level: Option<HashMap<usize, usize>> = match (st.only_next, focus) {
        (true, Some(f)) => {
            let mut lv = HashMap::from([(f, 0usize)]);
            let mut frontier = vec![f];
            for d in 1..=st.depth {
                let mut next = Vec::new();
                for &n in &frontier {
                    for k in combo::scar_order(g, n) {
                        if let Some(t) = g.edges[k].to {
                            if !lv.contains_key(&t) {
                                lv.insert(t, d);
                                next.push(t);
                            }
                        }
                    }
                }
                frontier = next;
            }
            Some(lv)
        }
        _ => None,
    };
    let trail: Vec<usize> = if level.is_some() { st.trail.iter().copied().filter(|&n| n < g.nodes.len() && Some(n) != focus).collect() } else { Vec::new() };
    if let (Some(lv), Some(f)) = (&level, focus) {
        let key = (f, st.depth, trail.clone());
        if st.focus_key.as_ref() != Some(&key) {
            st.focus_key = Some(key);
            let shift = trail.len() as f32;
            let mut cols: Vec<Vec<usize>> = vec![Vec::new(); st.depth + 1];
            // Column order: the order SCAR tries the edges.
            let mut order = vec![f];
            let mut i = 0;
            while i < order.len() {
                for k in combo::scar_order(g, order[i]) {
                    if let Some(t) = g.edges[k].to {
                        if lv.contains_key(&t) && !order.contains(&t) {
                            order.push(t);
                        }
                    }
                }
                i += 1;
            }
            for n in order {
                // A clip already on the path keeps its place there.
                if !trail.contains(&n) {
                    cols[lv[&n]].push(n);
                }
            }
            st.focus_pos.clear();
            let col_w = NODE_W + 230.0;
            for (k, &n) in trail.iter().enumerate() {
                st.focus_pos.insert(n, pos2(k as f32 * col_w, 0.0));
            }
            for (c, col) in cols.iter().enumerate() {
                let h = col.len() as f32;
                for (r, &n) in col.iter().enumerate() {
                    st.focus_pos.insert(n, pos2((c as f32 + shift) * col_w, (r as f32 - (h - 1.0) * 0.5) * (NODE_H + 70.0)));
                }
            }
            // The focus a third into the canvas, the path to its left.
            let fx = shift * col_w * st.zoom;
            st.pan = vec2((rect.width() * 0.3 - fx).min(70.0), rect.height() * 0.5 - NODE_H * 0.5 * st.zoom);
        }
    }
    // Consecutive path steps: trail[0] → trail[1] → … → focus.
    let path_pairs: Vec<(usize, usize)> = match focus {
        Some(f) if level.is_some() => trail.iter().copied().zip(trail.iter().copied().skip(1).chain(std::iter::once(f))).collect(),
        _ => Vec::new(),
    };
    let on_path = |e: &combo::Edge| e.to.is_some_and(|t| path_pairs.contains(&(e.from, t)));
    let visible_node = |i: usize| level.as_ref().is_none_or(|lv| lv.contains_key(&i) || trail.contains(&i));
    // An edge shows when it leaves a clip short of the last step and lands
    // on a shown clip (or nowhere: IDLE, unmapped), or when it is a step of
    // the path walked so far.
    let visible_edge = |e: &combo::Edge| match &level {
        None => true,
        Some(lv) => on_path(e) || (lv.get(&e.from).is_some_and(|&d| d < st.depth) && !trail.contains(&e.from) && e.to.is_none_or(|t| lv.contains_key(&t) || trail.contains(&t))),
    };
    let positions: HashMap<usize, Pos2> = if level.is_some() { st.focus_pos.clone() } else { st.pos.clone() };

    let z = st.zoom;
    let to_screen = |q: Pos2| rect.min + st.pan + q.to_vec2() * z;

    let node_rect = |i: usize| -> Rect {
        let q = positions.get(&i).copied().unwrap_or(Pos2::ZERO);
        Rect::from_min_size(to_screen(q), vec2(NODE_W, NODE_H) * z)
    };
    let current = st.sim.current;
    let odds: HashMap<usize, f32> = if st.filter {
        (0..g.nodes.len()).flat_map(|n| combo::odds(g, n, st.dist, st.angle)).collect()
    } else {
        HashMap::new()
    };
    let last_edge = st.sim.chain.iter().rev().find_map(|s| s.edge);

    // Edges.
    let font = FontId::proportional(11.0 * z.max(0.7));
    let mut hovered_edge: Option<usize> = None;
    let mut pair_count: HashMap<(usize, Option<usize>), usize> = HashMap::new();
    let mut dangling_row: HashMap<usize, usize> = HashMap::new();
    for (k, e) in g.edges.iter().enumerate() {
        if !visible_edge(e) {
            continue;
        }
        let a = node_rect(e.from);
        let slot = {
            let c = pair_count.entry((e.from, e.to)).or_insert(0);
            *c += 1;
            *c - 1
        } as f32;
        let color = crate::scar_panel::type_color(&e.action.ty);
        let chance = odds.get(&k).copied();
        let dim = chance.is_some_and(|c| c <= 0.0);
        let col = if dim { color.gamma_multiply(0.18) } else { color };
        let pts: [Pos2; 4] = match e.to {
            Some(to) if to == e.from => {
                let s = a.right_top() + vec2(-20.0 * z, 0.0);
                let h = (34.0 + slot * 14.0) * z;
                [s, s + vec2(10.0 * z, -h), s + vec2(-40.0 * z, -h), a.center_top() + vec2(-10.0 * z, 0.0)]
            }
            Some(to) => {
                let b = node_rect(to);
                let s = a.right_center() + vec2(0.0, (slot - 1.0) * 6.0 * z);
                let t = b.left_center() + vec2(0.0, (slot - 1.0) * 6.0 * z);
                if b.center().x > a.center().x + 10.0 {
                    let dx = ((t.x - s.x) * 0.45).max(40.0 * z);
                    [s, s + vec2(dx, 0.0), t - vec2(dx, 0.0), t]
                } else {
                    // Back edge: around below the boxes.
                    let s = a.center_bottom() + vec2(slot * 8.0 * z, 0.0);
                    let t = b.center_bottom() + vec2(slot * 8.0 * z, 0.0);
                    let dip = (60.0 + slot * 18.0) * z;
                    [s, s + vec2(0.0, dip), t + vec2(0.0, dip), t]
                }
            }
            None => {
                let row = {
                    let r = dangling_row.entry(e.from).or_insert(0);
                    *r += 1;
                    *r - 1
                } as f32;
                let s = a.right_center();
                let t = s + vec2(70.0 * z, (row - 0.5) * 16.0 * z + 30.0 * z);
                [s, s + vec2(30.0 * z, 0.0), t - vec2(20.0 * z, 0.0), t]
            }
        };
        let samples: Vec<Pos2> = (0..=24).map(|i| bezier(pts, i as f32 / 24.0)).collect();
        if hovered_edge.is_none() && st.drag_node.is_none() {
            if let Some(q) = pointer {
                if samples.windows(2).any(|w| dist_to_segment(q, w[0], w[1]) < 6.0) {
                    hovered_edge = Some(k);
                }
            }
        }
        let sel = st.selected_edge == Some(k) || hovered_edge == Some(k);
        let taken = last_edge == Some(k) || on_path(e);
        let width: f32 = if sel || taken { 3.0 } else { 1.6 };
        p.add(Shape::line(samples.clone(), Stroke::new(width, if taken { Color32::from_rgb(255, 230, 90) } else { col })));
        // Arrow head.
        let tip = pts[3];
        let dir = (tip - bezier(pts, 0.92)).normalized();
        let n = vec2(-dir.y, dir.x);
        let sz = 7.0 * z.max(0.6);
        p.add(Shape::convex_polygon(vec![tip, tip - dir * sz + n * sz * 0.5, tip - dir * sz - n * sz * 0.5], col, Stroke::NONE));
        // Label.
        let base_t = if level.is_some() { 0.68 } else { 0.5 };
        let mid = bezier(pts, (base_t + (slot - 1.0) * 0.1).clamp(0.2, 0.85));
        let text = match chance {
            Some(c) => format!("{} · {:.0}%", e.action.ty, c * 100.0),
            None => edge_label(e),
        };
        if (st.labels && !dim) || sel {
            let galley = p.layout_no_wrap(text, font.clone(), Color32::WHITE);
            let r = Rect::from_center_size(mid, galley.size() + vec2(6.0, 2.0));
            p.rect_filled(r, 3.0, Color32::from_rgba_unmultiplied(20, 20, 24, 220));
            p.galley(r.min + vec2(3.0, 1.0), galley, Color32::WHITE);
        }
        if e.to.is_none() {
            p.text(tip + vec2(4.0, 0.0), Align2::LEFT_CENTER, &e.target_label, font.clone(), Color32::from_gray(150));
        }
    }

    // Nodes.
    let mut hovered_node = None;
    for (i, n) in g.nodes.iter().enumerate() {
        if !visible_node(i) {
            continue;
        }
        let r = node_rect(i);
        if pointer.is_some_and(|q| r.contains(q)) {
            hovered_node = Some(i);
        }
        let is_start = g.starts.contains(&i);
        let open = combo::is_open(app, g, i);
        let fill = if current == Some(i) { Color32::from_rgb(90, 80, 30) } else { Color32::from_rgb(44, 46, 56) };
        p.rect_filled(r, 5.0, fill);
        let border = if open {
            Color32::from_rgb(120, 200, 255)
        } else if is_start {
            Color32::from_rgb(110, 200, 110)
        } else {
            Color32::from_gray(110)
        };
        p.rect_stroke(r, 5.0, Stroke::new(if hovered_node == Some(i) || focus == Some(i) { 2.5_f32 } else { 1.5 }, border), egui::StrokeKind::Inside);
        let clip = p.with_clip_rect(r.shrink(2.0));
        clip.text(r.left_top() + vec2(6.0, 4.0) * z, Align2::LEFT_TOP, short(&n.name), FontId::proportional(13.0 * z.max(0.7)), Color32::WHITE);
        let leaf = n.actions.is_empty();
        let sub = format!(
            "{}{}",
            sub_label(ctx, n.sub),
            if leaf { sds_i18n::t!(" · конец цепочки", " · chain end") } else { "" }
        );
        clip.text(r.left_bottom() + vec2(6.0, -4.0) * z, Align2::LEFT_BOTTOM, sub, FontId::proportional(10.5 * z.max(0.7)), Color32::from_gray(170));
    }

    // ---- interaction ----
    if resp.drag_started() {
        st.drag_node = hovered_node;
    }
    if resp.dragged() {
        match st.drag_node {
            Some(i) => {
                let d = resp.drag_delta() / st.zoom;
                let map = if level.is_some() { &mut st.focus_pos } else { &mut st.pos };
                *map.entry(i).or_insert(Pos2::ZERO) += d;
            }
            None => st.pan += resp.drag_delta(),
        }
    }
    if resp.drag_stopped() {
        st.drag_node = None;
    }
    if let Some(k) = hovered_edge {
        let e = &g.edges[k];
        let a = &e.action;
        let mut tip = format!(
            "{} → {}\n{} {}\n{}: {:.0}–{:.0}\n{}: {:+.0}°..{:+.0}°\nChance {:.0}%{}\n{} {:.3} s",
            short(&g.nodes[e.from].name),
            e.to.map(|t| short(&g.nodes[t].name).to_string()).unwrap_or_else(|| e.target_label.clone()),
            a.ty,
            a.idle,
            sds_i18n::t!("дистанция", "distance"),
            a.min_distance,
            a.max_distance,
            sds_i18n::t!("угол", "angle"),
            a.start_angle,
            a.end_angle,
            a.chance,
            if e.variant != 0 { format!(" · SCAR_AttackVariants = {}", e.variant) } else { String::new() },
            sds_i18n::t!("аннотация в", "annotation at"),
            e.time,
        );
        if let Some(c) = odds.get(&k) {
            tip += &sds_i18n::tf!("\nдля цели {:.0} / {:+.0}°: {:.0}%", "\nfor target {:.0} / {:+.0}°: {:.0}%", st.dist, st.angle, c * 100.0);
        }
        tip += sds_i18n::t!("\nклик — открыть клип и выделить ActionData", "\nclick — open the clip and select the ActionData");
        resp.clone().on_hover_text_at_pointer(tip);
    } else if let Some(i) = hovered_node {
        let n = &g.nodes[i];
        resp.clone().on_hover_text_at_pointer(sds_i18n::tf!(
            "{}\n{}\nдлительность {:.2} с · ActionData {} · ComboStart {}\nMCO_nextattack {} · MCO_nextpowerattack {}\nклик — открыть клип, тяните — переместить",
            "{}\n{}\nduration {:.2} s · ActionData {} · ComboStart {}\nMCO_nextattack {} · MCO_nextpowerattack {}\nclick — open the clip, drag — move",
            n.name,
            n.place.label(),
            n.duration,
            n.actions.len(),
            n.combo_starts.len(),
            n.next_attack.map(|v| v.to_string()).unwrap_or("—".into()),
            n.next_power.map(|v| v.to_string()).unwrap_or("—".into()),
        ));
    }
    if resp.clicked() {
        if let Some(k) = hovered_edge {
            st.selected_edge = Some(k);
            let e = &g.edges[k];
            let text = scar_text(app, g, e);
            open_place(app, &g.nodes[e.from].place.clone(), Some((text, e.time)));
        } else if let Some(i) = hovered_node {
            walk_to(st, focus, i, level.is_some());
            open_place(app, &g.nodes[i].place.clone(), None);
        } else {
            st.selected_edge = None;
        }
    }

    // Legend.
    p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - 20.0), rect.right_bottom()), 0.0, Color32::from_rgba_unmultiplied(24, 24, 28, 235));
    let mut x = rect.left() + 8.0;
    let y = rect.bottom() - 10.0;
    for ty in ["RA", "RPA", "LA", "LPA", "BA", "DA", "IDLE"] {
        p.circle_filled(pos2(x, y), 4.0, crate::scar_panel::type_color(ty));
        x += 8.0 + p.text(pos2(x + 7.0, y), Align2::LEFT_CENTER, ty, FontId::proportional(10.5), Color32::from_gray(180)).width() + 10.0;
    }
    p.text(
        pos2(x + 6.0, y),
        Align2::LEFT_CENTER,
        sds_i18n::t!("зелёная рамка — старт (dummy), голубая — открытый клип, жёлтый — текущий шаг симулятора", "green frame — start (dummy), blue — open clip, yellow — current simulator step"),
        FontId::proportional(10.5),
        Color32::from_gray(150),
    );
}

/// Moves the focus to `to`. In the focused view the old focus joins the
/// path (a click on a path clip goes back to it); elsewhere the path resets.
fn walk_to(st: &mut ComboState, focus: Option<usize>, to: usize, focused_view: bool) {
    if !focused_view {
        st.trail.clear();
    } else if let Some(k) = st.trail.iter().position(|&n| n == to) {
        st.trail.truncate(k);
    } else if let Some(f) = focus.filter(|&f| f != to) {
        st.trail.push(f);
        if st.trail.len() > 12 {
            st.trail.remove(0);
        }
    }
    st.focus = Some(to);
}

/// The annotation text of an edge's ActionData, to select it after opening.
fn scar_text(_app: &StudioApp, _g: &Graph, e: &combo::Edge) -> String {
    format!("\"IdleAnimation\":\"{}\"", e.action.idle)
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

// ---- simulator ---------------------------------------------------------

fn simulator(ui: &mut egui::Ui, app: &mut StudioApp, st: &mut ComboState, g: &Graph, ctx: Option<&combo::Context>) {
    ui.heading(sds_i18n::t!("Симулятор комбо", "Combo simulator"));
    ui.checkbox(&mut st.sim.auto, sds_i18n::t!("бросать шанс (иначе выбор вручную)", "roll Chance (else pick by hand)"));
    ui.checkbox(&mut st.sim.use_path, sds_i18n::t!("цель по пути NPC в клипе", "target from the NPC path in the clip"))
        .on_hover_text(sds_i18n::t!(
            "Вкл: клип открывается в студии, дистанция и угол до цели берутся на SCAR_ComboStart с учётом root motion и SDSCombat, после перехода цель переносится в следующий клип. Выкл: цель неподвижна относительно NPC (значения «Цель» сверху).",
            "On: the clip opens in the studio, distance and angle to the target are taken at SCAR_ComboStart with root motion and SDSCombat, and the target carries over into the next clip. Off: the target stays fixed relative to the NPC (the «Target» values above)."
        ));
    ui.horizontal(|ui| {
        if ui.button(sds_i18n::t!("▶ Старт", "▶ Start")).on_hover_text(sds_i18n::t!("С первого dummy (или открытого клипа, если он в графе)", "From the first dummy (or the open clip if it is in the graph)")).clicked() {
            let open = (0..g.nodes.len()).find(|&i| combo::is_open(app, g, i));
            st.sim.current = open.or(g.starts.first().copied());
            st.sim.chain.clear();
            st.sim.offer.clear();
            if let Some(n) = st.sim.current {
                if !combo::is_open(app, g, n) {
                    open_place(app, &g.nodes[n].place.clone(), None);
                }
            }
        }
        let can = st.sim.current.is_some() && st.sim.offer.is_empty();
        if ui.add_enabled(can, egui::Button::new(sds_i18n::t!("Шаг ⏵", "Step ⏵"))).clicked() {
            st.step_pending = true;
        }
        if ui.add_enabled(st.sim.current.is_some() && !st.sim.use_path && st.sim.auto, egui::Button::new(sds_i18n::t!("×20", "×20")))
            .on_hover_text(sds_i18n::t!("20 шагов без открытия клипов (неподвижная цель)", "20 steps without opening clips (fixed target)"))
            .clicked()
        {
            for _ in 0..20 {
                decide(app, st, g, false);
            }
        }
        if ui.button(sds_i18n::t!("Сброс", "Reset")).clicked() {
            st.sim.current = None;
            st.sim.chain.clear();
            st.sim.offer.clear();
            st.step_pending = false;
        }
    });

    if st.auto_steps > 0 && !st.step_pending && st.sim.offer.is_empty() {
        if st.sim.current.is_none() {
            st.sim.current = g.starts.first().copied();
        }
        st.auto_steps -= 1;
        st.step_pending = true;
    }
    // A pending step runs once its clip is open (path mode).
    if st.step_pending {
        if let Some(n) = st.sim.current {
            if !st.sim.use_path || combo::is_open(app, g, n) {
                st.step_pending = false;
                decide(app, st, g, true);
            } else if st.sim.waiting != Some(n) {
                st.sim.waiting = Some(n);
                open_place(app, &g.nodes[n].place.clone(), None);
            }
        }
    }

    // Manual pick.
    if !st.sim.offer.is_empty() {
        let (at, d, a) = st.sim.offer_at;
        ui.separator();
        ui.label(sds_i18n::tf!("Выбор на {:.2} с, цель {:.0} / {:+.0}°:", "Pick at {:.2} s, target {:.0} / {:+.0}°:", at, d, a));
        let offer = st.sim.offer.clone();
        for (k, odds) in offer {
            let e = &g.edges[k];
            let to = e.to.map(|t| short(&g.nodes[t].name).to_string()).unwrap_or_else(|| e.target_label.clone());
            if ui.button(format!("{} → {to} ({:.0}%)", edge_label(e), odds * 100.0)).clicked() {
                st.sim.offer.clear();
                transition(app, st, g, Some(k), at, d, a, String::new());
            }
        }
        if ui.button(sds_i18n::t!("ничего (конец цепочки)", "nothing (chain ends)")).clicked() {
            st.sim.offer.clear();
            transition(app, st, g, None, at, d, a, sds_i18n::t!("выбрано вручную: без продолжения", "picked by hand: no follow-up").into());
        }
    }

    ui.separator();
    if let Some(n) = st.sim.current {
        ui.label(sds_i18n::tf!("Сейчас: {}", "Now: {}", short(&g.nodes[n].name)));
        ui.small(sub_label(ctx, g.nodes[n].sub));
    }
    for (k, s) in st.sim.chain.iter().enumerate() {
        let from = short(&g.nodes[s.node].name);
        let line = match s.edge {
            Some(e) => {
                let e = &g.edges[e];
                let to = e.to.map(|t| short(&g.nodes[t].name).to_string()).unwrap_or_else(|| e.target_label.clone());
                format!("{}. {from} @{:.2}s · {:.0}/{:+.0}° → {} {}{} → {to}", k + 1, s.at, s.dist, s.angle, e.action.ty, e.action.chance, if e.variant != 0 { format!("% V{}", e.variant) } else { "%".into() })
            }
            None => format!("{}. {from} · {}", k + 1, s.note),
        };
        ui.small(line);
    }
    if st.sim.chain.len() > 1 {
        // How often each clip came up.
        let mut count: HashMap<usize, usize> = HashMap::new();
        for s in &st.sim.chain {
            *count.entry(s.node).or_insert(0) += 1;
        }
        let mut v: Vec<_> = count.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        ui.separator();
        ui.small(sds_i18n::t!("Частота клипов в цепочке:", "Clip frequency in the chain:"));
        for (n, c) in v {
            ui.small(format!("{} × {}", short(&g.nodes[n].name), c));
        }
    }
}

/// One decision at the current node.
fn decide(app: &mut StudioApp, st: &mut ComboState, g: &Graph, interactive: bool) {
    let Some(node) = st.sim.current else { return };
    let n = &g.nodes[node];
    if n.actions.is_empty() {
        let end = n.duration;
        let (d, a) = if st.sim.use_path && interactive {
            let (_, _, d, a) = crate::scar_panel::target_local(app, end);
            (d, a)
        } else {
            (st.dist, st.angle)
        };
        transition(app, st, g, None, end, d, a, sds_i18n::t!("нет ActionData — возврат в бой", "no ActionData — back to combat idle").into());
        return;
    }
    let times: Vec<f32> = if n.combo_starts.is_empty() { vec![0.0] } else { n.combo_starts.clone() };
    for &t in &times {
        let (d, a) = if st.sim.use_path && interactive {
            let (_, _, d, a) = crate::scar_panel::target_local(app, t);
            (d, a)
        } else {
            (st.dist, st.angle)
        };
        if st.sim.auto {
            if let Some(k) = st.sim.pick(g, node, d, a) {
                transition(app, st, g, Some(k), t, d, a, String::new());
                return;
            }
        } else {
            let offer: Vec<(usize, f32)> = combo::odds(g, node, d, a).into_iter().filter(|(_, o)| *o > 0.0).collect();
            if !offer.is_empty() {
                st.sim.offer = offer;
                st.sim.offer_at = (t, d, a);
                app.play.time = t;
                return;
            }
        }
    }
    let (d, a) = if st.sim.use_path && interactive {
        let (_, _, d, a) = crate::scar_panel::target_local(app, n.duration);
        (d, a)
    } else {
        (st.dist, st.angle)
    };
    let in_zone = n.actions.iter().any(|(_, x)| x.in_zone(d, a));
    let note = if in_zone {
        sds_i18n::t!("шансы не выпали — клип доигран, возврат в бой", "no Chance roll succeeded — clip played out, back to combat idle")
    } else {
        sds_i18n::t!("цель вне всех зон — клип доигран, возврат в бой", "target outside every zone — clip played out, back to combat idle")
    };
    transition(app, st, g, None, n.duration, d, a, note.into());
}

#[allow(clippy::too_many_arguments)]
fn transition(app: &mut StudioApp, st: &mut ComboState, g: &Graph, edge: Option<usize>, at: f32, d: f32, a: f32, note: String) {
    let Some(node) = st.sim.current else { return };
    st.sim.chain.push(Step { node, at, dist: d, angle: a, edge, note });
    let next = match edge {
        Some(k) => g.edges[k].to,
        // Back to the dummy: the AI attacks again from combat idle.
        None => g.starts.first().copied(),
    };
    let Some(next) = next else {
        st.sim.current = None;
        return;
    };
    st.sim.current = Some(next);
    if st.only_next && edge.is_some() {
        // A new chain (after one ended) starts a new path; a chain end keeps
        // the walked path on screen.
        if st.focus != Some(node) {
            st.trail.clear();
            st.focus = Some(node);
        }
        walk_to(st, Some(node), next, true);
    }
    if st.sim.use_path {
        // The next clip starts where this one was cut: the target keeps its
        // place relative to the NPC.
        app.scene.target_dist = d;
        app.scene.target_angle = a;
        app.play.time = 0.0;
        if !combo::is_open(app, g, next) {
            open_place(app, &g.nodes[next].place.clone(), None);
        }
    }
}
