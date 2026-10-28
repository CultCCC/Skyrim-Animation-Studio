//! SCAR combo graph of one OAR moveset, and a step-by-step combo simulator.
//!
//! How a SCAR moveset chains, read off SDS movesets (Snow Elf King):
//! - a clip's `SCAR_ActionData` entries are the follow-ups SCAR may pick at
//!   its `SCAR_ComboStart` moments (the `scar_*` dummy idle: whenever the AI
//!   attacks);
//! - the action type picks the clip: RA → `bfco_attack{N}`, RPA →
//!   `bfco_powerattack{M}`, where the CURRENT clip set N and M through
//!   `PIE.@SGVI|MCO_nextattack|N` / `MCO_nextpowerattack|M` (1 when unset);
//! - `VariantID` lands in the graph variable `SCAR_AttackVariants`, and OAR
//!   picks the submod by it: the highest priority submod holding that clip
//!   whose conditions pass — a variant submod (`SCAR_AttackVariants == v`)
//!   only for its own v, a submod without that condition for any v.
//!
//! The moveset ("context") is one game state of an OAR mod: the condition
//! set of one of its submods (variant check aside). OAR conditions stack,
//! so every submod whose own conditions are a SUBSET of that state applies
//! there too — a phase-2 state (keyword + phase effect) also runs the
//! keyword-only variant submods, and the phase-1 main submod where phase 2
//! has no clip of its own (priority decides overlaps).
//!
//! SCAR's choice (from the SDS author): the ActionData annotation time is
//! its weight — actions are tried from the LATEST time down; each one in
//! range rolls its Chance, the first success wins.

use crate::app::StudioApp;
use crate::oar::{OarMod, Place};
use sds_anno::{parse, Kind, ScarAction};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SubVar {
    pub folder: String,
    pub name: String,
    pub priority: i64,
    pub disabled: bool,
    /// `SCAR_AttackVariants == v` condition; None = any variant.
    pub variant: Option<i64>,
    pub files: Vec<(String, Place)>,
}

#[derive(Debug, Clone)]
pub struct Context {
    pub key: String,
    /// The state's conditions (canonical JSON each), variant aside.
    pub conds: Vec<String>,
    pub mod_name: String,
    pub label: String,
    pub subs: Vec<SubVar>,
}

/// A clip as the graph needs it.
#[derive(Debug, Clone)]
pub struct ClipLite {
    pub name: String,
    pub place: Place,
    pub sub: usize,
    pub duration: f32,
    /// (annotation time, action), in annotation order.
    pub actions: Vec<(f32, ScarAction)>,
    pub combo_starts: Vec<f32>,
    pub next_attack: Option<i64>,
    pub next_power: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: usize,
    pub to: Option<usize>,
    pub action: ScarAction,
    pub time: f32,
    pub variant: i64,
    /// What the action asks for when it does not resolve to a clip.
    pub target_label: String,
}

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub nodes: Vec<ClipLite>,
    pub edges: Vec<Edge>,
    /// BFS depth of each node from the start nodes.
    pub depth: Vec<usize>,
    pub starts: Vec<usize>,
    pub notes: Vec<String>,
}

fn variant_of(cond: &Value) -> Option<i64> {
    let a = cond.get("Value A")?;
    let var = a.get("graphVariable")?.as_str()?;
    if !var.eq_ignore_ascii_case("SCAR_AttackVariants") || cond.get("condition")?.as_str()? != "CompareValues" {
        return None;
    }
    if cond.get("Comparison").and_then(|c| c.as_str()) != Some("==") || cond.get("negated").and_then(|n| n.as_bool()) == Some(true) {
        return None;
    }
    cond.get("Value B")?.get("value")?.as_f64().map(|v| v.round() as i64)
}

/// The submod's conditions without the variant check (canonical JSON,
/// sorted), and the variant.
fn split_conditions(config: &Value) -> (Vec<String>, Option<i64>) {
    let mut variant = None;
    let mut rest = Vec::new();
    for c in config.get("conditions").and_then(|c| c.as_array()).into_iter().flatten() {
        if c.get("disabled").and_then(|d| d.as_bool()) == Some(true) {
            continue;
        }
        match variant_of(c) {
            Some(v) => variant = Some(v),
            None => rest.push(serde_json::to_string(c).unwrap_or_default()),
        }
    }
    rest.sort();
    rest.dedup();
    (rest, variant)
}

