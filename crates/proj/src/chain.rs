//! Projectile chains: each trigger unfolded left to right — what it catches,
//! what it turns the projectile into, what that spawns, and which triggers
//! the spawned projectiles run into again. Shared instances are repeated
//! where they are used; a step that leads back into its own chain is cut
//! and flagged as a loop.

use crate::model::*;
use crate::world::World;
use crate::Sel;
use sds_i18n::{t, tf};
use sds_records::FormKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Trigger,
    Func,
    Homing,
    Emitter,
    Follower,
    Multicast,
    McItem,
    Projectile,
    Loop,
    Missing,
    Note,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: Kind,
    pub sel: Sel,
    pub title: String,
    pub lines: Vec<String>,
    /// (edge label, child).
    pub children: Vec<(String, Node)>,
    /// Marks the node (loop, broken link, cancelled origin).
    pub alert: bool,
    /// Dimmed: a step that does not happen (callTriggers off).
    pub dim: bool,
}

impl Node {
    fn new(kind: Kind, sel: Sel, title: impl Into<String>) -> Node {
        Node { kind, sel, title: title.into(), lines: Vec::new(), children: Vec::new(), alert: false, dim: false }
    }
    fn line(mut self, l: impl Into<String>) -> Node {
        self.lines.push(l.into());
        self
    }
}

pub struct Ctx<'a> {
    pub c: &'a Config,
    pub w: Option<&'a World>,
}

impl Ctx<'_> {
    fn form_short(&self, v: &str) -> String {
        if v == "Current" {
            return t!("текущий", "current").into();
        }
        let r = self.c.resolve_form(v);
        match self.w {
            Some(w) if !r.starts_with("key_") => {
                let s = w.short(r);
                if s == r {
                    v.to_string()
                } else {
                    s
                }
            }
            _ => v.to_string(),
        }
    }

    fn form_key(&self, v: &str) -> Option<FormKey> {
        let r = self.c.resolve_form(v);
        self.w?.key_of(r)
    }

    /// The projectile a trigger is about, when one condition pins it.
    pub fn trigger_projectile(&self, t: &Trigger) -> Option<FormKey> {
        let w = self.w?;
        for c in &t.conditions {
            if c.invert {
                continue;
            }
            match c.kind.as_str() {
                "ProjBaseIsFormID" => return self.form_key(&c.value),
                "SpellIsFormID" => return self.form_key(&c.value).and_then(|s| w.spell_projectile(s)),
                _ => {}
            }
        }
        None
    }

    /// Triggers of this file that catch projectile `p` when it appears.
    pub fn catching(&self, p: FormKey) -> Vec<usize> {
        let w = self.w;
        self.c
            .triggers
            .iter()
            .enumerate()
            .filter(|(_, t)| t.event == "ProjAppeared")
            .filter(|(_, t)| {
                t.conditions.iter().any(|c| {
                    !c.invert
                        && match c.kind.as_str() {
                            "ProjBaseIsFormID" => self.form_key(&c.value) == Some(p),
                            "SpellIsFormID" => w.is_some_and(|w| self.form_key(&c.value).and_then(|s| w.spell_projectile(s)) == Some(p)),
                            _ => false,
                        }
                })
            })
            .map(|(i, _)| i)
            .collect()
    }
}

pub fn event_label(e: &str) -> &'static str {
    match e {
        "ProjAppeared" => t!("Появился снаряд", "Projectile appeared"),
        "Swing" => t!("Взмах оружием", "Weapon swing"),
        "HitMelee" => t!("Актёр ударил (ближний бой)", "Actor hit someone (melee)"),
        "HitByMelee" => t!("Актёра ударили (ближний бой)", "Actor was hit (melee)"),
        "HitProjectile" => t!("Актёр попал снарядом", "Actor hit with a projectile"),
        "HitByProjectile" => t!("В актёра попал снаряд", "Actor was hit by a projectile"),
        "Cast" => t!("Актёр кастует", "Actor casts"),
        "EffectStart" => t!("На актёра наложен эффект", "Effect applied to actor"),
        "EffectEnd" => t!("С актёра снят эффект", "Effect removed from actor"),
        "ProjDestroyed" => t!("Снаряд уничтожен", "Projectile destroyed"),
        "ProjImpact" => t!("Снаряд во что-то попал", "Projectile impact"),
        _ => "?",
    }
}

pub fn cond_label(k: &str) -> &'static str {
    match k {
        "ProjBaseIsFormID" => t!("снаряд =", "projectile ="),
        "SpellIsFormID" => t!("заклинание =", "spell ="),
        "SpellHasKwd" => t!("у заклинания ключевое слово", "spell has keyword"),
        "EffectsIsFormID" => t!("в заклинании есть эффект", "spell has effect"),
        "EffectsHasKwd" => t!("у эффекта заклинания ключевое слово", "a spell effect has keyword"),
        "EffectIsFormID" => t!("эффект события =", "event effect ="),
        "EffectHasKwd" => t!("у эффекта события ключевое слово", "event effect has keyword"),
        "CasterIsFormID" => t!("кастер =", "caster ="),
        "CasterBaseIsFormID" => t!("база кастера =", "caster base ="),
        "CasterHasKwd" => t!("у кастера ключевое слово", "caster has keyword"),
        "WeaponBaseIsFormID" => t!("оружие =", "weapon ="),
        "WeaponHasKwd" => t!("у оружия ключевое слово", "weapon has keyword"),
        "Hand" => t!("рука", "hand"),
        _ => "?",
    }
}

pub fn func_label(k: &str) -> &'static str {
    match k {
        "SetHoming" => t!("Самонаведение", "Homing"),
        "SetRotationHoming" => t!("Повернуть к цели", "Turn to target"),
        "SetRotationToSight" => t!("Повернуть по взгляду", "Turn to sight"),
        "SetEmitter" => t!("Эмиттер", "Emitter"),
        "SetFollower" => t!("Спутник (follower)", "Follower"),
        "ApplyMultiCast" => t!("Мультикаст", "Multicast"),
        "ChangeSpeed" => t!("Скорость", "Speed"),
        "ChangeRange" => t!("Дальность", "Range"),
        "DisableHoming" => t!("Выкл. самонаведение", "Disable homing"),
        "DisableFollower" => t!("Выкл. спутника", "Disable follower"),
        "DisableEmitter" => t!("Выкл. эмиттер", "Disable emitter"),
        "Placeatme" => t!("Заспавнить объект", "Place object"),
        "SendAnimEvent" => t!("Аним-событие", "Anim event"),
        "Explode" => t!("Взрыв", "Explode"),
        "SetColLayer" => t!("Слой коллизии", "Collision layer"),
        _ => "?",
    }
}

pub fn shape_summary(p: &Pattern) -> String {
    let f = &p.figure;
    let mut s = f.shape.clone();
    if f.shape != "Single" {
        s += &format!(" ×{}", f.count.unwrap_or(1));
        if let Some(sz) = f.size {
            s += &format!(", {sz}");
        }
    }
    if let Some(o) = &p.origin {
        s += &format!(" @ {o}");
    }
    s
}

pub fn trigger_conditions(cx: &Ctx, t: &Trigger) -> Vec<String> {
    t.conditions
        .iter()
        .map(|c| {
            let v = if c.kind == "Hand" { c.value.clone() } else { cx.form_short(&c.value) };
            let not = if c.invert { t!("НЕ ", "NOT ") } else { "" };
            let or = if c.or { t!("  ИЛИ", "  OR") } else { "" };
            format!("{not}{} {v}{or}", cond_label(&c.kind))
        })
        .collect()
}

struct B<'a, 'b> {
    cx: &'a Ctx<'b>,
    /// Triggers on the current path (loop cut).
    stack: Vec<usize>,
    depth: usize,
    /// (trigger, a trigger it leads back into).
    pub loops: Vec<(usize, usize)>,
}

const MAX_DEPTH: usize = 7;