/// Every moveset of every OAR mod.
pub fn contexts<'a>(mods: impl Iterator<Item = &'a OarMod>) -> Vec<Context> {
    let mut out: Vec<Context> = Vec::new();
    for m in mods {
        let subs: Vec<(Vec<String>, SubVar)> = m
            .subs
            .iter()
            .map(|s| {
                let (conds, variant) = split_conditions(&s.config);
                (conds, SubVar { folder: s.folder.clone(), name: s.name.clone(), priority: s.priority, disabled: s.disabled, variant, files: s.files.clone() })
            })
            .collect();
        // One state per distinct condition set; members: every subset.
        let mut states: Vec<Vec<String>> = Vec::new();
        for (c, _) in &subs {
            if !states.contains(c) {
                states.push(c.clone());
            }
        }
        for state in states {
            let members: Vec<SubVar> = subs.iter().filter(|(c, _)| c.iter().all(|x| state.contains(x))).map(|(_, s)| s.clone()).collect();
            let own: Vec<&str> = subs.iter().filter(|(c, s)| *c == state && s.variant.is_none()).map(|(_, s)| s.name.as_str()).collect();
            let names = if own.is_empty() { subs.iter().filter(|(c, _)| *c == state).map(|(_, s)| s.name.as_str()).collect::<Vec<_>>() } else { own };
            let vars = members.iter().filter(|s| s.variant.is_some()).count();
            let label = format!("{} — {}{}", m.name, names.join(", "), if vars > 0 { sds_i18n::tf!(" (+{} вариантов)", " (+{} variants)", vars) } else { String::new() });
            out.push(Context { key: format!("{}|{}", m.folder, state.join("&")), conds: state, mod_name: m.name.clone(), label, subs: members });
        }
    }
    out
}

/// OAR's pick for `name` with `SCAR_AttackVariants == variant`.
pub fn resolve(ctx: &Context, name: &str, variant: i64) -> Option<usize> {
    ctx.subs
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.disabled && s.variant.is_none_or(|v| v == variant))
        .filter(|(_, s)| s.files.iter().any(|(r, _)| r.eq_ignore_ascii_case(name)))
        .max_by_key(|(_, s)| s.priority)
        .map(|(i, _)| i)
}

fn sgvi(text: &str, var: &str) -> Option<i64> {
    let body = text.trim().strip_prefix("PIE.")?;
    let parts: Vec<&str> = body.split('|').map(str::trim).collect();
    (parts.len() >= 3 && parts[0].eq_ignore_ascii_case("@SGVI") && parts[1].eq_ignore_ascii_case(var)).then(|| parts[2].parse().ok()).flatten()
}

fn lite(name: &str, place: Place, sub: usize, duration: f32, annos: &[(f32, String)]) -> ClipLite {
    let mut c = ClipLite { name: name.to_string(), place, sub, duration, actions: Vec::new(), combo_starts: Vec::new(), next_attack: None, next_power: None };
    for (t, text) in annos {
        if let Some(n) = sgvi(text, "MCO_nextattack") {
            c.next_attack = Some(n);
        }
        if let Some(n) = sgvi(text, "MCO_nextpowerattack") {
            c.next_power = Some(n);
        }
        match parse(text).kind {
            Kind::ScarAction(a) => c.actions.push((*t, a)),
            _ if text.trim().eq_ignore_ascii_case("SCAR_ComboStart") => c.combo_starts.push(*t),
            _ => {}
        }
    }
    c
}

/// The clip this action plays from `from`, by the BFCO naming.
fn target_name(from: &ClipLite, a: &ScarAction) -> Result<String, String> {
    let prefix = if from.name.starts_with("mco_") { "mco_" } else { "bfco_" };
    match a.ty.to_ascii_uppercase().as_str() {
        "RA" => Ok(format!("{prefix}attack{}.hkx", from.next_attack.unwrap_or(1))),
        "RPA" => Ok(format!("{prefix}powerattack{}.hkx", from.next_power.unwrap_or(1))),
        "IDLE" => Err(format!("IDLE {}", a.idle)),
        other => Err(sds_i18n::tf!("{} ({}) — клип не сопоставлен", "{} ({}) — clip not mapped", other, a.idle)),
    }
}

impl ScarActionExt for ScarAction {
    fn variant(&self) -> i64 {
        self.extra.iter().find(|(k, _)| k.eq_ignore_ascii_case("VariantID")).and_then(|(_, v)| v.as_f64()).map(|v| v.round() as i64).unwrap_or(0)
    }
    fn in_zone(&self, dist: f32, angle: f32) -> bool {
        if dist < self.min_distance || dist > self.max_distance {
            return false;
        }
        let (s, e) = (self.start_angle, self.end_angle);
        if e - s >= 360.0 {
            return true;
        }
        let a = sds_anno::sdscombat::wrap(angle);
        let (s, e) = (sds_anno::sdscombat::wrap(s), sds_anno::sdscombat::wrap(e));
        if s <= e {
            a >= s && a <= e
        } else {
            a >= s || a <= e
        }
    }
}

pub trait ScarActionExt {
    fn variant(&self) -> i64;
    fn in_zone(&self, dist: f32, angle: f32) -> bool;
}

/// Reads every clip the moveset reaches from its `scar_*` dummies (or from
/// `start` when given), breadth first.
pub fn build(app: &StudioApp, ctx: &Context, start: Option<&str>) -> Graph {
    let mut g = Graph::default();
    let vfs = app.loaded.as_ref().map(|l| l.vfs.clone());
    let mut index: HashMap<(String, usize), usize> = HashMap::new();
    let mut read = |name: &str, sub: usize, g: &mut Graph| -> Option<usize> {
        let key = (name.to_ascii_lowercase(), sub);
        if let Some(&i) = index.get(&key) {
            return Some(i);
        }
        let place = ctx.subs[sub].files.iter().find(|(r, _)| r.eq_ignore_ascii_case(name))?.1.clone();
        // The open clip with its unsaved edits, else the saved copy, else the build.
        let open = app.doc.as_ref().filter(|d| match (&d.source, &place) {
            (crate::doc::ClipSource::Vfs(a), Place::Vfs(b)) => a.eq_ignore_ascii_case(b),
            (crate::doc::ClipSource::Disk(a), Place::Disk(b)) => a == b,
            _ => false,
        });
        let (duration, annos) = match open {
            Some(d) => (d.duration(), d.annos.iter().map(|a| (a.time, a.text.clone())).collect::<Vec<_>>()),
            None => {
                let bytes = match &place {
                    Place::Vfs(p) => app.output_copy(p).and_then(|o| std::fs::read(o).ok()).or_else(|| vfs.as_ref().and_then(|v| v.read(p).ok())),
                    Place::Disk(p) => std::fs::read(p).ok(),
                };
                match bytes.and_then(|b| sds_hkx::read_clip(&b).ok()) {
                    Some(c) => (c.duration, c.annotations.iter().map(|a| (a.time, a.text.clone())).collect()),
                    None => {
                        g.notes.push(sds_i18n::tf!("{}: не прочитан", "{}: unreadable", place.label()));
                        return None;
                    }
                }
            }
        };
        let i = g.nodes.len();
        g.nodes.push(lite(name, place, sub, duration, &annos));
        g.depth.push(usize::MAX);
        index.insert(key, i);
        Some(i)
    };

    let mut queue = std::collections::VecDeque::new();
    let starts: Vec<(String, usize)> = match start {
        Some(name) => resolve(ctx, name, 0).map(|s| (name.to_string(), s)).into_iter().collect(),
        None => {
            let mut v: Vec<(String, usize)> = Vec::new();
            for (si, s) in ctx.subs.iter().enumerate().filter(|(_, s)| s.variant.is_none() && !s.disabled) {
                for (r, _) in &s.files {
                    if r.starts_with("scar_") && r.ends_with(".hkx") && resolve(ctx, r, 0) == Some(si) && !v.iter().any(|(n, _)| n == r) {
                        v.push((r.clone(), si));
                    }
                }
            }
            v
        }
    };
    if starts.is_empty() {
        g.notes.push(sds_i18n::t!("В наборе нет клипа scar_* (dummy) — откройте клип и постройте граф от него", "The moveset has no scar_* clip (dummy) — open a clip and build the graph from it").into());
    }
    for (name, sub) in starts {
        if let Some(i) = read(&name, sub, &mut g) {
            g.depth[i] = 0;
            g.starts.push(i);
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let from = g.nodes[i].clone();
        for (t, a) in &from.actions {
            let variant = a.variant();
            let (to, label) = match target_name(&from, a) {
                Ok(name) => match resolve(ctx, &name, variant) {
                    Some(sub) => (read(&name, sub, &mut g), name),
                    None => (None, sds_i18n::tf!("{} (нет в наборе)", "{} (not in the moveset)", name)),
                },
                Err(label) => (None, label),
            };
            if let Some(j) = to {
                if g.depth[j] == usize::MAX {
                    g.depth[j] = g.depth[i] + 1;
                    queue.push_back(j);
                }
            }
            g.edges.push(Edge { from: i, to, action: a.clone(), time: *t, variant, target_label: label });
        }
    }
    g
}

/// `from`'s edges in the order SCAR tries them: the annotation time is the
/// weight, latest first (ties keep annotation order).
pub fn scar_order(g: &Graph, from: usize) -> Vec<usize> {
    let mut v: Vec<usize> = g.edges.iter().enumerate().filter(|(_, e)| e.from == from).map(|(k, _)| k).collect();
    v.sort_by(|a, b| g.edges[*b].time.total_cmp(&g.edges[*a].time));
    v
}

/// Chance of each of `from`'s edges for a target at (dist, angle):
/// latest annotation time first, each in-range action rolls; the first success wins.
pub fn odds(g: &Graph, from: usize, dist: f32, angle: f32) -> Vec<(usize, f32)> {
    let mut left = 1.0f32;
    let mut out = Vec::new();
    for k in scar_order(g, from) {
        let e = &g.edges[k];
        if !e.action.in_zone(dist, angle) {
            out.push((k, 0.0));
            continue;
        }
        let c = (e.action.chance / 100.0).clamp(0.0, 1.0);
        out.push((k, left * c));
        left *= 1.0 - c;
    }
    out
}

// ---- simulator ------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Step {
    pub node: usize,
    /// Decision time in the clip, target distance and angle then.
    pub at: f32,
    pub dist: f32,
    pub angle: f32,
    /// The edge taken, or None: the chain ended here.
    pub edge: Option<usize>,
    pub note: String,
}