impl B<'_, '_> {
    fn trigger(&mut self, i: usize) -> Node {
        let t = &self.cx.c.triggers[i];
        let mut n = Node::new(Kind::Trigger, Sel::Trigger(i), format!("#{} {}", i + 1, event_label(&t.event)));
        for l in trigger_conditions(self.cx, t) {
            n = n.line(l);
        }
        if t.conditions.is_empty() {
            n = n.line(t!("(без условий)", "(no conditions)"));
        }
        if t.tf.disable_origin == Some(true) {
            n = n.line(t!("✖ исходный снаряд отменён", "✖ origin projectile cancelled"));
        }
        if let (Some(w), Some(p)) = (self.cx.w, self.cx.trigger_projectile(t)) {
            if let Some(pi) = w.proj_info(p) {
                n = n.line(tf!("{} · {:.0} ед/с · {:.0} ед", "{} · {:.0} u/s · {:.0} u", pi.kind, pi.speed, pi.range));
            }
        }
        self.stack.push(i);
        self.depth += 1;
        let at = Sel::Trigger(i);
        n.children = self.funcs(&t.tf, &at);
        self.depth -= 1;
        self.stack.pop();
        n
    }

    fn funcs(&mut self, tf: &TriggerFunctions, at: &Sel) -> Vec<(String, Node)> {
        let mut out = Vec::new();
        for (fi, f) in tf.functions.iter().enumerate() {
            let label = if f.on_followers == Some(true) { tf!("{} (на спутниках)", "{} (on followers)", func_label(&f.kind)) } else { func_label(&f.kind).to_string() };
            out.push((label, self.func(f, at, fi)));
        }
        out
    }