#[derive(Debug, Clone, Default)]
pub struct Sim {
    pub current: Option<usize>,
    pub chain: Vec<Step>,
    /// Roll the dice (else the user picks among in-range actions).
    pub auto: bool,
    /// Target from the NPC path in the open clip (else the fixed filter values).
    pub use_path: bool,
    pub rng: u32,
    /// Waiting for the studio to open this node's clip.
    pub waiting: Option<usize>,
    /// Candidates offered for a manual pick: (edge, odds).
    pub offer: Vec<(usize, f32)>,
    pub offer_at: (f32, f32, f32),
}

impl Sim {
    pub fn roll(&mut self) -> f32 {
        let mut x = if self.rng == 0 { 0x2545_F491 } else { self.rng };
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// The edge SCAR would take: per in-range action in order, a Chance roll.
    pub fn pick(&mut self, g: &Graph, from: usize, dist: f32, angle: f32) -> Option<usize> {
        for k in scar_order(g, from) {
            let e = &g.edges[k];
            if e.action.in_zone(dist, angle) && self.roll() * 100.0 < e.action.chance {
                return Some(k);
            }
        }
        None
    }
}

/// Is `node`'s clip the one open in the studio?
pub fn is_open(app: &StudioApp, g: &Graph, node: usize) -> bool {
    let Some(d) = &app.doc else { return false };
    match (&d.source, &g.nodes[node].place) {
        (crate::doc::ClipSource::Vfs(a), Place::Vfs(b)) => a.eq_ignore_ascii_case(b),
        (crate::doc::ClipSource::Disk(a), Place::Disk(b)) => a == b,
        _ => false,
    }
}