    fn func(&mut self, f: &Func, at: &Sel, fi: usize) -> Node {
        let c = self.cx.c;
        let fsel = Sel::Func(Box::new(at.clone()), fi);
        if self.depth > MAX_DEPTH {
            return Node::new(Kind::Note, fsel, "…");
        }
        let missing = |key: &str| {
            let mut n = Node::new(Kind::Missing, fsel.clone(), if key.is_empty() { t!("ключ не выбран", "no key chosen").to_string() } else { tf!("{key} — нет такого", "{key} — not defined") });
            n.alert = true;
            n
        };
        match f.kind.as_str() {
            "SetHoming" | "SetRotationHoming" => match c.homing.iter().find(|(k, _)| *k == f.id) {
                Some((k, h)) => {
                    let mut n = Node::new(Kind::Homing, Sel::Homing(k.clone()), k.clone()).line(match h.kind.as_str() {
                        "ConstSpeed" => tf!("постоянная скорость, 180° за {} с", "const speed, 180° in {} s", h.rotation_time.unwrap_or(0.0)),
                        "ConstAccel" => tf!("постоянное ускорение {}", "const accel {}", h.acceleration.unwrap_or(0.0)),
                        o => o.to_string(),
                    });
                    let tgt = h.target.as_deref().unwrap_or("Nearest");
                    n = n.line(if tgt == "Cursor" { tf!("цель у прицела (конус {}°)", "target near cursor ({}° cone)", h.cursor_angle.unwrap_or(0.0)) } else { t!("ближайшая цель", "nearest target").to_string() });
                    n = n.line(tf!("кого: {}{}", "whom: {}{}", h.aggressive.as_deref().unwrap_or("Hostile"), if h.check_los == Some(true) { t!(", в поле зрения", ", in sight") } else { "" }));
                    n
                }
                None => missing(&f.id),
            },
            "SetEmitter" => match c.emitters.iter().find(|(k, _)| *k == f.id) {
                Some((k, e)) => {
                    let mut n = Node::new(Kind::Emitter, Sel::Emitter(k.clone()), k.clone());
                    let times = if e.limited == Some(true) { tf!(", {} раз", ", {} times", e.count.unwrap_or(1)) } else { t!(", без ограничения", ", unlimited").to_string() };
                    n = n.line(tf!("каждые {} с{times}", "every {} s{times}", e.interval));
                    if e.destroy_after == Some(true) {
                        n = n.line(t!("затем снаряд уничтожается", "then the projectile is destroyed"));
                    }
                    let esel = Sel::Emitter(k.clone());
                    self.depth += 1;
                    for (ei, ef) in e.functions.iter().enumerate() {
                        if ef.kind == "AccelerateToMaxSpeed" {
                            n.children.push((
                                t!("разгон", "accelerate").into(),
                                Node::new(Kind::Func, Sel::EmFunc(k.clone(), ei), t!("Разгон до макс. скорости", "Accelerate to max speed")).line(format!("{} s, {}", ef.time, ef.speed_type)),
                            ));
                        } else {
                            let sub = Sel::EmFunc(k.clone(), ei);
                            let _ = &esel;
                            for (l, ch) in self.funcs(&ef.tf, &sub) {
                                n.children.push((l, ch));
                            }
                        }
                    }
                    self.depth -= 1;
                    n
                }
                None => missing(&f.id),
            },
            "SetFollower" => match c.followers.iter().find(|(k, _)| *k == f.id) {
                Some((k, fo)) => {
                    let mut n = Node::new(Kind::Follower, Sel::Follower(k.clone()), k.clone()).line(shape_summary(&fo.pattern));
                    if let Some(r) = fo.rounding.as_deref().filter(|r| *r != "None") {
                        n = n.line(tf!("плавает: {r}, R {}", "floats: {r}, R {}", fo.rounding_r.unwrap_or(0.0)));
                    }
                    n = n.line(tf!("коллизия {}, скорость ×{}", "collision {}, speed ×{}", fo.collision.as_deref().unwrap_or("Actor"), fo.speed.unwrap_or(1.0)));
                    n
                }
                None => missing(&f.id),
            },
            "ApplyMultiCast" => match c.multicast.iter().find(|(k, _)| *k == f.id) {
                Some((k, items)) => {
                    let mut n = Node::new(Kind::Multicast, Sel::Multicast(k.clone()), k.clone()).line(tf!("{} групп(ы)", "{} group(s)", items.len()));
                    self.depth += 1;
                    for (ii, it) in items.iter().enumerate() {
                        let child = self.mc_item(k, ii, it);
                        n.children.push((String::new(), child));
                    }
                    self.depth -= 1;
                    n
                }
                None => missing(&f.id),
            },
            _ => {
                let mut n = Node::new(Kind::Func, fsel, func_label(&f.kind));
                match func_arg(&f.kind) {
                    FuncArg::Num => n = n.line(format!("{} {}", f.num.op, f.num.value)),
                    FuncArg::RestoreSpeed => n = n.line(if f.restore_speed == Some(false) { t!("скорость не восстанавливать", "keep current speed") } else { t!("вернуть обычную скорость", "restore default speed") }),
                    FuncArg::Form | FuncArg::Explosion => n = n.line(self.cx.form_short(&f.arg)),
                    FuncArg::AnimEvent | FuncArg::Layer => n = n.line(f.arg.clone()),
                    _ => {}
                }
                n
            }
        }
    }

    fn mc_item(&mut self, key: &str, ii: usize, it: &McItem) -> Node {
        let c = self.cx.c;
        let sel = Sel::McItem(key.to_string(), ii);
        let group = c.spawn_groups.iter().find(|(k, _)| *k == it.spawn_group);
        let title = match group {
            Some((k, g)) => format!("{k}: {}", shape_summary(&g.pattern)),
            None => tf!("{} — нет такой группы", "{} — no such group", it.spawn_group),
        };
        let mut n = Node::new(Kind::McItem, sel.clone(), title);
        n.alert = group.is_none();
        if let Some((_, g)) = group {
            let rot = g.rotation.as_deref().unwrap_or("Parallel");
            n = n.line(tf!("направление: {rot}", "direction: {rot}"));
        }
        let (what, proj) = match &it.source {
            McSource::Spell(s) => (tf!("заклинание {}", "spell {}", self.cx.form_short(s)), self.spawned_projectile(s, true)),
            McSource::Arrow { weap, arrow } => (
                tf!("стрела {} из {}", "arrow {} from {}", arrow.as_deref().map(|a| self.cx.form_short(a)).unwrap_or_else(|| t!("текущая", "current").into()), self.cx.form_short(weap)),
                None,
            ),
        };
        n = n.line(what);
        let call = it.call_triggers == Some(true);
        if let Some(t) = &it.functions {
            self.depth += 1;
            for (l, ch) in self.funcs(t, &sel) {
                n.children.push((tf!("{l} → новым", "{l} → on new"), ch));
            }
            self.depth -= 1;
        }
        match proj {
            Some(p) => {
                let w = self.cx.w.unwrap();
                let mut pn = Node::new(Kind::Projectile, Sel::Record(p), w.entry(p).map(|e| e.edid.clone()).unwrap_or_else(|| w.formid(p)));
                if let Some(pi) = w.proj_info(p) {
                    pn = pn.line(tf!("{} · {:.0} ед/с", "{} · {:.0} u/s", pi.kind, pi.speed));
                }
                let catching = self.cx.catching(p);
                if call {
                    if catching.is_empty() {
                        pn = pn.line(t!("в этом файле его не ловит ни один триггер", "no trigger in this file catches it"));
                    }
                    self.depth += 1;
                    for ti in catching {
                        if self.stack.contains(&ti) {
                            let from = *self.stack.last().unwrap();
                            self.loops.push((from, ti));
                            let mut l = Node::new(Kind::Loop, Sel::Trigger(ti), tf!("↻ снова триггер #{}", "↻ back to trigger #{}", ti + 1));
                            l = l.line(t!("бесконечная цепочка!", "endless chain!"));
                            l.alert = true;
                            pn.children.push((String::new(), l));
                        } else {
                            let child = self.trigger(ti);
                            pn.children.push((t!("триггер", "trigger").into(), child));
                        }
                    }
                    self.depth -= 1;
                } else if !catching.is_empty() {
                    pn = pn.line(tf!("callTriggers выкл. — триггеры #{} не сработают", "callTriggers off — triggers #{} will not fire", catching.iter().map(|i| (i + 1).to_string()).collect::<Vec<_>>().join(", #")));
                    pn.dim = true;
                }
                n.children.push((t!("выпускает", "fires").into(), pn));
            }
            None => {
                if call {
                    n = n.line(t!("callTriggers: вкл.", "callTriggers: on"));
                }
            }
        }
        n
    }

    /// Projectile spawned by `spellID`; `Current` keeps the chain's projectile.
    fn spawned_projectile(&self, spell: &str, current_ok: bool) -> Option<FormKey> {
        let w = self.cx.w?;
        if spell == "Current" {
            if !current_ok {
                return None;
            }
            let ti = *self.stack.last()?;
            return self.cx.trigger_projectile(&self.cx.c.triggers[ti]);
        }
        let k = self.cx.form_key(spell)?;
        w.spell_projectile(k)
    }
}

pub struct Chains {
    pub lanes: Vec<Node>,
    pub loops: Vec<(usize, usize)>,
    /// Triggers reached from another trigger's chain: (callee, caller).
    pub called_from: Vec<(usize, usize)>,
}

pub fn build(cx: &Ctx) -> Chains {
    let mut b = B { cx, stack: Vec::new(), depth: 0, loops: Vec::new() };
    let mut lanes = Vec::new();
    for i in 0..cx.c.triggers.len() {
        lanes.push(b.trigger(i));
    }
    let mut called_from = Vec::new();
    for (i, lane) in lanes.iter().enumerate() {
        collect_called(lane, i, &mut called_from, true);
    }
    called_from.sort();
    called_from.dedup();
    let mut loops = b.loops;
    loops.sort();
    loops.dedup();
    Chains { lanes, loops, called_from }
}

fn collect_called(n: &Node, root: usize, out: &mut Vec<(usize, usize)>, is_root: bool) {
    if !is_root && n.kind == Kind::Trigger {
        if let Sel::Trigger(t) = n.sel {
            out.push((t, root));
        }
    }
    for (_, c) in &n.children {
        collect_called(c, root, out, false);
    }
}
